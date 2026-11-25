//! The MIX page: a strip per sound, the audio scenes as master strips beside
//! them, and the same sends as a matrix.

use egui::{Align2, CursorIcon, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2, pos2, vec2};
use moq_scene::{AudioScene, Collection};

use super::levels::{History, latest};
use super::shell::rename_sheet;
use super::*;

/// The page's two views of the same sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
	/// A strip per sound and per audio scene, as on a mixing desk.
	#[default]
	Strips,
	/// Who hears what, as a matrix of sounds and audio scenes.
	Routes,
}

impl View {
	const ALL: [Self; 2] = [Self::Strips, Self::Routes];

	fn name(self) -> &'static str {
		match self {
			Self::Strips => "Strips",
			Self::Routes => "Routes",
		}
	}
}

/// What the MIX page keeps between frames.
#[derive(Default)]
pub(super) struct Desk {
	view: View,
	/// The sound whose strip is open, by name.
	opened: Option<String>,
}

impl Desk {
	/// The page in `view`, with the strip of the sound called `opened` open.
	#[cfg(test)]
	pub(super) fn new(view: View, opened: Option<&str>) -> Self {
		Self {
			view,
			opened: opened.map(str::to_string),
		}
	}
}

/// The decibels marked on a strip's scale, and where each sits on it, from
/// silence at the foot to the most boost at the head: finer near unity, where
/// a fader is ridden, as a desk's taper is.
const SCALE: [(f32, f32); 9] = [
	(MUTE_DB, 0.0),
	(-48.0, 0.1),
	(-36.0, 0.22),
	(-24.0, 0.38),
	(-12.0, 0.56),
	(-6.0, 0.68),
	(0.0, UNITY),
	(6.0, 0.9),
	(12.0, 1.0),
];

/// Where unity gain, and full scale on a meter, sit on a strip's scale.
pub(super) const UNITY: f32 = 0.8;

/// Where `db` sits on a strip's scale, from 0 at its foot to 1 at its head.
pub(super) fn taper(db: f32) -> f32 {
	let db = db.clamp(SCALE[0].0, SCALE[SCALE.len() - 1].0);
	let next = SCALE
		.iter()
		.position(|&(mark, _)| mark >= db)
		.unwrap_or(SCALE.len() - 1);
	if next == 0 {
		return 0.0;
	}
	let ((low, from), (high, to)) = (SCALE[next - 1], SCALE[next]);
	egui::lerp(from..=to, (db - low) / (high - low))
}

/// The decibels at `at` on a strip's scale, the inverse of [`taper`].
pub(super) fn untaper(at: f32) -> f32 {
	let at = at.clamp(0.0, 1.0);
	let next = SCALE
		.iter()
		.position(|&(_, mark)| mark >= at)
		.unwrap_or(SCALE.len() - 1);
	if next == 0 {
		return MUTE_DB;
	}
	let ((low, from), (high, to)) = (SCALE[next - 1], SCALE[next]);
	egui::lerp(low..=high, (at - from) / (to - from))
}

/// Where a level of `amplitude` lights a strip's meter, which runs from
/// silence at the scale's foot to full scale at unity.
pub(super) fn lit(amplitude: f32) -> f32 {
	(taper(decibels(amplitude)) / UNITY).clamp(0.0, 1.0)
}

/// A meter on the desk's scale, `level` and `peak` placed by [`lit`], hot
/// from the same level as on every other page.
fn desk_meter(level: f32, peak: f32) -> kit::Meter {
	kit::Meter::new(level, peak).hot(lit(gain(HOT_DB)))
}

/// How a sound or an audio scene reaches an audio scene, as its send dot
/// shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reach {
	/// Sent straight into it.
	Sent,
	/// Heard in it only through the audio scene named, which it holds.
	Through(String),
	/// Not heard in it.
	Not,
	/// The audio scene itself.
	Own,
	/// An audio scene the sender holds, which would then hold itself.
	Loop,
}

impl Reach {
	/// How the strip `from` reaches the audio scene `to` of `collection`.
	fn of(collection: &Collection, from: Strip<'_>, to: &AudioScene) -> Self {
		match from {
			Strip::Sound(name) if to.sounds.contains(name) => Self::Sent,
			Strip::Sound(name) => {
				let through = to.scenes.iter().find(|held| collection.heard(held).contains(name));
				through.map_or(Self::Not, |held| Self::Through(held.clone()))
			}
			Strip::Scene(name) if name == to.name => Self::Own,
			Strip::Scene(name) if collection.nests(name, &to.name) => Self::Loop,
			Strip::Scene(name) if to.scenes.contains(name) => Self::Sent,
			Strip::Scene(name) => {
				let through = to.scenes.iter().find(|held| collection.nests(held, name));
				through.map_or(Self::Not, |held| Self::Through(held.clone()))
			}
		}
	}

	/// Whether it is heard there, by any route.
	fn heard(&self) -> bool {
		matches!(self, Self::Sent | Self::Through(_))
	}

	/// What its dot says under the pointer, for the strip called `from`
	/// reaching the audio scene called `to`.
	fn says(&self, from: &str, to: &str) -> String {
		match self {
			Self::Sent => format!("{from} is sent to {to}; click to take it out"),
			Self::Through(held) => {
				format!("{from} reaches {to} through {held}, and plays there once; click to send it straight in too")
			}
			Self::Not => format!("{from} is left out of {to}; click to send it in"),
			Self::Own => format!("{to} itself"),
			Self::Loop => format!("{from} holds {to}, so {to} cannot hear it back"),
		}
	}
}

/// A sound, as its strip shows it.
struct Channel {
	name: String,
	/// What kind of source it is, such as a microphone.
	kind: &'static str,
	microphone: bool,
	/// Whether it is published independently, so viewers hear it from its
	/// own broadcast rather than through a mix the studio encodes.
	published: bool,
	audio: moq_scene::Audio,
	/// How it reaches each audio scene, in their order.
	reach: Vec<Reach>,
}

/// An audio scene, as its master strip shows it.
struct Master {
	name: String,
	/// The letter its send dots carry.
	letter: String,
	/// Whether the stream carries it.
	program: bool,
	/// Whether recordings hold it.
	recorded: bool,
	/// How it reaches each audio scene, its own included, in their order.
	reach: Vec<Reach>,
	/// What it holds, as the matrix's foot says it.
	holds: String,
}

/// What the page shows, copied under one short lock a frame.
struct Board {
	sounds: Vec<Channel>,
	masters: Vec<Master>,
	/// The audio scene Listen plays, by name, and why it stopped, if it did.
	listening: Option<(String, Option<String>)>,
}

impl Board {
	fn take(state: &State, listening: Option<(String, Option<String>)>) -> Self {
		let collection = state.collection();
		let scenes = &collection.audio_scenes;
		let reach = |from: Strip<'_>| scenes.iter().map(|to| Reach::of(collection, from, to)).collect();
		let sounds = state
			.sources
			.iter()
			.filter(|(_, feed)| feed.strip().is_some())
			.map(|(name, _)| {
				let source = collection.sources.get(name);
				Channel {
					name: name.to_string(),
					kind: kind(source),
					microphone: matches!(source, Some(Source::Microphone { .. })),
					published: collection.published.contains_key(name),
					audio: collection.audio.get(name).cloned().unwrap_or_default(),
					reach: reach(Strip::Sound(name)),
				}
			})
			.collect();
		let recorded = collection.recorded_audio();
		let masters = scenes
			.iter()
			.map(|scene| Master {
				name: scene.name.clone(),
				letter: letter(&scene.name),
				program: scene.program,
				recorded: recorded == Some(scene.name.as_str()),
				reach: reach(Strip::Scene(&scene.name)),
				holds: holds(scene),
			})
			.collect();
		Self {
			sounds,
			masters,
			listening,
		}
	}

	/// Whether Listen plays the audio scene called `name`.
	fn heard(&self, name: &str) -> bool {
		self.listening.as_ref().is_some_and(|(heard, _)| heard == name)
	}
}

/// What a source with sound is, as its opened strip says.
fn kind(source: Option<&Source>) -> &'static str {
	match source {
		Some(Source::Microphone { .. }) => "Microphone",
		Some(Source::SystemAudio) => "System audio",
		Some(Source::Media { .. }) => "Media file",
		Some(Source::Broadcast { .. }) => "Broadcast",
		Some(Source::Browser { .. }) => "Browser",
		Some(Source::Replay) => "Replay",
		Some(Source::Clone { .. }) => "Clone",
		_ => "Source",
	}
}

/// The letter an audio scene's send dots carry: its name's first.
fn letter(name: &str) -> String {
	name.chars()
		.next()
		.map(|first| first.to_uppercase().collect())
		.unwrap_or_default()
}

/// What `scene` holds, in a few words: "4 sounds", "1 sound and Stream".
fn holds(scene: &AudioScene) -> String {
	let sounds = match scene.sounds.len() {
		0 => None,
		1 => Some("1 sound".to_string()),
		count => Some(format!("{count} sounds")),
	};
	let parts: Vec<String> = sounds.into_iter().chain(scene.scenes.iter().cloned()).collect();
	match parts.as_slice() {
		[] => "Nothing".to_string(),
		[one] => one.clone(),
		[rest @ .., last] => format!("{} and {last}", rest.join(", ")),
	}
}

/// An edit the page asked for this frame, made once it is drawn.
pub(super) enum Change {
	/// One of the sound called `name`'s settings.
	Audio {
		name: String,
		set: Setting,
		label: String,
		gesture: Option<u64>,
	},
	/// Send a sound, or an audio scene if `scene`, into the audio scene `to`,
	/// or stop.
	Send {
		scene: bool,
		from: String,
		to: String,
		on: bool,
	},
	/// Put the audio scene called this on the stream.
	Program(String),
	/// Add an audio scene.
	Add,
	/// Open the sheet renaming the audio scene called this.
	Rename(String),
	/// Rename the audio scene called `from` to `to`.
	Renamed { from: String, to: String },
	/// Move the audio scene called this one place left (-1) or right (1).
	Move(String, isize),
	/// Remove the audio scene called this.
	Remove(String),
	/// Listen to the audio scene called this, or stop.
	Listen(Option<String>),
	/// Listen on this output, or the system default.
	Speaker(Option<String>),
}

/// One setting of a sound's audio, as one control changes it.
///
/// An edit carries only the setting its control changed, made on the sound's
/// audio as it is when applied: a whole copy from when the page was drawn
/// would undo an edit made in between, such as the panic key's mute from a
/// hotkey's thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Setting {
	Gain(f32),
	Muted(bool),
	Pan(f32),
	Delay(Duration),
}

impl Setting {
	/// Make it on `audio`.
	fn on(self, audio: &mut moq_scene::Audio) {
		match self {
			Self::Gain(gain) => audio.gain = gain,
			Self::Muted(muted) => audio.muted = muted,
			Self::Pan(pan) => audio.pan = pan,
			Self::Delay(delay) => audio.delay = delay,
		}
	}
}

/// Room around the page's contents.
const PAD: f32 = 20.0;
/// The bar over the strips or the matrix.
const BAR: f32 = 44.0;
/// A sound's strip, and as narrow as one gets beside an opened one.
const WIDE: f32 = 128.0;
const NARROW: f32 = 92.0;
/// The smallest and largest a send dot under a strip gets, and the room
/// between dots.
const DOT: [f32; 2] = [12.0, 22.0];
const DOT_GAP: f32 = 5.0;
/// The narrowest and widest an audio scene's strip gets.
const MASTER: [f32; 2] = [150.0, 240.0];
/// Room between strips, and between the sounds and the audio scenes.
const GAP: f32 = 8.0;
const GROUP: f32 = 18.0;
/// How wide the room an opened strip grows is.
const OPENED: f32 = 300.0;
/// The tile that adds an audio scene.
const ADD: f32 = 40.0;
/// A caption over a group of strips, with the room under it.
const CAPTION: f32 = 22.0;
/// The shortest a strip's fader gets before the page scrolls.
const FADER_MIN: f32 = 90.0;
/// What a strip has besides its fader: its head, value, buttons, sends, and
/// the room between them.
const STRIP_REST: f32 = 12.0 + 58.0 + 10.0 + 10.0 + 22.0 + 10.0 + 30.0 + 10.0 + 33.0 + 12.0;

impl Panels {
	/// The MIX page in the stage's room: the bar that turns between the
	/// strips and the matrix, and the one shown.
	pub(super) fn mix(&mut self, ui: &mut Ui, stage: Rect) {
		let engine = self.engine.clone();
		let board = {
			let state = engine.state.lock().unwrap();
			Board::take(&state, engine.listening())
		};
		// A strip opened on a sound since removed or renamed closes.
		if let Some(opened) = &self.desk.opened
			&& !board.sounds.iter().any(|sound| sound.name == *opened)
		{
			self.desk.opened = None;
		}
		let bar = Rect::from_min_size(stage.min, vec2(stage.width(), BAR));
		self.mix_bar(ui, bar);
		let body = stage.with_min_y(bar.bottom()).shrink2(vec2(PAD, 0.0));
		let body = body.with_max_y(body.bottom() - PAD);
		let mut changes = Vec::new();
		match self.desk.view {
			View::Strips => self.strips(ui, body, &board, &mut changes),
			View::Routes => self.routes(ui, body, &board, &mut changes),
		}
		if let Some((from, to)) = rename_sheet(ui.ctx(), "rename audio scene", &mut self.renaming_audio) {
			changes.push(Change::Renamed { from, to });
		}
		self.apply(changes);
	}

	/// The switch between the strips and the matrix, and what a send dot's
	/// look means.
	fn mix_bar(&mut self, ui: &mut Ui, rect: Rect) {
		let room = rect.shrink2(vec2(PAD, 0.0));
		let names = View::ALL.map(|view| {
			let galley = kit::caps_galley(ui.painter(), view.name(), kit::display(13.0), kit::TEXT, None);
			(view, galley)
		});
		let width: f32 = names.iter().map(|(_, galley)| galley.size().x + 28.0).sum::<f32>() + 6.0;
		let switch = Rect::from_min_size(pos2(room.left(), room.center().y - 15.0), vec2(width, 30.0));
		ui.painter()
			.rect(switch, 8, kit::P2, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
		let mut x = switch.left() + 3.0;
		for (view, galley) in names {
			let at = Rect::from_min_size(pos2(x, switch.top() + 3.0), vec2(galley.size().x + 28.0, 24.0));
			x = at.right();
			let response = ui.interact(at, ui.id().with(("mix view", view.name())), Sense::click());
			let on = self.desk.view == view;
			response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, view.name()));
			if on {
				ui.painter().rect_filled(at, 6, kit::P3);
			}
			let ink = if on || response.hovered() { kit::TEXT } else { kit::DIM };
			ui.painter().galley(at.center() - galley.size() / 2.0, galley, ink);
			if response.clicked() {
				self.desk.view = view;
			}
		}

		// The legend, from the right, left out where it would run into the
		// switch: each dot's tooltip says its route anyway.
		let font = kit::ui(11.5);
		let mut right = room.right();
		let keys = [
			(Reach::Not, "not heard"),
			(Reach::Through(String::new()), "through another audio scene"),
			(Reach::Sent, "sent"),
		]
		.map(|(reach, says)| (reach, ui.painter().layout_no_wrap(says.into(), font.clone(), kit::DIM)));
		let legend: f32 = keys.iter().map(|(_, galley)| galley.size().x + 7.0 + 14.0 + 16.0).sum();
		if right - legend < switch.right() {
			return;
		}
		for (reach, galley) in keys {
			right -= galley.size().x;
			ui.painter()
				.galley(pos2(right, rect.center().y - galley.size().y / 2.0), galley, kit::DIM);
			right -= 7.0 + 14.0;
			let at = Rect::from_min_size(pos2(right, rect.center().y - 7.0), Vec2::splat(14.0));
			paint_dot(ui.painter(), at, &reach, "", false);
			right -= 16.0;
		}
		if right - switch.right() > 160.0 {
			let galley = ui
				.painter()
				.layout_no_wrap("Where each sound is heard".into(), font, kit::DIM);
			let at = pos2(right - galley.size().x, rect.center().y - galley.size().y / 2.0);
			ui.painter().galley(at, galley, kit::DIM);
		}
	}

	/// A strip for each sound, then one for each audio scene, scrolling
	/// sideways when they do not fit. An opened strip grows its controls
	/// beside it and its neighbours narrow, so no meter is covered.
	fn strips(&mut self, ui: &mut Ui, body: Rect, board: &Board, changes: &mut Vec<Change>) {
		let sounds = board.sounds.len();
		let masters = board.masters.len();
		let opened = self.desk.opened.clone();
		let grow = if opened.is_some() { OPENED } else { 0.0 };
		let group = if sounds > 0 { GROUP } else { 0.0 };
		// Every strip holds a dot for each audio scene, however many there are.
		let floor = strip_floor(masters);
		let (narrow, wide) = (NARROW.max(floor), WIDE.max(floor));
		let (master_min, master_max) = (MASTER[0].max(floor), MASTER[1].max(floor));
		let masters_min = masters as f32 * (master_min + GAP) + ADD;
		let gaps = sounds.saturating_sub(1) as f32 * GAP;
		// The opened strip keeps its width; the others share what is left.
		let others = sounds.saturating_sub(usize::from(opened.is_some()));
		let room = body.width() - masters_min - group - gaps - grow - if opened.is_some() { wide } else { 0.0 };
		let width = match others {
			0 => wide,
			others => (room / others as f32).clamp(narrow, wide),
		};
		let sounds_width = others as f32 * width + if opened.is_some() { wide + grow } else { 0.0 } + gaps;
		let spare = body.width() - sounds_width - group - ADD - masters as f32 * GAP;
		let master = match masters {
			0 => 0.0,
			masters => (spare / masters as f32).clamp(master_min, master_max),
		};
		let content = vec2(
			(sounds_width + group + masters as f32 * (master + GAP) + ADD).max(body.width()),
			body.height().max(CAPTION + STRIP_REST + FADER_MIN),
		);

		ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
			egui::ScrollArea::both()
				.id_salt("mix strips")
				.auto_shrink(false)
				.show(ui, |ui| {
					let (area, _) = ui.allocate_exact_size(content, Sense::hover());
					let top = area.top() + CAPTION;
					let bottom = area.bottom();
					let mut x = area.left();
					if sounds > 0 {
						caption(ui, pos2(x, area.top()), "Sounds");
					}
					for sound in &board.sounds {
						let open = opened.as_deref() == Some(sound.name.as_str());
						let strip_width = if open { wide } else { width };
						let strip = Rect::from_min_max(pos2(x, top), pos2(x + strip_width, bottom));
						if open {
							let whole = strip.with_max_x(strip.right() + grow);
							ui.painter()
								.rect(whole, 12, kit::P1, Stroke::new(1.5, kit::ACCENT), StrokeKind::Inside);
							self.sound_strip(ui, strip, sound, board, false, changes);
							let panel = whole.with_min_x(strip.right());
							ui.painter().vline(
								panel.left() + 0.5,
								panel.y_range().shrink(1.0),
								Stroke::new(1.0, kit::LINE),
							);
							self.opened_strip(ui, panel, sound, board, changes);
							x = whole.right() + GAP;
						} else {
							self.sound_strip(ui, strip, sound, board, true, changes);
							x = strip.right() + GAP;
						}
					}
					x += group - if sounds > 0 { GAP } else { 0.0 };
					caption(ui, pos2(x, area.top()), "Audio scenes");
					for index in 0..masters {
						let strip = Rect::from_min_max(pos2(x, top), pos2(x + master, bottom));
						self.master_strip(ui, strip, index, board, changes);
						x = strip.right() + GAP;
					}
					let add = Rect::from_min_max(pos2(x, top), pos2(x + ADD, bottom));
					if add_tile(ui, add) {
						changes.push(Change::Add);
					}
				});
		});
	}

	/// A sound's strip in `rect`: its icon and name, which open it, its
	/// meter and fader, its gain, its mute, and its sends. Framed unless it
	/// is the opened one, whose frame takes in its controls too.
	fn sound_strip(
		&mut self,
		ui: &mut Ui,
		rect: Rect,
		sound: &Channel,
		board: &Board,
		framed: bool,
		changes: &mut Vec<Change>,
	) {
		if framed {
			ui.painter()
				.rect(rect, 12, kit::P1, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
		}
		let parts = Parts::of(rect);
		let muted = sound.audio.muted;
		let name = &sound.name;

		// The head: a click opens the strip, or closes it.
		let head = ui.interact(parts.head, ui.id().with(("open strip", name)), Sense::click());
		let open = self.desk.opened.as_deref() == Some(name.as_str());
		let says = if open {
			format!("Close {name}")
		} else {
			format!("Open {name}")
		};
		head.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &says));
		let head = kit::tip(head.on_hover_cursor(CursorIcon::PointingHand), &says, None);
		if head.clicked() {
			self.desk.opened = (!open).then(|| name.clone());
		}
		ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
			if muted {
				ui.multiply_opacity(0.45);
			}
			let painter = ui.painter();
			if head.hovered() {
				painter.rect_filled(parts.head.expand2(vec2(4.0, 4.0)), 8, kit::P2);
			}
			let tile = Rect::from_center_size(pos2(parts.head.center().x, parts.head.top() + 16.0), Vec2::splat(32.0));
			let (fill, icon) = match sound.microphone {
				true => (kit::ACCENT, Icon::Mic),
				false => (kit::P3, Icon::Sound),
			};
			painter.rect_filled(tile, 8, fill);
			icon.paint(ui, tile, 16.0, kit::TEXT);
			let galley = kit::caps_galley(painter, name, kit::display(15.0), kit::TEXT, Some(rect.width() - 16.0));
			let at = pos2(
				parts.head.center().x - galley.size().x / 2.0,
				parts.head.bottom() - galley.size().y,
			);
			painter.galley(at, galley, kit::TEXT);

			// The meter, the scale, and the fader, centered together.
			let (level, peak) = reading(self.levels.sound(name));
			let width = 10.0 + 6.0 + 22.0 + 2.0 + 28.0;
			let left = parts.fader.center().x - width / 2.0;
			meter(ui, parts.fader, left, 10.0, level, peak);
			scale(ui.painter(), parts.fader, left + 16.0, true);
			let db = decibels(sound.audio.gain);
			let mut at = taper(db);
			let fader = Rect::from_min_size(pos2(left + 40.0, parts.fader.top()), vec2(28.0, parts.fader.height()));
			let response = ui
				.put(fader, kit::Fader::new(&mut at).height(fader.height()).mark(UNITY))
				.on_hover_text("Gain: drag; double-click for unity");
			let response = kit::named(response, &format!("{name} fader"));
			let gesture = self.gestures.of(&response);
			let reset = double_clicked(ui, &response);
			if response.changed() || reset {
				let db = if reset { 0.0 } else { untaper(at) };
				changes.push(gain_change(sound, db, gesture));
			}

			// The gain, to read or type.
			let mut typed = db;
			let field = Rect::from_center_size(parts.value.center(), vec2((rect.width() - 20.0).min(96.0), 22.0));
			let response = ui
				.put(
					field,
					kit::Number::new(gain_field(&mut typed)).unit("dB").width(field.width()),
				)
				.on_hover_text("Gain in decibels: drag, or type a level such as 0");
			let response = kit::named(response, &format!("{name} gain"));
			let gesture = self.gestures.of(&response);
			if response.changed() {
				changes.push(gain_change(sound, typed, gesture));
			}
		});

		let (icon, verb) = match (muted, sound.microphone) {
			(true, true) => (Icon::MicMuted, "Unmute"),
			(true, false) => (Icon::Muted, "Unmute"),
			(false, true) => (Icon::Mic, "Mute"),
			(false, false) => (Icon::Sound, "Mute"),
		};
		let key = self.key(&Action::Mute(name.clone()), ui.ctx());
		let button = IconButton::new(icon, verb)
			.key(key.as_deref())
			.look(kit::Look::Boxed)
			.lit(muted, kit::CUE)
			.width(parts.buttons.width());
		if kit::named(ui.put(parts.buttons, button), &format!("{verb} {name}")).clicked() {
			changes.push(Change::Audio {
				name: name.clone(),
				set: Setting::Muted(!muted),
				label: format!("{verb} `{name}`"),
				gesture: None,
			});
		}
		sends(ui, parts.sends, Strip::Sound(name), &sound.reach, board, changes);
	}

	/// An audio scene's strip in `rect`: its name and what it is for, its
	/// meter, Listen, and its sends into the others.
	fn master_strip(&mut self, ui: &mut Ui, rect: Rect, index: usize, board: &Board, changes: &mut Vec<Change>) {
		let scene = &board.masters[index];
		let painter = ui.painter();
		let (fill, stroke) = match scene.program {
			true => (kit::P2, Stroke::new(1.5, kit::ACCENT)),
			false => (kit::P2, Stroke::new(1.0, kit::LINE)),
		};
		painter.rect(rect, 12, fill, stroke, StrokeKind::Inside);
		let parts = Parts::of(rect);
		let name = &scene.name;

		// The badge and name, then what it is for.
		let galley = kit::caps_galley(painter, name, kit::display(17.0), kit::TEXT, Some(rect.width() - 50.0));
		let line = galley.size().x + 26.0;
		let left = parts.head.center().x - line / 2.0;
		let badge = Rect::from_center_size(pos2(left + 9.0, parts.head.top() + 10.0), Vec2::splat(18.0));
		paint_dot(painter, badge, &Reach::Sent, &scene.letter, scene.program);
		painter.galley(
			pos2(left + 26.0, badge.center().y - galley.size().y / 2.0),
			galley,
			kit::TEXT,
		);
		let header = Rect::from_x_y_ranges(rect.x_range().shrink(8.0), rect.top()..=badge.bottom() + 4.0);
		scene_menu(ui, header, index, board, changes);
		let tags = Rect::from_min_size(
			pos2(rect.left() + 8.0, badge.bottom() + 8.0),
			vec2(rect.width() - 16.0, 18.0),
		);
		self.master_tags(ui, tags, scene, board, changes);

		// The meter on the same scale as the sounds', without a fader: an
		// audio scene has no gain of its own.
		let (level, peak) = reading(self.levels.mix(name));
		let width = 14.0 + 6.0 + 22.0;
		let left = parts.fader.center().x - width / 2.0;
		meter(ui, parts.fader, left, 14.0, level, peak);
		scale(ui.painter(), parts.fader, left + 20.0, false);

		let failed = board
			.listening
			.as_ref()
			.and_then(|(listened, failed)| (listened == name).then_some(failed.as_deref()).flatten());
		if let Some(failed) = failed {
			let response = ui.interact(parts.value, ui.id().with(("listen failed", name)), Sense::hover());
			tag(
				ui,
				parts.value.shrink2(vec2(4.0, 2.0)),
				"Listen stopped",
				kit::ERR,
				kit::TEXT,
			);
			kit::tip(response, &format!("Listening stopped: {failed}"), None);
		}
		let note = match mix::MONITOR {
			true => "Play this audio scene on the output picked under its name; use headphones",
			false => "Needs a build with the `capture` feature",
		};
		let heard = board.heard(name);
		let says = if heard {
			"Stop listening".to_string()
		} else {
			format!("Listen to {name}")
		};
		let button = IconButton::new(Icon::Listen, &says)
			.look(kit::Look::Boxed)
			.lit(heard, kit::ACCENT)
			.width(parts.buttons.width())
			.note(Some(note));
		let response = ui
			.add_enabled_ui(mix::MONITOR, |ui| ui.put(parts.buttons, button))
			.inner;
		if response.clicked() {
			changes.push(Change::Listen((!heard).then(|| name.clone())));
		}
		sends(ui, parts.sends, Strip::Scene(name), &scene.reach, board, changes);
	}

	/// The tag on the audio scene Listen plays, which names the output it
	/// plays on and picks another.
	fn listen_tag(&mut self, ui: &mut Ui, rect: Rect, changes: &mut Vec<Change>) {
		let output = self.speakers.named(&self.runtime, self.speaker.as_deref());
		let text = format!("On {output}");
		let response = tag_button(ui, rect, &text, "listen on");
		let response = kit::tip(response, "The output Listen plays on: pick another", None);
		if response.clicked() {
			self.speakers.refresh(&self.runtime);
		}
		egui::Popup::from_toggle_button_response(&response)
			.frame(kit::popover_frame())
			.align(egui::RectAlign::BOTTOM)
			.gap(6.0)
			.width(280.0)
			// A click on the list's scrollbar keeps it open; a pick closes it.
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.show(|ui| {
				let mut speaker = self.speaker.clone();
				if self.speakers.menu(ui, &mut speaker) {
					changes.push(Change::Speaker(speaker));
					ui.close();
				}
				if ui.add(kit::button(Icon::Reload, "Look for outputs again")).clicked() {
					self.speakers.refresh(&self.runtime);
				}
			});
	}

	/// The opened strip's controls, in `rect` beside it: its name and kind,
	/// its gain, pan, and delay, and where it is heard.
	fn opened_strip(&mut self, ui: &mut Ui, rect: Rect, sound: &Channel, board: &Board, changes: &mut Vec<Change>) {
		let name = &sound.name;
		let room = rect.shrink2(vec2(16.0, 14.0));
		ui.scope_builder(UiBuilder::new().max_rect(room), |ui| {
			ui.horizontal(|ui| {
				ui.label(kit::caps(name, kit::display(18.0)).color(kit::TEXT));
				ui.label(egui::RichText::new(sound.kind).font(kit::ui(12.0)).color(kit::DIM));
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					if ui.add(IconButton::new(Icon::Close, "Close").small()).clicked() {
						self.desk.opened = None;
					}
				});
			});
			ui.add_space(6.0);
			let audio = &sound.audio;
			egui::Grid::new(("opened strip", name))
				.num_columns(3)
				.spacing(vec2(10.0, 8.0))
				.show(ui, |ui| {
					// Gain, on the fader's taper.
					ui.label("Gain");
					let db = decibels(audio.gain);
					let mut at = taper(db);
					let slid = kit::named(ui.add(kit::Slider::new(&mut at).width(130.0)), &format!("{name} gain"));
					let gesture = self.gestures.of(&slid);
					let reset = double_clicked(ui, &slid);
					if slid.changed() || reset {
						let db = if reset { 0.0 } else { untaper(at) };
						changes.push(gain_change(sound, db, gesture));
					}
					let mut typed = db;
					let field = kit::named(
						ui.add(kit::Number::new(gain_field(&mut typed)).unit("dB").width(84.0)),
						&format!("{name} gain in dB"),
					);
					let gesture = self.gestures.of(&field);
					if field.changed() {
						changes.push(gain_change(sound, typed, gesture));
					}
					ui.end_row();

					// Pan, centered by a double click.
					ui.label("Pan");
					let mut at = (audio.pan + 1.0) / 2.0;
					let slid = ui
						.add(kit::Slider::new(&mut at).width(130.0))
						.on_hover_text("Place the sound between left and right; double-click to center");
					let slid = kit::named(slid, &format!("{name} pan"));
					let gesture = self.gestures.of(&slid);
					let reset = double_clicked(ui, &slid);
					let mut pan = audio.pan;
					if slid.changed() {
						pan = at * 2.0 - 1.0;
					}
					if reset {
						pan = 0.0;
					}
					let mut typed = audio.pan;
					let field = kit::named(
						ui.add(kit::Number::new(pan_field(&mut typed)).width(84.0)),
						&format!("{name} pan value"),
					);
					let field_gesture = self.gestures.of(&field);
					if field.changed() {
						pan = typed;
					}
					if pan != audio.pan {
						changes.push(Change::Audio {
							name: name.clone(),
							set: Setting::Pan(pan),
							label: format!("Set `{name}` pan"),
							gesture: gesture.or(field_gesture),
						});
					}
					ui.end_row();

					// Delay, up to the bus's most.
					ui.label("Delay");
					let max = moq_audio::bus::MAX_DELAY.as_millis() as u64;
					let mut at = audio.delay.as_millis() as f32 / max as f32;
					let slid = ui
						.add(kit::Slider::new(&mut at).width(130.0))
						.on_hover_text("Hold the sound back to line it up with a camera that lags it");
					let slid = kit::named(slid, &format!("{name} delay"));
					let mut millis = audio.delay.as_millis() as u64;
					let gesture = self.gestures.of(&slid);
					if slid.changed() {
						millis = (at * max as f32).round() as u64;
					}
					let mut typed = millis;
					let field = kit::named(
						ui.add(
							kit::Number::new(egui::DragValue::new(&mut typed).range(0..=max))
								.unit("ms")
								.width(84.0),
						),
						&format!("{name} delay in ms"),
					);
					let field_gesture = self.gestures.of(&field);
					if field.changed() {
						millis = typed;
					}
					if millis != audio.delay.as_millis() as u64 {
						changes.push(Change::Audio {
							name: name.clone(),
							set: Setting::Delay(Duration::from_millis(millis)),
							label: format!("Set `{name}` delay"),
							gesture: gesture.or(field_gesture),
						});
					}
					ui.end_row();
				});
			// Clap to measure (quest/m0/sync-clap.md) goes beside the delay,
			// and the filter chain (quest/m0/audio-filters.md) between it and
			// where the sound is heard.

			ui.add_space(10.0);
			ui.separator();
			ui.label(kit::caps("Heard in", kit::display(11.0)).color(kit::DIM));
			if sound.published {
				ui.label(
					egui::RichText::new(
						"Published independently: viewers hear it from its own broadcast, so the stream, \
						 recordings, and published scenes leave it out; Listen and guests hear it where it is sent",
					)
					.color(kit::DIM),
				);
			}
			for (master, reach) in board.masters.iter().zip(&sound.reach) {
				ui.horizontal(|ui| {
					let (at, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
					let shown = if reach.heard() { &Reach::Sent } else { &Reach::Not };
					paint_dot(ui.painter(), at, shown, &master.letter, master.program);
					ui.label(&master.name);
					if let Reach::Through(held) = reach {
						ui.label(egui::RichText::new(format!("through {held}")).color(kit::DIM));
					}
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						let mut sent = *reach == Reach::Sent;
						let response = ui.add(kit::Toggle::new(&mut sent));
						let response = kit::tip(response, &reach.says(name, &master.name), None);
						if response.changed() {
							changes.push(Change::Send {
								scene: false,
								from: name.clone(),
								to: master.name.clone(),
								on: sent,
							});
						}
					});
				});
			}
		});
	}

	/// Who hears what: a row for each sound and each audio scene, a column
	/// for each audio scene, and a dot where they meet.
	fn routes(&mut self, ui: &mut Ui, body: Rect, board: &Board, changes: &mut Vec<Change>) {
		const ROW: f32 = 50.0;
		const HEAD: f32 = 98.0;
		const SECTION: f32 = 34.0;
		const FOOT: f32 = 36.0;
		const COLUMN: f32 = 178.0;
		let columns = board.masters.len() as f32;
		let label = (body.width() - columns * COLUMN - ADD - 24.0).clamp(300.0, 520.0);
		let width = label + columns * COLUMN + ADD + 24.0;
		let rows = board.sounds.len() + board.masters.len();
		let height = HEAD + rows as f32 * ROW + SECTION + FOOT;
		let size = vec2(width, height);

		ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
			egui::ScrollArea::both()
				.id_salt("mix routes")
				.auto_shrink(false)
				.show(ui, |ui| {
					let left = (ui.available_width() - width).max(0.0) / 2.0;
					let (area, _) = ui.allocate_exact_size(vec2(width + left, height), Sense::hover());
					let table = Rect::from_min_size(area.min + vec2(left, 0.0), size);
					let painter = ui.painter();
					painter.rect(table, 12, kit::P1, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
					let line = Stroke::new(1.0, kit::LINE);
					let column = |index: usize| {
						let x = table.left() + label + index as f32 * COLUMN;
						egui::Rangef::new(x, x + COLUMN)
					};

					// The title, and a head for each audio scene.
					let title = table.left_top() + vec2(18.0, 24.0);
					painter.text(title, Align2::LEFT_TOP, "WHO HEARS WHAT", kit::display(22.0), kit::TEXT);
					painter.text(
						title + vec2(0.0, 32.0),
						Align2::LEFT_TOP,
						"A row is sent into a column. Click a dot to change it.",
						kit::ui(11.5),
						kit::DIM,
					);
					for (index, master) in board.masters.iter().enumerate() {
						let x = column(index);
						let head = Rect::from_x_y_ranges(x, table.top()..=table.top() + HEAD);
						if master.program {
							let body = Rect::from_x_y_ranges(x, table.top() + 1.0..=table.bottom() - FOOT);
							ui.painter().rect_filled(body, 0, kit::ACCENT.gamma_multiply(0.06));
						}
						ui.painter().vline(x.min, table.y_range().shrink(1.0), line);
						self.route_head(ui, head, index, board, changes);
					}
					let add = Rect::from_center_size(
						pos2(column(board.masters.len()).min + 12.0 + ADD / 2.0, table.top() + 48.0),
						Vec2::splat(28.0),
					);
					let response = ui.put(
						add,
						IconButton::new(Icon::Add, "Add an audio scene").look(kit::Look::Boxed),
					);
					if response.clicked() {
						changes.push(Change::Add);
					}

					// A row for each sound, then each audio scene.
					let mut y = table.top() + HEAD;
					let painter = ui.painter().clone();
					for sound in &board.sounds {
						let row = Rect::from_x_y_ranges(table.x_range(), y..=y + ROW);
						painter.hline(row.x_range(), row.top() + 0.5, line);
						let name = row.with_max_x(table.left() + label);
						self.route_label(
							ui,
							name,
							&sound.name,
							RowIcon::Sound(sound),
							self.levels.sound(&sound.name),
						);
						for (index, (master, reach)) in board.masters.iter().zip(&sound.reach).enumerate() {
							let cell = Rect::from_x_y_ranges(column(index), row.y_range());
							let send = Dot {
								from: Strip::Sound(&sound.name),
								to: master,
								reach,
								letter: false,
							};
							route_cell(ui, cell, send, changes);
						}
						y += ROW;
					}
					let section = Rect::from_x_y_ranges(table.x_range(), y..=y + SECTION);
					painter.hline(section.x_range(), section.top() + 0.5, line);
					painter.rect_filled(section.shrink(1.0), 0, kit::P1);
					let at = pos2(section.left() + 18.0, section.center().y);
					let caps = kit::caps_galley(&painter, "Audio scenes", kit::display(11.0), kit::DIM, None);
					let caps_width = caps.size().x;
					painter.galley(pos2(at.x, at.y - caps.size().y / 2.0), caps, kit::DIM);
					painter.text(
						at + vec2(caps_width + 12.0, 0.0),
						Align2::LEFT_CENTER,
						"An audio scene can be sent into another. A sound still plays once, however it gets there.",
						kit::ui(11.5),
						kit::DIM,
					);
					y += SECTION;
					for from in &board.masters {
						let row = Rect::from_x_y_ranges(table.x_range(), y..=y + ROW);
						painter.hline(row.x_range(), row.top() + 0.5, line);
						let name = row.with_max_x(table.left() + label);
						self.route_label(ui, name, &from.name, RowIcon::Scene(from), self.levels.mix(&from.name));
						for (index, (master, reach)) in board.masters.iter().zip(&from.reach).enumerate() {
							let cell = Rect::from_x_y_ranges(column(index), row.y_range());
							let send = Dot {
								from: Strip::Scene(&from.name),
								to: master,
								reach,
								letter: false,
							};
							route_cell(ui, cell, send, changes);
						}
						y += ROW;
					}

					// What each audio scene holds.
					let foot = Rect::from_x_y_ranges(table.x_range(), y..=table.bottom());
					painter.hline(foot.x_range(), foot.top() + 0.5, line);
					painter.text(
						pos2(foot.left() + 18.0, foot.center().y),
						Align2::LEFT_CENTER,
						"Holds",
						kit::ui(11.5),
						kit::DIM,
					);
					for (index, master) in board.masters.iter().enumerate() {
						let x = column(index);
						painter.text(
							pos2(x.center(), foot.center().y),
							Align2::CENTER_CENTER,
							&master.holds,
							kit::ui(11.5),
							kit::DIM,
						);
					}
				});
		});
	}

	/// What an audio scene is for, as tags from `first` down: whether the
	/// stream hears it, or a button putting it on, then Listen's output where
	/// it is listened to, or that recordings hold it.
	fn master_tags(&mut self, ui: &mut Ui, first: Rect, scene: &Master, board: &Board, changes: &mut Vec<Change>) {
		let name = &scene.name;
		if scene.program {
			tag(ui, first, "Stream hears this", kit::ACCENT, kit::TEXT);
		} else {
			let response = tag_button(ui, first, "Put on stream", ("put on stream", name));
			let response = kit::tip(
				response,
				&format!("Put {name} on the stream, without dropping or repeating audio"),
				None,
			);
			if response.clicked() {
				changes.push(Change::Program(name.clone()));
			}
		}
		let second = first.translate(vec2(0.0, 22.0));
		if board.heard(name) {
			self.listen_tag(ui, second, changes);
		} else if scene.recorded {
			tag(ui, second, "In the recording", kit::P3, kit::DIM);
		}
	}

	/// An audio scene's column head: its badge and name, what it is for,
	/// and its meter.
	fn route_head(&mut self, ui: &mut Ui, head: Rect, index: usize, board: &Board, changes: &mut Vec<Change>) {
		let master = &board.masters[index];
		let painter = ui.painter();
		let galley = kit::caps_galley(
			painter,
			&master.name,
			kit::display(16.0),
			kit::TEXT,
			Some(head.width() - 50.0),
		);
		let line = galley.size().x + 26.0;
		let left = head.center().x - line / 2.0;
		let badge = Rect::from_center_size(pos2(left + 9.0, head.top() + 22.0), Vec2::splat(18.0));
		paint_dot(painter, badge, &Reach::Sent, &master.letter, master.program);
		painter.galley(
			pos2(left + 26.0, badge.center().y - galley.size().y / 2.0),
			galley,
			kit::TEXT,
		);
		let header = Rect::from_x_y_ranges(head.x_range().shrink(12.0), head.top() + 8.0..=badge.bottom() + 4.0);
		scene_menu(ui, header, index, board, changes);
		let tags = Rect::from_center_size(
			pos2(head.center().x, head.top() + 46.0),
			vec2(head.width() - 24.0, 18.0),
		);
		self.master_tags(ui, tags, master, board, changes);
		let (level, peak) = reading(self.levels.mix(&master.name));
		let at = Rect::from_center_size(pos2(head.center().x, head.top() + 88.0), vec2(head.width() - 24.0, 8.0));
		ui.put(at, desk_meter(level, peak).run(kit::Run::Across, at.width(), 8.0));
	}

	/// A row's label: its icon, name, and meter.
	fn route_label(&self, ui: &mut Ui, rect: Rect, name: &str, icon: RowIcon<'_>, history: Option<&History>) {
		let painter = ui.painter();
		let tile = Rect::from_center_size(pos2(rect.left() + 30.0, rect.center().y), Vec2::splat(24.0));
		let muted = match icon {
			RowIcon::Sound(sound) => {
				let (fill, glyph) = match sound.microphone {
					true => (kit::ACCENT, Icon::Mic),
					false => (kit::P3, Icon::Sound),
				};
				painter.rect_filled(tile, 6, fill);
				glyph.paint(ui, tile, 13.0, kit::TEXT);
				sound.audio.muted
			}
			RowIcon::Scene(master) => {
				paint_dot(painter, tile.shrink(2.0), &Reach::Sent, &master.letter, master.program);
				false
			}
		};
		// The name, cut short where it would run into the Muted tag or the
		// meter, and then shown whole under the pointer.
		let along = (rect.width() * 0.37).min(190.0);
		let meter_left = rect.right() - along - 24.0;
		let left = tile.right() + 14.0;
		let room = meter_left - 16.0 - left - if muted { 10.0 + 54.0 } else { 0.0 };
		let ink = if muted { kit::DIM } else { kit::TEXT };
		let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), kit::ui_medium(14.0), ink);
		job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
		let text = painter.layout_job(job);
		let shown = Rect::from_min_size(pos2(left, rect.center().y - text.size().y / 2.0), text.size());
		let elided = text.elided;
		painter.galley(shown.min, text, ink);
		if elided {
			let scene = matches!(icon, RowIcon::Scene(_));
			let response = ui.interact(shown, ui.id().with(("route label", name, scene)), Sense::hover());
			kit::tip(response, name, None);
		}
		if muted {
			let at = Rect::from_min_size(pos2(shown.right() + 10.0, rect.center().y - 9.0), vec2(54.0, 18.0));
			tag(ui, at, "Muted", kit::CUE, kit::ON_CUE);
		}
		let (level, peak) = match muted {
			true => (0.0, 0.0),
			false => reading(history),
		};
		let at = Rect::from_min_size(pos2(meter_left, rect.center().y - 4.0), vec2(along, 8.0));
		ui.put(at, desk_meter(level, peak).run(kit::Run::Across, along, 8.0));
	}

	/// Make the edits the page asked for, under one lock.
	pub(super) fn apply(&mut self, changes: Vec<Change>) {
		if changes.is_empty() {
			return;
		}
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let mut result = Ok(());
		for change in changes {
			let done = match change {
				Change::Audio {
					name,
					set,
					label,
					gesture,
				} => {
					let mut audio = state.collection().audio.get(&name).cloned().unwrap_or_default();
					set.on(&mut audio);
					edit(&mut state, label, gesture, |draft| draft.set_audio(&name, Some(audio)))
				}
				Change::Send { scene, from, to, on } => {
					let strip = if scene {
						Strip::Scene(&from)
					} else {
						Strip::Sound(&from)
					};
					let mut scenes = state.collection().audio_scenes.clone();
					match strip.send(&mut scenes, &to, on) {
						true => edit(&mut state, strip.label(&to, on), None, |draft| {
							draft.set_audio_scenes(scenes)
						}),
						false => Ok(()),
					}
				}
				Change::Program(name) => {
					let (scenes, label) = on_program(&state.collection().audio_scenes, &name);
					edit(&mut state, label, None, |draft| draft.set_audio_scenes(scenes))
				}
				Change::Add => {
					let collection = state.collection();
					let name = (collection.audio_scenes.len() + 1..)
						.map(|number| format!("Audio scene {number}"))
						.find(|name| collection.audio_scene(name).is_none())
						.expect("a free name");
					let mut scenes = collection.audio_scenes.clone();
					scenes.push(AudioScene::new(name.clone()));
					let done = edit(&mut state, format!("Add audio scene `{name}`"), None, |draft| {
						draft.set_audio_scenes(scenes)
					});
					done.map(|()| self.say(Ok(format!("added audio scene `{name}`; right-click it to rename it"))))
				}
				Change::Rename(name) => {
					self.renaming_audio = Some((name.clone(), name));
					Ok(())
				}
				Change::Renamed { from, to } => self.rename_audio(&mut state, &from, &to),
				Change::Move(name, by) => {
					let mut scenes = state.collection().audio_scenes.clone();
					match scenes.iter().position(|scene| scene.name == name) {
						Some(from) => {
							let to = from.saturating_add_signed(by).min(scenes.len() - 1);
							let scene = scenes.remove(from);
							scenes.insert(to, scene);
							edit(&mut state, format!("Reorder audio scene `{name}`"), None, |draft| {
								draft.set_audio_scenes(scenes)
							})
						}
						None => Err(anyhow::anyhow!("no audio scene is called `{name}`")),
					}
				}
				Change::Remove(name) => edit(&mut state, format!("Remove audio scene `{name}`"), None, |draft| {
					draft.remove_audio_scene(&name)
				}),
				Change::Listen(scene) => self.listen(&mut state, scene.as_deref()),
				Change::Speaker(speaker) => {
					self.speaker = speaker;
					// The monitor moves to the new output, still playing what it did.
					match (self.engine.listening(), self.engine.soloing()) {
						(Some((heard, _)), _) => self.listen(&mut state, Some(&heard)),
						(None, Some((solo, _))) => {
							self.engine
								.monitor_source(&mut state, &self.runtime, Some(&solo), self.speaker.as_deref())
						}
						(None, None) => Ok(()),
					}
				}
			};
			result = result.and(done);
		}
		drop(state);
		self.check(result);
	}

	/// Rename the audio scene called `from` to `to`; Listen follows it.
	fn rename_audio(&self, state: &mut State, from: &str, to: &str) -> anyhow::Result<()> {
		if from == to {
			return Ok(());
		}
		edit(state, format!("Rename audio scene `{from}` to `{to}`"), None, |draft| {
			draft.rename_audio_scene(from, to)
		})?;
		match self.engine.listening() {
			Some((heard, _)) if heard == from => self.listen(state, Some(to)),
			_ => Ok(()),
		}
	}

	/// Play the audio scene called `scene` to the speakers picked, or stop
	/// with `None`.
	pub(super) fn listen(&self, state: &mut State, scene: Option<&str>) -> anyhow::Result<()> {
		let listen = scene.map(|scene| mix::Listen {
			scene,
			speaker: self.speaker.as_deref(),
		});
		self.engine.listen(state, &self.runtime, listen)
	}
}

/// What a matrix row stands for.
#[derive(Clone, Copy)]
enum RowIcon<'a> {
	Sound(&'a Channel),
	Scene(&'a Master),
}

/// The narrowest a strip gets that still holds a send dot, at its smallest,
/// for each of `masters` audio scenes.
fn strip_floor(masters: usize) -> f32 {
	let count = masters as f32;
	count * DOT[0] + (count - 1.0).max(0.0) * DOT_GAP + 2.0 * Parts::INSET.x
}

/// Where a strip's parts sit in it, top to bottom.
struct Parts {
	head: Rect,
	/// The meter, scale, and fader.
	fader: Rect,
	value: Rect,
	buttons: Rect,
	sends: Rect,
}

impl Parts {
	/// Room between a strip's edges and its parts.
	const INSET: Vec2 = vec2(10.0, 12.0);

	fn of(rect: Rect) -> Self {
		let inner = rect.shrink2(Self::INSET);
		let head = Rect::from_min_size(inner.min, vec2(inner.width(), 58.0));
		let sends = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 33.0), inner.max);
		let buttons = Rect::from_min_max(
			pos2(inner.left(), sends.top() - 10.0 - 30.0),
			pos2(inner.right(), sends.top() - 10.0),
		);
		let value = Rect::from_min_max(
			pos2(inner.left(), buttons.top() - 10.0 - 22.0),
			pos2(inner.right(), buttons.top() - 10.0),
		);
		let fader = Rect::from_min_max(
			pos2(inner.left(), head.bottom() + 10.0),
			pos2(inner.right(), value.top() - 10.0),
		);
		Self {
			head,
			fader,
			value,
			buttons,
			sends,
		}
	}
}

/// The latest step's RMS and peak where they light a strip's meter.
pub(super) fn reading(history: Option<&History>) -> (f32, f32) {
	let level = latest(history);
	(lit(level.rms()), lit(level.peak()))
}

/// A strip's meter, `width` wide at `left` in its fader's room, from the
/// scale's foot up to unity, which is full scale.
fn meter(ui: &mut Ui, room: Rect, left: f32, width: f32, level: f32, peak: f32) {
	let top = fader_y(room, UNITY);
	let bottom = fader_y(room, 0.0);
	let at = Rect::from_min_max(pos2(left, top), pos2(left + width, bottom));
	ui.put(at, desk_meter(level, peak).run(kit::Run::Up, at.height(), width));
}

/// Where `at` on the scale sits in a fader's room, as [`kit::Fader`] places
/// its cap.
fn fader_y(room: Rect, at: f32) -> f32 {
	egui::lerp(room.bottom() - kit::Fader::INSET..=room.top() + kit::Fader::INSET, at)
}

/// The scale's marks, 22 points wide at `left` in a fader's room, the boost
/// above unity too if `boost`. Below unity the marks drop their sign, as a
/// desk prints them.
fn scale(painter: &egui::Painter, room: Rect, left: f32, boost: bool) {
	for (db, at) in SCALE {
		if db > 0.0 && !boost {
			continue;
		}
		let text = match db {
			0.0 => "0".to_string(),
			db if db > 0.0 => format!("+{db:.0}"),
			db => format!("{:.0}", -db),
		};
		let ink = if db == 0.0 { kit::TEXT } else { kit::FAINT };
		painter.text(
			pos2(left + 11.0, fader_y(room, at)),
			Align2::CENTER_CENTER,
			text,
			kit::mono(9.0),
			ink,
		);
	}
}

/// The gain edit setting `sound` to `db` decibels.
fn gain_change(sound: &Channel, db: f32, gesture: Option<u64>) -> Change {
	let name = &sound.name;
	Change::Audio {
		name: name.clone(),
		set: Setting::Gain(gain(db)),
		label: format!("Set `{name}` gain"),
		gesture,
	}
}

/// A field for a pan, `C` in the middle, `L50` halfway left, that drags or
/// takes a place typed.
fn pan_field(pan: &mut f32) -> egui::DragValue<'_> {
	egui::DragValue::new(pan)
		.range(-1.0..=1.0)
		.speed(0.01)
		.custom_formatter(|pan, _| pan_words(pan as f32))
		.custom_parser(|text| {
			let text = text.trim().to_ascii_uppercase();
			let percent = |side: &str| side.trim().parse::<f64>().ok().map(|n| n / 100.0);
			match text.as_str() {
				"C" | "CENTER" => Some(0.0),
				_ if let Some(side) = text.strip_prefix('L') => percent(side).map(|n| -n),
				_ if let Some(side) = text.strip_prefix('R') => percent(side),
				_ => text.parse().ok(),
			}
		})
}

/// A pan as a person reads it: `Center`, `L50` halfway left, `R100` hard right.
pub(super) fn pan_words(pan: f32) -> String {
	match pan {
		0.0 => "Center".to_string(),
		pan if pan < 0.0 => format!("L{:.0}", -pan * 100.0),
		pan => format!("R{:.0}", pan * 100.0),
	}
}

/// Whether the pointer double-clicked on `response`, which senses drags
/// alone, so a press starts a drag at once.
pub(super) fn double_clicked(ui: &Ui, response: &egui::Response) -> bool {
	response.contains_pointer() && ui.input(|input| input.pointer.button_double_clicked(egui::PointerButton::Primary))
}

/// A caption over a group of strips at `at`.
fn caption(ui: &Ui, at: egui::Pos2, text: &str) {
	let galley = kit::caps_galley(ui.painter(), text, kit::display(11.0), kit::DIM, None);
	ui.painter().galley(at, galley, kit::DIM);
}

/// A tag reading `text` in `rect`, on `fill`.
fn tag(ui: &Ui, rect: Rect, text: &str, fill: egui::Color32, ink: egui::Color32) {
	let galley = kit::caps_galley(ui.painter(), text, kit::display(11.0), ink, Some(rect.width() - 14.0));
	let at = Rect::from_center_size(rect.center(), vec2(galley.size().x + 14.0, rect.height()));
	ui.painter().rect_filled(at, 4, fill);
	ui.painter().galley(at.center() - galley.size() / 2.0, galley, ink);
}

/// A tag reading `text` in `rect` that does something when clicked, known by
/// `id` wherever the strips move it.
fn tag_button(ui: &mut Ui, rect: Rect, text: &str, id: impl egui::AsIdSalt) -> egui::Response {
	let galley = kit::caps_galley(
		ui.painter(),
		text,
		kit::display(11.0),
		kit::DIM,
		Some(rect.width() - 14.0),
	);
	let at = Rect::from_center_size(rect.center(), vec2(galley.size().x + 14.0, rect.height()));
	let response = ui.interact(at, ui.id().with(id), Sense::click());
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
	let (fill, ink) = match response.hovered() {
		true => (kit::P3, kit::TEXT),
		false => (kit::P2, kit::DIM),
	};
	ui.painter()
		.rect(at, 4, fill, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
	ui.painter().galley(at.center() - galley.size() / 2.0, galley, ink);
	response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The menu a right click on the audio scene at `index` opens over `rect`,
/// its name: rename it, move it, or remove it, as a scene's pad's does.
fn scene_menu(ui: &mut Ui, rect: Rect, index: usize, board: &Board, changes: &mut Vec<Change>) {
	let scene = &board.masters[index];
	let name = &scene.name;
	let response = ui.interact(rect, ui.id().with(("audio scene menu", name)), Sense::click());
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Audio scene {name}")));
	let response = kit::tip(
		response,
		&format!("Right-click to rename, move, or remove {name}"),
		None,
	);
	response.context_menu(|ui| {
		let mut asked = None;
		if ui.button("Rename").clicked() {
			asked = Some(Change::Rename(name.clone()));
		}
		if ui.add_enabled(index > 0, egui::Button::new("Move left")).clicked() {
			asked = Some(Change::Move(name.clone(), -1));
		}
		let last = index + 1 == board.masters.len();
		if ui.add_enabled(!last, egui::Button::new("Move right")).clicked() {
			asked = Some(Change::Move(name.clone(), 1));
		}
		let remove = ui
			.add_enabled(!scene.program, egui::Button::new("Remove"))
			.on_disabled_hover_text("The stream hears it; put another on the stream first");
		if remove.clicked() {
			asked = Some(Change::Remove(name.clone()));
		}
		if let Some(asked) = asked {
			changes.push(asked);
			ui.close();
		}
	});
}

/// The dashed tile that adds an audio scene, saying whether it was clicked.
fn add_tile(ui: &mut Ui, rect: Rect) -> bool {
	let response = ui.interact(rect, ui.id().with("add audio scene"), Sense::click());
	let response = kit::tip(response, "Add an audio scene", None);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add an audio scene"));
	let ink = if response.hovered() { kit::TEXT } else { kit::FAINT };
	let stroke = Stroke::new(1.5, ink.gamma_multiply(0.6));
	ui.painter()
		.rect_stroke(rect.shrink(1.0), 12, stroke, StrokeKind::Inside);
	Icon::Add.paint(ui, rect, 18.0, ink);
	response.clicked()
}

/// A send dot: from a strip into an audio scene, and how it reaches it.
#[derive(Clone, Copy)]
struct Dot<'a> {
	from: Strip<'a>,
	to: &'a Master,
	reach: &'a Reach,
	/// Whether the dot carries its audio scene's letter, as a strip's do and
	/// the matrix's, under their columns' names, do not.
	letter: bool,
}

impl Dot<'_> {
	fn from(&self) -> &str {
		match self.from {
			Strip::Sound(name) | Strip::Scene(name) => name,
		}
	}

	/// Draw it in `at`, and toggle the send on a click.
	fn ui(self, ui: &mut Ui, at: Rect, changes: &mut Vec<Change>) {
		let (from, to, reach) = (self.from(), &self.to.name, self.reach);
		let scene = matches!(self.from, Strip::Scene(_));
		let enabled = !matches!(reach, Reach::Own | Reach::Loop);
		let sense = if enabled { Sense::click() } else { Sense::hover() };
		let response = ui.interact(at, ui.id().with(("send", from, scene, to)), sense);
		response.widget_info(|| {
			let name = format!("Send {from} to {to}");
			egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *reach == Reach::Sent, name)
		});
		let response = kit::tip(response, &reach.says(from, to), None);
		let letter = if self.letter { self.to.letter.as_str() } else { "" };
		paint_dot(ui.painter(), at, reach, letter, self.to.program);
		if response.hovered() && enabled {
			ui.painter()
				.circle_stroke(at.center(), at.width() / 2.0 + 3.0, Stroke::new(2.0, kit::ACCENT));
		}
		if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
			changes.push(Change::Send {
				scene,
				from: from.to_string(),
				to: to.clone(),
				on: *reach != Reach::Sent,
			});
		}
	}
}

/// A dot under a strip for each audio scene, in `rect`, from `from`, which
/// reaches each as `reach` says.
fn sends(ui: &mut Ui, rect: Rect, from: Strip<'_>, reach: &[Reach], board: &Board, changes: &mut Vec<Change>) {
	ui.painter()
		.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, kit::LINE));
	let dots = dot_rects(rect, board.masters.len());
	for ((to, reach), at) in board.masters.iter().zip(reach).zip(dots) {
		let send = Dot {
			from,
			to,
			reach,
			letter: true,
		};
		send.ui(ui, at, changes);
	}
}

/// Where `count` send dots sit in `rect`: a row along its foot, centered, as
/// big as fit.
fn dot_rects(rect: Rect, count: usize) -> impl Iterator<Item = Rect> {
	let n = count.max(1) as f32;
	let size = ((rect.width() - (n - 1.0) * DOT_GAP) / n).clamp(DOT[0], DOT[1]);
	let left = rect.center().x - (n * size + (n - 1.0) * DOT_GAP) / 2.0;
	let y = rect.bottom() - size / 2.0;
	(0..count).map(move |at| {
		let x = left + at as f32 * (size + DOT_GAP) + size / 2.0;
		Rect::from_center_size(pos2(x, y), Vec2::splat(size))
	})
}

/// A matrix cell: its dot, and the audio scene a sound comes through, or
/// stripes where an audio scene meets itself.
fn route_cell(ui: &mut Ui, cell: Rect, send: Dot<'_>, changes: &mut Vec<Change>) {
	if *send.reach == Reach::Own {
		let painter = ui.painter().with_clip_rect(cell.shrink(1.0));
		painter.extend(kit::stripes(cell, kit::LINE.gamma_multiply(0.5)));
		return;
	}
	let center = match send.reach {
		Reach::Through(_) => cell.center() - vec2(0.0, 6.0),
		_ => cell.center(),
	};
	let at = Rect::from_center_size(center, Vec2::splat(22.0));
	send.ui(ui, at, changes);
	if let Reach::Through(held) = send.reach {
		ui.painter().text(
			pos2(cell.center().x, at.bottom() + 8.0),
			Align2::CENTER_CENTER,
			format!("through {held}"),
			kit::ui(10.5),
			kit::DIM,
		);
	}
}

/// Paint a send dot in `at`: filled where it is sent, the accent's fill on
/// the audio scene the stream carries; ringed where it is heard only through
/// another audio scene; a faint dot where it is not heard; a dash where it
/// cannot be sent. `letter` names its audio scene.
fn paint_dot(painter: &egui::Painter, at: Rect, reach: &Reach, letter: &str, program: bool) {
	let center = at.center();
	let radius = at.width() / 2.0;
	let font = kit::display((at.width() * 0.55).round());
	let ink = match reach {
		Reach::Sent => {
			let fill = if program { kit::ACCENT } else { kit::TEXT };
			painter.circle_filled(center, radius, fill);
			if program {
				kit::TEXT
			} else {
				egui::Color32::from_rgb(0x11, 0x11, 0x11)
			}
		}
		Reach::Through(_) => {
			painter.circle_stroke(center, radius - 1.0, Stroke::new(2.0, kit::TEXT));
			kit::TEXT
		}
		Reach::Not => {
			painter.circle_filled(center, radius, kit::P2);
			kit::FAINT
		}
		Reach::Own | Reach::Loop => {
			let half = (radius * 0.4).max(3.0);
			painter.hline(center.x - half..=center.x + half, center.y, Stroke::new(2.0, kit::LINE));
			return;
		}
	};
	if !letter.is_empty() && at.width() >= 14.0 {
		painter.text(center, Align2::CENTER_CENTER, letter, font, ink);
	}
}

/// The height the output picker's popover takes besides its list of outputs:
/// its heading, the system default, the button to look again, its margins,
/// and the gap and the half of the button it opens from.
const OUTPUTS_AROUND: f32 = 120.0;

/// The least the list of outputs shows, a few rows, however short the window.
const OUTPUTS_LEAST: f32 = 96.0;

/// The audio outputs Listen can play on, listed when its tag first names a
/// picked output or its menu opens, and again on asking.
#[derive(Default)]
pub(super) struct Speakers {
	listing: Option<Result<Vec<mix::Speaker>, String>>,
	pending: Option<std::sync::mpsc::Receiver<Result<Vec<mix::Speaker>, String>>>,
}

impl Speakers {
	/// List the outputs again, in the background.
	pub(super) fn refresh(&mut self, runtime: &tokio::runtime::Handle) {
		if self.pending.is_some() {
			return;
		}
		let (send, recv) = std::sync::mpsc::channel();
		runtime.spawn(async move {
			let _ = send.send(mix::speakers().await);
		});
		self.pending = Some(recv);
	}

	/// Take a listing that has come back.
	fn poll(&mut self) {
		if let Some(listing) = self.pending.as_ref().and_then(|pending| pending.try_recv().ok()) {
			self.listing = Some(listing);
			self.pending = None;
		}
	}

	/// The output `speaker` names, as the listing calls it, listing the
	/// outputs first if nothing has yet; marked if the listing lacks it,
	/// such as a headset since unplugged.
	pub(super) fn named(&mut self, runtime: &tokio::runtime::Handle, speaker: Option<&str>) -> String {
		self.poll();
		let Some(id) = speaker else {
			return "System default".to_string();
		};
		match &self.listing {
			Some(Ok(speakers)) => speakers
				.iter()
				.find(|found| found.id == id)
				.map_or_else(|| format!("{id} (not found)"), |found| found.name.clone()),
			Some(Err(_)) => id.to_string(),
			None => {
				self.refresh(runtime);
				id.to_string()
			}
		}
	}

	/// The outputs to pick `speaker` from, or the system default; whether
	/// the pick changed.
	pub(super) fn menu(&mut self, ui: &mut Ui, speaker: &mut Option<String>) -> bool {
		self.poll();
		let before = speaker.clone();
		ui.label(kit::caps("Listen on", kit::display(11.0)).color(kit::DIM));
		ui.selectable_value(speaker, None, "System default");
		match &self.listing {
			Some(Ok(speakers)) => {
				// A desktop can list dozens of outputs. Popovers flip to the
				// roomier side of their button, which has at least half the
				// window, so the list scrolls within that, less the rows around it.
				let room = (ui.ctx().content_rect().height() / 2.0 - OUTPUTS_AROUND).max(OUTPUTS_LEAST);
				egui::ScrollArea::vertical()
					.id_salt("outputs")
					.max_height(room)
					.auto_shrink([false, true])
					.show(ui, |ui| {
						for found in speakers {
							ui.selectable_value(speaker, Some(found.id.clone()), &found.name);
						}
					});
			}
			Some(Err(err)) => {
				ui.colored_label(kit::ERR, format!("Cannot list the outputs: {err}"));
			}
			None => {
				ui.spinner();
			}
		}
		*speaker != before
	}
}

#[cfg(test)]
mod tests {
	use egui_kittest::kittest::Queryable;

	use super::*;

	/// However many outputs a desktop lists, the picker opened from a tag at
	/// the foot of a short window stays in it, and the wheel scrolls to the
	/// last output to pick it.
	#[test]
	fn many_outputs_scroll_in_the_window() {
		const WINDOW: Vec2 = vec2(800.0, 400.0);
		let listing = (1..=28)
			.map(|at| mix::Speaker {
				id: format!("out{at}"),
				name: format!("Output {at}"),
			})
			.collect();
		let speakers = Speakers {
			listing: Some(Ok(listing)),
			pending: None,
		};
		// The outputs, the pick, the popover's rect, and whether the kit's
		// faces, which take effect a frame later, are installed.
		let mut harness = egui_kittest::Harness::builder().with_size(WINDOW).build_ui_state(
			|ui, (speakers, speaker, popover, ready): &mut (Speakers, Option<String>, Rect, bool)| {
				if !std::mem::replace(ready, true) {
					kit::install(ui.ctx());
					return;
				}
				ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
					let response = ui.button("On System default");
					let shown = egui::Popup::from_response(&response)
						.open(true)
						.frame(kit::popover_frame())
						.align(egui::RectAlign::BOTTOM)
						.gap(6.0)
						.width(280.0)
						.show(|ui| {
							speakers.menu(ui, speaker);
							ui.add(kit::button(Icon::Reload, "Look for outputs again"));
						});
					*popover = shown.expect("the popover is open").response.rect;
				});
			},
			(speakers, None, Rect::NOTHING, false),
		);
		harness.run();
		let window = Rect::from_min_size(pos2(0.0, 0.0), WINDOW);
		let popover = harness.state().2;
		assert!(window.contains_rect(popover), "{popover:?} in {window:?}");
		assert!(
			harness.get_by_label("Output 28").rect().top() > popover.bottom(),
			"the last output starts out of sight"
		);
		harness.get_by_label("Output 1").hover();
		harness.run();
		harness.event(egui::Event::MouseWheel {
			unit: egui::MouseWheelUnit::Point,
			delta: vec2(0.0, -2000.0),
			phase: egui::TouchPhase::Move,
			modifiers: egui::Modifiers::NONE,
		});
		// The wheel scrolls smoothly, a frame at a time.
		harness.run_steps(60);
		let last = harness.get_by_label("Output 28").rect();
		assert!(popover.contains_rect(last), "{last:?} in {popover:?}");
		assert!(
			popover.contains_rect(harness.get_by_label("System default").rect()),
			"the system default stays above the list"
		);
		harness.get_by_label("Output 28").click();
		harness.run();
		assert_eq!(harness.state().1.as_deref(), Some("out28"));
	}

	/// The taper runs from silence to the most boost, unity where the fader
	/// marks it, and back again.
	#[test]
	fn the_taper_runs_both_ways() {
		assert_eq!(taper(MUTE_DB), 0.0);
		assert_eq!(taper(-200.0), 0.0, "quieter than silence is silence");
		assert_eq!(taper(0.0), UNITY);
		assert_eq!(taper(12.0), 1.0);
		assert_eq!(taper(40.0), 1.0, "a scene file's bigger boost tops the fader out");
		let mut last = -1.0;
		for step in 0..=720 {
			let db = MUTE_DB + step as f32 * 0.1;
			let at = taper(db);
			assert!(at > last, "rises at {db} dB");
			last = at;
			assert!((untaper(at) - db).abs() < 1e-3, "{db} dB comes back");
		}
		assert_eq!(untaper(0.0), MUTE_DB);
	}

	/// However many audio scenes there are, a strip at its narrowest holds
	/// each one's dot within it, so a click never lands on a neighbour's.
	#[test]
	fn every_send_dot_fits_its_strip() {
		for masters in 0..=16 {
			let floor = strip_floor(masters);
			for width in [NARROW.max(floor), WIDE.max(floor), MASTER[0].max(floor)] {
				let strip = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 400.0));
				let room = Parts::of(strip).sends;
				let dots: Vec<Rect> = dot_rects(room, masters).collect();
				assert_eq!(dots.len(), masters);
				for (at, dot) in dots.iter().enumerate() {
					assert!(
						room.expand(1e-3).contains_rect(*dot),
						"{masters} dots at {width}: {dot:?} in {room:?}"
					);
					assert!(dot.width() >= DOT[0]);
					if let Some(next) = dots.get(at + 1) {
						assert!(dot.right() < next.left(), "{masters} dots at {width} overlap");
					}
				}
			}
		}
	}

	/// A desk meter turns hot at the same level as the strip's, which places
	/// levels evenly in decibels from silence and keeps the meter's own
	/// threshold.
	#[test]
	fn meters_agree_on_hot() {
		let hot = gain(HOT_DB);
		assert!((super::super::levels::position(hot) - kit::Meter::HOT).abs() < 1e-5);
		assert!((untaper(lit(hot) * UNITY) - HOT_DB).abs() < 1e-3);
		assert!(lit(gain(HOT_DB - 1.0)) < lit(hot) && lit(hot) < lit(gain(HOT_DB + 1.0)));
	}

	/// A meter lights to its top at full scale, which is unity on the scale.
	#[test]
	fn full_scale_lights_the_meter() {
		assert_eq!(lit(1.0), 1.0);
		assert_eq!(lit(0.0), 0.0);
		assert!((lit(0.5) - taper(-6.0206) / UNITY).abs() < 1e-3);
	}

	/// "Mics" holds the microphone and "Stream" holds Mics, so the microphone
	/// reaches Stream through Mics, and "Headphones" holds Stream and the
	/// microphone too, which it still hears once.
	fn collection() -> Collection {
		let mut collection = Collection::default();
		let scene = |name: &str, sounds: &[&str], scenes: &[&str]| AudioScene {
			sounds: sounds.iter().map(|name| name.to_string()).collect(),
			scenes: scenes.iter().map(|name| name.to_string()).collect(),
			..AudioScene::new(name)
		};
		collection.audio_scenes = vec![
			AudioScene {
				program: true,
				..scene("Stream", &["music"], &["Mics"])
			},
			scene("Mics", &["mic"], &[]),
			scene("Headphones", &["mic"], &["Stream"]),
			scene("Recording", &[], &[]),
		];
		collection
	}

	fn reach(collection: &Collection, from: Strip<'_>, to: &str) -> Reach {
		Reach::of(collection, from, collection.audio_scene(to).unwrap())
	}

	/// A dot is filled where a sound is sent, even if it also comes through
	/// another audio scene, ringed where it comes only through one, and empty
	/// where it is not heard.
	#[test]
	fn a_sound_reaches_by_any_route() {
		let collection = collection();
		let mic = Strip::Sound("mic");
		assert_eq!(reach(&collection, mic, "Mics"), Reach::Sent);
		assert_eq!(reach(&collection, mic, "Stream"), Reach::Through("Mics".into()));
		assert_eq!(
			reach(&collection, mic, "Headphones"),
			Reach::Sent,
			"sent and through Stream"
		);
		assert_eq!(reach(&collection, mic, "Recording"), Reach::Not);
		let music = Strip::Sound("music");
		assert_eq!(reach(&collection, music, "Headphones"), Reach::Through("Stream".into()));
		assert_eq!(reach(&collection, music, "Mics"), Reach::Not);
		assert!(!reach(&collection, Strip::Sound("other"), "Stream").heard());
	}

	/// A sound published independently reaches each audio scene as it is
	/// sent, which is where people in the studio hear it, though the mixes
	/// the studio encodes leave it out.
	#[test]
	fn a_published_sound_reaches_where_it_is_sent() {
		let unpublished = collection();
		let mut collection = collection();
		collection.published.insert("mic".into(), Default::default());
		for to in ["Mics", "Stream", "Headphones", "Recording"] {
			assert_eq!(
				reach(&collection, Strip::Sound("mic"), to),
				reach(&unpublished, Strip::Sound("mic"), to),
				"{to}"
			);
		}
	}

	/// An audio scene cannot be sent into itself, nor into one it holds,
	/// even through another; it can reach one through another that holds it.
	#[test]
	fn an_audio_scene_never_holds_itself() {
		let collection = collection();
		let mics = Strip::Scene("Mics");
		assert_eq!(reach(&collection, mics, "Mics"), Reach::Own);
		assert_eq!(reach(&collection, mics, "Stream"), Reach::Sent);
		assert_eq!(reach(&collection, mics, "Headphones"), Reach::Through("Stream".into()));
		assert_eq!(reach(&collection, mics, "Recording"), Reach::Not);
		let headphones = Strip::Scene("Headphones");
		assert_eq!(reach(&collection, headphones, "Stream"), Reach::Loop);
		assert_eq!(reach(&collection, headphones, "Mics"), Reach::Loop, "through Stream");
		assert_eq!(reach(&collection, Strip::Scene("Stream"), "Headphones"), Reach::Sent);
	}

	/// The matrix's foot says what each audio scene holds.
	#[test]
	fn what_an_audio_scene_holds() {
		let collection = collection();
		let holds = |name: &str| holds(collection.audio_scene(name).unwrap());
		assert_eq!(holds("Stream"), "1 sound and Mics");
		assert_eq!(holds("Recording"), "Nothing");
		let mut scene = AudioScene::new("All");
		scene.sounds = ["a", "b"].map(String::from).into();
		scene.scenes = ["Mics", "Stream"].map(String::from).into();
		assert_eq!(super::holds(&scene), "2 sounds, Mics and Stream");
	}
}
