//! The BUILD page: the live picture, or a scene open off air, edited where
//! it shows, with the scene's layers on the left and the selected layer's
//! inspector on the right.

use super::cue::Corner;
use super::shell::Snapshot;
use super::*;

/// The inspector's width, and the layers' column's.
const SIDE: std::ops::RangeInclusive<f32> = 260.0..=320.0;

/// Room between the selected layer and its toolbar.
const TOOLBAR_GAP: f32 = 8.0;

/// Why nothing goes always on top, or comes off it, while a scene is open
/// off air.
pub(super) const OFF_AIR_TOP: &str = "Always on top is drawn on program; go back to live to change it";

impl Panels {
	/// The BUILD page: the layers and Add on the left, the live picture, or
	/// the scene open off air under its banner, in the middle, and the
	/// inspector on the right, or among the left's tabs when the window is
	/// narrow.
	pub(super) fn build(&mut self, ui: &mut Ui, fx: &mut Fx, stage: egui::Rect, snapshot: &Snapshot) {
		let side = (stage.width() * 0.22).clamp(*SIDE.start(), *SIDE.end());
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
		self.side_column(ui, left, !wide);
		if let Some(right) = right {
			self.inspector(ui, right);
		}
		kit::wash(ui.painter(), middle);
		let mut room = middle.shrink(28.0);
		let mut shape = snapshot.shape;
		if let Some(cued) = snapshot.cued.as_ref().filter(|cued| cued.off_air) {
			room.min.y = self.off_air_header(ui, cued, room, snapshot.shape);
			shape = cued.shape();
		}
		self.stage(ui, fx, snapshot, super::layout::fit(room, shape));
		self.build_keys(ui);
	}

	/// The page's keys, unless a field or a page has the keyboard: A adds a
	/// layer, and Escape leaves cropping, then the selection.
	fn build_keys(&mut self, ui: &Ui) {
		if ui.ctx().text_edit_focused() || self.interact.has_keyboard() || self.interacting {
			return;
		}
		let (add, escape) = ui.input_mut(|input| {
			(
				input.consume_key(egui::Modifiers::NONE, egui::Key::A),
				input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
			)
		});
		if add {
			self.side = Side::Add;
		}
		if escape {
			match self.cropping {
				true => self.cropping = false,
				false => self.select(None),
			}
		}
	}

	/// The live picture at `at`, or the scene open off air: a click selects
	/// the layer under it, a drag moves, resizes, or crops it, and a right
	/// click offers what applies there. While interacting, the pointer and
	/// keyboard pass to the selected layer's web page instead, until Escape.
	/// The NEXT card docks on the live picture while a scene is cued.
	fn stage(&mut self, ui: &mut Ui, fx: &mut Fx, snapshot: &Snapshot, at: egui::Rect) {
		self.staged = true;
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		// Decided under this lock, as the edits below are, so the picture
		// shown is the one they edit even if a key took a scene since the
		// snapshot.
		let off_air = self.off_air(&state).map(|cued| {
			let collection = state.collection();
			let canvas = collection.canvas(Some(cued)).unwrap_or(collection.canvas);
			// Viewers may see it anyway, such as a published scene.
			let tag = match state.seen(cued) {
				Some(_) => "Not on program",
				None => "Off air",
			};
			(canvas, tag)
		});
		let (canvas, id, frame) = match off_air {
			Some((canvas, tag)) => (canvas, self.preview.id, TallyFrame::new(Tally::Cue).tag(tag)),
			// It is the live picture, so its frame says whether it is on air.
			None => (self.program_drawn, self.program.id, self.program_frame()),
		};
		let builder = egui::UiBuilder::new().max_rect(at.expand(10.0));
		if self.interacting {
			let page = self.interacted(&state);
			let escape = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
			if let Some(page) = page.filter(|_| !escape) {
				let image = ui
					.scope_builder(builder, |ui| {
						frame.show(ui, Some(fx), |ui| picture(ui, id, canvas, egui::Sense::hover()))
					})
					.inner
					.rect;
				let (scene, since) = self.edited_since(&state);
				let scene = scene.map(|scene| (scene, since));
				self.rebind(&state.sources);
				// `interacted` found the page through the selection, so one is.
				let picked = self.selection.clone().expect("interacting with the selected layer");
				let only = Some((&picked.layer, picked.index));
				let staged = super::slots::staged(&state, scene.map(|(scene, _)| scene));
				let delivered = self.interact.update(ui, image, &staged, scene, only);
				deliver(&state.sources, delivered);
				self.rebind(&state.sources);
				let stop = interacting_banner(ui, image, &page);
				if stop {
					self.stop_interacting(&state);
				}
				return;
			}
			self.stop_interacting(&state);
		}
		let response = ui
			.scope_builder(builder, |ui| {
				frame.show(ui, Some(fx), |ui| {
					picture(ui, id, canvas, egui::Sense::click_and_drag())
				})
			})
			.inner;
		let (spot, result) = self.arrange(ui, &mut state, &response, canvas);
		let (spot, result) = match self.on_picture(ui, &mut state, &response, canvas) {
			Some(acted) => acted,
			None => (spot, result),
		};
		self.drops_on_picture(ui, &mut state, response.rect, canvas);
		// Docked on the live picture, away from the selected layer and its
		// toolbar, which a drag moves with it.
		let (width, height) = (canvas.width.max(1) as f32, canvas.height.max(1) as f32);
		let selected = self.shown(&state).into_iter().find_map(|(picked, item)| {
			(Some(&picked) == self.selection.as_ref()).then(|| {
				[
					(item.rect.x + item.rect.width / 2.0) / width,
					(item.rect.y + item.rect.height / 2.0) / height,
				]
			})
		});
		drop(state);
		if let Some(spot) = spot {
			self.fail(spot, result);
		}
		if let Some(cued) = snapshot.cued.as_ref().filter(|_| off_air.is_none()) {
			self.next(ui, fx, cued, response.rect, Corner::away_from(selected));
		}
	}

	/// The web page the selected layer shows, while it is a visible layer of
	/// the picture that shows one.
	fn interacted(&self, state: &State) -> Option<String> {
		let picked = self.selection.as_ref()?;
		let collection = state.collection();
		let item = picked.item(collection).filter(|item| item.visible)?;
		let shown = match &picked.layer.scene {
			Some(scene) => self.edited(state) == Some(scene.as_str()),
			None => self.off_air(state).is_none(),
		};
		let page = collection.capture(&item.source);
		let web = matches!(collection.sources.get(page), Some(Source::Browser { .. }));
		(shown && web).then(|| page.to_string())
	}

	/// Stop passing input to a page, letting go of what it holds.
	fn stop_interacting(&mut self, state: &State) {
		self.interacting = false;
		self.rebind(&state.sources);
		deliver(&state.sources, self.interact.release());
	}

	/// The layers the picture edits, front last, each with its place: the
	/// scene's, then the always-on-top ones over it, unless the scene is open
	/// off air, as they are drawn on program, each in its slot where the
	/// scene's arrangement put it.
	pub(super) fn shown(&self, state: &State) -> Vec<(Picked, Item)> {
		let (_, since) = self.edited_since(state);
		let top = self.off_air(state).is_none().then(Layer::top);
		let layers = self.scene(state).ok().into_iter().chain(top);
		layers
			.flat_map(|layer| {
				let items = super::slots::placed(state, &layer);
				let posed = items.into_iter().map(move |item| item.posed(since).into_owned());
				posed.enumerate().map(move |(index, item)| {
					let picked = Picked {
						layer: layer.clone(),
						index,
					};
					(picked, item)
				})
			})
			.collect()
	}

	/// Select, move, resize, and crop the layers by dragging them on the live
	/// picture, snapping as they go. A layer is grabbed where its keyframes
	/// have it on the picture, and a drag moves where it rests, which carries
	/// its keyframes along. Where a failure shows, and the result.
	pub(super) fn arrange(
		&mut self,
		ui: &Ui,
		state: &mut State,
		response: &egui::Response,
		canvas: Canvas,
	) -> (Option<Spot>, anyhow::Result<()>) {
		let gesture = self.gestures.of(response);
		// The release still moves the layer to where the pointer let go.
		if !response.dragged() && !response.drag_stopped() {
			self.drag = None;
		}
		// A panel squeezed to nothing has no picture to map the pointer onto.
		if !response.rect.is_positive() {
			return (None, Ok(()));
		}
		let view = View::new(response.rect, canvas);
		let painter = ui.painter_at(response.rect);
		if self.snapping.grid {
			view.grid(&painter, self.snapping.spacing as f32);
		}

		let shown = self.shown(state);
		let items: Vec<Item> = shown.iter().map(|(_, item)| item.clone()).collect();
		let selected = shown
			.iter()
			.position(|(picked, _)| Some(picked) == self.selection.as_ref());
		let reach = view.canvas(REACH);
		let hit = |pos: egui::Pos2| snap::hit(&items, selected, view.point(pos), reach);

		// Focused, the picture takes the arrow keys, which nudge.
		if response.clicked() || response.drag_started() || response.secondary_clicked() {
			response.request_focus();
		}
		if response.clicked() || response.secondary_clicked() {
			// A right click on the selected layer keeps it, even where the
			// pointer would pass through to one behind.
			let held = response.secondary_clicked()
				&& selected.is_some_and(|index| {
					let at = response.interact_pointer_pos().map(|pos| view.point(pos));
					at.is_some_and(|at| snap::Grab::at(&items[index].rect, at, reach).is_some())
				});
			if !held {
				let picked = response.interact_pointer_pos().and_then(hit);
				self.select(picked.map(|(index, _)| shown[index].0.clone()));
			}
		}
		if response.drag_started()
			&& let Some(origin) = ui.input(|input| input.pointer.press_origin())
			&& let Some((index, grab)) = hit(origin)
		{
			let picked = shown[index].0.clone();
			self.select(Some(picked.clone()));
			let collection = state.collection();
			let resting = picked.item(collection).cloned();
			if let Some(resting) = resting {
				let drawn = match resting.slot {
					Some(_) => super::slots::drawn(state, &picked.layer, picked.index),
					None => None,
				};
				let drawn = drawn.unwrap_or((resting.rect, resting.crop));
				let collection = state.collection();
				let source = collection.sources.get(collection.capture(&resting.source));
				self.drag = Some(Drag {
					picked,
					// A layer in a slot moves only to another slot.
					grab: if resting.slot.is_some() && !self.cropping {
						Grab::Move
					} else {
						grab
					},
					slotted: resting.slot.is_some(),
					// A layer in a slot is cropped in the slot it is drawn in,
					// from what it shows there; that crop already fits the
					// slot, so its arrangement crops no further.
					rect: drawn.0,
					crop: drawn.1,
					origin: view.point(origin),
					fixed: fixed_shape(source),
					gesture,
					hints: None,
				});
			}
		}

		let mut spot = None;
		let mut result = Ok(());
		let (scene, since) = self.edited_since(state);
		let scene = scene.map(str::to_string);
		// A layer dragged onto a slot, from the picture or from Layers, swaps
		// places with the layer in it.
		let mut target = None;
		let pointer = response.hover_pos().or(response.interact_pointer_pos());
		let dropping = response.dnd_hover_payload::<Picked>().is_some();
		if dropping || self.drag.as_ref().is_some_and(|drag| drag.slotted && !self.cropping) {
			let dragged = self.drag.as_ref().map(|drag| &drag.picked);
			target = pointer
				.and_then(|pos| super::slots::slot_at(&shown, view.point(pos)))
				.filter(|&at| Some(&shown[at].0) != dragged);
		}
		if let Some(dropped) = response.dnd_release_payload::<Picked>() {
			if let Some(at) = target {
				let held = shown[at].0.clone();
				spot = Some(Spot::Layer(held.layer.clone(), shown[at].1.source.clone()));
				result = super::slots::assign(state, &dropped, &held);
				if result.is_ok() {
					self.select(Some(held));
				}
			}
			target = None;
		}
		if let Some(drag) = self.drag.as_ref().filter(|drag| drag.slotted && !self.cropping) {
			if response.drag_stopped()
				&& let Some(at) = target
			{
				let held = shown[at].0.clone();
				spot = Some(Spot::Layer(held.layer.clone(), shown[at].1.source.clone()));
				result = super::slots::assign(state, &drag.picked.clone(), &held);
				if result.is_ok() {
					self.select(Some(held));
				}
			}
		} else if let Some(drag) = &mut self.drag
			&& let Some(pos) = response.interact_pointer_pos()
		{
			let point = view.point(pos);
			// Whole canvas pixels, so an unsnapped drag saves round numbers.
			let delta = [(point[0] - drag.origin[0]).round(), (point[1] - drag.origin[1]).round()];
			let item = drag.picked.item(state.collection()).cloned();
			let held = ui.input(|input| input.modifiers);
			let (rect, crop) = match self.cropping {
				true => {
					drag.hints = None;
					drag.grab.crop(drag.rect, drag.crop, delta)
				}
				false => {
					// Shift flips whether a resize keeps the layer's shape.
					let aspect = (drag.fixed != held.shift).then(|| snap::aspect(&drag.rect)).flatten();
					let rect = drag.grab.drag(drag.rect, delta, aspect);
					drag.hints = (self.snapping.on && !held.command).then(|| {
						// How far the keyframes have the layer from where it rests.
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
						let others = others(state, scene.as_deref(), &drag.picked, since);
						snap.apply(drag.grab, rect, &others, &canvas)
					});
					let rect = drag.hints.as_ref().map_or(rect, |snapped| snapped.rect);
					(rect, drag.crop)
				}
			};
			// Its slot keeps placing a layer in one.
			let rect = match (&item, drag.slotted) {
				(Some(item), true) => item.rect,
				_ => rect,
			};
			if let Some(item) = item
				&& (item.rect != rect || item.crop != crop)
			{
				let verb = match (self.cropping, drag.grab) {
					(true, _) => "Crop",
					(false, Grab::Move) => "Move",
					(false, _) => "Resize",
				};
				let label = format!("{verb} `{}`", item.source);
				spot = Some(Spot::Layer(drag.picked.layer.clone(), item.source.clone()));
				let picked = &drag.picked;
				result = edit(state, label, drag.gesture, |draft| {
					draft.set_item(&picked.layer, picked.index, Item { rect, crop, ..item })
				});
			}
		}
		if !response.dragged() {
			self.drag = None;
		}
		if response.has_focus() && self.drag.is_none() && result.is_ok() {
			(spot, result) = self.nudge(ui, state, response.id);
		}

		// Drawn from the collection and selection as this frame left them.
		let shown = self.shown(state);
		let room = self.engine.room();
		let room = room.as_ref().map(|room| room.path());
		super::slots::outlines(&painter, &view, state, room, &shown, target);
		let selected = shown.iter().find(|(picked, _)| Some(picked) == self.selection.as_ref());
		let accent = ui.visuals().selection.stroke.color;
		// A hidden layer has no handles, as the picture cannot grab it, and a
		// locked one only its outline.
		if let Some((picked, item)) = selected.filter(|(_, item)| item.visible) {
			if self.cropping {
				let collection = state.collection();
				let resting = picked.item(collection).map_or(item.crop, |item| item.crop);
				view.whole(&painter, &item.rect, &resting);
			}
			// A layer in a slot has no size of its own to drag.
			match item.locked || (item.slot.is_some() && !self.cropping) {
				true => view.hovered(&painter, &item.rect, accent),
				false => view.selected(&painter, &item.rect, accent, self.cropping),
			}
		}
		let items: Vec<Item> = shown.iter().map(|(_, item)| item.clone()).collect();
		let selected = shown
			.iter()
			.position(|(picked, _)| Some(picked) == self.selection.as_ref());
		let grab = match &self.drag {
			Some(drag) => {
				if let Some(snapped) = &drag.hints {
					view.hints(&painter, snapped);
				}
				Some(drag.grab)
			}
			None => response
				.hover_pos()
				.and_then(|pos| snap::hit(&items, selected, view.point(pos), reach))
				.map(|(index, grab)| {
					if selected != Some(index) {
						view.hovered(&painter, &items[index].rect, accent);
					}
					grab
				}),
		};
		let slotted = |index: usize| items[index].slot.is_some() && !self.cropping;
		let hovered_slot = response
			.hover_pos()
			.and_then(|pos| snap::hit(&items, selected, view.point(pos), reach))
			.is_some_and(|(index, _)| slotted(index));
		if let Some(grab) = grab {
			let slot = self
				.drag
				.as_ref()
				.map_or(hovered_slot, |drag| drag.slotted && !self.cropping);
			ui.ctx()
				.set_cursor_icon(match (self.cropping && grab == Grab::Move) || slot {
					true => egui::CursorIcon::Grab,
					false => cursor(grab),
				});
		}
		(spot, result)
	}

	/// Move the selected layer a canvas pixel for each arrow key pressed
	/// while the picture, `id`, has focus, or ten with Shift held. A held key
	/// undoes as one move. A locked layer stays.
	pub(super) fn nudge(&mut self, ui: &Ui, state: &mut State, id: egui::Id) -> (Option<Spot>, anyhow::Result<()>) {
		let arrows = egui::EventFilter {
			horizontal_arrows: true,
			vertical_arrows: true,
			..Default::default()
		};
		ui.memory_mut(|memory| memory.set_focus_lock_filter(id, arrows));
		let Some(picked) = self.selection.clone() else {
			return (None, Ok(()));
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
		let Some(item) = picked
			.item(state.collection())
			.filter(|item| !item.locked && item.slot.is_none())
			.cloned()
		else {
			return (None, Ok(()));
		};
		if delta == [0.0; 2] {
			return (None, Ok(()));
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
		let spot = Spot::Layer(picked.layer.clone(), item.source.clone());
		let result = edit(state, label, Some(gesture), |draft| {
			draft.set_item(&picked.layer, picked.index, Item { rect, ..item })
		});
		(Some(spot), result)
	}

	/// The selected layer's toolbar over the picture, and what a right click
	/// on the picture offers: the layer under it's actions, or the picture's
	/// own. Where a failure shows and the result, if either acted.
	fn on_picture(
		&mut self,
		ui: &mut Ui,
		state: &mut State,
		response: &egui::Response,
		canvas: Canvas,
	) -> Option<(Option<Spot>, anyhow::Result<()>)> {
		let view = View::new(response.rect, canvas);
		let shown = self.shown(state);
		let off_air = self.off_air(state).is_some();
		let selected = shown
			.iter()
			.find(|(picked, item)| Some(picked) == self.selection.as_ref() && item.visible);
		let mut action = None;
		if let Some((picked, item)) = selected
			&& self.drag.is_none()
			&& response.rect.is_positive()
			&& let Some(menu) = Menu::of(state, picked, off_air)
		{
			let at = view.screen(&item.rect).intersect(response.rect);
			let cropping = self.cropping;
			action = toolbar(ui, response.rect, at, &menu, cropping);
		}
		let context = Menu::of_selected(state, self.selection.as_ref(), &shown, off_air);
		let mut picture = None;
		response.context_menu(|ui| match &context {
			Some(menu) => {
				if let Some(chosen) = layer_menu(ui, menu) {
					action = Some(chosen);
					ui.close();
				}
			}
			None => {
				picture = self.picture_menu(ui, canvas);
				if picture.is_some() {
					ui.close();
				}
			}
		});
		match picture {
			Some(PictureAction::Paste) => {
				// The text arrives next frame; without any, the picture is pasted.
				ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
				self.dropping.paste_from_menu();
			}
			Some(PictureAction::Add) => self.side = Side::Add,
			None => {}
		}
		let action = action?;
		let picked = self.selection.clone()?;
		Some(self.act(ui.ctx(), state, &picked, action))
	}

	/// What a right click on the picture away from every layer offers.
	fn picture_menu(&mut self, ui: &mut Ui, canvas: Canvas) -> Option<PictureAction> {
		let mut action = None;
		if ui.add(kit::button(Icon::Add, "Add a layer")).clicked() {
			action = Some(PictureAction::Add);
		}
		if ui.button("Paste").clicked() {
			action = Some(PictureAction::Paste);
		}
		ui.separator();
		let free = ui.ctx().format_modifiers(egui::Modifiers::COMMAND);
		let flip = ui.ctx().format_modifiers(egui::Modifiers::SHIFT);
		ui.add(kit::Toggle::new(&mut self.snapping.on).label("Snap"))
			.on_hover_text(format!(
				"Snap a dragged layer to the canvas, other layers, and the grid. Hold {free} to drag freely.\n\
				 Resizing keeps the shape of a camera, screen, image, or video, but not of other sources; hold \
				 {flip} to flip that. With the picture focused, arrow keys nudge the selected layer, ten pixels \
				 with {flip}."
			));
		ui.horizontal(|ui| {
			ui.add(kit::Toggle::new(&mut self.snapping.grid).label("Grid"));
			let largest = canvas.width.max(canvas.height);
			ui.add_enabled(
				self.snapping.grid,
				egui::DragValue::new(&mut self.snapping.spacing)
					.range(2..=largest)
					.suffix(" px"),
			);
		});
		action
	}

	/// Do what a layer's toolbar, menu, or row asked of `picked`; where a
	/// failure shows, and the result.
	pub(super) fn act(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		picked: &Picked,
		action: LayerAction,
	) -> (Option<Spot>, anyhow::Result<()>) {
		let collection = state.collection();
		let Some(item) = picked.item(collection).cloned() else {
			return (None, Ok(()));
		};
		let layer = &picked.layer;
		let index = picked.index;
		let spot = Some(Spot::Layer(layer.clone(), item.source.clone()));
		let name = item.source.clone();
		let set = |state: &mut State, label: String, item: Item| {
			edit(state, label, None, |draft| draft.set_item(layer, index, item))
		};
		let result = match action {
			LayerAction::Show(visible) => {
				let verb = if visible { "Show" } else { "Hide" };
				set(state, format!("{verb} `{name}`"), Item { visible, ..item })
			}
			LayerAction::Lock(locked) => {
				let verb = if locked { "Lock" } else { "Unlock" };
				if locked {
					self.cropping = false;
				}
				set(state, format!("{verb} `{name}`"), Item { locked, ..item })
			}
			LayerAction::Crop => {
				self.cropping = !self.cropping;
				Ok(())
			}
			LayerAction::Fit | LayerAction::Fill if item.slot.is_some() => Err(anyhow::anyhow!(
				"`{name}` is in a slot, which places it; set it free to place it yourself"
			)),
			LayerAction::Slot(slot) => super::slots::set_slot(state, picked, slot),
			LayerAction::Fit => {
				let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
				let rect = match aspect(state, &item, true) {
					Some(aspect) => snap::fit(aspect, &canvas),
					None => Rect::fill(&canvas),
				};
				set(state, format!("Fit `{name}`"), Item { rect, ..item })
			}
			LayerAction::Fill => {
				let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
				let crop = match aspect(state, &item, false) {
					Some(aspect) => snap::fill(aspect, &canvas),
					None => Crop::default(),
				};
				let rect = Rect::fill(&canvas);
				set(state, format!("Fill with `{name}`"), Item { rect, crop, ..item })
			}
			LayerAction::Shape(corners) => {
				let shape = moq_scene::Shape { corners, ..item.shape };
				set(state, format!("Shape `{name}`"), Item { shape, ..item })
			}
			LayerAction::CutOut(removed) => {
				let selection = Selection::Source(name.clone());
				match selection.look(collection) {
					Some(look) if look.background.is_some() => {
						let background = Some(match removed {
							true => Background::Remove,
							false => Background::Keep,
						});
						let verb = if removed { "Cut out" } else { "Stop cutting out" };
						edit(state, format!("{verb} `{name}`"), None, |draft| {
							selection.set_look(draft, Look { background, ..look })
						})
					}
					_ => Err(anyhow::anyhow!("`{name}` has no background to remove")),
				}
			}
			LayerAction::Interact => {
				self.interacting = true;
				Ok(())
			}
			LayerAction::Reorder(to) => {
				let result = edit(state, format!("Reorder `{name}`"), None, |draft| {
					draft.move_item(layer, index, to)
				});
				if result.is_ok() {
					self.follow(layer, |at| Some(reordered(at, index, to)));
				}
				result
			}
			LayerAction::Top(_) if self.off_air(state).is_some() => Err(anyhow::anyhow!(OFF_AIR_TOP)),
			LayerAction::Top(top) => {
				let to = match top {
					true => Layer::top(),
					false => match self.scene(state) {
						Ok(scene) => scene,
						Err(why) => return (spot, Err(anyhow::anyhow!(why))),
					},
				};
				let count = collection.items(&to).map_or(0, <[Item]>::len);
				// The always-on-top layers never go on, so they have no
				// keyframes to play, nor slots; it stays where it was drawn.
				let (rect, crop) = super::slots::drawn(state, layer, index).unwrap_or((item.rect, item.crop));
				let moved = Item {
					keyframes: if top { Vec::new() } else { item.keyframes.clone() },
					slot: None,
					rect,
					crop,
					..item.clone()
				};
				let label = match top {
					true => format!("Keep `{name}` always on top"),
					false => format!("Move `{name}` into {}", place(&to)),
				};
				let result = edit(state, label, None, |draft| {
					draft.remove_item(layer, index)?;
					draft.insert_item(&to, count, moved)
				});
				if result.is_ok() {
					self.select(Some(Picked {
						layer: to,
						index: count,
					}));
				}
				result
			}
			LayerAction::Duplicate { independent } => {
				let mut copy = Item {
					auto: false,
					..item.clone()
				};
				// A scene has one main slot, so the copy takes a tile.
				if copy.slot == Some(moq_scene::Slot::Main) {
					copy.slot = Some(moq_scene::Slot::Tile);
				}
				let mut source = None;
				if independent {
					let (cloned, video) = clone_of(collection, &name);
					let named = collection.unused_name(&format!("{} copy", collection.capture(&name)));
					copy.source = named.clone();
					source = Some((named, cloned, video));
				}
				let label = match independent {
					true => format!("Duplicate `{name}` as its own copy"),
					false => format!("Duplicate `{name}`"),
				};
				let result = edit(state, label, None, |draft| {
					if let Some((named, cloned, video)) = source {
						draft.set_source(&named, Some(cloned))?;
						draft.set_video(&named, video)?;
					}
					draft.insert_item(layer, index + 1, copy)
				});
				if result.is_ok() {
					self.follow(layer, |at| Some(if at > index { at + 1 } else { at }));
					self.select(Some(Picked {
						layer: layer.clone(),
						index: index + 1,
					}));
				}
				result
			}
			LayerAction::Independent => make_independent(state, layer, index, &item),
			// The caller holds the state, so this copies from it rather than
			// through `copy`, which takes the lock.
			LayerAction::Copy => clip(ctx, state, &item).map(|said| self.say(Ok(said))),
			LayerAction::Remove => {
				let label = format!("Remove `{name}` {}", place(layer));
				let result = edit(state, label, None, |draft| draft.remove_item(layer, index).map(drop));
				if result.is_ok() {
					self.follow(layer, |selected| match selected.cmp(&index) {
						std::cmp::Ordering::Less => Some(selected),
						std::cmp::Ordering::Equal => None,
						std::cmp::Ordering::Greater => Some(selected - 1),
					});
				}
				result
			}
		};
		(spot, result)
	}
}

/// What a right click on the picture away from the layers asked for.
enum PictureAction {
	Add,
	Paste,
}

/// What a layer's toolbar, menu, or row asks of it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LayerAction {
	/// Show it, or hide it.
	Show(bool),
	/// Lock it, or unlock it.
	Lock(bool),
	/// Start or stop cropping it on the picture.
	Crop,
	/// Fit its picture inside the canvas, keeping its shape.
	Fit,
	/// Fill the canvas with its picture, keeping its shape and cropping the rest.
	Fill,
	/// Put it in a slot of its scene, or set it free where it is drawn.
	Slot(Option<moq_scene::Slot>),
	/// Round its corners so.
	Shape([f32; 4]),
	/// Remove its source's background, or stop.
	CutOut(bool),
	/// Pass the pointer and keyboard to its web page.
	Interact,
	/// Move it to this index of its list, back to front.
	Reorder(usize),
	/// Keep it always on top, or move it into the scene.
	Top(bool),
	/// Put another layer just in front, showing the same source, or a copy
	/// of its own.
	Duplicate {
		independent: bool,
	},
	/// Show a copy of its own of what it shows, so its look is its own.
	Independent,
	Copy,
	Remove,
}

/// Where `at` lands once the layer at `from` moves to `to` of the same list.
pub(super) fn reordered(at: usize, from: usize, to: usize) -> usize {
	match at {
		at if at == from => to,
		at if from < to && (from + 1..=to).contains(&at) => at - 1,
		at if to < from && (to..from).contains(&at) => at + 1,
		at => at,
	}
}

/// What a layer's menu and toolbar offer, from the collection as it is.
pub(super) struct Menu {
	item: Item,
	/// Its place in its list, and the list's length.
	index: usize,
	count: usize,
	/// Whether it is always on top.
	top: bool,
	/// Whether its source shows a web page, which it can be interacted with.
	web: bool,
	/// Whether its source can have its background removed, and whether it is.
	cut: Option<bool>,
	/// How many lists of layers show its source.
	linked: usize,
	/// Whether its scene is open off air, where nothing goes always on top,
	/// as that is drawn on program.
	off_air: bool,
}

impl Menu {
	/// What `picked` offers, if it is still there, `off_air` or not.
	pub(super) fn of(state: &State, picked: &Picked, off_air: bool) -> Option<Self> {
		let collection = state.collection();
		let item = picked.item(collection)?.clone();
		let capture = collection.sources.get(collection.capture(&item.source));
		let source = collection.sources.get(&item.source);
		let background = collection.video.get(&item.source).map(|video| video.background);
		let cut = source
			.filter(|source| source.removes_background())
			.map(|_| background == Some(Background::Remove));
		Some(Self {
			index: picked.index,
			count: collection.items(&picked.layer).map_or(0, <[Item]>::len),
			top: picked.layer.scene.is_none(),
			web: matches!(capture, Some(Source::Browser { .. })),
			cut,
			linked: shown_in(collection, &item.source).len(),
			off_air,
			item,
		})
	}

	/// What the selected layer offers, if it is shown on the picture.
	fn of_selected(state: &State, selection: Option<&Picked>, shown: &[(Picked, Item)], off_air: bool) -> Option<Self> {
		let picked = selection.filter(|picked| shown.iter().any(|(shown, _)| shown == *picked))?;
		Self::of(state, picked, off_air)
	}
}

/// A layer's actions, as its toolbar's ⋯, a right click on it, and its
/// row's menu offer them; the one chosen.
pub(super) fn layer_menu(ui: &mut Ui, menu: &Menu) -> Option<LayerAction> {
	let mut action = None;
	let item = &menu.item;
	let (verb, visible) = match item.visible {
		true => ("Hide", false),
		false => ("Show", true),
	};
	if ui.button(verb).clicked() {
		action = Some(LayerAction::Show(visible));
	}
	let (verb, locked) = match item.locked {
		true => ("Unlock", false),
		false => ("Lock", true),
	};
	if ui
		.button(verb)
		.on_hover_text("A locked layer stays put: clicks on the picture pass through it")
		.clicked()
	{
		action = Some(LayerAction::Lock(locked));
	}
	if menu.web && item.visible && ui.add(kit::button(Icon::Interact, "Interact")).clicked() {
		action = Some(LayerAction::Interact);
	}
	ui.separator();
	let front = menu.count.saturating_sub(1);
	let moves = [
		("Bring to front", menu.index < front, front),
		("Bring forward", menu.index < front, (menu.index + 1).min(front)),
		("Send backward", menu.index > 0, menu.index.saturating_sub(1)),
		("Send to back", menu.index > 0, 0),
	];
	for (name, enabled, to) in moves {
		if ui.add_enabled(enabled, egui::Button::new(name)).clicked() {
			action = Some(LayerAction::Reorder(to));
		}
	}
	if !menu.top {
		use moq_scene::Slot;
		ui.separator();
		let slots = [
			(
				Some(Slot::Main),
				"Put in the main slot",
				"The largest slot; the layer in it moves to the tiles",
			),
			(
				Some(Slot::Tile),
				"Put in a tile",
				"Beside the main slot, in the order of the layers",
			),
			(None, "Set free", "Placed where it is now, by its own rect"),
		];
		for (slot, name, hover) in slots {
			if item.slot != slot && ui.button(name).on_hover_text(hover).clicked() {
				action = Some(LayerAction::Slot(slot));
			}
		}
	}
	let top = match menu.top {
		true => "Move into this scene",
		false => "Keep always on top",
	};
	let hover = match menu.top {
		true => "Show it in the scene on program alone, under the always-on-top layers",
		false => "Draw it over every scene on program, through every cut; it loses its keyframes",
	};
	let button = ui
		.add_enabled(!menu.off_air, egui::Button::new(top))
		.on_hover_text(hover)
		.on_disabled_hover_text(OFF_AIR_TOP);
	if button.clicked() {
		action = Some(LayerAction::Top(!menu.top));
	}
	ui.separator();
	if ui
		.button("Duplicate")
		.on_hover_text("Another layer showing the same source: an edit to the source's look shows in both")
		.clicked()
	{
		action = Some(LayerAction::Duplicate { independent: false });
	}
	if ui
		.button("Duplicate as its own copy")
		.on_hover_text("Another layer showing a copy of the source with a look of its own")
		.clicked()
	{
		action = Some(LayerAction::Duplicate { independent: true });
	}
	if menu.linked > 1
		&& ui
			.button("Make independent")
			.on_hover_text(linked_note(menu.linked))
			.clicked()
	{
		action = Some(LayerAction::Independent);
	}
	ui.separator();
	if ui.add(kit::button(Icon::Copy, "Copy")).clicked() {
		action = Some(LayerAction::Copy);
	}
	if ui
		.add(kit::button(Icon::Remove, "Remove"))
		.on_hover_text("Take it out of this list; its source stays, under Add while nothing shows it")
		.clicked()
	{
		action = Some(LayerAction::Remove);
	}
	action
}

/// What making a layer linked `linked` times independent does.
pub(super) fn linked_note(linked: usize) -> String {
	format!(
		"Shown in {linked} places, which share its look. Make independent to give this layer a copy of its \
		 own, with a crop and filters of its own."
	)
}

/// The selected layer's toolbar, over the picture at `at` within `picture`:
/// crop, fit, fill, shape, cut out or interact, and the rest of its actions.
fn toolbar(ui: &mut Ui, picture: egui::Rect, at: egui::Rect, menu: &Menu, cropping: bool) -> Option<LayerAction> {
	let height = 32.0;
	// Over the layer, or under it, or inside its top, wherever there is room.
	let (pivot, anchor) = if at.top() - TOOLBAR_GAP - height >= picture.top() {
		(
			egui::Align2::RIGHT_BOTTOM,
			at.right_top() - egui::vec2(0.0, TOOLBAR_GAP),
		)
	} else if at.bottom() + TOOLBAR_GAP + height <= picture.bottom() {
		(
			egui::Align2::RIGHT_TOP,
			at.right_bottom() + egui::vec2(0.0, TOOLBAR_GAP),
		)
	} else {
		(
			egui::Align2::RIGHT_TOP,
			at.right_top() + egui::vec2(-TOOLBAR_GAP, TOOLBAR_GAP),
		)
	};
	let mut action = None;
	egui::Area::new(ui.id().with("toolbar"))
		.order(egui::Order::Middle)
		.pivot(pivot)
		.fixed_pos(anchor)
		.constrain_to(picture.expand(8.0))
		.show(ui.ctx(), |ui| {
			egui::Frame::new()
				.fill(kit::POPOVER)
				.stroke(egui::Stroke::new(1.0, kit::LINE))
				.corner_radius(8)
				.inner_margin(egui::Margin::same(3))
				.show(ui, |ui| {
					ui.spacing_mut().item_spacing.x = 2.0;
					ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
					ui.horizontal(|ui| {
						let locked = menu.item.locked;
						let crop = egui::Button::selectable(cropping, (Icon::Crop.image(13.0, kit::TEXT), "Crop"));
						let crop = ui
							.add_enabled(!locked, crop)
							.on_hover_text("Drag an edge to trim the picture, or inside to move it under the frame")
							.on_disabled_hover_text("Locked; unlock it to crop it");
						if crop.clicked() {
							action = Some(LayerAction::Crop);
						}
						// Its slot places a layer in one.
						if menu.item.slot.is_none() {
							let fit = ui
								.add(egui::Button::selectable(false, "Fit"))
								.on_hover_text("As large as fits on the canvas, keeping its shape");
							if fit.clicked() {
								action = Some(LayerAction::Fit);
							}
							let fill = ui
								.add(egui::Button::selectable(false, "Fill"))
								.on_hover_text("Cover the canvas, keeping its shape and cropping the rest");
							if fill.clicked() {
								action = Some(LayerAction::Fill);
							}
						}
						let shape = ui.add(egui::Button::selectable(!menu.item.shape.is_square(), "Shape"));
						egui::Popup::menu(&shape).show(|ui| {
							if let Some(corners) = shapes(ui, menu.item.shape.corners) {
								action = Some(LayerAction::Shape(corners));
							}
						});
						match (menu.web, menu.cut) {
							(true, _) => {
								let interact = egui::Button::new((Icon::Interact.image(13.0, kit::TEXT), "Interact"));
								let interact = ui
									.add(interact)
									.on_hover_text("Pass the mouse and keyboard to the page; Escape stops");
								if interact.clicked() {
									action = Some(LayerAction::Interact);
								}
							}
							(false, Some(cut)) => {
								let button =
									egui::Button::selectable(cut, (Icon::CutOut.image(13.0, kit::TEXT), "Cut out"));
								let note = match menu.linked {
									1 => "Remove the background behind a person".to_string(),
									n => format!(
										"Remove the background behind a person, on all {n} layers showing this source"
									),
								};
								if ui.add(button).on_hover_text(note).clicked() {
									action = Some(LayerAction::CutOut(!cut));
								}
							}
							(false, None) => {}
						}
						let more = ui.add(IconButton::new(Icon::More, "More").small());
						egui::Popup::menu(&more).show(|ui| {
							if let Some(chosen) = layer_menu(ui, menu) {
								action = Some(chosen);
								ui.close();
							}
						});
					});
				});
		});
	action
}

/// The corner shapes, square, rounded, circle, and arch; the one picked, if
/// one is, other than `current`.
pub(super) fn shapes(ui: &mut Ui, current: [f32; 4]) -> Option<[f32; 4]> {
	const SHAPES: [(&str, [f32; 4]); 4] = [
		("Square", [0.0; 4]),
		("Rounded", [0.12; 4]),
		("Circle, or a pill on a layer that is not square", [0.5; 4]),
		("Arch", [0.5, 0.5, 0.06, 0.06]),
	];
	let mut picked = None;
	ui.horizontal(|ui| {
		for (name, corners) in SHAPES {
			let on = corners == current;
			let (rect, response) = ui.allocate_exact_size(egui::vec2(48.0, 40.0), egui::Sense::click());
			let response = kit::tip(response, name, None);
			let (fill, stroke) = match (on, response.hovered()) {
				(true, _) => (kit::ACCENT.gamma_multiply(0.18), kit::ACCENT),
				(false, true) => (kit::P3, kit::LINE),
				(false, false) => (kit::P2, kit::LINE),
			};
			let painter = ui.painter();
			painter.rect(
				rect,
				kit::RADIUS,
				fill,
				egui::Stroke::new(1.0, stroke),
				egui::StrokeKind::Inside,
			);
			let tile = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0));
			let radius = corners.map(|corner| (corner * 20.0).round() as u8);
			let corner = egui::CornerRadius {
				nw: radius[0],
				ne: radius[1],
				se: radius[2],
				sw: radius[3],
			};
			painter.rect_filled(tile, corner, if on { kit::ACCENT } else { kit::DIM });
			if response.clicked() && !on {
				picked = Some(corners);
			}
		}
	});
	picked
}

/// While interacting, a note over the top of the picture `image` naming the
/// page, with a button that stops; whether it was clicked.
fn interacting_banner(ui: &mut Ui, image: egui::Rect, page: &str) -> bool {
	let mut stop = false;
	egui::Area::new(ui.id().with("interacting"))
		.order(egui::Order::Middle)
		.pivot(egui::Align2::CENTER_TOP)
		.fixed_pos(image.center_top() + egui::vec2(0.0, 12.0))
		.show(ui.ctx(), |ui| {
			kit::popover_frame()
				.stroke(egui::Stroke::new(1.0, kit::CUE))
				.inner_margin(egui::Margin::symmetric(12, 6))
				.show(ui, |ui| {
					ui.horizontal(|ui| {
						ui.label(format!("Interacting with `{page}`. Escape stops."));
						stop = ui.button("Stop").clicked();
					});
				});
		});
	stop
}

/// The layers of every list showing `source`, each once.
pub(super) fn shown_in(collection: &Collection, source: &str) -> Vec<Layer> {
	collection
		.layers()
		.filter(|(_, items)| items.iter().any(|item| item.source == source))
		.map(|(layer, _)| layer.clone())
		.collect()
}

/// Point the layer at `index` of `layer` at a copy of its own of what it
/// shows, which keeps its look, so editing that look no longer changes the
/// layers it was linked to.
pub(super) fn make_independent(state: &mut State, layer: &Layer, index: usize, item: &Item) -> anyhow::Result<()> {
	let collection = state.collection();
	let (source, video) = clone_of(collection, &item.source);
	let name = collection.unused_name(&format!("{} copy", collection.capture(&item.source)));
	let label = format!("Make `{}` {} independent", item.source, place(layer));
	let shown = Item {
		source: name.clone(),
		..item.clone()
	};
	edit(state, label, None, |draft| {
		draft.set_source(&name, Some(source))?;
		draft.set_video(&name, video)?;
		draft.set_item(layer, index, shown)
	})
}

/// Add a scene after the one at `index`, as large and showing the same
/// layers, named for it, and return its name. Its layers show the same
/// sources, or with `independent` each a copy of its own, keeping the look,
/// except a scene or the program drawn in it, which stays one picture.
pub(super) fn duplicate_scene(state: &mut State, index: usize, independent: bool) -> anyhow::Result<String> {
	let collection = state.collection();
	let scene = collection.scenes.get(index).context("the scene is gone")?;
	let name = std::iter::once(format!("{} copy", scene.name))
		.chain((2..).map(|n| format!("{} copy {n}", scene.name)))
		.find(|name| collection.scene(name).is_none())
		.expect("an unused name");
	let mut copy = Scene {
		name: name.clone(),
		size: scene.size,
		arrangement: scene.arrangement,
		// A copy is the host's, never the studio's to give away.
		items: scene
			.items
			.iter()
			.map(|item| Item {
				auto: false,
				..item.clone()
			})
			.collect(),
		..Scene::new("")
	};
	// Each source shown, and the copy of its own each of its layers shows.
	let mut copies: BTreeMap<String, (String, Source, Option<Video>)> = BTreeMap::new();
	if independent {
		let mut taken = BTreeSet::new();
		for item in &mut copy.items {
			let nested = matches!(
				collection.sources.get(collection.capture(&item.source)),
				Some(Source::Scene { .. } | Source::Program)
			);
			if nested {
				continue;
			}
			let (named, ..) = copies.entry(item.source.clone()).or_insert_with(|| {
				let base = format!("{} copy", collection.capture(&item.source));
				let named = std::iter::once(base.clone())
					.chain((2..).map(|n| format!("{base} {n}")))
					.find(|name| !collection.sources.contains_key(name) && !taken.contains(name))
					.expect("an unused name");
				taken.insert(named.clone());
				let (source, video) = clone_of(collection, &item.source);
				(named, source, video)
			});
			item.source = named.clone();
		}
	}
	let label = match independent {
		true => format!("Duplicate scene `{}` with its own copies", scene.name),
		false => format!("Duplicate scene `{}`", scene.name),
	};
	edit(state, label, None, |draft| {
		for (named, source, video) in copies.into_values() {
			draft.set_source(&named, Some(source))?;
			draft.set_video(&named, video)?;
		}
		draft.insert_scene(index + 1, copy)
	})?;
	Ok(name)
}

/// The width over height of what `item` shows: after its crop if `cropped`,
/// else after its source's alone. `None` for a source that fills any shape,
/// such as a color.
fn aspect(state: &State, item: &Item, cropped: bool) -> Option<f32> {
	let collection = state.collection();
	let capture = collection.capture(&item.source);
	let (width, height) = match collection.sources.get(capture)? {
		Source::Browser { width, height, .. } | Source::Overlay { width, height, .. } => (*width, *height),
		Source::Scene { scene } => collection
			.canvas(Some(scene))
			.map(|canvas| (canvas.width, canvas.height))?,
		Source::Program => (collection.canvas.width, collection.canvas.height),
		Source::Color { .. } => return None,
		_ => state.compositor.picture_size(capture)?,
	};
	let crop = match cropped {
		true => collection.crop(item),
		false => collection
			.video
			.get(&item.source)
			.map(|video| video.crop)
			.unwrap_or_default(),
	};
	let width = width as f32 * (1.0 - crop.left - crop.right);
	let height = height as f32 * (1.0 - crop.top - crop.bottom);
	let aspect = width / height;
	(aspect.is_finite() && aspect > 0.0).then_some(aspect)
}
