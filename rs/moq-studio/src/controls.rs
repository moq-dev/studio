//! What the operator does from a panel's button or a hotkey: switch scenes,
//! replay, save a clip, and record. Hotkeys fire from outside the window's
//! frames, so a key pressed while a game has focus acts at once, even while
//! the window is hidden and draws nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use moq_scene::Source;

use crate::engine::Engine;
use crate::replay;

/// Something a hotkey can be bound to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Action {
	/// Put the preview on program at once.
	Cut,
	/// Take the preview to program by the transition the Program panel sets.
	Transition,
	/// Put a scene, by name, in preview.
	Scene(String),
	/// Play the recent past the Replay panel picks in its replay source.
	Replay,
	/// Save the recent past the Replay panel picks as a clip.
	SaveClip,
	/// Start recording, or stop the recording under way.
	Record,
}

impl Action {
	/// The actions every scene file has, as the Hotkeys panel lists them.
	pub const FIXED: [Self; 5] = [Self::Cut, Self::Transition, Self::Replay, Self::SaveClip, Self::Record];
}

impl std::fmt::Display for Action {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Cut => write!(f, "Cut"),
			Self::Transition => write!(f, "Transition"),
			Self::Scene(name) => write!(f, "Preview scene `{name}`"),
			Self::Replay => write!(f, "Replay"),
			Self::SaveClip => write!(f, "Save clip"),
			Self::Record => write!(f, "Start or stop recording"),
		}
	}
}

/// The engine as panels and hotkeys drive it, with the picks both share.
pub struct Controls {
	pub engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	/// The Replay panel's picks, which a replay or clip from a hotkey takes too.
	pub replay: Mutex<Replay>,
	/// Whether the next recording gives each source a file of its own too.
	pub record_sources: AtomicBool,
	/// How the last action a hotkey fired went, until the status bar takes it.
	fired: Mutex<Option<Result<String, String>>>,
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
	pub fn new(engine: Arc<Engine>, runtime: tokio::runtime::Handle) -> Self {
		Self {
			engine,
			runtime,
			replay: Mutex::default(),
			record_sources: AtomicBool::new(false),
			fired: Mutex::default(),
		}
	}

	/// Do `action`, saying what it did.
	pub fn fire(&self, action: &Action) -> anyhow::Result<String> {
		match action {
			Action::Cut => {
				let mut state = self.engine.state.lock().unwrap();
				let state = &mut *state;
				state.compositor.cut(&state.sources);
				Ok(format!("cut to {}", on_program(state.compositor.program_scene())))
			}
			Action::Transition => {
				let mut state = self.engine.state.lock().unwrap();
				let state = &mut *state;
				let transition = state.collection().transition.clone();
				let now = kio::clock::Instant::now();
				state.compositor.transition(transition, &state.sources, now);
				Ok(format!("took {}", on_program(state.compositor.program_scene())))
			}
			Action::Scene(name) => {
				let mut state = self.engine.state.lock().unwrap();
				anyhow::ensure!(state.collection().scene(name).is_some(), "no scene is called `{name}`");
				state.compositor.set_preview(Some(name.clone()));
				Ok(format!("previewing `{name}`"))
			}
			Action::Replay => self.replay(true),
			Action::SaveClip => self.replay(false),
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

	/// Play the Replay panel's pick in its replay source, or save it as a
	/// clip. Copied now, without waiting on anything, so the span ends here and
	/// replays apply in the order they were asked for.
	fn replay(&self, play: bool) -> anyhow::Result<String> {
		let replay = self.replay.lock().unwrap();
		let outcome = replay.outcome.clone();
		let result = self.take(&replay, play);
		let message = result.as_ref().map_err(|err| format!("{err:#}")).cloned();
		*outcome.lock().unwrap() = Some(message);
		result
	}

	fn take(&self, replay: &Replay, play: bool) -> anyhow::Result<String> {
		let into = match play {
			true => Some(self.replay_source(replay.into.as_deref())?),
			false => None,
		};
		let take = replay::take(&self.engine, &replay.of, replay.length, into.is_some())?;
		let of = replay.of.clone();
		if let Some(into) = into {
			replay::play(&self.engine, take, replay.speed, &into)?;
			return Ok(format!("replaying {of} in `{into}`"));
		}
		let saving = format!("saving {of}");
		let engine = self.engine.clone();
		let outcome = replay.outcome.clone();
		self.runtime.spawn(async move {
			let result = replay::save(take, engine.autosave.path())
				.await
				.map(|path| format!("saved {of} to {}", path.display()));
			*outcome.lock().unwrap() = Some(result.map_err(|err| format!("{err:#}")));
		});
		Ok(saving)
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
	pub fn report(&self, action: &Action, result: anyhow::Result<String>) {
		let result = result.map_err(|err| format!("{action}: {err:#}"));
		*self.fired.lock().unwrap() = Some(result);
	}

	/// How the last action a hotkey fired went, once.
	pub fn fired(&self) -> Option<Result<String, String>> {
		self.fired.lock().unwrap().take()
	}
}

fn on_program(scene: Option<&str>) -> String {
	match scene {
		Some(scene) => format!("`{scene}`"),
		None => "nothing".into(),
	}
}
