//! Background removal: a person segmentation model finds who is in each of a
//! source's pictures, and the rest of the picture turns transparent.
//!
//! The compositor shrinks each picture on the GPU to the model's size and
//! reads that back. A thread of the matte's own runs the model on the CPU,
//! with tract, and the compositor cuts the held full-size picture out with
//! the result. Each picture is drawn with its own matte, a frame or more
//! late, never with an older one; pictures that arrive while the model works
//! are skipped for the newest.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, mpsc};

use moq_video::render::wgpu;
use tract_onnx::prelude::*;

use crate::Error;
use crate::compositor::FORMAT;

/// MediaPipe's selfie segmentation, landscape variant: see `model/README.md`.
const MODEL: &[u8] = include_bytes!("model/selfie_segmentation_landscape.onnx");

/// The model's input and output size: RGB in, a person's alpha out.
const WIDTH: u32 = 256;
const HEIGHT: u32 = 144;

type Model = Arc<TypedRunnableModel>;

/// The model, parsed and optimized once per process, on the first matte's
/// thread.
fn model() -> Result<&'static Model, String> {
	static MODEL_PLAN: OnceLock<Result<Model, String>> = OnceLock::new();
	let load = || -> TractResult<Model> {
		let model = tract_onnx::onnx().model_for_read(&mut &MODEL[..])?.into_typed()?;
		let batch = model.symbols.sym("batch_size");
		model
			.set_symbols(&HashMap::from([(batch, TDim::from(1))]))?
			.into_optimized()?
			.into_runnable()
	};
	MODEL_PLAN
		.get_or_init(|| load().map_err(|err| format!("the segmentation model: {err}")))
		.as_ref()
		.map_err(Clone::clone)
}

/// The alpha of each pixel of a `WIDTH` by `HEIGHT` RGBA picture: 255 where
/// it shows a person, 0 where it shows what is behind them.
fn segment(model: &Model, rgba: &[u8]) -> Result<Vec<u8>, String> {
	let plane = (WIDTH * HEIGHT) as usize;
	let mut input = vec![0.0f32; 3 * plane];
	for (at, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
		for (channel, value) in pixel[..3].iter().enumerate() {
			input[channel * plane + at] = f32::from(*value) / 255.0;
		}
	}
	let run = || -> TractResult<Vec<u8>> {
		let input = Tensor::from_shape(&[1, 3, HEIGHT as usize, WIDTH as usize], &input)?;
		let output = model.run(tvec!(input.into()))?;
		let alpha = output[0].to_plain_array_view::<f32>()?;
		Ok(alpha
			.iter()
			.map(|alpha| (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
			.collect())
	};
	run().map_err(|err| format!("the segmentation model: {err}"))
}

/// Cuts a person out of one source's pictures. Dropping it stops its thread
/// once the picture in hand is done.
pub(crate) struct Matte {
	device: wgpu::Device,
	queue: wgpu::Queue,
	copy: wgpu::RenderPipeline,
	shrink: wgpu::RenderPipeline,
	cut: wgpu::RenderPipeline,
	picture_layout: wgpu::BindGroupLayout,
	sampler: wgpu::Sampler,
	/// The picture at the model's size, read back into `readback`.
	small: wgpu::Texture,
	readback: wgpu::Buffer,
	mask: wgpu::Texture,
	mask_bind: wgpu::BindGroup,
	/// The newest picture started, and its cut-out, at its size.
	held: Option<Held>,
	jobs: mpsc::Sender<wgpu::SubmissionIndex>,
	results: mpsc::Receiver<Result<Vec<u8>, String>>,
	/// Whether the thread has a picture, which also has `readback` mapped.
	busy: bool,
	/// Whether the picture the thread has was overtaken, so its cut-out is
	/// thrown away.
	stale: bool,
	/// The thread's result, once a test has waited for it.
	#[cfg(test)]
	ready: Option<Result<Vec<u8>, String>>,
}

struct Held {
	picture: wgpu::Texture,
	bind: wgpu::BindGroup,
	cut: wgpu::Texture,
}

impl Matte {
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, Error> {
		let shader = device.create_shader_module(wgpu::include_wgsl!("matte.wgsl"));
		let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
			binding,
			visibility: wgpu::ShaderStages::FRAGMENT,
			ty: wgpu::BindingType::Texture {
				sample_type: wgpu::TextureSampleType::Float { filterable: true },
				view_dimension: wgpu::TextureViewDimension::D2,
				multisampled: false,
			},
			count: None,
		};
		let picture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene matte picture"),
			entries: &[
				texture_entry(0),
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
			],
		});
		let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene matte mask"),
			entries: &[texture_entry(0)],
		});
		let pipeline = |entry: &str, layouts: &[Option<&wgpu::BindGroupLayout>]| {
			let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
				label: Some("moq-scene matte"),
				bind_group_layouts: layouts,
				immediate_size: 0,
			});
			device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
				label: Some("moq-scene matte"),
				layout: Some(&layout),
				vertex: wgpu::VertexState {
					module: &shader,
					entry_point: Some("vs_main"),
					buffers: &[],
					compilation_options: Default::default(),
				},
				fragment: Some(wgpu::FragmentState {
					module: &shader,
					entry_point: Some(entry),
					targets: &[Some(FORMAT.into())],
					compilation_options: Default::default(),
				}),
				primitive: Default::default(),
				depth_stencil: None,
				multisample: Default::default(),
				multiview_mask: None,
				cache: None,
			})
		};
		let copy = pipeline("fs_copy", &[Some(&picture_layout)]);
		let shrink = pipeline("fs_shrink", &[Some(&picture_layout)]);
		let cut = pipeline("fs_cut", &[Some(&picture_layout), Some(&mask_layout)]);
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("moq-scene matte"),
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});

		let size = wgpu::Extent3d {
			width: WIDTH,
			height: HEIGHT,
			depth_or_array_layers: 1,
		};
		let small = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene matte input"),
			size,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: FORMAT,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
			view_formats: &[],
		});
		// A row of the model's input is 1024 bytes, already aligned for a copy.
		let readback = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("moq-scene matte input"),
			size: u64::from(WIDTH * HEIGHT * 4),
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mask = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene matte"),
			size,
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::R8Unorm,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let mask_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene matte mask"),
			layout: &mask_layout,
			entries: &[wgpu::BindGroupEntry {
				binding: 0,
				resource: wgpu::BindingResource::TextureView(&mask.create_view(&Default::default())),
			}],
		});

		let (jobs, inbox) = mpsc::channel();
		let (outbox, results) = mpsc::channel();
		let thread = Thread {
			device: device.clone(),
			readback: readback.clone(),
		};
		std::thread::Builder::new()
			.name("moq-scene matte".into())
			.spawn(move || thread.run(inbox, outbox))
			.map_err(|err| Error::Unsupported(format!("cannot start background removal: {err}")))?;

		Ok(Self {
			device: device.clone(),
			queue: queue.clone(),
			copy,
			shrink,
			cut,
			picture_layout,
			sampler,
			small,
			readback,
			mask,
			mask_bind,
			held: None,
			jobs,
			results,
			busy: false,
			stale: false,
			#[cfg(test)]
			ready: None,
		})
	}

	/// Whether a picture is still being cut out, so another cannot start.
	pub fn busy(&self) -> bool {
		self.busy
	}

	/// Start cutting `picture` out: hold a copy, since its texture may be
	/// drawn over, and hand the thread a shrunk one.
	pub fn start(&mut self, picture: &wgpu::Texture) {
		assert!(!self.busy, "a matte cuts one picture at a time");
		let size = picture.size();
		let held = match self.held.take() {
			Some(held) if held.picture.size() == size => held,
			_ => self.hold(size),
		};

		let bind = self.bind(picture);
		let mut encoder = self.device.create_command_encoder(&Default::default());
		self.pass(&mut encoder, &self.copy, &held.picture, &[&bind]);
		self.pass(&mut encoder, &self.shrink, &self.small, &[&held.bind]);
		encoder.copy_texture_to_buffer(
			self.small.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &self.readback,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(WIDTH * 4),
					rows_per_image: None,
				},
			},
			self.small.size(),
		);
		let submission = self.queue.submit([encoder.finish()]);
		self.held = Some(held);
		// A thread that is gone reports so through `finish`.
		let _ = self.jobs.send(submission);
		self.busy = true;
	}

	/// Throw away the cut-out of the picture in hand, such as when the source
	/// clears its picture, so it never shows after what overtook it.
	pub fn discard(&mut self) {
		self.stale = self.busy;
	}

	/// The cut-out of the picture last started, once the thread has its
	/// matte; `None` while it works on it, when nothing is started, or when it
	/// was discarded.
	pub fn finish(&mut self) -> Option<Result<&wgpu::Texture, Error>> {
		if !self.busy {
			return None;
		}
		#[cfg(test)]
		if let Some(result) = self.ready.take() {
			return self.cut(result);
		}
		let result = match self.results.try_recv() {
			Ok(result) => result,
			Err(mpsc::TryRecvError::Empty) => return None,
			Err(mpsc::TryRecvError::Disconnected) => Err("the background removal thread stopped".into()),
		};
		self.cut(result)
	}

	/// Block until the thread is done with the picture last started, for
	/// the next [`finish`](Self::finish) to cut out.
	#[cfg(test)]
	pub fn wait(&mut self) {
		assert!(self.busy, "nothing is started");
		let result = self
			.results
			.recv()
			.unwrap_or_else(|_| Err("the background removal thread stopped".into()));
		self.ready = Some(result);
	}

	fn cut(&mut self, alpha: Result<Vec<u8>, String>) -> Option<Result<&wgpu::Texture, Error>> {
		self.busy = false;
		if std::mem::take(&mut self.stale) {
			return None;
		}
		Some(self.paint(alpha))
	}

	fn paint(&mut self, alpha: Result<Vec<u8>, String>) -> Result<&wgpu::Texture, Error> {
		let alpha = alpha.map_err(Error::Matte)?;
		self.queue.write_texture(
			self.mask.as_image_copy(),
			&alpha,
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(WIDTH),
				rows_per_image: None,
			},
			self.mask.size(),
		);
		let held = self.held.as_ref().expect("a started picture is held");
		let mut encoder = self.device.create_command_encoder(&Default::default());
		self.pass(&mut encoder, &self.cut, &held.cut, &[&held.bind, &self.mask_bind]);
		self.queue.submit([encoder.finish()]);
		Ok(&held.cut)
	}

	/// The textures a picture of `size` is held and cut out in.
	fn hold(&self, size: wgpu::Extent3d) -> Held {
		let texture = |label| {
			self.device.create_texture(&wgpu::TextureDescriptor {
				label: Some(label),
				size,
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: FORMAT,
				usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
				view_formats: &[],
			})
		};
		let picture = texture("moq-scene matte held");
		Held {
			bind: self.bind(&picture),
			picture,
			cut: texture("moq-scene matte cut"),
		}
	}

	fn bind(&self, texture: &wgpu::Texture) -> wgpu::BindGroup {
		self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("moq-scene matte picture"),
			layout: &self.picture_layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::Sampler(&self.sampler),
				},
			],
		})
	}

	/// Draw `pipeline` over the whole of `target`.
	fn pass(
		&self,
		encoder: &mut wgpu::CommandEncoder,
		pipeline: &wgpu::RenderPipeline,
		target: &wgpu::Texture,
		binds: &[&wgpu::BindGroup],
	) {
		let view = target.create_view(&Default::default());
		let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
			label: Some("moq-scene matte"),
			color_attachments: &[Some(wgpu::RenderPassColorAttachment {
				view: &view,
				depth_slice: None,
				resolve_target: None,
				ops: wgpu::Operations {
					load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
					store: wgpu::StoreOp::Store,
				},
			})],
			..Default::default()
		});
		pass.set_pipeline(pipeline);
		for (index, bind) in binds.iter().enumerate() {
			pass.set_bind_group(index as u32, *bind, &[]);
		}
		pass.draw(0..3, 0..1);
	}
}

/// What a matte's thread holds: the device to wait on, and the buffer the
/// shrunk picture lands in.
struct Thread {
	device: wgpu::Device,
	readback: wgpu::Buffer,
}

impl Thread {
	/// Run the model on each shrunk picture, once the GPU has it, until the
	/// matte is dropped.
	fn run(self, jobs: mpsc::Receiver<wgpu::SubmissionIndex>, results: mpsc::Sender<Result<Vec<u8>, String>>) {
		for submission in jobs {
			let alpha = self.read(submission).and_then(|rgba| segment(model()?, &rgba));
			if results.send(alpha).is_err() {
				return;
			}
		}
	}

	/// The shrunk picture, once `submission` has copied it into the buffer.
	fn read(&self, submission: wgpu::SubmissionIndex) -> Result<Vec<u8>, String> {
		let (mapped, map) = mpsc::channel();
		self.readback.map_async(wgpu::MapMode::Read, .., move |result| {
			let _ = mapped.send(result);
		});
		self.device
			.poll(wgpu::PollType::Wait {
				submission_index: Some(submission),
				timeout: None,
			})
			.map_err(|err| format!("waiting for the GPU: {err}"))?;
		map.recv()
			.map_err(|_| "the GPU dropped the background removal buffer".to_string())?
			.map_err(|err| format!("reading the picture back: {err}"))?;
		let pixels = self
			.readback
			.slice(..)
			.get_mapped_range()
			.map_err(|err| format!("reading the picture back: {err}"))?
			.to_vec();
		self.readback.unmap();
		Ok(pixels)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The model finds the person in a portrait and nothing beside them.
	/// `person.jpg` is Ilan Ramon's NASA portrait (public domain), cropped to
	/// head and shoulders and shrunk to the model's size.
	#[test]
	fn finds_the_person() {
		let image = image::load_from_memory(include_bytes!("test_data/person.jpg"))
			.unwrap()
			.to_rgba8();
		assert_eq!(image.dimensions(), (WIDTH, HEIGHT));
		let alpha = segment(model().unwrap(), image.as_raw()).unwrap();
		let at = |x: u32, y: u32| alpha[(y * WIDTH + x) as usize];
		// The face, and the suit below it.
		assert!(at(128, 50) > 240, "{}", at(128, 50));
		assert!(at(128, 130) > 240, "{}", at(128, 130));
		// The flag on the left and the shuttle model on the right.
		for (x, y) in [(10, 10), (30, 100), (245, 10), (225, 70)] {
			assert!(at(x, y) < 15, "({x}, {y}): {}", at(x, y));
		}
	}

	/// A plain picture shows nobody.
	#[test]
	fn finds_nobody_in_a_wall() {
		let wall = [180u8, 170, 160, 255].repeat((WIDTH * HEIGHT) as usize);
		let alpha = segment(model().unwrap(), &wall).unwrap();
		assert!(alpha.iter().all(|alpha| *alpha < 15));
	}
}
