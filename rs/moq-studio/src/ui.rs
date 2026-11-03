//! The window: docked panels over the engine's state.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use egui::{Color32, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use moq_scene::source::{Feed, Status};
use moq_scene::{Clip, Collection, Draft, Filter, Item, Layer, Rect, Scene, Source, Video, wgpu};

use crate::engine::{Engine, State};
use crate::health::{self, Verdict};
use crate::replay::{self, Of};

/// The panels, each a tab the user can move, split, or stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Tab {
	Preview,
	Program,
	Scenes,
	History,
	Items,
	Sources,
	Devices,
	Filters,
	Audio,
	Output,
	Health,
	Replay,
}

impl Tab {
	fn title(self) -> &'static str {
		match self {
			Self::Preview => "Preview",
			Self::Program => "Program",
			Self::Scenes => "Scenes",
			Self::History => "History",
			Self::Items => "Items",
			Self::Sources => "Sources",
			Self::Devices => "Devices",
			Self::Filters => "Filters",
			Self::Audio => "Audio",
			Self::Output => "Output",
			Self::Health => "Health",
			Self::Replay => "Replay",
		}
	}
}

/// The studio window.
pub struct Studio {
	dock: DockState<Tab>,
	panels: Panels,
}

impl Studio {
	pub fn new(
		cc: &eframe::CreationContext<'_>,
		engine: Arc<Engine>,
		runtime: tokio::runtime::Handle,
	) -> anyhow::Result<Self> {
		let render = cc
			.wgpu_render_state
			.as_ref()
			.context("the window is not drawn with wgpu")?;

		// The compositor's textures live as long as the engine, so registering
		// them once is enough: egui samples whatever they hold each frame.
		let (preview, program, dropped) = {
			let state = engine.state.lock().unwrap();
			let mut renderer = render.renderer.write();
			let mut register = |texture: &wgpu::Texture| {
				let view = texture.create_view(&Default::default());
				renderer.register_native_texture(&render.device, &view, wgpu::FilterMode::Linear)
			};
			(
				register(state.compositor.preview()),
				register(state.compositor.program()),
				state.dropped,
			)
		};

		let mut dock = DockState::new(vec![Tab::Preview]);
		let tree = dock.main_surface_mut();
		let [top, bottom] = tree.split_below(NodeIndex::root(), 0.62, vec![Tab::Scenes, Tab::History]);
		tree.split_right(top, 0.5, vec![Tab::Program]);
		let [_, items] = tree.split_right(bottom, 0.18, vec![Tab::Items]);
		let [_, sources] = tree.split_right(items, 0.4, vec![Tab::Sources, Tab::Devices]);
		tree.split_right(
			sources,
			0.5,
			vec![Tab::Filters, Tab::Replay, Tab::Health, Tab::Audio, Tab::Output],
		);

		Ok(Self {
			dock,
			panels: Panels {
				engine,
				runtime,
				preview,
				program,
				top: false,
				selection: None,
				gestures: Gestures::default(),
				new_scene: String::new(),
				new_item: None,
				add: AddSource::default(),
				devices: Devices::default(),
				replay: Replay::default(),
				message: (dropped > 0).then(|| Err(dropped_message(dropped))),
			},
		})
	}
}

impl eframe::App for Studio {
	fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
		// Redraw at the canvas rate, so preview and program are live.
		let framerate = self.panels.engine.state.lock().unwrap().collection().canvas.framerate;
		ui.ctx().request_repaint_after(Duration::from_secs(1) / framerate);

		self.panels.shortcuts(ui.ctx());
		egui::Panel::top("menu").show(ui, |ui| self.panels.menu(ui));
		egui::Panel::bottom("status").show(ui, |ui| self.panels.status(ui));
		DockArea::new(&mut self.dock)
			.style(Style::from_egui(ui.style()))
			.show_inside(ui, &mut self.panels);
	}
}

/// What every panel draws from, plus the half-typed forms.
struct Panels {
	engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	preview: egui::TextureId,
	program: egui::TextureId,
	/// Whether the Items panel shows the top layer rather than the scene in
	/// preview.
	top: bool,
	/// What the Filters panel edits and a copy takes.
	selection: Option<Selection>,
	gestures: Gestures,
	new_scene: String,
	new_item: Option<String>,
	add: AddSource,
	devices: Devices,
	replay: Replay,
	/// The outcome of the last action, shown in the status bar.
	message: Option<Result<String, String>>,
}

impl TabViewer for Panels {
	type Tab = Tab;

	fn id(&mut self, tab: &mut Tab) -> egui::Id {
		egui::Id::new(*tab)
	}

	fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
		tab.title().into()
	}

	fn is_closeable(&self, _tab: &Tab) -> bool {
		false
	}

	fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
		match tab {
			Tab::Preview => self.preview(ui),
			Tab::Program => self.program(ui),
			Tab::Scenes => self.scenes(ui),
			Tab::History => self.history(ui),
			Tab::Items => self.items(ui),
			Tab::Sources => self.sources(ui),
			Tab::Devices => self.devices(ui),
			Tab::Filters => self.filters(ui),
			Tab::Audio => self.audio(ui),
			Tab::Output => self.output(ui),
			Tab::Health => self.health(ui),
			Tab::Replay => self.replay(ui),
		}
	}
}

/// A source or an item: what the Filters panel edits.
#[derive(Clone, Debug, PartialEq)]
enum Selection {
	Source(String),
	Item(Layer, usize),
}

impl Selection {
	/// The filters of what is selected, or `None` if it is gone or has no picture.
	fn filters(&self, collection: &Collection) -> Option<Vec<Filter>> {
		match self {
			Self::Source(name) => {
				collection.sources.get(name).filter(|source| source.has_video())?;
				Some(collection.video.get(name).cloned().unwrap_or_default().filters)
			}
			Self::Item(layer, index) => Some(collection.items(layer)?.get(*index)?.filters.clone()),
		}
	}

	fn set_filters(&self, draft: &mut Draft, filters: Vec<Filter>) -> Result<(), moq_scene::Error> {
		match self {
			Self::Source(name) => draft.set_video(name, Some(Video { filters })),
			Self::Item(layer, index) => {
				let item = draft
					.collection()
					.items(layer)
					.and_then(|items| items.get(*index))
					.cloned();
				let item =
					item.ok_or_else(|| moq_scene::Error::Conflict(format!("there is no item {index} in {layer}")))?;
				draft.set_item(layer, *index, Item { filters, ..item })
			}
		}
	}

	fn describe(&self, collection: &Collection) -> String {
		match self {
			Self::Source(name) => format!("source `{name}`"),
			Self::Item(layer, index) => {
				let item = collection.items(layer).and_then(|items| items.get(*index));
				let source = item.map_or("nothing", |item| item.source.as_str());
				format!("`{source}` in {layer}")
			}
		}
	}
}

/// Numbers the drags and typing sessions, so the edits one makes over many
/// frames undo as one.
#[derive(Default)]
struct Gestures(u64);

impl Gestures {
	/// The gesture `response` is part of, if it lasts across frames.
	fn of(&mut self, response: &egui::Response) -> Option<u64> {
		if response.drag_started() || response.gained_focus() {
			self.0 += 1;
		}
		(response.dragged() || response.has_focus()).then_some(self.0)
	}
}

/// Build an edit labelled `label` with `build` and commit it.
fn edit(
	state: &mut State,
	label: impl Into<String>,
	gesture: Option<u64>,
	build: impl FnOnce(&mut Draft) -> Result<(), moq_scene::Error>,
) -> anyhow::Result<()> {
	let mut draft = state.collection().draft(label);
	build(&mut draft)?;
	state.commit(draft, gesture)
}

fn dropped_message(dropped: usize) -> String {
	format!("{dropped} history entries no longer matched the scene file and were dropped")
}

impl Panels {
	/// The keyboard shortcuts, and the clipboard.
	fn shortcuts(&mut self, ctx: &egui::Context) {
		// A focused text field keeps its own undo, copy, and paste; any other
		// focused widget, such as a slider, leaves them to the studio.
		if ctx.text_edit_focused() {
			return;
		}
		// Shift first, since the plain shortcut also matches with it held.
		let redo = ctx.input_mut(|input| {
			input.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z)
				|| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
		});
		if redo {
			self.redo();
		} else if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
			self.undo();
		}
		let (copy, paste) = ctx.input(|input| {
			let copy = input.events.iter().any(|event| matches!(event, egui::Event::Copy));
			let paste = input.events.iter().find_map(|event| match event {
				egui::Event::Paste(text) => Some(text.clone()),
				_ => None,
			});
			(copy, paste)
		});
		if copy {
			self.copy(ctx);
		}
		if let Some(text) = paste {
			self.paste(&text);
		}
	}

	fn menu(&mut self, ui: &mut Ui) {
		egui::MenuBar::new().ui(ui, |ui| {
			ui.menu_button("File", |ui| {
				if ui.button("Reload").clicked() {
					self.deselect_item();
					let result = self.engine.reload();
					let result = result.map(|()| match self.engine.state.lock().unwrap().dropped {
						0 => "reloaded".to_string(),
						dropped => format!("reloaded; {}", dropped_message(dropped)),
					});
					self.report(result);
				}
			});
			ui.menu_button("Edit", |ui| {
				if ui.button("Undo").clicked() {
					self.undo();
				}
				if ui.button("Redo").clicked() {
					self.redo();
				}
				ui.separator();
				if ui.button("Copy item").clicked() {
					self.copy(ui.ctx());
				}
				if ui.button("Paste").clicked() {
					// Arrives as a paste event on a later frame.
					ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
				}
			});
		});
	}

	fn status(&mut self, ui: &mut Ui) {
		ui.horizontal(|ui| {
			let autosave = &self.engine.autosave;
			ui.label(autosave.path().display().to_string());
			if let Some(error) = autosave.error() {
				ui.separator();
				ui.colored_label(ui.visuals().error_fg_color, format!("not saved: {error}"));
			}
			match &self.message {
				Some(Ok(message)) => {
					ui.separator();
					ui.label(message);
				}
				Some(Err(message)) => {
					ui.separator();
					ui.colored_label(ui.visuals().error_fg_color, message);
				}
				None => {}
			}
		});
	}

	fn undo(&mut self) {
		self.deselect_item();
		let result = self.engine.state.lock().unwrap().undo();
		self.report(result.map(|label| match label {
			Some(label) => format!("undid: {label}"),
			None => "nothing to undo".into(),
		}));
	}

	fn redo(&mut self) {
		self.deselect_item();
		let result = self.engine.state.lock().unwrap().redo();
		self.report(result.map(|label| match label {
			Some(label) => format!("redid: {label}"),
			None => "nothing to redo".into(),
		}));
	}

	fn report(&mut self, result: anyhow::Result<String>) {
		self.message = Some(result.map_err(|err| format!("{err:#}")));
	}

	/// Report a failed edit; a successful one shows for itself.
	fn check(&mut self, result: anyhow::Result<()>) {
		if let Err(err) = result {
			self.report(Err(err));
		}
	}

	/// Keep a selected item of `layer` on the item it named, now at `moved`'s
	/// index, or deselect it if `moved` says it is gone.
	fn follow(&mut self, layer: &Layer, moved: impl FnOnce(usize) -> Option<usize>) {
		if let Some(Selection::Item(selected, index)) = &self.selection
			&& selected == layer
		{
			self.selection = moved(*index).map(|index| Selection::Item(layer.clone(), index));
		}
	}

	/// Deselect an item, after an edit that may have moved any of them.
	fn deselect_item(&mut self) {
		if matches!(self.selection, Some(Selection::Item(..))) {
			self.selection = None;
		}
	}

	/// The layer the Items panel shows, and new sources are placed in.
	fn layer(&self, state: &State) -> Result<Layer, String> {
		if self.top {
			return Ok(Layer::Top);
		}
		match state.compositor.preview_scene() {
			Some(name) => Ok(Layer::Scene(name.to_string())),
			None => Err("Pick a scene to edit what it shows.".into()),
		}
	}

	/// Copy the selected item, with what it shows, to the clipboard.
	fn copy(&mut self, ctx: &egui::Context) {
		let state = self.engine.state.lock().unwrap();
		let collection = state.collection();
		let item = match &self.selection {
			Some(Selection::Item(layer, index)) => collection.items(layer).and_then(|items| items.get(*index)),
			_ => None,
		};
		let Some(item) = item.cloned() else {
			drop(state);
			self.report(Err(anyhow::anyhow!("select an item to copy it")));
			return;
		};
		let dir = &state.sources.context().dir;
		let result = Clip::items(collection, vec![item.clone()], dir).to_toml();
		drop(state);
		let result = result.map(|text| {
			ctx.copy_text(text);
			format!("copied `{}`", item.source)
		});
		self.report(result.map_err(Into::into));
	}

	/// Paste items into the layer the Items panel shows, or filters onto the
	/// selection.
	fn paste(&mut self, text: &str) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let result = (|| {
			let clip = Clip::from_toml(text).context("the clipboard holds no items or filters")?;
			match clip {
				Clip::Items(items) => {
					let layer = self.layer(&state).map_err(anyhow::Error::msg)?;
					let dir = state.sources.context().dir.clone();
					edit(&mut state, format!("Paste into {layer}"), None, |draft| {
						draft.paste(&layer, &items, &dir)
					})?;
					Ok(format!("pasted into {layer}"))
				}
				Clip::Filters(pasted) => {
					let selection = self
						.selection
						.clone()
						.context("select a source or an item to paste filters onto")?;
					let mut filters = selection
						.filters(state.collection())
						.context("the selection has no picture to filter")?;
					filters.extend(pasted.filters);
					let what = selection.describe(state.collection());
					edit(&mut state, format!("Paste filters onto {what}"), None, |draft| {
						selection.set_filters(draft, filters)
					})?;
					Ok(format!("pasted filters onto {what}"))
				}
			}
		})();
		drop(state);
		self.report(result);
	}

	fn preview(&mut self, ui: &mut Ui) {
		let state = self.engine.state.lock().unwrap();
		let canvas = state.collection().canvas;
		ui.label(format!(
			"Preview: {}",
			state.compositor.preview_scene().unwrap_or("nothing")
		));
		drop(state);
		picture(ui, self.preview, canvas);
	}

	fn program(&mut self, ui: &mut Ui) {
		let mut state = self.engine.state.lock().unwrap();
		let canvas = state.collection().canvas;
		let fade = state.collection().transition.fade;
		ui.horizontal(|ui| {
			if ui.button("Cut").clicked() {
				state.compositor.cut();
			}
			if ui.button(format!("Fade ({} ms)", fade.as_millis())).clicked() {
				state.compositor.fade(fade, kio::clock::Instant::now());
			}
			ui.label(format!(
				"Program: {}",
				state.compositor.program_scene().unwrap_or("nothing")
			));
			if state.compositor.fading() {
				ui.weak("fading");
			}
		});
		drop(state);
		picture(ui, self.program, canvas);
	}

	fn scenes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let preview = state.compositor.preview_scene().map(str::to_owned);
		let program = state.compositor.program_scene().map(str::to_owned);
		let mut pick = None;
		let mut remove = None;

		egui::ScrollArea::vertical().show(ui, |ui| {
			for (index, scene) in state.collection().scenes.iter().enumerate() {
				ui.horizontal(|ui| {
					let selected = preview.as_deref() == Some(scene.name.as_str());
					if ui.selectable_label(selected, &scene.name).clicked() {
						pick = Some(scene.name.clone());
					}
					if program.as_deref() == Some(scene.name.as_str()) {
						ui.colored_label(Color32::RED, "on program");
					}
					if ui.small_button("Remove").clicked() {
						remove = Some((index, scene.name.clone()));
					}
				});
			}
		});

		ui.separator();
		ui.horizontal(|ui| {
			ui.text_edit_singleline(&mut self.new_scene);
			if ui.button("Add scene").clicked() {
				let name = self.new_scene.trim().to_string();
				let index = state.collection().scenes.len();
				let result = edit(&mut state, format!("Add scene `{name}`"), None, |draft| {
					draft.insert_scene(index, Scene::new(name.clone()))
				});
				match result {
					Ok(()) => {
						pick = Some(name);
						self.new_scene.clear();
					}
					Err(err) => self.report(Err(err)),
				}
			}
		});

		if let Some(pick) = pick {
			state.compositor.set_preview(Some(pick));
		}
		if let Some((index, name)) = remove {
			let result = edit(&mut state, format!("Remove scene `{name}`"), None, |draft| {
				draft.remove_scene(index)
			});
			drop(state);
			self.check(result);
		}
	}

	/// Every edit, oldest first; pick one to undo or redo to just after it.
	fn history(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let position = state.history().position();
		let count = state.history().entries().len();
		let mut seek = None;
		ui.horizontal(|ui| {
			if ui.add_enabled(position > 0, egui::Button::new("Undo")).clicked() {
				seek = Some(position - 1);
			}
			if ui.add_enabled(position < count, egui::Button::new("Redo")).clicked() {
				seek = Some(position + 1);
			}
		});
		ui.separator();
		egui::ScrollArea::vertical().show(ui, |ui| {
			if ui.selectable_label(position == 0, "Start").clicked() {
				seek = Some(0);
			}
			for (index, edit) in state.history().entries().enumerate() {
				// Undone edits are dimmed, as redo would bring them back.
				let mut label = egui::RichText::new(&edit.label);
				if index >= position {
					label = label.weak();
				}
				if ui.selectable_label(position == index + 1, label).clicked() {
					seek = Some(index + 1);
				}
			}
		});
		if let Some(seek) = seek {
			let result = state.seek(seek);
			drop(state);
			self.deselect_item();
			self.check(result);
		}
	}

	/// The items of the scene in preview or the top layer, back to front.
	fn items(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let scene = state.compositor.preview_scene().unwrap_or("no scene").to_string();
		ui.horizontal(|ui| {
			ui.selectable_value(&mut self.top, false, format!("Scene: {scene}"));
			ui.selectable_value(&mut self.top, true, "Top layer")
				.on_hover_text("Drawn over every scene, in preview and on program");
		});
		ui.separator();
		let layer = match self.layer(&state) {
			Ok(layer) => layer,
			Err(message) => {
				ui.label(message);
				return;
			}
		};
		let collection = state.collection();
		let Some(items) = collection.items(&layer).cloned() else {
			ui.label(format!("There is no {layer}."));
			return;
		};
		let canvas = collection.canvas;
		let mut action = None;

		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("items").striped(true).show(ui, |ui| {
				for heading in ["Source", "Shown", "x", "y", "Width", "Height", ""] {
					ui.strong(heading);
				}
				ui.end_row();

				let count = items.len();
				for (index, item) in items.iter().enumerate() {
					let selection = Selection::Item(layer.clone(), index);
					ui.horizontal(|ui| {
						let selected = self.selection.as_ref() == Some(&selection);
						if ui.selectable_label(selected, &item.source).clicked() {
							self.selection = Some(selection);
						}
						linked(ui, collection, &layer, index, item);
					});

					let mut visible = item.visible;
					if ui.checkbox(&mut visible, "").changed() {
						let verb = if visible { "Show" } else { "Hide" };
						let label = format!("{verb} `{}`", item.source);
						action = Some(ItemAction::Set(
							index,
							Item {
								visible,
								..item.clone()
							},
							label,
							None,
						));
					}

					let mut rect = item.rect;
					let responses = [
						("Move", ui.add(egui::DragValue::new(&mut rect.x))),
						("Move", ui.add(egui::DragValue::new(&mut rect.y))),
						(
							"Resize",
							ui.add(egui::DragValue::new(&mut rect.width).range(0.0..=f32::MAX)),
						),
						(
							"Resize",
							ui.add(egui::DragValue::new(&mut rect.height).range(0.0..=f32::MAX)),
						),
					];
					for (verb, response) in responses {
						let gesture = self.gestures.of(&response);
						if response.changed() {
							let label = format!("{verb} `{}`", item.source);
							action = Some(ItemAction::Set(index, Item { rect, ..item.clone() }, label, gesture));
						}
					}

					ui.horizontal(|ui| {
						if ui.small_button("Fill").clicked() {
							let rect = Rect::fill(&canvas);
							let label = format!("Fill with `{}`", item.source);
							action = Some(ItemAction::Set(index, Item { rect, ..item.clone() }, label, None));
						}
						if ui.add_enabled(index > 0, egui::Button::new("Back").small()).clicked() {
							action = Some(ItemAction::Move(index, index - 1));
						}
						if ui
							.add_enabled(index + 1 < count, egui::Button::new("Front").small())
							.clicked()
						{
							action = Some(ItemAction::Move(index, index + 1));
						}
						if ui.small_button("Remove").clicked() {
							action = Some(ItemAction::Remove(index));
						}
					});
					ui.end_row();
				}
			});

			ui.separator();
			ui.horizontal(|ui| {
				let pictures: Vec<&String> = collection
					.sources
					.iter()
					.filter(|(_, source)| source.has_video())
					.map(|(name, _)| name)
					.collect();
				// A pick whose source was removed or reloaded away would add a dangling item.
				if self.new_item.as_ref().is_some_and(|name| !pictures.contains(&name)) {
					self.new_item = None;
				}
				egui::ComboBox::from_id_salt("new item")
					.selected_text(self.new_item.as_deref().unwrap_or("Pick a source"))
					.show_ui(ui, |ui| {
						for name in pictures {
							ui.selectable_value(&mut self.new_item, Some(name.clone()), name);
						}
					});
				if ui.button("Add").clicked()
					&& let Some(source) = self.new_item.take()
				{
					action = Some(ItemAction::Add(Item::new(source, Rect::fill(&canvas))));
				}
			});
		});

		let Some(action) = action else { return };
		let result = match action {
			ItemAction::Set(index, item, label, gesture) => {
				edit(&mut state, label, gesture, |draft| draft.set_item(&layer, index, item))
			}
			ItemAction::Move(from, to) => {
				let label = format!("Reorder `{}`", items[from].source);
				let result = edit(&mut state, label, None, |draft| draft.move_item(&layer, from, to));
				if result.is_ok() {
					// Adjacent items swap, and the selection stays on its item.
					self.follow(&layer, |index| match index {
						i if i == from => Some(to),
						i if i == to => Some(from),
						i => Some(i),
					});
				}
				result
			}
			ItemAction::Remove(index) => {
				let label = format!("Remove `{}` from {layer}", items[index].source);
				let result = edit(&mut state, label, None, |draft| {
					draft.remove_item(&layer, index).map(drop)
				});
				if result.is_ok() {
					self.follow(&layer, |selected| match selected.cmp(&index) {
						std::cmp::Ordering::Less => Some(selected),
						std::cmp::Ordering::Equal => None,
						std::cmp::Ordering::Greater => Some(selected - 1),
					});
				}
				result
			}
			ItemAction::Add(item) => {
				let label = format!("Add `{}` to {layer}", item.source);
				edit(&mut state, label, None, |draft| {
					draft.insert_item(&layer, items.len(), item)
				})
			}
		};
		drop(state);
		self.check(result);
	}

	fn sources(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let mut remove = None;
		let mut clone = None;
		let now = kio::clock::now();

		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("sources").striped(true).show(ui, |ui| {
				let collection = state.collection();
				for (name, source) in &collection.sources {
					let selection = Selection::Source(name.clone());
					let selected = self.selection.as_ref() == Some(&selection);
					if ui.selectable_label(selected, name).clicked() {
						self.selection = Some(selection);
					}
					ui.label(describe(source));
					// A clone is as live as what it clones.
					let capture = collection.capture(name);
					let feed = state.sources.get(capture);
					match (
						state.compositor.error(capture),
						feed,
						feed.and_then(|feed| feed.stalled(now)),
					) {
						(Some(err), _, _) => status(ui, &Status::Failed(err.to_string())),
						(None, _, Some(since)) => {
							ui.colored_label(DEGRADED, "stalled")
								.on_hover_text(format!("no new picture for {:.0} s", since.as_secs_f64()));
						}
						(None, Some(feed), None) => status(ui, &feed.status()),
						(None, None, None) => status(ui, &Status::Starting),
					}
					// A track can fail while its source still draws, such as on a GPU
					// out of encode sessions, so its reason is spelled out. A clone
					// publishes none: it shows its source's capture, not a feed of its own.
					match state.sources.get(name).and_then(Feed::track) {
						Some(Status::Failed(reason)) => {
							ui.colored_label(ui.visuals().error_fg_color, format!("track failed: {reason}"));
						}
						Some(track) => {
							ui.horizontal(|ui| {
								ui.weak("track");
								status(ui, &track);
							});
						}
						None => {
							ui.label("");
						}
					}
					let places = collection
						.layers()
						.filter(|(_, items)| items.iter().any(|item| item.source == *name))
						.map(|(layer, _)| layer.to_string())
						.collect::<Vec<_>>();
					match places.len() {
						0 => ui.weak("unused"),
						1 => ui.weak("in 1 place"),
						n => ui.weak(format!("linked in {n} places")),
					}
					.on_hover_text(places.join("\n"));
					ui.horizontal(|ui| {
						if source.has_video()
							&& ui
								.small_button("Clone")
								.on_hover_text("A new source showing the same capture, with filters of its own")
								.clicked()
						{
							clone = Some(name.clone());
						}
						if ui.small_button("Remove").clicked() {
							remove = Some(name.clone());
						}
					});
					ui.end_row();
				}
			});

			ui.separator();
			if let Some(source) = self.add.ui(ui) {
				let name = self.add.name.trim().to_string();
				let layer = self.layer(&state).ok();
				let result = add(&mut state, layer, name, source, None);
				if result.is_ok() {
					self.add.name.clear();
				}
				self.report(result.map(|name| format!("added `{name}`")));
			}
		});

		let result = if let Some(name) = remove {
			// Removing the items that show it moves the ones after them.
			self.deselect_item();
			edit(&mut state, format!("Remove `{name}`"), None, |draft| {
				draft.remove_source(&name)
			})
		} else if let Some(name) = clone {
			// A clone of a clone shows the same capture, so it clones that, with
			// a copy of the clone's filters.
			let collection = state.collection();
			let source = Source::Clone {
				source: collection.capture(&name).to_string(),
			};
			let video = collection.video.get(&name).cloned();
			let clone = collection.unused_name(&format!("{name} clone"));
			let layer = self.layer(&state).ok();
			add(&mut state, layer, clone, source, video).map(drop)
		} else {
			Ok(())
		};
		drop(state);
		self.check(result);
	}

	fn devices(&mut self, ui: &mut Ui) {
		self.devices.poll();
		if ui.button("Refresh").clicked() {
			self.devices.refresh(&self.runtime);
		}
		let Some(picked) = self.devices.ui(ui) else { return };
		let mut state = self.engine.state.lock().unwrap();
		// Two cameras of the same model share a name; number the later ones.
		let name = state.collection().unused_name(&picked.name);
		let layer = self.layer(&state).ok();
		let result = add(&mut state, layer, name, picked.source, None);
		drop(state);
		self.report(result.map(|name| format!("added `{name}`")));
	}

	/// The filters of the selected source or item, in order.
	fn filters(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let collection = state.collection();
		let Some(selection) = self.selection.clone() else {
			ui.label("Select a source or an item to filter it.");
			return;
		};
		let what = selection.describe(collection);
		let Some(filters) = selection.filters(collection) else {
			ui.label(format!("{what} has no picture to filter."));
			return;
		};
		ui.strong(format!("Filters on {what}"));
		if let Selection::Item(layer, index) = &selection
			&& let Some(item) = collection.items(layer).and_then(|items| items.get(*index))
			&& let Some(video) = collection.video.get(&item.source)
		{
			let source = video.filters.iter().map(describe_filter).collect::<Vec<_>>();
			ui.weak(format!("after `{}`'s own: {}", item.source, source.join(", ")));
		}

		let mut next = filters.clone();
		let mut action: Option<(String, Option<u64>)> = None;
		let count = next.len();
		let mut moved = None;
		let mut remove = None;
		for (index, filter) in next.iter_mut().enumerate() {
			ui.horizontal(|ui| {
				match filter {
					Filter::Opacity { amount } => {
						ui.label("Opacity");
						let response = ui.add(egui::Slider::new(amount, 0.0..=1.0));
						let gesture = self.gestures.of(&response);
						if response.changed() {
							action = Some((format!("Set opacity on {what}"), gesture));
						}
					}
				}
				if ui.add_enabled(index > 0, egui::Button::new("Up").small()).clicked() {
					moved = Some((index, index - 1));
				}
				if ui
					.add_enabled(index + 1 < count, egui::Button::new("Down").small())
					.clicked()
				{
					moved = Some((index, index + 1));
				}
				if ui.small_button("Remove").clicked() {
					remove = Some(index);
				}
			});
		}
		if let Some((from, to)) = moved {
			next.swap(from, to);
			action = Some((format!("Reorder filters on {what}"), None));
		}
		if let Some(index) = remove {
			next.remove(index);
			action = Some((format!("Remove a filter from {what}"), None));
		}

		ui.separator();
		ui.horizontal(|ui| {
			if ui.button("Add opacity").clicked() {
				next.push(Filter::Opacity { amount: 1.0 });
				action = Some((format!("Add opacity to {what}"), None));
			}
			if ui.add_enabled(!filters.is_empty(), egui::Button::new("Copy")).clicked() {
				let clip = Clip::Filters(moq_scene::clip::Filters {
					filters: filters.clone(),
				});
				match clip.to_toml() {
					Ok(text) => {
						ui.ctx().copy_text(text);
						self.message = Some(Ok(format!("copied the filters on {what}")));
					}
					Err(err) => self.message = Some(Err(err.to_string())),
				}
			}
			if ui.button("Paste").clicked() {
				// Arrives as a paste event on a later frame, adding the filters.
				ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
			}
		});

		let Some((label, gesture)) = action else { return };
		let result = edit(&mut state, label, gesture, |draft| selection.set_filters(draft, next));
		drop(state);
		self.check(result);
	}

	fn audio(&mut self, ui: &mut Ui) {
		ui.horizontal(|ui| {
			ui.strong("Program");
			meter(ui, self.engine.bus.level());
		});
		ui.separator();

		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let mut change = None;
		egui::Grid::new("audio").striped(true).show(ui, |ui| {
			for (name, feed) in state.sources.iter() {
				let Some(strip) = feed.strip() else { continue };
				let mut audio = state.collection().audio.get(name).copied().unwrap_or_default();
				let mut db = decibels(audio.gain);
				ui.label(name);
				if ui.checkbox(&mut audio.muted, "Mute").changed() {
					let verb = if audio.muted { "Mute" } else { "Unmute" };
					change = Some((name.to_string(), audio, format!("{verb} `{name}`"), None));
				}
				let response = ui.add(egui::Slider::new(&mut db, MUTE_DB..=12.0).suffix(" dB"));
				let gesture = self.gestures.of(&response);
				if response.changed() {
					audio.gain = if db <= MUTE_DB { 0.0 } else { 10f32.powf(db / 20.0) };
					change = Some((name.to_string(), audio, format!("Set `{name}` gain"), gesture));
				}
				meter(ui, strip.level());
				ui.end_row();
			}
		});
		if let Some((name, audio, label, gesture)) = change {
			let result = edit(&mut state, label, gesture, |draft| draft.set_audio(&name, Some(audio)));
			drop(state);
			self.check(result);
		}
	}

	fn output(&mut self, ui: &mut Ui) {
		let stats = &self.engine.stats;
		let count = |value: &std::sync::atomic::AtomicU64| value.load(Ordering::Relaxed);
		let millis = |value: &std::sync::atomic::AtomicU64| format!("{:.1} ms", count(value) as f64 / 1000.0);

		egui::Grid::new("output").show(ui, |ui| {
			match &self.engine.output {
				Some(output) => {
					ui.label("Broadcast");
					ui.label(&output.path);
					ui.end_row();
					ui.label("Encoding");
					ui.label(match (output.on_demand, stats.watched.load(Ordering::Relaxed)) {
						(false, _) => "always",
						(true, true) => "while watched; watched now",
						(true, false) => "while watched; idle now",
					});
					ui.end_row();
				}
				None => {
					ui.label("Relay");
					ui.label("none; start with --connect to publish");
					ui.end_row();
				}
			}
			ui.label("Drawn");
			ui.label(format!("{} frames", count(&stats.rendered)));
			ui.end_row();
			ui.label("Handed to the encoder");
			ui.label(format!(
				"{} frames, last {}",
				count(&stats.copied),
				millis(&stats.copy_us)
			));
			ui.end_row();
			ui.label("Read back to the CPU");
			ui.label(if stats.zero_copy.load(Ordering::Relaxed) {
				"none; the encoder reads the GPU".to_string()
			} else {
				format!("{} frames", count(&stats.read_back))
			});
			ui.end_row();
			ui.label("Dropped");
			ui.label(format!("{} frames", count(&stats.dropped)));
			ui.end_row();
			ui.label("Encoder");
			ui.label(
				stats
					.encoder
					.lock()
					.unwrap()
					.clone()
					.unwrap_or_else(|| "not open".into()),
			);
			ui.end_row();
			ui.label("Encoded");
			ui.label(format!(
				"{} frames, last {}",
				count(&stats.encoded),
				millis(&stats.encode_us)
			));
			ui.end_row();
		});
		if let Some(error) = stats.error.lock().unwrap().as_ref() {
			ui.colored_label(ui.visuals().error_fg_color, error);
		}
	}

	/// Replay the recent past of the program or a source in a replay source,
	/// or save it as a clip.
	fn replay(&mut self, ui: &mut Ui) {
		let (pictures, replays) = {
			let state = self.engine.state.lock().unwrap();
			let named = |kind: fn(&Source) -> bool| -> Vec<String> {
				let sources = &state.collection().sources;
				sources
					.iter()
					.filter(|(_, source)| kind(source))
					.map(|(name, _)| name.clone())
					.collect()
			};
			(
				named(|source| source.has_video() && !matches!(source, Source::Image { .. } | Source::Color { .. })),
				named(|source| matches!(source, Source::Replay)),
			)
		};
		let panel = &mut self.replay;
		if panel.into.as_ref().is_none_or(|into| !replays.contains(into)) {
			panel.into = replays.first().cloned();
		}

		egui::Grid::new("replay").show(ui, |ui| {
			ui.label("Replay");
			egui::ComboBox::from_id_salt("replay of")
				.selected_text(panel.of.to_string())
				.show_ui(ui, |ui| {
					ui.selectable_value(&mut panel.of, Of::Program, Of::Program.to_string());
					for name in pictures {
						let of = Of::Source(name);
						let label = of.to_string();
						ui.selectable_value(&mut panel.of, of, label);
					}
				});
			ui.end_row();
			ui.label("Length");
			egui::ComboBox::from_id_salt("replay length")
				.selected_text(format!("{} s", panel.length.as_secs()))
				.show_ui(ui, |ui| {
					for length in replay::LENGTHS {
						ui.selectable_value(&mut panel.length, length, format!("{} s", length.as_secs()));
					}
				});
			ui.end_row();
			ui.label("Speed");
			egui::ComboBox::from_id_salt("replay speed")
				.selected_text(format!("{:.0}%", panel.speed * 100.0))
				.show_ui(ui, |ui| {
					for speed in replay::SPEEDS {
						ui.selectable_value(&mut panel.speed, speed, format!("{:.0}%", speed * 100.0));
					}
				});
			ui.end_row();
			ui.label("Play in");
			match &mut panel.into {
				Some(into) => {
					egui::ComboBox::from_id_salt("replay into")
						.selected_text(into.as_str())
						.show_ui(ui, |ui| {
							for name in replays {
								let label = name.clone();
								ui.selectable_value(into, name, label);
							}
						});
				}
				None => {
					ui.weak("no replay source; add one in Sources");
				}
			}
			ui.end_row();
		});

		let (play, save) = ui
			.horizontal(|ui| {
				let play = ui
					.add_enabled(panel.into.is_some(), egui::Button::new("Replay"))
					.on_hover_text("Play the last moments in the replay source")
					.clicked();
				let save = ui
					.button("Save clip")
					.on_hover_text("Save the last moments, in real time, beside the scene file")
					.clicked();
				(play, save)
			})
			.inner;
		if let Some(outcome) = panel.outcome.lock().unwrap().as_ref() {
			match outcome {
				Ok(message) => ui.label(message),
				Err(err) => ui.colored_label(ui.visuals().error_fg_color, err),
			};
		}
		if !play && !save {
			return;
		}

		// The span ends at the click, however long copying it takes to start.
		let outcome = panel.outcome.clone();
		let target = match self.engine.target(&panel.of) {
			Ok(target) => target,
			Err(err) => {
				*outcome.lock().unwrap() = Some(Err(format!("{err:#}")));
				return;
			}
		};
		let engine = self.engine.clone();
		let (of, length, speed, into) = (panel.of.clone(), panel.length, panel.speed, panel.into.clone());
		self.runtime.spawn(async move {
			let result = match into.filter(|_| play) {
				Some(into) => replay::play(engine, target, length, speed, into.clone())
					.await
					.map(|()| format!("replaying {of} in `{into}`")),
				None => replay::save(target, length, engine.autosave.path())
					.await
					.map(|path| format!("saved {of} to {}", path.display())),
			};
			*outcome.lock().unwrap() = Some(result.map_err(|err| format!("{err:#}")));
		});
	}

	/// How the stream reaches viewers: what is wrong and why, the uplink, each
	/// track, and each viewer that reports.
	fn health(&mut self, ui: &mut Ui) {
		let report = self.engine.health.lock().unwrap();
		let Some(relay) = &report.relay else {
			ui.label("Not publishing; start with --connect to see how the stream is delivered.");
			for problem in &report.problems {
				ui.colored_label(color(ui, problem.verdict), &problem.cause);
			}
			return;
		};

		egui::ScrollArea::vertical().show(ui, |ui| {
			if report.problems.is_empty() {
				ui.colored_label(HEALTHY, "No problems.");
			}
			for problem in &report.problems {
				ui.colored_label(color(ui, problem.verdict), &problem.cause);
			}
			ui.separator();

			let uplink = &report.uplink;
			let viewers = &report.viewers;
			egui::Grid::new("uplink").show(ui, |ui| {
				ui.label("Relay");
				match relay {
					health::Relay::Connecting => ui.weak("connecting"),
					health::Relay::Connected => ui.colored_label(HEALTHY, "connected"),
					health::Relay::Disconnected(reason) => ui
						.colored_label(ui.visuals().error_fg_color, "unreachable")
						.on_hover_text(reason),
				};
				ui.end_row();
				ui.label("Uplink");
				verdict(ui, uplink.verdict);
				ui.end_row();
				ui.label("Round trip");
				ui.label(or_unknown(uplink.rtt.map(|rtt| format!("{} ms", rtt.as_millis()))));
				ui.end_row();
				ui.label("Estimated capacity");
				ui.label(or_unknown(uplink.capacity.map(health::rate)));
				ui.end_row();
				ui.label("Media written");
				ui.label(or_unknown(uplink.media.map(health::rate)));
				ui.end_row();
				ui.label("Lost and resent");
				ui.label(or_unknown(uplink.loss.map(|loss| format!("{:.2}%", loss * 100.0))));
				ui.end_row();
				ui.label("Watching");
				match viewers.watching {
					Some(watching) => ui.label(format!("{watching} sessions, by the relay's stats")),
					None => ui.weak("unknown; the relay shares no stats with this connection"),
				};
				ui.end_row();
				ui.label("Reporting");
				ui.label(match viewers.unread {
					0 => format!("{} viewers", viewers.reports.len()),
					unread => format!("{} viewers read, {unread} more not", viewers.reports.len()),
				});
				ui.end_row();
			});
			ui.separator();

			egui::Grid::new("tracks").striped(true).show(ui, |ui| {
				for heading in [
					"Track",
					"Sent",
					"Frames",
					"Viewers",
					"Lowest received",
					"Latency",
					"Grade",
				] {
					ui.strong(heading);
				}
				ui.end_row();
				for track in &report.tracks {
					ui.label(&track.label).on_hover_text(&track.id);
					ui.label(or_unknown(track.sent.map(health::rate)));
					ui.label(or_unknown(track.framerate.map(|framerate| format!("{framerate:.0}/s"))));
					ui.label(track.viewers.to_string());
					ui.label(or_unknown(track.received.map(health::rate)));
					ui.label(or_unknown(track.latency.map(millis)));
					verdict(ui, track.verdict);
					ui.end_row();
				}
			});

			if viewers.reports.is_empty() {
				return;
			}
			ui.separator();
			egui::Grid::new("viewers").striped(true).show(ui, |ui| {
				for heading in ["Viewer", "Grade", "Round trip", "Latency", "Playout"] {
					ui.strong(heading);
				}
				ui.end_row();
				for viewer in &viewers.reports {
					ui.label(&viewer.name);
					match &viewer.error {
						Some(error) => {
							ui.colored_label(ui.visuals().error_fg_color, "unreadable")
								.on_hover_text(error);
						}
						None => verdict(ui, viewer.verdict),
					}
					ui.label(or_unknown(viewer.rtt.map(millis)));
					ui.label(or_unknown(viewer.latency.map(millis)));
					ui.label(or_unknown(viewer.playout.map(millis)));
					ui.end_row();
				}
			});
		});
	}
}

const HEALTHY: Color32 = Color32::from_rgb(80, 200, 120);
const DEGRADED: Color32 = Color32::from_rgb(220, 180, 60);

fn color(ui: &Ui, verdict: Verdict) -> Color32 {
	match verdict {
		Verdict::Healthy => HEALTHY,
		Verdict::Degraded => DEGRADED,
		Verdict::Unhealthy => ui.visuals().error_fg_color,
		Verdict::Unknown => ui.visuals().weak_text_color(),
	}
}

fn verdict(ui: &mut Ui, verdict: Verdict) {
	let text = match verdict {
		Verdict::Unknown => "unknown",
		Verdict::Healthy => "healthy",
		Verdict::Degraded => "degraded",
		Verdict::Unhealthy => "unhealthy",
	};
	ui.colored_label(color(ui, verdict), text);
}

fn or_unknown(value: Option<String>) -> String {
	value.unwrap_or_else(|| "-".into())
}

fn millis(duration: Duration) -> String {
	format!("{} ms", duration.as_millis())
}

/// An edit the Items panel makes.
enum ItemAction {
	/// Replace the item at an index, labelled, as part of a gesture.
	Set(usize, Item, String, Option<u64>),
	Move(usize, usize),
	Remove(usize),
	Add(Item),
}

/// Say where else the item at `index` of `layer` shows its source, and what
/// a clone clones.
fn linked(ui: &mut Ui, collection: &Collection, layer: &Layer, index: usize, item: &Item) {
	if let Some(Source::Clone { source }) = collection.sources.get(&item.source) {
		ui.weak(format!("clone of `{source}`"));
	}
	let elsewhere: Vec<String> = collection
		.layers()
		.flat_map(|(other, items)| {
			let at = items.iter().enumerate();
			at.filter(|(i, shown)| shown.source == item.source && !(other == *layer && *i == index))
				.map(|_| other.to_string())
				.collect::<Vec<_>>()
		})
		.collect();
	if !elsewhere.is_empty() {
		ui.weak("linked")
			.on_hover_text(format!("Also shown in:\n{}", elsewhere.join("\n")));
	}
}

fn describe_filter(filter: &Filter) -> String {
	match filter {
		Filter::Opacity { amount } => format!("opacity {:.0}%", amount * 100.0),
	}
}

/// Add `source` as `name`, with `video` filters, and place it in front of
/// `layer` if it has a picture. Returns the name.
fn add(
	state: &mut State,
	layer: Option<Layer>,
	name: String,
	source: Source,
	video: Option<Video>,
) -> anyhow::Result<String> {
	anyhow::ensure!(!name.is_empty(), "a source needs a name");
	anyhow::ensure!(
		!state.collection().sources.contains_key(&name),
		"there is already a source named `{name}`"
	);
	let canvas = state.collection().canvas;
	let place = source.has_video();
	edit(state, format!("Add `{name}`"), None, |draft| {
		draft.set_source(&name, Some(source))?;
		draft.set_video(&name, video)?;
		if place && let Some(layer) = layer {
			let index = draft.collection().items(&layer).map_or(0, Vec::len);
			draft.insert_item(&layer, index, Item::new(name.clone(), Rect::fill(&canvas)))?;
		}
		Ok(())
	})?;
	Ok(name)
}

/// Draw `texture` as large as fits, keeping the canvas's shape.
fn picture(ui: &mut Ui, texture: egui::TextureId, canvas: moq_scene::Canvas) {
	let available = ui.available_size();
	let aspect = canvas.width as f32 / canvas.height as f32;
	let mut size = egui::vec2(available.x, available.x / aspect);
	if size.y > available.y {
		size = egui::vec2(available.y * aspect, available.y);
	}
	ui.vertical_centered(|ui| ui.image((texture, size)));
}

fn describe(source: &Source) -> String {
	let or_default = |id: &Option<String>| id.clone().unwrap_or_else(|| "default".into());
	match source {
		Source::Camera { id } => format!("camera: {}", or_default(id)),
		Source::Display { id } => format!("display: {}", or_default(id)),
		Source::Window { id } => format!("window: {id}"),
		Source::Microphone { id } => format!("microphone: {}", or_default(id)),
		Source::SystemAudio => "system audio".into(),
		Source::Image { path } => format!("image: {}", path.display()),
		Source::Color { color } => format!("color: {color}"),
		Source::Broadcast { path } => format!("broadcast: {path}"),
		Source::Clone { source } => format!("clone of {source}"),
		Source::Replay => "replay".into(),
	}
}

fn status(ui: &mut Ui, status: &Status) {
	match status {
		Status::Starting => ui.weak("starting"),
		Status::Live => ui.colored_label(HEALTHY, "live"),
		Status::Offline => ui.colored_label(DEGRADED, "offline"),
		Status::Failed(reason) => ui
			.colored_label(ui.visuals().error_fg_color, "failed")
			.on_hover_text(reason),
		_ => ui.weak("unknown"),
	};
}

/// The quietest gain the slider offers, which it treats as silence.
const MUTE_DB: f32 = -60.0;

fn decibels(gain: f32) -> f32 {
	if gain <= 0.0 {
		MUTE_DB
	} else {
		(20.0 * gain.log10()).max(MUTE_DB)
	}
}

/// A level meter: RMS filled, peak as a tick, on a decibel scale.
fn meter(ui: &mut Ui, level: moq_audio::Level) {
	let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 12.0), egui::Sense::hover());
	let position = |amplitude: f32| (1.0 - decibels(amplitude) / MUTE_DB).clamp(0.0, 1.0);
	let painter = ui.painter_at(rect);
	painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
	let color = if level.peak() >= 1.0 {
		Color32::RED
	} else {
		Color32::from_rgb(80, 200, 120)
	};
	let mut filled = rect;
	filled.set_width(rect.width() * position(level.rms()));
	painter.rect_filled(filled, 2.0, color);
	let x = rect.left() + rect.width() * position(level.peak());
	painter.vline(x, rect.y_range(), egui::Stroke::new(2.0, color));
}

/// The form that adds a source by hand.
struct AddSource {
	name: String,
	kind: Kind,
	/// A path, broadcast path, or device id, depending on the kind.
	value: String,
	color: Color32,
}

impl Default for AddSource {
	fn default() -> Self {
		Self {
			name: String::new(),
			kind: Kind::default(),
			value: String::new(),
			color: Color32::WHITE,
		}
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
	#[default]
	Color,
	Image,
	Broadcast,
	Camera,
	Display,
	Window,
	Microphone,
	SystemAudio,
	Replay,
}

impl Kind {
	const ALL: [Self; 9] = [
		Self::Color,
		Self::Image,
		Self::Broadcast,
		Self::Camera,
		Self::Display,
		Self::Window,
		Self::Microphone,
		Self::SystemAudio,
		Self::Replay,
	];

	fn label(self) -> &'static str {
		match self {
			Self::Color => "Color",
			Self::Image => "Image",
			Self::Broadcast => "Broadcast",
			Self::Camera => "Camera",
			Self::Display => "Display",
			Self::Window => "Window",
			Self::Microphone => "Microphone",
			Self::SystemAudio => "System audio",
			Self::Replay => "Replay",
		}
	}

	/// What the value field holds, if this kind takes one.
	fn value(self) -> Option<&'static str> {
		match self {
			Self::Color | Self::SystemAudio | Self::Replay => None,
			Self::Image => Some("file"),
			Self::Broadcast => Some("path"),
			Self::Window => Some("window id"),
			Self::Camera | Self::Display | Self::Microphone => Some("id (empty for the default)"),
		}
	}
}

impl AddSource {
	/// Draw the form; the source to add once the user asks for it.
	fn ui(&mut self, ui: &mut Ui) -> Option<Source> {
		let mut add = false;
		egui::Grid::new("add source").show(ui, |ui| {
			ui.label("Name");
			ui.text_edit_singleline(&mut self.name);
			ui.end_row();
			ui.label("Kind");
			egui::ComboBox::from_id_salt("kind")
				.selected_text(self.kind.label())
				.show_ui(ui, |ui| {
					for kind in Kind::ALL {
						ui.selectable_value(&mut self.kind, kind, kind.label());
					}
				});
			ui.end_row();
			if let Some(label) = self.kind.value() {
				ui.label(label);
				ui.text_edit_singleline(&mut self.value);
				ui.end_row();
			}
			if self.kind == Kind::Color {
				ui.label("Color");
				egui::color_picker::color_edit_button_srgba(ui, &mut self.color, egui::color_picker::Alpha::OnlyBlend);
				ui.end_row();
			}
			add = ui.button("Add source").clicked();
			ui.end_row();
		});
		if !add {
			return None;
		}

		let value = self.value.trim().to_string();
		let id = (!value.is_empty()).then(|| value.clone());
		Some(match self.kind {
			Kind::Color => Source::Color {
				color: moq_scene::Rgba(self.color.to_srgba_unmultiplied()),
			},
			Kind::Image => Source::Image { path: value.into() },
			Kind::Broadcast => Source::Broadcast { path: value },
			Kind::Camera => Source::Camera { id },
			Kind::Display => Source::Display { id },
			Kind::Window => Source::Window { id: value },
			Kind::Microphone => Source::Microphone { id },
			Kind::SystemAudio => Source::SystemAudio,
			Kind::Replay => Source::Replay,
		})
	}
}

/// The Replay panel's choices, and how the last replay or clip went.
struct Replay {
	of: Of,
	length: Duration,
	speed: f32,
	/// The replay source a replay plays in.
	into: Option<String>,
	/// Set by the replay or save running in the background.
	outcome: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Default for Replay {
	fn default() -> Self {
		Self {
			of: Of::Program,
			length: replay::LENGTHS[1],
			speed: replay::SPEEDS[0],
			into: None,
			outcome: Arc::default(),
		}
	}
}

/// A device the user can add as a source.
struct Pick {
	name: String,
	source: Source,
}

/// The capture devices this machine has, listed in the background.
#[derive(Default)]
struct Devices {
	listing: Option<Listing>,
	pending: Option<std::sync::mpsc::Receiver<Listing>>,
}

/// One listing of every kind of device, each of which can fail on its own.
struct Listing {
	kinds: Vec<(&'static str, Result<Vec<Pick>, String>)>,
}

impl Devices {
	fn refresh(&mut self, runtime: &tokio::runtime::Handle) {
		let (send, recv) = std::sync::mpsc::channel();
		self.pending = Some(recv);
		// The capture backends' listing futures are `!Send` on macOS, as their
		// capture futures are (which is why each source runs on its own thread),
		// so poll the listing on a blocking thread rather than spawn it.
		let handle = runtime.clone();
		runtime.spawn_blocking(move || {
			let _ = send.send(handle.block_on(list()));
		});
	}

	fn poll(&mut self) {
		if let Some(listing) = self.pending.as_ref().and_then(|pending| pending.try_recv().ok()) {
			self.listing = Some(listing);
			self.pending = None;
		}
	}

	/// Draw the listing; the device to add once the user picks one.
	fn ui(&mut self, ui: &mut Ui) -> Option<Pick> {
		if self.pending.is_some() {
			ui.spinner();
		}
		let Some(listing) = &mut self.listing else {
			ui.label("Refresh to list this machine's cameras, screens, and inputs.");
			return None;
		};
		let mut picked = None;
		egui::ScrollArea::vertical()
			.show(ui, |ui| {
				for (kind, devices) in &mut listing.kinds {
					ui.strong(*kind);
					match devices {
						Ok(devices) if devices.is_empty() => {
							ui.weak("none");
						}
						Ok(devices) => {
							for (index, device) in devices.iter().enumerate() {
								ui.horizontal(|ui| {
									if ui.small_button("Add").clicked() {
										picked = Some(index);
									}
									ui.label(&device.name);
								});
							}
							if let Some(index) = picked.take() {
								return Some(devices.remove(index));
							}
						}
						Err(err) => {
							ui.colored_label(ui.visuals().error_fg_color, err.as_str());
						}
					}
				}
				None
			})
			.inner
	}
}

#[cfg(feature = "capture")]
async fn list() -> Listing {
	fn picks<T>(result: Result<Vec<T>, impl std::fmt::Display>, pick: impl Fn(T) -> Pick) -> Result<Vec<Pick>, String> {
		result
			.map(|devices| devices.into_iter().map(pick).collect())
			.map_err(|err| err.to_string())
	}

	let cameras = picks(moq_video::capture::cameras().await, |camera| Pick {
		name: camera.name,
		source: Source::Camera { id: Some(camera.id) },
	});
	let displays = picks(moq_video::capture::displays().await, |display| Pick {
		name: display.name,
		source: Source::Display { id: Some(display.id) },
	});
	let windows = picks(moq_video::capture::windows().await, |window| Pick {
		name: format!("{} ({})", window.title, window.app),
		source: Source::Window { id: window.id },
	});
	let mut microphones = picks(moq_audio::capture::devices().await, |device| Pick {
		name: format!("{} ({})", device.name, device.host),
		source: Source::Microphone { id: Some(device.id) },
	});
	if cfg!(target_os = "macos")
		&& let Ok(microphones) = &mut microphones
	{
		microphones.push(Pick {
			name: "System audio".into(),
			source: Source::SystemAudio,
		});
	}

	Listing {
		kinds: vec![
			("Cameras", cameras),
			("Displays", displays),
			("Windows", windows),
			("Audio inputs", microphones),
		],
	}
}

#[cfg(not(feature = "capture"))]
async fn list() -> Listing {
	Listing {
		kinds: vec![(
			"Devices",
			Err("this build cannot capture; rebuild with `--features capture`".into()),
		)],
	}
}
