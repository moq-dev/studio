//! The helper process: CEF's browser process, rendering one page offscreen
//! and handing each picture and sound to the studio over the socket on its
//! stdin.

use std::cell::RefCell;
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};

use cef::*;

use super::protocol::{Channel, Command, Event, Format, Open, Plane, Texture};
use super::{Button, Input, Modifiers};

/// Run one of CEF's subprocesses (renderer, GPU, utility, zygote).
pub(super) fn subprocess() -> ExitCode {
	let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
	let args = args::Args::new();
	let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
	ExitCode::from(code.clamp(0, 255) as u8)
}

/// Run the helper until the studio closes the socket or the page fails.
pub(super) fn run() -> ExitCode {
	match serve() {
		Ok(()) => ExitCode::SUCCESS,
		Err(err) => {
			eprintln!("browser source helper: {err}");
			ExitCode::FAILURE
		}
	}
}

/// What every handler shares.
struct Shared {
	open: Open,
	/// Sends from the UI thread (pictures) and CEF's audio thread (sound).
	writer: Mutex<Channel>,
	/// Each [`Command::Painted`], which lets a painted picture go back to CEF.
	painted: Mutex<mpsc::Receiver<()>>,
}

impl Shared {
	fn send(&self, event: &Event, fds: &[BorrowedFd<'_>]) -> bool {
		self.writer.lock().unwrap().send(event, fds).is_ok()
	}

	/// Send a picture and wait until the studio has copied it.
	fn paint(&self, event: &Event, fds: &[BorrowedFd<'_>]) {
		if self.send(event, fds) {
			// Fails only once the studio is gone, and the reader is quitting.
			let _ = self.painted.lock().unwrap().recv();
		}
	}

	fn fail(&self, reason: String) {
		self.send(&Event::Failed(reason), &[]);
	}
}

thread_local! {
	/// The page, on CEF's UI thread, where input is delivered.
	static BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) };
}

fn serve() -> Result<(), String> {
	// The studio hands over the socket as stdin. Take it, and leave CEF's
	// subprocesses a harmless stdin instead.
	let socket = rustix::io::fcntl_dupfd_cloexec(rustix::stdio::stdin(), 3).map_err(|err| format!("stdin: {err}"))?;
	let null = std::fs::File::open("/dev/null").map_err(|err| format!("/dev/null: {err}"))?;
	rustix::stdio::dup2_stdin(&null).map_err(|err| format!("stdin: {err}"))?;
	let reader = Channel::new(UnixStream::from(socket));
	let writer = reader.try_clone().map_err(|err| format!("socket: {err}"))?;

	let open = match reader.recv::<Command>() {
		Ok(Some((Command::Open(open), _))) => open,
		Ok(Some((command, _))) => return Err(format!("expected Open, got {command:?}")),
		Ok(None) => return Ok(()),
		Err(err) => return Err(format!("socket: {err}")),
	};

	let (painted_tx, painted) = mpsc::channel();
	let shared = Arc::new(Shared {
		open,
		writer: Mutex::new(writer),
		painted: Mutex::new(painted),
	});
	// The studio shows why, rather than only that the helper exited.
	page(&shared, reader, painted_tx).inspect_err(|reason| shared.fail(reason.clone()))
}

/// Open the page `shared` describes and run CEF until it closes.
fn page(shared: &Arc<Shared>, reader: Channel, painted_tx: mpsc::Sender<()>) -> Result<(), String> {
	let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
	let args = args::Args::new();
	let mut app = HelperApp::new(shared.open.render_node.clone());
	let settings = Settings {
		windowless_rendering_enabled: 1,
		root_cache_path: shared.open.profile.as_str().into(),
		log_severity: LogSeverity::WARNING,
		// Transparent, so an overlay shows what is under it.
		background_color: 0,
		..Default::default()
	};
	if initialize(
		Some(args.as_main_args()),
		Some(&settings),
		Some(&mut app),
		std::ptr::null_mut(),
	) != 1
	{
		return Err("CEF failed to initialize".into());
	}

	let window = WindowInfo {
		windowless_rendering_enabled: 1,
		shared_texture_enabled: shared.open.accelerated.into(),
		..Default::default()
	};
	let browser_settings = BrowserSettings {
		windowless_frame_rate: shared.open.framerate as _,
		background_color: 0,
		..Default::default()
	};
	let mut client = HelperClient::new(
		HelperRender::new(shared.clone()),
		HelperAudio::new(shared.clone()),
		HelperLoad::new(shared.clone()),
		HelperRequest::new(shared.clone()),
		HelperLife::new(),
	);
	let url = CefString::from(shared.open.url.as_str());
	let browser = browser_host_create_browser_sync(
		Some(&window),
		Some(&mut client),
		Some(&url),
		Some(&browser_settings),
		None,
		None,
	)
	.ok_or("CEF failed to create the page")?;
	BROWSER.with_borrow_mut(|slot| *slot = Some(browser));

	std::thread::Builder::new()
		.name("browser-commands".into())
		.spawn(move || commands(reader, painted_tx))
		.map_err(|err| format!("cannot start the helper's command thread: {err}"))?;

	run_message_loop();
	BROWSER.with_borrow_mut(|slot| *slot = None);
	shutdown();
	Ok(())
}

/// Read the studio's commands until it closes the socket, then close the page,
/// which ends the message loop.
fn commands(reader: Channel, painted: mpsc::Sender<()>) {
	loop {
		match reader.recv::<Command>() {
			Ok(Some((Command::Painted, _))) => {
				let _ = painted.send(());
			}
			Ok(Some((Command::Input(input), _))) => post(move || deliver(input)),
			Ok(Some((Command::FrameRate(rate), _))) => post(move || retime(rate)),
			Ok(Some((Command::Open(_), _))) | Ok(None) | Err(_) => break,
		}
	}
	// Unblocks a paint waiting on the studio.
	drop(painted);
	post(|| {
		BROWSER.with_borrow(|browser| {
			if let Some(host) = browser.as_ref().and_then(|browser| browser.host()) {
				host.close_browser(1);
			}
		})
	});
}

/// Paint the page `rate` times a second. Runs on the UI thread.
fn retime(rate: u32) {
	BROWSER.with_borrow(|browser| {
		if let Some(host) = browser.as_ref().and_then(|browser| browser.host()) {
			host.set_windowless_frame_rate(rate.try_into().unwrap_or(i32::MAX));
		}
	})
}

/// Hand one input event to the page. Runs on the UI thread.
fn deliver(input: Input) {
	BROWSER.with_borrow(|browser| {
		let Some(host) = browser.as_ref().and_then(|browser| browser.host()) else {
			return;
		};
		let mouse = |x: i32, y: i32, modifiers: Modifiers| MouseEvent {
			x,
			y,
			modifiers: flags(modifiers),
		};
		match input {
			Input::Move { x, y, modifiers } => host.send_mouse_move_event(Some(&mouse(x, y, modifiers)), 0),
			Input::Leave { modifiers } => host.send_mouse_move_event(Some(&mouse(0, 0, modifiers)), 1),
			Input::Button {
				x,
				y,
				button,
				down,
				clicks,
				modifiers,
			} => {
				let button = match button {
					Button::Left => MouseButtonType::LEFT,
					Button::Middle => MouseButtonType::MIDDLE,
					Button::Right => MouseButtonType::RIGHT,
				};
				host.send_mouse_click_event(
					Some(&mouse(x, y, modifiers)),
					button,
					(!down).into(),
					clicks.min(3) as _,
				);
			}
			Input::Wheel {
				x,
				y,
				dx,
				dy,
				modifiers,
			} => host.send_mouse_wheel_event(Some(&mouse(x, y, modifiers)), dx, dy),
			Input::Key { code, down, modifiers } => {
				let event = KeyEvent {
					type_: if down {
						KeyEventType::RAWKEYDOWN
					} else {
						KeyEventType::KEYUP
					},
					modifiers: flags(modifiers),
					windows_key_code: code,
					..Default::default()
				};
				host.send_key_event(Some(&event));
			}
			Input::Text(text) => {
				for unit in text.encode_utf16() {
					let event = KeyEvent {
						type_: KeyEventType::CHAR,
						windows_key_code: unit.into(),
						character: unit,
						unmodified_character: unit,
						..Default::default()
					};
					host.send_key_event(Some(&event));
				}
			}
			// The focused frame holds the selection, which may be in an
			// iframe.
			Input::Copy => {
				if let Some(frame) = browser.as_ref().and_then(|browser| browser.focused_frame()) {
					frame.copy();
				}
			}
			Input::Cut => {
				if let Some(frame) = browser.as_ref().and_then(|browser| browser.focused_frame()) {
					frame.cut();
				}
			}
			Input::Focus(focused) => host.set_focus(focused.into()),
		}
	});
}

fn flags(modifiers: Modifiers) -> u32 {
	use sys::cef_event_flags_t as F;
	[
		(modifiers.shift, F::EVENTFLAG_SHIFT_DOWN),
		(modifiers.ctrl, F::EVENTFLAG_CONTROL_DOWN),
		(modifiers.alt, F::EVENTFLAG_ALT_DOWN),
		(modifiers.meta, F::EVENTFLAG_COMMAND_DOWN),
		(modifiers.left, F::EVENTFLAG_LEFT_MOUSE_BUTTON),
		(modifiers.middle, F::EVENTFLAG_MIDDLE_MOUSE_BUTTON),
		(modifiers.right, F::EVENTFLAG_RIGHT_MOUSE_BUTTON),
	]
	.into_iter()
	.filter(|(held, _)| *held)
	.fold(0, |flags, (_, flag)| flags | flag.0)
}

/// Run `job` on CEF's UI thread.
fn post(job: impl FnOnce() + Send + 'static) {
	let job: Job = Arc::new(Mutex::new(Some(Box::new(job))));
	let mut task = HelperTask::new(job);
	post_task(ThreadId::UI, Some(&mut task));
}

type Job = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

wrap_task! {
	struct HelperTask {
		job: Job,
	}

	impl Task {
		fn execute(&self) {
			if let Some(job) = self.job.lock().unwrap().take() {
				job();
			}
		}
	}
}

wrap_app! {
	struct HelperApp {
		render_node: Option<String>,
	}

	impl App {
		fn on_before_command_line_processing(
			&self,
			_process_type: Option<&CefString>,
			command_line: Option<&mut CommandLine>,
		) {
			let Some(command_line) = command_line else {
				return;
			};
			// An alert box plays its sound without anyone clicking the page.
			command_line.append_switch_with_value(
				Some(&"autoplay-policy".into()),
				Some(&"no-user-gesture-required".into()),
			);
			// Paint on the studio's GPU, which is the only one it imports from.
			if let Some(node) = &self.render_node {
				command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&node.as_str().into()));
			}
		}
	}
}

wrap_client! {
	struct HelperClient {
		render: RenderHandler,
		audio: AudioHandler,
		load: LoadHandler,
		request: RequestHandler,
		life: LifeSpanHandler,
	}

	impl Client {
		fn render_handler(&self) -> Option<RenderHandler> {
			Some(self.render.clone())
		}

		fn audio_handler(&self) -> Option<AudioHandler> {
			Some(self.audio.clone())
		}

		fn load_handler(&self) -> Option<LoadHandler> {
			Some(self.load.clone())
		}

		fn request_handler(&self) -> Option<RequestHandler> {
			Some(self.request.clone())
		}

		fn life_span_handler(&self) -> Option<LifeSpanHandler> {
			Some(self.life.clone())
		}
	}
}

wrap_render_handler! {
	struct HelperRender {
		shared: Arc<Shared>,
	}

	impl RenderHandler {
		fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
			if let Some(rect) = rect {
				rect.width = self.shared.open.width as _;
				rect.height = self.shared.open.height as _;
			}
		}

		fn on_paint(
			&self,
			_browser: Option<&mut Browser>,
			type_: PaintElementType,
			_dirty_rects: Option<&[Rect]>,
			buffer: *const u8,
			width: ::std::os::raw::c_int,
			height: ::std::os::raw::c_int,
		) {
			if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
				return;
			}
			let len = width as usize * height as usize * 4;
			// SAFETY: CEF hands over `width * height` BGRA pixels, valid for
			// this call.
			let data = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
			let event = Event::Pixels {
				width: width as u32,
				height: height as u32,
				data,
			};
			self.shared.paint(&event, &[]);
		}

		fn on_accelerated_paint(
			&self,
			_browser: Option<&mut Browser>,
			type_: PaintElementType,
			_dirty_rects: Option<&[Rect]>,
			info: Option<&AcceleratedPaintInfo>,
		) {
			let Some(info) = info else { return };
			if type_ != PaintElementType::VIEW {
				return;
			}
			let format = match info.format {
				ColorType::RGBA_8888 => Format::Rgba,
				ColorType::BGRA_8888 => Format::Bgra,
				other => {
					self.shared.fail(format!("CEF painted an unknown format {:?}", other.as_ref()));
					return;
				}
			};
			let count = (info.plane_count.max(0) as usize).min(info.planes.len());
			let planes = &info.planes[..count];
			let texture = Texture {
				width: info.extra.coded_size.width as u32,
				height: info.extra.coded_size.height as u32,
				format,
				modifier: info.modifier,
				planes: planes
					.iter()
					.map(|plane| Plane {
						stride: plane.stride,
						offset: plane.offset,
						size: plane.size,
					})
					.collect(),
			};
			// SAFETY: CEF owns these descriptors and keeps them open for this
			// call, which outlasts the send that duplicates them.
			let fds: Vec<_> = planes.iter().map(|plane| unsafe { BorrowedFd::borrow_raw(plane.fd) }).collect();
			self.shared.paint(&Event::Texture(texture), &fds);
		}
	}
}

wrap_audio_handler! {
	struct HelperAudio {
		shared: Arc<Shared>,
	}

	impl AudioHandler {
		fn audio_parameters(
			&self,
			_browser: Option<&mut Browser>,
			params: Option<&mut AudioParameters>,
		) -> ::std::os::raw::c_int {
			let Some(params) = params else { return 0 };
			params.channel_layout = match self.shared.open.channels {
				1 => ChannelLayout::LAYOUT_MONO,
				2 => ChannelLayout::LAYOUT_STEREO,
				_ => return 0,
			};
			params.sample_rate = self.shared.open.sample_rate as _;
			1
		}

		fn on_audio_stream_started(
			&self,
			_browser: Option<&mut Browser>,
			params: Option<&AudioParameters>,
			channels: ::std::os::raw::c_int,
		) {
			let rate = params.map(|params| params.sample_rate);
			if channels as u32 != self.shared.open.channels || rate != Some(self.shared.open.sample_rate as _) {
				self.shared.fail(format!(
					"the page's sound arrived as {channels} channels at {rate:?} Hz, not the bus's {} at {} Hz",
					self.shared.open.channels, self.shared.open.sample_rate
				));
			}
		}

		fn on_audio_stream_packet(
			&self,
			_browser: Option<&mut Browser>,
			data: *mut *const f32,
			frames: ::std::os::raw::c_int,
			_pts: i64,
		) {
			if data.is_null() || frames <= 0 {
				return;
			}
			let channels = self.shared.open.channels as usize;
			let frames = frames as usize;
			// SAFETY: CEF hands over one plane of `frames` samples per
			// channel it started the stream with, which is the bus's count.
			let planes: Vec<&[f32]> = (0..channels)
				.map(|channel| unsafe { std::slice::from_raw_parts(*data.add(channel), frames) })
				.collect();
			let samples = (0..frames)
				.flat_map(|frame| planes.iter().map(move |plane| plane[frame]))
				.collect();
			self.shared.send(&Event::Audio(samples), &[]);
		}

		fn on_audio_stream_error(&self, _browser: Option<&mut Browser>, message: Option<&CefString>) {
			let message = message.map(|message| message.to_string()).unwrap_or_default();
			self.shared.fail(format!("the page's sound failed: {message}"));
		}
	}
}

wrap_load_handler! {
	struct HelperLoad {
		shared: Arc<Shared>,
	}

	impl LoadHandler {
		fn on_load_error(
			&self,
			_browser: Option<&mut Browser>,
			frame: Option<&mut Frame>,
			error_code: Errorcode,
			error_text: Option<&CefString>,
			failed_url: Option<&CefString>,
		) {
			// A subresource or an iframe failing leaves the page standing, and
			// ABORTED is a navigation replaced by another.
			if frame.is_none_or(|frame| frame.is_main() == 0) || error_code == Errorcode::ABORTED {
				return;
			}
			let url = failed_url.map(|url| url.to_string()).unwrap_or_default();
			let text = error_text.map(|text| text.to_string()).unwrap_or_default();
			self.shared.fail(format!("cannot load {url}: {text}"));
		}
	}
}

wrap_request_handler! {
	struct HelperRequest {
		shared: Arc<Shared>,
	}

	impl RequestHandler {
		fn on_render_process_terminated(
			&self,
			_browser: Option<&mut Browser>,
			_status: TerminationStatus,
			error_code: ::std::os::raw::c_int,
			error_string: Option<&CefString>,
		) {
			let reason = error_string.map(|reason| reason.to_string()).unwrap_or_default();
			self.shared.fail(format!("the page's renderer stopped ({error_code}): {reason}"));
		}
	}
}

wrap_life_span_handler! {
	struct HelperLife;

	impl LifeSpanHandler {
		// A source shows one page, and nothing would show a popup, so
		// `window.open` and `target="_blank"` open nothing. That also keeps
		// the page the only browser, whose closing ends the helper.
		fn on_before_popup(
			&self,
			_browser: Option<&mut Browser>,
			_frame: Option<&mut Frame>,
			_popup_id: ::std::os::raw::c_int,
			_target_url: Option<&CefString>,
			_target_frame_name: Option<&CefString>,
			_target_disposition: WindowOpenDisposition,
			_user_gesture: ::std::os::raw::c_int,
			_popup_features: Option<&PopupFeatures>,
			_window_info: Option<&mut WindowInfo>,
			_client: Option<&mut Option<Client>>,
			_settings: Option<&mut BrowserSettings>,
			_extra_info: Option<&mut Option<DictionaryValue>>,
			_no_javascript_access: Option<&mut ::std::os::raw::c_int>,
		) -> ::std::os::raw::c_int {
			1
		}

		fn on_before_close(&self, _browser: Option<&mut Browser>) {
			quit_message_loop();
		}
	}
}
