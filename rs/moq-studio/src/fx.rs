//! Effects in the studio's own window: glows, pulses, bursts, and fills from
//! rune-fx, drawn inside egui's render pass through paint callbacks. They
//! never reach the program, a published scene, or any encode.
//!
//! A widget marks where its effect goes as it draws ([`Mark`], [`Spot`]) and
//! hands the spot to its effect at once; the effects are filled in once the
//! frame is done, and every layer of a frame is uploaded once, by whichever
//! of its callbacks egui prepares first. Their colors are the kit's.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use eframe::egui_wgpu;
use egui::{Color32, Rect};
use rune_fx::{
	ElectricParams, ElectricStyle, Fx as Effect, FxFrame, FxLayer, FxOptions, FxRenderer, GlowParams, GlowStyle,
	GradientKind, GrainGradientParams, LiquidMetalParams, PlasticParams, PulseParams, PulseStyle, Rgb, Shape,
	SparkleParams, Stops,
};

use crate::health::Verdict;
use crate::kit;

mod motion;

/// The window's effects: whether they move, and this frame's layers.
pub struct Fx {
	/// The system's reduced-motion preference, or `None` to always move.
	system: Option<motion::System>,
	/// What this frame shows, and when: whether its effects hold still,
	/// seconds since the window opened, and the layers' clock, which wraps.
	reduced: bool,
	time: f64,
	clock: f32,
	layers: Vec<FxLayer>,
	frame: Arc<Frame>,
	frames: u64,
	look: Look,
}

/// The layers of one frame, shared by its callbacks once the frame is done.
struct Frame {
	id: u64,
	layers: OnceLock<Vec<FxLayer>>,
}

impl Fx {
	/// Draw on `render`'s device, following the system's reduced motion.
	pub fn new(render: &egui_wgpu::RenderState, ctx: &egui::Context, runtime: &tokio::runtime::Handle) -> Self {
		Self::with(render, Some(motion::System::new(ctx, runtime)))
	}

	/// Draw on `render`'s device, always moving whatever the system asks, so
	/// a view looks the same on every machine.
	#[cfg(test)]
	pub fn moving(render: &egui_wgpu::RenderState) -> Self {
		Self::with(render, None)
	}

	fn with(render: &egui_wgpu::RenderState, system: Option<motion::System>) -> Self {
		let mut fx = FxRenderer::new(&render.device, render.target_format, FxOptions { dither: true });
		// egui blends in gamma space on a non-sRGB target, as a Rune frame
		// with sRGB blending does.
		fx.set_srgb_blend(!render.target_format.is_srgb());
		render
			.renderer
			.write()
			.callback_resources
			.insert(Renderer { fx, prepared: None });
		Self {
			system,
			reduced: false,
			time: 0.0,
			clock: 0.0,
			layers: Vec::new(),
			frame: Arc::new(Frame {
				id: 0,
				layers: OnceLock::new(),
			}),
			frames: 0,
			look: Look::default(),
		}
	}

	/// Start a frame: what it shows, and when.
	pub fn begin(&mut self, ctx: &egui::Context) {
		self.reduced = self.system.as_ref().is_some_and(motion::System::reduced);
		self.time = ctx.input(|input| input.time);
		self.clock = (self.time % WRAP) as f32;
		self.layers.clear();
		self.frames += 1;
		self.frame = Arc::new(Frame {
			id: self.frames,
			layers: OnceLock::new(),
		});
	}

	/// Hand the frame's layers to its callbacks.
	pub fn finish(&mut self) {
		let _ = self.frame.layers.set(std::mem::take(&mut self.layers));
	}

	/// Draw `layers` at `at` in `painter`'s layer, clipped to it.
	fn draw(&mut self, painter: &egui::Painter, at: egui::layers::ShapeIdx, layers: impl IntoIterator<Item = FxLayer>) {
		let (clock, reduced) = (self.clock, self.reduced);
		let start = self.layers.len();
		self.layers
			.extend(layers.into_iter().map(|layer| layer.at(clock).reduced(reduced)));
		if self.layers.len() == start {
			return;
		}
		let callback = Draw {
			frame: self.frame.clone(),
			layers: start..self.layers.len(),
		};
		painter.set(
			at,
			egui_wgpu::Callback::new_paint_callback(painter.clip_rect(), callback),
		);
	}

	/// A panel's background: a faint grain over its fill, which never moves.
	pub fn panel(&mut self, spot: &Spot) {
		let base = kit::P1;
		let params = GrainGradientParams {
			kind: GradientKind::Linear,
			colors: Stops::new(&[linear(base), linear(mix(base, Color32::WHITE, 0.03))]),
			angle: 1.2,
			amount: 0.25,
			size: 1.0,
			mono: true,
			fps: 0.0,
			..GrainGradientParams::ember()
		};
		self.draw(&spot.painter, spot.under, [Effect::fill(params).on(spot.shape(0.0))]);
	}

	/// A key's cap: dark plastic, dished.
	pub fn keycap(&mut self, spot: &Spot) {
		let params = PlasticParams {
			color: linear(kit::P3),
			bevel: 3.0,
			..PlasticParams::dished()
		};
		self.draw(&spot.painter, spot.under, [Effect::fill(params).on(spot.shape(4.0))]);
	}

	/// The text color over a [`button`](Self::button).
	pub const INK: Color32 = Color32::from_gray(24);

	/// A button of polished metal that ripples while the pointer is on it.
	pub fn button(&mut self, spot: &Spot) {
		let hovered = spot
			.painter
			.ctx()
			.pointer_hover_pos()
			.is_some_and(|pos| spot.rect.contains(pos));
		let ripple = &mut self.look.ripple;
		ripple.set(self.time, hovered);
		let params = LiquidMetalParams {
			flow: 0.6 * ripple.at(self.time),
			// Flat, so one reflection lies under the label rather than a
			// horizon crossing it.
			dome: 0.0,
			bevel: 3.0,
			..LiquidMetalParams::chrome()
		};
		self.draw(&spot.painter, spot.under, [Effect::fill(params).on(spot.shape(4.0))]);
	}

	/// What the effects follow this frame: whether the show is on air, how
	/// many takes there have been, and the worst health. Called every frame,
	/// whatever is drawn, so a picture or alert shown again later picks up
	/// where it would have been rather than bursting late.
	pub fn follow(&mut self, on_air: bool, takes: u64, verdict: Verdict) {
		let (time, look) = (self.time, &mut self.look);
		look.on_air.set(time, on_air);
		look.take(time, takes, f64::from(sparkles().burst_life().max(TAKE)));
		look.alerts.set(time, verdict);
	}

	/// The live picture: a glow while on air, and a burst on each take.
	pub fn program(&mut self, spot: &Spot) {
		let (time, clock, look) = (self.time, self.clock, &self.look);
		let sparkles = sparkles();
		// Kept off the controls above the monitor.
		let mut clip = spot.painter.clip_rect();
		clip.min.y = clip.min.y.max(spot.rect.top() - 3.0);
		let painter = spot.painter.with_clip_rect(clip);
		let shape = spot.shape(2.0);
		let glow = Effect::glow(GlowParams {
			style: GlowStyle::Neon,
			color: linear(kit::ACCENT),
			core: rgb(0xdce8ff),
			radius: 8.0,
			occlusion: 0.0,
			breathe: 0.25,
			heat: look.on_air.at(time),
			..GlowParams::default()
		});
		let center = spot.rect.center();
		let bursts: Vec<FxLayer> = look
			.bursts
			.iter()
			.flat_map(|&(start, seed)| {
				// On the layers' clock, which only keeps a burst's age precise.
				let start = clock - (time - start) as f32;
				let ripple = Effect::pulse(PulseParams {
					style: PulseStyle::Ripple {
						at: [center.x, center.y],
					},
					color: rgb(0x9cc8ff),
					core: rgb(0xffffff),
					reach: 0.0,
					width: 2.4,
					t0: start,
					duration: TAKE,
					intensity: 0.8,
					..PulseParams::default()
				});
				let sparkles = Effect::sparkles(SparkleParams {
					burst: Some(start),
					..sparkles
				});
				[ripple.on(shape).over(), sparkles.on(shape).over()].map(|layer| layer.seed(seed))
			})
			.collect();
		self.draw(&painter, spot.under, [glow.on(shape)]);
		self.draw(&painter, spot.over, bursts);
	}

	/// The dot that says the studio records, which pulses.
	pub fn recording(&mut self, spot: &Spot) {
		let center = spot.rect.center();
		let pulse = Effect::pulse(PulseParams {
			style: PulseStyle::Radar,
			color: linear(kit::ACCENT),
			core: rgb(0xd0e0ff),
			count: 2,
			period: 1.6,
			reach: 10.0,
			width: 1.2,
			occlusion: 0.0,
			..PulseParams::default()
		});
		let shape = Shape::circle(center.x, center.y, spot.rect.width() / 2.0);
		self.draw(&spot.painter, spot.under, [pulse.on(shape)]);
	}

	/// The health of the stream, by how bad the worst problem is: a warning
	/// pulse while degraded, and electric flicker while unhealthy.
	pub fn health(&mut self, spot: &Spot) {
		let layers = self.look.alerts.layers(self.time, spot.shape(4.0));
		self.draw(&spot.painter, spot.over, layers);
	}
}

/// The sparkles a take bursts into.
fn sparkles() -> SparkleParams {
	SparkleParams {
		count: 12,
		size: 9.0,
		..SparkleParams::default()
	}
}

/// How long a take's ripple runs, in seconds.
const TAKE: f32 = 0.7;

/// How many takes burst at once.
const BURSTS: usize = 4;

/// When the layers' clock starts over, in seconds: often enough that an f32
/// keeps it to a quarter millisecond, and rarely enough that the jump in a
/// breathing glow's phase goes unseen.
const WRAP: f64 = 3600.0;

/// The effects' state across frames.
struct Look {
	on_air: Light,
	/// The metal [`button`](Fx::button)'s ripple.
	ripple: Light,
	/// The take count last seen, and when each burst it fired started, with
	/// its seed.
	takes: Option<u64>,
	bursts: Vec<(f64, u32)>,
	alerts: Alerts,
}

impl Look {
	/// Burst for each take since the count last seen, and end the bursts
	/// older than `life`. The first count seen is where counting starts.
	fn take(&mut self, time: f64, takes: u64, life: f64) {
		self.bursts.retain(|(start, _)| time - start < life);
		if self.takes.replace(takes).is_some_and(|seen| seen != takes) {
			// Overlapping takes each burst, the oldest giving way.
			if self.bursts.len() == BURSTS {
				self.bursts.remove(0);
			}
			self.bursts.push((time, takes as u32));
		}
	}
}

impl Default for Look {
	fn default() -> Self {
		Self {
			on_air: Light::new(0.4, 1.2),
			ripple: Light::new(0.25, 0.8),
			takes: None,
			bursts: Vec::new(),
			alerts: Alerts::default(),
		}
	}
}

/// The Health alerts, each fading in and out with its verdict.
struct Alerts {
	degraded: Light,
	unhealthy: Light,
}

impl Default for Alerts {
	fn default() -> Self {
		Self {
			degraded: Light::new(0.3, 0.8),
			unhealthy: Light::new(0.15, 0.8),
		}
	}
}

impl Alerts {
	fn set(&mut self, t: f64, verdict: Verdict) {
		self.degraded.set(t, verdict == Verdict::Degraded);
		self.unhealthy.set(t, verdict == Verdict::Unhealthy);
	}

	fn layers(&self, t: f64, shape: Shape) -> Vec<FxLayer> {
		let mut layers = Vec::new();
		let degraded = self.degraded.at(t);
		if degraded > 0.0 {
			let pulse = Effect::pulse(PulseParams {
				style: PulseStyle::Breathe,
				color: linear(kit::CUE),
				core: rgb(0xfff1c4),
				period: 1.8,
				reach: 8.0,
				occlusion: 0.0,
				intensity: degraded,
				..PulseParams::default()
			});
			layers.push(pulse.on(shape).over());
		}
		let unhealthy = self.unhealthy.at(t);
		if unhealthy > 0.0 {
			let electric = Effect::electric(ElectricParams {
				style: ElectricStyle::Border,
				// Red, as an error is: unhealthy means something failed.
				color: linear(kit::ERR),
				core: rgb(0xffdce0),
				cool: rgb(0x9b1c2a),
				amplitude: 2.5,
				glow: 3.0,
				strands: 1,
				gap: 0.0,
				forks: 0.25,
				fork_len: 8.0,
				occlusion: 0.0,
				intensity: unhealthy,
				..ElectricParams::default()
			});
			layers.push(electric.on(shape).over());
		}
		layers
	}
}

/// A light that warms up over `attack` seconds when switched on and cools
/// over `cool` when off, rather than cutting, as rune-fx's `Heat` does on the
/// window's f64 clock.
struct Light {
	attack: f32,
	cool: f32,
	/// When it last switched, how lit it was then, and whether it is on.
	switched: f64,
	from: f32,
	on: bool,
}

impl Light {
	fn new(attack: f32, cool: f32) -> Self {
		Self {
			attack,
			cool,
			switched: 0.0,
			from: 0.0,
			on: false,
		}
	}

	fn set(&mut self, t: f64, on: bool) {
		if on != self.on {
			self.from = self.at(t);
			self.switched = t;
			self.on = on;
		}
	}

	/// How lit it is at `t`, from 0 to 1; 0 once it has cooled out of sight.
	fn at(&self, t: f64) -> f32 {
		let since = (t - self.switched).max(0.0) as f32;
		if self.on {
			let warmed = (since / self.attack).min(1.0);
			self.from + (1.0 - self.from) * (1.0 - (1.0 - warmed).powi(3))
		} else {
			let lit = self.from * (-since / self.cool).exp();
			if lit > 1.0 / 512.0 { lit } else { 0.0 }
		}
	}
}

/// Where an effect will go under what a panel draws next, before it is
/// drawn.
pub struct Mark {
	painter: egui::Painter,
	under: egui::layers::ShapeIdx,
}

impl Mark {
	pub fn new(painter: &egui::Painter) -> Self {
		Self {
			painter: painter.clone(),
			under: painter.add(egui::Shape::Noop),
		}
	}

	/// The spot around `rect`, once what goes over the effect is drawn.
	pub fn around(self, rect: Rect) -> Spot {
		let over = self.painter.add(egui::Shape::Noop);
		Spot {
			painter: self.painter,
			under: self.under,
			over,
			rect,
		}
	}
}

/// Where an effect goes: under and over what was drawn around `rect`.
#[derive(Clone)]
pub struct Spot {
	painter: egui::Painter,
	under: egui::layers::ShapeIdx,
	over: egui::layers::ShapeIdx,
	pub rect: Rect,
}

impl Spot {
	/// The same spot, around `rect` shrunk by `by` on each side.
	pub fn inset(self, by: f32) -> Self {
		Self {
			rect: self.rect.shrink(by),
			..self
		}
	}

	fn shape(&self, radius: f32) -> Shape {
		let rect = self.rect;
		Shape::rounded(rect.min.x, rect.min.y, rect.width(), rect.height(), radius)
	}
}

/// What egui's callbacks share: the effects renderer, and the frame it last
/// uploaded.
struct Renderer {
	fx: FxRenderer,
	prepared: Option<u64>,
}

/// Some of a frame's layers, drawn where egui painted the callback.
struct Draw {
	frame: Arc<Frame>,
	layers: Range<usize>,
}

impl egui_wgpu::CallbackTrait for Draw {
	fn prepare(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		screen: &egui_wgpu::ScreenDescriptor,
		_encoder: &mut wgpu::CommandEncoder,
		resources: &mut egui_wgpu::CallbackResources,
	) -> Vec<wgpu::CommandBuffer> {
		let Some(renderer) = resources.get_mut::<Renderer>() else {
			return Vec::new();
		};
		if renderer.prepared == Some(self.frame.id) {
			return Vec::new();
		}
		let Some(layers) = self.frame.layers.get() else {
			return Vec::new();
		};
		renderer.fx.prepare(
			device,
			queue,
			&FxFrame {
				viewport: screen.size_in_pixels,
				scale: screen.pixels_per_point,
				layers,
				backdrop: None,
				sources: &[],
			},
		);
		renderer.prepared = Some(self.frame.id);
		Vec::new()
	}

	fn paint(
		&self,
		info: egui::PaintCallbackInfo,
		pass: &mut wgpu::RenderPass<'static>,
		resources: &egui_wgpu::CallbackResources,
	) {
		let Some(renderer) = resources.get::<Renderer>() else {
			return;
		};
		if renderer.prepared != Some(self.frame.id) {
			return;
		}
		// Effects are placed in window pixels; egui's scissor still clips
		// them to the panel.
		let [width, height] = info.screen_size_px;
		pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
		for layer in self.layers.clone() {
			renderer.fx.draw_layer(pass, layer);
		}
	}
}

/// An sRGB hex color, in the linear light effects take.
fn rgb(hex: u32) -> Rgb {
	rune_fx::rgb(hex)
}

/// An egui color, in the linear light effects take.
fn linear(color: Color32) -> Rgb {
	let [r, g, b, _] = color.to_array();
	rgb(u32::from_be_bytes([0, r, g, b]))
}

/// `from` moved `by` of the way toward `to`.
fn mix(from: Color32, to: Color32, by: f32) -> Color32 {
	from.lerp_to_gamma(to, by)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A light switched off while still warming cools from where it got to,
	/// rather than jumping, and is out once out of sight.
	#[test]
	fn a_light_cools_from_where_it_got() {
		let mut light = Light::new(1.0, 1.0);
		light.set(10.0, true);
		let warm = light.at(10.5);
		assert!(warm > 0.0 && warm < 1.0, "{warm}");
		light.set(10.5, false);
		assert_eq!(light.at(10.5), warm);
		assert!(light.at(11.0) < warm);
		assert_eq!(light.at(20.0), 0.0);
	}

	/// Opening the studio on a count already past zero bursts nothing; each
	/// take after does, overlapping up to a limit, until it ends.
	#[test]
	fn each_take_bursts_once() {
		let mut look = Look::default();
		look.take(0.0, 7, 1.0);
		assert!(look.bursts.is_empty());
		look.take(0.1, 7, 1.0);
		assert!(look.bursts.is_empty());
		for take in 8..8 + BURSTS as u64 + 1 {
			look.take(0.2, take, 1.0);
		}
		assert_eq!(look.bursts.len(), BURSTS);
		assert_eq!(look.bursts[0].1, 9, "the oldest gave way");
		look.take(1.3, 8 + BURSTS as u64, 1.0);
		assert!(look.bursts.is_empty());
	}
}
