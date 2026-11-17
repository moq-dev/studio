//! Where the pointer is while the system drags files over the window.
//!
//! winit 0.30 names the files a drag holds but not where it is: the drag
//! source holds the pointer, so the window hears no motion until the drop,
//! and the drop says no place either. So the window asks the windowing
//! system each frame of a drag. Wayland has no drops in winit 0.30, so it is
//! never asked there.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Asks the windowing system where the pointer is over the window.
#[derive(Default)]
pub struct Pointer {
	/// The X server, reached the first time it is asked.
	#[cfg(target_os = "linux")]
	x11: Option<x11rb::rust_connection::RustConnection>,
}

impl Pointer {
	/// The pointer over `frame`'s window, in `ctx`'s points, or `None` where
	/// the windowing system cannot say, such as in a test with no window.
	pub fn at(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> Option<egui::Pos2> {
		let handle = frame.window_handle().ok()?.as_raw();
		let at = self.ask(handle, ctx);
		if at.is_none() {
			tracing::debug!(?handle, "cannot tell where the pointer is during a drag");
		}
		at
	}

	#[cfg(target_os = "linux")]
	fn ask(&mut self, handle: RawWindowHandle, ctx: &egui::Context) -> Option<egui::Pos2> {
		use x11rb::protocol::xproto::ConnectionExt as _;

		let window = match handle {
			RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok()?,
			RawWindowHandle::Xcb(handle) => handle.window.get(),
			_ => return None,
		};
		if self.x11.is_none() {
			self.x11 = Some(x11rb::connect(None).ok()?.0);
		}
		let reply = self.x11.as_ref()?.query_pointer(window).ok()?.reply();
		// A connection that failed stays failed; reach the server again next time.
		let Ok(reply) = reply else {
			self.x11 = None;
			return None;
		};
		// Off the window's screen, the position says nothing.
		reply.same_screen.then(|| physical(ctx, reply.win_x.into(), reply.win_y.into()))
	}

	#[cfg(windows)]
	fn ask(&mut self, handle: RawWindowHandle, ctx: &egui::Context) -> Option<egui::Pos2> {
		use windows_sys::Win32::Foundation::POINT;
		use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
		use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

		let RawWindowHandle::Win32(handle) = handle else {
			return None;
		};
		let mut point = POINT { x: 0, y: 0 };
		// SAFETY: `point` outlives both calls, and the handle is the live
		// window's, which this frame draws.
		let found = unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(handle.hwnd.get() as _, &mut point) != 0 };
		found.then(|| physical(ctx, point.x as f32, point.y as f32))
	}

	#[cfg(target_os = "macos")]
	fn ask(&mut self, handle: RawWindowHandle, ctx: &egui::Context) -> Option<egui::Pos2> {
		use objc2_app_kit::NSView;

		let RawWindowHandle::AppKit(handle) = handle else {
			return None;
		};
		// SAFETY: the handle is the live window's content view, which this
		// frame draws on the main thread, where AppKit is asked.
		let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
		let window = view.window()?;
		let point = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
		// AppKit counts up from the bottom unless the view is flipped.
		let y = match view.isFlipped() {
			true => point.y,
			false => view.bounds().size.height - point.y,
		};
		// AppKit's points are the system's; egui's include its zoom.
		let scale = ctx.native_pixels_per_point().unwrap_or(1.0);
		Some(physical(ctx, point.x as f32 * scale, y as f32 * scale))
	}

	#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
	fn ask(&mut self, _handle: RawWindowHandle, _ctx: &egui::Context) -> Option<egui::Pos2> {
		None
	}
}

/// `x` and `y` physical pixels from the window's top left, in egui's points.
fn physical(ctx: &egui::Context, x: f32, y: f32) -> egui::Pos2 {
	egui::pos2(x, y) / ctx.pixels_per_point()
}
