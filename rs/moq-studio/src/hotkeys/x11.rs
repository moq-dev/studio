//! Hotkeys on X11, grabbed from the X server by keycode: the key where it
//! sits, so any key binds, one winit cannot name included, and a layout
//! change moves none.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use winit::keyboard::PhysicalKey;
use winit::platform::scancode::PhysicalKeyExtScancode;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask, Window};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

use super::Binding;

/// A grabbed key: its keycode, and the modifiers held with it.
type Grab = (u8, ModMask);

/// The keys grabbed from the X server, heard on a thread of its own.
pub struct X11 {
	conn: Arc<RustConnection>,
	root: Window,
	/// The binding each grabbed key fires, which the thread hearing them reads.
	grabbed: Arc<Mutex<HashMap<Grab, Binding>>>,
}

impl X11 {
	/// Hear grabbed keys from now on, sending each pressed to `fire`, from a
	/// thread that waits on the X server for as long as the studio runs.
	pub fn new(fire: mpsc::Sender<Binding>) -> anyhow::Result<Self> {
		let (conn, screen) = x11rb::connect(None).context("cannot reach the X server")?;
		let xkb = conn.xkb_use_extension(1, 0)?.reply()?;
		anyhow::ensure!(xkb.supported, "the X server has no XKB");
		// A held key repeats as presses alone, rather than a release and a
		// press each, so it fires once.
		conn.xkb_per_client_flags(
			xkb::ID::USE_CORE_KBD.into(),
			xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
			xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
			Default::default(),
			Default::default(),
			Default::default(),
		)?
		.reply()?;
		let root = conn.setup().roots[screen].root;
		let conn = Arc::new(conn);
		let grabbed: Arc<Mutex<HashMap<Grab, Binding>>> = Arc::default();
		let (waiting, reading) = (conn.clone(), grabbed.clone());
		std::thread::spawn(move || {
			let mut held = HashSet::new();
			loop {
				let event = match waiting.wait_for_event() {
					Ok(event) => event,
					Err(err) => {
						tracing::warn!(error = %err, "lost the X server; hotkeys no longer fire");
						return;
					}
				};
				match event {
					Event::KeyPress(press) => {
						if !held.insert(press.detail) {
							continue;
						}
						let held = ModMask::from(u16::from(press.state)) & modifiers();
						if let Some(key) = reading.lock().unwrap().get(&(press.detail, held)) {
							let _ = fire.send(key.clone());
						}
					}
					Event::KeyRelease(release) => {
						held.remove(&release.detail);
					}
					_ => {}
				}
			}
		});
		Ok(Self { conn, root, grabbed })
	}

	/// Grab `keys` in place of the keys grabbed now, returning why the X
	/// server refused each it would not let go of.
	pub fn hear(&mut self, keys: &[Binding]) -> Vec<(Binding, String)> {
		let was = std::mem::take(&mut *self.grabbed.lock().unwrap());
		for (keycode, held) in was.keys() {
			self.ungrab(*keycode, *held, &locks());
		}
		let mut refused = Vec::new();
		let mut grabbed = HashMap::new();
		for key in keys {
			match self.grab(key) {
				Ok(grab) => drop(grabbed.insert(grab, key.clone())),
				Err(why) => refused.push((key.clone(), why)),
			}
		}
		if let Err(err) = self.conn.flush() {
			tracing::warn!(error = %err, "cannot reach the X server");
		}
		*self.grabbed.lock().unwrap() = grabbed;
		refused
	}

	/// Grab `key` with every mix of the locks, or none of them.
	fn grab(&self, key: &Binding) -> Result<Grab, String> {
		let setup = self.conn.setup();
		let keycode = keycode(key.key)
			.filter(|keycode| (setup.min_keycode..=setup.max_keycode).contains(keycode))
			.ok_or("the X server has no keycode for this key")?;
		let held = mask(key.modifiers);
		let locks = locks();
		for (taken, lock) in locks.iter().enumerate() {
			let grabbed = self
				.conn
				.grab_key(
					false,
					self.root,
					held | *lock,
					keycode,
					GrabMode::ASYNC,
					GrabMode::ASYNC,
				)
				.map_err(ReplyError::from)
				.and_then(|cookie| cookie.check());
			if let Err(err) = grabbed {
				self.ungrab(keycode, held, &locks[..taken]);
				return Err(match err {
					ReplyError::X11Error(err) if err.error_kind == ErrorKind::Access => {
						"another app holds this key".into()
					}
					err => format!("the X server refused this key: {err}"),
				});
			}
		}
		Ok((keycode, held))
	}

	fn ungrab(&self, keycode: u8, held: ModMask, locks: &[ModMask]) {
		for lock in locks {
			if let Ok(cookie) = self.conn.ungrab_key(keycode, self.root, held | *lock) {
				cookie.ignore_error();
			}
		}
	}
}

/// The modifiers a binding can hold, which a grab matches exactly.
fn modifiers() -> ModMask {
	ModMask::SHIFT | ModMask::CONTROL | ModMask::M1 | ModMask::M4
}

/// NumLock (Mod2) and CapsLock, which X11 counts as held modifiers, so each
/// key is grabbed with every mix of them.
fn locks() -> [ModMask; 4] {
	[
		ModMask::from(0u16),
		ModMask::M2,
		ModMask::LOCK,
		ModMask::M2 | ModMask::LOCK,
	]
}

/// The X11 keycode of `key`, which is its Linux scancode plus 8, for a key
/// winit names and one it leaves unidentified alike.
fn keycode(key: PhysicalKey) -> Option<u8> {
	u8::try_from(key.to_scancode()? + 8).ok()
}

/// `modifiers` as the X11 modifier mask they hold.
fn mask(modifiers: egui::Modifiers) -> ModMask {
	let mut mask = ModMask::from(0u16);
	for (on, modifier) in [
		(modifiers.shift, ModMask::SHIFT),
		(modifiers.ctrl, ModMask::CONTROL),
		(modifiers.alt, ModMask::M1),
		(modifiers.mac_cmd, ModMask::M4),
	] {
		if on {
			mask = mask | modifier;
		}
	}
	mask
}

#[cfg(test)]
mod tests {
	use winit::keyboard::{KeyCode, NativeKeyCode};

	use super::*;

	#[test]
	fn keys_grab_by_keycode() {
		// evdev's KEY_1 and KEY_KP1, plus 8.
		assert_eq!(keycode(PhysicalKey::Code(KeyCode::Digit1)), Some(10));
		assert_eq!(keycode(PhysicalKey::Code(KeyCode::Numpad1)), Some(87));
		assert_eq!(keycode(PhysicalKey::Code(KeyCode::NumpadEnter)), Some(104));
		// The key a US keyboard has `;` on, which types ö on German.
		assert_eq!(keycode(PhysicalKey::Code(KeyCode::Semicolon)), Some(47));
		assert_eq!(keycode(PhysicalKey::Unidentified(NativeKeyCode::Xkb(200))), Some(208));
		assert_eq!(keycode(PhysicalKey::Unidentified(NativeKeyCode::Xkb(300))), None);
		let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
		assert_eq!(mask(ctrl_shift), ModMask::CONTROL | ModMask::SHIFT);
	}
}
