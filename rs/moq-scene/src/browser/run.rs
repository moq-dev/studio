//! The studio's side of a browser source: start its helper, then deliver what
//! the helper paints and plays until the source stops.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};

use moq_audio::bus::Channel as Sound;

use super::protocol::{Channel, Command, Event, Open};
use super::{ARG, Helper, Page, Paint};
use crate::Error;
use crate::source::{Mailbox, Picture, Status};

/// Run the page `open` describes until its helper stops, delivering pictures
/// to `picture` and sound to `sound`. Dropping the future stops the helper.
pub(crate) async fn run(
	helper: &Helper,
	open: Open,
	page: &Page,
	picture: &Mailbox,
	sound: Option<Sound>,
	status: &kio::Producer<Status>,
) -> Result<(), Error> {
	let (mut process, events) = spawn(helper)
		.map_err(|err| Error::Browser(format!("cannot start {}: {err}", helper.program.display())))?;
	let commands = events.try_clone().map_err(|err| Error::Browser(format!("socket: {err}")))?;
	let end = match commands.send(&Command::Open(open), &[]) {
		Ok(()) => {
			page.state.lock().unwrap().commands = Some(commands);
			let reader = {
				let (helper, page, picture, status) = (helper.clone(), page.clone(), picture.clone(), status.clone());
				tokio::task::spawn_blocking(move || deliver(&helper, &events, &page, &picture, sound, &status))
			};
			let end = reader.await;
			page.state.lock().unwrap().commands = None;
			end.unwrap_or_else(|err| End::Failed(format!("the page's reader panicked: {err}")))
		}
		Err(err) => closed(err),
	};
	match end {
		// The helper is exiting or gone, so its status says why.
		End::Closed => {
			let exit = process.child.wait().map_err(|err| Error::Browser(format!("the browser process: {err}")))?;
			Err(Error::Browser(format!("the browser process exited: {exit}")))
		}
		End::Failed(reason) => Err(Error::Browser(reason)),
	}
}

/// Why a page stopped.
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

/// The helper process, killed when dropped.
struct Process {
	child: Child,
}

impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

/// Start a helper, handing it one end of a socket as its stdin.
fn spawn(helper: &Helper) -> std::io::Result<(Process, Channel)> {
	let (ours, theirs) = UnixStream::pair()?;
	let child = std::process::Command::new(&helper.program)
		.arg(ARG)
		.stdin(OwnedFd::from(theirs))
		.stdout(Stdio::null())
		.spawn()?;
	Ok((Process { child }, Channel::new(ours)))
}

/// Read the helper's events until it closes the socket or reports a failure.
fn deliver(
	helper: &Helper,
	events: &Channel,
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
		if let Some(commands) = &page.state.lock().unwrap().commands
			&& let Err(err) = commands.send(&Command::Painted, &[])
		{
			return closed(err);
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
