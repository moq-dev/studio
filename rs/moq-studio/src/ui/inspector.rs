//! The BUILD page's inspector, which follows the selection: the selected
//! layer's place, shape, look, and source, and where else it shows, with its
//! keyframes under Animate; with no layer selected, the scene's own settings.

use super::build::{LayerAction, linked_note, shapes, shown_in};
use super::layers::{section, status_of};
use super::*;

/// How wide a border's slider reaches, in canvas pixels.
const BORDER: f32 = 32.0;

/// A drop shadow's blur at full strength, in canvas pixels.
const SHADOW: f32 = 32.0;

/// What the inspector asked of the selected layer.
enum Asked {
	/// Replace the layer, labelled, as part of a gesture.
	Item(Item, String, Option<u64>),
	/// Set the look of the layer or its source.
	Look(Selection, Look, String, Option<u64>),
	/// What its toolbar or menu could ask too.
	Act(LayerAction),
	/// Play a media source from its start, stop it, or reload it.
	Play(String),
	Stop(String),
	Reload(String),
	/// Publish the source independently, or stop.
	Publish(String, bool),
	/// Fill the role called this with this source, or leave it unfilled.
	Fill(String, Option<String>),
}

impl Panels {
	/// The inspector in its own column at `rect`.
	pub(super) fn inspector(&mut self, ui: &mut Ui, rect: egui::Rect) {
		ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(12.0)), |ui| {
			self.inspector_body(ui);
		});
	}

	/// The inspector's tabs, the layer's name and Animate, or the scene's
	/// name with no layer selected, and the one shown.
	pub(super) fn inspector_body(&mut self, ui: &mut Ui) {
		let (layer, scene) = {
			let engine = self.engine.clone();
			let state = engine.state.lock().unwrap();
			self.follow_reloads(&state);
			let collection = state.collection();
			let layer = self.selection.as_ref().and_then(|picked| picked.item(collection));
			let layer = layer.map(|item| item.source.clone());
			(layer, self.edited(&state).map(str::to_string))
		};
		let Some(name) = layer else {
			let name = scene.unwrap_or_else(|| "Scene".into());
			super::shell::tabs(ui, &mut (), &[()], |()| name.as_str());
			ui.add_space(4.0);
			egui::ScrollArea::vertical()
				.id_salt("inspector")
				.auto_shrink(false)
				.show(ui, |ui| self.scene_inspector(ui));
			return;
		};
		let mut inspect = self.inspect;
		super::shell::tabs(ui, &mut inspect, &[Inspect::Layer, Inspect::Animate], |tab| match tab {
			Inspect::Layer => name.as_str(),
			Inspect::Animate => "Animate",
		});
		self.inspect = inspect;
		ui.add_space(4.0);
		egui::ScrollArea::vertical()
			.id_salt("inspector")
			.auto_shrink(false)
			.show(ui, |ui| match self.inspect {
				Inspect::Layer => self.layer_inspector(ui),
				Inspect::Animate => self.keyframes(ui),
			});
	}

	/// The selected layer's place, shape, look, source, and where it shows.
	fn layer_inspector(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let Some(picked) = self.selection.clone() else { return };
		let collection = state.collection();
		let Some(item) = picked.item(collection).cloned() else {
			return;
		};
		let spot = Spot::Layer(picked.layer.clone(), item.source.clone());
		if let Some(fault) = self.fault(&spot) {
			ui.add_space(6.0);
			ui.colored_label(kit::ERR, fault);
		}
		let name = item.source.clone();
		let mut asked = None;

		section(ui, "Place");
		if picked.layer.scene.is_some() {
			use moq_scene::Slot;
			let name = |slot: Option<Slot>| match slot {
				None => "Free",
				Some(Slot::Main) => "Main slot",
				Some(Slot::Tile) => "A tile",
			};
			let mut slot = item.slot;
			ui.horizontal(|ui| {
				ui.label("Slot");
				egui::ComboBox::from_id_salt("slot")
					.selected_text(name(slot))
					.show_ui(ui, |ui| {
						for each in [None, Some(Slot::Main), Some(Slot::Tile)] {
							ui.selectable_value(&mut slot, each, name(each));
						}
					})
					.response
					.on_hover_text(
						"A layer in a slot is placed by the scene's arrangement, which closes the slot up when \
						 its source goes away; a free one stays where its rect says",
					);
			});
			if slot != item.slot {
				asked = Some(Asked::Act(LayerAction::Slot(slot)));
			}
		}
		// Where it is shown, and its place in the picture BUILD edits, for a
		// layer drawn in both.
		let format = self.format;
		if collection.dual(&picked.layer) {
			let mut shown = item.format;
			ui.horizontal(|ui| {
				ui.label("Format");
				let options = super::formats::shown_in_options();
				ui.add(kit::Segmented::new(&mut shown, &options))
					.on_hover_text("Which pictures show it: both, or only the landscape or the portrait one");
			});
			if shown != item.format {
				asked = Some(Asked::Item(
					super::formats::shown(&item, shown),
					super::formats::shown_label(&name, shown),
					None,
				));
			}
			if item.slot.is_none() && item.shown_in(format) {
				ui.label(
					egui::RichText::new(format!("Its place in the {} picture", format.name().to_lowercase()))
						.color(kit::DIM)
						.font(kit::ui(11.0)),
				);
			}
		}
		let rest = picked.rest(collection, format).unwrap_or_else(|| item.clone());
		let placed = |rect, crop| {
			picked
				.placed(collection, format, rect, crop)
				.unwrap_or_else(|| item.clone())
		};
		let mut rect = rest.rect;
		if item.slot.is_some() {
			ui.label(
				egui::RichText::new("Placed by the scene's arrangement; drag it onto another slot to swap them")
					.color(kit::DIM)
					.font(kit::ui(11.0)),
			);
		}
		if item.slot.is_none() {
			ui.horizontal(|ui| {
				ui.spacing_mut().item_spacing.x = 4.0;
				let width = ((ui.available_width() - 12.0) / 4.0).max(40.0);
				let field = |drag, label| kit::Number::new(drag).label(label).width(width);
				let size = 0.0..=f32::MAX;
				let responses = [
					("Move", ui.add(field(egui::DragValue::new(&mut rect.x), "X"))),
					("Move", ui.add(field(egui::DragValue::new(&mut rect.y), "Y"))),
					(
						"Resize",
						ui.add(field(egui::DragValue::new(&mut rect.width).range(size.clone()), "W")),
					),
					(
						"Resize",
						ui.add(field(egui::DragValue::new(&mut rect.height).range(size), "H")),
					),
				];
				for (verb, response) in responses {
					if let Some(gesture) = changed(&response, &mut self.gestures) {
						let label = format!("{verb} `{name}`");
						asked = Some(Asked::Item(placed(rect, rest.crop), label, gesture));
					}
				}
			});
		}
		let mut crop = rest.crop;
		ui.horizontal(|ui| {
			ui.label(egui::RichText::new("Crop").color(kit::DIM));
			if let Some(gesture) = crop_ui(ui, &mut crop, &mut self.gestures) {
				asked = Some(Asked::Item(placed(rest.rect, crop), format!("Crop `{name}`"), gesture));
			}
		})
		.response
		.on_hover_text("Percent of the picture trimmed from the left, top, right, and bottom");
		ui.horizontal(|ui| {
			let fits = match item.slot {
				Some(_) => &[][..],
				None => &[
					(
						"Fit",
						LayerAction::Fit,
						"As large as fits on the canvas, keeping its shape",
					),
					(
						"Fill",
						LayerAction::Fill,
						"Cover the canvas, keeping its shape and cropping the rest",
					),
				][..],
			};
			for (text, action, hover) in fits.iter().cloned() {
				if ui.button(text).on_hover_text(hover).clicked() {
					asked = Some(Asked::Act(action));
				}
			}
			let (icon, text, locked) = match item.locked {
				true => (Icon::Unlock, "Unlock", false),
				false => (Icon::Lock, "Lock", true),
			};
			if ui
				.add(kit::button(icon, text))
				.on_hover_text("A locked layer stays put: clicks on the picture pass through it")
				.clicked()
			{
				asked = Some(Asked::Act(LayerAction::Lock(locked)));
			}
		});

		section(ui, "Shape");
		if let Some(corners) = shapes(ui, item.shape.corners) {
			asked = Some(Asked::Act(LayerAction::Shape(corners)));
		}
		let mut shape = item.shape;
		egui::Grid::new("shape").num_columns(3).show(ui, |ui| {
			ui.label("Border");
			let mut width = (shape.border / BORDER).clamp(0.0, 1.0);
			let slid = ui.add(kit::Slider::new(&mut width).width(130.0));
			if let Some(gesture) = changed(&slid, &mut self.gestures) {
				shape.border = (width * BORDER).round();
				let label = format!("Set the border of `{name}`");
				asked = Some(Asked::Item(Item { shape, ..item.clone() }, label, gesture));
			}
			if color_ui(ui, &mut shape.color).changed() {
				let label = format!("Color the border of `{name}`");
				asked = Some(Asked::Item(Item { shape, ..item.clone() }, label, None));
			}
			ui.end_row();
			ui.label("Shadow");
			let shadow = item
				.filters
				.iter()
				.position(|filter| matches!(filter, Filter::DropShadow { .. }));
			let mut strength = match shadow.map(|at| &item.filters[at]) {
				Some(Filter::DropShadow { blur, .. }) => (blur / SHADOW).clamp(0.0, 1.0),
				_ => 0.0,
			};
			let slid = ui
				.add(kit::Slider::new(&mut strength).width(130.0))
				.on_hover_text("A soft shadow under the layer, which follows its shape");
			if let Some(gesture) = changed(&slid, &mut self.gestures) {
				let mut filters = item.filters.clone();
				let color = match shadow.map(|at| &filters[at]) {
					Some(Filter::DropShadow { color, .. }) => *color,
					_ => moq_scene::Rgba([0, 0, 0, 160]),
				};
				let cast = Filter::DropShadow {
					x: 0.0,
					y: (strength * SHADOW / 2.0).round(),
					blur: (strength * SHADOW).round(),
					color,
				};
				match (shadow, strength > 0.0) {
					(Some(at), true) => filters[at] = cast,
					(Some(at), false) => drop(filters.remove(at)),
					(None, true) => filters.push(cast),
					(None, false) => {}
				}
				let label = format!("Set the shadow of `{name}`");
				asked = Some(Asked::Item(
					Item {
						filters,
						..item.clone()
					},
					label,
					gesture,
				));
			}
			ui.end_row();
		});

		section(ui, "Look");
		let source = collection.sources.get(&item.source);
		let linked = shown_in(collection, &item.source).len();
		if source.is_some_and(Source::removes_background) {
			let background = collection.video.get(&item.source).map(|video| video.background);
			let mut removed = background == Some(Background::Remove);
			let note = match linked {
				1 => "Cut the person out of the picture, leaving the rest transparent, before the crop and \
				      every filter. Each picture shows a frame or more late, and the source's own track keeps \
				      the background."
					.to_string(),
				n => format!(
					"Cut the person out of the picture on all {n} layers showing this source; make it \
					 independent to cut out here alone."
				),
			};
			if ui
				.add(kit::Toggle::new(&mut removed).label("Remove background"))
				.on_hover_text(note)
				.changed()
			{
				asked = Some(Asked::Act(LayerAction::CutOut(removed)));
			}
		}
		let luts: Vec<String> = collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Lut { .. }))
			.map(|(name, _)| name.clone())
			.collect();
		let graded = item
			.filters
			.iter()
			.position(|filter| matches!(filter, Filter::Lut { .. }));
		let mut grade = graded.and_then(|at| match &item.filters[at] {
			Filter::Lut { source, .. } => Some(source.clone()),
			_ => None,
		});
		let before = grade.clone();
		ui.horizontal(|ui| {
			ui.label("Color grade");
			egui::ComboBox::from_id_salt("grade")
				.selected_text(grade.as_deref().unwrap_or("None"))
				.show_ui(ui, |ui| {
					ui.selectable_value(&mut grade, None, "None");
					for lut in &luts {
						ui.selectable_value(&mut grade, Some(lut.clone()), lut);
					}
				})
				.response
				.on_hover_text("A LUT source's look over the layer; add one with Add, Any source, LUT");
		});
		if grade != before {
			let mut filters = item.filters.clone();
			match (graded, grade) {
				(Some(at), Some(source)) => filters[at] = Filter::Lut { source, amount: 1.0 },
				(Some(at), None) => drop(filters.remove(at)),
				(None, Some(source)) => filters.push(Filter::Lut { source, amount: 1.0 }),
				(None, None) => {}
			}
			let label = format!("Grade `{name}`");
			asked = Some(Asked::Item(
				Item {
					filters,
					..item.clone()
				},
				label,
				None,
			));
		}
		if let Some(video) = collection.video.get(&item.source)
			&& !video.filters.is_empty()
		{
			let own: Vec<String> = video.filters.iter().map(describe_filter).collect();
			let note = format!("After its source's own: {}", own.join(", "));
			ui.label(egui::RichText::new(note).color(kit::DIM).font(kit::ui(11.0)));
		}
		if let Some((look, label, gesture)) = self.look_ui(ui, collection, &picked.look(), "layer") {
			asked = Some(Asked::Look(picked.look(), look, label, gesture));
		}

		section(ui, "Shown in");
		let places = shown_in(collection, &item.source);
		if let Some(Source::Clone { source }) = source {
			ui.label(format!("Its own copy of `{source}`, with a look of its own"));
		}
		ui.horizontal(|ui| {
			let (icon, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
			Icon::Link.paint(ui, icon, 14.0, kit::DIM);
			let all_scenes = places.iter().all(|layer| layer.scene.is_some());
			let text = match (places.len(), all_scenes) {
				(0 | 1, _) => "Only here".to_string(),
				(n, true) => format!("Linked in {n} scenes"),
				(n, false) => format!("Linked in {n} places"),
			};
			let list: Vec<String> = places.iter().map(place).collect();
			ui.label(text).on_hover_text(list.join("\n"));
			if places.len() > 1 {
				ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
					let copyable = !collection.unfilled(&item.source);
					if ui
						.add_enabled(copyable, egui::Button::new("Make independent"))
						.on_hover_text(linked_note(places.len()))
						.on_disabled_hover_text(super::build::NOTHING_TO_COPY)
						.clicked()
					{
						asked = Some(Asked::Act(LayerAction::Independent));
					}
				});
			}
		});

		section(ui, "Source");
		if let Some(source) = source {
			ui.label(egui::RichText::new(describe(source)).color(kit::DIM));
			let health = status_of(&state, &item.source, kio::clock::now());
			for line in &health.lines {
				ui.horizontal(|ui| {
					let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
					ui.painter().circle_filled(dot.center(), 3.5, health.color);
					ui.add(egui::Label::new(line).wrap());
				});
			}
			ui.horizontal(|ui| {
				if matches!(source, Source::Media { .. }) {
					if ui
						.add(kit::button(Icon::Play, "Play"))
						.on_hover_text("Play from the start")
						.clicked()
					{
						asked = Some(Asked::Play(name.clone()));
					}
					if ui
						.add(kit::button(Icon::Stop, "Stop"))
						.on_hover_text("Blank and silent until played again")
						.clicked()
					{
						asked = Some(Asked::Stop(name.clone()));
					}
				}
				let reloads = match source {
					Source::Browser { .. } => {
						Some("Restart the page, keeping the cookies and logins every page shares")
					}
					Source::Media { .. } => Some("Read the file again, such as after it changed"),
					Source::PickedDisplay | Source::PickedWindow => {
						Some("Open again, asking the system picker if it holds no grant")
					}
					_ => None,
				};
				if let Some(hover) = reloads
					&& ui
						.add(kit::button(Icon::Reload, "Reload"))
						.on_hover_text(hover)
						.clicked()
				{
					asked = Some(Asked::Reload(name.clone()));
				}
			});
			if let Source::Role { source: filled } = source {
				let fillers: Vec<String> = collection
					.sources
					.iter()
					.filter(|(_, other)| other.has_video() && !other.is_role() && !matches!(other, Source::Lut { .. }))
					.map(|(name, _)| name.clone())
					.collect();
				let mut fill = filled.clone();
				ui.horizontal(|ui| {
					ui.label("Filled by");
					egui::ComboBox::from_id_salt("fill")
						.selected_text(fill.as_deref().unwrap_or("Nothing"))
						.show_ui(ui, |ui| {
							ui.selectable_value(&mut fill, None, "Nothing");
							for other in &fillers {
								ui.selectable_value(&mut fill, Some(other.clone()), other);
							}
						})
						.response
						.on_hover_text(
							"What the role shows in every scene of the show, so a scene built from roles \
							 survives a change of hardware. Unfilled, it draws nothing and its slot closes up.",
						);
				});
				if fill != *filled {
					asked = Some(Asked::Fill(name.clone(), fill));
				}
			}
			if source.publishes() {
				let mut published = collection.published.contains(&name);
				if ui
					.add(kit::Toggle::new(&mut published).label("Published independently"))
					.on_hover_text(
						"Publish this source's own track to the relay beside the program, for viewers to \
						 place over it as the layer layout says, and leave it out of the program. It \
						 encodes for as long as it is published. Every player must reproduce how it is \
						 placed, so it takes no filter but opacity, no blend mode, no shape, and not its \
						 background removed, since its track keeps the background.",
					)
					.changed()
				{
					asked = Some(Asked::Publish(name.clone(), published));
				}
			}
			// A role takes the look of the source filling it.
			if source.has_video() && !source.is_role() {
				let own = Selection::Source(name.clone());
				egui::CollapsingHeader::new("Its own crop and filters")
					.id_salt("source look")
					.show(ui, |ui| {
						ui.label(
							egui::RichText::new("Before the layer's own, on every layer showing it")
								.color(kit::DIM)
								.font(kit::ui(11.0)),
						);
						if let Some((look, label, gesture)) = self.look_ui(ui, collection, &own, "source") {
							asked = Some(Asked::Look(own.clone(), look, label, gesture));
						}
					});
			}
		}

		let Some(asked) = asked else { return };
		let result = match asked {
			Asked::Item(item, label, gesture) => edit(&mut state, label, gesture, |draft| {
				draft.set_item(&picked.layer, picked.index, item)
			}),
			Asked::Look(selection, look, label, gesture) => {
				edit(&mut state, label, gesture, |draft| selection.set_look(draft, look))
			}
			Asked::Act(action) => {
				let (_, result) = self.act(ui.ctx(), &mut state, &picked, action);
				result
			}
			Asked::Play(name) => match state.sources.get(&name) {
				Some(feed) => feed.play().with_context(|| format!("cannot play `{name}`")),
				None => Ok(()),
			},
			Asked::Stop(name) => match state.sources.get(&name) {
				Some(feed) => feed.stop().with_context(|| format!("cannot stop `{name}`")),
				None => Ok(()),
			},
			Asked::Reload(name) => {
				state.restart(&name);
				Ok(())
			}
			Asked::Fill(role, source) => {
				let label = match &source {
					Some(source) => format!("Fill the role `{role}` with `{source}`"),
					None => format!("Leave the role `{role}` unfilled"),
				};
				edit(&mut state, label, None, |draft| {
					draft.set_source(&role, Some(Source::Role { source }))
				})
			}
			Asked::Publish(name, published) => {
				let verb = if published { "Publish" } else { "Stop publishing" };
				edit(&mut state, format!("{verb} `{name}` independently"), None, |draft| {
					draft.set_published(&name, published)
				})
			}
		};
		drop(state);
		self.fail(spot, result);
	}

	/// The filters in order of `selection`, a layer's blend mode, a source's
	/// crop, and copying and pasting the filters; the look an edit asked for,
	/// its label, and its gesture. `id` keeps two on one page apart.
	fn look_ui(
		&mut self,
		ui: &mut Ui,
		collection: &Collection,
		selection: &Selection,
		id: &str,
	) -> Option<(Look, String, Option<u64>)> {
		let look = selection.look(collection)?;
		let what = selection.describe(collection);
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
		ui.push_id(id, |ui| {
			if let Selection::Source(_) = selection {
				ui.horizontal(|ui| {
					ui.label(egui::RichText::new("Crop").color(kit::DIM));
					if let Some(gesture) = crop_ui(ui, &mut next.crop, &mut self.gestures) {
						action = Some((format!("Crop {what}"), gesture));
					}
				});
			}
			let count = next.filters.len();
			let mut moved = None;
			let mut remove = None;
			for (index, filter) in next.filters.iter_mut().enumerate() {
				ui.push_id(index, |ui| {
					ui.horizontal_wrapped(|ui| {
						if let Some(gesture) = filter_ui(ui, filter, &pictures, &luts, &mut self.gestures) {
							action = Some((format!("Set {} on {what}", filter_name(filter)), gesture));
						}
						if ui
							.add_enabled(index > 0, IconButton::new(Icon::Up, "Earlier").small())
							.clicked()
						{
							moved = Some((index, index - 1));
						}
						if ui
							.add_enabled(index + 1 < count, IconButton::new(Icon::Down, "Later").small())
							.clicked()
						{
							moved = Some((index, index + 1));
						}
						if ui.add(IconButton::new(Icon::Remove, "Remove").small()).clicked() {
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
			ui.horizontal(|ui| {
				let add = ui.add(kit::button(Icon::Add, "Add a filter"));
				egui::Popup::menu(&add).show(|ui| {
					for filter in templates(&pictures, &luts) {
						if ui.button(filter_name(&filter)).clicked() {
							action = Some((format!("Add {} to {what}", filter_name(&filter)), None));
							next.filters.push(filter);
						}
					}
				});
				let filters = look.filters.clone();
				if ui
					.add_enabled(
						!filters.is_empty(),
						IconButton::new(Icon::Copy, "Copy the filters").small(),
					)
					.clicked()
				{
					let clip = Clip::Filters(moq_scene::clip::Filters { filters });
					match clip.to_toml() {
						Ok(text) => ui.ctx().copy_text(text),
						Err(err) => self.say(Err(err.to_string())),
					}
				}
				// Pasted filters land on the selected layer.
				if let Selection::Item(..) = selection
					&& ui.add(IconButton::new(Icon::Paste, "Paste filters").small()).clicked()
				{
					// Arrives as a paste event on a later frame, adding the filters.
					ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
				}
			});
			if let Some(blend) = &mut next.blend {
				ui.horizontal(|ui| {
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
				});
			}
		});
		let (label, gesture) = action?;
		Some((next, label, gesture))
	}

	/// The settings of the scene BUILD edits, with no layer selected: its
	/// name, whether it is safe or published, and how every scene goes on air.
	fn scene_inspector(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let collection = state.collection();
		let program = self.edited(&state).map(str::to_string);
		let found = program
			.as_deref()
			.and_then(|name| collection.scenes.iter().position(|scene| scene.name == name));
		let Some(index) = found else {
			ui.label("Take a scene to edit it.");
			return;
		};
		if let Some(fault) = self.fault(&Spot::Scene) {
			ui.add_space(6.0);
			ui.colored_label(kit::ERR, fault);
		}
		let scene = collection.scenes[index].clone();
		let audio_scenes = collection.audio_scenes.clone();
		let program_audio = collection.program_audio().unwrap_or_default().to_string();
		let mut action = None;
		section(ui, "Scene");
		ui.horizontal(|ui| {
			match &mut self.renaming {
				Some((renaming, name)) if *renaming == index => {
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
				}
				_ => {
					ui.label(kit::caps(&scene.name, kit::display(16.0)));
					if ui.add(IconButton::new(Icon::Rename, "Rename").small()).clicked() {
						self.renaming = Some((index, scene.name.clone()));
					}
				}
			}
		});
		let size = match scene.size {
			Some(size) => format!("{}x{}, a size of its own", size.width, size.height),
			None => format!(
				"{}x{}, the show's size",
				collection.canvas.width, collection.canvas.height
			),
		};
		ui.label(egui::RichText::new(size).color(kit::DIM));
		ui.horizontal_wrapped(|ui| {
			if let Some(settings) = scene_settings(ui, index, &scene, &audio_scenes, &program_audio) {
				action = Some(settings);
			}
		});
		ui.label(
			egui::RichText::new("Select a layer, on the picture or in Layers, to edit it.")
				.color(kit::DIM)
				.font(kit::ui(12.0)),
		);
		let slotted = self.slots(ui, &mut state, index);
		if let Some(action) = action {
			let result = self.scene_action(&mut state, action);
			drop(state);
			self.fail(Spot::Scene, result);
		} else {
			drop(state);
		}
		if let Some(result) = slotted {
			self.fail(Spot::Scene, result);
		}
		section(ui, "Every scene");
		self.transition(ui);
	}

	/// How a scene goes on air, which a pad's click or key takes it by.
	pub(super) fn transition(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let transition = state.collection().transition.clone();
		let mut next = transition.clone();
		let media: Vec<String> = state
			.collection()
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Media { .. }))
			.map(|(name, _)| name.clone())
			.collect();
		let mut gesture = None;
		ui.horizontal_wrapped(|ui| {
			ui.label("Transition");
			match &next {
				Transition::Slide { direction, .. } | Transition::Wipe { direction, .. } => self.direction = *direction,
				Transition::Stinger { cut, .. } => self.cut = *cut,
				_ => {}
			}
			if let Some(duration) = next.duration() {
				self.duration = duration;
			}
			let (duration, direction) = (self.duration, self.direction);
			// A stinger plays a media source's clip, so it is offered only
			// once there is one.
			let stinger = match &next {
				Transition::Stinger { .. } => Some(next.clone()),
				_ => media.first().map(|source| Transition::Stinger {
					source: source.clone(),
					cut: self.cut,
				}),
			};
			egui::ComboBox::from_id_salt("transition")
				.selected_text(transition_name(&next))
				.show_ui(ui, |ui| {
					let kinds = [
						Transition::Fade { duration },
						Transition::Slide { duration, direction },
						Transition::Wipe { duration, direction },
						Transition::Move { duration },
					];
					for kind in kinds.into_iter().chain(stinger) {
						let name = transition_name(&kind);
						ui.selectable_value(&mut next, kind, name);
					}
				})
				.response
				.on_hover_text("How a scene goes on air; a stinger plays a media source's clip over it");
			match &mut next {
				Transition::Slide { direction, .. } | Transition::Wipe { direction, .. } => {
					egui::ComboBox::from_id_salt("direction")
						.selected_text(direction_name(*direction))
						.show_ui(ui, |ui| {
							for to in Direction::ALL {
								ui.selectable_value(direction, to, direction_name(to));
							}
						});
				}
				Transition::Stinger { source, cut } => {
					egui::ComboBox::from_id_salt("stinger")
						.selected_text(source.as_str())
						.show_ui(ui, |ui| {
							for name in &media {
								ui.selectable_value(source, name.clone(), name);
							}
						})
						.response
						.on_hover_text("The media source whose clip plays");
					let mut millis = cut.as_millis() as u64;
					let response = ui
						.add(egui::DragValue::new(&mut millis).prefix("cut at ").suffix(" ms"))
						.on_hover_text("How far into the clip the scenes switch, while it covers the canvas");
					let dragging = self.gestures.of(&response);
					if response.changed() {
						*cut = Duration::from_millis(millis);
						gesture = dragging;
					}
				}
				_ => {}
			}
			if let Some(duration) = next.duration_mut() {
				let mut millis = duration.as_millis() as u64;
				let response = ui
					.add(egui::DragValue::new(&mut millis).suffix(" ms"))
					.on_hover_text("How long the transition takes");
				let dragging = self.gestures.of(&response);
				if response.changed() {
					*duration = Duration::from_millis(millis);
					gesture = dragging;
				}
			}
		});
		let result = if next == transition {
			Ok(())
		} else {
			edit(&mut state, "Set the transition", gesture, |draft| {
				draft.set_transition(next)
			})
		};
		drop(state);
		self.fail(Spot::Scene, result);
	}

	/// Make the edit the scene's inspector, its pad, or the graph picked for a scene.
	pub(super) fn scene_action(&mut self, state: &mut State, action: SceneAction) -> anyhow::Result<()> {
		let name = state.collection().scenes[action.index()].name.clone();
		match action {
			SceneAction::Rename(index, to) => {
				let result = edit(state, format!("Rename scene `{name}` to `{to}`"), None, |draft| {
					draft.rename_scene(index, to.clone())
				});
				if result.is_ok() {
					self.renamed(&name, &to);
				}
				result
			}
			SceneAction::Move(from, to) => edit(state, format!("Reorder scene `{name}`"), None, |draft| {
				draft.move_scene(from, to)
			}),
			SceneAction::Publish(index, published) => {
				let scene = state.collection().scenes[index].clone();
				let label = match published {
					true => format!("Publish scene `{name}`"),
					false => format!("Stop publishing scene `{name}`"),
				};
				edit(state, label, None, |draft| {
					draft.set_scene(index, Scene { published, ..scene })
				})
			}
			SceneAction::Safe(_, safe) => {
				let label = match safe {
					true => format!("Make scene `{name}` the safe scene"),
					false => format!("Stop cutting to scene `{name}` on panic"),
				};
				edit(state, label, None, |draft| {
					draft.set_safe(safe.then_some(name.as_str()))
				})
			}
			SceneAction::Audio(index, audio) => {
				let scene = state.collection().scenes[index].clone();
				let label = match &audio {
					Some(audio) => format!("Have scene `{name}` carry audio scene `{audio}`"),
					None => format!("Have scene `{name}` carry the audio scene on program"),
				};
				edit(state, label, None, |draft| {
					draft.set_scene(index, Scene { audio, ..scene })
				})
			}
			SceneAction::Remove(index) => {
				let result = edit(state, format!("Remove scene `{name}`"), None, |draft| {
					draft.remove_scene(index)
				});
				if result.is_ok() {
					// Removing its scene sources removes the items showing
					// them, which moves the ones after them.
					self.deselect_item();
				}
				result
			}
		}
	}

	/// The selected layer's keyframes: when each is, how the layer eases into
	/// it, and how far it takes the layer from where it rests.
	fn keyframes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let selected = self.selection.clone().and_then(|picked| {
			let item = picked.item(state.collection())?.clone();
			Some((picked, item))
		});
		let Some((Picked { layer, index }, item)) = selected else {
			ui.label("Select a layer to animate it.");
			return;
		};
		if layer.scene.is_none() {
			ui.label("An always-on-top layer plays over every scene, so it has no keyframes to play.");
			return;
		}
		let spot = Spot::Layer(layer.clone(), item.source.clone());
		let what = format!("`{}`", item.source);
		let mut play = false;
		// Play replays a scene open off air in its own picture, never on air.
		let replays = match self.off_air(&state) {
			Some(_) => "Play the keyframes of the scene open off air again from the start, in its own picture only",
			None => "Play the keyframes of the scene on air again from the start, on air",
		};
		ui.horizontal(|ui| {
			ui.strong(format!("Keyframes of {what}"));
			ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
				play = ui.add(kit::button(Icon::Play, "Play")).on_hover_text(replays).clicked();
			});
		});
		ui.label(
			egui::RichText::new(
				"Each keyframe moves the layer away from where it rests, so dragging it on the picture \
				 moves its whole animation. They play once, from when its scene goes on, and the last \
				 one holds.",
			)
			.color(kit::DIM)
			.font(kit::ui(12.0)),
		);

		let mut next = item.keyframes.clone();
		let mut action: Option<(String, Option<u64>)> = None;
		let mut remove = None;
		{
			for at in 0..next.len() {
				// Between its neighbors, so the keyframes stay in order.
				let earliest = at.checked_sub(1).map_or(0, |before| next[before].at.as_millis() as u64);
				let latest = next.get(at + 1).map_or(u64::MAX, |after| after.at.as_millis() as u64);
				let keyframe = &mut next[at];
				ui.push_id(at, |ui| {
					ui.horizontal_wrapped(|ui| {
						let mut millis = keyframe.at.as_millis() as u64;
						let response = ui
							.add(egui::DragValue::new(&mut millis).range(earliest..=latest).suffix(" ms"))
							.on_hover_text("How long after its scene goes on");
						if let Some(gesture) = changed(&response, &mut self.gestures) {
							keyframe.at = Duration::from_millis(millis);
							action = Some((format!("Time a keyframe of {what}"), gesture));
						}
						let before = keyframe.ease;
						egui::ComboBox::from_id_salt("ease")
							.selected_text(ease_name(keyframe.ease))
							.show_ui(ui, |ui| {
								for ease in Ease::ALL {
									ui.selectable_value(&mut keyframe.ease, ease, ease_name(ease));
								}
							})
							.response
							.on_hover_text("How the layer eases into this keyframe from the one before");
						if keyframe.ease != before {
							action = Some((format!("Ease a keyframe of {what}"), None));
						}
						let offsets = [
							("x", &mut keyframe.x),
							("y", &mut keyframe.y),
							("width", &mut keyframe.width),
							("height", &mut keyframe.height),
						];
						for (label, offset) in offsets {
							ui.weak(label);
							let response = ui.add(egui::DragValue::new(offset).suffix(" px"));
							if let Some(gesture) = changed(&response, &mut self.gestures) {
								action = Some((format!("Move a keyframe of {what}"), gesture));
							}
						}
						let crop = &mut keyframe.crop;
						let edges = [
							("crop left", &mut crop.left),
							("top", &mut crop.top),
							("right", &mut crop.right),
							("bottom", &mut crop.bottom),
						];
						for (label, edge) in edges {
							ui.weak(label);
							let response = ui.add(egui::DragValue::new(edge).range(0.0..=0.95).speed(0.002));
							if let Some(gesture) = changed(&response, &mut self.gestures) {
								action = Some((format!("Crop a keyframe of {what}"), gesture));
							}
						}
						if ui.small_button("Remove").clicked() {
							remove = Some(at);
						}
					});

					let mut dropped = None;
					for (index, filter) in keyframe.filters.iter_mut().enumerate() {
						ui.push_id(index, |ui| {
							ui.horizontal_wrapped(|ui| {
								if let Some(gesture) = filter_ui(ui, filter, &[], &[], &mut self.gestures) {
									let label = format!("Set {} on a keyframe of {what}", filter_name(filter));
									action = Some((label, gesture));
								}
								if ui.small_button("Remove").clicked() {
									dropped = Some(index);
								}
							});
						});
					}
					if let Some(index) = dropped {
						keyframe.filters.remove(index);
						action = Some((format!("Remove a filter from a keyframe of {what}"), None));
					}
					// A keyframe's filters read no other source, so no LUT or mask.
					egui::ComboBox::from_id_salt("add filter")
						.selected_text("Add filter")
						.show_ui(ui, |ui| {
							for filter in templates(&[], &[]) {
								if ui.selectable_label(false, filter_name(&filter)).clicked() {
									let label = format!("Add {} to a keyframe of {what}", filter_name(&filter));
									action = Some((label, None));
									keyframe.filters.push(filter);
								}
							}
						})
						.response
						.on_hover_text(
							"Applied after the layer's own. Settings mix into the next keyframe's when \
							 its filters are of the same kinds, in order, and otherwise change halfway.",
						);
					ui.separator();
				});
			}
		}
		if let Some(at) = remove {
			next.remove(at);
			action = Some((format!("Remove a keyframe from {what}"), None));
		}
		if ui
			.add(kit::button(Icon::Add, "Add a keyframe"))
			.on_hover_text("Half a second after the last, where the layer rests, with the last one's filters")
			.clicked()
		{
			let keyframe = match next.last() {
				Some(last) => Keyframe {
					ease: last.ease,
					filters: last.filters.clone(),
					..Keyframe::new(last.at + Duration::from_millis(500))
				},
				None => Keyframe::new(Duration::ZERO),
			};
			next.push(keyframe);
			action = Some((format!("Add a keyframe to {what}"), None));
		}

		if play {
			match self.off_air(&state).map(str::to_string) {
				// Replayed where it is drawn, never on program.
				Some(cued) => state.compositor.set_preview(Some(cued)),
				None => state.compositor.set_program_start(kio::clock::now()),
			}
		}
		let Some((label, gesture)) = action else { return };
		let result = edit(&mut state, label, gesture, |draft| {
			draft.set_item(
				&layer,
				index,
				Item {
					keyframes: next,
					..item
				},
			)
		});
		drop(state);
		self.fail(spot, result);
	}
}

/// Edit `crop`'s four edges as percents, each leaving some of the picture;
/// the gesture of the edit, if it made one.
fn crop_ui(ui: &mut Ui, crop: &mut Crop, gestures: &mut Gestures) -> Option<Option<u64>> {
	let mut edited = None;
	let most = 95.0;
	let mut percents = [crop.left, crop.top, crop.right, crop.bottom].map(|edge| edge * 100.0);
	for at in 0..4 {
		// Each edge leaves what its opposite one has not taken.
		let opposite = percents[(at + 2) % 4];
		let range = 0.0..=(most - opposite).max(0.0);
		let response = ui.add(
			egui::DragValue::new(&mut percents[at])
				.range(range)
				.speed(0.2)
				.max_decimals(1)
				.suffix("%"),
		);
		if let Some(gesture) = changed(&response, gestures) {
			edited = Some(gesture);
		}
	}
	let [left, top, right, bottom] = percents.map(|percent| percent / 100.0);
	*crop = Crop {
		left,
		top,
		right,
		bottom,
	};
	edited
}
