//! The MIX page: every sound and audio scene, with its sends.

use super::*;

impl Panels {
	/// The mixer: which audio scene the program carries and which plays to
	/// the speakers, and a strip for each sound and each audio scene, with its
	/// meter and a send to each audio scene. A sound's strip sets its gain,
	/// mute, and delay too.
	pub(super) fn audio(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let scenes = state.collection().audio_scenes.clone();
		let listening = engine.listening();
		let mut scenes_change = None;
		let mut listen = None;

		egui::Grid::new("audio scenes").show(ui, |ui| {
			for scene in &scenes {
				let name = &scene.name;
				ui.label(name);
				meter(ui, self.levels.mix(name));
				if ui.radio(scene.program, "On program").clicked() && !scene.program {
					scenes_change = Some(on_program(&scenes, name));
				}
				let heard = listening.as_ref().is_some_and(|(heard, _)| heard == name);
				let note = match mix::MONITOR {
					true => "Play this audio scene to the speakers; use headphones",
					false => "Needs a build with the `capture` feature",
				};
				let button = IconButton::new(Icon::Listen, "Listen")
					.lit(heard, kit::ACCENT)
					.small()
					.note(Some(note));
				let response = ui.add_enabled(mix::MONITOR, button);
				if response.clicked() {
					listen = Some((!heard).then(|| name.clone()));
				}
				ui.end_row();
			}
		});
		if mix::MONITOR && self.speakers.ui(ui, &self.runtime, &mut self.speaker) {
			// The monitor moves to the new output.
			if let Some((heard, _)) = &listening {
				listen = Some(Some(heard.clone()));
			}
		}
		if let Some((_, Some(failed))) = &listening {
			ui.colored_label(ui.visuals().error_fg_color, format!("Listening stopped: {failed}"));
		}
		ui.separator();

		// A send toggled from a sound or an audio scene into an audio scene.
		let send = |from: Strip<'_>, to: &str, on: bool| {
			let mut next = scenes.clone();
			from.send(&mut next, to, on);
			(next, from.label(to, on))
		};
		let mut change = None;
		egui::Grid::new("audio").striped(true).show(ui, |ui| {
			ui.label("");
			ui.label("");
			ui.label("Gain");
			ui.label("Pan");
			ui.label("Delay")
				.on_hover_text("Hold the source back to line it up with a camera that lags it");
			for scene in &scenes {
				ui.label(&scene.name);
			}
			ui.end_row();
			for (name, feed) in state.sources.iter() {
				if feed.strip().is_none() {
					continue;
				}
				let audio = state.collection().audio.get(name).cloned().unwrap_or_default();
				ui.label(name);
				if let Some((audio, label, gesture)) = strip_controls(ui, &mut self.gestures, name, &audio) {
					change = Some((name.to_string(), audio, label, gesture));
				}
				for scene in &scenes {
					let mut sent = scene.sounds.contains(name);
					if ui.add(kit::Toggle::new(&mut sent)).changed() {
						scenes_change = Some(send(Strip::Sound(name), &scene.name, sent));
					}
				}
				meter(ui, self.levels.sound(name));
				ui.end_row();
			}
			// Each audio scene sends into others too, never into itself, even
			// through others.
			for from in &scenes {
				let name = &from.name;
				ui.label(name).on_hover_text("An audio scene");
				ui.weak("audio scene");
				ui.label("");
				ui.label("");
				ui.label("");
				for scene in &scenes {
					let mut sent = scene.scenes.contains(name);
					let loops = scene.name == *name || state.collection().nests(name, &scene.name);
					let response = ui
						.add_enabled(!loops, egui::Checkbox::without_text(&mut sent))
						.on_disabled_hover_text("It would hold itself");
					if response.changed() {
						scenes_change = Some(send(Strip::Scene(name), &scene.name, sent));
					}
				}
				meter(ui, self.levels.mix(name));
				ui.end_row();
			}
		});

		let mut result = Ok(());
		if let Some((name, audio, label, gesture)) = change {
			result = edit(&mut state, label, gesture, |draft| draft.set_audio(&name, Some(audio)));
		}
		if let Some((next, label)) = scenes_change {
			result = result.and(edit(&mut state, label, None, |draft| draft.set_audio_scenes(next)));
		}
		if let Some(listen) = listen {
			result = result.and(self.listen(&mut state, listen.as_deref()));
		}
		drop(state);
		self.check(result);
	}

	/// Play the audio scene called `scene` to the speakers picked, or stop
	/// with `None`.
	pub(super) fn listen(&self, state: &mut State, scene: Option<&str>) -> anyhow::Result<()> {
		let listen = scene.map(|scene| mix::Listen {
			scene,
			speaker: self.speaker.as_deref(),
		});
		self.engine.listen(state, &self.runtime, listen)
	}
}
