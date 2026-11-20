//! What the operator does from a button, a hotkey, or the command palette:
//! take a scene, mute a sound, panic, replay, save a clip, drop a marker,
//! record, and go live and end the stream, each registered once in the table
//! here. Hotkeys fire from outside the window's frames, so a key pressed while
//! a game has focus acts at once, even while the window is hidden and draws
//! nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use kio::clock::Instant;
use moq_scene::Source;

use crate::engine::{Engine, OnAir, State};
use crate::markers::Markers;
use crate::replay;
use crate::stream::{Stream, Switched};

/// Something the studio does, from a hotkey, the command palette, or a
/// button. Bindings are saved by the variant's name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Action {
	/// TAKE: put the cued scene on program at once.
	Cut,
	/// AUTO: take the cued scene to program by the show's transition.
	Transition,
	/// Take a scene, by name, on program by the show's transition, or cue it
	/// while Studio mode is on.
	Scene(String),
	/// Turn Studio mode on or off.
	StudioMode,
	/// Play the stretch of the recent past REVIEW picks in its replay source.
	Replay,
	/// Save the stretch of the recent past REVIEW picks as a clip.
	SaveClip,
	/// Mark this moment of the show.
	Marker,
	/// Start recording, or stop the recording under way.
	Record,
	/// Cut to the safe scene and mute every microphone and guest.
	Panic,
	/// Go live to the relays switched on, skipping the go-live sheet's checks.
	GoLive,
	/// End the stream, handing the relays its tail first.
	EndStream,
	/// Mute a sound, by its source's name, or unmute it.
	Mute(String),
}

/// What an action is for, which the Hotkeys pane and the palette group it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
	Live,
	StudioMode,
	Scenes,
	Sounds,
	Review,
}

impl Group {
	/// Each group, in the order the Hotkeys pane and the palette list them.
	pub const ALL: [Self; 5] = [Self::Live, Self::StudioMode, Self::Scenes, Self::Sounds, Self::Review];

	pub fn name(self) -> &'static str {
		match self {
			Self::Live => "Live",
			Self::StudioMode => "Studio mode",
			Self::Scenes => "Scenes",
			Self::Sounds => "Sounds",
			Self::Review => "Review",
		}
	}
}

/// An action every show has, as the table registers it.
struct Fixed {
	action: Action,
	group: Group,
	/// What the desktop's shortcut settings keep its key under, the same
	/// across runs, so a key the operator picked there stays.
	id: &'static str,
	name: &'static str,
	/// A word or two the Hotkeys pane shows beside the name.
	detail: Option<&'static str>,
	/// What else the palette finds it by.
	words: &'static [&'static str],
}

/// The actions every show has, each registered once, in the order the
/// Hotkeys pane and the palette list them within their group. Each scene and
/// sound adds its own: see [`Action::listed`].
const FIXED: [Fixed; 10] = [
	Fixed {
		action: Action::GoLive,
		group: Group::Live,
		id: "go-live",
		name: "Go live",
		detail: None,
		words: &["start", "stream", "broadcast"],
	},
	Fixed {
		action: Action::EndStream,
		group: Group::Live,
		id: "end-stream",
		name: "End stream",
		detail: Some("held"),
		words: &["stop", "off air", "finish"],
	},
	Fixed {
		action: Action::Panic,
		group: Group::Live,
		id: "panic",
		name: "Panic",
		detail: Some("safe scene, mics muted"),
		words: &["safe", "emergency", "mute all", "hide"],
	},
	Fixed {
		action: Action::Record,
		group: Group::Live,
		id: "record",
		name: "Start or stop recording",
		detail: None,
		words: &["rec"],
	},
	Fixed {
		action: Action::StudioMode,
		group: Group::StudioMode,
		id: "studio-mode",
		name: "Turn Studio mode on or off",
		detail: None,
		words: &["preview", "cue"],
	},
	Fixed {
		action: Action::Cut,
		group: Group::StudioMode,
		id: "cut",
		name: "Take the cued scene",
		detail: None,
		words: &["cut"],
	},
	Fixed {
		action: Action::Transition,
		group: Group::StudioMode,
		id: "transition",
		name: "Auto: take the cued scene by the transition",
		detail: None,
		words: &["fade"],
	},
	Fixed {
		action: Action::Replay,
		group: Group::Review,
		id: "replay",
		name: "Replay",
		detail: None,
		words: &["instant", "play back"],
	},
	Fixed {
		action: Action::SaveClip,
		group: Group::Review,
		id: "save-clip",
		name: "Save clip",
		detail: None,
		words: &["highlight"],
	},
	Fixed {
		action: Action::Marker,
		group: Group::Review,
		id: "marker",
		name: "Drop a marker",
		detail: None,
		words: &["mark", "flag", "bookmark", "moment"],
	},
];

impl Action {
	/// Every action `state`'s show has, by group, in the order the Hotkeys
	/// pane and the palette list them: the table's, a scene's for each scene,
	/// and a mute for each sound.
	pub fn listed(state: &State) -> Vec<Self> {
		let scenes = state
			.collection()
			.scenes
			.iter()
			.map(|scene| Self::Scene(scene.name.clone()));
		let sounds = state
			.sources
			.iter()
			.filter(|(_, feed)| feed.strip().is_some())
			.map(|(name, _)| Self::Mute(name.to_string()));
		let mut listed: Vec<Self> = FIXED.iter().map(|fixed| fixed.action.clone()).collect();
		listed.extend(scenes.chain(sounds));
		// Stable, so each group keeps its order.
		listed.sort_by_key(|action| Group::ALL.iter().position(|group| *group == action.group()));
		listed
	}

	fn fixed(&self) -> &'static Fixed {
		FIXED
			.iter()
			.find(|fixed| fixed.action == *self)
			.expect("every action without a name is in the table")
	}

	/// What it is for.
	pub fn group(&self) -> Group {
		match self {
			Self::Scene(_) => Group::Scenes,
			Self::Mute(_) => Group::Sounds,
			fixed => fixed.fixed().group,
		}
	}

	/// What the desktop's shortcut settings keep its key under, the same
	/// across runs, or `None` for a scene's, which is named for its key so
	/// it follows the scene through renames. A source is never renamed.
	pub fn id(&self) -> Option<String> {
		match self {
			Self::Scene(_) => None,
			Self::Mute(name) => Some(format!("mute/{name}")),
			fixed => Some(fixed.fixed().id.into()),
		}
	}

	/// What it does with Studio mode on or off, as the Hotkeys tab, the
	/// palette, and a hotkey's message name it.
	pub fn name(&self, studio: bool) -> String {
		match (self, studio) {
			(Self::Scene(name), true) => format!("Cue scene `{name}`"),
			(Self::Scene(name), false) => format!("Take scene `{name}`"),
			_ => self.to_string(),
		}
	}

	/// A word or two the Hotkeys pane shows beside the name, with Studio mode
	/// on or off.
	pub fn detail(&self, studio: bool) -> Option<&'static str> {
		match self {
			Self::Scene(_) => studio.then_some("cues"),
			Self::Mute(_) => None,
			fixed => fixed.fixed().detail,
		}
	}

	/// What else the palette finds it by, besides its name and group.
	pub fn words(&self) -> &'static [&'static str] {
		match self {
			Self::Scene(_) => &["take", "cue", "pad", "switch"],
			Self::Mute(_) => &["unmute", "sound", "audio", "mic", "silence"],
			fixed => fixed.fixed().words,
		}
	}

	/// Whether a phone paired as a remote may fire it: what its page shows,
	/// a scene's pad, a mute, TAKE, and AUTO. Every action says, so a new one
	/// is kept off the network until it is placed here.
	pub fn remote(&self) -> bool {
		match self {
			Self::Scene(_) | Self::Mute(_) | Self::Cut | Self::Transition => true,
			Self::StudioMode
			| Self::Replay
			| Self::SaveClip
			| Self::Record
			| Self::Panic
			| Self::GoLive
			| Self::EndStream => false,
		}
	}
}

impl std::fmt::Display for Action {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			// The desktop's shortcut settings show it whichever mode is on.
			Self::Scene(name) => write!(f, "Take or cue scene `{name}`"),
			Self::Mute(name) => write!(f, "Mute or unmute `{name}`"),
			fixed => f.write_str(fixed.fixed().name),
		}
	}
}

/// How long a go-live waits for encoder checks still running.
const PROBED: std::time::Duration = std::time::Duration::from_secs(10);

/// The encoder checks, each holding a hardware encode session the
/// program's would count against, and whether a go-live is opening the
/// program's, which admits no new check.
#[derive(Default)]
struct Probes {
	running: usize,
	starting: bool,
}

/// An encoder check running, until dropped: see [`Controls::probing`].
pub struct Probing(Arc<tokio::sync::watch::Sender<Probes>>);

impl Drop for Probing {
	fn drop(&mut self) {
		self.0.send_modify(|probes| probes.running -= 1);
	}
}

/// The engine as panels and hotkeys drive it, with the picks both share.
pub struct Controls {
	pub engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	/// Where the show streams, as Settings edits it.
	pub stream: Mutex<Stream>,
	/// Which relays going live sends to, as the go-live sheet switches them.
	pub switched: Mutex<Switched>,
	/// Why the last go-live failed, until the go-live sheet or the status bar
	/// shows it.
	pub refused: Mutex<Option<String>>,
	/// Whether a go-live is under way, from the press until it is live or
	/// refused, which covers reading the tokens before the engine says
	/// Starting, so a second press is refused at once.
	going: AtomicBool,
	/// Whether the show is closing, which refuses a go-live. Closing holds
	/// [`Self::going`] too, so the two are claimed by one atomic swap: a
	/// press that a go-live won is one closing refuses, and the reverse.
	closing: AtomicBool,
	/// Whether the relays wait to be followed: the show's, until its first
	/// follow, and those saved on air until the stream ends, since following
	/// them then would cut the broadcast sources and guests on program.
	rewatch: AtomicBool,
	/// Held while the relays are followed, and by a go-live while it follows
	/// any still waiting, so each follow reads the settings after the last
	/// one landed, and none is followed once it is on air.
	following: Mutex<()>,
	/// The relay and guest settings of the room last joined, and whether it
	/// could be.
	joined: Mutex<Option<(JoinedAt, Result<(), String>)>>,
	/// The encoder checks running, which a go-live waits out, a closed
	/// go-live sheet's included, whose check runs to its end, and admits no
	/// more of until it is live or refused.
	probes: Arc<tokio::sync::watch::Sender<Probes>>,
	/// REVIEW's replay picks, which a replay or clip from a hotkey takes too.
	pub replay: Mutex<Replay>,
	/// The moments marked this session.
	pub markers: Markers,
	/// Whether the next recording gives each source a file of its own too.
	pub record_sources: AtomicBool,
	/// Whether Studio mode is on: a scene's pad or key cues it rather than
	/// taking it.
	studio: AtomicBool,
	/// How the last action a hotkey fired went, until the status bar takes it.
	fired: Arc<Mutex<Option<Result<String, String>>>>,
}

/// What a guests' room is joined by: the relay, and the guest settings.
type JoinedAt = (Option<url::Url>, crate::guests::Config);

/// REVIEW's replay choices, and how the last replay or clip went.
pub struct Replay {
	pub of: replay::Of,
	pub stretch: replay::Stretch,
	pub speed: f32,
	/// The replay source a replay plays in.
	pub into: Option<String>,
	/// Set by the replay or save running in the background.
	pub outcome: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Default for Replay {
	fn default() -> Self {
		Self {
			of: replay::Of::Program,
			stretch: replay::Stretch::default(),
			speed: replay::SPEEDS[0],
			into: None,
			outcome: Arc::default(),
		}
	}
}

impl Controls {
	pub fn new(engine: Arc<Engine>, stream: Stream, runtime: tokio::runtime::Handle) -> Self {
		engine.set_guests_appear(stream.guests_appear());
		let markers = Markers::new(engine.recorder.show());
		Self {
			engine,
			runtime,
			stream: Mutex::new(stream),
			switched: Mutex::default(),
			refused: Mutex::default(),
			going: AtomicBool::new(false),
			closing: AtomicBool::new(false),
			rewatch: AtomicBool::new(false),
			following: Mutex::new(()),
			joined: Mutex::default(),
			probes: Arc::new(tokio::sync::watch::Sender::new(Probes::default())),
			replay: Mutex::default(),
			markers,
			record_sources: AtomicBool::new(false),
			studio: AtomicBool::new(false),
			fired: Arc::default(),
		}
	}

	/// Whether Studio mode is on.
	pub fn studio(&self) -> bool {
		self.studio.load(Ordering::Relaxed)
	}

	/// Turn Studio mode on or off.
	pub fn set_studio(&self, on: bool) {
		self.studio.store(on, Ordering::Relaxed);
	}

	/// Do what a scene's key does to the scene called `name`, under `state`
	/// locked by the caller: cue it in Studio mode, or else take it.
	pub fn scene(&self, state: &mut State, name: &str) -> anyhow::Result<String> {
		match self.studio() {
			true => cue(state, name),
			false => take(state, name),
		}
	}

	/// Do `action`, saying what it did.
	pub fn fire(self: &Arc<Self>, action: &Action) -> anyhow::Result<String> {
		match action {
			Action::Cut => {
				let scene = self.engine.state.lock().unwrap().take_cued(None)?;
				Ok(format!("took `{scene}`"))
			}
			Action::Transition => {
				let mut state = self.engine.state.lock().unwrap();
				let transition = state.collection().transition.clone();
				let scene = state.take_cued(Some((transition, kio::clock::Instant::now())))?;
				Ok(format!("took `{scene}`"))
			}
			Action::Scene(name) => self.scene(&mut self.engine.state.lock().unwrap(), name),
			Action::StudioMode => {
				let on = !self.studio.fetch_xor(true, Ordering::Relaxed);
				Ok(match on {
					true => "Studio mode on: a scene's pad or key cues it".into(),
					false => "Studio mode off: a scene's pad or key takes it".into(),
				})
			}
			Action::Panic => {
				let rooms = self.engine.rooms();
				self.engine.state.lock().unwrap().panic(&rooms)
			}
			Action::Replay => self.replay(true, false, None),
			Action::SaveClip => self.replay(false, false, None),
			Action::Marker => {
				let number = self.markers.mark(&self.engine.recorder.dir(), Instant::now())?;
				Ok(format!("dropped marker {number}"))
			}
			Action::Record => {
				let recorder = &self.engine.recorder;
				match recorder.status().filter(|status| !status.done) {
					Some(status) if status.stopping => anyhow::bail!("the last recording is still finishing"),
					Some(_) => {
						recorder.stop();
						Ok("stopping the recording".into())
					}
					None => {
						recorder.start(self.record_sources.load(Ordering::Relaxed))?;
						Ok(format!("recording into {}", recorder.dir().display()))
					}
				}
			}
			Action::GoLive => self.go_live(),
			Action::EndStream => self.end_stream(),
			Action::Mute(name) => mute(&mut self.engine.state.lock().unwrap(), name),
		}
	}

	/// Why `action` cannot run now, under `state` locked by the caller, if it
	/// cannot, as the palette shows it beside the action.
	pub fn refused(&self, state: &State, action: &Action) -> Option<String> {
		let collection = state.collection();
		match action {
			Action::GoLive if self.engine.on_air() != OnAir::Off => Some("the program is already on air".into()),
			Action::GoLive if self.going() => Some("already going live".into()),
			Action::GoLive if self.closing.load(Ordering::Acquire) => Some("the show is closing".into()),
			Action::EndStream if self.engine.on_air().live().is_none() => Some("the program is not on air".into()),
			Action::Cut | Action::Transition => match state.cued() {
				None => Some("nothing is cued".into()),
				Some(scene) => collection.refuses(scene),
			},
			Action::Scene(name) => collection.refuses(name),
			Action::Mute(name) if !collection.sources.contains_key(name) => {
				Some(format!("there is no source `{name}`"))
			}
			Action::Marker => self.markers.refused(),
			// Whichever is picked, any replay source plays it.
			Action::Replay => replay_source(collection, None).err().map(|err| err.to_string()),
			Action::Record => self
				.engine
				.recorder
				.status()
				.is_some_and(|status| status.stopping && !status.done)
				.then(|| "the last recording is still finishing".into()),
			_ => None,
		}
	}

	/// Go live to the relays switched on, in the background: reading their
	/// tokens blocks on the credential store, and dialing them takes a while.
	/// How it went shows in the status bar, and a refusal in the go-live
	/// sheet too.
	pub fn go_live(self: &Arc<Self>) -> anyhow::Result<String> {
		anyhow::ensure!(
			self.engine.on_air() == OnAir::Off,
			"the program is already on air, or on its way on or off"
		);
		anyhow::ensure!(!self.closing.load(Ordering::Acquire), "the show is closing");
		anyhow::ensure!(!self.going.swap(true, Ordering::AcqRel), "already going live");
		*self.refused.lock().unwrap() = None;
		let controls = self.clone();
		let spawned = std::thread::Builder::new().name("go live".into()).spawn(move || {
			// Each relay's client binds its socket on the runtime.
			let _runtime = controls.runtime.enter();
			// One snapshot of the settings, both followed, if their relays wait
			// to be, and gone live with, after any follow already under way.
			// A save after it waits for this stream to end.
			let stream = {
				let _following = controls.following.lock().unwrap();
				let (stream, waiting) = controls.snapshot();
				controls.catch_up(&stream, waiting);
				stream
			};
			// Wait out the encoder checks still running before opening anything.
			controls.runtime.block_on(controls.probed());
			let switched = *controls.switched.lock().unwrap();
			let destination = stream.destination(switched);
			let result = destination.and_then(|destination| {
				let to = destination.broadcast.clone();
				controls.runtime.block_on(controls.engine.go_live(destination))?;
				Ok(format!("live as {to}"))
			});
			let result = result.map_err(|err| format!("{err:#}"));
			if let Err(err) = &result {
				*controls.refused.lock().unwrap() = Some(err.clone());
			}
			// Before `going`, which admits the next go-live, whose own gate this
			// would otherwise lift.
			controls.probes.send_modify(|probes| probes.starting = false);
			controls.going.store(false, Ordering::Release);
			// Relays saved while this started, followed now if it failed;
			// once live, when it ends. Whichever of a save and this runs
			// last follows them.
			drop(controls.follow_saved_relays());
			controls.report(&Action::GoLive, result);
		});
		if let Err(err) = spawned {
			self.going.store(false, Ordering::Release);
			return Err(err).context("cannot go live");
		}
		Ok("going live".into())
	}

	/// Note an encoder check running until the returned guard drops, which
	/// a go-live waits for before the program's encoders open, or `None`
	/// while a go-live is opening them, whose sessions it would take.
	pub fn probing(&self) -> Option<Probing> {
		let mut admitted = false;
		self.probes.send_if_modified(|probes| {
			admitted = !probes.starting;
			probes.running += usize::from(admitted);
			admitted
		});
		admitted.then(|| Probing(self.probes.clone()))
	}

	/// Admit no more encoder checks until this go-live ends, and wait out
	/// those running, bounded at [`PROBED`], past which the program's
	/// encoders open anyway and a refused one says why.
	async fn probed(&self) {
		self.probes.send_modify(|probes| probes.starting = true);
		let mut probes = self.probes.subscribe();
		let waited = kio::clock::timeout(PROBED, probes.wait_for(|probes| probes.running == 0)).await;
		if waited.is_err() {
			tracing::warn!("going live beside an encoder check still running");
		}
	}

	/// Whether a go-live is under way, from the press until it is live or
	/// refused.
	pub fn going(&self) -> bool {
		self.going.load(Ordering::Acquire)
	}

	/// Claim the show for closing, which refuses a go-live from now on, from a
	/// hotkey included, until [`Self::release_close`]. Refused while a stream
	/// is on air, on its way on or off, or going live, whose press the engine
	/// does not show until it has read the tokens and checked the encoders.
	pub fn reserve_close(&self) -> anyhow::Result<()> {
		anyhow::ensure!(
			!self.going.swap(true, Ordering::AcqRel),
			"end the stream first: it is going live"
		);
		if self.engine.on_air() != OnAir::Off {
			self.going.store(false, Ordering::Release);
			anyhow::bail!("end the stream first");
		}
		self.closing.store(true, Ordering::Release);
		Ok(())
	}

	/// Give the show back after a close that left it open.
	pub fn release_close(&self) {
		self.closing.store(false, Ordering::Release);
		self.going.store(false, Ordering::Release);
	}

	/// Take `stream` as the settings, as Settings saved them. When
	/// `followed_changed`, such as a new relay or guests' room, follow them
	/// now if off air, or once the stream ends; says whether they wait for it.
	/// Otherwise a guests' room that failed is tried again, off air.
	pub fn set_stream(self: &Arc<Self>, stream: Stream, followed_changed: bool) -> bool {
		self.engine.set_guests_appear(stream.guests_appear());
		{
			// Marked with the settings it belongs to, so a snapshot never
			// holds the new relays without knowing they wait to be followed.
			let mut current = self.stream.lock().unwrap();
			*current = stream;
			if followed_changed {
				self.rewatch.store(true, Ordering::Release);
			}
		}
		if !followed_changed {
			// A room that failed is tried again on any save, such as once its
			// key file is back, off air.
			if self.room_failure().is_some() {
				drop(self.follow_saved_relays());
			}
			return false;
		}
		let waits = self.going() || self.engine.on_air() != OnAir::Off;
		drop(self.follow_saved_relays());
		waits
	}

	/// Follow `stream`'s relays, which wait to be followed again if any could
	/// not be set up, such as on a token the credential store would not read,
	/// so the next go-live, end of stream, or save tries them again; and join
	/// its guests' room on them, unless joined already. Call it holding
	/// [`Self::following`], inside the runtime.
	fn follow(self: &Arc<Self>, stream: &Stream) {
		if !crate::show::follow(&self.engine, stream) {
			self.rewatch.store(true, Ordering::Release);
		}
		self.join_room(stream);
	}

	/// Follow `stream`'s relays if they `waiting` to be, or else join its
	/// guests' room if the last try failed, so a room whose key file is back
	/// joins at the next go-live, end of stream, or save, as well as on Try
	/// again. Call it holding [`Self::following`], inside the runtime.
	fn catch_up(self: &Arc<Self>, stream: &Stream, waiting: bool) {
		match waiting {
			true => self.follow(stream),
			false => self.join_room(stream),
		}
	}

	/// Join the guests' room `stream` names, unless joined to it already: a
	/// room that failed is tried again, such as once its key file is back.
	/// Call it holding [`Self::following`], which keeps two joins apart, as
	/// [`Self::joined`] is let go while one reads the key file, so the UI
	/// reading the failure never waits on the disk; inside the runtime.
	fn join_room(self: &Arc<Self>, stream: &Stream) {
		let room = (stream.relay().cloned(), stream.guests());
		{
			let mut joined = self.joined.lock().unwrap();
			match &*joined {
				Some((at, Ok(()))) if *at == room => return,
				// The engine starts in no room.
				None if room.1 == crate::guests::Config::default() => {
					*joined = Some((room, Ok(())));
					return;
				}
				_ => {}
			}
		}
		// Panic mutes its guests even if it cannot be joined, such as when
		// its key file is gone, since one already in a scene may still play.
		if let Some(path) = room.1.path() {
			self.engine.remember_room(&path);
		}
		let joining =
			crate::guests::Joining::connect(&room.1, room.0.as_ref(), &stream.client(), stream.quic().clone());
		// Weak, so the remotes never keep the controls, and the engine, alive.
		let board: std::sync::Weak<Self> = Arc::downgrade(self);
		let joining = joining.map(|joining| joining.map(|joining| joining.driving(board)));
		// A room that cannot be leaves the last one too, since its settings
		// are gone; it is tried again once they change.
		let result = match joining {
			Ok(joining) => self.engine.join(joining),
			Err(err) => self.engine.join(None).and(Err(err)),
		};
		if let Err(err) = &result {
			tracing::error!(error = format!("{err:#}"), "guests cannot join");
		}
		*self.joined.lock().unwrap() = Some((room, result.map_err(|err| format!("{err:#}"))));
	}

	/// Try the guests' room that failed again, on a thread of its own, since
	/// the credential store blocks. Off air only: on air, the settings may
	/// hold a room that waits for the stream to end, and its end tries again.
	pub fn retry_room(self: &Arc<Self>) {
		drop(self.follow_saved_relays());
	}

	/// Why guests cannot join the room the settings name, if they cannot.
	pub fn room_failure(&self) -> Option<String> {
		let joined = self.joined.lock().unwrap();
		joined.as_ref().and_then(|(_, result)| result.clone().err())
	}

	/// The settings, and whether their relays wait to be followed, which
	/// this takes on, as one, so a save never falls between the two.
	fn snapshot(&self) -> (Stream, bool) {
		let stream = self.stream.lock().unwrap();
		(stream.clone(), self.rewatch.swap(false, Ordering::AcqRel))
	}

	/// Follow broadcast sources, guests, and viewers' reports through the
	/// show's relays, as the settings hold them once the follow runs, in the
	/// background. Ordered with every save and go-live after it, as a save's
	/// follow is, so it never lands after a later one with relays since
	/// replaced, nor once a stream is on air.
	pub fn follow_relays(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
		self.rewatch.store(true, Ordering::Release);
		self.follow_saved_relays()
	}

	/// Follow the saved relays, if any wait to be, or join the guests' room
	/// if it failed, on a thread of its own, since the credential store
	/// blocks, unless a stream is on air or on its way, which follows them
	/// itself.
	fn follow_saved_relays(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
		let controls = self.clone();
		let spawned = std::thread::Builder::new().name("follow".into()).spawn(move || {
			let _following = controls.following.lock().unwrap();
			if controls.going() || controls.engine.on_air() != OnAir::Off {
				return;
			}
			let (stream, waiting) = controls.snapshot();
			// Each relay's client binds its socket on the runtime.
			let _runtime = controls.runtime.enter();
			controls.catch_up(&stream, waiting);
		});
		spawned
			.inspect_err(|err| tracing::warn!(error = format!("{err:#}"), "cannot follow the relays"))
			.ok()
	}

	/// End the stream in the background, handing the relays its tail first.
	pub fn end_stream(self: &Arc<Self>) -> anyhow::Result<String> {
		anyhow::ensure!(self.engine.on_air().live().is_some(), "the program is not on air");
		let controls = self.clone();
		self.runtime.spawn(async move {
			let result = controls.engine.end_stream().await;
			drop(controls.follow_saved_relays());
			let result = result.map(|()| "the stream ended".to_string());
			controls.report(&Action::EndStream, result.map_err(|err| format!("{err:#}")));
		});
		Ok("ending the stream".into())
	}

	/// Do `action` for a hotkey, keeping how it went for the status bar.
	pub fn press(self: &Arc<Self>, action: &Action) {
		match action {
			Action::Replay => drop(self.replay(true, true, None)),
			Action::SaveClip => drop(self.replay(false, true, None)),
			action => self.report(action, self.fire(action).map_err(|err| format!("{err:#}"))),
		}
	}

	/// Save a clip of what REVIEW picks around marker `number`, as Save clip
	/// does, saying how it went in REVIEW.
	pub fn clip_marker(&self, number: u32) -> anyhow::Result<String> {
		let marker = self
			.markers
			.get(number)
			.with_context(|| format!("there is no marker {number}"))?;
		let stretch = replay::Stretch::around(marker.at, Instant::now());
		let stretch = stretch.with_context(|| format!("cannot clip around {}", marker.title()))?;
		self.replay(false, false, Some(stretch))
	}

	/// Play REVIEW's pick in its replay source, or save it as a clip, over
	/// the stretch `around` a marker, if given. Copied now, without waiting
	/// on anything, so the span ends here and replays apply in the order they
	/// were asked for. How it went shows in REVIEW, and in the status bar too
	/// when a hotkey `pressed` it, since the panel may be hidden.
	fn replay(&self, play: bool, pressed: bool, around: Option<replay::Stretch>) -> anyhow::Result<String> {
		let replay = self.replay.lock().unwrap();
		let action = if play { Action::Replay } else { Action::SaveClip };
		let stretch = match (around, pressed) {
			(Some(around), _) => around,
			(None, true) => replay.stretch.last(),
			(None, false) => replay.stretch,
		};
		let taken = self.take(&replay, play, stretch);
		let message = taken
			.as_ref()
			.map(|(message, _)| message.clone())
			.map_err(|err| format!("{err:#}"));
		*replay.outcome.lock().unwrap() = Some(message.clone());
		if pressed {
			self.report(&action, message);
		}
		let (message, save) = taken?;
		// Started once the messages above are shown, so how it went replaces them.
		if let Some(save) = save {
			let (outcome, fired) = (replay.outcome.clone(), self.fired.clone());
			self.runtime.spawn(async move {
				let result = save.await.map_err(|err| format!("{err:#}"));
				*outcome.lock().unwrap() = Some(result.clone());
				if pressed {
					*fired.lock().unwrap() = Some(result.map_err(|err| format!("{action}: {err}")));
				}
			});
		}
		Ok(message)
	}

	/// What a replay or clip of `stretch` does at once, and the save a clip
	/// still runs. One pressed from outside REVIEW's own buttons, by a hotkey
	/// or the shell's clip button, takes the last stretch as long as the one
	/// picked.
	fn take(
		&self,
		replay: &Replay,
		play: bool,
		stretch: replay::Stretch,
	) -> anyhow::Result<(String, Option<impl Future<Output = anyhow::Result<String>> + use<>>)> {
		let into = match play {
			true => Some(self.replay_source(replay.into.as_deref())?),
			false => None,
		};
		let (ago, length) = stretch.span(kio::clock::Instant::now())?;
		let take = replay::take(&self.engine, &replay.of, ago, length, into.is_some())?;
		let of = replay.of.clone();
		if let Some(into) = into {
			replay::play(&self.engine, take, replay.speed, &into)?;
			return Ok((format!("replaying {of} in `{into}`"), None));
		}
		let saving = format!("saving {of}");
		let engine = self.engine.clone();
		let save = async move {
			let recorder = &engine.recorder;
			let path = replay::save(take, &recorder.dir(), recorder.show()).await?;
			Ok(format!("saved {of} to {}", path.display()))
		};
		Ok((saving, Some(save)))
	}

	/// The replay source a replay plays in, as [`replay_source`] picks it.
	fn replay_source(&self, picked: Option<&str>) -> anyhow::Result<String> {
		replay_source(self.engine.state.lock().unwrap().collection(), picked)
	}

	/// Keep how `action`, fired by a hotkey, went for the status bar.
	pub fn report(&self, action: &Action, result: Result<String, String>) {
		let action = action.name(self.studio());
		*self.fired.lock().unwrap() = Some(result.map_err(|err| format!("{action}: {err}")));
	}

	/// How the last action a hotkey fired went, once.
	pub fn fired(&self) -> Option<Result<String, String>> {
		self.fired.lock().unwrap().take()
	}
}

/// The replay source a replay plays in: `picked`, or the first there is
/// while REVIEW has picked none, or one since removed.
fn replay_source(collection: &moq_scene::Collection, picked: Option<&str>) -> anyhow::Result<String> {
	let replays = || {
		collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Replay))
			.map(|(name, _)| name)
	};
	let name = replays()
		.find(|name| Some(name.as_str()) == picked)
		.or_else(|| replays().next())
		.context("no replay source; add one in Sources")?;
	Ok(name.clone())
}

/// Mute the sound called `name`, or unmute it, under `state` locked by the
/// caller, as its strip's button does.
pub fn mute(state: &mut State, name: &str) -> anyhow::Result<String> {
	let collection = state.collection();
	anyhow::ensure!(collection.sources.contains_key(name), "there is no source `{name}`");
	let mut audio = collection.audio.get(name).cloned().unwrap_or_default();
	audio.muted = !audio.muted;
	let (verb, done) = match audio.muted {
		true => ("Mute", "muted"),
		false => ("Unmute", "unmuted"),
	};
	let mut draft = collection.draft(format!("{verb} `{name}`"));
	draft.set_audio(name, Some(audio))?;
	state.commit(draft, None)?;
	Ok(format!("{done} `{name}`"))
}

/// Take the scene called `name` on program, under `state` locked by the
/// caller, following any rename the window has yet to move its key through.
pub fn take(state: &mut State, name: &str) -> anyhow::Result<String> {
	let scene = state.take(name, kio::clock::Instant::now())?;
	Ok(format!("took `{scene}`"))
}

/// Cue the scene called `name`, under `state` locked by the caller, as
/// [`take`] takes it.
pub fn cue(state: &mut State, name: &str) -> anyhow::Result<String> {
	let scene = state.cue(name)?;
	Ok(match state.cued() {
		Some(_) => format!("cued `{scene}`; TAKE or AUTO puts it on air"),
		None => format!("`{scene}` is on air; nothing is cued"),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Each action without a name of its own is in the table once, under an
	/// id no other has, so the desktop never confuses two keys.
	#[test]
	fn every_action_is_registered_once() {
		// Every variant, so a new one fails to build here until it is placed.
		let registered = |action: &Action| match action {
			Action::Scene(_) | Action::Mute(_) => false,
			Action::Cut
			| Action::Transition
			| Action::StudioMode
			| Action::Replay
			| Action::SaveClip
			| Action::Marker
			| Action::Record
			| Action::Panic
			| Action::GoLive
			| Action::EndStream => true,
		};
		for (index, fixed) in FIXED.iter().enumerate() {
			assert!(registered(&fixed.action), "{:?}", fixed.action);
			for other in &FIXED[index + 1..] {
				assert_ne!(fixed.action, other.action);
				assert_ne!(fixed.id, other.id);
			}
			assert_eq!(fixed.action.to_string(), fixed.name);
		}
		let scene = Action::Scene("Gameplay".into());
		let mute = Action::Mute("Gameplay".into());
		assert_eq!(scene.id(), None, "named for its key");
		assert_ne!(mute.id(), Some("scene/Gameplay".into()));
		assert_eq!((scene.group(), mute.group()), (Group::Scenes, Group::Sounds));
	}

	/// Each group lists its actions in the table's order, the groups in
	/// theirs.
	#[test]
	fn groups_keep_the_tables_order() {
		let order = |group: Group| {
			FIXED
				.iter()
				.filter(move |fixed| fixed.group == group)
				.map(|fixed| &fixed.action)
		};
		let mut seen = Vec::new();
		for group in Group::ALL {
			seen.extend(order(group));
		}
		assert_eq!(seen.len(), FIXED.len(), "every action has a group the pane lists");
		let live: Vec<_> = order(Group::Live).cloned().collect();
		assert_eq!(live, [Action::GoLive, Action::EndStream, Action::Panic, Action::Record]);
	}
}
