//! What a browser source says to its helper process, and back.
//!
//! One Unix stream socket carries both directions, each message a
//! little-endian `u32` length and that many bytes of postcard. A shared
//! texture's DMA-BUF descriptors ride with the first byte of its message as
//! `SCM_RIGHTS`, so they arrive with the message that describes them.

use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;

use rustix::net::{
	RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::Input;

/// The most planes a shared texture has, and so the most descriptors one
/// message carries: CEF's own limit.
pub(crate) const MAX_PLANES: usize = 4;

/// The longest message either side accepts: the largest page's pixels, with
/// room for the rest of the message. Bounds what a misbehaving peer can make
/// the other allocate.
const MAX_MESSAGE: usize = (super::MAX_SIDE * super::MAX_SIDE * 4) as usize + 1024;

/// A page's number, unique among those one helper opens.
pub(crate) type Id = u64;

/// What the studio asks of the helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Command {
	/// The first message: how the helper runs every page.
	Start(Start),
	/// Open a page.
	Open(Id, Open),
	/// Close a page.
	Close(Id),
	/// The last picture is copied, so the helper may let CEF reuse it.
	Painted,
	/// Interaction with a page.
	Input(Id, Input),
	/// The canvas's frame rate changed, so the page paints at this one.
	FrameRate(Id, u32),
	/// The last message: close every page, save the profile, and exit. The
	/// studio holds the socket open until then, so the helper reads its end
	/// as the studio dying and exits at once.
	Stop,
}

/// How a helper runs every page it opens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Start {
	pub raster: Raster,
	/// The profile every page shares: cookies, storage, and cache. CEF lets
	/// one process at a time hold it.
	pub profile: String,
}

/// Where a helper's pages paint.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) enum Raster {
	/// On the GPU, into shared textures the studio imports: on the DRM render
	/// node named, the studio's GPU, or else on the one CEF picks. CEF's GPU
	/// process needs `display` to allocate them.
	Gpu {
		render_node: Option<String>,
		display: Display,
	},
	/// In software, into memory the studio uploads, with no display opened.
	/// Startup never waits on a GPU process CEF cannot start, and WebGL runs
	/// on SwiftShader.
	#[default]
	Cpu,
}

/// The display server the GPU path connects to, the one the studio runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Display {
	/// `$WAYLAND_DISPLAY`, preferred: under XWayland, CEF's GPU process finds
	/// no GBM device on some drivers, NVIDIA's among them, and never paints.
	Wayland,
	/// `$DISPLAY`.
	X11,
}

/// A page to open.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Open {
	pub url: String,
	/// The page's viewport, in pixels.
	pub width: u32,
	pub height: u32,
	/// How often the page paints: the canvas's frame rate.
	pub framerate: u32,
	/// The sound the page plays is delivered at the program bus's rate and
	/// channel count, so it joins the bus without resampling.
	pub sample_rate: u32,
	pub channels: u32,
}

/// What the helper tells the studio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum Event {
	/// A page painted a layer into a shared texture, whose plane descriptors
	/// ride with this message. Valid until the studio answers
	/// [`Command::Painted`].
	Texture(Id, Layer, Texture),
	/// A page painted `layer` into memory: `width * height` premultiplied
	/// BGRA pixels, tightly packed.
	Pixels {
		page: Id,
		layer: Layer,
		width: u32,
		height: u32,
		data: Vec<u8>,
	},
	/// A page's popup opened or moved to this area of its view, or closed.
	Popup(Id, Option<Area>),
	/// Interleaved samples at the [`Open`] rate and channel count.
	Audio(Id, Vec<f32>),
	/// The text a [`Input::Copy`] or [`Input::Cut`] took from a page, for the
	/// studio's clipboard.
	Copied(String),
	/// A page cannot be shown, and why.
	Failed(Id, String),
	/// No page can be shown, and why. The helper exits next.
	Fatal(String),
}

/// Which of a page's pictures a paint is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Layer {
	/// The page itself.
	View,
	/// Its open popup, such as a `<select>`'s list, which CEF paints apart
	/// from the page, drawn over it at the area the last [`Event::Popup`]
	/// gave.
	Popup,
}

/// A rectangle of a page's view, in pixels from its top-left corner. It may
/// reach past the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Area {
	pub x: i32,
	pub y: i32,
	pub width: u32,
	pub height: u32,
}

/// A shared texture's layout, CEF's `cef_accelerated_paint_info_t` minus the
/// descriptors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Texture {
	pub width: u32,
	pub height: u32,
	pub format: Format,
	/// The DRM format modifier: how the planes are tiled.
	pub modifier: u64,
	pub planes: Vec<Plane>,
}

/// The pixel layouts CEF paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Format {
	Rgba,
	Bgra,
}

/// One plane of a shared texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Plane {
	pub stride: u32,
	pub offset: u64,
	pub size: u64,
}

/// One end of the socket.
///
/// Sending takes `&self`, so a caller sending from several threads holds a
/// lock around each [`send`](Self::send); receiving is for one thread.
pub(crate) struct Channel {
	socket: UnixStream,
}

impl Channel {
	pub fn new(socket: UnixStream) -> Self {
		Self { socket }
	}

	/// Another handle on the same socket, so one thread can send while
	/// another receives.
	pub fn try_clone(&self) -> io::Result<Self> {
		Ok(Self::new(self.socket.try_clone()?))
	}

	/// Send `message` with `fds` attached.
	pub fn send<T: Serialize>(&self, message: &T, fds: &[BorrowedFd<'_>]) -> io::Result<()> {
		assert!(fds.len() <= MAX_PLANES, "more descriptors than a message carries");
		let payload = postcard::to_stdvec(message).map_err(io::Error::other)?;
		if payload.len() > MAX_MESSAGE {
			return Err(io::Error::new(io::ErrorKind::InvalidInput, "message too long"));
		}
		let mut bytes = Vec::with_capacity(4 + payload.len());
		bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
		bytes.extend_from_slice(&payload);

		let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_PLANES))];
		let mut control = SendAncillaryBuffer::new(&mut space);
		if !fds.is_empty() {
			let pushed = control.push(SendAncillaryMessage::ScmRights(fds));
			assert!(pushed, "the control buffer holds every descriptor");
		}
		// The descriptors go with the first byte sent; a short send leaves the
		// rest for a plain write.
		let sent = rustix::net::sendmsg(&self.socket, &[IoSlice::new(&bytes)], &mut control, SendFlags::NOSIGNAL)?;
		(&self.socket).write_all(&bytes[sent..])
	}

	/// The next message and the descriptors that came with it, or `None` once
	/// the peer has closed the socket.
	pub fn recv<T: DeserializeOwned>(&self) -> io::Result<Option<(T, Vec<OwnedFd>)>> {
		let mut length = [0u8; 4];
		let mut read = 0;
		let mut fds = Vec::new();
		while read < length.len() {
			let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_PLANES))];
			let mut control = RecvAncillaryBuffer::new(&mut space);
			let received = rustix::net::recvmsg(
				&self.socket,
				&mut [IoSliceMut::new(&mut length[read..])],
				&mut control,
				RecvFlags::CMSG_CLOEXEC,
			)?;
			for message in control.drain() {
				if let RecvAncillaryMessage::ScmRights(received) = message {
					fds.extend(received);
				}
			}
			if received.flags.contains(rustix::net::ReturnFlags::CTRUNC) {
				return Err(invalid("more descriptors than a message carries"));
			}
			match received.bytes {
				0 if read == 0 => return Ok(None),
				0 => return Err(io::ErrorKind::UnexpectedEof.into()),
				bytes => read += bytes,
			}
		}

		let length = u32::from_le_bytes(length) as usize;
		if length > MAX_MESSAGE {
			return Err(invalid("message too long"));
		}
		let mut payload = vec![0u8; length];
		(&self.socket).read_exact(&mut payload)?;
		let message = postcard::from_bytes(&payload).map_err(invalid)?;
		Ok(Some((message, fds)))
	}
}

impl AsFd for Channel {
	fn as_fd(&self) -> BorrowedFd<'_> {
		self.socket.as_fd()
	}
}

fn invalid(message: impl std::fmt::Display) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pair() -> (Channel, Channel) {
		let (a, b) = UnixStream::pair().unwrap();
		(Channel::new(a), Channel::new(b))
	}

	/// Descriptors arrive with the message they were sent with, not the one
	/// before or after, however the stream splits.
	#[test]
	fn descriptors_ride_with_their_message() {
		let (studio, helper) = pair();
		let texture = Texture {
			width: 1280,
			height: 720,
			format: Format::Bgra,
			modifier: 0,
			planes: vec![Plane {
				stride: 1280 * 4,
				offset: 0,
				size: 1280 * 720 * 4,
			}],
		};
		let file = tempfile::tempfile().unwrap();
		helper.send(&Event::Audio(1, vec![0.5; 3]), &[]).unwrap();
		helper
			.send(&Event::Texture(1, Layer::View, texture.clone()), &[file.as_fd()])
			.unwrap();
		helper.send(&Event::Failed(1, "gone".into()), &[]).unwrap();
		drop(helper);

		let (event, fds) = studio.recv::<Event>().unwrap().unwrap();
		assert_eq!(event, Event::Audio(1, vec![0.5; 3]));
		assert!(fds.is_empty());

		let (event, fds) = studio.recv::<Event>().unwrap().unwrap();
		assert_eq!(event, Event::Texture(1, Layer::View, texture));
		assert_eq!(fds.len(), 1);
		let ours = rustix::fs::fstat(&fds[0]).unwrap();
		let theirs = rustix::fs::fstat(&file).unwrap();
		assert_eq!(
			(ours.st_dev, ours.st_ino),
			(theirs.st_dev, theirs.st_ino),
			"the same file"
		);

		let (event, fds) = studio.recv::<Event>().unwrap().unwrap();
		assert_eq!(event, Event::Failed(1, "gone".into()));
		assert!(fds.is_empty());

		assert!(studio.recv::<Event>().unwrap().is_none(), "closed");
	}

	/// A message larger than the socket buffer arrives whole.
	#[test]
	fn a_large_message_arrives_whole() {
		let (studio, helper) = pair();
		let data: Vec<u8> = (0..1920 * 1080 * 4).map(|i| i as u8).collect();
		let sent = Event::Pixels {
			page: 1,
			layer: Layer::View,
			width: 1920,
			height: 1080,
			data,
		};
		let sender = {
			let sent = sent.clone();
			std::thread::spawn(move || helper.send(&sent, &[]).unwrap())
		};
		let (received, _) = studio.recv::<Event>().unwrap().unwrap();
		sender.join().unwrap();
		assert_eq!(received, sent);
	}

	/// A peer cannot make the other side allocate past the bound.
	#[test]
	fn refuses_an_oversized_length() {
		let (studio, helper) = pair();
		(&helper.socket).write_all(&u32::MAX.to_le_bytes()).unwrap();
		let err = studio.recv::<Event>().unwrap_err();
		assert_eq!(err.kind(), io::ErrorKind::InvalidData);
	}

	/// A socket closed partway through a message is an error, not a clean end.
	#[test]
	fn a_truncated_message_is_an_error() {
		let (studio, helper) = pair();
		(&helper.socket).write_all(&[8, 0]).unwrap();
		drop(helper);
		let err = studio.recv::<Event>().unwrap_err();
		assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
	}
}
