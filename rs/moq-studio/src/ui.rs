//! The window: one live picture, with the scenes, the panic key, and the
//! sound always around it, and pages for the rest. Settings and the stream's
//! health in detail are windows over the pages.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use egui::{Color32, Ui};
use moq_scene::browser::Paint;
use moq_scene::source::{Feed, Status};
use moq_scene::{
	Background, Blend, Canvas, Clip, Collection, Crop, Direction, Draft, Ease, Filter, Item, Keyframe, Layer, MaskMode,
	Rect, Scene, Source, Thumbnail, Transition, Video,
};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::controls::{Action, Controls};
use crate::devices::{self, Listing, Pick};
use crate::engine::{Engine, State};
use crate::fx::{Fx, Mark};
use crate::health::{self, Verdict};
use crate::hotkeys::{self, Bindings, Hotkeys, Reach};
use crate::interact::Interact;
use crate::kit::{self, Icon, IconButton, Tally, TallyFrame};
use crate::replay::{self, Of};
use crate::snap::{self, Grab, Snap};
use crate::{mix, record};

mod build;
mod graph;
mod inspector;
mod layers;
mod layout;
mod levels;
mod mixer;
mod review;
mod settings;
mod shell;

use levels::{Levels, meter};

/// The smallest the window gets, at which every part still has room.
pub const MIN_SIZE: egui::Vec2 = layout::MIN;

/// The pages, each the window's middle: the title bar and the strip of
/// scenes, the panic key, and the sound are around every one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
	/// The live picture, as large as the window allows.
	#[default]
	Live,
	/// The live picture with its layers, what can be added, and the
	/// selected layer's inspector.
	Build,
	/// Every sound and audio scene, with its sends.
	Mix,
	/// Replays, clips, and recordings.
	Review,
}

impl Page {
	const ALL: [Self; 4] = [Self::Live, Self::Build, Self::Mix, Self::Review];

	fn name(self) -> &'static str {
		match self {
			Self::Live => "Live",
			Self::Build => "Build",
			Self::Mix => "Mix",
			Self::Review => "Review",
		}
	}

	fn icon(self) -> Icon {
		match self {
			Self::Live => Icon::Live,
			Self::Build => Icon::Build,
			Self::Mix => Icon::Mix,
			Self::Review => Icon::Review,
		}
	}
}

/// The Settings window's tabs: what a broadcaster sets once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Setting {
	/// The canvas, each scene's size, the encoders, and the virtual cameras.
	#[default]
	Show,
	Hotkeys,
	Effects,
	/// The scene graph: every source, scene, and audio scene, wired.
	Advanced,
}

impl Setting {
	const ALL: [Self; 4] = [Self::Show, Self::Hotkeys, Self::Effects, Self::Advanced];

	fn name(self) -> &'static str {
		match self {
			Self::Show => "Show",
			Self::Hotkeys => "Hotkeys",
			Self::Effects => "Effects",
			Self::Advanced => "Advanced",
		}
	}
}

/// The BUILD page's left column: the layers, or what can be added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Side {
	#[default]
	Layers,
	Add,
	/// The inspector, a tab of its own when the window is too narrow for
	/// its column.
	Inspect,
}

impl Side {
	/// The tabs beside the inspector's own column.
	const ALL: [Self; 2] = [Self::Layers, Self::Add];

	fn name(self) -> &'static str {
		match self {
			Self::Layers => "Layers",
			Self::Add => "Add",
			Self::Inspect => "Inspector",
		}
	}
}

/// The inspector's tabs: the selected layer, or the scene with none, and the
/// layer's keyframes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Inspect {
	#[default]
	Layer,
	Animate,
}

/// The studio window.
pub struct Studio {
	panels: Panels,
	/// Keys saved for actions this version does not offer, such as Studio
	/// mode's Cut and Transition, kept for the version that does.
	hidden: Bindings,
}

impl Studio {
	/// The studio on `engine`, `framed` if its title bar is the window's frame.
	pub fn new(
		render: &eframe::egui_wgpu::RenderState,
		engine: Arc<Engine>,
		runtime: tokio::runtime::Handle,
		framed: bool,
	) -> Self {
		// egui samples whatever the compositor's textures hold each frame, so
		// they are registered again only when a resize replaces them, or the
		// program starts or stops showing layers over it.
		let (program, dropped) = {
			let mut state = engine.state.lock().unwrap();
			// Nothing shows the preview: the scene edited is the one on air,
			// until Studio mode (quest/m0/ux/studio-mode.md).
			state.compositor.set_previewing(false);
			(Registered::new(render, state.compositor.layered()), state.dropped)
		};
		let controls = Arc::new(Controls::new(engine.clone(), runtime.clone()));
		let mut panels = Panels {
			engine,
			runtime,
			controls,
			hotkeys: None,
			binding: None,
			hotkeys_shown: false,
			refusal: None,
			program,
			program_drawn: Canvas::default(),
			framed,
			page: Page::default(),
			setting: None,
			details: false,
			side: Side::default(),
			inspect: Inspect::default(),
			launched: kio::clock::now(),
			selection: None,
			cropping: false,
			gestures: Gestures::default(),
			snapping: Snapping::default(),
			direction: Direction::Left,
			duration: Transition::default().duration().unwrap_or_default(),
			cut: STINGER_CUT,
			drag: None,
			new_scene: String::new(),
			renaming: None,
			title_needs: 0.0,
			renaming_audio: None,
			renaming_pad: None,
			speaker: None,
			speakers: Speakers::default(),
			canvas: None,
			sizes: BTreeMap::new(),
			add: AddSource::default(),
			devices: Devices::default(),
			interacting: false,
			interact: Interact::default(),
			moving: None,
			staged: false,
			thumbnails: BTreeMap::new(),
			thumbnails_shown: BTreeSet::new(),
			bound: BTreeMap::new(),
			guest: String::new(),
			shown: None,
			message: None,
			faults: Vec::new(),
			reloads: 0,
			levels: Levels::default(),
			graph: graph::View::default(),
		};
		if dropped > 0 {
			panels.report(Err(anyhow::anyhow!(dropped_message(dropped))));
		}
		Self {
			panels,
			hidden: Bindings::new(),
		}
	}
}

#[cfg(test)]
impl Studio {
	/// Turn to `page`, as its button in the title bar would.
	pub(crate) fn turn(&mut self, page: Page) {
		self.panels.page = page;
	}

	/// Open Settings at `setting`, as the gear would.
	pub(crate) fn open(&mut self, setting: Setting) {
		self.panels.setting = Some(setting);
	}

	/// Open the stream's health in detail, as the health dot's popover would.
	pub(crate) fn details(&mut self) {
		self.panels.details = true;
	}

	/// Turn to BUILD with `side` and `inspect` shown, and the layer at
	/// `index` of the scene on program selected, front first, as a click on
	/// its row would.
	pub(crate) fn build(&mut self, side: Side, inspect: Inspect, index: Option<usize>) {
		let panels = &mut self.panels;
		panels.page = Page::Build;
		panels.side = side;
		panels.inspect = inspect;
		let state = panels.engine.state.lock().unwrap();
		let layer = state.compositor.program_scene().map(Layer::scene);
		let count = layer
			.as_ref()
			.and_then(|layer| state.collection().items(layer))
			.map_or(0, <[Item]>::len);
		panels.selection = index.zip(layer).map(|(index, layer)| Picked {
			layer,
			index: count - 1 - index,
		});
	}
}

/// Where this user's picks are kept, in the app's settings rather than the
/// scene file, which moves between machines without one operator's keys.
const SPEAKER: &str = "speaker";
const SNAPPING: &str = "snapping";
const HOTKEYS: &str = "hotkeys";

impl Studio {
	/// Take up what this user picked last time, and start hearing the keys
	/// they bound. Called from the window's thread, which `window` is.
	pub fn load(&mut self, window: hotkeys::Window<'_>) {
		let mut bindings = Bindings::new();
		if let Some(storage) = window.frame.storage() {
			let panels = &mut self.panels;
			panels.speaker = eframe::get_value::<Option<String>>(storage, SPEAKER).flatten();
			panels.snapping = eframe::get_value(storage, SNAPPING).unwrap_or_default();
			bindings = eframe::get_value(storage, HOTKEYS).unwrap_or_default();
		}
		// A key bound to an action not offered does nothing until it is.
		let (offered, hidden) = bindings.into_iter().partition(|(action, _)| action.offered());
		self.hidden = hidden;
		let hotkeys = Hotkeys::new(window, self.panels.controls.clone(), &self.panels.runtime, offered);
		self.panels.hotkeys = Some(hotkeys);
	}
}

/// Put back in `bindings` the key of each action not offered, which `hidden`
/// held, unless another action took that key meanwhile.
fn keep_hidden(bindings: &mut Bindings, hidden: &Bindings) {
	for (action, key) in hidden {
		if !bindings.values().any(|bound| bound.same(key)) {
			bindings.insert(action.clone(), key.clone());
		}
	}
}

impl Studio {
	pub fn save(&mut self, storage: &mut dyn eframe::Storage) {
		let panels = &self.panels;
		eframe::set_value(storage, SPEAKER, &panels.speaker);
		eframe::set_value(storage, SNAPPING, &panels.snapping);
		if let Some(hotkeys) = &panels.hotkeys {
			let mut bindings = hotkeys.bindings();
			keep_hidden(&mut bindings, &self.hidden);
			eframe::set_value(storage, HOTKEYS, &bindings);
		}
	}

	/// Run before each frame, and while the window is hidden, when `ui` is not.
	pub fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
		// A wait for a key ends once the operator leaves the window, such as
		// for a game, so the hotkeys are not held off while they play.
		let left = ctx.input(|input| {
			let viewport = input.viewport();
			viewport.focused == Some(false) || viewport.minimized == Some(true) || viewport.occluded == Some(true)
		});
		if left {
			self.panels.end_binding();
		}
		// Before each frame, and while the window is hidden, so the keys
		// follow a keyboard layout change while a game has focus.
		if let Some(hotkeys) = &mut self.panels.hotkeys {
			hotkeys.hear();
		}
	}

	/// Draw the window, with `fx` drawing its effects.
	pub fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, fx: &mut Fx) {
		let panels = &mut self.panels;
		// Everything the title bar, the strip, and the live picture show,
		// under one short lock, so drawing them never holds the engine's
		// state, which the program's render thread wants each frame.
		let snapshot = panels.snapshot(ui.ctx(), frame);
		if let Some(fired) = panels.controls.fired() {
			panels.say(fired);
		}
		if let Some(hotkeys) = &panels.hotkeys {
			hotkeys.focused(panels.interact.has_keyboard() || ui.ctx().text_edit_focused());
		}
		panels.take_binding(ui.ctx());
		panels.shortcuts(ui.ctx());
		panels.staged = false;
		let verdict = {
			let report = panels.engine.health.lock().unwrap();
			report
				.problems
				.first()
				.map_or(Verdict::Healthy, |problem| problem.verdict)
		};
		fx.follow(panels.engine.output.is_some(), snapshot.takes, verdict);
		panels.window(ui, fx, &snapshot);
		// A stage not shown hears no input, so it lets go of the pages, as
		// turning Interact off does.
		if !panels.staged {
			panels.let_go();
		}
		// A thumbnail nothing shows is not drawn, until it is again.
		let mut state = panels.engine.state.lock().unwrap();
		state.compositor.set_thumbnails(panels.thumbnails_shown.iter().cloned());
	}
}

/// A compositor texture as egui shows it.
pub(crate) struct Registered {
	pub(crate) id: egui::TextureId,
	texture: wgpu::Texture,
}

impl Registered {
	pub(crate) fn new(render: &eframe::egui_wgpu::RenderState, texture: &wgpu::Texture) -> Self {
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
	pub(crate) fn follow(&mut self, render: &eframe::egui_wgpu::RenderState, texture: &wgpu::Texture) {
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

/// What every page draws from, plus the half-typed forms.
struct Panels {
	engine: Arc<Engine>,
	runtime: tokio::runtime::Handle,
	/// What buttons and hotkeys both do, with the picks they share.
	controls: Arc<Controls>,
	/// The bound keys, once the window can register them.
	hotkeys: Option<Hotkeys>,
	/// The action the Hotkeys tab waits on a key for.
	binding: Option<Action>,
	/// Whether the Hotkeys tab was drawn since the last frame took this.
	hotkeys_shown: bool,
	/// The last key the Hotkeys tab would not bind, by the action it was
	/// pressed for, and why.
	refusal: Option<(Action, String)>,
	/// The program as viewers see it: the live picture.
	program: Registered,
	/// The program's size this frame, as its texture was drawn.
	program_drawn: Canvas,
	/// Whether the title bar is the window's frame, which moves, resizes,
	/// and closes it.
	framed: bool,
	/// The page the window shows.
	page: Page,
	/// The Settings window's tab, while it is open.
	setting: Option<Setting>,
	/// Whether the stream's health shows in detail.
	details: bool,
	/// The BUILD page's left column and inspector tabs.
	side: Side,
	inspect: Inspect,
	/// When the studio started, which it is live from with `--connect`, until
	/// going live is its own step (quest/m0/ux/go-live.md).
	launched: kio::clock::Instant,
	/// The layer the inspector edits, the picture's handles hold, and a copy
	/// takes.
	selection: Option<Picked>,
	/// Whether a drag on the selected layer's edges crops it rather than
	/// resizing it.
	cropping: bool,
	gestures: Gestures,
	snapping: Snapping,
	/// The direction a slide or wipe last had, which the transition picker
	/// offers again after a fade or move.
	direction: Direction,
	/// How long a timed transition last took, which the transition picker
	/// offers again after a stinger.
	duration: Duration,
	/// The cut point a stinger last had, which the transition picker offers
	/// again after another kind.
	cut: Duration,
	/// The layer a drag on the BUILD page's picture holds.
	drag: Option<Drag>,
	/// The layer row being dragged to a new place, by its index.
	moving: Option<usize>,
	new_scene: String,
	/// The scene being renamed, by index, and the name typed so far.
	renaming: Option<(usize, String)>,
	/// The width the title bar's full controls took when last drawn, past
	/// which it draws them compact.
	title_needs: f32,
	/// The audio scene being renamed, by its name, and the name typed so far.
	renaming_audio: Option<(String, String)>,
	/// The scene its pad's sheet renames, by name, and the name typed so far.
	renaming_pad: Option<(String, String)>,
	/// The output Listen plays on, by [`mix::Speaker::id`], or `None` for the
	/// system default. Saved per user, not in the shared scene file.
	speaker: Option<String>,
	speakers: Speakers,
	/// The canvas the Show tab's form holds until it is applied, or `None`
	/// while it shows the collection's.
	canvas: Option<Canvas>,
	/// The size each scene's form holds until it is applied, by name.
	sizes: BTreeMap<String, (u32, u32)>,
	add: AddSource,
	devices: Devices,
	/// Whether the BUILD page's picture passes the mouse and keyboard to the
	/// selected layer's web page, until Escape.
	interacting: bool,
	interact: Interact,
	/// Whether this frame drew the BUILD page's picture, which hears input.
	staged: bool,
	/// Each thumbnail the compositor drew of those shown last frame.
	thumbnails: BTreeMap<Thumbnail, Registered>,
	/// The thumbnails this frame shows, which the compositor draws from now on.
	thumbnails_shown: BTreeSet<Thumbnail>,
	/// The page `interact` holds state for under each name, so one stopped or
	/// restarted under it, as an edit or a reload does, is forgotten.
	bound: BTreeMap<String, moq_scene::browser::Page>,
	/// The name typed for the next guest.
	guest: String,
	/// The join link on screen.
	shown: Option<Shown>,
	/// The outcome of the last action that happened nowhere in particular,
	/// and when, shown for a while.
	message: Option<(Result<String, String>, kio::clock::Instant)>,
	/// Failures shown where they happened, newest last.
	faults: Vec<Fault>,
	/// The engine's reloads seen so far.
	reloads: u64,
	/// Each sound's and audio scene's recent levels, for the meters.
	levels: Levels,
	graph: graph::View,
}

/// A layer, by the list it is in and its place there, back to front.
#[derive(Clone, Debug, PartialEq)]
struct Picked {
	layer: Layer,
	index: usize,
}

impl Picked {
	/// The layer as `collection` has it, if it is still there.
	fn item<'a>(&self, collection: &'a Collection) -> Option<&'a Item> {
		collection.items(&self.layer)?.get(self.index)
	}

	/// What its look edits.
	fn look(&self) -> Selection {
		Selection::Item(self.layer.clone(), self.index)
	}
}

/// Where a failure happened, which shows it there.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Spot {
	/// The rows and the inspector of the layers of `Layer` showing a source.
	Layer(Layer, String),
	/// A scene's pad, by its name.
	Pad(String),
	/// The scene's inspector, with no layer selected.
	Scene,
	/// The Add tab.
	Add,
	/// The picture itself, such as for a paste.
	Picture,
}

/// A failure at a spot, and when, shown there for a while.
struct Fault {
	spot: Spot,
	text: String,
	at: kio::clock::Instant,
}

/// How long a failure shows where it happened, unless the next edit there
/// succeeds first.
const FAULT: Duration = Duration::from_secs(10);

/// A source or a layer: what a look editor edits.
#[derive(Clone, Debug, PartialEq)]
enum Selection {
	Source(String),
	Item(Layer, usize),
}

/// What a look editor sets on a selection.
#[derive(Clone, Debug, PartialEq)]
struct Look {
	/// A source's background, where it can be removed; an item has none.
	background: Option<Background>,
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
				let source = collection.sources.get(name).filter(|source| source.has_video())?;
				let video = collection.video.get(name).cloned().unwrap_or_default();
				Some(Look {
					background: source.removes_background().then_some(video.background),
					crop: video.crop,
					filters: video.filters,
					blend: None,
				})
			}
			Self::Item(layer, index) => {
				let item = collection.items(layer)?.get(*index)?;
				Some(Look {
					background: None,
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
		let Look {
			background,
			crop,
			filters,
			blend,
		} = look;
		match self {
			Self::Source(name) => draft.set_video(
				name,
				Some(Video {
					background: background.unwrap_or_default(),
					crop,
					filters,
				}),
			),
			Self::Item(layer, index) => {
				let item = draft
					.collection()
					.items(layer)
					.and_then(|items| items.get(*index))
					.cloned();
				let item =
					item.ok_or_else(|| moq_scene::Error::Conflict(format!("there is no layer {index} in {layer}")))?;
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
				format!("`{source}` {}", place(layer))
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
		// The release is the drag's last frame, and can still edit.
		(response.dragged() || response.drag_stopped() || response.has_focus()).then_some(self.0)
	}

	/// A gesture of its own.
	fn next(&mut self) -> u64 {
		self.0 += 1;
		self.0
	}
}

/// Where `layer`'s layers are, as an edit's label says it: in a scene, or
/// always on top.
fn place(layer: &Layer) -> String {
	match &layer.scene {
		Some(scene) => format!("in scene `{scene}`"),
		None => "always on top".into(),
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
		// A focused text field, or a page the picture passes the keyboard to,
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
		self.say(result.map_err(|err| format!("{err:#}")));
	}

	/// Show `message` for a while.
	fn say(&mut self, message: Result<String, String>) {
		self.message = Some((message, kio::clock::now()));
	}

	/// Report a failure that happened nowhere in particular; a success shows
	/// for itself.
	fn check(&mut self, result: anyhow::Result<()>) {
		if let Err(err) = result {
			self.report(Err(err));
		}
	}

	/// Show a failure at `spot`, or clear the last one there on a success.
	fn fail(&mut self, spot: Spot, result: anyhow::Result<()>) {
		self.faults.retain(|fault| fault.spot != spot);
		if let Err(err) = result {
			self.faults.push(Fault {
				spot,
				text: format!("{err:#}"),
				at: kio::clock::now(),
			});
		}
	}

	/// The failure showing at `spot`, if one does.
	fn fault(&self, spot: &Spot) -> Option<&str> {
		let now = kio::clock::now();
		let fault = self.faults.iter().rev().find(|fault| fault.spot == *spot)?;
		(now.saturating_duration_since(fault.at) < FAULT).then_some(fault.text.as_str())
	}

	/// Keep a selected layer of `layer` on the one it named, now at `moved`'s
	/// index, or deselect it if `moved` says it is gone.
	fn follow(&mut self, layer: &Layer, moved: impl FnOnce(usize) -> Option<usize>) {
		if let Some(picked) = &self.selection
			&& picked.layer == *layer
		{
			self.selection = moved(picked.index).map(|index| Picked {
				layer: layer.clone(),
				index,
			});
		}
	}

	/// Deselect the layer, after an edit that may have moved any of them, and
	/// let go of any drag, crop, and scene rename under way.
	fn deselect_item(&mut self) {
		self.selection = None;
		self.drag = None;
		self.moving = None;
		self.cropping = false;
		self.renaming = None;
	}

	/// Select `picked`, leaving cropping and interacting with the last one.
	fn select(&mut self, picked: Option<Picked>) {
		if self.selection != picked {
			self.cropping = false;
		}
		self.selection = picked;
	}

	/// The layers BUILD edits, and new sources go in: the scene on program's.
	/// The always-on-top layers over it are [`Layer::top`].
	fn scene(&self, state: &State) -> Result<Layer, String> {
		match state.compositor.program_scene() {
			Some(name) => Ok(Layer::scene(name)),
			None => Err("Take a scene to edit what it shows.".into()),
		}
	}

	/// Copy the selected layer, with what it shows, to the clipboard.
	fn copy(&mut self, ctx: &egui::Context) {
		let engine = self.engine.clone();
		let state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let collection = state.collection();
		let item = self.selection.as_ref().and_then(|picked| picked.item(collection));
		let Some(item) = item.cloned() else {
			drop(state);
			self.report(Err(anyhow::anyhow!("select a layer to copy it")));
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

	/// Paste layers into the selected layer's list, or the scene on program,
	/// or filters onto the selected layer.
	fn paste(&mut self, text: &str) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let mut spot = Spot::Picture;
		let result = (|| {
			let clip = Clip::from_toml(text).context("the clipboard holds no layers or filters")?;
			match clip {
				Clip::Items(items) => {
					let layer = match &self.selection {
						Some(picked) => picked.layer.clone(),
						None => self.scene(&state).map_err(anyhow::Error::msg)?,
					};
					let dir = state.sources.context().dir.clone();
					edit(&mut state, format!("Paste into {}", place(&layer)), None, |draft| {
						draft.paste(&layer, &items, &dir)
					})?;
					Ok(())
				}
				Clip::Filters(pasted) => {
					let picked = self.selection.clone().context("select a layer to paste filters onto")?;
					let item = picked.item(state.collection()).context("the selected layer is gone")?;
					spot = Spot::Layer(picked.layer.clone(), item.source.clone());
					let selection = picked.look();
					let mut filters = selection
						.filters(state.collection())
						.context("the layer has no picture to filter")?;
					filters.extend(pasted.filters);
					let what = selection.describe(state.collection());
					edit(&mut state, format!("Paste filters onto {what}"), None, |draft| {
						selection.set_filters(draft, filters)
					})
				}
			}
		})();
		drop(state);
		self.fail(spot, result);
	}

	/// Let go of every page the picture passed input to.
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

	/// Keep a selected layer of the scene renamed from `from` selected.
	fn renamed(&mut self, from: &str, to: &str) {
		if let Some(Picked {
			layer: Layer { scene: Some(scene) },
			..
		}) = &mut self.selection
			&& scene == from
		{
			*scene = to.to_string();
		}
	}
}

/// Who watches one broadcast, how each of its tracks reaches them, and what
/// each viewer reports.
fn broadcast_health(ui: &mut Ui, broadcast: &health::Published) {
	let viewers = &broadcast.viewers;
	egui::Grid::new(("watching", &broadcast.name)).show(ui, |ui| {
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
	tracks_health(ui, ("tracks", &broadcast.name), &broadcast.tracks, false);

	if viewers.reports.is_empty() {
		return;
	}
	egui::Grid::new(("viewers", &broadcast.name))
		.striped(true)
		.show(ui, |ui| {
			for heading in ["Viewer", "Grade", "Round trip", "Latency"] {
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
				ui.end_row();
			}
		});
}

/// Each of `tracks`, under the source whose own it is when `sources`.
fn tracks_health(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, tracks: &[health::Track], sources: bool) {
	egui::Grid::new(id).striped(true).show(ui, |ui| {
		let headings = [
			"Track",
			"Granted",
			"Sent",
			"Frames",
			"Viewers",
			"Lowest received",
			"Latency",
			"Grade",
		];
		if sources {
			ui.strong("Source");
		}
		for heading in headings {
			ui.strong(heading);
		}
		ui.end_row();
		// Each source named once, beside its first track.
		let mut group = None;
		for track in tracks {
			if sources {
				let source = track.source.as_deref();
				ui.label(if source == group {
					""
				} else {
					source.unwrap_or_default()
				});
				group = source;
			}
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
}

/// The recordings a crash left unfinished, found at launch, and how each
/// repair went.
fn repairs(ui: &mut Ui, repairs: &[record::Repair]) {
	if repairs.is_empty() {
		return;
	}
	let file = |path: &std::path::Path| path.file_name().unwrap_or_default().to_string_lossy().into_owned();
	for repair in repairs {
		let name = file(&repair.path);
		match &repair.outcome {
			None => ui.weak(format!("Repairing {name}, which the studio left unfinished")),
			Some(record::Repaired::Saved(repaired)) => ui.colored_label(
				kit::CUE,
				format!("{name} was left unfinished; repaired as {}", file(repaired)),
			),
			Some(record::Repaired::Finished) => ui.weak(format!("{name} was already finished")),
			Some(record::Repaired::Empty) => ui.weak(format!("{name} was left unfinished with nothing recorded")),
			Some(record::Repaired::Failed(reason)) => {
				ui.colored_label(ui.visuals().error_fg_color, format!("Cannot repair {name}: {reason}"))
			}
		}
		.on_hover_text(repair.path.display().to_string());
	}
	ui.separator();
}

/// A verdict's color: green while healthy, amber while degraded, red once
/// unhealthy, which is a failure.
fn color(verdict: Verdict) -> Color32 {
	match verdict {
		Verdict::Healthy => kit::OK,
		Verdict::Degraded => kit::CUE,
		Verdict::Unhealthy => kit::ERR,
		Verdict::Unknown => kit::DIM,
	}
}

fn verdict(ui: &mut Ui, verdict: Verdict) {
	let text = match verdict {
		Verdict::Unknown => "Unknown",
		Verdict::Healthy => "Healthy",
		Verdict::Degraded => "Degraded",
		Verdict::Unhealthy => "Unhealthy",
	};
	ui.add(kit::Pill::new(text).dot(color(verdict)));
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
	/// Publish it, or stop.
	Publish(usize, bool),
	/// Carry the audio scene by this name while published, or the one on
	/// program.
	Audio(usize, Option<String>),
	/// Make it the scene the panic key cuts to, or stop.
	Safe(usize, bool),
}

impl SceneAction {
	fn index(&self) -> usize {
		match self {
			Self::Rename(index, _)
			| Self::Move(index, _)
			| Self::Remove(index)
			| Self::Publish(index, _)
			| Self::Audio(index, _)
			| Self::Safe(index, _) => *index,
		}
	}
}

/// An edit the Scenes panel makes to the audio scenes, each by its place.
enum AudioAction {
	/// To the name typed.
	Rename(usize, String),
	Move(usize, usize),
	Remove(usize),
}

/// A strip of the mixer, which sends into audio scenes: a source's sound, or
/// an audio scene, each by name.
#[derive(Clone, Copy)]
enum Strip<'a> {
	Sound(&'a str),
	Scene(&'a str),
}

impl Strip<'_> {
	/// The strip as an edit's label names it.
	fn what(self) -> String {
		match self {
			Self::Sound(name) => format!("`{name}`"),
			Self::Scene(name) => format!("audio scene `{name}`"),
		}
	}

	/// What sending it into the audio scene `to`, or stopping, is called in
	/// the history.
	fn label(self, to: &str, on: bool) -> String {
		let what = self.what();
		match on {
			true => format!("Put {what} in `{to}`"),
			false => format!("Take {what} out of `{to}`"),
		}
	}

	/// Send it into the audio scene `to` of `scenes`, or stop; whether that
	/// changed them.
	fn send(self, scenes: &mut [moq_scene::AudioScene], to: &str, on: bool) -> bool {
		let Some(into) = scenes.iter_mut().find(|scene| scene.name == to) else {
			return false;
		};
		let (held, name) = match self {
			Self::Sound(name) => (&mut into.sounds, name),
			Self::Scene(name) => (&mut into.scenes, name),
		};
		match on {
			true => held.insert(name.to_string()),
			false => held.remove(name),
		}
	}
}

/// `scenes` with the one called `name` on program instead, and the edit's
/// label.
fn on_program(scenes: &[moq_scene::AudioScene], name: &str) -> (Vec<moq_scene::AudioScene>, String) {
	let mut next = scenes.to_vec();
	for scene in &mut next {
		scene.program = scene.name == name;
	}
	(next, format!("Put audio scene `{name}` on program"))
}

/// A sound's mute, gain, pan, and delay, as its Audio panel strip and Graph node
/// show them, and the change made to `audio` with its label and gesture.
fn strip_controls(
	ui: &mut Ui,
	gestures: &mut Gestures,
	name: &str,
	audio: &moq_scene::Audio,
) -> Option<(moq_scene::Audio, String, Option<u64>)> {
	let mut change = None;
	let (icon, verb) = match audio.muted {
		true => (Icon::Muted, "Unmute"),
		false => (Icon::Sound, "Mute"),
	};
	if ui
		.add(IconButton::new(icon, verb).lit(audio.muted, kit::CUE).small())
		.clicked()
	{
		let audio = moq_scene::Audio {
			muted: !audio.muted,
			..audio.clone()
		};
		change = Some((audio, format!("{verb} `{name}`"), None));
	}
	// The slider runs over decibels, from silence to 12 dB of boost, and the
	// field beside it takes an exact level.
	let db = decibels(audio.gain);
	let mut at = (db - MUTE_DB) / (12.0 - MUTE_DB);
	let mut typed = db;
	// The slider and its field share one cell, so a grid's columns stay
	// under their headings.
	let (slid, field) = ui
		.horizontal(|ui| {
			let slid = ui.add(kit::Slider::new(&mut at).width(120.0)).on_hover_text("Gain");
			let level = egui::DragValue::new(&mut typed)
				.range(MUTE_DB..=12.0)
				// A scene file may hold more boost; showing it must not cut it.
				.clamp_existing_to_range(false)
				.speed(0.1)
				.custom_formatter(|db, _| match db <= f64::from(MUTE_DB) {
					true => "-inf".to_string(),
					false => format!("{db:+.1}"),
				})
				.custom_parser(|text| match text.trim() {
					"-inf" => Some(f64::from(MUTE_DB)),
					text => text.parse().ok(),
				});
			let field = ui
				.add(kit::Number::new(level).unit("dB").width(84.0))
				.on_hover_text("Gain in decibels: drag, or type a level such as 0");
			(slid, field)
		})
		.inner;
	for (response, db) in [(slid, MUTE_DB + at * (12.0 - MUTE_DB)), (field, typed)] {
		let gesture = gestures.of(&response);
		if response.changed() {
			let gain = if db <= MUTE_DB { 0.0 } else { 10f32.powf(db / 20.0) };
			let audio = moq_scene::Audio { gain, ..audio.clone() };
			change = Some((audio, format!("Set `{name}` gain"), gesture));
		}
	}
	let mut pan = audio.pan;
	let response = ui
		.add(
			egui::Slider::new(&mut pan, -1.0..=1.0)
				.custom_formatter(|pan, _| match pan {
					0.0 => "C".to_string(),
					pan if pan < 0.0 => format!("L{:.0}", -pan * 100.0),
					pan => format!("R{:.0}", pan * 100.0),
				})
				.custom_parser(|text| {
					let text = text.trim().to_ascii_uppercase();
					let percent = |side: &str| side.trim().parse::<f64>().ok().map(|n| n / 100.0);
					match text.as_str() {
						"C" => Some(0.0),
						_ if let Some(side) = text.strip_prefix('L') => percent(side).map(|n| -n),
						_ if let Some(side) = text.strip_prefix('R') => percent(side),
						_ => text.parse().ok(),
					}
				}),
		)
		.on_hover_text("Pan: place the sound between left and right; double-click to center");
	let gesture = gestures.of(&response);
	if response.double_clicked() {
		pan = 0.0;
	}
	if pan != audio.pan {
		let audio = moq_scene::Audio { pan, ..audio.clone() };
		change = Some((audio, format!("Set `{name}` pan"), gesture));
	}
	let max = moq_audio::bus::MAX_DELAY.as_millis() as u64;
	let mut millis = audio.delay.as_millis() as u64;
	let response = ui
		.add(kit::Number::new(egui::DragValue::new(&mut millis).range(0..=max)).unit("ms"))
		.on_hover_text("Delay: hold the source back to line it up with a camera that lags it");
	let gesture = gestures.of(&response);
	if response.changed() {
		let delay = Duration::from_millis(millis);
		let audio = moq_scene::Audio { delay, ..audio.clone() };
		change = Some((audio, format!("Set `{name}` delay"), gesture));
	}
	change
}

/// A scene's Publish setting, and the audio scene its broadcast carries
/// while published, as its Scenes panel row and Graph node show them.
fn scene_settings(
	ui: &mut Ui,
	index: usize,
	scene: &Scene,
	audio_scenes: &[moq_scene::AudioScene],
	program_audio: &str,
) -> Option<SceneAction> {
	let mut action = None;
	let mut safe = scene.safe;
	if ui
		.add(kit::Toggle::new(&mut safe).label("Safe"))
		.on_hover_text("The scene the panic key cuts to, as it mutes every microphone and guest")
		.changed()
	{
		action = Some(SceneAction::Safe(index, safe));
	}
	let mut published = scene.published;
	if ui
		.add(kit::Toggle::new(&mut published).label("Publish"))
		.on_hover_text("Live as its own broadcast beneath the program's, at its own size")
		.changed()
	{
		action = Some(SceneAction::Publish(index, published));
	}
	if scene.published {
		let on_program = format!("on program ({program_audio})");
		let mut audio = scene.audio.clone();
		egui::ComboBox::from_id_salt(("scene audio", index))
			.selected_text(audio.as_deref().unwrap_or(&on_program))
			.show_ui(ui, |ui| {
				ui.selectable_value(&mut audio, None, &on_program);
				for scene in audio_scenes {
					ui.selectable_value(&mut audio, Some(scene.name.clone()), &scene.name);
				}
			})
			.response
			.on_hover_text("The audio scene its broadcast carries");
		if audio != scene.audio {
			action = Some(SceneAction::Audio(index, audio));
		}
	}
	action
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

fn transition_name(transition: &Transition) -> &'static str {
	match transition {
		Transition::Fade { .. } => "fade",
		Transition::Slide { .. } => "slide",
		Transition::Wipe { .. } => "wipe",
		Transition::Move { .. } => "move",
		Transition::Stinger { .. } => "stinger",
	}
}

/// Where the transition picker puts a new stinger's cut point.
const STINGER_CUT: Duration = Duration::from_millis(500);

fn direction_name(direction: Direction) -> &'static str {
	match direction {
		Direction::Left => "left",
		Direction::Right => "right",
		Direction::Up => "up",
		Direction::Down => "down",
	}
}

fn ease_name(ease: Ease) -> &'static str {
	match ease {
		Ease::Linear => "linear",
		Ease::In => "ease in",
		Ease::Out => "ease out",
		Ease::InOut => "ease in and out",
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

/// The source and video settings of a clone of `name`. A clone of a clone
/// shows the same capture, so it clones that. It copies the crop and filters,
/// but not a removed background: the clone already shows the cut-out.
fn clone_of(collection: &Collection, name: &str) -> (Source, Option<Video>) {
	let source = Source::Clone {
		source: collection.capture(name).to_string(),
	};
	let video = collection.video.get(name).map(|video| Video {
		background: Background::Keep,
		..video.clone()
	});
	(source, video)
}

/// Add `source` as `name`, with `video` filters, place it in front of
/// `layer` if it has a picture, and put its sound in the audio scene on
/// program if it has sound. Returns the name.
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
	let canvas = layer
		.as_ref()
		.and_then(|layer| state.collection().canvas(layer.scene.as_deref()));
	let canvas = canvas.unwrap_or(state.collection().canvas);
	let place = source.has_video();
	let heard = source.has_audio();
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
		if heard {
			draft.hear(&name)?;
		}
		if place && let Some(layer) = layer {
			let index = draft.collection().items(&layer).map_or(0, <[Item]>::len);
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

/// What each encoder of the published scene `name`, the program for `None`,
/// costs: the sizes it publishes, or with `recording`, what the recording's
/// own encoder costs.
fn renditions(ui: &mut Ui, name: Option<&str>, video: &crate::engine::VideoStats, recording: bool) {
	let count = |value: &std::sync::atomic::AtomicU64| value.load(Ordering::Relaxed);
	let millis = |value: &std::sync::atomic::AtomicU64| format!("{:.1} ms", count(value) as f64 / 1000.0);
	// Only the sizes published, since a small picture, or an encoder that
	// refused the half size, has the full size alone.
	let [canvas, half, record] = &video.renditions;
	let published = video.sizes.load(Ordering::Relaxed);
	let mut columns = match recording {
		true => vec![("Recording", record)],
		false => vec![("Full size", canvas)],
	};
	if !recording && published > 1 {
		columns.push(("Half size", half));
	}
	egui::Grid::new(("renditions", name, recording)).show(ui, |ui| {
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
		let zero_copy = video.zero_copy.load(Ordering::Relaxed);
		row(ui, "Read back to the CPU", &|load| match zero_copy {
			true => "none; the encoder reads the GPU".to_string(),
			false => format!("{} frames", count(&load.read_back)),
		});
		row(ui, "Dropped", &|load| format!("{} frames", count(&load.dropped)));
		row(ui, "Encoded", &|load| {
			format!("{} frames, last {}", count(&load.encoded), millis(&load.encode_us))
		});
	});
	let off = match recording {
		true => record.error().map(|error| format!("recording encoder off: {error}")),
		false => half.error().map(|error| format!("half size off: {error}")),
	};
	if let Some(off) = off {
		ui.colored_label(ui.visuals().warn_fg_color, off);
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

/// How the live picture snaps a dragged item, kept in this user's settings.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
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

/// A layer a drag on the BUILD page's picture holds.
struct Drag {
	picked: Picked,
	grab: Grab,
	/// The layer's rect and crop, and where the pointer was in canvas
	/// pixels, when the drag started. Each frame drags from these, so snaps
	/// never accumulate.
	rect: Rect,
	crop: Crop,
	origin: [f32; 2],
	/// Whether the item's source draws a picture of its own shape, which a
	/// resize keeps unless Shift is held.
	fixed: bool,
	/// Kept from the start, since the frame that releases the drag no longer
	/// reports it but still moves the item.
	gesture: Option<u64>,
	/// The snapped rect, its guides, and its spacing, unless snapping is off.
	hints: Option<snap::Snapped>,
}

/// How far, in points on screen, the pointer reaches to an edge or a line.
const REACH: f32 = 8.0;

/// Where the visible layers of the scene on program and the always-on-top
/// ones over it show `since` the scene went on program, other than
/// `dragged`: what a dragged layer snaps to.
fn others(state: &State, dragged: &Picked, since: Duration) -> Vec<Rect> {
	let scene = state.compositor.program_scene();
	let collection = state.collection();
	let mut rects = Vec::new();
	for shown in scene.map(Layer::scene).into_iter().chain([Layer::top()]) {
		let items = collection.items(&shown).unwrap_or_default();
		for (at, item) in items.iter().enumerate() {
			if item.visible && !(shown == dragged.layer && at == dragged.index) {
				rects.push(item.posed(since).rect);
			}
		}
	}
	rects
}

/// Whether `source` draws a picture of its own shape, as a camera, a screen,
/// an image, or a video does, rather than filling any shape.
fn fixed_shape(source: Option<&Source>) -> bool {
	matches!(
		source,
		Some(
			Source::Camera { .. }
				| Source::Display { .. }
				| Source::Window { .. }
				| Source::PickedDisplay
				| Source::PickedWindow
				| Source::Image { .. }
				| Source::Media { .. }
				| Source::Broadcast { .. }
				| Source::Replay
		)
	)
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

/// The live picture on screen, mapping canvas pixels to points.
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

	/// An outline with a handle at each corner and edge: squares that
	/// resize, or bars that crop while `cropping`.
	fn selected(&self, painter: &egui::Painter, rect: &Rect, color: Color32, cropping: bool) {
		let rect = self.screen(rect);
		painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.5, color), egui::StrokeKind::Outside);
		for x in [rect.left(), rect.center().x, rect.right()] {
			for y in [rect.top(), rect.center().y, rect.bottom()] {
				if (x, y) == (rect.center().x, rect.center().y) {
					continue;
				}
				let size = match (cropping, x == rect.center().x, y == rect.center().y) {
					(false, _, _) => egui::Vec2::splat(7.0),
					(true, true, _) => egui::vec2(16.0, 4.0),
					(true, _, true) => egui::vec2(4.0, 16.0),
					(true, _, _) => egui::Vec2::splat(9.0),
				};
				let handle = egui::Rect::from_center_size(egui::pos2(x, y), size);
				let edge = egui::Stroke::new(1.0, Color32::WHITE);
				painter.rect(handle, 1.0, color, edge, egui::StrokeKind::Outside);
			}
		}
	}

	/// While cropping a layer at `rect` showing `crop` of its picture, where
	/// the whole picture would be, dashed, so a drag can bring it back.
	fn whole(&self, painter: &egui::Painter, rect: &Rect, crop: &Crop) {
		let (wide, tall) = (1.0 - crop.left - crop.right, 1.0 - crop.top - crop.bottom);
		if wide <= 0.0 || tall <= 0.0 {
			return;
		}
		let (width, height) = (rect.width / wide, rect.height / tall);
		let whole = Rect {
			x: rect.x - crop.left * width,
			y: rect.y - crop.top * height,
			width,
			height,
		};
		let stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(120));
		let outline = self.screen(&whole);
		let corners = [
			outline.left_top(),
			outline.right_top(),
			outline.right_bottom(),
			outline.left_bottom(),
			outline.left_top(),
		];
		for side in corners.windows(2) {
			painter.extend(egui::Shape::dashed_line(side, stroke, 4.0, 4.0));
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
const GUIDE: Color32 = Color32::from_rgb(0xff, 0x3d, 0xf0);

fn describe(source: &Source) -> String {
	let or_default = |id: &Option<String>| id.clone().unwrap_or_else(|| "default".into());
	match source {
		Source::Camera { id } => format!("camera: {}", or_default(id)),
		Source::Display { id } => format!("display: {}", or_default(id)),
		Source::Window { id } => format!("window: {id}"),
		Source::PickedDisplay => "display: picked".into(),
		Source::PickedWindow => "window: picked".into(),
		Source::Microphone { id } => format!("microphone: {}", or_default(id)),
		Source::SystemAudio => "system audio".into(),
		Source::Image { path } => format!("image: {}", path.display()),
		Source::Media { path, looping, .. } => {
			let ending = if *looping { "looping" } else { "once" };
			format!("media: {} ({ending})", path.display())
		}
		Source::Color { color } => format!("color: {color}"),
		Source::Broadcast { path } => format!("broadcast: {path}"),
		Source::Browser { url, width, height } => format!("browser: {url} ({width}x{height})"),
		Source::Overlay { width, height, .. } => format!("overlay ({width}x{height})"),
		Source::Clone { source } => format!("clone of {source}"),
		Source::Replay => "replay".into(),
		Source::Lut { path } => format!("LUT: {}", path.display()),
		Source::Scene { scene } => format!("scene: {scene}"),
		Source::Layer { broadcast, source } => format!("{source}, published in {broadcast}"),
		Source::Program => "program".into(),
	}
}

fn status(ui: &mut Ui, status: &Status) {
	match status {
		Status::Starting => ui.weak("starting"),
		Status::Live => ui.colored_label(kit::OK, "live"),
		Status::Offline => ui.colored_label(kit::CUE, "offline"),
		Status::Ended => ui.colored_label(kit::CUE, "ended"),
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
	/// Whether a media file loops, and whether it or an overlay starts over
	/// each time it goes on program.
	looping: bool,
	restart: bool,
	/// The scene a scene source draws, or `None` for the program.
	scene: Option<String>,
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
			looping: false,
			restart: true,
			scene: None,
		}
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
	#[default]
	Color,
	Image,
	Media,
	Broadcast,
	Camera,
	Display,
	Window,
	PickedDisplay,
	PickedWindow,
	Microphone,
	SystemAudio,
	Browser,
	Overlay,
	Replay,
	Lut,
	Scene,
}

impl Kind {
	const ALL: [Self; 16] = [
		Self::Color,
		Self::Image,
		Self::Media,
		Self::Broadcast,
		Self::Camera,
		Self::Display,
		Self::Window,
		Self::PickedDisplay,
		Self::PickedWindow,
		Self::Microphone,
		Self::SystemAudio,
		Self::Browser,
		Self::Overlay,
		Self::Replay,
		Self::Lut,
		Self::Scene,
	];

	fn label(self) -> &'static str {
		match self {
			Self::Color => "Color",
			Self::Image => "Image",
			Self::Media => "Media file",
			Self::Broadcast => "Broadcast",
			Self::Camera => "Camera",
			Self::Display => "Display",
			Self::Window => "Window",
			Self::PickedDisplay => "Display (system picker)",
			Self::PickedWindow => "Window (system picker)",
			Self::Microphone => "Microphone",
			Self::SystemAudio => "System audio",
			Self::Browser => "Browser",
			Self::Overlay => "Overlay",
			Self::Replay => "Replay",
			Self::Lut => "LUT",
			Self::Scene => "Scene",
		}
	}

	/// What the value field holds, if this kind takes one.
	fn value(self) -> Option<&'static str> {
		match self {
			Self::Color | Self::SystemAudio | Self::Replay | Self::Scene | Self::PickedDisplay | Self::PickedWindow => {
				None
			}
			Self::Image | Self::Media => Some("file"),
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
	/// Draw the form, offering `scenes` to a scene source; the source to add
	/// once the user asks for it.
	fn ui(&mut self, ui: &mut Ui, scenes: &[String]) -> Option<Source> {
		let mut add = false;
		egui::Grid::new("add source").show(ui, |ui| {
			ui.label("Name");
			ui.text_edit_singleline(&mut self.name);
			ui.end_row();
			ui.label("Kind");
			egui::ComboBox::from_id_salt("kind")
				.selected_text(self.kind.label())
				.show_ui(ui, |ui| {
					// Only Linux has the system picker.
					let picker = |kind: &Kind| {
						cfg!(target_os = "linux") || !matches!(kind, Kind::PickedDisplay | Kind::PickedWindow)
					};
					for kind in Kind::ALL.into_iter().filter(picker) {
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
			if self.kind == Kind::Scene {
				// A pick whose scene was removed or renamed away names no scene.
				if self.scene.as_ref().is_some_and(|scene| !scenes.contains(scene)) {
					self.scene = None;
				}
				ui.label("Draws");
				egui::ComboBox::from_id_salt("scene")
					.selected_text(self.scene.as_deref().unwrap_or("Program"))
					.show_ui(ui, |ui| {
						ui.selectable_value(&mut self.scene, None, "Program")
							.on_hover_text("Whatever is on air, transitions included");
						for scene in scenes {
							ui.selectable_value(&mut self.scene, Some(scene.clone()), scene);
						}
					});
				ui.end_row();
			}
			if self.kind == Kind::Overlay {
				ui.label("Animation");
				ui.add(kit::Toggle::new(&mut self.restart).label("Restart on program"))
					.on_hover_text(
						"Start its animations over each time its scene goes on program, rather than run on, as a clock should",
					);
				ui.end_row();
			}
			if self.kind == Kind::Media {
				ui.label("Playback");
				ui.horizontal(|ui| {
					ui.add(kit::Toggle::new(&mut self.looping).label("Loop"))
						.on_hover_text("Start over at the end, rather than hold the last picture");
					ui.add(kit::Toggle::new(&mut self.restart).label("Restart on program"))
						.on_hover_text("Play from the start each time it goes on program");
				});
				ui.end_row();
			}
			add = ui.add(kit::button(Icon::Add, "Add source")).clicked();
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
			Kind::Media => Source::Media {
				path: value.into(),
				looping: self.looping,
				restart: self.restart,
			},
			Kind::Lut => Source::Lut { path: value.into() },
			Kind::Broadcast => Source::Broadcast { path: value },
			Kind::Camera => Source::Camera { id },
			Kind::Display => Source::Display { id },
			Kind::Window => Source::Window { id: value },
			Kind::PickedDisplay => Source::PickedDisplay,
			Kind::PickedWindow => Source::PickedWindow,
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
				restart: self.restart,
			},
			Kind::Replay => Source::Replay,
			Kind::Scene => match &self.scene {
				Some(scene) => Source::Scene { scene: scene.clone() },
				None => Source::Program,
			},
		})
	}
}

/// The audio outputs Listen can play on, listed once the Audio panel is shown
/// and again on Refresh.
#[derive(Default)]
struct Speakers {
	listing: Option<Result<Vec<mix::Speaker>, String>>,
	pending: Option<std::sync::mpsc::Receiver<Result<Vec<mix::Speaker>, String>>>,
}

impl Speakers {
	/// Pick the output `speaker` names, or `None` for the system default.
	/// Returns whether the pick changed.
	fn ui(&mut self, ui: &mut Ui, runtime: &tokio::runtime::Handle, speaker: &mut Option<String>) -> bool {
		if let Some(listing) = self.pending.as_ref().and_then(|pending| pending.try_recv().ok()) {
			self.listing = Some(listing);
			self.pending = None;
		}
		let mut refresh = self.listing.is_none() && self.pending.is_none();
		let before = speaker.clone();
		ui.horizontal(|ui| {
			ui.label("Listen on");
			let speakers = self.listing.as_ref().and_then(|listing| listing.as_ref().ok());
			let named = |id: &str| {
				let found = speakers.and_then(|speakers| speakers.iter().find(|speaker| speaker.id == id));
				found.map_or_else(|| format!("{id} (not found)"), |speaker| speaker.name.clone())
			};
			let shown = speaker.as_deref().map_or_else(|| "System default".to_string(), named);
			egui::ComboBox::from_id_salt("speaker")
				.selected_text(shown)
				.show_ui(ui, |ui| {
					ui.selectable_value(speaker, None, "System default");
					for found in speakers.into_iter().flatten() {
						ui.selectable_value(speaker, Some(found.id.clone()), &found.name);
					}
				});
			if self.pending.is_some() {
				ui.spinner();
			} else if ui.small_button("Refresh").clicked() {
				refresh = true;
			}
		});
		if let Some(Err(err)) = &self.listing {
			ui.colored_label(ui.visuals().error_fg_color, format!("Cannot list the outputs: {err}"));
		}
		if refresh {
			let (send, recv) = std::sync::mpsc::channel();
			runtime.spawn(async move {
				let _ = send.send(mix::speakers().await);
			});
			self.pending = Some(recv);
		}
		*speaker != before
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

	/// Draw the listing, leaving out the cameras `own` holds the ids of, the
	/// studio's own virtual cameras, so what one shows is not fed back into
	/// itself; the device to add once the user picks one.
	fn ui(&mut self, ui: &mut Ui, own: &[String]) -> Option<Pick> {
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
								if let Source::Camera { id: Some(id) } = &device.source
									&& own.contains(id)
								{
									continue;
								}
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

#[cfg(test)]
mod tests {
	use super::*;

	/// A selection follows its layer as another moves past it in the list.
	#[test]
	fn a_reorder_moves_the_layers_between() {
		let moved = |from, to| (0..5).map(|at| build::reordered(at, from, to)).collect::<Vec<_>>();
		assert_eq!(moved(1, 3), [0, 3, 1, 2, 4], "to the front, the ones passed step back");
		assert_eq!(
			moved(3, 1),
			[0, 2, 3, 1, 4],
			"to the back, the ones passed step forward"
		);
		assert_eq!(moved(2, 2), [0, 1, 2, 3, 4]);
		// Each layer lands on a place of its own.
		for (from, to) in [(0, 4), (4, 0), (2, 3)] {
			let mut places = moved(from, to);
			places.sort();
			assert_eq!(places, [0, 1, 2, 3, 4]);
		}
	}

	/// A key bound to Cut or Transition, which the window no longer offers,
	/// is kept across a save for when they come back, unless the operator
	/// bound that key to something else meanwhile.
	#[test]
	fn a_hidden_action_keeps_its_key() {
		let key = |code| hotkeys::Binding {
			modifiers: egui::Modifiers::NONE,
			key: PhysicalKey::Code(code),
			name: winit::keyboard::Key::Named(winit::keyboard::NamedKey::F1),
		};
		let saved = Bindings::from([
			(Action::Cut, key(KeyCode::F5)),
			(Action::Transition, key(KeyCode::F6)),
			(Action::Panic, key(KeyCode::F1)),
		]);
		let (mut bindings, hidden): (Bindings, Bindings) = saved.into_iter().partition(|(action, _)| action.offered());
		assert_eq!(bindings, Bindings::from([(Action::Panic, key(KeyCode::F1))]));
		bindings.insert(Action::Replay, key(KeyCode::F6));
		keep_hidden(&mut bindings, &hidden);
		let kept = Bindings::from([
			(Action::Panic, key(KeyCode::F1)),
			(Action::Replay, key(KeyCode::F6)),
			(Action::Cut, key(KeyCode::F5)),
		]);
		assert_eq!(bindings, kept, "Transition lost F6 to Replay");
	}

	/// The gain field shows a boost past its range without cutting it, and
	/// takes an exact level, so a sound can go back to exactly 0 dB.
	#[test]
	fn a_typed_gain_is_exact() {
		use egui::accesskit::Role;
		use egui_kittest::kittest::Queryable;

		let audio = moq_scene::Audio {
			gain: 10.0,
			..Default::default()
		};
		// The kit's faces take effect a frame after they are installed.
		let mut harness = egui_kittest::Harness::new_ui_state(
			|ui, (audio, gestures, ready): &mut (moq_scene::Audio, Gestures, bool)| {
				if !std::mem::replace(ready, true) {
					kit::install(ui.ctx());
					return;
				}
				if let Some((changed, ..)) = strip_controls(ui, gestures, "tone", audio) {
					*audio = changed;
				}
			},
			(audio, Gestures::default(), false),
		);
		harness.run();
		assert_eq!(harness.state().0.gain, 10.0, "showing +20 dB changed it");
		harness.get_all_by_role(Role::SpinButton).next().unwrap().click();
		harness.run();
		harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
		harness.get_all_by_role(Role::SpinButton).next().unwrap().type_text("0");
		harness.key_press(egui::Key::Enter);
		harness.run();
		assert_eq!(harness.state().0.gain, 1.0);
	}

	/// Every edit a slider drag makes, the one its release makes included,
	/// is one gesture, so it undoes as one.
	#[test]
	fn a_drag_and_its_release_are_one_gesture() {
		use egui::accesskit::Role;
		use egui_kittest::kittest::Queryable;

		// The value, the gestures of the edits made, and the gesture counter.
		let mut harness = egui_kittest::Harness::new_ui_state(
			|ui, (value, edits, gestures): &mut (f32, Vec<Option<u64>>, Gestures)| {
				let response = ui.add(kit::Slider::new(value).width(214.0));
				if let Some(gesture) = changed(&response, gestures) {
					edits.push(gesture);
				}
			},
			(0.0, Vec::new(), Gestures::default()),
		);
		let rect = harness.get_by_role(Role::Slider).rect();
		let at = |fraction| egui::pos2(egui::lerp(rect.x_range(), fraction), rect.center().y);
		harness.drag_at(at(0.75));
		harness.run();
		harness.hover_at(at(0.5));
		harness.run();
		harness.hover_at(at(0.4));
		harness.drop_at(at(0.25));
		harness.run();
		let (value, edits, _) = harness.state();
		assert!((value - 0.25).abs() < 0.05, "{value}");
		assert!(edits.len() >= 2, "{edits:?}");
		assert!(
			edits[0].is_some() && edits.iter().all(|edit| *edit == edits[0]),
			"{edits:?}"
		);
	}

	/// A clone of a source with its background removed copies its filters,
	/// and the edit adding it is valid.
	#[test]
	fn clones_a_source_with_its_background_removed() {
		let collection = Collection::from_toml(
			r#"
version = 1

[canvas]
width = 1280
height = 720
framerate = 30

[sources.cam]
kind = "camera"

[video.cam]
background = "remove"
filters = [{ kind = "opacity", amount = 0.5 }]
"#,
		)
		.unwrap();

		let (source, video) = clone_of(&collection, "cam");
		let video = video.unwrap();
		assert_eq!(video.background, Background::Keep);
		assert_eq!(video.filters, collection.video["cam"].filters);

		let mut draft = collection.draft("clone");
		draft.set_source("clone", Some(source)).unwrap();
		draft.set_video("clone", Some(video)).unwrap();
		draft.finish().unwrap();
	}
}
