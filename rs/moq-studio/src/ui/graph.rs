//! The Graph panel: sources, scenes, and audio scenes as nodes, wired to what
//! draws or holds them, left to right. It edits the collection the Scenes,
//! Items, and Audio panels do, for building and checking routing; the live
//! controls stay a mixer.

use std::collections::HashMap;

use egui::epaint::CubicBezierShape;
use egui::{Color32, Pos2, Stroke, Ui};
use moq_scene::{Audio, AudioScene, Collection, Draft, Item, Layer, Rect, Source};

use super::levels;
use super::{
	Panels, SceneAction, Selection, Strip, describe, edit, on_program, scene_settings, status, strip_controls,
};
use crate::engine::State;
use crate::mix;

/// Something that draws, holds, or is drawn or held.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Node {
	/// A source with a picture or sound, by name.
	Source(String),
	/// A scene, by name.
	Scene(String),
	/// The top layer, over the program.
	Top,
	/// An audio scene, by name.
	Audio(String),
}

impl Node {
	/// The layer a picture wired into it is placed in.
	fn layer(&self) -> Option<Layer> {
		match self {
			Self::Scene(name) => Some(Layer::scene(name)),
			Self::Top => Some(Layer::top()),
			Self::Source(_) | Self::Audio(_) => None,
		}
	}
}

/// What a wire carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
	Picture,
	Sound,
}

impl Kind {
	fn color(self) -> Color32 {
		match self {
			Self::Picture => Color32::from_rgb(100, 160, 240),
			Self::Sound => Color32::from_rgb(80, 200, 120),
		}
	}
}

/// From what is drawn or held to what draws or holds it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Wire {
	from: Node,
	to: Node,
	kind: Kind,
}

/// What a wire made by hand stands for in the collection.
enum Route<'a> {
	/// A source placed in a layer, by every item showing it there.
	Placed(&'a str, Layer),
	/// A sound or audio scene sent into the audio scene by this name.
	Sent(Strip<'a>, &'a str),
}

impl Wire {
	/// What the wire stands for, or `None` for one into a source, which
	/// follows from what the source shows rather than being made by hand.
	fn route(&self) -> Option<Route<'_>> {
		match (&self.from, &self.to, self.kind) {
			(Node::Source(source), to, Kind::Picture) => to.layer().map(|layer| Route::Placed(source, layer)),
			(Node::Source(source), Node::Audio(to), Kind::Sound) => Some(Route::Sent(Strip::Sound(source), to)),
			(Node::Audio(from), Node::Audio(to), Kind::Sound) if from != to => {
				Some(Route::Sent(Strip::Scene(from), to))
			}
			_ => None,
		}
	}

	/// Whether a drag can make it in `collection`, as the Audio panel offers:
	/// an audio scene never holds one that holds it.
	fn offered(&self, collection: &Collection) -> bool {
		match self.route() {
			Some(Route::Sent(Strip::Scene(from), to)) => !collection.nests(from, to),
			Some(_) => true,
			None => false,
		}
	}

	/// What making it, or taking it out with `on` false, is called in the
	/// history.
	fn label(&self, on: bool) -> String {
		match self.route() {
			Some(Route::Placed(source, layer)) if on => format!("Add `{source}` to {layer}"),
			Some(Route::Placed(source, layer)) => format!("Remove `{source}` from {layer}"),
			Some(Route::Sent(strip, to)) => strip.label(to, on),
			None => "Wire".into(),
		}
	}

	/// What it stands for, as the panel names the wire picked.
	fn describe(&self) -> String {
		match self.route() {
			Some(Route::Placed(source, layer)) => format!("`{source}` in {layer}"),
			Some(Route::Sent(strip, to)) => format!("{} in `{to}`", strip.what()),
			None => format!("{:?} into {:?}", self.from, self.to),
		}
	}

	/// Make it, placing the source to fill the layer or sending it into the
	/// audio scene, or take it out with `on` false, removing every item
	/// showing the source there or no longer sending it.
	fn apply(&self, draft: &mut Draft, on: bool) -> Result<(), moq_scene::Error> {
		let conflict = moq_scene::Error::Conflict;
		match self.route() {
			None => Err(moq_scene::Error::Invalid(format!(
				"{:?} cannot be wired into {:?} by hand",
				self.from, self.to
			))),
			Some(Route::Placed(source, layer)) => {
				let collection = draft.collection();
				let items = collection
					.items(&layer)
					.ok_or_else(|| conflict(format!("there is no {layer}")))?;
				let placed: Vec<usize> = (0..items.len()).filter(|&i| items[i].source == source).collect();
				match (on, placed.is_empty()) {
					(true, true) => {
						let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
						let item = Item::new(source, Rect::fill(&canvas));
						draft.insert_item(&layer, items.len(), item)
					}
					(true, false) => Err(conflict(format!("{layer} already places `{source}`"))),
					(false, true) => Err(conflict(format!("{layer} does not place `{source}`"))),
					(false, false) => {
						for index in placed.into_iter().rev() {
							draft.remove_item(&layer, index)?;
						}
						Ok(())
					}
				}
			}
			Some(Route::Sent(strip, to)) => {
				let mut scenes = draft.collection().audio_scenes.clone();
				if !strip.send(&mut scenes, to, on) {
					let what = strip.what();
					return Err(conflict(match on {
						true => format!("`{to}` already holds {what}"),
						false => format!("`{to}` does not hold {what}"),
					}));
				}
				draft.set_audio_scenes(scenes)
			}
		}
	}
}

/// A wire as the collection has it.
#[derive(Debug, PartialEq)]
struct Edge {
	wire: Wire,
	/// Whether what it carries plays: some item it stands for is shown, or
	/// the sound is not muted. One that does not is dimmed.
	shown: bool,
}

/// The nodes in columns, each node right of every one wired into it, and the
/// wires between them.
#[derive(Debug)]
struct Graph {
	columns: Vec<Vec<Slot>>,
	edges: Vec<Edge>,
}

impl Graph {
	fn of(collection: &Collection) -> Self {
		// Each node with the group it sorts in among those its column holds
		// with nothing wired in: pictures above sounds.
		let mut nodes = Vec::new();
		let mut edges = Vec::new();
		let into = |from, to, kind, shown| Edge {
			wire: Wire { from, to, kind },
			shown,
		};
		for (name, source) in &collection.sources {
			// A LUT is only a filter's.
			let group = match (source.has_video(), source.has_audio()) {
				(false, false) => continue,
				(true, false) => 0,
				(true, true) => 1,
				(false, true) => 2,
			};
			let node = Node::Source(name.clone());
			match source {
				Source::Scene { scene } => {
					edges.push(into(Node::Scene(scene.clone()), node.clone(), Kind::Picture, true))
				}
				Source::Clone { source } => {
					edges.push(into(Node::Source(source.clone()), node.clone(), Kind::Picture, true));
				}
				_ => {}
			}
			nodes.push((node, group));
		}
		for (layer, items) in collection.layers() {
			let to = match layer.scene {
				Some(name) => Node::Scene(name),
				None => Node::Top,
			};
			let mut placed: Vec<Edge> = Vec::new();
			for item in items {
				let from = Node::Source(item.source.clone());
				match placed.iter_mut().find(|edge| edge.wire.from == from) {
					Some(edge) => edge.shown |= item.visible,
					None => placed.push(into(from, to.clone(), Kind::Picture, item.visible)),
				}
			}
			edges.extend(placed);
			nodes.push((to, 0));
		}
		for scene in &collection.audio_scenes {
			let to = Node::Audio(scene.name.clone());
			for name in &scene.sounds {
				let muted = collection.audio.get(name).is_some_and(|audio| audio.muted);
				edges.push(into(Node::Source(name.clone()), to.clone(), Kind::Sound, !muted));
			}
			for name in &scene.scenes {
				edges.push(into(Node::Audio(name.clone()), to.clone(), Kind::Sound, true));
			}
			nodes.push((to, 2));
		}
		let columns = columns(nodes, &edges);
		Self { columns, edges }
	}
}

/// A place in a column: a node, or a lane a wire passes through on its way
/// to a column further on, by the wire's index in [`Graph::edges`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Slot {
	Node(Node),
	Lane(usize),
}

/// Put each node in the column after the longest chain of wires into it,
/// sources first and everything they go into after, with a lane in each
/// column a wire skips, so no wire runs under a node. Then order each column
/// by group and by where what feeds each slot sits, so wires cross less.
fn columns(nodes: Vec<(Node, u8)>, edges: &[Edge]) -> Vec<Vec<Slot>> {
	let mut rank: HashMap<&Node, usize> = nodes
		.iter()
		.map(|(node, _)| (node, usize::from(!matches!(node, Node::Source(_)))))
		.collect();
	// Each pass settles one more link of the longest chain. A valid
	// collection's wires never loop, and the bound keeps one that does from
	// hanging the window.
	for _ in 0..nodes.len() {
		let mut moved = false;
		for edge in edges {
			let Some(&from) = rank.get(&edge.wire.from) else {
				continue;
			};
			if let Some(to) = rank.get_mut(&edge.wire.to)
				&& *to <= from
			{
				*to = from + 1;
				moved = true;
			}
		}
		if !moved {
			break;
		}
	}

	let count = rank.values().max().map_or(0, |last| last + 1);
	let mut columns: Vec<Vec<(Slot, u8)>> = vec![Vec::new(); count];
	for (node, group) in &nodes {
		columns[rank[node]].push((Slot::Node(node.clone()), *group));
	}
	// The columns each wire drawn goes from and to, by its index.
	let mut spans: HashMap<usize, (usize, usize)> = HashMap::new();
	for (index, edge) in edges.iter().enumerate() {
		let (Some(&from), Some(&to)) = (rank.get(&edge.wire.from), rank.get(&edge.wire.to)) else {
			continue;
		};
		spans.insert(index, (from, to));
		let group = match edge.wire.kind {
			Kind::Picture => 0,
			Kind::Sound => 2,
		};
		for column in &mut columns[from + 1..to] {
			column.push((Slot::Lane(index), group));
		}
	}

	// Where each slot of the previous column sits, once it is ordered.
	let mut previous: HashMap<Slot, usize> = HashMap::new();
	columns
		.into_iter()
		.enumerate()
		.map(|(at, column)| {
			// What feeds a slot through the wire at `index` sits in the
			// previous column: the node it comes from, or the lane it last
			// passed through.
			let feeder = |index: usize| match spans[&index].0 + 1 == at {
				true => Slot::Node(edges[index].wire.from.clone()),
				false => Slot::Lane(index),
			};
			let mut keyed: Vec<(u8, f32, Slot)> = column
				.into_iter()
				.map(|(slot, group)| {
					let fed: Vec<usize> = match &slot {
						Slot::Node(node) => (0..edges.len())
							.filter(|index| edges[*index].wire.to == *node && spans.contains_key(index))
							.filter_map(|index| previous.get(&feeder(index)).copied())
							.collect(),
						Slot::Lane(index) => previous.get(&feeder(*index)).copied().into_iter().collect(),
					};
					let mean = match fed.len() {
						0 => f32::INFINITY,
						len => fed.iter().sum::<usize>() as f32 / len as f32,
					};
					(group, mean, slot)
				})
				.collect();
			// Stable, so ties keep the collection's order.
			keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
			let column: Vec<Slot> = keyed.into_iter().map(|(_, _, slot)| slot).collect();
			previous = column.iter().cloned().zip(0..).collect();
			column
		})
		.collect()
}

/// What the Graph panel keeps between frames.
#[derive(Default)]
pub(super) struct View {
	/// The output a wire is being dragged from.
	dragging: Option<(Node, Kind)>,
	/// The wire clicked, which Remove takes out.
	picked: Option<Wire>,
}

/// An edit or pick the Graph panel makes.
enum Action {
	/// Make a wire, or take it out.
	Wire(Wire, bool),
	/// Set a sound's strip, labelled, as part of a gesture.
	Sound(String, Audio, String, Option<u64>),
	Scene(SceneAction),
	AudioScenes(Vec<AudioScene>, String),
	/// Listen to the audio scene by this name, or stop.
	Listen(Option<String>),
	Preview(String),
}

/// What a frame's nodes are drawn from, and what they leave for the wires.
struct Pass<'a> {
	state: &'a State,
	graph: &'a Graph,
	/// Each node's input as drawn, which a wire ends on.
	inputs: HashMap<Node, Pos2>,
	/// Each node's outputs as drawn, by what they carry.
	outputs: HashMap<(Node, Kind), Pos2>,
	/// Each node's body, where a click is the node's rather than a wire's.
	bodies: Vec<egui::Rect>,
	/// Where each wire that skips columns enters and leaves each of their
	/// lanes, left to right, by its index in [`Graph::edges`].
	lanes: HashMap<usize, Vec<(Pos2, Pos2)>>,
	/// The output a wire was let go from this frame, and where.
	released: Option<(Node, Kind, Pos2)>,
	action: Option<Action>,
}

/// How wide a node is.
const NODE_WIDTH: f32 = 200.0;
/// The space between columns, which wires cross.
const COLUMN_GAP: f32 = 72.0;
/// The space between nodes in a column.
const NODE_GAP: f32 = 16.0;
/// How tall a lane a wire passes through is.
const LANE: f32 = 10.0;
/// A port's radius.
const PORT: f32 = 5.0;
/// How near the pointer a port or wire counts as under it.
const REACH: f32 = 8.0;

impl Panels {
	/// The scene graph, laid out from the collection each frame.
	pub(super) fn graph(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let graph = Graph::of(state.collection());
		// An undo or reload can take the picked wire out.
		if let Some(picked) = &self.graph.picked
			&& !graph.edges.iter().any(|edge| edge.wire == *picked)
		{
			self.graph.picked = None;
		}
		// A drag whose port went away ends with the button.
		if !ui.input(|input| input.pointer.any_down()) {
			self.graph.dragging = None;
		}

		let mut pass = Pass {
			state: &state,
			graph: &graph,
			inputs: HashMap::new(),
			outputs: HashMap::new(),
			bodies: Vec::new(),
			lanes: HashMap::new(),
			released: None,
			action: None,
		};
		ui.horizontal(|ui| match &self.graph.picked {
			Some(wire) => {
				ui.label(wire.describe());
				if ui.button("Remove").clicked() {
					pass.action = Some(Action::Wire(wire.clone(), false));
				}
			}
			None => {
				ui.weak(
					"Drag from a node's right edge onto another's left edge to wire them; click a wire to pick it.",
				);
			}
		});
		ui.separator();

		egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
			let wires = ui.painter().add(egui::Shape::Noop);
			ui.horizontal_top(|ui| {
				ui.add_space(REACH);
				for column in &graph.columns {
					let mut lanes = Vec::new();
					let drawn = ui.vertical(|ui| {
						for slot in column {
							match slot {
								Slot::Node(node) => {
									self.node(ui, &mut pass, node);
									ui.add_space(NODE_GAP);
								}
								Slot::Lane(index) => {
									let (lane, _) = ui.allocate_exact_size(egui::vec2(0.0, LANE), egui::Sense::hover());
									lanes.push((*index, lane.center().y));
								}
							}
						}
					});
					// A lane runs the column's whole width.
					let drawn = drawn.response.rect;
					for (index, y) in lanes {
						let through = (Pos2::new(drawn.left(), y), Pos2::new(drawn.right(), y));
						pass.lanes.entry(index).or_default().push(through);
					}
					ui.add_space(COLUMN_GAP);
				}
			});
			self.wires(ui, &mut pass, wires);
		});

		let Some(action) = pass.action else { return };
		let result = match action {
			Action::Wire(wire, on) => {
				// The items taking a picture out removes, which those after
				// them move down past.
				let removed = match wire.route() {
					Some(Route::Placed(source, layer)) if !on => {
						let items = state.collection().items(&layer).unwrap_or_default();
						let removed = (0..items.len()).filter(|&i| items[i].source == source);
						Some((layer, removed.collect::<Vec<usize>>()))
					}
					_ => None,
				};
				let result = edit(&mut state, wire.label(on), None, |draft| wire.apply(draft, on));
				if result.is_ok()
					&& let Some((layer, removed)) = removed
				{
					self.follow(&layer, |index| {
						let before = removed.iter().filter(|&&at| at < index).count();
						(!removed.contains(&index)).then_some(index - before)
					});
				}
				result
			}
			Action::Sound(name, audio, label, gesture) => {
				edit(&mut state, label, gesture, |draft| draft.set_audio(&name, Some(audio)))
			}
			Action::Scene(action) => self.scene_action(&mut state, action),
			Action::AudioScenes(scenes, label) => edit(&mut state, label, None, |draft| draft.set_audio_scenes(scenes)),
			Action::Listen(scene) => self.listen(&mut state, scene.as_deref()),
			Action::Preview(name) => {
				state.compositor.set_preview(Some(name));
				Ok(())
			}
		};
		drop(state);
		self.check(result);
	}

	/// Draw `node` and its ports, and start or end a wire dragged from them.
	fn node(&mut self, ui: &mut Ui, pass: &mut Pass<'_>, node: &Node) {
		// Where the node's first row sits, and its waveform if it has one.
		let mut header = 0.0;
		let mut sound = None;
		let frame = egui::Frame::group(ui.style()).fill(ui.visuals().window_fill);
		let body = frame
			.show(ui, |ui| {
				ui.set_width(NODE_WIDTH);
				(header, sound) = match node {
					Node::Source(name) => self.source_node(ui, pass, name),
					Node::Scene(name) => self.scene_node(ui, pass, name),
					Node::Top => {
						let label = ui.strong("Top layer");
						let label = label
							.on_hover_text("Drawn over program, and over a scene of the program's size in preview");
						(label.rect.center().y, None)
					}
					Node::Audio(name) => self.audio_node(ui, pass, name),
				};
			})
			.response
			.rect;
		pass.bodies.push(body);

		let wired_in = pass.graph.edges.iter().any(|edge| edge.wire.to == *node);
		if node.layer().is_some() || matches!(node, Node::Audio(_)) || wired_in {
			let at = Pos2::new(body.left(), header);
			pass.inputs.insert(node.clone(), at);
		}
		let mut outputs = Vec::new();
		match node {
			Node::Source(name) => {
				let source = &pass.state.collection().sources[name];
				if source.has_video() {
					outputs.push((Kind::Picture, header, true));
				}
				if let Some(sound) = sound {
					outputs.push((Kind::Sound, sound, true));
				}
			}
			// A scene goes into another through a scene source, which names it.
			Node::Scene(_) if pass.graph.edges.iter().any(|edge| edge.wire.from == *node) => {
				outputs.push((Kind::Picture, header, false));
			}
			Node::Audio(_) => outputs.push((Kind::Sound, sound.unwrap_or(header), true)),
			Node::Scene(_) | Node::Top => {}
		}
		for (kind, y, draggable) in outputs {
			let at = Pos2::new(body.right(), y);
			pass.outputs.insert((node.clone(), kind), at);
			if !draggable {
				continue;
			}
			let id = ui.id().with(("output", node, kind));
			let rect = egui::Rect::from_center_size(at, egui::Vec2::splat(REACH * 2.0));
			let response = ui.interact(rect, id, egui::Sense::drag());
			let response = response.on_hover_text("Drag onto a node's left edge to wire it there");
			if response.drag_started() {
				self.graph.dragging = Some((node.clone(), kind));
			}
			if response.drag_stopped()
				&& let Some(pointer) = ui.ctx().pointer_latest_pos()
			{
				pass.released = Some((node.clone(), kind, pointer));
			}
			if response.hovered() || response.dragged() {
				ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
			}
		}
	}

	/// A source's name, kind, and status, and its sound's strip and waveform.
	fn source_node(&mut self, ui: &mut Ui, pass: &mut Pass<'_>, name: &str) -> (f32, Option<f32>) {
		let collection = pass.state.collection();
		let source = &collection.sources[name];
		let header = ui.horizontal(|ui| {
			let selection = Selection::Source(name.to_string());
			let picked = self.selection.as_ref() == Some(&selection);
			let label = ui.selectable_label(picked, egui::RichText::new(name).strong());
			if source.has_video() && label.on_hover_text("Pick it for Filters").clicked() {
				self.selection = Some(selection);
			}
			if let Some(feed) = pass.state.sources.get(name) {
				status(ui, &feed.status());
			}
		});
		ui.add(egui::Label::new(egui::RichText::new(describe(source)).weak()).truncate());
		if !source.has_audio() {
			return (header.response.rect.center().y, None);
		}
		let audio = collection.audio.get(name).cloned().unwrap_or_default();
		ui.horizontal_wrapped(|ui| {
			ui.spacing_mut().slider_width = 90.0;
			if let Some((audio, label, gesture)) = strip_controls(ui, &mut self.gestures, name, &audio) {
				pass.action = Some(Action::Sound(name.to_string(), audio, label, gesture));
			}
		});
		let waveform = levels::waveform(ui, self.levels.sound(name));
		(header.response.rect.center().y, Some(waveform.rect.center().y))
	}

	/// A scene's name, which picks it for preview, and its settings.
	fn scene_node(&mut self, ui: &mut Ui, pass: &mut Pass<'_>, name: &str) -> (f32, Option<f32>) {
		let state = pass.state;
		let collection = state.collection();
		let Some(index) = collection.scenes.iter().position(|scene| scene.name == name) else {
			return (ui.min_rect().top(), None);
		};
		let scene = &collection.scenes[index];
		let header = ui.horizontal(|ui| {
			let previewed = state.compositor.preview_scene() == Some(name);
			let label = ui.selectable_label(previewed, egui::RichText::new(name).strong());
			if label.on_hover_text("Preview it").clicked() {
				pass.action = Some(Action::Preview(name.to_string()));
			}
			if state.compositor.program_scene() == Some(name) {
				ui.colored_label(Color32::RED, "on program");
			}
		});
		if let Some(size) = scene.size {
			ui.weak(format!("{}x{}", size.width, size.height));
		}
		let program_audio = collection.program_audio().unwrap_or_default();
		ui.horizontal_wrapped(|ui| {
			if let Some(action) = scene_settings(ui, index, scene, &collection.audio_scenes, program_audio) {
				pass.action = Some(Action::Scene(action));
			}
		});
		(header.response.rect.center().y, None)
	}

	/// An audio scene's name, whether it is on program, Listen, and its
	/// mix's waveform.
	fn audio_node(&mut self, ui: &mut Ui, pass: &mut Pass<'_>, name: &str) -> (f32, Option<f32>) {
		let scenes = &pass.state.collection().audio_scenes;
		let Some(scene) = scenes.iter().find(|scene| scene.name == name) else {
			return (ui.min_rect().top(), None);
		};
		let header = ui.horizontal(|ui| {
			ui.strong(name);
			ui.weak("audio");
		});
		ui.horizontal(|ui| {
			if ui.radio(scene.program, "On program").clicked() && !scene.program {
				let (scenes, label) = on_program(scenes, name);
				pass.action = Some(Action::AudioScenes(scenes, label));
			}
			let heard = self.engine.listening().is_some_and(|(heard, _)| heard == name);
			let response = ui
				.add_enabled(mix::MONITOR, egui::Button::selectable(heard, "Listen"))
				.on_hover_text("Play this audio scene to the speakers; use headphones")
				.on_disabled_hover_text("Needs a build with the `capture` feature");
			if response.clicked() {
				pass.action = Some(Action::Listen((!heard).then(|| name.to_string())));
			}
		});
		let waveform = levels::waveform(ui, self.levels.mix(name));
		(header.response.rect.center().y, Some(waveform.rect.center().y))
	}

	/// Draw the wires under the nodes into `slot`, and the ports over them;
	/// pick a wire clicked, and make one let go of on an input.
	fn wires(&mut self, ui: &mut Ui, pass: &mut Pass<'_>, slot: egui::layers::ShapeIdx) {
		let pointer = ui.ctx().pointer_latest_pos();
		let over_node = pointer.is_some_and(|pointer| pass.bodies.iter().any(|body| body.contains(pointer)));
		let in_view = pointer.is_some_and(|pointer| ui.clip_rect().contains(pointer));
		// A wire from `from` to `to`, leaving and arriving level.
		let bend = |from: Pos2, to: Pos2| {
			let reach = ((to.x - from.x) / 2.0).max(REACH * 4.0);
			let points = [from, from + egui::vec2(reach, 0.0), to - egui::vec2(reach, 0.0), to];
			CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::NONE).flatten(Some(0.5))
		};

		// The wire made by hand under the pointer, if no node or drag has it.
		let mut hovered: Option<&Wire> = None;
		let mut nearest = REACH;
		let mut paths = Vec::new();
		for (index, edge) in pass.graph.edges.iter().enumerate() {
			let wire = &edge.wire;
			let (Some(&from), Some(&to)) = (
				pass.outputs.get(&(wire.from.clone(), wire.kind)),
				pass.inputs.get(&wire.to),
			) else {
				continue;
			};
			// Through the lanes of the columns it skips.
			let mut points = Vec::new();
			let mut at = from;
			for &(left, right) in pass.lanes.get(&index).into_iter().flatten() {
				points.extend(bend(at, left));
				at = right;
			}
			points.extend(bend(at, to));
			if let Some(pointer) = pointer
				&& in_view && !over_node
				&& self.graph.dragging.is_none()
				&& wire.route().is_some()
			{
				let distance = distance(&points, pointer);
				if distance < nearest {
					nearest = distance;
					hovered = Some(wire);
				}
			}
			paths.push((edge, points));
		}

		let mut shapes = Vec::new();
		for (edge, points) in paths {
			let picked = self.graph.picked.as_ref() == Some(&edge.wire);
			if picked {
				let halo = Stroke::new(8.0, ui.visuals().selection.bg_fill);
				shapes.push(egui::Shape::line(points.clone(), halo));
			}
			let mut color = edge.wire.kind.color();
			if !edge.shown {
				color = color.gamma_multiply(0.35);
			}
			let width = if picked || hovered == Some(&edge.wire) {
				3.5
			} else {
				2.0
			};
			shapes.push(egui::Shape::line(points, Stroke::new(width, color)));
		}
		// The wire being dragged, to the pointer.
		let dragging = self.graph.dragging.clone();
		if let Some((from, kind)) = &dragging
			&& let Some(&at) = pass.outputs.get(&(from.clone(), *kind))
			&& let Some(pointer) = pointer
		{
			shapes.push(egui::Shape::line(bend(at, pointer), Stroke::new(2.0, kind.color())));
		}
		ui.painter().set(slot, egui::Shape::Vec(shapes));

		// The ports, with those a dragged wire can end on lit.
		let collection = pass.state.collection();
		let ring = ui.visuals().window_stroke.color;
		for ((_, kind), &at) in &pass.outputs {
			ui.painter().circle(at, PORT, kind.color(), Stroke::new(1.0, ring));
		}
		for (node, &at) in &pass.inputs {
			let kind = match node {
				Node::Audio(_) => Kind::Sound,
				_ => Kind::Picture,
			};
			let takes = dragging.as_ref().is_some_and(|(from, kind)| {
				let wire = Wire {
					from: from.clone(),
					to: node.clone(),
					kind: *kind,
				};
				wire.offered(collection)
			});
			let radius = if takes { PORT * 1.6 } else { PORT };
			let stroke = match takes {
				true => Stroke::new(2.0, ui.visuals().selection.stroke.color),
				false => Stroke::new(1.0, ring),
			};
			ui.painter().circle(at, radius, kind.color(), stroke);
		}

		if let Some((from, kind, at)) = pass.released.take() {
			let target = pass.inputs.iter().find(|(node, input)| {
				let wire = Wire {
					from: from.clone(),
					to: (*node).clone(),
					kind,
				};
				input.distance(at) <= REACH * 1.5 && wire.offered(collection)
			});
			if let Some((to, _)) = target {
				let wire = Wire {
					from,
					to: to.clone(),
					kind,
				};
				pass.action = Some(Action::Wire(wire, true));
			}
		}
		if in_view && !over_node && ui.input(|input| input.pointer.primary_clicked()) {
			self.graph.picked = hovered.cloned();
		}
	}
}

/// How far `at` is from the line through `points`.
fn distance(points: &[Pos2], at: Pos2) -> f32 {
	let segment = |a: Pos2, b: Pos2| {
		let along = b - a;
		let t = ((at - a).dot(along) / along.length_sq().max(f32::EPSILON)).clamp(0.0, 1.0);
		(a + along * t).distance(at)
	};
	points
		.windows(2)
		.map(|pair| segment(pair[0], pair[1]))
		.fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn collection(toml: &str) -> Collection {
		let head = "version = 1\n[canvas]\nwidth = 1280\nheight = 720\nframerate = 30\n";
		Collection::from_toml(&format!("{head}{toml}")).unwrap()
	}

	fn source(name: &str) -> Node {
		Node::Source(name.into())
	}

	fn wire(from: Node, to: Node, kind: Kind) -> Wire {
		Wire { from, to, kind }
	}

	/// Each node sits right of everything wired into it, a scene drawn
	/// through a scene source two columns past it, and pictures above sounds.
	#[test]
	fn lays_out_left_to_right() {
		let collection = collection(
			r#"
[sources.cam]
kind = "camera"
[sources.mic]
kind = "microphone"
[sources.clip]
kind = "media"
path = "clip.mp4"
[sources.grade]
kind = "lut"
path = "grade.cube"
[sources.inset]
kind = "scene"
scene = "Inset"

[[scenes]]
name = "Inset"
[[scenes.items]]
source = "cam"
rect = { x = 0, y = 0, width = 640, height = 360 }
visible = false

[[scenes]]
name = "Main"
[[scenes.items]]
source = "inset"
rect = { x = 0, y = 0, width = 640, height = 360 }

[[audio_scenes]]
name = "Program"
program = true
sounds = ["mic"]
scenes = ["Music"]

[[audio_scenes]]
name = "Music"
sounds = ["clip"]
"#,
		);
		let graph = Graph::of(&collection);
		let name = |slot: &Slot| match slot {
			Slot::Node(node) => format!("{node:?}"),
			Slot::Lane(index) => format!(
				"{:?} -> {:?}",
				graph.edges[*index].wire.from, graph.edges[*index].wire.to
			),
		};
		let columns: Vec<Vec<String>> = graph
			.columns
			.iter()
			.map(|column| column.iter().map(name).collect())
			.collect();
		// The microphone's wire passes the first audio scene's column in a lane.
		assert_eq!(
			columns,
			[
				vec![r#"Source("cam")"#, r#"Source("clip")"#, r#"Source("mic")"#],
				vec![
					r#"Scene("Inset")"#,
					"Top",
					r#"Audio("Music")"#,
					r#"Source("mic") -> Audio("Program")"#,
				],
				vec![r#"Source("inset")"#, r#"Audio("Program")"#],
				vec![r#"Scene("Main")"#],
			]
		);
		// A hidden item keeps its wire, dimmed; the LUT has none.
		let hidden = graph
			.edges
			.iter()
			.find(|edge| edge.wire.to == Node::Scene("Inset".into()));
		assert!(!hidden.unwrap().shown);
		assert!(graph.edges.iter().all(|edge| edge.wire.from != source("grade")));
		// Only wires into scenes, the top layer, and audio scenes are made by hand.
		let fixed = wire(Node::Scene("Inset".into()), source("inset"), Kind::Picture);
		assert!(graph.edges.iter().any(|edge| edge.wire == fixed));
		assert!(fixed.route().is_none());
		assert!(
			wire(source("mic"), Node::Scene("Main".into()), Kind::Sound)
				.route()
				.is_none()
		);
	}

	/// A wire places its source to fill the layer, and taking it out removes
	/// every item showing the source there; sends toggle; neither twice.
	#[test]
	fn wires_edit_the_collection() {
		let collection = collection(
			r#"
[sources.cam]
kind = "camera"
[sources.mic]
kind = "microphone"

[[scenes]]
name = "Main"
size = { width = 640, height = 360 }
[[scenes.items]]
source = "cam"
rect = { x = 0, y = 0, width = 10, height = 10 }
[[scenes.items]]
source = "cam"
rect = { x = 10, y = 0, width = 10, height = 10 }
"#,
		);
		let run = |collection: &Collection, wire: &Wire, on: bool| -> Result<Collection, moq_scene::Error> {
			let mut draft = collection.draft(wire.label(on));
			wire.apply(&mut draft, on)?;
			let next = draft.collection().clone();
			draft.finish()?;
			Ok(next)
		};

		let main = Node::Scene("Main".into());
		let placed = wire(source("cam"), main.clone(), Kind::Picture);
		assert!(run(&collection, &placed, true).is_err(), "already placed");
		let removed = run(&collection, &placed, false).unwrap();
		assert!(removed.scene("Main").unwrap().items.is_empty());
		let added = run(&removed, &placed, true).unwrap();
		let items = &added.scene("Main").unwrap().items;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].rect, Rect::fill(&added.canvas(Some("Main")).unwrap()));

		let program = Node::Audio("Program".into());
		let sent = wire(source("mic"), program.clone(), Kind::Sound);
		let heard = run(&collection, &sent, true).unwrap();
		assert!(heard.audio_scene("Program").unwrap().sounds.contains("mic"));
		assert!(run(&heard, &sent, true).is_err(), "already sent");
		assert!(run(&collection, &sent, false).is_err(), "never sent");

		// An audio scene into itself, even through another, is refused.
		let mut nested = heard.clone();
		nested.audio_scenes.push(AudioScene::new("Music"));
		let into = wire(program.clone(), Node::Audio("Music".into()), Kind::Sound);
		let nested = run(&nested, &into, true).unwrap();
		let back = wire(Node::Audio("Music".into()), program.clone(), Kind::Sound);
		assert!(!back.offered(&nested));
		assert!(run(&nested, &back, true).is_err());
		assert!(wire(program.clone(), program, Kind::Sound).route().is_none());
	}

	#[test]
	fn distance_to_a_bent_line() {
		let points = [Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0)];
		assert_eq!(distance(&points, Pos2::new(5.0, 3.0)), 3.0);
		assert_eq!(distance(&points, Pos2::new(13.0, 5.0)), 3.0);
		assert_eq!(distance(&points, Pos2::new(-4.0, -3.0)), 5.0);
	}
}
