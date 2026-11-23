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

use crate::hotkeys;

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
	/// Scroll what is under `target` by a distance in points: positive `dy`
	/// brings what is below into view.
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
				let value: f32 = value
					.trim()
					.parse()
					.with_context(|| format!("`{value}` is no coordinate"))?;
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
		tree: Option<Box<Node>>,
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
	/// Its name, as a screen reader says it: its own label, or the text of
	/// the labels it is labelled by.
	#[serde(skip_serializing_if = "Option::is_none")]
	label: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	value: Option<String>,
	/// What it can be asked to do, such as `click` or `focus`.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	actions: Vec<&'static str>,
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

/// The actions a snapshot lists, by the name it gives each.
const ACTIONS: [(accesskit::Action, &str); 5] = [
	(accesskit::Action::Click, "click"),
	(accesskit::Action::Focus, "focus"),
	(accesskit::Action::SetValue, "set-value"),
	(accesskit::Action::Increment, "increment"),
	(accesskit::Action::Decrement, "decrement"),
];

/// The window's latest AccessKit tree, egui's whole tree each frame.
#[derive(Default)]
struct Tree {
	nodes: BTreeMap<accesskit::NodeId, accesskit::Node>,
	/// Where each of egui's widgets takes the pointer: its rect, clipped to
	/// what the panes holding it show. A node egui paints rather than lays
	/// out, such as a selection's handle, has none.
	clips: BTreeMap<accesskit::NodeId, egui::Rect>,
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
			label: self.name(node),
			value: node.value().map(str::to_string),
			actions: ACTIONS
				.iter()
				.filter(|(action, _)| node.supports_action(*action))
				.map(|(_, name)| *name)
				.collect(),
			number: node.numeric_value(),
			disabled: node.is_disabled(),
			checked: node.toggled().map(|toggled| toggled == accesskit::Toggled::True),
			selected: node.is_selected(),
			rect: node
				.bounds()
				.map(|r| [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32]),
			children: node.children().iter().filter_map(|child| self.node(*child)).collect(),
		})
	}

	/// `node`'s accessible name: its label, or else the text of what labels
	/// it, as a field beside its label in a form is named.
	fn name(&self, node: &accesskit::Node) -> Option<String> {
		if let Some(label) = node.label().filter(|label| !label.is_empty()) {
			return Some(label.to_string());
		}
		let by: Vec<&str> = node
			.labelled_by()
			.iter()
			.filter_map(|id| self.nodes.get(id))
			.filter_map(|label| label.label().or(label.value()))
			.collect();
		(!by.is_empty()).then(|| by.join(" "))
	}

	fn snapshot(&self) -> Option<Node> {
		self.node(self.root?)
	}

	/// The widgets `target` names, which must be one to act on.
	fn find(&self, target: &Target) -> Vec<(accesskit::NodeId, &accesskit::Node)> {
		self.nodes
			.iter()
			.filter(|(id, node)| match target {
				Target::Label(label) => self.name(node).as_deref() == Some(label.as_str()),
				Target::Contains(text) => self.name(node).is_some_and(|name| name.contains(text.as_str())),
				Target::Id(want) => id.0 == *want,
				Target::At(_) => false,
			})
			.map(|(id, node)| (*id, node))
			.collect()
	}

	/// The one widget `target` names, or why there is not one.
	fn one(&self, target: &Target) -> anyhow::Result<(accesskit::NodeId, &accesskit::Node)> {
		// A group stands for the controls in it, such as a form row's, unless a
		// widget is named so itself.
		let (groups, mut found): (Vec<_>, Vec<_>) = self
			.find(target)
			.into_iter()
			.partition(|(_, node)| node.role() == accesskit::Role::Group);
		if found.is_empty() {
			found = groups.into_iter().flat_map(|(id, _)| self.controls(id)).collect();
		}
		match found.len() {
			0 => anyhow::bail!("no widget is {}", describe(target)),
			1 => Ok(found.remove(0)),
			_ => {
				let each: Vec<String> = found
					.iter()
					.map(|(id, node)| {
						format!(
							"id:{} ({:?} {:?})",
							id.0,
							node.role(),
							self.name(node).unwrap_or_default()
						)
					})
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

	/// The controls under `id`, not counting `id`.
	fn controls(&self, id: accesskit::NodeId) -> Vec<(accesskit::NodeId, &accesskit::Node)> {
		let Some(node) = self.nodes.get(&id) else {
			return Vec::new();
		};
		node.children()
			.iter()
			.filter_map(|child| Some((*child, self.nodes.get(child)?)))
			.flat_map(|(child, node)| match control(node) {
				true => vec![(child, node)],
				false => self.controls(child),
			})
			.collect()
	}

	/// Where a press on `target` lands: a point in the window, or the middle
	/// of what shows of its widget, which must be enabled.
	fn point(&self, target: &Target, screen: egui::Rect) -> anyhow::Result<Spot> {
		let what = describe(target);
		let (pos, widget) = match target {
			Target::At([x, y]) => (egui::pos2(*x, *y), None),
			_ => {
				let (id, node) = self.one(target)?;
				anyhow::ensure!(!node.is_disabled(), "{what} is disabled");
				let pos = self.spot(id).with_context(|| format!("{what} is out of view"))?;
				(pos, self.clips.contains_key(&id).then_some(id))
			}
		};
		anyhow::ensure!(
			screen.contains(pos),
			"{what} is at {pos:?}, outside the window's {screen:?}"
		);
		Ok(Spot { pos, widget, what })
	}

	/// The middle of `id`'s bounds.
	fn middle(&self, id: accesskit::NodeId) -> Option<egui::Pos2> {
		let rect = self.nodes.get(&id)?.bounds()?;
		Some(egui::pos2(
			((rect.x0 + rect.x1) / 2.0) as f32,
			((rect.y0 + rect.y1) / 2.0) as f32,
		))
	}

	/// The middle of what shows of `id`: of its widget as its panes clip it,
	/// or of its bounds for a node egui paints.
	fn spot(&self, id: accesskit::NodeId) -> Option<egui::Pos2> {
		match self.clips.get(&id) {
			Some(clip) => clip.is_positive().then(|| clip.center()),
			None => self.middle(id),
		}
	}

	/// Whether a pane clips `id`'s widget past its middle, so a person would
	/// scroll to it first.
	fn hidden(&self, id: accesskit::NodeId) -> bool {
		match (self.clips.get(&id), self.middle(id)) {
			(Some(clip), Some(middle)) => !clip.contains(middle),
			_ => false,
		}
	}
}

/// Where a pointer request acts: a point, and the widget there, if a
/// widget was named, to check a press there reaches it.
struct Spot {
	pos: egui::Pos2,
	widget: Option<accesskit::NodeId>,
	/// How an error names it.
	what: String,
}

/// Whether a person can act on `node`: press it, or give it the keyboard.
fn control(node: &accesskit::Node) -> bool {
	node.role() != accesskit::Role::Label
		&& (node.supports_action(accesskit::Action::Click) || node.supports_action(accesskit::Action::Focus))
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
	path: Unlink,
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
				Err(err) => {
					return Err(err).with_context(|| format!("cannot tell whether {} is in use", path.display()));
				}
			}
		}
		let listener = std::os::unix::net::UnixListener::bind(path)
			.with_context(|| format!("cannot listen on {}", path.display()))?;
		// Gone with the studio, even one that fails to start.
		let unlink = Unlink(path.to_path_buf());
		// Until this, the umask's mode let others connect; each connection's
		// peer is checked as well.
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
			.with_context(|| format!("cannot make {} private", path.display()))?;
		listener.set_nonblocking(true)?;
		Ok(Self {
			listener: tokio::net::UnixListener::from_std(listener)?,
			path: unlink,
		})
	}

	/// Serve requests to the window drawn by `ctx` until the process ends,
	/// pressing keys through `keyboard` as the desktop's are.
	pub fn serve(self, ctx: &egui::Context, runtime: &tokio::runtime::Handle, keyboard: hotkeys::Keyboard) -> Control {
		// The tree is drawn only once something asks for it; a screen reader
		// turns it on the same way.
		ctx.enable_accesskit();
		let (send, requests) = mpsc::channel(1);
		let state = Arc::new(Mutex::new(State::new(requests, keyboard)));
		ctx.add_plugin(Plugin {
			state: state.clone(),
			passes: Vec::new(),
		});
		let listener = self.listener;
		let wake = Wake(Arc::new(Mutex::new(Some(ctx.clone()))));
		let waking = wake.clone();
		runtime.spawn(async move {
			if let Err(err) = accept(listener, send, waking).await {
				tracing::error!(error = format!("{err:#}"), "the control socket failed");
			}
		});
		Control {
			state,
			wake,
			_path: self.path,
		}
	}
}

/// No control socket, where there are no Unix sockets.
#[cfg(not(unix))]
pub enum Socket {}

#[cfg(not(unix))]
impl Socket {
	pub fn bind(path: &Path) -> anyhow::Result<Self> {
		anyhow::bail!(
			"--control {} takes a Unix socket, which this system lacks",
			path.display()
		)
	}

	pub fn serve(self, _: &egui::Context, _: &tokio::runtime::Handle, _: hotkeys::Keyboard) -> Control {
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

/// The window's side of the control socket.
pub struct Control {
	state: Arc<Mutex<State>>,
	wake: Wake,
	_path: Unlink,
}

impl Drop for Control {
	fn drop(&mut self) {
		// Here, on the window's thread, rather than on the runtime's.
		let ctx = self.wake.0.lock().unwrap().take();
		drop(ctx);
	}
}

/// Wakes the window from the socket's task while there is a window.
/// [`Control`] takes it on the way out, so the task, which the runtime drops
/// as it ends, never holds egui's last reference: egui's teardown lets go of
/// what blocks on the runtime, such as a D-Bus proxy, which panics there.
#[derive(Clone)]
struct Wake(Arc<Mutex<Option<egui::Context>>>);

impl Wake {
	fn wake(&self) {
		if let Some(ctx) = &*self.0.lock().unwrap() {
			ctx.request_repaint_of(egui::ViewportId::ROOT);
		}
	}
}

impl Control {
	/// Run in every frame of the window, drawn or not: give the studio's
	/// state, as `describe` says it, to a request that reads it, and answer
	/// what an undrawn window cannot.
	pub fn logic(&self, ctx: &egui::Context, describe: impl FnOnce() -> serde_json::Value) {
		let mut state = self.state.lock().unwrap();
		if state.wanted {
			state.studio = Some(describe());
		}
		// eframe runs only this, and no pass, so none of the plugin's hooks, for
		// a minimized or hidden window.
		if !state.drawing {
			state.undrawn(ctx);
		}
	}
}

#[cfg(unix)]
/// Take connections one at a time from this user's processes.
async fn accept(listener: tokio::net::UnixListener, send: mpsc::Sender<Asked>, wake: Wake) -> anyhow::Result<()> {
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
		if let Err(err) = connection(stream, &send, &wake).await {
			tracing::debug!(error = format!("{err:#}"), "a control connection ended");
		}
	}
}

#[cfg(unix)]
/// Answer one connection's requests in turn until it closes.
async fn connection(stream: tokio::net::UnixStream, send: &mpsc::Sender<Asked>, wake: &Wake) -> anyhow::Result<()> {
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
			Ok(request) => ask(request, send, wake, &mut read).await?,
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
	wake: &Wake,
	read: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<Reply> {
	let (reply, answer) = oneshot::channel();
	send.send((request, reply)).await.context("the window is gone")?;
	// The window draws only when asked to, as when nothing moves in it.
	wake.wake();
	tokio::select! {
		answer = answer => Ok(answer.unwrap_or_else(|_| Reply::error("the window closed"))),
		// Nothing more is read until this is answered, so anything is a hang-up
		// or a request out of turn.
		_ = read.fill_buf() => {
			// A frame to let go of what the request pressed.
			wake.wake();
			anyhow::bail!("the asker hung up or sent a request out of turn")
		}
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
	/// Input to give, a frame's at a time, once its targets are found.
	Input {
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
	stage: Stage,
	make: fn(&[Spot], &Request) -> VecDeque<Frame>,
	request: Request,
}

/// How far finding an input's targets has come, a frame or more each.
#[derive(Clone, Copy, PartialEq)]
enum Stage {
	/// Drawing a frame with the tree on, which a screen reader leaving may
	/// have turned off since the last request.
	Look,
	/// Scrolling the first target into view if it is out of it, as a person
	/// would scroll to it first.
	Reveal,
	/// Waiting for the scroll to land: the frame after it lays out at the old
	/// offset, and an animated one moves over several. Its bounds when last
	/// seen, and the frames waited.
	Scrolling { seen: Option<accesskit::Rect>, frames: u8 },
}

/// The longest a target is waited for to scroll into view, a second of
/// frames, past egui's longest scroll animation.
const SCROLLING: u8 = 60;

/// What one frame gives egui.
#[derive(Debug)]
struct Frame(Vec<Give>);

impl Frame {
	fn of(events: impl IntoIterator<Item = egui::Event>) -> Self {
		Self(events.into_iter().map(Give::Event).collect())
	}
}

/// One thing a frame gives egui.
#[derive(Debug)]
enum Give {
	Event(egui::Event),
	/// The modifiers held from now on, which the studio's keyboard hears too.
	Modifiers(egui::Modifiers),
	/// A key pressed, which the studio's keyboard hears first, as it hears the
	/// desktop's, so a bound hotkey fires or binds rather than reaching egui.
	Press(Press),
	/// Before a press: that it reaches the widget the pointer was moved onto,
	/// rather than one over it, so nothing else is pressed in its place.
	Reaches {
		widget: accesskit::NodeId,
		what: String,
		reach: Reach,
	},
}

/// What of a widget's a pointer reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reach {
	/// A press: the widget egui would click or drag there.
	Press,
	/// A rest: one of the widgets holding the pointer.
	Rest,
}

/// The widgets egui found under the pointer as the last frame began.
#[derive(Default)]
struct Hits {
	click: Option<accesskit::NodeId>,
	drag: Option<accesskit::NodeId>,
	holding: Vec<accesskit::NodeId>,
}

/// A key pressed: the key egui names, where it sits, and what a US layout
/// prints on it.
#[derive(Clone, Debug, PartialEq)]
struct Press {
	key: egui::Key,
	physical: winit::keyboard::KeyCode,
	name: winit::keyboard::Key,
	modifiers: egui::Modifiers,
}

/// The egui plugin that turns requests into input and reads the window back,
/// its state shared with [`Control`], which runs when it does not.
struct Plugin {
	state: Arc<Mutex<State>>,
	/// The windows whose passes are under way, the innermost last: egui's own
	/// stack is popped before a pass's output reaches plugins.
	passes: Vec<egui::ViewportId>,
}

struct State {
	requests: mpsc::Receiver<Asked>,
	op: Option<Op>,
	tree: Tree,
	/// Screenshots asked for so far, to tell this one's picture by.
	screenshots: u64,
	keyboard: hotkeys::Keyboard,
	/// Where the pointer was put last, where a canceled press lets go.
	pointer: Option<egui::Pos2>,
	/// Releases owed by requests their askers gave up on, given next frame.
	owed: Vec<Give>,
	hits: Hits,
	/// Whether the window's pass is under way, between its begin and output.
	drawing: bool,
	/// Whether this frame's request reads the studio's state, and the state.
	wanted: bool,
	studio: Option<serde_json::Value>,
}

/// A screenshot's tag, which egui hands back with its picture.
struct Shot(u64);

impl State {
	fn new(requests: mpsc::Receiver<Asked>, keyboard: hotkeys::Keyboard) -> Self {
		Self {
			requests,
			op: None,
			tree: Tree::default(),
			screenshots: 0,
			keyboard,
			pointer: None,
			owed: Vec::new(),
			hits: Hits::default(),
			drawing: false,
			wanted: false,
			studio: None,
		}
	}

	/// Start the next request, if one waits and none is in progress, dropping
	/// any whose asker hung up while it waited: it must not act.
	fn next(&mut self, ctx: &egui::Context) {
		if self.op.is_some() {
			return;
		}
		let (request, reply) = loop {
			match self.requests.try_recv() {
				Ok((_, reply)) if reply.is_closed() => continue,
				Ok(asked) => break asked,
				Err(_) => return,
			}
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
				ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
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

	/// Drop the request in progress, owing egui the release of whatever it
	/// pressed and has yet to let go of, and return where its answer goes.
	fn cancel(&mut self) -> Option<oneshot::Sender<Reply>> {
		let op = self.op.take()?;
		if let Work::Input { frames, .. } = op.work {
			let pointer = self.pointer;
			let owed = frames.into_iter().flat_map(|frame| frame.0);
			self.owed.extend(owed.filter_map(|give| release(give, pointer)));
		}
		Some(op.reply)
	}

	/// A frame of a minimized or hidden window, which draws nothing to read or
	/// press: quit, and fail anything else.
	fn undrawn(&mut self, ctx: &egui::Context) {
		self.next(ctx);
		if matches!(self.op, Some(Op { work: Work::Quit, .. })) {
			return self.finish(Reply::Done);
		}
		if let Some(reply) = self.cancel() {
			// What a person would do first. Wayland lets no app restore itself,
			// so there this does nothing.
			ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
			let _ = reply.send(Reply::error(
				"the window is minimized or hidden, so it draws nothing to act on; it was asked to show again",
			));
		}
	}
}

impl egui::Plugin for Plugin {
	fn debug_name(&self) -> &'static str {
		"control"
	}

	// The multiview's window runs these hooks too; only the studio's own is
	// read and driven.

	fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
		self.passes.push(input.viewport_id);
		let mut state = self.state.lock().unwrap();
		// Pictures reach whichever window draws next, the multiview's too.
		state.screenshot(input);
		if input.viewport_id == egui::ViewportId::ROOT {
			state.input(ctx, input);
		}
	}

	fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
		if self.passes.last() == Some(&egui::ViewportId::ROOT) {
			self.state.lock().unwrap().begin(ui.ctx());
		}
	}

	fn on_end_pass(&mut self, ui: &mut egui::Ui) {
		if self.passes.last() == Some(&egui::ViewportId::ROOT) {
			self.state.lock().unwrap().end(ui.ctx());
		}
	}

	fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
		if self.passes.pop() == Some(egui::ViewportId::ROOT) {
			self.state.lock().unwrap().output(output);
		}
	}
}

impl State {
	/// Before a frame: start the next request, and give egui its input.
	fn input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
		if self.op.as_ref().is_some_and(|op| op.reply.is_closed()) {
			// Its asker hung up.
			self.cancel();
		}
		if !self.owed.is_empty() {
			let owed = Frame(std::mem::take(&mut self.owed));
			// Only releases, which reach whatever holds them.
			let _ = self.give(owed, ctx, input);
		}
		self.next(ctx);
		let screen = input.screen_rect.unwrap_or(egui::Rect::EVERYTHING);
		let Some(op) = &mut self.op else { return };
		// eframe turns the tree off when a screen reader leaves.
		ctx.enable_accesskit();
		let frame = match &mut op.work {
			Work::Input { find, frames, .. } => {
				if let Some(seeking) = find {
					match seek(seeking, &self.tree, screen, input) {
						Ok(None) => return,
						Ok(Some(at)) => *frames = (seeking.make)(&at, &seeking.request),
						Err(err) => return self.finish(Reply::error(format!("{err:#}"))),
					}
					*find = None;
				}
				frames.pop_front()
			}
			_ => None,
		};
		if let Some(frame) = frame
			&& let Err(err) = self.give(frame, ctx, input)
		{
			self.finish(Reply::error(format!("{err:#}")));
		}
	}

	/// As a frame ends, while a request is under way: note where egui's
	/// widgets take the pointer, and which it found under the pointer.
	fn end(&mut self, ctx: &egui::Context) {
		if self.op.is_none() {
			return;
		}
		let id = |widget: &egui::WidgetRect| widget.id.accesskit_id();
		let (widgets, hits) = ctx.viewport(|viewport| {
			let widgets: Vec<egui::WidgetRect> = viewport
				.this_pass
				.widgets
				.layers()
				.flat_map(|(_, widgets)| widgets.iter().copied())
				.collect();
			let hits = &viewport.hits;
			let hits = Hits {
				click: hits.click.as_ref().map(id),
				drag: hits.drag.as_ref().map(id),
				holding: hits.contains_pointer.iter().map(id).collect(),
			};
			(widgets, hits)
		});
		// Laid out in their layers' own coordinates, as a zoomed pane's are.
		self.tree.clips = widgets
			.iter()
			.map(|widget| {
				let clip = match ctx.layer_transform_to_global(widget.layer_id) {
					Some(to_global) => to_global * widget.interact_rect,
					None => widget.interact_rect,
				};
				(id(widget), clip)
			})
			.collect();
		self.hits = hits;
	}

	/// Answer a screenshot with its picture, if it is among `input`'s events.
	fn screenshot(&mut self, input: &egui::RawInput) {
		let Some(Op {
			work: Work::Screenshot { asked: Some(asked) },
			..
		}) = &self.op
		else {
			return;
		};
		let image = input.events.iter().find_map(|event| match event {
			egui::Event::Screenshot { user_data, image, .. }
				if user_data
					.data
					.as_ref()
					.and_then(|data| data.downcast_ref::<Shot>())
					.is_some_and(|shot| shot.0 == *asked) =>
			{
				Some(image.clone())
			}
			_ => None,
		});
		if let Some(image) = image {
			let reply = match png(&image) {
				Ok(bytes) => Reply::Screenshot {
					png: bytes.len(),
					bytes,
				},
				Err(err) => Reply::error(format!("{err:#}")),
			};
			self.finish(reply);
		}
	}

	/// As a frame begins: ask for what the request reads, and keep drawing.
	fn begin(&mut self, ctx: &egui::Context) {
		self.drawing = true;
		let Some(op) = &mut self.op else { return };
		match &mut op.work {
			Work::Screenshot { asked } if asked.is_none() => {
				self.screenshots += 1;
				*asked = Some(self.screenshots);
				let shot = egui::UserData::new(Shot(self.screenshots));
				ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Screenshot(shot));
			}
			Work::Snapshot
			| Work::Wait {
				until: Until::State { .. } | Until::Settled,
				..
			} => self.wanted = true,
			_ => {}
		}
		// Keep drawing until it is answered: at once for input, and for a
		// wait as often as the window would draw anyway.
		match op.work {
			Work::Wait { .. } => ctx.request_repaint_after_for(Duration::from_millis(16), egui::ViewportId::ROOT),
			_ => ctx.request_repaint_of(egui::ViewportId::ROOT),
		}
	}

	/// After a frame: read the window back, and answer what it settles.
	fn output(&mut self, output: &egui::FullOutput) {
		self.drawing = false;
		if let Some(update) = &output.platform_output.accesskit_update {
			self.tree.update(update);
		}
		self.wanted = false;
		let state = self.studio.take();
		let Some(op) = &mut self.op else { return };
		let reply = match &mut op.work {
			Work::Snapshot => Some(Reply::Snapshot {
				tree: self.tree.snapshot().map(Box::new),
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

	/// Give egui a frame's input, the keys through the studio's keyboard,
	/// failing before a press that would reach another widget than its own.
	fn give(&mut self, frame: Frame, ctx: &egui::Context, input: &mut egui::RawInput) -> anyhow::Result<()> {
		for give in frame.0 {
			match give {
				Give::Event(event) => {
					if let egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } = &event {
						self.pointer = Some(*pos);
					}
					input.events.push(event);
				}
				Give::Modifiers(modifiers) => {
					self.keyboard.modifiers(held(modifiers));
					input.events.push(egui::Event::ModifiersChanged(modifiers));
				}
				Give::Press(press) => {
					let physical = winit::keyboard::PhysicalKey::Code(press.physical);
					if self.keyboard.injected(physical, press.name.clone()) {
						match pressed(&press) {
							Typed::Event(event) => input.events.push(event),
							// Read from the window's own clipboard, as its desktop's paste is.
							Typed::Paste => {
								ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaste)
							}
						}
					}
				}
				Give::Reaches { widget, what, reach } => {
					let reached = match reach {
						Reach::Press => [self.hits.click, self.hits.drag].contains(&Some(widget)),
						Reach::Rest => self.hits.holding.contains(&widget),
					};
					if !reached {
						let over = self.hits.click.or(self.hits.drag).or(self.hits.holding.last().copied());
						let over =
							over.and_then(|id| Some((id, self.tree.nodes.get(&id)?)))
								.map(|(id, node)| match self.tree.name(node) {
									Some(name) => format!("{name:?}"),
									None => format!("id:{}", id.0),
								});
						anyhow::bail!(
							"{what} is covered where it shows: the pointer there is over {}",
							over.unwrap_or_else(|| "nothing that takes it".into())
						);
					}
				}
			}
		}
		Ok(())
	}
}

/// Take finding `find`'s targets a step further as a frame in `screen`
/// begins, returning where they are once found.
fn seek(
	find: &mut Find,
	tree: &Tree,
	screen: egui::Rect,
	input: &mut egui::RawInput,
) -> anyhow::Result<Option<Vec<Spot>>> {
	match find.stage {
		Stage::Look => {
			find.stage = Stage::Reveal;
			return Ok(None);
		}
		Stage::Reveal => {
			// Only a widget out of view: egui centers what it reveals, which would
			// move a pane already showing it.
			if let Ok((id, _)) = tree.one(&find.targets[0])
				&& tree.hidden(id)
			{
				input
					.events
					.push(egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
						action: accesskit::Action::ScrollIntoView,
						target_tree: accesskit::TreeId::ROOT,
						target_node: id,
						data: None,
					}));
				find.stage = Stage::Scrolling { seen: None, frames: 0 };
				return Ok(None);
			}
		}
		Stage::Scrolling { seen, frames } => {
			let now = tree
				.one(&find.targets[0])
				.ok()
				.and_then(|(id, node)| Some((id, node.bounds()?)));
			let landed = now.is_some_and(|(id, bounds)| seen == Some(bounds) && !tree.hidden(id));
			if !landed && frames < SCROLLING {
				find.stage = Stage::Scrolling {
					seen: now.map(|(_, bounds)| bounds),
					frames: frames + 1,
				};
				return Ok(None);
			}
		}
	}
	find.targets
		.iter()
		.map(|target| tree.point(target, screen))
		.collect::<anyhow::Result<_>>()
		.map(Some)
}

/// An input request whose frames are made once `targets` are found.
fn input(targets: Vec<Target>, request: Request, make: fn(&[Spot], &Request) -> VecDeque<Frame>) -> Work {
	Work::Input {
		find: Some(Find {
			targets,
			stage: Stage::Look,
			make,
			request,
		}),
		frames: VecDeque::new(),
		settle: SETTLE,
	}
}

/// Before a press or rest on `spot`, that it reaches the widget there.
fn reaches(spot: &Spot, reach: Reach) -> Option<Give> {
	Some(Give::Reaches {
		widget: spot.widget?,
		what: spot.what.clone(),
		reach,
	})
}

/// The frames of a click, hover, or scroll at `at[0]`: the pointer moves
/// there a frame before it acts, as a hand would.
fn pointer(at: &[Spot], request: &Request) -> VecDeque<Frame> {
	let spot = &at[0];
	let pos = spot.pos;
	let mut frames = VecDeque::from([Frame::of([egui::Event::PointerMoved(pos)])]);
	let press = |button: Button, pressed| {
		Give::Event(egui::Event::PointerButton {
			pos,
			button: button.into(),
			pressed,
			modifiers: egui::Modifiers::NONE,
		})
	};
	match request {
		Request::Click { button, double, .. } => {
			for _ in 0..if *double { 2 } else { 1 } {
				let reach = reaches(spot, Reach::Press);
				frames.push_back(Frame(reach.into_iter().chain([press(*button, true)]).collect()));
				frames.push_back(Frame(vec![press(*button, false)]));
			}
		}
		Request::Scroll { dx, dy, .. } => {
			// A trackpad's gesture, which egui applies at once, where it eases
			// a wheel's notch in over frames the reply would not wait for.
			// egui's delta moves the content, so it is the scroll's opposite.
			let wheel = |phase, delta| {
				Give::Event(egui::Event::MouseWheel {
					unit: egui::MouseWheelUnit::Point,
					delta,
					phase,
					modifiers: egui::Modifiers::NONE,
				})
			};
			let gesture = [
				wheel(egui::TouchPhase::Start, egui::Vec2::ZERO),
				wheel(egui::TouchPhase::Move, -egui::vec2(*dx, *dy)),
			];
			let reach = reaches(spot, Reach::Rest);
			frames.push_back(Frame(reach.into_iter().chain(gesture).collect()));
			frames.push_back(Frame(vec![wheel(egui::TouchPhase::End, egui::Vec2::ZERO)]));
		}
		_ => frames.extend(reaches(spot, Reach::Rest).map(|reach| Frame(vec![reach]))),
	}
	frames
}

/// The frames of a drag from `at[0]` to `at[1]`.
fn drag(at: &[Spot], _: &Request) -> VecDeque<Frame> {
	let (from, to) = (&at[0], at[1].pos);
	let button = |pos, pressed| {
		Give::Event(egui::Event::PointerButton {
			pos,
			button: egui::PointerButton::Primary,
			pressed,
			modifiers: egui::Modifiers::NONE,
		})
	};
	let reach = reaches(from, Reach::Press);
	let mut frames = VecDeque::from([
		Frame::of([egui::Event::PointerMoved(from.pos)]),
		Frame(reach.into_iter().chain([button(from.pos, true)]).collect()),
	]);
	for step in 1..=DRAG_STEPS {
		let pos = from.pos.lerp(to, step as f32 / DRAG_STEPS as f32);
		frames.push_back(Frame::of([egui::Event::PointerMoved(pos)]));
	}
	frames.push_back(Frame(vec![button(to, false)]));
	frames
}

/// What a canceled request still owes egui of `give`: a button let go,
/// where the pointer is rather than where it was headed, a key let go, or
/// the modifiers or a scroll gesture ended.
fn release(give: Give, pointer: Option<egui::Pos2>) -> Option<Give> {
	match give {
		Give::Event(egui::Event::PointerButton {
			pos,
			button,
			pressed: false,
			modifiers,
		}) => Some(Give::Event(egui::Event::PointerButton {
			pos: pointer.unwrap_or(pos),
			button,
			pressed: false,
			modifiers,
		})),
		Give::Event(
			event @ (egui::Event::Key { pressed: false, .. }
			| egui::Event::MouseWheel {
				phase: egui::TouchPhase::End,
				..
			}),
		) => Some(Give::Event(event)),
		Give::Modifiers(modifiers) if modifiers.is_none() => Some(Give::Modifiers(modifiers)),
		_ => None,
	}
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
			// As egui-winit sets them: Command is Ctrl, except on macOS.
			"ctrl" | "control" => {
				modifiers.ctrl = true;
				modifiers.command |= !cfg!(target_os = "macos");
			}
			"shift" => modifiers.shift = true,
			"alt" | "option" => modifiers.alt = true,
			"cmd" | "command" if cfg!(target_os = "macos") => {
				modifiers |= egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND
			}
			"cmd" | "command" => modifiers |= egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
			other => anyhow::bail!("`{other}` is no modifier: ctrl, shift, alt, or cmd"),
		}
	}
	let key = egui::Key::from_name(name).with_context(|| format!("egui names no key `{name}`"))?;
	let (physical, name) = place(key)?;
	let release = egui::Event::Key {
		key,
		physical_key: Some(key),
		pressed: false,
		repeat: false,
		modifiers,
	};
	Ok([
		Frame(vec![
			Give::Modifiers(modifiers),
			Give::Press(Press {
				key,
				physical,
				name,
				modifiers,
			}),
		]),
		Frame(vec![Give::Event(release), Give::Modifiers(egui::Modifiers::NONE)]),
	]
	.into())
}

/// Where `key` sits on a US layout, and what that layout prints on it, as
/// the desktop reports a press of it.
fn place(key: egui::Key) -> anyhow::Result<(winit::keyboard::KeyCode, winit::keyboard::Key)> {
	use egui::Key as K;
	let printed = |text: &str| winit::keyboard::Key::Character(text.into());
	let text = key.name();
	let (code, name) = match key {
		K::Comma => ("Comma".to_string(), printed(",")),
		K::Minus => ("Minus".to_string(), printed("-")),
		K::Period => ("Period".to_string(), printed(".")),
		K::Semicolon => ("Semicolon".to_string(), printed(";")),
		K::Backslash => ("Backslash".to_string(), printed("\\")),
		K::Slash => ("Slash".to_string(), printed("/")),
		K::Quote => ("Quote".to_string(), printed("'")),
		K::Backtick => ("Backquote".to_string(), printed("`")),
		K::OpenBracket => ("BracketLeft".to_string(), printed("[")),
		K::CloseBracket => ("BracketRight".to_string(), printed("]")),
		K::Equals => ("Equal".to_string(), printed("=")),
		K::Colon
		| K::Plus
		| K::Pipe
		| K::Questionmark
		| K::Exclamationmark
		| K::OpenCurlyBracket
		| K::CloseCurlyBracket => {
			anyhow::bail!("{text} is typed with shift on another key: press that key with shift")
		}
		_ if text.len() == 1 && text.as_bytes()[0].is_ascii_uppercase() => {
			(format!("Key{text}"), printed(&text.to_ascii_lowercase()))
		}
		_ if text.len() == 1 && text.as_bytes()[0].is_ascii_digit() => (format!("Digit{text}"), printed(text)),
		_ => {
			// A key of its own, which egui and the desktop name alike, but for
			// the arrows' prefix.
			let code = match key {
				K::ArrowDown | K::ArrowLeft | K::ArrowRight | K::ArrowUp => format!("Arrow{text}"),
				_ => text.to_string(),
			};
			let named = serde_json::from_value(serde_json::Value::String(code.clone()))
				.with_context(|| format!("the desktop names no key {text}"))?;
			(code, winit::keyboard::Key::Named(named))
		}
	};
	let physical = serde_json::from_value(serde_json::Value::String(code))
		.with_context(|| format!("no key sits where egui's {text} does"))?;
	Ok((physical, name))
}

/// What egui hears of a key pressed.
#[derive(Debug, PartialEq)]
enum Typed {
	Event(egui::Event),
	/// A paste, which the window reads its clipboard for.
	Paste,
}

/// What egui-winit makes of `press`: the clipboard's commands, as egui
/// hears them, or the key.
fn pressed(press: &Press) -> Typed {
	use egui::Key as K;
	let Press { key, modifiers, .. } = *press;
	let windows = cfg!(windows);
	let command = modifiers.command;
	if key == K::Cut || (command && key == K::X) || (windows && modifiers.shift && key == K::Delete) {
		return Typed::Event(egui::Event::Cut);
	}
	if key == K::Copy || (command && key == K::C) || (windows && modifiers.ctrl && key == K::Insert) {
		return Typed::Event(egui::Event::Copy);
	}
	if key == K::Paste || (command && key == K::V) || (windows && modifiers.shift && key == K::Insert) {
		return Typed::Paste;
	}
	Typed::Event(egui::Event::Key {
		key,
		physical_key: Some(key),
		pressed: true,
		repeat: false,
		modifiers,
	})
}

/// `modifiers` as the desktop reports them held.
fn held(modifiers: egui::Modifiers) -> winit::keyboard::ModifiersState {
	use winit::keyboard::ModifiersState;
	let mut state = ModifiersState::empty();
	state.set(ModifiersState::CONTROL, modifiers.ctrl);
	state.set(ModifiersState::SHIFT, modifiers.shift);
	state.set(ModifiersState::ALT, modifiers.alt);
	state.set(ModifiersState::SUPER, modifiers.mac_cmd);
	state
}

/// Whether every source has started and the program has drawn a second of
/// frames since, as `state` says, noting in `since` when they started.
fn settled(state: Option<&serde_json::Value>, since: &mut Option<u64>) -> bool {
	let Some(studio) = state.and_then(|state| state.get("studio")) else {
		return false;
	};
	let starting = studio["starting"]
		.as_array()
		.is_some_and(|starting| !starting.is_empty());
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

	/// The press in `frames`, which a combination makes after its modifiers.
	fn press(frames: &VecDeque<Frame>) -> &Press {
		match &frames[0].0[..] {
			[Give::Modifiers(_), Give::Press(press)] => press,
			other => panic!("not a press: {other:?}"),
		}
	}

	/// A combination presses its modifiers with its key, where the desktop
	/// says that key sits, and lets go of both; an unknown modifier or key is
	/// refused.
	#[test]
	fn keys_parse() {
		use winit::keyboard::{Key, KeyCode, NamedKey};

		let frames = keys("ctrl+shift+Z").unwrap();
		let press = press(&frames);
		assert_eq!(
			(press.key, press.physical, &press.name),
			(egui::Key::Z, KeyCode::KeyZ, &Key::Character("z".into()))
		);
		assert!(press.modifiers.ctrl && press.modifiers.shift && !press.modifiers.alt);
		assert!(matches!(
			&frames[1].0[..],
			[
				Give::Event(egui::Event::Key {
					key: egui::Key::Z,
					pressed: false,
					..
				}),
				Give::Modifiers(egui::Modifiers::NONE)
			]
		));
		// What a shortcut on Command, such as the palette's, matches.
		assert!(press_of("cmd+k").modifiers.matches_logically(egui::Modifiers::COMMAND));
		#[cfg(not(target_os = "macos"))]
		assert!(press_of("ctrl+k").modifiers.matches_logically(egui::Modifiers::COMMAND));
		for (combo, physical, name) in [
			("F9", KeyCode::F9, Key::Named(NamedKey::F9)),
			("Escape", KeyCode::Escape, Key::Named(NamedKey::Escape)),
			("ArrowDown", KeyCode::ArrowDown, Key::Named(NamedKey::ArrowDown)),
			("Space", KeyCode::Space, Key::Named(NamedKey::Space)),
			("7", KeyCode::Digit7, Key::Character("7".into())),
			("[", KeyCode::BracketLeft, Key::Character("[".into())),
		] {
			let press = press_of(combo);
			assert_eq!((press.physical, &press.name), (physical, &name), "{combo}");
		}
		for refused in ["hyper+a", "ctrl+", "ctrl+NoSuchKey", "", "Colon"] {
			assert!(keys(refused).is_err(), "{refused} was accepted");
		}
	}

	fn press_of(combo: &str) -> Press {
		press(&keys(combo).unwrap()).clone()
	}

	/// A shortcut for the clipboard reaches egui as the command, as the
	/// desktop's does, a paste through the window's clipboard, and any other
	/// key as itself.
	#[test]
	fn clipboard_shortcuts_are_commands() {
		assert_eq!(pressed(&press_of("cmd+c")), Typed::Event(egui::Event::Copy));
		assert_eq!(pressed(&press_of("cmd+x")), Typed::Event(egui::Event::Cut));
		assert_eq!(pressed(&press_of("cmd+v")), Typed::Paste);
		assert!(matches!(
			pressed(&press_of("c")),
			Typed::Event(egui::Event::Key {
				key: egui::Key::C,
				pressed: true,
				..
			})
		));
	}

	/// A spot `ctl` points at, on no widget in particular.
	fn at(x: f32, y: f32) -> Spot {
		Spot {
			pos: egui::pos2(x, y),
			widget: None,
			what: format!("at {x},{y}"),
		}
	}

	/// A request given up on halfway through a drag lets go where the pointer
	/// got to, and owes nothing it never pressed; one given up on after it,
	/// before a frame lets go, owes its own releases besides.
	#[test]
	fn canceled_requests_let_go_where_they_are() {
		let (_, requests) = mpsc::channel(1);
		let mut state = State::new(requests, hotkeys::Keyboard::default());
		let ctx = egui::Context::default();
		let mut input = egui::RawInput::default();
		let cancel = |state: &mut State, frames| {
			let (reply, _) = oneshot::channel();
			state.op = Some(Op {
				reply,
				work: Work::Input {
					find: None,
					frames,
					settle: SETTLE,
				},
			});
			state.cancel();
		};
		let mut frames = drag(&[at(0.0, 0.0), at(80.0, 0.0)], &Request::Quit {});
		for _ in 0..4 {
			state.give(frames.pop_front().unwrap(), &ctx, &mut input).unwrap();
		}
		cancel(&mut state, frames);
		cancel(&mut state, keys("ctrl+z").unwrap());
		assert!(matches!(
			&state.owed[..],
			[
				Give::Event(egui::Event::PointerButton { pos, pressed: false, .. }),
				Give::Event(egui::Event::Key { key: egui::Key::Z, pressed: false, .. }),
				Give::Modifiers(egui::Modifiers::NONE),
			] if *pos == egui::pos2(20.0, 0.0)
		));
	}

	/// A request whose asker hung up while it waited in the queue is dropped
	/// unstarted, freeing the queue, and the next is started.
	#[test]
	fn a_request_abandoned_in_the_queue_never_starts() {
		let (send, requests) = mpsc::channel(1);
		let mut state = State::new(requests, hotkeys::Keyboard::default());
		let ctx = egui::Context::default();
		let key = || Request::Key { combo: "F9".into() };
		let (reply, answer) = oneshot::channel();
		send.try_send((key(), reply)).unwrap();
		drop(answer);
		state.next(&ctx);
		assert!(state.op.is_none());
		let (reply, _answer) = oneshot::channel();
		send.try_send((key(), reply)).unwrap();
		state.next(&ctx);
		assert!(matches!(
			state.op,
			Some(Op {
				work: Work::Input { .. },
				..
			})
		));
	}

	/// A press where another widget lies over the target fails, naming it,
	/// before anything is pressed.
	#[test]
	fn a_press_on_a_covered_widget_fails() {
		let (_, requests) = mpsc::channel(1);
		let mut state = State::new(requests, hotkeys::Keyboard::default());
		let mut backdrop = accesskit::Node::new(accesskit::Role::Button);
		backdrop.set_label("Backdrop");
		state.tree.nodes.insert(accesskit::NodeId(9), backdrop);
		state.hits.click = Some(accesskit::NodeId(9));
		let spot = Spot {
			widget: Some(accesskit::NodeId(4)),
			..at(10.0, 10.0)
		};
		let mut frames = pointer(
			&[spot],
			&Request::Click {
				target: Target::Id(4),
				button: Button::Primary,
				double: false,
			},
		);
		let ctx = egui::Context::default();
		let mut input = egui::RawInput::default();
		state.give(frames.pop_front().unwrap(), &ctx, &mut input).unwrap();
		let err = state.give(frames.pop_front().unwrap(), &ctx, &mut input).unwrap_err();
		assert!(format!("{err}").contains("\"Backdrop\""), "{err}");
		assert!(
			!input
				.events
				.iter()
				.any(|event| matches!(event, egui::Event::PointerButton { .. }))
		);
		state.hits.click = Some(accesskit::NodeId(4));
		let mut input = egui::RawInput::default();
		state.give(frames.pop_front().unwrap(), &ctx, &mut input).unwrap();
	}

	/// A widget a pane clips past its middle is hidden, so it is scrolled to
	/// first, and a press lands on the middle of what shows; one clipped away
	/// entirely is out of view.
	#[test]
	fn a_press_lands_on_what_shows() {
		let rect = |x0, y0, x1, y1| accesskit::Rect { x0, y0, x1, y1 };
		let mut tree = Tree::default();
		for (id, bounds, clip) in [
			(
				1,
				rect(0.0, 0.0, 100.0, 20.0),
				egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 20.0)),
			),
			(
				2,
				rect(0.0, 190.0, 100.0, 210.0),
				egui::Rect::from_min_max(egui::pos2(0.0, 190.0), egui::pos2(100.0, 196.0)),
			),
			(3, rect(0.0, 300.0, 100.0, 320.0), egui::Rect::NOTHING),
		] {
			let mut node = accesskit::Node::new(accesskit::Role::Button);
			node.set_bounds(bounds);
			tree.nodes.insert(accesskit::NodeId(id), node);
			tree.clips.insert(accesskit::NodeId(id), clip);
		}
		let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
		assert!(!tree.hidden(accesskit::NodeId(1)));
		assert!(tree.hidden(accesskit::NodeId(2)));
		assert_eq!(tree.point(&Target::Id(2), screen).unwrap().pos, egui::pos2(50.0, 193.0));
		assert!(tree.point(&Target::Id(3), screen).is_err());
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
		assert_eq!(
			line,
			r#"{"cmd":"wait","until":{"shown":{"contains":"Undo"}},"timeout":"5s"}"#
		);
		assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
		assert!(serde_json::from_str::<Request>(r#"{"cmd":"quit","extra":1}"#).is_err());
		assert!(serde_json::from_str::<Request>(r#"{"cmd":"format-disk"}"#).is_err());
	}

	/// A drag moves the pointer in steps between its ends, pressed the whole
	/// way.
	#[test]
	fn a_drag_moves_between_its_ends() {
		let frames = drag(&[at(0.0, 0.0), at(80.0, 40.0)], &Request::Quit {});
		assert_eq!(frames.len(), DRAG_STEPS + 3);
		let moved: Vec<egui::Pos2> = frames
			.iter()
			.flat_map(|frame| &frame.0)
			.filter_map(|give| match give {
				Give::Event(egui::Event::PointerMoved(pos)) => Some(*pos),
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
