//! What the operator does from a button or a hotkey: take a scene, panic,
//! replay, save a clip, and record. Hotkeys fire from outside the window's
//! frames, so a key pressed while a game has focus acts at once, even while
//! the window is hidden and draws nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use moq_scene::Source;

use crate::engine::{Engine, State};
use crate::replay;

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
	/// Play the stretch of the recent past REVIEW picks in its replay source.
	Replay,
	/// Save the stretch of the recent past REVIEW picks as a clip.
	SaveClip,
	/// Start recording, or stop the recording under way.
	Record,
	/// Cut to the safe scene and mute every microphone and guest.
	Panic,
}

impl Action {
	/// The actions every scene file has, as the Hotkeys panel lists them.
	pub const FIXED: [Self; 4] = [Self::Panic, Self::Replay, Self::SaveClip, Self::Record];

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
		}
	}
}

/// The engine as panels and hotkeys drive it, with the picks both share.
pub struct Controls {
	pub engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	/// REVIEW's replay picks, which a replay or clip from a hotkey takes too.
	pub replay: Mutex<Replay>,
	/// Whether the next recording gives each source a file of its own too.
	pub record_sources: AtomicBool,
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
	pub fn new(engine: Arc<Engine>, runtime: tokio::runtime::Handle) -> Self {
		Self {
			engine,
			runtime,
			replay: Mutex::default(),
			record_sources: AtomicBool::new(false),
			fired: Arc::default(),
		}
	}

	/// Do `action`, saying what it did.
	pub fn fire(&self, action: &Action) -> anyhow::Result<String> {
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
		}
	}

	/// Do `action` for a hotkey, keeping how it went for the status bar.
	pub fn press(&self, action: &Action) {
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
