//! Staging a scene that is not on air: the NEXT card Studio mode cues into,
//! with TAKE and AUTO, and the banner and small live picture over a scene open
//! for editing off air. Both show the compositor's preview.

use egui::{Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::*;

/// The scene cued, or open off air, as a frame shows it.
pub(super) struct Cued {
	pub(super) name: String,
	/// Why it cannot go on air, if it cannot.
	pub(super) refused: Option<String>,
	/// Its size, which its picture is drawn at.
	pub(super) canvas: Canvas,
	/// Its portrait picture's size, while it goes out in both.
	pub(super) portrait: Option<Canvas>,
	/// Whether it is open for editing off air, rather than only cued.
	pub(super) off_air: bool,
	/// Why viewers see it anyway, if they do, such as being published.
	pub(super) live: Option<String>,
	/// The show's transition, which AUTO takes it by, in words.
	pub(super) transition: String,
}

impl Cued {
	/// Its picture's shape, width over height.
	pub(super) fn shape(&self) -> f32 {
		super::shell::shape(self.canvas)
	}

	/// The shape of its pictures side by side, while it goes out in both,
	/// with room between them, or else of its picture.
	fn card_shape(&self) -> f32 {
		match self.portrait {
			Some(portrait) => super::layout::pair_shape([self.shape(), super::shell::shape(portrait)], CARD_GAP),
			None => self.shape(),
		}
	}
}

/// The gap between the NEXT card's pictures, as a share of their height.
const CARD_GAP: f32 = 0.06;

/// A corner of the live picture, where the NEXT card docks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Corner {
	TopRight,
	BottomLeft,
}

impl Corner {
	/// Where the NEXT card docks while the selected layer is centered at
	/// `selected`, as a fraction of the picture across and down: top right,
	/// or the opposite corner while the layer is in that one, so it never
	/// hides the layer or its toolbar.
	pub(super) fn away_from(selected: Option<[f32; 2]>) -> Self {
		match selected {
			Some([x, y]) if x >= 0.5 && y < 0.5 => Self::BottomLeft,
			_ => Self::TopRight,
		}
	}
}

/// What the NEXT card or the off-air banner asked for.
enum Ask {
	Take,
	Auto,
	/// Open the cued scene off air on BUILD.
	Edit,
	/// Leave nothing cued.
	Clear,
	/// Stop editing off air, leaving the scene cued in Studio mode.
	BackToLive,
}

/// The least room the off-air banner's words get beside the live picture
/// before it moves under it.
const MIN_TEXT: f32 = 160.0;
/// Room inside the card around what it holds.
const PADDING: f32 = 10.0;
/// How far in from the picture's edges the card docks.
const INSET: f32 = 12.0;

impl Panels {
	/// The NEXT card docked in `corner` of `picture`: the cued scene's own
	/// picture, with TAKE, AUTO, Edit, and a close that clears the cue.
	pub(super) fn next(&mut self, ui: &mut Ui, fx: &mut Fx, cued: &Cued, picture: Rect, corner: Corner) {
		let width = (picture.width() * 0.26).clamp(200.0, 300.0);
		let inner = width - 2.0 * PADDING;
		// The rest of the card is about 120 points tall; the picture gets what
		// the live picture leaves.
		let tall = (inner / cued.card_shape())
			.min(picture.height() - 2.0 * INSET - 120.0)
			.max(24.0);
		let (header, buttons, hint) = (24.0, 40.0, 14.0);
		let height = PADDING + header + 6.0 + tall + 8.0 + buttons + 6.0 + hint + PADDING;
		let at = match corner {
			Corner::TopRight => pos2(picture.right() - INSET - width, picture.top() + INSET),
			Corner::BottomLeft => pos2(picture.left() + INSET, picture.bottom() - INSET - height),
		};
		let card = Rect::from_min_size(at, vec2(width, height));
		// Under the card's controls, so a click on it never reaches the picture.
		ui.interact(card, ui.id().with("next card"), Sense::click_and_drag());
		let painter = ui.painter();
		let shadow = egui::Shadow {
			offset: [0, 8],
			blur: 24,
			spread: 0,
			color: Color32::from_black_alpha(0x90),
		};
		painter.add(shadow.as_shape(card, egui::CornerRadius::same(12)));
		painter.rect(
			card,
			12,
			kit::POPOVER,
			Stroke::new(1.0, kit::LINE),
			egui::StrokeKind::Inside,
		);

		let mut ask = None;
		let mut y = card.top() + PADDING;
		let row = Rect::from_min_size(pos2(card.left() + PADDING, y), vec2(inner, header));
		let close = Rect::from_center_size(pos2(row.right() - 11.0, row.center().y), Vec2::splat(22.0));
		if ui
			.put(close, IconButton::new(Icon::Close, "Clear the cue").small())
			.clicked()
		{
			ask = Some(Ask::Clear);
		}
		let edit = Rect::from_min_max(
			pos2(close.left() - 52.0, row.top() + 1.0),
			pos2(close.left() - 6.0, row.bottom() - 1.0),
		);
		let edit = ui
			.put(edit, egui::Button::new(egui::RichText::new("Edit").font(kit::ui(12.0))))
			.on_hover_text("Open it on Build to edit it off air");
		if edit.clicked() {
			ask = Some(Ask::Edit);
		}
		let painter = ui.painter();
		let next = kit::caps_galley(painter, "Next", kit::display_bold(11.0), kit::CUE, None);
		painter.galley(
			pos2(row.left(), row.center().y - next.size().y / 2.0),
			next.clone(),
			kit::CUE,
		);
		let left = row.left() + next.size().x + 8.0;
		let room = (edit.rect.left() - 8.0 - left).max(0.0);
		let name = kit::caps_galley(painter, &cued.name, kit::display_bold(15.0), kit::TEXT, Some(room));
		painter.galley(pos2(left, row.center().y - name.size().y / 2.0), name, kit::TEXT);
		y = row.bottom() + 6.0;

		// The cued scene as it will go on air, framed in amber.
		let frame = Rect::from_min_size(pos2(card.left() + PADDING, y), vec2(inner, tall));
		painter.rect_filled(frame, 8, kit::BG);
		// Both of its pictures, side by side, while it goes out in both.
		let room = frame.shrink(3.0);
		let pictures = match (cued.portrait, &self.portrait_preview) {
			(Some(portrait), Some(registered)) => {
				let shapes = [cued.shape(), super::shell::shape(portrait)];
				let gap = CARD_GAP * room.height();
				let [wide, tall] = super::layout::pair(room, shapes, gap);
				vec![(self.preview.id, wide), (registered.id, tall)]
			}
			_ => vec![(self.preview.id, super::layout::fit(room, cued.shape()))],
		};
		for (id, at) in pictures {
			egui::Image::new((id, at.size())).corner_radius(6).paint_at(ui, at);
		}
		ui.painter()
			.rect_stroke(frame, 8, Stroke::new(2.0, kit::CUE), egui::StrokeKind::Inside);
		y = frame.bottom() + 8.0;

		let enabled = cued.refused.is_none();
		let why = cued.refused.as_deref();
		let half = (inner - 8.0) / 2.0;
		let take_at = Rect::from_min_size(pos2(card.left() + PADDING, y), vec2(half, buttons));
		let auto_at = Rect::from_min_size(pos2(take_at.right() + 8.0, y), vec2(half, buttons));
		let take_key = self.key(&Action::Cut, ui.ctx());
		let auto_key = self.key(&Action::Transition, ui.ctx());
		let take = Big {
			text: "Take",
			note: why.unwrap_or("Put it on air at once"),
			key: take_key.as_deref(),
			enabled,
			metal: true,
		};
		if big_button(ui, fx, take_at, take).clicked() {
			ask = Some(Ask::Take);
		}
		let auto_note = format!("Put it on air by the show's transition: {}", cued.transition);
		let auto = Big {
			text: "Auto",
			note: why.unwrap_or(&auto_note),
			key: auto_key.as_deref(),
			enabled,
			metal: false,
		};
		if big_button(ui, fx, auto_at, auto).clicked() {
			ask = Some(Ask::Auto);
		}
		y = take_at.bottom() + 6.0;

		let mut words: Vec<String> = Vec::new();
		words.extend(take_key.map(|key| format!("{key} take")));
		words.extend(auto_key.map(|key| format!("{key} auto")));
		words.push(match why {
			Some(_) => "cannot go on air".into(),
			None => format!("auto: {}", cued.transition),
		});
		let hint = ui.painter().layout(words.join(" · "), kit::ui(11.0), kit::DIM, inner);
		let at = pos2(card.center().x - hint.size().x / 2.0, y);
		ui.painter().galley(at, hint, kit::DIM);

		if let Some(ask) = ask {
			self.ask(ask, &cued.name);
		}
	}

	/// Over the picture of a scene open off air, in `room`'s top: the banner
	/// saying so, with Back to live and Take it live, and the live picture
	/// small at its right. Where the room left for the picture starts.
	pub(super) fn off_air_header(&mut self, ui: &mut Ui, cued: &Cued, room: Rect, shape: f32) -> f32 {
		// No taller than a fifth of the room, so a portrait show's picture
		// leaves the scene open off air most of it.
		let width = (room.width() * 0.24).clamp(140.0, 240.0);
		let size = vec2(width, width / shape.max(0.1));
		let size = size * (room.height() * 0.2 / size.y).min(1.0);
		let live = Rect::from_min_size(pos2(room.right() - size.x, room.top()), size);
		self.on_air(ui, None, live);

		let painter = ui.painter();
		let font = kit::display_bold(14.0);
		let back = kit::caps_galley(painter, "Back to live", kit::ui_strong(12.5), kit::CUE, None);
		let take = kit::caps_galley(painter, "Take it live", kit::ui_strong(12.5), Color32::WHITE, None);
		let buttons = back.size().x + take.size().x + 2.0 * 20.0 + 8.0;
		// One row beside the live picture where it fits; or else under it, the
		// words over the buttons, as in a narrow window.
		let beside = Rect::from_min_max(room.left_top(), pos2(live.left() - 24.0, live.bottom()));
		let one_row = beside.width() >= 12.0 + MIN_TEXT + 12.0 + buttons + 6.0;
		let area = match one_row {
			true => beside,
			false => Rect::from_min_size(pos2(room.left(), live.bottom() + 18.0), vec2(room.width(), 68.0)),
		};
		let room_for_text = match one_row {
			true => area.width() - buttons - 12.0 - 12.0 - 6.0,
			false => area.width() - 24.0,
		};
		let text = match cued.live {
			None => format!("Editing {}, not on air", cued.name),
			Some(_) => format!("Editing {}, not on program", cued.name),
		};
		let text = kit::caps_galley(painter, &text, font, kit::ON_CUE, Some(room_for_text.max(40.0)));
		let width = match one_row {
			true => 12.0 + text.size().x + 12.0 + buttons + 6.0,
			false => (12.0 + text.size().x + 12.0).max(buttons + 12.0),
		};
		let height = if one_row { 38.0 } else { 68.0 };
		let banner = Rect::from_center_size(area.center(), vec2(width.min(area.width()), height));
		painter.rect_filled(banner, 8, kit::CUE);
		// An edit here reaches viewers anyway, so say how.
		let mut bottom = banner.bottom().max(live.bottom());
		if let Some(why) = &cued.live {
			let why = painter.layout(
				format!("Viewers still see it: {why}."),
				kit::ui(12.0),
				kit::CUE,
				area.width(),
			);
			let at = pos2(banner.center().x - why.size().x / 2.0, banner.bottom() + 6.0);
			bottom = bottom.max(at.y + why.size().y);
			painter.galley(at, why, kit::CUE);
		}
		let (text_at, buttons_right, buttons_top) = match one_row {
			true => (
				pos2(banner.left() + 12.0, banner.center().y - text.size().y / 2.0),
				banner.right() - 6.0,
				banner.top() + 6.0,
			),
			false => (
				pos2(
					banner.center().x - text.size().x / 2.0,
					banner.top() + 15.0 - text.size().y / 2.0,
				),
				banner.center().x + buttons / 2.0,
				banner.top() + 32.0,
			),
		};
		painter.galley(text_at, text, kit::ON_CUE);
		let mut ask = None;
		let take_at = Rect::from_min_size(
			pos2(buttons_right - take.size().x - 20.0, buttons_top),
			vec2(take.size().x + 20.0, 26.0),
		);
		let back_at = Rect::from_min_size(
			pos2(take_at.left() - 8.0 - back.size().x - 20.0, take_at.top()),
			vec2(back.size().x + 20.0, take_at.height()),
		);
		let enabled = cued.refused.is_none();
		let (fill, ink) = match enabled {
			true => (kit::ACCENT, Color32::WHITE),
			false => (kit::ACCENT.gamma_multiply(0.4), Color32::from_white_alpha(0x99)),
		};
		for (at, galley, label, fill, ink) in [
			(back_at, back, "Back to live", kit::ON_CUE, kit::CUE),
			(take_at, take, "Take it live", fill, ink),
		] {
			let live = label == "Take it live";
			let sense = if live && !enabled {
				Sense::hover()
			} else {
				Sense::click()
			};
			let response = ui.interact(at, ui.id().with(("off air", label)), sense);
			response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !live || enabled, label));
			let note = match (live, cued.refused.as_deref()) {
				(true, Some(why)) => why.to_string(),
				(true, None) => format!("Put it on air by the show's transition: {}", cued.transition),
				(false, _) => "Stop editing it and edit the scene on air".to_string(),
			};
			let response = kit::tip(response, &note, None);
			let fill = if response.hovered() {
				fill.gamma_multiply(0.85)
			} else {
				fill
			};
			ui.painter().rect_filled(at, 6, fill);
			ui.painter().galley(at.center() - galley.size() / 2.0, galley, ink);
			if response.clicked() {
				ask = Some(if live { Ask::Auto } else { Ask::BackToLive });
			}
		}
		if let Some(ask) = ask {
			self.ask(ask, &cued.name);
		}
		bottom + 22.0
	}

	/// Studio mode's switch at `at`, lit while it is `on`.
	pub(super) fn studio_switch(&mut self, ui: &mut Ui, on: bool, at: Rect) {
		let key = self.key(&Action::StudioMode, ui.ctx());
		let note = match on {
			true => "On: a scene's pad or key cues it, and TAKE or AUTO puts it on air",
			false => "Off: a scene's pad or key takes it. Turn on to cue a scene, then TAKE or AUTO it.",
		};
		let button = IconButton::new(Icon::StudioMode, "Studio mode")
			.lit(on, kit::CUE)
			.look(kit::Look::Boxed)
			.key(key.as_deref())
			.note(Some(note));
		if ui.put(at, button).clicked() {
			self.controls.press(&Action::StudioMode);
		}
	}

	/// Do what the NEXT card or the off-air banner asked for, about the scene
	/// called `scene`.
	fn ask(&mut self, ask: Ask, scene: &str) {
		match ask {
			Ask::Take => self.controls.press(&Action::Cut),
			Ask::Auto => self.controls.press(&Action::Transition),
			Ask::Edit => {
				let result = self.open_off_air(scene);
				self.check(result);
			}
			Ask::Clear => {
				self.editing = None;
				self.engine.state.lock().unwrap().uncue();
			}
			Ask::BackToLive => {
				self.editing = None;
				// Studio mode keeps it cued for TAKE; otherwise it is done with.
				if !self.controls.studio() {
					self.engine.state.lock().unwrap().uncue();
				}
			}
		}
	}
}

/// TAKE or AUTO.
struct Big<'a> {
	text: &'a str,
	/// What it does, or why it cannot, under the pointer with its key.
	note: &'a str,
	key: Option<&'a str>,
	enabled: bool,
	/// Polished metal, as TAKE is, rather than plain.
	metal: bool,
}

/// `big` at `rect`.
fn big_button(ui: &mut Ui, fx: &mut Fx, rect: Rect, big: Big<'_>) -> egui::Response {
	let Big {
		text,
		note,
		key,
		enabled,
		metal,
	} = big;
	let sense = if enabled { Sense::click() } else { Sense::hover() };
	let response = ui.interact(rect, ui.id().with(("big button", text)), sense);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text.to_uppercase()));
	let response = kit::tip(response, &format!("{text}: {note}"), key);
	let mark = (metal && enabled).then(|| Mark::new(ui.painter()));
	let painter = ui.painter();
	let ink = match (metal, enabled) {
		(_, false) => {
			painter.rect(rect, 8, kit::P2, Stroke::new(1.0, kit::LINE), egui::StrokeKind::Inside);
			kit::FAINT
		}
		(true, true) => Fx::INK,
		(false, true) => {
			let fill = if response.hovered() {
				kit::P3.gamma_multiply(1.3)
			} else {
				kit::P3
			};
			painter.rect(rect, 8, fill, Stroke::new(1.0, kit::LINE), egui::StrokeKind::Inside);
			kit::TEXT
		}
	};
	let galley = kit::caps_galley(painter, text, kit::display_bold(17.0), ink, None);
	painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
	if let Some(mark) = mark {
		fx.button(&mark.around(rect));
	}
	response
}

/// `transition` in a few words, such as "fade, 300 ms".
pub(super) fn transition_words(transition: &Transition) -> String {
	let name = transition_name(transition);
	match transition.duration() {
		Some(duration) if !matches!(transition, Transition::Stinger { .. }) => {
			format!("{name}, {} ms", duration.as_millis())
		}
		_ => name.to_string(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The card stays top right unless the selected layer is there, when it
	/// moves to the opposite corner.
	#[test]
	fn the_card_docks_away_from_the_selected_layer() {
		assert_eq!(Corner::away_from(None), Corner::TopRight);
		assert_eq!(Corner::away_from(Some([0.9, 0.1])), Corner::BottomLeft);
		assert_eq!(
			Corner::away_from(Some([0.5, 0.0])),
			Corner::BottomLeft,
			"the middle counts as right"
		);
		assert_eq!(Corner::away_from(Some([0.1, 0.1])), Corner::TopRight);
		assert_eq!(
			Corner::away_from(Some([0.9, 0.5])),
			Corner::TopRight,
			"the middle counts as bottom"
		);
		assert_eq!(Corner::away_from(Some([0.1, 0.9])), Corner::TopRight);
	}

	#[test]
	fn a_transition_in_words() {
		let fade = Transition::Fade {
			duration: Duration::from_millis(300),
		};
		assert_eq!(transition_words(&fade), "fade, 300 ms");
	}
}
