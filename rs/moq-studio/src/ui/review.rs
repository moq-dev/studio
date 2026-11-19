//! The REVIEW page: the last 30 seconds as a strip to pick a stretch from,
//! what to replay and how fast, Replay and Save clip, and beside them this
//! session's markers and this show's recordings and clips.

use egui::{Align2, CursorIcon, Rect, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};
use kio::clock::Instant;

use super::*;
use crate::controls;
use crate::markers::{self, Marker};
use crate::past::{self, Past};
use crate::shelf::{self, Shelf};

/// How wide the recordings column is, at most.
const SHELF: f32 = 384.0;
/// How wide the "replay what" column is.
const WHAT: f32 = 160.0;
/// How tall a card in it is.
const WHAT_CARD: f32 = 90.0;
/// How tall the Replay and Save clip buttons are.
const ACTION: f32 = 70.0;
/// How far from a handle a press still grabs it, in points.
const GRAB: f32 = 12.0;
/// How near the strip's right edge a dragged end pins the stretch to now.
const PIN: f32 = 8.0;
/// How tall the sound under the strip's pictures is, and how wide each bar.
const WAVE: f32 = 28.0;
const BAR: f32 = 6.0;
/// How tall a marker's row is, and how many show before the list scrolls.
const MARKER_ROW: f32 = 32.0;
const MARKER_ROWS: usize = 4;

/// The page's own state: the strip, the cards, and what a drag holds.
pub(super) struct Review {
	past: Past,
	shelf: Shelf,
	/// Each picture on screen, uploaded once, by its id.
	textures: BTreeMap<u64, egui::TextureHandle>,
	/// Where the scrub head is, or `None` at the stretch's end.
	head: Option<Instant>,
	/// What a drag on the strip moves.
	grip: Option<Grip>,
	/// The last outcome of a replay or clip shown, so each shows once.
	outcome: Option<Result<String, String>>,
	/// Each marker's name as typed, by number, until its field lets go.
	drafts: BTreeMap<u32, String>,
	/// The moment the page draws as now, held still for the window's views.
	frozen: Option<Instant>,
}

/// What a drag on the strip moves.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
	/// The stretch's start or end.
	From,
	To,
	/// The scrub head.
	Head,
}

impl Review {
	pub(super) fn new(engine: &Arc<Engine>, runtime: &tokio::runtime::Handle) -> Self {
		let recorder = &engine.recorder;
		Self {
			past: Past::new(engine.clone()),
			shelf: Shelf::new(&recorder.dir(), recorder.show(), runtime.clone()),
			textures: BTreeMap::new(),
			head: None,
			grip: None,
			outcome: None,
			drafts: BTreeMap::new(),
			frozen: None,
		}
	}

	/// List the recordings folder `engine` records into now, as it moved.
	pub(super) fn shelve(&mut self, engine: &Engine, runtime: &tokio::runtime::Handle) {
		let recorder = &engine.recorder;
		self.shelf = Shelf::new(&recorder.dir(), recorder.show(), runtime.clone());
	}

	/// Show `listing` as the cards, for the window's views.
	#[cfg(test)]
	pub(super) fn seed_shelf(&mut self, listing: shelf::Listing) {
		self.shelf.seed(listing);
	}

	/// Show `view` as the strip, held still at `now`, with the head `head`
	/// ago, for the window's views.
	#[cfg(test)]
	pub(super) fn seed_past(&mut self, view: past::View, now: Instant, head: Duration) {
		self.frozen = Some(now);
		self.head = Some(now.saturating_sub(head));
		self.past.seed(view);
	}

	fn now(&self) -> Instant {
		self.frozen.unwrap_or_else(Instant::now)
	}

	/// The picture shown at or before `at`, or the first after it when none was.
	fn picture_at<'a>(&self, view: &'a past::View, at: Instant) -> Option<&'a past::Picture> {
		let before = view.pictures.iter().rev().find(|picture| picture.at <= at);
		before.or(view.pictures.first()).map(|picture| &**picture)
	}

	/// Upload each picture not on the GPU yet, and let go of those gone.
	fn upload(&mut self, ctx: &egui::Context, view: &past::View) {
		let mut kept = BTreeMap::new();
		for picture in &view.pictures {
			let texture = self.textures.remove(&picture.id).unwrap_or_else(|| {
				let image = egui::ColorImage::from_rgba_unmultiplied(picture.size, &picture.rgba);
				ctx.load_texture(format!("past-{}", picture.id), image, egui::TextureOptions::LINEAR)
			});
			kept.insert(picture.id, texture);
		}
		self.textures = kept;
	}

	fn texture(&self, picture: &past::Picture) -> Option<egui::load::SizedTexture> {
		let texture = self.textures.get(&picture.id)?;
		let size = vec2(picture.size[0] as f32, picture.size[1] as f32);
		Some(egui::load::SizedTexture::new(texture.id(), size))
	}
}

/// What a marker's row asks for, besides its name, which it takes itself.
enum Row {
	Clip,
	Remove,
}

/// Why REVIEW's picks cannot play or save now, if they cannot.
struct Refusals {
	replay: Option<String>,
	save: Option<String>,
}

impl Panels {
	/// The page in `stage`: the strip and its actions, and the markers over
	/// the recordings in the column on the right. `shape` is the program's,
	/// width over height.
	pub(super) fn review(&mut self, ui: &mut Ui, fx: &mut Fx, stage: Rect, shape: f32) {
		let side = SHELF.min(stage.width() * 0.34);
		let shelf = stage.with_min_x(stage.right() - side);
		let main = stage.with_max_x(shelf.left());
		kit::wash(ui.painter(), main);
		shell::bar(ui, fx, shelf);
		ui.painter()
			.vline(shelf.left() + 0.5, shelf.y_range(), Stroke::new(1.0, kit::LINE));
		self.replays(ui, main.shrink2(vec2(24.0, 14.0)), shape);
		let column = shelf.shrink2(vec2(16.0, 14.0));
		let marked = self.controls.markers.list();
		let rows = match marked.len() {
			// Room for the lines saying how to drop one.
			0 => 1.5,
			count => count.min(MARKER_ROWS) as f32,
		};
		let height = (34.0 + rows * MARKER_ROW).min(column.height() * 0.45);
		let marks = column.with_max_y(column.top() + height);
		ui.painter()
			.hline(column.x_range(), marks.bottom() + 9.5, Stroke::new(1.0, kit::LINE));
		self.markers(ui, marks, &marked);
		self.shelf(ui, column.with_min_y(marks.bottom() + 20.0));
	}

	/// This session's markers, the newest first, each with how long ago it
	/// was dropped, its name to type, a clip around it, and its removal.
	fn markers(&mut self, ui: &mut Ui, rect: Rect, marked: &[Marker]) {
		let painter = ui.painter();
		let title = kit::caps_galley(painter, "Markers", kit::display_bold(22.0), kit::TEXT, None);
		let title_height = title.size().y;
		painter.galley(rect.left_top(), title, kit::TEXT);
		painter.text(
			pos2(rect.right(), rect.top() + title_height / 2.0),
			Align2::RIGHT_CENTER,
			"of this session",
			kit::ui(11.5),
			kit::DIM,
		);
		let list = rect.with_min_y(rect.top() + title_height + 8.0);
		let now = self.review.now();
		let failure = self.controls.markers.failure();
		self.review
			.drafts
			.retain(|number, _| marked.iter().any(|marker| marker.number == *number));
		let key = self.key(&Action::Marker, ui.ctx());
		// What a row asked for, done once the list is drawn.
		let mut asked = None;
		ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
			if let Some(failure) = &failure {
				ui.label(egui::RichText::new(capitalize(failure)).color(kit::ERR));
			}
			if marked.is_empty() {
				let how = match &key {
					Some(key) => format!("Drop one with {key} or the flag beside the panic key as the show runs."),
					None => {
						"Drop one with the flag beside the panic key, or a key bound in Settings, as the show runs."
							.into()
					}
				};
				ui.label(egui::RichText::new(how).color(kit::DIM));
				return;
			}
			egui::ScrollArea::vertical()
				.id_salt("markers")
				.auto_shrink(false)
				.show(ui, |ui| {
					ui.spacing_mut().item_spacing.y = 0.0;
					for marker in marked.iter().rev() {
						let size = vec2(ui.available_width(), MARKER_ROW);
						ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
							ui.spacing_mut().item_spacing.x = 6.0;
							if let Some(row) = self.marker_row(ui, marker, now) {
								asked = Some((marker.number, row));
							}
						});
					}
				});
		});
		let dir = self.engine.recorder.dir();
		match asked {
			Some((number, Row::Clip)) => {
				let result = self.controls.clip_marker(number);
				self.report(result);
			}
			Some((number, Row::Remove)) => {
				let result = self.controls.markers.remove(&dir, number);
				self.check(result);
			}
			None => {}
		}
	}

	/// One marker's row, naming the marker once its name loses focus, and
	/// returning what else it asked for.
	fn marker_row(&mut self, ui: &mut Ui, marker: &Marker, now: Instant) -> Option<Row> {
		let badge = ui
			.painter()
			.layout_no_wrap(marker.number.to_string(), kit::mono(11.0), kit::TEXT);
		let (badge_rect, _) = ui.allocate_exact_size(vec2(badge.size().x.max(14.0) + 10.0, 20.0), Sense::hover());
		ui.painter().rect_filled(badge_rect, 4, kit::P3);
		ui.painter()
			.galley(badge_rect.center() - badge.size() / 2.0, badge, kit::TEXT);

		let ago = now.saturating_duration_since(marker.at);
		let clippable = ago < replay::WINDOW;
		let when = ui.painter().layout_no_wrap(since(ago), kit::mono(11.0), kit::DIM);
		// The name takes what the age and the two buttons leave.
		let field = (ui.available_width() - when.size().x - 2.0 * (24.0 + 6.0) - 12.0).max(40.0);
		let mut text = match self.review.drafts.get(&marker.number) {
			Some(draft) => draft.clone(),
			None => marker.name.clone().unwrap_or_default(),
		};
		// Named for the marker, so focus stays on it when a newer one is
		// dropped above it.
		let edit = egui::TextEdit::singleline(&mut text)
			.id(egui::Id::new(("marker name", marker.number)))
			.hint_text(format!("Marker {}", marker.number))
			.char_limit(markers::MAX_NAME)
			.desired_width(field);
		let response = ui.add(edit);
		let mut asked = None;
		if response.gained_focus() || response.changed() {
			self.review.drafts.insert(marker.number, text);
		}
		// Taken here, so a click on the row's buttons, or on another marker's
		// name, that ends the typing keeps the name too.
		if response.lost_focus()
			&& let Some(draft) = self.review.drafts.remove(&marker.number)
			&& draft.trim() != marker.name.as_deref().unwrap_or("")
		{
			let dir = self.engine.recorder.dir();
			let result = self.controls.markers.rename(&dir, marker.number, &draft);
			self.check(result);
		}
		let (when_rect, _) = ui.allocate_exact_size(when.size(), Sense::hover());
		ui.painter().galley(when_rect.min, when, kit::DIM);
		let note = match clippable {
			true => format!(
				"Save from {} s before it to {} s after it, as Save clip does",
				replay::BEFORE.as_secs(),
				replay::AFTER.as_secs()
			),
			false => format!(
				"It is older than the last {} s, as far back as a clip reaches",
				replay::WINDOW.as_secs()
			),
		};
		let clip = IconButton::new(Icon::Clip, "Clip around it").small().note(Some(&note));
		if ui.add_enabled(clippable, clip).clicked() {
			asked = Some(Row::Clip);
		}
		let remove = IconButton::new(Icon::Close, "Remove the marker").small();
		if ui.add(remove).clicked() {
			asked = Some(Row::Remove);
		}
		asked
	}

	/// The large picture and what to replay, the strip, the speed, and the
	/// two actions, top to bottom in `rect`.
	fn replays(&mut self, ui: &mut Ui, rect: Rect, shape: f32) {
		let replayable = self.replayable();
		let controls = self.controls.clone();
		let mut replay = controls.replay.lock().unwrap();
		if let Of::Source(name) = &replay.of
			&& !replayable.iter().any(|(theirs, _)| *theirs == *name)
		{
			replay.of = Of::Program;
		}
		let mut view = self.review.past.watch(&replay.of);
		// Until the thread samples a new pick, nothing of the old one shows.
		if view.of != replay.of {
			view = Arc::new(past::View {
				of: replay.of.clone(),
				..past::View::default()
			});
		}
		self.review.upload(ui.ctx(), &view);
		ui.ctx().request_repaint_after(Duration::from_millis(250));

		let actions = Rect::from_min_max(pos2(rect.left(), rect.bottom() - ACTION), rect.max);
		let strip_height = 22.0 + (rect.width() / 6.0 * 9.0 / 16.0).clamp(48.0, 96.0) + 6.0 + WAVE + 50.0;
		let strip = Rect::from_min_max(
			pos2(rect.left(), actions.top() - 18.0 - strip_height),
			pos2(rect.right(), actions.top() - 18.0),
		);
		let top = Rect::from_min_max(rect.min, pos2(rect.right(), strip.top() - 14.0));
		let what = Rect::from_min_max(pos2(top.right() - WHAT, top.top()), top.max);
		let picture = super::layout::fit(top.with_max_x(what.left() - 18.0).shrink(6.0), shape);

		self.large(ui, &view, picture, &replay);
		self.what(ui, what, &replayable, &mut replay);
		self.past_strip(ui, strip, &view, &mut replay);
		let sound_only = view
			.held
			.as_ref()
			.and_then(|held| held.as_ref().ok())
			.is_some_and(|held| !held.video)
			|| replayable
				.iter()
				.any(|(name, sound)| *sound && matches!(&replay.of, Of::Source(of) if of == name));
		let refusals = self.refusals(&view, &replay, sound_only);
		let fired = self.actions(ui, actions, &mut replay, &refusals);
		// How a replay or clip went, once, as the window's message.
		let outcome = replay.outcome.lock().unwrap().clone();
		drop(replay);
		if outcome != self.review.outcome {
			self.review.outcome = outcome.clone();
			if let Some(outcome) = outcome {
				self.message = Some((outcome, kio::clock::Instant::now()));
			}
		}
		if let Some(action) = fired {
			let _ = controls.fire(&action);
		}
	}

	/// Each source with a past to replay, and whether it is sound alone.
	fn replayable(&self) -> Vec<(String, bool)> {
		let state = self.engine.state.lock().unwrap();
		let collection = state.collection();
		// What a source shows or plays, a clone its source's, has a past only
		// if something encodes it: not an image, a color, a web page, an
		// overlay, a LUT, or a scene or the program drawn into another.
		collection
			.sources
			.keys()
			.filter_map(|name| {
				let captured = collection.sources.get(collection.capture(name))?;
				match captured {
					Source::Image { .. }
					| Source::Color { .. }
					| Source::Browser { .. }
					| Source::Overlay { .. }
					| Source::Lut { .. }
					| Source::Scene { .. }
					| Source::Program
					| Source::Replay => None,
					Source::Microphone { .. } | Source::SystemAudio => Some((name.clone(), true)),
					_ => Some((name.clone(), false)),
				}
			})
			.collect()
	}

	/// The picture under the scrub head, large, with how long ago it was.
	fn large(&mut self, ui: &mut Ui, view: &past::View, rect: Rect, replay: &controls::Replay) {
		let painter = ui.painter();
		painter.rect_filled(rect, 10, kit::P1);
		painter.rect_stroke(
			rect.expand(3.0),
			12,
			Stroke::new(2.0, kit::LINE),
			egui::StrokeKind::Outside,
		);
		let now = self.review.now();
		let head = self.head(replay, now);
		let picture = self.review.picture_at(view, head);
		match (picture.and_then(|picture| self.review.texture(picture)), &view.held) {
			(Some(texture), _) => {
				egui::Image::new(texture)
					.corner_radius(10)
					.uv(cover(texture.size, rect.size()))
					.paint_at(ui, rect);
			}
			(None, held) => {
				let why = match held {
					None => "Reading the last 30 seconds".to_string(),
					Some(Err(why)) => capitalize(why),
					Some(Ok(held)) if !held.video => "Sound only: nothing to show".to_string(),
					Some(Ok(_)) => "No pictures held".to_string(),
				};
				let galley = ui.painter().layout(why, kit::ui(14.0), kit::DIM, rect.width() - 80.0);
				ui.painter()
					.galley(rect.center() - galley.size() / 2.0, galley, kit::DIM);
			}
		}
		let ago = now.saturating_duration_since(head);
		let text = match ago < Duration::from_millis(500) {
			true => "Now".to_string(),
			false => format!("{} s ago", ago.as_secs_f32().round()),
		};
		Tag::AGO.paint(ui.painter(), rect.left_top() + vec2(14.0, 14.0), &text);
	}

	/// Where the scrub head is: where it was put, or the stretch's end.
	fn head(&self, replay: &controls::Replay, now: Instant) -> Instant {
		let (_, to) = ages(replay.stretch, now);
		let oldest = now.saturating_sub(replay::WINDOW);
		self.review.head.unwrap_or(now.saturating_sub(to)).max(oldest).min(now)
	}

	/// What to replay: the program's live picture, then each source with a
	/// past, by its live picture or, for sound alone, its name.
	fn what(&mut self, ui: &mut Ui, rect: Rect, replayable: &[(String, bool)], replay: &mut controls::Replay) {
		let caption = Rect::from_min_size(rect.min, vec2(rect.width(), 18.0));
		caption_text(ui.painter(), caption.left_center(), "Replay what", Align2::LEFT_CENTER);
		let room = rect.with_min_y(caption.bottom() + 4.0);
		ui.scope_builder(UiBuilder::new().max_rect(room), |ui| {
			egui::ScrollArea::vertical()
				.id_salt("replay what")
				.auto_shrink(false)
				.show(ui, |ui| {
					ui.spacing_mut().item_spacing.y = 8.0;
					let program = egui::load::SizedTexture::new(
						self.program.id,
						vec2(self.program_drawn.width as f32, self.program_drawn.height as f32),
					);
					let card = What {
						name: "Program",
						picture: Some(program),
						sound: false,
						on: replay.of == Of::Program,
						air: self.engine.on_air().live().is_some(),
					};
					if card.show(ui).clicked() {
						replay.of = Of::Program;
					}
					for (name, sound) in replayable {
						let thumbnail = Thumbnail::Source(name.clone());
						let picture = match sound {
							true => None,
							false => self
								.thumbnails
								.get(&thumbnail)
								.map(|registered| egui::load::SizedTexture::new(registered.id, vec2(16.0, 9.0))),
						};
						let card = What {
							name,
							picture,
							sound: *sound,
							on: matches!(&replay.of, Of::Source(of) if of == name),
							air: false,
						};
						let response = card.show(ui);
						if !sound && ui.is_rect_visible(response.rect) {
							self.thumbnails_shown.insert(thumbnail);
						}
						if response.clicked() {
							replay.of = Of::Source(name.clone());
						}
					}
				});
		});
	}
}

/// A card in the "replay what" column.
struct What<'a> {
	name: &'a str,
	/// Its live picture, or `None` for sound alone, or before one is drawn.
	picture: Option<egui::load::SizedTexture>,
	/// Whether it is sound alone, which replays only at 100%.
	sound: bool,
	/// Whether it is picked.
	on: bool,
	/// Whether it is the program while live, tagged ON AIR.
	air: bool,
}

impl What<'_> {
	/// Draw the card, as wide as `ui`.
	fn show(self, ui: &mut Ui) -> egui::Response {
		let Self {
			name,
			picture,
			sound,
			on,
			air,
		} = self;
		let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), WHAT_CARD), Sense::click());
		response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
		let response = response.on_hover_cursor(CursorIcon::PointingHand);
		let painter = ui.painter();
		painter.rect_filled(rect, 9, kit::P2);
		match picture {
			Some(picture) => {
				egui::Image::new(picture)
					.corner_radius(9)
					.uv(cover(picture.size, rect.size()))
					.paint_at(ui, rect);
				let fade = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 30.0), rect.max);
				let mut mesh = egui::Mesh::default();
				let clear = Color32::TRANSPARENT;
				let dark = Color32::from_black_alpha(210);
				mesh.colored_vertex(fade.left_top(), clear);
				mesh.colored_vertex(fade.right_top(), clear);
				mesh.colored_vertex(fade.right_bottom(), dark);
				mesh.colored_vertex(fade.left_bottom(), dark);
				mesh.add_triangle(0, 1, 2);
				mesh.add_triangle(0, 2, 3);
				ui.painter().add(mesh);
			}
			None if sound => {
				Icon::Mic.paint(ui, rect.translate(vec2(0.0, -8.0)), 18.0, kit::DIM);
				let note = ui.painter().layout_no_wrap("100% only".into(), kit::ui(10.5), kit::DIM);
				ui.painter()
					.galley(rect.right_bottom() - vec2(8.0, 7.0) - note.size(), note, kit::DIM);
			}
			None => {}
		}
		let painter = ui.painter();
		let ink = if sound { kit::DIM } else { kit::TEXT };
		let label = kit::caps_galley(painter, name, kit::display_bold(13.0), ink, Some(rect.width() - 60.0));
		painter.galley(
			pos2(rect.left() + 8.0, rect.bottom() - 6.0 - label.size().y),
			label,
			ink,
		);
		if air {
			Tag::AIR.paint(painter, rect.left_top() + vec2(6.0, 6.0), "On air");
		}
		let (width, color) = match (on, response.hovered()) {
			(true, _) => (3.0, kit::TEXT),
			(false, true) => (1.0, kit::DIM),
			(false, false) => (1.0, kit::LINE),
		};
		painter.rect_stroke(rect, 9, Stroke::new(width, color), egui::StrokeKind::Inside);
		response
	}
}

impl Panels {
	/// The last 30 seconds, with the stretch picked between amber handles and
	/// the scrub head, in `rect`.
	fn past_strip(&mut self, ui: &mut Ui, rect: Rect, view: &past::View, replay: &mut controls::Replay) {
		let now = self.review.now();
		let header = Rect::from_min_size(rect.min, vec2(rect.width(), 16.0));
		caption_text(
			ui.painter(),
			header.left_center(),
			"The last 30 seconds",
			Align2::LEFT_CENTER,
		);
		let thumbs = Rect::from_min_max(
			pos2(rect.left() + 2.0, header.bottom() + 12.0),
			pos2(rect.right() - 2.0, rect.bottom() - 40.0 - WAVE - 6.0),
		);
		let wave = Rect::from_min_max(
			pos2(thumbs.left(), thumbs.bottom() + 6.0),
			pos2(thumbs.right(), thumbs.bottom() + 6.0 + WAVE),
		);
		let track = thumbs.union(wave);
		let window = replay::WINDOW.as_secs_f32();
		let x_at = |age: Duration| thumbs.left() + (1.0 - age.as_secs_f32() / window).clamp(0.0, 1.0) * thumbs.width();
		let age_at = |x: f32| {
			Duration::from_secs_f32(((1.0 - (x - thumbs.left()) / thumbs.width()) * window).clamp(0.0, window))
		};

		// A slot a picture wide, each showing the picture shown at its middle.
		let painter = ui.painter().with_clip_rect(thumbs);
		painter.rect_filled(thumbs, 8, kit::P2);
		let slots = (thumbs.width() / (thumbs.height() * 16.0 / 9.0)).ceil().max(1.0) as usize;
		let slot = thumbs.width() / slots as f32;
		let oldest = view
			.held
			.as_ref()
			.and_then(|held| held.as_ref().ok())
			.map(|held| held.oldest);
		for index in 0..slots {
			let cell = Rect::from_min_size(
				pos2(thumbs.left() + index as f32 * slot, thumbs.top()),
				vec2(slot, thumbs.height()),
			);
			let middle = now.saturating_sub(age_at(cell.center().x));
			if oldest.is_none_or(|oldest| middle < oldest) {
				continue;
			}
			let Some(texture) = self
				.review
				.picture_at(view, middle)
				.and_then(|picture| self.review.texture(picture))
			else {
				continue;
			};
			let inner = cell.shrink2(vec2(1.0, 0.0));
			painter.image(texture.id, inner, cover(texture.size, inner.size()), Color32::WHITE);
		}
		// What the window does not hold, such as before the source was on
		// program, is darker still.
		if let Some(oldest) = oldest {
			let held_from = x_at(now.saturating_duration_since(oldest));
			if held_from > thumbs.left() + 1.0 {
				let gone = thumbs.with_max_x(held_from);
				painter.rect_filled(gone, 0, Color32::from_black_alpha(140));
			}
		}
		// The stretch picked, and the head.
		let (from, to) = ages(replay.stretch, now);
		let (from_x, to_x) = (x_at(from), x_at(to));
		let shade = Color32::from_rgba_unmultiplied(8, 8, 12, 153);
		painter.rect_filled(thumbs.with_max_x(from_x), 0, shade);
		painter.rect_filled(thumbs.with_min_x(to_x), 0, shade);
		// The sound, a bar each few points for the loudest moment under it,
		// bright inside the stretch.
		if let Ok(sound) = &view.sound
			&& !sound.is_empty()
		{
			let bars = (wave.width() / BAR).floor() as usize;
			for index in 0..bars {
				let left = wave.left() + index as f32 * BAR;
				let (newer, older) = (now.saturating_sub(age_at(left + BAR)), now.saturating_sub(age_at(left)));
				let peak = sound
					.iter()
					.filter(|(at, _)| (older..newer).contains(at))
					.map(|(_, peak)| *peak)
					.fold(None, |loudest: Option<f32>, peak| {
						Some(loudest.unwrap_or(0.0).max(peak))
					});
				let Some(peak) = peak else { continue };
				let height = 3.0 + peak.sqrt() * (WAVE - 3.0);
				let bar = Rect::from_center_size(pos2(left + BAR / 2.0, wave.center().y), vec2(BAR - 2.0, height));
				let inside = bar.center().x > from_x && bar.center().x < to_x;
				ui.painter()
					.rect_filled(bar, 1, if inside { kit::TEXT } else { kit::FAINT });
			}
		}
		if let Err(why) = &view.sound {
			let note = format!("No sound: {why}");
			ui.painter()
				.text(wave.left_center(), Align2::LEFT_CENTER, note, kit::ui(11.0), kit::DIM);
		}
		let pick = Rect::from_min_max(pos2(from_x, thumbs.top() - 5.0), pos2(to_x, wave.bottom() + 5.0));
		let painter = ui.painter();
		painter.rect_stroke(pick, 9, Stroke::new(3.0, kit::CUE), egui::StrokeKind::Inside);
		painter.add(
			egui::Shadow {
				offset: [0, 0],
				blur: 26,
				spread: 0,
				color: kit::CUE.gamma_multiply(0.2),
			}
			.as_shape(pick, egui::CornerRadius::same(9)),
		);
		for (x, left) in [(from_x, true), (to_x, false)] {
			let handle = match left {
				true => Rect::from_min_max(pick.left_top(), pos2(x + 15.0, pick.bottom())),
				false => Rect::from_min_max(pos2(x - 15.0, pick.top()), pick.right_bottom()),
			};
			let corners = match left {
				true => egui::CornerRadius {
					nw: 9,
					sw: 9,
					ne: 0,
					se: 0,
				},
				false => egui::CornerRadius {
					nw: 0,
					sw: 0,
					ne: 9,
					se: 9,
				},
			};
			painter.rect_filled(handle, corners, kit::CUE);
			let grip = Rect::from_center_size(handle.center(), vec2(3.0, 26.0));
			painter.rect_filled(grip, 2, kit::ON_CUE);
		}
		let head = self.head(replay, now);
		let head_x = x_at(now.saturating_duration_since(head));
		painter.vline(
			head_x,
			thumbs.top() - 12.0..=wave.bottom() + 5.0,
			Stroke::new(2.0, Color32::WHITE),
		);
		painter.rect_filled(
			Rect::from_center_size(pos2(head_x, thumbs.top() - 9.0), vec2(14.0, 10.0)),
			egui::CornerRadius {
				nw: 3,
				ne: 3,
				sw: 6,
				se: 6,
			},
			Color32::WHITE,
		);
		// Each marker still in the window: a pennant over the pictures with its
		// number, and a line down through the sound.
		for marker in self.controls.markers.list() {
			let age = now.saturating_duration_since(marker.at);
			if age >= replay::WINDOW {
				continue;
			}
			let x = x_at(age);
			let top = thumbs.top() - 12.0;
			painter.vline(x, top..=wave.bottom(), Stroke::new(1.5, kit::TEXT.gamma_multiply(0.8)));
			painter.add(egui::Shape::convex_polygon(
				vec![pos2(x, top), pos2(x + 9.0, top + 3.5), pos2(x, top + 7.0)],
				kit::TEXT,
				Stroke::NONE,
			));
			painter.text(
				pos2(x + 11.0, top + 3.5),
				Align2::LEFT_CENTER,
				marker.number.to_string(),
				kit::mono(10.0),
				kit::TEXT,
			);
		}
		// Why there are no pictures, over everything.
		let note = match &view.held {
			None => Some("Reading the last 30 seconds".to_string()),
			Some(Err(why)) => Some(capitalize(why)),
			Some(Ok(held)) if !held.video => Some("Sound only".to_string()),
			Some(Ok(_)) => None,
		};
		if let Some(note) = note {
			let galley = painter.layout(note, kit::ui(13.0), kit::TEXT, thumbs.width() - 80.0);
			let back = Rect::from_center_size(thumbs.center(), galley.size() + vec2(24.0, 12.0));
			painter.rect_filled(back, 8, kit::P1.gamma_multiply(0.92));
			painter.galley(back.center() - galley.size() / 2.0, galley, kit::TEXT);
		}

		// How long it is, and when it starts and ends.
		let length = from.saturating_sub(to);
		let picked = painter.layout_no_wrap(
			format!("{:.1} s picked", length.as_secs_f32()),
			kit::mono(11.5),
			kit::DIM,
		);
		painter.galley(
			pos2(
				header.right() - picked.size().x,
				header.center().y - picked.size().y / 2.0,
			),
			picked,
			kit::DIM,
		);
		let say = |age: Duration| match age < Duration::from_millis(250) {
			true => "now".to_string(),
			false => format!("{} s ago", age.as_secs_f32().round()),
		};
		let under = wave.bottom() + 9.0;
		let from_label = painter.layout_no_wrap(say(from), kit::mono(11.0), kit::CUE);
		let to_label = painter.layout_no_wrap(say(to), kit::mono(11.0), kit::CUE);
		let from_at = (from_x)
			.min(to_x - to_label.size().x - from_label.size().x - 8.0)
			.max(thumbs.left());
		let to_at = (to_x - to_label.size().x)
			.max(from_at + from_label.size().x + 8.0)
			.min(thumbs.right() - to_label.size().x);
		let marks = [(from_at, from_label.size().x), (to_at, to_label.size().x)];
		painter.galley(pos2(from_at, under), from_label, kit::CUE);
		painter.galley(pos2(to_at, under), to_label, kit::CUE);
		// The ruler, where the stretch's own labels leave room.
		let ruler = under + 18.0;
		for (age, text) in [(30, "30 s ago"), (20, "20"), (10, "10"), (0, "now")] {
			let galley = painter.layout_no_wrap(text.into(), kit::mono(10.5), kit::FAINT);
			let x = x_at(Duration::from_secs(age));
			let x = match age {
				30 => x,
				0 => x - galley.size().x,
				_ => x - galley.size().x / 2.0,
			};
			let clear = marks
				.iter()
				.all(|(at, width)| x + galley.size().x < at - 6.0 || x > at + width + 6.0);
			if clear {
				painter.galley(pos2(x, ruler), galley, kit::FAINT);
			}
		}

		// A press near a handle moves it, anywhere else moves the head.
		let response = ui.interact(
			track.expand2(vec2(8.0, 12.0)),
			ui.id().with("past strip"),
			Sense::click_and_drag(),
		);
		let pointer = response.interact_pointer_pos().or(response.hover_pos());
		if let Some(pointer) = pointer {
			let over = grip_at(pointer.x, from_x, to_x);
			if over.is_some() || self.review.grip.is_some_and(|grip| grip != Grip::Head) {
				ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
			}
			if response.drag_started() || response.clicked() {
				self.review.grip = Some(over.unwrap_or(Grip::Head));
			}
		}
		if (response.dragged() || response.clicked())
			&& let (Some(grip), Some(pointer)) = (self.review.grip, response.interact_pointer_pos())
		{
			let age = age_at(pointer.x);
			let min = replay::MIN_STRETCH;
			match grip {
				Grip::Head => self.review.head = Some(now.saturating_sub(age)),
				Grip::From => {
					let from = age.max(to + min).min(replay::WINDOW);
					replay.stretch = match replay.stretch {
						replay::Stretch::Last(_) => replay::Stretch::Last(from),
						replay::Stretch::Between(_, end) => replay::Stretch::Between(now.saturating_sub(from), end),
					};
				}
				Grip::To => {
					let to = age.min(from.saturating_sub(min));
					replay.stretch = match pointer.x >= thumbs.right() - PIN {
						true => replay::Stretch::Last(from),
						false => replay::Stretch::Between(now.saturating_sub(from), now.saturating_sub(to)),
					};
				}
			}
		}
		if response.drag_stopped() || response.clicked() {
			self.review.grip = None;
		}
	}

	/// Why the picks cannot replay or save now.
	fn refusals(&self, view: &past::View, replay: &controls::Replay, sound_only: bool) -> Refusals {
		let mut refusals = Refusals {
			replay: None,
			save: None,
		};
		let stretch = replay
			.stretch
			.span(self.review.now())
			.err()
			.map(|err| capitalize(&format!("{err:#}")));
		let held = match &view.held {
			Some(Err(why)) => Some(capitalize(why)),
			_ => None,
		};
		refusals.save = held.clone().or(stretch.clone());
		let state = self.engine.state.lock().unwrap();
		let into = state
			.collection()
			.sources
			.iter()
			.any(|(_, source)| matches!(source, Source::Replay));
		drop(state);
		refusals.replay = held.or(stretch).or_else(|| match () {
			_ if !into => Some("Add a replay source to play replays in".into()),
			_ if sound_only && replay.speed < 1.0 => Some("Sound plays only at 100%".into()),
			_ => None,
		});
		refusals
	}

	/// The speed, Replay, and Save clip, in `rect`, returning what was fired.
	fn actions(&self, ui: &mut Ui, rect: Rect, replay: &mut controls::Replay, refusals: &Refusals) -> Option<Action> {
		let speed = Rect::from_min_size(rect.min, vec2(200.0_f32.min(rect.width() * 0.25), rect.height()));
		caption_text(
			ui.painter(),
			speed.left_top() + vec2(0.0, 6.0),
			"Speed",
			Align2::LEFT_CENTER,
		);
		let segments = Rect::from_min_size(speed.left_top() + vec2(0.0, 18.0), vec2(speed.width(), 30.0));
		let each = segments.width() / replay::SPEEDS.len() as f32;
		ui.painter().rect(
			segments,
			10,
			kit::P2,
			Stroke::new(1.0, kit::LINE),
			egui::StrokeKind::Inside,
		);
		for (index, speed) in replay::SPEEDS.into_iter().enumerate() {
			let cell = Rect::from_min_size(
				segments.min + vec2(index as f32 * each, 0.0),
				vec2(each, segments.height()),
			);
			let label = format!("{:.0}%", speed * 100.0);
			let response = ui.interact(cell, ui.id().with(("speed", index)), Sense::click());
			let on = replay.speed == speed;
			response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, &label));
			if response.clicked() {
				replay.speed = speed;
			}
			let ink = match on {
				true => {
					ui.painter().rect_filled(cell.shrink(1.0), 9, kit::TEXT);
					Color32::from_rgb(0x11, 0x11, 0x11)
				}
				false if response.hovered() => kit::TEXT,
				false => kit::DIM,
			};
			ui.painter()
				.text(cell.center(), Align2::CENTER_CENTER, label, kit::ui_medium(12.5), ink);
		}
		// With more than one replay source, which one plays takes the note's
		// row, and the Replay button's note says what the row said.
		let replays = self.replay_sources(replay);
		match (&mut replay.into, replays.len() > 1) {
			(Some(into), true) => {
				let row = Rect::from_min_size(segments.left_bottom() + vec2(0.0, 2.0), vec2(speed.width(), 20.0));
				ui.scope_builder(UiBuilder::new().max_rect(row), |ui| {
					egui::ComboBox::from_id_salt("replay into")
						.selected_text(format!("Into {into}"))
						.width(speed.width())
						.show_ui(ui, |ui| {
							for name in &replays {
								ui.selectable_value(into, name.clone(), name.as_str());
							}
						});
				});
			}
			_ => {
				ui.painter().text(
					segments.left_bottom() + vec2(0.0, 12.0),
					Align2::LEFT_CENTER,
					"No sound below 100%",
					kit::ui(11.5),
					kit::DIM,
				);
			}
		}

		let buttons = rect.with_min_x(speed.right() + 24.0);
		let gap = 14.0;
		let play_width = (buttons.width() - gap) * 1.25 / 2.25;
		let play_rect = buttons.with_max_x(buttons.left() + play_width);
		let save_rect = buttons.with_min_x(play_rect.right() + gap);
		let (_, length) = replay.stretch.span(self.review.now()).unwrap_or_default();
		let seconds = format!("{} s", length.as_secs_f32().round().max(1.0));
		let into = replay.into.as_ref().map(|into| format!("`{into}`"));
		let mut pace = match replay.speed {
			1.0 => "at full speed".to_string(),
			0.5 => "at half speed".to_string(),
			0.25 => "at quarter speed".to_string(),
			speed => format!("at {:.0}%", speed * 100.0),
		};
		if replays.len() > 1 && replay.speed < 1.0 {
			pace.push_str(" with no sound");
		}
		let play_note = match &refusals.replay {
			Some(why) => why.clone(),
			None => format!(
				"{seconds} {pace}, into {}",
				into.as_deref().unwrap_or("the replay source")
			),
		};
		let save_note = match &refusals.save {
			Some(why) => why.clone(),
			None => format!("{seconds} at full speed, into the folder"),
		};
		let key = self.key(&Action::SaveClip, ui.ctx());
		let play = Big {
			icon: Icon::Play,
			text: "Replay",
			note: &play_note,
			filled: true,
			enabled: refusals.replay.is_none(),
			key: None,
		}
		.show(ui, play_rect);
		let save = Big {
			icon: Icon::Save,
			text: "Save clip",
			note: &save_note,
			filled: false,
			enabled: refusals.save.is_none(),
			key: key.as_deref(),
		}
		.show(ui, save_rect);
		match (play.clicked(), save.clicked()) {
			(true, _) => Some(Action::Replay),
			(_, true) => Some(Action::SaveClip),
			_ => None,
		}
	}

	/// The show's replay sources, keeping what `replay` plays in one of them:
	/// the first, unless it names another.
	fn replay_sources(&self, replay: &mut controls::Replay) -> Vec<String> {
		let state = self.engine.state.lock().unwrap();
		let replays: Vec<String> = state
			.collection()
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Replay))
			.map(|(name, _)| name.clone())
			.collect();
		if replay.into.as_ref().is_none_or(|into| !replays.contains(into)) {
			replay.into = replays.first().cloned();
		}
		replays
	}

	/// This show's recordings and clips as cards, with the folder below.
	fn shelf(&mut self, ui: &mut Ui, rect: Rect) {
		let painter = ui.painter();
		let title = kit::caps_galley(painter, "Recordings", kit::display_bold(22.0), kit::TEXT, None);
		let title_height = title.size().y;
		painter.galley(rect.left_top(), title, kit::TEXT);
		painter.text(
			pos2(rect.right(), rect.top() + title_height / 2.0),
			Align2::RIGHT_CENTER,
			"of this show",
			kit::ui(11.5),
			kit::DIM,
		);
		let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 34.0), rect.max);
		let list = Rect::from_min_max(
			pos2(rect.left(), rect.top() + title_height + 12.0),
			pos2(rect.right(), footer.top() - 10.0),
		);
		let listing = self.review.shelf.cards();
		let last = self.engine.recorder.status();
		// Each file of the recording under way or the last that stopped
		// writing early, and why.
		let failed: Vec<String> = last
			.iter()
			.flat_map(|status| &status.files)
			.filter_map(|file| match &file.finished {
				Some(record::Finished::Failed(why)) => {
					let name = file.path.file_name().unwrap_or_default().to_string_lossy();
					Some(format!("{name} stopped early: {why}"))
				}
				_ => None,
			})
			.collect();
		let status = last.filter(|status| !status.done);
		let show = self.engine.recorder.show().to_string();
		let live = status.as_ref().and_then(|status| {
			let first = status.files.first()?;
			match shelf::parse(&show, &first.path.file_name()?.to_string_lossy())? {
				shelf::File::Recording { stamp, .. } => Some(stamp),
				shelf::File::Clip(_) => None,
			}
		});
		let mut opened = None;
		ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
			egui::ScrollArea::vertical()
				.id_salt("recordings")
				.auto_shrink(false)
				.show(ui, |ui| {
					ui.spacing_mut().item_spacing.y = 8.0;
					for failed in &failed {
						ui.label(egui::RichText::new(failed).color(kit::ERR));
					}
					if let Some(status) = &status {
						self.live_card(ui, status, &show);
					}
					for card in &listing.cards {
						if matches!(&card.kind, shelf::Kind::Recording { stamp, .. } if Some(stamp) == live.as_ref()) {
							continue;
						}
						if self.card(ui, card).clicked() {
							opened = Some(card.path.clone());
						}
					}
					if listing.cards.is_empty() && status.is_none() {
						let why = listing
							.error
							.clone()
							.unwrap_or_else(|| "Nothing recorded for this show yet. Record from the title bar.".into());
						ui.label(egui::RichText::new(why).color(kit::DIM));
					}
				});
		});
		if let Some(path) = opened {
			let result = shelf::open(&path);
			self.check(result);
		}
		let open = ui.put(
			Rect::from_min_size(footer.left_center() - vec2(0.0, 15.0), vec2(150.0, 30.0)),
			kit::button(Icon::Folder, "Open the folder"),
		);
		let open = open.on_hover_text(self.review.shelf.dir().display().to_string());
		if open.clicked() {
			let result = shelf::open(self.review.shelf.dir());
			self.check(result);
		}
		if let Some(free) = listing.free {
			ui.painter().text(
				footer.right_center(),
				Align2::RIGHT_CENTER,
				format!("{} free", bytes(free)),
				kit::ui(11.5),
				kit::DIM,
			);
		}
	}

	/// The recording under way, its picture the live one.
	fn live_card(&self, ui: &mut Ui, status: &record::Status, show: &str) {
		let written: u64 = status.files.iter().map(|file| file.written).sum();
		let sources = status
			.files
			.iter()
			.filter_map(|file| shelf::parse(show, &file.path.file_name()?.to_string_lossy()))
			.filter_map(|file| match file {
				shelf::File::Recording { source, .. } => source,
				shelf::File::Clip(_) => None,
			})
			.collect::<BTreeSet<_>>()
			.len();
		let elapsed = kio::clock::now().saturating_duration_since(status.started);
		let picture = egui::load::SizedTexture::new(
			self.program.id,
			vec2(self.program_drawn.width as f32, self.program_drawn.height as f32),
		);
		let (title, meta) = match status.stopping {
			true => ("Finishing the recording", format!("{} written", bytes(written))),
			false => ("Recording now", format!("{} so far", bytes(written))),
		};
		let chip = (sources > 0).then(|| (None, format!("Program and {sources} sources")));
		let card = Card {
			picture: Some(picture),
			length: &clock(elapsed),
			title,
			meta: &meta,
			chip,
			live: true,
			file: None,
		};
		card.show(ui);
	}

	/// A finished recording's or a clip's card.
	fn card(&self, ui: &mut Ui, card: &shelf::Card) -> egui::Response {
		let size = bytes(card.bytes);
		let ago = card.modified.map(ago);
		let (title, meta, chip) = match &card.kind {
			shelf::Kind::Clip(n) => (
				format!("Clip {n}"),
				match ago {
					Some(ago) => format!("{size}, {ago}"),
					None => size,
				},
				Some((Some(Icon::Clip), "Clip".to_string())),
			),
			shelf::Kind::Recording { sources, repaired, .. } => (
				ago.map(|ago| capitalize(&ago)).unwrap_or_else(|| "Recording".into()),
				size,
				match (repaired, sources) {
					(true, _) => Some((Some(Icon::Repaired), "Repaired after a crash".to_string())),
					(false, 0) => None,
					(false, sources) => Some((None, format!("Program and {sources} sources"))),
				},
			),
		};
		let length = card.length.map(clock).unwrap_or_default();
		let name = card.path.file_name().unwrap_or_default().to_string_lossy();
		let card = Card {
			picture: None,
			length: &length,
			title: &title,
			meta: &meta,
			chip,
			live: false,
			file: Some(&name),
		};
		card.show(ui)
	}
}

/// A recording's or clip's card: a picture with its length over it, the
/// title, a line under it, and a chip.
struct Card<'a> {
	picture: Option<egui::load::SizedTexture>,
	length: &'a str,
	title: &'a str,
	meta: &'a str,
	/// Such as CLIP, with its icon.
	chip: Option<(Option<Icon>, String)>,
	/// Whether it is the recording under way.
	live: bool,
	/// The file a click opens, by name.
	file: Option<&'a str>,
}

impl Card<'_> {
	/// Draw the card, as wide as `ui`.
	fn show(self, ui: &mut Ui) -> egui::Response {
		let Self {
			picture,
			length,
			title,
			meta,
			chip,
			live,
			file,
		} = self;
		let sense = if file.is_some() { Sense::click() } else { Sense::hover() };
		let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 88.0), sense);
		response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
		let response = match file {
			Some(file) => response
				.on_hover_text(format!("Open {file}"))
				.on_hover_cursor(CursorIcon::PointingHand),
			None => response,
		};
		let painter = ui.painter();
		let border = match (live, response.hovered() && file.is_some()) {
			(_, true) => kit::DIM,
			(true, false) => Color32::from_rgb(0x3f, 0x3f, 0x4d),
			(false, false) => kit::LINE,
		};
		painter.rect(rect, 12, kit::P2, Stroke::new(1.0, border), egui::StrokeKind::Inside);
		let thumb = Rect::from_min_size(rect.left_top() + vec2(7.0, 8.0), vec2(128.0, 72.0));
		match picture {
			Some(picture) => {
				egui::Image::new(picture)
					.corner_radius(7)
					.uv(cover(picture.size, thumb.size()))
					.paint_at(ui, thumb);
			}
			// What a finished file shows waits on reading a regular MP4's first
			// picture (quest/m3/card-pictures.md).
			None => {
				ui.painter().rect_filled(thumb, 7, kit::P3);
				Icon::Review.paint(ui, thumb, 20.0, kit::FAINT);
			}
		}
		let painter = ui.painter();
		if !length.is_empty() {
			let galley = painter.layout_no_wrap(length.into(), kit::mono(10.5), Color32::WHITE);
			let at = thumb.right_bottom() - vec2(5.0, 5.0) - galley.size() - vec2(5.0, 3.0);
			let back = Rect::from_min_size(at, galley.size() + vec2(10.0, 6.0));
			painter.rect_filled(back, 4, Color32::from_black_alpha(204));
			painter.galley(at + vec2(5.0, 3.0), galley, Color32::WHITE);
		}
		let x = thumb.right() + 12.0;
		let width = rect.right() - 8.0 - x;
		let mut y = rect.top() + 18.0;
		if live {
			painter.circle_filled(pos2(x + 4.0, y + 7.0), 4.0, kit::ACCENT);
		}
		let title_x = if live { x + 14.0 } else { x };
		let title = painter.layout(title.into(), kit::ui_strong(13.5), kit::TEXT, width);
		let title_height = title.size().y;
		painter.galley(pos2(title_x, y), title, kit::TEXT);
		y += title_height + 6.0;
		let meta = painter.layout(meta.into(), kit::ui(11.5), kit::DIM, width);
		let meta_height = meta.size().y;
		painter.galley(pos2(x, y), meta, kit::DIM);
		y += meta_height + 6.0;
		if let Some((icon, text)) = chip {
			let outlined = icon == Some(Icon::Repaired);
			let galley = kit::caps_galley(painter, &text, kit::display_bold(11.0), kit::TEXT, Some(width - 20.0));
			let icon_width = if icon.is_some() { 17.0 } else { 0.0 };
			let chip = Rect::from_min_size(pos2(x, y), vec2(galley.size().x + 14.0 + icon_width, 19.0));
			match outlined {
				true => painter.rect_stroke(chip, 4, Stroke::new(1.0, kit::DIM), egui::StrokeKind::Inside),
				false => painter.rect_filled(chip, 4, kit::P3),
			};
			if let Some(icon) = icon {
				icon.paint(
					ui,
					Rect::from_center_size(pos2(chip.left() + 7.0 + 6.0, chip.center().y), Vec2::splat(12.0)),
					12.0,
					kit::TEXT,
				);
			}
			ui.painter().galley(
				pos2(chip.left() + 7.0 + icon_width, chip.center().y - galley.size().y / 2.0),
				galley,
				kit::TEXT,
			);
		}
		response
	}
}

/// One of the page's two large buttons: Replay, filled, and Save clip.
struct Big<'a> {
	icon: Icon,
	text: &'a str,
	/// What it does, or why it cannot.
	note: &'a str,
	filled: bool,
	enabled: bool,
	/// The key bound to it.
	key: Option<&'a str>,
}

impl Big<'_> {
	/// Draw the button filling `rect`.
	fn show(self, ui: &mut Ui, rect: Rect) -> egui::Response {
		let Self {
			icon,
			text,
			note,
			filled,
			enabled,
			key,
		} = self;
		let sense = if enabled { Sense::click() } else { Sense::hover() };
		let response = ui.interact(rect, ui.id().with(text), sense);
		response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
		let hovered = enabled && response.hovered();
		let (fill, ink, dim, stroke) = match (filled, enabled) {
			(true, true) => (
				if hovered { Color32::WHITE } else { kit::TEXT },
				Color32::from_rgb(0x11, 0x11, 0x11),
				Color32::from_rgba_unmultiplied(0x11, 0x11, 0x11, 0xbb),
				Color32::TRANSPARENT,
			),
			(false, true) => (
				if hovered { kit::P3 } else { kit::P2 },
				kit::TEXT,
				kit::DIM,
				Color32::from_rgb(0x3f, 0x3f, 0x4d),
			),
			(_, false) => (kit::P1, kit::FAINT, kit::DIM, kit::LINE),
		};
		let painter = ui.painter();
		if filled && enabled {
			painter.add(
				egui::Shadow {
					offset: [0, 10],
					blur: 30,
					spread: 0,
					color: Color32::from_black_alpha(136),
				}
				.as_shape(rect, egui::CornerRadius::same(13)),
			);
		}
		painter.rect(rect, 13, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
		let icon_rect = Rect::from_center_size(pos2(rect.left() + 22.0 + 13.0, rect.center().y), Vec2::splat(26.0));
		icon.paint(ui, icon_rect, 26.0, ink);
		let x = icon_rect.right() + 16.0;
		let key_width = key.map_or(0.0, |_| 90.0);
		let width = (rect.right() - 22.0 - key_width - x).max(10.0);
		let painter = ui.painter();
		let word = kit::caps_galley(painter, text, kit::display_bold(28.0), ink, Some(width));
		// A refusal can take two lines.
		let mut job = egui::text::LayoutJob::simple(note.into(), kit::ui(12.0), dim, width);
		job.wrap.max_rows = 2;
		let note = painter.layout_job(job);
		let height = word.size().y + 4.0 + note.size().y;
		let top = rect.center().y - height / 2.0;
		let word_height = word.size().y;
		painter.galley(pos2(x, top), word, ink);
		painter.galley(pos2(x, top + word_height + 4.0), note, dim);
		if let Some(key) = key {
			let galley = painter.layout_no_wrap(key.into(), kit::mono(11.0), ink);
			let back = Rect::from_min_size(
				pos2(rect.right() - 22.0 - galley.size().x - 12.0, rect.center().y - 11.0),
				vec2(galley.size().x + 12.0, 22.0),
			);
			painter.rect_stroke(back, 4, Stroke::new(1.0, kit::FAINT), egui::StrokeKind::Inside);
			painter.galley(back.center() - galley.size() / 2.0, galley, ink);
		}
		response
	}
}

/// How long ago the stretch starts and ends, as of `now`.
fn ages(stretch: replay::Stretch, now: Instant) -> (Duration, Duration) {
	match stretch {
		replay::Stretch::Last(length) => (length.min(replay::WINDOW), Duration::ZERO),
		replay::Stretch::Between(from, to) => (
			now.saturating_duration_since(from).min(replay::WINDOW),
			now.saturating_duration_since(to).min(replay::WINDOW),
		),
	}
}

/// Which handle a press at `x` takes, given where the stretch's start and end
/// are drawn. Where both are in reach, as when a stretch has aged out and both
/// sit on the strip's oldest edge, the one on the press's side, so the end can
/// always be dragged free.
fn grip_at(x: f32, from_x: f32, to_x: f32) -> Option<Grip> {
	let near = |at: f32| (x - at).abs() <= GRAB;
	match (near(from_x), near(to_x)) {
		(true, true) if x < (from_x + to_x) / 2.0 => Some(Grip::From),
		(true, true) | (false, true) => Some(Grip::To),
		(true, false) => Some(Grip::From),
		(false, false) => None,
	}
}

/// The part of a `texture` that covers a rect of `room`'s shape, centered.
fn cover(texture: Vec2, room: Vec2) -> Rect {
	let (texture, room) = (texture.x / texture.y.max(1.0), room.x / room.y.max(1.0));
	match texture > room {
		true => {
			let width = room / texture;
			Rect::from_min_max(pos2((1.0 - width) / 2.0, 0.0), pos2((1.0 + width) / 2.0, 1.0))
		}
		false => {
			let height = texture / room;
			Rect::from_min_max(pos2(0.0, (1.0 - height) / 2.0), pos2(1.0, (1.0 + height) / 2.0))
		}
	}
}

/// A small tag, such as ON AIR, its top left at `at`.
struct Tag {
	fill: Color32,
	ink: Color32,
	size: f32,
}

impl Tag {
	/// The program's, in the accent.
	const AIR: Self = Self {
		fill: kit::ACCENT,
		ink: Color32::WHITE,
		size: 11.0,
	};
	/// How long ago the large picture was, in amber.
	const AGO: Self = Self {
		fill: kit::CUE,
		ink: kit::ON_CUE,
		size: 17.0,
	};

	/// Draw `text` in it, its top left at `at`.
	fn paint(&self, painter: &egui::Painter, at: egui::Pos2, text: &str) {
		let galley = kit::caps_galley(painter, text, kit::display_bold(self.size), self.ink, None);
		let back = Rect::from_min_size(at, galley.size() + vec2(self.size * 0.8, self.size * 0.35));
		painter.rect_filled(back, 3, self.fill);
		painter.galley(back.center() - galley.size() / 2.0, galley, self.ink);
	}
}

/// A section's caption, small and spaced.
fn caption_text(painter: &egui::Painter, at: egui::Pos2, text: &str, align: Align2) {
	let galley = kit::caps_galley(painter, text, kit::display(11.5), kit::DIM, None);
	let rect = align.anchor_size(at, galley.size());
	painter.galley(rect.min, galley, kit::DIM);
}

/// `text` with a capital first letter, for an error shown as a sentence.
fn capitalize(text: &str) -> String {
	let mut chars = text.chars();
	match chars.next() {
		Some(first) => first.to_uppercase().chain(chars).collect(),
		None => String::new(),
	}
}

/// A size on disk: "11 MB", "6.1 GB".
fn bytes(bytes: u64) -> String {
	match bytes {
		0..1_000_000_000 => format!("{} MB", bytes.div_ceil(1_000_000)),
		_ => format!("{:.1} GB", bytes as f64 / 1e9),
	}
}

/// How long ago `then` was, as a phrase: "2 minutes ago".
fn ago(then: kio::clock::Wall) -> String {
	let ago = kio::clock::wall().saturating_duration_since(then).as_secs();
	let plural = |n: u64, unit: &str| match n {
		1 => format!("1 {unit} ago"),
		n => format!("{n} {unit}s ago"),
	};
	match ago {
		0..60 => "just now".into(),
		60..3600 => plural(ago / 60, "minute"),
		3600..86_400 => plural(ago / 3600, "hour"),
		86_400..172_800 => "yesterday".into(),
		_ => plural(ago / 86_400, "day"),
	}
}

/// How long ago a marker was dropped: "now", "12 s ago", "4 min ago",
/// "1 h 5 min ago".
fn since(ago: Duration) -> String {
	let seconds = ago.as_secs();
	match seconds {
		0 => "now".into(),
		1..60 => format!("{seconds} s ago"),
		60..3600 => format!("{} min ago", seconds / 60),
		_ => format!("{} h {} min ago", seconds / 3600, seconds / 60 % 60),
	}
}

/// A length: "0:08", "52:10", "2:10:44".
pub(super) fn clock(length: Duration) -> String {
	let seconds = length.as_secs();
	match seconds {
		0..3600 => format!("{}:{:02}", seconds / 60, seconds % 60),
		_ => format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sizes_and_lengths_read_as_the_mock_up_has_them() {
		assert_eq!(bytes(11_000_000), "11 MB");
		assert_eq!(bytes(6_100_000_000), "6.1 GB");
		assert_eq!(clock(Duration::from_secs(8)), "0:08");
		assert_eq!(clock(Duration::from_secs(52 * 60 + 10)), "52:10");
		assert_eq!(clock(Duration::from_secs(2 * 3600 + 10 * 60 + 44)), "2:10:44");
		assert_eq!(since(Duration::from_millis(900)), "now");
		assert_eq!(since(Duration::from_secs(59)), "59 s ago");
		assert_eq!(since(Duration::from_secs(4 * 60 + 59)), "4 min ago");
		assert_eq!(since(Duration::from_secs(3600 + 5 * 60)), "1 h 5 min ago");
	}

	/// Each handle is taken near it, and where both are in reach, the one on
	/// the press's side: an aged-out stretch's end, drawn on its start at the
	/// strip's oldest edge, is still taken by a press just right of it.
	#[test]
	fn a_press_takes_the_handle_on_its_side() {
		assert_eq!(grip_at(100.0, 100.0, 300.0), Some(Grip::From));
		assert_eq!(grip_at(305.0, 100.0, 300.0), Some(Grip::To));
		assert_eq!(grip_at(200.0, 100.0, 300.0), None);
		assert_eq!(grip_at(24.0, 20.0, 20.0), Some(Grip::To));
		assert_eq!(grip_at(16.0, 20.0, 20.0), Some(Grip::From));
		assert_eq!(grip_at(98.0, 100.0, 106.0), Some(Grip::From));
		assert_eq!(grip_at(104.0, 100.0, 106.0), Some(Grip::To));
	}

	/// A wide picture in a narrow room is cut at its sides, a tall one at its
	/// top and bottom, and one of the room's shape is whole.
	#[test]
	fn a_picture_covers_its_room() {
		let wide = cover(vec2(160.0, 90.0), vec2(90.0, 90.0));
		assert!((wide.width() - 90.0 / 160.0).abs() < 1e-6 && wide.height() == 1.0);
		let tall = cover(vec2(90.0, 160.0), vec2(90.0, 90.0));
		assert!((tall.height() - 90.0 / 160.0).abs() < 1e-6 && tall.width() == 1.0);
		assert_eq!(
			cover(vec2(16.0, 9.0), vec2(160.0, 90.0)),
			Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
		);
	}
}
