//! The CoreMediaIO provider: the program's camera, a camera per scene the
//! studio asks for, and each camera's two streams.
//!
//! CoreMediaIO holds a source only weakly, so each object here keeps the
//! sources below it alive: the [`Provider`] its cameras, and each [`Camera`]
//! its device, its streams, and their [`Stream`] sources.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchTime, GlobalQueueIdentifier};
use moq_studio_camera::{APP, CONFIG, Config, READING, TEAM};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, Message, define_class, msg_send};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFRunLoop, CFString, CFType};
use objc2_core_media::{CMClock, CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescriptionCreate};
use objc2_core_media_io::{
	CMIOExtensionClient, CMIOExtensionDevice, CMIOExtensionDeviceProperties, CMIOExtensionDeviceSource,
	CMIOExtensionPropertyDeviceModel, CMIOExtensionPropertyDeviceTransportType,
	CMIOExtensionPropertyProviderManufacturer, CMIOExtensionPropertyProviderName, CMIOExtensionPropertyState,
	CMIOExtensionPropertyStreamActiveFormatIndex, CMIOExtensionPropertyStreamFrameDuration,
	CMIOExtensionPropertyStreamSinkBufferQueueSize, CMIOExtensionPropertyStreamSinkBufferUnderrunCount,
	CMIOExtensionPropertyStreamSinkBuffersRequiredForStartup, CMIOExtensionPropertyStreamSinkEndOfData,
	CMIOExtensionProvider, CMIOExtensionProviderProperties, CMIOExtensionProviderSource, CMIOExtensionScheduledOutput,
	CMIOExtensionStream, CMIOExtensionStreamClockType, CMIOExtensionStreamDirection,
	CMIOExtensionStreamDiscontinuityFlags, CMIOExtensionStreamFormat, CMIOExtensionStreamProperties,
	CMIOExtensionStreamSource,
};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_foundation::{
	NSArray, NSDictionary, NSError, NSLocalizedDescriptionKey, NSNumber, NSObject, NSObjectProtocol, NSSet, NSString,
	NSUUID, ns_string,
};

/// The pictures a sink holds before the extension takes them. Two lets the
/// studio queue the next picture while the extension forwards one.
const SINK_QUEUE: u32 = 2;

/// `kIOAudioDeviceTransportTypeVirtual`, what apps show a software camera as.
const VIRTUAL: u32 = u32::from_be_bytes(*b"virt");

/// Serve the cameras until the system stops the extension.
pub fn run() -> ! {
	let source = Provider::new();
	let provider =
		unsafe { CMIOExtensionProvider::providerWithSource_clientQueue(ProtocolObject::from_ref(&*source), None) };
	source.start(&provider);
	unsafe { CMIOExtensionProvider::startServiceWithProvider(&provider) };
	CFRunLoop::run();
	// The run loop returns only once every source is gone.
	std::process::exit(0)
}

/// An object handed to a dispatch queue. Each one sent touches its own state
/// only through mutexes, atomics, and CoreMediaIO objects, which CoreMediaIO
/// itself uses from any queue.
struct Queued<T>(T);

// SAFETY: as the type says.
unsafe impl<T> Send for Queued<T> {}

impl<T> Queued<T> {
	/// Unwrapped through a method, so a closure captures the whole wrapper
	/// rather than the field it would destructure.
	fn take(self) -> T {
		self.0
	}
}

/// The current host time, the clock every camera runs on, in nanoseconds.
fn now() -> u64 {
	let time = unsafe { CMClock::host_time_clock().time() };
	let nanos = i128::from(time.value) * 1_000_000_000 / i128::from(time.timescale.max(1));
	u64::try_from(nanos).unwrap_or(0)
}

unsafe extern "C" {
	fn NSLog(format: &NSString, ...);
}

/// Log to the unified log, where Console and `log stream` find it: the
/// system starts the extension with nowhere for stderr to go.
fn log(message: &str) {
	unsafe { NSLog(ns_string!("%@"), &*NSString::from_str(message)) };
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
	static kSecGuestAttributePid: &'static CFString;
	fn SecCodeCopyGuestWithAttributes(
		host: *const c_void,
		attributes: &CFDictionary,
		flags: u32,
		guest: *mut *const c_void,
	) -> i32;
	fn SecRequirementCreateWithString(text: &CFString, flags: u32, requirement: *mut *const c_void) -> i32;
	fn SecCodeCheckValidity(code: *const c_void, flags: u32, requirement: *const c_void) -> i32;
}

/// Whether the process `pid` is the studio: the app, signed by its team
/// with a certificate Apple issued. Its signing identifier alone proves
/// nothing, since anyone can sign anything with it.
fn studio(pid: i32) -> bool {
	/// Take over a reference a Security create or copy function handed out.
	fn owned(object: *const c_void) -> Option<CFRetained<CFType>> {
		NonNull::new(object.cast_mut().cast::<CFType>()).map(|object| unsafe { CFRetained::from_raw(object) })
	}

	let requirement = CFString::from_str(&format!(
		"identifier \"{APP}\" and anchor apple generic and certificate leaf[subject.OU] = \"{TEAM}\""
	));
	let pid = CFNumber::new_i32(pid);
	let attributes = CFDictionary::<CFString, CFNumber>::from_slices(&[unsafe { kSecGuestAttributePid }], &[&pid]);
	let mut code = std::ptr::null();
	let mut compiled = std::ptr::null();
	let status = unsafe { SecCodeCopyGuestWithAttributes(std::ptr::null(), attributes.as_opaque(), 0, &mut code) };
	let code = owned(code).filter(|_| status == 0);
	let status = unsafe { SecRequirementCreateWithString(&requirement, 0, &mut compiled) };
	let compiled = owned(compiled).filter(|_| status == 0);
	let (Some(code), Some(compiled)) = (code, compiled) else {
		return false;
	};
	let code: *const CFType = &*code;
	let compiled: *const CFType = &*compiled;
	unsafe { SecCodeCheckValidity(code.cast(), 0, compiled.cast()) == 0 }
}

/// An error for a client, saying `message`.
fn error(message: &str) -> Retained<NSError> {
	log(message);
	let message = NSString::from_str(message);
	let info = NSDictionary::from_slices(
		&[unsafe { NSLocalizedDescriptionKey }],
		&[message.as_ref() as &AnyObject],
	);
	unsafe { NSError::errorWithDomain_code_userInfo(ns_string!("live.kixel.studio.camera"), 1, Some(&info)) }
}

/// Write `err` to an Objective-C error out parameter, if the caller gave one.
fn fail(out: *mut *mut NSError, err: Retained<NSError>) {
	if let Some(out) = NonNull::new(out) {
		unsafe { out.write(Retained::autorelease_return(err)) };
	}
}

fn set_of(properties: &[&NSString]) -> Retained<NSSet<NSString>> {
	NSSet::from_slice(properties)
}

fn state(value: &AnyObject) -> Retained<CMIOExtensionPropertyState> {
	unsafe { CMIOExtensionPropertyState::propertyStateWithValue(Some(value)) }
}

struct ProviderIvars {
	provider: OnceLock<Weak<CMIOExtensionProvider>>,
	/// Every camera, the program's first.
	cameras: Mutex<Vec<Retained<Camera>>>,
	/// The device id of the scene camera added last that no client feeds yet.
	/// Any app can write [`CONFIG`], so a new ask removes it: one never holds
	/// more than one camera the studio does not feed.
	pending: Mutex<Option<String>>,
}

define_class!(
	#[unsafe(super(NSObject))]
	#[name = "MoqStudioCameraProvider"]
	#[ivars = ProviderIvars]
	struct Provider;

	unsafe impl NSObjectProtocol for Provider {}

	unsafe impl CMIOExtensionProviderSource for Provider {
		#[unsafe(method(connectClient:error:))]
		fn connect_client(&self, _client: &CMIOExtensionClient, _error: *mut *mut NSError) -> Bool {
			Bool::YES
		}

		#[unsafe(method(disconnectClient:))]
		fn disconnect_client(&self, _client: &CMIOExtensionClient) {}

		#[unsafe(method_id(availableProperties))]
		fn available_properties(&self) -> Retained<NSSet<NSString>> {
			set_of(&[unsafe { CMIOExtensionPropertyProviderName }, unsafe {
				CMIOExtensionPropertyProviderManufacturer
			}])
		}

		#[unsafe(method_id(providerPropertiesForProperties:error:))]
		fn provider_properties(
			&self,
			properties: &NSSet<NSString>,
			_error: *mut *mut NSError,
		) -> Retained<CMIOExtensionProviderProperties> {
			let result =
				unsafe { CMIOExtensionProviderProperties::providerPropertiesWithDictionary(&NSDictionary::new()) };
			let name = NSString::from_str(moq_studio_camera::NAME);
			unsafe {
				if properties.containsObject(CMIOExtensionPropertyProviderName) {
					result.setName(Some(&name));
				}
				if properties.containsObject(CMIOExtensionPropertyProviderManufacturer) {
					result.setManufacturer(Some(&name));
				}
			}
			result
		}

		#[unsafe(method(setProviderProperties:error:))]
		fn set_provider_properties(
			&self,
			_properties: &CMIOExtensionProviderProperties,
			_error: *mut *mut NSError,
		) -> Bool {
			// Both are read only.
			Bool::YES
		}
	}
);

impl Provider {
	fn new() -> Retained<Self> {
		let this = Self::alloc().set_ivars(ProviderIvars {
			provider: OnceLock::new(),
			cameras: Mutex::new(Vec::new()),
			pending: Mutex::new(None),
		});
		unsafe { msg_send![super(this), init] }
	}

	/// Remember `provider` and add the program's camera to it.
	fn start(&self, provider: &CMIOExtensionProvider) {
		let _ = self.ivars().provider.set(Weak::from(provider));
		if let Err(err) = self.add(Config::program()) {
			// Without it the studio has nothing to ask through, so the
			// extension has nothing to serve.
			log(&format!("cannot add the program's camera: {err}"));
			std::process::exit(1);
		}
	}

	fn provider(&self) -> Option<Retained<CMIOExtensionProvider>> {
		self.ivars().provider.get().and_then(Weak::load)
	}

	/// Show `config` on a camera of its own, replacing the camera with its
	/// device id if that shows something else and the studio does not feed
	/// it. Called on the provider's queue, never from inside a property
	/// setter of the camera it replaces.
	fn ask(&self, config: Config) {
		let id = config.device();
		let replaced: Vec<_> = {
			let cameras = self.ivars().cameras.lock().unwrap();
			if cameras.iter().any(|camera| camera.ivars().config == config) {
				return;
			}
			let pending = self.ivars().pending.lock().unwrap().clone();
			let replaced: Vec<_> = cameras
				.iter()
				.filter(|camera| {
					let other = camera.ivars().config.device();
					other == id || (config.scene.is_some() && pending.as_ref() == Some(&other))
				})
				.cloned()
				.collect();
			// Any app may write the config, so none may pull a camera out from
			// under the studio's feed. The studio's own reopen stops its sink
			// before it asks, so this refuses only someone else's ask.
			if replaced.iter().any(|camera| camera.fed()) {
				log(&format!("refused to replace a fed camera with {config}"));
				return;
			}
			replaced
		};
		for camera in replaced {
			self.remove(&camera);
		}
		match self.add(config.clone()) {
			Ok(()) if config.scene.is_some() => *self.ivars().pending.lock().unwrap() = Some(id),
			Ok(()) => {}
			Err(err) => log(&format!("cannot add a camera for {config}: {err}")),
		}
	}

	fn add(&self, config: Config) -> Result<(), String> {
		let provider = self.provider().ok_or("the provider is gone")?;
		let camera = Camera::new(config, self)?;
		let device = camera.ivars().device.get().expect("set by Camera::new");
		unsafe { provider.addDevice_error(device) }.map_err(|err| err.to_string())?;
		self.ivars().cameras.lock().unwrap().push(camera);
		Ok(())
	}

	fn remove(&self, camera: &Camera) {
		let id = camera.ivars().config.device();
		if let Some(provider) = self.provider()
			&& let Some(device) = camera.ivars().device.get()
			&& let Err(err) = unsafe { provider.removeDevice_error(device) }
		{
			log(&format!("cannot remove the camera {id}: {err}"));
		}
		self.ivars()
			.cameras
			.lock()
			.unwrap()
			.retain(|other| !std::ptr::eq(&**other, camera));
		let mut pending = self.ivars().pending.lock().unwrap();
		if pending.as_ref() == Some(&id) {
			*pending = None;
		}
	}

	/// The studio started feeding `camera`.
	fn fed(&self, camera: &Camera) {
		let mut pending = self.ivars().pending.lock().unwrap();
		if pending.as_ref() == Some(&camera.ivars().config.device()) {
			*pending = None;
		}
	}

	/// The studio stopped feeding `camera`, having turned it off or quit: a
	/// scene's camera goes with it. The program's stays, so apps keep it.
	fn starved(&self, camera: &Camera) {
		if camera.ivars().config.scene.is_some() {
			self.remove(camera);
		}
	}

	/// Run `work` on the provider's queue, where CoreMediaIO calls every
	/// source, after the call in progress returns.
	fn later(&self, work: impl FnOnce(&Provider) + 'static) {
		let Some(provider) = self.provider() else { return };
		let queue = unsafe { provider.clientQueue() };
		let this = Queued((self.retain(), work));
		queue.exec_async(move || {
			let (this, work) = this.take();
			work(&this);
		});
	}
}

struct CameraIvars {
	config: Config,
	provider: Weak<Provider>,
	device: OnceLock<Retained<CMIOExtensionDevice>>,
	/// What apps read.
	source: OnceLock<(Retained<CMIOExtensionStream>, Retained<Stream>)>,
	/// What the studio feeds.
	sink: OnceLock<(Retained<CMIOExtensionStream>, Retained<Stream>)>,
	/// How many apps read the source stream.
	readers: AtomicUsize,
}

define_class!(
	#[unsafe(super(NSObject))]
	#[name = "MoqStudioCameraDevice"]
	#[ivars = CameraIvars]
	struct Camera;

	unsafe impl NSObjectProtocol for Camera {}

	unsafe impl CMIOExtensionDeviceSource for Camera {
		#[unsafe(method_id(availableProperties))]
		fn available_properties(&self) -> Retained<NSSet<NSString>> {
			let config = NSString::from_str(&CONFIG.name());
			let reading = NSString::from_str(&READING.name());
			set_of(&[
				unsafe { CMIOExtensionPropertyDeviceTransportType },
				unsafe { CMIOExtensionPropertyDeviceModel },
				&config,
				&reading,
			])
		}

		#[unsafe(method_id(devicePropertiesForProperties:error:))]
		fn device_properties(
			&self,
			properties: &NSSet<NSString>,
			_error: *mut *mut NSError,
		) -> Retained<CMIOExtensionDeviceProperties> {
			let result = unsafe { CMIOExtensionDeviceProperties::devicePropertiesWithDictionary(&NSDictionary::new()) };
			unsafe {
				if properties.containsObject(CMIOExtensionPropertyDeviceTransportType) {
					result.setTransportType(Some(&NSNumber::new_u32(VIRTUAL)));
				}
				if properties.containsObject(CMIOExtensionPropertyDeviceModel) {
					result.setModel(Some(ns_string!("kixel.live Studio")));
				}
			}
			let config = NSString::from_str(&CONFIG.name());
			if properties.containsObject(&config) {
				let value = NSString::from_str(&self.ivars().config.to_string());
				unsafe { result.setPropertyState_forProperty(Some(&state(&value)), &config) };
			}
			let reading = NSString::from_str(&READING.name());
			if properties.containsObject(&reading) {
				unsafe { result.setPropertyState_forProperty(Some(&state(&self.reading_value())), &reading) };
			}
			result
		}

		#[unsafe(method(setDeviceProperties:error:))]
		fn set_device_properties(&self, properties: &CMIOExtensionDeviceProperties, error: *mut *mut NSError) -> Bool {
			let key = NSString::from_str(&CONFIG.name());
			let Some(asked) = unsafe { properties.propertiesDictionary() }.objectForKey(&key) else {
				// The rest are read only.
				return Bool::YES;
			};
			let config = unsafe { asked.value() }
				.and_then(|value| value.downcast::<NSString>().ok())
				.ok_or_else(|| "a camera config is text".to_string())
				.and_then(|text| text.to_string().parse::<Config>().map_err(|err| err.to_string()))
				.and_then(|config| match self.ivars().config.scene {
					// Only the program's camera, which is always there, takes asks.
					None => Ok(config),
					Some(_) => Err("only the program's camera takes a camera config".to_string()),
				});
			match (config, self.ivars().provider.load()) {
				(Ok(config), Some(provider)) => {
					// Replacing this very camera from inside its own setter
					// would pull the device out from under the call.
					provider.later(move |provider| provider.ask(config));
					Bool::YES
				}
				(Err(err), _) => {
					fail(error, self::error(&err));
					Bool::NO
				}
				(_, None) => {
					fail(error, self::error("the provider is gone"));
					Bool::NO
				}
			}
		}
	}
);

impl Camera {
	/// A camera showing `config`, with its streams, not yet added.
	fn new(config: Config, provider: &Provider) -> Result<Retained<Self>, String> {
		let format = format(&config)?;
		let id = NSUUID::from_string(&NSString::from_str(&config.device())).ok_or("a bad device id")?;
		let name = NSString::from_str(&config.name());
		let framerate = config.framerate;
		let this = Self::alloc().set_ivars(CameraIvars {
			config,
			provider: Weak::from(provider),
			device: OnceLock::new(),
			source: OnceLock::new(),
			sink: OnceLock::new(),
			readers: AtomicUsize::new(0),
		});
		let this: Retained<Self> = unsafe { msg_send![super(this), init] };
		let device = unsafe {
			CMIOExtensionDevice::deviceWithLocalizedName_deviceID_source(&name, &id, ProtocolObject::from_ref(&*this))
		};
		for direction in [CMIOExtensionStreamDirection::Source, CMIOExtensionStreamDirection::Sink] {
			let sink = direction == CMIOExtensionStreamDirection::Sink;
			let source = Stream::new(&this, sink, format.clone(), framerate);
			let label = if sink {
				ns_string!("kixel.live Studio sink")
			} else {
				&name
			};
			let stream = unsafe {
				CMIOExtensionStream::streamWithLocalizedName_streamID_direction_clockType_source(
					label,
					&NSUUID::UUID(),
					direction,
					CMIOExtensionStreamClockType::HostTime,
					ProtocolObject::from_ref(&*source),
				)
			};
			unsafe { device.addStream_error(&stream) }.map_err(|err| err.to_string())?;
			let slot = if sink { &this.ivars().sink } else { &this.ivars().source };
			let _ = slot.set((stream, source));
		}
		let _ = this.ivars().device.set(device);
		Ok(this)
	}

	/// Whether the studio feeds the camera.
	fn fed(&self) -> bool {
		self.ivars()
			.sink
			.get()
			.is_some_and(|(_, stream)| stream.ivars().running.load(Ordering::Relaxed))
	}

	/// Whether an app reads the camera.
	fn reading(&self) -> bool {
		self.ivars().readers.load(Ordering::Relaxed) > 0
	}

	fn reading_value(&self) -> Retained<NSString> {
		NSString::from_str(if self.reading() { "1" } else { "0" })
	}

	/// Count an app starting to read the camera, or stopping, and say so
	/// when the first starts or the last stops, so the studio copies
	/// pictures only while any app reads.
	fn read(&self, starts: bool) {
		let readers = &self.ivars().readers;
		let changed = match starts {
			true => readers.fetch_add(1, Ordering::Relaxed) == 0,
			false => readers
				.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| count.checked_sub(1))
				.is_ok_and(|count| count == 1),
		};
		let Some(device) = self.ivars().device.get().filter(|_| changed) else {
			return;
		};
		let key = NSString::from_str(&READING.name());
		let changed = NSDictionary::from_slices(&[&*key], &[&*state(&self.reading_value())]);
		unsafe { device.notifyPropertiesChanged(&changed) };
	}
}

/// What apps read a camera as: 32-bit BGRA at its size and frame rate, the
/// format every app takes and the studio's RGBA becomes with a swizzle.
fn format(config: &Config) -> Result<Retained<CMIOExtensionStreamFormat>, String> {
	let mut description: *const CMFormatDescription = std::ptr::null();
	let status = unsafe {
		CMVideoFormatDescriptionCreate(
			None,
			kCVPixelFormatType_32BGRA,
			config.width as i32,
			config.height as i32,
			None,
			NonNull::from(&mut description),
		)
	};
	let description = NonNull::new(description.cast_mut())
		.filter(|_| status == 0)
		.ok_or_else(|| format!("cannot describe {config}: {status}"))?;
	// SAFETY: a create function hands over its reference.
	let description = unsafe { objc2_core_foundation::CFRetained::from_raw(description) };
	let duration = frame(config.framerate);
	Ok(unsafe {
		CMIOExtensionStreamFormat::streamFormatWithFormatDescription_maxFrameDuration_minFrameDuration_validFrameDurations(
			&description,
			duration,
			duration,
			None,
		)
	})
}

fn frame(framerate: u32) -> CMTime {
	unsafe { CMTime::new(1, framerate as i32) }
}

struct StreamIvars {
	camera: Weak<Camera>,
	/// Whether the studio feeds it, rather than apps reading it.
	sink: bool,
	format: Retained<CMIOExtensionStreamFormat>,
	framerate: u32,
	/// The studio, once it is allowed to feed the sink.
	client: Mutex<Option<Retained<CMIOExtensionClient>>>,
	/// Whether the sink takes pictures.
	running: AtomicBool,
}

define_class!(
	#[unsafe(super(NSObject))]
	#[name = "MoqStudioCameraStream"]
	#[ivars = StreamIvars]
	struct Stream;

	unsafe impl NSObjectProtocol for Stream {}

	unsafe impl CMIOExtensionStreamSource for Stream {
		#[unsafe(method_id(formats))]
		fn formats(&self) -> Retained<NSArray<CMIOExtensionStreamFormat>> {
			NSArray::from_retained_slice(&[self.ivars().format.clone()])
		}

		#[unsafe(method_id(availableProperties))]
		fn available_properties(&self) -> Retained<NSSet<NSString>> {
			unsafe {
				let mut properties = vec![
					CMIOExtensionPropertyStreamActiveFormatIndex,
					CMIOExtensionPropertyStreamFrameDuration,
				];
				if self.ivars().sink {
					properties.extend([
						CMIOExtensionPropertyStreamSinkBufferQueueSize,
						CMIOExtensionPropertyStreamSinkBuffersRequiredForStartup,
						CMIOExtensionPropertyStreamSinkBufferUnderrunCount,
						CMIOExtensionPropertyStreamSinkEndOfData,
					]);
				}
				set_of(&properties)
			}
		}

		#[unsafe(method_id(streamPropertiesForProperties:error:))]
		fn stream_properties(
			&self,
			properties: &NSSet<NSString>,
			_error: *mut *mut NSError,
		) -> Retained<CMIOExtensionStreamProperties> {
			let result = unsafe { CMIOExtensionStreamProperties::streamPropertiesWithDictionary(&NSDictionary::new()) };
			unsafe {
				if properties.containsObject(CMIOExtensionPropertyStreamActiveFormatIndex) {
					result.setActiveFormatIndex(Some(&NSNumber::new_u32(0)));
				}
				if properties.containsObject(CMIOExtensionPropertyStreamFrameDuration)
					&& let Some(duration) = frame(self.ivars().framerate).as_dictionary(None)
				{
					// Toll-free bridged.
					let duration: &NSDictionary = &*(&*duration as *const _ as *const NSDictionary);
					result.setFrameDuration(Some(duration));
				}
				if properties.containsObject(CMIOExtensionPropertyStreamSinkBufferQueueSize) {
					result.setSinkBufferQueueSize(Some(&NSNumber::new_u32(SINK_QUEUE)));
				}
				if properties.containsObject(CMIOExtensionPropertyStreamSinkBuffersRequiredForStartup) {
					result.setSinkBuffersRequiredForStartup(Some(&NSNumber::new_u32(1)));
				}
				if properties.containsObject(CMIOExtensionPropertyStreamSinkBufferUnderrunCount) {
					result.setSinkBufferUnderrunCount(Some(&NSNumber::new_u32(0)));
				}
				if properties.containsObject(CMIOExtensionPropertyStreamSinkEndOfData) {
					result.setSinkEndOfData(Some(&NSNumber::new_u32(0)));
				}
			}
			result
		}

		#[unsafe(method(setStreamProperties:error:))]
		fn set_stream_properties(
			&self,
			_properties: &CMIOExtensionStreamProperties,
			_error: *mut *mut NSError,
		) -> Bool {
			// One format at one frame rate: there is nothing else to pick.
			Bool::YES
		}

		#[unsafe(method(authorizedToStartStreamForClient:))]
		fn authorized(&self, client: &CMIOExtensionClient) -> Bool {
			if !self.ivars().sink {
				return Bool::YES;
			}
			// Only the studio feeds a camera, so no other app shows its
			// pictures under the studio's name.
			let studio = studio(unsafe { client.pid() });
			if studio {
				*self.ivars().client.lock().unwrap() = Some(client.retain());
			} else {
				log(&format!("refused to let pid {} feed a camera", unsafe { client.pid() }));
			}
			Bool::new(studio)
		}

		#[unsafe(method(startStreamAndReturnError:))]
		fn start_stream(&self, _error: *mut *mut NSError) -> Bool {
			let Some(camera) = self.ivars().camera.load() else {
				return Bool::YES;
			};
			if self.ivars().sink {
				self.ivars().running.store(true, Ordering::Relaxed);
				if let Some(provider) = camera.ivars().provider.load() {
					provider.fed(&camera);
				}
				self.consume();
			} else {
				camera.read(true);
			}
			Bool::YES
		}

		#[unsafe(method(stopStreamAndReturnError:))]
		fn stop_stream(&self, _error: *mut *mut NSError) -> Bool {
			let Some(camera) = self.ivars().camera.load() else {
				return Bool::YES;
			};
			if self.ivars().sink {
				self.ivars().running.store(false, Ordering::Relaxed);
				self.ivars().client.lock().unwrap().take();
				if let Some(provider) = camera.ivars().provider.load() {
					// Removing the camera stops this stream, so not from inside
					// its own stop.
					provider.later(move |provider| provider.starved(&camera));
				}
			} else {
				camera.read(false);
			}
			Bool::YES
		}
	}
);

impl Stream {
	fn new(camera: &Camera, sink: bool, format: Retained<CMIOExtensionStreamFormat>, framerate: u32) -> Retained<Self> {
		let this = Self::alloc().set_ivars(StreamIvars {
			camera: Weak::from(camera),
			sink,
			format,
			framerate,
			client: Mutex::new(None),
			running: AtomicBool::new(false),
		});
		unsafe { msg_send![super(this), init] }
	}

	/// Take the studio's next picture off the sink and show it on the
	/// source, then take the one after, until the sink stops.
	fn consume(&self) {
		if !self.ivars().running.load(Ordering::Relaxed) {
			return;
		}
		let Some(client) = self.ivars().client.lock().unwrap().clone() else {
			return;
		};
		let Some(camera) = self.ivars().camera.load() else {
			return;
		};
		let Some((sink, _)) = camera.ivars().sink.get() else {
			return;
		};
		let this = Weak::from(self);
		let framerate = self.ivars().framerate;
		let done = RcBlock::new(
			move |picture: *mut CMSampleBuffer,
			      sequence: u64,
			      discontinuity: CMIOExtensionStreamDiscontinuityFlags,
			      _more: Bool,
			      _error: *mut NSError| {
				let Some(this) = this.load() else { return };
				match unsafe { picture.as_ref() } {
					Some(picture) => {
						this.forward(picture, sequence, discontinuity);
						this.consume();
					}
					None => {
						// Nothing to take yet: look again a frame later
						// rather than spin.
						let this = Queued(this);
						let when = DispatchTime::NOW.time(1_000_000_000 / i64::from(framerate));
						let _ = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(
							DispatchQueueGlobalPriority::Default,
						))
						.after(when, move || {
							this.take().consume();
						});
					}
				}
			},
		);
		unsafe { sink.consumeSampleBufferFromClient_completionHandler(&client, &done) };
	}

	/// Show `picture`, sink sample `sequence`, to the apps reading the camera.
	fn forward(&self, picture: &CMSampleBuffer, sequence: u64, discontinuity: CMIOExtensionStreamDiscontinuityFlags) {
		let Some(camera) = self.ivars().camera.load() else {
			return;
		};
		let (Some((source, _)), Some((sink, _))) = (camera.ivars().source.get(), camera.ivars().sink.get()) else {
			return;
		};
		let now = now();
		if camera.reading() {
			unsafe { source.sendSampleBuffer_discontinuity_hostTimeInNanoseconds(picture, discontinuity, now) };
		}
		let output = unsafe {
			CMIOExtensionScheduledOutput::scheduledOutputWithSequenceNumber_hostTimeInNanoseconds(sequence, now)
		};
		unsafe { sink.notifyScheduledOutputChanged(&output) };
	}
}
