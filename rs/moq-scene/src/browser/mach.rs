//! A page's shared texture on macOS: an IOSurface, which crosses processes
//! only as a Mach port, and a socket carries no Mach ports. So the studio
//! checks in a bootstrap service of its own, the helper looks it up, and
//! sends each picture's surface there just before the socket message that
//! describes it. The studio takes it once that message arrives, and only from
//! its helper.

use std::ffi::CString;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

use mach2::bootstrap::{bootstrap_check_in, bootstrap_look_up, bootstrap_port, bootstrap_strerror};
use mach2::kern_return::{KERN_SUCCESS, kern_return_t};
use mach2::mach_port::{mach_port_deallocate, mach_port_mod_refs};
use mach2::message::{
	MACH_MSG_PORT_DESCRIPTOR, MACH_MSG_SUCCESS, MACH_MSG_TRAILER_FORMAT_0, MACH_MSG_TYPE_COPY_SEND,
	MACH_MSG_TYPE_MOVE_SEND, MACH_MSGH_BITS, MACH_MSGH_BITS_COMPLEX, MACH_RCV_MSG, MACH_RCV_TIMED_OUT,
	MACH_RCV_TIMEOUT, MACH_RCV_TOO_LARGE, MACH_RCV_TRAILER_AUDIT, MACH_SEND_INTERRUPTED, MACH_SEND_MSG,
	MACH_SEND_TIMED_OUT, MACH_SEND_TIMEOUT, mach_msg, mach_msg_audit_trailer_t, mach_msg_body_t, mach_msg_destroy,
	mach_msg_header_t, mach_msg_id_t, mach_msg_option_t, mach_msg_port_descriptor_t,
};
use mach2::port::{MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE, mach_port_t};
use mach2::traps::mach_task_self;
use objc2_core_foundation::CFRetained;
use objc2_io_surface::IOSurfaceRef;

/// The id of the one message there is: a picture's surface.
const SURFACE: mach_msg_id_t = i32::from_be_bytes(*b"moqs");

/// The most messages from other processes the studio passes over looking for
/// its helper's. Anyone may send to a bootstrap service; the queue holds only
/// a few, and the helper's always arrives before the studio looks.
const STRANGERS: usize = 16;

/// A picture's surface, as sent.
#[repr(C)]
struct Message {
	header: mach_msg_header_t,
	body: mach_msg_body_t,
	surface: mach_msg_port_descriptor_t,
}

/// A picture's surface, as received, with who sent it.
#[repr(C)]
struct Received {
	message: Message,
	trailer: mach_msg_audit_trailer_t,
}

/// The studio's end: the bootstrap service its helper sends surfaces to.
pub(super) struct Receiver {
	port: mach_port_t,
	name: String,
}

impl Receiver {
	/// Check in a service unique to this studio and this helper.
	pub fn new() -> Result<Self, String> {
		static NEXT: AtomicU64 = AtomicU64::new(0);
		let name = format!(
			"dev.moq.scene.browser.{}.{}",
			std::process::id(),
			NEXT.fetch_add(1, Ordering::Relaxed)
		);
		let c_name = CString::new(name.as_str()).expect("no NUL in the name");
		let mut port = MACH_PORT_NULL;
		// SAFETY: `port` receives a receive right this process then owns.
		let kr = unsafe { bootstrap_check_in(bootstrap_port, c_name.as_ptr(), &mut port) };
		if kr != KERN_SUCCESS {
			return Err(format!(
				"cannot check in the bootstrap service {name}: {}",
				bootstrap_error(kr)
			));
		}
		Ok(Self { port, name })
	}

	/// The service's name, which the helper looks up.
	pub fn name(&self) -> &str {
		&self.name
	}

	/// The surface the helper, process `pid`, sent ahead of the picture
	/// whose message just arrived. Anyone else's messages are dropped.
	pub fn surface(&self, pid: u32) -> Result<CFRetained<IOSurfaceRef>, String> {
		for _ in 0..STRANGERS {
			// SAFETY: all zeroes is a valid message and trailer.
			let mut received: Received = unsafe { std::mem::zeroed() };
			let options = MACH_RCV_MSG
				| MACH_RCV_TIMEOUT
				| trailer_type(MACH_MSG_TRAILER_FORMAT_0)
				| trailer_elements(MACH_RCV_TRAILER_AUDIT);
			// SAFETY: the buffer is `Received`'s size, and a zero timeout
			// returns at once.
			let kr = unsafe {
				mach_msg(
					&mut received.message.header,
					options,
					0,
					size_of::<Received>() as u32,
					self.port,
					0,
					MACH_PORT_NULL,
				)
			};
			match kr {
				MACH_MSG_SUCCESS => {}
				MACH_RCV_TIMED_OUT => return Err("the page's texture never arrived".into()),
				// Not the helper's, which fits: the kernel dropped it.
				MACH_RCV_TOO_LARGE => continue,
				kr => return Err(format!("cannot receive the page's texture: {kr:#x}")),
			}
			// The pid in an audit token, which the kernel fills in.
			if received.trailer.msgh_audit.val[5] != pid {
				// SAFETY: the message was received, so its rights are ours.
				unsafe { mach_msg_destroy(&mut received.message.header) };
				continue;
			}
			let message = &received.message;
			let complete = message.header.msgh_bits & MACH_MSGH_BITS_COMPLEX != 0
				&& message.header.msgh_size as usize == size_of::<Message>()
				&& message.header.msgh_id == SURFACE
				&& message.body.msgh_descriptor_count == 1
				&& u32::from(message.surface.type_) == MACH_MSG_PORT_DESCRIPTOR
				&& u32::from(message.surface.disposition) == MACH_MSG_TYPE_MOVE_SEND;
			if !complete {
				// SAFETY: as above.
				unsafe { mach_msg_destroy(&mut received.message.header) };
				return Err("the browser process sent a malformed texture".into());
			}
			let port = message.surface.name;
			let surface = IOSurfaceRef::lookup_from_mach_port(port);
			// SAFETY: the send right came with the message; the surface, if
			// any, holds its own reference.
			unsafe { mach_port_deallocate(mach_task_self(), port) };
			return surface.ok_or_else(|| "the page's texture is not a surface".into());
		}
		Err("the page's texture never arrived among others' messages".into())
	}
}

impl Drop for Receiver {
	fn drop(&mut self) {
		// SAFETY: the receive right is ours; destroying it ends the service
		// and drops whatever it still holds.
		unsafe { mach_port_mod_refs(mach_task_self(), self.port, MACH_PORT_RIGHT_RECEIVE, -1) };
	}
}

/// The helper's end: a send right to the studio's service.
#[cfg_attr(
	not(any(test, feature = "cef")),
	expect(dead_code, reason = "only a helper sends, which takes CEF")
)]
pub(super) struct Sender {
	port: mach_port_t,
}

#[cfg_attr(
	not(any(test, feature = "cef")),
	expect(dead_code, reason = "only a helper sends, which takes CEF")
)]
impl Sender {
	/// Look up the studio's service `name`.
	pub fn look_up(name: &str) -> Result<Self, String> {
		let c_name = CString::new(name).map_err(|_| format!("the bootstrap service {name:?} has a NUL"))?;
		let mut port = MACH_PORT_NULL;
		// SAFETY: `port` receives a send right this process then owns.
		let kr = unsafe { bootstrap_look_up(bootstrap_port, c_name.as_ptr(), &mut port) };
		if kr != KERN_SUCCESS {
			return Err(format!(
				"cannot find the studio's bootstrap service {name}: {}",
				bootstrap_error(kr)
			));
		}
		Ok(Self { port })
	}

	/// Send `surface` to the studio, which takes it with the picture's
	/// message, sent next.
	pub fn send(&self, surface: &IOSurfaceRef) -> Result<(), String> {
		let port = surface.create_mach_port();
		if port == MACH_PORT_NULL {
			return Err("cannot share the page's texture".into());
		}
		let mut message = Message {
			header: mach_msg_header_t {
				msgh_bits: MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX,
				msgh_size: size_of::<Message>() as u32,
				msgh_remote_port: self.port,
				msgh_local_port: MACH_PORT_NULL,
				msgh_voucher_port: MACH_PORT_NULL,
				msgh_id: SURFACE,
			},
			body: mach_msg_body_t {
				msgh_descriptor_count: 1,
			},
			surface: mach_msg_port_descriptor_t::new(port, MACH_MSG_TYPE_MOVE_SEND),
		};
		// Never waits: the studio takes each surface before the helper sends
		// the next, so only others' messages can fill its queue.
		// SAFETY: the message is complete, and its size is its own.
		let kr = unsafe {
			mach_msg(
				&mut message.header,
				MACH_SEND_MSG | MACH_SEND_TIMEOUT,
				size_of::<Message>() as u32,
				0,
				MACH_PORT_NULL,
				0,
				MACH_PORT_NULL,
			)
		};
		if kr == MACH_MSG_SUCCESS {
			return Ok(());
		}
		// A failure after the kernel took the rights hands them back in the
		// message; one before leaves the surface's right with this process.
		// SAFETY: either way, each right released is one this process holds.
		unsafe {
			match kr {
				MACH_SEND_TIMED_OUT | MACH_SEND_INTERRUPTED => mach_msg_destroy(&mut message.header),
				_ => _ = mach_port_deallocate(mach_task_self(), port),
			}
		}
		Err(format!("cannot send the page's texture to the studio: {kr:#x}"))
	}
}

impl Drop for Sender {
	fn drop(&mut self) {
		// SAFETY: the send right is ours.
		unsafe { mach_port_deallocate(mach_task_self(), self.port) };
	}
}

/// `MACH_RCV_TRAILER_TYPE`, which mach2 lacks.
fn trailer_type(kind: u32) -> mach_msg_option_t {
	((kind & 0xf) << 28) as mach_msg_option_t
}

/// `MACH_RCV_TRAILER_ELEMENTS`, which mach2 lacks.
fn trailer_elements(elements: u32) -> mach_msg_option_t {
	((elements & 0xf) << 24) as mach_msg_option_t
}

fn bootstrap_error(kr: kern_return_t) -> String {
	// SAFETY: bootstrap_strerror returns a static string for any code.
	unsafe { std::ffi::CStr::from_ptr(bootstrap_strerror(kr)) }
		.to_string_lossy()
		.into_owned()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A surface sent through the service arrives as the same surface, and a
	/// receiver waiting on another process's surface takes none.
	#[test]
	fn a_surface_crosses_the_service() {
		use objc2_core_foundation::{CFDictionary, CFNumber, CFString};

		let receiver = Receiver::new().unwrap();
		let sender = Sender::look_up(receiver.name()).unwrap();
		let number = |value: i32| CFNumber::new_i32(value);
		let keys = [
			CFString::from_static_str("IOSurfaceWidth"),
			CFString::from_static_str("IOSurfaceHeight"),
			CFString::from_static_str("IOSurfaceBytesPerElement"),
			CFString::from_static_str("IOSurfacePixelFormat"),
		];
		let values = [number(64), number(32), number(4), number(i32::from_be_bytes(*b"BGRA"))];
		let properties = CFDictionary::from_slices(
			&keys.each_ref().map(|key| &**key),
			&values.each_ref().map(|value| &**value),
		);
		// SAFETY: the dictionary holds the properties IOSurfaceCreate reads.
		let surface = unsafe { IOSurfaceRef::new(properties.as_opaque()) }.expect("an IOSurface");

		sender.send(&surface).unwrap();
		assert!(
			receiver.surface(std::process::id() + 1).is_err(),
			"taken from another process"
		);

		sender.send(&surface).unwrap();
		let received = receiver.surface(std::process::id()).unwrap();
		assert_eq!(received.id(), surface.id(), "the same surface");
		assert_eq!((received.width(), received.height()), (64, 32));
		assert!(receiver.surface(std::process::id()).is_err(), "only one was sent");
	}
}
