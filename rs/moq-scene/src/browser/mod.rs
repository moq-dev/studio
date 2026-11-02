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
pub struct Helper {
	program: PathBuf,
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// The device's DRM render node, which CEF must paint on for the studio to
	/// import what it paints.
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "browser sources are Linux-only")
	)]
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
	#[cfg(target_os = "linux")]
	commands: Option<protocol::Channel>,
	paint: Option<Paint>,
}

impl Page {
	/// How the newest picture arrived, once one has.
	pub fn paint(&self) -> Option<Paint> {
		self.state.lock().unwrap().paint
	}

	/// Interact with the page. Dropped while the page is starting or after it
	/// stopped, as a click on a window that is not there yet would be.
	pub fn send(&self, input: Input) {
		#[cfg(target_os = "linux")]
		{
			let mut state = self.state.lock().unwrap();
			let sent = state
				.commands
				.as_ref()
				.map(|commands| commands.send(&protocol::Command::Input(input), &[]));
			// A failed send means the helper is gone, which its source reports.
			if let Some(Err(_)) = sent {
				state.commands = None;
			}
		}
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
