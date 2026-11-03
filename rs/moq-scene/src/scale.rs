//! Scale a texture down on the GPU, for the smaller rungs of the program.

use moq_video::render::wgpu;

use crate::compositor::FORMAT;

const SHADER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

struct Varyings {
	@builtin(position) position: vec4<f32>,
	@location(0) uv: vec2<f32>,
}

// One triangle covering the whole target.
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
	let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
	var out: Varyings;
	out.position = vec4<f32>(corner * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
	out.uv = corner;
	return out;
}

// Four bilinear taps a quarter of a target pixel from its center, each
// averaging four source texels: a box over the pixel's footprint, so a third
// of the size still averages every texel rather than skipping some.
@fragment fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
	let offset = fwidth(in.uv) * 0.25;
	let sum = textureSample(image, image_sampler, in.uv + vec2<f32>(-offset.x, -offset.y))
		+ textureSample(image, image_sampler, in.uv + vec2<f32>(offset.x, -offset.y))
		+ textureSample(image, image_sampler, in.uv + vec2<f32>(-offset.x, offset.y))
		+ textureSample(image, image_sampler, in.uv + vec2<f32>(offset.x, offset.y));
	return sum * 0.25;
}
"#;

/// Draws one texture into a smaller one, as [`moq_video::render::export`]
/// takes it.
///
/// Bound to its source texture, such as the compositor's program, which keeps
/// one texture and redraws it each frame: [`scale`](Self::scale) draws whatever
/// it holds now.
pub struct Scaler {
	queue: wgpu::Queue,
	device: wgpu::Device,
	pipeline: wgpu::RenderPipeline,
	bind: wgpu::BindGroup,
	output: wgpu::Texture,
	view: wgpu::TextureView,
}

impl Scaler {
	/// Scale `source`, a [`FORMAT`] texture created with `TEXTURE_BINDING`, to
	/// `size`.
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, source: &wgpu::Texture, size: moq_video::Size) -> Self {
		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene scale"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene scale"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("moq-scene scale"),
			source: wgpu::ShaderSource::Wgsl(SHADER.into()),
		});
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("moq-scene scale"),
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
				targets: &[Some(wgpu::ColorTargetState {
					format: FORMAT,
					blend: None,
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			multiview_mask: None,
			cache: None,
		});
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("moq-scene scale"),
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});
		let source = source.create_view(&Default::default());
		let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene scale"),
			layout: &layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&source),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::Sampler(&sampler),
				},
			],
		});
		// What the exporter copies from: a texture it can sample and copy.
		let output = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene scaled"),
			size: wgpu::Extent3d {
				width: size.width,
				height: size.height,
				depth_or_array_layers: 1,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: FORMAT,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT
				| wgpu::TextureUsages::TEXTURE_BINDING
				| wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		let view = output.create_view(&Default::default());
		Self {
			queue: queue.clone(),
			device: device.clone(),
			pipeline,
			bind,
			output,
			view,
		}
	}

	/// Draw the source as it is now, returning the texture it is drawn into.
	/// Submitted before this returns, so work queued after it reads the result.
	pub fn scale(&self) -> &wgpu::Texture {
		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("moq-scene scale"),
		});
		{
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("moq-scene scale"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view: &self.view,
					depth_slice: None,
					resolve_target: None,
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
						store: wgpu::StoreOp::Store,
					},
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			pass.set_pipeline(&self.pipeline);
			pass.set_bind_group(0, &self.bind, &[]);
			pass.draw(0..3, 0..1);
		}
		self.queue.submit([encoder.finish()]);
		&self.output
	}
}
