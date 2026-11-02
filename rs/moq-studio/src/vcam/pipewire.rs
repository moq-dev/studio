//! The camera as a PipeWire video node.
//!
//! A `Video/Source` node with the `Camera` role is what PipeWire-aware apps
//! (browsers through the camera portal, conferencing apps) list as a camera.
//! It offers one format, the canvas's RGBA at its size and frame rate, so the
//! program reaches an app without a conversion here.
//!
//! PipeWire objects stay on the thread that made them, so the node runs its
//! own main loop. The `vcam` thread leaves the newest picture in a [`Slot`]
//! and wakes the loop; the loop copies it into a PipeWire buffer.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use moq_video::Size;
use pipewire as pw;
use pw::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};

use super::{Shared, Sink};

/// The node's name, which `pw-cli` and pickers that show ids print.
const NAME: &str = "moq-studio";

/// What apps list the camera as.
const DESCRIPTION: &str = "MoQ Studio";

pub struct Node {
	slot: Arc<Slot>,
	wake: pw::channel::Sender<Message>,
	thread: Option<std::thread::JoinHandle<()>>,
	shared: Arc<Shared>,
}

/// The newest picture, handed from the `vcam` thread to the node's loop.
#[derive(Default)]
struct Slot {
	picture: Mutex<Vec<u8>>,
	/// A wake is on its way to the loop, so another is not sent: the channel
	/// never holds more than one.
	woken: AtomicBool,
}

enum Message {
	/// A new picture is in the slot.
	Picture,
	Quit,
}

impl Node {
	/// Add a `size` camera node at `framerate` to the session's PipeWire.
	///
	/// Fails when there is no PipeWire session to add it to.
	pub fn open(size: Size, framerate: u32, shared: Arc<Shared>) -> anyhow::Result<Self> {
		let slot = Arc::new(Slot::default());
		let (wake, messages) = pw::channel::channel();
		let (ready, opened) = mpsc::channel();
		let thread = {
			let slot = slot.clone();
			let shared = shared.clone();
			std::thread::Builder::new()
				.name("vcam-pipewire".into())
				.spawn(move || {
					let ready = Rc::new(RefCell::new(Some(ready)));
					if let Err(err) = run(size, framerate, slot, shared, messages, ready.clone())
						&& let Some(ready) = ready.take()
					{
						let _ = ready.send(Err(format!("{err:#}")));
					}
				})
				.context("cannot start the PipeWire thread")?
		};
		match opened.recv() {
			Ok(Ok(())) => Ok(Self {
				slot,
				wake,
				thread: Some(thread),
				shared,
			}),
			Ok(Err(err)) => {
				let _ = thread.join();
				Err(anyhow::anyhow!(err))
			}
			Err(_) => {
				let _ = thread.join();
				anyhow::bail!("the PipeWire thread stopped")
			}
		}
	}
}

impl Sink for Node {
	fn write(&mut self, rgba: &[u8]) -> anyhow::Result<()> {
		if self.shared.failed() {
			anyhow::bail!("the PipeWire node stopped");
		}
		{
			let mut picture = self.slot.picture.lock().unwrap();
			picture.clear();
			picture.extend_from_slice(rgba);
		}
		if !self.slot.woken.swap(true, Ordering::AcqRel) && self.wake.send(Message::Picture).is_err() {
			anyhow::bail!("the PipeWire node stopped");
		}
		Ok(())
	}
}

impl Drop for Node {
	fn drop(&mut self) {
		let _ = self.wake.send(Message::Quit);
		if let Some(thread) = self.thread.take() {
			let _ = thread.join();
		}
	}
}

/// Whoever waits for the node to be added, until it is or PipeWire refuses it.
type Ready = Rc<RefCell<Option<mpsc::Sender<Result<(), String>>>>>;

/// Add the node and run it until it is dropped or PipeWire ends it, saying on
/// `ready` whether PipeWire added it. A failure before then goes to `ready`
/// rather than `shared`, so the caller can still fall back.
fn run(
	size: Size,
	framerate: u32,
	slot: Arc<Slot>,
	shared: Arc<Shared>,
	messages: pw::channel::Receiver<Message>,
	ready: Ready,
) -> anyhow::Result<()> {
	pw::init();
	let mainloop = pw::main_loop::MainLoopRc::new(None).context("PipeWire main loop")?;
	let context = pw::context::ContextRc::new(&mainloop, None).context("PipeWire context")?;
	let core = context.connect_rc(None).context("connecting to PipeWire")?;
	let stream = pw::stream::StreamRc::new(
		core,
		NAME,
		pw::properties::properties! {
			*pw::keys::MEDIA_TYPE => "Video",
			*pw::keys::MEDIA_CATEGORY => "Capture",
			*pw::keys::MEDIA_CLASS => "Video/Source",
			*pw::keys::MEDIA_ROLE => "Camera",
			*pw::keys::NODE_NAME => NAME,
			*pw::keys::NODE_DESCRIPTION => DESCRIPTION,
		},
	)
	.context("PipeWire stream")?;

	let stride = size.width * 4;
	let _listener = stream
		.add_local_listener::<()>()
		.state_changed({
			let mainloop = mainloop.downgrade();
			let shared = shared.clone();
			move |_, _, _, state| {
				tracing::debug!(?state, "virtual camera node");
				// Streaming once an app links to the node, paused when it leaves.
				let reading = matches!(state, pw::stream::StreamState::Streaming);
				shared.reading.store(reading, Ordering::Relaxed);
				// Our own teardown quits the loop before anything disconnects, so
				// either of these is PipeWire ending the node.
				let ended = match state {
					pw::stream::StreamState::Error(err) => Some(format!("PipeWire: {err}")),
					pw::stream::StreamState::Unconnected => Some("the PipeWire session ended".to_string()),
					// Paused first once PipeWire has added the node.
					pw::stream::StreamState::Paused => {
						if let Some(ready) = ready.take() {
							let _ = ready.send(Ok(()));
						}
						None
					}
					_ => None,
				};
				if let Some(ended) = ended {
					match ready.take() {
						Some(ready) => {
							let _ = ready.send(Err(ended));
						}
						None => shared.fail(ended),
					}
					if let Some(mainloop) = mainloop.upgrade() {
						mainloop.quit();
					}
				}
			}
		})
		.param_changed({
			let shared = shared.clone();
			move |stream, _, id, param| {
				let Some(param) = param else { return };
				if id != spa::param::ParamType::Format.as_raw() {
					return;
				}
				// The offer is fixed, so the format an app picks can only be it.
				let mut info = VideoInfoRaw::new();
				if info.parse(param).is_err() || info.format() != VideoFormat::RGBA {
					shared.fail(format!("an app negotiated {:?}, not RGBA", info.format()));
					return;
				}
				let buffers = buffers(size, stride);
				let Some(buffers) = spa::pod::Pod::from_bytes(&buffers) else {
					return;
				};
				if let Err(err) = stream.update_params(&mut [buffers]) {
					shared.fail(format!("PipeWire buffers: {err}"));
				}
			}
		})
		.process({
			let slot = slot.clone();
			let shared = shared.clone();
			move |stream, _| {
				let picture = slot.picture.lock().unwrap();
				// Nothing to show before the first picture lands.
				if picture.is_empty() {
					return;
				}
				// Every buffer is with the app; it gets the next picture instead.
				let Some(mut buffer) = stream.dequeue_buffer() else {
					return;
				};
				let Some(data) = buffer.datas_mut().first_mut() else {
					return;
				};
				let Some(target) = data.data().and_then(|target| target.get_mut(..picture.len())) else {
					// Sized by our own buffers param, so this is PipeWire
					// handing over something else.
					shared.fail("PipeWire handed over a buffer smaller than a picture");
					return;
				};
				target.copy_from_slice(&picture);
				let chunk = data.chunk_mut();
				*chunk.offset_mut() = 0;
				*chunk.size_mut() = picture.len() as u32;
				*chunk.stride_mut() = stride as i32;
			}
		})
		.register()
		.context("PipeWire stream listener")?;

	let format = format(size, framerate);
	let format = spa::pod::Pod::from_bytes(&format).context("the PipeWire format offer")?;
	// Driving, so the node runs on the program clock: each picture triggers a
	// graph cycle that hands it to whichever app reads the camera. No
	// autoconnect: an app links to the node when it opens the camera.
	stream
		.connect(
			spa::utils::Direction::Output,
			None,
			pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
			&mut [format],
		)
		.context("adding the PipeWire node")?;

	let _messages = messages.attach(mainloop.loop_(), {
		let mainloop = mainloop.downgrade();
		let slot = slot.clone();
		let stream = stream.downgrade();
		move |message| match message {
			Message::Picture => {
				slot.woken.store(false, Ordering::Release);
				let Some(stream) = stream.upgrade() else { return };
				// Another driver runs the graph and takes the newest picture on
				// its own cycle; only a driving node triggers one.
				if stream.is_driving()
					&& let Err(err) = stream.trigger_process()
				{
					tracing::debug!(%err, "cannot trigger the virtual camera");
				}
			}
			Message::Quit => {
				if let Some(mainloop) = mainloop.upgrade() {
					mainloop.quit();
				}
			}
		}
	});

	mainloop.run();
	Ok(())
}

/// The one format the node offers: `size` RGBA at `framerate`.
fn format(size: Size, framerate: u32) -> Vec<u8> {
	let object = spa::pod::object!(
		spa::utils::SpaTypes::ObjectParamFormat,
		spa::param::ParamType::EnumFormat,
		spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
		spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
		spa::pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::RGBA),
		spa::pod::property!(
			FormatProperties::VideoSize,
			Rectangle,
			spa::utils::Rectangle {
				width: size.width,
				height: size.height,
			}
		),
		spa::pod::property!(
			FormatProperties::VideoFramerate,
			Fraction,
			spa::utils::Fraction {
				num: framerate,
				denom: 1,
			}
		),
	);
	serialize(spa::pod::Value::Object(object))
}

/// Buffers sized for one `size` RGBA picture in shared memory.
fn buffers(size: Size, stride: u32) -> Vec<u8> {
	let mem_ptr = 1 << spa::buffer::DataType::MemPtr.as_raw();
	let mem_fd = 1 << spa::buffer::DataType::MemFd.as_raw();
	let object = spa::pod::Object {
		type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
		id: spa::param::ParamType::Buffers.as_raw(),
		properties: vec![
			spa::pod::Property::new(
				spa::sys::SPA_PARAM_BUFFERS_buffers,
				spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
					spa::utils::ChoiceFlags::empty(),
					spa::utils::ChoiceEnum::Range {
						default: 4,
						min: 2,
						max: 8,
					},
				))),
			),
			spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, spa::pod::Value::Int(1)),
			spa::pod::Property::new(
				spa::sys::SPA_PARAM_BUFFERS_size,
				spa::pod::Value::Int((stride * size.height) as i32),
			),
			spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, spa::pod::Value::Int(stride as i32)),
			spa::pod::Property::new(
				spa::sys::SPA_PARAM_BUFFERS_dataType,
				spa::pod::Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
					spa::utils::ChoiceFlags::empty(),
					spa::utils::ChoiceEnum::Flags {
						default: mem_fd | mem_ptr,
						flags: Vec::new(),
					},
				))),
			),
		],
	};
	serialize(spa::pod::Value::Object(object))
}

fn serialize(value: spa::pod::Value) -> Vec<u8> {
	spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
		.expect("serializing a pod into memory cannot fail")
		.0
		.into_inner()
}
