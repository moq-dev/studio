//! The Studio page's left column: the layers the picture draws, front to
//! back, and a button opening Add.

use egui::{Align2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::build::{LayerAction, OFF_AIR_TOP, layer_menu, shown_in};
use super::*;

/// A layer row's height.
const ROW: f32 = 46.0;

/// Whether nothing draws `name`: no layer shows it, and no clone, role,
/// filter, or the stinger reads it. Add lists it under Not in any scene
/// once no audio scene hears it either.
pub(super) fn unplaced(collection: &Collection, name: &str) -> bool {
	let shown = (collection.layers()).any(|(_, items)| items.iter().any(|item| item.source == name));
	let filters = collection
		.layers()
		.flat_map(|(_, items)| items.iter().flat_map(|item| &item.filters));
	let mut read =
		(filters.chain(collection.video.values().flat_map(|video| &video.filters))).filter_map(Filter::source);
	// A clone or a role naming it shows it.
	let cloned = collection.sources.values().any(|source| match source {
		Source::Clone { source } | Source::Role { source: Some(source) } => source == name,
		_ => false,
	});
	// Removing the stinger's source would quietly reset the transition.
	let stinger = collection.transition.stinger() == Some(name);
	!shown && !cloned && !stinger && !read.any(|source| source == name)
}

impl Panels {
	/// The left column in `rect`: Layers, and the inspector too while
	/// it has anything to show when `narrow` leaves no room for its own
	/// column.
	pub(super) fn side_column(&mut self, ui: &mut Ui, rect: egui::Rect, narrow: bool) {
		let tabs: &[Side] = match narrow && self.inspecting {
			true => &[Side::Layers, Side::Inspect],
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
				Side::Inspect => self.inspector_body(ui),
			}
		});
	}

	/// The layers of the scene Studio edits and the always-on-top ones over
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
					let pinned = state
						.collection()
						.items(&Layer::top())
						.is_some_and(|items| !items.is_empty());
					for (layer, caption) in groups.into_iter().filter(|(layer, _)| pinned || layer.scene.is_some()) {
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
			self.open_add();
		}
		let Some((picked, action)) = acted else { return };
		let (spot, result) = self.act(&mut state, &picked, action);
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
		self.devices.poll(&self.runtime);
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
			let window = matches!(status.lost, Some(Lost::Dark(_))) && matches!(capture, Some(Source::Window { .. }));
			if window {
				self.devices.list(&self.runtime);
			}
			let fault = self.fault(&Spot::Layer(layer.clone(), item.source.clone()));
			let what = words(collection, &item.source);
			let what = match item.slot {
				Some(moq_scene::Slot::Main) => format!("Main slot · {what}"),
				Some(moq_scene::Slot::Tile) => format!("Tile · {what}"),
				None => what,
			};
			let row = Row {
				item,
				kind: kind(collection, &item.source),
				words: what,
				status,
				capture,
				windows: self.devices.listing.as_ref().map(|listing| &listing.windows),
				fault,
				selected,
			};
			let (response, asked, opened) = row.show(ui, ui.id().with(("layer", layer.scene.as_deref(), index)));
			// Its popover, opened, lists the windows afresh: the listing kept
			// may be from before the window went, or another opened.
			if opened && window {
				self.devices.renew(&self.runtime);
			}
			if let Some(asked) = asked {
				action = Some((picked.clone(), asked));
			}
			if response.clicked() || response.secondary_clicked() || response.drag_started() {
				self.select(Some(picked.clone()));
			}
			if response.drag_started() {
				self.moving = Some(index);
			}
			// Dropped on a slot on the picture, it takes that slot.
			response.dnd_set_drag_payload(picked.clone());
			if let Some(menu) = self.menu(state, &picked) {
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
		let column = rows.first().map(|(_, row)| row.rect.x_range());
		if let Some((from, response)) = dragged
			&& let Some(pointer) = response.interact_pointer_pos()
			// Out over the picture, it is dropped on a slot instead.
			&& column.is_some_and(|column| column.contains(pointer.x))
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
	/// Draw the row with `id`; its response, what its lock, eye, or badge
	/// asked, and whether its badge was clicked open.
	fn show(self, ui: &mut Ui, id: egui::Id) -> (egui::Response, Option<LayerAction>, bool) {
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
		let mut opened = false;
		// A source live and well needs no mark; the inspector says so. One
		// gone dark or stalled has a badge in its place, which opens what to do.
		let marked = self.status.color != kit::OK || self.fault.is_some();
		let lost = self.status.lost.filter(|_| self.fault.is_none());
		let dark = lost.is_some();
		let badge = Rect::from_center_size(dot.center() - vec2(4.0, 0.0), Vec2::splat(20.0));
		if let Some(lost) = lost
			&& let Some(capture) = self.capture
		{
			let mend;
			(opened, mend) = super::dark::badge_button(
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
		// A failure besides, such as its track's, keeps its red dot beside
		// the badge, with the words on hover.
		if dark && self.status.color == kit::ERR {
			let beside = Rect::from_center_size(pos2(badge.left() - 8.0, dot.center().y), Vec2::splat(12.0));
			painter.circle_filled(beside.center(), 3.5, kit::ERR);
			let failed = ui.interact(beside, id.with("failed"), Sense::hover());
			if failed.hovered() {
				failed.on_hover_text(self.status.lines.join("\n"));
			}
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
			if kit::named(ui.put(lock, button), &format!("{name} {}", self.item.source)).clicked() {
				asked = Some(LayerAction::Lock(!locked));
			}
		}
		let (icon, name) = match self.item.visible {
			true => (Icon::Shown, "Hide"),
			false => (Icon::Hidden, "Show"),
		};
		let eye = ui.put(eye, IconButton::new(icon, name).small());
		if kit::named(eye, &format!("{name} {}", self.item.source)).clicked() {
			asked = Some(LayerAction::Show(!self.item.visible));
		}
		(response, asked, opened)
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
		Some(Source::Layer { path, .. }) => format!("Composed {path}"),
		Some(Source::Scene { scene }) => format!("Scene {scene}"),
		Some(Source::Program) => "The program".to_string(),
		Some(Source::Replay) => "Replay".to_string(),
		_ => "Nothing".to_string(),
	};
	let what = match collection.sources.get(name) {
		Some(Source::Clone { source }) => format!("{what} · own copy of {source}"),
		Some(Source::Role { source: Some(source) }) => format!("{what} · role, filled by {source}"),
		Some(Source::Role { source: None }) => "Role, unfilled".to_string(),
		_ => what,
	};
	match shown_in(collection, name).len() {
		0 | 1 => what,
		linked => format!("{what} · linked ×{linked}"),
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
