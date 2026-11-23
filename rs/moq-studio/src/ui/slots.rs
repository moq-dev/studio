//! Slot layouts on BUILD: each slot's outline on the picture with the role
//! in it, a layer dragged onto a slot, and the scene's arrangement and slots
//! in the inspector.

use moq_scene::{Arrangement, Crop, Slot};

use super::layers::section;
use super::*;

/// `layer`'s layers where they are drawn in `format`'s picture, in order:
/// each at its place there, hidden unless shown there, and those of a scene
/// in slots where the last render drew them, hidden while their slot is
/// closed.
pub(super) fn placed(state: &State, layer: &Layer, format: Format) -> Vec<Item> {
	let collection = state.collection();
	let format = collection.format_of(layer, format);
	let items = collection.items(layer).unwrap_or_default();
	// A layer set free that keeps clear, where it was placed.
	let items = items
		.iter()
		.map(|item| super::keep::placed(state, layer, &collection.portrayed(item, format), format));
	let arranged = (layer.scene.as_deref()).and_then(|scene| state.compositor.placed(format, scene));
	// One frame behind an edit, until the next render arranges it.
	let Some(arranged) = arranged.filter(|arranged| arranged.len() == items.len()) else {
		return items.collect();
	};
	let placed = items.zip(arranged).map(|(item, arranged)| match item.slot {
		Some(_) => Item {
			rect: arranged.rect,
			visible: item.visible && arranged.visible,
			..item
		},
		None => item,
	});
	placed.collect()
}

/// The collection as `format`'s picture draws it, with the scene called
/// `scene` as the last render drew it there, cover crops and a glide under
/// way included, so a page in a slot takes input where it is drawn.
pub(super) fn staged<'a>(state: &'a State, scene: Option<&str>, format: Format) -> std::borrow::Cow<'a, Collection> {
	let collection = state.collection();
	let format = scene.map_or(format, |scene| collection.format_of(&Layer::scene(scene), format));
	let found = scene.and_then(|scene| {
		let index = collection.scenes.iter().position(|found| found.name == scene)?;
		let arranged = state.compositor.placed(format, scene)?;
		(arranged.len() == collection.scenes[index].items.len()).then_some((index, arranged))
	});
	let mut staged = collection.formatted(format);
	if let Some((index, arranged)) = found {
		staged.to_mut().scenes[index].items = arranged.to_vec();
	}
	staged
}

/// Where the layer at `index` of `layer` is drawn in `format`'s picture and
/// what it shows of its picture, cover crop included, so setting it free
/// keeps how it looks; its own place there for a layer set free or not yet
/// arranged.
pub(super) fn drawn(state: &State, layer: &Layer, index: usize, format: Format) -> Option<(Rect, Crop)> {
	let collection = state.collection();
	let format = collection.format_of(layer, format);
	let item = &collection.portrayed(collection.items(layer)?.get(index)?, format);
	let canvas = match collection.dual(layer) {
		true => collection.canvas_of(format),
		false => collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas),
	};
	let arranged =
		arranged(state, layer, index, format).filter(|arranged| item.slot.is_some() && !arranged.rect.is_empty());
	Some(match (arranged, item.slot) {
		(Some(arranged), _) => (arranged.rect, arranged.crop),
		(None, Some(_)) => (Rect::fill(&canvas), item.crop),
		(None, None) => (item.rect, item.crop),
	})
}

/// Whether the layer at `index` of `layer` is in a slot its arrangement
/// closes, as while its source is offline, so it is drawn nowhere.
pub(super) fn closed(state: &State, layer: &Layer, index: usize) -> bool {
	arranged(state, layer, index, Format::Landscape).is_some_and(|item| item.slot.is_some() && !item.visible)
}

/// The layer at `index` of `layer` as `format`'s picture arranges it now,
/// once the compositor has arranged its scene.
fn arranged<'a>(state: &'a State, layer: &Layer, index: usize, format: Format) -> Option<&'a Item> {
	let collection = state.collection();
	let format = collection.format_of(layer, format);
	layer
		.scene
		.as_deref()
		.and_then(|scene| state.compositor.placed(format, scene))
		.filter(|arranged| arranged.len() == collection.items(layer).map_or(0, <[Item]>::len))
		.and_then(|arranged| arranged.get(index))
}

/// What a slot's chip calls it: the role in it, a guest, or the slot.
fn chip(collection: &Collection, room: Option<&moq_net::Path>, item: &Item) -> String {
	let source = collection.sources.get(&item.source);
	if matches!(source, Some(Source::Role { .. })) {
		return item.source.clone();
	}
	if room
		.zip(source)
		.is_some_and(|(room, source)| crate::guests::plays_guest(room, source))
	{
		return "Guest".into();
	}
	match item.slot {
		Some(Slot::Main) => "Main".into(),
		_ => "Tile".into(),
	}
}

/// The front-most slot at `point` among `shown`, in canvas pixels.
pub(super) fn slot_at(shown: &[(Picked, Item)], point: [f32; 2]) -> Option<usize> {
	shown.iter().rposition(|(_, item)| {
		let rect = &item.rect;
		item.visible
			&& item.slot.is_some()
			&& (rect.x..=rect.x + rect.width).contains(&point[0])
			&& (rect.y..=rect.y + rect.height).contains(&point[1])
	})
}

/// Each open slot among `shown` outlined on the picture `view` maps, with a
/// chip naming the role or guest in it and the source it shows; `target`,
/// a slot something is dragged over, lit.
pub(super) fn outlines(
	painter: &egui::Painter,
	view: &View,
	state: &State,
	room: Option<&moq_net::Path>,
	shown: &[(Picked, Item)],
	target: Option<usize>,
) {
	let collection = state.collection();
	for (index, (_, item)) in shown.iter().enumerate() {
		if !item.visible || item.slot.is_none() {
			continue;
		}
		let rect = view.screen(&item.rect).shrink(2.0);
		if target == Some(index) {
			painter.rect_filled(rect, 4.0, kit::ACCENT.gamma_multiply(0.22));
		}
		let stroke = egui::Stroke::new(1.5, kit::ACCENT.gamma_multiply(0.9));
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
		// Too small to read, the outline says enough.
		if rect.width() < 60.0 || rect.height() < 34.0 {
			continue;
		}
		let label = chip(collection, room, item).to_uppercase();
		let galley = painter.layout_no_wrap(label, kit::display_bold(10.0), Color32::WHITE);
		let at = rect.left_top() + egui::vec2(6.0, 6.0);
		let pill = egui::Rect::from_min_size(at, galley.size() + egui::vec2(10.0, 4.0));
		painter.rect_filled(pill, 3.0, kit::ACCENT);
		painter.galley(at + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
		// The source it shows, a role's filling it.
		let shows = collection.resolve(&item.source).to_string();
		let galley = painter.layout_no_wrap(shows, kit::ui_medium(11.0), kit::TEXT);
		let at = rect.left_bottom() + egui::vec2(6.0, -10.0 - galley.size().y);
		let tag = egui::Rect::from_min_size(at, galley.size() + egui::vec2(10.0, 4.0));
		painter.rect_filled(tag, 3.0, Color32::from_black_alpha(190));
		painter.galley(at + egui::vec2(5.0, 2.0), galley, kit::TEXT);
	}
}

/// Swap `dragged` with the layer in the slot `target` holds, both of the
/// same scene: `dragged` takes its slot and its place in the order, and it
/// takes the slot `dragged` was in, or, for a layer set free, where it is
/// drawn.
pub(super) fn assign(state: &mut State, dragged: &Picked, target: &Picked) -> anyhow::Result<()> {
	anyhow::ensure!(
		dragged.layer == target.layer,
		"drag a layer of this scene onto one of its slots"
	);
	if dragged.index == target.index {
		return Ok(());
	}
	let collection = state.collection();
	let (Some(moved), Some(held)) = (dragged.item(collection).cloned(), target.item(collection).cloned()) else {
		return Ok(());
	};
	anyhow::ensure!(held.slot.is_some(), "`{}` is in no slot", held.source);
	let label = format!("Put `{}` in the slot of `{}`", moved.source, held.source);
	// In a slot, the arrangement places it, so it no longer keeps clear.
	let taking = Item {
		slot: held.slot,
		rect: Rect::default(),
		portrait: None,
		keep_clear: None,
		..moved.clone()
	};
	// Set free where `moved` is drawn in each picture, as a layer keeping
	// clear may be off where it is saved, cropped to its shape there rather
	// than stretched.
	let free = |format: Format| {
		let resting = super::keep::placed(state, &dragged.layer, &moved, format);
		let Placement { rect, .. } = state.collection().placement(&resting, format);
		let (from, crop) = drawn(state, &target.layer, target.index, format).unwrap_or((rect, held.crop));
		let shape = |rect: &Rect| rect.width / rect.height;
		let crop = crop.then(&Crop::covering(shape(&from), shape(&rect)));
		Placement { rect, crop }
	};
	let leaving = match moved.slot {
		Some(_) => Item {
			slot: moved.slot,
			rect: Rect::default(),
			..held
		},
		None => {
			let dual = state.collection().dual(&target.layer);
			let landscape = free(Format::Landscape);
			Item {
				slot: None,
				rect: landscape.rect,
				crop: landscape.crop,
				portrait: (dual && held.shown_in(Format::Portrait)).then(|| free(Format::Portrait)),
				..held
			}
		}
	};
	edit(state, label, None, |draft| {
		draft.set_item(&target.layer, target.index, taking)?;
		draft.set_item(&dragged.layer, dragged.index, leaving)
	})
}

/// Put `picked` in `slot` of its scene, or set it free where it is drawn.
/// Taking the main slot moves the layer in it to the tiles.
pub(super) fn set_slot(state: &mut State, picked: &Picked, slot: Option<Slot>) -> anyhow::Result<()> {
	anyhow::ensure!(
		picked.layer.scene.is_some(),
		"an always-on-top layer has no slots; move it into the scene first"
	);
	let collection = state.collection();
	let Some(item) = picked.item(collection).cloned() else {
		return Ok(());
	};
	if item.slot == slot {
		return Ok(());
	}
	let name = item.source.clone();
	// Set free where it is drawn in each picture.
	let drawn = |format| drawn(state, &picked.layer, picked.index, format).unwrap_or((item.rect, item.crop));
	let (rect, crop) = match slot {
		Some(_) => (Rect::default(), item.crop),
		None => drawn(Format::Landscape),
	};
	let portrait = match slot {
		None if collection.dual(&picked.layer) && item.shown_in(Format::Portrait) => {
			let (rect, crop) = drawn(Format::Portrait);
			Some(Placement { rect, crop })
		}
		_ => None,
	};
	let main = match slot {
		Some(Slot::Main) => collection
			.items(&picked.layer)
			.unwrap_or_default()
			.iter()
			.position(|item| item.slot == Some(Slot::Main)),
		_ => None,
	};
	let held = main.and_then(|index| Some((index, collection.items(&picked.layer)?[index].clone())));
	let label = match slot {
		Some(Slot::Main) => format!("Put `{name}` in the main slot"),
		Some(Slot::Tile) => format!("Put `{name}` in a tile"),
		None => format!("Set `{name}` free of its slot"),
	};
	edit(state, label, None, |draft| {
		if let Some((index, held)) = held {
			let tile = Item {
				slot: Some(Slot::Tile),
				..held
			};
			draft.set_item(&picked.layer, index, tile)?;
		}
		draft.set_item(
			&picked.layer,
			picked.index,
			Item {
				slot,
				rect,
				crop,
				portrait,
				// In a slot, the arrangement places it.
				keep_clear: item.keep_clear.filter(|_| slot.is_none()),
				..item
			},
		)
	})
}

/// What an arrangement is called.
fn arrangement_name(arrangement: Arrangement) -> &'static str {
	match arrangement {
		Arrangement::Column => "Side column",
		Arrangement::Row => "Bottom row",
		Arrangement::Grid => "Grid",
		Arrangement::Inset => "Picture in picture",
	}
}

/// A button showing `arrangement`'s slots on `canvas`, a main item and
/// three tiles, lit while `on`.
fn arrangement_button(ui: &mut Ui, arrangement: Arrangement, canvas: Canvas, on: bool) -> egui::Response {
	// As tall as a landscape one, so a row of either lines up.
	let size = egui::vec2(33.0 * canvas.width as f32 / canvas.height as f32, 33.0);
	let (rect, response) = ui.allocate_exact_size(size + egui::vec2(8.0, 8.0), egui::Sense::click());
	response.widget_info(|| {
		egui::WidgetInfo::selected(
			egui::WidgetType::Button,
			ui.is_enabled(),
			on,
			arrangement_name(arrangement),
		)
	});
	let response = response.on_hover_text(arrangement_name(arrangement));
	let painter = ui.painter();
	let fill = match (on, response.hovered()) {
		(true, _) => kit::ACCENT.gamma_multiply(0.18),
		(false, true) => kit::P3,
		(false, false) => kit::P2,
	};
	let stroke = match on {
		true => egui::Stroke::new(1.5, kit::ACCENT),
		false => egui::Stroke::new(1.0, kit::LINE),
	};
	painter.rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);
	let picture = egui::Rect::from_center_size(rect.center(), size);
	let scale = size.x / canvas.width as f32;
	let (main, tiles) = arrangement.places(canvas, true, 3);
	let at = |slot: Rect| {
		egui::Rect::from_min_size(
			picture.min + egui::vec2(slot.x, slot.y) * scale,
			egui::vec2(slot.width, slot.height) * scale,
		)
	};
	let ink = if on { kit::ACCENT } else { kit::DIM };
	if let Some(main) = main {
		painter.rect_filled(at(main), 1.5, ink.gamma_multiply(0.7));
	}
	for tile in tiles {
		painter.rect_filled(at(tile), 1.0, ink);
	}
	response
}

/// What the scene's Slots section asked for.
enum Asked {
	Arrange(Arrangement),
	/// Fill the role called this with this source, or leave it unfilled.
	Fill(String, Option<String>),
	/// Add a role of this name, if there is none, and a slot showing it.
	Add(String, Slot),
}

impl Panels {
	/// The Slots section of the scene inspector for the scene at `index`:
	/// how it arranges its slots in the picture BUILD edits, each slot and
	/// the role or source in it, and a slot to add for the main picture or
	/// the camera; the result of what it asked for, if it asked.
	pub(super) fn slots(&mut self, ui: &mut Ui, state: &mut State, index: usize) -> Option<anyhow::Result<()>> {
		let collection = state.collection();
		let scene = collection.scenes[index].clone();
		let format = collection.format_of(&Layer::scene(&scene.name), self.format);
		let (arranged, canvas) = match format {
			Format::Landscape => (scene.arrangement, Canvas::default()),
			Format::Portrait => (scene.portrait.arrangement, Canvas::default().turned()),
		};
		let room = self.engine.room();
		let room = room.as_ref().map(|room| room.path());
		let mut asked = None;
		section(ui, "Slots");
		ui.label(
			egui::RichText::new(
				"Layers in a slot are placed by the arrangement, and close up when their source goes away",
			)
			.color(kit::DIM)
			.font(kit::ui(11.0)),
		);
		if collection.dual(&Layer::scene(&scene.name)) {
			ui.label(
				egui::RichText::new(format!("Arranged in {}", format.name().to_lowercase()))
					.color(kit::DIM)
					.font(kit::ui(11.0)),
			);
		}
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().item_spacing.x = 6.0;
			for arrangement in Arrangement::ALL {
				if arrangement_button(ui, arrangement, canvas, arranged == arrangement).clicked() {
					asked = Some(Asked::Arrange(arrangement));
				}
			}
		});
		let fillers: Vec<String> = collection
			.sources
			.iter()
			.filter(|(_, source)| source.has_video() && !source.is_role() && !matches!(source, Source::Lut { .. }))
			.map(|(name, _)| name.clone())
			.collect();
		let slotted: Vec<&Item> = scene.items.iter().filter(|item| item.slot.is_some()).collect();
		if slotted.is_empty() {
			ui.label(
				egui::RichText::new("No layer is in a slot; put one in from its menu, or add one below")
					.color(kit::FAINT)
					.font(kit::ui(12.0)),
			);
		}
		egui::Grid::new("slots")
			.num_columns(2)
			.spacing([8.0, 6.0])
			.show(ui, |ui| {
				for item in &slotted {
					egui::Frame::new()
						.fill(kit::ACCENT)
						.corner_radius(3.0)
						.inner_margin(egui::Margin::symmetric(5, 2))
						.show(ui, |ui| {
							ui.label(
								kit::caps(chip(collection, room, item), kit::display_bold(10.0)).color(Color32::WHITE),
							);
						});
					match collection.sources.get(&item.source) {
						Some(Source::Role { source }) => {
							let mut filled = source.clone();
							egui::ComboBox::from_id_salt(("fill", &item.source))
								.selected_text(filled.as_deref().unwrap_or("Nothing"))
								.show_ui(ui, |ui| {
									ui.selectable_value(&mut filled, None, "Nothing");
									for name in &fillers {
										ui.selectable_value(&mut filled, Some(name.clone()), name);
									}
								})
								.response
								.on_hover_text("What the role shows in every scene of the show");
							if filled != *source {
								asked = Some(Asked::Fill(item.source.clone(), filled));
							}
						}
						_ => {
							ui.label(&item.source);
						}
					}
					ui.end_row();
				}
			});
		ui.horizontal(|ui| {
			ui.label(egui::RichText::new("Add").color(kit::DIM));
			let main = scene.items.iter().any(|item| item.slot == Some(Slot::Main));
			let added = ui
				.add_enabled(!main, egui::Button::new("Main slot"))
				.on_hover_text("The largest slot, showing the role Main: the show's main picture")
				.on_disabled_hover_text("The scene has a main slot");
			if added.clicked() {
				asked = Some(Asked::Add("Main".into(), Slot::Main));
			}
			let added = ui
				.button("Camera slot")
				.on_hover_text("A tile showing the role Camera: the show's main camera");
			if added.clicked() {
				asked = Some(Asked::Add("Camera".into(), Slot::Tile));
			}
		});

		let asked = asked?;
		Some(match asked {
			Asked::Arrange(arrangement) => {
				let label = format!(
					"Arrange `{}` as {}",
					scene.name,
					arrangement_name(arrangement).to_lowercase()
				);
				let scene = match format {
					Format::Landscape => Scene { arrangement, ..scene },
					Format::Portrait => Scene {
						portrait: moq_scene::Portrait { arrangement },
						..scene
					},
				};
				edit(state, label, None, |draft| draft.set_scene(index, scene))
			}
			Asked::Fill(role, source) => {
				let label = match &source {
					Some(source) => format!("Fill the role `{role}` with `{source}`"),
					None => format!("Leave the role `{role}` unfilled"),
				};
				edit(state, label, None, |draft| {
					draft.set_source(&role, Some(Source::Role { source }))
				})
			}
			Asked::Add(role, _) if !state.collection().sources.get(&role).is_none_or(Source::is_role) => Err(
				anyhow::anyhow!("`{role}` is a source of another kind; rename it to add the role"),
			),
			Asked::Add(role, slot) => {
				let create = !state.collection().sources.contains_key(&role);
				let layer = Layer::scene(&scene.name);
				// After the last layer in a slot, so a logo stays in front.
				let at = scene.items.iter().rposition(|item| item.slot.is_some());
				let at = at.map_or(scene.items.len(), |at| at + 1);
				let item = Item {
					slot: Some(slot),
					..Item::new(role.clone(), Rect::default())
				};
				edit(state, format!("Add a slot for `{role}`"), None, |draft| {
					if create {
						draft.set_source(&role, Some(Source::Role { source: None }))?;
					}
					draft.insert_item(&layer, at, item)
				})
			}
		})
	}
}
