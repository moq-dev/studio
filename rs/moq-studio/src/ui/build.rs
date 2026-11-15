//! The BUILD page: the scenes, their layers, the sources, and the selected
//! layer's inspector, around the live picture they edit.

use super::*;

impl Panels {
	/// The BUILD page: the layers and the sources on the left, the live
	/// picture with its tools in the middle, and the selection's inspector on
	/// the right, or among the left's tabs when the window is narrow.
	pub(super) fn build(&mut self, ui: &mut Ui, fx: &mut Fx, stage: egui::Rect, shape: f32) {
		let side = (stage.width() * 0.22).clamp(260.0, 320.0);
		let wide = stage.width() >= side * 2.0 + 360.0;
		let left = stage.with_max_x(stage.left() + side);
		let right = wide.then(|| stage.with_min_x(stage.right() - side));
		let middle = stage
			.with_min_x(left.right())
			.with_max_x(right.map_or(stage.right(), |right| right.left()));
		let line = egui::Stroke::new(1.0, kit::LINE);
		super::shell::bar(ui, fx, left);
		ui.painter().vline(left.right() - 0.5, left.y_range(), line);
		if let Some(right) = right {
			super::shell::bar(ui, fx, right);
			ui.painter().vline(right.left() + 0.5, right.y_range(), line);
		}
		let tools: &[Tool] = if wide { &Tool::LEFT } else { &Tool::ALL };
		if !tools.contains(&self.left) {
			self.left = Tool::Layers;
		}
		let mut tool = self.left;
		self.column(ui, left, &mut tool, tools);
		self.left = tool;
		if let Some(right) = right {
			let mut tool = self.right;
			self.column(ui, right, &mut tool, &Tool::RIGHT);
			self.right = tool;
		}
		kit::wash(ui.painter(), middle);
		let bar = middle.with_max_y(middle.top() + 44.0).shrink2(egui::vec2(16.0, 6.0));
		let room = middle.with_min_y(bar.bottom()).shrink(28.0);
		self.stage(ui, fx, bar, super::layout::fit(room, shape));
	}

	/// A column of `tools` in `rect`, showing `tool` under their tabs.
	fn column(&mut self, ui: &mut Ui, rect: egui::Rect, tool: &mut Tool, tools: &[Tool]) {
		ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(12.0)), |ui| {
			super::shell::tabs(ui, tool, tools, Tool::name);
			ui.add_space(8.0);
			egui::ScrollArea::both()
				.id_salt(("tool", rect.min.x as i32))
				.auto_shrink(false)
				.show(ui, |ui| match *tool {
					Tool::Layers => self.items(ui),
					Tool::Scenes => {
						self.transition(ui);
						ui.separator();
						self.scenes(ui);
					}
					Tool::Sources => self.sources(ui),
					Tool::Devices => self.devices(ui),
					Tool::Guests => self.guests(ui),
					Tool::Filters => self.filters(ui),
					Tool::Keyframes => self.keyframes(ui),
				});
		});
	}

	/// The live picture at `at`, where a drag arranges the items of the
	/// layer the Layers tab shows, or the mouse and keyboard pass to browser
	/// sources while Interact is on, with its tools in `bar`.
	fn stage(&mut self, ui: &mut Ui, fx: &mut Fx, bar: egui::Rect, at: egui::Rect) {
		self.staged = true;
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let canvas = self.program_drawn;
		let builder = egui::UiBuilder::new()
			.max_rect(bar)
			.layout(egui::Layout::left_to_right(egui::Align::Center));
		ui.scope_builder(builder, |ui| {
			let toggled = ui
				.add(kit::Toggle::new(&mut self.interacting).label("Interact"))
				.on_hover_text(
					"Pass the mouse to the browser source under it, and the keyboard to the one last clicked",
				)
				.changed();
			if toggled && !self.interacting {
				self.rebind(&state.sources);
				deliver(&state.sources, self.interact.release());
			}
			ui.separator();
			let free = ui.ctx().format_modifiers(egui::Modifiers::COMMAND);
			let flip = ui.ctx().format_modifiers(egui::Modifiers::SHIFT);
			ui.add(kit::Toggle::new(&mut self.snapping.on).label("Snap"))
				.on_hover_text(format!(
					"Snap a dragged item to the canvas, other items, and the grid. Hold {free} to drag freely.\n\
				 Resizing keeps the shape of a camera, screen, image, or video, but not of other sources; hold \
				 {flip} to flip that. With the picture focused, arrow keys nudge the selected item, ten pixels \
				 with {flip}."
				));
			ui.add(kit::Toggle::new(&mut self.snapping.grid).label("Grid"));
			let largest = canvas.width.max(canvas.height);
			ui.add_enabled(
				self.snapping.grid,
				egui::DragValue::new(&mut self.snapping.spacing)
					.range(2..=largest)
					.suffix(" px"),
			);
		});
		// It is the live picture, so the accent frame stays while editing it.
		let frame = TallyFrame::new(Tally::Air).tag("On air");
		let id = self.program.id;
		let builder = egui::UiBuilder::new().max_rect(at.expand(10.0));
		// Interacting, the pointer belongs to the pages rather than to arranging.
		if self.interacting {
			let image = ui
				.scope_builder(builder, |ui| {
					frame.show(ui, Some(fx), |ui| picture(ui, id, canvas, egui::Sense::hover()))
				})
				.inner
				.rect;
			let since = state.compositor.program_since();
			let scene = state.compositor.program_scene().map(|scene| (scene, since));
			self.rebind(&state.sources);
			let delivered = self.interact.update(ui, image, state.collection(), scene);
			deliver(&state.sources, delivered);
			self.rebind(&state.sources);
			return;
		}
		let response = ui
			.scope_builder(builder, |ui| {
				frame.show(ui, Some(fx), |ui| {
					picture(ui, id, canvas, egui::Sense::click_and_drag())
				})
			})
			.inner;
		let result = self.arrange(ui, &mut state, &response, canvas);
		drop(state);
		self.check(result);
	}

	/// Select, move, and resize the items of the layer the Layers tab shows
	/// by dragging them on the live picture, snapping as they go. An item is
	/// grabbed where its keyframes have it on the picture, and a drag moves
	/// where it rests, which carries its keyframes along.
	pub(super) fn arrange(
		&mut self,
		ui: &Ui,
		state: &mut State,
		response: &egui::Response,
		canvas: Canvas,
	) -> anyhow::Result<()> {
		let gesture = self.gestures.of(response);
		// The release still moves the item to where the pointer let go.
		if !response.dragged() && !response.drag_stopped() {
			self.drag = None;
		}
		// A panel squeezed to nothing has no picture to map the pointer onto.
		if !response.rect.is_positive() {
			return Ok(());
		}
		let view = View::new(response.rect, canvas);
		let painter = ui.painter_at(response.rect);
		if self.snapping.grid {
			view.grid(&painter, self.snapping.spacing as f32);
		}

		let Ok(layer) = self.layer(state) else { return Ok(()) };
		let Some(resting) = state.collection().items(&layer).map(<[Item]>::to_vec) else {
			return Ok(());
		};
		let since = state.compositor.program_since();
		let items: Vec<Item> = resting.iter().map(|item| item.posed(since).into_owned()).collect();
		let selected = match &self.selection {
			Some(Selection::Item(selected, index)) if *selected == layer => Some(*index),
			_ => None,
		};
		let reach = view.canvas(REACH);
		let hit = |pos: egui::Pos2| snap::hit(&items, selected, view.point(pos), reach);

		// Focused, the picture takes the arrow keys, which nudge.
		if response.clicked() || response.drag_started() {
			response.request_focus();
		}
		if response.clicked() {
			match response.interact_pointer_pos().and_then(hit) {
				Some((index, _)) => self.selection = Some(Selection::Item(layer.clone(), index)),
				None => self.deselect_item(),
			}
		}
		if response.drag_started()
			&& let Some(origin) = ui.input(|input| input.pointer.press_origin())
			&& let Some((index, grab)) = hit(origin)
		{
			self.selection = Some(Selection::Item(layer.clone(), index));
			let collection = state.collection();
			let source = collection.sources.get(collection.capture(&resting[index].source));
			self.drag = Some(Drag {
				layer: layer.clone(),
				index,
				grab,
				rect: resting[index].rect,
				origin: view.point(origin),
				fixed: fixed_shape(source),
				gesture,
				hints: None,
			});
		}

		let mut result = Ok(());
		if let Some(drag) = &mut self.drag
			&& let Some(pos) = response.interact_pointer_pos()
		{
			let point = view.point(pos);
			// Whole canvas pixels, so an unsnapped drag saves round numbers.
			let delta = [(point[0] - drag.origin[0]).round(), (point[1] - drag.origin[1]).round()];
			let item = state
				.collection()
				.items(&drag.layer)
				.and_then(|items| items.get(drag.index))
				.cloned();
			let held = ui.input(|input| input.modifiers);
			// Shift flips whether a resize keeps the item's shape.
			let aspect = (drag.fixed != held.shift).then(|| snap::aspect(&drag.rect)).flatten();
			let rect = drag.grab.drag(drag.rect, delta, aspect);
			drag.hints = (self.snapping.on && !held.command).then(|| {
				// How far the keyframes have the item from where it rests.
				let shown = item.as_ref().map_or(drag.rect, |item| item.posed(since).rect);
				let rest = item.as_ref().map_or(drag.rect, |item| item.rect);
				let snap = Snap {
					reach,
					grid: self.snapping.grid.then_some(self.snapping.spacing as f32),
					aspect,
					posed: Rect {
						x: shown.x - rest.x,
						y: shown.y - rest.y,
						width: shown.width - rest.width,
						height: shown.height - rest.height,
					},
				};
				let others = others(state, &drag.layer, drag.index, since);
				snap.apply(drag.grab, rect, &others, &canvas)
			});
			let rect = drag.hints.as_ref().map_or(rect, |snapped| snapped.rect);
			if let Some(item) = item
				&& item.rect != rect
			{
				let verb = if drag.grab == Grab::Move { "Move" } else { "Resize" };
				let label = format!("{verb} `{}`", item.source);
				result = edit(state, label, drag.gesture, |draft| {
					draft.set_item(&drag.layer, drag.index, Item { rect, ..item })
				});
			}
		}
		if !response.dragged() {
			self.drag = None;
		}
		if response.has_focus() && self.drag.is_none() && result.is_ok() {
			result = self.nudge(ui, state, &layer, response.id);
		}

		// Drawn from the collection and selection as this frame left them.
		let items = state.collection().items(&layer).unwrap_or_default();
		let items: Vec<Item> = items.iter().map(|item| item.posed(since).into_owned()).collect();
		let selected = match &self.selection {
			Some(Selection::Item(selected, index)) if *selected == layer => Some(*index),
			_ => None,
		};
		let accent = ui.visuals().selection.stroke.color;
		// A hidden item has no handles, as the picture cannot grab it.
		if let Some(item) = selected.and_then(|index| items.get(index)).filter(|item| item.visible) {
			view.selected(&painter, &item.rect, accent);
		}
		let grab = match &self.drag {
			Some(drag) => {
				if let Some(snapped) = &drag.hints {
					view.hints(&painter, snapped);
				}
				Some(drag.grab)
			}
			None => response.hover_pos().and_then(hit).map(|(index, grab)| {
				if selected != Some(index)
					&& let Some(item) = items.get(index)
				{
					view.hovered(&painter, &item.rect, accent);
				}
				grab
			}),
		};
		if let Some(grab) = grab {
			ui.ctx().set_cursor_icon(cursor(grab));
		}
		result
	}

	/// Move the selected item of `layer` a canvas pixel for each arrow key
	/// pressed while the picture, `id`, has focus, or ten with Shift held. A
	/// held key undoes as one move.
	pub(super) fn nudge(&mut self, ui: &Ui, state: &mut State, layer: &Layer, id: egui::Id) -> anyhow::Result<()> {
		let arrows = egui::EventFilter {
			horizontal_arrows: true,
			vertical_arrows: true,
			..Default::default()
		};
		ui.memory_mut(|memory| memory.set_focus_lock_filter(id, arrows));
		let index = match &self.selection {
			Some(Selection::Item(selected, index)) if selected == layer => *index,
			_ => return Ok(()),
		};
		let mut delta = [0.0_f32; 2];
		let mut fresh = false;
		ui.input(|input| {
			for event in &input.events {
				let egui::Event::Key {
					key,
					pressed: true,
					repeat,
					modifiers,
					..
				} = event
				else {
					continue;
				};
				let step = if modifiers.shift { 10.0 } else { 1.0 };
				let [x, y] = match key {
					egui::Key::ArrowLeft => [-step, 0.0],
					egui::Key::ArrowRight => [step, 0.0],
					egui::Key::ArrowUp => [0.0, -step],
					egui::Key::ArrowDown => [0.0, step],
					_ => continue,
				};
				delta = [delta[0] + x, delta[1] + y];
				fresh |= !repeat;
			}
		});
		let Some(item) = state
			.collection()
			.items(layer)
			.and_then(|items| items.get(index))
			.cloned()
		else {
			return Ok(());
		};
		if delta == [0.0; 2] {
			return Ok(());
		}
		let gesture = match fresh {
			true => self.gestures.next(),
			false => self.gestures.0,
		};
		let rect = Rect {
			x: item.rect.x + delta[0],
			y: item.rect.y + delta[1],
			..item.rect
		};
		let label = format!("Move `{}`", item.source);
		edit(state, label, Some(gesture), |draft| {
			draft.set_item(layer, index, Item { rect, ..item })
		})
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
		self.check(result);
	}

	pub(super) fn scenes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let program = state.compositor.program_scene().map(str::to_owned);
		let on_air: Vec<String> = state.compositor.on_air().map(str::to_owned).collect();
		let mut action = None;
		let mut audio_action = None;
		let count = state.collection().scenes.len();
		let audio_scenes = state.collection().audio_scenes.clone();
		let program_audio = state.collection().program_audio().unwrap_or_default().to_string();

		egui::ScrollArea::vertical().show(ui, |ui| {
			for (index, scene) in state.collection().scenes.iter().enumerate() {
				ui.vertical(|ui| {
					if let Some((renaming, name)) = &mut self.renaming
						&& *renaming == index
					{
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
						return;
					}
					// Taken from its pad; here it is named, sized, and set.
					ui.horizontal(|ui| {
						let name =
							egui::Label::new(kit::caps(&scene.name, kit::display(14.0))).sense(egui::Sense::click());
						if ui.add(name).double_clicked() {
							self.renaming = Some((index, scene.name.clone()));
						}
						if program.as_deref() == Some(scene.name.as_str()) {
							ui.add(kit::Tag::new("On air", Tally::Air));
						}
						if let Some(size) = scene.size {
							ui.weak(format!("{}x{}", size.width, size.height));
						}
					});
					ui.horizontal_wrapped(|ui| {
						if let Some(settings) = scene_settings(ui, index, scene, &audio_scenes, &program_audio) {
							action = Some(settings);
						}
						ui.horizontal(|ui| {
							if ui.add(IconButton::new(Icon::Rename, "Rename").small()).clicked() {
								self.renaming = Some((index, scene.name.clone()));
							}
							if ui
								.add_enabled(index > 0, IconButton::new(Icon::Up, "Move up").small())
								.clicked()
							{
								action = Some(SceneAction::Move(index, index - 1));
							}
							let down = IconButton::new(Icon::Down, "Move down").small();
							if ui.add_enabled(index + 1 < count, down).clicked() {
								action = Some(SceneAction::Move(index, index + 1));
							}
							let live = on_air.contains(&scene.name);
							let remove = IconButton::new(Icon::Remove, "Remove")
								.small()
								.note(live.then_some("On air; take another scene first"));
							if ui.add_enabled(!live, remove).clicked() {
								action = Some(SceneAction::Remove(index));
							}
						});
					});
				});
			}
			// Audio scenes, which no pad takes: the stream carries the one on
			// program.
			let count = audio_scenes.len();
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
					ui.add(kit::Tag::new("Audio", Tally::Off));
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
							.then_some("On program; put another audio scene on program in Audio first"),
					);
					if ui.add_enabled(!scene.program, remove).clicked() {
						audio_action = Some(AudioAction::Remove(index));
					}
				});
			}
		});

		ui.separator();
		ui.horizontal(|ui| {
			ui.text_edit_singleline(&mut self.new_scene);
			if ui.button("Add audio scene").clicked() {
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
			if ui.add(kit::button(Icon::Add, "Add scene")).clicked() {
				let name = self.new_scene.trim().to_string();
				let index = state.collection().scenes.len();
				let result = edit(&mut state, format!("Add scene `{name}`"), None, |draft| {
					draft.insert_scene(index, Scene::new(name.clone()))
				});
				match result {
					Ok(()) => self.new_scene.clear(),
					Err(err) => self.report(Err(err)),
				}
			}
		});

		if let Some(action) = audio_action {
			let result = self.audio_scene(&mut state, &audio_scenes, action);
			self.check(result);
		}
		let Some(action) = action else { return };
		let result = self.scene_action(&mut state, action);
		drop(state);
		self.check(result);
	}

	/// Make the edit the Scenes or Graph panel picked for a scene.
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

	/// Rename, reorder, or remove one of `scenes`, the audio scenes as the
	/// Scenes panel listed them. Listening follows a renamed one.
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

	/// The items of the scene on program or the top layer, back to front.
	pub(super) fn items(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let scene = state.compositor.program_scene().unwrap_or("no scene").to_string();
		ui.horizontal(|ui| {
			ui.selectable_value(&mut self.top, false, format!("Scene: {scene}"));
			ui.selectable_value(&mut self.top, true, "Top layer")
				.on_hover_text("Drawn over every scene on program");
		});
		ui.separator();
		let layer = match self.layer(&state) {
			Ok(layer) => layer,
			Err(message) => {
				ui.label(message);
				return;
			}
		};
		let collection = state.collection();
		let Some(items) = collection.items(&layer).map(<[Item]>::to_vec) else {
			ui.label(format!("There is no {layer}."));
			return;
		};
		// The layer's own size as the collection has it now, rather than the
		// program's as last drawn, which lags an edit by a frame.
		let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
		let mut action = None;

		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("items").striped(true).show(ui, |ui| {
				for heading in ["Source", "Shown", "Place", "", "Size", "", ""] {
					ui.label(kit::caps(heading, kit::display(11.0)).color(kit::DIM));
				}
				ui.end_row();

				let count = items.len();
				for (index, item) in items.iter().enumerate() {
					let selection = Selection::Item(layer.clone(), index);
					ui.horizontal(|ui| {
						let selected = self.selection.as_ref() == Some(&selection);
						if ui.selectable_label(selected, &item.source).clicked() {
							self.selection = Some(selection);
						}
						linked(ui, collection, &layer, index, item);
					});

					let (icon, name) = match item.visible {
						true => (Icon::Shown, "Hide"),
						false => (Icon::Hidden, "Show"),
					};
					if ui.add(IconButton::new(icon, name).small()).clicked() {
						let visible = !item.visible;
						let verb = if visible { "Show" } else { "Hide" };
						let label = format!("{verb} `{}`", item.source);
						action = Some(ItemAction::Set(
							index,
							Item {
								visible,
								..item.clone()
							},
							label,
							None,
						));
					}

					let mut rect = item.rect;
					let field = |drag, label| kit::Number::new(drag).label(label).width(64.0);
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
						let gesture = self.gestures.of(&response);
						if response.changed() {
							let label = format!("{verb} `{}`", item.source);
							action = Some(ItemAction::Set(index, Item { rect, ..item.clone() }, label, gesture));
						}
					}

					ui.horizontal(|ui| {
						if ui.small_button("Fill").clicked() {
							let rect = Rect::fill(&canvas);
							let label = format!("Fill with `{}`", item.source);
							action = Some(ItemAction::Set(index, Item { rect, ..item.clone() }, label, None));
						}
						if ui.add_enabled(index > 0, egui::Button::new("Back").small()).clicked() {
							action = Some(ItemAction::Move(index, index - 1));
						}
						if ui
							.add_enabled(index + 1 < count, egui::Button::new("Front").small())
							.clicked()
						{
							action = Some(ItemAction::Move(index, index + 1));
						}
						if ui.add(IconButton::new(Icon::Remove, "Remove").small()).clicked() {
							action = Some(ItemAction::Remove(index));
						}
					});
					ui.end_row();
				}
			});

			ui.separator();
			ui.horizontal(|ui| {
				let pictures: Vec<&String> = collection
					.sources
					.iter()
					.filter(|(_, source)| source.has_video())
					.map(|(name, _)| name)
					.collect();
				// A pick whose source was removed or reloaded away would add a dangling item.
				if self.new_item.as_ref().is_some_and(|name| !pictures.contains(&name)) {
					self.new_item = None;
				}
				egui::ComboBox::from_id_salt("new item")
					.selected_text(self.new_item.as_deref().unwrap_or("Pick a source"))
					.show_ui(ui, |ui| {
						for name in pictures {
							ui.selectable_value(&mut self.new_item, Some(name.clone()), name);
						}
					});
				if ui.add(kit::button(Icon::Add, "Add")).clicked()
					&& let Some(source) = self.new_item.take()
				{
					action = Some(ItemAction::Add(Item::new(source, Rect::fill(&canvas))));
				}
			});
		});

		let Some(action) = action else { return };
		let result = match action {
			ItemAction::Set(index, item, label, gesture) => {
				edit(&mut state, label, gesture, |draft| draft.set_item(&layer, index, item))
			}
			ItemAction::Move(from, to) => {
				let label = format!("Reorder `{}`", items[from].source);
				let result = edit(&mut state, label, None, |draft| draft.move_item(&layer, from, to));
				if result.is_ok() {
					// Adjacent items swap, and the selection stays on its item.
					self.follow(&layer, |index| match index {
						i if i == from => Some(to),
						i if i == to => Some(from),
						i => Some(i),
					});
				}
				result
			}
			ItemAction::Remove(index) => {
				let label = format!("Remove `{}` from {layer}", items[index].source);
				let result = edit(&mut state, label, None, |draft| {
					draft.remove_item(&layer, index).map(drop)
				});
				if result.is_ok() {
					self.follow(&layer, |selected| match selected.cmp(&index) {
						std::cmp::Ordering::Less => Some(selected),
						std::cmp::Ordering::Equal => None,
						std::cmp::Ordering::Greater => Some(selected - 1),
					});
				}
				result
			}
			ItemAction::Add(item) => {
				let label = format!("Add `{}` to {layer}", item.source);
				edit(&mut state, label, None, |draft| {
					draft.insert_item(&layer, items.len(), item)
				})
			}
		};
		drop(state);
		self.check(result);
	}

	pub(super) fn sources(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		let mut remove = None;
		let mut clone = None;
		let mut reload = None;
		let mut play = None;
		let mut stop = None;
		let mut publish = None;
		let now = kio::clock::now();

		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("sources").striped(true).show(ui, |ui| {
				let collection = state.collection();
				for (name, source) in &collection.sources {
					let selection = Selection::Source(name.clone());
					let selected = self.selection.as_ref() == Some(&selection);
					if ui.selectable_label(selected, name).clicked() {
						self.selection = Some(selection);
					}
					ui.label(describe(source));
					// A clone is as live as what it clones.
					let capture = collection.capture(name);
					let feed = state.sources.get(capture);
					match (
						state.compositor.error(capture),
						feed,
						feed.and_then(|feed| feed.stalled(now)),
					) {
						(Some(err), _, _) => status(ui, &Status::Failed(err.to_string())),
						(None, _, Some(stall)) => {
							ui.colored_label(kit::CUE, "stalled")
								.on_hover_text(health::without(stall));
						}
						(None, Some(feed), None) => status(ui, &feed.status()),
						// A scene is drawn from other sources, so it runs nothing.
						(None, None, None) if collection.nested(name).is_some() => status(ui, &Status::Live),
						(None, None, None) => status(ui, &Status::Starting),
					}
					// A track can fail while its source still draws, such as on a GPU
					// out of encode sessions, so its reason is spelled out. A clone
					// publishes none: it shows its source's capture, not a feed of its own.
					// Nor does a page, which shows how it paints instead.
					match state.sources.get(name).and_then(Feed::track) {
						Some(Status::Failed(reason)) => {
							ui.colored_label(ui.visuals().error_fg_color, format!("track failed: {reason}"));
						}
						Some(track) => {
							ui.horizontal(|ui| {
								ui.weak("track");
								status(ui, &track);
							});
						}
						// A broadcast publishes no track either, and shows how far
						// behind its publisher it plays.
						None if let Some(delay) = feed.and_then(Feed::delay) => {
							ui.weak(format!("{} ms behind", delay.as_millis())).on_hover_text(
								"Plays this far behind what arrives, to ride out the network's jitter, with the picture and sound on one clock",
							);
						}
						None => match feed.and_then(Feed::page).and_then(|page| page.paint()) {
							Some(Paint::Gpu) => {
								ui.weak("GPU").on_hover_text("The page is copied on the GPU");
							}
							Some(Paint::Cpu(why)) => {
								ui.colored_label(kit::CUE, "CPU")
									.on_hover_text(format!("The page is uploaded from memory every frame: {why}"));
							}
							None => {
								ui.label("");
							}
						},
					}
					let places = collection
						.layers()
						.filter(|(_, items)| items.iter().any(|item| item.source == *name))
						.map(|(layer, _)| layer.to_string())
						.collect::<Vec<_>>();
					match places.len() {
						0 => ui.weak("unused"),
						1 => ui.weak("in 1 place"),
						n => ui.weak(format!("linked in {n} places")),
					}
					.on_hover_text(places.join("\n"));
					ui.horizontal(|ui| {
						if source.has_video()
							&& ui
								.small_button("Clone")
								.on_hover_text("A new source showing the same capture, with filters of its own")
								.clicked()
						{
							clone = Some(name.clone());
						}
						if matches!(source, Source::Media { .. }) {
							if ui
								.add(IconButton::new(Icon::Play, "Play from the start").small())
								.clicked()
							{
								play = Some(name.clone());
							}
							let stopping = IconButton::new(Icon::Stop, "Stop")
								.small()
								.note(Some("Blank and silent until played again"));
							if ui.add(stopping).clicked() {
								stop = Some(name.clone());
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
								.add(IconButton::new(Icon::Reload, "Reload").small().note(Some(hover)))
								.clicked()
						{
							reload = Some(name.clone());
						}
						if source.publishes() {
							let mut published = collection.published.contains(name);
							if ui
								.add(kit::Toggle::new(&mut published).label("Published independently"))
								.on_hover_text(
									"Publish this source's own track to the relay beside the program, for viewers to \
									 place over it as the layer layout says, and leave it out of the program. It \
									 encodes for as long as it is published. Every player must reproduce how it is \
									 placed, so it takes no filter but opacity and no blend mode, and not its \
									 background removed, since its track keeps the background.",
								)
								.changed()
							{
								publish = Some((name.clone(), published));
							}
						}
						if ui.small_button("Remove").clicked() {
							remove = Some(name.clone());
						}
					});
					ui.end_row();
				}
			});

			ui.separator();
			let scenes: Vec<String> = state
				.collection()
				.scenes
				.iter()
				.map(|scene| scene.name.clone())
				.collect();
			if let Some(source) = self.add.ui(ui, &scenes) {
				let name = self.add.name.trim().to_string();
				let layer = self.layer(&state).ok();
				let result = add(&mut state, layer, name, source, None);
				if result.is_ok() {
					self.add.name.clear();
				}
				self.report(result.map(|name| format!("added `{name}`")));
			}
		});

		if let Some(name) = reload {
			state.restart(&name);
		}
		if let Some(name) = play
			&& let Some(feed) = state.sources.get(&name)
		{
			let played = feed.play().with_context(|| format!("cannot play `{name}`"));
			self.report(played.map(|()| format!("playing `{name}` from its start")));
		}
		if let Some(name) = stop
			&& let Some(feed) = state.sources.get(&name)
		{
			let stopped = feed.stop().with_context(|| format!("cannot stop `{name}`"));
			self.report(stopped.map(|()| format!("stopped `{name}`")));
		}
		let result = if let Some(name) = remove {
			// Removing the items that show it moves the ones after them.
			self.deselect_item();
			edit(&mut state, format!("Remove `{name}`"), None, |draft| {
				draft.remove_source(&name)
			})
		} else if let Some((name, published)) = publish {
			let verb = if published { "Publish" } else { "Stop publishing" };
			edit(&mut state, format!("{verb} `{name}` independently"), None, |draft| {
				draft.set_published(&name, published)
			})
		} else if let Some(name) = clone {
			let collection = state.collection();
			let (source, video) = clone_of(collection, &name);
			let clone = collection.unused_name(&format!("{name} clone"));
			let layer = self.layer(&state).ok();
			add(&mut state, layer, clone, source, video).map(drop)
		} else {
			Ok(())
		};
		drop(state);
		self.check(result);
	}

	pub(super) fn devices(&mut self, ui: &mut Ui) {
		self.devices.poll();
		if ui.button("Refresh").clicked() {
			let refreshed = self.devices.refresh(&self.runtime).context("cannot list devices");
			self.check(refreshed);
		}
		let own: Vec<String> = self
			.engine
			.state
			.lock()
			.unwrap()
			.cameras()
			.map(|camera| camera.id)
			.collect();
		let Some(picked) = self.devices.ui(ui, &own) else {
			return;
		};
		let mut state = self.engine.state.lock().unwrap();
		// Two cameras of the same model share a name; number the later ones.
		let name = state.collection().unused_name(&picked.name);
		let layer = self.layer(&state).ok();
		let result = add(&mut state, layer, name, picked.source, None);
		drop(state);
		self.report(result.map(|name| format!("added `{name}`")));
	}

	/// Invite guests, show their links, and see who joined.
	pub(super) fn guests(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let Some(room) = &engine.room else {
			ui.label("Start with --room and --room-key to invite guests.");
			return;
		};

		ui.horizontal(|ui| {
			ui.label("Name");
			let field = ui.text_edit_singleline(&mut self.guest);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			if ui.add(kit::button(Icon::Add, "Invite")).clicked() || entered {
				let name = self.guest.trim().to_string();
				match room.invite(&name) {
					Ok(invite) => {
						self.show(ui.ctx(), name, invite);
						self.guest.clear();
					}
					Err(err) => self.report(Err(err)),
				}
			}
		});

		let now = kio::clock::wall();
		if let Some(shown) = &self.shown {
			let title = format!("Invite {}", shown.name);
			let close = kit::Sheet::new(egui::Id::new("join link"), &title).show(ui.ctx(), |ui| {
				ui.label(format!("Their join link {}.", expiry(shown.invite.expires, now)));
				ui.add_space(8.0);
				// The link is the guest's credential, so it shows only as a code to scan.
				match &shown.qr {
					Ok(qr) => {
						ui.vertical_centered(|ui| ui.image((qr.id(), egui::vec2(240.0, 240.0))));
					}
					Err(err) => {
						ui.colored_label(ui.visuals().error_fg_color, format!("{err}; copy the link instead."));
					}
				}
				ui.add_space(8.0);
				if ui.add(kit::button(Icon::Copy, "Copy link")).clicked() {
					ui.ctx().copy_text(shown.invite.link.to_string());
				}
			});
			if close {
				self.shown = None;
			}
		}

		ui.separator();
		if let Some(failure) = room.renewal_failure() {
			ui.colored_label(
				ui.visuals().error_fg_color,
				format!("Guests' tokens are not renewing, so each leaves when their link expires: {failure}"),
			);
		}
		let participants = room.participants();
		if participants.is_empty() {
			ui.weak("Nobody invited yet.");
			return;
		}
		let mut show = None;
		let mut invite = None;
		let mut remove = None;
		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("guests").striped(true).show(ui, |ui| {
				for heading in ["Guest", "Camera", "Screen", "Link", ""] {
					ui.strong(heading);
				}
				ui.end_row();
				for (name, participant) in &participants {
					ui.label(name);
					for online in [participant.camera, participant.screen] {
						if online {
							ui.colored_label(kit::OK, "live");
						} else {
							ui.weak("offline");
						}
					}
					// A renewed guest no longer depends on their link's expiry.
					match &participant.invite {
						_ if participant.renewed() => ui.label("renewing"),
						_ if participant.removed => ui.weak("not renewed"),
						Some(link) => ui.label(expiry(link.expires, now)),
						None => ui.weak("invited before this session"),
					};
					ui.horizontal(|ui| {
						// Only a link that still works is worth sharing again.
						if let Some(link) = &participant.invite
							&& link.expires > now
							&& ui.small_button("Show link").clicked()
						{
							show = Some((name.clone(), link.clone()));
						}
						if ui.small_button("New link").clicked() {
							invite = Some(name.clone());
						}
						if ui
							.small_button("Remove from scenes")
							.on_hover_text(
								"Take their camera and screen, and any mask reading them, out of every scene and mute them. Their token stops renewing, so they leave within --guest-expiry. Undo puts them back in the scenes; only New link renews them again.",
							)
							.clicked()
						{
							remove = Some(name.clone());
						}
					});
					ui.end_row();
				}
			});
		});

		if let Some((name, link)) = show {
			self.show(ui.ctx(), name, link);
		}
		if let Some(name) = invite {
			match room.invite(&name) {
				Ok(link) => self.show(ui.ctx(), name, link),
				Err(err) => self.report(Err(err)),
			}
		}
		if let Some(name) = remove {
			if let Err(err) = room.stop_renewing(&name) {
				self.report(Err(err));
				return;
			}
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft(format!("Remove `{name}` from scenes"));
			let result = match room.take_off(&mut draft, &name) {
				Ok(false) => Ok(()),
				Ok(true) => {
					// Removing items moves the ones after them.
					self.deselect_item();
					state.commit(draft, None)
				}
				Err(err) => Err(err.into()),
			};
			drop(state);
			self.report(result.map(|()| format!("removed {name} from every scene and muted them")));
		}
	}

	/// Put `invite`'s link for `name` on screen as a QR code.
	pub(super) fn show(&mut self, ctx: &egui::Context, name: String, invite: crate::guests::Invite) {
		let qr = qr(ctx, invite.link.as_str());
		self.shown = Some(Shown { name, invite, qr });
	}

	/// The crop, filters in order, and blend mode of the selected source or item.
	pub(super) fn filters(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let collection = state.collection();
		let Some(selection) = self.selection.clone() else {
			ui.label("Select a source or an item to filter it.");
			return;
		};
		let what = selection.describe(collection);
		let Some(look) = selection.look(collection) else {
			ui.label(format!("{what} has no picture to filter."));
			return;
		};
		ui.strong(format!("Filters on {what}"));
		if let Selection::Item(layer, index) = &selection
			&& let Some(item) = collection.items(layer).and_then(|items| items.get(*index))
			&& let Some(video) = collection.video.get(&item.source)
		{
			let source = video.filters.iter().map(describe_filter).collect::<Vec<_>>();
			ui.weak(format!("after `{}`'s own: {}", item.source, source.join(", ")));
		}
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
		egui::Grid::new("look").show(ui, |ui| {
			if let Some(background) = &mut next.background {
				ui.label("Background");
				let mut removed = *background == Background::Remove;
				let response = ui
					.add(kit::Toggle::new(&mut removed).label("Remove"))
					.on_hover_text("Cut the person out of the picture, leaving the rest transparent, before the crop and every filter. Each picture shows a frame or more late, and the source's own track keeps the background.");
				if response.changed() {
					*background = if removed { Background::Remove } else { Background::Keep };
					let verb = if removed { "Remove" } else { "Keep" };
					action = Some((format!("{verb} the background of {what}"), None));
				}
				ui.end_row();
			}
			ui.label("Crop");
			ui.horizontal(|ui| {
				let crop = &mut next.crop;
				for (label, edge) in [
					("left", &mut crop.left),
					("top", &mut crop.top),
					("right", &mut crop.right),
					("bottom", &mut crop.bottom),
				] {
					ui.weak(label);
					let response = ui.add(egui::DragValue::new(edge).range(0.0..=0.95).speed(0.002));
					if let Some(gesture) = changed(&response, &mut self.gestures) {
						action = Some((format!("Crop {what}"), gesture));
					}
				}
			})
			.response
			.on_hover_text("The fraction of the picture trimmed from each edge, the rest stretched to fill");
			ui.end_row();
			if let Some(blend) = &mut next.blend {
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
				ui.end_row();
			}
		});

		let count = next.filters.len();
		let mut moved = None;
		let mut remove = None;
		for (index, filter) in next.filters.iter_mut().enumerate() {
			ui.push_id(index, |ui| {
				ui.horizontal_wrapped(|ui| {
					if let Some(gesture) = filter_ui(ui, filter, &pictures, &luts, &mut self.gestures) {
						action = Some((format!("Set {} on {what}", filter_name(filter)), gesture));
					}
					if ui.add_enabled(index > 0, egui::Button::new("Up").small()).clicked() {
						moved = Some((index, index - 1));
					}
					if ui
						.add_enabled(index + 1 < count, egui::Button::new("Down").small())
						.clicked()
					{
						moved = Some((index, index + 1));
					}
					if ui.small_button("Remove").clicked() {
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

		ui.separator();
		let filters = look.filters;
		ui.horizontal(|ui| {
			egui::ComboBox::from_id_salt("add filter")
				.selected_text("Add filter")
				.show_ui(ui, |ui| {
					for filter in templates(&pictures, &luts) {
						if ui.selectable_label(false, filter_name(&filter)).clicked() {
							action = Some((format!("Add {} to {what}", filter_name(&filter)), None));
							next.filters.push(filter);
						}
					}
				})
				.response
				.on_hover_text("A LUT needs a LUT source, and a mask a source with a picture of its own");
			if ui.add_enabled(!filters.is_empty(), egui::Button::new("Copy")).clicked() {
				let clip = Clip::Filters(moq_scene::clip::Filters {
					filters: filters.clone(),
				});
				match clip.to_toml() {
					Ok(text) => {
						ui.ctx().copy_text(text);
						self.say(Ok(format!("copied the filters on {what}")));
					}
					Err(err) => self.say(Err(err.to_string())),
				}
			}
			if ui.button("Paste").clicked() {
				// Arrives as a paste event on a later frame, adding the filters.
				ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
			}
		});

		let Some((label, gesture)) = action else { return };
		let result = edit(&mut state, label, gesture, |draft| selection.set_look(draft, next));
		drop(state);
		self.check(result);
	}

	/// The selected item's keyframes: when each is, how the item eases into
	/// it, and how far it takes the item from where it rests.
	pub(super) fn keyframes(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let selected = match &self.selection {
			Some(Selection::Item(layer, index)) => {
				let item = state.collection().items(layer).and_then(|items| items.get(*index));
				item.map(|item| (layer.clone(), *index, item.clone()))
			}
			_ => None,
		};
		let Some((layer, index, item)) = selected else {
			ui.label("Select an item to animate it.");
			return;
		};
		if layer.scene.is_none() {
			ui.label("The top layer never goes on program, so its items have no keyframes to play.");
			return;
		}
		let what = format!("`{}`", item.source);
		let mut play = false;
		ui.horizontal(|ui| {
			ui.strong(format!("Keyframes of {what}"));
			play = ui
				.button("Play")
				.on_hover_text("Play the keyframes of the scene on air again from the start, on air")
				.clicked();
		});
		ui.weak(
			"Each keyframe moves the item away from where it rests, so dragging it on the picture \
			 moves its whole animation. They play once, from when the item's scene goes on, and the \
			 last one holds.",
		);

		let mut next = item.keyframes.clone();
		let mut action: Option<(String, Option<u64>)> = None;
		let mut remove = None;
		egui::ScrollArea::vertical().show(ui, |ui| {
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
							.on_hover_text("How long after the item's scene goes on");
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
							.on_hover_text("How the item eases into this keyframe from the one before");
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
							"Applied after the item's own. Settings mix into the next keyframe's when \
							 its filters are of the same kinds, in order, and otherwise change halfway.",
						);
					ui.separator();
				});
			}
		});
		if let Some(at) = remove {
			next.remove(at);
			action = Some((format!("Remove a keyframe from {what}"), None));
		}
		if ui
			.button("Add keyframe")
			.on_hover_text("Half a second after the last, where the item rests, with the last one's filters")
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
			state.compositor.set_program_start(kio::clock::now());
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
		self.check(result);
	}
}
