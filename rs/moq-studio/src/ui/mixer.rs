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

	/// The audio scenes by name, each renamed, reordered, or removed here, and
	/// a new one added. The MIX page's redesign gives each a master strip
	/// (quest/m0/ux/mix.md).
	pub(super) fn audio_scenes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let audio_scenes = state.collection().audio_scenes.clone();
		let count = audio_scenes.len();
		let mut audio_action = None;
		for (index, scene) in audio_scenes.iter().enumerate() {
			ui.horizontal(|ui| {
				if let Some((renaming, name)) = &mut self.renaming_audio
					&& *renaming == scene.name
				{
					let response = ui.text_edit_singleline(name);
					if !response.has_focus() && !response.lost_focus() {
						response.request_focus();
					}
					if response.lost_focus() {
						// Escape cancels; Enter or a click away renames.
						if !ui.input(|input| input.key_pressed(egui::Key::Escape)) {
							audio_action = Some(AudioAction::Rename(index, name.trim().to_string()));
						}
						self.renaming_audio = None;
					}
					return;
				}
				let label = ui.label(kit::caps(&scene.name, kit::display(14.0)));
				if scene.program {
					ui.add(kit::Tag::new("On air", Tally::Air));
				}
				let rename = ui.add(IconButton::new(Icon::Rename, "Rename").small());
				if label.double_clicked() || rename.clicked() {
					self.renaming_audio = Some((scene.name.clone(), scene.name.clone()));
				}
				if ui
					.add_enabled(index > 0, IconButton::new(Icon::Up, "Move up").small())
					.clicked()
				{
					audio_action = Some(AudioAction::Move(index, index - 1));
				}
				let down = IconButton::new(Icon::Down, "Move down").small();
				if ui.add_enabled(index + 1 < count, down).clicked() {
					audio_action = Some(AudioAction::Move(index, index + 1));
				}
				let remove = IconButton::new(Icon::Remove, "Remove").small().note(
					scene
						.program
						.then_some("On program; put another audio scene on program first"),
				);
				if ui.add_enabled(!scene.program, remove).clicked() {
					audio_action = Some(AudioAction::Remove(index));
				}
			});
		}
		ui.horizontal(|ui| {
			ui.add(egui::TextEdit::singleline(&mut self.new_scene).hint_text("New audio scene"));
			if ui.add(kit::button(Icon::Add, "Add audio scene")).clicked() {
				let name = self.new_scene.trim().to_string();
				let mut next = audio_scenes.clone();
				next.push(moq_scene::AudioScene::new(name.clone()));
				let result = edit(&mut state, format!("Add audio scene `{name}`"), None, |draft| {
					draft.set_audio_scenes(next)
				});
				match result {
					Ok(()) => self.new_scene.clear(),
					Err(err) => self.report(Err(err)),
				}
			}
		});
		if let Some(action) = audio_action {
			let result = self.audio_scene(&mut state, &audio_scenes, action);
			drop(state);
			self.check(result);
		}
	}

	/// Rename, reorder, or remove one of `scenes`, the audio scenes as their
	/// list showed them. Listening follows a renamed one.
	pub(super) fn audio_scene(
		&self,
		state: &mut State,
		scenes: &[moq_scene::AudioScene],
		action: AudioAction,
	) -> anyhow::Result<()> {
		match action {
			AudioAction::Rename(index, to) => {
				let from = &scenes[index].name;
				if *from == to {
					return Ok(());
				}
				edit(state, format!("Rename audio scene `{from}` to `{to}`"), None, |draft| {
					draft.rename_audio_scene(from, &to)
				})?;
				let listening = self.engine.listening();
				if listening.is_some_and(|(heard, _)| heard == *from) {
					self.listen(state, Some(&to))?;
				}
				Ok(())
			}
			AudioAction::Move(from, to) => {
				let mut next = scenes.to_vec();
				let scene = next.remove(from);
				let label = format!("Reorder audio scene `{}`", scene.name);
				next.insert(to, scene);
				edit(state, label, None, |draft| draft.set_audio_scenes(next))
			}
			AudioAction::Remove(index) => {
				let name = &scenes[index].name;
				edit(state, format!("Remove audio scene `{name}`"), None, |draft| {
					draft.remove_audio_scene(name)
				})
			}
		}
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
