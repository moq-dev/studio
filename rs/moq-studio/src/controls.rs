//! What the operator does from a button or a hotkey: take a scene, panic,
//! replay, save a clip, record, and go live and end the stream. Hotkeys fire
//! from outside the window's frames, so a key pressed while a game has focus
//! acts at once, even while the window is hidden and draws nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use moq_scene::Source;

use crate::engine::{Engine, OnAir, State};
use crate::replay;
use crate::stream::{Stream, Switched};

/// Something a hotkey can be bound to.
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
	/// Start recording, or stop the recording under way.
	Record,
	/// Cut to the safe scene and mute every microphone and guest.
	Panic,
	/// Go live to the relays switched on, skipping the go-live sheet's checks.
	GoLive,
	/// End the stream, handing the relays its tail first.
	EndStream,
}

impl Action {
	/// The actions every scene file has, as the Hotkeys tab lists them.
	pub const FIXED: [Self; 9] = [
		Self::Panic,
		Self::Cut,
		Self::Transition,
		Self::StudioMode,
		Self::Replay,
		Self::SaveClip,
		Self::Record,
		Self::GoLive,
		Self::EndStream,
	];

	/// What it does with Studio mode on or off, as the Hotkeys tab and a
	/// hotkey's message name it.
	pub fn name(&self, studio: bool) -> String {
		match (self, studio) {
			(Self::Scene(name), true) => format!("Cue scene `{name}`"),
			(Self::Scene(name), false) => format!("Take scene `{name}`"),
			_ => self.to_string(),
		}
	}
}

impl std::fmt::Display for Action {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Cut => write!(f, "Take the cued scene"),
			Self::Transition => write!(f, "Auto: take the cued scene by the transition"),
			// The desktop's shortcut settings show it whichever mode is on.
			Self::Scene(name) => write!(f, "Take or cue scene `{name}`"),
			Self::StudioMode => write!(f, "Turn Studio mode on or off"),
			Self::Replay => write!(f, "Replay"),
			Self::SaveClip => write!(f, "Save clip"),
			Self::Record => write!(f, "Start or stop recording"),
			Self::Panic => write!(f, "Panic"),
			Self::GoLive => write!(f, "Go live"),
			Self::EndStream => write!(f, "End stream"),
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
	/// Whether the relays wait to be followed: the show's, until its first
	/// follow, and those saved on air until the stream ends, since following
	/// them then would cut the broadcast sources and guests on program.
	rewatch: AtomicBool,
	/// Held while the relays are followed, and by a go-live while it follows
	/// any still waiting, so each follow reads the settings after the last
	/// one landed, and none is followed once it is on air.
	following: Mutex<()>,
	/// The encoder checks running, which a go-live waits out, a closed
	/// go-live sheet's included, whose check runs to its end, and admits no
	/// more of until it is live or refused.
	probes: Arc<tokio::sync::watch::Sender<Probes>>,
	/// REVIEW's replay picks, which a replay or clip from a hotkey takes too.
	pub replay: Mutex<Replay>,
	/// Whether the next recording gives each source a file of its own too.
	pub record_sources: AtomicBool,
	/// Whether Studio mode is on: a scene's pad or key cues it rather than
	/// taking it.
	studio: AtomicBool,
	/// How the last action a hotkey fired went, until the status bar takes it.
	fired: Arc<Mutex<Option<Result<String, String>>>>,
}

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
		Self {
			engine,
			runtime,
			stream: Mutex::new(stream),
			switched: Mutex::default(),
			refused: Mutex::default(),
			going: AtomicBool::new(false),
			rewatch: AtomicBool::new(false),
			following: Mutex::new(()),
			probes: Arc::new(tokio::sync::watch::Sender::new(Probes::default())),
			replay: Mutex::default(),
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
			Action::Panic => self.engine.state.lock().unwrap().panic(self.engine.room.as_deref()),
			Action::Replay => self.replay(true, false),
			Action::SaveClip => self.replay(false, false),
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
				if waiting {
					controls.follow(&stream);
				}
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
			controls.going.store(false, Ordering::Release);
			controls.probes.send_modify(|probes| probes.starting = false);
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

	/// Take `stream` as the settings, as Settings saved them. When
	/// `relays_changed`, follow the new relays now if off air, or once the
	/// stream ends; says whether they wait for it.
	pub fn set_stream(self: &Arc<Self>, stream: Stream, relays_changed: bool) -> bool {
		{
			// Marked with the settings it belongs to, so a snapshot never
			// holds the new relays without knowing they wait to be followed.
			let mut current = self.stream.lock().unwrap();
			*current = stream;
			if relays_changed {
				self.rewatch.store(true, Ordering::Release);
			}
		}
		if !relays_changed {
			return false;
		}
		let waits = self.going() || self.engine.on_air() != OnAir::Off;
		drop(self.follow_saved_relays());
		waits
	}

	/// Follow `stream`'s relays, which wait to be followed again if any could
	/// not be set up, such as on a token the credential store would not read,
	/// so the next go-live, end of stream, or save tries them again. Call it
	/// holding [`Self::following`].
	fn follow(&self, stream: &Stream) {
		if !crate::show::follow(&self.engine, stream) {
			self.rewatch.store(true, Ordering::Release);
		}
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

	/// Follow the saved relays, if any wait to be, on a thread of its own,
	/// since the credential store blocks, unless a stream is on air or on its
	/// way, which follows them itself.
	fn follow_saved_relays(self: &Arc<Self>) -> Option<std::thread::JoinHandle<()>> {
		let controls = self.clone();
		let spawned = std::thread::Builder::new().name("follow".into()).spawn(move || {
			let _following = controls.following.lock().unwrap();
			if controls.going() || controls.engine.on_air() != OnAir::Off {
				return;
			}
			let (stream, waiting) = controls.snapshot();
			if !waiting {
				return;
			}
			// Each relay's client binds its socket on the runtime.
			let _runtime = controls.runtime.enter();
			controls.follow(&stream);
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
			Action::Replay => drop(self.replay(true, true)),
			Action::SaveClip => drop(self.replay(false, true)),
			action => self.report(action, self.fire(action).map_err(|err| format!("{err:#}"))),
		}
	}

	/// Play REVIEW's pick in its replay source, or save it as a
	/// clip. Copied now, without waiting on anything, so the span ends here and
	/// replays apply in the order they were asked for. How it went shows in
	/// REVIEW, and in the status bar too when a hotkey `pressed` it,
	/// since the panel may be hidden.
	fn replay(&self, play: bool, pressed: bool) -> anyhow::Result<String> {
		let replay = self.replay.lock().unwrap();
		let action = if play { Action::Replay } else { Action::SaveClip };
		let taken = self.take(&replay, play, pressed);
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

	/// What a replay or clip does at once, and the save a clip still runs.
	/// One `pressed` from outside REVIEW's own buttons, by a hotkey or the
	/// shell's clip button, takes the last stretch as long as the one picked.
	fn take(
		&self,
		replay: &Replay,
		play: bool,
		pressed: bool,
	) -> anyhow::Result<(String, Option<impl Future<Output = anyhow::Result<String>> + use<>>)> {
		let into = match play {
			true => Some(self.replay_source(replay.into.as_deref())?),
			false => None,
		};
		let stretch = match pressed {
			true => replay.stretch.last(),
			false => replay.stretch,
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
			let path = replay::save(take, recorder.dir(), recorder.show()).await?;
			Ok(format!("saved {of} to {}", path.display()))
		};
		Ok((saving, Some(save)))
	}

	/// The replay source a replay plays in: `picked`, or the first there is
	/// while the panel has picked none, or one since removed.
	fn replay_source(&self, picked: Option<&str>) -> anyhow::Result<String> {
		let state = self.engine.state.lock().unwrap();
		let sources = &state.collection().sources;
		let replays = || {
			sources
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
