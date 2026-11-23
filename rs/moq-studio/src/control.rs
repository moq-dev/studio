//! Driving the window from another process, as a person at it would:
//! `--control <socket>` takes requests on a Unix socket, and `moq-studio ctl`
//! sends them.
//!
//! The window is read through what it tells screen readers, its AccessKit
//! tree, and moved with the pointer and keyboard events egui hears from the
//! desktop, so a request finds and presses a widget by the name a screen
//! reader says. Requests come one connection at a time, a JSON object a line,
//! each answered with a JSON line before the next is read. A screenshot's
//! line is followed by its PNG, as many bytes as the line says.
//!
//! The socket is the user's own: it is made readable by them alone, and a
//! connection from another user's process is refused.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use egui::accesskit;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

#[cfg(unix)]
mod ctl;
pub mod headless;

/// Run `moq-studio ctl`, as the process's arguments say.
pub fn ctl() -> anyhow::Result<()> {
	#[cfg(unix)]
	return ctl::Driver::run();
	#[cfg(not(unix))]
	anyhow::bail!("`ctl` drives a studio through a Unix socket, which this system lacks")
}

/// The longest request line: a typed text, a label, or a JSON value to wait
/// for, never more.
const LINE: usize = 64 * 1024;

/// The frames a pointer takes between a drag's ends, so egui sees it move
/// rather than jump.
const DRAG_STEPS: usize = 8;

/// The frames drawn after a request's last event before it is answered:
/// one for egui to act on what it heard, one to lay out what changed.
const SETTLE: u8 = 2;

/// What a request asks of the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
	/// The AccessKit tree, and what the studio holds: its page, scenes,
	/// selection, sounds, and whether it is on air or recording.
	Snapshot {},
	/// The window's picture as PNG.
	Screenshot {},
	/// Press and release a pointer button on `target`, twice if `double`.
	Click {
		target: Target,
		#[serde(default)]
		button: Button,
		#[serde(default)]
		double: bool,
	},
	/// Rest the pointer on `target`.
	Hover { target: Target },
	/// Press on `from`, move to `to`, and release there.
	Drag { from: Target, to: Target },
	/// Turn the wheel over `target` by a distance in points.
	Scroll { target: Target, dx: f32, dy: f32 },
	/// Type text into the widget with the keyboard focus.
	Type { text: String },
	/// Press and release a key with modifiers, such as `ctrl+z` or `Escape`.
	Key { combo: String },
	/// Wait until `until` holds, failing after `timeout`.
	Wait {
		until: Until,
		#[serde(with = "humantime_serde")]
		timeout: Duration,
	},
	/// Close the window, as its Close button does.
	Quit {},
}

/// A widget, by the AccessKit tree, or a point in the window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Target {
	/// The one widget named exactly this.
	Label(String),
	/// The one widget whose name holds this.
	Contains(String),
	/// The widget with this AccessKit node ID, as a snapshot lists it.
	Id(u64),
	/// A point, in the window's logical pixels from its top left.
	At([f32; 2]),
}

impl Target {
	/// Parse what `ctl` takes: `id:<n>`, `at:<x>,<y>`, `contains:<text>`,
	/// `label:<text>`, or a bare label.
	pub fn parse(text: &str) -> anyhow::Result<Self> {
		if let Some(id) = text.strip_prefix("id:") {
			return Ok(Self::Id(id.parse().with_context(|| format!("`{id}` is no node ID"))?));
		}
		if let Some(at) = text.strip_prefix("at:") {
			let (x, y) = at.split_once(',').context("a point is `at:<x>,<y>`")?;
			let coord = |value: &str| -> anyhow::Result<f32> {
				let value: f32 = value.trim().parse().with_context(|| format!("`{value}` is no coordinate"))?;
				anyhow::ensure!(value.is_finite(), "`{value}` is no coordinate");
				Ok(value)
			};
			return Ok(Self::At([coord(x)?, coord(y)?]));
		}
		if let Some(contains) = text.strip_prefix("contains:") {
			return Ok(Self::Contains(contains.to_string()));
		}
		Ok(Self::Label(text.strip_prefix("label:").unwrap_or(text).to_string()))
	}
}

/// A pointer button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Button {
	#[default]
	Primary,
	Secondary,
	Middle,
}

impl From<Button> for egui::PointerButton {
	fn from(button: Button) -> Self {
		match button {
			Button::Primary => Self::Primary,
			Button::Secondary => Self::Secondary,
			Button::Middle => Self::Middle,
		}
	}
}

/// What a wait waits for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Until {
	/// A widget is in the tree.
	Shown(Target),
	/// No widget matches.
	Gone(Target),
	/// Every source has started, and the program has drawn a second of
	/// frames since.
	Settled,
	/// The value at a JSON pointer into the snapshot's state is this.
	State { pointer: String, value: serde_json::Value },
}

/// The answer to a request, a line of JSON.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Reply {
	Done,
	Snapshot {
		tree: Option<Node>,
		focus: Option<u64>,
		state: serde_json::Value,
	},
	/// The PNG's length; its bytes follow the line.
	Screenshot {
		png: usize,
		#[serde(skip)]
		bytes: Vec<u8>,
	},
	Error(String),
}

impl Reply {
	fn error(err: impl std::fmt::Display) -> Self {
		Self::Error(err.to_string())
	}
}

/// A widget as a snapshot lists it.
#[derive(Debug, Serialize)]
pub struct Node {
	id: u64,
	role: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	label: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	value: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	number: Option<f64>,
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	disabled: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	checked: Option<bool>,
	#[serde(skip_serializing_if = "Option::is_none")]
	selected: Option<bool>,
	/// Its bounds in logical pixels: left, top, right, bottom.
	#[serde(skip_serializing_if = "Option::is_none")]
	rect: Option<[f32; 4]>,
	#[serde(skip_serializing_if = "Vec::is_empty")]
	children: Vec<Node>,
}

/// The window's latest AccessKit tree, egui's whole tree each frame.
#[derive(Default)]
struct Tree {
	nodes: BTreeMap<accesskit::NodeId, accesskit::Node>,
	root: Option<accesskit::NodeId>,
	focus: Option<accesskit::NodeId>,
}

impl Tree {
	fn update(&mut self, update: &accesskit::TreeUpdate) {
		self.nodes = update.nodes.iter().cloned().collect();
		if let Some(tree) = &update.tree {
			self.root = Some(tree.root);
		}
		self.focus = Some(update.focus).filter(|focus| Some(*focus) != self.root);
	}

	fn node(&self, id: accesskit::NodeId) -> Option<Node> {
		let node = self.nodes.get(&id)?;
		Some(Node {
			id: id.0,
			role: format!("{:?}", node.role()),
			label: node.label().map(str::to_string),
			value: node.value().map(str::to_string),
			number: node.numeric_value(),
			disabled: node.is_disabled(),
			checked: node.toggled().map(|toggled| toggled == accesskit::Toggled::True),
			selected: node.is_selected(),
			rect: node.bounds().map(|r| [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32]),
			children: node.children().iter().filter_map(|child| self.node(*child)).collect(),
		})
	}

	fn snapshot(&self) -> Option<Node> {
		self.node(self.root?)
	}

	/// The widgets `target` names, which must be one to act on.
	fn find(&self, target: &Target) -> Vec<(accesskit::NodeId, &accesskit::Node)> {
		self.nodes
			.iter()
			.filter(|(id, node)| match target {
				Target::Label(label) => node.label() == Some(label.as_str()),
				Target::Contains(text) => node.label().is_some_and(|label| label.contains(text.as_str())),
				Target::Id(want) => id.0 == *want,
				Target::At(_) => false,
			})
			.map(|(id, node)| (*id, node))
			.collect()
	}

	/// The one widget `target` names, or why there is not one.
	fn one(&self, target: &Target) -> anyhow::Result<(accesskit::NodeId, &accesskit::Node)> {
		let mut found = self.find(target);
		match found.len() {
			0 => anyhow::bail!("no widget is {}", describe(target)),
			1 => Ok(found.remove(0)),
			_ => {
				let each: Vec<String> = found
					.iter()
					.map(|(id, node)| format!("id:{} ({:?} {:?})", id.0, node.role(), node.label().unwrap_or("")))
					.collect();
				anyhow::bail!(
					"{} widgets are {}; pick one by ID: {}",
					found.len(),
					describe(target),
					each.join(", ")
				)
			}
		}
	}

	/// Where a press on `target` lands: a point, or the middle of its widget,
	/// which must be enabled and have bounds.
	fn point(&self, target: &Target, screen: egui::Rect) -> anyhow::Result<egui::Pos2> {
		let pos = match target {
			Target::At([x, y]) => egui::pos2(*x, *y),
			_ => {
				let (_, node) = self.one(target)?;
				anyhow::ensure!(!node.is_disabled(), "{} is disabled", describe(target));
				let rect = node.bounds().with_context(|| format!("{} has no bounds", describe(target)))?;
				egui::pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32)
			}
		};
		anyhow::ensure!(
			screen.contains(pos),
			"{} is at {pos:?}, outside the window's {screen:?}",
			describe(target)
		);
		Ok(pos)
	}
}

/// How an error names `target`.
fn describe(target: &Target) -> String {
	match target {
		Target::Label(label) => format!("named {label:?}"),
		Target::Contains(text) => format!("named with {text:?}"),
		Target::Id(id) => format!("id:{id}"),
		Target::At([x, y]) => format!("at {x},{y}"),
	}
}

/// A request and where its answer goes.
type Asked = (Request, oneshot::Sender<Reply>);

/// The control socket, bound and waiting for the window to serve it.
#[cfg(unix)]
pub struct Socket {
	listener: tokio::net::UnixListener,
	path: PathBuf,
}

#[cfg(unix)]
impl Socket {
	/// Bind `path`, readable by this user alone, on the current runtime. A
	/// socket left there by a studio that ended is replaced; anything else
	/// there is refused.
	pub fn bind(path: &Path) -> anyhow::Result<Self> {
		use std::os::unix::fs::{FileTypeExt, PermissionsExt};

		if let Ok(meta) = std::fs::symlink_metadata(path) {
			anyhow::ensure!(
				meta.file_type().is_socket(),
				"{} exists and is not a socket",
				path.display()
			);
			match std::os::unix::net::UnixStream::connect(path) {
				Ok(_) => anyhow::bail!("another studio is controlled through {}", path.display()),
				Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => {
					std::fs::remove_file(path).with_context(|| format!("cannot replace {}", path.display()))?;
				}
				Err(err) => return Err(err).with_context(|| format!("cannot tell whether {} is in use", path.display())),
			}
		}
		let listener = std::os::unix::net::UnixListener::bind(path)
			.with_context(|| format!("cannot listen on {}", path.display()))?;
		// Until this, the umask's mode let others connect; each connection's
		// peer is checked as well.
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
			.with_context(|| format!("cannot make {} private", path.display()))?;
		listener.set_nonblocking(true)?;
		Ok(Self {
			listener: tokio::net::UnixListener::from_std(listener)?,
			path: path.to_path_buf(),
		})
	}

	/// Serve requests to the window drawn by `ctx` until the process ends,
	/// returning what the window answers them with.
	pub fn serve(self, ctx: &egui::Context, runtime: &tokio::runtime::Handle) -> Control {
		// The tree is drawn only once something asks for it; a screen reader
		// turns it on the same way.
		ctx.enable_accesskit();
		let (send, receive) = mpsc::channel(1);
		let shared = Arc::new(Mutex::new(Shared::default()));
		ctx.add_plugin(Plugin {
			requests: receive,
			op: None,
			tree: Tree::default(),
			shared: shared.clone(),
			screenshots: 0,
		});
		let listener = self.listener;
		let ctx = ctx.clone();
		runtime.spawn(async move {
			if let Err(err) = accept(listener, send, ctx).await {
				tracing::error!(error = format!("{err:#}"), "the control socket failed");
			}
		});
		Control {
			shared,
			_path: Unlink(self.path),
		}
	}
}

/// No control socket, where there are no Unix sockets.
#[cfg(not(unix))]
pub enum Socket {}

#[cfg(not(unix))]
impl Socket {
	pub fn bind(path: &Path) -> anyhow::Result<Self> {
		anyhow::bail!("--control {} takes a Unix socket, which this system lacks", path.display())
	}

	pub fn serve(self, _: &egui::Context, _: &tokio::runtime::Handle) -> Control {
		match self {}
	}
}

/// Removes the socket's file once the studio ends.
struct Unlink(PathBuf);

impl Drop for Unlink {
	fn drop(&mut self) {
		let _ = std::fs::remove_file(&self.0);
	}
}

/// The window's side of the control socket: the state a snapshot or a wait
/// asks the studio for.
pub struct Control {
	shared: Arc<Mutex<Shared>>,
	_path: Unlink,
}

impl Control {
	/// Give the studio's state, as `describe` says it, to the request that
	/// asks for it this frame.
	pub fn describe(&self, describe: impl FnOnce() -> serde_json::Value) {
		let mut shared = self.shared.lock().unwrap();
		if shared.wanted {
			shared.state = Some(describe());
		}
	}
}

/// What the window and its plugin share within a frame.
#[derive(Default)]
struct Shared {
	/// Whether this frame's request reads the studio's state.
	wanted: bool,
	state: Option<serde_json::Value>,
}

#[cfg(unix)]
/// Take connections one at a time from this user's processes.
async fn accept(listener: tokio::net::UnixListener, send: mpsc::Sender<Asked>, ctx: egui::Context) -> anyhow::Result<()> {
	let uid = rustix::process::getuid().as_raw();
	loop {
		let (stream, _) = listener.accept().await?;
		match stream.peer_cred() {
			Ok(peer) if peer.uid() == uid => {}
			Ok(peer) => {
				tracing::warn!(uid = peer.uid(), "refused a control connection from another user");
				continue;
			}
			Err(err) => {
				tracing::warn!(%err, "refused a control connection whose user is unknown");
				continue;
			}
		}
		if let Err(err) = connection(stream, &send, &ctx).await {
			tracing::debug!(error = format!("{err:#}"), "a control connection ended");
		}
	}
}

#[cfg(unix)]
/// Answer one connection's requests in turn until it closes.
async fn connection(stream: tokio::net::UnixStream, send: &mpsc::Sender<Asked>, ctx: &egui::Context) -> anyhow::Result<()> {
	let (read, mut write) = stream.into_split();
	let mut read = tokio::io::BufReader::new(read);
	let mut line = Vec::new();
	loop {
		line.clear();
		(&mut read).take(LINE as u64 + 1).read_until(b'\n', &mut line).await?;
		if line.is_empty() {
			return Ok(());
		}
		if line.last() != Some(&b'\n') {
			let reply = match line.len() > LINE {
				true => Reply::Error(format!("a request is at most {LINE} bytes")),
				false => Reply::error("a request ends with a newline"),
			};
			return respond(&mut write, reply).await;
		}
		let reply = match serde_json::from_slice::<Request>(&line) {
			Ok(request) => ask(request, send, ctx, &mut read).await?,
			Err(err) => Reply::Error(format!("not a request: {err}")),
		};
		respond(&mut write, reply).await?;
	}
}

#[cfg(unix)]
/// Hand `request` to the window and wait for its answer, giving up if the
/// asker hangs up first.
async fn ask(
	request: Request,
	send: &mpsc::Sender<Asked>,
	ctx: &egui::Context,
	read: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<Reply> {
	let (reply, answer) = oneshot::channel();
	send.send((request, reply)).await.context("the window is gone")?;
	// The window draws only when asked to, as when nothing moves in it.
	ctx.request_repaint();
	tokio::select! {
		answer = answer => Ok(answer.unwrap_or_else(|_| Reply::error("the window closed"))),
		// Nothing more is read until this is answered, so anything is a hang-up
		// or a request out of turn.
		_ = read.fill_buf() => anyhow::bail!("the asker hung up or sent a request out of turn"),
	}
}

#[cfg(unix)]
async fn respond(write: &mut tokio::net::unix::OwnedWriteHalf, reply: Reply) -> anyhow::Result<()> {
	let mut line = serde_json::to_vec(&reply)?;
	line.push(b'\n');
	write.write_all(&line).await?;
	if let Reply::Screenshot { bytes, .. } = &reply {
		write.write_all(bytes).await?;
	}
	Ok(())
}

/// The request in progress, and where its answer goes.
struct Op {
	reply: oneshot::Sender<Reply>,
	work: Work,
}

enum Work {
	/// Answered with the tree and the state once the studio gives it.
	Snapshot,
	/// Waiting for the picture asked for with this number.
	Screenshot { asked: Option<u64> },
	/// Input to give, a frame's events at a time, once its targets are found.
	Input {
		/// Targets still to find; the first scrolled into view a frame
		/// before.
		find: Option<Find>,
		frames: VecDeque<Frame>,
		/// Frames left to draw once every event is given.
		settle: u8,
	},
	Wait {
		until: Until,
		deadline: kio::clock::Instant,
		timeout: Duration,
		/// For [`Until::Settled`], the program's frames once every source
		/// started.
		since: Option<u64>,
	},
	/// Answered once the window is told to close.
	Quit,
}

/// The targets of an input request, and how to make its frames from where
/// they land.
struct Find {
	targets: Vec<Target>,
	/// Whether the first target was asked to scroll into view.
	revealed: bool,
	make: fn(&[egui::Pos2], &Request) -> VecDeque<Frame>,
	request: Request,
}

/// The events of one frame.
struct Frame(Vec<egui::Event>);

impl Frame {
	fn of(events: impl IntoIterator<Item = egui::Event>) -> Self {
		Self(events.into_iter().collect())
	}
}

/// The egui plugin that turns requests into input and reads the window back.
struct Plugin {
	requests: mpsc::Receiver<Asked>,
	op: Option<Op>,
	tree: Tree,
	shared: Arc<Mutex<Shared>>,
	/// Screenshots asked for so far, to tell this one's picture by.
	screenshots: u64,
}

/// A screenshot's tag, which egui hands back with its picture.
struct Shot(u64);

impl Plugin {
	/// Start the next request, if one waits and none is in progress.
	fn next(&mut self, ctx: &egui::Context) {
		if self.op.is_some() {
			return;
		}
		let Ok((request, reply)) = self.requests.try_recv() else {
			return;
		};
		let work = match &request {
			Request::Snapshot {} => Work::Snapshot,
			Request::Screenshot {} => Work::Screenshot { asked: None },
			Request::Click { target, .. } | Request::Hover { target } | Request::Scroll { target, .. } => {
				input(vec![target.clone()], request.clone(), pointer)
			}
			Request::Drag { from, to } => input(vec![from.clone(), to.clone()], request.clone(), drag),
			Request::Type { text } => Work::Input {
				find: None,
				frames: [Frame::of([egui::Event::Text(text.clone())])].into(),
				settle: SETTLE,
			},
			Request::Key { combo } => match keys(combo) {
				Ok(frames) => Work::Input {
					find: None,
					frames,
					settle: SETTLE,
				},
				Err(err) => {
					let _ = reply.send(Reply::error(format!("{err:#}")));
					return;
				}
			},
			Request::Wait { until, timeout } => Work::Wait {
				until: until.clone(),
				deadline: kio::clock::now().saturating_add(*timeout),
				timeout: *timeout,
				since: None,
			},
			Request::Quit {} => {
				ctx.send_viewport_cmd(egui::ViewportCommand::Close);
				Work::Quit
			}
		};
		self.op = Some(Op { reply, work });
	}

	fn finish(&mut self, reply: Reply) {
		if let Some(op) = self.op.take() {
			let _ = op.reply.send(reply);
		}
	}
}

/// An input request whose frames are made once `targets` are found.
fn input(targets: Vec<Target>, request: Request, make: fn(&[egui::Pos2], &Request) -> VecDeque<Frame>) -> Work {
	Work::Input {
		find: Some(Find {
			targets,
			revealed: false,
			make,
			request,
		}),
		frames: VecDeque::new(),
		settle: SETTLE,
	}
}

/// The frames of a click, hover, or scroll at `at[0]`: the pointer moves
/// there a frame before it acts, as a hand would.
fn pointer(at: &[egui::Pos2], request: &Request) -> VecDeque<Frame> {
	let pos = at[0];
	let mut frames = VecDeque::from([Frame::of([egui::Event::PointerMoved(pos)])]);
	let press = |button: Button, pressed| {
		Frame::of([egui::Event::PointerButton {
			pos,
			button: button.into(),
			pressed,
			modifiers: egui::Modifiers::NONE,
		}])
	};
	match request {
		Request::Click { button, double, .. } => {
			for _ in 0..if *double { 2 } else { 1 } {
				frames.push_back(press(*button, true));
				frames.push_back(press(*button, false));
			}
		}
		Request::Scroll { dx, dy, .. } => frames.push_back(Frame::of([egui::Event::MouseWheel {
			unit: egui::MouseWheelUnit::Point,
			delta: egui::vec2(*dx, *dy),
			phase: egui::TouchPhase::Move,
			modifiers: egui::Modifiers::NONE,
		}])),
		_ => {}
	}
	frames
}

/// The frames of a drag from `at[0]` to `at[1]`.
fn drag(at: &[egui::Pos2], _: &Request) -> VecDeque<Frame> {
	let (from, to) = (at[0], at[1]);
	let button = |pos, pressed| egui::Event::PointerButton {
		pos,
		button: egui::PointerButton::Primary,
		pressed,
		modifiers: egui::Modifiers::NONE,
	};
	let mut frames = VecDeque::from([
		Frame::of([egui::Event::PointerMoved(from)]),
		Frame::of([button(from, true)]),
	]);
	for step in 1..=DRAG_STEPS {
		let pos = from.lerp(to, step as f32 / DRAG_STEPS as f32);
		frames.push_back(Frame::of([egui::Event::PointerMoved(pos)]));
	}
	frames.push_back(Frame::of([button(to, false)]));
	frames
}

/// The frames of pressing and releasing `combo`, such as `ctrl+shift+z`:
/// `ctrl`, `shift`, `alt`, and `cmd` (Command on macOS, Ctrl elsewhere),
/// then a key as egui names it.
fn keys(combo: &str) -> anyhow::Result<VecDeque<Frame>> {
	let mut modifiers = egui::Modifiers::NONE;
	let mut parts: Vec<&str> = combo.split('+').map(str::trim).collect();
	let name = parts.pop().filter(|name| !name.is_empty()).context("no key named")?;
	for part in parts {
		match part.to_ascii_lowercase().as_str() {
			"ctrl" | "control" => modifiers.ctrl = true,
			"shift" => modifiers.shift = true,
			"alt" | "option" => modifiers.alt = true,
			"cmd" | "command" => modifiers |= egui::Modifiers::COMMAND,
			other => anyhow::bail!("`{other}` is no modifier: ctrl, shift, alt, or cmd"),
		}
	}
	let key = egui::Key::from_name(name).with_context(|| format!("egui names no key `{name}`"))?;
	let event = |pressed| egui::Event::Key {
		key,
		physical_key: Some(key),
		pressed,
		repeat: false,
		modifiers,
	};
	Ok([
		Frame::of([egui::Event::ModifiersChanged(modifiers), event(true)]),
		Frame::of([event(false), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]),
	]
	.into())
}

impl egui::Plugin for Plugin {
	fn debug_name(&self) -> &'static str {
		"control"
	}

	fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
		if self.op.as_ref().is_some_and(|op| op.reply.is_closed()) {
			// Its asker hung up.
			self.op = None;
		}
		self.next(ctx);
		let Some(op) = &mut self.op else { return };
		match &mut op.work {
			Work::Input {
				find: Some(find),
				frames,
				..
			} => {
				let screen = input.screen_rect.unwrap_or(egui::Rect::EVERYTHING);
				if !find.revealed {
					find.revealed = true;
					// Scrolled into view first, where a person would scroll to it.
					if let Ok((id, _)) = self.tree.one(&find.targets[0]) {
						input.events.push(egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
							action: accesskit::Action::ScrollIntoView,
							target_tree: accesskit::TreeId::ROOT,
							target_node: id,
							data: None,
						}));
					}
					return;
				}
				let found: anyhow::Result<Vec<_>> = find.targets.iter().map(|target| self.tree.point(target, screen)).collect();
				match found {
					Ok(at) => {
						*frames = (find.make)(&at, &find.request);
						let Work::Input { find, .. } = &mut op.work else {
							unreachable!()
						};
						*find = None;
					}
					Err(err) => return self.finish(Reply::error(format!("{err:#}"))),
				}
				self.give(input);
			}
			Work::Input { find: None, .. } => self.give(input),
			Work::Screenshot { asked: Some(asked) } => {
				let asked = *asked;
				let image = input.events.iter().find_map(|event| match event {
					egui::Event::Screenshot { user_data, image, .. }
						if user_data
							.data
							.as_ref()
							.and_then(|data| data.downcast_ref::<Shot>())
							.is_some_and(|shot| shot.0 == asked) =>
					{
						Some(image.clone())
					}
					_ => None,
				});
				if let Some(image) = image {
					let reply = match png(&image) {
						Ok(bytes) => Reply::Screenshot { png: bytes.len(), bytes },
						Err(err) => Reply::error(format!("{err:#}")),
					};
					self.finish(reply);
				}
			}
			_ => {}
		}
	}

	fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
		let Some(op) = &mut self.op else { return };
		let ctx = ui.ctx();
		match &mut op.work {
			Work::Screenshot { asked } if asked.is_none() => {
				self.screenshots += 1;
				*asked = Some(self.screenshots);
				ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(Shot(
					self.screenshots,
				))));
			}
			Work::Snapshot => self.shared.lock().unwrap().wanted = true,
			Work::Wait {
				until: Until::State { .. } | Until::Settled,
				..
			} => self.shared.lock().unwrap().wanted = true,
			_ => {}
		}
		// Keep drawing until it is answered: at once for input, and for a
		// wait as often as the window would draw anyway.
		match op.work {
			Work::Wait { .. } => ctx.request_repaint_after(Duration::from_millis(16)),
			_ => ctx.request_repaint(),
		}
	}

	fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
		if let Some(update) = &output.platform_output.accesskit_update {
			self.tree.update(update);
		}
		let state = {
			let mut shared = self.shared.lock().unwrap();
			shared.wanted = false;
			shared.state.take()
		};
		let Some(op) = &mut self.op else { return };
		let reply = match &mut op.work {
			Work::Snapshot => Some(Reply::Snapshot {
				tree: self.tree.snapshot(),
				focus: self.tree.focus.map(|id| id.0),
				state: state.unwrap_or(serde_json::Value::Null),
			}),
			Work::Input {
				find: None,
				frames,
				settle,
			} if frames.is_empty() => {
				*settle = settle.saturating_sub(1);
				(*settle == 0).then_some(Reply::Done)
			}
			Work::Wait {
				until,
				deadline,
				timeout,
				since,
			} => {
				let held = match until {
					Until::Shown(target) => !self.tree.find(target).is_empty(),
					Until::Gone(target) => self.tree.find(target).is_empty(),
					Until::State { pointer, value } => {
						state.as_ref().and_then(|state| state.pointer(pointer)) == Some(&*value)
					}
					Until::Settled => settled(state.as_ref(), since),
				};
				match held {
					true => Some(Reply::Done),
					false if kio::clock::now() >= *deadline => Some(Reply::Error(format!(
						"timed out after {} waiting for {until:?}",
						humantime::format_duration(*timeout)
					))),
					false => None,
				}
			}
			Work::Quit => Some(Reply::Done),
			_ => None,
		};
		if let Some(reply) = reply {
			self.finish(reply);
		}
	}
}

impl Plugin {
	/// Give the next frame's events to egui.
	fn give(&mut self, input: &mut egui::RawInput) {
		let Some(Op {
			work: Work::Input { frames, .. },
			..
		}) = &mut self.op
		else {
			return;
		};
		if let Some(frame) = frames.pop_front() {
			input.events.extend(frame.0);
		}
	}
}

/// Whether every source has started and the program has drawn a second of
/// frames since, as `state` says, noting in `since` when they started.
fn settled(state: Option<&serde_json::Value>, since: &mut Option<u64>) -> bool {
	let Some(studio) = state.and_then(|state| state.get("studio")) else {
		return false;
	};
	let starting = studio["starting"].as_array().is_some_and(|starting| !starting.is_empty());
	let (Some(frames), Some(rate)) = (studio["frames"].as_u64(), studio["framerate"].as_u64()) else {
		return false;
	};
	match since {
		None if !starting => *since = Some(frames),
		Some(since) => return frames >= *since + rate,
		None => {}
	}
	false
}

/// `image` as PNG, its alpha dropped, since the window is opaque.
fn png(image: &egui::ColorImage) -> anyhow::Result<Vec<u8>> {
	let [width, height] = image.size;
	let mut rgb = Vec::with_capacity(width * height * 3);
	for pixel in &image.pixels {
		rgb.extend_from_slice(&pixel.to_srgba_unmultiplied()[..3]);
	}
	let mut bytes = Vec::new();
	let mut encoder = png::Encoder::new(&mut bytes, width.try_into()?, height.try_into()?);
	encoder.set_color(png::ColorType::Rgb);
	encoder.set_depth(png::BitDepth::Eight);
	let mut writer = encoder.write_header()?;
	writer.write_image_data(&rgb)?;
	writer.finish()?;
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// `ctl`'s target words parse to the target they name, and a malformed
	/// point is refused rather than taken as a label.
	#[test]
	fn targets_parse() {
		assert_eq!(Target::parse("Go live").unwrap(), Target::Label("Go live".into()));
		assert_eq!(Target::parse("label:id:7").unwrap(), Target::Label("id:7".into()));
		assert_eq!(Target::parse("id:7").unwrap(), Target::Id(7));
		assert_eq!(Target::parse("at:10, 20.5").unwrap(), Target::At([10.0, 20.5]));
		assert_eq!(Target::parse("contains:Undo").unwrap(), Target::Contains("Undo".into()));
		for refused in ["id:x", "at:10", "at:1,NaN", "at:a,b"] {
			assert!(Target::parse(refused).is_err(), "{refused} was accepted");
		}
	}

	/// A combination presses its modifiers with its key, and an unknown
	/// modifier or key is refused.
	#[test]
	fn keys_parse() {
		let frames = keys("ctrl+shift+Z").unwrap();
		assert_eq!(frames.len(), 2);
		let egui::Event::Key { key, pressed, modifiers, .. } = &frames[0].0[1] else {
			panic!("a key event")
		};
		assert_eq!((*key, *pressed), (egui::Key::Z, true));
		assert!(modifiers.ctrl && modifiers.shift && !modifiers.alt);
		assert!(keys("Escape").is_ok());
		for refused in ["hyper+a", "ctrl+", "ctrl+NoSuchKey", ""] {
			assert!(keys(refused).is_err(), "{refused} was accepted");
		}
	}

	/// A request line is the JSON `ctl` writes, and anything else, such as an
	/// unknown field, is refused.
	#[test]
	fn requests_round_trip() {
		let request = Request::Wait {
			until: Until::Shown(Target::Contains("Undo".into())),
			timeout: Duration::from_secs(5),
		};
		let line = serde_json::to_string(&request).unwrap();
		assert_eq!(line, r#"{"cmd":"wait","until":{"shown":{"contains":"Undo"}},"timeout":"5s"}"#);
		assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
		assert!(serde_json::from_str::<Request>(r#"{"cmd":"quit","extra":1}"#).is_err());
		assert!(serde_json::from_str::<Request>(r#"{"cmd":"format-disk"}"#).is_err());
	}

	/// A drag moves the pointer in steps between its ends, pressed the whole
	/// way.
	#[test]
	fn a_drag_moves_between_its_ends() {
		let frames = drag(&[egui::pos2(0.0, 0.0), egui::pos2(80.0, 40.0)], &Request::Quit {});
		assert_eq!(frames.len(), DRAG_STEPS + 3);
		let moved: Vec<egui::Pos2> = frames
			.iter()
			.flat_map(|frame| &frame.0)
			.filter_map(|event| match event {
				egui::Event::PointerMoved(pos) => Some(*pos),
				_ => None,
			})
			.collect();
		assert_eq!(moved.first(), Some(&egui::pos2(0.0, 0.0)));
		assert_eq!(moved.last(), Some(&egui::pos2(80.0, 40.0)));
		assert!(moved.windows(2).all(|pair| pair[1].x > pair[0].x));
	}

	/// Settled waits for every source to start, then for a second of the
	/// program's frames from then.
	#[test]
	fn settled_waits_a_second_of_frames_after_the_sources_start() {
		let state = |starting: &[&str], frames: u64| serde_json::json!({"studio": {"starting": starting, "frames": frames, "framerate": 30}});
		let mut since = None;
		assert!(!settled(Some(&state(&["cam"], 100)), &mut since));
		assert_eq!(since, None);
		assert!(!settled(Some(&state(&[], 110)), &mut since));
		assert!(!settled(Some(&state(&[], 139)), &mut since));
		assert!(settled(Some(&state(&[], 140)), &mut since));
		assert!(!settled(None, &mut None));
	}
}
