//! The REVIEW page: replays, clips, and recordings.

use super::*;

impl Panels {
	/// Start and stop recording, and see what the recording wrote.
	pub(super) fn record(&mut self, ui: &mut Ui) {
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
					ui.colored_label(kit::ACCENT, format!("recording {elapsed}"));
				}
			}
			None => {
				let mut each = self.controls.record_sources.load(Ordering::Relaxed);
				if ui.button("Start recording").clicked() {
					let result = recorder.start(each);
					self.check(result);
				}
				if ui.add(kit::Toggle::new(&mut each).label("each source too")).changed() {
					self.controls.record_sources.store(each, Ordering::Relaxed);
				}
				ui.weak(format!("into {}", recorder.dir().display()));
			}
		});
		self.recorded_scene(ui);
		self.recorded_audio(ui);
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

	/// Pick the picture recordings hold, the program or any scene, even while
	/// recording, which goes on in a new file when the size changes.
	pub(super) fn recorded_scene(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let current = state.collection().recorded().map(str::to_string);
		let scenes = state.collection().scenes.iter();
		let names: Vec<String> = scenes.map(|scene| scene.name.clone()).collect();
		let mut picked = current.clone();
		ui.horizontal(|ui| {
			ui.label("Picture")
				.on_hover_text("A scene of another size than the program's goes on in a new file");
			egui::ComboBox::from_id_salt("recorded scene")
				.selected_text(picked.as_deref().unwrap_or("the program"))
				.show_ui(ui, |ui| {
					ui.selectable_value(&mut picked, None, "the program");
					for name in &names {
						ui.selectable_value(&mut picked, Some(name.clone()), name);
					}
				});
		});
		if picked == current {
			return;
		}
		let label = match &picked {
			Some(name) => format!("Record scene `{name}`"),
			None => "Record the program".into(),
		};
		let result = edit(&mut state, label, None, |draft| draft.set_recorded(picked.as_deref()));
		drop(state);
		self.check(result);
	}

	/// Pick the audio scene recordings hold, even while recording, which goes
	/// on with the new one from its next block.
	pub(super) fn recorded_audio(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let scenes = state.collection().audio_scenes.clone();
		let program = state.collection().program_audio().unwrap_or_default();
		let on_program = format!("on program ({program})");
		let current = scenes
			.iter()
			.find(|scene| scene.recorded)
			.map(|scene| scene.name.clone());
		let mut picked = current.clone();
		ui.horizontal(|ui| {
			ui.label("Audio")
				.on_hover_text("An audio scene nothing else carries stays off the relay");
			egui::ComboBox::from_id_salt("recorded audio")
				.selected_text(picked.as_deref().unwrap_or(&on_program))
				.show_ui(ui, |ui| {
					ui.selectable_value(&mut picked, None, &on_program);
					for scene in &scenes {
						ui.selectable_value(&mut picked, Some(scene.name.clone()), &scene.name);
					}
				});
		});
		if picked == current {
			return;
		}
		let label = match &picked {
			Some(name) => format!("Record audio scene `{name}`"),
			None => "Record the audio scene on program".into(),
		};
		let mut next = scenes;
		for scene in &mut next {
			scene.recorded = picked.as_deref() == Some(scene.name.as_str());
		}
		let result = edit(&mut state, label, None, |draft| draft.set_audio_scenes(next));
		drop(state);
		self.check(result);
	}

	/// Replay the recent past of the program or a source in a replay source,
	/// or save it as a clip.
	pub(super) fn replay(&mut self, ui: &mut Ui) {
		let (replayable, replays) = {
			let state = self.engine.state.lock().unwrap();
			let named = |kind: fn(&Source) -> bool| -> Vec<String> {
				let sources = &state.collection().sources;
				sources
					.iter()
					.filter(|(_, source)| kind(source))
					.map(|(name, _)| name.clone())
					.collect()
			};
			// What a source shows or plays, a clone its source's, has a past
			// only if something encodes it: not an image, a color, a web page,
			// an overlay, a LUT, or a scene or the program drawn into another.
			let collection = state.collection();
			let replayable = collection
				.sources
				.keys()
				.filter(|name| {
					let captured = collection.sources.get(collection.capture(name));
					captured.is_some_and(|source| {
						!matches!(
							source,
							Source::Image { .. }
								| Source::Color { .. } | Source::Browser { .. }
								| Source::Overlay { .. } | Source::Lut { .. }
								| Source::Scene { .. } | Source::Program
						)
					})
				})
				.cloned()
				.collect::<Vec<_>>();
			(replayable, named(|source| matches!(source, Source::Replay)))
		};
		let controls = self.controls.clone();
		// Held while drawing, never with the engine's state, which a replay
		// locks under it.
		let mut guard = controls.replay.lock().unwrap();
		let panel = &mut *guard;
		if panel.into.as_ref().is_none_or(|into| !replays.contains(into)) {
			panel.into = replays.first().cloned();
		}

		egui::Grid::new("replay").show(ui, |ui| {
			ui.label("Replay");
			egui::ComboBox::from_id_salt("replay of")
				.selected_text(panel.of.to_string())
				.show_ui(ui, |ui| {
					ui.selectable_value(&mut panel.of, Of::Program, Of::Program.to_string());
					for name in replayable {
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
					.add(kit::button(Icon::Clip, "Save clip"))
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
		let action = match (play, save) {
			(true, _) => Action::Replay,
			(_, true) => Action::SaveClip,
			_ => return,
		};
		drop(guard);
		// How it went shows above, from the next frame.
		let _ = controls.fire(&action);
	}
}
