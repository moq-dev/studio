//! The program, or a published scene, as a system camera, for video-call and
//! conferencing apps that offer no ingest.
//!
//! On Linux the camera is a PipeWire video node (with the `pipewire` feature),
//! which PipeWire-aware apps such as browsers list as a camera. Without
//! PipeWire it is an existing v4l2loopback device, since that kernel module
//! needs root to load and so is never the first choice.
//!
//! The program clock offers every frame it draws of what a camera shows;
//! [`Camera::offer`] has moq-video read it back only while an app reads the
//! camera, and the `vcam` thread waits for the copy and hands the pixels to
//! the backend.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use moq_scene::{Canvas, wgpu};
use moq_video::render::readback::{Pending, Readback};

#[cfg(all(target_os = "linux", feature = "pipewire"))]
mod pipewire;
#[cfg(target_os = "linux")]
mod v4l2;

/// The program, or a published scene, as a camera other apps can open.
///
/// Dropping it removes the camera.
pub struct Camera {
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// The canvas the backend was opened for.
	canvas: Canvas,
	readback: Readback,
	/// `None` once dropping.
	work: Option<mpsc::Sender<Work>>,
	worker: Option<std::thread::JoinHandle<()>>,
	shared: Arc<Shared>,
}

/// What the program clock hands the `vcam` thread.
enum Work {
	/// A copy on its way back from the GPU. Unbounded in the channel
	/// only in name: the readback lends out at most [`Readback::DEPTH`] at once.
	Picture(Pending),
	/// What the camera shows changed size or frame rate: close the backend
	/// and open it again for this canvas.
	Reopen(Canvas),
}

/// What the camera's threads report to the program clock and the UI.
#[derive(Default)]
struct Shared {
	/// Where the camera lives now.
	opened: Mutex<Opened>,
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

/// Where an open camera lives.
#[derive(Clone, Default)]
struct Opened {
	/// For the Output panel: `PipeWire`, or the v4l2loopback device's path.
	backend: String,
	/// The id moq-video's camera capture lists it by.
	id: String,
	/// The size and frame rate it shows.
	canvas: Canvas,
}

/// How the camera is doing, for the Output panel.
pub struct Status {
	/// Where the camera lives, such as a PipeWire node or a `/dev/videoN` path.
	pub backend: String,
	/// The id the Devices panel would list the camera by, so it can leave it out.
	pub id: String,
	/// The size and frame rate it shows.
	pub canvas: Canvas,
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
	/// Publish the program, or the published scene called `scene`, drawn on
	/// `device` at `canvas`, as a camera named after it.
	///
	/// Fails, naming why each backend refused, when this machine has no way to
	/// publish one.
	pub fn open(
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		scene: Option<&str>,
		canvas: Canvas,
	) -> anyhow::Result<Self> {
		let readback = Readback::new(device, queue, size(canvas))?;
		let shared = Arc::new(Shared::default());
		let scene = scene.map(str::to_string);
		let sink = open(scene.as_deref(), canvas, &shared)?;

		let (work, pending) = mpsc::channel();
		let worker = {
			let shared = shared.clone();
			std::thread::Builder::new()
				.name("vcam".into())
				.spawn(move || run(pending, sink, scene.as_deref(), &shared))
				.context("cannot start the virtual camera thread")?
		};
		Ok(Self {
			device: device.clone(),
			queue: queue.clone(),
			canvas,
			readback,
			work: Some(work),
			worker: Some(worker),
			shared,
		})
	}

	/// Whether an app reads the camera, so what it shows is worth drawing.
	pub fn wanted(&self) -> bool {
		self.shared.wanted()
	}

	/// Copy `picture`, what the camera shows as drawn at `canvas`, to the
	/// camera if an app reads it, or show nothing new for `None`, when it was
	/// not drawn.
	///
	/// A canvas of a new size or frame rate reopens the camera for it, so an
	/// app reading it opens it again.
	///
	/// The copy is submitted before this returns, so the caller draws into
	/// `picture` again freely. Never waits on the GPU.
	pub fn offer(&mut self, picture: Option<&wgpu::Texture>, canvas: Canvas) {
		let Some(work) = &self.work else { return };
		if canvas != self.canvas {
			self.canvas = canvas;
			match Readback::new(&self.device, &self.queue, size(canvas)) {
				Ok(readback) => self.readback = readback,
				Err(err) => return self.shared.fail(format!("{err:#}")),
			}
			// A send fails only once the thread stopped, having said why.
			let _ = work.send(Work::Reopen(canvas));
		}
		let Some(picture) = picture.filter(|_| self.shared.wanted()) else {
			return;
		};
		match self.readback.copy(picture) {
			Ok(Some(pending)) => {
				let _ = work.send(Work::Picture(pending));
			}
			Ok(None) => {
				self.shared.dropped.fetch_add(1, Ordering::Relaxed);
			}
			Err(err) => self.shared.fail(format!("{err:#}")),
		}
	}

	pub fn status(&self) -> Status {
		let Opened { backend, id, canvas } = self.shared.opened.lock().unwrap().clone();
		Status {
			backend,
			id,
			canvas,
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
		self.work.take();
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

fn size(canvas: Canvas) -> moq_video::Size {
	moq_video::Size::new(canvas.width, canvas.height)
}

/// Open the first backend that takes `canvas` for the camera of the program,
/// or of `scene`, and say where it lives in `shared`.
fn open(scene: Option<&str>, canvas: Canvas, shared: &Arc<Shared>) -> anyhow::Result<Box<dyn Sink>> {
	let (sink, backend, id) = backend(scene, size(canvas), canvas.framerate, shared)?;
	tracing::info!(%backend, "virtual camera on");
	*shared.opened.lock().unwrap() = Opened { backend, id, canvas };
	Ok(sink)
}

/// The first backend that takes a `size` camera of the program, or of
/// `scene`, at `framerate`, with where it lives and its capture id.
#[cfg(target_os = "linux")]
fn backend(
	scene: Option<&str>,
	size: moq_video::Size,
	framerate: u32,
	shared: &Arc<Shared>,
) -> anyhow::Result<(Box<dyn Sink>, String, String)> {
	#[cfg(feature = "pipewire")]
	let pipewire = match pipewire::Node::open(scene, size, framerate, shared.clone()) {
		Ok(node) => {
			let id = node.id();
			return Ok((Box::new(node), "PipeWire".to_string(), id));
		}
		Err(err) => {
			tracing::warn!(err = %format!("{err:#}"), "no PipeWire camera, trying v4l2loopback");
			format!("{err:#}")
		}
	};
	#[cfg(not(feature = "pipewire"))]
	let pipewire = {
		// A loopback device is named when its module loads, not by the studio.
		let _ = scene;
		"this build has no `pipewire` feature".to_string()
	};

	match v4l2::Loopback::open(size, framerate) {
		Ok(loopback) => {
			let path = loopback.name();
			// The kernel keeps no count of readers to wait for.
			shared.reading.store(true, Ordering::Relaxed);
			Ok((Box::new(loopback), path.clone(), path))
		}
		Err(err) => anyhow::bail!("no virtual camera. PipeWire: {pipewire}. v4l2loopback: {err:#}"),
	}
}

#[cfg(not(target_os = "linux"))]
fn backend(
	scene: Option<&str>,
	size: moq_video::Size,
	framerate: u32,
	shared: &Arc<Shared>,
) -> anyhow::Result<(Box<dyn Sink>, String, String)> {
	let _ = (scene, size, framerate, shared);
	anyhow::bail!("the virtual camera is not available on this platform yet")
}

/// Hand each copy to `sink`, the camera of the program or of `scene`, as it
/// lands, until the camera is dropped or fails.
fn run(pending: mpsc::Receiver<Work>, mut sink: Box<dyn Sink>, scene: Option<&str>, shared: &Arc<Shared>) {
	while let Ok(work) = pending.recv() {
		let result = match work {
			Work::Picture(pending) => pending
				.wait()
				.context("reading the picture back")
				.and_then(|rgba| sink.write(rgba.data())),
			Work::Reopen(canvas) => {
				// Closed first, so the new one finds the device free.
				drop(sink);
				shared.reading.store(false, Ordering::Relaxed);
				match open(scene, canvas, shared) {
					Ok(opened) => {
						sink = opened;
						continue;
					}
					Err(err) => return shared.fail(format!("{err:#}")),
				}
			}
		};
		if let Err(err) = result {
			return shared.fail(format!("{err:#}"));
		}
		shared.sent.fetch_add(1, Ordering::Relaxed);
	}
}
