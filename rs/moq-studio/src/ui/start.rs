//! The quick start on an empty picture: a camera, a screen, a media file, or
//! a web page or text, each the scene's first layer in one click.

use egui::{Align2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::drop::Arrival;
use super::*;
use crate::task::Task;

/// A card's size at full scale, the mock-up's.
const CARD: Vec2 = vec2(168.0, 145.0);
/// Room between cards.
const GAP: f32 = 16.0;
/// The heading, the line under it, and the room around them, above the cards.
const HEAD: f32 = 120.0;

/// What a text overlay starts as: a lower third to edit in the inspector.
const TEXT: &str = "<div style=\"position: absolute; left: 64px; bottom: 64px; padding: 16px 28px; \
	border-radius: 12px; background: rgba(0, 0, 0, 0.6); color: white; \
	font: 600 48px sans-serif\">Your text</div>";

/// The quick start's own state, kept while the picture is empty.
#[derive(Default)]
pub(super) struct QuickStart {
	/// The system's file dialog for a media file, while open.
	dialog: Option<Task<Option<std::path::PathBuf>>>,
	/// The web address typed so far.
	url: String,
}

/// What a card adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Card {
	Camera,
	Screen,
	Media,
	Web,
}

impl Panels {
	/// Over `picture`, while the scene it shows has no layers, and on LIVE
	/// only off air, the quick start: four cards, each adding the scene's
	/// first layer, and a word that anything dropped or pasted on the picture
	/// lands there too.
	pub(super) fn quick_start(&mut self, ui: &mut Ui, picture: Rect) {
		let engine = self.engine.clone();
		let (layer, own) = {
			let state = engine.state.lock().unwrap();
			let Ok(layer) = self.scene(&state) else { return };
			let collection = state.collection();
			let empty = |layer: &Layer| collection.items(layer).is_none_or(<[Item]>::is_empty);
			// LIVE shows the program, not a scene open off air, and never offers
			// to change the program on air or on its way there, where one click
			// would put a device straight on the stream.
			let live = self.engine.on_air() != crate::engine::OnAir::Off || self.controls.going();
			let shown = self.page != Page::Live || (!live && self.off_air(&state).is_none());
			if !shown || !empty(&layer) || !empty(&Layer::top()) {
				return;
			}
			// Never the studio's own virtual cameras, which would feed the
			// picture back into itself.
			let own: Vec<String> = state.cameras().map(|camera| camera.id).collect();
			(layer, own)
		};
		self.devices.poll();
		if self.devices.listing.is_none()
			&& self.devices.pending.is_none()
			&& let Err(err) = self.devices.refresh(&self.runtime)
		{
			let failed = || Err(format!("cannot list devices: {err}"));
			self.devices.listing = Some(Listing {
				cameras: failed(),
				displays: failed(),
				windows: failed(),
				audio: failed(),
			});
		}
		if self.devices.pending.is_some() {
			ui.ctx().request_repaint_after(Duration::from_millis(100));
		}

		let room = picture.shrink(24.0);
		let wide = 4.0 * CARD.x + 3.0 * GAP;
		let scale = (room.width() / wide)
			.min((room.height() - HEAD) / CARD.y)
			.clamp(0.4, 1.0);
		let card = CARD * scale;
		let gap = GAP * scale;
		let row = 4.0 * card.x + 3.0 * gap;
		// The heading and its line, when they fit above the cards.
		let head = (room.height() >= HEAD + card.y).then_some(HEAD);
		let top = picture.center().y - (head.unwrap_or(0.0) + card.y) / 2.0;
		let painter = ui.painter_at(picture);
		if head.is_some() {
			painter.text(
				pos2(picture.center().x, top + 8.0),
				Align2::CENTER_TOP,
				"Start your scene in one click",
				kit::ui_strong(26.0),
				kit::TEXT,
			);
			let line = painter.layout(
				"Your show is ready and saved as you go. Pick a source below, or drop any image, video, or web \
				 address onto the picture."
					.into(),
				kit::ui(14.0),
				kit::DIM,
				row.min(480.0),
			);
			let at = pos2(picture.center().x - line.size().x / 2.0, top + 54.0);
			painter.galley(at, line, kit::DIM);
		}
		let cards_top = top + head.unwrap_or(0.0);
		let left = picture.center().x - row / 2.0;
		let cards = [Card::Camera, Card::Screen, Card::Media, Card::Web];
		for (index, kind) in cards.into_iter().enumerate() {
			let rect = Rect::from_min_size(pos2(left + index as f32 * (card.x + gap), cards_top), card);
			let (enabled, line, tip) = self.card_line(kind, &own);
			let response = draw_card(ui, rect, kind, &line, enabled, scale);
			let response = response.on_hover_text(tip.unwrap_or(line));
			match kind {
				Card::Web => self.web_popover(&response, &layer),
				_ if response.clicked() => self.quick_add(kind, &layer, &own),
				_ => {}
			}
		}
	}

	/// Whether `card` can add anything now, the line under its name, and its
	/// hover when that says more: the device it adds, or why there is none.
	fn card_line(&self, card: Card, own: &[String]) -> (bool, String, Option<String>) {
		let line = |devices: &Option<&Result<Vec<Pick>, String>>| match devices {
			None => (false, "Looking…".to_string(), None),
			Some(Ok(devices)) => match first(devices, own) {
				Some(device) => (true, device.name.clone(), None),
				None => (false, "None found".to_string(), None),
			},
			// An error is too long for the card, so it goes in the hover.
			Some(Err(err)) => (false, "Unavailable".to_string(), Some(err.clone())),
		};
		let listing = self.devices.listing.as_ref();
		match card {
			Card::Camera => line(&listing.map(|listing| &listing.cameras)),
			Card::Screen => match line(&listing.map(|listing| &listing.displays)) {
				(true, _, _) => (true, "Display or app".to_string(), None),
				_ if listing.is_some_and(|listing| display(&listing.displays).is_some()) => (
					true,
					"Main display".to_string(),
					Some("Asks for Screen Recording access first".to_string()),
				),
				not => not,
			},
			Card::Media => (self.quick.dialog.is_none(), "Image or video".to_string(), None),
			Card::Web => (true, "Web page or text".to_string(), None),
		}
	}

	/// Add what `card` adds as the first layer of `layer`, never one of the
	/// `own` cameras.
	fn quick_add(&mut self, card: Card, layer: &Layer, own: &[String]) {
		let device = |devices: &Result<Vec<Pick>, String>| {
			devices.as_ref().ok().and_then(|devices| first(devices, own)).cloned()
		};
		let listing = self.devices.listing.as_ref();
		let pick = match card {
			Card::Camera => listing.and_then(|listing| device(&listing.cameras)),
			Card::Screen => listing.and_then(|listing| display(&listing.displays)),
			Card::Media => {
				self.quick.dialog = Some(media_dialog(&self.runtime));
				return;
			}
			Card::Web => return,
		};
		let Some(pick) = pick else { return };
		self.quick_add_source(layer, &pick.name, pick.source);
	}

	/// Add `source` under `name`, numbered past any source called that, as a
	/// layer of `layer`, and select it.
	fn quick_add_source(&mut self, layer: &Layer, name: &str, source: Source) {
		let mut state = self.engine.state.lock().unwrap();
		let name = state.collection().unused_name(name);
		let result = add(&mut state, Some(layer.clone()), name, source, None);
		drop(state);
		match result {
			Ok(name) => self.added(Ok(name)),
			Err(err) => self.report(Err(err.context("cannot add the source"))),
		}
	}

	/// The Web / Text card's popover: a web address to show, or a text over
	/// the picture.
	fn web_popover(&mut self, response: &egui::Response, layer: &Layer) {
		// A page to add, or `None` for text.
		let mut asked: Option<Option<url::Url>> = None;
		kit::popover(response).align(egui::RectAlign::BOTTOM).show(|ui| {
			ui.set_width(280.0);
			ui.label(egui::RichText::new("A web page").color(kit::DIM));
			let field = ui.add(
				egui::TextEdit::singleline(&mut self.quick.url)
					.hint_text("https://")
					.desired_width(f32::INFINITY),
			);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			let url = url::Url::parse(self.quick.url.trim())
				.ok()
				.filter(|url| matches!(url.scheme(), "http" | "https"));
			ui.add_space(6.0);
			ui.horizontal(|ui| {
				let add = ui.add_enabled(url.is_some(), kit::button(Icon::Web, "Add the page"));
				if let Some(url) = url.filter(|_| add.clicked() || entered) {
					asked = Some(Some(url));
				}
				if ui.add(kit::button(Icon::Text, "Add text")).clicked() {
					asked = Some(None);
				}
			});
		});
		// A page lands as one pasted on the picture does.
		match asked {
			Some(Some(url)) => {
				self.quick.url.clear();
				self.dropping.arrive(Arrival::Page(url), None);
			}
			Some(None) => {
				let canvas = {
					let state = self.engine.state.lock().unwrap();
					let collection = state.collection();
					collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas)
				};
				let text = Source::Overlay {
					html: TEXT.into(),
					width: canvas.width,
					height: canvas.height,
					restart: true,
				};
				self.quick_add_source(layer, "Text", text);
			}
			None => {}
		}
	}

	/// Land the media file the dialog picked on the picture, as a drop there,
	/// whatever the window shows by then.
	pub(super) fn follow_media_dialog(&mut self, ctx: &egui::Context) {
		let Some(dialog) = &mut self.quick.dialog else { return };
		let Some(picked) = dialog.poll() else {
			ctx.request_repaint_after(Duration::from_millis(100));
			return;
		};
		if let Some(path) = picked.clone() {
			self.dropping.arrive(Arrival::File(path), None);
		}
		self.quick.dialog = None;
	}
}

/// The first of `devices` that is none of the `own` cameras.
fn first<'a>(devices: &'a [Pick], own: &[String]) -> Option<&'a Pick> {
	devices
		.iter()
		.find(|pick| !matches!(&pick.source, Source::Camera { id: Some(id) } if own.contains(id)))
}

/// The display the Screen card adds: the first listed, or on macOS, the main
/// display when none is, since listing them needs the Screen Recording access
/// that only opening one asks for.
fn display(displays: &Result<Vec<Pick>, String>) -> Option<Pick> {
	let listed = displays.as_ref().ok().and_then(|displays| displays.first()).cloned();
	listed.or_else(|| {
		cfg!(target_os = "macos").then(|| Pick {
			name: "Display".into(),
			source: Source::Display { id: None },
		})
	})
}

/// The system's dialog for an image or video file.
fn media_dialog(runtime: &tokio::runtime::Handle) -> Task<Option<std::path::PathBuf>> {
	let dialog = Task::spawn(runtime, || async {
		let picked = rfd::AsyncFileDialog::new()
			.set_title("Add an image or video")
			.pick_file()
			.await;
		picked.map(|file| file.path().to_path_buf())
	});
	dialog.unwrap_or_else(|err| {
		tracing::warn!(%err, "cannot open the file dialog");
		Task::Done(None)
	})
}

/// Draw `card` in `rect`: its icon, its name, and `line` under it, at
/// `scale`; dimmed unless `enabled`.
fn draw_card(ui: &mut Ui, rect: Rect, card: Card, line: &str, enabled: bool, scale: f32) -> egui::Response {
	let (icon, name) = match card {
		Card::Camera => (Icon::Camera, "Camera"),
		Card::Screen => (Icon::Display, "Screen"),
		Card::Media => (Icon::Image, "Media"),
		Card::Web => (Icon::Web, "Web / Text"),
	};
	let sense = match enabled {
		true => Sense::click(),
		false => Sense::hover(),
	};
	let response = ui.interact(rect, ui.id().with(("quick start", name)), sense);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
	// The camera is the first a broadcaster adds, so it leads.
	let lit = enabled && (response.hovered() || card == Card::Camera);
	let painter = ui.painter();
	let (fill, stroke) = match lit {
		true => (kit::ACCENT.gamma_multiply(0.12), kit::ACCENT.gamma_multiply(0.7)),
		false => (kit::P1, kit::LINE),
	};
	let radius = (12.0 * scale) as u8;
	painter.rect(rect, radius, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
	let ink = if enabled { kit::TEXT } else { kit::FAINT };
	let well = Rect::from_center_size(
		pos2(rect.center().x, rect.top() + 44.0 * scale),
		Vec2::splat(48.0 * scale),
	);
	let well_fill = if lit && card == Card::Camera {
		kit::ACCENT
	} else {
		kit::P3
	};
	painter.rect_filled(well, (10.0 * scale) as u8, well_fill);
	icon.paint(
		ui,
		Rect::from_center_size(well.center(), Vec2::splat(20.0 * scale)),
		20.0 * scale,
		ink,
	);
	let painter = ui.painter_at(rect.shrink(6.0));
	painter.text(
		pos2(rect.center().x, well.bottom() + 16.0 * scale),
		Align2::CENTER_TOP,
		name,
		kit::ui_strong(15.0 * scale.max(0.7)),
		ink,
	);
	painter.text(
		pos2(rect.center().x, well.bottom() + 42.0 * scale),
		Align2::CENTER_TOP,
		line,
		kit::ui(12.5 * scale.max(0.75)),
		kit::DIM,
	);
	response
}
