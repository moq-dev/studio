//! The studio's look: the tokens every page draws with (colors, faces, sizes),
//! its icons, and its own widgets, drawn as the mock-ups in
//! `quest/m0/ux/mock/kit.css` draw them.
//!
//! Widgets keep no state of their own: each takes the value it shows and
//! edits from its caller, so a page can draw from a snapshot of the engine.

use egui::{Color32, FontFamily, FontId, RichText};

mod fonts;
#[cfg(test)]
pub(crate) mod gallery;
mod icon;
mod widgets;

pub use icon::Icon;
pub use widgets::*;

/// The window's backdrop, behind every surface.
pub const BG: Color32 = Color32::from_rgb(0x09, 0x09, 0x0c);
/// A panel: the title bar, the dock, the side columns.
pub const P1: Color32 = Color32::from_rgb(0x11, 0x11, 0x16);
/// A control on a panel: a button, a field, a pad.
pub const P2: Color32 = Color32::from_rgb(0x18, 0x18, 0x1f);
/// A control raised or picked: a hovered button, the page shown, a track.
pub const P3: Color32 = Color32::from_rgb(0x22, 0x22, 0x2b);
/// Every border and divider.
pub const LINE: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x35);
/// A popover or a menu, over the panels.
pub const POPOVER: Color32 = Color32::from_rgb(0x17, 0x17, 0x1e);

/// Text, at full strength.
pub const TEXT: Color32 = Color32::from_rgb(0xf3, 0xf3, 0xf6);
/// Text that labels rather than says: captions, inactive pages, units.
pub const DIM: Color32 = Color32::from_rgb(0x8d, 0x8d, 0x9c);
/// Text that hints: placeholders, unlit segments' numbers.
pub const FAINT: Color32 = Color32::from_rgb(0x5b, 0x5b, 0x69);

/// The studio's own color: what is on air (the tally, its tag, the live
/// control), the layer picked, a toggle on, a slider's fill. Branding changes
/// this one value.
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xff);
/// An error, and nothing else: a failure, a clipping meter.
pub const ERR: Color32 = Color32::from_rgb(0xff, 0x2d, 0x46);
/// Everything not on air that is not an error: cued, off air, muted,
/// degraded, or needing attention.
pub const CUE: Color32 = Color32::from_rgb(0xff, 0xb0, 0x20);
/// Text on [`CUE`], which white does not read on.
pub const ON_CUE: Color32 = Color32::from_rgb(0x1a, 0x12, 0x00);
/// A healthy level or link.
pub const OK: Color32 = Color32::from_rgb(0x35, 0xd0, 0x7f);

/// The corner of a control: a button, a field, a pad's picture.
pub const RADIUS: u8 = 8;

/// The condensed face, for state and names: ON AIR, scene names, page names.
pub fn display(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name(fonts::DISPLAY.into()))
}

/// The condensed face at its boldest, for the tally's tag and the live control.
pub fn display_bold(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name(fonts::DISPLAY_BOLD.into()))
}

/// The plain face, at its regular weight.
pub fn ui(size: f32) -> FontId {
	FontId::proportional(size)
}

/// The plain face, at medium weight, for buttons and names in a list.
pub fn ui_medium(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name(fonts::MEDIUM.into()))
}

/// The plain face, at semibold weight, for a title.
pub fn ui_strong(size: f32) -> FontId {
	FontId::new(size, FontFamily::Name(fonts::STRONG.into()))
}

/// The monospace, for clocks, rates, and keys, whose digits keep their place.
pub fn mono(size: f32) -> FontId {
	FontId::monospace(size)
}

/// `text` in capitals in the condensed face, spaced as the mock-ups space
/// state and names.
pub fn caps(text: impl AsRef<str>, font: FontId) -> RichText {
	let spacing = font.size * 0.1;
	RichText::new(text.as_ref().to_uppercase())
		.font(font)
		.extra_letter_spacing(spacing)
}

/// Give `ctx` the studio's faces, icons, and look, dark whatever the system's
/// theme is.
pub fn install(ctx: &egui::Context) {
	ctx.set_fonts(fonts::definitions());
	egui_extras::install_image_loaders(ctx);
	ctx.set_theme(egui::ThemePreference::Dark);
	ctx.set_style_of(egui::Theme::Dark, style());
}

/// egui's own widgets, such as a text field or a menu, in the kit's look.
fn style() -> egui::Style {
	use egui::{CornerRadius, Margin, Shadow, Stroke, TextStyle};

	let mut style = egui::Style {
		text_styles: [
			(TextStyle::Small, ui(11.0)),
			(TextStyle::Body, ui(13.0)),
			(TextStyle::Monospace, mono(12.0)),
			(TextStyle::Button, ui_medium(12.5)),
			(TextStyle::Heading, display(18.0)),
		]
		.into(),
		..Default::default()
	};

	let spacing = &mut style.spacing;
	spacing.item_spacing = egui::vec2(8.0, 6.0);
	spacing.button_padding = egui::vec2(10.0, 4.0);
	spacing.interact_size.y = 24.0;
	spacing.window_margin = Margin::same(12);
	spacing.menu_margin = Margin::same(6);
	spacing.icon_width = 16.0;
	spacing.icon_width_inner = 10.0;
	spacing.slider_rail_height = 4.0;

	let visuals = &mut style.visuals;
	*visuals = egui::Visuals::dark();
	visuals.panel_fill = P1;
	visuals.window_fill = POPOVER;
	visuals.window_stroke = Stroke::new(1.0, LINE);
	visuals.window_corner_radius = CornerRadius::same(12);
	visuals.menu_corner_radius = CornerRadius::same(10);
	visuals.window_shadow = Shadow {
		offset: [0, 16],
		blur: 40,
		spread: 0,
		color: Color32::from_black_alpha(170),
	};
	visuals.popup_shadow = Shadow {
		offset: [0, 10],
		blur: 30,
		spread: 0,
		color: Color32::from_black_alpha(170),
	};
	visuals.extreme_bg_color = BG;
	visuals.text_edit_bg_color = Some(P2);
	visuals.faint_bg_color = P2;
	visuals.code_bg_color = P2;
	visuals.hyperlink_color = ACCENT;
	visuals.warn_fg_color = CUE;
	visuals.error_fg_color = ERR;
	visuals.weak_text_color = Some(DIM);
	visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
	visuals.selection.stroke = Stroke::new(1.0, ACCENT);
	visuals.slider_trailing_fill = true;
	visuals.handle_shape = egui::style::HandleShape::Circle;
	visuals.striped = false;
	visuals.indent_has_left_vline = false;

	let radius = CornerRadius::same(RADIUS);
	let widgets = &mut visuals.widgets;
	widgets.noninteractive.bg_fill = P1;
	widgets.noninteractive.weak_bg_fill = P1;
	widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
	widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
	widgets.noninteractive.corner_radius = radius;
	for (state, fill, stroke) in [
		(&mut widgets.inactive, P2, LINE),
		(&mut widgets.hovered, P3, Color32::from_rgb(0x3a, 0x3a, 0x48)),
		(&mut widgets.active, P3, ACCENT),
		(&mut widgets.open, P3, LINE),
	] {
		state.weak_bg_fill = fill;
		// A checkbox's box, a slider's rail: a step above the control's fill.
		state.bg_fill = P3;
		state.bg_stroke = Stroke::new(1.0, stroke);
		state.fg_stroke = Stroke::new(1.5, TEXT);
		state.corner_radius = radius;
		state.expansion = 0.0;
	}
	widgets.inactive.fg_stroke = Stroke::new(1.5, TEXT);
	style
}
