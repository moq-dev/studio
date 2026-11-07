//! The Rune spike (quest/m1/rune-spike.md): the Preview and Program monitors
//! on Rune instead of egui. The compositor's textures draw in Rune's frames on
//! the device they share, items drag and resize in the preview as the egui
//! window's do, and Cut and Transition take the preview to program.
//!
//! A prototype for a verdict, not for `main`: it shows only the monitors, and
//! logs `bench rune ui` every 60 frames for comparing frame time with egui.
//!
//! ```sh
//! sh/rune-spike/setup.sh   # Rune at the pinned rev, patched with `adopt_gpu`
//! STUDIO_UI=rune cargo run -p moq-studio --features rune -- show.toml
//! # Headless, with Rune's own flags:
//! STUDIO_UI=rune RUNE_ARGS="--script drag.json --snap out.png --tree tree.json" \
//!     cargo run -p moq-studio --features rune -- show.toml
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use kio::clock::Instant;
use moq_scene::{Canvas, Item, Layer, Rect, wgpu};
use rune::compose::{Compositor, Frame};
use rune::render::{CLIP_WGSL, EffectSite};
use rune::{App, AppSpec, Color, Cursor, Cx, Drag, El, FrameEncoded, FrameOutcome, Mods, Phase, Tx, col, effect, el, row, spacer, text};
use rune_kit::{Button, ButtonVariant, Checkbox, Kit, SegmentedControl};

use crate::controls::{Action, Controls};
use crate::engine::{Engine, Gpu, State};
use crate::snap::{self, Grab, Snap};

/// The effect slots the monitors draw their pictures in.
const PREVIEW: u32 = 1;
const PROGRAM: u32 = 2;

/// How far, in points on screen, the pointer reaches to an edge.
const REACH: f32 = 8.0;

/// Show the monitors until the window closes, drawing on `gpu`, the device
/// the engine renders on.
pub fn run(engine: Arc<Engine>, gpu: Gpu, runtime: tokio::runtime::Handle) -> anyhow::Result<()> {
	engine.state.lock().unwrap().compositor.set_previewing(true);
	// The load the egui window is measured under too.
	if std::env::var_os("BENCH_RECORD").is_some() {
		engine.recorder.start(false)?;
	}
	let device = gpu.device.clone();
	// Rune opens its own device unless handed one; this is the spike's patch.
	rune::adopt_gpu(rune::render::Gpu {
		instance: gpu.instance,
		adapter: gpu.adapter,
		device: gpu.device,
		queue: gpu.queue,
	});
	let controls = Arc::new(Controls::new(engine.clone(), runtime));
	let waits = Arc::new(Waits::default());
	let spec = AppSpec {
		poll: Some(poll),
		..AppSpec::new(
			"MoQ Studio",
			(1440.0, 900.0),
			|| -> Spike { unreachable!("the spike's state comes from App::init") },
			view,
		)
	};
	// Rune's own flags, such as `--snap` and `--tree`, which the studio's
	// command line would refuse.
	let args: Vec<String> = std::env::var("RUNE_ARGS")
		.map(|args| args.split_whitespace().map(str::to_string).collect())
		.unwrap_or_default();
	let monitors = Monitors {
		engine: engine.clone(),
		waits: waits.clone(),
		device,
		draw: None,
	};
	let mut bench = Bench::new(waits.clone());
	App::new(spec)
		.init(move || Spike {
			engine,
			controls,
			waits,
			selected: None,
			held: None,
			hovered: None,
			snapping: true,
			gesture: 0,
			message: None,
			shown: Shown::default(),
		})
		.args(args)
		.compositor(monitors)
		.on_frame_encoded(move |frame| bench.frame(frame))
		.try_run()
		.map_err(anyhow::Error::msg)
}

/// What the window keeps between frames.
struct Spike {
	engine: Arc<Engine>,
	controls: Arc<Controls>,
	waits: Arc<Waits>,
	/// The selected item of the preview's scene, by index.
	selected: Option<usize>,
	held: Option<Held>,
	/// The item under the pointer, and what a press would grab of it.
	hovered: Option<(usize, Grab)>,
	snapping: bool,
	/// Numbers each drag, so its edits undo as one.
	gesture: u64,
	message: Option<Result<String, String>>,
	/// What the view last drew from the engine, so a poll redraws only when
	/// it changes.
	shown: Shown,
}

/// An item a drag in the preview holds, as the egui window's `Drag`.
struct Held {
	layer: Layer,
	index: usize,
	grab: Grab,
	rect: Rect,
	origin: [f32; 2],
	fixed: bool,
	gesture: u64,
	hints: Option<snap::Snapped>,
}

/// What the view shows of the engine's state.
#[derive(Default, PartialEq)]
struct Shown {
	scenes: Vec<String>,
	preview: Option<String>,
	program: Option<String>,
	transitioning: bool,
	preview_canvas: Option<Canvas>,
	program_canvas: Option<Canvas>,
	/// The preview scene's items where their keyframes have them.
	items: Vec<Item>,
}

impl Shown {
	fn of(state: &State) -> Self {
		let preview = state.compositor.preview_scene().map(str::to_string);
		let since = state.compositor.preview_since();
		let items = preview
			.as_deref()
			.and_then(|scene| state.collection().items(&Layer::scene(scene)))
			.unwrap_or_default()
			.iter()
			.map(|item| item.posed(since).into_owned())
			.collect();
		Self {
			scenes: state.collection().scenes.iter().map(|scene| scene.name.clone()).collect(),
			preview,
			program: state.compositor.program_scene().map(str::to_string),
			transitioning: state.compositor.transitioning(),
			preview_canvas: Some(state.compositor.preview_canvas()),
			program_canvas: Some(state.compositor.canvas()),
			items,
		}
	}
}

impl Spike {
	fn lock(&self) -> MutexGuard<'_, State> {
		lock(&self.engine.state, &self.waits)
	}

	fn pick(&mut self, scene: &str) {
		let result = crate::controls::preview(&mut self.lock(), scene);
		self.selected = None;
		self.report(result);
	}

	fn fire(&mut self, action: Action) {
		let result = self.controls.fire(&action);
		self.report(result);
	}

	fn report(&mut self, result: anyhow::Result<String>) {
		self.message = Some(result.map_err(|err| format!("{err:#}")));
	}

	/// Select, move, and resize the preview's items, as the egui window's
	/// `Panels::arrange` does.
	fn drag(&mut self, drag: &Drag, mods: Mods) {
		let engine = self.engine.clone();
		let mut state = lock(&engine.state, &self.waits);
		let Some(scene) = state.compositor.preview_scene().map(str::to_string) else { return };
		let layer = Layer::scene(&scene);
		let canvas = state.compositor.preview_canvas();
		if drag.w <= 0.0 {
			return;
		}
		// Canvas pixels per point.
		let scale = canvas.width as f32 / drag.w;
		let point = [drag.x * scale, drag.y * scale];
		let Some(resting) = state.collection().items(&layer).map(<[Item]>::to_vec) else { return };
		let since = state.compositor.preview_since();
		if drag.phase == Phase::Down {
			let posed: Vec<Item> = resting.iter().map(|item| item.posed(since).into_owned()).collect();
			self.held = None;
			self.selected = None;
			let Some((index, grab)) = snap::hit(&posed, self.selected, point, REACH * scale) else { return };
			self.selected = Some(index);
			self.gesture += 1;
			let collection = state.collection();
			let source = collection.sources.get(collection.capture(&resting[index].source));
			self.held = Some(Held {
				layer,
				index,
				grab,
				rect: resting[index].rect,
				origin: point,
				fixed: crate::ui::fixed_shape(source),
				gesture: self.gesture,
				hints: None,
			});
			return;
		}
		let Some(held) = &mut self.held else { return };
		let delta = [(point[0] - held.origin[0]).round(), (point[1] - held.origin[1]).round()];
		let Some(item) = resting.get(held.index).cloned() else { return };
		let aspect = (held.fixed != mods.shift).then(|| snap::aspect(&held.rect)).flatten();
		let rect = held.grab.drag(held.rect, delta, aspect);
		held.hints = (self.snapping && !mods.cmd).then(|| {
			let shown = item.posed(since).rect;
			let snap = Snap {
				reach: REACH * scale,
				grid: None,
				aspect,
				posed: Rect {
					x: shown.x - item.rect.x,
					y: shown.y - item.rect.y,
					width: shown.width - item.rect.width,
					height: shown.height - item.rect.height,
				},
			};
			let others = crate::ui::others(&state, &held.layer, held.index, since);
			snap.apply(held.grab, rect, &others, &canvas)
		});
		let rect = held.hints.as_ref().map_or(rect, |snapped| snapped.rect);
		let mut result = Ok(());
		if item.rect != rect {
			let verb = if held.grab == Grab::Move { "Move" } else { "Resize" };
			let label = format!("{verb} `{}`", item.source);
			let (layer, index) = (held.layer.clone(), held.index);
			result = crate::ui::edit(&mut state, label, Some(held.gesture), |draft| {
				draft.set_item(&layer, index, Item { rect, ..item })
			});
		}
		if drag.phase == Phase::Up {
			self.held = None;
		}
		drop(state);
		if let Err(err) = result {
			self.message = Some(Err(format!("{err:#}")));
		}
	}

	fn hover(&mut self, point: [f32; 2], width: f32) {
		let Some(canvas) = self.shown.preview_canvas.filter(|_| width > 0.0) else { return };
		let scale = canvas.width as f32 / width;
		let point = [point[0] * scale, point[1] * scale];
		self.hovered = snap::hit(&self.shown.items, self.selected, point, REACH * scale);
	}
}

/// Take `mutex`, adding how long it waited to `waits`.
fn lock<'a>(mutex: &'a Mutex<State>, waits: &Waits) -> MutexGuard<'a, State> {
	let start = Instant::now();
	let guard = mutex.lock().unwrap();
	waits.add(start.elapsed());
	guard
}

/// Runs each frame, since the compositor's monitors animate: redraw the view
/// when what it shows of the engine changed.
fn poll(s: &mut Spike, _t: f64, _tx: &mut Tx) -> bool {
	let shown = Shown::of(&s.lock());
	if shown == s.shown {
		return false;
	}
	s.shown = shown;
	true
}

fn view(s: &Spike, cx: &mut Cx<Spike>) -> El {
	let k = Kit::neutral(true);
	let t = rune::tok::theme(true);
	let accent = rune::tok::accent(0, true);
	let shown = &s.shown;

	let names = shown.scenes.clone();
	let picked = shown.preview.as_ref().and_then(|name| names.iter().position(|scene| scene == name));
	let message = match &s.message {
		Some(Ok(message)) => text(message.clone()).color(t.text_secondary),
		Some(Err(message)) => text(message.clone()).color(rune::tok::accent(2, true)),
		None => el(),
	};
	let bar = row()
		.w_full()
		.gap(8.0)
		.padding(8.0)
		.items_center()
		.child(text("Scenes").color(t.text_secondary))
		.child(
			SegmentedControl::new(&names, picked)
				.fit()
				.label("Scene in preview")
				.on_pick(move |s: &mut Spike, i, _tx: &mut Tx| s.pick(&names[i]))
				.build(&k),
		)
		.child(spacer())
		.child(message);

	// The preview, with its items' outlines, handles, and snap guides over it.
	let size = cx.layout_size("preview").unwrap_or_default();
	let canvas = shown.preview_canvas.unwrap_or_default();
	let points = if canvas.width > 0 { size[0] / canvas.width as f32 } else { 0.0 };
	let mut overlay = Vec::new();
	let outline = |rect: &Rect, width: f32, color: Color| {
		el().absolute(rect.x * points, rect.y * points)
			.size(rect.width * points, rect.height * points)
			.border(width, color)
			.inert()
	};
	if let Some(item) = s.selected.and_then(|index| shown.items.get(index)).filter(|item| item.visible) {
		overlay.push(outline(&item.rect, 1.5, accent));
		let r = &item.rect;
		for x in [r.x, r.x + r.width / 2.0, r.x + r.width] {
			for y in [r.y, r.y + r.height / 2.0, r.y + r.height] {
				if (x, y) != (r.x + r.width / 2.0, r.y + r.height / 2.0) {
					overlay.push(el().absolute(x * points - 3.0, y * points - 3.0).size(6.0, 6.0).radius(1.0).fill(accent).inert());
				}
			}
		}
	}
	if let Some((index, _)) = s.hovered.filter(|(index, _)| s.held.is_none() && Some(*index) != s.selected)
		&& let Some(item) = shown.items.get(index)
	{
		overlay.push(outline(&item.rect, 1.0, accent.alpha(0.6)));
	}
	let guide = rune::color::hex("#ff40a0").unwrap_or(accent);
	if let Some(snapped) = s.held.as_ref().and_then(|held| held.hints.as_ref()) {
		for line in &snapped.guides {
			let (x, y, w, h) = match line.axis {
				snap::Axis::X => (line.at, line.from, 0.0, line.to - line.from),
				snap::Axis::Y => (line.from, line.at, line.to - line.from, 0.0),
			};
			overlay.push(
				el().absolute(x * points, y * points)
					.size((w * points).max(1.0), (h * points).max(1.0))
					.fill(guide)
					.inert(),
			);
		}
	}
	let grab = s.held.as_ref().map(|held| held.grab).or(s.hovered.map(|(_, grab)| grab));
	let width = size[0];
	let preview = effect(PREVIEW)
		.key("preview")
		.aspect_fit(ratio(canvas))
		.cursor(grab.map_or(Cursor::Default, cursor))
		.on_drag(|s: &mut Spike, drag: &mut Drag, tx: &mut Tx| s.drag(drag, tx.mods()))
		.on_pointer_move_local(move |s: &mut Spike, pointer, _tx: &mut Tx| s.hover([pointer.x, pointer.y], width))
		.children(overlay);
	let preview_header = row()
		.gap(12.0)
		.items_center()
		.child(text(format!("Preview: {}", shown.preview.as_deref().unwrap_or("nothing"))).color(t.text))
		.child(
			Checkbox::new(s.snapping)
				.label("Snap")
				.on_toggle(|s: &mut Spike| s.snapping = !s.snapping)
				.build(&k),
		);

	let mut program_header = row()
		.gap(8.0)
		.items_center()
		.child(
			Button::new("Cut")
				.variant(ButtonVariant::Contrast)
				.on_press(|s: &mut Spike| s.fire(Action::Cut))
				.build(&k),
		)
		.child(
			Button::new("Transition")
				.on_press(|s: &mut Spike| s.fire(Action::Transition))
				.build(&k),
		)
		.child(text(format!("Program: {}", shown.program.as_deref().unwrap_or("nothing"))).color(t.text));
	if shown.transitioning {
		program_header = program_header.child(text("in transition").color(t.text_secondary));
	}
	let program = effect(PROGRAM)
		.key("program")
		.aspect_fit(ratio(shown.program_canvas.unwrap_or_default()));

	let monitors = row()
		.w_full()
		.grow(1.0)
		.min_h(0.0)
		.gap(8.0)
		.padding(8.0)
		.child(monitor(preview_header, preview))
		.child(monitor(program_header, program));
	col().w_full().h_full().fill(t.bg).child(bar).child(monitors)
}

/// A monitor's header over its picture, which keeps the canvas's shape in
/// whatever room is left.
fn monitor(header: El, picture: El) -> El {
	col().grow(1.0)
		.min_w(0.0)
		.min_h(0.0)
		.h_full()
		.gap(6.0)
		.child(header)
		.child(col().grow(1.0).min_h(0.0).w_full().center().child(picture))
}

fn ratio(canvas: Canvas) -> f32 {
	match canvas.height {
		0 => 16.0 / 9.0,
		height => canvas.width as f32 / height as f32,
	}
}

/// Rune has no diagonal resize cursors, so a corner shows a crosshair.
fn cursor(grab: Grab) -> Cursor {
	match grab {
		Grab::Resize { x: Some(_), y: None } => Cursor::ResizeColumn,
		Grab::Resize { x: None, y: Some(_) } => Cursor::ResizeRow,
		Grab::Resize { x: Some(_), y: Some(_) } => Cursor::Crosshair,
		Grab::Move | Grab::Resize { x: None, y: None } => Cursor::Move,
	}
}

/// Draws the compositor's preview and program textures at the monitors'
/// effect slots, sampling them where the compositor left them.
struct Monitors {
	engine: Arc<Engine>,
	waits: Arc<Waits>,
	/// The device the compositor renders on, which the frame must be on too.
	device: wgpu::Device,
	draw: Option<Draw>,
}

impl Compositor<Spike> for Monitors {
	fn render(&mut self, f: &mut Frame<'_, Spike>) {
		let clear = f.rt.clear_color();
		f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
		// wgpu compares devices by id, which another instance's device can
		// share, so this catches a missing `adopt_gpu` only most of the time.
		if *f.device != self.device {
			tracing::error!("Rune draws on a device of its own, which cannot sample the compositor's textures");
			f.ui.draw(f.encoder, f.target, clear);
			return;
		}
		let draw = self.draw.get_or_insert_with(|| Draw::new(f.device));
		{
			let state = lock(&self.engine.state, &self.waits);
			draw.slots[0].follow(f.device, &draw.layout, &draw.sampler, state.compositor.preview());
			draw.slots[1].follow(f.device, &draw.layout, &draw.sampler, state.compositor.program());
		}
		let (device, queue) = (f.device, f.queue);
		f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| draw.paint(device, queue, pass, site));
	}

	/// The monitors are live video.
	fn animating(&self) -> bool {
		true
	}
}

struct Draw {
	layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	module: wgpu::ShaderModule,
	pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
	slots: [Slot; 2],
}

const SHADER: &str = "
struct U { rect: vec4f, clip: vec4f, fade: vec4<u32>, frame: vec4f, flags: vec4f }
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
struct V { @builtin(position) pos: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@builtin(vertex_index) vi: u32) -> V {
	let c = vec2f(f32(vi & 1u), f32(vi >> 1u));
	let l = (u.rect.xy + c * u.rect.zw - u.frame.xy) / u.frame.zw;
	return V(vec4f(l.x * 2.0 - 1.0, 1.0 - l.y * 2.0, 0.0, 1.0), c);
}
fn decode(x: vec3f) -> vec3f {
	return select(pow((x + 0.055) / 1.055, vec3f(2.4)), x / 12.92, x <= vec3f(0.04045));
}
@fragment fn fs(v: V) -> @location(0) vec4f {
	let a = clip_alpha(v.pos.xy + u.frame.xy, u.clip, u.fade);
	var rgb = textureSample(picture, bilinear, v.uv).rgb;
	// The compositor's texture holds sRGB-encoded values, which an sRGB-blended
	// frame keeps and a linear one decodes.
	if (u.flags.x < 0.5) { rgb = decode(rgb); }
	return vec4f(rgb * a, a);
}
";

impl Draw {
	fn new(device: &wgpu::Device) -> Self {
		let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
			binding,
			visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
			ty,
			count: None,
		};
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("monitor"),
			entries: &[
				entry(
					0,
					wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
				),
				entry(
					1,
					wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
				),
				entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
			],
		});
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("monitor"),
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});
		let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("monitor"),
			source: wgpu::ShaderSource::Wgsl(format!("{CLIP_WGSL}\n{SHADER}").into()),
		});
		let slot = || Slot {
			uniform: device.create_buffer(&wgpu::BufferDescriptor {
				label: Some("monitor"),
				size: 80,
				usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
				mapped_at_creation: false,
			}),
			texture: None,
			group: None,
		};
		Self {
			layout,
			sampler,
			module,
			pipelines: Vec::new(),
			slots: [slot(), slot()],
		}
	}

	/// The pipeline for a pass in `format`: the window's, or an isolated layer's.
	fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
		if let Some(at) = self.pipelines.iter().position(|(made, _)| *made == format) {
			return &self.pipelines[at].1;
		}
		let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("monitor"),
			bind_group_layouts: &[Some(&self.layout)],
			immediate_size: 0,
		});
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("monitor"),
			layout: Some(&layout),
			vertex: wgpu::VertexState {
				module: &self.module,
				entry_point: Some("vs"),
				compilation_options: Default::default(),
				buffers: &[],
			},
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleStrip,
				..Default::default()
			},
			depth_stencil: None,
			multisample: Default::default(),
			fragment: Some(wgpu::FragmentState {
				module: &self.module,
				entry_point: Some("fs"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format,
					blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			multiview_mask: None,
			cache: None,
		});
		self.pipelines.push((format, pipeline));
		&self.pipelines.last().unwrap().1
	}

	fn paint(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, site: &EffectSite<'_>) {
		let at = match site.mark {
			PREVIEW => 0,
			PROGRAM => 1,
			_ => return,
		};
		if self.slots[at].group.is_none() {
			return;
		}
		let i = site.instance;
		let mut bytes = Vec::with_capacity(80);
		for value in i.rect.iter().chain(&i.clip) {
			bytes.extend_from_slice(&value.to_ne_bytes());
		}
		for value in &i.fade {
			bytes.extend_from_slice(&value.to_ne_bytes());
		}
		let srgb = if site.srgb_blend { 1.0f32 } else { 0.0 };
		let frame = [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32, srgb, 0.0, 0.0, 0.0];
		for value in frame {
			bytes.extend_from_slice(&value.to_ne_bytes());
		}
		// Lands ahead of the frame's commands, which the frame submits after this.
		queue.write_buffer(&self.slots[at].uniform, 0, &bytes);
		let pipeline = self.pipeline(device, site.format).clone();
		pass.set_pipeline(&pipeline);
		pass.set_bind_group(0, self.slots[at].group.as_ref(), &[]);
		pass.draw(0..4, 0..1);
	}
}

/// One monitor's picture: the texture it samples, bound with its uniforms.
struct Slot {
	uniform: wgpu::Buffer,
	texture: Option<wgpu::Texture>,
	group: Option<wgpu::BindGroup>,
}

impl Slot {
	/// Sample `texture` from now on, if a resize replaced the one bound.
	fn follow(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, texture: &wgpu::Texture) {
		if self.texture.as_ref() == Some(texture) {
			return;
		}
		let view = texture.create_view(&Default::default());
		self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("monitor"),
			layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: self.uniform.as_entire_binding(),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&view),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Sampler(sampler),
				},
			],
		}));
		self.texture = Some(texture.clone());
	}
}

/// How long the window's thread waited on the engine's state lock.
#[derive(Default)]
struct Waits {
	nanos: AtomicU64,
	count: AtomicU64,
	max: AtomicU64,
}

impl Waits {
	fn add(&self, waited: std::time::Duration) {
		let nanos = waited.as_nanos() as u64;
		self.nanos.fetch_add(nanos, Ordering::Relaxed);
		self.count.fetch_add(1, Ordering::Relaxed);
		self.max.fetch_max(nanos, Ordering::Relaxed);
	}

	/// The total, the count, and the longest since the last take.
	fn take(&self) -> (f64, u64, f64) {
		let ms = |nanos: u64| nanos as f64 / 1e6;
		(
			ms(self.nanos.swap(0, Ordering::Relaxed)),
			self.count.swap(0, Ordering::Relaxed),
			ms(self.max.swap(0, Ordering::Relaxed)),
		)
	}
}

/// Frame times, logged every 60 drawn frames as `bench rune ui`, beside the
/// egui window's `bench ui` from the same instrumentation.
struct Bench {
	waits: Arc<Waits>,
	#[expect(clippy::disallowed_types, reason = "Rune stamps its frames on std's clock")]
	last: Option<std::time::Instant>,
	frames: u32,
	interval: (f64, f64),
	cpu: (f64, f64),
	update: f64,
	encode: f64,
}

impl Bench {
	fn new(waits: Arc<Waits>) -> Self {
		Self {
			waits,
			last: None,
			frames: 0,
			interval: (0.0, 0.0),
			cpu: (0.0, 0.0),
			update: 0.0,
			encode: 0.0,
		}
	}

	fn frame(&mut self, frame: &FrameEncoded) {
		if frame.outcome() != FrameOutcome::Drawn {
			return;
		}
		let at = frame.at();
		let Some(last) = self.last.replace(at) else { return };
		let interval = (at - last).as_secs_f64() * 1e3;
		let (update, encode) = (frame.update_ms().unwrap_or_default(), frame.encode_ms().unwrap_or_default());
		self.interval = (self.interval.0 + interval, self.interval.1.max(interval));
		self.cpu = (self.cpu.0 + update + encode, self.cpu.1.max(update + encode));
		self.update += update;
		self.encode += encode;
		self.frames += 1;
		if self.frames < 60 {
			return;
		}
		let n = f64::from(self.frames);
		let (wait, locks, wait_max) = self.waits.take();
		tracing::info!(
			interval_ms = format!("{:.2} max {:.2}", self.interval.0 / n, self.interval.1),
			cpu_ms = format!("{:.2} max {:.2}", self.cpu.0 / n, self.cpu.1),
			update_ms = format!("{:.2}", self.update / n),
			encode_ms = format!("{:.2}", self.encode / n),
			lock_wait_ms = format!("{:.3} max {:.2} locks {}", wait / n, wait_max, locks),
			"bench rune ui"
		);
		*self = Self::new(self.waits.clone());
		self.last = Some(at);
	}
}
