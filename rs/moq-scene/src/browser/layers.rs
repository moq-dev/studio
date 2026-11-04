//! What a page shows: its view and, while one is open, the popup CEF paints
//! apart from it, such as a `<select>`'s list, drawn over it.

use super::protocol::{Area, Layer};
use crate::wgpu;
use crate::wgpu::util::DeviceExt;

/// The formats a page arrives in: CEF's shared textures, and the uploads.
const FORMATS: [wgpu::TextureFormat; 2] = [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Bgra8Unorm];

/// Each fragment of the popup's area reads the popup's pixel `offset` from
/// it, whatever order the popup's channels are in.
const SHADER: &str = "
struct Place { offset: vec2<i32> }
@group(0) @binding(0) var popup: texture_2d<f32>;
@group(0) @binding(1) var<uniform> place: Place;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
	let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
	return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
	return textureLoad(popup, vec2<i32>(position.xy) + place.offset, 0);
}
";

/// The studio's device, which pages' pictures are copied to, and the pass that
/// draws a popup over its page there.
pub(super) struct Gpu {
	pub device: wgpu::Device,
	pub queue: wgpu::Queue,
	layout: wgpu::BindGroupLayout,
	/// One per [`FORMATS`], in order.
	pipelines: Vec<wgpu::RenderPipeline>,
}

impl Gpu {
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene popup"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: false },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene popup"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("moq-scene popup"),
			source: wgpu::ShaderSource::Wgsl(SHADER.into()),
		});
		let pipelines = FORMATS
			.iter()
			.map(|&format| {
				device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
					label: Some("moq-scene popup"),
					layout: Some(&pipeline_layout),
					vertex: wgpu::VertexState {
						module: &shader,
						entry_point: Some("vs_main"),
						compilation_options: Default::default(),
						buffers: &[],
					},
					primitive: Default::default(),
					depth_stencil: None,
					multisample: Default::default(),
					fragment: Some(wgpu::FragmentState {
						module: &shader,
						entry_point: Some("fs_main"),
						compilation_options: Default::default(),
						// Both arrive premultiplied, so a popup's translucent
						// or not yet painted pixels show the page under them.
						targets: &[Some(wgpu::ColorTargetState {
							format,
							blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
							write_mask: wgpu::ColorWrites::ALL,
						})],
					}),
					multiview_mask: None,
					cache: None,
				})
			})
			.collect();
		Self {
			device: device.clone(),
			queue: queue.clone(),
			layout,
			pipelines,
		}
	}

	/// A copy of `view` with `popup` drawn over it at `area`, or `view` itself
	/// when none of the popup lands on it.
	fn over(&self, view: &wgpu::Texture, popup: &wgpu::Texture, area: Area) -> Result<wgpu::Texture, String> {
		let Some((from, to, size)) = clip(area, popup.size(), view.size()) else {
			return Ok(view.clone());
		};
		let pipeline = FORMATS
			.iter()
			.position(|format| *format == view.format())
			.map(|at| &self.pipelines[at])
			.ok_or_else(|| format!("cannot draw a popup over a {:?} page", view.format()))?;
		let texture = self.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene page with popup"),
			size: view.size(),
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: view.format(),
			// A copy source like every other picture of a page.
			usage: wgpu::TextureUsages::TEXTURE_BINDING
				| wgpu::TextureUsages::COPY_SRC
				| wgpu::TextureUsages::COPY_DST
				| wgpu::TextureUsages::RENDER_ATTACHMENT,
			view_formats: &[],
		});
		let offset = [from.x as i32 - to.x as i32, from.y as i32 - to.y as i32];
		let place = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label: Some("moq-scene popup"),
			contents: bytemuck::cast_slice(&offset),
			usage: wgpu::BufferUsages::UNIFORM,
		});
		let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene popup"),
			layout: &self.layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&popup.create_view(&Default::default())),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: place.as_entire_binding(),
				},
			],
		});

		let mut encoder = self.device.create_command_encoder(&Default::default());
		encoder.copy_texture_to_texture(view.as_image_copy(), texture.as_image_copy(), view.size());
		{
			let target = texture.create_view(&Default::default());
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("moq-scene popup"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view: &target,
					depth_slice: None,
					resolve_target: None,
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Load,
						store: wgpu::StoreOp::Store,
					},
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			pass.set_pipeline(pipeline);
			pass.set_bind_group(0, &bind, &[]);
			// One triangle over the viewport, which is the part that lands.
			pass.set_viewport(
				to.x as f32,
				to.y as f32,
				size.width as f32,
				size.height as f32,
				0.0,
				1.0,
			);
			pass.draw(0..3, 0..1);
		}
		self.queue.submit([encoder.finish()]);
		Ok(texture)
	}
}

/// A page's newest pictures.
#[derive(Default)]
pub(super) struct Layers {
	view: Option<wgpu::Texture>,
	/// The open popup's area of the view, and its picture once painted.
	popup: Option<(Area, Option<wgpu::Texture>)>,
}

impl Layers {
	/// Take `texture` as the newest picture of `layer`. A popup painted while
	/// none is open is dropped.
	pub fn paint(&mut self, layer: Layer, texture: wgpu::Texture) {
		match layer {
			Layer::View => self.view = Some(texture),
			Layer::Popup => {
				if let Some((_, picture)) = &mut self.popup {
					*picture = Some(texture);
				}
			}
		}
	}

	/// The popup opened or moved to `area`, or closed with `None`. One that
	/// moved keeps its picture until it paints again.
	pub fn place(&mut self, area: Option<Area>) {
		let picture = self.popup.take().and_then(|(_, picture)| picture);
		self.popup = area.map(|area| (area, picture));
	}

	/// What the page shows, once its view has painted: the view itself, or a
	/// copy of it with the popup over it, so the view stays clean for when the
	/// popup changes or closes.
	pub fn picture(&self, gpu: &Gpu) -> Result<Option<wgpu::Texture>, String> {
		let Some(view) = &self.view else { return Ok(None) };
		match &self.popup {
			Some((area, Some(popup))) => gpu.over(view, popup, *area).map(Some),
			_ => Ok(Some(view.clone())),
		}
	}
}

/// Where a `popup`-sized picture placed at `area` lands on a `view`-sized
/// one: the corner read from, the corner drawn to, and how much, or `None` if
/// none of it does. The area comes from the page, so it may reach past the
/// view, and the picture may not match it.
fn clip(
	area: Area,
	popup: wgpu::Extent3d,
	view: wgpu::Extent3d,
) -> Option<(wgpu::Origin3d, wgpu::Origin3d, wgpu::Extent3d)> {
	let (x, y) = (i64::from(area.x), i64::from(area.y));
	let (left, top) = (x.max(0), y.max(0));
	let right = (x + i64::from(area.width.min(popup.width))).min(view.width.into());
	let bottom = (y + i64::from(area.height.min(popup.height))).min(view.height.into());
	if left >= right || top >= bottom {
		return None;
	}
	let origin = |x: i64, y: i64| wgpu::Origin3d {
		x: x as u32,
		y: y as u32,
		z: 0,
	};
	let size = wgpu::Extent3d {
		width: (right - left) as u32,
		height: (bottom - top) as u32,
		depth_or_array_layers: 1,
	};
	Some((origin(left - x, top - y), origin(left, top), size))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn extent(width: u32, height: u32) -> wgpu::Extent3d {
		wgpu::Extent3d {
			width,
			height,
			depth_or_array_layers: 1,
		}
	}

	fn area(x: i32, y: i32, width: u32, height: u32) -> Area {
		Area { x, y, width, height }
	}

	/// A copy as `((from x, from y), (to x, to y), (width, height))`.
	type Copied = ((u32, u32), (u32, u32), (u32, u32));

	fn clipped(area: Area, popup: (u32, u32), view: (u32, u32)) -> Option<Copied> {
		clip(area, extent(popup.0, popup.1), extent(view.0, view.1))
			.map(|(from, to, size)| ((from.x, from.y), (to.x, to.y), (size.width, size.height)))
	}

	/// A popup reaching past any edge of the view, or a picture that does not
	/// match its area, draws only what lands on the view, from the matching
	/// part of the picture, and one entirely off the view draws nothing.
	#[test]
	fn a_popup_draws_only_what_lands_on_the_view() {
		let view = (320, 240);
		assert_eq!(
			clipped(area(10, 20, 100, 50), (100, 50), view),
			Some(((0, 0), (10, 20), (100, 50)))
		);
		assert_eq!(
			clipped(area(300, 200, 100, 100), (100, 100), view),
			Some(((0, 0), (300, 200), (20, 40)))
		);
		assert_eq!(
			clipped(area(-30, -40, 100, 100), (100, 100), view),
			Some(((30, 40), (0, 0), (70, 60)))
		);
		assert_eq!(
			clipped(area(i32::MIN, i32::MIN, u32::MAX, u32::MAX), (5000, 5000), view),
			None,
			"far off the view, however large"
		);
		assert_eq!(
			clipped(area(-10, 0, u32::MAX, u32::MAX), (4096, 4096), view),
			Some(((10, 0), (0, 0), (320, 240)))
		);
		assert_eq!(
			clipped(area(10, 10, 100, 100), (40, 300), view),
			Some(((0, 0), (10, 10), (40, 100))),
			"the smaller of the picture and its area"
		);
		assert_eq!(clipped(area(320, 0, 10, 10), (10, 10), view), None);
		assert_eq!(clipped(area(-10, 0, 10, 10), (10, 10), view), None);
		assert_eq!(clipped(area(0, 0, 0, 10), (10, 10), view), None);
	}

	const RED: [u8; 4] = [255, 0, 0, 255];
	const BLUE: [u8; 4] = [0, 0, 255, 255];

	/// A 4x4 texture of `format` holding `pixel(x, y)`, in RGBA.
	fn texture(gpu: &Gpu, format: wgpu::TextureFormat, pixel: impl Fn(u32, u32) -> [u8; 4]) -> wgpu::Texture {
		let size = extent(4, 4);
		let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
			label: None,
			size,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		let data: Vec<u8> = (0..4)
			.flat_map(|y| (0..4).map(move |x| (x, y)))
			.flat_map(|(x, y)| {
				let [r, g, b, a] = pixel(x, y);
				match format {
					wgpu::TextureFormat::Bgra8Unorm => [b, g, r, a],
					_ => [r, g, b, a],
				}
			})
			.collect();
		gpu.queue.write_texture(
			texture.as_image_copy(),
			&data,
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(16),
				rows_per_image: None,
			},
			size,
		);
		texture
	}

	/// A 4x4 RGBA texture's pixels, row by row.
	fn read(gpu: &Gpu, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
		let row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
		let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
			label: None,
			size: u64::from(row * 4),
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mut encoder = gpu.device.create_command_encoder(&Default::default());
		encoder.copy_texture_to_buffer(
			texture.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &buffer,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(row),
					rows_per_image: None,
				},
			},
			texture.size(),
		);
		gpu.queue.submit([encoder.finish()]);
		buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
		gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		let mapped = buffer.slice(..).get_mapped_range().unwrap();
		mapped
			.chunks_exact(row as usize)
			.flat_map(|row| row[..16].as_chunks::<4>().0.to_vec())
			.collect()
	}

	/// A popup hanging off the view's left edge draws its visible part where
	/// the page placed it, premultiplied over the page: a transparent pixel
	/// shows the page, a half-transparent one mixes with it. Its channel order
	/// need not match the page's, and the page's own picture is untouched.
	#[tokio::test]
	#[ignore = "needs a Vulkan device; run by just render-sw"]
	async fn a_popup_blends_over_the_page_where_placed() {
		let adapter = wgpu::Instance::default()
			.request_adapter(&wgpu::RequestAdapterOptions::default())
			.await
			.expect("a GPU adapter");
		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor::default())
			.await
			.expect("a GPU device");
		let gpu = Gpu::new(&device, &queue);

		let view = texture(&gpu, wgpu::TextureFormat::Rgba8Unorm, |_, _| RED);
		// Column 2 transparent; column 3 half-transparent green on top, blue
		// below.
		let popup = texture(&gpu, wgpu::TextureFormat::Bgra8Unorm, |x, y| match (x, y) {
			(0..=2, _) => [0; 4],
			(_, 0) => [0, 128, 0, 128],
			_ => BLUE,
		});
		let mut layers = Layers::default();
		layers.paint(Layer::View, view.clone());
		layers.place(Some(area(-2, 1, 4, 4)));
		layers.paint(Layer::Popup, popup);

		let shown = layers.picture(&gpu).unwrap().unwrap();
		let pixels = read(&gpu, &shown);
		let at = |x: usize, y: usize| pixels[y * 4 + x];
		assert_eq!(at(0, 1), RED, "transparent shows the page");
		let mixed = at(1, 1);
		assert!(
			mixed[0].abs_diff(127) <= 1 && mixed[1].abs_diff(128) <= 1 && mixed[2] == 0 && mixed[3] == 255,
			"half green over red: {mixed:?}"
		);
		assert_eq!(at(1, 2), BLUE);
		assert_eq!(at(1, 3), BLUE);
		for (x, y) in [(0, 0), (1, 0), (2, 1), (3, 3), (0, 3)] {
			assert_eq!(
				at(x, y),
				RED,
				"({x}, {y}) is off the popup, or under its transparent column"
			);
		}
		assert!(
			read(&gpu, &view).iter().all(|pixel| *pixel == RED),
			"the page stays clean"
		);

		layers.place(None);
		assert_eq!(
			layers.picture(&gpu).unwrap().unwrap(),
			view,
			"a closed popup shows the page"
		);
	}
}
