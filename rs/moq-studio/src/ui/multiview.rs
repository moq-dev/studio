//! The multiview: a window of its own, for another monitor, with every scene
//! and the sources picked live in a uniform grid, the scene on air framed in
//! the accent and the cued one amber, and a clock to read from across the
//! room. A click on a scene cues or takes it, as its pad does.
//!
//! It draws on a schedule of its own, apart from the studio's window, so it
//! stays live while that window is hidden, such as behind a game.

use std::sync::Mutex;

use egui::{Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, ViewportCommand, pos2, vec2};
use moq_scene::Detail;

use super::*;

/// Where this user's multiview is kept, in the app's settings: it is how one
/// operator's desk is laid out, not part of the show.
pub(super) const KEY: &str = "multiview";

/// How often a tile not on air or cued is redrawn, a second. Those on air
/// and cued are redrawn every frame the program draws.
const REST: u32 = 10;

/// Room around the window's contents, and between tiles.
const MARGIN: f32 = 28.0;
const GAP: f32 = 20.0;
/// How tall the header is.
const HEADER: f32 = 96.0;
/// How tall a tile's name bar is, at the mock-up's size.
const NAME: f32 = 38.0;
/// How wide a tile is in the mock-up, which the text in it scales from.
const TILE: f32 = 331.0;
/// The window's size the first time it opens, the mock-up's.
const SIZE: Vec2 = vec2(1440.0, 900.0);
/// How long a failed click's message shows.
const FAILED: Duration = Duration::from_secs(6);

/// The multiview's backdrop, darker than the studio's, as a monitor wall's.
const BACKDROP: Color32 = Color32::from_rgb(0x05, 0x05, 0x07);
/// A tile's name bar on air and cued.
const AIR_BAR: Color32 = Color32::from_rgb(0x0b, 0x15, 0x30);
const CUE_BAR: Color32 = Color32::from_rgb(0x2a, 0x1d, 0x05);

/// This user's multiview, as the app's settings keep it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct Saved {
	/// Whether it is open, which it is again at the next launch.
	open: bool,
	/// Its outer top-left corner when last seen out of full screen, where the
	/// desktop says, which Wayland does not.
	at: Option<Pos2>,
	/// Its inner size when last seen out of full screen.
	size: Option<Vec2>,
	fullscreen: bool,
	/// The sources each show's multiview shows, or leaves out, against
	/// [`live`]'s default, by the show's file and then the source's name.
	picks: BTreeMap<String, BTreeMap<String, bool>>,
}

impl Saved {
	/// Whether `show`'s multiview shows the source called `name`, a `source`.
	fn shows(&self, show: &str, name: &str, source: &Source) -> bool {
		let pick = self.picks.get(show).and_then(|picks| picks.get(name));
		source.has_video() && pick.copied().unwrap_or_else(|| live(source))
	}

	/// Show the source called `name`, a `source`, on `show`'s multiview, or
	/// leave it out, remembering only a pick against its default.
	fn pick(&mut self, show: &str, name: &str, source: &Source, shown: bool) {
		let picks = self.picks.entry(show.to_string()).or_default();
		match shown == live(source) {
			true => picks.remove(name),
			false => picks.insert(name.to_string(), shown),
		};
		if picks.is_empty() {
			self.picks.remove(show);
		}
	}

	/// Note where the window is, from what the desktop said this frame.
	fn follow(&mut self, info: &egui::ViewportInfo) {
		match info.fullscreen {
			Some(true) => self.fullscreen = true,
			Some(false) => {
				self.fullscreen = false;
				if let Some(outer) = info.outer_rect {
					self.at = Some(outer.min);
				}
				if let Some(inner) = info.inner_rect {
					self.size = Some(inner.size());
				}
			}
			None => {}
		}
	}
}

/// Whether a source's picture moves on its own, which puts it on the
/// multiview unless the operator leaves it out: a camera, a screen, a
/// window, a guest, a web page, a video file, or a replay. A still or a
/// color, a copy, and a scene, which has a tile of its own, are left out
/// unless picked.
fn live(source: &Source) -> bool {
	matches!(
		source,
		Source::Camera { .. }
			| Source::Display { .. }
			| Source::Window { .. }
			| Source::PickedDisplay
			| Source::PickedWindow
			| Source::Broadcast { .. }
			| Source::Layer { .. }
			| Source::Browser { .. }
			| Source::Media { .. }
			| Source::Replay
	)
}

/// The multiview, shared between the studio's window, which opens it and
/// keeps its picks, and its own, which draws it.
pub(super) struct Multiview(Arc<Mutex<Inner>>);

/// What the multiview draws from, and what it holds between frames.
struct Inner {
	engine: Arc<Engine>,
	controls: Arc<Controls>,
	render: eframe::egui_wgpu::RenderState,
	saved: Saved,
	/// What it opened with, kept while it is open, so the place it was moved
	/// to since is not sent back to the desktop.
	opened: Option<egui::ViewportBuilder>,
	/// Each thumbnail the compositor drew of those shown last frame.
	thumbnails: BTreeMap<Thumbnail, Registered>,
	/// The thumbnails it shows, at their detail.
	shown: BTreeMap<Thumbnail, Detail>,
	/// The thumbnails the studio's window showed last, which the compositor
	/// draws beside these.
	studio_shown: BTreeSet<Thumbnail>,
	/// The keys bound, as the studio's window last had them.
	bindings: Bindings,
	/// The last click that failed, and when.
	failed: Option<(String, kio::clock::Instant)>,
	/// Whether a test draws it as the window itself.
	alone: bool,
}

impl Multiview {
	pub(super) fn new(render: eframe::egui_wgpu::RenderState, engine: Arc<Engine>, controls: Arc<Controls>) -> Self {
		Self(Arc::new(Mutex::new(Inner {
			engine,
			controls,
			render,
			saved: Saved::default(),
			opened: None,
			thumbnails: BTreeMap::new(),
			shown: BTreeMap::new(),
			studio_shown: BTreeSet::new(),
			bindings: Bindings::new(),
			failed: None,
			alone: false,
		})))
	}

	/// Take up what this user saved.
	pub(super) fn load(&self, saved: Saved) {
		self.0.lock().unwrap().saved = saved;
	}

	/// Keep what this user picked.
	pub(super) fn save(&self, storage: &mut dyn eframe::Storage) {
		eframe::set_value(storage, KEY, &self.0.lock().unwrap().saved);
	}

	pub(super) fn open(&self) -> bool {
		self.0.lock().unwrap().saved.open
	}

	/// Open it, or close it.
	pub(super) fn set_open(&self, open: bool) {
		self.0.lock().unwrap().set_open(open);
	}

	/// Whether a test draws it as the window itself, in place of the studio.
	pub(super) fn alone(&self) -> bool {
		self.0.lock().unwrap().alone
	}

	/// Draw it as the window itself, with the sources called `picked` on it.
	#[cfg(test)]
	pub(super) fn set_alone(&self, picked: &[&str]) {
		let mut inner = self.0.lock().unwrap();
		inner.alone = true;
		inner.saved.open = true;
		for name in picked {
			inner.pick_source(name, true);
		}
	}

	/// Its window, while it is open, which draws itself from then on. Called
	/// each frame of the studio's window, which `ctx` draws.
	pub(super) fn show(&self, ctx: &egui::Context) {
		let builder = {
			let mut inner = self.0.lock().unwrap();
			if !inner.saved.open {
				return;
			}
			inner.builder()
		};
		let inner = self.0.clone();
		let id = egui::ViewportId::from_hash_of(KEY);
		ctx.show_viewport_deferred(id, builder, move |ui, _| {
			let mut inner = inner.lock().unwrap();
			let info = ui.ctx().input(|input| input.viewport().clone());
			if info.close_requested() {
				inner.set_open(false);
				// So the studio's window stops showing it.
				ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
				return;
			}
			if !inner.saved.open {
				return;
			}
			inner.saved.follow(&info);
			let rect = ui.max_rect();
			inner.ui(ui, rect);
		});
	}

	/// Draw it into `ui`, as the window itself.
	pub(super) fn ui(&self, ui: &mut Ui) {
		let rect = ui.max_rect();
		self.0.lock().unwrap().ui(ui, rect);
	}

	/// Have the compositor draw the thumbnails the studio's window shows,
	/// `studio_shown`, beside the multiview's, and note the keys bound,
	/// `bindings`, which its scenes' tiles show.
	pub(super) fn share(&self, studio_shown: &BTreeSet<Thumbnail>, bindings: Bindings) {
		let mut inner = self.0.lock().unwrap();
		inner.studio_shown.clone_from(studio_shown);
		inner.bindings = bindings;
		inner.set_thumbnails();
	}

	/// Give back the thumbnails it registered with the window.
	pub(super) fn release(&self) {
		self.0.lock().unwrap().release();
	}
}

/// A source as the multiview shows it.
struct SourceTile {
	name: String,
	icon: Icon,
	/// Whether the live picture shows it.
	on_air: bool,
	/// Whether it has a tile.
	shown: bool,
}

/// A scene as its tile shows it.
struct SceneTile {
	name: String,
	canvas: Canvas,
	/// Why it cannot go on program, if it cannot.
	refused: Option<String>,
	key: Option<String>,
	tally: Tally,
	tag: Option<&'static str>,
}

/// What the multiview shows this frame, copied under one short lock.
struct Snapshot {
	scenes: Vec<SceneTile>,
	sources: Vec<SourceTile>,
	/// The program's shape, width over height.
	shape: f32,
	framerate: u32,
	/// The show's name: its scene file's, without the extension.
	show: String,
}

/// What a tile shows.
enum Tile<'a> {
	Scene(&'a SceneTile),
	Source(&'a SourceTile),
}

/// What a tile asked for.
enum TileAction {
	/// Take the scene called this, or cue it in Studio mode.
	Click(String),
	/// Leave the source called this out.
	Leave(String),
}

/// Where the tiles go: how many across, and each one's picture's size.
#[derive(Debug, PartialEq)]
struct Grid {
	columns: usize,
	rows: usize,
	/// A tile's picture, at the show's shape.
	picture: Vec2,
	/// The room between rows.
	gap: f32,
}

impl Grid {
	/// The uniform grid that shows `count` tiles of `shape`, width over
	/// height, largest within `room`, with a name bar `name` tall under each.
	fn fit(room: Vec2, count: usize, shape: f32, name: f32) -> Self {
		let count = count.max(1);
		let mut best: Option<Self> = None;
		for columns in 1..=count {
			let rows = count.div_ceil(columns);
			let across = (room.x - GAP * (columns - 1) as f32) / columns as f32;
			let down = ((room.y - GAP * (rows - 1) as f32) / rows as f32 - name) * shape;
			let width = across.min(down).max(1.0);
			if best.as_ref().is_none_or(|best| width > best.picture.x + 0.5) {
				best = Some(Self {
					columns,
					rows,
					picture: vec2(width, width / shape),
					gap: GAP,
				});
			}
		}
		let mut grid = best.expect("a grid of at least one column");
		// The rows spread down the room, as the mock-up's do, but no further
		// apart than twice the gap.
		if grid.rows > 1 {
			let tall = grid.rows as f32 * (grid.picture.y + name);
			grid.gap = ((room.y - tall) / (grid.rows - 1) as f32).clamp(GAP, GAP * 2.0);
		}
		grid
	}
}

impl Inner {
	/// Open it, or close it, letting go of its thumbnails.
	fn set_open(&mut self, open: bool) {
		self.saved.open = open;
		if !open {
			self.opened = None;
			self.shown.clear();
			self.release();
			self.set_thumbnails();
		}
	}

	/// Give back the thumbnails it registered with the window.
	fn release(&mut self) {
		let mut renderer = self.render.renderer.write();
		for registered in std::mem::take(&mut self.thumbnails).into_values() {
			renderer.free_texture(&registered.id);
		}
	}

	/// Have the compositor draw what this and the studio's window show.
	fn set_thumbnails(&self) {
		let studio = self
			.studio_shown
			.iter()
			.map(|thumbnail| (thumbnail.clone(), Detail::default()));
		let shown = self
			.shown
			.iter()
			.map(|(thumbnail, detail)| (thumbnail.clone(), *detail));
		let mut state = self.engine.state.lock().unwrap();
		state.compositor.set_thumbnails(studio.chain(shown));
	}

	/// What the window opens with: where and how large it last was, full
	/// screen if it was.
	fn builder(&mut self) -> egui::ViewportBuilder {
		let saved = &self.saved;
		self.opened
			.get_or_insert_with(|| {
				let mut builder = egui::ViewportBuilder::default()
					.with_title("Multiview")
					.with_app_id("live.kixel.studio")
					.with_icon(crate::icon())
					.with_inner_size(saved.size.unwrap_or(SIZE))
					.with_min_inner_size([480.0, 300.0])
					.with_fullscreen(saved.fullscreen);
				if let Some(at) = saved.at {
					builder = builder.with_position(at);
				}
				builder
			})
			.clone()
	}

	/// The show's file, which its picks are kept under.
	fn show_file(&self) -> String {
		self.engine.autosave.path().display().to_string()
	}

	/// What this frame shows, under one short lock, having followed the
	/// thumbnails the compositor drew of those shown last frame.
	fn snapshot(&mut self, ctx: &egui::Context) -> Snapshot {
		let show = self.show_file();
		let live = self.engine.on_air().live().is_some();
		let engine = self.engine.clone();
		let state = engine.state.lock().unwrap();
		self.follow_thumbnails(&state.compositor);
		let collection = state.collection();
		let program = state.compositor.program_scene();
		let cued = state.cued();
		let scenes = collection
			.scenes
			.iter()
			.map(|scene| {
				let name = scene.name.as_str();
				let (tally, tag) = shell::tally(program == Some(name), live, cued == Some(name));
				let action = Action::Scene(scene.name.clone());
				SceneTile {
					name: scene.name.clone(),
					canvas: scene.canvas(collection.canvas),
					refused: collection.refuses(name),
					key: self.bindings.get(&action).map(|binding| binding.shown(ctx)),
					tally,
					tag,
				}
			})
			.collect();
		let on_air = state.compositor.on_air_sources(collection);
		let sources = collection
			.sources
			.iter()
			.filter(|(_, source)| source.has_video())
			.map(|(name, source)| SourceTile {
				name: name.clone(),
				icon: layers::kind(collection, name),
				// The compositor names a copy by the source it copies.
				on_air: on_air.contains(collection.capture(name)),
				shown: self.saved.shows(&show, name, source),
			})
			.collect();
		let canvas = state.compositor.canvas();
		let path = self.engine.autosave.path();
		Snapshot {
			scenes,
			sources,
			shape: canvas.width as f32 / canvas.height.max(1) as f32,
			framerate: canvas.framerate,
			show: path
				.file_stem()
				.unwrap_or(path.as_os_str())
				.to_string_lossy()
				.into_owned(),
		}
	}

	/// Show each thumbnail the compositor drew of those shown last frame, and
	/// let go of the rest, so this frame starts with none shown.
	fn follow_thumbnails(&mut self, compositor: &moq_scene::Compositor) {
		let mut gone = std::mem::take(&mut self.thumbnails);
		for thumbnail in std::mem::take(&mut self.shown).into_keys() {
			let Some(texture) = compositor.thumbnail(&thumbnail) else {
				continue;
			};
			let registered = match gone.remove(&thumbnail) {
				Some(mut registered) => {
					registered.follow(&self.render, texture);
					registered
				}
				None => Registered::new(&self.render, texture),
			};
			self.thumbnails.insert(thumbnail, registered);
		}
		let mut renderer = self.render.renderer.write();
		for registered in gone.into_values() {
			renderer.free_texture(&registered.id);
		}
	}

	/// The multiview in `rect`: the header, then a tile for each scene and
	/// each source picked.
	fn ui(&mut self, ui: &mut Ui, rect: Rect) {
		let snapshot = self.snapshot(ui.ctx());
		// Live at the program's rate, whether or not the studio's window draws.
		// egui takes its predicted frame time off the delay, which would
		// redraw at the display's rate instead, so it is added back.
		let period = Duration::from_secs(1) / snapshot.framerate.max(1);
		let predicted = ui.input(|input| input.predicted_dt);
		let predicted = Duration::try_from_secs_f32(predicted).unwrap_or_default();
		ui.ctx().request_repaint_after(period + predicted.min(period));
		ui.painter().rect_filled(rect, 0, BACKDROP);
		let shown: Vec<&SourceTile> = snapshot.sources.iter().filter(|source| source.shown).collect();

		let inner = rect.shrink2(vec2(MARGIN, 26.0));
		let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER));
		let subtitle = format!(
			"{} · {} scenes, {} sources",
			snapshot.show,
			snapshot.scenes.len(),
			shown.len()
		);
		if let Some((name, on)) = self.header(ui, header, &subtitle, &snapshot.sources) {
			self.pick_source(&name, on);
		}

		let room = Rect::from_min_max(pos2(inner.left(), header.bottom() + GAP), inner.max);
		let count = snapshot.scenes.len() + shown.len();
		let scale = (Grid::fit(room.size(), count, snapshot.shape, NAME).picture.x / TILE).clamp(0.8, 2.0);
		let grid = Grid::fit(room.size(), count, snapshot.shape, NAME * scale);
		let tile = vec2(grid.picture.x, grid.picture.y + NAME * scale);
		let used = vec2(
			grid.columns as f32 * tile.x + (grid.columns - 1) as f32 * GAP,
			grid.rows as f32 * tile.y + (grid.rows - 1) as f32 * grid.gap,
		);
		let origin = pos2(room.center().x - used.x / 2.0, room.top());
		let ppp = ui.ctx().pixels_per_point();
		let detail = |live: bool| Detail {
			// The picture's longest side in pixels, a step of 64 up so a
			// window being resized does not make a new texture every frame.
			side: ((grid.picture.max_elem() * ppp / 64.0).ceil() * 64.0) as u32,
			rate: if live { u32::MAX } else { REST },
		};

		let tiles = snapshot.scenes.iter().map(Tile::Scene);
		let tiles: Vec<Tile> = tiles.chain(shown.iter().map(|source| Tile::Source(source))).collect();
		let mut action = None;
		for (index, shows) in tiles.iter().enumerate() {
			let (column, row) = (index % grid.columns, index / grid.columns);
			let at = origin + vec2(column as f32 * (tile.x + GAP), row as f32 * (tile.y + grid.gap));
			let rect = Rect::from_min_size(at, tile);
			let (thumbnail, live) = match shows {
				Tile::Scene(scene) => (Thumbnail::Scene(scene.name.clone()), scene.tally != Tally::Off),
				Tile::Source(source) => (Thumbnail::Source(source.name.clone()), false),
			};
			let picture = self.thumbnails.get(&thumbnail).map(|registered| {
				let size = match shows {
					Tile::Scene(scene) => vec2(scene.canvas.width as f32, scene.canvas.height as f32),
					Tile::Source(_) => vec2(registered.texture.width() as f32, registered.texture.height() as f32),
				};
				egui::load::SizedTexture::new(registered.id, size)
			});
			self.shown.insert(thumbnail, detail(live));
			let response = tile_ui(ui, rect, shows, picture, grid.picture.y, scale);
			match shows {
				Tile::Scene(scene) => {
					if response.clicked() && scene.refused.is_none() {
						action = Some(TileAction::Click(scene.name.clone()));
					}
				}
				Tile::Source(source) => {
					response.context_menu(|ui| {
						if ui.button("Leave out of the multiview").clicked() {
							action = Some(TileAction::Leave(source.name.clone()));
							ui.close();
						}
					});
				}
			}
		}
		match action {
			Some(TileAction::Click(scene)) => self.click(&scene),
			Some(TileAction::Leave(name)) => self.pick_source(&name, false),
			None => {}
		}
		self.set_thumbnails();
		if let Some((why, _)) = self.failed.as_ref().filter(|(_, at)| at.elapsed() < FAILED) {
			let painter = ui.painter();
			let text = painter.layout(why.clone(), kit::ui_medium(14.0), kit::TEXT, inner.width() / 2.0);
			let bottom = pos2(rect.center().x, rect.bottom() - 18.0);
			let back = Rect::from_center_size(
				bottom - vec2(0.0, text.size().y / 2.0 + 8.0),
				text.size() + vec2(28.0, 16.0),
			);
			painter.rect(back, 8, kit::POPOVER, Stroke::new(1.0, kit::ERR), StrokeKind::Inside);
			painter.galley(back.center() - text.size() / 2.0, text, kit::TEXT);
		}

		// Full screen on whichever monitor the window is on, as a projector
		// goes.
		let fullscreen = self.saved.fullscreen;
		let (toggle, escape) = ui
			.ctx()
			.input(|input| (input.key_pressed(egui::Key::F11), input.key_pressed(egui::Key::Escape)));
		if toggle || (escape && fullscreen) {
			ui.ctx().send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
		}
	}

	/// Take the scene called `scene`, or cue it in Studio mode, as a click on
	/// its pad does, saying why if it cannot.
	fn click(&mut self, scene: &str) {
		let mut state = self.engine.state.lock().unwrap();
		let result = match self.controls.studio() {
			true => crate::controls::cue(&mut state, scene),
			false => crate::controls::take(&mut state, scene),
		};
		drop(state);
		if let Err(err) = result {
			let why = format!("{err:#}");
			tracing::warn!(%scene, error = why, "the multiview cannot put the scene on");
			self.failed = Some((why, kio::clock::Instant::now()));
		}
	}

	/// The header in `rect`: the title and `subtitle`, the clock, the
	/// stream's health and time on air, and the controls, and the source
	/// picked or left out in the sources' popover, if one was.
	fn header(&mut self, ui: &mut Ui, rect: Rect, subtitle: &str, sources: &[SourceTile]) -> Option<(String, bool)> {
		let painter = ui.painter();
		let scale = (rect.width() / (SIZE.x - 2.0 * MARGIN)).clamp(0.6, 2.0);
		// A double click on the header goes full screen or back.
		let header = ui.interact(rect, ui.id().with("multiview header"), Sense::click());
		let fullscreen = self.saved.fullscreen;
		if header.double_clicked() {
			ui.ctx().send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
		}

		let title = kit::caps_galley(painter, "Multiview", kit::display_bold(26.0), kit::TEXT, None);
		let title_height = title.size().y;
		let sub = painter.layout_no_wrap(subtitle.to_string(), kit::ui(12.5), kit::DIM);
		let top = rect.center().y - (title_height + 8.0 + sub.size().y) / 2.0;
		painter.galley(pos2(rect.left(), top), title, kit::TEXT);
		painter.galley(pos2(rect.left(), top + title_height + 8.0), sub, kit::DIM);

		let time = painter.layout_no_wrap(wall_clock(), kit::display_bold(76.0 * scale), kit::TEXT);
		painter.galley(rect.center() - time.size() / 2.0, time, kit::TEXT);

		let mut picked = None;
		let builder = egui::UiBuilder::new()
			.max_rect(rect)
			.layout(egui::Layout::right_to_left(egui::Align::Center));
		ui.scope_builder(builder, |ui| {
			ui.spacing_mut().item_spacing.x = 14.0;
			let name = match fullscreen {
				true => "Leave full screen",
				false => "Full screen",
			};
			let button = IconButton::new(Icon::Maximize, name).key(Some("F11"));
			if ui.add(button.lit(fullscreen, kit::P3)).clicked() {
				ui.ctx().send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
			}
			let button = ui.add(IconButton::new(Icon::Shown, "Sources shown"));
			kit::popover(&button).show(|ui| {
				ui.set_width(260.0);
				if sources.is_empty() {
					ui.label(egui::RichText::new("This show has no source with a picture.").color(kit::DIM));
				}
				for source in sources {
					kit::popover_row(ui, false, |ui| {
						ui.add(source.icon.image(14.0, kit::DIM));
						let mut shown = source.shown;
						if ui.add(kit::Toggle::new(&mut shown).label(&source.name)).changed() {
							picked = Some((source.name.clone(), shown));
						}
					});
				}
			});
			ui.add_space(8.0);
			on_air(ui, &self.engine);
			let (word, dot) = live::health_word(&self.engine);
			ui.add(kit::Pill::new(word).dot(dot));
		});
		picked
	}

	/// Show the source called `name` on this show's multiview, or leave it
	/// out.
	fn pick_source(&mut self, name: &str, shown: bool) {
		let show = self.show_file();
		let state = self.engine.state.lock().unwrap();
		if let Some(source) = state.collection().sources.get(name) {
			self.saved.pick(&show, name, source, shown);
		}
	}
}

/// LIVE and the time on air, or OFF AIR.
fn on_air(ui: &mut Ui, engine: &Engine) {
	let painter = ui.painter().clone();
	let font = kit::display_bold(17.0);
	let (fill, word, clock) = match engine.on_air() {
		crate::engine::OnAir::Live(live) => (kit::ACCENT, "Live", Some(shell::clock(live.since.elapsed()))),
		crate::engine::OnAir::Starting => (kit::P2, "Going live", None),
		crate::engine::OnAir::Ending => (kit::ACCENT, "Ending", None),
		crate::engine::OnAir::Off => (kit::P2, "Off air", None),
	};
	let ink = if fill == kit::P2 { kit::DIM } else { Color32::WHITE };
	let word = kit::caps_galley(&painter, word, font, ink, None);
	let clock = clock.map(|clock| painter.layout_no_wrap(clock, kit::mono(14.0), ink));
	let width = 12.0 + word.size().x + clock.as_ref().map_or(0.0, |clock| 10.0 + clock.size().x) + 12.0;
	let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
	if fill == kit::ACCENT {
		let glow = egui::Shadow {
			offset: [0, 0],
			blur: 22,
			spread: 0,
			color: kit::ACCENT.gamma_multiply(0.4),
		};
		painter.add(glow.as_shape(rect, egui::CornerRadius::same(6)));
	}
	painter.rect(rect, 6, fill, Stroke::new(1.0, kit::LINE), StrokeKind::Inside);
	let at = pos2(rect.left() + 12.0, rect.center().y - word.size().y / 2.0);
	let after = at.x + word.size().x + 10.0;
	painter.galley(at, word, ink);
	if let Some(clock) = clock {
		painter.galley(pos2(after, rect.center().y - clock.size().y / 2.0), clock, ink);
	}
}

/// Draw a tile at `rect`: `picture`, `picture_height` tall, over its name,
/// its text `scale` times the mock-up's. A scene's tile hears clicks; a
/// source's, its menu.
fn tile_ui(
	ui: &Ui,
	rect: Rect,
	shows: &Tile,
	picture: Option<egui::load::SizedTexture>,
	picture_height: f32,
	scale: f32,
) -> egui::Response {
	let (name, refused, tally) = match shows {
		Tile::Scene(scene) => (scene.name.as_str(), scene.refused.as_deref(), scene.tally),
		Tile::Source(source) => (source.name.as_str(), None, Tally::Off),
	};
	let id = ui.id().with(("tile", matches!(shows, Tile::Scene(_)), name));
	let response = ui.interact(rect, id, Sense::click());
	let clickable = matches!(shows, Tile::Scene(_)) && refused.is_none();
	response.widget_info(|| {
		egui::WidgetInfo::selected(egui::WidgetType::Button, refused.is_none(), tally == Tally::Air, name)
	});
	let response = match refused {
		Some(why) => kit::tip(response, why, None),
		None => response,
	};
	if !ui.is_rect_visible(rect) {
		return response;
	}
	let painter = ui.painter();
	let radius = 9;
	let (outline, bar) = match tally {
		Tally::Air => (Stroke::new(3.0, kit::ACCENT), AIR_BAR),
		Tally::Cue => (Stroke::new(3.0, kit::CUE), CUE_BAR),
		Tally::Off if clickable && response.hovered() => (Stroke::new(1.0, kit::DIM), kit::P1),
		Tally::Off => (Stroke::new(1.0, kit::LINE), kit::P1),
	};
	let glow = match tally {
		Tally::Air => Some(kit::ACCENT.gamma_multiply(0.25)),
		Tally::Cue => Some(kit::CUE.gamma_multiply(0.15)),
		Tally::Off => None,
	};
	if let Some(color) = glow {
		let glow = egui::Shadow {
			offset: [0, 0],
			blur: 40,
			spread: 0,
			color,
		};
		painter.add(glow.as_shape(rect, egui::CornerRadius::same(radius)));
	}
	painter.rect_filled(rect, radius, bar);
	let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), picture_height));
	let top = egui::CornerRadius {
		nw: radius,
		ne: radius,
		sw: 0,
		se: 0,
	};
	painter.rect_filled(thumb, top, kit::BG);
	if let Tile::Source(_) = shows {
		checker(painter, thumb);
	}
	if let Some(texture) = picture {
		let fit = texture.size * (thumb.size() / texture.size).min_elem();
		let at = Rect::from_center_size(thumb.center(), fit);
		egui::Image::new(texture).corner_radius(top).paint_at(ui, at);
	}
	if refused.is_some() {
		painter.rect_filled(thumb, top, kit::BG.gamma_multiply(0.6));
	}
	painter.rect_stroke(rect, radius, outline, StrokeKind::Outside);

	// The name bar: the key or the source's kind, the name, and the tally's
	// tag or the accent dot.
	let bar = Rect::from_min_max(pos2(rect.left(), thumb.bottom()), rect.max);
	let mut x = bar.left() + 11.0 * scale;
	let middle = bar.center().y;
	match shows {
		Tile::Scene(SceneTile { key: Some(key), .. }) => {
			let text = painter.layout_no_wrap(key.clone(), kit::mono(10.0 * scale), kit::DIM);
			let cap = Rect::from_min_size(
				pos2(x, middle - (text.size().y + 6.0) / 2.0),
				text.size() + vec2(10.0, 6.0),
			);
			painter.rect(
				cap,
				4,
				Color32::from_black_alpha(0x66),
				Stroke::new(1.0, kit::LINE),
				StrokeKind::Inside,
			);
			painter.galley(cap.center() - text.size() / 2.0, text, kit::DIM);
			x = cap.right() + 9.0 * scale;
		}
		Tile::Scene(_) => {}
		Tile::Source(source) => {
			let size = 15.0 * scale;
			let icon = Rect::from_min_size(pos2(x, middle - size / 2.0), Vec2::splat(size));
			source.icon.paint(ui, icon, size, kit::DIM);
			x = icon.right() + 9.0 * scale;
		}
	}
	let tag = match (shows, tally) {
		(Tile::Scene(scene), Tally::Air) => Some((scene.tag.unwrap_or("On air"), kit::ACCENT, Color32::WHITE)),
		(Tile::Scene(scene), Tally::Cue) => Some((scene.tag.unwrap_or("Next"), kit::CUE, kit::ON_CUE)),
		_ => None,
	};
	let mut right = bar.right() - 11.0 * scale;
	if let Some((text, fill, ink)) = tag {
		let galley = kit::caps_galley(painter, text, kit::display_bold(13.0 * scale), ink, None);
		let size = galley.size() + vec2(14.0, 4.0) * scale;
		let rect = Rect::from_min_size(pos2(right - size.x, middle - size.y / 2.0), size);
		painter.rect_filled(rect, 4, fill);
		painter.galley(rect.center() - galley.size() / 2.0, galley, ink);
		right = rect.left() - 9.0 * scale;
	}
	if let Tile::Source(source) = shows
		&& source.on_air
	{
		painter.circle_filled(pos2(right - 4.0 * scale, middle), 4.0 * scale, kit::ACCENT);
		right -= 17.0 * scale;
	}
	let ink = if refused.is_some() { kit::DIM } else { kit::TEXT };
	let text = kit::caps_galley(
		painter,
		name,
		kit::display_bold(17.0 * scale),
		ink,
		Some((right - x).max(0.0)),
	);
	painter.galley(pos2(x, middle - text.size().y / 2.0), text, ink);
	response
}

/// A checkerboard over `rect`, which shows through where a source's picture
/// is transparent.
fn checker(painter: &egui::Painter, rect: Rect) {
	let side = (rect.width() * 0.07).max(6.0);
	let light = Color32::from_rgb(0x1c, 0x1c, 0x23);
	painter.rect_filled(rect, 0, Color32::from_rgb(0x13, 0x13, 0x18));
	let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
	let (across, down) = (
		(rect.width() / side).ceil() as usize,
		(rect.height() / side).ceil() as usize,
	);
	for row in 0..down {
		for column in (row % 2..across).step_by(2) {
			let at = rect.min + vec2(column as f32 * side, row as f32 * side);
			painter.rect_filled(Rect::from_min_size(at, Vec2::splat(side)), 0, light);
		}
	}
}

/// The time of day here, as a clock on the wall reads it.
fn wall_clock() -> String {
	let since = kio::clock::wall().saturating_duration_since(kio::clock::Wall::UNIX_EPOCH);
	let now = jiff::Timestamp::from_second(since.as_secs() as i64).unwrap_or_default();
	now.to_zoned(jiff::tz::TimeZone::system())
		.strftime("%H:%M:%S")
		.to_string()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The mock-up's twelve tiles fit four across in its window, and the rows
	/// spread down the room left.
	#[test]
	fn twelve_tiles_go_four_across() {
		let grid = Grid::fit(vec2(1384.0, 730.0), 12, 16.0 / 9.0, NAME);
		assert_eq!((grid.columns, grid.rows), (4, 3));
		assert_eq!(grid.picture.x, 331.0);
		assert!(grid.gap > GAP && grid.gap <= 2.0 * GAP, "{grid:?}");
	}

	/// A 4x4 grid's sixteen tiles, and one tile filling its room.
	#[test]
	fn the_grid_fits_its_room() {
		let shape = 16.0 / 9.0;
		for (count, columns) in [(16, 4), (1, 1), (2, 2), (20, 5)] {
			let room = vec2(1384.0, 730.0);
			let grid = Grid::fit(room, count, shape, NAME);
			assert_eq!(grid.columns, columns, "{count} tiles: {grid:?}");
			let wide = grid.columns as f32 * grid.picture.x + (grid.columns - 1) as f32 * GAP;
			let tall = grid.rows as f32 * (grid.picture.y + NAME) + (grid.rows - 1) as f32 * GAP;
			assert!(
				wide <= room.x + 0.01 && tall <= room.y + 0.01,
				"{count} tiles: {grid:?}"
			);
		}
	}

	/// A source that moves is on the multiview until left out; a still is
	/// not until picked; a microphone never is. Only a pick against the
	/// default is kept.
	#[test]
	fn sources_are_picked_against_their_default() {
		let camera = Source::Camera { id: None };
		let color = Source::Color {
			color: moq_scene::Rgba([0, 0, 0, 255]),
		};
		let mic = Source::Microphone { id: None };
		let mut saved = Saved::default();
		assert!(saved.shows("show", "cam", &camera));
		assert!(!saved.shows("show", "bg", &color));
		assert!(!saved.shows("show", "mic", &mic));

		saved.pick("show", "cam", &camera, false);
		saved.pick("show", "bg", &color, true);
		assert!(!saved.shows("show", "cam", &camera));
		assert!(saved.shows("show", "bg", &color));
		assert!(saved.shows("other", "cam", &camera), "another show's own");

		saved.pick("show", "cam", &camera, true);
		saved.pick("show", "bg", &color, false);
		assert!(saved.picks.is_empty(), "back to the defaults: {:?}", saved.picks);
	}
}
