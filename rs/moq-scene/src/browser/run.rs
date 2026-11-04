//! The studio's side of a browser source: start its helper, then deliver what
//! the helper paints and plays until the source stops.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc;

use moq_audio::bus::Channel as Sound;

use super::protocol::{Channel, Command, Event, Open};
use super::{ARG, Helper, Page, Paint};
use crate::Error;
use crate::source::{Mailbox, Picture, Status};

/// Commands waiting for the helper to read them. A helper this far behind is
/// hung, and [`Page::send`] drops new input rather than wait for it.
const BACKLOG: usize = 256;

/// Run the page `open` describes until its helper stops, delivering pictures
/// to `picture` and sound to `sound`. Dropping the future stops the helper.
///
/// Fills in how the helper paints and the page's frame rate: `open` names
/// only the page and the bus's format.
pub(crate) async fn run(
	helper: &Helper,
	mut open: Open,
	page: &Page,
	picture: &Mailbox,
	sound: Option<Sound>,
	status: &kio::Producer<Status>,
) -> Result<(), Error> {
	// Declared first so it is removed last, once the helper is gone.
	let profile = tempfile::tempdir().map_err(|err| Error::Browser(format!("cannot create a profile: {err}")))?;
	open.profile = profile.path().to_string_lossy().into_owned();
	// A later change waits for the writer, which sends it after `open`.
	open.framerate = {
		let mut state = page.state.lock().unwrap();
		state.retime = false;
		state.framerate
	};
	open.accelerated = super::import::supported(helper);
	open.render_node = helper
		.render_node
		.as_ref()
		.and_then(|node| node.to_str())
		.map(str::to_string);
	let (mut process, events) =
		spawn(helper).map_err(|err| Error::Browser(format!("cannot start {}: {err}", helper.program.display())))?;
	let commands = events
		.try_clone()
		.map_err(|err| Error::Browser(format!("socket: {err}")))?;
	let end = match commands
		.send(&Command::Open(open), &[])
		.map(|()| writer(commands, page))
	{
		Ok(Ok(commands)) => {
			{
				let mut state = page.state.lock().unwrap();
				state.commands = Some(commands.clone());
				state.flush();
			}
			let end = reader(helper, events, commands, page, picture, sound, status).await;
			page.state.lock().unwrap().commands = None;
			end
		}
		Ok(Err(err)) => End::Failed(format!("cannot start the page's writer: {err}")),
		Err(err) => closed(err),
	};
	// A stopped page leaves the canvas rather than freezing on its last
	// picture, which for an alert or a ticker would show stale news.
	*picture.lock().unwrap() = Some(Picture::Rgba {
		size: moq_video::Size::new(1, 1),
		pixels: vec![0; 4],
	});
	match end {
		// The helper is exiting or gone, so its status says why.
		End::Closed => {
			let exit = process
				.exited()
				.map_err(|err| Error::Browser(format!("the browser process: {err}")))?;
			Err(Error::Browser(format!("the browser process exited: {exit}")))
		}
		End::Failed(reason) => Err(Error::Browser(reason)),
	}
}

/// Why a page stopped.
#[derive(Clone)]
enum End {
	/// The helper closed the socket: it exited, or crashed.
	Closed,
	/// The helper reported a failure, or broke the protocol.
	Failed(String),
}

/// What a socket error says about the helper.
fn closed(err: std::io::Error) -> End {
	use std::io::ErrorKind::*;
	match err.kind() {
		BrokenPipe | ConnectionReset | UnexpectedEof => End::Closed,
		_ => End::Failed(format!("the browser process's socket: {err}")),
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
/// sender holding the studio's state. Each write makes room for the releases
/// `page` owes. The thread ends once every sender is dropped or the socket
/// fails, as it does when the helper is killed.
fn writer(channel: Channel, page: &Page) -> std::io::Result<mpsc::SyncSender<Command>> {
	let (sender, queue) = mpsc::sync_channel(BACKLOG);
	// Weak, since the page holds a sender, which would keep the thread waiting.
	let page = std::sync::Arc::downgrade(&page.state);
	std::thread::Builder::new()
		.name("browser-commands".into())
		.spawn(move || {
			for command in queue {
				if channel.send(&command, &[]).is_err() {
					break;
				}
				if let Some(page) = page.upgrade() {
					page.lock().unwrap().flush();
				}
			}
		})?;
	Ok(sender)
}

/// Run [`deliver`] on a thread of its own, since it blocks on the socket, and
/// wait for why it ended. A thread that cannot start, or that stops without
/// saying why, fails the page with the reason rather than leaving it live.
async fn reader(
	helper: &Helper,
	events: Channel,
	commands: mpsc::SyncSender<Command>,
	page: &Page,
	picture: &Mailbox,
	sound: Option<Sound>,
	status: &kio::Producer<Status>,
) -> End {
	let done = kio::Producer::new(None);
	let ended = done.consume();
	let spawned = {
		let (helper, page, picture, status) = (helper.clone(), page.clone(), picture.clone(), status.clone());
		std::thread::Builder::new()
			.name("browser-events".into())
			.spawn(move || {
				let end = deliver(&helper, &events, &commands, &page, &picture, sound, &status);
				if let Ok(mut done) = done.write() {
					*done = Some(end);
				}
			})
	};
	if let Err(err) = spawned {
		return End::Failed(format!("cannot start the page's reader: {err}"));
	}
	let end = ended
		.wait(|end: &kio::Ref<'_, Option<End>>| match end.as_ref() {
			Some(end) => std::task::Poll::Ready(end.clone()),
			None => std::task::Poll::Pending,
		})
		.await;
	end.unwrap_or_else(|_| End::Failed("the page's reader stopped".into()))
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

/// Read the helper's events until it closes the socket or reports a failure.
fn deliver(
	helper: &Helper,
	events: &Channel,
	commands: &mpsc::SyncSender<Command>,
	page: &Page,
	picture: &Mailbox,
	mut sound: Option<Sound>,
	status: &kio::Producer<Status>,
) -> End {
	let mut live = false;
	loop {
		let (event, fds) = match events.recv::<Event>() {
			Ok(Some(received)) => received,
			Ok(None) => return End::Closed,
			Err(err) => return closed(err),
		};
		let (texture, paint) = match event {
			Event::Texture(texture) => (super::import::texture(helper, &texture, fds), Paint::Gpu),
			Event::Pixels { width, height, data } => (
				super::upload(&helper.device, &helper.queue, width, height, &data),
				Paint::Cpu,
			),
			Event::Audio(samples) => {
				if let Some(sound) = &mut sound
					&& let Err(err) = sound.write(bytemuck::cast_slice(&samples))
				{
					return End::Failed(format!("the page's sound: {err}"));
				}
				continue;
			}
			Event::Failed(reason) => return End::Failed(reason),
		};
		// The copy is done, so the helper may hand the source back to CEF.
		// Waits behind queued input, never dropped: the helper waits on it. A
		// closed queue means the writer lost the socket, so the helper is gone.
		if commands.send(Command::Painted).is_err() {
			return End::Closed;
		}
		let texture = match texture {
			Ok(texture) => texture,
			Err(reason) => return End::Failed(reason),
		};
		page.state.lock().unwrap().paint = Some(paint);
		*picture.lock().unwrap() = Some(Picture::Texture(texture));
		if !std::mem::replace(&mut live, true)
			&& let Ok(mut status) = status.write()
		{
			*status = Status::Live;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::browser::{Input, Modifiers};

	/// A helper that stops reading costs dropped input, never a sender stuck
	/// on a full socket while it holds the studio's state.
	#[test]
	fn a_helper_that_stops_reading_never_blocks_input() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(writer(Channel::new(ours), &page).unwrap());
		// Far more than the socket buffers and the backlog hold together.
		for _ in 0..4096 {
			page.send(Input::Text("x".repeat(1024)));
		}
		assert!(page.state.lock().unwrap().backlogged);
		drop(theirs);
	}

	/// Each write makes room for an owed release, so it reaches the page once
	/// the helper reads again, with no newer input to carry it.
	#[test]
	fn the_writer_delivers_owed_releases() {
		let (ours, theirs) = UnixStream::pair().unwrap();
		let page = Page::default();
		let commands = writer(Channel::new(ours), &page).unwrap();
		let release = Input::Key {
			code: 65,
			down: false,
			modifiers: Modifiers::default(),
		};
		{
			let mut state = page.state.lock().unwrap();
			state.commands = Some(commands.clone());
			state.owed.push_back(release.clone());
		}
		commands.send(Command::Painted).unwrap();

		let helper = Channel::new(theirs);
		assert!(matches!(helper.recv::<Command>().unwrap(), Some((Command::Painted, _))));
		let Some((Command::Input(input), _)) = helper.recv().unwrap() else {
			panic!("the owed release never came");
		};
		assert_eq!(input, release);
	}
}
