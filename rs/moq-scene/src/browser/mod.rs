//! Browser sources: a web page, such as an overlay, alert box, or chat widget,
//! rendered offscreen by CEF (the Chromium Embedded Framework) and drawn by the
//! compositor.
//!
//! Every page runs in one helper process, CEF's browser process, so they
//! share one persistent profile: cookies and logins survive a restart of the
//! source or the studio, and CEF lets only one process hold a profile. Each
//! page still renders in a process of its own, so a page that crashes fails
//! only its own source. A crash of the helper itself fails every page, and the
//! next page to start starts a new helper. The helper is the studio's own
//! executable started again, which calls [`child`] first thing in `main` to
//! become one; CEF's own subprocesses start the same way. The helper paints
//! into a shared GPU texture the studio copies on its device, or into memory
//! the studio uploads when either side cannot share textures or there is no
//! display to paint them through ([`Paint`] says which, and why). Its popups, such as a
//! `<select>`'s list, arrive the same way apart from the page, and the studio
//! draws them over it. A page's sound joins the program bus at the bus's rate.
//!
//! Linux only for now, and only with the `cef` feature: elsewhere a browser
//! source reports that it is unsupported.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::wgpu;

#[cfg(target_os = "linux")]
mod layers;
#[cfg(target_os = "linux")]
mod outbox;
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
/// their helper, the profile they share, and the device their pictures are
/// copied to. Clones share one helper.
#[derive(Clone)]
#[cfg_attr(
	not(target_os = "linux"),
	expect(dead_code, reason = "browser sources are Linux-only")
)]
pub struct Helper {
	program: PathBuf,
	profile: PathBuf,
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// Where pages paint.
	#[cfg(target_os = "linux")]
	raster: protocol::Raster,
	/// Why a page that paints into memory does.
	#[cfg(target_os = "linux")]
	fallback: Arc<str>,
	/// The helper, once a page has started it.
	#[cfg(target_os = "linux")]
	running: Arc<Mutex<Option<Arc<run::Instance>>>>,
}

impl Helper {
	/// Start the helper from `program`, which calls [`child`] first thing in
	/// `main`, usually the running executable, keep every page's cookies and
	/// storage in the directory `profile`, created if missing, and copy their
	/// pictures to `device`, opened from `adapter`.
	///
	/// The profile holds the user's logins: keep it with their other state,
	/// not beside a scene file meant to be shared. One studio at a time may
	/// use it; a second's pages fail saying so.
	pub fn new(
		program: impl Into<PathBuf>,
		profile: impl Into<PathBuf>,
		adapter: &wgpu::Adapter,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
	) -> Self {
		#[cfg(not(target_os = "linux"))]
		let _ = adapter;
		#[cfg(target_os = "linux")]
		let (raster, fallback) = match gpu(adapter, device) {
			Ok(raster) => (raster, "its GPU process cannot share textures".into()),
			Err(why) => (protocol::Raster::Cpu, why.into()),
		};
		Self {
			program: program.into(),
			profile: profile.into(),
			device: device.clone(),
			queue: queue.clone(),
			#[cfg(target_os = "linux")]
			raster,
			#[cfg(target_os = "linux")]
			fallback,
			#[cfg(target_os = "linux")]
			running: Default::default(),
		}
	}

	/// Take what a page last copied or cut, once, for the desktop's
	/// clipboard, which CEF reaches itself only through an X display.
	#[cfg(target_os = "linux")]
	pub fn copied(&self) -> Option<String> {
		let running = self.running.lock().unwrap();
		running.as_ref()?.copied()
	}

	/// Take what a page last copied or cut: nothing, without browser sources.
	#[cfg(not(target_os = "linux"))]
	pub fn copied(&self) -> Option<String> {
		None
	}

	/// Stop the helper, failing every page, and wait for it to exit, so the
	/// profile is saved and free for the next studio. The next page to start
	/// starts a new helper.
	pub fn stop(&self) {
		#[cfg(target_os = "linux")]
		{
			// Held throughout, so no page starts another helper on the profile
			// until this one has exited.
			let mut running = self.running.lock().unwrap();
			if let Some(instance) = running.take() {
				instance.stop();
			}
		}
	}
}

/// The GPU path pages paint on, decided once from what the studio's device
/// and session offer, or why they cannot take it.
#[cfg(target_os = "linux")]
fn gpu(adapter: &wgpu::Adapter, device: &wgpu::Device) -> Result<protocol::Raster, String> {
	use protocol::{Display, Raster};

	if !cfg!(feature = "cef") {
		return Err("this build cannot share textures".into());
	}
	let info = adapter.get_info();
	// A software device holds a shared texture in memory anyway, so there the
	// page paints in software too.
	if info.device_type == wgpu::DeviceType::Cpu {
		return Err(format!("the studio's GPU, {}, renders in software", info.name));
	}
	if !device
		.features()
		.contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
	{
		return Err(format!("the studio's GPU, {}, cannot import shared textures", info.name));
	}
	let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
	let display = if set("WAYLAND_DISPLAY") {
		Display::Wayland
	} else if set("DISPLAY") {
		x11_gpu(&info)?;
		Display::X11
	} else {
		// CEF's GPU process cannot allocate shared textures without one.
		return Err("there is no display to paint through".into());
	};
	// A machine with two GPUs would otherwise paint on whichever CEF picks,
	// and the studio cannot import from another GPU.
	let render_node = (!info.device_pci_bus_id.is_empty())
		.then(|| std::fs::canonicalize(format!("/dev/dri/by-path/pci-{}-render", info.device_pci_bus_id)).ok())
		.flatten()
		.and_then(|node| node.into_os_string().into_string().ok());
	Ok(Raster::Gpu { render_node, display })
}

/// Check that the X server in `$DISPLAY` runs on the studio's GPU, `info`.
/// On X11, CEF allocates shared textures on the device the X server hands out
/// over DRI3, whatever `--render-node-override` says, and another GPU's
/// tiling comes out scrambled on the studio's.
#[cfg(target_os = "linux")]
fn x11_gpu(info: &wgpu::AdapterInfo) -> Result<(), String> {
	use std::os::unix::fs::MetadataExt;

	use x11rb::connection::{Connection, RequestConnection};
	use x11rb::protocol::dri3::{self, ConnectionExt};

	let (connection, screen) =
		x11rb::connect(None).map_err(|err| format!("cannot connect to the X display: {err}"))?;
	let extension = connection
		.extension_information(dri3::X11_EXTENSION_NAME)
		.map_err(|err| format!("the X display: {err}"))?;
	if extension.is_none() {
		return Err("the X server offers no DRI3 device to share textures on".into());
	}
	let root = connection.setup().roots[screen].root;
	let device = connection
		.dri3_open(root, x11rb::NONE)
		.map_err(|err| format!("the X display: {err}"))?
		.reply()
		.map_err(|err| format!("the X server's DRI3 device: {err}"))?
		.device_fd;
	let rdev = std::fs::File::from(device)
		.metadata()
		.map_err(|err| format!("the X server's DRI3 device: {err}"))?
		.rdev();
	let sys = format!("/sys/dev/char/{}:{}/device", rustix::fs::major(rdev), rustix::fs::minor(rdev));
	let bus = std::fs::canonicalize(&sys)
		.ok()
		.and_then(|path| Some(path.file_name()?.to_str()?.to_owned()))
		.ok_or_else(|| format!("cannot tell which GPU the X server runs on from {sys}"))?;
	if info.device_pci_bus_id.is_empty() {
		return Err(format!(
			"cannot tell whether the studio's GPU, {}, is the one the X server runs on ({bus})",
			info.name
		));
	}
	if bus != info.device_pci_bus_id {
		return Err(format!(
			"the X server runs on another GPU ({bus}) than the studio's, {} ({}), and under X11 pages paint only on the X server's",
			info.name, info.device_pci_bus_id
		));
	}
	Ok(())
}

/// How a page's pictures reach the compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Paint {
	/// A shared texture, copied on the GPU.
	Gpu,
	/// Pixels in memory, uploaded every frame, and why: the fallback when the
	/// studio's device, the session, or the page's GPU process cannot share
	/// textures.
	Cpu(Arc<str>),
}

/// A running browser source: how it paints, and the way to interact with it.
#[derive(Clone, Default)]
pub struct Page {
	state: Arc<Mutex<PageState>>,
}

#[derive(Default)]
struct PageState {
	/// The queue to the helper's socket, while the page is open.
	#[cfg(target_os = "linux")]
	commands: Option<outbox::Commands>,
	/// The page's number in the helper.
	#[cfg(target_os = "linux")]
	id: protocol::Id,
	/// Whether input is being dropped because the backlog is full.
	#[cfg(target_os = "linux")]
	backlogged: bool,
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
	/// Queue `input`, which past the backlog drops, a release aside.
	fn send(&mut self, input: Input) {
		self.flush();
		let Some(commands) = &self.commands else { return };
		match commands.input(self.id, input) {
			Ok(()) => self.backlogged = false,
			Err(outbox::Refused::Full) => {
				if !std::mem::replace(&mut self.backlogged, true) {
					tracing::warn!("a browser source's helper is not reading input; dropping it");
				}
			}
			Err(outbox::Refused::Gone) => self.disconnect(),
		}
	}

	/// Queue a new frame rate.
	fn flush(&mut self) {
		let Some(commands) = &self.commands else { return };
		if self.retime {
			if !commands.rate(self.id, self.framerate) {
				return self.disconnect();
			}
			self.retime = false;
		}
	}

	/// The page closed, or the helper is gone, which its source reports.
	fn disconnect(&mut self) {
		self.commands = None;
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
		self.state.lock().unwrap().paint.clone()
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
	/// Copy the page's selection to the clipboard, through
	/// [`Helper::copied`] unless the page paints through an X display.
	Copy,
	/// Cut the page's selection to the clipboard, through
	/// [`Helper::copied`] unless the page paints through an X display.
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
/// paints, and a copy source for drawing a popup over the page.
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
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
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

	/// A new canvas frame rate reaches the page once, the newest of those
	/// waiting, even past the backlog.
	#[test]
	fn a_new_frame_rate_reaches_the_page() {
		let commands = outbox::Commands::default();
		let page = Page::default();
		page.state.lock().unwrap().commands = Some(commands.clone());
		page.set_framerate(30);
		page.set_framerate(30);
		assert!(matches!(commands.take()[..], [protocol::Command::FrameRate(_, 30)]));
		assert!(commands.take().is_empty(), "an unchanged rate is not sent again");

		while !page.state.lock().unwrap().backlogged {
			page.send(Input::Text("x".into()));
		}
		page.set_framerate(60);
		page.set_framerate(90);
		let rates: Vec<_> = commands
			.take()
			.into_iter()
			.filter_map(|command| match command {
				protocol::Command::FrameRate(_, rate) => Some(rate),
				_ => None,
			})
			.collect();
		assert_eq!(rates, [90]);
	}
}
