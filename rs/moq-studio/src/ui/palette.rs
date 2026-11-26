//! The command palette: one key opens a search box that runs anything the
//! studio can do. Its actions are the table in `controls`, each with its key;
//! one that cannot run now is left out until searched for, then listed last,
//! saying why. Beside them are the pages, the settings, and what can be added
//! or selected, each found by its own name.

use egui::{Align2, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::*;
use crate::devices::Pick;

/// The key that opens the palette, and closes it again.
pub(super) const OPEN: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K);

/// The palette's width, and the most of the window its list takes.
const WIDTH: f32 = 600.0;
const LIST: f32 = 420.0;

/// The palette, while open: what is typed, and the row the arrows picked
/// among those it finds.
pub(super) struct Palette {
	query: String,
	/// The row Enter runs, if any: none once the picked row is gone, such as
	/// a camera unplugged, until the arrows, the pointer, or typing pick one.
	picked: Option<usize>,
	/// The picked row's [`Row::id`], which keeps it picked as rows found
	/// since, such as devices, move it.
	picked_row: Option<String>,
}

impl Palette {
	/// Follow the picked row to where it is in `rows`, or pick none if it is
	/// gone, then move by the arrows pressed.
	fn repick(&mut self, rows: &[Row], up: bool, down: bool) {
		if let Some(picked) = &self.picked_row {
			self.picked = rows.iter().position(|row| row.id() == *picked);
		}
		let last = rows.len().saturating_sub(1);
		self.picked = match (self.picked, up, down) {
			(Some(picked), true, false) => Some(picked.saturating_sub(1)),
			(Some(picked), false, true) => Some((picked + 1).min(last)),
			(None, true, false) | (None, false, true) => Some(0),
			(picked, _, _) => picked.map(|picked| picked.min(last)),
		};
		self.picked_row = self.picked.and_then(|picked| rows.get(picked)).map(Row::id);
	}
}

impl Default for Palette {
	fn default() -> Self {
		Self {
			query: String::new(),
			picked: Some(0),
			picked_row: None,
		}
	}
}

/// What a row does.
#[derive(Clone, Debug)]
enum Command {
	/// An action from the table, as its hotkey fires it.
	Act(Action),
	Page(Page),
	/// Open Settings at a pane, at the row named, if any.
	Setting(Setting, Option<&'static str>),
	/// Select a layer on Studio, the item of this id there.
	Layer(Picked, moq_scene::Id),
	/// Add a device the last listing found as a layer of the scene Studio
	/// edits.
	Device(Pick),
	/// Open Add over Studio's picture.
	Add,
	Undo,
	Redo,
	Multiview,
	Health,
	Graph,
}

/// A row of the palette.
struct Row {
	command: Command,
	name: String,
	/// What it is for, shown beside it.
	group: String,
	/// What else finds it.
	words: Vec<&'static str>,
	/// The key bound to it.
	key: Option<String>,
	/// Why it cannot run now.
	refused: Option<String>,
}

impl Row {
	/// What tells it from every other row as the rows change: the device it
	/// adds, as the system knows it, since two of one model share a name, the
	/// layer it selects, since a duplicate shares its source, or else its
	/// name and group.
	fn id(&self) -> String {
		match &self.command {
			Command::Device(pick) => format!("{:?}", pick.source),
			Command::Layer(_, id) => format!("{id:?}"),
			_ => format!("{} {}", self.name, self.group),
		}
	}

	fn new(command: Command, name: impl Into<String>, group: impl Into<String>, words: &[&'static str]) -> Self {
		Self {
			command,
			name: name.into(),
			group: group.into(),
			words: words.to_vec(),
			key: None,
			refused: None,
		}
	}

	/// How well `query` finds it, or `None` if it does not.
	fn score(&self, query: &str) -> Option<u32> {
		let also = format!("{} {}", self.group, self.words.join(" "));
		score(query, &self.name, &also)
	}
}

/// How well `query` finds a row called `name` that also answers to `also`:
/// each of its words must start a word there, or else appear inside one,
/// and a word starting one of the name's counts most. `None` if a word is
/// found nowhere.
fn score(query: &str, name: &str, also: &str) -> Option<u32> {
	let (name, also) = (name.to_lowercase(), also.to_lowercase());
	let starts = |text: &str, term: &str| words(text).any(|word| word.starts_with(term));
	let mut score = 0;
	for term in words(&query.to_lowercase()) {
		score += if starts(&name, term) {
			3
		} else if starts(&also, term) {
			2
		} else if name.contains(term) || also.contains(term) {
			1
		} else {
			return None;
		};
	}
	Some(score)
}

/// The words of `text`, which punctuation and spaces split.
fn words(text: &str) -> impl Iterator<Item = &str> {
	text.split(|c: char| !c.is_alphanumeric())
		.filter(|word| !word.is_empty())
}

/// The rows `query` finds, best first, and in the order listed among equals.
/// Those that cannot run now come last, and only once something is typed, so
/// the list as it opens holds only what runs.
fn found(rows: Vec<Row>, query: &str) -> Vec<Row> {
	let typed = !query.trim().is_empty();
	let mut scored: Vec<(u32, Row)> = rows
		.into_iter()
		.filter(|row| typed || row.refused.is_none())
		.filter_map(|row| row.score(query).map(|score| (score, row)))
		.collect();
	scored.sort_by_key(|(score, row)| (row.refused.is_some(), std::cmp::Reverse(*score)));
	scored.into_iter().map(|(_, row)| row).collect()
}

impl Panels {
	/// Open the palette, or close it if open.
	pub(super) fn toggle_palette(&mut self) {
		if self.palette.take().is_some() {
			return;
		}
		// The search box takes the keyboard from a page the picture passed it
		// to, which would type into both.
		if self.interacting {
			let engine = self.engine.clone();
			self.stop_interacting(&engine.state.lock().unwrap());
		}
		self.palette = Some(Palette::default());
		// What can be added is listed afresh, so a camera plugged in since shows.
		self.devices.renew(&self.runtime);
	}

	/// Everything the palette offers now, in the order it lists them before
	/// anything is typed.
	fn palette_rows(&self, ctx: &egui::Context) -> Vec<Row> {
		let studio = self.controls.studio();
		let bindings = self.hotkeys.as_ref().map(Hotkeys::bindings).unwrap_or_default();
		let state = self.engine.state.lock().unwrap();
		let collection = state.collection();
		let mut rows = Vec::new();
		let mods = &self.controls.mods;
		let on = |needs: Option<Mod>| needs.is_none_or(|needs| mods.on(needs));

		for action in Action::listed(&state, &self.controls.mods) {
			let name = match &action {
				Action::Mute(name) if collection.audio.get(name).is_some_and(|audio| audio.muted) => {
					format!("Unmute `{name}`")
				}
				Action::Mute(name) => format!("Mute `{name}`"),
				action => action.name(studio),
			};
			let mut row = Row::new(
				Command::Act(action.clone()),
				name,
				action.group().name(),
				action.words(),
			);
			row.key = bindings.get(&action).map(|bound| bound.shown(ctx));
			row.refused = self.controls.refused(&state, &action);
			rows.push(row);
		}

		// The layers Studio edits, by the source each shows: off air, only the
		// scene's own, as the always-on-top ones are drawn on program.
		if let Ok(layer) = self.scene(&state) {
			let off_air = self.off_air(&state).is_some();
			let layers = [(!off_air).then(Layer::top), Some(layer)];
			for top in layers.into_iter().flatten() {
				let items = collection.items(&top).unwrap_or_default();
				for (index, item) in items.iter().enumerate().rev() {
					let picked = Picked {
						layer: top.clone(),
						index,
					};
					let group = match top.scene {
						Some(_) => "Layers",
						None => "Layers, always on top",
					};
					let name = format!("Select `{}`", item.source);
					rows.push(Row::new(
						Command::Layer(picked, item.id),
						name,
						group,
						&["layer", "source", "inspect"],
					));
				}
			}
		}

		// What can be added: each device listed as the palette opened, once
		// found, not the listing before it, whose display a monitor unplugged
		// since can make another; and Add itself.
		let devices = &["device", "source", "layer"];
		if let Some(listing) = self.devices.listing.as_ref().filter(|_| self.devices.pending.is_none()) {
			let kinds = [
				("Cameras", &listing.cameras),
				("Displays", &listing.displays),
				("Windows", &listing.windows),
				("Audio inputs", &listing.audio),
			];
			// Never the studio's own virtual cameras, which would feed what
			// one shows back into itself.
			let own: Vec<String> = state.cameras().map(|camera| camera.id).collect();
			let own = |pick: &&Pick| matches!(&pick.source, Source::Camera { id: Some(id) } if own.contains(id));
			for (kind, picks) in kinds {
				for pick in picks.iter().flatten().filter(|pick| !own(pick)) {
					let name = format!("Add `{}`", pick.name);
					rows.push(Row::new(
						Command::Device(pick.clone()),
						name,
						format!("Add, {kind}"),
						devices,
					));
				}
			}
		}
		rows.push(Row::new(
			Command::Add,
			"Add a source",
			"Add",
			&[
				"new",
				"layer",
				"camera",
				"screen",
				"window",
				"microphone",
				"file",
				"web",
				"guest",
			],
		));

		for page in Page::ALL.into_iter().filter(|page| on(page.needs())) {
			rows.push(Row::new(
				Command::Page(page),
				format!("Go to {}", page.name()),
				"Pages",
				&["page"],
			));
		}
		for setting in Setting::ALL.into_iter().filter(|setting| on(setting.needs())) {
			let mut words = vec!["settings", "preferences"];
			words.extend(setting.words());
			let row = Row::new(Command::Setting(setting, None), setting.name(), "Settings", &words);
			rows.push(row);
			let group = format!("Settings, {}", setting.name());
			for (name, words, _) in setting.rows().iter().filter(|(_, _, needs)| on(*needs)) {
				rows.push(Row::new(Command::Setting(setting, Some(name)), *name, &group, words));
			}
		}
		// Each mod, off or on, so one can be found to switch on.
		for each in Mod::ALL {
			let group = format!("Settings, {}", Setting::Mods.name());
			rows.push(Row::new(
				Command::Setting(Setting::Mods, Some(each.name())),
				each.name(),
				group,
				each.words(),
			));
		}

		let history = state.history();
		let position = history.position();
		let undo = position.checked_sub(1).and_then(|at| history.entries().nth(at));
		let redo = history.entries().nth(position);
		for (command, entry, verb) in [(Command::Undo, undo, "Undo"), (Command::Redo, redo, "Redo")] {
			let mut row = match entry {
				Some(edit) => Row::new(command, format!("{verb}: {}", edit.label), "Edit", &["history"]),
				None => Row::new(command, verb, "Edit", &["history"]),
			};
			row.refused = entry.is_none().then(|| format!("nothing to {}", verb.to_lowercase()));
			rows.push(row);
		}
		if mods.on(Mod::Multiview) {
			let multiview = match self.multiview.open() {
				true => "Close the multiview",
				false => "Open the multiview",
			};
			rows.push(Row::new(
				Command::Multiview,
				multiview,
				"Windows",
				&["monitor", "tiles", "sources"],
			));
		}
		rows.push(Row::new(
			Command::Health,
			"Stream health in detail",
			"Windows",
			&["encoder", "viewers", "problems"],
		));
		if mods.on(Mod::Graph) {
			rows.push(Row::new(
				Command::Graph,
				"Open the scene graph",
				"Windows",
				&["wires", "nodes", "map"],
			));
		}
		rows
	}

	/// The palette, while open: the search box, and under it the rows it
	/// finds, each with its key, and why it cannot run if it cannot. The
	/// arrows pick a row, Enter or a click runs it, and Escape closes.
	pub(super) fn palette(&mut self, ctx: &egui::Context) {
		let Some(mut palette) = self.palette.take() else { return };
		// The listing asked for as it opened arrives on any page.
		self.devices.poll(&self.runtime);
		if self.devices.pending.is_some() {
			ctx.request_repaint_after(Duration::from_millis(100));
		}
		let all = self.palette_rows(ctx);
		// Taken before the search box sees them, which would move its cursor
		// or lose focus on them.
		let (up, down, enter) = ctx.input_mut(|input| {
			(
				input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
				input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
				input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
			)
		});

		let frame = egui::Frame::new()
			.fill(kit::P1)
			.stroke(Stroke::new(1.0, egui::Color32::from_rgb(0x3b, 0x3b, 0x4a)))
			.corner_radius(14)
			.shadow(egui::Shadow {
				offset: [0, 24],
				blur: 80,
				spread: 0,
				color: egui::Color32::BLACK,
			});
		let top = (ctx.content_rect().height() * 0.12).min(96.0);
		let area = egui::Modal::default_area(egui::Id::new("palette")).anchor(Align2::CENTER_TOP, vec2(0.0, top));
		let width = WIDTH.min(ctx.content_rect().width() - 24.0);
		let modal = egui::Modal::new(egui::Id::new("palette"))
			.area(area)
			.frame(frame)
			.backdrop_color(egui::Color32::from_rgba_unmultiplied(6, 6, 11, 0x99))
			.show(ctx, |ui| {
				ui.set_width(width);
				ui.add_space(12.0);
				ui.horizontal(|ui| {
					ui.add_space(14.0);
					ui.add(Icon::Search.image(18.0, kit::DIM));
					let field = egui::TextEdit::singleline(&mut palette.query)
						.hint_text("Run anything: a scene, a sound, an action, a setting")
						.font(kit::ui(16.0))
						.frame(egui::Frame::NONE)
						.desired_width(ui.available_width() - 14.0);
					let field = ui.add(field);
					field.request_focus();
					if field.changed() {
						palette.picked = Some(0);
						palette.picked_row = None;
					}
				});
				// Found from what the box holds after this frame's typing, so
				// Enter pressed with it runs what was typed, not what was before.
				let rows = found(all, &palette.query);
				palette.repick(&rows, up, down);
				let mut run = palette.picked.filter(|_| enter);
				ui.add_space(10.0);
				let y = ui.cursor().top();
				ui.painter()
					.hline(ui.max_rect().x_range(), y, Stroke::new(1.0, kit::LINE));
				ui.add_space(6.0);
				let height = LIST.min(ctx.content_rect().height() - top - 140.0).max(120.0);
				egui::ScrollArea::vertical()
					.max_height(height)
					.auto_shrink([false, true])
					.show(ui, |ui| {
						if rows.is_empty() {
							ui.add_space(10.0);
							ui.vertical_centered(|ui| {
								ui.label(RichText::new("Nothing found").font(kit::ui(13.0)).color(kit::DIM));
							});
							ui.add_space(10.0);
						}
						for (index, row) in rows.iter().enumerate() {
							let picked = palette.picked == Some(index);
							let response = row_ui(ui, row, picked);
							if (up || down) && picked {
								response.scroll_to_me(None);
							}
							if response.hovered() && ui.input(|input| input.pointer.delta() != egui::Vec2::ZERO) {
								palette.picked = Some(index);
								palette.picked_row = Some(row.id());
							}
							if response.clicked() {
								run = Some(index);
							}
						}
					});
				ui.add_space(6.0);
				let hint = "↑ ↓ pick  ·  Enter runs  ·  Esc closes";
				ui.horizontal(|ui| {
					ui.add_space(14.0);
					ui.label(RichText::new(hint).font(kit::ui(11.0)).color(kit::FAINT));
				});
				ui.add_space(8.0);
				run.and_then(|index| rows.into_iter().nth(index))
			});
		let mut open = !modal.should_close();
		if let Some(row) = modal.inner {
			match &row.refused {
				// Shown on its row already; the palette stays open on it.
				Some(_) => {}
				None => {
					open = false;
					self.run(ctx, row.command);
				}
			}
		}
		if open {
			self.palette = Some(palette);
		}
	}

	/// Do what a row of the palette does. One that turns to a page or opens
	/// a window closes Settings, which would hide it.
	fn run(&mut self, ctx: &egui::Context, command: Command) {
		if matches!(
			command,
			Command::Act(Action::GoLive)
				| Command::Act(Action::EndStream)
				| Command::Page(_)
				| Command::Layer(..)
				| Command::Device(_)
				| Command::Add
				| Command::Health
				| Command::Graph
		) {
			self.close_settings();
		}
		match command {
			// The window's GO LIVE: on air once the check passes, or its
			// popover, which asks for a relay the show lacks.
			Command::Act(Action::GoLive) => self.press_go_live(),
			// The clock's End stream popover, as a click on it opens it.
			Command::Act(Action::EndStream) => self.confirm_end = true,
			Command::Act(action) => self.controls.press(&action),
			Command::Page(page) => self.page = page,
			Command::Setting(setting, row) => {
				self.setting = Some(setting);
				if let Some(row) = row {
					super::settings::seek(ctx, row);
				}
			}
			Command::Layer(picked, id) => {
				// By its id, as a guest taking a tile since the rows were
				// found may have moved its index.
				let state = self.engine.state.lock().unwrap();
				let items = state.collection().items(&picked.layer).unwrap_or_default();
				let index = items.iter().position(|item| item.id == id);
				drop(state);
				let Some(index) = index else {
					self.report(Err(anyhow::anyhow!("that layer is gone")));
					return;
				};
				self.page = Page::Studio;
				self.side = Side::Layers;
				self.rail = true;
				self.select(Some(Picked { index, ..picked }));
			}
			Command::Device(pick) => {
				self.page = Page::Studio;
				let mut state = self.engine.state.lock().unwrap();
				// Two cameras of the same model share a name; number the later ones.
				let name = state.collection().unused_name(&pick.name);
				let layer = self.scene(&state).ok();
				let new = New {
					name,
					source: pick.source,
					layer,
					size: pick.size,
				};
				let result = add(&mut state, new);
				drop(state);
				self.added(result);
				if let Some(fault) = self.fault(&Spot::Add) {
					let fault = fault.to_string();
					self.say(Err(fault));
				}
			}
			Command::Add => {
				self.page = Page::Studio;
				self.open_add();
			}
			Command::Undo => self.undo(),
			Command::Redo => self.redo(),
			Command::Multiview => self.multiview.set_open(!self.multiview.open()),
			Command::Health => self.details = true,
			Command::Graph => self.graph_open = true,
		}
	}
}

/// A row of the palette, lit while `picked`: its name, what it is for, and
/// its key, with why it cannot run under it, if it cannot.
fn row_ui(ui: &mut Ui, row: &Row, picked: bool) -> egui::Response {
	let height = if row.refused.is_some() { 48.0 } else { 34.0 };
	let width = ui.available_width();
	let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
	// Keyed by the row, not its place, so a row found mid-click cannot take
	// the click.
	let response = ui.interact(rect, ui.id().with(("row", row.id())), Sense::click());
	let enabled = row.refused.is_none();
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &row.name));
	if !ui.is_rect_visible(rect) {
		return response;
	}
	let inner = rect.shrink2(vec2(8.0, 0.0));
	if picked {
		ui.painter().rect_filled(inner, 8, kit::P3);
	}
	let inner = inner.shrink2(vec2(10.0, 0.0));
	let mut right = inner.right();
	if let Some(key) = &row.key {
		let cap = ui.painter().layout_no_wrap(key.clone(), kit::mono(10.0), kit::TEXT);
		let size = cap.size() + vec2(10.0, 6.0);
		let at = Rect::from_min_size(pos2(right - size.x, rect.top() + 17.0 - size.y / 2.0), size);
		ui.painter().rect(
			at,
			4,
			egui::Color32::from_black_alpha(0x66),
			Stroke::new(1.0, egui::Color32::from_white_alpha(0x22)),
			egui::StrokeKind::Inside,
		);
		ui.painter().galley(at.center() - cap.size() / 2.0, cap, kit::TEXT);
		right = at.left() - 10.0;
	}
	let group = ui
		.painter()
		.layout_no_wrap(row.group.clone(), kit::ui(11.5), kit::FAINT);
	let group_at = pos2(right - group.size().x, rect.top() + 17.0 - group.size().y / 2.0);
	right = group_at.x - 12.0;
	ui.painter().galley(group_at, group, kit::FAINT);
	let ink = if enabled { kit::TEXT } else { kit::DIM };
	let name = ui.painter().layout(
		row.name.clone(),
		kit::ui_medium(13.5),
		ink,
		(right - inner.left()).max(40.0),
	);
	let rows = name.rows.len().max(1) as f32;
	let name_at = pos2(inner.left(), rect.top() + 17.0 - name.size().y / rows / 2.0);
	ui.painter()
		.with_clip_rect(Rect::from_x_y_ranges(inner.left()..=right, rect.y_range()))
		.galley(name_at, name, ink);
	if let Some(why) = &row.refused {
		let why = ui
			.painter()
			.layout_no_wrap(format!("Cannot now: {why}"), kit::ui(11.5), kit::CUE);
		ui.painter()
			.with_clip_rect(rect)
			.galley(pos2(inner.left(), rect.top() + 30.0), why, kit::CUE);
	}
	response
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_pick_follows_its_row_and_is_lost_with_it() {
		let device = |name: &str, id: &str| {
			let pick = Pick {
				name: name.into(),
				source: Source::Camera { id: Some(id.into()) },
				size: None,
			};
			Row::new(Command::Device(pick), name, "Devices", &[])
		};
		let add = || Row::new(Command::Add, "Add a source", "Studio", &[]);
		let mut palette = Palette::default();
		palette.repick(&[add()], false, false);
		assert_eq!(palette.picked, Some(0));
		// Two cameras of one model, listed above it as they arrive.
		let listed = [device("Cam", "a"), device("Cam", "b"), add()];
		palette.repick(&listed, false, false);
		assert_eq!(palette.picked, Some(2), "still Add a source");
		palette.repick(&listed, true, false);
		palette.repick(&listed, true, false);
		assert_eq!(palette.picked, Some(0));
		// The first camera unplugged: its twin does not take its place.
		let unplugged = [device("Cam", "b"), add()];
		palette.repick(&unplugged, false, false);
		assert_eq!(palette.picked, None, "Enter runs nothing until a pick");
		palette.repick(&unplugged, false, true);
		assert_eq!(palette.picked, Some(0));

		// A duplicated layer shares its source's name; its row is its own.
		let layer = |index: usize| {
			let picked = Picked {
				layer: Layer::scene("Main"),
				index,
			};
			Row::new(
				Command::Layer(picked, moq_scene::Id::fresh()),
				"Select `cam`",
				"Layers",
				&[],
			)
		};
		let layers = [layer(1), layer(0)];
		palette.repick(&layers, false, true);
		palette.repick(&layers, false, true);
		assert_eq!(palette.picked, Some(1));
		palette.repick(&layers, false, false);
		assert_eq!(palette.picked, Some(1), "still the second, not the first of that name");
	}

	fn names(rows: Vec<Row>, query: &str) -> Vec<String> {
		found(rows, query).into_iter().map(|row| row.name).collect()
	}

	fn rows() -> Vec<Row> {
		let act = |action: Action| {
			let name = action.name(false);
			Row::new(
				Command::Act(action.clone()),
				name,
				action.group().name(),
				action.words(),
			)
		};
		vec![
			act(Action::GoLive),
			act(Action::EndStream),
			act(Action::Record),
			act(Action::Scene("Gameplay".into())),
			act(Action::Mute("desk mic".into())),
			Row::new(
				Command::Setting(Setting::Show, Some("Frame rate")),
				"Frame rate",
				"Settings, Show",
				&["fps"],
			),
			Row::new(
				Command::Setting(Setting::Recording, None),
				"Recording",
				"Settings",
				&["folder"],
			),
		]
	}

	/// Each word typed must be found, at the start of a word or inside one,
	/// in the name, what it is for, or what else it answers to.
	#[test]
	fn every_word_typed_is_found() {
		assert_eq!(names(rows(), "mute desk"), ["Mute or unmute `desk mic`"]);
		assert_eq!(names(rows(), "fps"), ["Frame rate"]);
		assert_eq!(names(rows(), "game"), ["Take scene `Gameplay`"]);
		assert_eq!(names(rows(), "folder"), ["Recording"]);
		assert!(names(rows(), "mute nobody").is_empty());
		// Inside a word, as a last resort.
		assert_eq!(names(rows(), "play"), ["Take scene `Gameplay`"]);
	}

	/// A word of the name beats what else a row answers to, which beats a
	/// match inside a word; equals keep the order listed, actions first, as
	/// does an empty query.
	#[test]
	fn the_name_ranks_first() {
		assert_eq!(names(rows(), "rec"), ["Start or stop recording", "Recording"]);
		assert_eq!(
			names(rows(), "st"),
			["End stream", "Start or stop recording", "Go live"]
		);
		// The scene's name over its group's, which every scene shares.
		assert_eq!(names(rows(), "scene"), ["Take scene `Gameplay`"]);
		assert_eq!(names(rows(), "").len(), rows().len());
		assert_eq!(names(rows(), "  ")[0], "Go live");
	}

	/// What cannot run now is left out of the list as it opens, and comes
	/// after everything that can once searched for, saying why.
	#[test]
	fn what_cannot_run_waits_to_be_searched_for() {
		let off_air = || {
			let mut listed = rows();
			listed[1].refused = Some("the program is not on air".into());
			listed
		};
		let shown = names(off_air(), "");
		assert!(!shown.contains(&"End stream".to_string()), "{shown:?}");
		assert_eq!(shown.len(), rows().len() - 1);
		assert_eq!(
			names(off_air(), "st"),
			["Start or stop recording", "Go live", "End stream"]
		);
	}

	/// Punctuation typed is no part of a word, as in `go-live`.
	#[test]
	fn punctuation_splits_words() {
		assert_eq!(names(rows(), "go-live")[0], "Go live");
		assert_eq!(names(rows(), "`gameplay`")[0], "Take scene `Gameplay`");
	}
}
