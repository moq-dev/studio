//! The studio's side of browser sources: start the helper every page shares,
//! then deliver what it paints and plays to each page's source until the
//! source stops.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use moq_audio::bus::Channel as Sound;

use super::protocol::{Channel, Command, Event, Id, Open, Start};
use super::{ARG, Helper, Page, Paint};
use crate::Error;
use crate::source::{Mailbox, Picture, Status};
use crate::wgpu;

/// Commands waiting for the helper to read them. A helper this far behind is
/// hung, and [`Page::send`] drops new input rather than wait for it.
const BACKLOG: usize = 256;

/// Run the page `open` describes until it fails or its helper stops,
/// delivering pictures to `picture` and sound to `sound`. Dropping the future
/// closes the page.
///
/// Fills in the page's frame rate: `open` names only the page and the bus's
/// format.
pub(crate) async fn run(
	helper: &Helper,
	mut open: Open,
	page: &Page,
	picture: &Mailbox,
	sound: Option<Sound>,
	status: &kio::Producer<Status>,
) -> Result<(), Error> {
	// A later change waits for the writer, which sends it after `open`.
	open.framerate = {
		let mut state = page.state.lock().unwrap();
		state.retime = false;
		state.framerate
	};
	let instance = helper.instance()?;
	let route = Route {
		page: page.clone(),
		picture: picture.clone(),
		sound,
		status: status.clone(),
		live: false,
		end: kio::Producer::new(None),
	};
	let ended = route.end.consume();
	let opened = instance.open(open, route)?;
	let reason = ended
		.wait(|end: &kio::Ref<'_, Option<String>>| match end.as_ref() {
			Some(reason) => std::task::Poll::Ready(reason.clone()),
			None => std::task::Poll::Pending,
		})
		.await
		.unwrap_or_else(|_| "the page's reader stopped".into());
	drop(opened);
	// A stopped page leaves the canvas rather than freezing on its last
	// picture, which for an alert or a ticker would show stale news.
	*picture.lock().unwrap() = Some(Picture::Rgba {
		size: moq_video::Size::new(1, 1),
		pixels: vec![0; 4],
	});
	Err(Error::Browser(reason))
}

impl Helper {
	/// The running helper, started if there is none or it stopped.
	fn instance(&self) -> Result<Arc<Instance>, Error> {
		let mut running = self.running.lock().unwrap();
		if let Some(instance) = running.as_ref()
			&& instance.routes.lock().unwrap().ended.is_none()
		{
			return Ok(instance.clone());
		}
		// The last one, if any, has exited, which freed the profile.
		let instance = Arc::new(Instance::start(self)?);
		*running = Some(instance.clone());
		Ok(instance)
	}
}

/// The helper every page runs in.
pub(crate) struct Instance {
	/// Queued commands, which [`writer`] writes.
	commands: mpsc::SyncSender<Command>,
	routes: Arc<Mutex<Routes>>,
	/// The next page's number.
	next: AtomicU64,
	/// The socket, shut down to stop the helper.
	socket: Channel,
	/// The thread delivering the helper's events, which ends once the helper
	/// has exited.
	reader: Mutex<Option<JoinHandle<()>>>,
}

/// Where each open page's events go.
#[derive(Default)]
struct Routes {
	pages: HashMap<Id, Route>,
	/// Pages closed while the command queue was full, which [`writer`] closes
	/// as the queue makes room.
	closing: Vec<Id>,
	/// Why the helper stopped, once it has: no page opens in it after.
	ended: Option<String>,
}

/// One open page's source.
struct Route {
	page: Page,
	picture: Mailbox,
	sound: Option<Sound>,
	status: kio::Producer<Status>,
	/// Whether a picture has arrived.
	live: bool,
	/// Why the page stopped, once it has.
	end: kio::Producer<Option<String>>,
}

impl Route {
	fn end(self, reason: String) {
		if let Ok(mut end) = self.end.write() {
			*end = Some(reason);
		}
	}
}

impl Routes {
	/// Stop the page `id`, if it is still open, saying why.
	fn end(&mut self, id: Id, reason: String) {
		if let Some(route) = self.pages.remove(&id) {
			route.end(reason);
		}
	}

	/// Show `texture`, painted the way `paint` says, on page `id`.
	fn show(&mut self, id: Id, texture: wgpu::Texture, paint: Paint) {
		let Some(route) = self.pages.get_mut(&id) else { return };
		route.page.state.lock().unwrap().paint = Some(paint);
		*route.picture.lock().unwrap() = Some(Picture::Texture(texture));
		if !std::mem::replace(&mut route.live, true)
			&& let Ok(mut status) = route.status.write()
		{
			*status = Status::Live;
		}
	}

	/// Play page `id`'s `samples`.
	fn play(&mut self, id: Id, samples: &[f32]) {
		let Some(route) = self.pages.get_mut(&id) else { return };
		if let Some(sound) = &mut route.sound
			&& let Err(err) = sound.write(bytemuck::cast_slice(samples))
		{
			self.end(id, format!("the page's sound: {err}"));
		}
	}
}

impl Instance {
	/// Start a helper on `helper`'s profile.
	fn start(helper: &Helper) -> Result<Self, Error> {
		let failed = |what: &str, err: std::io::Error| Error::Browser(format!("{what}: {err}"));
		let profile =
			std::path::absolute(&helper.profile).map_err(|err| failed("cannot find the browser profile", err))?;
		// Only its owner may read the logins it keeps.
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(&profile)
			.map_err(|err| failed(&format!("cannot create the browser profile {}", profile.display()), err))?;
		let start = Start {
			accelerated: super::import::supported(helper),
			render_node: helper
				.render_node
				.as_ref()
				.and_then(|node| node.to_str())
				.map(str::to_string),
			profile: profile
				.to_str()
				.ok_or_else(|| Error::Browser(format!("the browser profile {} is not UTF-8", profile.display())))?
				.to_string(),
		};

		let (process, events) =
			spawn(helper).map_err(|err| failed(&format!("cannot start {}", helper.program.display()), err))?;
		let socket = events.try_clone().map_err(|err| failed("socket", err))?;
		let commands = socket.try_clone().map_err(|err| failed("socket", err))?;
		commands
			.send(&Command::Start(start), &[])
			.map_err(|err| failed("the browser process's socket", err))?;
		let routes = Arc::new(Mutex::new(Routes::default()));
		let commands = writer(commands, routes.clone()).map_err(|err| failed("cannot start the page writer", err))?;
		let reader = {
			let (helper, commands, routes) = (helper.clone(), commands.clone(), routes.clone());
			std::thread::Builder::new()
				.name("browser-events".into())
				.spawn(move || read(&helper, process, &events, &commands, &routes))
				.map_err(|err| failed("cannot start the page reader", err))?
		};
		Ok(Self {
			commands,
			routes,
			next: AtomicU64::new(0),
			socket,
			reader: Mutex::new(Some(reader)),
		})
	}

	/// Open a page, delivering its events to `route` until it is closed or
	/// stops. Dropping the returned guard closes it.
	fn open(self: &Arc<Self>, open: Open, route: Route) -> Result<Opened, Error> {
		let id = self.next.fetch_add(1, Ordering::Relaxed);
		let page = route.page.clone();
		{
			let mut routes = self.routes.lock().unwrap();
			if let Some(reason) = &routes.ended {
				return Err(Error::Browser(reason.clone()));
			}
			// Routed before it opens, so none of its events miss it.
			routes.pages.insert(id, route);
		}
		let opened = Opened {
			instance: self.clone(),
			page: page.clone(),
			id,
		};
		// A closed queue means the writer lost the socket, so the helper is
		// gone and the reader ends the page.
		let _ = self.commands.send(Command::Open(id, open));
		// After the page opens, since the helper delivers only to an open page.
		let mut state = page.state.lock().unwrap();
		state.id = id;
		state.commands = Some(self.commands.clone());
		state.flush();
		Ok(opened)
	}

	/// Ask the helper to exit, and wait until it has.
	pub(super) fn stop(&self) {
		self.end();
		if let Some(reader) = self.reader.lock().unwrap().take() {
			let _ = reader.join();
		}
	}

	/// Ask the helper to exit, which it does once it has closed every page.
	fn end(&self) {
		let _ = rustix::net::shutdown(&self.socket, rustix::net::Shutdown::Write);
	}
}

impl Drop for Instance {
	fn drop(&mut self) {
		self.end();
	}
}

/// An open page, closed when dropped.
struct Opened {
	instance: Arc<Instance>,
	page: Page,
	id: Id,
}

impl Drop for Opened {
	fn drop(&mut self) {
		self.page.state.lock().unwrap().disconnect();
		let mut routes = self.instance.routes.lock().unwrap();
		routes.pages.remove(&self.id);
		// Never waits on the helper: a full queue closes it as it drains.
		if let Err(mpsc::TrySendError::Full(_)) = self.instance.commands.try_send(Command::Close(self.id)) {
			routes.closing.push(self.id);
		}
	}
}

/// The helper process, killed with every CEF process it started when
/// dropped.
///
/// The helper leads its own process group, which CEF's subprocesses join, so
/// killing the group leaves none of them behind. The group is killed before
/// the helper is reaped, while its id cannot belong to anything else.
struct Process {
	child: Child,
	exit: Option<ExitStatus>,
}

impl Process {
	/// Wait for the helper to exit on its own, then stop the rest of its
	/// group, returning how it exited.
	fn exited(&mut self) -> std::io::Result<ExitStatus> {
		let pid = rustix::process::Pid::from_child(&self.child);
		let options = rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT;
		rustix::process::waitid(rustix::process::WaitId::Pid(pid), options)?;
		self.stop()
	}

	/// Kill the helper's group and reap the helper.
	fn stop(&mut self) -> std::io::Result<ExitStatus> {
		if let Some(exit) = self.exit {
			return Ok(exit);
		}
		let group = rustix::process::Pid::from_child(&self.child);
		let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
		let exit = self.child.wait()?;
		self.exit = Some(exit);
		Ok(exit)
	}
}

impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.stop();
	}
}

/// Write the commands queued on the returned sender to `channel`, on a thread
/// of its own, so a helper that stops reading blocks only that thread, never a
/// sender holding the studio's state. Each write makes room for the pages
/// `routes` closed and the releases their pages owe. The thread ends once
/// every sender is dropped or a write fails, as one does when the helper
/// exits, and ends the socket on its way out.
fn writer(channel: Channel, routes: Arc<Mutex<Routes>>) -> std::io::Result<mpsc::SyncSender<Command>> {
	let (sender, queue) = mpsc::sync_channel(BACKLOG);
	std::thread::Builder::new()
		.name("browser-commands".into())
		.spawn(move || {
			'write: for command in queue {
				if channel.send(&command, &[]).is_err() {
					break;
				}
				// Taken first, so a closing page never waits on a socket write.
				let (closing, pages): (Vec<Id>, Vec<Page>) = {
					let mut routes = routes.lock().unwrap();
					let pages = routes.pages.values().map(|route| route.page.clone()).collect();
					(std::mem::take(&mut routes.closing), pages)
				};
				for id in closing {
					if channel.send(&Command::Close(id), &[]).is_err() {
						break 'write;
					}
				}
				for page in pages {
					page.state.lock().unwrap().flush();
				}
			}
			// The helper exits once it reads the end, so one the studio can no
			// longer reach never keeps running unheard.
			let _ = rustix::net::shutdown(&channel, rustix::net::Shutdown::Write);
		})?;
	Ok(sender)
}

/// Start a helper, handing it one end of a socket as its stdin.
fn spawn(helper: &Helper) -> std::io::Result<(Process, Channel)> {
	let (ours, theirs) = UnixStream::pair()?;
	let child = std::process::Command::new(&helper.program)
		.arg(ARG)
		.stdin(OwnedFd::from(theirs))
		.stdout(Stdio::null())
		.process_group(0)
		.spawn()?;
	Ok((Process { child, exit: None }, Channel::new(ours)))
}

/// Deliver the helper's events until it exits or breaks the protocol, then
/// stop it and every page still open, saying why.
fn read(
	helper: &Helper,
	mut process: Process,
	events: &Channel,
	commands: &mpsc::SyncSender<Command>,
	routes: &Mutex<Routes>,
) {
	let reason = match deliver(helper, events, commands, routes) {
		// Gone, or exiting: wait for it, so the next helper finds the profile
		// free.
		Ok(()) => match process.exited() {
			Ok(exit) => format!("the browser process exited: {exit}"),
			Err(err) => format!("the browser process: {err}"),
		},
		Err(reason) => {
			let _ = process.stop();
			reason
		}
	};
	let mut routes = routes.lock().unwrap();
	routes.ended = Some(reason.clone());
	for (_, route) in routes.pages.drain() {
		route.end(reason.clone());
	}
}

/// Route the helper's events to their pages until it closes the socket, or
/// fail with why it cannot go on.
fn deliver(
	helper: &Helper,
	events: &Channel,
	commands: &mpsc::SyncSender<Command>,
	routes: &Mutex<Routes>,
) -> Result<(), String> {
	use std::io::ErrorKind::*;

	loop {
		let (event, fds) = match events.recv::<Event>() {
			Ok(Some(received)) => received,
			Ok(None) => return Ok(()),
			Err(err) if matches!(err.kind(), BrokenPipe | ConnectionReset | UnexpectedEof) => return Ok(()),
			Err(err) => return Err(format!("the browser process's socket: {err}")),
		};
		let (id, texture, paint) = match event {
			Event::Texture(id, texture) => (id, super::import::texture(helper, &texture, fds), Paint::Gpu),
			Event::Pixels {
				page,
				width,
				height,
				data,
			} => (
				page,
				super::upload(&helper.device, &helper.queue, width, height, &data),
				Paint::Cpu,
			),
			Event::Audio(id, samples) => {
				routes.lock().unwrap().play(id, &samples);
				continue;
			}
			Event::Failed(id, reason) => {
				routes.lock().unwrap().end(id, reason);
				continue;
			}
			Event::Fatal(reason) => return Err(reason),
		};
		// The copy is done, so the helper may hand the source back to CEF,
		// even for a page closed since. Waits behind queued input, never
		// dropped: the helper waits on it. A closed queue means the writer lost
		// the socket, so the helper is exiting, and stops waiting; reading on
		// until it has exited keeps it from blocking on a full socket.
		let _ = commands.send(Command::Painted);
		let mut routes = routes.lock().unwrap();
		match texture {
			Ok(texture) => routes.show(id, texture, paint),
			Err(reason) => routes.end(id, reason),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::browser::{Input, Modifiers};

	/// An open page, as [`Instance::open`] routes one, with a writer to
	/// `ours`.
	fn routed(ours: UnixStream) -> (Page, mpsc::SyncSender<Command>, Arc<Mutex<Routes>>) {
		let routes = Arc::new(Mutex::new(Routes::default()));
		let commands = writer(Channel::new(ours), routes.clone()).unwrap();
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(commands.clone());
		routes.lock().unwrap().pages.insert(
			0,
			Route {
				page: page.clone(),
				picture: Default::default(),
				sound: None,
				status: kio::Producer::new(Status::Starting),
				live: false,
				end: kio::Producer::new(None),
			},
		);
		(page, commands, routes)
	}

	/// A helper that stops reading costs dropped input, never a sender stuck
	/// on a full socket while it holds the studio's state.
	#[test]
	fn a_helper_that_stops_reading_never_blocks_input() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let (page, _commands, _routes) = routed(ours);
		// Far more than the socket buffers and the backlog hold together.
		for _ in 0..4096 {
			page.send(Input::Text("x".repeat(1024)));
		}
		assert!(page.state.lock().unwrap().backlogged);
		drop(theirs);
	}

	/// Each write makes room for an owed release and a page closed past the
	/// backlog, so both reach the helper once it reads again, with nothing
	/// newer to carry them.
	#[test]
	fn the_writer_delivers_owed_releases_and_closes() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let (page, commands, routes) = routed(ours);
		let release = Input::Key {
			code: 65,
			down: false,
			modifiers: Modifiers::default(),
		};
		page.state.lock().unwrap().owed.push_back(release.clone());
		routes.lock().unwrap().closing.push(7);
		commands.send(Command::Painted).unwrap();

		let helper = Channel::new(theirs);
		assert!(matches!(helper.recv::<Command>().unwrap(), Some((Command::Painted, _))));
		assert!(matches!(
			helper.recv::<Command>().unwrap(),
			Some((Command::Close(7), _))
		));
		let Some((Command::Input(0, input), _)) = helper.recv().unwrap() else {
			panic!("the owed release never came");
		};
		assert_eq!(input, release);
	}
}
