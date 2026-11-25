//! Keep out of the way: where the picture draws a layer that moves itself
//! off what matters, what it saw there, and its settings in the inspector.

use std::borrow::Cow;
use std::time::Duration;

use egui::{Color32, Pos2, Stroke};
use moq_scene::place::{Placement, Signal, Slot};
use moq_scene::{KeepClear, Shape};

use super::*;

/// What matters on screen, as the stage shows it.
const HEAT: Color32 = Color32::from_rgb(0x4d, 0xe3, 0xff);

/// The glides the inspector offers.
const GLIDES: [Duration; 5] = [
	Duration::ZERO,
	Duration::from_millis(200),
	Duration::from_millis(400),
	Duration::from_millis(800),
	Duration::from_millis(1500),
];

/// The legend's height under the stage, with its gap.
pub(super) const LEGEND: f32 = 30.0;

/// How the last render placed `item`, one of `layer`, in `format`'s
/// picture while it keeps clear. Keeping clear places only the landscape
/// picture, so none in the portrait one.
pub(super) fn placement<'a>(state: &'a State, layer: &Layer, item: &Item, format: Format) -> Option<&'a Placement> {
	item.keep_clear?;
	layer.scene.as_ref()?;
	if state.collection().format_of(layer, format) != Format::Landscape {
		return None;
	}
	state.compositor.placement(item.id)
}

/// `item`, one of `layer`, resting where `format`'s picture draws it: a
/// layer keeping clear where it was placed.
pub(super) fn placed(state: &State, layer: &Layer, item: &Item, format: Format) -> Item {
	match placement(state, layer, item, format) {
		Some(placement) => Item {
			rect: placement.drawn(item.rect),
			..item.clone()
		},
		None => item.clone(),
	}
}

/// `rect`, a new rect in `format`'s picture for `item` from a gesture other
/// than a move, such as a resize, to save: a layer placed off where it rests
/// keeps resting where it did, so the resize leaves it free to keep clear.
pub(super) fn resized(state: &State, layer: &Layer, item: &Item, rect: Rect, format: Format) -> Rect {
	match placement(state, layer, item, format) {
		Some(placement) if placement.drawn(item.rect) != item.rect => Rect {
			x: item.rect.x,
			y: item.rect.y,
			..rect
		},
		_ => rect,
	}
}

/// Note that `item`, one of `layer`, was moved by hand in `format`'s
/// picture: a layer keeping clear stays where it was put until the content
/// under it changes. A move in the portrait picture, which keeping clear
/// does not place, holds nothing.
pub(super) fn hold(state: &mut State, layer: &Layer, item: &Item, format: Format) {
	if item.keep_clear.is_some()
		&& layer.scene.is_some()
		&& state.collection().format_of(layer, format) == Format::Landscape
	{
		state.compositor.hold(item.id);
	}
}

/// Note the layer a gesture on `format`'s picture has in hand, if any: a
/// layer keeping clear does not choose a place while in hand in the
/// landscape picture, the one it places.
pub(super) fn grab(state: &mut State, picked: Option<&Picked>, format: Format) {
	let collection = state.collection();
	let grabbed = picked.and_then(|picked| {
		picked.layer.scene.as_ref()?;
		let item = picked.item(collection)?;
		item.keep_clear?;
		(collection.format_of(&picked.layer, format) == Format::Landscape).then_some(item.id)
	});
	state.compositor.grab(grabbed, kio::clock::now());
}

/// `staged`, the collection as `format`'s picture draws it, with each
/// layer keeping clear where it was placed, so a pointer over the picture
/// finds what it sees.
pub(super) fn drawn<'a>(state: &State, staged: Cow<'a, Collection>, format: Format) -> Cow<'a, Collection> {
	let collection = state.collection();
	let mut drawn = staged;
	for (number, scene) in collection.scenes.iter().enumerate() {
		if collection.format_of(&Layer::scene(&scene.name), format) != Format::Landscape {
			continue;
		}
		for (index, item) in scene.items.iter().enumerate() {
			if item.keep_clear.is_none() {
				continue;
			}
			let Some(placement) = state.compositor.placement(item.id) else {
				continue;
			};
			// A scene staged one frame behind an edit may lack it.
			let staged = drawn.scenes.get(number).and_then(|staged| staged.items.get(index));
			let rect = placement.drawn(item.rect);
			if staged.is_some_and(|staged| staged.id == item.id && staged.rect != rect) {
				drawn.to_mut().scenes[number].items[index].rect = rect;
			}
		}
	}
	drawn
}

fn slot_name(slot: Slot) -> &'static str {
	match slot {
		Slot::TopLeft => "Top left",
		Slot::Top => "Top",
		Slot::TopRight => "Top right",
		Slot::Left => "Left",
		Slot::Right => "Right",
		Slot::BottomLeft => "Bottom left",
		Slot::Bottom => "Bottom",
		Slot::BottomRight => "Bottom right",
	}
}

/// Its chip in the inspector, its tag on the stage, and what a layer left
/// over it.
fn signal_names(signal: Signal) -> (&'static str, &'static str, &'static str) {
	match signal {
		Signal::Motion => ("Motion", "MOVING", "the action"),
		Signal::Text => ("Text", "TEXT", "the text"),
		Signal::Hud => ("HUD", "HUD", "the HUD"),
	}
}

/// The inspector's Keep out of the way, for a layer keeping clear as `keep`
/// says, placed as `placement` says: its switch and, while on, what it stays
/// clear of, where it is, and how it glides. The settings asked for, with a
/// label, if any changed.
pub(super) fn inspector(
	ui: &mut Ui,
	keep: Option<KeepClear>,
	placement: Option<&Placement>,
) -> Option<(Option<KeepClear>, &'static str)> {
	let mut asked = None;
	ui.horizontal(|ui| {
		let label = ui.label("Keep out of the way");
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			let mut on = keep.is_some();
			if ui
				.add(kit::Toggle::new(&mut on))
				.labelled_by(label.id)
				.on_hover_text(
					"Move off what matters in the picture under it: motion, text, and a HUD that holds still. \
					 Decided on this machine.",
				)
				.changed()
			{
				let label = if on {
					"Keep out of the way"
				} else {
					"Stop keeping out of the way"
				};
				asked = Some((on.then(KeepClear::default), label));
			}
			ui.label(
				egui::RichText::new("moves off what matters")
					.color(kit::DIM)
					.font(kit::ui(11.0)),
			);
		});
	});
	let Some(mut keep) = keep else { return asked };
	ui.add_space(4.0);
	egui::Frame::new()
		.fill(kit::P2)
		.stroke(Stroke::new(1.0, kit::LINE))
		.corner_radius(kit::RADIUS)
		.inner_margin(10.0)
		.show(ui, |ui| {
			ui.set_width(ui.available_width());
			ui.spacing_mut().item_spacing.y = 9.0;
			ui.label(
				egui::RichText::new("Stays clear of")
					.color(kit::DIM)
					.font(kit::ui(12.0)),
			);
			ui.horizontal_wrapped(|ui| {
				ui.spacing_mut().item_spacing.x = 6.0;
				for signal in [Signal::Motion, Signal::Text, Signal::Hud] {
					let on = match signal {
						Signal::Motion => &mut keep.motion,
						Signal::Text => &mut keep.text,
						Signal::Hud => &mut keep.hud,
					};
					let (name, _, _) = signal_names(signal);
					if chip(ui, name, on).clicked() {
						asked = Some((Some(keep), "Change what it stays clear of"));
					}
				}
			});
			ui.horizontal(|ui| {
				ui.spacing_mut().item_spacing.x = 10.0;
				let (slot, held) = placement.map_or((None, false), |placement| (placement.slot, placement.held));
				cells(ui, slot);
				let (name, note) = match (slot, held) {
					(Some(slot), _) => (slot_name(slot), "Corners first"),
					(None, true) => ("Where you put it", "Until the content changes"),
					(None, false) => ("Where it was put", "Until a corner is clearer"),
				};
				ui.vertical(|ui| {
					ui.spacing_mut().item_spacing.y = 2.0;
					ui.label(egui::RichText::new(name).font(kit::ui(12.0)));
					ui.label(egui::RichText::new(note).color(kit::FAINT).font(kit::ui(10.5)));
				});
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					let mut glide = keep.glide;
					egui::ComboBox::from_id_salt("keep clear glide")
						.selected_text(format!("Glide · {}", seconds(glide)))
						.width(96.0)
						.show_ui(ui, |ui| {
							for choice in GLIDES {
								let text = match choice.is_zero() {
									true => "Jump".to_string(),
									false => seconds(choice),
								};
								ui.selectable_value(&mut glide, choice, text);
							}
						});
					if glide != keep.glide {
						keep.glide = glide;
						asked = Some((Some(keep), "Change how it glides"));
					}
				});
			});
			let moves = format!(
				"Moves when the content changes, or after {} s in the worst place.",
				moq_scene::place::WORST.as_secs()
			);
			ui.label(egui::RichText::new(moves).color(kit::DIM).font(kit::ui(12.0)));
		});
	asked
}

/// `duration` in seconds, as the glide reads.
fn seconds(duration: Duration) -> String {
	format!("{:.1} s", duration.as_secs_f32())
}

/// A chip that is on or off, ticked while on.
fn chip(ui: &mut Ui, text: &str, on: &mut bool) -> egui::Response {
	let galley = ui
		.painter()
		.layout_no_wrap(text.into(), kit::ui_medium(12.0), kit::TEXT);
	let size = egui::vec2(10.0 + 14.0 + 5.0 + galley.size().x + 10.0, 26.0);
	let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
	if response.clicked() {
		*on = !*on;
		response.mark_changed();
	}
	let lit = *on;
	response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, lit, text));
	let painter = ui.painter();
	let (fill, stroke, ink) = match lit {
		true => (kit::ACCENT.gamma_multiply(0.14), kit::ACCENT, kit::TEXT),
		false => (Color32::TRANSPARENT, kit::LINE, kit::DIM),
	};
	let stroke = match response.hovered() {
		true => stroke.gamma_multiply(1.3),
		false => stroke,
	};
	painter.rect(rect, 6, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
	let mark = egui::Rect::from_center_size(egui::pos2(rect.left() + 17.0, rect.center().y), egui::Vec2::splat(14.0));
	if lit {
		Icon::Check.paint(ui, mark, 13.0, kit::TEXT);
	}
	let at = egui::pos2(mark.right() + 5.0, rect.center().y - galley.size().y / 2.0);
	ui.painter().galley(at, galley, ink);
	response
}

/// The slots as a 3x3 grid, `slot` lit and the middle, where it never goes,
/// dashed.
fn cells(ui: &mut Ui, slot: Option<Slot>) {
	let (cell, gap) = (egui::vec2(17.0, 11.0), 3.0);
	let size = egui::vec2(cell.x * 3.0 + gap * 2.0, cell.y * 3.0 + gap * 2.0);
	let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
	let painter = ui.painter();
	let lit = slot.map(Slot::cell);
	for row in 0..3u8 {
		for column in 0..3u8 {
			let min = rect.min + egui::vec2(f32::from(column) * (cell.x + gap), f32::from(row) * (cell.y + gap));
			let at = egui::Rect::from_min_size(min, cell);
			let grey = Color32::from_rgb(0x3d, 0x3d, 0x4b);
			match (column, row) {
				(1, 1) => super::dashed(painter, at, Stroke::new(1.0, grey)),
				_ if lit == Some((column, row)) => drop(painter.rect_filled(at, 2, kit::ACCENT)),
				_ => drop(painter.rect_filled(at, 2, grey)),
			}
		}
	}
}

/// What a layer keeping clear saw and did, over the stage: what matters on
/// screen, glowing and tagged by kind; the slots it may go to, dashed in its
/// shape; and where it last moved from, with an arrow to where it went.
pub(super) fn overlay(painter: &egui::Painter, view: &View, placement: &Placement, shape: &Shape, glide: Duration) {
	let heat = &placement.heat;
	let screen = |x: f32, y: f32| view.rect.min + egui::vec2(x * view.rect.width(), y * view.rect.height());
	glow(painter, heat, screen);
	// A tag for each kind of thing that matters, in the middle of where it
	// matters most: the tile whose neighbors matter most for it with it.
	let mut tags: Vec<(Signal, f32, Pos2)> = Vec::new();
	let mattering = |column: usize, row: usize, kind: Signal| {
		let (matters, signal) = heat.tiles[row * heat.columns + column];
		if signal == kind { matters } else { 0.0 }
	};
	for row in 0..heat.rows {
		for column in 0..heat.columns {
			let (matters, signal) = heat.tiles[row * heat.columns + column];
			if matters < 0.35 {
				continue;
			}
			let near = |at: usize, length: usize| at.saturating_sub(1)..(at + 2).min(length);
			let around: f32 = near(row, heat.rows)
				.flat_map(|row| near(column, heat.columns).map(move |column| (column, row)))
				.map(|(column, row)| mattering(column, row, signal))
				.sum();
			let (x0, y0, x1, y1) = heat.tile(column, row);
			let center = screen((x0 + x1) / 2.0, (y0 + y1) / 2.0);
			match tags.iter_mut().find(|(kind, _, _)| *kind == signal) {
				Some(tag) if tag.1 < around => *tag = (signal, around, center),
				Some(_) => {}
				None => tags.push((signal, around, center)),
			}
		}
	}

	let dashed = Stroke::new(1.5, Color32::from_white_alpha(128));
	for (slot, rect) in &placement.slots {
		// Its own slot shows as it, and the one it left as that.
		let left = placement.left.is_some_and(|left| left.rect == *rect);
		if Some(*slot) == placement.slot || left {
			continue;
		}
		let outline = outline(view.screen(rect), shape);
		painter.extend(egui::Shape::dashed_line(&outline, dashed, 5.0, 4.0));
	}

	for (signal, _, at) in tags {
		let (_, tag, _) = signal_names(signal);
		let galley = painter.layout_no_wrap(tag.into(), kit::display_bold(10.0), Color32::from_rgb(0xc9, 0xf6, 0xff));
		let rect = egui::Rect::from_center_size(at, galley.size() + egui::vec2(10.0, 6.0));
		painter.rect_filled(rect, 3, Color32::from_rgba_unmultiplied(0x06, 0x22, 0x2b, 0xd9));
		painter.galley(rect.min + egui::vec2(5.0, 3.0), galley, Color32::WHITE);
	}
	// Its notes over the tags, which they name again where they meet.
	if let Some(left) = placement.left {
		moved(painter, view, placement, left, shape, glide);
	}
}

/// Where a layer keeping clear moved from, what it left there, and an arrow
/// to where it went.
fn moved(
	painter: &egui::Painter,
	view: &View,
	placement: &Placement,
	left: moq_scene::place::Left,
	shape: &Shape,
	glide: Duration,
) {
	let was = view.screen(&left.rect);
	let outline_was = outline(was, shape);
	painter.add(egui::Shape::convex_polygon(
		outline_was.clone(),
		Color32::from_white_alpha(30),
		Stroke::NONE,
	));
	painter.extend(egui::Shape::dashed_line(
		&outline_was,
		Stroke::new(2.0, Color32::WHITE),
		6.0,
		4.0,
	));
	let to = view.screen(&placement_target(placement));
	if was.center().distance(to.center()) > 4.0 {
		arrow(painter, was, to);
		if !glide.is_zero() {
			let mid = was.center() + (to.center() - was.center()) / 2.0;
			note(painter, mid, &format!("{} glide", seconds(glide)));
		}
	}
	let text = match left.over {
		Some(signal) => format!("Was here, over {}", signal_names(signal).2),
		None => "Was here".to_string(),
	};
	note(painter, egui::pos2(was.center().x, was.top() - 14.0), &text);
}

/// What matters, as a soft glow: each corner of the tiles as bright as the
/// tiles around it, blended between.
fn glow(painter: &egui::Painter, heat: &moq_scene::place::Heat, screen: impl Fn(f32, f32) -> Pos2) {
	let (columns, rows) = (heat.columns, heat.rows);
	if columns == 0 || rows == 0 {
		return;
	}
	let (across, down) = &heat.edges;
	let mut mesh = egui::Mesh::default();
	for (row, y) in down.iter().enumerate() {
		for (column, x) in across.iter().enumerate() {
			let mut sum = 0.0;
			let mut count = 0.0;
			for (c, r) in [
				(column, row),
				(column.wrapping_sub(1), row),
				(column, row.wrapping_sub(1)),
				(column.wrapping_sub(1), row.wrapping_sub(1)),
			] {
				if c < columns && r < rows {
					sum += heat.tiles[r * columns + c].0;
					count += 1.0;
				}
			}
			let matters = sum / count;
			// Faint for what barely matters, so the picture shows through.
			let alpha = ((matters - 0.08) / 0.62).clamp(0.0, 1.0) * 0.55;
			mesh.colored_vertex(screen(*x, *y), HEAT.gamma_multiply(alpha));
		}
	}
	let stride = (columns + 1) as u32;
	for row in 0..rows as u32 {
		for column in 0..columns as u32 {
			let at = row * stride + column;
			mesh.add_triangle(at, at + 1, at + stride);
			mesh.add_triangle(at + 1, at + stride + 1, at + stride);
		}
	}
	painter.add(egui::Shape::mesh(mesh));
}

/// Where a layer keeping clear is going: its slot, or where it is.
fn placement_target(placement: &Placement) -> Rect {
	let slot = placement
		.slot
		.and_then(|slot| placement.slots.iter().find(|(fits, _)| *fits == slot));
	slot.map_or(placement.rect, |(_, rect)| *rect)
}

/// A note in a dark pill, centered on `at`, kept on the picture.
fn note(painter: &egui::Painter, at: Pos2, text: &str) {
	let galley = painter.layout_no_wrap(text.into(), kit::ui_medium(11.5), kit::TEXT);
	let size = galley.size() + egui::vec2(16.0, 10.0);
	let mut rect = egui::Rect::from_center_size(at, size);
	let clip = painter.clip_rect().shrink(4.0);
	rect = rect.translate(egui::vec2(
		(clip.left() - rect.left()).max(0.0) - (rect.right() - clip.right()).max(0.0),
		(clip.top() - rect.top()).max(0.0) - (rect.bottom() - clip.bottom()).max(0.0),
	));
	painter.rect(
		rect,
		6,
		Color32::from_black_alpha(204),
		Stroke::new(1.0, Color32::from_white_alpha(51)),
		egui::StrokeKind::Inside,
	);
	painter.galley(rect.min + egui::vec2(8.0, 5.0), galley, kit::TEXT);
}

/// A curved arrow from the edge of `from` to the edge of `to`.
fn arrow(painter: &egui::Painter, from: egui::Rect, to: egui::Rect) {
	let (start, end) = (from.center(), to.center());
	let along = end - start;
	let length = along.length();
	let direction = along / length;
	// Out of each rect, so the arrow runs between them.
	let start = start + direction * (from.width().min(from.height()) / 2.0 + 4.0);
	let end = end - direction * (to.width().min(to.height()) / 2.0 + 6.0);
	let normal = egui::vec2(-direction.y, direction.x);
	let control = start + (end - start) / 2.0 + normal * (length * 0.25);
	let points: Vec<Pos2> = (0..=24)
		.map(|step| {
			let t = step as f32 / 24.0;
			let (a, b) = (start + (control - start) * t, control + (end - control) * t);
			a + (b - a) * t
		})
		.collect();
	let stroke = Stroke::new(2.5, Color32::WHITE);
	let shadow = Stroke::new(4.5, Color32::from_black_alpha(90));
	painter.add(egui::Shape::line(points.clone(), shadow));
	painter.add(egui::Shape::line(points.clone(), stroke));
	let tip = points[points.len() - 1];
	let back = (points[points.len() - 3] - tip).normalized() * 12.0;
	let side = egui::vec2(-back.y, back.x) * 0.5;
	painter.add(egui::Shape::line(
		vec![tip + back + side, tip, tip + back - side],
		stroke,
	));
}

/// The outline of `shape` around `rect` on screen, as points round it.
fn outline(rect: egui::Rect, shape: &Shape) -> Vec<Pos2> {
	let side = rect.width().min(rect.height());
	let radius = |corner: f32| (corner.clamp(0.0, 0.5) * side).max(0.0);
	// Top left, top right, bottom right, bottom left, each with the angle its
	// arc starts at.
	let corners = [
		(rect.left_top(), egui::vec2(1.0, 1.0), std::f32::consts::PI),
		(rect.right_top(), egui::vec2(-1.0, 1.0), 1.5 * std::f32::consts::PI),
		(rect.right_bottom(), egui::vec2(-1.0, -1.0), 0.0),
		(rect.left_bottom(), egui::vec2(1.0, -1.0), 0.5 * std::f32::consts::PI),
	];
	let mut points = Vec::new();
	for ((corner, inward, start), round) in corners.into_iter().zip(shape.corners) {
		let radius = radius(round);
		let center = corner + inward * radius;
		for step in 0..=8 {
			let angle = start + step as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
			points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
		}
	}
	points.push(points[0]);
	points
}

/// The legend under the stage while a layer keeping clear is selected: what
/// the overlay's marks mean, and that the decision stays on this machine.
pub(super) fn legend(ui: &Ui, below: egui::Rect) {
	let painter = ui.painter();
	let y = below.bottom() + 18.0;
	let font = kit::ui(12.0);
	let mut x = below.left() + 4.0;
	let mark = |painter: &egui::Painter, x: f32, kind: u8| {
		let center = egui::pos2(x + 7.0, y);
		match kind {
			0 => {
				painter.circle_filled(center, 7.0, HEAT.gamma_multiply(0.25));
				painter.circle_filled(center, 4.0, HEAT);
			}
			1 => {
				let ring: Vec<Pos2> = (0..=24)
					.map(|step| {
						let angle = step as f32 / 24.0 * std::f32::consts::TAU;
						center + egui::vec2(angle.cos(), angle.sin()) * 6.5
					})
					.collect();
				painter.extend(egui::Shape::dashed_line(
					&ring,
					Stroke::new(1.5, Color32::from_white_alpha(150)),
					2.5,
					2.0,
				));
			}
			_ => {
				let square = egui::Rect::from_center_size(center, egui::Vec2::splat(13.0));
				painter.rect_stroke(square, 3, Stroke::new(2.0, kit::ACCENT), egui::StrokeKind::Inside);
			}
		}
	};
	let right = below.right() - 4.0;
	let laid = |labels: [&str; 3]| {
		let galleys = labels.map(|text| painter.layout_no_wrap(text.into(), font.clone(), kit::DIM));
		let width: f32 = galleys.iter().map(|galley| 21.0 + galley.size().x + 18.0).sum();
		(galleys, width - 18.0)
	};
	// Narrow, as Studio is in a small window, the labels shorten.
	let (galleys, width) = laid(["What matters on screen", "Places it may go", "Where it went"]);
	let galleys = match x + width <= right {
		true => galleys,
		false => laid(["What matters", "May go", "Went"]).0,
	};
	for (kind, galley) in galleys.into_iter().enumerate() {
		mark(painter, x, kind as u8);
		let width = galley.size().x;
		painter.galley(egui::pos2(x + 21.0, y - galley.size().y / 2.0), galley, kit::DIM);
		x += 21.0 + width + 18.0;
	}
	// The note goes where it fits beside them.
	let galley = painter.layout_no_wrap("Decided on this machine".into(), font, kit::DIM);
	let at = egui::pos2(right - galley.size().x, y - galley.size().y / 2.0);
	if at.x - 20.0 < x {
		return;
	}
	let icon = egui::Rect::from_center_size(egui::pos2(at.x - 12.0, y), egui::Vec2::splat(14.0));
	painter.galley(at, galley, kit::DIM);
	Icon::Panic.paint(ui, icon, 13.0, kit::DIM);
}
