//! The program as a system camera, for video-call and conferencing apps that
//! offer no ingest.
//!
//! On Linux the camera is a PipeWire video node (with the `pipewire` feature),
//! which PipeWire-aware apps such as browsers list as a camera. Without
//! PipeWire it is an existing v4l2loopback device, since that kernel module
//! needs root to load and so is never the first choice.
//!
//! The program clock offers every canvas frame; [`Camera::offer`] copies it
//! into a staging buffer only while an app reads the camera, and the `vcam`
//! thread waits for the copy and hands the pixels to the backend.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use moq_scene::{Canvas, wgpu};

#[cfg(all(target_os = "linux", feature = "pipewire"))]
mod pipewire;
#[cfg(target_os = "linux")]
mod v4l2;

/// Copies in flight at once: one being handed over, one being copied, and one
/// spare, as the encoder's export keeps.
const DEPTH: usize = 3;

/// A canvas's program, published as a camera other apps can open.
///
/// Dropping it removes the camera.
pub struct Camera {
	/// Where the camera lives, for the Output panel.
	backend: String,
	readback: Readback,
	/// `None` once dropping.
	frames: Option<mpsc::SyncSender<Staged>>,
	worker: Option<std::thread::JoinHandle<()>>,
	shared: Arc<Shared>,
}

/// What the camera's threads report to the program clock and the UI.
#[derive(Default)]
struct Shared {
	/// Whether an app reads the camera, so frames are worth copying.
	reading: AtomicBool,
	/// Frames handed to the backend.
	sent: AtomicU64,
	/// Frames not copied because every staging buffer was still in flight.
	dropped: AtomicU64,
	/// Set with `error`, so the program clock checks it without a lock.
	failed: AtomicBool,
	/// Why the camera stopped, if it did.
	error: Mutex<Option<String>>,
}

impl Shared {
	fn fail(&self, err: impl std::fmt::Display) {
		tracing::error!(%err, "virtual camera stopped");
		self.error.lock().unwrap().get_or_insert_with(|| err.to_string());
		self.failed.store(true, Ordering::Relaxed);
	}

	fn failed(&self) -> bool {
		self.failed.load(Ordering::Relaxed)
	}

	/// Whether a frame is worth copying: an app reads the camera, which has
	/// not stopped.
	fn wanted(&self) -> bool {
		self.reading.load(Ordering::Relaxed) && !self.failed()
	}
}

/// How the camera is doing, for the Output panel.
pub struct Status {
	/// Where the camera lives, such as a PipeWire node or a `/dev/videoN` path.
	pub backend: String,
	/// Whether an app reads the camera.
	pub reading: bool,
	pub sent: u64,
	pub dropped: u64,
	/// Why the camera stopped, if it did.
	pub error: Option<String>,
}

/// Where the camera's frames go, on the `vcam` thread.
trait Sink: Send {
	/// Show one picture: tightly packed RGBA rows at the canvas size.
	fn write(&mut self, rgba: &[u8]) -> anyhow::Result<()>;
}

impl Camera {
	/// Publish `canvas`-sized programs drawn on `device` as a camera.
	///
	/// Fails, naming why each backend refused, when this machine has no way to
	/// publish one.
	pub fn open(device: &wgpu::Device, queue: &wgpu::Queue, canvas: Canvas) -> anyhow::Result<Self> {
		let size = moq_video::Size::new(canvas.width, canvas.height);
		let shared = Arc::new(Shared::default());
		let (sink, backend) = backend(size, canvas.framerate, &shared)?;
		tracing::info!(%backend, "virtual camera on");

		let (frames, pending) = mpsc::sync_channel(DEPTH);
		let worker = {
			let device = device.clone();
			let shared = shared.clone();
			std::thread::Builder::new()
				.name("vcam".into())
				.spawn(move || run(&device, pending, sink, &shared))
				.context("cannot start the virtual camera thread")?
		};
		Ok(Self {
			backend,
			readback: Readback::new(device, queue, size),
			frames: Some(frames),
			worker: Some(worker),
			shared,
		})
	}

	/// Copy `program` to the camera, if an app reads it.
	///
	/// The copy is submitted before this returns, so the caller draws into
	/// `program` again freely. Never waits on the GPU.
	pub fn offer(&mut self, program: &wgpu::Texture) {
		if !self.shared.wanted() {
			return;
		}
		let Some(frames) = &self.frames else { return };
		let Some(staged) = self.readback.copy(program) else {
			self.shared.dropped.fetch_add(1, Ordering::Relaxed);
			return;
		};
		// The pool holds no more buffers than the channel, so it is never full;
		// a send fails only once the thread stopped, having said why.
		let _ = frames.try_send(staged);
	}

	pub fn status(&self) -> Status {
		Status {
			backend: self.backend.clone(),
			reading: self.shared.wanted(),
			sent: self.shared.sent.load(Ordering::Relaxed),
			dropped: self.shared.dropped.load(Ordering::Relaxed),
			error: self.shared.error.lock().unwrap().clone(),
		}
	}
}

impl Drop for Camera {
	fn drop(&mut self) {
		// Ends the thread, which removes the camera before this returns, so
		// turning it straight back on finds the device free.
		self.frames.take();
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

/// The first backend that takes a `size` camera at `framerate`.
#[cfg(target_os = "linux")]
fn backend(size: moq_video::Size, framerate: u32, shared: &Arc<Shared>) -> anyhow::Result<(Box<dyn Sink>, String)> {
	#[cfg(feature = "pipewire")]
	let pipewire = match pipewire::Node::open(size, framerate, shared.clone()) {
		Ok(node) => return Ok((Box::new(node), "PipeWire".to_string())),
		Err(err) => {
			tracing::warn!(err = %format!("{err:#}"), "no PipeWire camera, trying v4l2loopback");
			format!("{err:#}")
		}
	};
	#[cfg(not(feature = "pipewire"))]
	let pipewire = "this build has no `pipewire` feature".to_string();

	match v4l2::Loopback::open(size, framerate) {
		Ok(loopback) => {
			let backend = loopback.name();
			// The kernel keeps no count of readers to wait for.
			shared.reading.store(true, Ordering::Relaxed);
			Ok((Box::new(loopback), backend))
		}
		Err(err) => anyhow::bail!("no virtual camera. PipeWire: {pipewire}. v4l2loopback: {err:#}"),
	}
}

#[cfg(not(target_os = "linux"))]
fn backend(size: moq_video::Size, framerate: u32, shared: &Arc<Shared>) -> anyhow::Result<(Box<dyn Sink>, String)> {
	let _ = (size, framerate, shared);
	anyhow::bail!("the virtual camera is not available on this platform yet")
}

/// Hand each copy to `sink` as it lands, until the camera is dropped or fails.
fn run(device: &wgpu::Device, pending: mpsc::Receiver<Staged>, mut sink: Box<dyn Sink>, shared: &Shared) {
	let mut rgba = Vec::new();
	while let Ok(staged) = pending.recv() {
		if let Err(err) = staged.read(device, &mut rgba).and_then(|()| sink.write(&rgba)) {
			shared.fail(format!("{err:#}"));
			return;
		}
		shared.sent.fetch_add(1, Ordering::Relaxed);
	}
}

/// Copies the program into a small pool of staging buffers the CPU can map.
struct Readback {
	device: wgpu::Device,
	queue: wgpu::Queue,
	size: moq_video::Size,
	/// Bytes per row in a staging buffer, padded to the copy alignment.
	row: u32,
	pool: Arc<Mutex<Pool>>,
}

/// Staging buffers not held by a [`Staged`].
#[derive(Default)]
struct Pool {
	free: Vec<wgpu::Buffer>,
	/// Buffers in existence, up to [`DEPTH`].
	allocated: usize,
}

impl Readback {
	fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: moq_video::Size) -> Self {
		Self {
			device: device.clone(),
			queue: queue.clone(),
			size,
			row: (size.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
			pool: Arc::default(),
		}
	}

	/// Start copying `texture`, or `None` when every buffer is in flight.
	fn copy(&mut self, texture: &wgpu::Texture) -> Option<Staged> {
		let mut pool = self.pool.lock().unwrap();
		let buffer = match pool.free.pop() {
			Some(buffer) => buffer,
			None if pool.allocated < DEPTH => {
				pool.allocated += 1;
				self.device.create_buffer(&wgpu::BufferDescriptor {
					label: Some("moq-studio vcam"),
					size: u64::from(self.row) * u64::from(self.size.height),
					usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
					mapped_at_creation: false,
				})
			}
			None => return None,
		};
		drop(pool);

		let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("moq-studio vcam"),
		});
		encoder.copy_texture_to_buffer(
			texture.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &buffer,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(self.row),
					rows_per_image: Some(self.size.height),
				},
			},
			wgpu::Extent3d {
				width: self.size.width,
				height: self.size.height,
				depth_or_array_layers: 1,
			},
		);
		let submission = self.queue.submit([encoder.finish()]);
		Some(Staged {
			buffer: Some(buffer),
			submission,
			size: self.size,
			row: self.row,
			pool: self.pool.clone(),
			mapping: false,
		})
	}
}

/// One copy in flight. Dropping it returns its buffer to the pool.
struct Staged {
	buffer: Option<wgpu::Buffer>,
	submission: wgpu::SubmissionIndex,
	size: moq_video::Size,
	row: u32,
	pool: Arc<Mutex<Pool>>,
	/// Set while a mapping was requested and not yet undone, so a read that
	/// failed partway does not hand a mapped buffer to the next copy.
	mapping: bool,
}

impl Staged {
	/// Block until the copy lands, then fill `rgba` with tightly packed rows.
	fn read(mut self, device: &wgpu::Device, rgba: &mut Vec<u8>) -> anyhow::Result<()> {
		let buffer = self.buffer.as_ref().expect("held until drop");
		let (send, recv) = mpsc::channel();
		self.mapping = true;
		buffer.map_async(wgpu::MapMode::Read, .., move |result| {
			let _ = send.send(result);
		});
		device
			.poll(wgpu::PollType::Wait {
				submission_index: Some(self.submission.clone()),
				timeout: None,
			})
			.map_err(|err| anyhow::anyhow!("waiting for the program copy: {err}"))?;
		recv.recv()
			.context("the program copy was dropped")?
			.context("mapping the program copy")?;

		let width = self.size.width as usize * 4;
		rgba.clear();
		{
			let mapped = buffer
				.slice(..)
				.get_mapped_range()
				.map_err(|err| anyhow::anyhow!("reading the program copy: {err}"))?;
			for row in mapped.chunks_exact(self.row as usize) {
				rgba.extend_from_slice(&row[..width]);
			}
		}
		buffer.unmap();
		self.mapping = false;
		Ok(())
	}
}

impl Drop for Staged {
	fn drop(&mut self) {
		let Some(buffer) = self.buffer.take() else { return };
		let mut pool = self.pool.lock().unwrap();
		// One a failed read left mapped is let go and replaced.
		if self.mapping {
			pool.allocated -= 1;
		} else {
			pool.free.push(buffer);
		}
	}
}
