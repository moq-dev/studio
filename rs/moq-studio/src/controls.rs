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
	/// Put the preview on program at once. Studio mode's TAKE
	/// (quest/m0/ux/studio-mode.md); until then the preview is always the
	/// scene on program, so it is not offered.
	Cut,
	/// Take the preview to program by the show's transition: Studio mode's
	/// AUTO, not offered until then either.
	Transition,
	/// Take a scene, by name, on program by the show's transition.
	Scene(String),
	/// Play the recent past the Replay panel picks in its replay source.
	Replay,
	/// Save the recent past the Replay panel picks as a clip.
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
	/// The actions every scene file has, as the Hotkeys panel lists them.
	pub const FIXED: [Self; 6] = [
		Self::Panic,
		Self::Replay,
		Self::SaveClip,
		Self::Record,
		Self::GoLive,
		Self::EndStream,
	];

	/// Whether a key can be bound to it: a fixed action, or a scene.
	pub fn offered(&self) -> bool {
		matches!(self, Self::Scene(_)) || Self::FIXED.contains(self)
	}
}

impl std::fmt::Display for Action {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Cut => write!(f, "Cut"),
			Self::Transition => write!(f, "Transition"),
			Self::Scene(name) => write!(f, "Take scene `{name}`"),
			Self::Replay => write!(f, "Replay"),
			Self::SaveClip => write!(f, "Save clip"),
			Self::Record => write!(f, "Start or stop recording"),
			Self::Panic => write!(f, "Panic"),
			Self::GoLive => write!(f, "Go live"),
			Self::EndStream => write!(f, "End stream"),
		}
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
	/// Whether relays saved on air wait for the stream to end to be
	/// followed, which would cut the broadcast sources and guests on program.
	rewatch: AtomicBool,
	/// The Replay panel's picks, which a replay or clip from a hotkey takes too.
	pub replay: Mutex<Replay>,
	/// Whether the next recording gives each source a file of its own too.
	pub record_sources: AtomicBool,
	/// How the last action a hotkey fired went, until the status bar takes it.
	fired: Arc<Mutex<Option<Result<String, String>>>>,
}

/// The Replay panel's choices, and how the last replay or clip went.
pub struct Replay {
	pub of: replay::Of,
	pub length: std::time::Duration,
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
			length: replay::LENGTHS[1],
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
			replay: Mutex::default(),
			record_sources: AtomicBool::new(false),
			fired: Arc::default(),
		}
	}

	/// Do `action`, saying what it did.
	pub fn fire(self: &Arc<Self>, action: &Action) -> anyhow::Result<String> {
		match action {
			Action::Cut => {
				let mut state = self.engine.state.lock().unwrap();
				state.cut()?;
				Ok(format!("cut to {}", on_program(state.compositor.program_scene())))
			}
			Action::Transition => {
				let mut state = self.engine.state.lock().unwrap();
				let transition = state.collection().transition.clone();
				state.transition(transition, kio::clock::Instant::now())?;
				Ok(format!("took {}", on_program(state.compositor.program_scene())))
			}
			Action::Scene(name) => take(&mut self.engine.state.lock().unwrap(), name),
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
			let switched = *controls.switched.lock().unwrap();
			// Cloned in a statement of its own, so the lock is not held
			// while the credential store blocks, which would freeze the
			// window reading the settings.
			let stream = controls.stream.lock().unwrap().clone();
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
			// Relays saved while this started, followed now if it failed;
			// once live, when it ends. Whichever of a save and this runs
			// last follows them.
			controls.follow_saved_relays();
			controls.report(&Action::GoLive, result);
		});
		if let Err(err) = spawned {
			self.going.store(false, Ordering::Release);
			return Err(err).context("cannot go live");
		}
		Ok("going live".into())
	}

	/// Whether a go-live is under way, from the press until it is live or
	/// refused.
	pub fn going(&self) -> bool {
		self.going.load(Ordering::Acquire)
	}

	/// The stream settings' relays were saved: follow them now when off air,
	/// or once the stream ends. Says whether it waits for the stream.
	pub fn relays_saved(&self) -> bool {
		self.rewatch.store(true, Ordering::Release);
		self.follow_saved_relays();
		self.rewatch.load(Ordering::Acquire)
	}

	/// Follow the saved relays, if any wait to be, unless a stream is on air
	/// or on its way.
	fn follow_saved_relays(&self) {
		if self.going() || self.engine.on_air() != OnAir::Off || !self.rewatch.swap(false, Ordering::AcqRel) {
			return;
		}
		let stream = self.stream.lock().unwrap().clone();
		crate::show::watch(&self.engine, &stream, &self.runtime);
	}

	/// End the stream in the background, handing the relays its tail first.
	pub fn end_stream(self: &Arc<Self>) -> anyhow::Result<String> {
		anyhow::ensure!(self.engine.on_air().live().is_some(), "the program is not on air");
		let controls = self.clone();
		self.runtime.spawn(async move {
			let result = controls.engine.end_stream().await;
			controls.follow_saved_relays();
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

	/// Play the Replay panel's pick in its replay source, or save it as a
	/// clip. Copied now, without waiting on anything, so the span ends here and
	/// replays apply in the order they were asked for. How it went shows in
	/// the Replay panel, and in the status bar too when a hotkey `pressed` it,
	/// since the panel may be hidden.
	fn replay(&self, play: bool, pressed: bool) -> anyhow::Result<String> {
		let replay = self.replay.lock().unwrap();
		let action = if play { Action::Replay } else { Action::SaveClip };
		let taken = self.take(&replay, play);
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
	fn take(
		&self,
		replay: &Replay,
		play: bool,
	) -> anyhow::Result<(String, Option<impl Future<Output = anyhow::Result<String>> + use<>>)> {
		let into = match play {
			true => Some(self.replay_source(replay.into.as_deref())?),
			false => None,
		};
		let take = replay::take(&self.engine, &replay.of, replay.length, into.is_some())?;
		let of = replay.of.clone();
		if let Some(into) = into {
			replay::play(&self.engine, take, replay.speed, &into)?;
			return Ok((format!("replaying {of} in `{into}`"), None));
		}
		let saving = format!("saving {of}");
		let engine = self.engine.clone();
		let save = async move {
			let path = replay::save(take, engine.autosave.path()).await?;
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

fn on_program(scene: Option<&str>) -> String {
	match scene {
		Some(scene) => format!("`{scene}`"),
		None => "nothing".into(),
	}
}
