//! The helper process: CEF's browser process, rendering every page offscreen
//! in one profile and handing each picture and sound to the studio over the
//! socket on its stdin.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use cef::*;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use super::outbox::Commands;
use super::protocol::{Area, Channel, Command, Event, Format, Id, Layer, Open, Plane, Raster, Start, Texture};
use super::{Button, Input, Modifiers};

/// What the helper asks of a page's focused frame: its selection, and whether
/// to cut it.
const COPY: &str = "moq-studio-copy";
/// What the frame's renderer answers: the text it copied.
const COPIED: &str = "moq-studio-copied";

/// Run one of CEF's subprocesses (renderer, GPU, utility, zygote).
pub(super) fn subprocess() -> ExitCode {
	let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
	let args = args::Args::new();
	let mut app = RendererApp::new(RendererCopy::new());
	let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
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

/// What every page's handlers share.
struct Shared {
	/// Sends from the UI thread (pictures) and CEF's audio thread (sound).
	writer: Mutex<Channel>,
	/// Each [`Command::Painted`], which lets a painted picture go back to CEF.
	/// Pictures are painted on the UI thread, one at a time, so one answer is
	/// never another page's.
	painted: Mutex<mpsc::Receiver<()>>,
	accelerated: bool,
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
}

/// One page's handlers' view of it.
struct Paged {
	shared: Arc<Shared>,
	id: Id,
	open: Open,
	/// The copies asked of its renderers and not yet answered, so a renderer
	/// sets the studio's clipboard only when asked.
	copies: AtomicUsize,
}

impl Paged {
	fn fail(&self, reason: String) {
		self.shared.send(&Event::Failed(self.id, reason), &[]);
	}
}

thread_local! {
	/// The open pages, on CEF's UI thread, where input is delivered.
	static PAGES: RefCell<HashMap<Id, (Browser, Arc<Paged>)>> = RefCell::new(HashMap::new());
	/// Whether the studio is gone, so the helper quits once its last page
	/// closes.
	static QUITTING: Cell<bool> = const { Cell::new(false) };
}

fn serve() -> Result<(), String> {
	// The studio hands over the socket as stdin. Take it, and leave CEF's
	// subprocesses a harmless stdin instead.
	let socket = rustix::io::fcntl_dupfd_cloexec(rustix::stdio::stdin(), 3).map_err(|err| format!("stdin: {err}"))?;
	let null = std::fs::File::open("/dev/null").map_err(|err| format!("/dev/null: {err}"))?;
	rustix::stdio::dup2_stdin(&null).map_err(|err| format!("stdin: {err}"))?;
	let reader = Channel::new(UnixStream::from(socket));
	let writer = reader.try_clone().map_err(|err| format!("socket: {err}"))?;

	let start = match reader.recv::<Command>() {
		Ok(Some((Command::Start(start), _))) => start,
		Ok(Some((command, _))) => return Err(format!("expected Start, got {command:?}")),
		Ok(None) => return Ok(()),
		Err(err) => return Err(format!("socket: {err}")),
	};

	let (painted_tx, painted) = mpsc::channel();
	let shared = Arc::new(Shared {
		writer: Mutex::new(writer),
		painted: Mutex::new(painted),
		accelerated: matches!(start.raster, Raster::Gpu { .. }),
	});
	// The studio shows why, rather than only that the helper exited.
	browse(&shared, start, reader, painted_tx).inspect_err(|reason| _ = shared.send(&Event::Fatal(reason.clone()), &[]))
}

/// Run CEF on the profile `start` names, opening and closing pages as the
/// studio asks, until it closes the socket and the last page closes.
fn browse(shared: &Arc<Shared>, start: Start, reader: Channel, painted_tx: mpsc::Sender<()>) -> Result<(), String> {
	// A crashed process is reported only once it is gone, and one dumping
	// core is not gone yet: systemd-coredump took 20 to 70 seconds over a
	// renderer, which left its page Starting meanwhile. CEF's subprocesses
	// inherit the limit. 1, not 0: the kernel ignores the limit for a pipe
	// core_pattern (systemd-coredump, Apport) except at 1, where it skips the
	// handler, and a core file needs a page. A socket core_pattern (`@`), or
	// a pipe under a hard limit of 0, which forbids 1, relies instead on the
	// handler honouring the limit, as systemd-coredump does.
	let core = getrlimit(Resource::Core);
	setrlimit(
		Resource::Core,
		Rlimit {
			current: Some(core.maximum.map_or(1, |maximum| maximum.min(1))),
			maximum: core.maximum,
		},
	)
	.map_err(|err| format!("cannot turn off core dumps: {err}"))?;

	let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
	let args = args::Args::new();
	let mut app = HelperApp::new(HelperProcess::new(), start.raster);
	let profile = CefString::from(start.profile.as_str());
	let settings = Settings {
		windowless_rendering_enabled: 1,
		// The global profile, kept on disk: without a cache path CEF keeps
		// everything in memory.
		root_cache_path: profile.clone(),
		cache_path: profile,
		// A login that sets a cookie without an expiry survives a restart, as
		// it would in a browser left open.
		persist_session_cookies: 1,
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
		let notified = sys::cef_resultcode_t::CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED as i32;
		return Err(match get_exit_code() == notified {
			true => format!(
				"another studio is using the browser profile {}; quit it, then reload the source",
				start.profile
			),
			false => "CEF failed to initialize".into(),
		});
	}

	let commands = {
		let shared = shared.clone();
		std::thread::Builder::new()
			.name("browser-commands".into())
			.spawn(move || commands(shared, reader, painted_tx))
	};
	if let Err(err) = commands {
		shutdown();
		return Err(format!("cannot start the helper's command thread: {err}"));
	}

	run_message_loop();
	PAGES.with_borrow_mut(HashMap::clear);
	// Saves the profile, such as cookies not yet written.
	shutdown();
	Ok(())
}

/// Read the studio's commands until it closes the socket, then close every
/// page, which ends the message loop.
///
/// What the UI thread has yet to apply waits coalesced, as in the studio, with
/// one task at a time posted to apply it, so a stalled UI thread holds what
/// the pages open need rather than a task per command. Input past the
/// backlog drops, as for a hung page.
fn commands(shared: Arc<Shared>, reader: Channel, painted: mpsc::Sender<()>) {
	let pending = Commands::default();
	let posted = Arc::new(AtomicBool::new(false));
	let schedule = || {
		if !posted.swap(true, Ordering::AcqRel) {
			let (shared, pending, posted) = (shared.clone(), pending.clone(), posted.clone());
			post(move || {
				// Cleared first, so anything queued from here on posts again.
				posted.store(false, Ordering::Release);
				apply(&shared, pending.take());
			});
		}
	};
	loop {
		match reader.recv::<Command>() {
			Ok(Some((Command::Painted, _))) => {
				let _ = painted.send(());
				continue;
			}
			Ok(Some((Command::Open(id, open), _))) => pending.open(id, open),
			Ok(Some((Command::Close(id), _))) => pending.close(id),
			Ok(Some((Command::Input(id, input), _))) => _ = pending.input(id, input),
			Ok(Some((Command::FrameRate(id, rate), _))) => _ = pending.rate(id, rate),
			Ok(Some((Command::Start(_), _))) | Ok(None) | Err(_) => break,
		}
		schedule();
	}
	// Unblocks a paint waiting on the studio.
	drop(painted);
	post(move || {
		apply(&shared, pending.take());
		QUITTING.set(true);
		let ids: Vec<Id> = PAGES.with_borrow(|pages| pages.keys().copied().collect());
		if ids.is_empty() {
			quit_message_loop();
		}
		for id in ids {
			close(id);
		}
	});
}

/// Apply the studio's commands. Runs on the UI thread.
fn apply(shared: &Arc<Shared>, batch: Vec<Command>) {
	for command in batch {
		match command {
			Command::Open(id, open) => self::open(shared.clone(), id, open),
			Command::Close(id) => close(id),
			Command::Input(id, input) => deliver(id, input),
			Command::FrameRate(id, rate) => retime(id, rate),
			Command::Start(_) | Command::Painted => unreachable!("never pending"),
		}
	}
}

/// Open page `id`. Runs on the UI thread.
fn open(shared: Arc<Shared>, id: Id, open: Open) {
	let window = WindowInfo {
		windowless_rendering_enabled: 1,
		shared_texture_enabled: shared.accelerated.into(),
		..Default::default()
	};
	let browser_settings = BrowserSettings {
		windowless_frame_rate: open.framerate as _,
		background_color: 0,
		..Default::default()
	};
	let url = CefString::from(open.url.as_str());
	let paged = Arc::new(Paged {
		shared,
		id,
		open,
		copies: AtomicUsize::new(0),
	});
	let mut client = HelperClient::new(
		paged.clone(),
		HelperRender::new(paged.clone()),
		HelperAudio::new(paged.clone()),
		HelperLoad::new(paged.clone()),
		HelperRequest::new(paged.clone()),
		HelperLife::new(paged.clone()),
	);
	match browser_host_create_browser_sync(
		Some(&window),
		Some(&mut client),
		Some(&url),
		Some(&browser_settings),
		None,
		None,
	) {
		Some(browser) => _ = PAGES.with_borrow_mut(|pages| pages.insert(id, (browser, paged))),
		None => paged.fail("CEF failed to create the page".into()),
	}
}

/// Close page `id`, which leaves [`PAGES`] once CEF has closed it. Runs on
/// the UI thread.
fn close(id: Id) {
	host(id, |host| host.close_browser(1));
}

/// Run `f` on page `id`'s host, if the page is open. Runs on the UI thread.
fn host(id: Id, f: impl FnOnce(BrowserHost)) {
	if let Some(host) = PAGES.with_borrow(|pages| pages.get(&id).and_then(|(browser, _)| browser.host())) {
		f(host);
	}
}

/// Paint page `id` `rate` times a second. Runs on the UI thread.
fn retime(id: Id, rate: u32) {
	host(id, |host| {
		host.set_windowless_frame_rate(rate.try_into().unwrap_or(i32::MAX))
	});
}

/// Hand one input event to page `id`. Runs on the UI thread.
fn deliver(id: Id, input: Input) {
	let Some((browser, page)) = PAGES.with_borrow(|pages| pages.get(&id).cloned()) else {
		return;
	};
	let Some(host) = browser.host() else {
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
		Input::Copy => copy(&browser, &page, false),
		Input::Cut => copy(&browser, &page, true),
		Input::Focus(focused) => host.set_focus(focused.into()),
	}
}

/// Ask the page's focused frame, which may be an iframe, for its selection,
/// and to cut it if `cut`. Its renderer answers, and the studio puts what it
/// copied on the desktop's clipboard: CEF's own reaches that only through a
/// display, which the CPU path never opens.
fn copy(browser: &Browser, page: &Paged, cut: bool) {
	let (Some(frame), Some(mut message)) = (browser.focused_frame(), process_message_create(Some(&COPY.into()))) else {
		return;
	};
	if let Some(args) = message.argument_list() {
		args.set_bool(0, cut.into());
	}
	page.copies.fetch_add(1, Ordering::Relaxed);
	frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
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
		process: BrowserProcessHandler,
		raster: Raster,
	}

	impl App {
		fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
			Some(self.process.clone())
		}

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
			match &self.raster {
				// Paint on the studio's GPU, which is the only one it imports
				// from.
				Raster::Gpu { render_node: Some(node) } => {
					command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&node.as_str().into()));
				}
				Raster::Gpu { render_node: None } => {}
				// The pictures come back in memory anyway. Left to find a GPU,
				// Chromium walks its fallbacks, and one can hang until its
				// watchdog kills it seconds later, delaying the first paint.
				// WebGL then needs SwiftShader, which Chromium runs only when
				// told to, since it JITs page shaders in the GPU process. A
				// browser source runs the pages its user picked, not the web.
				//
				// Painting offscreen in software needs no display either,
				// so none is opened, which lets the helper run with no X
				// server, as under Wayland without XWayland.
				Raster::Cpu => {
					command_line.append_switch(Some(&"disable-gpu".into()));
					command_line.append_switch(Some(&"enable-unsafe-swiftshader".into()));
					command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"headless".into()));
				}
			}
		}
	}
}

wrap_browser_process_handler! {
	struct HelperProcess;

	impl BrowserProcessHandler {
		// A second studio starting on this profile is refused, and told so
		// by its own failure; this one opens no window for it.
		fn on_already_running_app_relaunch(
			&self,
			_command_line: Option<&mut CommandLine>,
			_current_directory: Option<&CefString>,
		) -> ::std::os::raw::c_int {
			1
		}
	}
}

wrap_app! {
	struct RendererApp {
		render: RenderProcessHandler,
	}

	impl App {
		fn render_process_handler(&self) -> Option<RenderProcessHandler> {
			Some(self.render.clone())
		}
	}
}

wrap_render_process_handler! {
	struct RendererCopy;

	impl RenderProcessHandler {
		fn on_process_message_received(
			&self,
			_browser: Option<&mut Browser>,
			frame: Option<&mut Frame>,
			source_process: ProcessId,
			message: Option<&mut ProcessMessage>,
		) -> ::std::os::raw::c_int {
			let (Some(frame), Some(message)) = (frame, message) else {
				return 0;
			};
			if source_process != ProcessId::BROWSER || CefString::from(&message.name()).to_string() != COPY {
				return 0;
			}
			let cut = message.argument_list().is_some_and(|args| args.bool(0) != 0);
			let copied = Arc::new(Mutex::new(String::new()));
			frame.visit_dom(Some(&mut SelectionReader::new(copied.clone())));
			let Some(mut answer) = process_message_create(Some(&COPIED.into())) else {
				return 1;
			};
			if let Some(args) = answer.argument_list() {
				args.set_string(0, Some(&copied.lock().unwrap().as_str().into()));
			}
			frame.send_process_message(ProcessId::BROWSER, Some(&mut answer));
			// After reading the selection, which cutting removes.
			if cut {
				frame.cut();
			}
			1
		}
	}
}

wrap_domvisitor! {
	struct SelectionReader {
		text: Arc<Mutex<String>>,
	}

	impl Domvisitor {
		fn visit(&self, document: Option<&mut Domdocument>) {
			let Some(document) = document else { return };
			// A browser never copies a password field's text.
			let password = document.focused_node().is_some_and(|node| {
				node.is_form_control_element() != 0
					&& node.form_control_element_type() == DomFormControlType::INPUT_PASSWORD
			});
			if !password {
				*self.text.lock().unwrap() = CefString::from(&document.selection_as_text()).to_string();
			}
		}
	}
}

wrap_client! {
	struct HelperClient {
		page: Arc<Paged>,
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

		fn on_process_message_received(
			&self,
			_browser: Option<&mut Browser>,
			_frame: Option<&mut Frame>,
			source_process: ProcessId,
			message: Option<&mut ProcessMessage>,
		) -> ::std::os::raw::c_int {
			let Some(message) = message else { return 0 };
			if source_process != ProcessId::RENDERER || CefString::from(&message.name()).to_string() != COPIED {
				return 0;
			}
			// One the helper never asked for is dropped.
			let asked = self.page.copies.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |copies| copies.checked_sub(1));
			let text = message.argument_list().map(|args| CefString::from(&args.string(0)).to_string());
			if let (Ok(_), Some(text)) = (asked, text)
				&& !text.is_empty()
			{
				self.page.shared.send(&Event::Copied(self.page.id, text), &[]);
			}
			1
		}
	}
}

wrap_render_handler! {
	struct HelperRender {
		page: Arc<Paged>,
	}

	impl RenderHandler {
		fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
			if let Some(rect) = rect {
				rect.width = self.page.open.width as _;
				rect.height = self.page.open.height as _;
			}
		}

		// The studio draws a popup widget over the page, and CEF hands input
		// over it to it, so the studio sends input as for the page alone.
		fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
			if show == 0 {
				self.page.shared.send(&Event::Popup(self.page.id, None), &[]);
			}
		}

		fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
			let Some(rect) = rect else { return };
			let area = Area {
				x: rect.x,
				y: rect.y,
				width: rect.width.max(0) as u32,
				height: rect.height.max(0) as u32,
			};
			self.page.shared.send(&Event::Popup(self.page.id, Some(area)), &[]);
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
			let Some(layer) = layer(type_) else { return };
			if buffer.is_null() || width <= 0 || height <= 0 {
				return;
			}
			let len = width as usize * height as usize * 4;
			// SAFETY: CEF hands over `width * height` BGRA pixels, valid for
			// this call.
			let data = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
			let event = Event::Pixels {
				page: self.page.id,
				layer,
				width: width as u32,
				height: height as u32,
				data,
			};
			self.page.shared.paint(&event, &[]);
		}

		fn on_accelerated_paint(
			&self,
			_browser: Option<&mut Browser>,
			type_: PaintElementType,
			_dirty_rects: Option<&[Rect]>,
			info: Option<&AcceleratedPaintInfo>,
		) {
			let (Some(info), Some(layer)) = (info, layer(type_)) else {
				return;
			};
			let format = match info.format {
				ColorType::RGBA_8888 => Format::Rgba,
				ColorType::BGRA_8888 => Format::Bgra,
				other => {
					self.page.fail(format!("CEF painted an unknown format {:?}", other.as_ref()));
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
			self.page.shared.paint(&Event::Texture(self.page.id, layer, texture), &fds);
		}
	}
}

/// The layer CEF painted, if it is one the studio shows.
fn layer(type_: PaintElementType) -> Option<Layer> {
	if type_ == PaintElementType::VIEW {
		Some(Layer::View)
	} else if type_ == PaintElementType::POPUP {
		Some(Layer::Popup)
	} else {
		None
	}
}

wrap_audio_handler! {
	struct HelperAudio {
		page: Arc<Paged>,
	}

	impl AudioHandler {
		fn audio_parameters(
			&self,
			_browser: Option<&mut Browser>,
			params: Option<&mut AudioParameters>,
		) -> ::std::os::raw::c_int {
			let Some(params) = params else { return 0 };
			params.channel_layout = match self.page.open.channels {
				1 => ChannelLayout::LAYOUT_MONO,
				2 => ChannelLayout::LAYOUT_STEREO,
				_ => return 0,
			};
			params.sample_rate = self.page.open.sample_rate as _;
			1
		}

		fn on_audio_stream_started(
			&self,
			_browser: Option<&mut Browser>,
			params: Option<&AudioParameters>,
			channels: ::std::os::raw::c_int,
		) {
			let rate = params.map(|params| params.sample_rate);
			if channels as u32 != self.page.open.channels || rate != Some(self.page.open.sample_rate as _) {
				self.page.fail(format!(
					"the page's sound arrived as {channels} channels at {rate:?} Hz, not the bus's {} at {} Hz",
					self.page.open.channels, self.page.open.sample_rate
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
			let channels = self.page.open.channels as usize;
			let frames = frames as usize;
			// SAFETY: CEF hands over one plane of `frames` samples per
			// channel it started the stream with, which is the bus's count.
			let planes: Vec<&[f32]> = (0..channels)
				.map(|channel| unsafe { std::slice::from_raw_parts(*data.add(channel), frames) })
				.collect();
			let samples = (0..frames)
				.flat_map(|frame| planes.iter().map(move |plane| plane[frame]))
				.collect();
			self.page.shared.send(&Event::Audio(self.page.id, samples), &[]);
		}

		fn on_audio_stream_error(&self, _browser: Option<&mut Browser>, message: Option<&CefString>) {
			let message = message.map(|message| message.to_string()).unwrap_or_default();
			self.page.fail(format!("the page's sound failed: {message}"));
		}
	}
}

wrap_load_handler! {
	struct HelperLoad {
		page: Arc<Paged>,
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
			self.page.fail(format!("cannot load {url}: {text}"));
		}
	}
}

wrap_request_handler! {
	struct HelperRequest {
		page: Arc<Paged>,
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
			self.page.fail(format!("the page's renderer stopped ({error_code}): {reason}"));
		}
	}
}

wrap_life_span_handler! {
	struct HelperLife {
		page: Arc<Paged>,
	}

	impl LifeSpanHandler {
		// A source shows one page, and nothing would show another window, so
		// `window.open` and `target="_blank"` open nothing. That also keeps
		// the studio's pages the only browsers, whose closing ends the helper.
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
			// A page can close itself with `window.close()`, which its source
			// reports; one the studio closed has left it already.
			self.page.fail("the page closed itself".into());
			let last = PAGES.with_borrow_mut(|pages| {
				pages.remove(&self.page.id);
				pages.is_empty()
			});
			if last && QUITTING.get() {
				quit_message_loop();
			}
		}
	}
}
