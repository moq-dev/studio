//! The kit's widgets. Each draws what its caller hands it and reports what
//! was done to it; none keeps state across frames.

use egui::{
	Color32, CornerRadius, Frame, Margin, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, Widget,
	WidgetInfo, WidgetType, emath::Rot2, epaint::Mesh, pos2, vec2,
};

use super::{ACCENT, BG, CUE, DIM, ERR, FAINT, Icon, LINE, OK, ON_CUE, P1, P2, P3, POPOVER, RADIUS, TEXT};
use crate::fx::{Fx, Mark};

/// How a control is being touched, which picks how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
	Rest,
	Hover,
	Press,
	/// Disabled: shown, but not touchable.
	Off,
}

impl Pose {
	fn of(response: &Response) -> Self {
		if !response.enabled() {
			Self::Off
		} else if response.is_pointer_button_down_on() {
			Self::Press
		} else if response.hovered() {
			Self::Hover
		} else {
			Self::Rest
		}
	}
}

/// A border a step brighter than [`LINE`], for what the pointer is on.
const LINE_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x48);

/// Show `name`, and `key` on a keycap, while the pointer is on `response`,
/// enabled or not.
pub fn tip(response: Response, name: &str, key: Option<&str>) -> Response {
	let open = egui::Tooltip::should_show_tooltip(&response, true);
	tooltip(&response, open, name, key, None);
	response
}

/// [`tip`]'s tooltip, shown while `open`, with a `note` under the name.
pub(super) fn tooltip(response: &Response, open: bool, name: &str, key: Option<&str>, note: Option<&str>) {
	let frame = Frame::new()
		.fill(Color32::BLACK)
		.stroke(Stroke::new(1.0, LINE))
		.corner_radius(7)
		.inner_margin(Margin::symmetric(8, 6));
	let mut tooltip = egui::Tooltip::for_widget(response);
	tooltip.popup = tooltip.popup.frame(frame).open(open);
	tooltip.show(|ui| {
		ui.horizontal(|ui| {
			ui.label(egui::RichText::new(name).font(super::ui_medium(11.5)).color(TEXT));
			if let Some(key) = key {
				ui.add(Keycap::new(key));
			}
		});
		if let Some(note) = note {
			ui.label(egui::RichText::new(note).font(super::ui(11.5)).color(DIM));
		}
	});
}

/// What an icon button looks like at rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
	/// Just the icon, dim, until the pointer is on it.
	Plain,
	/// On a control's fill, with a border.
	Boxed,
	/// Outlined in [`CUE`]: a control that changes the show for safety, such
	/// as the panic key.
	Cue,
}

/// A control that is only an icon, named by its tooltip with its key.
pub struct IconButton<'a> {
	icon: Icon,
	name: &'a str,
	key: Option<&'a str>,
	look: Look,
	/// The fill while lit, such as a mute while muted.
	lit: Option<Color32>,
	size: f32,
	/// How wide it is, if wider than tall, such as a strip's mute.
	width: Option<f32>,
	/// Said under the name, such as why it is disabled.
	note: Option<&'a str>,
	pose: Option<Pose>,
}

impl<'a> IconButton<'a> {
	/// `icon`, named `name` in its tooltip.
	pub fn new(icon: Icon, name: &'a str) -> Self {
		Self {
			icon,
			name,
			key: None,
			look: Look::Plain,
			lit: None,
			size: 30.0,
			width: None,
			note: None,
			pose: None,
		}
	}

	/// Make it `width` wide, as a mixer strip's buttons fill the strip.
	pub fn width(mut self, width: f32) -> Self {
		self.width = Some(width);
		self
	}

	/// The key that does the same, shown in the tooltip.
	pub fn key(mut self, key: Option<&'a str>) -> Self {
		self.key = key;
		self
	}

	pub fn look(mut self, look: Look) -> Self {
		self.look = look;
		self
	}

	/// Fill the button with `color` while `lit`, as a mute does while muted.
	pub fn lit(mut self, lit: bool, color: Color32) -> Self {
		self.lit = lit.then_some(color);
		self
	}

	/// Say `note` under the name in the tooltip, such as why it is disabled.
	pub fn note(mut self, note: Option<&'a str>) -> Self {
		self.note = note;
		self
	}

	/// The size of a button in a list's row rather than a bar.
	pub fn small(mut self) -> Self {
		self.size = 24.0;
		self
	}

	/// Draw it as though touched so, for the gallery.
	#[cfg(test)]
	pub fn pose(mut self, pose: Pose) -> Self {
		self.pose = Some(pose);
		self
	}
}

impl Widget for IconButton<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let size = vec2(self.width.unwrap_or(self.size), self.size);
		let (rect, response) = ui.allocate_exact_size(size, Sense::click());
		response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.name));
		if ui.is_rect_visible(rect) {
			let pose = self.pose.unwrap_or_else(|| Pose::of(&response));
			let (fill, stroke, mut ink) = match (self.lit, self.look, pose) {
				(Some(lit), ..) => (lit, Stroke::NONE, ink_on(lit)),
				(None, Look::Plain, Pose::Rest | Pose::Off) => (Color32::TRANSPARENT, Stroke::NONE, DIM),
				(None, Look::Plain, Pose::Hover) => (P2, Stroke::NONE, TEXT),
				(None, Look::Plain, Pose::Press) => (P3, Stroke::NONE, TEXT),
				(None, Look::Boxed, Pose::Rest | Pose::Off) => (P2, Stroke::new(1.0, LINE), TEXT),
				(None, Look::Boxed, Pose::Hover | Pose::Press) => (P3, Stroke::new(1.0, LINE_HOVER), TEXT),
				(None, Look::Cue, Pose::Press) => (CUE.gamma_multiply(0.25), Stroke::new(1.0, CUE), CUE),
				(None, Look::Cue, _) => (Color32::TRANSPARENT, Stroke::new(1.0, CUE), CUE),
			};
			if pose == Pose::Off {
				ink = ink.gamma_multiply(0.4);
			}
			let corner = if self.size < 30.0 { 6 } else { RADIUS };
			ui.painter().rect(rect, corner, fill, stroke, StrokeKind::Inside);
			self.icon.paint(ui, rect, (self.size * 0.6).round(), ink);
		}
		let open = egui::Tooltip::should_show_tooltip(&response, true);
		tooltip(&response, open, self.name, self.key, self.note);
		response
	}
}

/// A quick action: an icon and a word on a control, such as the panic key
/// with the scene it cuts to, named by its tooltip with its key.
pub struct Quick<'a> {
	icon: Icon,
	text: &'a str,
	name: &'a str,
	key: Option<&'a str>,
	note: Option<&'a str>,
	look: Look,
	width: f32,
	pose: Option<Pose>,
}

impl<'a> Quick<'a> {
	/// `icon` and `text`, `width` points wide, named `name` in its tooltip.
	pub fn new(icon: Icon, text: &'a str, name: &'a str, width: f32) -> Self {
		Self {
			icon,
			text,
			name,
			key: None,
			note: None,
			look: Look::Boxed,
			width,
			pose: None,
		}
	}

	/// The key that does the same, shown in the tooltip.
	pub fn key(mut self, key: Option<&'a str>) -> Self {
		self.key = key;
		self
	}

	/// Say `note` under the name in the tooltip, such as what it does.
	pub fn note(mut self, note: Option<&'a str>) -> Self {
		self.note = note;
		self
	}

	pub fn look(mut self, look: Look) -> Self {
		self.look = look;
		self
	}

	/// Draw it as though touched so, for the gallery.
	#[cfg(test)]
	pub fn pose(mut self, pose: Pose) -> Self {
		self.pose = Some(pose);
		self
	}
}

impl Widget for Quick<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let (rect, response) = ui.allocate_exact_size(vec2(self.width, 30.0), Sense::click());
		response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.name));
		if ui.is_rect_visible(rect) {
			let pose = self.pose.unwrap_or_else(|| Pose::of(&response));
			let (fill, stroke, ink) = match (self.look, pose) {
				(Look::Cue, Pose::Press) => (CUE.gamma_multiply(0.25), CUE, CUE),
				(Look::Cue, Pose::Hover) => (CUE.gamma_multiply(0.12), CUE, CUE),
				(Look::Cue, _) => (Color32::TRANSPARENT, CUE, CUE),
				(_, Pose::Hover | Pose::Press) => (P3, LINE_HOVER, TEXT),
				(_, _) => (P2, LINE, TEXT),
			};
			let ink = if pose == Pose::Off {
				ink.gamma_multiply(0.4)
			} else {
				ink
			};
			ui.painter()
				.rect(rect, RADIUS, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
			let font = super::display(13.0);
			let room = rect.width() - 22.0 - 16.0 - 6.0;
			let galley = caps_galley(ui.painter(), self.text, font, ink, Some(room));
			let width = 16.0 + 6.0 + galley.size().x;
			let left = rect.center().x - width / 2.0;
			let icon = Rect::from_center_size(pos2(left + 8.0, rect.center().y), Vec2::splat(16.0));
			self.icon.paint(ui, icon, 16.0, ink);
			let at = pos2(left + 22.0, rect.center().y - galley.size().y / 2.0);
			ui.painter().galley(at, galley, ink);
		}
		let open = egui::Tooltip::should_show_tooltip(&response, true);
		tooltip(&response, open, self.name, self.key, self.note);
		response
	}
}

/// egui's button, with `icon` before `text`.
pub fn button(icon: Icon, text: &str) -> egui::Button<'_> {
	egui::Button::image_and_text(icon.image(14.0, TEXT), text)
}

/// Dark ink on a light fill, light on a dark one.
fn ink_on(fill: Color32) -> Color32 {
	let [r, g, b, _] = fill.to_array();
	let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
	if luma > 140.0 { ON_CUE } else { TEXT }
}

/// A rounded label for a state, such as the stream's health: an optional dot,
/// a name, and an optional value in the monospace.
pub struct Pill<'a> {
	text: &'a str,
	dot: Option<Color32>,
	value: Option<&'a str>,
	sense: Sense,
}

impl<'a> Pill<'a> {
	pub fn new(text: &'a str) -> Self {
		Self {
			text,
			dot: None,
			value: None,
			sense: Sense::hover(),
		}
	}

	pub fn dot(mut self, color: Color32) -> Self {
		self.dot = Some(color);
		self
	}

	#[cfg_attr(
		not(test),
		expect(dead_code, reason = "the pages draw with it: the rest of quest/m0/ux")
	)]
	pub fn value(mut self, value: &'a str) -> Self {
		self.value = Some(value);
		self
	}

	/// Clickable, as a pill that opens a popover is.
	pub fn clickable(mut self) -> Self {
		self.sense = Sense::click();
		self
	}
}

impl Widget for Pill<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let painter = ui.painter();
		let text = painter.layout_no_wrap(self.text.into(), super::ui_medium(12.0), TEXT);
		let value = self
			.value
			.map(|value| painter.layout_no_wrap(value.into(), super::mono(12.0), TEXT));
		let dot = if self.dot.is_some() { 8.0 + 7.0 } else { 0.0 };
		let gap = if value.is_some() { 7.0 } else { 0.0 };
		let value_width = value.as_ref().map_or(0.0, |value| value.size().x);
		let size = vec2(10.0 + dot + text.size().x + gap + value_width + 10.0, 28.0);
		let (rect, response) = ui.allocate_exact_size(size, self.sense);
		response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), self.text));
		if ui.is_rect_visible(rect) {
			let stroke = match response.hovered() && self.sense.senses_click() {
				true => LINE_HOVER,
				false => LINE,
			};
			let painter = ui.painter();
			painter.rect(rect, 14, P2, Stroke::new(1.0, stroke), StrokeKind::Inside);
			let mut x = rect.left() + 10.0;
			if let Some(color) = self.dot {
				painter.circle_filled(pos2(x + 4.0, rect.center().y), 4.0, color);
				x += dot;
			}
			let width = text.size().x;
			painter.galley(pos2(x, rect.center().y - text.size().y / 2.0), text, TEXT);
			x += width + gap;
			if let Some(value) = value {
				painter.galley(pos2(x, rect.center().y - value.size().y / 2.0), value, TEXT);
			}
		}
		response
	}
}

/// A switch for a setting that is on or off, with its label beside it.
pub struct Toggle<'a> {
	on: &'a mut bool,
	label: Option<&'a str>,
}

impl<'a> Toggle<'a> {
	pub fn new(on: &'a mut bool) -> Self {
		Self { on, label: None }
	}

	pub fn label(mut self, label: &'a str) -> Self {
		self.label = Some(label);
		self
	}
}

impl Widget for Toggle<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let label = self
			.label
			.map(|label| ui.painter().layout_no_wrap(label.into(), super::ui(13.0), TEXT));
		let track = vec2(32.0, 18.0);
		let width = track.x + label.as_ref().map_or(0.0, |label| 8.0 + label.size().x);
		let (rect, mut response) = ui.allocate_exact_size(vec2(width, track.y.max(20.0)), Sense::click());
		if response.clicked() {
			*self.on = !*self.on;
			response.mark_changed();
		}
		let on = *self.on;
		response.widget_info(|| {
			WidgetInfo::selected(
				WidgetType::Checkbox,
				ui.is_enabled(),
				on,
				self.label.unwrap_or_default(),
			)
		});
		if ui.is_rect_visible(rect) {
			let painter = ui.painter();
			let dim = if ui.is_enabled() { 1.0 } else { 0.4 };
			let track = Rect::from_min_size(pos2(rect.left(), rect.center().y - 9.0), track);
			let fill = if on { ACCENT } else { P3 };
			let fill = if response.hovered() && !on { LINE_HOVER } else { fill };
			painter.rect_filled(track, 9, fill.gamma_multiply(dim));
			let how_on = ui.ctx().animate_bool_responsive(response.id, on);
			let x = egui::lerp(track.left() + 9.0..=track.right() - 9.0, how_on);
			painter.circle_filled(pos2(x, track.center().y), 7.0, Color32::WHITE.gamma_multiply(dim));
			if let Some(label) = label {
				let at = pos2(track.right() + 8.0, rect.center().y - label.size().y / 2.0);
				painter.galley(at, label, TEXT.gamma_multiply(dim));
			}
		}
		response
	}
}

/// One of a few named choices, side by side, the one picked lit, such as a
/// canvas size.
pub struct Segmented<'a, T> {
	current: &'a mut T,
	options: &'a [(T, &'a str)],
	reclick: bool,
}

impl<'a, T: PartialEq + Clone> Segmented<'a, T> {
	pub fn new(current: &'a mut T, options: &'a [(T, &'a str)]) -> Self {
		Self {
			current,
			options,
			reclick: false,
		}
	}

	/// Count a click on the option already chosen as a change too, such as
	/// Custom opening its editor again.
	pub fn reclick(mut self) -> Self {
		self.reclick = true;
		self
	}
}

impl<T: PartialEq + Clone> Widget for Segmented<'_, T> {
	fn ui(self, ui: &mut Ui) -> Response {
		let font = super::ui_strong(12.5);
		let layout =
			|painter: &egui::Painter, name: &str, ink: Color32| painter.layout_no_wrap(name.into(), font.clone(), ink);
		let widths: Vec<f32> = self
			.options
			.iter()
			.map(|(_, name)| layout(ui.painter(), name, TEXT).size().x + 28.0)
			.collect();
		let size = vec2(widths.iter().sum::<f32>() + 4.0, 34.0);
		let (rect, mut response) = ui.allocate_exact_size(size, Sense::hover());
		let painter = ui.painter().clone();
		painter.rect(rect, 9, P2, Stroke::new(1.0, LINE), StrokeKind::Inside);
		let mut x = rect.left() + 2.0;
		for (index, (value, name)) in self.options.iter().enumerate() {
			let at = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(widths[index], rect.height() - 4.0));
			x = at.right();
			let option = ui.interact(at, response.id.with(index), Sense::click());
			let on = *self.current == *value;
			option.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), on, *name));
			if option.clicked() && (!on || self.reclick) {
				*self.current = value.clone();
				response.mark_changed();
			}
			let on = *self.current == *value;
			let dim = if ui.is_enabled() { 1.0 } else { 0.4 };
			if on {
				painter.rect_filled(at, 7, TEXT.gamma_multiply(dim));
			} else if option.hovered() {
				painter.rect_filled(at, 7, P3);
			}
			let ink = match (on, option.hovered()) {
				(true, _) => BG,
				(false, true) => TEXT,
				(false, false) => DIM,
			};
			let galley = layout(&painter, name, ink.gamma_multiply(dim));
			painter.galley(at.center() - galley.size() / 2.0, galley, ink);
		}
		response
	}
}

/// Where `pos` falls along `range`, from 0 to 1.
fn fraction(pos: f32, range: egui::Rangef) -> f32 {
	((pos - range.min) / range.span()).clamp(0.0, 1.0)
}

/// Put `to` in `value`, saying whether that changed it, so a drag that holds
/// still edits nothing.
fn set(value: &mut f32, to: f32) -> bool {
	let changed = *value != to;
	*value = to;
	changed
}

/// Step a 0-to-1 `value` by the arrow keys while `response` has focus, which
/// keeps the arrows from moving focus to the next control.
fn arrows(ui: &Ui, response: &Response, value: &mut f32) -> bool {
	if !response.has_focus() {
		return false;
	}
	ui.memory_mut(|memory| {
		let filter = egui::EventFilter {
			horizontal_arrows: true,
			vertical_arrows: true,
			..Default::default()
		};
		memory.set_focus_lock_filter(response.id, filter);
	});
	let step = ui.input(|input| {
		let up = input.key_pressed(egui::Key::ArrowUp) || input.key_pressed(egui::Key::ArrowRight);
		let down = input.key_pressed(egui::Key::ArrowDown) || input.key_pressed(egui::Key::ArrowLeft);
		f32::from(u8::from(up)) - f32::from(u8::from(down))
	});
	if step == 0.0 {
		return false;
	}
	*value = (*value + step * 0.02).clamp(0.0, 1.0);
	true
}

/// A horizontal slider over 0 to 1: a thin track filled to the value, and a
/// knob. The caller maps its own units, such as decibels, onto it.
pub struct Slider<'a> {
	value: &'a mut f32,
	width: Option<f32>,
}

impl<'a> Slider<'a> {
	pub fn new(value: &'a mut f32) -> Self {
		Self { value, width: None }
	}

	pub fn width(mut self, width: f32) -> Self {
		self.width = Some(width);
		self
	}
}

impl Widget for Slider<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let width = self.width.unwrap_or_else(|| ui.spacing().slider_width);
		// Drag alone, so a press starts the drag: its jump and the drag are one gesture.
		let (rect, mut response) = ui.allocate_exact_size(vec2(width, 18.0), Sense::drag());
		let track = egui::Rangef::new(rect.left() + 7.0, rect.right() - 7.0);
		// The release too, so a move in its frame is kept.
		if (response.dragged() || response.drag_stopped())
			&& let Some(pos) = response.interact_pointer_pos()
			&& set(self.value, fraction(pos.x, track))
		{
			response.mark_changed();
		}
		if arrows(ui, &response, self.value) {
			response.mark_changed();
		}
		let value = *self.value;
		response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), f64::from(value), ""));
		if ui.is_rect_visible(rect) {
			let painter = ui.painter();
			let y = rect.center().y;
			let x = egui::lerp(track.min..=track.max, value);
			let rail = Rect::from_min_max(pos2(track.min, y - 2.0), pos2(track.max, y + 2.0));
			painter.rect_filled(rail, 2, P3);
			painter.rect_filled(Rect::from_min_max(rail.min, pos2(x, rail.max.y)), 2, ACCENT);
			let knob = if response.hovered() || response.dragged() {
				8.0
			} else {
				7.0
			};
			painter.circle_filled(pos2(x, y), knob, Color32::WHITE);
		}
		response
	}
}

/// A vertical fader over 0 to 1, as on a mixing desk: a slot, a cap, and a
/// line across the slot at a mark, such as unity gain.
pub struct Fader<'a> {
	value: &'a mut f32,
	height: f32,
	mark: Option<f32>,
}

impl<'a> Fader<'a> {
	/// How far in from its top and bottom the cap's center stops, so a scale
	/// beside it lines up with its travel.
	pub const INSET: f32 = 8.0;

	pub fn new(value: &'a mut f32) -> Self {
		Self {
			value,
			height: 160.0,
			mark: None,
		}
	}

	pub fn height(mut self, height: f32) -> Self {
		self.height = height;
		self
	}

	/// Mark the value `at` across the slot, as unity on a gain fader.
	pub fn mark(mut self, at: f32) -> Self {
		self.mark = Some(at);
		self
	}
}

impl Widget for Fader<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let (rect, mut response) = ui.allocate_exact_size(vec2(28.0, self.height), Sense::drag());
		let travel = egui::Rangef::new(rect.top() + Self::INSET, rect.bottom() - Self::INSET);
		// The release too, so a move in its frame is kept.
		if (response.dragged() || response.drag_stopped())
			&& let Some(pos) = response.interact_pointer_pos()
			&& set(self.value, 1.0 - fraction(pos.y, travel))
		{
			response.mark_changed();
		}
		if arrows(ui, &response, self.value) {
			response.mark_changed();
		}
		let value = *self.value;
		response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), f64::from(value), ""));
		if ui.is_rect_visible(rect) {
			let painter = ui.painter();
			let slot = Rect::from_center_size(rect.center(), vec2(8.0, rect.height()));
			painter.rect_filled(slot, 4, Color32::BLACK);
			if let Some(mark) = self.mark {
				let y = egui::lerp(travel.max..=travel.min, mark);
				painter.hline(rect.x_range().shrink(2.0), y, Stroke::new(2.0, FAINT));
			}
			let y = egui::lerp(travel.max..=travel.min, value);
			let cap = Rect::from_center_size(pos2(rect.center().x, y), vec2(26.0, 16.0));
			// A cap lit from above: light on top, darker below, and a groove.
			let shadow = cap.translate(vec2(0.0, 2.0));
			painter.rect_filled(shadow, 4, Color32::BLACK);
			gradient(
				painter,
				cap,
				4,
				Color32::from_rgb(0xf2, 0xf2, 0xf5),
				Color32::from_rgb(0xa5, 0xa5, 0xb1),
			);
			painter.hline(cap.x_range().shrink(4.0), y, Stroke::new(1.0, Color32::from_gray(90)));
		}
		response
	}
}

/// Fill `rect`, rounded by `corner`, from `top` down to `bottom`.
fn gradient(painter: &egui::Painter, rect: Rect, corner: u8, top: Color32, bottom: Color32) {
	// The rounded body in the bottom color, its top half's gradient over it.
	painter.rect_filled(rect, corner, bottom);
	let inner = rect.shrink2(vec2(0.0, f32::from(corner)));
	let mut mesh = Mesh::default();
	mesh.colored_vertex(inner.left_top(), top);
	mesh.colored_vertex(inner.right_top(), top);
	mesh.colored_vertex(inner.left_bottom(), bottom);
	mesh.colored_vertex(inner.right_bottom(), bottom);
	mesh.add_triangle(0, 1, 2);
	mesh.add_triangle(1, 2, 3);
	let cap = Rect::from_min_max(rect.min, pos2(rect.max.x, inner.min.y));
	painter.rect_filled(
		cap,
		CornerRadius {
			nw: corner,
			ne: corner,
			sw: 0,
			se: 0,
		},
		top,
	);
	painter.add(mesh);
}

/// Which way a meter runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
	/// Left to right, as a strip's meter in a row.
	Across,
	/// Bottom to top, as the stream's meter beside the strips.
	Up,
}

/// A level meter in segments: green while healthy, amber once hot, and the
/// top segment red once the peak clips, with the peak held as a lit segment.
/// Levels run from 0 (silence) to 1 (full scale).
pub struct Meter {
	level: f32,
	peak: f32,
	hot: f32,
	run: Run,
	size: Vec2,
}

impl Meter {
	/// Where a meter turns from healthy to hot, unless told otherwise.
	pub const HOT: f32 = 0.7;

	pub fn new(level: f32, peak: f32) -> Self {
		Self {
			level,
			peak,
			hot: Self::HOT,
			run: Run::Across,
			size: vec2(160.0, 9.0),
		}
	}

	/// Turn hot from `at` along the run, for a meter on a scale of its own.
	pub fn hot(mut self, at: f32) -> Self {
		self.hot = at;
		self
	}

	/// Run `run`, `size` points along it and across.
	pub fn run(mut self, run: Run, along: f32, across: f32) -> Self {
		self.run = run;
		self.size = match run {
			Run::Across => vec2(along, across),
			Run::Up => vec2(across, along),
		};
		self
	}
}

impl Widget for Meter {
	fn ui(self, ui: &mut Ui) -> Response {
		let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
		if !ui.is_rect_visible(rect) {
			return response;
		}
		let (segment, gap) = match self.run {
			Run::Across => (5.0, 2.0),
			Run::Up => (4.0, 2.0),
		};
		let along = match self.run {
			Run::Across => rect.width(),
			Run::Up => rect.height(),
		};
		let count = ((along + gap) / (segment + gap)).floor().max(1.0) as usize;
		let peak = (self.peak.clamp(0.0, 1.0) * count as f32).ceil() as usize;
		let clipped = self.peak >= 1.0;
		let painter = ui.painter();
		for index in 0..count {
			let at = (index as f32 + 0.5) / count as f32;
			let lit = at <= self.level;
			let color = match (lit || index + 1 == peak, at >= self.hot) {
				(false, _) => P3,
				(true, false) => OK,
				(true, true) if clipped && index + 1 == count => ERR,
				(true, true) => CUE,
			};
			let start = index as f32 * (segment + gap);
			let cell = match self.run {
				Run::Across => Rect::from_min_size(pos2(rect.left() + start, rect.top()), vec2(segment, rect.height())),
				Run::Up => Rect::from_min_size(
					pos2(rect.left(), rect.bottom() - start - segment),
					vec2(rect.width(), segment),
				),
			};
			painter.rect_filled(cell, 1, color);
		}
		response
	}
}

/// A number that drags: its field, a faint label before it such as `X`, and
/// a faint unit after it such as `ms`. Click to type.
pub struct Number<'a> {
	drag: egui::DragValue<'a>,
	label: Option<&'a str>,
	unit: Option<&'a str>,
	width: f32,
}

impl<'a> Number<'a> {
	/// Show `drag`, ranged and stepped as the caller set it.
	pub fn new(drag: egui::DragValue<'a>) -> Self {
		Self {
			drag,
			label: None,
			unit: None,
			width: 72.0,
		}
	}

	pub fn label(mut self, label: &'a str) -> Self {
		self.label = Some(label);
		self
	}

	pub fn unit(mut self, unit: &'a str) -> Self {
		self.unit = Some(unit);
		self
	}

	pub fn width(mut self, width: f32) -> Self {
		self.width = width;
		self
	}
}

impl Widget for Number<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let (rect, _) = ui.allocate_exact_size(vec2(self.width, 28.0), Sense::hover());
		let field = ui.painter().add(Shape::Noop);
		let mut child = ui.new_child(
			egui::UiBuilder::new()
				.max_rect(rect)
				.layout(egui::Layout::left_to_right(egui::Align::Center)),
		);
		// One button the size of the field, its label and unit inside it, so a
		// press anywhere on the field, not just on the numeral, drags or types.
		child.spacing_mut().interact_size = rect.size();
		child.spacing_mut().button_padding = vec2(8.0, 0.0);
		let visuals = &mut child.visuals_mut().widgets;
		for state in [&mut visuals.inactive, &mut visuals.hovered, &mut visuals.active] {
			state.weak_bg_fill = Color32::TRANSPARENT;
			state.bg_stroke = Stroke::NONE;
		}
		child.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
		let faint = |text| egui::RichText::new(text).font(super::ui_medium(10.0)).color(FAINT);
		let gap = |side| egui::Atom::custom(egui::Id::new(("number gap", side)), vec2(6.0, 0.0));
		let mut drag = self.drag;
		if let Some(label) = self.label {
			drag = drag.prefix((faint(label), gap("label")));
		}
		if let Some(unit) = self.unit {
			drag = drag.suffix((gap("unit"), faint(unit)));
		}
		// The blank after the text grows, keeping the text at the field's start.
		let response = child.add(drag.suffix(egui::Atom::grow()));
		let stroke = match (response.has_focus() || response.dragged(), response.hovered()) {
			(true, _) => ACCENT,
			(false, true) => LINE_HOVER,
			(false, false) => LINE,
		};
		ui.painter().set(
			field,
			Shape::Vec(vec![
				Shape::rect_filled(rect, 6, P2),
				Shape::rect_stroke(rect, 6, Stroke::new(1.0, stroke), StrokeKind::Inside),
			]),
		);
		response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
	}
}

/// The popover's frame: a raised card over the window.
pub fn popover_frame() -> Frame {
	Frame::new()
		.fill(POPOVER)
		.stroke(Stroke::new(1.0, LINE))
		.corner_radius(12)
		.inner_margin(Margin {
			left: 14,
			right: 14,
			top: 6,
			bottom: 10,
		})
		.shadow(egui::Shadow {
			offset: [0, 20],
			blur: 50,
			spread: 0,
			color: Color32::from_black_alpha(190),
		})
}

/// A popover under `response`, opened and closed by clicking it, such as the
/// health dot's.
pub fn popover(response: &Response) -> egui::Popup<'_> {
	egui::Popup::from_toggle_button_response(response)
		.frame(popover_frame())
		.align(egui::RectAlign::BOTTOM_END)
		.gap(6.0)
		.width(316.0)
}

/// A row of a popover: what `add` draws, with a divider under it unless it
/// is the `last`.
pub fn popover_row<R>(ui: &mut Ui, last: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
	let inner = ui
		.allocate_ui_with_layout(
			vec2(ui.available_width(), 32.0),
			egui::Layout::left_to_right(egui::Align::Center),
			add,
		)
		.inner;
	if !last {
		let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
		ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, LINE));
	}
	inner
}

/// A sheet: a modal card for a task the window waits on, such as going live,
/// with its title and a close button.
pub struct Sheet<'a> {
	id: egui::Id,
	title: &'a str,
}

impl<'a> Sheet<'a> {
	pub fn new(id: egui::Id, title: &'a str) -> Self {
		Self { id, title }
	}

	/// Draw it over everything in `ctx`, returning whether it asks to close:
	/// on its close button, Escape, or a click outside it.
	pub fn show(self, ctx: &egui::Context, add: impl FnOnce(&mut Ui)) -> bool {
		let frame = Frame::new()
			.fill(P1)
			.stroke(Stroke::new(1.0, LINE))
			.corner_radius(14)
			.inner_margin(Margin::same(18))
			.shadow(egui::Shadow {
				offset: [0, 24],
				blur: 60,
				spread: 0,
				color: Color32::from_black_alpha(200),
			});
		let mut closed = false;
		let modal = egui::Modal::new(self.id)
			.frame(frame)
			.backdrop_color(BG.gamma_multiply(0.7))
			.show(ctx, |ui| {
				ui.set_width(520.0);
				ui.horizontal(|ui| {
					ui.label(super::caps(self.title, super::display(20.0)).color(TEXT));
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						closed = ui.add(IconButton::new(Icon::Close, "Close").key(Some("Esc"))).clicked();
					});
				});
				ui.add_space(8.0);
				add(ui)
			});
		closed || modal.should_close()
	}
}

/// A key on a cap, as tooltips and pads show it.
pub struct Keycap<'a> {
	key: &'a str,
	fx: Option<&'a mut Fx>,
}

impl<'a> Keycap<'a> {
	pub fn new(key: &'a str) -> Self {
		Self { key, fx: None }
	}

	/// A plastic cap drawn by `fx`, while it draws.
	pub fn fx(mut self, fx: &'a mut Fx) -> Self {
		self.fx = Some(fx);
		self
	}
}

impl Widget for Keycap<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let effects = self.fx.filter(|fx| fx.on());
		let mark = effects.is_some().then(|| Mark::new(ui.painter()));
		let font = super::mono(10.0);
		let text = ui
			.painter()
			.layout_no_wrap(self.key.into(), font, Color32::from_white_alpha(0xbb));
		let size = text.size() + vec2(10.0, 6.0);
		let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
		if ui.is_rect_visible(rect) {
			let painter = ui.painter();
			if mark.is_none() {
				painter.rect(
					rect,
					4,
					Color32::from_black_alpha(0x66),
					Stroke::new(1.0, Color32::from_white_alpha(0x22)),
					StrokeKind::Inside,
				);
			}
			painter.galley(rect.center() - text.size() / 2.0, text, TEXT);
		}
		if let (Some(fx), Some(mark)) = (effects, mark) {
			fx.keycap(&mark.around(rect));
		}
		response
	}
}

/// What a scene pad or the tally frame says about its scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tally {
	/// Not on air, nor next.
	Off,
	/// On air: the accent.
	Air,
	/// Cued, or open off air for editing: amber.
	Cue,
}

impl Tally {
	fn color(self) -> Option<Color32> {
		match self {
			Self::Off => None,
			Self::Air => Some(ACCENT),
			Self::Cue => Some(CUE),
		}
	}

	fn ink(self) -> Color32 {
		match self {
			Self::Cue => ON_CUE,
			_ => Color32::WHITE,
		}
	}
}

/// `text` laid out in capitals, spaced, in `font`, cut to `width` if given.
pub fn caps_galley(
	painter: &egui::Painter,
	text: &str,
	font: egui::FontId,
	color: Color32,
	width: Option<f32>,
) -> std::sync::Arc<egui::Galley> {
	let spacing = font.size * 0.1;
	let mut job = egui::text::LayoutJob::simple_singleline(text.to_uppercase(), font, color);
	job.sections[0].format.extra_letter_spacing = spacing;
	if let Some(width) = width {
		job.wrap = egui::text::TextWrapping::truncate_at_width(width);
	}
	painter.layout_job(job)
}

/// A tag on a picture, such as ON AIR, in its tally's color.
fn tag(painter: &egui::Painter, at: Pos2, text: &str, tally: Tally, size: f32) -> Rect {
	let color = tally.color().unwrap_or(P3);
	let galley = caps_galley(painter, text, super::display_bold(size), tally.ink(), None);
	let rect = Rect::from_min_size(at, galley.size() + vec2(size * 0.7, size * 0.3));
	painter.rect_filled(rect, 4, color);
	painter.galley(rect.center() - galley.size() / 2.0, galley, tally.ink());
	rect
}

/// A tally's tag on its own, such as ON AIR beside a scene's name in a list.
pub struct Tag<'a> {
	text: &'a str,
	tally: Tally,
}

impl<'a> Tag<'a> {
	pub fn new(text: &'a str, tally: Tally) -> Self {
		Self { text, tally }
	}
}

impl Widget for Tag<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let galley = caps_galley(
			ui.painter(),
			self.text,
			super::display_bold(11.0),
			self.tally.ink(),
			None,
		);
		let size = galley.size() + vec2(11.0 * 0.7, 11.0 * 0.3);
		let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
		response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, self.text));
		if ui.is_rect_visible(rect) {
			tag(ui.painter(), rect.min, self.text, self.tally, 11.0);
		}
		response
	}
}

/// A scene's pad: its live thumbnail, its name, its key, and whether it is
/// on air or next. A click takes it.
pub struct Pad<'a> {
	name: &'a str,
	picture: Option<egui::load::SizedTexture>,
	key: Option<&'a str>,
	tally: Tally,
	/// The tag over the tally's own, such as FIRST for the scene a stream
	/// opens on.
	tag: Option<&'a str>,
	/// Why the scene cannot be taken, if it cannot.
	refused: Option<&'a str>,
	/// What just failed on it, such as a rename.
	fault: Option<&'a str>,
	width: f32,
	/// The thumbnail's shape, width over height: the show's.
	shape: f32,
	pose: Option<Pose>,
}

impl<'a> Pad<'a> {
	pub fn new(name: &'a str) -> Self {
		Self {
			name,
			picture: None,
			key: None,
			tally: Tally::Off,
			tag: None,
			refused: None,
			fault: None,
			width: 124.0,
			shape: 16.0 / 9.0,
			pose: None,
		}
	}

	/// The scene's thumbnail, once there is one, fit to the pad whatever its
	/// shape.
	pub fn picture(mut self, picture: Option<egui::load::SizedTexture>) -> Self {
		self.picture = picture;
		self
	}

	pub fn key(mut self, key: Option<&'a str>) -> Self {
		self.key = key;
		self
	}

	pub fn tally(mut self, tally: Tally) -> Self {
		self.tally = tally;
		self
	}

	/// Tagged `tag` in its tally's color, rather than the tally's own word.
	pub fn tag(mut self, tag: Option<&'a str>) -> Self {
		self.tag = tag;
		self
	}

	/// Shown, but not takeable, for `why`. It still hears a click, such as
	/// for its menu, which its caller does not take it on.
	pub fn refused(mut self, why: Option<&'a str>) -> Self {
		self.refused = why;
		self
	}

	/// Ringed in red, saying `what` failed on hover, such as a rename.
	pub fn fault(mut self, what: Option<&'a str>) -> Self {
		self.fault = what;
		self
	}

	/// How wide it is, its picture following.
	pub fn width(mut self, width: f32) -> Self {
		self.width = width;
		self
	}

	/// The thumbnail's shape, width over height, such as a portrait show's.
	pub fn shape(mut self, shape: f32) -> Self {
		if shape.is_finite() && shape > 0.0 {
			self.shape = shape;
		}
		self
	}

	/// How tall a pad `width` wide of a show of `shape` is.
	pub fn height(width: f32, shape: f32) -> f32 {
		4.0 + ((width - 8.0) / shape).round() + 29.0
	}

	#[cfg(test)]
	pub fn pose(mut self, pose: Pose) -> Self {
		self.pose = Some(pose);
		self
	}
}

impl Widget for Pad<'_> {
	fn ui(self, ui: &mut Ui) -> Response {
		let picture = vec2(self.width - 8.0, ((self.width - 8.0) / self.shape).round());
		let size = vec2(self.width, 4.0 + picture.y + 29.0);
		let (rect, response) = ui.allocate_exact_size(size, Sense::click());
		let enabled = ui.is_enabled() && self.refused.is_none();
		response.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, self.tally == Tally::Air, self.name));
		if ui.is_rect_visible(rect) {
			let pose = self.pose.unwrap_or_else(|| Pose::of(&response));
			let painter = ui.painter();
			if let Some(color) = self.tally.color() {
				shadow(painter, rect, 10, color.gamma_multiply(0.25), 12.0);
			}
			let border = match (self.tally.color(), pose) {
				_ if self.fault.is_some() => ERR,
				(Some(color), _) => color,
				(None, Pose::Hover | Pose::Press) if self.refused.is_none() => LINE_HOVER,
				(None, _) => Color32::TRANSPARENT,
			};
			let fill = if pose == Pose::Press { P3 } else { P2 };
			painter.rect(rect, 10, fill, Stroke::new(2.0, border), StrokeKind::Inside);
			let thumb = Rect::from_min_size(rect.min + vec2(4.0, 4.0), picture);
			painter.rect_filled(thumb, 7, BG);
			if let Some(texture) = self.picture {
				let fit = texture.size * (thumb.size() / texture.size).min_elem();
				let at = Rect::from_center_size(thumb.center(), fit);
				egui::Image::new(texture).corner_radius(7).paint_at(ui, at);
			}
			if self.refused.is_some() {
				painter.rect_filled(thumb, 7, BG.gamma_multiply(0.6));
			}
			let tag_text = match self.tally {
				Tally::Off => None,
				Tally::Air => Some(self.tag.unwrap_or("On air")),
				Tally::Cue => Some(self.tag.unwrap_or("Next")),
			};
			if let Some(text) = tag_text {
				tag(painter, thumb.min + vec2(4.0, 4.0), text, self.tally, 11.0);
			}
			if let Some(key) = self.key {
				let text = painter.layout_no_wrap(key.into(), super::mono(10.0), TEXT);
				let cap = Rect::from_min_size(
					pos2(thumb.right() - 4.0 - text.size().x - 10.0, thumb.top() + 4.0),
					text.size() + vec2(10.0, 6.0),
				);
				painter.rect(
					cap,
					4,
					Color32::from_black_alpha(0xaa),
					Stroke::new(1.0, Color32::from_white_alpha(0x22)),
					StrokeKind::Inside,
				);
				painter.galley(cap.center() - text.size() / 2.0, text, Color32::from_white_alpha(0xbb));
			}
			let ink = if self.refused.is_some() { DIM } else { TEXT };
			let galley = caps_galley(painter, self.name, super::display(14.0), ink, Some(rect.width() - 16.0));
			let at = pos2(
				rect.left() + 8.0,
				thumb.bottom() + (rect.bottom() - thumb.bottom() - galley.size().y) / 2.0,
			);
			painter.galley(at, galley, TEXT);
		}
		match self.fault.or(self.refused) {
			Some(why) => tip(response, why, None),
			None => response,
		}
	}
}

/// A soft glow of `color` around `rect`, `blur` points wide.
fn shadow(painter: &egui::Painter, rect: Rect, corner: u8, color: Color32, blur: f32) {
	let shadow = egui::Shadow {
		offset: [0, 0],
		blur: blur as u8,
		spread: 0,
		color,
	};
	painter.add(shadow.as_shape(rect, CornerRadius::same(corner)));
}

/// The frame around the live picture: the accent while on air, amber and striped
/// while it shows a scene that is not, neutral off air, with a tag naming which.
pub struct TallyFrame<'a> {
	tally: Tally,
	tag: Option<(&'a str, Tally)>,
}

/// The neutral frame's outline, off air.
const NEUTRAL: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x46);

impl<'a> TallyFrame<'a> {
	pub fn new(tally: Tally) -> Self {
		Self { tally, tag: None }
	}

	/// The tag in the picture's corner, such as ON AIR.
	pub fn tag(mut self, tag: &'a str) -> Self {
		self.tag = Some((tag, self.tally));
		self
	}

	/// The tag in `tally`'s color rather than the frame's, such as OFF AIR
	/// in amber on the neutral frame.
	pub fn tag_as(mut self, tag: &'a str, tally: Tally) -> Self {
		self.tag = Some((tag, tally));
		self
	}

	/// Draw the frame around what `picture` draws, and `fx`'s on-air glow
	/// and take bursts around it while it draws.
	pub fn show(self, ui: &mut Ui, fx: Option<&mut Fx>, picture: impl FnOnce(&mut Ui) -> Response) -> Response {
		let effects = fx.filter(|fx| fx.on());
		let mark = effects.is_some().then(|| Mark::new(ui.painter()));
		let glow = ui.painter().add(Shape::Noop);
		let band = ui.painter().add(Shape::Noop);
		// Room for the outline and its glow, which a panel's edge would cut.
		let response = Frame::new().inner_margin(10).show(ui, picture).inner;
		let rect = response.rect;
		let painter = ui.painter();
		if let Some(color) = self.tally.color() {
			let outline = rect.expand(4.5);
			// Without effects, a plain glow stands in for the neon one.
			if mark.is_none() {
				let shadow = egui::Shadow {
					offset: [0, 0],
					blur: 60,
					spread: 0,
					color: color.gamma_multiply(0.2),
				};
				painter.set(glow, shadow.as_shape(outline, CornerRadius::same(12)));
			}
			if self.tally == Tally::Cue {
				// Clipped to the band around the picture: a setter's clip
				// is the one a shape keeps.
				let clip = painter.clip_rect().intersect(rect.expand(10.0));
				let stripes = stripes(rect.expand(10.0), color.gamma_multiply(0.2));
				painter.with_clip_rect(clip).set(band, Shape::Vec(stripes));
			}
			painter.rect_stroke(outline, 12, Stroke::new(3.0, color), StrokeKind::Middle);
		} else {
			painter.rect_stroke(rect.expand(4.5), 12, Stroke::new(3.0, NEUTRAL), StrokeKind::Middle);
		}
		if let Some((text, tally)) = self.tag {
			tag(painter, rect.min + vec2(12.0, 12.0), text, tally, 15.0);
		}
		if let (Some(fx), Some(mark)) = (effects, mark) {
			fx.program(&mark.around(rect));
		}
		response
	}
}

/// Diagonal stripes of `color` filling `rect`, clipped to it.
pub fn stripes(rect: Rect, color: Color32) -> Vec<Shape> {
	let mut shapes = Vec::new();
	let pitch = 24.0;
	let rot = Rot2::from_angle(-std::f32::consts::FRAC_PI_4);
	let mut x = rect.left() - rect.height();
	while x < rect.right() + pitch {
		let a = pos2(x, rect.bottom());
		let b = a + rot * vec2(rect.height() * std::f32::consts::SQRT_2, 0.0);
		shapes.push(Shape::line_segment(
			[a, b],
			Stroke::new(pitch / 2.0 / std::f32::consts::SQRT_2, color),
		));
		x += pitch;
	}
	shapes
}

/// The stage's wash: a soft light from the top center of `rect`, fading to
/// the backdrop, as a radial gradient egui cannot draw, built as a mesh.
pub fn wash(painter: &egui::Painter, rect: Rect) {
	painter.rect_filled(rect, 0, BG);
	let light = Color32::from_rgb(0x15, 0x15, 0x1d);
	let center = pos2(rect.center().x, rect.top());
	let radius = vec2(rect.width() * 0.6, rect.height() * 0.9);
	const RINGS: u32 = 24;
	const SPOKES: u32 = 48;
	let mut mesh = Mesh::default();
	mesh.colored_vertex(center, light);
	for ring in 1..=RINGS {
		let t = ring as f32 / RINGS as f32;
		// Eased, so the falloff has no visible edge.
		let color = light.lerp_to_gamma(BG, t * t * (3.0 - 2.0 * t));
		for spoke in 0..=SPOKES {
			let angle = std::f32::consts::PI * spoke as f32 / SPOKES as f32;
			let at = center + vec2(angle.cos() * radius.x, angle.sin() * radius.y) * t;
			mesh.colored_vertex(at, color);
		}
	}
	let row = SPOKES + 1;
	for spoke in 0..SPOKES {
		mesh.add_triangle(0, 1 + spoke, 2 + spoke);
	}
	for ring in 1..RINGS {
		let inner = 1 + (ring - 1) * row;
		let outer = 1 + ring * row;
		for spoke in 0..SPOKES {
			mesh.add_triangle(inner + spoke, outer + spoke, inner + spoke + 1);
			mesh.add_triangle(inner + spoke + 1, outer + spoke, outer + spoke + 1);
		}
	}
	painter.with_clip_rect(rect).add(mesh);
}

#[cfg(test)]
mod tests {
	use egui::accesskit::Role;
	use egui_kittest::Harness;
	use egui_kittest::kittest::Queryable;

	use super::*;

	/// A click puts a slider where the pointer is, and with focus the arrow
	/// keys step it, never past its end.
	#[test]
	fn a_slider_follows_the_pointer_and_the_keys() {
		let mut harness = Harness::new_ui_state(
			|ui, value: &mut f32| {
				ui.add(Slider::new(value).width(214.0));
			},
			0.0,
		);
		harness.get_by_role(Role::Slider).click();
		harness.run();
		assert!((harness.state() - 0.5).abs() < 0.01, "{}", harness.state());
		harness.get_by_role(Role::Slider).focus();
		harness.run();
		for _ in 0..30 {
			harness.key_press(egui::Key::ArrowRight);
			harness.run();
		}
		assert_eq!(*harness.state(), 1.0);
	}

	/// With focus on one of two sliders side by side, the arrow keys step it
	/// alone, rather than moving focus to the next.
	#[test]
	fn a_focused_slider_keeps_the_arrow_keys() {
		let mut harness = Harness::new_ui_state(
			|ui, values: &mut [f32; 2]| {
				ui.horizontal(|ui| {
					for value in values {
						ui.add(Slider::new(value).width(100.0));
					}
				});
			},
			[0.0; 2],
		);
		harness.get_all_by_role(Role::Slider).next().unwrap().focus();
		harness.run();
		for _ in 0..60 {
			harness.key_press(egui::Key::ArrowRight);
			harness.run();
		}
		assert_eq!(*harness.state(), [1.0, 0.0]);
	}

	/// A press on the track starts the drag at once, and its release edits
	/// nothing, so the jump to the pointer and the drag after it are one
	/// gesture to undo.
	#[test]
	fn a_press_on_a_slider_starts_its_drag() {
		// The value, and whether it ever changed outside a drag.
		let mut harness = Harness::new_ui_state(
			|ui, (value, loose): &mut (f32, bool)| {
				let response = ui.add(Slider::new(value).width(214.0));
				*loose |= response.changed() && !(response.dragged() || response.drag_stopped());
			},
			(0.0, false),
		);
		let rect = harness.get_by_role(Role::Slider).rect();
		harness.drag_at(egui::pos2(egui::lerp(rect.x_range(), 0.75), rect.center().y));
		harness.run();
		harness.hover_at(egui::pos2(egui::lerp(rect.x_range(), 0.25), rect.center().y));
		harness.run();
		harness.drop_at(egui::pos2(egui::lerp(rect.x_range(), 0.25), rect.center().y));
		harness.run();
		let (value, loose) = *harness.state();
		assert!((value - 0.25).abs() < 0.05, "{value}");
		assert!(!loose, "the press changed the value before its drag started");
	}

	/// A release in the frame the pointer last moves keeps that move, and the
	/// edit it makes belongs to the drag.
	#[test]
	fn a_slider_keeps_the_move_its_release_makes() {
		// The value, and whether it ever changed outside a drag.
		let mut harness = Harness::new_ui_state(
			|ui, (value, loose): &mut (f32, bool)| {
				let response = ui.add(Slider::new(value).width(214.0));
				*loose |= response.changed() && !(response.dragged() || response.drag_stopped());
			},
			(0.0, false),
		);
		let rect = harness.get_by_role(Role::Slider).rect();
		let at = |fraction| egui::pos2(egui::lerp(rect.x_range(), fraction), rect.center().y);
		harness.drag_at(at(0.75));
		harness.run();
		harness.hover_at(at(0.5));
		harness.drop_at(at(0.25));
		harness.run();
		let (value, loose) = *harness.state();
		assert!((value - 0.25).abs() < 0.05, "{value}");
		assert!(!loose, "the release changed the value outside its drag");
	}

	/// A drag from the field's blank middle, well past a short numeral, edits
	/// the value.
	#[test]
	fn a_number_drags_from_anywhere_on_its_field() {
		// The value, the field's rect, and whether the kit's faces, which take
		// effect a frame later, are installed.
		let mut harness = Harness::new_ui_state(
			|ui, (value, field, ready): &mut (i32, egui::Rect, bool)| {
				if !std::mem::replace(ready, true) {
					super::super::install(ui.ctx());
					return;
				}
				*field = ui
					.add(Number::new(egui::DragValue::new(value)).unit("ms").width(120.0))
					.rect;
			},
			(0, egui::Rect::NOTHING, false),
		);
		harness.run();
		let field = harness.state().1;
		let at = |fraction| egui::pos2(egui::lerp(field.x_range(), fraction), field.center().y);
		harness.drag_at(at(0.6));
		harness.run();
		harness.hover_at(at(0.8));
		harness.run();
		harness.drop_at(at(0.8));
		harness.run();
		assert!(harness.state().0 > 0, "{}", harness.state().0);
	}

	/// A toggle flips on each click, and says which way it is.
	#[test]
	fn a_toggle_flips() {
		let mut harness = Harness::new_ui_state(
			|ui, on: &mut bool| {
				ui.add(Toggle::new(on).label("Snap"));
			},
			false,
		);
		harness.get_by_label("Snap").click();
		harness.run();
		assert!(harness.state());
		harness.get_by_label("Snap").click();
		harness.run();
		assert!(!harness.state());
	}
}
