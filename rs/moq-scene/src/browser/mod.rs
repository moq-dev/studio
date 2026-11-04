//! Browser sources: a web page, such as an overlay, alert box, or chat widget,
//! rendered offscreen by CEF (the Chromium Embedded Framework) and drawn by the
//! compositor.
//!
//! Each page runs in a helper process of its own, so a crash there fails that
//! source and nothing else. The helper is the studio's own executable started
//! again, which calls [`child`] first thing in `main` to become one; CEF's own
//! subprocesses start the same way. The helper paints into a shared GPU texture
//! the studio copies on its device, or into memory the studio uploads when
//! either side cannot share textures ([`Paint`] says which). The page's sound
//! joins the program bus at the bus's rate.
//!
//! Linux only for now, and only with the `cef` feature: elsewhere a browser
//! source reports that it is unsupported.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::wgpu;

#[cfg(target_os = "linux")]
mod protocol;
#[cfg(target_os = "linux")]
mod run;

#[cfg(all(target_os = "linux", feature = "cef"))]
mod host;
#[cfg(target_os = "linux")]
mod import;

#[cfg(target_os = "linux")]
pub(crate) use protocol::Open;
#[cfg(target_os = "linux")]
pub(crate) use run::run;

/// The longest side of a page, in pixels: room for a 4K canvas.
pub(crate) const MAX_SIDE: u32 = 4096;

/// The argument that starts the studio's executable as a helper.
#[cfg_attr(
	not(target_os = "linux"),
	expect(dead_code, reason = "browser sources are Linux-only")
)]
const ARG: &str = "--moq-browser-helper";

/// Become a browser source's helper, or one of CEF's subprocesses, if this
/// process was started as one; `None` for any other process.
///
/// A binary that offers browser sources calls this before anything else in
/// `main`, and exits with the code it returns.
pub fn child() -> Option<ExitCode> {
	#[cfg(all(target_os = "linux", feature = "cef"))]
	{
		if std::env::args_os().nth(1).is_some_and(|arg| arg == ARG) {
			return Some(host::run());
		}
		if std::env::args_os().any(|arg| arg.as_encoded_bytes().starts_with(b"--type=")) {
			return Some(host::subprocess());
		}
	}
	None
}

/// What browser sources need from the studio: the executable that becomes
/// their helper, and the device their pictures are copied to.
#[derive(Clone)]
#[cfg_attr(
	not(target_os = "linux"),
	expect(dead_code, reason = "browser sources are Linux-only")
)]
pub struct Helper {
	program: PathBuf,
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// The device's DRM render node, which CEF must paint on for the studio to
	/// import what it paints.
	render_node: Option<PathBuf>,
}

impl Helper {
	/// Start helpers from `program`, which calls [`child`] first thing in
	/// `main`, usually the running executable, and copy their pictures to
	/// `device`, opened from `adapter`.
	pub fn new(
		program: impl Into<PathBuf>,
		adapter: &wgpu::Adapter,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
	) -> Self {
		// A machine with two GPUs would otherwise paint on whichever CEF
		// picks, and the studio cannot import from another GPU.
		let bus = adapter.get_info().device_pci_bus_id;
		let render_node = (!bus.is_empty())
			.then(|| std::fs::canonicalize(format!("/dev/dri/by-path/pci-{bus}-render")).ok())
			.flatten();
		Self {
			program: program.into(),
			device: device.clone(),
			queue: queue.clone(),
			render_node,
		}
	}
}

/// How a page's pictures reach the compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
	/// A shared texture, copied on the GPU.
	Gpu,
	/// Pixels in memory, uploaded every frame: the fallback when the page's
	/// GPU process or the studio's device cannot share textures.
	Cpu,
}

/// A running browser source: how it paints, and the way to interact with it.
#[derive(Clone, Default)]
pub struct Page {
	state: Arc<Mutex<PageState>>,
}

#[derive(Default)]
struct PageState {
	/// The queue to the helper's socket, which [`run::writer`] drains.
	#[cfg(target_os = "linux")]
	commands: Option<std::sync::mpsc::SyncSender<protocol::Command>>,
	/// Whether input is being dropped because the queue is full.
	#[cfg(target_os = "linux")]
	backlogged: bool,
	/// Releases that did not fit the queue, oldest first, at most one per
	/// button, key, focus, and pointer, sent ahead of any newer input.
	#[cfg(target_os = "linux")]
	owed: std::collections::VecDeque<Input>,
	/// The canvas's frame rate, which the page paints at.
	#[cfg(target_os = "linux")]
	framerate: u32,
	/// Whether the helper has yet to hear `framerate`.
	#[cfg(target_os = "linux")]
	retime: bool,
	paint: Option<Paint>,
}

#[cfg(target_os = "linux")]
impl PageState {
	/// Queue `input` behind any owed releases. Past the backlog, a release is
	/// owed rather than dropped, so a helper that catches up never leaves the
	/// page holding a button or key the studio let go of; other input drops.
	fn send(&mut self, input: Input) {
		use std::sync::mpsc::TrySendError;

		self.flush();
		let Some(commands) = &self.commands else { return };
		let input = match self.owed.is_empty() {
			true => match commands.try_send(protocol::Command::Input(input)) {
				Ok(()) => {
					self.backlogged = false;
					return;
				}
				Err(TrySendError::Full(protocol::Command::Input(input))) => input,
				Err(TrySendError::Full(_)) => unreachable!("only input is sent here"),
				Err(TrySendError::Disconnected(_)) => return self.disconnect(),
			},
			// Newer input waits behind the owed releases.
			false => input,
		};
		if let Some(release) = Release::of(&input) {
			self.owed.retain(|owed| Release::of(owed) != Some(release));
			self.owed.push_back(input);
		}
		if !std::mem::replace(&mut self.backlogged, true) {
			tracing::warn!("a browser source's helper is not reading input; dropping it");
		}
	}

	/// Queue a new frame rate, then the owed releases, oldest first, while the
	/// queue has room.
	fn flush(&mut self) {
		use std::sync::mpsc::TrySendError;

		let Some(commands) = &self.commands else { return };
		if self.retime {
			match commands.try_send(protocol::Command::FrameRate(self.framerate)) {
				Ok(()) => self.retime = false,
				Err(TrySendError::Full(_)) => return,
				Err(TrySendError::Disconnected(_)) => return self.disconnect(),
			}
		}
		while let Some(input) = self.owed.pop_front() {
			match commands.try_send(protocol::Command::Input(input)) {
				Ok(()) => {}
				Err(TrySendError::Full(protocol::Command::Input(input))) => {
					self.owed.push_front(input);
					return;
				}
				Err(TrySendError::Full(_)) => unreachable!("only input is sent here"),
				Err(TrySendError::Disconnected(_)) => return self.disconnect(),
			}
		}
	}

	/// The helper is gone, which its source reports.
	fn disconnect(&mut self) {
		self.commands = None;
		self.owed.clear();
	}
}

/// What an input lets go of, which a later release of the same supersedes.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Release {
	Button(Button),
	Key(i32),
	Focus,
	Pointer,
}

#[cfg(target_os = "linux")]
impl Release {
	fn of(input: &Input) -> Option<Self> {
		match *input {
			Input::Button {
				button, down: false, ..
			} => Some(Self::Button(button)),
			Input::Key { code, down: false, .. } => Some(Self::Key(code)),
			Input::Focus(false) => Some(Self::Focus),
			Input::Leave { .. } => Some(Self::Pointer),
			_ => None,
		}
	}
}

impl Page {
	/// Whether `other` is this page, rather than one restarted under the same
	/// name.
	pub fn same(&self, other: &Page) -> bool {
		Arc::ptr_eq(&self.state, &other.state)
	}

	/// How the newest picture arrived, once one has.
	pub fn paint(&self) -> Option<Paint> {
		self.state.lock().unwrap().paint
	}

	/// Paint at `rate`, the canvas's frame rate, from now on, without waiting on
	/// the helper. One that is behind hears it once it catches up.
	pub(crate) fn set_framerate(&self, rate: u32) {
		#[cfg(target_os = "linux")]
		{
			let mut state = self.state.lock().unwrap();
			if state.framerate != rate {
				state.framerate = rate;
				state.retime = true;
				state.flush();
			}
		}
		#[cfg(not(target_os = "linux"))]
		let _ = rate;
	}

	/// Interact with the page, without waiting on its helper. Dropped while
	/// the page is starting or after it stopped, as a click on a window that is
	/// not there yet would be, and while the helper is too far behind to take
	/// more, as a hung window's would be, except that a release is delivered
	/// once the helper catches up.
	pub fn send(&self, input: Input) {
		#[cfg(target_os = "linux")]
		self.state.lock().unwrap().send(input);
		#[cfg(not(target_os = "linux"))]
		let _ = input;
	}
}

/// Interaction with a page. Positions are in page pixels from its top-left
/// corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Input {
	/// The pointer moved over the page.
	Move { x: i32, y: i32, modifiers: Modifiers },
	/// The pointer left the page.
	Leave { modifiers: Modifiers },
	/// A mouse button went down or up; `clicks` counts a double or triple
	/// click.
	Button {
		x: i32,
		y: i32,
		button: Button,
		down: bool,
		clicks: u32,
		modifiers: Modifiers,
	},
	/// A scroll, in pixels.
	Wheel {
		x: i32,
		y: i32,
		dx: i32,
		dy: i32,
		modifiers: Modifiers,
	},
	/// A key went down or up. `code` is the Windows virtual-key code, which is
	/// what CEF takes on every platform.
	Key {
		code: i32,
		down: bool,
		modifiers: Modifiers,
	},
	/// Text typed, after the keyboard layout and any input method.
	Text(String),
	/// Copy the page's selection to the clipboard.
	Copy,
	/// Cut the page's selection to the clipboard.
	Cut,
	/// The page gained or lost keyboard focus.
	Focus(bool),
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
	Left,
	Middle,
	Right,
}

/// The keys and buttons held during an [`Input`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
	pub shift: bool,
	pub ctrl: bool,
	pub alt: bool,
	/// Command on macOS, the Windows key elsewhere.
	pub meta: bool,
	pub left: bool,
	pub middle: bool,
	pub right: bool,
}

/// The texture a page's pixels are uploaded to: premultiplied BGRA, as CEF
/// paints.
#[cfg_attr(
	not(target_os = "linux"),
	expect(dead_code, reason = "browser sources are Linux-only")
)]
fn upload(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	width: u32,
	height: u32,
	data: &[u8],
) -> Result<wgpu::Texture, String> {
	let limit = device.limits().max_texture_dimension_2d;
	if width == 0 || height == 0 || width > limit || height > limit {
		return Err(format!(
			"a {width}x{height} page does not fit this GPU's {limit}px texture limit"
		));
	}
	if data.len() != width as usize * height as usize * 4 {
		return Err(format!("{} bytes is not a {width}x{height} picture", data.len()));
	}
	let size = wgpu::Extent3d {
		width,
		height,
		depth_or_array_layers: 1,
	};
	let texture = device.create_texture(&wgpu::TextureDescriptor {
		label: Some("moq-scene page"),
		size,
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: wgpu::TextureFormat::Bgra8Unorm,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	});
	queue.write_texture(
		texture.as_image_copy(),
		data,
		wgpu::TexelCopyBufferLayout {
			offset: 0,
			bytes_per_row: Some(width * 4),
			rows_per_image: Some(height),
		},
		size,
	);
	Ok(texture)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
	use super::*;

	fn button(down: bool) -> Input {
		Input::Button {
			x: 1,
			y: 1,
			button: Button::Left,
			down,
			clicks: 1,
			modifiers: Modifiers::default(),
		}
	}

	/// Past the backlog, a release is owed rather than dropped, once per
	/// button, and goes ahead of newer input as the queue makes room, so the
	/// page never keeps holding what the studio let go of.
	#[test]
	fn a_release_past_the_backlog_is_owed() {
		let (sender, queue) = std::sync::mpsc::sync_channel(4);
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(sender);
		page.send(button(true));
		while !page.state.lock().unwrap().backlogged {
			page.send(Input::Text("x".into()));
		}
		page.send(button(false));
		page.send(button(true));
		page.send(button(false));
		page.send(Input::Move {
			x: 2,
			y: 2,
			modifiers: Modifiers::default(),
		});
		assert_eq!(page.state.lock().unwrap().owed, [button(false)]);

		// The helper catches up, and each write makes room, as the writer's do.
		let mut received = Vec::new();
		while let Ok(protocol::Command::Input(input)) = queue.try_recv() {
			received.push(input);
			page.state.lock().unwrap().flush();
		}
		assert_eq!(received.first(), Some(&button(true)));
		assert_eq!(received.last(), Some(&button(false)));
		assert_eq!(received.iter().filter(|input| **input == button(false)).count(), 1);
		assert!(!received.iter().any(|input| matches!(input, Input::Move { .. })));
	}

	/// A new canvas frame rate reaches the page once, even past the backlog.
	#[test]
	fn a_new_frame_rate_reaches_the_page() {
		let (sender, queue) = std::sync::mpsc::sync_channel(1);
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(sender);
		page.set_framerate(30);
		page.set_framerate(30);
		assert!(matches!(queue.try_recv(), Ok(protocol::Command::FrameRate(30))));
		assert!(queue.try_recv().is_err(), "an unchanged rate is not sent again");

		page.send(Input::Text("x".into()));
		page.set_framerate(60);
		assert!(matches!(queue.try_recv(), Ok(protocol::Command::Input(_))));
		page.state.lock().unwrap().flush();
		assert!(matches!(queue.try_recv(), Ok(protocol::Command::FrameRate(60))));
	}
}
