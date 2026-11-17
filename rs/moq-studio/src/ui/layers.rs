//! The BUILD page's left column: the layers the picture draws, front to back,
//! and what can be added to it.

use egui::{Align2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::build::{LayerAction, Menu, OFF_AIR_TOP, layer_menu, shown_in};
use super::*;

/// A layer row's height.
const ROW: f32 = 46.0;

impl Panels {
	/// The left column in `rect`: Layers and Add, and the inspector too when
	/// `narrow` leaves no room for its own column.
	pub(super) fn side_column(&mut self, ui: &mut Ui, rect: egui::Rect, narrow: bool) {
		let tabs: &[Side] = match narrow {
			true => &[Side::Layers, Side::Add, Side::Inspect],
			false => &Side::ALL,
		};
		if !tabs.contains(&self.side) {
			self.side = Side::Layers;
		}
		ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(12.0)), |ui| {
			let mut side = self.side;
			super::shell::tabs(ui, &mut side, tabs, Side::name);
			self.side = side;
			ui.add_space(8.0);
			match self.side {
				Side::Layers => self.layers(ui),
				Side::Add => {
					egui::ScrollArea::vertical()
						.id_salt("add")
						.auto_shrink(false)
						.show(ui, |ui| self.adding(ui));
				}
				Side::Inspect => self.inspector_body(ui),
			}
		});
	}

	/// The layers of the scene BUILD edits and the always-on-top ones over
	/// it, front first, with Add a layer under them. A scene open off air
	/// has only its own, as the always-on-top ones are drawn on program.
	fn layers(&mut self, ui: &mut Ui) {
		let footer = 40.0;
		let list = ui
			.available_rect_before_wrap()
			.with_max_y(ui.max_rect().bottom() - footer - 8.0);
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let mut acted = None;
		ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
			egui::ScrollArea::vertical()
				.id_salt("layers")
				.auto_shrink(false)
				.show(ui, |ui| {
					ui.spacing_mut().item_spacing.y = 0.0;
					let groups = [(Layer::top(), "Always on top"), (Layer::scene(""), "This scene")];
					let off_air = self.off_air(&state).is_some();
					for (layer, caption) in groups {
						if off_air && layer.scene.is_none() {
							caption_row(ui, caption);
							ui.label(egui::RichText::new(OFF_AIR_TOP).color(kit::DIM));
							ui.add_space(8.0);
							continue;
						}
						let layer = match layer.scene {
							None => layer,
							Some(_) => match self.scene(&state) {
								Ok(scene) => scene,
								Err(why) => {
									caption_row(ui, caption);
									ui.label(egui::RichText::new(why).color(kit::DIM));
									continue;
								}
							},
						};
						caption_row(ui, caption);
						if let Some(group) = self.group(ui, &state, &layer) {
							acted = Some(group);
						}
						ui.add_space(8.0);
					}
				});
		});
		let button = Rect::from_min_size(
			pos2(ui.max_rect().left(), ui.max_rect().bottom() - footer),
			vec2(ui.max_rect().width(), footer),
		);
		if ui.put(button, AddLayer).clicked() {
			self.side = Side::Add;
		}
		let Some((picked, action)) = acted else { return };
		let (spot, result) = self.act(ui.ctx(), &mut state, &picked, action);
		drop(state);
		if let Some(spot) = spot {
			self.fail(spot, result);
		}
	}

	/// The rows of `layer`'s layers, front first; a row's action, if one was
	/// asked for.
	fn group(&mut self, ui: &mut Ui, state: &State, layer: &Layer) -> Option<(Picked, LayerAction)> {
		let collection = state.collection();
		let items = collection.items(layer).unwrap_or_default();
		if items.is_empty() {
			let empty = match layer.scene {
				None => "Nothing; a layer's menu keeps it on top of every scene",
				Some(_) => "Nothing yet; add a layer",
			};
			ui.add_space(4.0);
			ui.label(egui::RichText::new(empty).color(kit::FAINT).font(kit::ui(12.0)));
			ui.add_space(4.0);
			return None;
		}
		let now = kio::clock::now();
		let mut action = None;
		let mut rows = Vec::new();
		self.devices.poll();
		for index in (0..items.len()).rev() {
			let item = &items[index];
			let picked = Picked {
				layer: layer.clone(),
				index,
			};
			let selected = self.selection.as_ref() == Some(&picked);
			let status = status_of(state, &item.source, now);
			let capture = collection.sources.get(collection.capture(&item.source));
			// A window chosen by id offers the windows listed now in its place.
			if matches!(status.lost, Some(Lost::Dark(_)))
				&& matches!(capture, Some(Source::Window { .. }))
				&& self.devices.listing.is_none()
				&& self.devices.pending.is_none()
			{
				let refreshed = self.devices.refresh(&self.runtime).context("cannot list windows");
				self.fail(Spot::Add, refreshed);
			}
			let fault = self.fault(&Spot::Layer(layer.clone(), item.source.clone()));
			let row = Row {
				item,
				kind: kind(collection, &item.source),
				words: words(collection, &item.source),
				status,
				capture,
				windows: self.devices.listing.as_ref().map(|listing| &listing.windows),
				fault,
				selected,
			};
			let (response, asked) = row.show(ui, ui.id().with(("layer", layer.scene.as_deref(), index)));
			if let Some(asked) = asked {
				action = Some((picked.clone(), asked));
			}
			if response.clicked() || response.secondary_clicked() || response.drag_started() {
				self.select(Some(picked.clone()));
			}
			if response.drag_started() {
				self.moving = Some(index);
			}
			if let Some(menu) = Menu::of(state, &picked, self.off_air(state).is_some()) {
				response.context_menu(|ui| {
					if let Some(chosen) = layer_menu(ui, &menu) {
						action = Some((picked.clone(), chosen));
						ui.close();
					}
				});
			}
			rows.push((index, response));
		}
		// A dragged row lands where the pointer lets go among the others.
		let dragged = rows
			.iter()
			.find(|(index, response)| Some(*index) == self.moving && (response.dragged() || response.drag_stopped()));
		if let Some((from, response)) = dragged
			&& let Some(pointer) = response.interact_pointer_pos()
		{
			let from = *from;
			// Rows go front first, so the slot counted from the top is the
			// index counted from the back.
			let slot = rows
				.iter()
				.position(|(_, row)| pointer.y < row.rect.center().y)
				.unwrap_or(rows.len());
			let y = match rows.get(slot) {
				Some((_, row)) => row.rect.top(),
				None => rows.last().map_or(pointer.y, |(_, row)| row.rect.bottom()),
			};
			let line = rows[0].1.rect.x_range();
			ui.painter().hline(line, y, Stroke::new(2.0, kit::ACCENT));
			ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
			if response.drag_stopped() {
				self.moving = None;
				// The slot above row `slot` holds index `len - slot`; after
				// taking the row out, the indices past it shift down.
				let above = items.len() - slot;
				let to = if above > from { above - 1 } else { above };
				if to != from {
					action = Some((
						Picked {
							layer: layer.clone(),
							index: from,
						},
						LayerAction::Reorder(to),
					));
				}
			}
		}
		action
	}
}

/// A caption over a group of rows, as the mock-up's ALWAYS ON TOP.
fn caption_row(ui: &mut Ui, caption: &str) {
	ui.add_space(6.0);
	ui.label(kit::caps(caption, kit::display(11.0)).color(kit::DIM));
	ui.add_space(4.0);
}

/// A layer's row: its kind's icon, its name and what it shows, its status,
/// and its lock and eye.
struct Row<'a> {
	item: &'a Item,
	kind: Icon,
	words: String,
	status: Health,
	/// The source it captures, a copy's what it copies.
	capture: Option<&'a Source>,
	/// The windows this machine lists, once listed.
	windows: Option<&'a Result<Vec<Pick>, String>>,
	fault: Option<&'a str>,
	selected: bool,
}

impl Row<'_> {
	/// Draw the row with `id`; its response and what its lock or eye asked.
	fn show(self, ui: &mut Ui, id: egui::Id) -> (egui::Response, Option<LayerAction>) {
		let size = vec2(ui.available_width(), ROW);
		let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
		let response = ui.interact(rect, id, Sense::click_and_drag());
		response.widget_info(|| {
			egui::WidgetInfo::selected(egui::WidgetType::Button, true, self.selected, &self.item.source)
		});
		let painter = ui.painter_at(rect);
		let hovered = response.hovered() || response.dragged();
		if self.selected {
			painter.rect_filled(rect, 6, kit::ACCENT.gamma_multiply(0.14));
			painter.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0, kit::ACCENT);
		} else if hovered {
			painter.rect_filled(rect, 6, kit::P2);
		}
		let dim = !self.item.visible;
		let tile = Rect::from_min_size(rect.min + vec2(10.0, 9.0), Vec2::splat(28.0));
		let tile_fill = if self.selected { kit::ACCENT } else { kit::P3 };
		painter.rect_filled(tile, 7, tile_fill);
		let ink = if dim { kit::FAINT } else { kit::TEXT };
		self.kind.paint(ui, tile, 15.0, ink);

		// The lock and eye on the right, each a button of its own.
		let eye = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(24.0));
		let lock = eye.translate(vec2(-26.0, 0.0));
		let dot = Rect::from_center_size(pos2(lock.left() - 12.0, rect.center().y), Vec2::splat(14.0));
		let text_right = dot.left() - 6.0;

		let left = tile.right() + 10.0;
		let name = egui::RichText::new(&self.item.source)
			.font(kit::ui_medium(13.0))
			.color(ink);
		let galley = egui::WidgetText::from(name).into_galley(
			ui,
			Some(egui::TextWrapMode::Truncate),
			text_right - left,
			egui::TextStyle::Body,
		);
		painter.galley(pos2(left, rect.top() + 8.0), galley, ink);
		let (below, color) = match (self.fault, self.status.lost) {
			(Some(fault), _) => (fault.to_string(), kit::ERR),
			// Its icon says what kind it is.
			(None, Some(lost)) => (super::dark::words(lost), kit::CUE),
			(None, None) => (self.words, kit::DIM),
		};
		let small = egui::RichText::new(below).font(kit::ui(11.0)).color(color);
		let galley = egui::WidgetText::from(small).into_galley(
			ui,
			Some(egui::TextWrapMode::Truncate),
			text_right - left,
			egui::TextStyle::Small,
		);
		painter.galley(pos2(left, rect.top() + 26.0), galley, color);

		let mut asked = None;
		// A source live and well needs no mark; the inspector says so. One
		// gone dark or stalled has a badge in its place, which opens what to do.
		let marked = self.status.color != kit::OK || self.fault.is_some();
		let lost = self.status.lost.filter(|_| self.fault.is_none());
		let dark = lost.is_some();
		if let Some(lost) = lost
			&& let Some(capture) = self.capture
		{
			let badge = Rect::from_center_size(dot.center() - vec2(4.0, 0.0), Vec2::splat(20.0));
			let mend = super::dark::badge_button(
				ui,
				badge,
				id.with("dark"),
				&self.item.source,
				capture,
				lost,
				self.windows,
			);
			asked = mend.map(LayerAction::Mend);
		}
		let status = ui.interact(dot, id.with("status"), Sense::hover());
		if marked && !dark {
			let color = if self.fault.is_some() {
				kit::ERR
			} else {
				self.status.color
			};
			painter.circle_filled(dot.center(), 3.5, color);
		}
		if marked && !dark && (status.hovered() || self.fault.is_some()) {
			let mut lines = self.status.lines.clone();
			if let Some(fault) = self.fault {
				lines.insert(0, fault.to_string());
			}
			status.on_hover_text(lines.join("\n"));
		}
		let locked = self.item.locked;
		if locked || hovered || self.selected {
			let (icon, name) = match locked {
				true => (Icon::Lock, "Unlock"),
				false => (Icon::Unlock, "Lock"),
			};
			let button = IconButton::new(icon, name)
				.small()
				.lit(locked, kit::TEXT)
				.note(Some("A locked layer stays put: clicks on the picture pass through it"));
			if ui.put(lock, button).clicked() {
				asked = Some(LayerAction::Lock(!locked));
			}
		}
		let (icon, name) = match self.item.visible {
			true => (Icon::Shown, "Hide"),
			false => (Icon::Hidden, "Show"),
		};
		if ui.put(eye, IconButton::new(icon, name).small()).clicked() {
			asked = Some(LayerAction::Show(!self.item.visible));
		}
		(response, asked)
	}
}

/// The column's last row, which turns to Add.
struct AddLayer;

impl egui::Widget for AddLayer {
	fn ui(self, ui: &mut Ui) -> egui::Response {
		let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
		let response = kit::tip(response, "Add a layer", Some("A"));
		let fill = if response.hovered() { kit::P3 } else { kit::P2 };
		let painter = ui.painter();
		painter.rect(
			rect,
			kit::RADIUS,
			fill,
			Stroke::new(1.0, kit::LINE),
			egui::StrokeKind::Inside,
		);
		let icon = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(16.0));
		Icon::Add.paint(ui, icon, 15.0, kit::TEXT);
		painter.text(
			pos2(icon.right() + 10.0, rect.center().y),
			Align2::LEFT_CENTER,
			"Add a layer",
			kit::ui_medium(13.0),
			kit::TEXT,
		);
		let key = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(18.0, 18.0));
		painter.rect(key, 4, kit::P1, Stroke::new(1.0, kit::LINE), egui::StrokeKind::Inside);
		painter.text(key.center(), Align2::CENTER_CENTER, "A", kit::mono(10.0), kit::DIM);
		response
	}
}

/// How a source is doing, as a dot and the words for it.
pub(super) struct Health {
	pub(super) color: Color32,
	pub(super) lines: Vec<String>,
	/// Why it shows no new picture, once that counts: gone dark or stalled.
	pub(super) lost: Option<Lost>,
}

/// How `name` is doing: live, starting, stalled, ended, or failed with its
/// reason, with its track's state, how a web page paints, and how far behind
/// a broadcast plays.
pub(super) fn status_of(state: &State, name: &str, now: kio::clock::Instant) -> Health {
	let collection = state.collection();
	// A clone is as live as what it clones.
	let capture = collection.capture(name);
	let feed = state.sources.get(capture);
	let dark = state.dark(name, now);
	let lost = state.lost(name, now);
	let (mut color, first) = match (state.compositor.error(capture), feed, state.stalled(name, now)) {
		(Some(err), _, _) => (kit::ERR, format!("Failed: {err}")),
		(None, _, Some(stall)) => (kit::CUE, format!("Stalled: {}", health::without(stall))),
		(None, _, None) if let Some(dark) = dark => (kit::CUE, format!("All dark for {:.0} s", dark.as_secs_f64())),
		(None, Some(feed), None) => match feed.status() {
			Status::Starting => (kit::DIM, "Starting".to_string()),
			Status::Live => (kit::OK, "Live".to_string()),
			Status::Offline => (kit::CUE, "Offline".to_string()),
			Status::Ended => (kit::CUE, "Ended".to_string()),
			Status::Failed(reason) => (kit::ERR, format!("Failed: {reason}")),
			_ => (kit::DIM, "Unknown".to_string()),
		},
		// A scene is drawn from other sources, so it runs nothing.
		(None, None, None) if collection.nested(name).is_some() => (kit::OK, "Live".to_string()),
		(None, None, None) => (kit::DIM, "Starting".to_string()),
	};
	let mut lines = vec![first];
	// A track can fail while its source still draws, such as on a GPU out of
	// encode sessions. A clone publishes none, nor does a page or broadcast.
	match state.sources.get(name).and_then(Feed::track) {
		Some(Status::Failed(reason)) => {
			color = kit::ERR;
			lines.push(format!("Its track failed: {reason}"));
		}
		Some(Status::Live) => lines.push("Its track is live".into()),
		Some(Status::Starting) => lines.push("Its track is starting".into()),
		Some(_) => lines.push("Its track is off".into()),
		None if let Some(delay) = feed.and_then(Feed::delay) => lines.push(format!(
			"Plays {} ms behind what arrives, to ride out the network's jitter",
			delay.as_millis()
		)),
		None => match feed.and_then(Feed::page).and_then(|page| page.paint()) {
			Some(Paint::Gpu) => lines.push("Painted on the GPU".into()),
			Some(Paint::Cpu(why)) => {
				if color == kit::OK {
					color = kit::CUE;
				}
				lines.push(format!("Uploaded from memory every frame: {why}"));
			}
			None => {}
		},
	}
	Health { color, lines, lost }
}

/// The icon for what `name` shows, by its source's kind, or the kind of what
/// a copy copies.
pub(super) fn kind(collection: &Collection, name: &str) -> Icon {
	match collection.sources.get(collection.capture(name)) {
		Some(Source::Camera { .. }) => Icon::Camera,
		Some(Source::Display { .. } | Source::PickedDisplay) => Icon::Display,
		Some(Source::Window { .. } | Source::PickedWindow) => Icon::Window,
		Some(Source::Browser { .. }) => Icon::Web,
		Some(Source::Overlay { .. }) => Icon::Text,
		Some(Source::Image { .. }) => Icon::Image,
		Some(Source::Media { .. }) => Icon::Media,
		Some(Source::Color { .. }) => Icon::Color,
		Some(Source::Broadcast { .. } | Source::Layer { .. }) => Icon::Broadcast,
		Some(Source::Scene { .. } | Source::Program) => Icon::Scene,
		Some(Source::Replay) => Icon::Replay,
		_ => Icon::Image,
	}
}

/// What `name` shows, in words, and how many lists of layers share it.
pub(super) fn words(collection: &Collection, name: &str) -> String {
	let what = match collection.sources.get(collection.capture(name)) {
		Some(Source::Camera { .. }) => "Camera".to_string(),
		Some(Source::Display { .. }) => "Screen".to_string(),
		Some(Source::PickedDisplay) => "Screen, picked".to_string(),
		Some(Source::Window { .. }) => "Window".to_string(),
		Some(Source::PickedWindow) => "Window, picked".to_string(),
		Some(Source::Browser { .. }) => "Web page".to_string(),
		Some(Source::Overlay { .. }) => "Overlay".to_string(),
		Some(Source::Image { .. }) => "Image".to_string(),
		Some(Source::Media { .. }) => "Video file".to_string(),
		Some(Source::Color { color }) => format!("Color {color}"),
		Some(Source::Broadcast { path }) => format!("Broadcast {path}"),
		Some(Source::Layer { source, .. }) => format!("Broadcast layer {source}"),
		Some(Source::Scene { scene }) => format!("Scene {scene}"),
		Some(Source::Program) => "The program".to_string(),
		Some(Source::Replay) => "Replay".to_string(),
		_ => "Nothing".to_string(),
	};
	let what = match collection.sources.get(name) {
		Some(Source::Clone { source }) => format!("{what} · own copy of {source}"),
		_ => what,
	};
	match shown_in(collection, name).len() {
		0 | 1 => what,
		linked => format!("{what} · linked ×{linked}"),
	}
}

impl Panels {
	/// What can be added: the sources nothing shows, this machine's devices,
	/// any source by hand, and guests. The Add page replaces it with pictures
	/// (quest/m0/ux/add.md).
	fn adding(&mut self, ui: &mut Ui) {
		if let Some(fault) = self.fault(&Spot::Add) {
			ui.colored_label(kit::ERR, fault);
			ui.add_space(6.0);
		}
		self.unused(ui);
		section(ui, "Devices");
		self.devices(ui);
		section(ui, "Any source");
		self.by_hand(ui);
		section(ui, "Guests");
		self.guests(ui);
	}

	/// The sources no layer shows and no audio scene hears, each to add as a
	/// layer of the scene on program, or remove for good.
	fn unused(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let collection = state.collection();
		let heard = |name: &str| collection.audio_scenes.iter().any(|scene| scene.sounds.contains(name));
		let shown = |name: &str| {
			collection
				.layers()
				.any(|(_, items)| items.iter().any(|item| item.source == name))
		};
		let used_by_others = |name: &str| {
			let filters = collection
				.layers()
				.flat_map(|(_, items)| items.iter().flat_map(|item| &item.filters));
			let read = filters.chain(collection.video.values().flat_map(|video| &video.filters));
			let mut read = read.filter_map(Filter::source);
			let cloned = collection
				.sources
				.values()
				.any(|source| matches!(source, Source::Clone { source } if source == name));
			// Removing the stinger's source would quietly reset the transition.
			let stinger = collection.transition.stinger() == Some(name);
			cloned || stinger || read.any(|source| source == name)
		};
		let unused: Vec<String> = collection
			.sources
			.keys()
			.filter(|name| !shown(name) && !heard(name) && !used_by_others(name))
			.cloned()
			.collect();
		if unused.is_empty() {
			return;
		}
		section(ui, "Not in any scene");
		let mut add = None;
		let mut remove = None;
		for name in &unused {
			let source = &collection.sources[name];
			ui.horizontal(|ui| {
				let (icon, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
				kind(collection, name).paint(ui, icon, 15.0, kit::DIM);
				ui.vertical(|ui| {
					ui.label(egui::RichText::new(name).font(kit::ui_medium(13.0)));
					ui.label(
						egui::RichText::new(describe(source))
							.font(kit::ui(11.0))
							.color(kit::DIM),
					);
				});
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					let gone = IconButton::new(Icon::Remove, "Remove for good")
						.small()
						.note(Some("Remove the source from the show; undo brings it back"));
					if ui.add(gone).clicked() {
						remove = Some(name.clone());
					}
					if source.has_video() && ui.button("Add").clicked() {
						add = Some(name.clone());
					}
				});
			});
		}
		let result = if let Some(name) = add {
			match self.scene(&state) {
				Ok(layer) => {
					let canvas = state
						.collection()
						.canvas(layer.scene.as_deref())
						.unwrap_or(state.collection().canvas);
					let index = state.collection().items(&layer).map_or(0, <[Item]>::len);
					let item = Item::new(name.clone(), moq_scene::Rect::fill(&canvas));
					let result = edit(&mut state, format!("Add `{name}` {}", place(&layer)), None, |draft| {
						draft.insert_item(&layer, index, item)
					});
					if result.is_ok() {
						self.select(Some(Picked { layer, index }));
						self.side = Side::Layers;
					}
					result
				}
				Err(why) => Err(anyhow::anyhow!(why)),
			}
		} else if let Some(name) = remove {
			edit(&mut state, format!("Remove `{name}`"), None, |draft| {
				draft.remove_source(&name)
			})
		} else {
			return;
		};
		drop(state);
		self.fail(Spot::Add, result);
	}

	/// This machine's cameras, screens, and inputs, each added as a layer of
	/// the scene on program.
	fn devices(&mut self, ui: &mut Ui) {
		self.devices.poll();
		if ui.add(kit::button(Icon::Reload, "Refresh")).clicked() {
			let refreshed = self.devices.refresh(&self.runtime).context("cannot list devices");
			self.fail(Spot::Add, refreshed);
		}
		let own: Vec<String> = self
			.engine
			.state
			.lock()
			.unwrap()
			.cameras()
			.map(|camera| camera.id)
			.collect();
		let Some(picked) = self.devices.ui(ui, &own) else {
			return;
		};
		let mut state = self.engine.state.lock().unwrap();
		// Two cameras of the same model share a name; number the later ones.
		let name = state.collection().unused_name(&picked.name);
		let layer = self.scene(&state).ok();
		let result = add(&mut state, layer, name, picked.source, None);
		drop(state);
		self.added(result);
	}

	/// The form that adds any kind of source by hand.
	fn by_hand(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let scenes: Vec<String> = state
			.collection()
			.scenes
			.iter()
			.map(|scene| scene.name.clone())
			.collect();
		let Some(source) = self.add.ui(ui, &scenes) else { return };
		let name = self.add.name.trim().to_string();
		let layer = self.scene(&state).ok();
		let result = add(&mut state, layer, name, source, None);
		if result.is_ok() {
			self.add.name.clear();
		}
		drop(state);
		self.added(result);
	}

	/// Follow an added source to its layer, or show why it was not added.
	fn added(&mut self, result: anyhow::Result<String>) {
		match result {
			Ok(name) => {
				let state = self.engine.state.lock().unwrap();
				let picked = self.scene(&state).ok().and_then(|layer| {
					let items = state.collection().items(&layer)?;
					let index = items.iter().rposition(|item| item.source == name)?;
					Some(Picked { layer, index })
				});
				drop(state);
				if picked.is_some() {
					self.select(picked);
					self.side = Side::Layers;
				}
				self.fail(Spot::Add, Ok(()));
			}
			Err(err) => self.fail(Spot::Add, Err(err)),
		}
	}
}

/// A section's caption, under a line.
pub(super) fn section(ui: &mut Ui, caption: &str) {
	ui.add_space(10.0);
	let y = ui.cursor().top();
	ui.painter()
		.hline(ui.max_rect().x_range(), y, Stroke::new(1.0, kit::LINE));
	ui.add_space(10.0);
	ui.label(kit::caps(caption, kit::display(11.0)).color(kit::DIM));
	ui.add_space(4.0);
}

impl Panels {
	/// Invite guests, show their links, and see who joined.
	fn guests(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let Some(room) = engine.room() else {
			match self.controls.room_failure() {
				Some(failure) => ui.colored_label(kit::ERR, format!("Guests cannot join: {failure}")),
				None => ui.label("Guests join a room on your relay, which needs a room and its key."),
			};
			if ui.add(kit::button(Icon::Settings, "Set up the room")).clicked() {
				self.setting = Some(Setting::Guests);
			}
			return;
		};

		ui.horizontal(|ui| {
			ui.label("Name");
			let field = ui.text_edit_singleline(&mut self.guest);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			if ui.add(kit::button(Icon::Add, "Invite")).clicked() || entered {
				let name = self.guest.trim().to_string();
				match room.invite(&name) {
					Ok(invite) => {
						self.show(ui.ctx(), name, invite);
						self.guest.clear();
					}
					Err(err) => self.fail(Spot::Add, Err(err)),
				}
			}
		});

		let now = kio::clock::wall();
		if let Some(shown) = &self.shown {
			let title = format!("Invite {}", shown.name);
			let close = kit::Sheet::new(egui::Id::new("join link"), &title).show(ui.ctx(), |ui| {
				ui.label(format!("Their join link {}.", expiry(shown.invite.expires, now)));
				ui.add_space(8.0);
				// The link is the guest's credential, so it shows only as a code to scan.
				match &shown.qr {
					Ok(qr) => {
						ui.vertical_centered(|ui| ui.image((qr.id(), egui::vec2(240.0, 240.0))));
					}
					Err(err) => {
						ui.colored_label(ui.visuals().error_fg_color, format!("{err}; copy the link instead."));
					}
				}
				ui.add_space(8.0);
				if ui.add(kit::button(Icon::Copy, "Copy link")).clicked() {
					ui.ctx().copy_text(shown.invite.link.to_string());
				}
			});
			if close {
				self.shown = None;
			}
		}

		ui.separator();
		if let Some(failure) = room.renewal_failure() {
			ui.colored_label(
				ui.visuals().error_fg_color,
				format!("Guests' tokens are not renewing, so each leaves when their link expires: {failure}"),
			);
		}
		let participants = room.participants();
		if participants.is_empty() {
			ui.weak("Nobody invited yet.");
			return;
		}
		let mut show = None;
		let mut invite = None;
		let mut remove = None;
		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("guests").striped(true).show(ui, |ui| {
				for heading in ["Guest", "Camera", "Screen", "Link", ""] {
					ui.strong(heading);
				}
				ui.end_row();
				for (name, participant) in &participants {
					ui.label(name);
					for online in [participant.camera, participant.screen] {
						if online {
							ui.colored_label(kit::OK, "live");
						} else {
							ui.weak("offline");
						}
					}
					// A renewed guest no longer depends on their link's expiry.
					match &participant.invite {
						_ if participant.renewed() => ui.label("renewing"),
						_ if participant.removed => ui.weak("not renewed"),
						Some(link) => ui.label(expiry(link.expires, now)),
						None => ui.weak("invited before this session"),
					};
					ui.horizontal(|ui| {
						// Only a link that still works is worth sharing again.
						if let Some(link) = &participant.invite
							&& link.expires > now
							&& ui.small_button("Show link").clicked()
						{
							show = Some((name.clone(), link.clone()));
						}
						if ui.small_button("New link").clicked() {
							invite = Some(name.clone());
						}
						if ui
							.small_button("Remove from scenes")
							.on_hover_text(
								"Take their camera and screen, and any mask reading them, out of every scene and mute them. Their token stops renewing, so they leave within the link lifetime in Settings, Guests. Undo puts them back in the scenes; only New link renews them again.",
							)
							.clicked()
						{
							remove = Some(name.clone());
						}
					});
					ui.end_row();
				}
			});
		});

		if let Some((name, link)) = show {
			self.show(ui.ctx(), name, link);
		}
		if let Some(name) = invite {
			match room.invite(&name) {
				Ok(link) => self.show(ui.ctx(), name, link),
				Err(err) => self.fail(Spot::Add, Err(err)),
			}
		}
		if let Some(name) = remove {
			if let Err(err) = room.stop_renewing(&name) {
				self.report(Err(err));
				return;
			}
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft(format!("Remove `{name}` from scenes"));
			let result = match room.take_off(&mut draft, &name) {
				Ok(false) => Ok(()),
				Ok(true) => {
					// Removing items moves the ones after them.
					self.deselect_item();
					state.commit(draft, None)
				}
				Err(err) => Err(err.into()),
			};
			drop(state);
			self.report(result.map(|()| format!("removed {name} from every scene and muted them")));
		}
	}

	/// Put `invite`'s link for `name` on screen as a QR code.
	pub(super) fn show(&mut self, ctx: &egui::Context, name: String, invite: crate::guests::Invite) {
		let qr = qr(ctx, invite.link.as_str());
		self.shown = Some(Shown { name, invite, qr });
	}
}
