//! A show going out in landscape and portrait at once: what is said under
//! its two pictures, where a layer is shown, and which picture BUILD edits.

use egui::{Align2, Sense, Stroke, StrokeKind, pos2, vec2};
use moq_net::bandwidth::Rate;

use super::*;

/// Room between the two pictures on LIVE and BUILD.
pub(super) const GAP: f32 = 32.0;
/// Room under them for what is said about them.
pub(super) const UNDER: f32 = 48.0;

/// Where a layer may be shown, as the chip and the inspector offer it:
/// `None` for both pictures.
pub(super) fn shown_in_options() -> [(Option<Format>, &'static str); 3] {
	[
		(None, "Both"),
		(Some(Format::Landscape), "Landscape"),
		(Some(Format::Portrait), "Portrait"),
	]
}

/// `item` shown only in `format`, or in both for `None`. Shown only in
/// landscape, it keeps no place in portrait.
pub(super) fn shown(item: &Item, format: Option<Format>) -> Item {
	let portrait = match format {
		Some(Format::Landscape) => None,
		_ => item.portrait,
	};
	Item {
		format,
		portrait,
		..item.clone()
	}
}

/// The edit's label for showing `name` as [`shown`] does.
pub(super) fn shown_label(name: &str, format: Option<Format>) -> String {
	match format {
		None => format!("Show `{name}` in both pictures"),
		Some(format) => format!("Show `{name}` only in {}", format.name().to_lowercase()),
	}
}

/// Under a picture, in `rect`: its format, its size, and what its video sent
/// over the last interval, while on air, as many of those as fit, right to
/// left from the rate, with all of them on hover where some did not. The part
/// of `rect` left free between them.
pub(super) fn format_label(
	ui: &Ui,
	rect: egui::Rect,
	format: Format,
	canvas: Canvas,
	sent: Option<Rate>,
) -> egui::Rect {
	let painter = ui.painter();
	let mid = rect.center().y;
	mark(painter, pos2(rect.left() + 7.0, mid), format, kit::DIM);
	let dims = format!("{} × {}", canvas.width, canvas.height);
	let rate = sent.map(crate::health::rate);
	let name = kit::caps_galley(painter, format.name(), kit::display_bold(11.5), kit::TEXT, None);
	let size = painter.layout_no_wrap(dims.clone(), kit::ui(12.0), kit::DIM);
	let said = rate
		.clone()
		.map(|rate| painter.layout_no_wrap(rate, kit::ui(12.0), kit::DIM));
	// Past the mark, and the rate with its dot.
	let x = rect.left() + 20.0;
	let mut room = rect.right() - x;
	let said = said.filter(|said| said.size().x + 24.0 <= room);
	if let Some(said) = &said {
		room -= said.size().x + 24.0;
	}
	let named = name.size().x <= room;
	let sized = named && name.size().x + 10.0 + size.size().x <= room;
	let mut free = rect.with_min_x(x);
	if named {
		let width = name.size().x;
		painter.galley(pos2(x, mid - name.size().y / 2.0), name, kit::TEXT);
		free.min.x = x + width + 16.0;
	}
	if sized {
		let at = pos2(free.min.x - 6.0, mid - size.size().y / 2.0);
		free.min.x = at.x + size.size().x + 16.0;
		painter.galley(at, size, kit::DIM);
	}
	free.max.x = rect.right();
	let all = said.is_some() == rate.is_some();
	if let Some(said) = said {
		let at = pos2(rect.right() - said.size().x, mid - said.size().y / 2.0);
		painter.circle_filled(pos2(at.x - 9.0, mid), 3.5, kit::OK);
		free.max.x = at.x - 24.0;
		painter.galley(at, said, kit::DIM);
	}
	if !(named && sized && all) {
		let words = [Some(format.name().to_string()), Some(dims), rate];
		let words: Vec<String> = words.into_iter().flatten().collect();
		let id = ui.id().with(("format label", format));
		ui.interact(rect, id, Sense::hover()).on_hover_text(words.join(" · "));
	}
	free.max.x = free.max.x.max(free.min.x);
	free
}

/// A small outline of `format`'s shape, centered at `at`, in `ink`.
pub(super) fn mark(painter: &egui::Painter, at: egui::Pos2, format: Format, ink: Color32) {
	let size = match format {
		Format::Landscape => vec2(13.0, 9.0),
		Format::Portrait => vec2(8.0, 12.0),
	};
	let rect = egui::Rect::from_center_size(at, size);
	painter.rect_stroke(rect, 2, Stroke::new(1.3, ink), StrokeKind::Inside);
}

impl Panels {
	/// In `rect`, under the pictures, centered on `center` as near as it fits,
	/// or at its right for `None`: where the selected layer is shown, both
	/// pictures, landscape, or portrait, for a layer drawn in both. Picking
	/// another shows it there. Without its words where `rect` is too narrow
	/// for them, and not at all where it is too narrow for the choices, which
	/// the inspector offers too.
	pub(super) fn shown_in(&mut self, ui: &mut Ui, rect: egui::Rect, center: Option<f32>) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let Some(picked) = self.selection.clone() else { return };
		let collection = state.collection();
		if !collection.dual(&picked.layer) {
			return;
		}
		let Some(item) = picked.item(collection).cloned() else {
			return;
		};
		let options = shown_in_options();
		let font = kit::ui_strong(12.5);
		let words = format!("{} is shown in", item.source);
		let words = ui.painter().layout_no_wrap(words, kit::ui(12.0), kit::DIM);
		let choices: f32 = options
			.iter()
			.map(|(_, name)| {
				ui.painter()
					.layout_no_wrap(name.to_string(), font.clone(), kit::TEXT)
					.size()
					.x + 28.0
			})
			.sum::<f32>()
			+ 4.0;
		let full = 12.0 + words.size().x + 10.0 + choices + 4.0;
		let (width, words) = match full <= rect.width() {
			true => (full, Some(words)),
			false => (choices + 8.0, None),
		};
		if width > rect.width() {
			return;
		}
		let size = vec2(width, 42.0);
		let x = match center {
			Some(center) => (center - size.x / 2.0).clamp(rect.left(), rect.right() - size.x),
			None => rect.right() - size.x,
		};
		let chip = egui::Rect::from_min_size(pos2(x, rect.center().y - 21.0), size);
		ui.painter()
			.rect(chip, 10, kit::P1, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
		if let Some(words) = words {
			let at = pos2(chip.left() + 12.0, chip.center().y - words.size().y / 2.0);
			ui.painter().galley(at, words, kit::DIM);
		}
		let segmented = egui::Rect::from_min_size(
			pos2(chip.right() - 4.0 - choices, chip.top() + 4.0),
			vec2(choices, chip.height() - 8.0),
		);
		let mut format = item.format;
		ui.put(segmented, kit::Segmented::new(&mut format, &options));
		if format == item.format {
			return;
		}
		let name = item.source.clone();
		let shown = shown(&item, format);
		let spot = Spot::Layer(picked.layer.clone(), name.clone());
		let result = edit(&mut state, shown_label(&name, format), None, |draft| {
			draft.set_item(&picked.layer, picked.index, shown)
		});
		drop(state);
		self.fail(spot, result);
	}

	/// BUILD's choice of the picture it edits, a scene drawn in both: the
	/// landscape one or the portrait one, at the left of `rect`; where it is.
	pub(super) fn format_tabs(&mut self, ui: &mut Ui, rect: egui::Rect) -> egui::Rect {
		let options = [(Format::Landscape, "Landscape"), (Format::Portrait, "Portrait")];
		let mut format = self.format;
		let at = egui::Rect::from_min_size(rect.left_center() - vec2(0.0, 17.0), vec2(rect.width(), 34.0));
		let tabs = ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
			ui.add(kit::Segmented::new(&mut format, &options))
				.on_hover_text("The picture to edit: each places the layers on its own")
				.rect
		});
		self.format = format;
		tabs.inner
	}

	/// The picture BUILD is not editing, small in `rect`, `id` drawn on
	/// `canvas`, with its format under it. A click edits it instead.
	pub(super) fn other_picture(&mut self, ui: &mut Ui, rect: egui::Rect, id: egui::TextureId, canvas: Canvas) {
		let other = self.format.other();
		let response = ui
			.put(
				rect,
				egui::Image::new((id, rect.size()))
					.corner_radius(6)
					.sense(Sense::click()),
			)
			.on_hover_text(format!("Edit the {} picture", other.name().to_lowercase()))
			.on_hover_cursor(egui::CursorIcon::PointingHand);
		let ink = match response.hovered() {
			true => kit::FAINT,
			false => kit::LINE,
		};
		ui.painter()
			.rect_stroke(rect, 6, Stroke::new(1.5, ink), StrokeKind::Outside);
		let under = egui::Rect::from_min_size(rect.left_bottom() + vec2(0.0, 8.0), vec2(rect.width(), 18.0));
		let painter = ui.painter();
		mark(painter, pos2(under.left() + 7.0, under.center().y), other, kit::DIM);
		painter.text(
			pos2(under.left() + 18.0, under.center().y),
			Align2::LEFT_CENTER,
			format!("{} × {}", canvas.width, canvas.height),
			kit::ui(11.0),
			kit::DIM,
		);
		if response.clicked() {
			self.format = other;
		}
	}
}
