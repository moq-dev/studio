//! The window: docked panels over the engine's state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use egui::{Color32, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use moq_scene::browser::Paint;
use moq_scene::source::{Feed, Status};
use moq_scene::{
	Blend, Canvas, Clip, Collection, Crop, Draft, Filter, Item, Layer, MaskMode, Rect, Scene, Source, Transition,
	Video, wgpu,
};

use crate::autosave::Conflict;
use crate::devices::{self, Listing, Pick};
use crate::engine::{Engine, State};
use crate::health::{self, Verdict};
use crate::interact::Interact;
use crate::record;
use crate::replay::{self, Of};
use crate::snap::{self, Grab, Snap};

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
	Guests,
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
			Self::Guests => "Guests",
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
	pub fn new(render: &eframe::egui_wgpu::RenderState, engine: Arc<Engine>, runtime: tokio::runtime::Handle) -> Self {
		// egui samples whatever the compositor's textures hold each frame, so
		// they are registered again only when a resize replaces them.
		let (preview, program, dropped) = {
			let state = engine.state.lock().unwrap();
			(
				Registered::new(render, state.compositor.preview()),
				Registered::new(render, state.compositor.program()),
				state.dropped,
			)
		};

		let mut dock = DockState::new(vec![Tab::Preview]);
		let tree = dock.main_surface_mut();
		let [top, bottom] = tree.split_below(NodeIndex::root(), 0.62, vec![Tab::Scenes, Tab::History]);
		tree.split_right(top, 0.5, vec![Tab::Program]);
		let [_, items] = tree.split_right(bottom, 0.18, vec![Tab::Items]);
		let [_, sources] = tree.split_right(items, 0.4, vec![Tab::Sources, Tab::Devices, Tab::Guests]);
		tree.split_right(
			sources,
			0.5,
			vec![Tab::Filters, Tab::Replay, Tab::Health, Tab::Audio, Tab::Output],
		);

		Self {
			dock,
			panels: Panels {
				engine,
				runtime,
				preview,
				program,
				top: false,
				selection: None,
				gestures: Gestures::default(),
				snapping: Snapping::default(),
				drag: None,
				new_scene: String::new(),
				renaming: None,
				canvas: None,
				new_item: None,
				add: AddSource::default(),
				devices: Devices::default(),
				record_sources: false,
				interacting: false,
				interact: Interact::default(),
				previewed: false,
				bound: BTreeMap::new(),
				replay: Replay::default(),
				guest: String::new(),
				shown: None,
				message: (dropped > 0).then(|| Err(dropped_message(dropped))),
				reloads: 0,
			},
		}
	}
}

impl eframe::App for Studio {
	fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
		let engine = self.panels.engine.clone();
		let state = engine.state.lock().unwrap();
		if let Some(render) = frame.wgpu_render_state() {
			self.panels.preview.follow(render, state.compositor.preview());
			self.panels.program.follow(render, state.compositor.program());
		}
		// Redraw at the canvas rate, so preview and program are live.
		let framerate = state.compositor.canvas().framerate;
		ui.ctx().request_repaint_after(Duration::from_secs(1) / framerate);
		self.panels.follow_reloads(&state);
		drop(state);

		self.panels.shortcuts(ui.ctx());
		egui::Panel::top("menu").show(ui, |ui| self.panels.menu(ui));
		if let Some(conflict) = self.panels.engine.autosave.conflict() {
			egui::Panel::top("conflict").show(ui, |ui| self.panels.conflict(ui, conflict));
		}
		egui::Panel::bottom("status").show(ui, |ui| self.panels.status(ui));
		self.panels.previewed = false;
		DockArea::new(&mut self.dock)
			.style(Style::from_egui(ui.style()))
			.show_inside(ui, &mut self.panels);
		// A hidden preview hears no input, so it lets go of the pages, as
		// turning Interact off does.
		if !self.panels.previewed {
			self.panels.let_go();
		}
	}
}

/// A compositor texture as egui shows it.
struct Registered {
	id: egui::TextureId,
	texture: wgpu::Texture,
}

impl Registered {
	fn new(render: &eframe::egui_wgpu::RenderState, texture: &wgpu::Texture) -> Self {
		let view = texture.create_view(&Default::default());
		let id = render
			.renderer
			.write()
			.register_native_texture(&render.device, &view, wgpu::FilterMode::Linear);
		Self {
			id,
			texture: texture.clone(),
		}
	}

	/// Show `texture` from now on, if a resize replaced the one shown.
	fn follow(&mut self, render: &eframe::egui_wgpu::RenderState, texture: &wgpu::Texture) {
		if self.texture == *texture {
			return;
		}
		let view = texture.create_view(&Default::default());
		render.renderer.write().update_egui_texture_from_wgpu_texture(
			&render.device,
			&view,
			wgpu::FilterMode::Linear,
			self.id,
		);
		self.texture = texture.clone();
	}
}

/// What every panel draws from, plus the half-typed forms.
struct Panels {
	engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	preview: Registered,
	program: Registered,
	/// Whether the Items panel shows the top layer rather than the scene in
	/// preview.
	top: bool,
	/// What the Filters panel edits and a copy takes.
	selection: Option<Selection>,
	gestures: Gestures,
	snapping: Snapping,
	/// The item a drag in the preview holds.
	drag: Option<Drag>,
	new_scene: String,
	/// The scene being renamed, by index, and the name typed so far.
	renaming: Option<(usize, String)>,
	/// The canvas the Output panel's form holds until it is applied, or
	/// `None` while it shows the collection's.
	canvas: Option<Canvas>,
	new_item: Option<String>,
	add: AddSource,
	devices: Devices,
	/// Whether the next recording gives each source a file of its own too.
	record_sources: bool,
	/// Whether the preview passes the mouse and keyboard to browser sources.
	interacting: bool,
	interact: Interact,
	/// Whether this frame drew the preview, which a stacked tab can hide.
	previewed: bool,
	/// The page `interact` holds state for under each name, so one stopped or
	/// restarted under it, as an edit or a reload does, is forgotten.
	bound: BTreeMap<String, moq_scene::browser::Page>,
	replay: Replay,
	/// The name typed for the next guest.
	guest: String,
	/// The join link on screen.
	shown: Option<Shown>,
	/// The outcome of the last action, shown in the status bar.
	message: Option<Result<String, String>>,
	/// The engine's reloads seen so far.
	reloads: u64,
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
			Tab::Guests => self.guests(ui),
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

/// What the Filters panel sets on a selection.
#[derive(Clone, Debug, PartialEq)]
struct Look {
	crop: Crop,
	filters: Vec<Filter>,
	/// An item's blend mode; a source has none.
	blend: Option<Blend>,
}

impl Selection {
	/// The look of what is selected, or `None` if it is gone or has no picture.
	fn look(&self, collection: &Collection) -> Option<Look> {
		match self {
			Self::Source(name) => {
				collection.sources.get(name).filter(|source| source.has_video())?;
				let video = collection.video.get(name).cloned().unwrap_or_default();
				Some(Look {
					crop: video.crop,
					filters: video.filters,
					blend: None,
				})
			}
			Self::Item(layer, index) => {
				let item = collection.items(layer)?.get(*index)?;
				Some(Look {
					crop: item.crop,
					filters: item.filters.clone(),
					blend: Some(item.blend),
				})
			}
		}
	}

	/// The filters of what is selected, or `None` if it is gone or has no picture.
	fn filters(&self, collection: &Collection) -> Option<Vec<Filter>> {
		self.look(collection).map(|look| look.filters)
	}

	fn set_look(&self, draft: &mut Draft, look: Look) -> Result<(), moq_scene::Error> {
		let Look { crop, filters, blend } = look;
		match self {
			Self::Source(name) => draft.set_video(name, Some(Video { crop, filters })),
			Self::Item(layer, index) => {
				let item = draft
					.collection()
					.items(layer)
					.and_then(|items| items.get(*index))
					.cloned();
				let item =
					item.ok_or_else(|| moq_scene::Error::Conflict(format!("there is no item {index} in {layer}")))?;
				let blend = blend.unwrap_or(item.blend);
				draft.set_item(
					layer,
					*index,
					Item {
						crop,
						filters,
						blend,
						..item
					},
				)
			}
		}
	}

	fn set_filters(&self, draft: &mut Draft, filters: Vec<Filter>) -> Result<(), moq_scene::Error> {
		let look = self
			.look(draft.collection())
			.ok_or_else(|| moq_scene::Error::Conflict("the selection is gone".into()))?;
		self.set_look(draft, Look { filters, ..look })
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
		// A focused text field, or a page the preview passes the keyboard to,
		// keeps its own undo, copy, and paste; any other focused widget, such
		// as a slider, leaves them to the studio.
		if ctx.text_edit_focused() || self.interact.has_keyboard() {
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
					let result = self.engine.reload();
					self.check(result);
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

	/// Report a reload, from File > Reload or an outside edit the autosave
	/// took, and let go of a selected item, a drag, and a scene rename under
	/// way, whose index it may have given to another. Called under every lock
	/// that uses one, since the autosave can reload between two.
	fn follow_reloads(&mut self, state: &State) {
		if state.reloads == self.reloads {
			return;
		}
		self.reloads = state.reloads;
		self.deselect_item();
		let path = self.engine.autosave.path().display();
		self.report(Ok(match state.dropped {
			0 => format!("reloaded {path}"),
			dropped => format!("reloaded {path}; {}", dropped_message(dropped)),
		}));
	}

	/// An outside edit to the scene file the studio has not taken, and the
	/// choice of which version to keep.
	fn conflict(&mut self, ui: &mut Ui, conflict: Conflict) {
		ui.horizontal_wrapped(|ui| {
			let path = self.engine.autosave.path().display().to_string();
			let why = match &conflict.refused {
				None => "while the studio has unsaved edits".to_string(),
				Some(refused) => format!("and cannot be loaded: {refused}"),
			};
			ui.colored_label(
				ui.visuals().warn_fg_color,
				format!("{path} changed outside the studio {why}. Nothing is saved until you pick a version."),
			);
			if ui.button("Keep the studio's").clicked() {
				let result = self.engine.keep();
				self.report(result.map(|()| format!("saved the studio's version over {path}")));
			}
			if ui.button("Take the file's").clicked() {
				let result = self.engine.reload();
				self.check(result);
			}
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

	/// Deselect an item, after an edit that may have moved any of them, and
	/// let go of any drag and any scene rename under way.
	fn deselect_item(&mut self) {
		if matches!(self.selection, Some(Selection::Item(..))) {
			self.selection = None;
		}
		self.drag = None;
		self.renaming = None;
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
		let engine = self.engine.clone();
		let state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
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
		self.follow_reloads(&state);
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

	/// Let go of every page the preview passed input to.
	fn let_go(&mut self) {
		if self.interact.pages().next().is_none() {
			return;
		}
		let engine = self.engine.clone();
		let state = engine.state.lock().unwrap();
		self.rebind(&state.sources);
		deliver(&state.sources, self.interact.release());
	}

	/// Forget the pages `interact` holds that stopped or restarted, and note
	/// the page under each name it newly holds.
	fn rebind(&mut self, sources: &moq_scene::source::Sources) {
		let held: BTreeSet<String> = self.interact.pages().map(str::to_string).collect();
		self.bound.retain(|name, _| held.contains(name));
		for name in held {
			let current = sources.get(&name).and_then(Feed::page);
			match (self.bound.get(&name), current) {
				(Some(bound), Some(current)) if bound.same(current) => {}
				(None, Some(current)) => {
					self.bound.insert(name, current.clone());
				}
				_ => {
					self.interact.forget(&name);
					self.bound.remove(&name);
				}
			}
		}
	}

	fn preview(&mut self, ui: &mut Ui) {
		self.previewed = true;
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let canvas = state.collection().canvas;
		ui.horizontal(|ui| {
			ui.label(format!(
				"Preview: {}",
				state.compositor.preview_scene().unwrap_or("nothing")
			));
			let toggled = ui
				.toggle_value(&mut self.interacting, "Interact")
				.on_hover_text(
					"Pass the mouse to the browser source under it, and the keyboard to the one last clicked",
				)
				.changed();
			if toggled && !self.interacting {
				self.rebind(&state.sources);
				deliver(&state.sources, self.interact.release());
			}
			ui.separator();
			let free = ui.ctx().format_modifiers(egui::Modifiers::COMMAND);
			ui.checkbox(&mut self.snapping.on, "Snap").on_hover_text(format!(
				"Snap a dragged item to the canvas, other items, and the grid. Hold {free} to drag freely."
			));
			ui.checkbox(&mut self.snapping.grid, "Grid");
			let largest = canvas.width.max(canvas.height);
			ui.add_enabled(
				self.snapping.grid,
				egui::DragValue::new(&mut self.snapping.spacing)
					.range(2..=largest)
					.suffix(" px"),
			);
		});
		// Interacting, the pointer belongs to the pages rather than to arranging.
		if self.interacting {
			let image = picture(ui, self.preview.id, canvas, egui::Sense::hover()).rect;
			let scene = state.compositor.preview_scene();
			self.rebind(&state.sources);
			let delivered = self.interact.update(ui, image, state.collection(), scene);
			deliver(&state.sources, delivered);
			self.rebind(&state.sources);
			return;
		}
		let response = picture(ui, self.preview.id, canvas, egui::Sense::click_and_drag());
		let result = self.arrange(ui, &mut state, &response, canvas);
		drop(state);
		self.check(result);
	}

	/// Select, move, and resize the items of the layer the Items panel shows
	/// by dragging them in the preview, snapping as they go.
	fn arrange(&mut self, ui: &Ui, state: &mut State, response: &egui::Response, canvas: Canvas) -> anyhow::Result<()> {
		let gesture = self.gestures.of(response);
		// The release still moves the item to where the pointer let go.
		if !response.dragged() && !response.drag_stopped() {
			self.drag = None;
		}
		// A panel squeezed to nothing has no picture to map the pointer onto.
		if !response.rect.is_positive() {
			return Ok(());
		}
		let view = View::new(response.rect, canvas);
		let painter = ui.painter_at(response.rect);
		if self.snapping.grid {
			view.grid(&painter, self.snapping.spacing as f32);
		}

		let Ok(layer) = self.layer(state) else { return Ok(()) };
		let Some(items) = state.collection().items(&layer).cloned() else {
			return Ok(());
		};
		let selected = match &self.selection {
			Some(Selection::Item(selected, index)) if *selected == layer => Some(*index),
			_ => None,
		};
		let reach = view.canvas(REACH);
		let hit = |pos: egui::Pos2| snap::hit(&items, selected, view.point(pos), reach);

		if response.clicked() {
			match response.interact_pointer_pos().and_then(hit) {
				Some((index, _)) => self.selection = Some(Selection::Item(layer.clone(), index)),
				None => self.deselect_item(),
			}
		}
		if response.drag_started()
			&& let Some(origin) = ui.input(|input| input.pointer.press_origin())
			&& let Some((index, grab)) = hit(origin)
		{
			self.selection = Some(Selection::Item(layer.clone(), index));
			self.drag = Some(Drag {
				layer: layer.clone(),
				index,
				grab,
				rect: items[index].rect,
				origin: view.point(origin),
				gesture,
				hints: None,
			});
		}

		let mut result = Ok(());
		if let Some(drag) = &mut self.drag
			&& let Some(pos) = response.interact_pointer_pos()
		{
			let point = view.point(pos);
			// Whole canvas pixels, so an unsnapped drag saves round numbers.
			let delta = [(point[0] - drag.origin[0]).round(), (point[1] - drag.origin[1]).round()];
			let rect = drag.grab.drag(drag.rect, delta);
			let free = ui.input(|input| input.modifiers.command);
			drag.hints = (self.snapping.on && !free).then(|| {
				let snap = Snap {
					reach,
					grid: self.snapping.grid.then_some(self.snapping.spacing as f32),
				};
				snap.apply(drag.grab, rect, &others(state, &drag.layer, drag.index), &canvas)
			});
			let rect = drag.hints.as_ref().map_or(rect, |snapped| snapped.rect);
			let item = state
				.collection()
				.items(&drag.layer)
				.and_then(|items| items.get(drag.index));
			if let Some(item) = item.cloned()
				&& item.rect != rect
			{
				let verb = if drag.grab == Grab::Move { "Move" } else { "Resize" };
				let label = format!("{verb} `{}`", item.source);
				result = edit(state, label, drag.gesture, |draft| {
					draft.set_item(&drag.layer, drag.index, Item { rect, ..item })
				});
			}
		}
		if !response.dragged() {
			self.drag = None;
		}

		// Drawn from the collection and selection as this frame left them.
		let items = state.collection().items(&layer).cloned().unwrap_or_default();
		let selected = match &self.selection {
			Some(Selection::Item(selected, index)) if *selected == layer => Some(*index),
			_ => None,
		};
		let accent = ui.visuals().selection.stroke.color;
		// A hidden item has no handles, as the preview cannot grab it.
		if let Some(item) = selected.and_then(|index| items.get(index)).filter(|item| item.visible) {
			view.selected(&painter, &item.rect, accent);
		}
		let grab = match &self.drag {
			Some(drag) => {
				if let Some(snapped) = &drag.hints {
					view.hints(&painter, snapped);
				}
				Some(drag.grab)
			}
			None => response.hover_pos().and_then(hit).map(|(index, grab)| {
				if selected != Some(index)
					&& let Some(item) = items.get(index)
				{
					view.hovered(&painter, &item.rect, accent);
				}
				grab
			}),
		};
		if let Some(grab) = grab {
			ui.ctx().set_cursor_icon(cursor(grab));
		}
		result
	}

	fn program(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let canvas = state.collection().canvas;
		let fade = state.collection().transition.fade;
		let mut result = Ok(());
		ui.horizontal(|ui| {
			if ui.button("Cut").clicked() {
				state.compositor.cut();
			}
			if ui.button("Fade").clicked() {
				state.compositor.fade(fade, kio::clock::Instant::now());
			}
			let mut millis = fade.as_millis() as u64;
			let response = ui
				.add(egui::DragValue::new(&mut millis).suffix(" ms"))
				.on_hover_text("How long a fade takes");
			let gesture = self.gestures.of(&response);
			if response.changed() {
				let fade = Duration::from_millis(millis);
				result = edit(&mut state, "Set the fade time", gesture, |draft| {
					draft.set_transition(Transition { fade })
				});
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
		self.check(result);
		picture(ui, self.program.id, canvas, egui::Sense::hover());
	}

	fn scenes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let preview = state.compositor.preview_scene().map(str::to_owned);
		let program = state.compositor.program_scene().map(str::to_owned);
		let on_air: Vec<String> = state.compositor.on_air().map(str::to_owned).collect();
		let mut pick = None;
		let mut action = None;
		let count = state.collection().scenes.len();

		egui::ScrollArea::vertical().show(ui, |ui| {
			for (index, scene) in state.collection().scenes.iter().enumerate() {
				ui.horizontal(|ui| {
					if let Some((renaming, name)) = &mut self.renaming
						&& *renaming == index
					{
						let response = ui.text_edit_singleline(name);
						if !response.has_focus() && !response.lost_focus() {
							response.request_focus();
						}
						if response.lost_focus() {
							// Escape cancels; Enter or a click away renames.
							if !ui.input(|input| input.key_pressed(egui::Key::Escape)) {
								action = Some(SceneAction::Rename(index, name.trim().to_string()));
							}
							self.renaming = None;
						}
						return;
					}
					let selected = preview.as_deref() == Some(scene.name.as_str());
					let label = ui.selectable_label(selected, &scene.name);
					if label.clicked() {
						pick = Some(scene.name.clone());
					}
					if label.double_clicked() || ui.small_button("Rename").clicked() {
						self.renaming = Some((index, scene.name.clone()));
					}
					if program.as_deref() == Some(scene.name.as_str()) {
						ui.colored_label(Color32::RED, "on program");
					}
					if ui.add_enabled(index > 0, egui::Button::new("Up").small()).clicked() {
						action = Some(SceneAction::Move(index, index - 1));
					}
					if ui
						.add_enabled(index + 1 < count, egui::Button::new("Down").small())
						.clicked()
					{
						action = Some(SceneAction::Move(index, index + 1));
					}
					if ui
						.add_enabled(!on_air.contains(&scene.name), egui::Button::new("Remove").small())
						.on_disabled_hover_text("On air; cut to another scene first")
						.clicked()
					{
						action = Some(SceneAction::Remove(index));
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
		let Some(action) = action else { return };
		let name = state.collection().scenes[action.index()].name.clone();
		let result = match action {
			SceneAction::Rename(index, to) => {
				let result = edit(&mut state, format!("Rename scene `{name}` to `{to}`"), None, |draft| {
					draft.rename_scene(index, to.clone())
				});
				if result.is_ok() {
					self.renamed(&name, &to);
				}
				result
			}
			SceneAction::Move(from, to) => edit(&mut state, format!("Reorder scene `{name}`"), None, |draft| {
				draft.move_scene(from, to)
			}),
			SceneAction::Remove(index) => edit(&mut state, format!("Remove scene `{name}`"), None, |draft| {
				draft.remove_scene(index)
			}),
		};
		drop(state);
		self.check(result);
	}

	/// Keep a selected item of the scene renamed from `from` selected.
	fn renamed(&mut self, from: &str, to: &str) {
		if let Some(Selection::Item(Layer::Scene(scene), _)) = &mut self.selection
			&& scene == from
		{
			*scene = to.to_string();
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
		self.follow_reloads(&state);
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
		let mut reload = None;
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
						(None, _, Some(stall)) => {
							ui.colored_label(DEGRADED, "stalled")
								.on_hover_text(health::without(stall));
						}
						(None, Some(feed), None) => status(ui, &feed.status()),
						(None, None, None) => status(ui, &Status::Starting),
					}
					// A track can fail while its source still draws, such as on a GPU
					// out of encode sessions, so its reason is spelled out. A clone
					// publishes none: it shows its source's capture, not a feed of its own.
					// Nor does a page, which shows how it paints instead.
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
						None => match feed.and_then(Feed::page).and_then(|page| page.paint()) {
							Some(Paint::Gpu) => {
								ui.weak("GPU").on_hover_text("The page is copied on the GPU");
							}
							Some(Paint::Cpu) => {
								ui.colored_label(DEGRADED, "CPU").on_hover_text(
									"The page is uploaded from memory every frame: its GPU process or this device cannot share textures",
								);
							}
							None => {
								ui.label("");
							}
						},
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
						if matches!(source, Source::Browser { .. })
							&& ui
								.small_button("Reload")
								.on_hover_text("Restart the page, keeping the cookies and logins every page shares")
								.clicked()
						{
							reload = Some(name.clone());
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

		if let Some(name) = reload {
			state.restart(&name);
		}
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
			let refreshed = self.devices.refresh(&self.runtime).context("cannot list devices");
			self.check(refreshed);
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

	/// Invite guests, show their links, and see who joined.
	fn guests(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let Some(room) = &engine.room else {
			ui.label("Start with --room and --room-key to invite guests.");
			return;
		};

		ui.horizontal(|ui| {
			ui.label("Name");
			let field = ui.text_edit_singleline(&mut self.guest);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			if ui.button("Invite").clicked() || entered {
				let name = self.guest.trim().to_string();
				match room.invite(&name) {
					Ok(invite) => {
						self.show(ui.ctx(), name, invite);
						self.guest.clear();
					}
					Err(err) => self.report(Err(err)),
				}
			}
		});

		let now = kio::clock::wall();
		if let Some(shown) = &self.shown {
			ui.separator();
			ui.horizontal(|ui| {
				ui.strong(format!("Join link for {}", shown.name));
				ui.label(expiry(shown.invite.expires, now));
			});
			let mut hide = false;
			ui.horizontal(|ui| {
				if ui.button("Copy link").clicked() {
					ui.ctx().copy_text(shown.invite.link.to_string());
				}
				hide = ui.button("Hide").clicked();
			});
			// The link is the guest's credential, so it shows only as a code to scan.
			match &shown.qr {
				Ok(qr) => {
					let side = ui.available_width().min(240.0);
					ui.image((qr.id(), egui::vec2(side, side)));
				}
				Err(err) => {
					ui.colored_label(ui.visuals().error_fg_color, format!("{err}; copy the link instead."));
				}
			}
			if hide {
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
							ui.colored_label(HEALTHY, "live");
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
								"Take their camera and screen, and any mask reading them, out of every scene and mute them. Their token stops renewing, so they leave within --guest-expiry. Undo puts them back in the scenes; only New link renews them again.",
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
				Err(err) => self.report(Err(err)),
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
	fn show(&mut self, ctx: &egui::Context, name: String, invite: crate::guests::Invite) {
		let qr = qr(ctx, invite.link.as_str());
		self.shown = Some(Shown { name, invite, qr });
	}

	/// The crop, filters in order, and blend mode of the selected source or item.
	fn filters(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let collection = state.collection();
		let Some(selection) = self.selection.clone() else {
			ui.label("Select a source or an item to filter it.");
			return;
		};
		let what = selection.describe(collection);
		let Some(look) = selection.look(collection) else {
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
		// What a mask can show, a picture of its own, and what a LUT reads.
		let pictures: Vec<String> = collection
			.sources
			.iter()
			.filter(|(_, source)| source.has_video() && source.opens())
			.map(|(name, _)| name.clone())
			.collect();
		let luts: Vec<String> = collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Lut { .. }))
			.map(|(name, _)| name.clone())
			.collect();

		let mut next = look.clone();
		let mut action: Option<(String, Option<u64>)> = None;
		egui::Grid::new("look").show(ui, |ui| {
			ui.label("Crop");
			ui.horizontal(|ui| {
				let crop = &mut next.crop;
				for (label, edge) in [
					("left", &mut crop.left),
					("top", &mut crop.top),
					("right", &mut crop.right),
					("bottom", &mut crop.bottom),
				] {
					ui.weak(label);
					let response = ui.add(egui::DragValue::new(edge).range(0.0..=0.95).speed(0.002));
					if let Some(gesture) = changed(&response, &mut self.gestures) {
						action = Some((format!("Crop {what}"), gesture));
					}
				}
			})
			.response
			.on_hover_text("The fraction of the picture trimmed from each edge, the rest stretched to fill");
			ui.end_row();
			if let Some(blend) = &mut next.blend {
				ui.label("Blend");
				let before = *blend;
				egui::ComboBox::from_id_salt("blend")
					.selected_text(blend_name(*blend))
					.show_ui(ui, |ui| {
						for mode in Blend::ALL {
							ui.selectable_value(blend, mode, blend_name(mode));
						}
					});
				if *blend != before {
					action = Some((format!("Set the blend mode of {what}"), None));
				}
				ui.end_row();
			}
		});

		let count = next.filters.len();
		let mut moved = None;
		let mut remove = None;
		for (index, filter) in next.filters.iter_mut().enumerate() {
			ui.push_id(index, |ui| {
				ui.horizontal_wrapped(|ui| {
					if let Some(gesture) = filter_ui(ui, filter, &pictures, &luts, &mut self.gestures) {
						action = Some((format!("Set {} on {what}", filter_name(filter)), gesture));
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
			});
		}
		if let Some((from, to)) = moved {
			next.filters.swap(from, to);
			action = Some((format!("Reorder filters on {what}"), None));
		}
		if let Some(index) = remove {
			next.filters.remove(index);
			action = Some((format!("Remove a filter from {what}"), None));
		}

		ui.separator();
		let filters = look.filters;
		ui.horizontal(|ui| {
			egui::ComboBox::from_id_salt("add filter")
				.selected_text("Add filter")
				.show_ui(ui, |ui| {
					for filter in templates(&pictures, &luts) {
						if ui.selectable_label(false, filter_name(&filter)).clicked() {
							action = Some((format!("Add {} to {what}", filter_name(&filter)), None));
							next.filters.push(filter);
						}
					}
				})
				.response
				.on_hover_text("A LUT needs a LUT source, and a mask a source with a picture of its own");
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
		let result = edit(&mut state, label, gesture, |draft| selection.set_look(draft, next));
		drop(state);
		self.check(result);
	}

	fn audio(&mut self, ui: &mut Ui) {
		ui.horizontal(|ui| {
			ui.strong("Program");
			meter(ui, self.engine.mix.level());
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

	/// Start and stop recording, and see what the recording wrote.
	fn record(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let recorder = &engine.recorder;
		let status = recorder.status();
		ui.horizontal(|ui| match status.as_ref().filter(|status| !status.done) {
			Some(status) => {
				if ui
					.add_enabled(!status.stopping, egui::Button::new("Stop recording"))
					.clicked()
				{
					recorder.stop();
				}
				if status.stopping {
					ui.label("finishing");
				} else {
					let elapsed = kio::clock::now().saturating_duration_since(status.started).as_secs();
					let elapsed = format!("{}:{:02}:{:02}", elapsed / 3600, elapsed / 60 % 60, elapsed % 60);
					ui.colored_label(ui.visuals().error_fg_color, format!("recording {elapsed}"));
				}
			}
			None => {
				if ui.button("Start recording").clicked() {
					let result = recorder.start(self.record_sources);
					self.check(result);
				}
				ui.checkbox(&mut self.record_sources, "each source too");
				ui.weak(format!("into {}", recorder.dir().display()));
			}
		});
		for file in status.iter().flat_map(|status| &status.files) {
			let name = file.path.file_name().unwrap_or_default().to_string_lossy();
			let size = format!("{:.1} MB", file.written as f64 / 1e6);
			match &file.finished {
				None => ui.label(format!("{name}: {size}")),
				Some(record::Finished::Saved) => ui.label(format!("{name}: saved, {size}")),
				Some(record::Finished::Empty) => ui.weak(format!("{name}: nothing recorded, removed")),
				Some(record::Finished::Failed(reason)) => {
					ui.colored_label(ui.visuals().error_fg_color, format!("{name}: {reason}"))
				}
			};
		}
	}

	fn output(&mut self, ui: &mut Ui) {
		self.record(ui);
		ui.separator();
		self.canvas_form(ui);
		ui.separator();

		let stats = &self.engine.stats;
		let count = |value: &std::sync::atomic::AtomicU64| value.load(Ordering::Relaxed);
		let millis = |value: &std::sync::atomic::AtomicU64| format!("{:.1} ms", count(value) as f64 / 1000.0);
		let watched = stats.watched.load(Ordering::Relaxed);

		egui::Grid::new("output").show(ui, |ui| {
			match &self.engine.output {
				Some(output) => {
					ui.label("Broadcast");
					ui.label(&output.path);
					ui.end_row();
					ui.label("Encoding");
					ui.label(match (output.on_demand, watched) {
						(false, _) => "the canvas size always, half size while watched",
						(true, true) => "while watched; watched now",
						(true, false) => "while watched; idle now",
					});
					ui.end_row();
				}
				None => {
					ui.label("Relay");
					ui.label("none; start with --connect to publish");
					ui.end_row();
					ui.label("Encoding");
					ui.label("only the recording's encoder, while recording");
					ui.end_row();
				}
			}
			ui.label("Drawn");
			ui.label(format!("{} frames", count(&stats.rendered)));
			ui.end_row();
			ui.label("Encoder");
			ui.label(stats.encoder().unwrap_or_else(|| "not open".into()));
			ui.end_row();
			ui.label("Encoder sessions");
			ui.label(format!("{} open", stats.sessions.open()))
				.on_hover_text("Video encoders open now, the program's and the sources'. A GPU caps them.");
			ui.end_row();
		});
		ui.separator();
		// Only the sizes the program publishes, since a small canvas, or an
		// encoder that refused the half size, has the canvas size alone, and
		// the recording's own encoder.
		let [canvas, half, recording] = &stats.renditions;
		let published = stats.sizes.load(Ordering::Relaxed);
		let mut columns = vec![("Canvas size", canvas)];
		if published > 1 {
			columns.push(("Half size", half));
		}
		columns.push(("Recording", recording));
		egui::Grid::new("renditions").show(ui, |ui| {
			ui.strong("");
			for (heading, _) in &columns {
				ui.strong(*heading);
			}
			ui.end_row();
			let row = |ui: &mut Ui, label: &str, cell: &dyn Fn(&crate::engine::Load) -> String| {
				ui.label(label);
				for (_, load) in &columns {
					ui.label(cell(load));
				}
				ui.end_row();
			};
			row(ui, "Handed to the encoder", &|load| {
				format!("{} frames, last {}", count(&load.copied), millis(&load.copy_us))
			});
			let zero_copy = stats.zero_copy.load(Ordering::Relaxed);
			row(ui, "Read back to the CPU", &|load| match zero_copy {
				true => "none; the encoder reads the GPU".to_string(),
				false => format!("{} frames", count(&load.read_back)),
			});
			row(ui, "Dropped", &|load| format!("{} frames", count(&load.dropped)));
			row(ui, "Encoded", &|load| {
				format!("{} frames, last {}", count(&load.encoded), millis(&load.encode_us))
			});
		});
		if let Some(error) = stats.error() {
			ui.colored_label(ui.visuals().error_fg_color, error);
		}
		if let Some(error) = half.error() {
			ui.colored_label(ui.visuals().warn_fg_color, format!("half size off: {error}"));
		}
		if let Some(error) = recording.error() {
			ui.colored_label(ui.visuals().warn_fg_color, format!("recording encoder off: {error}"));
		}
		if let Some(error) = stats.recording_audio_error() {
			ui.colored_label(ui.visuals().warn_fg_color, format!("recording audio off: {error}"));
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
			// What a source shows, a clone its source's, has a past only if
			// something encodes it: not an image, a color, or a web page.
			let collection = state.collection();
			let pictures = collection
				.sources
				.keys()
				.filter(|name| {
					let shown = collection.sources.get(collection.capture(name));
					shown.is_some_and(|source| {
						source.has_video()
							&& !matches!(
								source,
								Source::Image { .. }
									| Source::Color { .. } | Source::Browser { .. }
									| Source::Overlay { .. }
							)
					})
				})
				.cloned()
				.collect::<Vec<_>>();
			(pictures, named(|source| matches!(source, Source::Replay)))
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
					.on_hover_text(
						"Save the last moments, in real time, beside the scene file. Copied without re-encoding, a \
						 clip starts at the keyframe before them, and video with B-frames can end early.",
					)
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

		// Copied at the click, without waiting on anything, so the span ends
		// there and replays apply in the order they were asked for.
		let outcome = panel.outcome.clone();
		let into = panel.into.as_ref().filter(|_| play);
		let take = replay::take(&self.engine, &panel.of, panel.length, into.is_some());
		let take = match take {
			Ok(take) => take,
			Err(err) => {
				*outcome.lock().unwrap() = Some(Err(format!("{err:#}")));
				return;
			}
		};
		let of = panel.of.clone();
		if let Some(into) = into {
			let result =
				replay::play(&self.engine, take, panel.speed, into).map(|()| format!("replaying {of} in `{into}`"));
			*outcome.lock().unwrap() = Some(result.map_err(|err| format!("{err:#}")));
			return;
		}
		let engine = self.engine.clone();
		self.runtime.spawn(async move {
			let result = replay::save(take, engine.autosave.path())
				.await
				.map(|path| format!("saved {of} to {}", path.display()));
			*outcome.lock().unwrap() = Some(result.map_err(|err| format!("{err:#}")));
		});
	}

	/// The canvas's size and frame rate, applied at once rather than as each
	/// field changes, since a new canvas starts a new rendition.
	fn canvas_form(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let current = state.collection().canvas;
		let mut canvas = self.canvas.unwrap_or(current);
		let mut apply = false;
		ui.horizontal(|ui| {
			ui.strong("Canvas");
			ui.add(egui::DragValue::new(&mut canvas.width).range(2..=u32::MAX));
			ui.label("x");
			ui.add(egui::DragValue::new(&mut canvas.height).range(2..=u32::MAX));
			ui.label("at");
			ui.add(
				egui::DragValue::new(&mut canvas.framerate)
					.range(1..=u32::MAX)
					.suffix(" fps"),
			);
			let edited = canvas != current;
			apply = ui.add_enabled(edited, egui::Button::new("Apply")).clicked();
			if ui.add_enabled(edited, egui::Button::new("Reset")).clicked() {
				canvas = current;
			}
		});
		self.canvas = (canvas != current).then_some(canvas);
		if !apply {
			return;
		}
		let label = format!(
			"Set the canvas to {}x{} at {} fps",
			canvas.width, canvas.height, canvas.framerate
		);
		let result = edit(&mut state, label, None, |draft| draft.set_canvas(canvas));
		if result.is_ok() {
			self.canvas = None;
		}
		drop(state);
		self.check(result);
	}

	/// How the stream reaches viewers: what is wrong and why, each relay, the
	/// uplink, each track, and each viewer that reports.
	fn health(&mut self, ui: &mut Ui) {
		let report = self.engine.health.lock().unwrap();
		if report.relays.is_empty() {
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

			egui::Grid::new("relays").striped(true).show(ui, |ui| {
				for heading in ["Relay", "State", "Protocol"] {
					ui.strong(heading);
				}
				ui.end_row();
				for relay in &report.relays {
					ui.label(&relay.endpoint);
					match &relay.state {
						health::State::Connecting => ui.weak("connecting"),
						health::State::Connected => ui.colored_label(HEALTHY, "connected"),
						health::State::Disconnected(reason) => ui
							.colored_label(ui.visuals().error_fg_color, "unreachable")
							.on_hover_text(reason),
					};
					ui.label(or_unknown(relay.version.map(|version| version.to_string())));
					ui.end_row();
				}
			});
			ui.separator();

			let uplink = &report.uplink;
			let viewers = &report.viewers;
			egui::Grid::new("uplink").show(ui, |ui| {
				ui.label("Uplink");
				ui.horizontal(|ui| {
					verdict(ui, uplink.verdict);
					match &uplink.relay {
						Some(relay) => ui.weak(format!("to {relay}")),
						None => ui.weak("no relay connected"),
					};
				});
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
					Some(watching) => ui.label(format!("{watching} sessions, by the relays' stats")),
					None => ui.weak("unknown; no relay shares its stats with this studio"),
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
					"Source",
					"Track",
					"Granted",
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
				// Each source named once, beside its first track. The program's
				// tracks come first and name none, so no source name can pass
				// for them.
				let mut group = None;
				for track in &report.tracks {
					let source = track.source.as_deref();
					ui.label(if source == group {
						""
					} else {
						source.unwrap_or_default()
					});
					group = source;
					ui.label(&track.label).on_hover_text(&track.id);
					match (track.enabled, track.source.is_some()) {
						(false, _) => ui
							.label("off")
							.on_hover_text("The grant cannot fund it, so viewers use another"),
						(true, true) => ui.label(""),
						(true, false) => ui.label(or_unknown(track.granted.map(health::rate))),
					};
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

/// An edit the Scenes panel makes to the scene at an index.
enum SceneAction {
	Rename(usize, String),
	/// From one index to the other, counted after the removal.
	Move(usize, usize),
	Remove(usize),
}

impl SceneAction {
	fn index(&self) -> usize {
		match self {
			Self::Rename(index, _) | Self::Move(index, _) | Self::Remove(index) => *index,
		}
	}
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
	let name = filter_name(filter);
	match filter {
		Filter::Opacity { amount }
		| Filter::Brightness { amount }
		| Filter::Contrast { amount }
		| Filter::Saturate { amount }
		| Filter::Grayscale { amount }
		| Filter::Sepia { amount }
		| Filter::Invert { amount } => format!("{name} {:.0}%", amount * 100.0),
		Filter::HueRotate { degrees } => format!("{name} {degrees:.0}°"),
		Filter::Blur { radius } => format!("{name} {radius:.1} px"),
		Filter::Lut { source, .. } | Filter::Mask { source, .. } => format!("{name} `{source}`"),
		_ => name.to_string(),
	}
}

fn filter_name(filter: &Filter) -> &'static str {
	match filter {
		Filter::Opacity { .. } => "opacity",
		Filter::Brightness { .. } => "brightness",
		Filter::Contrast { .. } => "contrast",
		Filter::Saturate { .. } => "saturation",
		Filter::Grayscale { .. } => "grayscale",
		Filter::Sepia { .. } => "sepia",
		Filter::Invert { .. } => "invert",
		Filter::HueRotate { .. } => "hue rotation",
		Filter::Blur { .. } => "blur",
		Filter::DropShadow { .. } => "drop shadow",
		Filter::ChromaKey { .. } => "chroma key",
		Filter::LumaKey { .. } => "luma key",
		Filter::Lut { .. } => "LUT",
		Filter::Mask { .. } => "mask",
	}
}

fn blend_name(blend: Blend) -> &'static str {
	match blend {
		Blend::Normal => "normal",
		Blend::Multiply => "multiply",
		Blend::Screen => "screen",
		Blend::Overlay => "overlay",
		Blend::Darken => "darken",
		Blend::Lighten => "lighten",
		Blend::ColorDodge => "color dodge",
		Blend::ColorBurn => "color burn",
		Blend::HardLight => "hard light",
		Blend::SoftLight => "soft light",
		Blend::Difference => "difference",
		Blend::Exclusion => "exclusion",
		Blend::Hue => "hue",
		Blend::Saturation => "saturation",
		Blend::Color => "color",
		Blend::Luminosity => "luminosity",
	}
}

/// A new filter of each kind, as the Add menu offers it: one that leaves the
/// picture as it is where the kind allows. A LUT or mask names the first
/// source it can read, and is left out without one.
fn templates(pictures: &[String], luts: &[String]) -> Vec<Filter> {
	let mut filters = vec![
		Filter::Opacity { amount: 1.0 },
		Filter::Brightness { amount: 1.0 },
		Filter::Contrast { amount: 1.0 },
		Filter::Saturate { amount: 1.0 },
		Filter::Grayscale { amount: 0.0 },
		Filter::Sepia { amount: 0.0 },
		Filter::Invert { amount: 0.0 },
		Filter::HueRotate { degrees: 0.0 },
		Filter::Blur { radius: 4.0 },
		Filter::DropShadow {
			x: 4.0,
			y: 4.0,
			blur: 4.0,
			color: moq_scene::Rgba([0, 0, 0, 128]),
		},
		Filter::ChromaKey {
			color: moq_scene::Rgba([0, 255, 0, 255]),
			similarity: 0.4,
			smoothness: 0.08,
			spill: 0.1,
		},
		Filter::LumaKey {
			min: 0.0,
			max: 1.0,
			smoothness: 0.05,
		},
	];
	if let Some(source) = luts.first() {
		filters.push(Filter::Lut {
			source: source.clone(),
			amount: 1.0,
		});
	}
	if let Some(source) = pictures.first() {
		filters.push(Filter::Mask {
			source: source.clone(),
			mode: MaskMode::Alpha,
		});
	}
	filters
}

/// The gesture an edit through `response` belongs to, if it changed anything.
fn changed(response: &egui::Response, gestures: &mut Gestures) -> Option<Option<u64>> {
	let gesture = gestures.of(response);
	response.changed().then_some(gesture)
}

/// Edit `filter`'s settings; the gesture of the edit, if it made one.
fn filter_ui(
	ui: &mut Ui,
	filter: &mut Filter,
	pictures: &[String],
	luts: &[String],
	gestures: &mut Gestures,
) -> Option<Option<u64>> {
	ui.label(filter_name(filter));
	let mut edits = Vec::new();
	let slider = |ui: &mut Ui, value: &mut f32, range| ui.add(egui::Slider::new(value, range));
	match filter {
		Filter::Opacity { amount }
		| Filter::Grayscale { amount }
		| Filter::Sepia { amount }
		| Filter::Invert { amount } => {
			edits.push(slider(ui, amount, 0.0..=1.0));
		}
		Filter::Brightness { amount } | Filter::Contrast { amount } | Filter::Saturate { amount } => {
			edits.push(slider(ui, amount, 0.0..=3.0));
		}
		Filter::HueRotate { degrees } => edits.push(ui.add(egui::Slider::new(degrees, -180.0..=180.0).suffix("°"))),
		Filter::Blur { radius } => {
			edits.push(ui.add(egui::Slider::new(radius, 0.0..=100.0).logarithmic(true).suffix(" px")));
		}
		Filter::DropShadow { x, y, blur, color } => {
			edits.push(ui.add(egui::DragValue::new(x).prefix("x ").suffix(" px")));
			edits.push(ui.add(egui::DragValue::new(y).prefix("y ").suffix(" px")));
			edits.push(ui.add(egui::Slider::new(blur, 0.0..=50.0).suffix(" px")));
			edits.push(color_ui(ui, color));
		}
		Filter::ChromaKey {
			color,
			similarity,
			smoothness,
			spill,
		} => {
			edits.push(color_ui(ui, color));
			for (label, value) in [("similarity", similarity), ("smoothness", smoothness), ("spill", spill)] {
				ui.weak(label);
				edits.push(slider(ui, value, 0.0..=1.0));
			}
		}
		Filter::LumaKey { min, max, smoothness } => {
			for (label, value) in [("min", &mut *min), ("max", &mut *max), ("smoothness", smoothness)] {
				ui.weak(label);
				edits.push(slider(ui, value, 0.0..=1.0));
			}
			// Dragging one end past the other moves both.
			*max = max.max(*min);
		}
		Filter::Lut { source, amount } => {
			edits.push(pick(ui, "lut", source, luts));
			edits.push(slider(ui, amount, 0.0..=1.0));
		}
		Filter::Mask { source, mode } => {
			edits.push(pick(ui, "mask", source, pictures));
			let before = *mode;
			let mut response = egui::ComboBox::from_id_salt("mode")
				.selected_text(match mode {
					MaskMode::Alpha => "alpha",
					MaskMode::Luminance => "luminance",
				})
				.show_ui(ui, |ui| {
					ui.selectable_value(mode, MaskMode::Alpha, "alpha");
					ui.selectable_value(mode, MaskMode::Luminance, "luminance");
				})
				.response;
			if *mode != before {
				response.mark_changed();
			}
			edits.push(response);
		}
	}
	edits.iter().find_map(|response| changed(response, gestures))
}

/// Pick one of `names` for `value`.
fn pick(ui: &mut Ui, id: &str, value: &mut String, names: &[String]) -> egui::Response {
	let before = value.clone();
	let mut response = egui::ComboBox::from_id_salt(id)
		.selected_text(value.as_str())
		.show_ui(ui, |ui| {
			for name in names {
				ui.selectable_value(value, name.clone(), name);
			}
		})
		.response;
	if *value != before {
		response.mark_changed();
	}
	response
}

/// Edit `color` with straight alpha.
fn color_ui(ui: &mut Ui, color: &mut moq_scene::Rgba) -> egui::Response {
	let [r, g, b, a] = color.0;
	let mut edited = Color32::from_rgba_unmultiplied(r, g, b, a);
	let response = egui::color_picker::color_edit_button_srgba(ui, &mut edited, egui::color_picker::Alpha::OnlyBlend);
	if response.changed() {
		*color = moq_scene::Rgba(edited.to_srgba_unmultiplied());
	}
	response
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
	// A page or overlay, or a clone of one, lands at its own size, pixel for
	// pixel; anything else fills the canvas.
	let shown = match &source {
		Source::Clone { source } => state.collection().sources.get(source),
		source => Some(source),
	};
	let rect = match shown {
		Some(Source::Browser { width, height, .. } | Source::Overlay { width, height, .. }) => Rect {
			x: 0.0,
			y: 0.0,
			width: *width as f32,
			height: *height as f32,
		},
		_ => Rect::fill(&canvas),
	};
	edit(state, format!("Add `{name}`"), None, |draft| {
		draft.set_source(&name, Some(source))?;
		draft.set_video(&name, video)?;
		if place && let Some(layer) = layer {
			let index = draft.collection().items(&layer).map_or(0, Vec::len);
			draft.insert_item(&layer, index, Item::new(name.clone(), rect))?;
		}
		Ok(())
	})?;
	Ok(name)
}

/// Hand each input to its page.
fn deliver(
	sources: &moq_scene::source::Sources,
	inputs: impl IntoIterator<Item = (String, moq_scene::browser::Input)>,
) {
	for (name, input) in inputs {
		if let Some(page) = sources.get(&name).and_then(|feed| feed.page()) {
			page.send(input);
		}
	}
}

/// A join link on screen.
struct Shown {
	name: String,
	invite: crate::guests::Invite,
	/// Or why the link cannot be one, so it can still be copied.
	qr: Result<egui::TextureHandle, String>,
}

/// `text` as a QR code, one texel per module, inside the quiet zone scanners need.
fn qr(ctx: &egui::Context, text: &str) -> Result<egui::TextureHandle, String> {
	const QUIET: i32 = 4;
	let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Low)
		.map_err(|err| format!("The join link is too long for a QR code ({err:?})"))?;
	let side = code.size() + 2 * QUIET;
	let pixels = (0..side)
		.flat_map(|y| (0..side).map(move |x| (x, y)))
		// Outside the code, which is the quiet zone, reads as light.
		.map(|(x, y)| match code.get_module(x - QUIET, y - QUIET) {
			true => Color32::BLACK,
			false => Color32::WHITE,
		})
		.collect();
	let image = egui::ColorImage::new([side as usize; 2], pixels);
	Ok(ctx.load_texture("join link", image, egui::TextureOptions::NEAREST))
}

/// When a join link stops working, from `now`.
fn expiry(expires: kio::clock::Wall, now: kio::clock::Wall) -> String {
	match expires.checked_duration_since(now) {
		Some(left) if !left.is_zero() => {
			// Whole minutes, rounded up, so a link never reads as expired early.
			let minutes = left.as_secs().div_ceil(60).max(1);
			let left = Duration::from_secs(minutes * 60);
			format!("expires in {}", moq_tokio::cli::Duration::from(left))
		}
		_ => "expired".into(),
	}
}

/// Draw `texture` as large as fits, keeping the canvas's shape.
fn picture(ui: &mut Ui, texture: egui::TextureId, canvas: Canvas, sense: egui::Sense) -> egui::Response {
	let available = ui.available_size();
	let aspect = canvas.width as f32 / canvas.height as f32;
	let mut size = egui::vec2(available.x, available.x / aspect);
	if size.y > available.y {
		size = egui::vec2(available.y * aspect, available.y);
	}
	ui.vertical_centered(|ui| ui.add(egui::Image::new((texture, size)).sense(sense)))
		.inner
}

/// How the preview snaps a dragged item, for this session.
struct Snapping {
	on: bool,
	/// Whether the grid is drawn and snapped to.
	grid: bool,
	/// The grid's spacing in canvas pixels.
	spacing: u32,
}

impl Default for Snapping {
	fn default() -> Self {
		Self {
			on: true,
			grid: false,
			spacing: 40,
		}
	}
}

/// An item a drag in the preview holds.
struct Drag {
	layer: Layer,
	index: usize,
	grab: Grab,
	/// The item's rect, and where the pointer was in canvas pixels, when the
	/// drag started. Each frame drags from these, so snaps never accumulate.
	rect: Rect,
	origin: [f32; 2],
	/// Kept from the start, since the frame that releases the drag no longer
	/// reports it but still moves the item.
	gesture: Option<u64>,
	/// The snapped rect, its guides, and its spacing, unless snapping is off.
	hints: Option<snap::Snapped>,
}

/// How far, in points on screen, the pointer reaches to an edge or a line.
const REACH: f32 = 8.0;

/// The visible items of the scene in preview and the top layer, other than
/// the one at `index` of `layer`: what a dragged item snaps to.
fn others(state: &State, layer: &Layer, index: usize) -> Vec<Rect> {
	let scene = state
		.compositor
		.preview_scene()
		.map(|name| Layer::Scene(name.to_string()));
	let collection = state.collection();
	let mut rects = Vec::new();
	for shown in scene.into_iter().chain([Layer::Top]) {
		let items = collection.items(&shown).map_or(&[][..], Vec::as_slice);
		for (at, item) in items.iter().enumerate() {
			if item.visible && !(shown == *layer && at == index) {
				rects.push(item.rect);
			}
		}
	}
	rects
}

fn cursor(grab: Grab) -> egui::CursorIcon {
	use snap::Side::{End, Start};
	match grab {
		Grab::Resize { x: Some(_), y: None } => egui::CursorIcon::ResizeHorizontal,
		Grab::Resize { x: None, y: Some(_) } => egui::CursorIcon::ResizeVertical,
		Grab::Resize {
			x: Some(Start),
			y: Some(Start),
		}
		| Grab::Resize {
			x: Some(End),
			y: Some(End),
		} => egui::CursorIcon::ResizeNwSe,
		Grab::Resize { x: Some(_), y: Some(_) } => egui::CursorIcon::ResizeNeSw,
		Grab::Move | Grab::Resize { x: None, y: None } => egui::CursorIcon::Move,
	}
}

/// The preview's picture on screen, mapping canvas pixels to points.
struct View {
	rect: egui::Rect,
	/// Canvas pixels per point.
	scale: f32,
}

impl View {
	fn new(rect: egui::Rect, canvas: Canvas) -> Self {
		Self {
			rect,
			scale: canvas.width as f32 / rect.width(),
		}
	}

	/// The canvas position under `pos`.
	fn point(&self, pos: egui::Pos2) -> [f32; 2] {
		let offset = (pos - self.rect.min) * self.scale;
		[offset.x, offset.y]
	}

	/// A distance on screen, in canvas pixels.
	fn canvas(&self, points: f32) -> f32 {
		points * self.scale
	}

	fn pos(&self, x: f32, y: f32) -> egui::Pos2 {
		self.rect.min + egui::vec2(x, y) / self.scale
	}

	fn screen(&self, rect: &Rect) -> egui::Rect {
		egui::Rect::from_two_pos(
			self.pos(rect.x, rect.y),
			self.pos(rect.x + rect.width, rect.y + rect.height),
		)
	}

	/// The line at `at` on `axis`, from `from` to `to` across it.
	fn line(&self, axis: snap::Axis, at: f32, from: f32, to: f32) -> [egui::Pos2; 2] {
		match axis {
			snap::Axis::X => [self.pos(at, from), self.pos(at, to)],
			snap::Axis::Y => [self.pos(from, at), self.pos(to, at)],
		}
	}

	fn grid(&self, painter: &egui::Painter, spacing: f32) {
		// Any finer on screen, the grid is a wash.
		if spacing / self.scale < 4.0 {
			return;
		}
		let stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(32));
		let size = self.rect.size() * self.scale;
		for (axis, length, across) in [(snap::Axis::X, size.x, size.y), (snap::Axis::Y, size.y, size.x)] {
			let mut at = spacing;
			while at < length {
				painter.line_segment(self.line(axis, at, 0.0, across), stroke);
				at += spacing;
			}
		}
	}

	/// An outline with a handle at each corner and edge.
	fn selected(&self, painter: &egui::Painter, rect: &Rect, color: Color32) {
		let rect = self.screen(rect);
		painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, color), egui::StrokeKind::Outside);
		for x in [rect.left(), rect.center().x, rect.right()] {
			for y in [rect.top(), rect.center().y, rect.bottom()] {
				if (x, y) != (rect.center().x, rect.center().y) {
					let handle = egui::Rect::from_center_size(egui::pos2(x, y), egui::Vec2::splat(6.0));
					painter.rect_filled(handle, 1.0, color);
				}
			}
		}
	}

	fn hovered(&self, painter: &egui::Painter, rect: &Rect, color: Color32) {
		let stroke = egui::Stroke::new(1.0, color.gamma_multiply(0.6));
		painter.rect_stroke(self.screen(rect), 0.0, stroke, egui::StrokeKind::Outside);
	}

	/// The guides the dragged item lines up on, and the equal spaces beside it.
	fn hints(&self, painter: &egui::Painter, snapped: &snap::Snapped) {
		let stroke = egui::Stroke::new(1.0, GUIDE);
		for guide in &snapped.guides {
			painter.line_segment(self.line(guide.axis, guide.at, guide.from, guide.to), stroke);
		}
		for gap in &snapped.gaps {
			// A gap along X is a horizontal span: a line at its `at` on Y.
			let (across, tick) = match gap.axis {
				snap::Axis::X => (snap::Axis::Y, egui::vec2(0.0, 4.0)),
				snap::Axis::Y => (snap::Axis::X, egui::vec2(4.0, 0.0)),
			};
			let [start, end] = self.line(across, gap.at, gap.start, gap.end);
			painter.line_segment([start, end], stroke);
			for pos in [start, end] {
				painter.line_segment([pos - tick, pos + tick], stroke);
			}
		}
	}
}

/// Smart guides and spacing hints, bright enough to read over any picture.
const GUIDE: Color32 = Color32::from_rgb(255, 64, 160);

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
		Source::Browser { url, width, height } => format!("browser: {url} ({width}x{height})"),
		Source::Overlay { width, height, .. } => format!("overlay ({width}x{height})"),
		Source::Clone { source } => format!("clone of {source}"),
		Source::Replay => "replay".into(),
		Source::Lut { path } => format!("LUT: {}", path.display()),
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
	/// A path, broadcast path, url, device id, or an overlay's HTML, depending
	/// on the kind.
	value: String,
	color: Color32,
	/// A browser source's page size, or an overlay's.
	width: u32,
	height: u32,
}

impl Default for AddSource {
	fn default() -> Self {
		let canvas = moq_scene::Canvas::default();
		Self {
			name: String::new(),
			kind: Kind::default(),
			value: String::new(),
			color: Color32::WHITE,
			width: canvas.width,
			height: canvas.height,
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
	Browser,
	Overlay,
	Replay,
	Lut,
}

impl Kind {
	const ALL: [Self; 12] = [
		Self::Color,
		Self::Image,
		Self::Broadcast,
		Self::Camera,
		Self::Display,
		Self::Window,
		Self::Microphone,
		Self::SystemAudio,
		Self::Browser,
		Self::Overlay,
		Self::Replay,
		Self::Lut,
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
			Self::Browser => "Browser",
			Self::Overlay => "Overlay",
			Self::Replay => "Replay",
			Self::Lut => "LUT",
		}
	}

	/// What the value field holds, if this kind takes one.
	fn value(self) -> Option<&'static str> {
		match self {
			Self::Color | Self::SystemAudio | Self::Replay => None,
			Self::Image => Some("file"),
			Self::Lut => Some(".cube file"),
			Self::Broadcast => Some("path"),
			Self::Window => Some("window id"),
			Self::Browser => Some("url"),
			Self::Overlay => Some("HTML and CSS"),
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
				if self.kind == Kind::Overlay {
					ui.add(egui::TextEdit::multiline(&mut self.value).code_editor());
				} else {
					ui.text_edit_singleline(&mut self.value);
				}
				ui.end_row();
			}
			if matches!(self.kind, Kind::Browser | Kind::Overlay) {
				ui.label("Size");
				ui.horizontal(|ui| {
					ui.add(egui::DragValue::new(&mut self.width).range(1..=4096));
					ui.label("x");
					ui.add(egui::DragValue::new(&mut self.height).range(1..=4096));
				});
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
			Kind::Lut => Source::Lut { path: value.into() },
			Kind::Broadcast => Source::Broadcast { path: value },
			Kind::Camera => Source::Camera { id },
			Kind::Display => Source::Display { id },
			Kind::Window => Source::Window { id: value },
			Kind::Microphone => Source::Microphone { id },
			Kind::SystemAudio => Source::SystemAudio,
			Kind::Browser => Source::Browser {
				url: value,
				width: self.width,
				height: self.height,
			},
			Kind::Overlay => Source::Overlay {
				html: value,
				width: self.width,
				height: self.height,
			},
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

/// The capture devices this machine has, listed in the background.
#[derive(Default)]
struct Devices {
	listing: Option<Listing>,
	pending: Option<std::sync::mpsc::Receiver<Listing>>,
}

impl Devices {
	fn refresh(&mut self, runtime: &tokio::runtime::Handle) -> std::io::Result<()> {
		let (send, recv) = std::sync::mpsc::channel();
		// The capture backends' listing futures are `!Send` on macOS, as their
		// capture futures are (which is why each source runs on its own thread),
		// so poll the listing on a thread rather than spawn it. Not the runtime's
		// blocking pool, which dropping the runtime waits out, so a listing stuck
		// on a permission prompt cannot hold the studio open once it closes.
		let handle = runtime.clone();
		std::thread::Builder::new().name("devices".into()).spawn(move || {
			let _ = send.send(handle.block_on(devices::list()));
		})?;
		self.pending = Some(recv);
		Ok(())
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
				for (kind, devices) in listing.kinds() {
					ui.strong(kind);
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
