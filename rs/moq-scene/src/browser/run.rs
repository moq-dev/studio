//! The studio's side of browser sources: start the helper every page shares,
//! then deliver what it paints and plays to each page's source until the
//! source stops.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use moq_audio::bus::Channel as Sound;

use super::layers::{Gpu, Layers};
use super::outbox::Commands;
use super::protocol::{Area, Channel, Command, Event, Id, Layer, Open, Start};
use super::{ARG, Helper, Page, Paint};
use crate::Error;
use crate::source::{Mailbox, Picture, Status};
use crate::wgpu;

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
		layers: Layers::default(),
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
	picture.lock().unwrap().clear();
	page.state.lock().unwrap().paint = None;
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
	/// What waits for [`writer`] to write it.
	commands: Commands,
	routes: Arc<Mutex<Routes>>,
	/// The next page's number.
	next: AtomicU64,
	/// The thread delivering the helper's events, which ends once the helper
	/// has exited.
	reader: Mutex<Option<JoinHandle<()>>>,
}

/// Where each open page's events go.
#[derive(Default)]
struct Routes {
	pages: HashMap<Id, Route>,
	/// Why the helper stopped, once it has: no page opens in it after.
	ended: Option<String>,
	/// What a page last copied or cut, until the studio takes it. One slot
	/// for every page, so a newer copy always replaces an older one.
	copied: Option<String>,
	/// Why a page that paints into memory does.
	fallback: Arc<str>,
}

/// One open page's source.
struct Route {
	page: Page,
	picture: Mailbox,
	sound: Option<Sound>,
	status: kio::Producer<Status>,
	layers: Layers,
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

	/// Take `texture`, painted the way `paint` says, as page `id`'s newest
	/// `layer`, and show the page.
	fn paint(&mut self, gpu: &Gpu, id: Id, layer: Layer, texture: wgpu::Texture, paint: Paint) {
		let Some(route) = self.pages.get_mut(&id) else { return };
		if layer == Layer::View {
			route.page.state.lock().unwrap().paint = Some(paint);
		}
		route.layers.paint(layer, texture);
		self.show(gpu, id);
	}

	/// Page `id`'s popup opened or moved to `area`, or closed.
	fn place(&mut self, gpu: &Gpu, id: Id, area: Option<Area>) {
		let Some(route) = self.pages.get_mut(&id) else { return };
		route.layers.place(area);
		self.show(gpu, id);
	}

	/// Show page `id`'s newest pictures, once it has painted.
	fn show(&mut self, gpu: &Gpu, id: Id) {
		let Some(route) = self.pages.get_mut(&id) else { return };
		let texture = match route.layers.picture(gpu) {
			Ok(Some(texture)) => texture,
			Ok(None) => return,
			Err(reason) => return self.end(id, reason),
		};
		route.picture.lock().unwrap().show(Picture::Texture(texture));
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
		// Only its owner may read the logins it keeps, even when it already
		// existed; a directory another user owns refuses the change.
		std::fs::DirBuilder::new()
			.recursive(true)
			.mode(0o700)
			.create(&profile)
			.and_then(|()| std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700)))
			.map_err(|err| failed(&format!("cannot create the browser profile {}", profile.display()), err))?;
		let start = Start {
			raster: helper.raster.clone(),
			profile: profile
				.to_str()
				.ok_or_else(|| Error::Browser(format!("the browser profile {} is not UTF-8", profile.display())))?
				.to_string(),
		};

		let (process, events) =
			spawn(helper).map_err(|err| failed(&format!("cannot start {}", helper.program.display()), err))?;
		let commands = events.try_clone().map_err(|err| failed("socket", err))?;
		commands
			.send(&Command::Start(start), &[])
			.map_err(|err| failed("the browser process's socket", err))?;
		let routes = Arc::new(Mutex::new(Routes {
			fallback: helper.fallback.clone(),
			..Default::default()
		}));
		let commands = writer(commands).map_err(|err| failed("cannot start the page writer", err))?;
		let reader = {
			// Not the helper, whose hold on this instance would keep it running
			// once every other owner is gone.
			let gpu = Gpu::new(&helper.device, &helper.queue);
			let (reading, routes) = (commands.clone(), routes.clone());
			let spawned = std::thread::Builder::new()
				.name("browser-events".into())
				.spawn(move || read(&gpu, process, &events, &reading, &routes));
			// The writer ends with it; the helper, dropped with the closure, is
			// killed.
			spawned.map_err(|err| {
				commands.stop();
				failed("cannot start the page reader", err)
			})?
		};
		Ok(Self {
			commands,
			routes,
			next: AtomicU64::new(0),
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
		self.commands.open(id, open);
		// After the page opens, since the helper delivers only to an open page.
		let mut state = page.state.lock().unwrap();
		state.id = id;
		state.commands = Some(self.commands.clone());
		state.flush();
		Ok(opened)
	}

	/// Take what a page last copied or cut, once.
	pub(super) fn copied(&self) -> Option<String> {
		self.routes.lock().unwrap().copied.take()
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
		self.commands.quit();
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
		self.instance.routes.lock().unwrap().pages.remove(&self.id);
		self.instance.commands.close(self.id);
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

/// Write the commands queued on the returned handle to `channel`, on a thread
/// of its own, so a helper that stops reading blocks only that thread, never a
/// sender holding the studio's state. The thread ends once the handle is
/// stopped or quit, or a write fails, as one does when the helper exits.
///
/// Once quit it leaves the socket open, since the helper reads its end as the
/// studio dying, and exits at once.
fn writer(channel: Channel) -> std::io::Result<Commands> {
	let commands = Commands::default();
	let outbox = commands.clone();
	std::thread::Builder::new()
		.name("browser-commands".into())
		.spawn(move || {
			'write: while let Some(batch) = outbox.next() {
				for command in batch {
					if channel.send(&command, &[]).is_err() {
						// Such as a command too long to send: a helper the
						// studio can no longer reach never keeps running
						// unheard.
						let _ = rustix::net::shutdown(&channel, rustix::net::Shutdown::Write);
						break 'write;
					}
				}
			}
			outbox.stop();
		})?;
	Ok(commands)
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
fn read(gpu: &Gpu, mut process: Process, events: &Channel, commands: &Commands, routes: &Mutex<Routes>) {
	let reason = match deliver(gpu, events, commands, routes) {
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
	// Nothing more reaches it, so the writer ends.
	commands.stop();
	let mut routes = routes.lock().unwrap();
	routes.ended = Some(reason.clone());
	for (_, route) in routes.pages.drain() {
		route.end(reason.clone());
	}
}

/// Route the helper's events to their pages until it closes the socket, or
/// fail with why it cannot go on.
fn deliver(gpu: &Gpu, events: &Channel, commands: &Commands, routes: &Mutex<Routes>) -> Result<(), String> {
	use std::io::ErrorKind::*;

	let Gpu { device, queue, .. } = gpu;
	let fallback = routes.lock().unwrap().fallback.clone();

	loop {
		let (event, fds) = match events.recv::<Event>() {
			Ok(Some(received)) => received,
			Ok(None) => return Ok(()),
			Err(err) if matches!(err.kind(), BrokenPipe | ConnectionReset | UnexpectedEof) => return Ok(()),
			Err(err) => return Err(format!("the browser process's socket: {err}")),
		};
		let (id, layer, texture, paint) = match event {
			Event::Texture(id, layer, texture) => (
				id,
				layer,
				super::import::texture(device, queue, &texture, fds),
				Paint::Gpu,
			),
			Event::Pixels {
				page,
				layer,
				width,
				height,
				data,
			} => (
				page,
				layer,
				super::upload(device, queue, width, height, &data),
				Paint::Cpu(fallback.clone()),
			),
			Event::Popup(id, area) => {
				routes.lock().unwrap().place(gpu, id, area);
				continue;
			}
			Event::Audio(id, samples) => {
				routes.lock().unwrap().play(id, &samples);
				continue;
			}
			Event::Copied(text) => {
				routes.lock().unwrap().copied = Some(text);
				continue;
			}
			Event::Failed(id, reason) => {
				routes.lock().unwrap().end(id, reason);
				continue;
			}
			Event::Fatal(reason) => return Err(reason),
		};
		// The copy is done, so the helper may hand the source back to CEF,
		// even for a page closed since: the helper waits on it. A closed queue
		// means the writer lost the socket, so the helper is exiting, and stops
		// waiting; reading on until it has exited keeps it from blocking on a
		// full socket.
		commands.painted();
		let mut routes = routes.lock().unwrap();
		match texture {
			Ok(texture) => routes.paint(gpu, id, layer, texture, paint),
			Err(reason) => routes.end(id, reason),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::browser::Input;

	/// A command the writer cannot send ends the socket, which the helper
	/// reads as the studio gone, rather than leave it waiting unheard.
	#[test]
	fn a_command_too_long_to_send_ends_the_socket() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let ours = Channel::new(ours);
		// Held open, as the studio's reader does.
		let _events = ours.try_clone().unwrap();
		let commands = writer(ours).unwrap();
		let long = Open {
			url: "x".repeat(crate::browser::protocol::MAX_MESSAGE),
			..Default::default()
		};
		commands.open(0, long);
		assert!(Channel::new(theirs).recv::<Command>().unwrap().is_none());
	}

	/// A helper that stops reading costs dropped input, never a sender stuck
	/// on a full socket while it holds the studio's state. Pages still open
	/// and close without waiting, and reloads meanwhile leave only the newest
	/// page to open once the helper reads again.
	#[test]
	fn a_helper_that_stops_reading_blocks_nothing() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let commands = writer(Channel::new(ours)).unwrap();
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(commands.clone());
		// An open far larger than the socket buffers: once the writer takes
		// it, it is stuck writing it until the helper reads.
		let stuck = Open {
			url: "x".repeat(16 << 20),
			..Default::default()
		};
		commands.open(0, stuck);
		while !commands.is_empty() {
			std::thread::yield_now();
		}

		for _ in 0..1024 {
			page.send(Input::Text("x".into()));
		}
		assert!(page.state.lock().unwrap().backlogged);
		for id in 1..100 {
			commands.open(id, Open::default());
			commands.close(id);
		}
		commands.open(100, Open::default());

		let helper = Channel::new(theirs);
		assert!(matches!(
			helper.recv::<Command>().unwrap(),
			Some((Command::Open(0, _), _))
		));
		loop {
			match helper.recv::<Command>().unwrap() {
				Some((Command::Open(100, _), _)) => break,
				Some((Command::Open(id, _) | Command::Close(id), _)) => panic!("page {id} was reloaded away"),
				Some(_) => {}
				None => panic!("the writer stopped"),
			}
		}
	}
}
