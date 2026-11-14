//! Every widget of the kit in every state, for `just shots kit` and
//! `just shots kit_sheet`: the page a change to the look is judged on.

use egui::{Color32, RichText, Ui, vec2};

use super::{
	AIR, BG, CUE, DIM, FAINT, Fader, Icon, IconButton, Keycap, LINE, Look, Meter, Number, OK, P1, P2, P3, POPOVER, Pad,
	Pill, Pose, Run, SEL, Sheet, Slider, TEXT, Tag, Tally, TallyFrame, Toggle,
};
use crate::fx::Fx;

/// The gallery as a window of its own.
pub struct Gallery {
	pub fx: Fx,
	/// Whether a sheet is open over it.
	pub sheet: bool,
	picture: Option<egui::TextureHandle>,
}

impl Gallery {
	pub fn new(fx: Fx, sheet: bool) -> Self {
		Self {
			fx,
			sheet,
			picture: None,
		}
	}
}

impl eframe::App for Gallery {
	fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
		self.fx.begin(ui.ctx());
		// A take long ago, on air: the glow lit and no burst in flight.
		self.fx.follow(true, 0, crate::health::Verdict::Healthy);
		let picture = self
			.picture
			.get_or_insert_with(|| ui.ctx().load_texture("gallery", sunset(), egui::TextureOptions::LINEAR))
			.id();
		egui::CentralPanel::default()
			.frame(egui::Frame::new().fill(BG).inner_margin(20))
			.show(ui, |ui| show(ui, &mut self.fx, picture, !self.sheet));
		if self.sheet {
			let _ = Sheet::new(egui::Id::new("gallery sheet"), "Go live").show(ui.ctx(), |ui| {
				ui.label("Where the stream goes, and what it carries.");
				ui.add_space(8.0);
				ui.horizontal(|ui| {
					ui.add(Pill::new("Relay").dot(OK).value("6.0 Mb/s"));
					ui.add(Pill::new("RTMP 2").dot(CUE).value("reconnecting"));
				});
				ui.add_space(8.0);
				let mut on = true;
				ui.add(Toggle::new(&mut on).label("Record while live"));
			});
		}
		self.fx.finish();
	}
}

/// A picture to stand in for a scene's: a dusk sky over the horizon.
fn sunset() -> egui::ColorImage {
	let (width, height) = (160, 90);
	let top = Color32::from_rgb(0x1b, 0x0b, 0x3a);
	let middle = Color32::from_rgb(0x5a, 0x1f, 0x6e);
	let glow = Color32::from_rgb(0xff, 0x7a, 0x3d);
	let ground = Color32::from_rgb(0x12, 0x06, 0x1f);
	let pixels = (0..height)
		.flat_map(|y| {
			let t = y as f32 / height as f32;
			let color = match t {
				t if t < 0.38 => top.lerp_to_gamma(middle, t / 0.38),
				t if t < 0.55 => middle.lerp_to_gamma(glow, (t - 0.38) / 0.17),
				_ => ground,
			};
			std::iter::repeat_n(color, width)
		})
		.collect();
	egui::ColorImage::new([width, height], pixels)
}

/// A section's caption.
fn caption(ui: &mut Ui, text: &str) {
	ui.add_space(6.0);
	ui.label(super::caps(text, super::display(11.0)).color(DIM));
}

/// Draw every widget, with a tooltip and a popover open if `overlays`.
fn show(ui: &mut Ui, fx: &mut Fx, picture: egui::TextureId, overlays: bool) {
	ui.columns_const(|[left, middle, right]| {
		tokens(left);
		type_scale(left);
		icons(left);
		buttons(left);
		controls(middle, fx);
		pads(middle, picture);
		stage(right, fx, picture);
		if overlays {
			self::overlays(right);
		}
	});
}

fn tokens(ui: &mut Ui) {
	caption(ui, "Colors");
	let swatches = [
		("bg", BG),
		("p1", P1),
		("p2", P2),
		("p3", P3),
		("line", LINE),
		("pop", POPOVER),
		("text", TEXT),
		("dim", DIM),
		("faint", FAINT),
		("air", AIR),
		("cue", CUE),
		("ok", OK),
		("sel", SEL),
	];
	ui.horizontal_wrapped(|ui| {
		ui.spacing_mut().item_spacing.x = 6.0;
		for (name, color) in swatches {
			ui.vertical(|ui| {
				let (rect, _) = ui.allocate_exact_size(vec2(26.0, 22.0), egui::Sense::hover());
				ui.painter()
					.rect(rect, 5, color, egui::Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
				ui.label(RichText::new(name).font(super::mono(9.0)).color(DIM));
			});
		}
	});
}

fn type_scale(ui: &mut Ui) {
	caption(ui, "Type");
	ui.horizontal(|ui| {
		ui.label(super::caps("On air", super::display_bold(22.0)).color(AIR));
		ui.label(super::caps("Gameplay", super::display(16.0)).color(TEXT));
		ui.label(RichText::new("01:24:07").font(super::mono(15.0)).color(TEXT));
	});
	ui.label(RichText::new("Friday Night Ranked").font(super::ui_strong(14.0)));
	ui.label(RichText::new("Desk mic").font(super::ui_medium(12.0)));
	ui.label("The relay is reachable and the encoder keeps up.");
	ui.label(
		RichText::new("6.0 Mb/s · 18 ms · 0 dropped")
			.font(super::mono(12.0))
			.color(DIM),
	);
	ui.label(RichText::new("Ünïcødé · Привет · Γειά").color(DIM));
}

fn icons(ui: &mut Ui) {
	caption(ui, "Icons");
	ui.horizontal_wrapped(|ui| {
		ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
		for icon in Icon::ALL {
			ui.add(icon.image(18.0, TEXT));
		}
	});
}

fn buttons(ui: &mut Ui) {
	caption(ui, "Icon buttons: rest, hover, press, disabled");
	for look in [Look::Plain, Look::Boxed, Look::Cue] {
		ui.horizontal(|ui| {
			for pose in [Pose::Rest, Pose::Hover, Pose::Press] {
				ui.add(IconButton::new(Icon::Panic, "Panic").look(look).pose(pose));
			}
			ui.add_enabled(false, IconButton::new(Icon::Panic, "Panic").look(look).pose(Pose::Off));
		});
	}
	caption(ui, "Lit, small");
	ui.horizontal(|ui| {
		ui.add(IconButton::new(Icon::Muted, "Unmute").lit(true, CUE).small());
		ui.add(IconButton::new(Icon::Sound, "Mute").look(Look::Boxed).small());
		ui.add(IconButton::new(Icon::Up, "Move up").small());
		ui.add(IconButton::new(Icon::Remove, "Remove").small().pose(Pose::Hover));
	});
	caption(ui, "Pills");
	ui.horizontal_wrapped(|ui| {
		ui.add(Pill::new("Excellent").dot(OK));
		ui.add(Pill::new("Degraded").dot(CUE));
		ui.add(Pill::new("Encoder").value("3.1 ms"));
	});
	caption(ui, "Tags");
	ui.horizontal(|ui| {
		ui.add(Tag::new("On air", Tally::Air));
		ui.add(Tag::new("Next", Tally::Cue));
		ui.add(Tag::new("Audio", Tally::Off));
	});
}

fn controls(ui: &mut Ui, fx: &mut Fx) {
	caption(ui, "Toggles: off, on, disabled");
	ui.horizontal(|ui| {
		ui.add(Toggle::new(&mut false).label("Snap"));
		ui.add(Toggle::new(&mut true).label("Grid"));
		ui.add_enabled(false, Toggle::new(&mut true).label("Loop"));
	});
	caption(ui, "Sliders");
	ui.add(Slider::new(&mut 0.25).width(220.0));
	ui.add(Slider::new(&mut 0.7).width(220.0));
	ui.add_enabled(false, Slider::new(&mut 0.5).width(220.0));
	caption(ui, "Faders and meters");
	ui.horizontal(|ui| {
		for (value, level) in [(0.75, 0.6), (0.5, 0.92), (0.0, 0.0)] {
			ui.add(Fader::new(&mut { value }).height(150.0));
			ui.add(Meter::new(level, level + 0.04).run(Run::Up, 150.0, 9.0));
			ui.add_space(6.0);
		}
		ui.add(Meter::new(0.58, 0.66).run(Run::Up, 150.0, 22.0));
	});
	ui.add_space(4.0);
	for (level, peak) in [(0.0, 0.0), (0.38, 0.45), (0.64, 0.7), (0.92, 1.0)] {
		ui.add(Meter::new(level, peak).run(Run::Across, 220.0, 9.0));
	}
	caption(ui, "Numbers that drag");
	ui.horizontal(|ui| {
		ui.add(Number::new(egui::DragValue::new(&mut 1920)).label("W").width(80.0));
		ui.add(Number::new(egui::DragValue::new(&mut 1080)).label("H").width(80.0));
		ui.add(Number::new(egui::DragValue::new(&mut 300)).unit("ms").width(80.0));
	});
	caption(ui, "Keycaps: flat, and drawn by the effects");
	ui.horizontal(|ui| {
		ui.add(Keycap::new("F1"));
		ui.add(Keycap::new("Ctrl Z"));
		ui.add(Keycap::new("F2").fx(fx));
		ui.add(Keycap::new("Shift F5").fx(fx));
	});
}

fn pads(ui: &mut Ui, picture: egui::TextureId) {
	let picture = egui::load::SizedTexture::new(picture, vec2(160.0, 90.0));
	caption(ui, "Scene pads: rest, hover, on air, next, refused, no picture yet");
	ui.horizontal_wrapped(|ui| {
		ui.add(Pad::new("Starting soon").picture(Some(picture)).key(Some("F1")));
		ui.add(
			Pad::new("Gameplay")
				.picture(Some(picture))
				.key(Some("F2"))
				.pose(Pose::Hover),
		);
		ui.add(
			Pad::new("Just chatting")
				.picture(Some(picture))
				.key(Some("F3"))
				.tally(Tally::Air),
		);
		ui.add(
			Pad::new("Guest duo")
				.picture(Some(picture))
				.key(Some("F4"))
				.tally(Tally::Cue),
		);
		ui.add(
			Pad::new("Vertical cut")
				.picture(Some(picture))
				.refused(Some("Another size than the program's")),
		);
		ui.add(Pad::new("A scene with a long name").key(Some("F6")));
	});
}

fn stage(ui: &mut Ui, fx: &mut Fx, picture: egui::TextureId) {
	caption(ui, "Stage wash, and the tally frame on air");
	let height = 190.0;
	let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
	super::wash(ui.painter(), rect);
	let mut stage = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(18.0)));
	let size = vec2(stage.available_height() * 16.0 / 9.0, stage.available_height());
	stage.vertical_centered(|ui| {
		TallyFrame::new(Tally::Air).tag("On air").show(ui, Some(fx), |ui| {
			ui.add(egui::Image::new((picture, size)).corner_radius(10))
		});
	});
	caption(ui, "Off air, and plain");
	ui.horizontal(|ui| {
		let size = vec2(150.0, 84.0);
		for (tally, tag) in [(Tally::Cue, "Off air"), (Tally::Off, "Preview")] {
			ui.add_space(14.0);
			let mut frame = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
				ui.cursor().min + vec2(0.0, 12.0),
				size + vec2(24.0, 24.0),
			)));
			TallyFrame::new(tally).tag(tag).show(&mut frame, None, |ui| {
				ui.add(egui::Image::new((picture, size)).corner_radius(10))
			});
			ui.allocate_space(size + vec2(24.0, 24.0));
		}
	});
}

fn overlays(ui: &mut Ui) {
	caption(ui, "Tooltip, and a popover");
	ui.horizontal(|ui| {
		let undo = ui.add(IconButton::new(Icon::Undo, "Undo").pose(Pose::Hover));
		super::widgets::tooltip(&undo, true, "Undo", Some("Ctrl Z"), Some("Undo: move the bar"));
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			let health = ui.add(Pill::new("Excellent").dot(OK).clickable());
			popover(&health);
		});
	});
}

/// The health pill's popover, open.
fn popover(health: &egui::Response) {
	super::popover(health).open(true).show(|ui| {
		let rows = [
			(OK, "Relay", "6.0 Mb/s · 18 ms"),
			(OK, "RTMP 1", "6.0 Mb/s"),
			(CUE, "RTMP 2 reconnecting", "0.0 Mb/s"),
		];
		for (index, (color, name, value)) in rows.into_iter().enumerate() {
			super::popover_row(ui, index + 1 == rows.len(), |ui| {
				let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
				ui.painter().circle_filled(dot.center(), 4.0, color);
				ui.label(name);
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					ui.label(RichText::new(value).font(super::mono(12.0)));
				});
			});
		}
	});
}
