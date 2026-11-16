//! What every page shares: the title bar, which is the window's own frame,
//! the scene pads, the panic key, and the sound, drawn from a snapshot of the
//! engine, and the windows and messages over the pages.

use egui::{
	Align, Align2, CursorIcon, Rect, ResizeDirection, Sense, Stroke, UiBuilder, Vec2, ViewportCommand, pos2, vec2,
};

use super::build::duplicate_scene;
use super::cue::{Corner, Cued, transition_words};
use super::layout::{Arrangement, Layout, SOUND_ROW, fit};
use super::levels::reading;
use super::*;

/// What the title bar, the strip, and the live picture show, copied under one
/// short lock a frame.
pub(super) struct Snapshot {
	/// The show's name: its scene file's, without the extension.
	show: String,
	/// The edits undo and redo would take back and bring back, by label.
	undo: Option<String>,
	redo: Option<String>,
	/// The scene on program.
	program: Option<String>,
	scenes: Vec<ScenePad>,
	/// The scene the panic key cuts to.
	safe: Option<String>,
	/// Each sound the stream carries.
	sounds: Vec<Sound>,
	/// The audio scene the stream carries.
	stream: Option<String>,
	/// How many scenes have been taken, which the effects burst on.
	pub(super) takes: u64,
	/// The program's shape, width over height.
	pub(super) shape: f32,
	/// The scene cued, or open off air.
	pub(super) cued: Option<Cued>,
	/// Whether Studio mode is on: a scene's pad cues it.
	pub(super) studio: bool,
}

/// A scene as its pad shows it.
struct ScenePad {
	name: String,
	canvas: Canvas,
	/// Why it cannot go on program, if it cannot.
	refused: Option<String>,
}

/// A sound the stream carries, as its row shows it.
struct Sound {
	name: String,
	microphone: bool,
	audio: moq_scene::Audio,
}

/// Room around the bars' contents.
const MARGIN: f32 = 14.0;
/// Room between pads.
const GAP: f32 = 10.0;
/// How wide a pad is in a row of them.
const PAD: f32 = 124.0;
/// How far in from its edges a drag resizes the window, in points.
const EDGE: f32 = 5.0;
/// How long a message shows, and a failure.
const SAID: Duration = Duration::from_secs(4);
const FAILED: Duration = Duration::from_secs(10);
/// Room on the title bar's left for macOS's own window buttons.
const TRAFFIC_LIGHTS: f32 = 64.0;

/// What a pad asked for, naming its scene: the pads are drawn from the
/// frame's snapshot, and a reload since may have moved or removed it.
enum PadAction {
	Take(String),
	Cue(String),
	/// Open it for editing off air.
	Edit(String),
	/// Make it the safe scene, or stop.
	Safe(String, bool),
	Remove(String),
	/// Ask for a new name.
	Rename(String),
	/// Name it the second.
	Renamed(String, String),
	/// A new scene after it showing what it shows: the same sources, linked,
	/// or each its own copy.
	Duplicate {
		name: String,
		independent: bool,
	},
	/// Move it this many places along the strip.
	Move(String, isize),
	Add,
}

impl Panels {
	/// Follow the engine's state as of this frame, and copy what the title
	/// bar, the strip, and the live picture show, under one short lock.
	pub(super) fn snapshot(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> Snapshot {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		// The size the program was drawn at, which an edit this frame, such
		// as an undo resizing the canvas, does not change until the next.
		self.program_drawn = state.compositor.canvas();
		if let Some(render) = frame.wgpu_render_state() {
			self.program.follow(render, state.compositor.layered());
			self.preview.follow(render, state.compositor.preview());
			self.follow_thumbnails(render, &state.compositor);
		}
		// Redraw at the canvas rate, so the picture and the pads are live.
		let framerate = state.compositor.canvas().framerate;
		ctx.request_repaint_after(Duration::from_secs(1) / framerate);
		self.follow_reloads(&state);
		self.levels.take(&state.sources, state.collection());
		// A key bound to a scene follows it through renames, undos, and redos,
		// under this lock, so a key fired meanwhile names the scene it means.
		let renames = state.take_renames();
		if let Some(hotkeys) = &mut self.hotkeys {
			for renames in &renames {
				hotkeys.rename(renames);
			}
		}
		// So does the scene open off air, which stays open while it is cued.
		for renames in &renames {
			if let Some(to) = self.editing.as_ref().and_then(|editing| renames.get(editing)) {
				self.editing = to.clone();
			}
		}
		let cued = state.cued().map(str::to_string);
		if self.editing != cued {
			self.editing = None;
		}
		// So does a size being typed, a renamed scene's winning its new name.
		for renames in &renames {
			let pending = std::mem::take(&mut self.sizes);
			let (moved, kept): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(name, _)| renames.contains_key(name));
			let moved = moved
				.into_iter()
				.filter_map(|(name, size)| Some((renames[&name].clone()?, size)));
			self.sizes = moved.collect();
			for (name, size) in kept {
				self.sizes.entry(name).or_insert(size);
			}
		}
		// Here rather than in the Show tab, which can draw after a rename this
		// frame and before it is followed.
		let scenes = &state.collection().scenes;
		self.sizes
			.retain(|name, _| scenes.iter().any(|scene| scene.name == *name));
		// A page not painting through an X display copies through the
		// studio's clipboard, even if the picture let go of the page before
		// the copy came back.
		if let Some(text) = state
			.sources
			.context()
			.browser
			.as_ref()
			.and_then(|helper| helper.copied())
		{
			ctx.copy_text(text);
		}

		let collection = state.collection();
		let history = state.history();
		let position = history.position();
		let undo = position.checked_sub(1).and_then(|at| history.entries().nth(at));
		let redo = history.entries().nth(position);
		let scenes = collection
			.scenes
			.iter()
			.map(|scene| ScenePad {
				name: scene.name.clone(),
				canvas: scene.canvas(collection.canvas),
				refused: collection.refuses(&scene.name),
			})
			.collect();
		let stream = collection.program_audio().map(str::to_string);
		let heard = stream
			.as_deref()
			.map(|stream| collection.heard(stream))
			.unwrap_or_default();
		let sounds = heard
			.into_iter()
			.filter_map(|name| {
				let source = collection.sources.get(name)?;
				Some(Sound {
					name: name.to_string(),
					microphone: matches!(source, Source::Microphone { .. }),
					audio: collection.audio.get(name).cloned().unwrap_or_default(),
				})
			})
			.collect();
		let cued = cued.map(|name| Cued {
			refused: collection.refuses(&name),
			canvas: collection.canvas(Some(name.as_str())).unwrap_or(collection.canvas),
			off_air: self.editing.as_ref() == Some(&name),
			live: state.seen(&name),
			transition: transition_words(&collection.transition),
			name,
		});
		let path = self.engine.autosave.path();
		let show = path
			.file_stem()
			.unwrap_or(path.as_os_str())
			.to_string_lossy()
			.into_owned();
		Snapshot {
			show,
			undo: undo.map(|edit| edit.label.clone()),
			redo: redo.map(|edit| edit.label.clone()),
			program: state.compositor.program_scene().map(str::to_string),
			scenes,
			safe: collection.safe().map(str::to_string),
			sounds,
			stream,
			takes: state.takes,
			shape: self.program_drawn.width as f32 / self.program_drawn.height.max(1) as f32,
			cued,
			studio: self.controls.studio(),
		}
	}

	/// Draw the window: the title bar, the page, and the strip, in the
	/// arrangement that gives the picture the most room, and the windows and
	/// messages over them.
	pub(super) fn window(&mut self, ui: &mut Ui, fx: &mut Fx, snapshot: &Snapshot) {
		let window = ui.max_rect();
		let (size, shape, sounds) = (window.size(), snapshot.shape, snapshot.sounds.len());
		let layout = match self.page {
			// MIX puts a live picture in the sound's slot.
			Page::Mix => Layout::monitor(size, shape, sounds),
			_ => Layout::new(size, shape, sounds),
		};
		let layout = layout.translate(window.min.to_vec2());
		ui.painter().rect_filled(window, 0, kit::BG);
		self.page(ui, fx, snapshot, &layout);
		self.title(ui, fx, snapshot, &layout);
		if let Some(sidebar) = layout.sidebar {
			self.sidebar(ui, fx, sidebar);
		}
		self.strip(ui, fx, snapshot, &layout);
		self.banners(ui.ctx(), layout.stage);
		self.toast(ui.ctx(), layout.stage);
		self.settings(ui.ctx(), fx, layout.stage);
		self.details(ui.ctx(), layout.stage);
		self.going_sheet(ui.ctx());
		// macOS keeps its own frame's edges, and resizes by them.
		if self.framed && !cfg!(target_os = "macos") {
			edges(ui, window);
		}
	}

	/// The page in the stage's room.
	fn page(&mut self, ui: &mut Ui, fx: &mut Fx, snapshot: &Snapshot, layout: &Layout) {
		let stage = layout.stage;
		match self.page {
			Page::Live => {
				kit::wash(ui.painter(), stage);
				let picture = self.on_air(ui, Some(fx), layout.picture).rect;
				if let Some(cued) = &snapshot.cued {
					self.next(ui, fx, cued, picture, Corner::TopRight);
				}
			}
			Page::Build => self.build(ui, fx, stage, snapshot),
			// The sound slot holds the live picture instead.
			Page::Mix => region(ui, stage, |ui| {
				self.audio_scenes(ui);
				ui.separator();
				self.audio(ui);
			}),
			// The program's card in "replay what" is the live picture.
			Page::Review => self.review(ui, fx, stage, snapshot.shape),
		}
	}

	/// The live picture in its tally frame, filling `picture`.
	pub(super) fn on_air(&mut self, ui: &mut Ui, fx: Option<&mut Fx>, picture: Rect) -> egui::Response {
		let id = self.program.id;
		let canvas = self.program_drawn;
		let builder = UiBuilder::new().max_rect(picture.expand(10.0));
		let frame = self.program_frame();
		ui.scope_builder(builder, |ui| {
			frame.show(ui, fx, |ui| super::picture(ui, id, canvas, Sense::hover()))
		})
		.inner
	}

	/// The title bar: the show's menu, undo and redo, the pages, the health
	/// dot, record, the live control, Settings, and the window's buttons. Its
	/// empty room moves the window.
	fn title(&mut self, ui: &mut Ui, fx: &mut Fx, snapshot: &Snapshot, layout: &Layout) {
		let rect = layout.title;
		bar(ui, fx, rect);
		ui.painter()
			.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, kit::LINE));
		// Under every control on the bar, so each takes its own clicks.
		let bar = ui.interact(rect, ui.id().with("title bar"), Sense::click_and_drag());
		if bar.drag_started_by(egui::PointerButton::Primary) {
			ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
		}
		if bar.double_clicked() {
			let maximized = ui.ctx().input(|input| input.viewport().maximized.unwrap_or(false));
			ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
		}
		let mut inner = rect.shrink2(vec2(MARGIN, 0.0));
		let mac = cfg!(target_os = "macos");
		if mac && self.framed {
			inner.min.x += TRAFFIC_LIGHTS;
		}
		// Compact while stacked, or where the full controls last needed more
		// room than the bar has, such as for a long show name.
		let compact = layout.arrangement == Arrangement::Stacked || self.title_needs > inner.width();
		let builder = UiBuilder::new()
			.max_rect(inner)
			.layout(egui::Layout::left_to_right(Align::Center));
		let (left, right) = ui
			.scope_builder(builder, |ui| {
				ui.spacing_mut().item_spacing.x = 8.0;
				self.show_menu(ui, snapshot, compact);
				if !compact {
					ui.add_space(4.0);
					self.undo_redo(ui, snapshot);
				}
				let left = ui.min_rect().right();
				let right = ui
					.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
						if self.framed && !mac {
							window_buttons(ui);
							ui.add_space(4.0);
						}
						let open = self.setting.is_some();
						let gear = IconButton::new(Icon::Settings, "Settings").lit(open, kit::P3);
						if ui.add(gear).clicked() {
							self.setting = match open {
								true => None,
								false => Some(Setting::default()),
							};
						}
						self.live_control(ui);
						self.record_button(ui, fx);
						self.health_dot(ui, fx, compact);
						ui.min_rect().left()
					})
					.inner;
				(left, right)
			})
			.inner;
		let nav = match layout.sidebar {
			Some(_) => 0.0,
			None => nav_width(ui, compact) + 24.0,
		};
		if !compact {
			let needs = (left - inner.left()) + nav + (inner.right() - right);
			if needs > inner.width() {
				// Drawn compact from the next frame.
				ui.ctx().request_repaint();
			}
			self.title_needs = needs;
		}
		if layout.sidebar.is_none() {
			// Centered on the bar, or as near as the controls beside it allow.
			let width = nav_width(ui, compact);
			let center = rect
				.center()
				.x
				.min(right - 12.0 - width / 2.0)
				.max(left + 12.0 + width / 2.0);
			let at = Rect::from_center_size(pos2(center, rect.center().y), vec2(width, 32.0));
			self.nav(ui, at, compact);
		}
	}

	/// The logo and the show's name, which open the show's menu.
	fn show_menu(&mut self, ui: &mut Ui, snapshot: &Snapshot, compact: bool) {
		let name = (!compact).then(|| {
			ui.painter()
				.layout_no_wrap(snapshot.show.clone(), kit::ui_strong(14.0), kit::TEXT)
		});
		let width = 22.0 + name.as_ref().map_or(0.0, |name| 10.0 + name.size().x + 4.0 + 12.0);
		let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
		let response = kit::tip(response, "The show", None);
		let painter = ui.painter();
		let logo = Rect::from_min_size(pos2(rect.left(), rect.center().y - 11.0), Vec2::splat(22.0));
		painter.rect_filled(logo, 7, LOGO);
		painter.rect_filled(logo.shrink(5.0), 3, LOGO_INNER);
		if let Some(name) = name {
			let at = pos2(logo.right() + 10.0, rect.center().y - name.size().y / 2.0);
			let after = at.x + name.size().x + 4.0;
			painter.galley(at, name, kit::TEXT);
			let ink = if response.hovered() { kit::TEXT } else { kit::DIM };
			let chevron = Rect::from_min_size(pos2(after, rect.center().y - 6.0), Vec2::splat(12.0));
			Icon::Down.paint(ui, chevron, 12.0, ink);
		}
		kit::popover(&response).align(egui::RectAlign::BOTTOM_START).show(|ui| {
			kit::popover_row(ui, false, |ui| {
				ui.label(egui::RichText::new(self.engine.autosave.path().display().to_string()).color(kit::DIM));
			});
			if ui.add(kit::button(Icon::Reload, "Reload the scene file")).clicked() {
				let result = self.engine.reload();
				self.check(result);
			}
			if ui.add(kit::button(Icon::Copy, "Copy the selected layer")).clicked() {
				self.copy(ui.ctx());
			}
			if ui.button("Paste").clicked() {
				// Arrives as a paste event on a later frame.
				ui.ctx().send_viewport_cmd(ViewportCommand::RequestPaste);
			}
		});
	}

	/// Undo and redo, each naming the edit it would undo or redo.
	fn undo_redo(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
		let shortcut = |shift| {
			let modifiers = match shift {
				true => egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
				false => egui::Modifiers::COMMAND,
			};
			ui.ctx()
				.format_shortcut(&egui::KeyboardShortcut::new(modifiers, egui::Key::Z))
		};
		let (undo_key, redo_key) = (shortcut(false), shortcut(true));
		let undo = match &snapshot.undo {
			Some(label) => format!("Undo: {label}"),
			None => "Nothing to undo".into(),
		};
		let redo = match &snapshot.redo {
			Some(label) => format!("Redo: {label}"),
			None => "Nothing to redo".into(),
		};
		let button = IconButton::new(Icon::Undo, &undo).key(Some(&undo_key));
		if ui.add_enabled(snapshot.undo.is_some(), button).clicked() {
			self.undo();
		}
		let button = IconButton::new(Icon::Redo, &redo).key(Some(&redo_key));
		if ui.add_enabled(snapshot.redo.is_some(), button).clicked() {
			self.redo();
		}
		let any = snapshot.undo.is_some() || snapshot.redo.is_some();
		let history = ui.add_enabled(any, IconButton::new(Icon::History, "History"));
		egui::Popup::menu(&history)
			.id(egui::Id::new("history"))
			.width(280.0)
			.show(|ui| self.history(ui));
	}

	/// The pages, as one segmented control at `rect`.
	fn nav(&mut self, ui: &mut Ui, rect: Rect, compact: bool) {
		let painter = ui.painter();
		painter.rect(rect, 10, kit::P2, Stroke::new(1.0, kit::LINE), egui::StrokeKind::Inside);
		let (font, pad) = nav_font(compact);
		let mut x = rect.left() + 3.0;
		for page in Page::ALL {
			let galley = kit::caps_galley(ui.painter(), page.name(), font.clone(), kit::TEXT, None);
			let width = galley.size().x + pad * 2.0;
			let at = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(width, rect.height() - 6.0));
			x += width + 2.0;
			let response = ui.interact(at, ui.id().with(("page", page.name())), Sense::click());
			response.widget_info(|| {
				egui::WidgetInfo::selected(egui::WidgetType::Button, true, self.page == page, page.name())
			});
			let on = self.page == page;
			if on {
				ui.painter().rect_filled(at, 7, kit::P3);
			}
			let ink = if on || response.hovered() { kit::TEXT } else { kit::DIM };
			ui.painter().galley(at.center() - galley.size() / 2.0, galley, ink);
			if response.clicked() {
				self.page = page;
			}
		}
	}

	/// The pages, down a sidebar, as a portrait show has them.
	fn sidebar(&mut self, ui: &mut Ui, fx: &mut Fx, rect: Rect) {
		bar(ui, fx, rect);
		ui.painter()
			.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, kit::LINE));
		for (index, page) in Page::ALL.into_iter().enumerate() {
			let at = Rect::from_min_size(
				pos2(rect.center().x - 26.0, rect.top() + 10.0 + index as f32 * 58.0),
				vec2(52.0, 54.0),
			);
			let response = ui.interact(at, ui.id().with(("page", page.name())), Sense::click());
			response.widget_info(|| {
				egui::WidgetInfo::selected(egui::WidgetType::Button, true, self.page == page, page.name())
			});
			let on = self.page == page;
			if on {
				ui.painter().rect_filled(at, 10, kit::P3);
			}
			let ink = if on || response.hovered() { kit::TEXT } else { kit::DIM };
			let icon = Rect::from_center_size(pos2(at.center().x, at.top() + 18.0), Vec2::splat(18.0));
			page.icon().paint(ui, icon, 18.0, ink);
			let galley = kit::caps_galley(ui.painter(), page.name(), kit::display_bold(11.0), ink, None);
			let text = pos2(at.center().x - galley.size().x / 2.0, at.top() + 33.0);
			ui.painter().galley(text, galley, ink);
			if response.clicked() {
				self.page = page;
			}
		}
	}

	/// Start or stop recording, from any page, pulsing and showing its clock
	/// while it records.
	fn record_button(&mut self, ui: &mut Ui, fx: &mut Fx) {
		let status = self.engine.recorder.status().filter(|status| !status.done);
		let key = self.key(&Action::Record, ui.ctx());
		let elapsed = status
			.as_ref()
			.map(|status| super::review::clock(kio::clock::now().saturating_duration_since(status.started)));
		let time = elapsed
			.clone()
			.map(|elapsed| ui.painter().layout_no_wrap(elapsed, kit::mono(12.0), kit::TEXT));
		let width = time.as_ref().map_or(30.0, |time| 30.0 + time.size().x + 8.0);
		let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
		let name = match (&status, &elapsed) {
			(None, _) => "Start recording".to_string(),
			(Some(status), _) if status.stopping => "Finishing the recording".to_string(),
			(Some(_), elapsed) => format!("Stop recording, {} in", elapsed.as_deref().unwrap_or_default()),
		};
		let response = kit::tip(response, &name, key.as_deref());
		let painter = ui.painter();
		let recording = status.is_some();
		let (fill, stroke) = match (recording, response.hovered()) {
			(true, _) => (kit::ACCENT.gamma_multiply(0.25), kit::ACCENT),
			(false, true) => (kit::P3, kit::LINE),
			(false, false) => (kit::P2, kit::LINE),
		};
		painter.rect(
			rect,
			kit::RADIUS,
			fill,
			Stroke::new(1.0, stroke),
			egui::StrokeKind::Inside,
		);
		let pulse = (recording && fx.on()).then(|| Mark::new(ui.painter()));
		let dot = Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), Vec2::splat(9.0));
		ui.painter().circle_filled(dot.center(), 4.5, kit::ACCENT);
		if let Some(pulse) = pulse {
			fx.recording(&pulse.around(dot));
		}
		if let Some(time) = time {
			let at = pos2(dot.right() + 7.0, rect.center().y - time.size().y / 2.0);
			ui.painter().galley(at, time, kit::TEXT);
		}
		if response.clicked() {
			self.controls.press(&Action::Record);
		}
	}

	/// The strip: the scene pads, the panic key, and the sound, along the
	/// bottom, or in columns beside a portrait picture.
	fn strip(&mut self, ui: &mut Ui, fx: &mut Fx, snapshot: &Snapshot, layout: &Layout) {
		let line = Stroke::new(1.0, kit::LINE);
		match layout.arrangement {
			Arrangement::Wide | Arrangement::Stacked => {
				let strip = layout.scenes.union(layout.panic).union(layout.sound);
				bar(ui, fx, strip);
				let painter = ui.painter();
				painter.hline(strip.x_range(), strip.top() + 0.5, line);
				painter.vline(layout.panic.left() + 0.5, layout.panic.y_range(), line);
				match layout.arrangement {
					Arrangement::Wide => painter.vline(layout.sound.left() + 0.5, layout.sound.y_range(), line),
					_ => painter.hline(layout.sound.x_range(), layout.sound.top() + 0.5, line),
				};
			}
			Arrangement::Tall => {
				let column = layout.sound.union(layout.panic);
				bar(ui, fx, layout.scenes);
				bar(ui, fx, column);
				let painter = ui.painter();
				painter.vline(layout.scenes.right() - 0.5, layout.scenes.y_range(), line);
				painter.vline(column.left() + 0.5, column.y_range(), line);
			}
		}
		let tall = layout.arrangement == Arrangement::Tall;
		self.scenes_strip(ui, snapshot, layout.scenes, tall);
		self.panic_key(ui, snapshot, layout.panic, tall);
		match self.page {
			// MIX is the sound, so its slot holds a small live picture.
			Page::Mix => {
				let room = layout.sound.shrink(MARGIN + 10.0);
				self.on_air(ui, None, fit(room, snapshot.shape));
			}
			_ => self.sound(ui, snapshot, layout.sound, tall),
		}
	}

	/// A pad for each scene, then one that adds a scene: in a row, or two to
	/// a row down a `column`.
	fn scenes_strip(&mut self, ui: &mut Ui, snapshot: &Snapshot, rect: Rect, column: bool) {
		let room = rect.shrink(MARGIN);
		// Studio mode's switch at the strip's end: right of a row, under a
		// column.
		let (room, switch) = match column {
			true => (
				room.with_max_y(room.bottom() - 30.0 - GAP),
				Rect::from_min_size(pos2(room.right() - 30.0, room.bottom() - 30.0), Vec2::splat(30.0)),
			),
			false => (
				room.with_max_x(room.right() - 30.0 - GAP),
				Rect::from_center_size(pos2(room.right() - 15.0, room.center().y), Vec2::splat(30.0)),
			),
		};
		self.studio_switch(ui, snapshot.studio, switch);
		let mut action = None;
		ui.scope_builder(UiBuilder::new().max_rect(room), |ui| {
			ui.spacing_mut().item_spacing = Vec2::splat(GAP);
			if column {
				// Two to a row, or one in a narrow column.
				let across = if room.width() >= 2.0 * 84.0 + GAP { 2 } else { 1 };
				let width = ((room.width() - GAP * (across - 1) as f32) / across as f32).floor();
				egui::ScrollArea::vertical().id_salt("scenes").show(ui, |ui| {
					let count = snapshot.scenes.len() + 1;
					for row in 0..count.div_ceil(across) {
						ui.horizontal(|ui| {
							for index in row * across..(row + 1) * across {
								let pad = self.strip_pad(ui, snapshot, index, width);
								action = action.take().or(pad);
							}
						});
					}
				});
				return;
			}
			// As wide as fits the strip's height, at the show's shape.
			let tall = kit::Pad::height(PAD, snapshot.shape);
			let width = match tall > room.height() {
				true => ((room.height() - 33.0) * snapshot.shape + 8.0).floor().max(48.0),
				false => PAD,
			};
			egui::ScrollArea::horizontal().id_salt("scenes").show(ui, |ui| {
				ui.horizontal_centered(|ui| {
					for index in 0..=snapshot.scenes.len() {
						let pad = self.strip_pad(ui, snapshot, index, width);
						action = action.take().or(pad);
					}
				});
			});
		});
		let action = action.or_else(|| self.rename_sheet(ui.ctx()));
		let Some(action) = action else { return };
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let at = |state: &State, name: &str| {
			let scenes = &state.collection().scenes;
			let index = scenes.iter().position(|scene| scene.name == name);
			index.with_context(|| format!("no scene is called `{name}`"))
		};
		let spot = match &action {
			PadAction::Take(name)
			| PadAction::Cue(name)
			| PadAction::Edit(name)
			| PadAction::Safe(name, _)
			| PadAction::Remove(name)
			| PadAction::Rename(name)
			| PadAction::Renamed(name, _)
			| PadAction::Duplicate { name, .. }
			| PadAction::Move(name, _) => Some(Spot::Pad(name.clone())),
			PadAction::Add => None,
		};
		let result = match action {
			PadAction::Take(name) => crate::controls::take(&mut state, &name).map(drop),
			PadAction::Cue(name) => crate::controls::cue(&mut state, &name).map(drop),
			PadAction::Edit(name) => {
				drop(state);
				let result = self.open_off_air(&name);
				self.fail(Spot::Pad(name), result);
				return;
			}
			PadAction::Safe(name, safe) => {
				at(&state, &name).and_then(|index| self.scene_action(&mut state, SceneAction::Safe(index, safe)))
			}
			PadAction::Remove(name) => {
				at(&state, &name).and_then(|index| self.scene_action(&mut state, SceneAction::Remove(index)))
			}
			PadAction::Rename(name) => {
				self.renaming_pad = Some((name.clone(), name));
				Ok(())
			}
			PadAction::Renamed(name, to) => at(&state, &name).and_then(|index| {
				let to = to.trim().to_string();
				if to == name {
					return Ok(());
				}
				self.scene_action(&mut state, SceneAction::Rename(index, to))
			}),
			PadAction::Move(name, by) => at(&state, &name).and_then(|from| {
				let count = state.collection().scenes.len();
				let to = from.saturating_add_signed(by).min(count.saturating_sub(1));
				match to == from {
					true => Ok(()),
					false => self.scene_action(&mut state, SceneAction::Move(from, to)),
				}
			}),
			PadAction::Duplicate { name, independent } => at(&state, &name)
				.and_then(|index| duplicate_scene(&mut state, index, independent))
				.map(|copy| self.say(Ok(format!("added scene `{copy}`")))),
			PadAction::Add => {
				let collection = state.collection();
				let name = (collection.scenes.len() + 1..)
					.map(|number| format!("Scene {number}"))
					.find(|name| collection.scene(name).is_none())
					.expect("a free name");
				let index = collection.scenes.len();
				let result = edit(&mut state, format!("Add scene `{name}`"), None, |draft| {
					draft.insert_scene(index, Scene::new(name.clone()))
				});
				result.map(|()| self.say(Ok(format!("added scene `{name}`; take it to fill it in Build"))))
			}
		};
		drop(state);
		match spot {
			Some(spot) => self.fail(spot, result),
			None => self.check(result),
		}
	}

	/// The sheet renaming a scene, while one is renamed, and the rename it
	/// asked for.
	fn rename_sheet(&mut self, ctx: &egui::Context) -> Option<PadAction> {
		let (from, mut to) = self.renaming_pad.clone()?;
		let mut asked = None;
		let title = format!("Rename {from}");
		let closed = kit::Sheet::new(egui::Id::new("rename scene"), &title).show(ctx, |ui| {
			let field = ui.add(egui::TextEdit::singleline(&mut to).desired_width(f32::INFINITY));
			if !field.has_focus() && !field.lost_focus() {
				field.request_focus();
			}
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			ui.add_space(8.0);
			ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
				if ui.button("Rename").clicked() || entered {
					asked = Some(PadAction::Renamed(from.clone(), to.clone()));
				}
			});
		});
		self.renaming_pad = match closed || asked.is_some() {
			true => None,
			false => Some((from, to)),
		};
		asked
	}

	/// The pad of the scene at `index`, or the one that adds a scene past
	/// the last, `width` wide, and what it asked for.
	fn strip_pad(&mut self, ui: &mut Ui, snapshot: &Snapshot, index: usize, width: f32) -> Option<PadAction> {
		let Some(pad) = snapshot.scenes.get(index) else {
			if index > snapshot.scenes.len() {
				return None;
			}
			let size = vec2(width, kit::Pad::height(width, snapshot.shape));
			let (rect, response) = ui.allocate_exact_size(size, Sense::click());
			let response = kit::tip(response, "Add a scene", None);
			let ink = if response.hovered() { kit::TEXT } else { kit::FAINT };
			dashed(ui.painter(), rect.shrink(1.0), Stroke::new(1.5, ink));
			Icon::Add.paint(ui, rect, 18.0, ink);
			return response.clicked().then_some(PadAction::Add);
		};
		let on_air = snapshot.program.as_deref() == Some(pad.name.as_str());
		let cued = snapshot.cued.as_ref().is_some_and(|cued| cued.name == pad.name);
		// Off air, the scene on program is the one the stream opens on.
		let live = self.engine.on_air().live().is_some();
		let (tally, tag) = match (on_air, live, cued) {
			(true, true, _) => (Tally::Air, None),
			(true, false, _) => (Tally::Cue, Some("First")),
			(false, _, true) => (Tally::Cue, None),
			(false, _, false) => (Tally::Off, None),
		};
		let response = self.pad(ui, pad, tally, tag, width, snapshot.shape);
		let click = match snapshot.studio {
			true => PadAction::Cue(pad.name.clone()),
			false => PadAction::Take(pad.name.clone()),
		};
		let mut action = (response.clicked() && pad.refused.is_none()).then_some(click);
		response.context_menu(|ui| {
			let takeable = pad.refused.is_none() || on_air;
			let take = ui.add_enabled(!on_air && pad.refused.is_none(), egui::Button::new("Take it on air"));
			if take.clicked() {
				action = Some(PadAction::Take(pad.name.clone()));
			}
			if snapshot.studio {
				let cue = ui.add_enabled(!on_air && !cued && pad.refused.is_none(), egui::Button::new("Cue it"));
				if cue.clicked() {
					action = Some(PadAction::Cue(pad.name.clone()));
				}
			}
			let edit = ui
				.add_enabled(!on_air, egui::Button::new("Edit off air"))
				.on_hover_text("Open it on Build without going on air")
				.on_disabled_hover_text("On air; Build edits it live");
			if edit.clicked() {
				action = Some(PadAction::Edit(pad.name.clone()));
			}
			let mut safe = snapshot.safe.as_deref() == Some(pad.name.as_str());
			let toggle = ui
				.add_enabled(takeable || safe, kit::Toggle::new(&mut safe).label("Safe scene"))
				.on_hover_text(
					"The scene the panic key cuts to, as it mutes every microphone and guest and hides the always-on-top layers",
				);
			if toggle.changed() {
				action = Some(PadAction::Safe(pad.name.clone(), safe));
			}
			if ui.button("Rename").clicked() {
				action = Some(PadAction::Rename(pad.name.clone()));
			}
			let duplicate = ui
				.button("Duplicate")
				.on_hover_text("A new scene showing the same sources: editing a layer's look edits both");
			if duplicate.clicked() {
				action = Some(PadAction::Duplicate {
					name: pad.name.clone(),
					independent: false,
				});
			}
			let own = ui
				.button("Duplicate with its own copies")
				.on_hover_text("A new scene whose layers each show a copy of their own, to edit apart");
			if own.clicked() {
				action = Some(PadAction::Duplicate {
					name: pad.name.clone(),
					independent: true,
				});
			}
			let count = snapshot.scenes.len();
			if ui.add_enabled(index > 0, egui::Button::new("Move left")).clicked() {
				action = Some(PadAction::Move(pad.name.clone(), -1));
			}
			if ui
				.add_enabled(index + 1 < count, egui::Button::new("Move right"))
				.clicked()
			{
				action = Some(PadAction::Move(pad.name.clone(), 1));
			}
			let remove = ui
				.add_enabled(!on_air, egui::Button::new("Remove"))
				.on_disabled_hover_text("On air; take another scene first");
			if remove.clicked() {
				action = Some(PadAction::Remove(pad.name.clone()));
			}
			if action.is_some() {
				ui.close();
			}
		});
		action
	}

	/// A scene's pad, `width` wide, with its thumbnail once the compositor
	/// has drawn it, which it draws while the pad is on screen.
	fn pad(
		&mut self,
		ui: &mut Ui,
		pad: &ScenePad,
		tally: Tally,
		tag: Option<&str>,
		width: f32,
		shape: f32,
	) -> egui::Response {
		let thumbnail = Thumbnail::Scene(pad.name.clone());
		let size = vec2(pad.canvas.width as f32, pad.canvas.height as f32);
		let picture = self
			.thumbnails
			.get(&thumbnail)
			.map(|registered| egui::load::SizedTexture::new(registered.id, size));
		let key = self.key(&Action::Scene(pad.name.clone()), ui.ctx());
		let fault = self.fault(&Spot::Pad(pad.name.clone()));
		let widget = kit::Pad::new(&pad.name)
			.picture(picture)
			.key(key.as_deref())
			.tally(tally)
			.tag(tag)
			.refused(pad.refused.as_deref())
			.fault(fault)
			.width(width)
			.shape(shape);
		let response = ui.add(widget);
		if ui.is_rect_visible(response.rect) {
			self.thumbnails_shown.insert(thumbnail);
		}
		response
	}

	/// The key bound to `action`, as its tooltip shows it.
	pub(super) fn key(&self, action: &Action, ctx: &egui::Context) -> Option<String> {
		let hotkeys = self.hotkeys.as_ref()?;
		let bindings = hotkeys.bindings();
		Some(bindings.get(action)?.shown(ctx))
	}

	/// The panic key, named for the scene it cuts to, and saving a clip.
	fn panic_key(&mut self, ui: &mut Ui, snapshot: &Snapshot, rect: Rect, tall: bool) {
		let room = rect.shrink(MARGIN);
		let note = match &snapshot.safe {
			Some(safe) => format!("Cut to `{safe}`, mute every microphone and guest, and hide the always-on-top layers"),
			None => "Mute every microphone and guest and hide the always-on-top layers. Mark a scene safe in its pad's menu to cut to it too.".into(),
		};
		let key = self.key(&Action::Panic, ui.ctx());
		let clip_key = self.key(&Action::SaveClip, ui.ctx());
		let text = snapshot.safe.as_deref().unwrap_or("Panic");
		let panic = kit::Quick::new(Icon::Panic, text, "Panic", room.width())
			.look(kit::Look::Cue)
			.key(key.as_deref())
			.note(Some(&note));
		let clip = IconButton::new(Icon::Clip, "Save clip")
			.look(kit::Look::Boxed)
			.key(clip_key.as_deref())
			.note(Some(
				"Save the last seconds, as many as REVIEW picks, as a clip into the recordings folder",
			));
		let (panic_at, clip_at) = match tall {
			true => (
				Rect::from_min_size(room.min, vec2(room.width(), 30.0)),
				Rect::from_min_size(room.min + vec2(0.0, 38.0), Vec2::splat(30.0)),
			),
			false => (
				Rect::from_center_size(room.center() - vec2(0.0, 19.0), vec2(room.width(), 30.0)),
				Rect::from_center_size(room.center() + vec2(0.0, 19.0), Vec2::splat(30.0)),
			),
		};
		if ui.put(panic_at, panic).clicked() {
			self.controls.press(&Action::Panic);
		}
		if ui.put(clip_at, clip).clicked() {
			self.controls.press(&Action::SaveClip);
		}
	}

	/// A row for each sound the stream carries, with its meter and mute, and
	/// the stream's own meter, with Listen.
	fn sound(&mut self, ui: &mut Ui, snapshot: &Snapshot, rect: Rect, tall: bool) {
		let room = rect.shrink2(vec2(MARGIN, 10.0));
		let (rows, stream) = match tall {
			// A caption over the rows, and the stream's meter as the last row.
			true => {
				let caption = Rect::from_min_size(room.min, vec2(room.width(), 18.0));
				ui.painter().galley(
					caption.left_top(),
					kit::caps_galley(ui.painter(), "Sound", kit::display(11.0), kit::DIM, None),
					kit::DIM,
				);
				let rows = room.with_min_y(caption.bottom() + 4.0);
				let count = snapshot.sounds.len().max(1) as f32;
				let bottom = (rows.top() + count * SOUND_ROW).min(rows.bottom() - SOUND_ROW - 8.0);
				let stream = Rect::from_min_size(pos2(rows.left(), bottom + 8.0), vec2(rows.width(), SOUND_ROW));
				(rows.with_max_y(bottom), stream)
			}
			// The stream's meter in a column of its own beside the rows.
			false => (
				room.with_max_x(room.right() - 46.0),
				room.with_min_x(room.right() - 34.0),
			),
		};
		let mut mute = None;
		ui.scope_builder(UiBuilder::new().max_rect(rows), |ui| {
			egui::ScrollArea::vertical().id_salt("sound").show(ui, |ui| {
				ui.spacing_mut().item_spacing.y = 0.0;
				if snapshot.sounds.is_empty() {
					ui.label(egui::RichText::new("Nothing on the stream makes a sound yet").color(kit::DIM));
				}
				for sound in &snapshot.sounds {
					let size = vec2(ui.available_width(), SOUND_ROW);
					ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(Align::Center), |ui| {
						ui.spacing_mut().item_spacing.x = 8.0;
						if self.sound_row(ui, sound) {
							mute = Some(sound);
						}
					});
				}
			});
		});
		if let Some(sound) = mute {
			let muted = !sound.audio.muted;
			let verb = if muted { "Mute" } else { "Unmute" };
			let audio = moq_scene::Audio {
				muted,
				..sound.audio.clone()
			};
			let engine = self.engine.clone();
			let mut state = engine.state.lock().unwrap();
			let label = format!("{verb} `{}`", sound.name);
			let result = edit(&mut state, label, None, |draft| {
				draft.set_audio(&sound.name, Some(audio))
			});
			drop(state);
			self.check(result);
		}
		self.stream(ui, snapshot, stream, tall);
	}

	/// A sound's icon, name, meter, and mute, saying whether the mute was
	/// clicked.
	fn sound_row(&self, ui: &mut Ui, sound: &Sound) -> bool {
		let muted = sound.audio.muted;
		let icon = match (sound.microphone, muted) {
			(true, false) => Icon::Mic,
			(true, true) => Icon::MicMuted,
			(false, false) => Icon::Sound,
			(false, true) => Icon::Muted,
		};
		let ink = if muted { kit::DIM } else { kit::TEXT };
		let (at, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
		icon.paint(ui, at, 15.0, kit::DIM);
		let name = (ui.available_width() * 0.3).clamp(64.0, 120.0);
		let label = egui::Label::new(egui::RichText::new(&sound.name).font(kit::ui(12.5)).color(ink)).truncate();
		let left = egui::Layout::left_to_right(Align::Center);
		ui.allocate_ui_with_layout(vec2(name, SOUND_ROW), left, |ui| {
			ui.set_min_width(name);
			ui.add(label);
		});
		let (level, peak) = match muted {
			true => (0.0, 0.0),
			false => reading(self.levels.sound(&sound.name)),
		};
		let along = (ui.available_width() - 24.0 - 8.0).max(16.0);
		ui.add(kit::Meter::new(level, peak).run(kit::Run::Across, along, 9.0));
		let name = if muted { "Unmute" } else { "Mute" };
		let icon = if sound.microphone { Icon::Mic } else { Icon::Sound };
		let button = IconButton::new(if muted { Icon::MicMuted } else { icon }, name)
			.lit(muted, kit::CUE)
			.look(kit::Look::Boxed)
			.small();
		ui.add(button).clicked()
	}

	/// The stream's meter, the mix of the audio scene it carries, with Listen.
	fn stream(&mut self, ui: &mut Ui, snapshot: &Snapshot, rect: Rect, row: bool) {
		let Some(stream) = &snapshot.stream else { return };
		let (level, peak) = reading(self.levels.mix(stream));
		let listening = self.engine.listening();
		let heard = listening.as_ref().is_some_and(|(heard, _)| heard == stream);
		let note = match mix::MONITOR {
			true => "Play the stream's sound to the speakers; use headphones",
			false => "Needs a build with the `capture` feature",
		};
		let name = if heard {
			"Stop listening"
		} else {
			"Listen to the stream"
		};
		let listen = IconButton::new(Icon::Listen, name)
			.lit(heard, kit::ACCENT)
			.small()
			.note(Some(note));
		let caption = kit::caps_galley(ui.painter(), "Stream", kit::display(10.0), kit::DIM, None);
		let clicked = match row {
			true => {
				let caption_at = pos2(rect.left(), rect.center().y - caption.size().y / 2.0);
				let meter_left = rect.left() + caption.size().x + 8.0;
				ui.painter().galley(caption_at, caption, kit::DIM);
				let meter = Rect::from_min_max(
					pos2(meter_left, rect.center().y - 4.5),
					pos2(rect.right() - 32.0, rect.center().y + 4.5),
				);
				let along = meter.width().max(16.0);
				ui.put(meter, kit::Meter::new(level, peak).run(kit::Run::Across, along, 9.0));
				let button = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), Vec2::splat(24.0));
				ui.add_enabled_ui(mix::MONITOR, |ui| ui.put(button, listen)).inner
			}
			false => {
				let caption_at = pos2(rect.center().x - caption.size().x / 2.0, rect.top());
				ui.painter().galley(caption_at, caption, kit::DIM);
				let meter = Rect::from_min_max(
					pos2(rect.center().x - 7.0, rect.top() + 16.0),
					pos2(rect.center().x + 7.0, rect.bottom() - 30.0),
				);
				let along = meter.height().max(16.0);
				ui.put(meter, kit::Meter::new(level, peak).run(kit::Run::Up, along, 14.0));
				let button = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 12.0), Vec2::splat(24.0));
				ui.add_enabled_ui(mix::MONITOR, |ui| ui.put(button, listen)).inner
			}
		};
		if !clicked.clicked() {
			return;
		}
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let result = self.listen(&mut state, (!heard).then_some(stream.as_str()));
		drop(state);
		self.check(result);
	}

	/// An outside edit to the scene file the studio has not taken, with the
	/// choice of which version to keep, and a save that failed, over the top
	/// of the stage until they are settled.
	fn banners(&mut self, ctx: &egui::Context, stage: Rect) {
		let mut top = stage.top() + 12.0;
		let width = (stage.width() - 40.0).min(760.0);
		let path = self.engine.autosave.path().display().to_string();
		if let Some(conflict) = self.engine.autosave.conflict() {
			let why = match &conflict.refused {
				None => "while the studio has unsaved edits".to_string(),
				Some(refused) => format!("and cannot be loaded: {refused}"),
			};
			let shown = banner(ctx, "conflict", pos2(stage.center().x, top), width, kit::CUE, |ui| {
				ui.label(format!(
					"{path} changed outside the studio {why}. Nothing is saved until you pick a version."
				));
				ui.horizontal(|ui| {
					if ui.button("Keep the studio's").clicked() {
						let result = self.engine.keep();
						self.report(result.map(|()| format!("saved the studio's version over {path}")));
					}
					if ui.button("Take the file's").clicked() {
						let result = self.engine.reload();
						self.check(result);
					}
				});
			});
			top = shown.bottom() + 8.0;
		}
		if let Some(error) = self.engine.autosave.error() {
			banner(ctx, "not saved", pos2(stage.center().x, top), width, kit::ERR, |ui| {
				ui.label(format!("{path} is not saved: {error}"));
			});
		}
	}

	/// The outcome of the last action, at the bottom of the stage for a
	/// while, longer for a failure.
	fn toast(&mut self, ctx: &egui::Context, stage: Rect) {
		let Some((message, at)) = &self.message else { return };
		let (text, color, lasts) = match message {
			Ok(text) => (text.clone(), kit::TEXT, SAID),
			Err(text) => (text.clone(), kit::ERR, FAILED),
		};
		if kio::clock::now().saturating_duration_since(*at) > lasts {
			self.message = None;
			return;
		}
		let mut closed = false;
		egui::Area::new(egui::Id::new("message"))
			.order(egui::Order::Foreground)
			.pivot(Align2::CENTER_BOTTOM)
			.fixed_pos(pos2(stage.center().x, stage.bottom() - 16.0))
			.show(ctx, |ui| {
				kit::popover_frame()
					.inner_margin(egui::Margin::symmetric(14, 8))
					.show(ui, |ui| {
						ui.set_max_width((stage.width() - 40.0).min(720.0));
						ui.horizontal(|ui| {
							ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
							closed = ui.add(IconButton::new(Icon::Close, "Dismiss").small()).clicked();
						});
					});
			});
		if closed {
			self.message = None;
		}
	}

	/// The Settings window, while open.
	fn settings(&mut self, ctx: &egui::Context, fx: &mut Fx, stage: Rect) {
		let Some(mut setting) = self.setting else { return };
		let mut open = true;
		egui::Window::new("Settings")
			.id(egui::Id::new("settings"))
			.open(&mut open)
			.frame(window_frame())
			.collapsible(false)
			.pivot(Align2::CENTER_TOP)
			.default_pos(stage.center_top() + vec2(0.0, MARGIN))
			.default_size(vec2(900.0, 620.0).min(stage.size() - Vec2::splat(2.0 * MARGIN)))
			.show(ctx, |ui| {
				tabs(ui, &mut setting, &Setting::ALL, Setting::name);
				ui.add_space(6.0);
				match setting {
					Setting::Show => {
						egui::ScrollArea::vertical().show(ui, |ui| self.output(ui));
					}
					Setting::Stream => {
						egui::ScrollArea::vertical().show(ui, |ui| self.stream_settings(ui));
					}
					Setting::Recording => {
						egui::ScrollArea::vertical().show(ui, |ui| self.recording(ui));
					}
					Setting::Hotkeys => {
						egui::ScrollArea::vertical().show(ui, |ui| self.hotkeys(ui, fx));
					}
					Setting::Effects => fx.menu(ui),
					Setting::Advanced => self.graph(ui),
				}
			});
		self.setting = open.then_some(setting);
	}

	/// The stream's health in detail, while open.
	fn details(&mut self, ctx: &egui::Context, stage: Rect) {
		if !self.details {
			return;
		}
		let mut open = true;
		egui::Window::new("Health")
			.id(egui::Id::new("health"))
			.open(&mut open)
			.frame(window_frame())
			.collapsible(false)
			.pivot(Align2::CENTER_TOP)
			.default_pos(stage.center_top() + vec2(0.0, MARGIN))
			.default_size(vec2(640.0, 520.0).min(stage.size() - Vec2::splat(2.0 * MARGIN)))
			.show(ctx, |ui| self.health(ui));
		self.details = open;
	}

	/// Show each thumbnail the compositor drew of those shown last frame, and
	/// let go of the rest, so this frame starts with none shown.
	pub(super) fn follow_thumbnails(
		&mut self,
		render: &eframe::egui_wgpu::RenderState,
		compositor: &moq_scene::Compositor,
	) {
		let mut gone = std::mem::take(&mut self.thumbnails);
		for thumbnail in std::mem::take(&mut self.thumbnails_shown) {
			let Some(texture) = compositor.thumbnail(&thumbnail) else {
				continue;
			};
			let registered = match gone.remove(&thumbnail) {
				Some(mut registered) => {
					registered.follow(render, texture);
					registered
				}
				None => Registered::new(render, texture),
			};
			self.thumbnails.insert(thumbnail, registered);
		}
		let mut renderer = render.renderer.write();
		for registered in gone.into_values() {
			renderer.free_texture(&registered.id);
		}
	}

	/// How the stream reaches viewers: what is wrong and why, each relay, the
	/// uplink, each track, and each viewer that reports.
	pub(super) fn health(&mut self, ui: &mut Ui) {
		repairs(ui, &self.engine.recorder.repairs());
		let report = self.engine.health.lock().unwrap();
		if report.relays.is_empty() {
			ui.label("Off air. Once live, this shows how the stream reaches viewers.");
			for problem in &report.problems {
				ui.colored_label(color(problem.verdict), &problem.cause);
			}
			return;
		};

		egui::ScrollArea::vertical().show(ui, |ui| {
			if report.problems.is_empty() {
				ui.colored_label(kit::OK, "No problems.");
			}
			for problem in &report.problems {
				ui.colored_label(color(problem.verdict), &problem.cause);
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
						health::State::Connected => ui.colored_label(kit::OK, "connected"),
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
				ui.label(or_unknown(uplink.media.map(health::rate)))
					.on_hover_text("By every broadcast: the program's and each published scene's");
				ui.end_row();
				ui.label("Lost and resent");
				ui.label(or_unknown(uplink.loss.map(|loss| format!("{:.2}%", loss * 100.0))));
				ui.end_row();
			});

			// Each broadcast, with its own viewers beneath it.
			for broadcast in &report.broadcasts {
				ui.separator();
				if report.broadcasts.len() > 1 {
					match &broadcast.name {
						Some(name) => ui.strong(format!("Scene `{name}`")),
						None => ui.strong("Program"),
					};
				}
				broadcast_health(ui, broadcast);
			}
			if !report.sources.is_empty() {
				ui.separator();
				ui.strong("Sources");
				tracks_health(ui, "sources", &report.sources, true);
			}
		});
	}
}

/// The logo's colors.
const LOGO: Color32 = Color32::from_rgb(0xff, 0x4a, 0x3d);
const LOGO_INNER: Color32 = Color32::from_rgb(0xff, 0x8a, 0x3d);

/// A bar's or column's fill, with the effects' faint grain over it, under
/// whatever is drawn on it next.
pub(super) fn bar(ui: &Ui, fx: &mut Fx, rect: Rect) {
	ui.painter().rect_filled(rect, 0, kit::P1);
	if fx.on() {
		let mark = Mark::new(ui.painter());
		fx.panel(&mark.around(rect));
	}
}

/// A page's room: what `add` draws, scrolled, inside `rect`.
fn region(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
	ui.scope_builder(UiBuilder::new().max_rect(rect.shrink(MARGIN + 4.0)), |ui| {
		egui::ScrollArea::both()
			.id_salt(("page", rect.min.x as i32))
			.auto_shrink(false)
			.show(ui, add);
	});
}

/// The pages' font and the room either side of each name, smaller when
/// `compact`.
fn nav_font(compact: bool) -> (egui::FontId, f32) {
	match compact {
		true => (kit::display_bold(12.0), 9.0),
		false => (kit::display_bold(14.0), 14.0),
	}
}

/// How wide the pages' control is.
fn nav_width(ui: &Ui, compact: bool) -> f32 {
	let (font, pad) = nav_font(compact);
	let names: f32 = Page::ALL
		.iter()
		.map(|page| {
			kit::caps_galley(ui.painter(), page.name(), font.clone(), kit::TEXT, None)
				.size()
				.x + pad * 2.0
		})
		.sum();
	names + 2.0 * (Page::ALL.len() - 1) as f32 + 6.0
}

/// The window's minimize, maximize, and close, right to left, which the
/// desktop draws none of since the title bar is the studio's.
fn window_buttons(ui: &mut Ui) {
	let maximized = ui.ctx().input(|input| input.viewport().maximized.unwrap_or(false));
	if ui.add(IconButton::new(Icon::Close, "Close").small()).clicked() {
		ui.ctx().send_viewport_cmd(ViewportCommand::Close);
	}
	let name = if maximized { "Restore" } else { "Maximize" };
	if ui.add(IconButton::new(Icon::Maximize, name).small()).clicked() {
		ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
	}
	if ui.add(IconButton::new(Icon::Minimize, "Minimize").small()).clicked() {
		ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
	}
}

/// Resize the window by dragging its edges and corners, which the desktop
/// draws none of. Last, so they take the pointer from what lies under them.
fn edges(ui: &mut Ui, window: Rect) {
	let viewport = ui.ctx().input(|input| input.viewport().clone());
	if viewport.maximized.unwrap_or(false) || viewport.fullscreen.unwrap_or(false) {
		return;
	}
	let corner = EDGE * 3.0;
	let (min, max) = (window.min, window.max);
	let edges = [
		(
			ResizeDirection::North,
			pos2(min.x + corner, min.y),
			pos2(max.x - corner, min.y + EDGE),
		),
		(
			ResizeDirection::South,
			pos2(min.x + corner, max.y - EDGE),
			pos2(max.x - corner, max.y),
		),
		(
			ResizeDirection::West,
			pos2(min.x, min.y + corner),
			pos2(min.x + EDGE, max.y - corner),
		),
		(
			ResizeDirection::East,
			pos2(max.x - EDGE, min.y + corner),
			pos2(max.x, max.y - corner),
		),
		(ResizeDirection::NorthWest, min, pos2(min.x + corner, min.y + corner)),
		(
			ResizeDirection::NorthEast,
			pos2(max.x - corner, min.y),
			pos2(max.x, min.y + corner),
		),
		(
			ResizeDirection::SouthWest,
			pos2(min.x, max.y - corner),
			pos2(min.x + corner, max.y),
		),
		(ResizeDirection::SouthEast, pos2(max.x - corner, max.y - corner), max),
	];
	for (index, (direction, from, to)) in edges.into_iter().enumerate() {
		let rect = Rect::from_min_max(from, to);
		let response = ui.interact(rect, ui.id().with(("edge", index)), Sense::drag());
		if response.hovered() || response.dragged() {
			ui.ctx().set_cursor_icon(match direction {
				ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
				ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
				ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
				ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
			});
		}
		if response.drag_started() {
			ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(direction));
		}
	}
}

/// `elapsed` as a clock, hours first.
pub(super) fn clock(elapsed: Duration) -> String {
	let seconds = elapsed.as_secs();
	format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

/// A dashed outline around `rect`, as the pad that adds a scene has.
pub(crate) fn dashed(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
	let corners = [
		rect.left_top(),
		rect.right_top(),
		rect.right_bottom(),
		rect.left_bottom(),
		rect.left_top(),
	];
	for side in corners.windows(2) {
		painter.extend(egui::Shape::dashed_line(side, stroke, 5.0, 4.0));
	}
}

/// A banner over the stage, hanging from `top`, outlined in `color`, holding
/// what `add` draws; where it was drawn.
fn banner(
	ctx: &egui::Context,
	id: &str,
	top: egui::Pos2,
	width: f32,
	color: Color32,
	add: impl FnOnce(&mut Ui),
) -> Rect {
	egui::Area::new(egui::Id::new(("banner", id)))
		.order(egui::Order::Foreground)
		.pivot(Align2::CENTER_TOP)
		.fixed_pos(top)
		.show(ctx, |ui| {
			kit::popover_frame()
				.stroke(Stroke::new(1.0, color))
				.inner_margin(egui::Margin::symmetric(14, 10))
				.show(ui, |ui| {
					ui.set_max_width(width);
					add(ui)
				})
		})
		.response
		.rect
}

/// A window over the pages, such as Settings.
fn window_frame() -> egui::Frame {
	kit::popover_frame().inner_margin(egui::Margin::same(14))
}

/// A row of tabs, `current` underlined, each of `all` named by `name`.
pub(super) fn tabs<T: Copy + PartialEq, S: AsRef<str>>(ui: &mut Ui, current: &mut T, all: &[T], name: impl Fn(T) -> S) {
	ui.horizontal_wrapped(|ui| {
		ui.spacing_mut().item_spacing.x = 16.0;
		for &tab in all {
			let on = *current == tab;
			let text = name(tab);
			let text = text.as_ref();
			let galley = kit::caps_galley(ui.painter(), text, kit::display(14.0), kit::TEXT, None);
			let (rect, response) = ui.allocate_exact_size(galley.size() + vec2(0.0, 10.0), Sense::click());
			response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, text));
			let ink = if on || response.hovered() { kit::TEXT } else { kit::DIM };
			ui.painter().galley(rect.left_top(), galley, ink);
			if on {
				ui.painter()
					.hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(2.0, kit::TEXT));
			}
			if response.clicked() {
				*current = tab;
			}
		}
	});
	let y = ui.cursor().top();
	ui.painter()
		.hline(ui.max_rect().x_range(), y, Stroke::new(1.0, kit::LINE));
}
