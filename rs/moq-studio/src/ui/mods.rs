//! Settings, Mods: a switch for each feature most streamers never use.
//! Switching one off turns its feature off, through undo where it edits the
//! show; a show that uses one switches it back on, so a mod's UI is never
//! hidden while its feature changes what goes out.

use std::collections::BTreeSet;

use moq_scene::{Arrangement, AudioScene, Placement, Portrait};

use super::settings::{Restart, row};
use super::*;

/// The mods `collection` uses: those whose feature changes what it sends.
fn uses(collection: &moq_scene::Collection) -> Vec<Mod> {
	let mut used = Vec::new();
	if collection.audio_scenes.len() > 1 {
		used.push(Mod::Audio);
	}
	let items = || collection.scenes.iter().flat_map(|scene| &scene.items);
	if items().any(|item| item.slot.is_some() || item.keep_clear.is_some()) {
		used.push(Mod::Layout);
	}
	if collection.portrait || collection.canvas.height > collection.canvas.width {
		used.push(Mod::Formats);
	}
	let published = |scene: &Scene| scene.published || scene.size.is_some();
	if !collection.published.is_empty() || collection.scenes.iter().any(published) {
		used.push(Mod::Published);
	}
	used
}

/// The mods `state`'s show uses: as [`uses`] finds them, and Audio scenes &
/// routes while a source making a sound is left out of the stream's mix,
/// where only MIX reaches it.
fn used(state: &State) -> Vec<Mod> {
	let mut used = uses(state.collection());
	if !used.contains(&Mod::Audio) && !unheard(state).is_empty() {
		used.push(Mod::Audio);
	}
	used
}

/// The sources the stream's mix leaves out: each sent only to another audio
/// scene, or making a sound and sent to none. A source with a picture that
/// nothing draws, such as a guest waiting to be placed, is not left out:
/// Add places it and hears it.
fn unheard(state: &State) -> BTreeSet<String> {
	let collection = state.collection();
	let heard = collection
		.program_audio()
		.map(|program| collection.heard(program))
		.unwrap_or_default();
	let sent = collection.audio_scenes.iter().flat_map(|scene| &scene.sounds);
	let waiting = |name: &str| {
		collection.sources.get(name).is_some_and(Source::has_video) && super::layers::unplaced(collection, name)
	};
	let sounding = collection
		.sources
		.keys()
		.filter(|name| crate::controls::sounds(state, name) && !waiting(name));
	sent.chain(sounding)
		.filter(|name| !heard.contains(name.as_str()) && collection.sources.contains_key(*name))
		.cloned()
		.collect()
}

impl Panels {
	/// Settings, Mods: each mod's switch, and under one that is on what it
	/// keeps here.
	pub(super) fn mods_pane(&mut self, ui: &mut Ui) {
		let mut switched = None;
		for each in Mod::ALL {
			let mut on = self.controls.mods.on(each);
			row(ui, each.name(), each.help(), |ui| {
				if ui.add(kit::Toggle::new(&mut on).named(each.name())).changed() {
					switched = Some((each, on));
				}
				if on && each == Mod::Graph && ui.add(kit::button(Icon::Detach, "Open")).clicked() {
					self.graph_open = true;
					self.setting = None;
				}
			});
			if on && each == Mod::Published {
				self.published(ui);
			}
		}
		if let Some((each, on)) = switched {
			self.switch(each, on);
		}
	}

	/// Switch `each` on, or off along with its feature.
	pub(super) fn switch(&mut self, each: Mod, on: bool) {
		if on {
			self.controls.mods.set(each, true);
			return;
		}
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let used = used(&state).contains(&each);
		let canvas = state.collection().canvas;
		// A portrait show turns to landscape, which restarts the video, so on
		// air it asks first and the mod waits for the answer.
		if each == Mod::Formats && canvas.height > canvas.width {
			drop(state);
			let turned = Restart::Landscape(canvas.turned());
			match self.engine.on_air() != crate::engine::OnAir::Off {
				true => self.restart = Some(turned),
				false => self.apply_restart(turned),
			}
			return;
		}
		let label = format!("Switch off {}", each.name());
		let result = match each {
			Mod::Audio if used => self.audio_off(&mut state, label),
			Mod::Layout if used => layout_off(&mut state, label),
			Mod::Published if used => published_off(&mut state, label),
			Mod::Formats if used => edit(&mut state, label, None, |draft| draft.set_portrait(false)),
			Mod::Studio => {
				// A scene open off air stays cued, for its banner to take live.
				if self.editing.is_none() {
					state.uncue();
				}
				self.controls.set_studio(false);
				Ok(())
			}
			_ => Ok(()),
		};
		drop(state);
		if result.is_ok() {
			self.controls.mods.set(each, false);
			self.leave(each);
		}
		self.check(result);
	}

	/// Close what `each` showed, now that it is off.
	fn leave(&mut self, each: Mod) {
		match each {
			Mod::Multiview => self.multiview.set_open(false),
			Mod::Graph => self.graph_open = false,
			Mod::Layout | Mod::Formats | Mod::Published => self.deselect_item(),
			_ => {}
		}
	}

	/// Switch on each mod the show uses, saying so if `say`, and turn from a
	/// page or pane whose mod is off. Run as the show opens, and before each
	/// frame draws, so an undo, a reload, or an outside edit that brings a
	/// feature back brings its UI.
	pub(super) fn follow_mods(&mut self, say: bool) {
		let used = used(&self.engine.state.lock().unwrap());
		let mods = &self.controls.mods;
		let switched: Vec<&str> = used
			.into_iter()
			.filter(|each| !mods.on(*each))
			.inspect(|each| mods.set(*each, true))
			.map(Mod::name)
			.collect();
		if say && !switched.is_empty() {
			let names = switched.join(", ");
			self.say(Ok(format!(
				"switched on {names} in Settings, Mods, since the show uses it"
			)));
		}
		let off = |needs: Option<Mod>| needs.is_some_and(|needs| !self.controls.mods.on(needs));
		if off(self.page.needs()) {
			self.page = Page::Live;
		}
		if self.setting.is_some_and(|setting| off(setting.needs())) {
			self.setting = Some(Setting::Mods);
		}
		if off(Some(Mod::Graph)) {
			self.graph_open = false;
		}
		if off(Some(Mod::Multiview)) && self.multiview.open() {
			self.multiview.set_open(false);
		}
		self.controls.follow_mods();
	}

	/// One mix: the audio scene on program, holding what it plays now and
	/// every other sound, muted, so the stream sounds as it did and no
	/// sound is left where only MIX could reach it.
	fn audio_off(&self, state: &mut State, label: String) -> anyhow::Result<()> {
		let collection = state.collection();
		let program = collection.program_audio().context("no audio scene is on program")?;
		let heard: BTreeSet<String> = collection.heard(program).into_iter().map(String::from).collect();
		let elsewhere = unheard(state);
		let collection = state.collection();
		let one = AudioScene {
			program: true,
			sounds: heard.union(&elsewhere).cloned().collect(),
			..AudioScene::new(program)
		};
		// Listening to an audio scene that goes, listen to the stream.
		let listened = self.engine.listening().is_some_and(|(heard, _)| heard != program);
		let carrying: Vec<(usize, Scene)> = collection
			.scenes
			.iter()
			.enumerate()
			.filter(|(_, scene)| scene.audio.is_some())
			.map(|(index, scene)| {
				(
					index,
					Scene {
						audio: None,
						..scene.clone()
					},
				)
			})
			.collect();
		let muted: Vec<(String, moq_scene::Audio)> = elsewhere
			.iter()
			.map(|sound| {
				let audio = collection.audio.get(sound).cloned().unwrap_or_default();
				(sound.clone(), moq_scene::Audio { muted: true, ..audio })
			})
			.collect();
		let program = program.to_string();
		edit(state, label, None, |draft| {
			for (index, scene) in carrying {
				draft.set_scene(index, scene)?;
			}
			for (sound, audio) in muted {
				draft.set_audio(&sound, Some(audio))?;
			}
			draft.set_audio_scenes(vec![one])
		})?;
		if listened {
			self.listen(state, Some(&program))?;
		}
		Ok(())
	}
}

/// No scene published or of its own size, and no source published
/// independently. The engine closes the virtual camera of each scene no
/// longer published.
fn published_off(state: &mut State, label: String) -> anyhow::Result<()> {
	let collection = state.collection();
	let scenes: Vec<usize> = collection
		.scenes
		.iter()
		.enumerate()
		.filter(|(_, scene)| scene.published || scene.size.is_some())
		.map(|(index, _)| index)
		.collect();
	let sources: Vec<String> = collection.published.keys().cloned().collect();
	edit(state, label, None, |draft| {
		for index in scenes {
			let scene = draft.collection().scenes[index].clone();
			draft.set_scene(
				index,
				Scene {
					published: false,
					..scene
				},
			)?;
			draft.set_size(index, None)?;
		}
		for name in sources {
			draft.set_published(&name, None)?;
		}
		Ok(())
	})
}

/// Every layer set free where it is drawn now, in each picture, and kept
/// there rather than out of the way.
pub(crate) fn layout_off(state: &mut State, label: String) -> anyhow::Result<()> {
	let collection = state.collection();
	let mut freed = Vec::new();
	let mut arranged = Vec::new();
	for (number, scene) in collection.scenes.iter().enumerate() {
		let layer = Layer::scene(&scene.name);
		let dual = collection.dual(&layer);
		for (index, item) in scene.items.iter().enumerate() {
			let (rect, crop, portrait) = match (item.slot, item.keep_clear) {
				(None, None) => continue,
				(Some(_), _) => {
					let drawn = |format| {
						// A frame behind an edit, where the slot is drawn is not
						// known yet, and a guess would be saved.
						anyhow::ensure!(
							super::slots::arranged(state, &layer, index, format).is_some(),
							"the layout is still settling; switch {} off again",
							Mod::Layout.name()
						);
						let (rect, crop) =
							super::slots::drawn(state, &layer, index, format).unwrap_or((item.rect, item.crop));
						Ok(Placement { rect, crop })
					};
					let Placement { rect, crop } = drawn(Format::Landscape)?;
					let portrait = (dual && item.shown_in(Format::Portrait))
						.then(|| drawn(Format::Portrait))
						.transpose()?;
					(rect, crop, portrait)
				}
				// Keeping clear moves only the landscape picture, so the
				// portrait one keeps the place it has from the layer's own.
				(None, Some(_)) => {
					let placed = super::keep::placed(state, &layer, item, Format::Landscape);
					let portrait = match dual && item.shown_in(Format::Portrait) {
						true => Some(collection.placement(item, Format::Portrait)),
						false => item.portrait,
					};
					(placed.rect, item.crop, portrait)
				}
			};
			// A slot closed now, as while its source is offline, shows
			// nothing, so its layer stays hidden rather than filling the
			// picture when the source returns.
			let closed = super::slots::closed(state, &layer, index);
			let free = Item {
				slot: None,
				keep_clear: None,
				rect,
				crop,
				portrait,
				visible: item.visible && !closed,
				..item.clone()
			};
			freed.push((layer.clone(), index, free));
		}
		if scene.arrangement != Arrangement::default() || scene.portrait != Portrait::default() {
			arranged.push(number);
		}
	}
	edit(state, label, None, |draft| {
		for (layer, index, item) in freed {
			draft.set_item(&layer, index, item)?;
		}
		for number in arranged {
			let scene = draft.collection().scenes[number].clone();
			let scene = Scene {
				arrangement: Arrangement::default(),
				portrait: Portrait::default(),
				..scene
			};
			draft.set_scene(number, scene)?;
		}
		Ok(())
	})
}

#[cfg(test)]
mod tests {
	use moq_scene::{Collection, KeepClear, Published, Slot};

	use super::*;

	/// A landscape show with one scene holding one layer, and one audio scene.
	fn plain() -> Collection {
		let mut collection = Collection::new();
		collection.canvas = Canvas {
			width: 1920,
			height: 1080,
			framerate: 30,
		};
		let mut scene = Scene::new("Main");
		scene.items.push(Item::new("cam", moq_scene::Rect::default()));
		collection.scenes = vec![scene];
		collection
	}

	#[test]
	fn each_feature_is_found_alone() {
		assert_eq!(uses(&plain()), [], "a plain show");
		type Case = (fn(&mut Collection), Mod);
		let cases: [Case; 8] = [
			(|c| c.audio_scenes.push(AudioScene::new("Phones")), Mod::Audio),
			(|c| c.scenes[0].items[0].slot = Some(Slot::Main), Mod::Layout),
			(
				|c| c.scenes[0].items[0].keep_clear = Some(KeepClear::default()),
				Mod::Layout,
			),
			(|c| c.portrait = true, Mod::Formats),
			(|c| c.canvas = c.canvas.turned(), Mod::Formats),
			(|c| c.scenes[0].published = true, Mod::Published),
			(
				|c| {
					c.scenes[0].size = Some(moq_scene::Size {
						width: 640,
						height: 360,
					})
				},
				Mod::Published,
			),
			(
				|c| _ = c.published.insert("cam".into(), Published::default()),
				Mod::Published,
			),
		];
		for (index, (change, which)) in cases.into_iter().enumerate() {
			let mut collection = plain();
			change(&mut collection);
			assert_eq!(uses(&collection), [which], "case {index}");
		}
	}
}
