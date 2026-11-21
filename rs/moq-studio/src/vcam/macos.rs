//! The camera through the camera extension the macOS app bundle carries.
//!
//! The first camera turned on in a run of the app asks the system to activate
//! the extension: that installs it, or the newer one an app update carries,
//! without a reboot. The user allows it once, in System Settings. The studio
//! then asks the program's camera, which the extension always lists, for the
//! camera it wants, waits for that camera to show it, and feeds the camera's
//! sink stream BGRA pictures while an app reads it. `moq_studio_camera` says
//! how the two talk.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Context;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchRetained, GlobalQueueIdentifier};
use moq_studio_camera::{CONFIG, Config, EXTENSION, PROGRAM, Property, READING};
use moq_video::Size;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
	CMClock, CMSampleBuffer, CMSampleTimingInfo, CMSimpleQueue, CMTime, CMVideoFormatDescription,
	CMVideoFormatDescriptionCreateForImageBuffer, kCMTimeInvalid,
};
use objc2_core_media_io::{
	CMIODeviceStartStream, CMIODeviceStopStream, CMIOObjectAddPropertyListenerBlock, CMIOObjectGetPropertyData,
	CMIOObjectGetPropertyDataSize, CMIOObjectID, CMIOObjectPropertyAddress, CMIOObjectRemovePropertyListenerBlock,
	CMIOObjectSetPropertyData, CMIOStreamCopyBufferQueue, kCMIODevicePropertyDeviceUID, kCMIODevicePropertyStreams,
	kCMIOHardwarePropertyDevices, kCMIOObjectPropertyElementMain, kCMIOObjectPropertyScopeGlobal,
	kCMIOObjectSystemObject, kCMIOStreamPropertyDirection,
};
use objc2_core_video::{
	CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress,
	CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey,
	kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
	kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_system_extensions::{
	OSSystemExtensionManager, OSSystemExtensionProperties, OSSystemExtensionReplacementAction,
	OSSystemExtensionRequest, OSSystemExtensionRequestDelegate, OSSystemExtensionRequestResult,
};

use super::{Shared, Sink};

/// How long activating an extension already allowed may take, which
/// replacing it with an update's includes.
const ACTIVATE: Duration = Duration::from_secs(30);

/// How long the extension may take to list the camera asked for.
const APPEAR: Duration = Duration::from_secs(10);

/// Where the user allows the extension.
const ALLOW: &str = "System Settings > General > Login Items & Extensions > Camera Extensions";

/// Whether this run of the app has activated the extension. Held while a
/// camera opens, from asking for it until feeding it: the extension keeps
/// only the camera asked for last until the studio feeds it, so two cameras
/// opening at once, as after a frame rate change, would remove each other's.
static OPENING: Mutex<bool> = Mutex::new(false);

/// A camera of the extension, fed from the `vcam` thread.
pub struct Extension {
	/// The device id apps list the camera by.
	id: String,
	/// Whether it feeds a scene's camera, which goes away with its sink.
	scene: bool,
	device: CMIOObjectID,
	stream: CMIOObjectID,
	/// The sink's queue: the studio enqueues, the extension dequeues.
	queue: CFRetained<CMSimpleQueue>,
	pool: CFRetained<CVPixelBufferPool>,
	/// Made from the first picture, as every later one matches it.
	format: Option<CFRetained<CMVideoFormatDescription>>,
	size: Size,
	framerate: u32,
	/// Says whether an app reads the camera, on `listening`.
	reading: RcBlock<dyn Fn(u32, *mut CMIOObjectPropertyAddress)>,
	listening: DispatchRetained<DispatchQueue>,
	shared: Arc<Shared>,
}

// SAFETY: CoreMediaIO's, CoreMedia's, and CoreVideo's objects are reference
// counted with atomics and safe to use from any thread; the `vcam` thread is
// the only one that uses them through this. The listener block captures only
// a `Shared` and a device id, both `Send`.
unsafe impl Send for Extension {}

impl Extension {
	/// Open the extension's camera of the program, or of the scene called
	/// `scene`, at `size` and `framerate`, and start feeding it, with the id
	/// apps list it by.
	pub fn open(
		scene: Option<&str>,
		size: Size,
		framerate: u32,
		shared: Arc<Shared>,
	) -> anyhow::Result<(Self, String)> {
		let mut activated = OPENING.lock().unwrap();
		if !*activated {
			activate()?;
			*activated = true;
		}
		let config = Config {
			scene: scene.map(str::to_string),
			width: size.width,
			height: size.height,
			framerate,
		};
		let wanted = config.to_string();
		let id = config.device();

		let program = appear(PROGRAM, None)
			.with_context(|| format!("the kixel.live Studio camera extension is not running: allow it in {ALLOW}"))?;
		set_string(program, CONFIG, &wanted).context("asking the camera extension for a camera")?;
		let device = appear(&id, Some(&wanted)).context("the camera extension never listed the camera")?;
		let stream = sink(device)?;

		let listening = DispatchQueue::new("live.kixel.studio.vcam", None);
		let reading = {
			let shared = shared.clone();
			RcBlock::new(move |_count: u32, _addresses: *mut CMIOObjectPropertyAddress| {
				read(device, &shared);
			})
		};
		let address = address(READING.selector());
		let status = unsafe {
			CMIOObjectAddPropertyListenerBlock(device, &address, Some(&listening), RcBlock::as_ptr(&reading))
		};
		anyhow::ensure!(status == 0, "cannot listen to the camera: {status}");
		read(device, &shared);

		let mut queue: *mut CMSimpleQueue = std::ptr::null_mut();
		let status = unsafe { CMIOStreamCopyBufferQueue(stream, Some(altered), std::ptr::null_mut(), &mut queue) };
		let queue = NonNull::new(queue)
			.filter(|_| status == 0)
			.map(|queue| unsafe { CFRetained::from_raw(queue) })
			.with_context(|| format!("cannot reach the camera's sink: {status}"))?;
		let pool = pool(size)?;
		let mut this = Self {
			id: id.clone(),
			// Not yet: only a camera the studio fed goes away when it stops.
			scene: false,
			device,
			stream,
			queue,
			pool,
			format: None,
			size,
			framerate,
			reading,
			listening,
			shared,
		};
		let status = unsafe { CMIODeviceStartStream(device, stream) };
		anyhow::ensure!(status == 0, "cannot start feeding the camera: {status}");
		this.scene = scene.is_some();
		Ok((this, id))
	}
}

impl Sink for Extension {
	fn write(&mut self, rgba: &[u8]) -> anyhow::Result<()> {
		// The extension has not taken the last pictures yet: this one would
		// only wait behind them.
		if unsafe { self.queue.count() >= self.queue.capacity() } {
			self.shared.dropped.fetch_add(1, Ordering::Relaxed);
			return Ok(());
		}
		let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
		let status = unsafe { CVPixelBufferPool::create_pixel_buffer(None, &self.pool, NonNull::from(&mut buffer)) };
		let buffer = NonNull::new(buffer)
			.filter(|_| status == 0)
			.map(|buffer| unsafe { CFRetained::from_raw(buffer) })
			.with_context(|| format!("cannot get a picture buffer: {status}"))?;
		swizzle(&buffer, rgba, self.size)?;

		let format = match &self.format {
			Some(format) => format.clone(),
			None => {
				let mut format: *const CMVideoFormatDescription = std::ptr::null();
				let status =
					unsafe { CMVideoFormatDescriptionCreateForImageBuffer(None, &buffer, NonNull::from(&mut format)) };
				let format = NonNull::new(format.cast_mut())
					.filter(|_| status == 0)
					.map(|format| unsafe { CFRetained::from_raw(format) })
					.with_context(|| format!("cannot describe the picture: {status}"))?;
				self.format.insert(format).clone()
			}
		};
		let timing = CMSampleTimingInfo {
			duration: unsafe { CMTime::new(1, self.framerate as i32) },
			presentationTimeStamp: unsafe { CMClock::host_time_clock().time() },
			decodeTimeStamp: unsafe { kCMTimeInvalid },
		};
		let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
		let status = unsafe {
			CMSampleBuffer::create_for_image_buffer(
				None,
				&buffer,
				true,
				None,
				std::ptr::null_mut(),
				&format,
				NonNull::from(&timing),
				NonNull::from(&mut sample),
			)
		};
		let sample = NonNull::new(sample)
			.filter(|_| status == 0)
			.with_context(|| format!("cannot wrap the picture: {status}"))?;
		// The queue takes the reference; the extension's side releases it.
		let status = unsafe { self.queue.enqueue(sample.cast::<c_void>()) };
		if status != 0 {
			drop(unsafe { CFRetained::from_raw(sample) });
			anyhow::bail!("cannot hand the camera a picture: {status}");
		}
		Ok(())
	}
}

impl Drop for Extension {
	fn drop(&mut self) {
		let address = address(READING.selector());
		unsafe {
			CMIOObjectRemovePropertyListenerBlock(
				self.device,
				&address,
				Some(&self.listening),
				RcBlock::as_ptr(&self.reading),
			)
		};
		unsafe { CMIODeviceStopStream(self.device, self.stream) };
		// Stopping the sink of a scene's camera removes the camera.
		if self.scene
			&& let Err(err) = vanish(&self.id)
		{
			tracing::warn!(err = %format!("{err:#}"), "virtual camera");
		}
	}
}

/// Called when the extension takes a picture off the sink's queue. Nothing
/// waits on it: the next `write` looks at the queue itself.
unsafe extern "C-unwind" fn altered(_stream: CMIOObjectID, _token: *mut c_void, _context: *mut c_void) {}

/// Copy `rgba`, tightly packed `size` rows, into `buffer` as BGRA.
fn swizzle(buffer: &CVPixelBuffer, rgba: &[u8], size: Size) -> anyhow::Result<()> {
	let width = size.width as usize * 4;
	anyhow::ensure!(
		rgba.len() == width * size.height as usize,
		"a picture of {} bytes is not {}x{} RGBA",
		rgba.len(),
		size.width,
		size.height
	);
	let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags(0)) };
	anyhow::ensure!(status == 0, "cannot lock the picture buffer: {status}");
	let stride = CVPixelBufferGetBytesPerRow(buffer);
	let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
	let fits = !base.is_null() && stride >= width;
	if fits {
		// SAFETY: locked, and the pool made it `size` BGRA with `stride` bytes
		// a row.
		let target = unsafe { std::slice::from_raw_parts_mut(base, stride * size.height as usize) };
		bgra(target, stride, rgba, width);
	}
	unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags(0)) };
	anyhow::ensure!(fits, "the picture buffer has no room");
	Ok(())
}

/// Copy `rgba` rows of `width` bytes into `target` rows of `stride` bytes,
/// swapping red and blue.
fn bgra(target: &mut [u8], stride: usize, rgba: &[u8], width: usize) {
	for (to, from) in target.chunks_exact_mut(stride).zip(rgba.chunks_exact(width)) {
		let (to, _) = to[..width].as_chunks_mut::<4>();
		let (from, _) = from.as_chunks::<4>();
		for (to, [r, g, b, a]) in to.iter_mut().zip(from) {
			*to = [*b, *g, *r, *a];
		}
	}
}

/// Reusable IOSurface-backed BGRA buffers at `size`, which reach the
/// extension without a copy.
fn pool(size: Size) -> anyhow::Result<CFRetained<CVPixelBufferPool>> {
	let format = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
	let width = CFNumber::new_i32(size.width as i32);
	let height = CFNumber::new_i32(size.height as i32);
	let surface = CFDictionary::<CFString, CFType>::empty();
	let attributes = unsafe {
		CFDictionary::<CFString, CFType>::from_slices(
			&[
				kCVPixelBufferPixelFormatTypeKey,
				kCVPixelBufferWidthKey,
				kCVPixelBufferHeightKey,
				kCVPixelBufferIOSurfacePropertiesKey,
			],
			&[&format, &width, &height, &surface],
		)
	};
	let mut pool: *mut CVPixelBufferPool = std::ptr::null_mut();
	let status =
		unsafe { CVPixelBufferPool::create(None, None, Some(attributes.as_opaque()), NonNull::from(&mut pool)) };
	NonNull::new(pool)
		.filter(|_| status == 0)
		.map(|pool| unsafe { CFRetained::from_raw(pool) })
		.with_context(|| format!("cannot make picture buffers: {status}"))
}

/// Store whether an app reads the camera `device` in `shared`.
fn read(device: CMIOObjectID, shared: &Shared) {
	match get_string(device, READING) {
		Ok(reading) => shared.reading.store(reading == "1", Ordering::Relaxed),
		Err(err) => shared.fail(format!("{err:#}")),
	}
}

/// Ask the system to activate the extension in the app bundle, which installs
/// it or the update's, replacing the old one. The extension
/// takes pictures only from the app, so a build run from outside it has no
/// camera.
fn activate() -> anyhow::Result<()> {
	let bundle = NSBundle::mainBundle().bundleIdentifier();
	anyhow::ensure!(
		bundle.is_some_and(|bundle| bundle.to_string() == moq_studio_camera::APP),
		"the virtual camera runs only from the kixel.live Studio app"
	);

	let (tx, rx) = mpsc::channel();
	let delegate = Activation::new(tx);
	let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Default));
	unsafe {
		let request =
			OSSystemExtensionRequest::activationRequestForExtension_queue(&NSString::from_str(EXTENSION), &queue);
		request.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
		OSSystemExtensionManager::sharedManager().submitRequest(&request);
	}
	// The request holds its delegate weakly, and still finishes once the user
	// allows the extension after this run stopped waiting on it.
	std::mem::forget(delegate);

	match rx.recv_timeout(ACTIVATE) {
		Ok(Activated::Done) => Ok(()),
		Ok(Activated::Approval) => {
			anyhow::bail!("allow the kixel.live Studio camera in {ALLOW}, then turn this camera off and on again")
		}
		Ok(Activated::Reboot) => anyhow::bail!("restart the Mac to finish installing the kixel.live Studio camera"),
		Ok(Activated::Failed(err)) => anyhow::bail!("cannot install the kixel.live Studio camera: {err}"),
		Err(_) => anyhow::bail!("the system never installed the kixel.live Studio camera"),
	}
}

/// How an activation request ended, or that it waits on the user.
enum Activated {
	Done,
	Approval,
	Reboot,
	Failed(String),
}

define_class!(
	#[unsafe(super(NSObject))]
	#[name = "MoqStudioCameraActivation"]
	#[ivars = Mutex<mpsc::Sender<Activated>>]
	struct Activation;

	unsafe impl NSObjectProtocol for Activation {}

	unsafe impl OSSystemExtensionRequestDelegate for Activation {
		#[unsafe(method(request:actionForReplacingExtension:withExtension:))]
		fn replace(
			&self,
			_request: &OSSystemExtensionRequest,
			_existing: &OSSystemExtensionProperties,
			_update: &OSSystemExtensionProperties,
		) -> OSSystemExtensionReplacementAction {
			// The bundle's is the one this app speaks to, newer or older.
			OSSystemExtensionReplacementAction::Replace
		}

		#[unsafe(method(requestNeedsUserApproval:))]
		fn needs_approval(&self, _request: &OSSystemExtensionRequest) {
			self.send(Activated::Approval);
		}

		#[unsafe(method(request:didFinishWithResult:))]
		fn finished(&self, _request: &OSSystemExtensionRequest, result: OSSystemExtensionRequestResult) {
			self.send(match result {
				OSSystemExtensionRequestResult::Completed => Activated::Done,
				_ => Activated::Reboot,
			});
		}

		#[unsafe(method(request:didFailWithError:))]
		fn failed(&self, _request: &OSSystemExtensionRequest, error: &NSError) {
			self.send(Activated::Failed(error.localizedDescription().to_string()));
		}
	}
);

impl Activation {
	fn new(tx: mpsc::Sender<Activated>) -> Retained<Self> {
		let this = Self::alloc().set_ivars(Mutex::new(tx));
		unsafe { msg_send![super(this), init] }
	}

	fn send(&self, activated: Activated) {
		// Nobody waits once the toggle reported the first answer.
		let _ = self.ivars().lock().unwrap().send(activated);
	}
}

fn address(selector: u32) -> CMIOObjectPropertyAddress {
	CMIOObjectPropertyAddress {
		mSelector: selector,
		mScope: kCMIOObjectPropertyScopeGlobal,
		mElement: kCMIOObjectPropertyElementMain,
	}
}

/// Wait until a camera with the device id `id` is listed, showing `config`
/// if given, and return its object.
fn appear(id: &str, config: Option<&str>) -> anyhow::Result<CMIOObjectID> {
	watch(|| find(id, config))?.with_context(|| format!("no camera {id}"))
}

/// Wait until no camera with the device id `id` is listed, so turning it
/// straight back on finds the new one rather than the one going away.
fn vanish(id: &str) -> anyhow::Result<()> {
	let gone = || Ok(find(id, None)?.is_none().then_some(()));
	watch(gone)?.with_context(|| format!("the camera {id} never went away"))
}

/// Check the cameras with `check` until it finds something, again each time
/// the list changes, for at most [`APPEAR`]; `None` once that passes.
fn watch<T>(mut check: impl FnMut() -> anyhow::Result<Option<T>>) -> anyhow::Result<Option<T>> {
	let (tx, rx) = mpsc::channel();
	let changed = RcBlock::new(move |_count: u32, _addresses: *mut CMIOObjectPropertyAddress| {
		let _ = tx.send(());
	});
	let queue = DispatchQueue::new("live.kixel.studio.vcam.devices", None);
	let address = address(kCMIOHardwarePropertyDevices);
	let status = unsafe {
		CMIOObjectAddPropertyListenerBlock(
			kCMIOObjectSystemObject,
			&address,
			Some(&queue),
			RcBlock::as_ptr(&changed),
		)
	};
	anyhow::ensure!(status == 0, "cannot watch the cameras: {status}");
	let start = kio::clock::now();
	let found = loop {
		match check() {
			Ok(None) => {}
			found => break found,
		}
		let left = APPEAR.saturating_sub(start.elapsed());
		if left.is_zero() || rx.recv_timeout(left).is_err() {
			break Ok(None);
		}
	};
	unsafe {
		CMIOObjectRemovePropertyListenerBlock(
			kCMIOObjectSystemObject,
			&address,
			Some(&queue),
			RcBlock::as_ptr(&changed),
		)
	};
	found
}

/// The listed camera with the device id `id`, showing `config` if given.
fn find(id: &str, config: Option<&str>) -> anyhow::Result<Option<CMIOObjectID>> {
	for device in get_ids(kCMIOObjectSystemObject, kCMIOHardwarePropertyDevices)? {
		// A camera that went away as it was listed has no id left to read.
		let Ok(uid) = get_cf_string(device, kCMIODevicePropertyDeviceUID) else {
			continue;
		};
		if uid != id {
			continue;
		}
		match config {
			None => return Ok(Some(device)),
			Some(config) if get_string(device, CONFIG).is_ok_and(|shown| shown == config) => return Ok(Some(device)),
			Some(_) => {}
		}
	}
	Ok(None)
}

/// The sink stream of the camera `device`: its output stream, in CoreMediaIO's
/// terms.
fn sink(device: CMIOObjectID) -> anyhow::Result<CMIOObjectID> {
	for stream in get_ids(device, kCMIODevicePropertyStreams)? {
		let mut direction = 0u32;
		get(
			stream,
			kCMIOStreamPropertyDirection,
			std::slice::from_mut(&mut direction),
		)?;
		if direction == 0 {
			return Ok(stream);
		}
	}
	anyhow::bail!("the camera has no sink")
}

/// Read the property `selector` of `object` into `data`, returning how many
/// bytes it filled.
fn get<T>(object: CMIOObjectID, selector: u32, data: &mut [T]) -> anyhow::Result<usize> {
	let address = address(selector);
	let mut used = 0u32;
	let status = unsafe {
		CMIOObjectGetPropertyData(
			object,
			&address,
			0,
			std::ptr::null(),
			std::mem::size_of_val(data) as u32,
			&mut used,
			data.as_mut_ptr().cast(),
		)
	};
	anyhow::ensure!(status == 0, "cannot read property {selector:#x}: {status}");
	Ok(used as usize)
}

/// The object ids the property `selector` of `object` lists.
fn get_ids(object: CMIOObjectID, selector: u32) -> anyhow::Result<Vec<CMIOObjectID>> {
	let address = address(selector);
	let mut size = 0u32;
	let status = unsafe { CMIOObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) };
	anyhow::ensure!(status == 0, "cannot size property {selector:#x}: {status}");
	let mut ids = vec![0; size as usize / std::mem::size_of::<CMIOObjectID>()];
	let used = get(object, selector, &mut ids)?;
	ids.truncate(used / std::mem::size_of::<CMIOObjectID>());
	Ok(ids)
}

/// A string property, which CoreMediaIO hands over as a retained CFString.
fn get_cf_string(object: CMIOObjectID, selector: u32) -> anyhow::Result<String> {
	let mut string: *const CFString = std::ptr::null();
	get(object, selector, std::slice::from_mut(&mut string))?;
	let string = NonNull::new(string.cast_mut()).context("an empty string property")?;
	Ok(unsafe { CFRetained::from_raw(string) }.to_string())
}

fn get_string(object: CMIOObjectID, property: Property) -> anyhow::Result<String> {
	get_cf_string(object, property.selector())
}

fn set_string(object: CMIOObjectID, property: Property, value: &str) -> anyhow::Result<()> {
	let value = CFString::from_str(value);
	let pointer: *const CFString = &*value;
	let address = address(property.selector());
	let status = unsafe {
		CMIOObjectSetPropertyData(
			object,
			&address,
			0,
			std::ptr::null(),
			std::mem::size_of_val(&pointer) as u32,
			(&raw const pointer).cast(),
		)
	};
	anyhow::ensure!(status == 0, "cannot set property {}: {status}", property.name());
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn swaps_red_and_blue_into_padded_rows() {
		// Two pixels a row, two rows, into rows padded to 12 bytes.
		let rgba = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
		let mut target = [0xff; 24];
		bgra(&mut target, 12, &rgba, 8);
		assert_eq!(
			target,
			[
				3, 2, 1, 4, 7, 6, 5, 8, 0xff, 0xff, 0xff, 0xff, 11, 10, 9, 12, 15, 14, 13, 16, 0xff, 0xff, 0xff, 0xff
			]
		);
	}
}
