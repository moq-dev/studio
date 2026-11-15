//! Settings: what a broadcaster sets once, such as the canvas and the
//! hotkeys, and the edit history.

use super::*;

impl Panels {
	/// Every edit, oldest first; pick one to undo or redo to just after it.
	pub(super) fn history(&mut self, ui: &mut Ui) {
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

	pub(super) fn output(&mut self, ui: &mut Ui) {
		self.canvas_form(ui);
		self.sizes_form(ui);
		ui.separator();

		let stats = &self.engine.stats;
		let count = |value: &std::sync::atomic::AtomicU64| value.load(Ordering::Relaxed);
		let watched = stats.video.watched.load(Ordering::Relaxed);

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
			ui.label(stats.video.encoder().unwrap_or_else(|| "not open".into()));
			ui.end_row();
			ui.label("Encoder sessions");
			ui.label(format!("{} open", stats.sessions.open())).on_hover_text(
				"Video encoders open now: the program's, each published scene's, the recording's while recording, \
				 and the sources'. A GPU caps them.",
			);
			ui.end_row();
		});
		ui.separator();
		renditions(ui, None, &stats.video, false);
		if let Some(error) = stats.error() {
			ui.colored_label(ui.visuals().error_fg_color, error);
		}
		renditions(ui, None, &stats.recording, true);
		if let Some(error) = stats.recording.error() {
			ui.colored_label(ui.visuals().warn_fg_color, format!("recording {error}"));
		}
		if let Some(error) = stats.recording_audio_error() {
			ui.colored_label(ui.visuals().warn_fg_color, format!("recording audio off: {error}"));
		}

		for (name, video) in stats.published() {
			ui.separator();
			ui.strong(format!("Scene `{name}`"));
			if let Some(output) = &self.engine.output {
				let state = self.engine.state.lock().unwrap();
				let audio = state
					.collection()
					.published_audio(&name)
					.unwrap_or_default()
					.to_string();
				drop(state);
				ui.label(format!("Broadcast {}/{name}, with audio scene `{audio}`", output.path));
			}
			ui.label(format!(
				"Encoder: {}",
				video.encoder().unwrap_or_else(|| "not open".into())
			));
			renditions(ui, Some(&name), &video, false);
			if let Some(error) = video.error() {
				ui.colored_label(ui.visuals().error_fg_color, error);
			}
			if let Some(error) = video.audio_error() {
				ui.colored_label(ui.visuals().error_fg_color, format!("audio stopped: {error}"));
			}
			self.camera(ui, Some(&name));
		}

		ui.separator();
		self.camera(ui, None);
	}

	/// The virtual camera of the program, or of the published scene `scene`:
	/// a toggle, and how it is doing while on.
	pub(super) fn camera(&mut self, ui: &mut Ui, scene: Option<&str>) {
		let camera = self.engine.state.lock().unwrap().camera(scene);
		let mut on = camera.is_some();
		let (shows, named) = match scene {
			None => ("the program".to_string(), "virtual camera".to_string()),
			Some(scene) => (format!("scene `{scene}`"), format!("scene `{scene}`'s virtual camera")),
		};
		if ui
			.add(kit::Toggle::new(&mut on).label("Virtual camera"))
			.on_hover_text(format!("Show {shows} as a camera that video-call apps can pick"))
			.changed()
		{
			let result = self.engine.set_camera(scene, on);
			let message = format!("{named} {}", if on { "on" } else { "off" });
			self.report(result.map(|()| message));
		}
		let Some(camera) = camera else { return };
		egui::Grid::new(("camera", scene)).show(ui, |ui| {
			ui.label("Camera");
			ui.label(&camera.backend);
			ui.end_row();
			ui.label("Picture");
			let canvas = camera.canvas;
			ui.label(format!(
				"{}x{} at {} fps",
				canvas.width, canvas.height, canvas.framerate
			));
			ui.end_row();
			ui.label("Apps");
			ui.label(if camera.reading { "reading" } else { "none; not copying" });
			ui.end_row();
			ui.label("Sent");
			ui.label(format!("{} frames, {} dropped", camera.sent, camera.dropped));
			ui.end_row();
		});
		if let Some(error) = &camera.error {
			ui.colored_label(ui.visuals().error_fg_color, error);
		}
	}

	/// Bind the key pressed to the action the Hotkeys panel waits on, or cancel
	/// on Escape.
	pub(super) fn take_binding(&mut self, ctx: &egui::Context) {
		let shown = std::mem::take(&mut self.hotkeys_shown);
		let (Some(action), Some(hotkeys)) = (self.binding.clone(), &mut self.hotkeys) else {
			return;
		};
		// A wait the operator left for another panel ends, so the hotkeys
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

	/// Bind keys to actions, heard even while another app has focus where the
	/// platform lets the studio hear them.
	pub(super) fn hotkeys(&mut self, ui: &mut Ui, fx: &mut Fx) {
		let Some(hotkeys) = &mut self.hotkeys else {
			return;
		};
		self.hotkeys_shown = true;
		let reach = hotkeys.reach();
		match &reach {
			Reach::Global => ui.label("Bound keys work even while another app, such as a game, has focus."),
			Reach::Desktop(_) => ui.label(
				"Bound keys work even while another app has focus. The desktop's shortcut settings decide the \
				 keys, shown beside each.",
			),
			Reach::Asking => ui.label("Asking the desktop to bind the keys."),
			Reach::Focused(why) => ui.colored_label(
				ui.visuals().warn_fg_color,
				format!("Bound keys work only while the studio has focus: {why}"),
			),
		};
		let scenes: Vec<String> = {
			let state = self.engine.state.lock().unwrap();
			state
				.collection()
				.scenes
				.iter()
				.map(|scene| scene.name.clone())
				.collect()
		};
		let mut actions: Vec<Action> = Action::FIXED.to_vec();
		actions.extend(scenes.iter().cloned().map(Action::Scene));
		let bindings = hotkeys.bindings();
		// A key bound to a scene the file no longer has, so it can be cleared.
		let gone: Vec<Action> = bindings
			.keys()
			.filter(|action| matches!(action, Action::Scene(name) if !scenes.contains(name)))
			.cloned()
			.collect();
		actions.extend(gone.iter().cloned());

		let mut bind = None;
		let mut clear = None;
		// A scene's key takes it, or cues it in Studio mode.
		let studio = self.controls.studio();
		egui::Grid::new("hotkeys").striped(true).show(ui, |ui| {
			for action in &actions {
				ui.label(action.name(studio));
				let bound = bindings.get(action);
				if self.binding.as_ref() == Some(action) {
					ui.strong("press a key, or Escape");
				} else {
					match bound {
						Some(bound) => ui.add(kit::Keycap::new(&bound.shown(ui.ctx())).fx(fx)),
						None => ui.weak("unbound"),
					};
				}
				if ui.add(IconButton::new(Icon::Hotkeys, "Bind a key").small()).clicked() {
					bind = Some(action.clone());
				}
				if ui
					.add_enabled(bound.is_some(), IconButton::new(Icon::Close, "Clear the key").small())
					.clicked()
				{
					clear = Some(action.clone());
				}
				let refusal = self.refusal.as_ref().filter(|(refused, _)| refused == action);
				if let Some((_, why)) = refusal {
					ui.colored_label(ui.visuals().error_fg_color, why);
				} else if let Some(why) = hotkeys.refused(action) {
					ui.colored_label(ui.visuals().error_fg_color, why);
				} else if gone.contains(action) {
					ui.weak("no such scene");
				} else if let (Reach::Desktop(triggers), Some(_)) = (&reach, bound) {
					match triggers.get(action) {
						Some(trigger) => ui.label(format!("the desktop bound {trigger}")),
						None => ui.weak("the desktop bound no key"),
					};
				}
				ui.end_row();
			}
		});
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

	/// The canvas's size and frame rate, applied at once rather than as each
	/// field changes, since a new canvas starts a new rendition.
	pub(super) fn canvas_form(&mut self, ui: &mut Ui) {
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

	/// Each scene's size, applied at once as the canvas's is: one without its
	/// own is the program's. Only a scene of the program's size goes on
	/// program; a published one publishes at its size.
	pub(super) fn sizes_form(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let program = state.collection().canvas;
		let scenes = state.collection().scenes.clone();
		let mut action = None;
		egui::CollapsingHeader::new("Scene sizes").show(ui, |ui| {
			for (index, scene) in scenes.iter().enumerate() {
				let own = scene.size.map(|size| (size.width, size.height));
				let current = own.unwrap_or((program.width, program.height));
				let (mut width, mut height) = self.sizes.get(&scene.name).copied().unwrap_or(current);
				// By name, so an edit under way stays on its scene as others
				// come and go.
				ui.push_id(&scene.name, |ui| {
					ui.horizontal(|ui| {
						ui.label(&scene.name);
						ui.add(egui::DragValue::new(&mut width).range(2..=u32::MAX));
						ui.label("x");
						ui.add(egui::DragValue::new(&mut height).range(2..=u32::MAX));
						let edited = (width, height) != current;
						if ui.add_enabled(edited, egui::Button::new("Apply")).clicked() {
							action = Some((index, Some(moq_scene::Size { width, height })));
						}
						if ui.add_enabled(edited, egui::Button::new("Reset")).clicked() {
							(width, height) = current;
						}
						if ui
							.add_enabled(own.is_some(), egui::Button::new("Program's"))
							.on_hover_text("Follow the program's size")
							.clicked()
						{
							action = Some((index, None));
						}
					})
				});
				match (width, height) == current {
					true => self.sizes.remove(&scene.name),
					false => self.sizes.insert(scene.name.clone(), (width, height)),
				};
			}
		});

		let Some((index, size)) = action else { return };
		let scene = scenes[index].clone();
		let name = scene.name.clone();
		let label = match size {
			Some(size) => format!("Size scene `{name}` {}x{}", size.width, size.height),
			None => format!("Size scene `{name}` as the program"),
		};
		let result = edit(&mut state, label, None, |draft| {
			draft.set_scene(index, Scene { size, ..scene })
		});
		if result.is_ok() {
			self.sizes.remove(&name);
		}
		drop(state);
		self.check(result);
	}
}
