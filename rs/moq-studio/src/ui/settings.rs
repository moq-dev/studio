//! Settings: what a broadcaster sets once, in one window over the pages, a
//! pane per category; and the edit history, under the title bar's undo.
//!
//! Each setting keeps its home on disk: the scene file for what belongs to
//! the show, this user's settings for what belongs to the operator, and
//! `<show>.stream.toml` for where it streams and who joins it.

use std::collections::HashSet;
use std::path::PathBuf;

use egui::{Align, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use super::live::Saving;
use super::*;
use crate::setup::Task;
use crate::stream::Saved;

/// The window's size, as the mock-up draws it, or less in a smaller window.
const WINDOW: Vec2 = vec2(960.0, 664.0);

/// The categories' column.
const NAV: f32 = 216.0;

/// The categories' column as icons alone, in a window too narrow for the
/// names beside a pane wide enough for its controls.
const NAV_ICONS: f32 = 60.0;

/// The pane's space each side of its rows.
const MARGIN: f32 = 32.0;

/// The share of a row's width its name takes, beside its control.
const NAME: f32 = 0.42;

/// A format card's size, and the space between the two.
const CARD: Vec2 = vec2(176.0, 76.0);
const CARD_GAP: f32 = 10.0;

/// The narrowest pane whose rows keep their control beside their name: the
/// widest control, the two format cards, then still fits. Narrower, each
/// row puts its control under its name.
fn stack(ui: &Ui) -> f32 {
	(2.0 * CARD.x + CARD_GAP + ui.spacing().item_spacing.x) / (1.0 - NAME)
}

/// The canvas sizes offered by name, by their short side, each 16:9; any
/// other is Custom.
const SIZES: [(u32, &str); 4] = [(720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "4K")];

/// The frame rates offered by name; any other is Custom.
const RATES: [u32; 2] = [30, 60];

/// How long a join link may last, offered by name.
const LIFETIMES: [(Duration, &str); 4] = [
	(Duration::from_secs(3600), "1 hour"),
	(Duration::from_secs(4 * 3600), "4 hours"),
	(Duration::from_secs(12 * 3600), "12 hours"),
	(Duration::from_secs(24 * 3600), "1 day"),
];

/// A change that restarts encoders, which waits for a yes while on air.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Restart {
	Canvas(Canvas),
	/// The scene of this name, published, to this size or the program's: by
	/// name, as an undo or reload while the restart waits may move it.
	Scene(String, Option<moq_scene::Size>),
}

/// What a system file dialog is picking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Picking {
	/// The recordings folder.
	Folder,
	/// The guests' room key.
	Key,
}

/// A system file dialog, open, and what it picks for.
pub(super) struct Picker {
	what: Picking,
	task: Task<Option<PathBuf>>,
}

/// The Guests pane's form, until saved.
pub(super) struct GuestForm {
	room: String,
	key: String,
	page: String,
	expiry: Duration,
	appear: bool,
}

impl GuestForm {
	/// The form holding what `saved` holds, not what the flags set.
	pub(super) fn new(saved: &Saved) -> Self {
		Self {
			room: saved.room.clone().unwrap_or_default(),
			key: saved
				.room_key
				.as_ref()
				.map(|key| key.display().to_string())
				.unwrap_or_default(),
			page: saved.join_page.as_ref().map(ToString::to_string).unwrap_or_default(),
			expiry: saved.guest_expiry.unwrap_or(crate::guests::DEFAULT_EXPIRY),
			appear: saved.guests_appear,
		}
	}

	/// What the form would save over `saved`, or why it cannot.
	fn saved(&self, saved: &Saved) -> Result<Saved, String> {
		let text = |text: &str| Some(text.trim().to_string()).filter(|text| !text.is_empty());
		let page = match text(&self.page) {
			None => None,
			Some(page) => Some(
				page.parse()
					.map_err(|err| format!("The join page is not a URL: {err}."))?,
			),
		};
		let expiry = (self.expiry != crate::guests::DEFAULT_EXPIRY).then_some(self.expiry);
		Ok(Saved {
			room: text(&self.room),
			room_key: text(&self.key).map(PathBuf::from),
			join_page: page,
			guest_expiry: expiry,
			guests_appear: self.appear,
			..saved.clone()
		})
	}

	/// Whether the form holds anything to save over `saved`.
	fn edited(&self, saved: &Saved) -> bool {
		self.saved(saved).is_ok_and(|form| form != *saved) || self.saved(saved).is_err()
	}
}

impl Panels {
	/// The Settings window, while open: the categories down the left, and
	/// the one picked on the right.
	pub(super) fn settings(&mut self, ctx: &egui::Context, fx: &mut Fx) {
		self.follow_picker(ctx);
		let Some(mut setting) = self.setting else { return };
		let frame = egui::Frame::new()
			.fill(kit::P1)
			.stroke(Stroke::new(1.0, egui::Color32::from_rgb(0x3b, 0x3b, 0x4a)))
			.corner_radius(16)
			.shadow(egui::Shadow {
				offset: [0, 40],
				blur: 120,
				spread: 0,
				color: egui::Color32::BLACK,
			});
		let size = WINDOW.min(ctx.content_rect().size() - Vec2::splat(24.0));
		let mut closed = false;
		let modal = egui::Modal::new(egui::Id::new("settings"))
			.frame(frame)
			.backdrop_color(egui::Color32::from_rgba_unmultiplied(6, 6, 11, 0xc7))
			.show(ctx, |ui| {
				ui.set_min_size(size);
				ui.set_max_size(size);
				let rect = ui.max_rect();
				let icons = rect.width() - NAV - 2.0 * MARGIN < stack(ui);
				let nav = Rect::from_min_size(rect.min, vec2(if icons { NAV_ICONS } else { NAV }, rect.height()));
				let pane = Rect::from_min_max(pos2(nav.right(), rect.top()), rect.max);
				self.settings_nav(ui, nav, icons, &mut setting);
				self.setting = Some(setting);
				let builder = egui::UiBuilder::new().max_rect(pane.shrink2(vec2(MARGIN, 0.0)));
				ui.scope_builder(builder, |ui| {
					ui.add_space(24.0);
					ui.horizontal(|ui| {
						ui.label(kit::caps(setting.name(), kit::display_bold(30.0)).color(kit::TEXT));
						ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
							closed = ui.add(IconButton::new(Icon::Close, "Close").key(Some("Esc"))).clicked();
						});
					});
					ui.add_space(6.0);
					ui.label(RichText::new(setting.lead()).font(kit::ui(12.5)).color(kit::DIM));
					ui.add_space(10.0);
					egui::ScrollArea::vertical()
						.id_salt(("settings pane", setting.name()))
						.auto_shrink([false, false])
						.show(ui, |ui| match setting {
							Setting::Show => self.show_pane(ui),
							Setting::Stream => self.stream_settings(ui),
							Setting::Recording => self.recording(ui),
							Setting::Hotkeys => self.hotkeys(ui, fx),
							Setting::Guests => self.guest_settings(ui),
							Setting::Remote => self.remote_settings(ui),
							Setting::Advanced => self.advanced(ui, fx),
						});
					// A pane closes the window by clearing it, as the scene
					// graph's Open does, or opens another by naming it, as
					// Remote's Set up the room does.
					match self.setting {
						None => closed = true,
						Some(asked) => setting = asked,
					}
				});
			});
		// Escape while a key is being bound cancels the binding instead.
		let escaped = modal.should_close() && self.binding.is_none();
		self.setting = (!closed && !escaped).then_some(setting);
		if self.setting.is_none() {
			self.restart = None;
		}
		self.confirm_restart(ctx);
	}

	/// Close Settings, dropping a restart it was asking about.
	pub(super) fn close_settings(&mut self) {
		self.setting = None;
		self.restart = None;
	}

	/// The categories, the one shown lit: by name, or by `icons` alone.
	fn settings_nav(&mut self, ui: &mut Ui, rect: Rect, icons: bool, setting: &mut Setting) {
		let painter = ui.painter();
		let fill = egui::Color32::from_rgb(0x0c, 0x0c, 0x11);
		painter.rect_filled(
			rect,
			egui::CornerRadius {
				nw: 16,
				sw: 16,
				ne: 0,
				se: 0,
			},
			fill,
		);
		painter.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, kit::LINE));
		let mut y = rect.top() + 22.0;
		if !icons {
			let title = kit::caps_galley(painter, "Settings", kit::display_bold(22.0), kit::TEXT, None);
			painter.galley(pos2(rect.left() + 20.0, y), title, kit::TEXT);
			y += 36.0;
		}
		for each in Setting::ALL {
			let at = Rect::from_min_size(pos2(rect.left() + 10.0, y), vec2(rect.width() - 20.0, 38.0));
			y += 40.0;
			let mut response = ui.interact(at, ui.id().with(("setting", each.name())), Sense::click());
			if icons {
				response = response.on_hover_text(each.name());
			}
			let on = *setting == each;
			response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, each.name()));
			if on {
				ui.painter().rect_filled(at, 9, kit::P3);
			}
			let ink = if on || response.hovered() { kit::TEXT } else { kit::DIM };
			let icon = Rect::from_center_size(pos2(at.left() + 20.0, at.center().y), Vec2::splat(16.0));
			each.icon().paint(ui, icon, 16.0, ink);
			if !icons {
				let name = ui
					.painter()
					.layout_no_wrap(each.name().into(), kit::ui_strong(13.5), ink);
				ui.painter()
					.galley(pos2(at.left() + 38.0, at.center().y - name.size().y / 2.0), name, ink);
			}
			if response.clicked() {
				*setting = each;
			}
		}
	}

	/// Settings, Show: the format, the canvas size and frame rate by name or
	/// by number, and each scene's own size.
	fn show_pane(&mut self, ui: &mut Ui) {
		let (current, scenes, both) = {
			let state = self.engine.state.lock().unwrap();
			let collection = state.collection();
			(collection.canvas, collection.scenes.clone(), collection.portrait)
		};
		let shown = self.canvas.unwrap_or(current);
		let mut wanted = None;

		let portrait = current.height > current.width;
		row(
			ui,
			"Format",
			"The shape of the show's picture. The layout of this window follows it.",
			|ui| {
				ui.spacing_mut().item_spacing.x = CARD_GAP;
				// Right to left, so Landscape comes first.
				for (tall, name, says, icon) in [
					(true, "Portrait", "Phones", Icon::Portrait),
					(false, "Landscape", "Desks and TVs", Icon::Landscape),
				] {
					if format_card(ui, icon, name, says, portrait == tall).clicked() && portrait != tall {
						wanted = Some(Canvas {
							width: current.height,
							height: current.width,
							..current
						});
					}
				}
			},
		);

		let mut on = both;
		row(
			ui,
			"Both at once",
			"Also send a portrait picture, for phones, as its own broadcast beside the landscape one. Each \
			 scene gets a portrait arrangement of its layers, and the machine encodes both.",
			|ui| {
				// As the scene file requires: only a canvas wider than tall turns.
				let wide = current.width > current.height;
				let toggle = ui
					.add_enabled(wide, kit::Toggle::new(&mut on).label("Landscape and portrait"))
					.on_disabled_hover_text(
						"Only a canvas wider than tall turns: a portrait show turns to landscape first",
					);
				if toggle.changed() {
					self.set_both(on);
				}
			},
		);

		let named = named_size(current).filter(|_| self.canvas.is_none());
		let mut size = named;
		let sizes: Vec<(Option<u32>, &str)> = SIZES
			.iter()
			.map(|(short, name)| (Some(*short), *name))
			.chain([(None, "Custom")])
			.collect();
		row(ui, "Canvas size", &upload(current, both), |ui| {
			// Custom lit, as for a size no name has, still opens its editor.
			if ui.add(kit::Segmented::new(&mut size, &sizes).reclick()).changed() {
				match size {
					Some(short) => {
						self.canvas = None;
						wanted = Some(sized(current, short));
					}
					None => self.canvas = Some(self.canvas.unwrap_or(current)),
				}
			}
		});

		let mut rate = RATES.contains(&shown.framerate).then_some(shown.framerate);
		let rates: Vec<(Option<u32>, &str)> = vec![(Some(30), "30"), (Some(60), "60"), (None, "Custom")];
		row(
			ui,
			"Frame rate",
			"60 is smoother for games and doubles the encoder's work.",
			|ui| {
				if ui.add(kit::Segmented::new(&mut rate, &rates).reclick()).changed() {
					match (rate, &mut self.canvas) {
						(Some(rate), Some(draft)) => draft.framerate = rate,
						(Some(rate), None) => {
							wanted = Some(Canvas {
								framerate: rate,
								..current
							})
						}
						(None, _) => self.canvas = Some(self.canvas.unwrap_or(current)),
					}
				}
			},
		);
		if let Some(apply) = self.custom_canvas(ui, current) {
			wanted = Some(apply);
		}

		rule(ui);
		ui.add_space(10.0);
		let resized = self.scene_sizes(ui, current, &scenes);

		let on_air = self.engine.on_air() != crate::engine::OnAir::Off;
		if on_air {
			ui.add_space(16.0);
			note(
				ui,
				"You are on air. A new size or frame rate pauses the video while the new encoders open.",
			);
		}

		let restart = match (wanted, resized) {
			(Some(canvas), _) if canvas != current => Some(Restart::Canvas(canvas)),
			(_, Some((index, size))) => {
				// Only a published scene encodes, so only its size restarts anything.
				let name = scenes[index].name.clone();
				match scenes[index].published && on_air {
					true => Some(Restart::Scene(name, size)),
					false => {
						self.apply_restart(Restart::Scene(name, size));
						None
					}
				}
			}
			_ => None,
		};
		match (restart, on_air) {
			(Some(restart), true) => self.restart = Some(restart),
			(Some(restart), false) => self.apply_restart(restart),
			(None, _) => {}
		}
	}

	/// The exact width, height, and frame rate, while Custom is open; the
	/// canvas to apply once Apply is pressed.
	fn custom_canvas(&mut self, ui: &mut Ui, current: Canvas) -> Option<Canvas> {
		let draft = self.canvas.as_mut()?;
		let mut apply = None;
		let mut cancel = false;
		ui.horizontal(|ui| {
			ui.add(kit::Number::new(egui::DragValue::new(&mut draft.width).range(2..=16384)).label("W"));
			ui.add(kit::Number::new(egui::DragValue::new(&mut draft.height).range(2..=16384)).label("H"));
			ui.add(
				kit::Number::new(egui::DragValue::new(&mut draft.framerate).range(1..=240))
					.label("at")
					.unit("fps"),
			);
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				cancel = ui.button("Cancel").clicked();
				let even = draft.width.is_multiple_of(2) && draft.height.is_multiple_of(2);
				let button = ui
					.add_enabled(even && *draft != current, egui::Button::new("Apply"))
					.on_disabled_hover_text("Each side must be even, since the encoder halves the color");
				if button.clicked() {
					apply = Some(*draft);
				}
			});
		});
		ui.add_space(10.0);
		if cancel || apply.is_some() {
			self.canvas = None;
		}
		apply
	}

	/// Each scene with a size of its own, editable, and a way to give one
	/// its own; the scene whose size changed, if any, by index.
	fn scene_sizes(
		&mut self,
		ui: &mut Ui,
		program: Canvas,
		scenes: &[Scene],
	) -> Option<(usize, Option<moq_scene::Size>)> {
		let own = scenes.iter().filter(|scene| scene.size.is_some()).count();
		let count = match own {
			0 => "Every scene is the program's size".to_string(),
			1 => "1 scene has its own size".to_string(),
			own => format!("{own} scenes have their own size"),
		};
		let mut action = None;
		let id = ui.make_persistent_id("scene sizes");
		let top = ui.cursor().top();
		let mut sizes = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
		// Found by the palette, it opens to show what it holds.
		if seeking(ui.ctx(), SCENE_SIZES) {
			sizes.set_open(true);
		}
		sizes
			.show_header(ui, |ui| {
				ui.label(RichText::new(SCENE_SIZES).font(kit::ui_strong(13.5)).color(kit::TEXT));
				ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
					ui.label(RichText::new(count).font(kit::ui(12.0)).color(kit::DIM));
				});
			})
			.body(|ui| {
				for (index, scene) in scenes.iter().enumerate() {
					let Some(size) = scene.size else { continue };
					let current = (size.width, size.height);
					let (mut width, mut height) = self.sizes.get(&scene.name).copied().unwrap_or(current);
					// By name, so an edit under way stays on its scene as others
					// come and go.
					ui.push_id(&scene.name, |ui| {
						ui.horizontal(|ui| {
							ui.add(Icon::Scene.image(14.0, kit::DIM));
							ui.label(&scene.name);
							ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
								if ui
									.add(IconButton::new(Icon::Close, "Back to the program's size").small())
									.clicked()
								{
									action = Some((index, None));
								}
								let edited = (width, height) != current;
								if ui.add_enabled(edited, egui::Button::new("Apply")).clicked() {
									action = Some((index, Some(moq_scene::Size { width, height })));
								}
								ui.add(kit::Number::new(egui::DragValue::new(&mut height).range(2..=16384)).label("H"));
								ui.add(kit::Number::new(egui::DragValue::new(&mut width).range(2..=16384)).label("W"));
							});
						})
					});
					match (width, height) == current {
						true => self.sizes.remove(&scene.name),
						false => self.sizes.insert(scene.name.clone(), (width, height)),
					};
				}
				let others: Vec<(usize, &Scene)> = scenes
					.iter()
					.enumerate()
					.filter(|(_, scene)| scene.size.is_none())
					.collect();
				if !others.is_empty() {
					ui.menu_button("Give a scene its own size", |ui| {
						for (index, scene) in others {
							if ui.button(&scene.name).clicked() {
								// The other format's, the size a second picture most
								// often wants.
								let turned = moq_scene::Size {
									width: program.height,
									height: program.width,
								};
								action = Some((index, Some(turned)));
							}
						}
					});
				}
				help(
					ui,
					"A scene of another size cannot go on air. Publish it as its own broadcast instead, under Advanced.",
				);
			});
		let rect = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
		sought(ui, SCENE_SIZES, rect);
		action
	}

	/// Ask before a change that restarts encoders applies on air.
	fn confirm_restart(&mut self, ctx: &egui::Context) {
		let Some(restart) = self.restart.clone() else { return };
		let what = match &restart {
			Restart::Canvas(canvas) => format!(
				"The program goes to {}x{} at {} fps.",
				canvas.width, canvas.height, canvas.framerate
			),
			Restart::Scene(name, _) => format!("Scene `{name}` publishes at its new size."),
		};
		let mut apply = false;
		let closed = kit::Sheet::new(egui::Id::new("restart"), "Restart the video?").show(ctx, |ui| {
			ui.label(format!(
				"{what} You are on air, so the video pauses while the new encoders open, and viewers \
				 switch to the new renditions."
			));
			ui.add_space(12.0);
			ui.horizontal(|ui| {
				apply = ui.button("Restart it").clicked();
			});
		});
		if apply {
			self.restart = None;
			self.apply_restart(restart);
		} else if closed {
			self.restart = None;
		}
	}

	/// Apply `restart`, as an edit undo takes back.
	fn apply_restart(&mut self, restart: Restart) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let result = match restart {
			Restart::Canvas(canvas) => {
				let label = format!(
					"Set the canvas to {}x{} at {} fps",
					canvas.width, canvas.height, canvas.framerate
				);
				// Only a landscape canvas goes out in portrait too.
				let turning_off = canvas.height >= canvas.width && state.collection().portrait;
				let result = edit(&mut state, label, None, |draft| {
					if canvas.height >= canvas.width {
						draft.set_portrait(false)?;
					}
					draft.set_canvas(canvas)
				});
				// As turning Both off does, it removes the layers shown only in
				// portrait, which may move the selected layer's index.
				if result.is_ok() && turning_off {
					self.deselect_item();
				}
				result
			}
			Restart::Scene(name, size) => match state.collection().scenes.iter().position(|scene| scene.name == name) {
				Some(index) => {
					// The program's size, while the show goes out in portrait
					// too, follows the program's, so the scene keeps its
					// portrait picture and can still go on program.
					let program = state.collection().canvas;
					let size = size.filter(|size| {
						!state.collection().portrait || (size.width, size.height) != (program.width, program.height)
					});
					let label = match size {
						Some(size) => format!("Size scene `{name}` {}x{}", size.width, size.height),
						None => format!("Size scene `{name}` as the program"),
					};
					let result = edit(&mut state, label, None, |draft| draft.set_size(index, size));
					if result.is_ok() {
						self.sizes.remove(&name);
						// Given its own size, its layers shown only in portrait go.
						if size.is_some() {
							self.deselect_item();
						}
					}
					result
				}
				None => Err(anyhow::anyhow!("the scene is gone")),
			},
		};
		drop(state);
		self.check(result);
	}

	/// Send a portrait picture too, or stop, as an edit undo takes back.
	fn set_both(&mut self, on: bool) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let label = match on {
			true => "Go out in landscape and portrait",
			false => "Go out in landscape only",
		};
		let result = edit(&mut state, label.to_string(), None, |draft| draft.set_portrait(on));
		drop(state);
		// Turned off, the layers shown only in portrait go, which may move
		// the selected layer's index.
		if result.is_ok() && !on {
			self.deselect_item();
		}
		self.check(result);
	}

	/// Every edit, newest first; pick one to undo or redo to just after it.
	pub(super) fn history(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let position = state.history().position();
		let mut seek = None;
		ui.label(kit::caps("History", kit::display(13.0)));
		ui.label(
			RichText::new("Pick an edit to go back to just after it; the dimmed ones redo")
				.color(kit::DIM)
				.font(kit::ui(11.0)),
		);
		egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
			// Each row as wide as the list, its words on the left.
			ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
				let entries: Vec<_> = state.history().entries().map(|edit| edit.label.clone()).collect();
				for (index, label) in entries.into_iter().enumerate().rev() {
					// Undone edits are dimmed, as redo would bring them back.
					let mut label = RichText::new(label);
					if index >= position {
						label = label.color(kit::FAINT);
					}
					if ui.add(egui::Button::selectable(position == index + 1, label)).clicked() {
						seek = Some(index + 1);
					}
				}
				if ui.add(egui::Button::selectable(position == 0, "Start")).clicked() {
					seek = Some(0);
				}
			});
		});
		if let Some(seek) = seek {
			ui.close();
			let result = state.seek(seek);
			drop(state);
			self.deselect_item();
			self.check(result);
		}
	}

	/// Settings, Recording: the folder, the picture and sound a recording
	/// holds, and whether each source gets a file of its own too.
	fn recording(&mut self, ui: &mut Ui) {
		let stream = self.controls.stream.lock().unwrap().clone();
		let dir = self.engine.recorder.dir();
		let flagged = stream.overridden().recordings;
		let busy = self.saving.is_some() || self.picker.is_some();
		// The folder stays put under a recording, as the recorder holds it.
		let recording = self.engine.recorder.status().is_some_and(|status| !status.done);
		const RECORDING: &str = "Stop the recording to move the folder.";
		rule(ui);
		ui.add_space(12.0);
		ui.horizontal(|ui| {
			let (tile, _) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::hover());
			ui.painter().rect_filled(tile, 10, kit::P3);
			Icon::Folder.paint(ui, tile.shrink(10.0), 18.0, kit::TEXT);
			ui.vertical(|ui| {
				ui.label(
					RichText::new(dir.display().to_string())
						.font(kit::ui_strong(13.5))
						.color(kit::TEXT),
				);
				let said = match (flagged, stream.saved().recordings.is_some()) {
					(true, _) => "--recordings sets this for this run.".to_string(),
					(false, true) => "Clips are saved here too.".to_string(),
					(false, false) => "Beside the show file, by default. Clips are saved here too.".to_string(),
				};
				ui.label(RichText::new(said).font(kit::ui(12.0)).color(match flagged {
					true => kit::CUE,
					false => kit::DIM,
				}));
			});
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				if ui.add(kit::button(Icon::Detach, "Open")).clicked() {
					let result = crate::shelf::open(&dir);
					self.check(result);
				}
				let button = |ui: &mut Ui, name: &str, enabled: bool| {
					let response = ui.add_enabled(enabled && !busy && !recording, egui::Button::new(name));
					match recording {
						true => response.on_disabled_hover_text(RECORDING),
						false => response,
					}
				};
				if button(ui, "Change", !flagged).clicked() {
					self.pick(Picking::Folder, &dir);
				}
				if stream.saved().recordings.is_some() && !flagged && button(ui, "Beside the show", true).clicked() {
					let saved = Saved {
						recordings: None,
						..stream.saved().clone()
					};
					self.move_recordings(stream.clone(), saved, ui.ctx());
				}
			});
		});
		self.saved_note(ui, Setting::Recording);
		ui.add_space(12.0);
		self.recorded_scene(ui);
		self.recorded_audio(ui);
		let mut each = self.controls.record_sources.load(Ordering::Relaxed);
		row(
			ui,
			"Each source too",
			"Every camera, screen, and microphone also saves a file of its own, before its filters. Each costs an \
			 encoder and its share of the disk.",
			|ui| {
				if ui.add(kit::Toggle::new(&mut each)).changed() {
					self.controls.record_sources.store(each, Ordering::Relaxed);
				}
			},
		);
		rule(ui);
		ui.add_space(16.0);
		note(
			ui,
			"A recording cut short by a crash is repaired the next time the studio opens.",
		);
	}

	/// Open the system's dialog to pick `what`, starting at `from`.
	fn pick(&mut self, what: Picking, from: &std::path::Path) {
		let from = from.to_path_buf();
		let task = Task::spawn(&self.runtime, move || async move {
			let dialog = rfd::AsyncFileDialog::new().set_directory(&from);
			let picked = match what {
				Picking::Folder => dialog.set_title("Where recordings go").pick_folder().await,
				Picking::Key => {
					dialog
						.set_title("The room key")
						.add_filter("Key", &["jwk", "json"])
						.pick_file()
						.await
				}
			};
			picked.map(|picked| picked.path().to_path_buf())
		});
		match task {
			Ok(task) => self.picker = Some(Picker { what, task }),
			Err(err) => self.report(Err(anyhow::anyhow!("cannot open the file dialog: {err}"))),
		}
	}

	/// Take what the system's dialog picked, once it closes: a recordings
	/// folder is saved at once, and a room key fills the Guests form.
	fn follow_picker(&mut self, ctx: &egui::Context) {
		let Some(picker) = &mut self.picker else { return };
		let Some(picked) = picker.task.poll() else {
			ctx.request_repaint_after(Duration::from_millis(100));
			return;
		};
		let (what, picked) = (picker.what, picked.clone());
		self.picker = None;
		let Some(picked) = picked else { return };
		match what {
			Picking::Folder => {
				let stream = self.controls.stream.lock().unwrap().clone();
				let saved = Saved {
					recordings: Some(picked),
					..stream.saved().clone()
				};
				self.move_recordings(stream, saved, ctx);
			}
			Picking::Key => {
				let stream = self.controls.stream.lock().unwrap().clone();
				let form = self.guests_form.get_or_insert_with(|| GuestForm::new(stream.saved()));
				form.key = picked.display().to_string();
			}
		}
	}

	/// How the last save from `setting`'s pane went, if it was the last.
	pub(super) fn saved_note(&self, ui: &mut Ui, setting: Setting) {
		if self.saving.as_ref().is_some_and(|saving| saving.from == setting) {
			ui.horizontal(|ui| {
				ui.spinner();
				ui.label(RichText::new("Saving").color(kit::DIM));
			});
		}
		if let Some((from, note)) = &self.saved
			&& *from == setting
		{
			ui.add(egui::Label::new(RichText::new(note).font(kit::ui(12.0)).color(kit::DIM)).wrap());
		}
	}

	/// Pick the picture recordings hold, the program or any scene, even while
	/// recording, which goes on in a new file when the size changes.
	fn recorded_scene(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let current = state.collection().recorded().map(str::to_string);
		let scenes = state.collection().scenes.iter();
		let names: Vec<String> = scenes.map(|scene| scene.name.clone()).collect();
		let mut picked = current.clone();
		row(
			ui,
			"Picture",
			"The program is what viewers see. A scene records at its own size, in a new file when it changes.",
			|ui| {
				egui::ComboBox::from_id_salt("recorded scene")
					.selected_text(picked.as_deref().unwrap_or("The program"))
					.show_ui(ui, |ui| {
						ui.selectable_value(&mut picked, None, "The program");
						for name in &names {
							ui.selectable_value(&mut picked, Some(name.clone()), name);
						}
					});
			},
		);
		if picked == current {
			return;
		}
		let label = match &picked {
			Some(name) => format!("Record scene `{name}`"),
			None => "Record the program".into(),
		};
		let result = edit(&mut state, label, None, |draft| draft.set_recorded(picked.as_deref()));
		drop(state);
		self.check(result);
	}

	/// Pick the audio scene recordings hold, even while recording, which goes
	/// on with the new one from its next block.
	fn recorded_audio(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let scenes = state.collection().audio_scenes.clone();
		let program = state.collection().program_audio().unwrap_or_default();
		let on_program = format!("On program ({program})");
		let current = scenes
			.iter()
			.find(|scene| scene.recorded)
			.map(|scene| scene.name.clone());
		let mut picked = current.clone();
		row(
			ui,
			"Sound",
			"The audio scene the recording holds. It can differ from the stream's, to keep music out; one nothing \
			 else carries stays off the relay.",
			|ui| {
				egui::ComboBox::from_id_salt("recorded audio")
					.selected_text(picked.as_deref().unwrap_or(&on_program))
					.show_ui(ui, |ui| {
						ui.selectable_value(&mut picked, None, &on_program);
						for scene in &scenes {
							ui.selectable_value(&mut picked, Some(scene.name.clone()), &scene.name);
						}
					});
			},
		);
		if picked == current {
			return;
		}
		let label = match &picked {
			Some(name) => format!("Record audio scene `{name}`"),
			None => "Record the audio scene on program".into(),
		};
		let mut next = scenes;
		for scene in &mut next {
			scene.recorded = picked.as_deref() == Some(scene.name.as_str());
		}
		let result = edit(&mut state, label, None, |draft| draft.set_audio_scenes(next));
		drop(state);
		self.check(result);
	}

	/// Settings, Guests: the room, its key, the join page, and how long a
	/// link lasts, kept beside the show. A setting a flag overrides for this
	/// run says so.
	fn guest_settings(&mut self, ui: &mut Ui) {
		let stream = self.controls.stream.lock().unwrap().clone();
		let overridden = stream.overridden();
		let busy = self.saving.is_some() || self.picker.is_some();
		let saved = stream.saved().clone();
		let form = self.guests_form.get_or_insert_with(|| GuestForm::new(&saved));
		let mut pick_key = false;

		// Where guests stand now.
		let room = self.engine.room();
		let failure = self.controls.room_failure();
		let (dot, said) = match (&room, &failure) {
			(_, Some(failure)) => (kit::ERR, format!("Guests cannot join: {failure}")),
			(Some(room), None) => match room.renewal_failure() {
				Some(failure) => (kit::CUE, format!("Tokens are not renewing: {failure}")),
				None => (
					kit::OK,
					format!("Guests join the room `{}`, from Add on BUILD.", room.path()),
				),
			},
			(None, None) => (
				kit::FAINT,
				"No room yet. Set one, with its key, to invite guests.".into(),
			),
		};
		ui.horizontal(|ui| {
			let (at, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
			ui.painter().circle_filled(at.center(), 4.5, dot);
			ui.add(egui::Label::new(RichText::new(said).font(kit::ui(12.5)).color(kit::TEXT)).wrap());
		});
		// Such as once the room key's file is back; on air, the stream's end
		// tries it again.
		if failure.is_some() && self.engine.on_air() == crate::engine::OnAir::Off {
			ui.add_space(6.0);
			if ui.add_enabled(!busy, egui::Button::new("Try again")).clicked() {
				self.controls.retry_room();
			}
		}
		ui.add_space(10.0);

		ui.add_enabled_ui(!busy, |ui| {
			field_row(
				ui,
				"Room",
				"A path under the relay's address, such as `guests`.",
				overridden.room.then_some("--room"),
				|ui| {
					ui.add(
						egui::TextEdit::singleline(&mut form.room)
							.id_salt("guests room")
							.hint_text("guests")
							.desired_width(f32::INFINITY),
					);
				},
			);
			field_row(
				ui,
				"Room key",
				"Signs each guest's link, and only reaches inside the room. Make one with `moq auth generate \
				 --root <relay path>/<room> --publish '**' --subscribe '**'`.",
				overridden.room_key.then_some("--room-key"),
				|ui| {
					ui.horizontal(|ui| {
						pick_key = ui.button("Choose").clicked();
						ui.add(
							egui::TextEdit::singleline(&mut form.key)
								.id_salt("guests room key")
								.hint_text("room.jwk")
								.desired_width(f32::INFINITY),
						);
					});
				},
			);
			field_row(
				ui,
				"Join page",
				"The page a guest's link opens. It must be https://.",
				overridden.join_page.then_some("--join-page"),
				|ui| {
					ui.add(
						egui::TextEdit::singleline(&mut form.page)
							.id_salt("guests join page")
							.hint_text("https://join.studio.moq.dev/")
							.desired_width(f32::INFINITY),
					);
				},
			);
			let mut lifetimes: Vec<(Duration, String)> = LIFETIMES
				.iter()
				.map(|(lasts, name)| (*lasts, name.to_string()))
				.collect();
			// One set by hand in the file is offered as it is.
			if !lifetimes.iter().any(|(lasts, _)| *lasts == form.expiry) {
				lifetimes.push((form.expiry, moq_tokio::cli::Duration::from(form.expiry).to_string()));
			}
			let options: Vec<(Duration, &str)> =
				lifetimes.iter().map(|(lasts, name)| (*lasts, name.as_str())).collect();
			field_row(
				ui,
				"Link lifetime",
				"How long a join link works. A guest in the room is renewed, so a guest removed from scenes leaves \
				 within it.",
				overridden.guest_expiry.then_some("--guest-expiry"),
				|ui| {
					ui.add(kit::Segmented::new(&mut form.expiry, &options));
				},
			);
			field_row(ui, "Appear on their own", "", None, |ui| {
				ui.add(kit::Toggle::new(&mut form.appear).label("In the next tile on program"))
					.on_hover_text(
						"A guest who joins takes the next tile of the scene on program, rather than waiting under Add",
					);
			});
		});

		let edited = form.edited(&saved);
		let result = form.saved(&saved);
		let mut revert = false;
		let mut save = false;
		rule(ui);
		ui.add_space(12.0);
		ui.horizontal(|ui| {
			save = ui.add_enabled(edited && !busy, egui::Button::new("Save")).clicked();
			revert = ui.add_enabled(edited && !busy, egui::Button::new("Revert")).clicked();
		});
		self.saved_note(ui, Setting::Guests);
		if save {
			match result {
				Ok(next) => self.save_stream(Setting::Guests, stream.clone(), next, Vec::new(), ui.ctx()),
				Err(err) => self.saved = Some((Setting::Guests, err)),
			}
		}
		if revert {
			self.guests_form = Some(GuestForm::new(&saved));
		}
		if pick_key {
			let from = stream
				.saved()
				.room_key
				.as_ref()
				.and_then(|key| key.parent())
				.map(std::path::Path::to_path_buf)
				.unwrap_or_else(|| self.engine.recorder.dir());
			self.pick(Picking::Key, &from);
		}
	}

	/// Save `saved`, with a new recordings folder, and move the recorder to
	/// it, refusing any recording until both are done, so the file and the
	/// recorder never name different folders: an unfinished recording is
	/// repaired only in the folder the file names. On a thread of its own,
	/// as any save is, so a slow disk never holds up the window.
	fn move_recordings(&mut self, mut stream: crate::stream::Stream, saved: Saved, ctx: &egui::Context) {
		if self.refuse_overlap(Setting::Recording) {
			return;
		}
		let engine = self.engine.clone();
		let ctx = ctx.clone();
		let (send, recv) = std::sync::mpsc::channel();
		let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
			let result = (|| -> anyhow::Result<crate::stream::Stream> {
				let moving = engine.recorder.moving(&saved.folder(engine.autosave.path()))?;
				stream.save(saved)?;
				moving.finish();
				Ok(stream)
			})();
			let _ = send.send(result.map_err(|err| format!("{err:#}")));
			// After the send, so the frame it asks for finds the result.
			ctx.request_repaint();
		});
		self.saved = None;
		self.saving = Some(super::live::Saving {
			from: Setting::Recording,
			retokened: false,
			task: match spawned {
				Ok(_) => Task::Running(recv),
				Err(err) => Task::Done(Err(format!("cannot save: {err}"))),
			},
		});
	}

	/// Save `saved` as the show's stream file, keeping `tokens`, for the
	/// `from` pane, which shows how it went.
	pub(super) fn save_stream(
		&mut self,
		from: Setting,
		stream: crate::stream::Stream,
		saved: Saved,
		tokens: Vec<super::live::Token>,
		ctx: &egui::Context,
	) {
		if self.refuse_overlap(from) {
			return;
		}
		self.saved = None;
		self.saving = Some(Saving::start(from, stream, saved, tokens, ctx));
	}

	/// Refuse a save for the `from` pane while another runs, which would
	/// lose its result and write the file from settings it has not seen.
	fn refuse_overlap(&mut self, from: Setting) -> bool {
		let running = self.saving.is_some();
		if running {
			self.saved = Some((
				from,
				"Another save is still running; try again once it finishes.".into(),
			));
		}
		running
	}

	/// Bind the key pressed to the action the Hotkeys pane waits on, or cancel
	/// on Escape.
	pub(super) fn take_binding(&mut self, ctx: &egui::Context) {
		let shown = std::mem::take(&mut self.hotkeys_shown);
		let (Some(action), Some(hotkeys)) = (self.binding.clone(), &mut self.hotkeys) else {
			return;
		};
		// A wait the operator left for another pane ends, so the hotkeys
		// are not held off unseen.
		if !shown {
			self.end_binding();
			return;
		}
		let Some(key) = hotkeys.taken() else { return };
		self.binding = None;
		hotkeys.suspend(false);
		if key.key == PhysicalKey::Code(KeyCode::Escape) {
			return;
		}
		let shown = key.shown(ctx);
		self.refusal = match hotkeys.bind(&action, Some(key)) {
			Ok(()) => None,
			Err(other) => Some((action, format!("{shown} is bound to {other}; clear it there first"))),
		};
	}

	/// Stop waiting for a key to bind, taking the hotkeys up again.
	pub(super) fn end_binding(&mut self) {
		if self.binding.take().is_some()
			&& let Some(hotkeys) = &mut self.hotkeys
		{
			hotkeys.suspend(false);
		}
	}

	/// Settings, Hotkeys: each action by what it is for, with its keys on
	/// keycaps, heard even while another app has focus where the platform
	/// lets the studio hear them.
	pub(super) fn hotkeys(&mut self, ui: &mut Ui, fx: &mut Fx) {
		let Some(hotkeys) = &mut self.hotkeys else {
			return;
		};
		self.hotkeys_shown = true;
		let reach = hotkeys.reach();
		let (strong, said, ink) = match &reach {
			Reach::Global => (
				"Hotkeys work while a game has focus.",
				"Bind keys you do not play or type with.".to_string(),
				kit::DIM,
			),
			Reach::Desktop(_) => (
				"Hotkeys work while another app has focus.",
				"The desktop's shortcut settings decide the keys, shown beside each.".to_string(),
				kit::DIM,
			),
			Reach::Asking => ("Asking the desktop to bind the keys.", String::new(), kit::DIM),
			Reach::Focused(why) => (
				"Hotkeys work only while the studio has focus.",
				why.to_string(),
				kit::CUE,
			),
		};
		egui::Frame::new()
			.fill(kit::P2)
			.stroke(Stroke::new(1.0, kit::LINE))
			.corner_radius(10)
			.inner_margin(egui::Margin::symmetric(12, 10))
			.show(ui, |ui| {
				ui.set_width(ui.available_width());
				ui.horizontal(|ui| {
					ui.add(Icon::Hotkeys.image(16.0, kit::TEXT));
					ui.vertical(|ui| {
						ui.label(RichText::new(strong).font(kit::ui_strong(12.5)).color(kit::TEXT));
						if !said.is_empty() {
							ui.add(egui::Label::new(RichText::new(said).font(kit::ui(12.5)).color(ink)).wrap());
						}
					});
				});
			});
		// A source can be in the scene file yet unlisted, such as a clip with no
		// audio, so a sound is gone only once the file lacks it, as firing it checks.
		let (listed, sources): (Vec<Action>, HashSet<String>) = {
			let state = self.engine.state.lock().unwrap();
			let sources = state.collection().sources.keys().cloned().collect();
			(Action::listed(&state), sources)
		};
		let bindings = hotkeys.bindings();
		// A key bound to an action not listed, so it can still be seen and cleared.
		let unlisted: Vec<Action> = bindings
			.keys()
			.filter(|action| !listed.contains(action))
			.cloned()
			.collect();
		let groups = Group::ALL.map(|group| {
			let actions: Vec<Action> = listed
				.iter()
				.chain(&unlisted)
				.filter(|action| action.group() == group)
				.cloned()
				.collect();
			(group.name(), actions)
		});

		let mut bind = None;
		let mut clear = None;
		// A scene's key takes it, or cues it in Studio mode.
		let studio = self.controls.studio();
		let ctx = ui.ctx().clone();
		for (caption, actions) in groups {
			if actions.is_empty() {
				continue;
			}
			ui.add_space(14.0);
			ui.label(kit::caps(caption, kit::display_bold(11.0)).color(kit::FAINT));
			ui.add_space(4.0);
			let half = actions.len().div_ceil(2);
			ui.columns(2, |columns| {
				for (column, actions) in columns.iter_mut().zip(actions.chunks(half)) {
					for action in actions {
						let name = match action {
							Action::Scene(name) | Action::Mute(name) => name.clone(),
							action => action.name(studio),
						};
						let detail = match action {
							Action::Scene(_) if unlisted.contains(action) => Some("no such scene"),
							Action::Mute(name) if !sources.contains(name) => Some("no such sound"),
							action => action.detail(studio),
						};
						let refusal = self
							.refusal
							.as_ref()
							.filter(|(refused, _)| refused == action)
							.map(|(_, why)| why.clone())
							.or_else(|| hotkeys.refused(action).map(str::to_string));
						let desktop = match (&reach, bindings.get(action)) {
							(Reach::Desktop(triggers), Some(_)) => Some(match triggers.get(action) {
								Some(trigger) => format!("the desktop bound {trigger}"),
								None => "the desktop bound no key".to_string(),
							}),
							_ => None,
						};
						let caps = bindings.get(action).map(|bound| bound.caps(&ctx));
						let waiting = self.binding.as_ref() == Some(action);
						match hotkey_row(column, fx, &name, detail, caps.as_deref(), waiting) {
							Some(true) => bind = Some(action.clone()),
							Some(false) => clear = Some(action.clone()),
							None => {}
						}
						if let Some(why) = refusal {
							column.label(RichText::new(why).font(kit::ui(11.5)).color(kit::ERR));
						} else if let Some(desktop) = desktop {
							column.label(RichText::new(desktop).font(kit::ui(11.5)).color(kit::DIM));
						}
					}
				}
			});
		}
		ui.add_space(16.0);
		note(
			ui,
			"A key bound to one action is refused for another until it is cleared there.",
		);
		if let Some(action) = bind {
			self.refusal = None;
			self.binding = Some(action);
			hotkeys.suspend(true);
		}
		if let Some(action) = clear {
			self.refusal = None;
			// Clearing the action waiting for a key stops the wait.
			if self.binding.as_ref() == Some(&action) {
				self.binding = None;
				hotkeys.suspend(false);
			}
			// Unbinding never conflicts.
			let _ = hotkeys.bind(&action, None);
		}
	}

	/// Settings, Advanced: scenes and sources published on their own, the
	/// virtual camera, the effects, and the scene graph.
	fn advanced(&mut self, ui: &mut Ui, fx: &mut Fx) {
		let engine = self.engine.clone();
		let (scenes, sources, published) = {
			let state = engine.state.lock().unwrap();
			let collection = state.collection();
			let sources: Vec<String> = collection
				.sources
				.iter()
				.filter(|(_, source)| source.independent())
				.map(|(name, _)| name.clone())
				.collect();
			(collection.scenes.clone(), sources, collection.published.clone())
		};

		let mut scene_publish = None;
		row(
			ui,
			"Publish a scene as its own broadcast",
			"It stays live whatever is on air, at its own size. Each one costs an encoder of its own.",
			|ui| {
				let others: Vec<(usize, &Scene)> = scenes
					.iter()
					.enumerate()
					.filter(|(_, scene)| !scene.published)
					.collect();
				add_menu(
					ui,
					"Publish a scene",
					others.iter().map(|(index, scene)| (*index, scene.name.as_str())),
					|index| {
						scene_publish = Some((index, true));
					},
				);
				for (index, scene) in scenes.iter().enumerate().rev().filter(|(_, scene)| scene.published) {
					if chip(ui, &scene.name, "Stop publishing it") {
						scene_publish = Some((index, false));
					}
				}
			},
		);
		let mut source_publish = None;
		row(
			ui,
			"Publish a source independently",
			"Viewers and services fetch it on its own. It leaves the program and the audio the studio encodes, though Listen still hears it.",
			|ui| {
				let others = sources.iter().filter(|name| !published.contains(*name));
				add_menu(
					ui,
					"Publish a source",
					others.map(|name| (name.clone(), name.as_str())),
					|name| {
						source_publish = Some((name, true));
					},
				);
				for name in published.iter().rev() {
					if chip(ui, name, "Stop publishing it independently") {
						source_publish = Some((name.clone(), false));
					}
				}
			},
		);
		if let Some((index, published)) = scene_publish {
			let name = scenes[index].name.clone();
			let label = match published {
				true => format!("Publish scene `{name}`"),
				false => format!("Stop publishing scene `{name}`"),
			};
			// The scene as it is under the lock that edits it, found by name,
			// so a reload since the copy above is kept.
			let mut state = engine.state.lock().unwrap();
			let result = match state.collection().scenes.iter().position(|scene| scene.name == name) {
				Some(index) => {
					let scene = state.collection().scenes[index].clone();
					edit(&mut state, label, None, |draft| {
						draft.set_scene(index, Scene { published, ..scene })
					})
				}
				None => Err(anyhow::anyhow!("the scene is gone")),
			};
			drop(state);
			self.check(result);
		}
		if let Some((name, published)) = source_publish {
			let verb = if published { "Publish" } else { "Stop publishing" };
			let mut state = engine.state.lock().unwrap();
			let result = edit(&mut state, format!("{verb} `{name}` independently"), None, |draft| {
				draft.set_published(&name, published)
			});
			drop(state);
			self.check(result);
		}

		self.camera(ui, None);
		for scene in scenes.iter().filter(|scene| scene.published) {
			self.camera(ui, Some(&scene.name));
		}
		row(
			ui,
			"Effects",
			"How this window moves and glows. It never reaches the stream.",
			|ui| {
				fx.picker(ui);
			},
		);
		row(
			ui,
			"Scene graph",
			"Every source, scene, and audio scene, drawn as one map you can wire.",
			|ui| {
				if ui.add(kit::button(Icon::Detach, "Open")).clicked() {
					self.graph_open = true;
					self.setting = None;
				}
			},
		);
	}

	/// The virtual camera of the program, or of the published scene `scene`:
	/// a toggle, and how it is doing while on.
	pub(super) fn camera(&mut self, ui: &mut Ui, scene: Option<&str>) {
		let camera = self.engine.state.lock().unwrap().camera(scene);
		let mut on = camera.is_some();
		let (title, help, named) = match scene {
			None => (
				"Virtual camera".to_string(),
				"Shows the program as a camera that video-call apps can pick.".to_string(),
				"virtual camera".to_string(),
			),
			Some(scene) => (
				format!("`{scene}` as a camera"),
				format!("Shows the published scene `{scene}` as a camera of its own."),
				format!("scene `{scene}`'s virtual camera"),
			),
		};
		let help = match &camera {
			Some(camera) => format!(
				"{} at {}x{}, {} fps. {}; {} frames sent, {} dropped.",
				camera.backend,
				camera.canvas.width,
				camera.canvas.height,
				camera.canvas.framerate,
				if camera.reading {
					"An app is reading it"
				} else {
					"No app reads it, so nothing is copied"
				},
				camera.sent,
				camera.dropped,
			),
			None => help,
		};
		let mut toggled = false;
		row(ui, &title, &help, |ui| {
			toggled = ui.add(kit::Toggle::new(&mut on)).changed();
		});
		if let Some(error) = camera.as_ref().and_then(|camera| camera.error.as_ref()) {
			ui.label(RichText::new(error).font(kit::ui(12.0)).color(kit::ERR));
		}
		if toggled {
			let result = self.engine.set_camera(scene, on);
			let message = format!("{named} {}", if on { "on" } else { "off" });
			self.report(result.map(|()| message));
		}
	}

	/// The scene graph, in a window of its own, while open.
	pub(super) fn graph_window(&mut self, ctx: &egui::Context, stage: Rect) {
		if !self.graph_open {
			return;
		}
		let mut open = true;
		egui::Window::new("Scene graph")
			.id(egui::Id::new("scene graph"))
			.open(&mut open)
			.frame(kit::popover_frame().inner_margin(egui::Margin::same(14)))
			.collapsible(false)
			.default_rect(stage.shrink(24.0))
			.show(ctx, |ui| self.graph(ui));
		self.graph_open = open;
	}
}

/// A line across the pane, above the next row.
fn rule(ui: &mut Ui) {
	let y = ui.cursor().top();
	ui.painter()
		.hline(ui.max_rect().x_range(), y, Stroke::new(1.0, kit::LINE));
}

/// A setting's row, under a line: its name and what it costs on the left,
/// its control on the right.
pub(super) fn row(ui: &mut Ui, name: &str, help: &str, control: impl FnOnce(&mut Ui)) {
	labeled(ui, name, help, NAME, |ui| {
		ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), control));
	});
}

/// A row whose control is a field filling the right side, which says when
/// `flag` sets it for this run instead, and is then not editable.
pub(super) fn field_row(ui: &mut Ui, name: &str, help: &str, flag: Option<&str>, field: impl FnOnce(&mut Ui)) {
	labeled(ui, name, help, 0.36, |ui| {
		ui.vertical(|ui| {
			ui.add_enabled_ui(flag.is_none(), field);
			if let Some(flag) = flag {
				flag_note(ui, flag);
			}
		});
	});
}

/// A row under a line: `name` and `help` in `share` of its width with
/// `control` beside them, or `control` under them in a pane too narrow.
fn labeled(ui: &mut Ui, name: &str, help: &str, share: f32, control: impl FnOnce(&mut Ui)) {
	let top = ui.cursor().top();
	rule(ui);
	ui.add_space(14.0);
	let width = ui.available_width();
	let label = |ui: &mut Ui| {
		ui.label(RichText::new(name).font(kit::ui_strong(13.5)).color(kit::TEXT));
		if !help.is_empty() {
			ui.add_space(2.0);
			help_label(ui, help);
		}
	};
	if width < stack(ui) {
		label(ui);
		ui.add_space(10.0);
		control(ui);
	} else {
		ui.horizontal(|ui| {
			ui.vertical(|ui| {
				ui.set_width(width * share);
				label(ui);
			});
			control(ui);
		});
	}
	ui.add_space(14.0);
	let rect = Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
	sought(ui, name, rect);
}

/// The setting the palette found, by its row's name, and when, which its
/// pane scrolls to once and lights for a moment.
#[derive(Clone)]
struct Sought {
	name: String,
	since: f64,
	scrolled: bool,
}

/// The Show pane's disclosure of each scene's own size, by its name.
pub(super) const SCENE_SIZES: &str = "Scene sizes";

/// Whether the palette found the setting called `name`, which its pane has
/// yet to light.
fn seeking(ctx: &egui::Context, name: &str) -> bool {
	let sought = ctx.data(|data| data.get_temp::<Sought>(egui::Id::new("sought setting")));
	sought.is_some_and(|sought| sought.name == name)
}

/// How long a setting the palette found stays lit.
const LIT: f64 = 1.5;

/// Scroll Settings to the row called `name` and light it, as the palette
/// does for a setting it found.
pub(super) fn seek(ctx: &egui::Context, name: &str) {
	let since = ctx.input(|input| input.time);
	let sought = Sought {
		name: name.to_string(),
		since,
		scrolled: false,
	};
	ctx.data_mut(|data| data.insert_temp(egui::Id::new("sought setting"), sought));
}

/// Scroll to the row called `name` at `rect` once, and light it a moment, if
/// the palette found it.
fn sought(ui: &mut Ui, name: &str, rect: Rect) {
	let id = egui::Id::new("sought setting");
	let Some(mut sought) = ui.ctx().data(|data| data.get_temp::<Sought>(id)) else {
		return;
	};
	if sought.name != name {
		return;
	}
	if !std::mem::replace(&mut sought.scrolled, true) {
		ui.scroll_to_rect(rect, Some(Align::Min));
	}
	let age = ui.input(|input| input.time) - sought.since;
	if age > LIT {
		ui.ctx().data_mut(|data| data.remove::<Sought>(id));
		return;
	}
	let alpha = (1.0 - age / LIT) as f32;
	ui.painter()
		.rect_filled(rect, 8, kit::ACCENT.gamma_multiply(0.12 * alpha));
	ui.ctx().request_repaint();
	ui.ctx().data_mut(|data| data.insert_temp(id, sought));
}

/// That `flag` sets a setting for this run, over the show's own.
pub(super) fn flag_note(ui: &mut Ui, flag: &str) {
	cue_note(ui, &format!("{flag} sets this for this run"));
}

/// Why a setting is not the one saved this run, such as a flag over it.
pub(super) fn cue_note(ui: &mut Ui, text: &str) {
	ui.add(egui::Label::new(RichText::new(text).font(kit::ui(11.5)).color(kit::CUE)).wrap());
}

/// Words under a setting's name.
fn help_label(ui: &mut Ui, text: &str) {
	ui.add(egui::Label::new(RichText::new(text).font(kit::ui(12.0)).color(kit::DIM)).wrap());
}

/// Words under a group of rows.
fn help(ui: &mut Ui, text: &str) {
	ui.add_space(6.0);
	help_label(ui, text);
}

/// A note at the foot of a pane, after an info mark.
fn note(ui: &mut Ui, text: &str) {
	ui.horizontal(|ui| {
		ui.add(Icon::Info.image(14.0, kit::DIM));
		ui.add(egui::Label::new(RichText::new(text).font(kit::ui(12.0)).color(kit::DIM)).wrap());
	});
}

/// One of the two formats, as a card with its shape: whether it was clicked.
fn format_card(ui: &mut Ui, icon: Icon, name: &str, says: &str, on: bool) -> egui::Response {
	let (rect, response) = ui.allocate_exact_size(CARD, Sense::click());
	response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, name));
	let painter = ui.painter();
	let (fill, stroke) = match (on, response.hovered()) {
		(true, _) => (kit::ACCENT.gamma_multiply(0.12), Stroke::new(2.0, kit::ACCENT)),
		(false, true) => (kit::P3, Stroke::new(1.0, kit::LINE)),
		(false, false) => (kit::P2, Stroke::new(1.0, kit::LINE)),
	};
	painter.rect(rect, 11, fill, stroke, StrokeKind::Inside);
	let shape = Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), Vec2::splat(36.0));
	icon.paint(ui, shape, 36.0, if on { kit::TEXT } else { kit::DIM });
	let painter = ui.painter();
	let name = painter.layout_no_wrap(name.into(), kit::ui_strong(13.5), kit::TEXT);
	let says = painter.layout_no_wrap(says.into(), kit::ui(11.5), kit::DIM);
	let x = rect.left() + 66.0;
	painter.galley(pos2(x, rect.center().y - name.size().y - 2.0), name, kit::TEXT);
	painter.galley(pos2(x, rect.center().y + 4.0), says, kit::DIM);
	response
}

/// A published thing's tag, with a cross that stops it: whether the cross
/// was clicked.
fn chip(ui: &mut Ui, name: &str, stop: &str) -> bool {
	let mut stopped = false;
	egui::Frame::new()
		.fill(kit::P2)
		.stroke(Stroke::new(1.0, kit::LINE))
		.corner_radius(8)
		.inner_margin(egui::Margin {
			left: 10,
			right: 4,
			top: 2,
			bottom: 2,
		})
		.show(ui, |ui| {
			ui.horizontal(|ui| {
				ui.label(RichText::new(name).font(kit::ui(12.0)).color(kit::TEXT));
				stopped = ui.add(IconButton::new(Icon::Close, stop).small()).clicked();
			});
		});
	stopped
}

/// A plus that opens a menu of `items`, calling `picked` with the one picked.
fn add_menu<'a, T>(ui: &mut Ui, name: &str, items: impl Iterator<Item = (T, &'a str)>, picked: impl FnOnce(T)) {
	let items: Vec<(T, &str)> = items.collect();
	let button = ui.add_enabled(!items.is_empty(), IconButton::new(Icon::Add, name).small());
	let mut chosen = None;
	egui::Popup::menu(&button).show(|ui| {
		for (value, label) in items {
			if ui.button(label).clicked() {
				chosen = Some(value);
			}
		}
	});
	if let Some(chosen) = chosen {
		picked(chosen);
	}
}

/// One action's row: its name and what it does, and its keys on keycaps
/// with a cross that clears them, a Bind button, or the wait for a key.
/// `Some(true)` asks to bind it, `Some(false)` to clear it.
fn hotkey_row(
	ui: &mut Ui,
	fx: &mut Fx,
	name: &str,
	detail: Option<&str>,
	caps: Option<&[String]>,
	waiting: bool,
) -> Option<bool> {
	let mut asked = None;
	let top = ui.cursor().top();
	ui.painter()
		.hline(ui.max_rect().x_range(), top, Stroke::new(1.0, kit::LINE));
	ui.allocate_ui_with_layout(
		vec2(ui.available_width(), 36.0),
		Layout::left_to_right(Align::Center),
		|ui| {
			ui.label(RichText::new(name).font(kit::ui(13.0)).color(kit::TEXT));
			if let Some(detail) = detail {
				ui.label(RichText::new(detail).font(kit::ui(11.5)).color(kit::DIM));
			}
			ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
				ui.spacing_mut().item_spacing.x = 4.0;
				if waiting {
					egui::Frame::new()
						.stroke(Stroke::new(1.0, kit::ACCENT))
						.corner_radius(8)
						.inner_margin(egui::Margin::symmetric(8, 3))
						.show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.spacing_mut().item_spacing.x = 5.0;
								ui.label(RichText::new("Press the keys now").font(kit::ui(11.5)).color(kit::DIM));
								ui.add(kit::Keycap::new("Esc"));
								ui.label(RichText::new("cancels").font(kit::ui(11.5)).color(kit::DIM));
							});
						});
					return;
				}
				match caps {
					Some(caps) => {
						if ui.add(IconButton::new(Icon::Close, "Clear the key").small()).clicked() {
							asked = Some(false);
						}
						// Right to left, so the last key first.
						let mut clicked = false;
						for cap in caps.iter().rev() {
							clicked |= ui
								.add(kit::Keycap::new(cap).fx(fx))
								.interact(Sense::click())
								.on_hover_text("Bind another key")
								.clicked();
						}
						if clicked {
							asked = Some(true);
						}
					}
					None => {
						if ui.button("Bind").clicked() {
							asked = Some(true);
						}
					}
				}
			});
		},
	);
	asked
}

/// The named canvas size `canvas` is, by its short side, if it is one.
fn named_size(canvas: Canvas) -> Option<u32> {
	let (short, long) = (canvas.width.min(canvas.height), canvas.width.max(canvas.height));
	SIZES
		.iter()
		.map(|(side, _)| *side)
		.find(|side| *side == short && long == side * 16 / 9)
}

/// `canvas` at the named size with `short` as its short side, keeping its
/// format and frame rate.
fn sized(canvas: Canvas, short: u32) -> Canvas {
	let long = short * 16 / 9;
	let (width, height) = match canvas.height > canvas.width {
		true => (short, long),
		false => (long, short),
	};
	Canvas {
		width,
		height,
		..canvas
	}
}

/// What `canvas` costs in upload: the most its renditions and the audio
/// send, as each rendition's ceiling allows, twice over, a picture in each
/// format, if `both`.
fn upload(canvas: Canvas, both: bool) -> String {
	// The portrait picture is a broadcast of its own, audio included.
	let pictures = std::iter::once(canvas).chain(both.then(|| canvas.turned()));
	let total: u64 = pictures
		.map(|canvas| {
			let ladder = crate::ladder::sizes(canvas.size()).into_iter();
			let video: u64 = ladder
				.map(|size| crate::ladder::ceiling(size, canvas.framerate()).as_bps())
				.sum();
			video + crate::engine::AUDIO_BITRATE.as_bps()
		})
		.sum();
	let both = match both {
		true => " in both formats",
		false => "",
	};
	let name = named_size(canvas)
		.and_then(|short| SIZES.iter().find(|(side, _)| *side == short))
		.map_or_else(
			|| format!("{}x{}", canvas.width, canvas.height),
			|(_, name)| name.to_string(),
		);
	format!(
		"Bigger needs a faster encoder and more upload: {name} at {}{both} wants up to about {:.1} Mb/s.",
		canvas.framerate,
		total as f64 / 1e6
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn canvas(width: u32, height: u32, framerate: u32) -> Canvas {
		Canvas {
			width,
			height,
			framerate,
		}
	}

	/// The portrait picture is a broadcast of its own, as costly as the
	/// landscape one.
	#[test]
	fn both_formats_want_twice_the_upload() {
		let canvas = canvas(1920, 1080, 30);
		let wanted = |both| {
			let said = upload(canvas, both);
			let number = said.rsplit("about ").next().unwrap().trim_end_matches(" Mb/s.");
			number.parse::<f64>().unwrap()
		};
		assert!(upload(canvas, true).contains("1080p at 30 in both formats"));
		assert!(
			(wanted(true) - 2.0 * wanted(false)).abs() <= 0.1,
			"{} {}",
			wanted(true),
			wanted(false)
		);
	}

	#[test]
	fn a_named_size_keeps_the_format_and_rate() {
		let landscape = canvas(1920, 1080, 60);
		assert_eq!(named_size(landscape), Some(1080));
		assert_eq!(sized(landscape, 720), canvas(1280, 720, 60));
		assert_eq!(sized(landscape, 2160), canvas(3840, 2160, 60));
		let portrait = canvas(1080, 1920, 30);
		assert_eq!(named_size(portrait), Some(1080));
		assert_eq!(sized(portrait, 1440), canvas(1440, 2560, 30));
		// Not 16:9, so Custom.
		assert_eq!(named_size(canvas(1080, 1080, 30)), None);
		assert_eq!(named_size(canvas(1366, 768, 30)), None);
	}
}
