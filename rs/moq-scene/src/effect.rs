//! The GPU passes that draw an item through its crop, filters, and blend mode.
//!
//! Per-pixel filters (color, keys, LUTs, masks) run in the shader that draws
//! the item, so an item without a blur or shadow costs one draw however many
//! it has. A blur or drop shadow needs the pixels around each one, so the
//! picture up to it is drawn into a raster first. A raster covers only what
//! can still reach the canvas, and never more than twice the canvas each way,
//! so its memory is bounded by the canvas rather than by the item or filter. A
//! wide blur halves its raster until it is a few texels across, so its cost is
//! bounded too.

use std::num::NonZeroU64;

use moq_video::render::wgpu;

use crate::collection::{Blend, Canvas, Crop, Filter, MaskMode, Rect, Rgba};
use crate::compositor::FORMAT;
use crate::lut::Cube;

const SHADER: &str = include_str!("effect.wgsl");

/// Per-pixel filters one draw runs; a longer run is split across rasters.
const OPS: usize = 8;

/// What rasters between passes hold: premultiplied, and fine enough that a
/// blur's faint tail does not band.
const RASTER: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The widest blur, in texels, drawn without first halving its raster.
const SIGMA: f32 = 4.0;

/// Texture coordinates covering a whole texture.
const UNIT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Clip space covering a whole target, top-left then bottom-right.
const FULL: [f32; 4] = [-1.0, 1.0, 1.0, -1.0];

/// One per-pixel filter, as `effect.wgsl`'s `Op`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Op {
	kind: [u32; 4],
	params: [f32; 4],
	/// Column-major, as WGSL's `mat4x4`.
	matrix: [[f32; 4]; 4],
	offset: [f32; 4],
}

impl Op {
	const MATRIX: u32 = 0;
	const CHROMA_KEY: u32 = 1;
	const LUMA_KEY: u32 = 2;
	const LUT: u32 = 3;
	const MASK: u32 = 4;

	/// `rows` times straight RGBA, plus `offset`, clamped.
	fn matrix(rows: [[f32; 4]; 4], offset: [f32; 4]) -> Self {
		let mut matrix = [[0.0; 4]; 4];
		for (row, values) in rows.iter().enumerate() {
			for (column, value) in values.iter().enumerate() {
				matrix[column][row] = *value;
			}
		}
		Self {
			kind: [Self::MATRIX, 0, 0, 0],
			matrix,
			offset,
			..Default::default()
		}
	}

	/// A 3x3 color matrix, leaving alpha alone.
	fn color(rgb: [[f32; 3]; 3], offset: f32) -> Self {
		let row = |r: [f32; 3]| [r[0], r[1], r[2], 0.0];
		Self::matrix(
			[row(rgb[0]), row(rgb[1]), row(rgb[2]), [0.0, 0.0, 0.0, 1.0]],
			[offset, offset, offset, 0.0],
		)
	}

	/// Each color channel times `slope`, plus `intercept`.
	fn linear(slope: f32, intercept: f32) -> Self {
		Self::color([[slope, 0.0, 0.0], [0.0, slope, 0.0], [0.0, 0.0, slope]], intercept)
	}

	/// The per-pixel filters that read nothing but the pixel, by the CSS Filter
	/// Effects spec's equivalents; `None` for every other filter.
	fn of(filter: &Filter) -> Option<Self> {
		Some(match *filter {
			Filter::Opacity { amount } => {
				let mut rows = IDENTITY;
				rows[3][3] = amount;
				Self::matrix(rows, [0.0; 4])
			}
			Filter::Brightness { amount } => Self::linear(amount, 0.0),
			Filter::Contrast { amount } => Self::linear(amount, 0.5 - 0.5 * amount),
			Filter::Invert { amount } => Self::linear(1.0 - 2.0 * amount, amount),
			Filter::Saturate { amount: s } => Self::color(
				[
					[0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
					[0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
					[0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
				],
				0.0,
			),
			Filter::Grayscale { amount } => {
				let a = 1.0 - amount;
				Self::color(
					[
						[0.2126 + 0.7874 * a, 0.7152 - 0.7152 * a, 0.0722 - 0.0722 * a],
						[0.2126 - 0.2126 * a, 0.7152 + 0.2848 * a, 0.0722 - 0.0722 * a],
						[0.2126 - 0.2126 * a, 0.7152 - 0.7152 * a, 0.0722 + 0.9278 * a],
					],
					0.0,
				)
			}
			Filter::Sepia { amount } => {
				let a = 1.0 - amount;
				Self::color(
					[
						[0.393 + 0.607 * a, 0.769 - 0.769 * a, 0.189 - 0.189 * a],
						[0.349 - 0.349 * a, 0.686 + 0.314 * a, 0.168 - 0.168 * a],
						[0.272 - 0.272 * a, 0.534 - 0.534 * a, 0.131 + 0.869 * a],
					],
					0.0,
				)
			}
			Filter::HueRotate { degrees } => {
				let (sin, cos) = degrees.to_radians().sin_cos();
				Self::color(
					[
						[
							0.213 + cos * 0.787 - sin * 0.213,
							0.715 - cos * 0.715 - sin * 0.715,
							0.072 - cos * 0.072 + sin * 0.928,
						],
						[
							0.213 - cos * 0.213 + sin * 0.143,
							0.715 + cos * 0.285 + sin * 0.140,
							0.072 - cos * 0.072 - sin * 0.283,
						],
						[
							0.213 - cos * 0.213 - sin * 0.787,
							0.715 - cos * 0.715 + sin * 0.715,
							0.072 + cos * 0.928 + sin * 0.072,
						],
					],
					0.0,
				)
			}
			Filter::ChromaKey {
				color,
				similarity,
				smoothness,
				spill,
			} => {
				let [r, g, b, _] = color.0.map(|channel| f32::from(channel) / 255.0);
				let cb = -0.1146 * r - 0.3854 * g + 0.5 * b;
				let cr = 0.5 * r - 0.4542 * g - 0.0458 * b;
				Self {
					kind: [Self::CHROMA_KEY, 0, 0, 0],
					params: [cb, cr, similarity, smoothness],
					offset: [spill, 0.0, 0.0, 0.0],
					..Default::default()
				}
			}
			Filter::LumaKey { min, max, smoothness } => Self {
				kind: [Self::LUMA_KEY, 0, 0, 0],
				params: [min, max, smoothness, 0.0],
				..Default::default()
			},
			Filter::Blur { .. } | Filter::DropShadow { .. } | Filter::Lut { .. } | Filter::Mask { .. } => return None,
		})
	}
}

const IDENTITY: [[f32; 4]; 4] = [
	[1.0, 0.0, 0.0, 0.0],
	[0.0, 1.0, 0.0, 0.0],
	[0.0, 0.0, 1.0, 0.0],
	[0.0, 0.0, 0.0, 1.0],
];

/// One draw's uniform, as `effect.wgsl`'s `Draw`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
	rect: [f32; 4],
	input: [f32; 4],
	valid: [f32; 4],
	item: [f32; 4],
	aux: [f32; 4],
	mode: [u32; 4],
	blur: [f32; 4],
	color: [f32; 4],
	ops: [Op; OPS],
}

/// A region of the canvas, in canvas pixels: left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
	x0: f32,
	y0: f32,
	x1: f32,
	y1: f32,
}

impl Area {
	fn of(rect: Rect) -> Self {
		Self {
			x0: rect.x,
			y0: rect.y,
			x1: rect.x + rect.width,
			y1: rect.y + rect.height,
		}
	}

	fn canvas(canvas: Canvas) -> Self {
		Self::of(Rect::fill(&canvas))
	}

	fn width(&self) -> f32 {
		self.x1 - self.x0
	}

	fn height(&self) -> f32 {
		self.y1 - self.y0
	}

	fn is_empty(&self) -> bool {
		!(self.x1 > self.x0 && self.y1 > self.y0)
	}

	fn grow(&self, x: f32, y: f32) -> Self {
		Self {
			x0: self.x0 - x,
			y0: self.y0 - y,
			x1: self.x1 + x,
			y1: self.y1 + y,
		}
	}

	fn shift(&self, x: f32, y: f32) -> Self {
		Self {
			x0: self.x0 + x,
			y0: self.y0 + y,
			x1: self.x1 + x,
			y1: self.y1 + y,
		}
	}

	fn intersect(&self, other: &Self) -> Self {
		Self {
			x0: self.x0.max(other.x0),
			y0: self.y0.max(other.y0),
			x1: self.x1.min(other.x1),
			y1: self.y1.min(other.y1),
		}
	}

	fn union(&self, other: &Self) -> Self {
		Self {
			x0: self.x0.min(other.x0),
			y0: self.y0.min(other.y0),
			x1: self.x1.max(other.x1),
			y1: self.y1.max(other.y1),
		}
	}

	/// The whole pixels covering this area, or `None` if it covers none.
	fn pixels(&self) -> Option<Self> {
		let area = Self {
			x0: self.x0.floor(),
			y0: self.y0.floor(),
			x1: self.x1.ceil(),
			y1: self.y1.ceil(),
		};
		(!self.is_empty()).then_some(area)
	}

	/// What coordinates `uv`, given at this area's corners, are at `other`'s.
	fn map(&self, uv: [f32; 4], other: &Self) -> [f32; 4] {
		let u = |x: f32| uv[0] + (x - self.x0) / self.width() * (uv[2] - uv[0]);
		let v = |y: f32| uv[1] + (y - self.y0) / self.height() * (uv[3] - uv[1]);
		[u(other.x0), v(other.y0), u(other.x1), v(other.y1)]
	}
}

/// A filter's place in the passes: per pixel, or reading around each pixel.
#[derive(Clone, Copy)]
enum Stage<'a> {
	Pixel(&'a Filter),
	Blur(f32),
	Shadow { x: f32, y: f32, blur: f32, color: Rgba },
}

impl<'a> Stage<'a> {
	fn of(filter: &'a Filter) -> Self {
		match *filter {
			Filter::Blur { radius } => Self::Blur(radius),
			Filter::DropShadow { x, y, blur, color } => Self::Shadow { x, y, blur, color },
			_ => Self::Pixel(filter),
		}
	}

	/// How far, in canvas pixels, an output pixel reads from.
	fn reach(&self) -> (f32, f32) {
		match *self {
			Self::Pixel(_) => (0.0, 0.0),
			Self::Blur(sigma) => (reach(sigma), reach(sigma)),
			Self::Shadow { x, y, blur, .. } => (x.abs() + reach(blur), y.abs() + reach(blur)),
		}
	}
}

/// Where a Gaussian of `sigma` is negligible: three standard deviations.
fn reach(sigma: f32) -> f32 {
	(3.0 * sigma).ceil()
}

/// A source's newest picture, bound to sample.
#[derive(Clone)]
pub(crate) struct Picture {
	view: wgpu::TextureView,
	bind: wgpu::BindGroup,
	/// Whether its alpha is premultiplied, as a web page's is.
	premultiplied: bool,
}

/// A LUT source's table on the GPU.
#[derive(Clone)]
pub(crate) struct Lut {
	view: wgpu::TextureView,
	size: u32,
	min: [f32; 3],
	max: [f32; 3],
}

/// What a draw samples: a picture covering `area` of the canvas, at texture
/// coordinates `uv` at its corners, and transparent outside `valid`.
#[derive(Clone)]
struct Input {
	picture: Picture,
	area: Area,
	uv: [f32; 4],
	valid: [f32; 4],
	premultiplied: bool,
}

/// Per-pixel filters waiting to run when an input is sampled.
#[derive(Clone, Default)]
struct Segment {
	ops: Vec<Op>,
	lut: Option<wgpu::TextureView>,
	mask: Option<wgpu::TextureView>,
}

/// An item ready to draw onto a canvas-sized target: its last input, the
/// filters still to run, and where on the canvas it can show.
#[derive(Clone)]
pub(crate) struct Prepared {
	input: Input,
	segment: Segment,
	item: Area,
	bounds: Area,
	blend: Blend,
}

/// Which pipeline a draw uses.
#[derive(Clone, Copy)]
enum Kind {
	/// Per-pixel filters, over a canvas.
	Canvas,
	/// Per-pixel filters, into a raster.
	Raster,
	/// Per-pixel filters, mixed with a backdrop by a blend mode.
	Blend,
	/// One direction of a blur, or a halving.
	Blur,
	/// A picture over its shadow.
	Shadow,
}

struct Draw {
	kind: Kind,
	input: wgpu::BindGroup,
	extra: wgpu::BindGroup,
	uniform: Uniform,
}

enum Command {
	/// Draws into `view`, first cleared to `clear` unless `None`.
	Pass {
		view: wgpu::TextureView,
		clear: Option<wgpu::Color>,
		draws: Vec<Draw>,
	},
	/// Copy `area` of a target into the backdrop, for a blend mode to read.
	Backdrop { texture: wgpu::Texture, area: Area },
}

/// One render's passes: the rasters items are filtered through, then the
/// targets, in the order they are drawn.
#[derive(Default)]
pub(crate) struct Frame {
	rasters: Vec<Command>,
	targets: Vec<Command>,
}

impl Frame {
	/// Start drawing a canvas-sized target, cleared to opaque black.
	pub fn target(&mut self, target: &Picture) {
		self.targets.push(Command::Pass {
			view: target.view.clone(),
			clear: Some(wgpu::Color::BLACK),
			draws: Vec::new(),
		});
	}

	fn draw(&mut self, draw: Draw) {
		match self.targets.last_mut() {
			Some(Command::Pass { draws, .. }) => draws.push(draw),
			_ => unreachable!("a draw follows its target"),
		}
	}
}

/// A raster texture, reused across renders while one of its size is needed.
#[derive(Clone)]
struct Raster {
	size: (u32, u32),
	picture: Picture,
}

/// What an item draws, as [`Effects::prepare`] takes it.
pub(crate) struct Item<'a> {
	/// Its source's newest picture.
	pub picture: &'a Picture,
	/// Where on the canvas.
	pub rect: Rect,
	/// Its source's crop, then its own.
	pub crop: Crop,
	/// Its source's filters, then its own.
	pub filters: Vec<&'a Filter>,
	/// How it mixes with what is under it.
	pub blend: Blend,
}

/// The pictures and tables filters read, by source name.
pub(crate) trait Library {
	/// The newest picture of the source called `name`.
	fn picture(&self, name: &str) -> Option<Picture>;
	/// The table of the LUT source called `name`.
	fn lut(&self, name: &str) -> Option<Lut>;
}

/// The GPU objects every effect draws with.
pub(crate) struct Effects {
	device: wgpu::Device,
	queue: wgpu::Queue,
	canvas: Canvas,

	input_layout: wgpu::BindGroupLayout,
	extra_layout: wgpu::BindGroupLayout,
	uniform_layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	canvas_pipeline: wgpu::RenderPipeline,
	raster_pipeline: wgpu::RenderPipeline,
	blend_pipeline: wgpu::RenderPipeline,
	blur_pipeline: wgpu::RenderPipeline,
	shadow_pipeline: wgpu::RenderPipeline,

	uniforms: Uniforms,
	/// Group 2 for a draw that reads no LUT, mask, or aux texture.
	none: wgpu::BindGroup,
	empty_2d: wgpu::TextureView,
	empty_3d: wgpu::TextureView,
	/// What a blend mode mixes with: a copy of its target under it.
	backdrop: wgpu::Texture,
	backdrop_view: wgpu::TextureView,

	/// Rasters used by the last render, reused by this one.
	free: Vec<Raster>,
	/// Rasters used by this render.
	used: Vec<Raster>,
}

/// What a blend mode mixes with, as large as the canvas.
fn backdrop(device: &wgpu::Device, canvas: Canvas) -> wgpu::Texture {
	device.create_texture(&wgpu::TextureDescriptor {
		label: Some("moq-scene backdrop"),
		size: wgpu::Extent3d {
			width: canvas.width,
			height: canvas.height,
			depth_or_array_layers: 1,
		},
		mip_level_count: 1,
		sample_count: 1,
		dimension: wgpu::TextureDimension::D2,
		format: FORMAT,
		usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
		view_formats: &[],
	})
}

impl Effects {
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, canvas: Canvas) -> Self {
		let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
			binding,
			visibility: wgpu::ShaderStages::FRAGMENT,
			ty: wgpu::BindingType::Texture {
				sample_type,
				view_dimension,
				multisampled: false,
			},
			count: None,
		};
		let float = wgpu::TextureSampleType::Float { filterable: true };
		let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene input"),
			entries: &[
				texture(0, float, wgpu::TextureViewDimension::D2),
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
			],
		});
		let extra_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene extra"),
			entries: &[
				// Rgba32Float, read with textureLoad: not every device filters it.
				texture(
					0,
					wgpu::TextureSampleType::Float { filterable: false },
					wgpu::TextureViewDimension::D3,
				),
				texture(1, float, wgpu::TextureViewDimension::D2),
				texture(2, float, wgpu::TextureViewDimension::D2),
			],
		});
		let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene draw"),
			entries: &[wgpu::BindGroupLayoutEntry {
				binding: 0,
				visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
				ty: wgpu::BindingType::Buffer {
					ty: wgpu::BufferBindingType::Uniform,
					has_dynamic_offset: true,
					min_binding_size: NonZeroU64::new(size_of::<Uniform>() as u64),
				},
				count: None,
			}],
		});
		let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene"),
			bind_group_layouts: &[Some(&uniform_layout), Some(&input_layout), Some(&extra_layout)],
			immediate_size: 0,
		});
		let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("moq-scene"),
			source: wgpu::ShaderSource::Wgsl(SHADER.into()),
		});
		let pipeline = |entry: &str, format, blend| {
			device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
				label: Some(entry),
				layout: Some(&layout),
				vertex: wgpu::VertexState {
					module: &shader,
					entry_point: Some("vs_main"),
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
					module: &shader,
					entry_point: Some(entry),
					compilation_options: Default::default(),
					targets: &[Some(wgpu::ColorTargetState {
						format,
						blend,
						write_mask: wgpu::ColorWrites::ALL,
					})],
				}),
				multiview_mask: None,
				cache: None,
			})
		};
		// Premultiplied over an opaque canvas, which stays opaque.
		let over = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("moq-scene"),
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});

		let empty = |dimension, format| {
			device
				.create_texture(&wgpu::TextureDescriptor {
					label: Some("moq-scene empty"),
					size: wgpu::Extent3d {
						width: 1,
						height: 1,
						depth_or_array_layers: 1,
					},
					mip_level_count: 1,
					sample_count: 1,
					dimension,
					format,
					usage: wgpu::TextureUsages::TEXTURE_BINDING,
					view_formats: &[],
				})
				.create_view(&Default::default())
		};
		let empty_2d = empty(wgpu::TextureDimension::D2, FORMAT);
		let empty_3d = empty(wgpu::TextureDimension::D3, wgpu::TextureFormat::Rgba32Float);
		let backdrop = backdrop(device, canvas);

		let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
		let stride = (size_of::<Uniform>() as u64).next_multiple_of(alignment);
		Self {
			canvas_pipeline: pipeline("fs_effect", FORMAT, over),
			raster_pipeline: pipeline("fs_effect", RASTER, None),
			blend_pipeline: pipeline("fs_blend", FORMAT, None),
			blur_pipeline: pipeline("fs_blur", RASTER, None),
			shadow_pipeline: pipeline("fs_shadow", RASTER, None),
			uniforms: Uniforms::new(device, &uniform_layout, stride, 16),
			none: extra_bind(device, &extra_layout, [&empty_3d, &empty_2d, &empty_2d]),
			device: device.clone(),
			queue: queue.clone(),
			canvas,
			input_layout,
			extra_layout,
			uniform_layout,
			sampler,
			empty_2d,
			empty_3d,
			backdrop_view: backdrop.create_view(&Default::default()),
			backdrop,
			free: Vec::new(),
			used: Vec::new(),
		}
	}

	/// Draw onto `canvas` from the next frame on, reallocating what is sized
	/// from it.
	pub fn set_canvas(&mut self, canvas: Canvas) {
		if canvas.width != self.canvas.width || canvas.height != self.canvas.height {
			self.backdrop = backdrop(&self.device, canvas);
			self.backdrop_view = self.backdrop.create_view(&Default::default());
		}
		self.canvas = canvas;
	}

	/// `texture`, a source's picture or a canvas-sized target, bound to sample.
	pub fn picture(&self, texture: &wgpu::Texture, premultiplied: bool) -> Picture {
		let view = texture.create_view(&Default::default());
		let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene input"),
			layout: &self.input_layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&view),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::Sampler(&self.sampler),
				},
			],
		});
		Picture {
			view,
			bind,
			premultiplied,
		}
	}

	/// Upload a LUT's table.
	pub fn lut(&self, cube: &Cube) -> Lut {
		let extent = wgpu::Extent3d {
			width: cube.size,
			height: cube.size,
			depth_or_array_layers: cube.size,
		};
		let texture = self.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene LUT"),
			size: extent,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D3,
			format: wgpu::TextureFormat::Rgba32Float,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let texels: Vec<[f32; 4]> = cube.table.iter().map(|&[r, g, b]| [r, g, b, 1.0]).collect();
		self.queue.write_texture(
			texture.as_image_copy(),
			bytemuck::cast_slice(&texels),
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(cube.size * 16),
				rows_per_image: Some(cube.size),
			},
			extent,
		);
		Lut {
			view: texture.create_view(&Default::default()),
			size: cube.size,
			min: cube.min,
			max: cube.max,
		}
	}

	/// Start a render's passes.
	pub fn frame(&mut self) -> Frame {
		// Rasters the last render did not use are dropped.
		self.free = std::mem::take(&mut self.used);
		Frame::default()
	}

	/// Run `frame`'s passes.
	pub fn submit(&mut self, frame: Frame) {
		let commands: Vec<Command> = frame.rasters.into_iter().chain(frame.targets).collect();
		let draws = commands.iter().flat_map(|command| match command {
			Command::Pass { draws, .. } => draws.as_slice(),
			Command::Backdrop { .. } => &[],
		});
		let count = draws.clone().count();
		self.uniforms.reserve(&self.device, &self.uniform_layout, count as u64);
		let stride = self.uniforms.stride as usize;
		let mut bytes = vec![0u8; count * stride];
		for (index, draw) in draws.enumerate() {
			let offset = index * stride;
			bytes[offset..offset + size_of::<Uniform>()].copy_from_slice(bytemuck::bytes_of(&draw.uniform));
		}
		if !bytes.is_empty() {
			self.queue.write_buffer(&self.uniforms.buffer, 0, &bytes);
		}

		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("moq-scene"),
		});
		let mut index = 0u32;
		for command in &commands {
			let (view, clear, draws) = match command {
				Command::Pass { view, clear, draws } => (view, clear, draws),
				Command::Backdrop { texture, area } => {
					let origin = wgpu::Origin3d {
						x: area.x0 as u32,
						y: area.y0 as u32,
						z: 0,
					};
					encoder.copy_texture_to_texture(
						wgpu::TexelCopyTextureInfo {
							origin,
							..texture.as_image_copy()
						},
						wgpu::TexelCopyTextureInfo {
							origin,
							..self.backdrop.as_image_copy()
						},
						wgpu::Extent3d {
							width: area.width() as u32,
							height: area.height() as u32,
							depth_or_array_layers: 1,
						},
					);
					continue;
				}
			};
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("moq-scene"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view,
					depth_slice: None,
					resolve_target: None,
					ops: wgpu::Operations {
						load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
						store: wgpu::StoreOp::Store,
					},
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			for draw in draws {
				pass.set_pipeline(match draw.kind {
					Kind::Canvas => &self.canvas_pipeline,
					Kind::Raster => &self.raster_pipeline,
					Kind::Blend => &self.blend_pipeline,
					Kind::Blur => &self.blur_pipeline,
					Kind::Shadow => &self.shadow_pipeline,
				});
				pass.set_bind_group(0, &self.uniforms.bind, &[index * stride as u32]);
				pass.set_bind_group(1, &draw.input, &[]);
				pass.set_bind_group(2, &draw.extra, &[]);
				pass.draw(0..4, 0..1);
				index += 1;
			}
		}
		self.queue.submit([encoder.finish()]);
	}

	/// `item` through its crop and filters, ready to draw onto targets: `None`
	/// if nothing of it reaches the canvas, or a source a filter reads has
	/// nothing in `library` yet.
	pub fn prepare(&mut self, frame: &mut Frame, item: Item<'_>, library: &dyn Library) -> Option<Prepared> {
		let area = Area::of(item.rect);
		let canvas = Area::canvas(self.canvas);
		if area.is_empty() {
			return None;
		}
		let crop = [
			item.crop.left,
			item.crop.top,
			1.0 - item.crop.right,
			1.0 - item.crop.bottom,
		];
		let mut input = Input {
			picture: item.picture.clone(),
			area,
			uv: crop,
			valid: crop,
			premultiplied: item.picture.premultiplied,
		};

		let stages: Vec<Stage> = item.filters.iter().map(|filter| Stage::of(filter)).collect();
		// How far past the canvas each stage's input must reach, for the stages
		// from it on.
		let mut margins = vec![(0.0, 0.0); stages.len() + 1];
		for (index, stage) in stages.iter().enumerate().rev() {
			let (x, y) = stage.reach();
			margins[index] = (margins[index + 1].0 + x, margins[index + 1].1 + y);
		}
		let clip = |index: usize| canvas.grow(margins[index].0, margins[index].1);

		let mut bounds = area;
		let mut segment = Segment::default();
		for (index, stage) in stages.iter().enumerate() {
			match *stage {
				Stage::Pixel(filter) => {
					let (op, lut, mask) = match filter {
						Filter::Lut { source, amount } => {
							let lut = library.lut(source)?;
							let op = Op {
								kind: [Op::LUT, 0, 0, 0],
								params: [*amount, lut.size as f32, 0.0, 0.0],
								matrix: [[lut.max[0], lut.max[1], lut.max[2], 0.0], [0.0; 4], [0.0; 4], [0.0; 4]],
								offset: [lut.min[0], lut.min[1], lut.min[2], 0.0],
							};
							(op, Some(lut.view), None)
						}
						Filter::Mask { source, mode } => {
							let mode = match mode {
								MaskMode::Alpha => 0.0,
								MaskMode::Luminance => 1.0,
							};
							let picture = library.picture(source)?;
							let op = Op {
								kind: [Op::MASK, 0, 0, 0],
								params: [mode, f32::from(u8::from(picture.premultiplied)), 0.0, 0.0],
								..Default::default()
							};
							(op, None, Some(picture.view))
						}
						filter => (Op::of(filter).expect("a per-pixel filter"), None, None),
					};
					let full = segment.ops.len() == OPS
						|| (lut.is_some() && segment.lut.is_some())
						|| (mask.is_some() && segment.mask.is_some());
					if full {
						let region = bounds.intersect(&clip(index)).pixels()?;
						input = self.raster(frame, &input, &segment, area, region);
						segment = Segment::default();
					}
					segment.ops.push(op);
					if mask.is_some() {
						// Nothing shows outside the mask, which covers the item.
						bounds = bounds.intersect(&area);
						segment.mask = mask;
					}
					segment.lut = lut.or(segment.lut);
				}
				Stage::Blur(sigma) => {
					if sigma == 0.0 {
						continue;
					}
					let spread = reach(sigma);
					let region = bounds.grow(spread, spread).intersect(&clip(index)).pixels()?;
					input = self.blur(frame, &input, &segment, area, region, sigma);
					segment = Segment::default();
					bounds = region;
				}
				Stage::Shadow { x, y, blur, color } => {
					let spread = reach(blur);
					let source = bounds.intersect(&clip(index)).pixels()?;
					let picture = self.raster(frame, &input, &segment, area, source);
					let shape = if blur == 0.0 {
						picture.clone()
					} else {
						let region = bounds.grow(spread, spread).intersect(&clip(index)).pixels()?;
						self.blur(frame, &picture, &Segment::default(), area, region, blur)
					};
					let shadow = bounds.shift(x, y).grow(spread, spread);
					let region = bounds.union(&shadow).intersect(&clip(index + 1)).pixels()?;
					input = self.shadow(frame, &picture, &shape, region, (x, y), color);
					segment = Segment::default();
					bounds = region;
				}
			}
		}

		let bounds = bounds.intersect(&canvas);
		if bounds.is_empty() {
			return None;
		}
		Some(Prepared {
			input,
			segment,
			item: area,
			bounds,
			blend: item.blend,
		})
	}

	/// A whole canvas-sized `picture` faded to `opacity`, to draw over another.
	pub fn fade(&self, picture: &Picture, opacity: f32) -> Prepared {
		let canvas = Area::canvas(self.canvas);
		Prepared {
			input: Input {
				picture: picture.clone(),
				area: canvas,
				uv: UNIT,
				valid: UNIT,
				premultiplied: false,
			},
			segment: Segment {
				ops: vec![Op::of(&Filter::Opacity { amount: opacity }).expect("per pixel")],
				..Default::default()
			},
			item: canvas,
			bounds: canvas,
			blend: Blend::Normal,
		}
	}

	/// Draw `prepared` onto the target `frame` last started, which is `texture`.
	pub fn draw(&self, frame: &mut Frame, texture: &wgpu::Texture, prepared: &Prepared) {
		let Prepared {
			input,
			segment,
			item,
			bounds,
			blend,
		} = prepared;
		let (width, height) = (self.canvas.width as f32, self.canvas.height as f32);
		let x = |px: f32| px / width * 2.0 - 1.0;
		let y = |px: f32| 1.0 - px / height * 2.0;
		let mut uniform = self.uniform(input, segment, item, bounds);
		uniform.rect = [x(bounds.x0), y(bounds.y0), x(bounds.x1), y(bounds.y1)];
		uniform.mode[2] = *blend as u32;

		if *blend == Blend::Normal {
			frame.draw(Draw {
				kind: Kind::Canvas,
				input: input.picture.bind.clone(),
				extra: self.extra(segment.lut.as_ref(), segment.mask.as_ref(), None),
				uniform,
			});
			return;
		}

		// A blend mode reads what is under it, which a pass cannot while drawing
		// it: copy that much aside, then carry on in a new pass.
		let area = bounds.pixels().expect("prepared bounds are not empty");
		let Some(Command::Pass { view, .. }) = frame.targets.last() else {
			unreachable!("a draw follows its target");
		};
		let view = view.clone();
		frame.targets.push(Command::Backdrop {
			texture: texture.clone(),
			area,
		});
		frame.targets.push(Command::Pass {
			view,
			clear: None,
			draws: Vec::new(),
		});
		frame.draw(Draw {
			kind: Kind::Blend,
			input: input.picture.bind.clone(),
			extra: self.extra(segment.lut.as_ref(), segment.mask.as_ref(), Some(&self.backdrop_view)),
			uniform,
		});
	}

	/// The uniform sampling `input` through `segment` over `quad` of the canvas.
	fn uniform(&self, input: &Input, segment: &Segment, item: &Area, quad: &Area) -> Uniform {
		let mut ops = [Op::default(); OPS];
		ops[..segment.ops.len()].copy_from_slice(&segment.ops);
		Uniform {
			rect: FULL,
			input: input.area.map(input.uv, quad),
			valid: input.valid,
			item: item.map(UNIT, quad),
			mode: [segment.ops.len() as u32, input.premultiplied.into(), 0, 0],
			ops,
			..Default::default()
		}
	}

	/// `input` through `segment`, drawn into a raster covering `area`.
	fn raster(&mut self, frame: &mut Frame, input: &Input, segment: &Segment, item: Area, area: Area) -> Input {
		let size = self.texels(&area);
		let raster = self.acquire(size);
		let draw = Draw {
			kind: Kind::Raster,
			input: input.picture.bind.clone(),
			extra: self.extra(segment.lut.as_ref(), segment.mask.as_ref(), None),
			uniform: self.uniform(input, segment, &item, &area),
		};
		frame.rasters.push(Command::Pass {
			view: raster.picture.view.clone(),
			clear: Some(wgpu::Color::TRANSPARENT),
			draws: vec![draw],
		});
		Input {
			picture: raster.picture,
			area,
			uv: UNIT,
			valid: UNIT,
			premultiplied: true,
		}
	}

	/// `input` through `segment`, blurred by `sigma` canvas pixels over `area`.
	fn blur(
		&mut self,
		frame: &mut Frame,
		input: &Input,
		segment: &Segment,
		item: Area,
		area: Area,
		sigma: f32,
	) -> Input {
		let mut current = self.raster(frame, input, segment, item, area);
		let mut size = self.texels(&area);
		let texels = |size: (u32, u32)| (size.0 as f32 / area.width(), size.1 as f32 / area.height());
		// Halve until the blur is a few texels wide; each texel of a halving
		// averages four, so nothing aliases.
		while sigma * texels(size).0.max(texels(size).1) > SIGMA && (size.0 > 1 || size.1 > 1) {
			size = (size.0.div_ceil(2), size.1.div_ceil(2));
			current = self.pass(frame, Kind::Blur, &current, size, [0.0, 0.0, 1.0, 0.0]);
		}
		let (x, y) = texels(size);
		current = self.pass(
			frame,
			Kind::Blur,
			&current,
			size,
			[1.0 / size.0 as f32, 0.0, sigma * x, reach(sigma * x)],
		);
		self.pass(
			frame,
			Kind::Blur,
			&current,
			size,
			[0.0, 1.0 / size.1 as f32, sigma * y, reach(sigma * y)],
		)
	}

	/// A whole raster `input` drawn into a new raster of `size` by `kind`.
	fn pass(&mut self, frame: &mut Frame, kind: Kind, input: &Input, size: (u32, u32), blur: [f32; 4]) -> Input {
		let raster = self.acquire(size);
		let uniform = Uniform {
			rect: FULL,
			input: UNIT,
			valid: UNIT,
			mode: [0, 1, 0, 0],
			blur,
			..Default::default()
		};
		frame.rasters.push(Command::Pass {
			view: raster.picture.view.clone(),
			clear: Some(wgpu::Color::TRANSPARENT),
			draws: vec![Draw {
				kind,
				input: input.picture.bind.clone(),
				extra: self.none.clone(),
				uniform,
			}],
		});
		Input {
			picture: raster.picture,
			..input.clone()
		}
	}

	/// `picture` over its `shape` moved by `offset` and filled with `color`,
	/// in a raster covering `area`.
	fn shadow(
		&mut self,
		frame: &mut Frame,
		picture: &Input,
		shape: &Input,
		area: Area,
		offset: (f32, f32),
		color: Rgba,
	) -> Input {
		let raster = self.acquire(self.texels(&area));
		let uniform = Uniform {
			rect: FULL,
			input: picture.area.map(UNIT, &area),
			valid: UNIT,
			aux: shape.area.map(UNIT, &area.shift(-offset.0, -offset.1)),
			mode: [0, 1, 0, 0],
			color: color.0.map(|channel| f32::from(channel) / 255.0),
			..Default::default()
		};
		frame.rasters.push(Command::Pass {
			view: raster.picture.view.clone(),
			clear: Some(wgpu::Color::TRANSPARENT),
			draws: vec![Draw {
				kind: Kind::Shadow,
				input: picture.picture.bind.clone(),
				extra: self.extra(None, None, Some(&shape.picture.view)),
				uniform,
			}],
		});
		Input {
			picture: raster.picture,
			area,
			uv: UNIT,
			valid: UNIT,
			premultiplied: true,
		}
	}

	/// The texels a raster covering `area` gets: one per canvas pixel, unless
	/// that is more than twice the canvas or the device allows either way.
	fn texels(&self, area: &Area) -> (u32, u32) {
		let limit = self.device.limits().max_texture_dimension_2d;
		let most = |canvas: u32| (canvas * 2).min(limit) as f32;
		let scale = 1f32
			.min(most(self.canvas.width) / area.width())
			.min(most(self.canvas.height) / area.height());
		let texels = |length: f32| ((length * scale).ceil() as u32).max(1);
		(texels(area.width()), texels(area.height()))
	}

	/// A raster of `size` for this render, reusing one the last render used.
	fn acquire(&mut self, size: (u32, u32)) -> Raster {
		let raster = match self.free.iter().position(|raster| raster.size == size) {
			Some(index) => self.free.swap_remove(index),
			None => {
				let texture = self.device.create_texture(&wgpu::TextureDescriptor {
					label: Some("moq-scene raster"),
					size: wgpu::Extent3d {
						width: size.0,
						height: size.1,
						depth_or_array_layers: 1,
					},
					mip_level_count: 1,
					sample_count: 1,
					dimension: wgpu::TextureDimension::D2,
					format: RASTER,
					usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
					view_formats: &[],
				});
				Raster {
					size,
					picture: self.picture(&texture, true),
				}
			}
		};
		self.used.push(raster.clone());
		raster
	}

	/// Group 2: the LUT, mask, and aux textures a draw reads.
	fn extra(
		&self,
		lut: Option<&wgpu::TextureView>,
		mask: Option<&wgpu::TextureView>,
		aux: Option<&wgpu::TextureView>,
	) -> wgpu::BindGroup {
		if lut.is_none() && mask.is_none() && aux.is_none() {
			return self.none.clone();
		}
		extra_bind(
			&self.device,
			&self.extra_layout,
			[
				lut.unwrap_or(&self.empty_3d),
				mask.unwrap_or(&self.empty_2d),
				aux.unwrap_or(&self.empty_2d),
			],
		)
	}
}

fn extra_bind(
	device: &wgpu::Device,
	layout: &wgpu::BindGroupLayout,
	views: [&wgpu::TextureView; 3],
) -> wgpu::BindGroup {
	let entries: Vec<wgpu::BindGroupEntry> = views
		.iter()
		.enumerate()
		.map(|(binding, view)| wgpu::BindGroupEntry {
			binding: binding as u32,
			resource: wgpu::BindingResource::TextureView(view),
		})
		.collect();
	device.create_bind_group(&wgpu::BindGroupDescriptor {
		label: Some("moq-scene extra"),
		layout,
		entries: &entries,
	})
}

/// A growable uniform buffer holding one [`Uniform`] per draw of a render.
struct Uniforms {
	buffer: wgpu::Buffer,
	bind: wgpu::BindGroup,
	/// Bytes between draws, rounded up to the device's dynamic-offset alignment.
	stride: u64,
	capacity: u64,
}

impl Uniforms {
	fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, stride: u64, capacity: u64) -> Self {
		let buffer = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene draws"),
			size: stride * capacity,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene draws"),
			layout,
			entries: &[wgpu::BindGroupEntry {
				binding: 0,
				resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
					buffer: &buffer,
					offset: 0,
					size: NonZeroU64::new(size_of::<Uniform>() as u64),
				}),
			}],
		});
		Self {
			buffer,
			bind,
			stride,
			capacity,
		}
	}

	/// Grow to hold `count` draws, doubling so a growing scene reallocates rarely.
	fn reserve(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, count: u64) {
		if count > self.capacity {
			*self = Self::new(device, layout, self.stride, count.next_power_of_two());
		}
	}
}
