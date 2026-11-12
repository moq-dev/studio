//! Each sound's and audio scene's level, taken once a step for every panel
//! that shows it, with the recent past for waveforms.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use egui::{Color32, Ui};
use moq_audio::Level;
use moq_scene::Collection;
use moq_scene::source::Sources;

use super::{MUTE_DB, decibels};

/// How often levels are taken. Taking one resets it, so the Audio and Graph
/// panels read these rather than each take their own and see half of it.
const STEP: Duration = Duration::from_millis(50);

/// How many steps a waveform shows: four seconds.
const KEPT: usize = 80;

/// The most steps missed at once that each hold the level taken after them:
/// a slow frame's worth.
const GAP: usize = 4;

/// The levels of the last [`KEPT`] steps, oldest first.
pub(super) type History = VecDeque<Level>;

/// Every playing sound's and audio scene's recent levels.
#[derive(Default)]
pub(super) struct Levels {
	/// When the last step was taken, or `None` before the first.
	taken: Option<kio::clock::Instant>,
	/// By source name.
	sounds: BTreeMap<String, History>,
	/// By audio scene name.
	mixes: BTreeMap<String, History>,
}

impl Levels {
	/// Take the steps due since the last call. A step missed by a slow frame
	/// holds the loudest level since the last one taken.
	pub fn take(&mut self, sources: &Sources, collection: &Collection) {
		let now = kio::clock::now();
		let Some(taken) = self.taken else {
			self.taken = Some(now);
			return;
		};
		let steps = now.saturating_duration_since(taken).as_nanos() / STEP.as_nanos();
		if steps == 0 {
			return;
		}
		// The rest of a step carries over, so steps keep their length
		// whatever the frame rate. After a long gap, such as while the window
		// was hidden, the waveforms start over rather than hold one level
		// across it.
		let steps = match usize::try_from(steps) {
			Ok(steps) if steps <= GAP => {
				self.taken = Some(taken + STEP * steps as u32);
				steps
			}
			_ => {
				self.taken = Some(now);
				self.sounds.clear();
				self.mixes.clear();
				1
			}
		};

		let mut sounds = BTreeMap::new();
		for (name, feed) in sources.iter() {
			let Some(strip) = feed.strip() else { continue };
			let mut history = self.sounds.remove(name).unwrap_or_default();
			push(&mut history, strip.level(), steps);
			sounds.insert(name.to_string(), history);
		}
		self.sounds = sounds;

		let mut mixes = BTreeMap::new();
		for scene in &collection.audio_scenes {
			// As Listen plays it and viewers hear it, published sources included.
			let Some(mix) = sources.listened(&scene.name) else {
				continue;
			};
			let mut history = self.mixes.remove(&scene.name).unwrap_or_default();
			push(&mut history, mix.level(), steps);
			mixes.insert(scene.name.clone(), history);
		}
		self.mixes = mixes;
	}

	/// The source called `name`'s sound, after its gain and mute, if it plays.
	pub fn sound(&self, name: &str) -> Option<&History> {
		self.sounds.get(name)
	}

	/// The audio scene called `name`'s mix, if it runs.
	pub fn mix(&self, name: &str) -> Option<&History> {
		self.mixes.get(name)
	}
}

fn push(history: &mut History, level: Level, steps: usize) {
	for _ in 0..steps {
		if history.len() == KEPT {
			history.pop_front();
		}
		history.push_back(level);
	}
}

/// Where `amplitude` falls on a meter, from 0 at [`MUTE_DB`] to 1 at full scale.
fn position(amplitude: f32) -> f32 {
	(1.0 - decibels(amplitude) / MUTE_DB).clamp(0.0, 1.0)
}

/// Green, or red once a level reaches full scale.
fn color(level: Level) -> Color32 {
	if level.peak() >= 1.0 {
		Color32::RED
	} else {
		Color32::from_rgb(80, 200, 120)
	}
}

/// A level meter of the latest step: RMS filled, peak as a tick, on a
/// decibel scale.
pub(super) fn meter(ui: &mut Ui, history: Option<&History>) {
	let level = history.and_then(|history| history.back().copied()).unwrap_or_default();
	let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 12.0), egui::Sense::hover());
	let painter = ui.painter_at(rect);
	painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
	let color = color(level);
	let mut filled = rect;
	filled.set_width(rect.width() * position(level.rms()));
	painter.rect_filled(filled, 2.0, color);
	let x = rect.left() + rect.width() * position(level.peak());
	painter.vline(x, rect.y_range(), egui::Stroke::new(2.0, color));
}

/// The last few seconds, newest on the right, mirrored about the middle on a
/// decibel scale: each step's peak faint, its RMS solid.
pub(super) fn waveform(ui: &mut Ui, history: Option<&History>) -> egui::Response {
	let size = egui::vec2(ui.available_width(), 28.0);
	let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
	let painter = ui.painter_at(rect);
	painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
	let Some(history) = history else {
		return response;
	};
	let width = rect.width() / KEPT as f32;
	let middle = rect.center().y;
	let half = rect.height() / 2.0;
	for (age, level) in history.iter().rev().enumerate() {
		let x = rect.right() - width * (age as f32 + 0.5);
		let color = color(*level);
		for (amplitude, color) in [(level.peak(), color.gamma_multiply(0.4)), (level.rms(), color)] {
			let reach = half * position(amplitude);
			if reach > 0.0 {
				let stroke = egui::Stroke::new(width.max(1.0), color);
				painter.vline(x, middle - reach..=middle + reach, stroke);
			}
		}
	}
	response
}
