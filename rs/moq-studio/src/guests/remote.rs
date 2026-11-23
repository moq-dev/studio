//! Remotes: a phone that scans a QR code opens a page showing the scene pads,
//! the mutes, the live clock, and a tally, and drives them from wherever it
//! reaches the relay, with nothing installed.
//!
//! A remote is paired by name in the guests' room. Its token, signed by the
//! room key like a guest's, publishes only its presses, at
//! `.press/<name>/<generation>`, and reads only the desk the studio publishes
//! it at `.remote/<name>/<generation>`: the show as its page shows it, how its
//! last press went, and its renewed tokens. The studio's session in the room
//! reads the presses of each remote paired and not removed, and publishes its
//! desk while its page is connected.
//!
//! A press is untrusted input from the network: one JSON value of at most
//! [`MAX_PRESS`] bytes naming one action [`Action::remote`] admits, refused
//! whole otherwise. The studio reads a remote's newest press only, at most one
//! a program frame, so a page pressing faster loses its own presses and costs
//! the studio no more; the page sends its next press once the desk says how
//! the last went. Removing a remote stops reading its presses at once, and
//! stops renewing its token, across restarts, until a new link moves it to the
//! next generation, which its old token cannot reach.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, Weak};
use std::task::{Poll, ready};
use std::time::Duration;

use anyhow::Context;
use moq_net::Path;

use super::{Invite, Room, TOKEN_TRACK};
use crate::controls::{Action, Controls};
use crate::engine::OnAir;
use crate::mods::Mod;

/// The prefix of each remote's desk, which only its token may read.
pub(super) const DESK: &str = ".remote";

/// The prefix of each remote's presses, which only its token may publish.
pub(super) const PRESS: &str = ".press";

/// The track of a remote's desk, a JSON snapshot. `js/join` reads the same name.
const DESK_TRACK: &str = "desk";

/// The track of a remote's presses, a JSON snapshot. `js/join` writes the same name.
const PRESS_TRACK: &str = "press";

/// The most of one press the studio reads. A press names one scene or sound,
/// so this only stops a page sending something that is not one.
const MAX_PRESS: usize = 4096;

/// How often a connected remote's desk catches up with what the studio
/// changed itself, such as a scene taken at the keyboard.
const TALLY: Duration = Duration::from_millis(250);

/// What a remote's page is told of a press it made before the studio
/// restarted or rejoined the room, which the studio cannot tell was done.
const RESTARTED: &str = "the studio restarted; check whether this was done before pressing again";

/// The remote's page, beside the join page: `js/join/remote.html`.
const PAGE: &str = "remote.html";

/// One phone paired as a remote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remote {
	/// The newest link the host made for it this session, if any.
	pub invite: Option<Invite>,
	/// How often a new link paired it again after a removal. Its token and
	/// desk are under it, so a token from before cannot read or press.
	pub generation: u64,
	/// Whether the host removed it, which stops reading its presses and
	/// renewing its token, across restarts, until a new link.
	pub removed: bool,
	/// Whether its page is connected now.
	pub connected: bool,
}

/// The remotes the room key paired, kept beside it.
#[derive(Default)]
pub(super) struct Remotes {
	paired: RwLock<BTreeMap<String, Remote>>,
	/// Where they are kept, or `None` to keep them in memory alone.
	file: Option<PathBuf>,
	/// Whether the studio is leaving the room, which changes them no more.
	left: AtomicBool,
	/// Bumped when which remotes are read changes.
	changed: tokio::sync::watch::Sender<()>,
}

/// One remote as its file keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
	generation: u64,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	removed: bool,
}

impl Remotes {
	/// The remotes kept in the file at `path`, which keeps them from now on.
	pub(super) fn load(path: PathBuf) -> anyhow::Result<Self> {
		let kept: BTreeMap<String, Kept> = match std::fs::read_to_string(&path) {
			Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
			Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
		};
		let paired = kept.into_iter().map(|(name, kept)| {
			let remote = Remote {
				generation: kept.generation,
				removed: kept.removed,
				..Remote::default()
			};
			(name, remote)
		});
		Ok(Self {
			paired: RwLock::new(paired.collect()),
			file: Some(path),
			left: AtomicBool::new(false),
			changed: Default::default(),
		})
	}

	/// Write `paired` to the file, if they are kept in one.
	fn save(&self, paired: &BTreeMap<String, Remote>) -> anyhow::Result<()> {
		let Some(path) = &self.file else { return Ok(()) };
		let kept: BTreeMap<&String, Kept> = paired
			.iter()
			.map(|(name, remote)| {
				let kept = Kept {
					generation: remote.generation,
					removed: remote.removed,
				};
				(name, kept)
			})
			.collect();
		super::replace(path, &toml::to_string(&kept)?)
	}

	/// Apply `change` to the remote called `name` once the file holds it, so
	/// a failed write leaves both as they were.
	fn change(&self, name: &str, change: impl FnOnce(&mut Remote)) -> anyhow::Result<Remote> {
		let mut paired = self.paired.write().unwrap();
		anyhow::ensure!(!self.left.load(Ordering::SeqCst), super::LEFT);
		let before = paired.get(name).cloned();
		let remote = paired.entry(name.to_string()).or_default();
		change(remote);
		let changed = remote.clone();
		if let Err(err) = self.save(&paired) {
			match before {
				Some(before) => paired.insert(name.to_string(), before),
				None => paired.remove(name),
			};
			return Err(err);
		}
		drop(paired);
		self.changed.send_replace(());
		Ok(changed)
	}

	/// The remotes whose presses are read, and in which generation.
	fn read(&self) -> Vec<(String, u64)> {
		let paired = self.paired.read().unwrap();
		let read = paired.iter().filter(|(_, remote)| !remote.removed);
		read.map(|(name, remote)| (name.clone(), remote.generation)).collect()
	}

	/// A hold on the remotes while the one called `name` in `generation` is
	/// read, or `None` once it is not: a removal waits for the hold, so what
	/// is done under it never follows one.
	fn reading(&self, name: &str, generation: u64) -> Option<RwLockReadGuard<'_, BTreeMap<String, Remote>>> {
		let paired = self.paired.read().unwrap();
		let remote = paired.get(name)?;
		(!remote.removed && remote.generation == generation).then_some(paired)
	}

	/// Note whether the page of the remote called `name` is connected.
	fn connect(&self, name: &str, connected: bool) {
		if let Some(remote) = self.paired.write().unwrap().get_mut(name) {
			remote.connected = connected;
		}
	}

	/// Refuse every change from now on, once any under way is kept.
	pub(super) fn leave(&self) {
		self.left.store(true, Ordering::SeqCst);
		drop(self.paired.write().unwrap());
	}
}

impl Room {
	/// Make a link pairing a phone called `name` as a remote, replacing any
	/// earlier one, which keeps working until it expires: a signed token
	/// cannot be taken back. A removed remote is paired again under the next
	/// generation.
	pub fn pair(&self, name: &str) -> anyhow::Result<Invite> {
		let name = name.trim();
		anyhow::ensure!(!name.is_empty(), "a remote needs a name");
		anyhow::ensure!(
			!name.contains(['/', '\\']),
			"a remote's name is one path segment, without `/` or `\\`"
		);
		anyhow::ensure!(!name.starts_with('.'), "a remote's name cannot start with `.`");
		// `Url::join` reads a last segment as a file, so a directory without its
		// `/` would put the remote's page beside the directory, not in it.
		let last = self.page.path_segments().and_then(Iterator::last).unwrap_or_default();
		anyhow::ensure!(
			last.is_empty() || last.contains('.'),
			"the join page {} ends in a directory without its `/`, so no remote's page is beside it; end it with `/`",
			self.page
		);
		let page = self.page.join(PAGE).context("the join page has no page beside it")?;
		let before = self.remotes.paired.read().unwrap().get(name).cloned();
		let generation = before
			.as_ref()
			.map_or(0, |remote| remote.generation + u64::from(remote.removed));
		// Refused before anything changes, as a name that cannot be a remote.
		let (token, expires) = self.sign_remote(name, generation)?;
		let invite = Invite {
			link: self.link(page, name, &token, generation),
			expires,
		};
		self.remotes.change(name, |remote| {
			remote.removed = false;
			remote.generation = generation;
			remote.invite = Some(invite.clone());
		})?;
		Ok(invite)
	}

	/// Stop reading the presses of the remote called `name`, and renewing its
	/// token, across restarts, until the host makes it a new link.
	pub fn unpair(&self, name: &str) -> anyhow::Result<()> {
		self.remotes.change(name, |remote| remote.removed = true)?;
		Ok(())
	}

	/// Every phone paired as a remote, by name.
	pub fn remotes(&self) -> BTreeMap<String, Remote> {
		self.remotes.paired.read().unwrap().clone()
	}

	/// A token for the remote called `name` in `generation`, and when it
	/// expires: it publishes only its presses and reads only its desk.
	pub(super) fn sign_remote(&self, name: &str, generation: u64) -> anyhow::Result<(String, kio::clock::Wall)> {
		let invalid = || format!("cannot pair `{name}`");
		let claims = moq_auth::Claims::default()
			.with_root(self.root.as_str())
			.with_publish([moq_auth::Pattern::literal(
				press_path(name, generation).with_context(invalid)?.as_str(),
			)?])
			.with_subscribe([moq_auth::Pattern::literal(
				desk_path(name, generation).with_context(invalid)?.as_str(),
			)?]);
		self.sign_claims(claims)
			.context("the room key cannot sign a remote's token")
	}
}

/// Where the remote called `name` in `generation` publishes its presses, in the room.
fn press_path(name: &str, generation: u64) -> anyhow::Result<Path> {
	Ok(Path::new(&format!("{PRESS}/{name}/{generation}"))?)
}

/// Where the studio publishes the desk of the remote called `name` in
/// `generation`, in the room.
fn desk_path(name: &str, generation: u64) -> anyhow::Result<Path> {
	Ok(Path::new(&format!("{DESK}/{name}/{generation}"))?)
}

/// What a remote drives: the studio's controls, or a test's stand-in.
pub trait Board: Send + Sync {
	/// The show as a remote shows it now, without how its last press went.
	fn desk(&self) -> Desk;

	/// How long the program shows each picture: no remote presses faster.
	fn frame(&self) -> Duration;

	/// Do `action`, saying what it did or why it could not.
	fn press(self: Arc<Self>, action: &Action) -> Result<String, String>;
}

/// The show as a remote's page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Desk {
	/// Whether the program is on air.
	pub air: Air,
	/// Whole seconds on air, while live, which the page counts on from.
	pub clock: Option<u64>,
	/// Whether Studio mode is on, so a pad cues its scene rather than taking it.
	pub studio: bool,
	/// The scene on program.
	pub program: Option<String>,
	/// The scene cued, which TAKE and AUTO put on program.
	pub cued: Option<String>,
	/// Every scene's pad, in the show's order.
	pub scenes: Vec<Pad>,
	/// Why TAKE and AUTO cannot run now, if they cannot.
	pub take_refused: Option<String>,
	/// Every sound's mute.
	pub sounds: Vec<Mute>,
	/// How the page's last press went, once the studio has read one.
	pub done: Option<Done>,
}

/// Whether the program is on air, as [`OnAir`] says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Air {
	#[default]
	Off,
	Starting,
	Live,
	Ending,
}

/// A scene's pad.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pad {
	pub name: String,
	/// Why it cannot go on program, if it cannot.
	pub refused: Option<String>,
}

/// A sound's mute.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mute {
	pub name: String,
	pub muted: bool,
}

/// How a press went.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Done {
	/// The page that pressed, as its press named it.
	pub id: String,
	/// The press, by its number.
	pub seq: u64,
	/// Whether it did what it asked.
	pub ok: bool,
	/// What it did, or why it did not.
	pub said: String,
}

/// A press, as a remote's page publishes it: its page, its number, and the
/// action, or none for the page saying it is there.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Press {
	id: String,
	seq: u64,
	action: Option<Action>,
}

impl Board for Controls {
	fn desk(&self) -> Desk {
		let studio = self.studio();
		let on_air = self.engine.on_air();
		let state = self.engine.state.lock().unwrap();
		let collection = state.collection();
		let mut scenes = Vec::new();
		let mut sounds = Vec::new();
		for action in Action::listed(&state, &self.mods) {
			match &action {
				Action::Scene(name) => scenes.push(Pad {
					name: name.clone(),
					refused: self.refused(&state, &action),
				}),
				Action::Mute(name) => sounds.push(Mute {
					name: name.clone(),
					muted: collection.audio.get(name).is_some_and(|audio| audio.muted),
				}),
				_ => {}
			}
		}
		let (air, clock) = match &on_air {
			OnAir::Off => (Air::Off, None),
			OnAir::Starting => (Air::Starting, None),
			OnAir::Live(live) => {
				let on = kio::clock::now().saturating_duration_since(live.since);
				(Air::Live, Some(on.as_secs()))
			}
			OnAir::Ending => (Air::Ending, None),
		};
		Desk {
			air,
			clock,
			studio,
			program: state.compositor.program_scene().map(str::to_string),
			cued: state.cued().map(str::to_string),
			scenes,
			take_refused: self.refused(&state, &Action::Cut),
			sounds,
			done: None,
		}
	}

	fn frame(&self) -> Duration {
		let framerate = self.engine.state.lock().unwrap().compositor.canvas().framerate;
		Duration::from_secs(1) / framerate.max(1)
	}

	fn press(self: Arc<Self>, action: &Action) -> Result<String, String> {
		// Pairings outlast the mod, but drive nothing while it is off, nor
		// anything whose own mod is off: `fire` lets TAKE and AUTO through
		// for the studio's off-air banner, which a phone never shows.
		let mut needs = [Some(Mod::Remote), action.needs()].into_iter().flatten();
		if let Some(off) = needs.find(|needs| !self.mods.on(*needs)) {
			return Err(format!("{} is off in the studio's Settings, Mods", off.name()));
		}
		let result = self.fire(action).map_err(|err| format!("{err:#}"));
		// The status bar says what a remote did, as it does a hotkey.
		self.report(action, result.clone());
		result
	}
}

/// Reads the presses of the room's remotes and publishes their desks, for as
/// long as the engine follows the room.
pub struct Service {
	room: Arc<Room>,
	board: Weak<dyn Board>,
}

impl Service {
	/// Let `room`'s remotes drive `board`.
	pub fn new(room: Arc<Room>, board: Weak<dyn Board>) -> Self {
		Self { room, board }
	}

	/// Serve each remote paired and not removed, through the studio's session
	/// in the room, until dropped. A room with no session serves none.
	pub async fn run(self) {
		let (Some(publish), Some(heard)) = (self.room.origin.get(), self.room.heard.get()) else {
			return;
		};
		let mut changes = self.room.remotes.changed.subscribe();
		let mut serving = BTreeMap::<String, Serving>::new();
		loop {
			let read = self.room.remotes.read();
			serving.retain(|name, serving| read.contains(&(name.clone(), serving.generation)));
			for (name, generation) in read {
				if serving.contains_key(&name) {
					continue;
				}
				let paired = Paired {
					room: self.room.clone(),
					board: self.board.clone(),
					name: name.clone(),
					generation,
					publish: publish.clone(),
				};
				let task = tokio::spawn(paired.serve(heard.consume()));
				let served = Serving {
					room: self.room.clone(),
					name: name.clone(),
					generation,
					task,
				};
				serving.insert(name, served);
			}
			// The room outlives this loop, so its changes never end.
			if changes.changed().await.is_err() {
				return;
			}
		}
	}
}

/// A remote served, until dropped.
struct Serving {
	room: Arc<Room>,
	name: String,
	generation: u64,
	task: tokio::task::JoinHandle<()>,
}

impl Drop for Serving {
	fn drop(&mut self) {
		self.task.abort();
		self.room.remotes.connect(&self.name, false);
	}
}

/// One remote the studio serves.
struct Paired {
	room: Arc<Room>,
	board: Weak<dyn Board>,
	name: String,
	generation: u64,
	/// What the studio's session in the room publishes.
	publish: moq_net::origin::Producer,
}

/// A remote's desk, published until dropped.
struct Published {
	route: moq_net::origin::Route,
	tokens: moq_net::track::Producer,
	/// The key of the next token's group, since the token track has no timescale.
	next: u64,
	desks: moq_json::snapshot::Producer<Desk>,
	/// The key of the next desk's group, likewise.
	written: u64,
}

impl Drop for Published {
	fn drop(&mut self) {
		let _ = self.tokens.clone().finish();
		let _ = self.desks.clone().finish();
		self.route.broadcast().close();
	}
}

impl Paired {
	/// Read the presses of each page that connects, one after another, and
	/// publish it a desk while it is connected, until dropped.
	async fn serve(self, heard: moq_net::origin::Consumer) {
		let path = match press_path(&self.name, self.generation) {
			Ok(path) => path,
			Err(err) => return tracing::error!(remote = self.name, error = format!("{err:#}"), "cannot read a remote"),
		};
		let mut following = match heard.follow(&path, None) {
			Ok(following) => following,
			Err(err) => return tracing::error!(remote = self.name, %err, "cannot read a remote"),
		};
		// How the last press went, across reconnects, so one read again is not done again.
		let mut done = None;
		let mut next = following.available().await;
		loop {
			let broadcast = match next {
				Ok(Some(broadcast)) => broadcast,
				// Not connected until a page serves the path.
				Ok(None) => {
					next = following.available().await;
					continue;
				}
				// A closed origin answers every later call at once.
				Err(moq_net::Error::Closed) => return,
				Err(err) => {
					tracing::debug!(remote = self.name, %err, "cannot follow a remote");
					next = following.available().await;
					continue;
				}
			};
			self.room.remotes.connect(&self.name, true);
			let changed = tokio::select! {
				result = self.drive(broadcast, &mut done) => {
					if let Err(err) = result {
						tracing::debug!(remote = self.name, error = format!("{err:#}"), "stopped reading a remote");
					}
					None
				}
				changed = following.available() => Some(changed),
			};
			// Not connected while waiting for the page's next broadcast.
			self.room.remotes.connect(&self.name, false);
			next = match changed {
				Some(changed) => changed,
				None => following.available().await,
			};
		}
	}

	/// Publish the page at `broadcast` its desk and renew its token, and do
	/// each press it makes, keeping how the last went in `done`, until it
	/// leaves or the studio closes.
	async fn drive(&self, broadcast: moq_net::broadcast::Consumer, done: &mut Option<Done>) -> anyhow::Result<()> {
		let route = self.publish.route(
			desk_path(&self.name, self.generation)?,
			moq_net::origin::RouteOptions::hidden(),
		)?;
		let tokens = route.broadcast().create_track(TOKEN_TRACK, None)?;
		let desk = route.broadcast().create_track(DESK_TRACK, None)?;
		// Each desk whole, so a page joining late reads one group.
		let desks = moq_json::snapshot::Producer::new(desk, moq_json::snapshot::Config::default().with_deltas(false));
		let mut published = Published {
			route,
			tokens,
			next: 0,
			desks,
			written: 0,
		};
		self.renew(&mut published)?;
		let board = self.board.upgrade().context("the studio closed")?;
		let desk = Desk {
			done: done.clone(),
			..board.desk()
		};
		self.show(&mut published, desk)?;
		drop(board);
		published.route.announce()?;

		let mut presses = Presses::new(broadcast.track(PRESS_TRACK)?);
		let period = self.room.expiry / 2;
		let mut renew = kio::clock::interval_at(kio::clock::now().saturating_add(period), period);
		// The desk is built again only on a press or a tally tick, since each
		// build waits on the state the render loop holds.
		let mut tally = kio::clock::interval_at(kio::clock::now().saturating_add(TALLY), TALLY);
		tally.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
		let mut listening = false;
		// When the next press may be read: a program frame after the last.
		let mut open = kio::clock::now();
		loop {
			let mut pressed = false;
			tokio::select! {
				_ = tally.tick() => {}
				_ = renew.tick() => {
					self.renew(&mut published)?;
					continue;
				}
				press = async {
					kio::clock::sleep_until(open).await;
					presses.latest().await
				} => {
					pressed = true;
					match press {
						Ok(Some(press)) => {
							let _reading = self.reading()?;
							*done = Some(self.handle(press, &mut listening, done.as_ref()));
						}
						Ok(None) => return Ok(()),
						Err(moq_json::Error::Net(err)) => return Err(err.into()),
						// Refused whole, as one the page cannot be told about.
						Err(err) => tracing::debug!(remote = self.name, %err, "refused a press"),
					}
				}
			}
			let board = self.board.upgrade().context("the studio closed")?;
			if pressed {
				// The frame now, as the show's frame rate may have changed.
				open = kio::clock::now().saturating_add(board.frame());
			}
			let desk = Desk {
				done: done.clone(),
				..board.desk()
			};
			self.show(&mut published, desk)?;
		}
	}

	/// Publish `desk`, unless it is the one published last.
	fn show(&self, published: &mut Published, desk: Desk) -> anyhow::Result<()> {
		let at = moq_net::Timestamp::unscaled(published.written);
		if published.desks.update(moq_json::Timed::new(&desk, at))?.is_some() {
			published.written += 1;
		}
		Ok(())
	}

	/// Hold the remote paired while a press is done or a token renewed, so
	/// neither follows its removal.
	fn reading(&self) -> anyhow::Result<RwLockReadGuard<'_, BTreeMap<String, Remote>>> {
		self.room
			.remotes
			.reading(&self.name, self.generation)
			.context("the remote was removed")
	}

	/// Publish the remote a fresh token.
	fn renew(&self, published: &mut Published) -> anyhow::Result<()> {
		let _reading = self.reading()?;
		let (token, _) = self.room.sign_remote(&self.name, self.generation)?;
		let at = moq_net::Timestamp::unscaled(published.next);
		published.tokens.write_frame(at, token)?;
		published.next += 1;
		Ok(())
	}

	/// Do `press`, saying how it went, after the `last` one. The first press
	/// read from a connection is one the studio may have done before it was
	/// listening, so it is never done again: it is the `last` if the page
	/// reconnected. The page names itself first on each connection, so any
	/// other first press is one the page made before the studio restarted or
	/// rejoined the room, which was almost surely done then; pressing it
	/// again could undo it, as with a mute, so the page is told to check.
	fn handle(&self, press: Press, listening: &mut bool, last: Option<&Done>) -> Done {
		let first = !std::mem::replace(listening, true);
		if first && let Some(last) = last.filter(|last| last.id == press.id && press.seq <= last.seq) {
			return last.clone();
		}
		let result = match press.action {
			None => Ok("connected".to_string()),
			Some(_) if first => Err(RESTARTED.to_string()),
			Some(action) if !action.remote() => Err(format!("{action} is not for a remote")),
			Some(action) => match self.board.upgrade() {
				Some(board) => board.press(&action),
				None => Err("the studio closed".to_string()),
			},
		};
		let (ok, said) = match result {
			Ok(said) => (true, said),
			Err(said) => (false, said),
		};
		Done {
			id: press.id,
			seq: press.seq,
			ok,
			said,
		}
	}
}

/// A remote's presses, each costing the studio one parse of at most
/// [`MAX_PRESS`] bytes: only the first frame of the newest group, decoded on
/// its own. The page writes each press as a group of its own; whatever else a
/// group holds, such as the merge patches a snapshot track may carry, is
/// never read, so a page cannot make one read do more work.
struct Presses {
	track: Pressing,
	decoder: moq_json::snapshot::Decoder<Press>,
}

enum Pressing {
	Subscribing(moq_net::track::Subscribing),
	Reading {
		// Boxed: the reader dwarfs the pending subscription.
		groups: Box<moq_net::track::Ordered>,
		/// The newest group, until its first frame is read.
		group: Option<moq_net::group::Consumer>,
		/// Whether the track has ended.
		ended: bool,
	},
}

impl Presses {
	fn new(track: moq_net::track::Consumer) -> Self {
		let mut config = moq_json::snapshot::consumer::Config::default();
		config.max_size = Some(MAX_PRESS);
		Self {
			track: Pressing::Subscribing(track.subscribe(None).into_inner()),
			decoder: moq_json::snapshot::Decoder::new(config),
		}
	}

	/// The newest press, skipping those a newer one superseded, or `None` once
	/// the page stops publishing. An error other than [`moq_json::Error::Net`]
	/// refuses that press alone.
	async fn latest(&mut self) -> Result<Option<Press>, moq_json::Error> {
		kio::wait(|waiter| self.poll_latest(waiter)).await
	}

	fn poll_latest(&mut self, waiter: &kio::Waiter) -> Poll<Result<Option<Press>, moq_json::Error>> {
		if let Pressing::Subscribing(pending) = &self.track {
			let groups = ready!(pending.poll_ok(waiter))?.ordered();
			self.track = Pressing::Reading {
				groups: Box::new(groups),
				group: None,
				ended: false,
			};
		}
		let Pressing::Reading { groups, group, ended } = &mut self.track else {
			unreachable!("subscribed above");
		};
		loop {
			// The newest group the reader has; a newer one supersedes the rest.
			while !*ended {
				match groups.poll_next_group(waiter)? {
					Poll::Ready(Some(item)) => {
						if let Some(newer) = item.group() {
							*group = Some(newer);
						}
					}
					Poll::Ready(None) => *ended = true,
					Poll::Pending => break,
				}
			}
			let Some(current) = group else {
				return match ended {
					true => Poll::Ready(Ok(None)),
					false => Poll::Pending,
				};
			};
			let read = ready!(current.poll_read_frame(waiter));
			// One frame a group, whatever follows it.
			*group = None;
			match read {
				Ok(Some(frame)) => {
					self.decoder.snapshot(&frame.payload)?;
					return Poll::Ready(self.decoder.decode());
				}
				Ok(None) => continue,
				Err(err) => {
					// A newer group follows, as a lost one is superseded.
					tracing::debug!(%err, "lost a press");
					continue;
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Mutex;

	use super::super::tests::{EXPIRY, Relay, fragment, key};
	use super::*;
	use url::Url;

	/// A board that keeps what it was asked to do, and how often its desk was built.
	struct Fake {
		desk: Mutex<Desk>,
		frame: Mutex<Duration>,
		pressed: Mutex<Vec<Action>>,
		built: std::sync::atomic::AtomicUsize,
	}

	impl Fake {
		fn new(frame: Duration) -> Arc<Self> {
			let desk = Desk {
				scenes: vec![Pad {
					name: "Gameplay".into(),
					refused: None,
				}],
				sounds: vec![Mute {
					name: "Desk mic".into(),
					muted: false,
				}],
				..Desk::default()
			};
			Arc::new(Self {
				desk: Mutex::new(desk),
				frame: Mutex::new(frame),
				pressed: Mutex::default(),
				built: Default::default(),
			})
		}

		fn pressed(&self) -> Vec<Action> {
			self.pressed.lock().unwrap().clone()
		}

		fn built(&self) -> usize {
			self.built.load(std::sync::atomic::Ordering::Relaxed)
		}
	}

	impl Board for Fake {
		fn desk(&self) -> Desk {
			self.built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
			self.desk.lock().unwrap().clone()
		}

		fn frame(&self) -> Duration {
			*self.frame.lock().unwrap()
		}

		fn press(self: Arc<Self>, action: &Action) -> Result<String, String> {
			self.pressed.lock().unwrap().push(action.clone());
			Ok(format!("did {action}"))
		}
	}

	fn room(key: moq_auth::Key) -> Room {
		let connect = Url::parse("https://relay.example/show").unwrap();
		let page = Url::parse("https://studio.example/join/").unwrap();
		Room::new(&connect, "guests", key, page, EXPIRY).unwrap()
	}

	fn press(seq: u64, action: Option<Action>) -> Press {
		Press {
			id: "page".into(),
			seq,
			action,
		}
	}

	/// A page publishing presses for the remote called `name` in
	/// `generation` into `origin`, as `js/join/src/remote.ts` does.
	struct Page {
		_route: moq_net::origin::Route,
		track: moq_net::track::Producer,
		presses: moq_json::snapshot::Producer<Press>,
		/// The next group's start, as each write is a group of its own.
		next: u64,
	}

	impl Page {
		fn open(origin: &moq_net::origin::Producer, name: &str, generation: u64) -> Self {
			let route = origin
				.route(press_path(name, generation).unwrap(), Default::default())
				.unwrap();
			let track = route.broadcast().create_track(PRESS_TRACK, None).unwrap();
			let config = moq_json::snapshot::Config::default().with_deltas(false);
			Self {
				_route: route,
				track: track.clone(),
				presses: moq_json::snapshot::Producer::new(track, config),
				next: 0,
			}
		}

		fn press(&mut self, press: Press) {
			let at = moq_net::Timestamp::unscaled(self.next);
			self.next += 1;
			self.presses.update(moq_json::Timed::new(&press, at)).unwrap();
		}

		/// Write each of `frames` in one group, whatever they are.
		fn raw(&mut self, frames: &[&[u8]]) {
			let at = moq_net::Timestamp::unscaled(self.next);
			self.next += 1;
			let mut group = self.track.append_group(at).unwrap();
			for frame in frames {
				group.write_frame(at, frame.to_vec()).unwrap();
			}
			group.finish().unwrap();
		}
	}

	/// The desks the studio publishes the remote called `name` in `generation`.
	async fn read_desks(
		origin: moq_net::origin::Consumer,
		name: &str,
		generation: u64,
	) -> moq_json::snapshot::Consumer<Desk> {
		let broadcast = origin
			.follow(desk_path(name, generation).unwrap(), None)
			.unwrap()
			.available()
			.await
			.unwrap()
			.expect("an instance");
		moq_json::snapshot::Consumer::new(broadcast.track(DESK_TRACK).unwrap(), Default::default())
	}

	/// The first desk that says how the press numbered `seq` went.
	async fn desk(desks: &mut moq_json::snapshot::Consumer<Desk>, seq: u64) -> Desk {
		loop {
			let desk = desks.latest().await.unwrap().expect("a desk").value;
			if desk.done.as_ref().is_some_and(|done| done.seq >= seq) {
				return desk;
			}
		}
	}

	/// How the press numbered `seq` went, once the desk says.
	async fn done(desks: &mut moq_json::snapshot::Consumer<Desk>, seq: u64) -> Done {
		desk(desks, seq).await.done.expect("how it went")
	}

	/// A link pairs one remote, whose token publishes only its presses and
	/// reads only its desk, in its generation, which a removal moves on.
	#[test]
	fn a_link_pairs_one_remote_in_its_generation() {
		let key = key("show/guests", &["**"], &["**"]);
		let room = room(key.clone());
		let invite = room.pair(" phone ").unwrap();
		assert_eq!(invite.link.path(), "/join/remote.html");
		let link = fragment(&invite.link);
		assert_eq!(link["url"], "https://relay.example/show/guests");
		assert_eq!(link["name"], "phone");
		assert_eq!(link["generation"], "0");
		let claims = key.verify(&link["jwt"]).unwrap();
		assert_eq!(claims.root, "show/guests");
		let patterns = |texts: &[&str]| texts.iter().map(|text| text.parse().unwrap()).collect();
		assert_eq!(claims.publish.path, patterns(&[".press/phone/0"]));
		assert_eq!(claims.subscribe.path, patterns(&[".remote/phone/0"]));

		// A new link while paired keeps the generation, and a removal moves it on.
		assert_eq!(fragment(&room.pair("phone").unwrap().link)["generation"], "0");
		room.unpair("phone").unwrap();
		assert!(room.remotes.read().is_empty(), "a removed remote is still read");
		assert_eq!(fragment(&room.pair("phone").unwrap().link)["generation"], "1");
		assert_eq!(room.remotes.read(), [("phone".to_string(), 1)]);

		for name in ["", " ", "a/b", ".hidden"] {
			assert!(room.pair(name).is_err(), "{name:?}");
		}
		assert_eq!(room.remotes().len(), 1, "a refused name is paired");

		// The remote's page is beside the join page, which a directory without
		// its `/` cannot say where.
		let connect = Url::parse("https://relay.example/show").unwrap();
		let at = |page: &str| {
			let page = Url::parse(page).unwrap();
			let room = Room::new(&connect, "guests", key.clone(), page, EXPIRY).unwrap();
			room.pair("phone").map(|invite| invite.link.path().to_string())
		};
		assert_eq!(
			at("https://studio.example/join/index.html").unwrap(),
			"/join/remote.html"
		);
		assert_eq!(at("https://studio.example/").unwrap(), "/remote.html");
		let err = at("https://studio.example/join").unwrap_err();
		assert!(format!("{err:#}").contains("end it with `/`"), "{err:#}");
	}

	/// Who is paired, removed, and in which generation outlasts a restart,
	/// whichever scene file the studio opens, so a removed remote stays
	/// unread.
	#[test]
	fn pairing_outlasts_a_restart() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("room.jwk");
		key("show/guests", &["**"], &["**"]).to_file(&path).unwrap();
		let connect = Url::parse("https://relay.example/show").unwrap();
		let config = super::super::Config {
			room: Some("guests".into()),
			room_key: Some(path),
			join_page: None,
			guest_expiry: Some(EXPIRY.into()),
		};
		let start = || config.load(Some(&connect)).unwrap().unwrap();

		let room = start();
		room.pair("phone").unwrap();
		room.pair("tablet").unwrap();
		room.unpair("tablet").unwrap();
		let file = dir.path().join("room.remotes.toml");
		assert!(file.exists());

		let room = start();
		assert_eq!(room.remotes.read(), [("phone".to_string(), 0)]);
		assert!(room.remotes()["tablet"].removed);
		room.pair("tablet").unwrap();
		assert_eq!(start().remotes()["tablet"].generation, 1);

		// The studio leaving the room for the next, which reads the file only
		// once the room left can change it no more: a removal there would be
		// missed, and an unexpired link would press again.
		room.leave();
		let next = start();
		let err = room.unpair("phone").unwrap_err();
		assert_eq!(format!("{err:#}"), super::super::LEFT);
		assert!(!room.remotes()["phone"].removed);
		assert!(room.pair("laptop").is_err(), "a pairing the next room would miss");
		assert!(
			room.stop_renewing("alice").is_err(),
			"a guest's removal the next room would miss"
		);
		assert!(
			room.invite("bob").is_err(),
			"a guest's link the next room would not renew"
		);
		next.unpair("phone").unwrap();
		assert!(start().remotes()["phone"].removed);
		next.pair("phone").unwrap();
		let room = next;

		// A removal the file cannot hold is not made.
		std::fs::remove_file(&file).unwrap();
		std::fs::create_dir(&file).unwrap();
		assert!(room.unpair("phone").is_err());
		assert!(!room.remotes()["phone"].removed);
	}

	/// A page's presses drive the board, its newest only, at most one a
	/// frame, and only those a remote may fire; the desk says how each went.
	/// A page's first press is never done, since the studio may have done it
	/// before it restarted. A removal stops reading it at once.
	#[tokio::test]
	async fn a_remote_drives_the_board_one_press_a_frame() {
		const FRAME: Duration = Duration::from_millis(20);
		let clock = kio::clock::Manual::new().install(&tokio::runtime::Handle::current());
		let room = Arc::new(room(key("show/guests", &["**"], &["**"])));
		let publish = moq_tokio::origin::spawn();
		let heard = moq_tokio::origin::spawn();
		let _ = room.origin.set(publish.clone());
		let _ = room.heard.set(heard.clone());
		let fake = Fake::new(FRAME);
		let board: Arc<dyn Board> = fake.clone();
		let _service = tokio::spawn(Service::new(room.clone(), Arc::downgrade(&board)).run());
		room.pair("phone").unwrap();

		let mut page = Page::open(&heard, "phone", 0);
		page.press(press(0, None));
		let mut desks = read_desks(publish.consume(), "phone", 0).await;
		let connected = desk(&mut desks, 0).await;
		assert_eq!(connected.done.unwrap().said, "connected");
		assert_eq!(connected.scenes, fake.desk.lock().unwrap().scenes, "the board's desk");
		assert!(room.remotes()["phone"].connected);

		let scene = Action::Scene("Gameplay".into());
		page.press(press(1, Some(scene.clone())));
		clock.advance(FRAME);
		let first = done(&mut desks, 1).await;
		assert!(first.ok, "{first:?}");
		assert_eq!(fake.pressed(), std::slice::from_ref(&scene));

		// A burst within one frame: only the newest is done, a frame later.
		for seq in 2..=11 {
			page.press(press(seq, Some(Action::Mute(format!("sound {seq}")))));
		}
		clock.advance(FRAME);
		assert_eq!(done(&mut desks, 2).await.seq, 11);
		assert_eq!(fake.pressed(), [scene.clone(), Action::Mute("sound 11".into())]);

		// Outside the vocabulary.
		page.press(press(12, Some(Action::GoLive)));
		clock.advance(FRAME);
		let refused = done(&mut desks, 12).await;
		assert!(!refused.ok);
		assert_eq!(refused.said, "Go live is not for a remote");
		assert_eq!(fake.pressed().len(), 2);

		// Not a press, or more than one could be: refused whole, saying
		// nothing, and the next press is still read.
		// Each is read on a frame of its own, as `latest` skips any it passed.
		let large = vec![b' '; MAX_PRESS + 1];
		for frame in [
			br#"{"id":"page","seq":13,"action":{"Launch":"rockets"}}"#.as_slice(),
			&large,
		] {
			page.raw(&[frame]);
			clock.advance(FRAME);
			for _ in 0..20 {
				tokio::task::yield_now().await;
			}
		}
		// A press is its group's first frame alone: patches after it, as many
		// as a page sends, are never applied.
		let patch = br#"{"action":{"Scene":"Gameplay"}}"#.as_slice();
		let mut frames = vec![br#"{"id":"page","seq":13,"action":"GoLive"}"#.as_slice()];
		frames.extend(std::iter::repeat_n(patch, 1000));
		page.raw(&frames);
		clock.advance(FRAME);
		let refused = done(&mut desks, 13).await;
		assert_eq!(refused.said, "Go live is not for a remote");
		assert_eq!(fake.pressed().len(), 2);

		let mute = Action::Mute("Desk mic".into());
		page.press(press(14, Some(mute.clone())));
		clock.advance(FRAME);
		let muted = done(&mut desks, 14).await;
		assert!(muted.ok, "{muted:?}");
		assert_eq!(fake.pressed().len(), 3);

		// The page back on a new connection: the press read first is the
		// last it made, which is not done again, and the desk still says how
		// it went.
		drop(page);
		while desks.latest().await.unwrap().is_some() {}
		let built = fake.built();
		let mut page = Page::open(&heard, "phone", 0);
		page.press(press(14, Some(mute)));
		let mut desks = read_desks(publish.consume(), "phone", 0).await;
		clock.advance(FRAME);
		// Read once the desk is built again after the one the page connected to.
		for _ in 0..100 {
			if fake.built() >= built + 2 {
				break;
			}
			tokio::task::yield_now().await;
		}
		assert_eq!(fake.built(), built + 2, "the press is read");
		let replayed = desks.latest().await.unwrap().unwrap().value.done;
		assert_eq!(replayed, Some(muted), "the last press, again");
		assert_eq!(fake.pressed().len(), 3, "done again");

		// A page whose press the studio may have done before it restarted,
		// which is told to check it, not done again.
		drop(page);
		while desks.latest().await.unwrap().is_some() {}
		let mut page = Page::open(&heard, "phone", 0);
		page.press(Press {
			id: "reloaded".into(),
			..press(1, Some(scene.clone()))
		});
		let mut desks = read_desks(publish.consume(), "phone", 0).await;
		clock.advance(FRAME);
		let again = loop {
			let desk = desks.latest().await.unwrap().unwrap().value;
			if let Some(done) = desk.done.filter(|done| done.id == "reloaded") {
				break done;
			}
		};
		assert!(!again.ok);
		assert_eq!(again.said, RESTARTED);
		assert_eq!(fake.pressed().len(), 3, "done again");

		// Removed while a press waits to be read: it is never done, and the
		// desk ends.
		page.press(press(2, Some(scene)));
		clock.advance(FRAME);
		room.unpair("phone").unwrap();
		while desks.latest().await.is_ok_and(|desk| desk.is_some()) {}
		for _ in 0..20 {
			tokio::task::yield_now().await;
		}
		assert_eq!(fake.pressed().len(), 3, "a removed remote drives");
		assert!(!room.remotes()["phone"].connected);
	}

	/// A connected page's desk is built on a press and at the tally rate,
	/// not every program frame, and still shows what the studio changed
	/// itself. Presses wait out the frame the program shows now, whatever it
	/// was when the page connected.
	#[tokio::test]
	async fn a_remote_keeps_pace_with_the_show() {
		const FRAME: Duration = Duration::from_millis(20);
		let clock = kio::clock::Manual::new().install(&tokio::runtime::Handle::current());
		let room = Arc::new(room(key("show/guests", &["**"], &["**"])));
		let publish = moq_tokio::origin::spawn();
		let heard = moq_tokio::origin::spawn();
		let _ = room.origin.set(publish.clone());
		let _ = room.heard.set(heard.clone());
		let fake = Fake::new(FRAME);
		let board: Arc<dyn Board> = fake.clone();
		let _service = tokio::spawn(Service::new(room.clone(), Arc::downgrade(&board)).run());
		room.pair("phone").unwrap();

		let mut page = Page::open(&heard, "phone", 0);
		page.press(press(0, None));
		let mut desks = read_desks(publish.consume(), "phone", 0).await;
		desk(&mut desks, 0).await;

		// A second of program frames with nothing pressed builds a desk a tally tick.
		let built = fake.built();
		for _ in 0..Duration::from_secs(1).div_duration_f64(FRAME) as u32 {
			clock.advance(FRAME);
			for _ in 0..20 {
				tokio::task::yield_now().await;
			}
		}
		let ticks = Duration::from_secs(1).div_duration_f64(TALLY) as usize;
		assert_eq!(fake.built() - built, ticks, "desks built in a second");

		// What the studio changed itself reaches the page within a tick.
		fake.desk.lock().unwrap().program = Some("Gameplay".into());
		clock.advance(TALLY);
		let program = loop {
			let desk = desks.latest().await.unwrap().unwrap().value;
			if desk.program.is_some() {
				break desk.program;
			}
		};
		assert_eq!(program.as_deref(), Some("Gameplay"));

		// The show slows to a frame a second: the next press waits for it.
		*fake.frame.lock().unwrap() = Duration::from_secs(1);
		let scene = Action::Scene("Gameplay".into());
		page.press(press(1, Some(scene.clone())));
		clock.advance(FRAME);
		assert!(done(&mut desks, 1).await.ok);
		page.press(press(2, Some(scene.clone())));
		for _ in 0..10 {
			clock.advance(FRAME);
			for _ in 0..20 {
				tokio::task::yield_now().await;
			}
		}
		assert_eq!(fake.pressed().len(), 1, "pressed within the program's frame");
		clock.advance(Duration::from_secs(1));
		assert!(done(&mut desks, 2).await.ok);
		assert_eq!(fake.pressed(), [scene.clone(), scene]);

		// The page stops pressing while its broadcast stays announced: its desk
		// ends, and it is no longer connected.
		let _finished = page.track.clone().finish().unwrap();
		clock.advance(Duration::from_secs(1));
		while desks.latest().await.unwrap().is_some() {}
		assert!(!room.remotes()["phone"].connected, "connected after its presses end");
	}

	/// A page that opens the link reaches the relay with its token, presses
	/// through it to the studio's session in the room, and reads its desk.
	#[tokio::test(flavor = "multi_thread")]
	async fn a_remote_drives_through_a_relay_that_verifies_tokens() {
		const TIMEOUT: Duration = Duration::from_secs(10);
		let relay = Relay::start().await;
		let room = relay.room();
		let fake = Fake::new(Duration::from_millis(10));
		let board: Arc<dyn Board> = fake.clone();
		let _service = tokio::spawn(Service::new(room.clone(), Arc::downgrade(&board)).run());

		let published = moq_tokio::origin::spawn();
		let read = moq_tokio::origin::spawn();
		let _page = relay.join(&room.pair("phone").unwrap().link, &published, &read);
		let mut page = Page::open(&published, "phone", 0);
		page.press(press(0, None));
		let mut desks = kio::clock::timeout(TIMEOUT, read_desks(read.consume(), "phone", 0))
			.await
			.expect("the studio publishes the desk");
		kio::clock::timeout(TIMEOUT, done(&mut desks, 0))
			.await
			.expect("the studio hears the page");

		let scene = Action::Scene("Gameplay".into());
		page.press(press(1, Some(scene.clone())));
		let done = kio::clock::timeout(TIMEOUT, done(&mut desks, 1))
			.await
			.expect("the press reaches the studio");
		assert!(done.ok, "{done:?}");
		assert_eq!(fake.pressed(), [scene]);
	}
}
