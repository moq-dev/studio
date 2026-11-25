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
	Background, Blend, Canvas, Clip, Collection, Crop, Direction, Draft, Ease, Filter, Format, Item, Keyframe, Layer,
	MaskMode, Placement, Rect, Scene, Source, Thumbnail, Transition, Video,
};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::controls::{Action, Controls, Group};
use crate::devices::{self, Listing, Pick};
use crate::engine::{Engine, Lost, State};
use crate::fx::{Fx, Mark};
use crate::health::{self, Verdict};
use crate::hotkeys::{self, Bindings, Hotkeys, Reach};
use crate::interact::Interact;
use crate::kit::{self, Icon, IconButton, Tally, TallyFrame};
use crate::mods::Mod;
use crate::replay::{self, Of};
use crate::snap::{self, Grab, Snap};
use crate::{mix, record};

mod add;
mod build;
mod cue;
mod dark;
mod drop;
mod formats;
mod graph;
mod inspector;
mod keep;
mod layers;
mod layout;
mod levels;
mod live;
mod mixer;
mod mods;
mod multiview;
mod palette;
mod remotes;
mod review;
mod settings;
mod shell;
mod shows;
mod slots;
mod sound;
mod start;

#[cfg(test)]
pub(crate) use add::Tab as AddTab;
use levels::Levels;
#[cfg(test)]
pub(crate) use mixer::View as MixView;
#[cfg(test)]
pub(crate) use mods::layout_off;
pub(crate) use shell::{dashed, window_buttons};
pub use shows::Leave;

/// The smallest the window gets, at which every part still has room.
pub const MIN_SIZE: egui::Vec2 = layout::MIN;

/// The pages, each the window's middle: the title bar and the strip of
/// scenes, the panic key, and the sound are around every one. Studio is the
/// only one until a mod adds another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
	/// The live picture, edited where it shows, with its layers beside it
	/// and the selected layer's inspector.
	#[default]
	Studio,
	/// Every sound and audio scene, with its sends.
	Mix,
	/// Replays, clips, and recordings.
	Review,
}

impl Page {
	const ALL: [Self; 3] = [Self::Studio, Self::Mix, Self::Review];

	/// The mod that must be on for the page to be listed, if any.
	fn needs(self) -> Option<Mod> {
		match self {
			Self::Studio => None,
			Self::Mix => Some(Mod::Audio),
			Self::Review => Some(Mod::Review),
		}
	}

	fn name(self) -> &'static str {
		match self {
			Self::Studio => "Studio",
			Self::Mix => "Mix",
			Self::Review => "Review",
		}
	}

	fn icon(self) -> Icon {
		match self {
			Self::Studio => Icon::Live,
			Self::Mix => Icon::Mix,
			Self::Review => Icon::Review,
		}
	}
}

/// The Settings window's panes: what a broadcaster sets once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Setting {
	/// The format, the canvas size and frame rate, the transition, and each
	/// scene's size.
	#[default]
	Show,
	/// Where the show streams: its relays, their tokens, and the broadcast.
	Destinations,
	/// The folder, and what a recording holds.
	Recording,
	Hotkeys,
	/// The room guests join, its key, the join page, and how long a link lasts.
	Guests,
	/// The phones paired as remotes.
	Remote,
	/// The features most streamers never use, each switched on here.
	Mods,
}

impl Setting {
	pub(crate) const ALL: [Self; 7] = [
		Self::Show,
		Self::Destinations,
		Self::Recording,
		Self::Hotkeys,
		Self::Guests,
		Self::Remote,
		Self::Mods,
	];

	/// The mod that must be on for the pane to be listed, if any.
	pub(crate) fn needs(self) -> Option<Mod> {
		match self {
			Self::Remote => Some(Mod::Remote),
			_ => None,
		}
	}

	pub(crate) fn name(self) -> &'static str {
		match self {
			Self::Show => "Show",
			Self::Destinations => "Destinations",
			Self::Recording => "Recording",
			Self::Hotkeys => "Hotkeys",
			Self::Guests => "Guests",
			Self::Remote => "Remote",
			Self::Mods => "Mods",
		}
	}

	fn icon(self) -> Icon {
		match self {
			Self::Show => Icon::Media,
			Self::Destinations => Icon::Broadcast,
			Self::Recording => Icon::Record,
			Self::Hotkeys => Icon::Hotkeys,
			Self::Guests => Icon::Guests,
			Self::Remote => Icon::Phone,
			Self::Mods => Icon::Advanced,
		}
	}

	/// Each setting in its pane by the name its row shows, which the palette
	/// finds it by, with what else it answers to and the mod it needs, if
	/// any. The Mods pane's rows are the mods themselves.
	pub(crate) fn rows(self) -> &'static [(&'static str, &'static [&'static str], Option<Mod>)] {
		match self {
			Self::Show => &[
				("Format", &["portrait", "landscape", "vertical"], Some(Mod::Formats)),
				("Both at once", &["dual", "portrait", "phones"], Some(Mod::Formats)),
				("Canvas size", &["resolution", "1080p", "720p"], None),
				("Frame rate", &["fps"], None),
				("Transition", &["fade", "slide", "wipe", "stinger", "cut"], None),
				(settings::SCENE_SIZES, &[], Some(Mod::Published)),
			],
			Self::Destinations => &[
				("Relay", &["token", "server", "url"], None),
				("Failover relay", &["backup"], None),
				("Broadcast name", &["path"], None),
				("Encode only while someone watches", &["on demand", "viewers"], None),
				("Virtual camera", &["webcam", "video call"], None),
			],
			Self::Recording => &[
				("Each source too", &["iso", "separate", "files"], None),
				("Picture", &["video"], None),
				("Sound", &["audio"], Some(Mod::Audio)),
			],
			Self::Hotkeys => &[],
			Self::Guests => &[
				("Room", &[], None),
				("Room key", &[], None),
				("Join page", &[], None),
				("Link lifetime", &["expiry", "expires"], None),
				("Appear on their own", &["tile"], Some(Mod::Layout)),
			],
			Self::Remote => &[],
			Self::Mods => &[],
		}
	}

	/// What else the palette finds the pane by, besides its name and lead.
	fn words(self) -> &'static [&'static str] {
		match self {
			Self::Show => &["canvas"],
			Self::Destinations => &["relay", "token", "stream"],
			Self::Recording => &["folder", "record"],
			Self::Hotkeys => &["keys", "shortcuts", "bind"],
			Self::Guests => &["invite", "join", "room"],
			Self::Remote => &["phone", "pair", "qr", "control"],
			Self::Mods => &["features", "extras", "advanced", "power"],
		}
	}

	/// The line under the pane's title.
	fn lead(self) -> &'static str {
		match self {
			Self::Show => "The shape of the picture, and how scenes go on air. It is saved in the show file.",
			Self::Destinations => {
				"Where the stream goes, saved as you change it. Tokens stay in the system's credential store, never in a \
				 file."
			}
			Self::Recording => "What the record button saves, from any page.",
			Self::Hotkeys => "Kept on this computer, not in the show.",
			Self::Guests => {
				"People who join from a web page, appearing as sources. Saved as you change it, beside the show."
			}
			Self::Remote => {
				"Phones that drive the show from a page their code opens: the scenes, TAKE, and the mutes. Kept beside the room key."
			}
			Self::Mods => {
				"Features most streamers never use, hidden until switched on. Kept on this computer, not in the show."
			}
		}
	}
}

/// The Studio page's left column: the layers, or the inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Side {
	#[default]
	Layers,
	/// The inspector, a tab of its own when the window is too narrow for
	/// its column.
	Inspect,
}

impl Side {
	/// The tabs beside the inspector's own column.
	const ALL: [Self; 1] = [Self::Layers];

	fn name(self) -> &'static str {
		match self {
			Self::Layers => "Layers",
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
}

impl Studio {
	/// The studio on `engine`, streaming as `stream` sets, `framed` if its
	/// title bar is the window's frame.
	pub fn new(
		render: &eframe::egui_wgpu::RenderState,
		engine: Arc<Engine>,
		stream: crate::stream::Stream,
		runtime: tokio::runtime::Handle,
		framed: bool,
	) -> Self {
		// egui samples whatever the compositor's textures hold each frame, so
		// they are registered again only when a resize replaces them, or the
		// program starts or stops showing layers over it. The engine draws the
		// preview only while a scene is cued or open off air, which shows it.
		let (program, preview, dropped) = {
			let state = engine.state.lock().unwrap();
			let program = Registered::new(render, state.compositor.layered());
			(
				program,
				Registered::new(render, state.compositor.preview()),
				state.dropped,
			)
		};
		let review = review::Review::new(&engine, &stream.saved().earlier, &runtime);
		let controls = Arc::new(Controls::new(engine.clone(), stream, runtime.clone()));
		drop(controls.follow_relays());
		let multiview = multiview::Multiview::new(render.clone(), engine.clone(), controls.clone());
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
			preview,
			portrait: None,
			portrait_drawn: None,
			portrait_preview: None,
			format: Format::Landscape,
			editing: None,
			framed,
			page: Page::default(),
			locked: false,
			rail: true,
			sound_options: None,
			setting: None,
			details: false,
			side: Side::default(),
			inspect: Inspect::default(),
			more: false,
			inspecting: false,
			going: None,
			confirm_end: false,
			ready: None,
			tokens: 0,
			dialing: None,
			stream_form: None,
			guests_form: None,
			typing: false,
			saving: None,
			saved: None,
			picker: None,
			restart: None,
			graph_open: false,
			selection: None,
			cropping: false,
			gestures: Gestures::default(),
			snapping: Snapping::default(),
			direction: Direction::Left,
			duration: Transition::default().duration().unwrap_or_default(),
			cut: STINGER_CUT,
			drag: None,
			legend: false,
			title_needs: 0.0,
			renaming_audio: None,
			renaming_pad: None,
			speaker: None,
			speakers: mixer::Speakers::default(),
			desk: mixer::Desk::default(),
			canvas: None,
			sizes: BTreeMap::new(),
			adding: add::Adding::default(),
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
			phone: String::new(),
			paired: None,
			message: None,
			faults: Vec::new(),
			reloads: 0,
			shifts: 0,
			leaving: None,
			shows: Vec::new(),
			show_dialog: None,
			renaming_show: None,
			quick: start::QuickStart::default(),
			levels: Levels::default(),
			graph: graph::View::default(),
			review,
			multiview,
			dropping: drop::Dropping::default(),
			keyboard: None,
			palette: None,
		};
		if dropped > 0 {
			panels.report(Err(anyhow::anyhow!(dropped_message(dropped))));
		}
		Self { panels }
	}
}

impl Studio {
	/// The show's engine.
	pub fn engine(&self) -> &Arc<Engine> {
		&self.panels.engine
	}

	/// What `--control` reports of the studio: the page, what is on program
	/// and cued, the selected layer, each source's status and level, whether
	/// it is on air or recording, and the show's whole scene document.
	pub fn describe(&self) -> serde_json::Value {
		let panels = &self.panels;
		let state = panels.engine.state.lock().unwrap();
		let collection = state.collection();
		let status = |status: Status| match status {
			Status::Starting => "starting".to_string(),
			Status::Live => "live".to_string(),
			Status::Offline => "offline".to_string(),
			Status::Failed(why) => format!("failed: {why}"),
			Status::Ended => "ended".to_string(),
			other => format!("{other:?}").to_lowercase(),
		};
		let sources: BTreeMap<&str, serde_json::Value> = state
			.sources
			.iter()
			.map(|(name, feed)| {
				let peak = levels::latest(panels.levels.sound(name)).peak();
				let value = serde_json::json!({
					"status": status(feed.status()),
					"track": feed.track().map(status),
					"peak_db": (peak > 0.0).then(|| decibels(peak)),
				});
				(name, value)
			})
			.collect();
		// As a view settles: a replay source's track starts only with its
		// first replay.
		let starting: Vec<&str> = state
			.sources
			.iter()
			.filter(|(_, feed)| {
				matches!(feed.status(), Status::Starting)
					|| (!matches!(feed.source(), Source::Replay) && matches!(feed.track(), Some(Status::Starting)))
			})
			.map(|(name, _)| name)
			.collect();
		let selection = panels.selection.as_ref().and_then(|picked| {
			let item = picked.item(collection)?;
			Some(serde_json::json!({
				"scene": picked.layer.scene,
				"index": picked.index,
				"source": item.source,
			}))
		});
		let on_air = match panels.engine.on_air() {
			crate::engine::OnAir::Off => "off",
			crate::engine::OnAir::Starting => "starting",
			crate::engine::OnAir::Live(_) => "live",
			crate::engine::OnAir::Ending => "ending",
		};
		let recording = panels.engine.recorder.status().is_some_and(|status| !status.done);
		serde_json::json!({
			"page": panels.page.name(),
			"program": state.compositor.program_scene(),
			"cued": state.cued(),
			"selection": selection,
			"on_air": on_air,
			"recording": recording,
			"sources": sources,
			"starting": starting,
			"frames": panels.engine.stats.rendered.load(Ordering::Relaxed),
			"framerate": state.compositor.canvas().framerate,
			"show": collection,
		})
	}

	/// Claim the show for closing against a go-live, which a hotkey could
	/// start after an off-air check alone.
	pub fn reserve_close(&self) -> anyhow::Result<()> {
		self.panels.controls.reserve_close()
	}

	/// Save the settings edits still held, even one being typed, on the way
	/// out of the show, since Settings saves them on its own.
	pub fn flush_settings(&mut self) -> anyhow::Result<()> {
		self.panels.flush_settings()
	}

	/// Give the show back after a close that left it open.
	pub fn release_close(&self) {
		self.panels.controls.release_close();
	}

	/// Say why the show stays open.
	pub fn refuse(&mut self, err: anyhow::Error) {
		self.panels.report(Err(err));
	}

	/// Give back what the studio registered with the window: the program, the
	/// preview, and each thumbnail, whose compositor textures they would
	/// otherwise keep alive.
	pub fn release(self, render: &eframe::egui_wgpu::RenderState) {
		let panels = self.panels;
		panels.multiview.release();
		let mut renderer = render.renderer.write();
		for registered in [&panels.program, &panels.preview]
			.into_iter()
			.chain(&panels.portrait)
			.chain(&panels.portrait_preview)
			.chain(panels.thumbnails.values())
		{
			renderer.free_texture(&registered.id);
		}
	}
}

#[cfg(test)]
impl Studio {
	/// Turn to `page`, as its button in the title bar would.
	pub(crate) fn turn(&mut self, page: Page) {
		self.panels.page = page;
	}

	/// Lock the picture against edits, or unlock it, as the title bar's
	/// Editable switch would.
	pub(crate) fn lock(&mut self, locked: bool) {
		self.panels.locked = locked;
	}

	/// Open the layers' rail or close it, as the title bar's Layers switch
	/// would.
	pub(crate) fn rail(&mut self, open: bool) {
		self.panels.rail = open;
	}

	/// Open the options of the sound called `name`, as its gear would.
	pub(crate) fn sound_options(&mut self, name: &str) {
		self.panels.sound_options = Some(name.to_string());
	}

	/// Edit `format`'s picture on Studio, as its tab under the picture would.
	pub(crate) fn edit_format(&mut self, format: Format) {
		self.panels.format = format;
	}

	/// Turn to MIX in `view`, with the strip of the sound called `opened`
	/// open, as clicking its name would.
	pub(crate) fn mix(&mut self, view: mixer::View, opened: Option<&str>) {
		self.panels.page = Page::Mix;
		self.panels.desk = mixer::Desk::new(view, opened);
	}

	/// List `listing` as this machine's devices, in place of any listing
	/// under way, and keep it as Add and the palette open.
	pub(crate) fn seed_devices(&mut self, listing: Listing) {
		self.panels.devices.listing = Some(listing);
		self.panels.devices.pending = None;
		self.panels.devices.fixed = true;
	}

	/// Open Settings at `setting`, as the gear would.
	pub(crate) fn open(&mut self, setting: Setting) {
		self.panels.setting = Some(setting);
	}

	/// Draw the multiview as the window itself, in place of the studio, with
	/// the sources called `picked` on it.
	pub(crate) fn multiview_alone(&mut self, picked: &[&str]) {
		self.panels.multiview.set_alone(picked);
	}

	/// Open the scene graph's window, as Settings, Mods would.
	pub(crate) fn open_graph(&mut self) {
		self.panels.graph_open = true;
	}

	/// Open the stream's health in detail, as the health dot's popover would.
	pub(crate) fn details(&mut self) {
		self.panels.details = true;
	}

	/// Scroll Settings to the row called `name` and light it, opening what
	/// holds it, as the palette does for a setting it found.
	#[cfg(test)]
	pub(crate) fn seek(&self, ctx: &egui::Context, name: &str) {
		settings::seek(ctx, name);
	}

	/// Press GO LIVE, as its button would, with the check it waits on
	/// started over, as a change to what it checks does.
	pub(crate) fn going(&mut self) {
		self.panels.ready = None;
		self.panels.press_go_live();
	}

	/// What the buttons and hotkeys do, such as going live.
	pub(crate) fn controls(&self) -> Arc<Controls> {
		self.panels.controls.clone()
	}

	/// Show `listing` as REVIEW's cards, as if the folder held them.
	pub(crate) fn seed_shelf(&mut self, listing: crate::shelf::Listing) {
		self.panels.review.seed_shelf(listing);
	}

	/// Show `view` as REVIEW's strip, as if the window held it, still at
	/// `now`, with the stretch `picked` ago, from and to, and the head `head`
	/// ago.
	pub(crate) fn seed_past(
		&mut self,
		view: crate::past::View,
		now: kio::clock::Instant,
		picked: (Duration, Duration),
		head: Duration,
	) {
		self.panels.review.seed_past(view, now, head);
		self.panels.controls.replay.lock().unwrap().stretch =
			replay::Stretch::Between(now.saturating_sub(picked.0), now.saturating_sub(picked.1));
	}

	/// Bind `key` to `action`, as the Hotkeys pane would.
	pub(crate) fn bind(&mut self, action: &Action, key: hotkeys::Binding) {
		let hotkeys = self.panels.hotkeys.as_mut().expect("the window handed over its keys");
		hotkeys.bind(action, Some(key)).expect("a key bound to nothing else");
	}

	/// Switch the mod `which` on or off, as its switch in Settings, Mods
	/// would.
	pub(crate) fn switch(&mut self, which: Mod, on: bool) {
		self.panels.switch(which, on);
	}

	/// Turn Studio mode on or off, as its switch would.
	pub(crate) fn studio_mode(&mut self, on: bool) {
		self.panels.controls.set_studio(on);
	}

	/// Cue the scene called `scene`, as its pad would in Studio mode.
	pub(crate) fn cue(&mut self, scene: &str) {
		let mut state = self.panels.engine.state.lock().unwrap();
		state.cue(scene).expect("a scene that can go on air");
	}

	/// Open the scene called `scene` off air on Studio, as its pad's menu would.
	pub(crate) fn edit_off_air(&mut self, scene: &str) {
		let result = self.panels.open_off_air(scene);
		result.expect("a scene to open");
	}

	/// Whether a layer is selected.
	pub(crate) fn selected(&self) -> bool {
		self.panels.selection.is_some()
	}

	/// Open Add on `tab`, as `A` and a click on its card would.
	pub(crate) fn open_add(&mut self, tab: add::Tab) {
		self.panels.page = Page::Studio;
		self.panels.open_add();
		self.panels.adding.tab = tab;
	}

	/// Turn to Studio with `side` and `inspect` shown, and the layer at
	/// `index` of the scene on program selected, front first, as a click on
	/// its row would.
	pub(crate) fn build(&mut self, side: Side, inspect: Inspect, index: Option<usize>) {
		let panels = &mut self.panels;
		panels.page = Page::Studio;
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

	/// Open the inspector's More, as a click on it would.
	pub(crate) fn more(&mut self) {
		self.panels.more = true;
	}
}

/// Where this user's picks are kept, in the app's settings rather than the
/// scene file, which moves between machines without one operator's keys.
const SPEAKER: &str = "speaker";
const SNAPPING: &str = "snapping";
const HOTKEYS: &str = "hotkeys";
/// How this operator runs a show, not part of the show.
const STUDIO_MODE: &str = "studio_mode";
const LOCKED: &str = "locked";
const RAIL: &str = "layers_rail";
const MODS: &str = "mods";

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
			let mods: Vec<Mod> = eframe::get_value(storage, MODS).unwrap_or_default();
			panels.controls.mods.load(&mods);
			panels.locked = eframe::get_value(storage, LOCKED).unwrap_or_default();
			panels.rail = eframe::get_value(storage, RAIL).unwrap_or(true);
			let studio = eframe::get_value(storage, STUDIO_MODE).unwrap_or_default();
			panels.controls.set_studio(studio);
			let multiview = eframe::get_value(storage, multiview::KEY).unwrap_or_default();
			panels.multiview.load(multiview);
		}
		// The show as it opens switches on what it uses without a word.
		self.panels.follow_mods(false);
		self.panels.keyboard = Some(window.keyboard.clone());
		let hotkeys = Hotkeys::new(window, self.panels.controls.clone(), &self.panels.runtime, bindings);
		self.panels.hotkeys = Some(hotkeys);
	}

	pub fn save(&mut self, storage: &mut dyn eframe::Storage) {
		let panels = &self.panels;
		eframe::set_value(storage, SPEAKER, &panels.speaker);
		eframe::set_value(storage, SNAPPING, &panels.snapping);
		eframe::set_value(storage, STUDIO_MODE, &panels.controls.studio());
		eframe::set_value(storage, LOCKED, &panels.locked);
		eframe::set_value(storage, RAIL, &panels.rail);
		eframe::set_value(storage, MODS, &panels.controls.mods.saved());
		panels.multiview.save(storage);
		if let Some(hotkeys) = &panels.hotkeys {
			eframe::set_value(storage, HOTKEYS, &hotkeys.bindings());
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
			// No field has the keyboard while the window does not, so an
			// edit left in one saves rather than wait for the window's return.
			self.panels.typing = false;
		}
		// Before each frame, and while the window is hidden, so the keys
		// follow a keyboard layout change while a game has focus.
		if let Some(hotkeys) = &mut self.panels.hotkeys {
			hotkeys.hear();
		}
		self.panels.follow_stream_save();
		self.panels.autosave_settings(ctx);
		self.panels.follow_ready();
	}

	/// Draw the window, with `fx` drawing its effects.
	pub fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, fx: &mut Fx) {
		let panels = &mut self.panels;
		// Everything the title bar, the strip, and the live picture show,
		// under one short lock, so drawing them never holds the engine's
		// state, which the program's render thread wants each frame.
		panels.follow_mods(true);
		let snapshot = panels.snapshot(ui.ctx(), frame);
		if let Some(fired) = panels.controls.fired() {
			panels.say(fired);
		}
		if let Some(hotkeys) = &panels.hotkeys {
			hotkeys.focused(panels.interact.has_keyboard() || ui.ctx().text_edit_focused());
		}
		panels.take_binding(ui.ctx());
		panels.dropping.follow(ui.ctx(), frame);
		panels.shortcuts(ui.ctx());
		panels.drops_read();
		panels.staged = false;
		let verdict = {
			let report = panels.engine.health.lock().unwrap();
			report
				.problems
				.first()
				.map_or(Verdict::Healthy, |problem| problem.verdict)
		};
		fx.follow(panels.engine.on_air().live().is_some(), snapshot.takes, verdict);
		match panels.multiview.alone() {
			true => panels.multiview.ui(ui),
			false => {
				panels.window(ui, fx, &snapshot);
				panels.palette(ui.ctx());
				panels.drops_elsewhere(ui.ctx());
				panels.multiview.show(ui.ctx());
			}
		}
		// A stage not shown hears no input, so it lets go of the pages, as
		// turning Interact off does.
		if !panels.staged {
			panels.let_go();
			panels.close_add();
		}
		// A thumbnail nothing shows is not drawn, until it is again.
		let bindings = panels.hotkeys.as_ref().map(Hotkeys::bindings).unwrap_or_default();
		panels.multiview.share(&panels.thumbnails_shown, bindings);
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

	/// Show `texture` in `registered` from now on, while there is one,
	/// registering it when it appears and giving it back when it goes, as the
	/// portrait picture does when the show stops going out in portrait.
	pub(crate) fn follow_while(
		registered: &mut Option<Self>,
		render: &eframe::egui_wgpu::RenderState,
		texture: Option<&wgpu::Texture>,
	) {
		match (registered.as_mut(), texture) {
			(Some(shown), Some(texture)) => shown.follow(render, texture),
			(None, Some(texture)) => *registered = Some(Self::new(render, texture)),
			(Some(_), None) => {
				if let Some(gone) = registered.take() {
					render.renderer.write().free_texture(&gone.id);
				}
			}
			(None, None) => {}
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
	/// The cued scene, or the one open off air, which the compositor draws
	/// while there is one.
	preview: Registered,
	/// The portrait picture as viewers see it, while the show goes out in
	/// portrait.
	portrait: Option<Registered>,
	/// Its size this frame, as its texture was drawn, while there is one.
	portrait_drawn: Option<Canvas>,
	/// The preview in portrait, while the show goes out in portrait.
	portrait_preview: Option<Registered>,
	/// The picture Studio edits a scene drawn in both in.
	format: Format,
	/// The scene open for editing off air on Studio, while it is the one cued.
	editing: Option<String>,
	/// Whether the title bar is the window's frame, which moves, resizes,
	/// and closes it.
	framed: bool,
	/// The page the window shows.
	page: Page,
	/// Whether the Studio page's picture is locked against edits, as during
	/// a broadcast where a stray drag would move a layer on air.
	locked: bool,
	/// Whether the Studio page's left rail, the layers and Add, is open.
	rail: bool,
	/// The sound whose options popover is open in the strip, by name.
	sound_options: Option<String>,
	/// The Settings window's tab, while it is open.
	setting: Option<Setting>,
	/// Whether the stream's health shows in detail.
	details: bool,
	/// The Studio page's left column and inspector tabs.
	side: Side,
	inspect: Inspect,
	/// Whether the inspector's More is open, for every layer alike.
	more: bool,
	/// Whether the Studio page draws the inspector, which a drag leaves as it
	/// found it, as it does the legend.
	inspecting: bool,
	/// The Go live popover, while open.
	going: Option<live::Going>,
	/// Whether the clock's End stream popover opens on the next frame, as the
	/// palette asks.
	confirm_end: bool,
	/// The check going live waits on, kept fresh in the background off air.
	ready: Option<live::Ready>,
	/// How many saves set a relay's token, which a check made before is
	/// stale after.
	tokens: u64,
	/// Since when the preferred relay has been on its first dial, which the
	/// check waits out a while.
	dialing: Option<kio::clock::Instant>,
	/// The Destinations pane's form, holding edits until they save.
	stream_form: Option<live::Form>,
	/// The Guests pane's form, holding edits until they save.
	guests_form: Option<settings::GuestForm>,
	/// Whether a field of the Settings pane shown had the keyboard last
	/// frame, which holds its edits from saving.
	typing: bool,
	/// A save of the stream file under way.
	saving: Option<live::Saving>,
	/// How the last save of the stream file went, for the pane that asked.
	saved: Option<(Setting, String)>,
	/// A system file dialog Settings opened, until it closes.
	picker: Option<settings::Picker>,
	/// A change that restarts encoders, waiting for a yes while on air.
	restart: Option<settings::Restart>,
	/// Whether the scene graph's window is open.
	graph_open: bool,
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
	/// The layer a drag on the Studio page's picture holds.
	drag: Option<Drag>,
	/// Whether the Studio page's picture leaves room for the keep-clear
	/// legend under it, which a drag leaves as it found it, so the picture
	/// under the pointer neither moves nor resizes mid-gesture.
	legend: bool,
	/// The layer row being dragged to a new place, by its index.
	moving: Option<usize>,
	/// The width the title bar's full controls took when last drawn, past
	/// which it draws them compact.
	title_needs: f32,
	/// The audio scene its strip's sheet renames, by name, and the name typed
	/// so far.
	renaming_audio: Option<(String, String)>,
	/// The scene its pad's sheet renames, by name, and the name typed so far.
	renaming_pad: Option<(String, String)>,
	/// The output Listen plays on, by [`mix::Speaker::id`], or `None` for the
	/// system default. Saved per user, not in the shared scene file.
	speaker: Option<String>,
	speakers: mixer::Speakers,
	/// The MIX page's view and the strip open on it.
	desk: mixer::Desk,
	/// The custom canvas the Show pane holds until it is applied, or `None`
	/// while it shows the collection's by name.
	canvas: Option<Canvas>,
	/// The size each scene's form holds until it is applied, by name.
	sizes: BTreeMap<String, (u32, u32)>,
	/// Add, over the stage while open.
	adding: add::Adding,
	devices: Devices,
	/// Whether the Studio page's picture passes the mouse and keyboard to the
	/// selected layer's web page, until Escape.
	interacting: bool,
	interact: Interact,
	/// Whether this frame drew the Studio page's picture, which hears input.
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
	/// The name typed for the next remote.
	phone: String,
	/// The remote's link on screen.
	paired: Option<Shown>,
	/// The outcome of the last action that happened nowhere in particular,
	/// and when, shown for a while.
	message: Option<(Result<String, String>, kio::clock::Instant)>,
	/// Failures shown where they happened, newest last.
	faults: Vec<Fault>,
	/// The engine's reloads seen so far.
	reloads: u64,
	/// The engine's [shifts](engine::State::shifts) seen so far.
	shifts: u64,
	/// What the Show menu asked the window for, until the window takes it.
	leaving: Option<Leave>,
	/// The other shows the Show menu lists.
	shows: Vec<crate::recent::Show>,
	/// The Show menu's file dialog, while open.
	show_dialog: Option<shows::Dialog>,
	/// The show's name and the one typed so far, while renaming it.
	renaming_show: Option<(String, String)>,
	/// The quick start's state, while the picture is empty.
	quick: start::QuickStart,
	/// Each sound's and audio scene's recent levels, for the meters.
	levels: Levels,
	graph: graph::View,
	/// The REVIEW page's strip, cards, and what is dragged on it.
	review: review::Review,
	/// The multiview's window, and this user's picks for it.
	multiview: multiview::Multiview,
	/// The files and pastes landing on the picture.
	dropping: drop::Dropping,
	/// The keys the window hears before egui, which say when a paste was
	/// pressed, once the window has handed them over.
	keyboard: Option<hotkeys::Keyboard>,
	/// The command palette, while open.
	palette: Option<palette::Palette>,
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

	/// The layer at rest in `format`'s picture, at its place there, if it is
	/// still there: in the landscape one for a layer drawn in one.
	fn rest(&self, collection: &Collection, format: Format) -> Option<Item> {
		let format = collection.format_of(&self.layer, format);
		Some(collection.portrayed(self.item(collection)?, format))
	}

	/// The layer placed at `rect` with `crop` in `format`'s picture, as it
	/// is in the other, if it is still there and shown in that picture.
	fn placed(&self, collection: &Collection, format: Format, rect: Rect, crop: Crop) -> Option<Item> {
		let format = collection.format_of(&self.layer, format);
		let mut item = self.item(collection)?.clone();
		// Not shown there, it has no place there.
		if !item.shown_in(format) {
			return None;
		}
		// Set free and drawn in both without a portrait place, it is fitted
		// there from its landscape one, so it keeps where it was fitted.
		let follows = format == Format::Landscape && collection.dual(&self.layer) && item.slot.is_none();
		if follows && item.portrait.is_none() && item.shown_in(Format::Portrait) {
			item.portrait = Some(collection.placement(&item, Format::Portrait));
		}
		Some(item.placed(format, Placement { rect, crop }))
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
	/// Add, over the stage.
	Add,
	/// The picture, where the last drop or paste landed.
	Dropped,
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
			Self::Source(name) => {
				// Not part of the look, so it stays as it was.
				let expect_dark = draft
					.collection()
					.video
					.get(name)
					.is_some_and(|video| video.expect_dark);
				draft.set_video(
					name,
					Some(Video {
						background: background.unwrap_or_default(),
						crop,
						filters,
						expect_dark,
					}),
				)
			}

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

/// Put `item`, with what it shows, on the clipboard; what to say.
fn clip(ctx: &egui::Context, state: &State, item: &Item) -> anyhow::Result<String> {
	let dir = &state.sources.context().dir;
	let text = Clip::items(state.collection(), vec![item.clone()], dir).to_toml()?;
	ctx.copy_text(text);
	Ok(format!("copied `{}`", item.source))
}

fn dropped_message(dropped: usize) -> String {
	format!("{dropped} history entries no longer matched the scene file and were dropped")
}

impl Panels {
	/// The keyboard shortcuts, and the clipboard.
	fn shortcuts(&mut self, ctx: &egui::Context) {
		// From anywhere, a text field included, and while the palette's own
		// search box has the keyboard, where it closes it again.
		if ctx.input_mut(|input| input.consume_shortcut(&palette::OPEN)) {
			self.toggle_palette();
		}
		// egui pastes only text, so a paste it does not report may be a picture;
		// one asked of the window waits for its text as a menu's Paste does.
		let pasted = self.keyboard.as_ref().and_then(hotkeys::Keyboard::pasted);
		if pasted == Some(hotkeys::Pasted::Asked) {
			self.dropping.paste_asked(ctx.pointer_latest_pos());
		}
		let pressed = pasted == Some(hotkeys::Pasted::Read);
		let text = ctx.input(|input| input.events.iter().any(|event| matches!(event, egui::Event::Paste(_))));
		let menu = self.dropping.menu_pasted(ctx, text);
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
		// At the pointer, or where a menu's Paste lands.
		let at = match menu {
			Some((at, _)) => at,
			None => ctx.pointer_latest_pos(),
		};
		let other = menu.is_some_and(|(_, other)| other);
		match paste {
			Some(text) => self.paste(&text, at),
			None if pressed || other => self.dropping.paste_clipboard(at),
			None => {}
		}
	}

	/// Report a reload, from File > Reload or an outside edit the autosave
	/// took, and let go of a selected item, a drag, and a scene rename under
	/// way, whose index it may have given to another. Called under every lock
	/// that uses one, since the autosave can reload between two.
	///
	/// Let go of a layer of a scene Studio no longer edits too, which a pad,
	/// a hotkey, or opening a scene off air took away from it, so a paste,
	/// the inspector, and the picture never act on a scene the picture does
	/// not show. Off air, that includes the always-on-top layers, which are
	/// drawn on program.
	fn follow_reloads(&mut self, state: &State) {
		let gone = self.selection.as_ref().is_some_and(|picked| match &picked.layer.scene {
			Some(scene) => self.edited(state) != Some(scene.as_str()),
			None => self.off_air(state).is_some(),
		});
		// A layer of its scene that an edit of the engine's own, such as a
		// guest appearing, inserted or replaced may have taken its index.
		let shifted = shifted_since(self.selection.as_ref(), &state.shifts, self.shifts);
		self.shifts = state.shifts.0;
		if gone || shifted {
			self.selection = None;
			self.drag = None;
			self.moving = None;
			self.cropping = false;
		}
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
	/// let go of any drag and crop under way.
	fn deselect_item(&mut self) {
		self.selection = None;
		self.drag = None;
		self.moving = None;
		self.cropping = false;
	}

	/// Select `picked`, leaving cropping and interacting with the last one.
	fn select(&mut self, picked: Option<Picked>) {
		if self.selection != picked {
			self.cropping = false;
		}
		self.selection = picked;
	}

	/// The layers Studio edits, and new sources go in: the scene open off
	/// air's, or else the scene on program's. The always-on-top layers over
	/// the program are [`Layer::top`].
	fn scene(&self, state: &State) -> Result<Layer, String> {
		match self.edited(state) {
			Some(name) => Ok(Layer::scene(name)),
			None => Err("Take a scene to edit what it shows.".into()),
		}
	}

	/// The scene open for editing off air, if one is.
	fn off_air<'a>(&self, state: &'a State) -> Option<&'a str> {
		state.cued().filter(|cued| self.editing.as_deref() == Some(*cued))
	}

	/// The scene Studio edits: the one open off air, or else the one on
	/// program.
	fn edited<'a>(&self, state: &'a State) -> Option<&'a str> {
		self.edited_since(state).0
	}

	/// The scene Studio edits, and how long it had been on as of the last
	/// render, the moment of its keyframes the picture shows.
	fn edited_since<'a>(&self, state: &'a State) -> (Option<&'a str>, Duration) {
		match self.off_air(state) {
			Some(cued) => (Some(cued), state.compositor.preview_since()),
			None => (state.compositor.program_scene(), state.compositor.program_since()),
		}
	}

	/// Open the scene called `scene` for editing off air, on Studio.
	fn open_off_air(&mut self, scene: &str) -> anyhow::Result<()> {
		let mut state = self.engine.state.lock().unwrap();
		let scene = state.open_off_air(scene)?;
		self.editing = state.cued().is_some().then_some(scene);
		self.page = Page::Studio;
		Ok(())
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
		let result = clip(ctx, &state, &item);
		drop(state);
		self.report(result);
	}

	/// Paste layers into the selected layer's list, or the scene on program,
	/// or filters onto the selected layer; or files or a web address at `at`
	/// on screen, as if dropped there. Other text may name what the clipboard
	/// holds beside it, as macOS Finder names copied files, so the clipboard's
	/// files or picture are pasted instead.
	fn paste(&mut self, text: &str, at: Option<egui::Pos2>) {
		if let Some(arrivals) = drop::Arrival::of_text(text) {
			for arrival in arrivals {
				self.dropping.arrive(arrival, at);
			}
			return;
		}
		let Ok(clip) = Clip::from_toml(text) else {
			self.dropping.paste_clipboard(at);
			return;
		};
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		// A failure with no layer to show it on is the toast's.
		let mut spot = None;
		let result = (|| match clip {
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
				spot = Some(Spot::Layer(picked.layer.clone(), item.source.clone()));
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
		})();
		drop(state);
		match spot {
			Some(spot) => self.fail(spot, result),
			None => self.check(result),
		}
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
			let field = ui
				.add(kit::Number::new(gain_field(&mut typed)).unit("dB").width(84.0))
				.on_hover_text("Gain in decibels: drag, or type a level such as 0");
			(slid, field)
		})
		.inner;
	for (response, db) in [(slid, MUTE_DB + at * (12.0 - MUTE_DB)), (field, typed)] {
		let gesture = gestures.of(&response);
		if response.changed() {
			let audio = moq_scene::Audio {
				gain: gain(db),
				..audio.clone()
			};
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

/// What a scene's settings offer beside Safe, by the mods on: `None` without
/// Published scenes, or its Publish switch, with the audio scenes and the one
/// on program to pick what it carries from while Audio scenes & routes is on.
type Publish<'a> = Option<Option<(&'a [moq_scene::AudioScene], &'a str)>>;

/// A scene's Publish setting, and the audio scene its broadcast carries
/// while published, as its Scenes panel row and Graph node show them.
fn scene_settings(ui: &mut Ui, index: usize, scene: &Scene, publish: Publish<'_>) -> Option<SceneAction> {
	let mut action = None;
	let mut safe = scene.safe;
	if ui
		.add(kit::Toggle::new(&mut safe).label("Safe"))
		.on_hover_text("The scene the panic key cuts to, as it mutes every microphone and guest")
		.changed()
	{
		action = Some(SceneAction::Safe(index, safe));
	}
	let Some(audio) = publish else { return action };
	let mut published = scene.published;
	if ui
		.add(kit::Toggle::new(&mut published).label("Publish"))
		.on_hover_text("Live as its own broadcast beneath the program's, at its own size")
		.changed()
	{
		action = Some(SceneAction::Publish(index, published));
	}
	if scene.published
		&& let Some((audio_scenes, program_audio)) = audio
	{
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

/// One change to how viewers may treat a published source.
#[derive(Clone, Copy, Debug)]
enum Viewers {
	Toggle(bool),
	Priority(i8),
}

impl Viewers {
	/// Apply it to the source called `name` as `draft` holds it now, changing
	/// only its own field, so an edit made since the controls drew is kept.
	fn apply(self, draft: &mut Draft, name: &str) -> Result<(), moq_scene::Error> {
		let mut published = draft
			.collection()
			.published
			.get(name)
			.copied()
			.ok_or_else(|| moq_scene::Error::Conflict(format!("`{name}` is no longer published")))?;
		match self {
			Self::Toggle(toggle) => published.toggle = toggle,
			Self::Priority(priority) => published.priority = priority,
		}
		draft.set_published(name, Some(published))
	}
}

/// Whether viewers may switch off the source called `name`, published
/// independently as `viewers` says, and its priority for them; what an edit
/// asked, its label, and its gesture.
fn viewers_ui(
	ui: &mut Ui,
	name: &str,
	viewers: moq_scene::Published,
	gestures: &mut Gestures,
) -> Option<(Viewers, String, Option<u64>)> {
	let mut asked = None;
	let mut toggle = viewers.toggle;
	if ui
		.add(kit::Toggle::new(&mut toggle).label("Viewers may switch it off"))
		.on_hover_text(
			"Let each viewer switch it off in their player, which then stops fetching it, \
			 such as a facecam or a screen share they can do without.",
		)
		.changed()
	{
		let label = match toggle {
			true => format!("Let viewers switch `{name}` off"),
			false => format!("Keep `{name}` on for viewers"),
		};
		asked = Some((Viewers::Toggle(toggle), label, None));
	}
	ui.horizontal(|ui| {
		ui.label("Priority");
		let mut priority = viewers.priority;
		let response = ui
			.add(egui::DragValue::new(&mut priority).range(i8::MIN..=i8::MAX))
			.on_hover_text(
				"How far ahead of the program viewers fetch it when their connection cannot \
				 carry everything: 0 ranks it with the program, below 0 it degrades first, \
				 above 0 last.",
			);
		if let Some(gesture) = changed(&response, gestures) {
			asked = Some((Viewers::Priority(priority), format!("Set `{name}`'s priority"), gesture));
		}
	});
	asked
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

/// Whether an edit the UI did not make inserted or replaced a layer of the
/// scene `selection` is in after the UI saw `seen` of the engine's
/// [shifts](engine::State::shifts).
fn shifted_since(selection: Option<&Picked>, shifts: &(u64, BTreeMap<String, u64>), seen: u64) -> bool {
	let (count, scenes) = shifts;
	let scene = selection.and_then(|picked| picked.layer.scene.as_ref());
	*count != seen && scene.is_some_and(|scene| scenes.get(scene).is_some_and(|at| *at > seen))
}

/// The source and video settings of a clone of `name`. A clone of a clone
/// shows the same capture, so it clones that. It copies the crop and filters,
/// but not a removed background: the clone already shows the cut-out. For a
/// role, it copies the source filling it, with that look.
fn clone_of(collection: &Collection, name: &str) -> (Source, Option<Video>) {
	let source = Source::Clone {
		source: collection.capture(name).to_string(),
	};
	let video = collection.video.get(collection.resolve(name)).map(|video| Video {
		background: Background::Keep,
		..video.clone()
	});
	(source, video)
}

/// A source to add: its name, what it is, and the layers it joins if it
/// has a picture, centered on `at` in canvas pixels, or where
/// [`add::place`] puts it.
struct New {
	name: String,
	source: Source,
	layer: Option<Layer>,
	at: Option<[f32; 2]>,
}

/// Add `new`, placed among its layers if it has a picture and heard in the
/// audio scene on program if it has sound. Returns its name.
fn add(state: &mut State, new: New) -> anyhow::Result<String> {
	let New {
		name,
		source,
		layer,
		at,
	} = new;
	anyhow::ensure!(!name.is_empty(), "a source needs a name");
	anyhow::ensure!(
		!state.collection().sources.contains_key(&name),
		"there is already a source named `{name}`"
	);
	let heard = source.has_audio();
	let placed = layer.filter(|_| source.has_video()).map(|layer| {
		let collection = state.collection();
		let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
		let (index, rect) = add::place(collection, &layer, canvas, &source, at);
		(layer, index, rect)
	});
	edit(state, format!("Add `{name}`"), None, |draft| {
		draft.set_source(&name, Some(source))?;
		if heard {
			draft.hear(&name)?;
		}
		if let Some((layer, index, rect)) = placed {
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
	/// The room it joins. Weak, so a room the studio left is never kept, and
	/// another never takes its place in [`Self::joins`].
	room: std::sync::Weak<crate::guests::Room>,
}

impl Shown {
	/// Whether it joins `room`, rather than one the studio has left.
	fn joins(&self, room: &Arc<crate::guests::Room>) -> bool {
		std::ptr::eq(self.room.as_ptr(), Arc::as_ptr(room))
	}
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

/// A layer a drag on the Studio page's picture holds.
struct Drag {
	picked: Picked,
	grab: Grab,
	/// Whether the layer is in a slot, so the drag moves it to another
	/// slot rather than where the pointer lets go.
	slotted: bool,
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

/// Where the visible layers of `scene` and the always-on-top ones over it,
/// if they are drawn over that scene, show `since` the scene went on, other
/// than `dragged`: what a dragged layer snaps to. A scene open off air still
/// snaps to the always-on-top ones: its picture draws them, as the program
/// will, though Studio leaves them to the program to edit.
fn others(state: &State, scene: Option<&str>, dragged: &Picked, since: Duration, format: Format) -> Vec<Rect> {
	let collection = state.collection();
	// The top layer goes over a scene of the program's size, as the
	// compositor draws it.
	let top = collection
		.canvas(scene)
		.is_none_or(|canvas| canvas.size() == collection.canvas.size());
	let mut rects = Vec::new();
	let layers = scene.map(Layer::scene).into_iter().chain(top.then(Layer::top));
	for shown in layers {
		let items = slots::placed(state, &shown, format);
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
				// Painted, not a widget: the picture takes the drag. Named in the
				// tree so a screen reader, or `--control`, can find it.
				let side = |at: f32, low: f32, high: f32, names: [&'static str; 2]| match at {
					_ if at == low => Some(names[0]),
					_ if at == high => Some(names[1]),
					_ => None,
				};
				let place = [
					side(y, rect.top(), rect.bottom(), ["top", "bottom"]),
					side(x, rect.left(), rect.right(), ["left", "right"]),
				];
				let place: Vec<&str> = place.into_iter().flatten().collect();
				let verb = if cropping { "Crop" } else { "Resize" };
				let name = format!("{verb} the layer from the {}", place.join(" "));
				painter
					.ctx()
					.accesskit_node_builder(egui::Id::new(("handle", verb, &name)), |node| {
						node.set_label(name.as_str());
						node.set_bounds(egui::accesskit::Rect {
							x0: handle.min.x.into(),
							y0: handle.min.y.into(),
							x1: handle.max.x.into(),
							y1: handle.max.y.into(),
						});
					});
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
		Source::Role { source: Some(source) } => format!("role: {source}"),
		Source::Role { source: None } => "role: unfilled".into(),
		Source::Replay => "replay".into(),
		Source::Lut { path } => format!("LUT: {}", path.display()),
		Source::Scene { scene } => format!("scene: {scene}"),
		Source::Layer { path, .. } => format!("composed: {path}"),
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

/// Where every page's meters turn from healthy to hot, in dB below full
/// scale: broadcast's usual alignment level.
const HOT_DB: f32 = -18.0;

fn decibels(gain: f32) -> f32 {
	if gain <= 0.0 {
		MUTE_DB
	} else {
		(20.0 * gain.log10()).max(MUTE_DB)
	}
}

/// The linear gain of `db` decibels, silence at or below [`MUTE_DB`].
fn gain(db: f32) -> f32 {
	if db <= MUTE_DB { 0.0 } else { 10f32.powf(db / 20.0) }
}

/// A field for a gain in decibels, `-inf` at [`MUTE_DB`], that drags or
/// takes an exact level typed.
fn gain_field(db: &mut f32) -> egui::DragValue<'_> {
	egui::DragValue::new(db)
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
		})
}

/// The capture devices this machine has, listed in the background.
#[derive(Default)]
struct Devices {
	listing: Option<Listing>,
	pending: Option<std::sync::mpsc::Receiver<Listing>>,
	/// A listing a test seeded, which [`Self::renew`] keeps.
	fixed: bool,
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

	/// List afresh unless a listing is under way, reporting why one cannot
	/// start as the failure of every kind.
	fn renew(&mut self, runtime: &tokio::runtime::Handle) {
		if self.pending.is_some() || self.fixed {
			return;
		}
		if let Err(err) = self.refresh(runtime) {
			let failed = || Err(format!("cannot list devices: {err}"));
			self.listing = Some(Listing {
				cameras: failed(),
				displays: failed(),
				windows: failed(),
				audio: failed(),
			});
		}
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

	/// A viewer control changes only its own field of the source as the
	/// scene holds it now, so an edit since the controls drew is kept, and
	/// refuses a source no longer published rather than publish it again.
	#[test]
	fn a_viewer_edit_keeps_what_changed_since() {
		let text = r#"
version = 1

[canvas]
width = 1280
height = 720
framerate = 30

[sources.cam]
kind = "camera"

[published.cam]
toggle = true
"#;
		let collection = Collection::from_toml(text).unwrap();
		let mut draft = collection.draft("priority");
		Viewers::Priority(-5).apply(&mut draft, "cam").unwrap();
		let published = moq_scene::Published {
			priority: -5,
			toggle: true,
		};
		assert_eq!(draft.collection().published["cam"], published, "the toggle was lost");

		let mut draft = collection.draft("unpublish");
		draft.set_published("cam", None).unwrap();
		let err = Viewers::Toggle(false).apply(&mut draft, "cam").unwrap_err();
		assert!(matches!(err, moq_scene::Error::Conflict(_)), "{err}");
		assert!(draft.collection().published.is_empty(), "it was published again");
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

	/// While the show goes out in both, placing a layer in the landscape
	/// picture leaves where the portrait picture draws it, even fitted there
	/// without a place of its own; a layer shown only in landscape takes no
	/// portrait place.
	#[test]
	fn a_landscape_edit_leaves_the_portrait_picture_alone() {
		let mut collection = Collection::new();
		collection.portrait = true;
		let color = moq_scene::Rgba([255, 255, 255, 255]);
		collection.sources.insert("logo".into(), Source::Color { color });
		let rect = |x| Rect {
			x,
			y: 40.0,
			width: 320.0,
			height: 180.0,
		};
		collection.scenes[0].items = vec![
			Item::new("logo", rect(40.0)),
			Item {
				format: Some(Format::Landscape),
				..Item::new("logo", rect(40.0))
			},
		];
		collection.validate().unwrap();
		let picked = |index| Picked {
			layer: Layer::scene("Scene"),
			index,
		};
		let fitted = collection.placement(&collection.scenes[0].items[0], Format::Portrait);
		let moved = picked(0).placed(&collection, Format::Landscape, rect(400.0), Crop::default());
		let moved = moved.unwrap();
		assert_eq!(moved.rect, rect(400.0));
		assert_eq!(moved.portrait, Some(fitted), "kept where it was fitted");
		let only = picked(1).placed(&collection, Format::Landscape, rect(400.0), Crop::default());
		assert_eq!(only.unwrap().portrait, None);

		// Placed in portrait, its landscape place stays.
		let tall = picked(0).placed(&collection, Format::Portrait, rect(0.0), Crop::default());
		let tall = tall.unwrap();
		assert_eq!(
			(tall.rect, tall.portrait.map(|placed| placed.rect)),
			(rect(40.0), Some(rect(0.0)))
		);
	}

	/// A scene duplicated with its own copies keeps an unfilled role as the
	/// role, as it does a scene drawn in it, and copies a filled one.
	#[test]
	fn an_unfilled_role_gets_no_copy_of_its_own() {
		let mut collection = Collection::new();
		let color = moq_scene::Rgba([255, 255, 255, 255]);
		collection.sources.insert("game".into(), Source::Color { color });
		collection.sources.insert("Main".into(), Source::Role { source: None });
		assert!(!build::own_copy(&collection, "Main"), "nothing to copy");
		assert!(build::own_copy(&collection, "game"));
		let filled = Source::Role {
			source: Some("game".into()),
		};
		collection.sources.insert("Main".into(), filled);
		assert!(build::own_copy(&collection, "Main"), "the source filling it");
	}

	/// A duplicated scene is laid out as the original in both formats, but is
	/// none of the scenes there is one of at most.
	#[test]
	fn a_duplicated_scene_keeps_its_portrait_arrangement() {
		let scene = moq_scene::Scene {
			recorded: true,
			safe: true,
			arrangement: moq_scene::Arrangement::Grid,
			portrait: moq_scene::Portrait {
				arrangement: moq_scene::Arrangement::Inset,
			},
			..moq_scene::Scene::new("Interview")
		};
		let copy = build::scene_copy(&scene, "Interview copy".into());
		assert_eq!(copy.name, "Interview copy");
		assert_eq!((copy.arrangement, copy.portrait), (scene.arrangement, scene.portrait));
		assert!(!copy.recorded && !copy.safe);
	}

	/// A layer the UI picked is let go of once an edit of the engine's own
	/// inserts or replaces a layer of its scene, and kept for one elsewhere.
	#[test]
	fn a_guest_appearing_lets_go_of_a_layer_of_their_scene() {
		let picked = |scene: &str| Picked {
			layer: Layer::scene(scene),
			index: 2,
		};
		let mut shifts = (0, BTreeMap::new());
		assert!(!shifted_since(Some(&picked("Interview")), &shifts, 0));
		shifts.0 += 1;
		shifts.1.insert("Interview".to_string(), shifts.0);
		assert!(shifted_since(Some(&picked("Interview")), &shifts, 0));
		assert!(!shifted_since(Some(&picked("Solo")), &shifts, 0), "another scene");
		assert!(!shifted_since(Some(&picked("Interview")), &shifts, 1), "already seen");
		assert!(!shifted_since(None, &shifts, 0));
		shifts.0 += 1;
		shifts.1.insert("Solo".to_string(), shifts.0);
		assert!(shifted_since(Some(&picked("Interview")), &shifts, 0), "two since");
		assert!(!shifted_since(Some(&picked("Interview")), &shifts, 1));
	}

	/// A layer showing a role nothing fills offers no copy of its own, to
	/// duplicate or make independent, as a clone of it would be refused; one
	/// it fills copies the source filling it.
	#[test]
	fn an_unfilled_role_has_nothing_to_copy() {
		let mut collection = Collection::new();
		let color = moq_scene::Rgba([255, 255, 255, 255]);
		collection.sources.insert("game".into(), Source::Color { color });
		collection.sources.insert("Main".into(), Source::Role { source: None });
		let err = build::copy_of(&collection, "Main").unwrap_err();
		assert_eq!(err.to_string(), build::NOTHING_TO_COPY);
		let filled = Source::Role {
			source: Some("game".into()),
		};
		collection.sources.insert("Main".into(), filled);
		let (copy, _) = build::copy_of(&collection, "Main").unwrap();
		assert!(matches!(&copy, Source::Clone { source } if source == "game"));
		let mut draft = collection.draft("Duplicate `Main` as its own copy");
		draft.set_source("Main copy", Some(copy)).unwrap();
		draft.finish().unwrap();
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

[sources.Camera]
kind = "role"
source = "cam"

[video.cam]
background = "remove"
filters = [{ kind = "opacity", amount = 0.5 }]
"#,
		)
		.unwrap();

		// A role's copy is of the source filling it, with that look.
		for name in ["cam", "Camera"] {
			let (source, video) = clone_of(&collection, name);
			assert_eq!(source, Source::Clone { source: "cam".into() }, "{name}");
			let video = video.unwrap();
			assert_eq!(video.background, Background::Keep);
			assert_eq!(video.filters, collection.video["cam"].filters, "{name}");

			let mut draft = collection.draft("clone");
			draft.set_source("clone", Some(source)).unwrap();
			draft.set_video("clone", Some(video)).unwrap();
			draft.finish().unwrap();
		}
	}
}
