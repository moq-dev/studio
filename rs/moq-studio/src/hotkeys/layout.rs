//! The virtual key a Windows keyboard layout puts where each key sits.
//! `RegisterHotKey` matches the virtual key the foreground window's layout
//! gives the key pressed, so a binding registers through that layout, and
//! again whenever it changes.

use global_hotkey::hotkey::Code;
use winit::keyboard::KeyCode;

/// The foreground window's layout, which a task keeps watching.
#[cfg(windows)]
pub struct Foreground {
	/// The layout the task saw last, as its `HKL`.
	seen: std::sync::Arc<std::sync::atomic::AtomicIsize>,
	/// The layout the keys registered through.
	used: isize,
}

#[cfg(windows)]
impl Foreground {
	/// Watch the foreground window's layout, waking `ctx` when it changes so
	/// the keys register again. Windows says nothing to an app whose window
	/// is not the one changing layout, so it is read a few times a second.
	pub fn new(ctx: egui::Context, runtime: &tokio::runtime::Handle) -> Self {
		use std::sync::atomic::{AtomicIsize, Ordering};
		use std::sync::{Arc, Weak};

		let used = foreground();
		let seen = Arc::new(AtomicIsize::new(used));
		let watching: Weak<AtomicIsize> = Arc::downgrade(&seen);
		runtime.spawn(async move {
			let mut tick = kio::clock::interval(std::time::Duration::from_millis(250));
			loop {
				tick.tick().await;
				let Some(seen) = watching.upgrade() else {
					return;
				};
				let now = foreground();
				if seen.swap(now, Ordering::Relaxed) != now {
					ctx.request_repaint();
				}
			}
		});
		Self { seen, used }
	}

	/// Whether the foreground window's layout changed since the keys
	/// registered through it.
	pub fn moved(&self) -> bool {
		self.seen.load(std::sync::atomic::Ordering::Relaxed) != self.used
	}

	/// Register keys through the foreground window's layout from now on.
	pub fn follow(&mut self) {
		self.used = self.seen.load(std::sync::atomic::Ordering::Relaxed);
	}

	/// The key global-hotkey registers for `key`, one [`relabeled`] says the
	/// layout picks the virtual key of.
	pub fn code(&self, key: KeyCode) -> Option<Code> {
		use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW};
		use winit::platform::scancode::PhysicalKeyExtScancode;

		let scancode = key.to_scancode()?;
		// SAFETY: a lookup in a layout handle Windows gave, with no pointers.
		let vk = unsafe { MapVirtualKeyExW(scancode, MAPVK_VSC_TO_VK_EX, self.used as _) };
		vk_code(vk)
	}
}

/// The layout the foreground window types with, as its `HKL`.
#[cfg(windows)]
fn foreground() -> isize {
	use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
	use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

	// SAFETY: plain reads with no pointers kept; a null thread id, with no
	// foreground window, reads this thread's layout, which the system's is.
	unsafe {
		let thread = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
		GetKeyboardLayout(thread) as isize
	}
}

/// Whether the layout picks the virtual key of `key`: the keys that type
/// letters, digits, and punctuation.
pub fn relabeled(key: KeyCode) -> bool {
	matches!(
		key,
		KeyCode::Backquote
			| KeyCode::Backslash
			| KeyCode::BracketLeft
			| KeyCode::BracketRight
			| KeyCode::Comma
			| KeyCode::Digit0
			| KeyCode::Digit1
			| KeyCode::Digit2
			| KeyCode::Digit3
			| KeyCode::Digit4
			| KeyCode::Digit5
			| KeyCode::Digit6
			| KeyCode::Digit7
			| KeyCode::Digit8
			| KeyCode::Digit9
			| KeyCode::Equal
			| KeyCode::IntlBackslash
			| KeyCode::IntlRo
			| KeyCode::IntlYen
			| KeyCode::KeyA
			| KeyCode::KeyB
			| KeyCode::KeyC
			| KeyCode::KeyD
			| KeyCode::KeyE
			| KeyCode::KeyF
			| KeyCode::KeyG
			| KeyCode::KeyH
			| KeyCode::KeyI
			| KeyCode::KeyJ
			| KeyCode::KeyK
			| KeyCode::KeyL
			| KeyCode::KeyM
			| KeyCode::KeyN
			| KeyCode::KeyO
			| KeyCode::KeyP
			| KeyCode::KeyQ
			| KeyCode::KeyR
			| KeyCode::KeyS
			| KeyCode::KeyT
			| KeyCode::KeyU
			| KeyCode::KeyV
			| KeyCode::KeyW
			| KeyCode::KeyX
			| KeyCode::KeyY
			| KeyCode::KeyZ
			| KeyCode::Minus
			| KeyCode::Period
			| KeyCode::Quote
			| KeyCode::Semicolon
			| KeyCode::Slash
	)
}

/// The key global-hotkey registers as the Windows virtual key `vk`, for the
/// keys [`relabeled`] covers.
fn vk_code(vk: u32) -> Option<Code> {
	const LETTERS: [Code; 26] = [
		Code::KeyA,
		Code::KeyB,
		Code::KeyC,
		Code::KeyD,
		Code::KeyE,
		Code::KeyF,
		Code::KeyG,
		Code::KeyH,
		Code::KeyI,
		Code::KeyJ,
		Code::KeyK,
		Code::KeyL,
		Code::KeyM,
		Code::KeyN,
		Code::KeyO,
		Code::KeyP,
		Code::KeyQ,
		Code::KeyR,
		Code::KeyS,
		Code::KeyT,
		Code::KeyU,
		Code::KeyV,
		Code::KeyW,
		Code::KeyX,
		Code::KeyY,
		Code::KeyZ,
	];
	const DIGITS: [Code; 10] = [
		Code::Digit0,
		Code::Digit1,
		Code::Digit2,
		Code::Digit3,
		Code::Digit4,
		Code::Digit5,
		Code::Digit6,
		Code::Digit7,
		Code::Digit8,
		Code::Digit9,
	];
	Some(match vk {
		0x30..=0x39 => DIGITS[(vk - 0x30) as usize],
		0x41..=0x5a => LETTERS[(vk - 0x41) as usize],
		0xba => Code::Semicolon,     // VK_OEM_1
		0xbb => Code::Equal,         // VK_OEM_PLUS
		0xbc => Code::Comma,         // VK_OEM_COMMA
		0xbd => Code::Minus,         // VK_OEM_MINUS
		0xbe => Code::Period,        // VK_OEM_PERIOD
		0xbf => Code::Slash,         // VK_OEM_2
		0xc0 => Code::Backquote,     // VK_OEM_3
		0xdb => Code::BracketLeft,   // VK_OEM_4
		0xdc => Code::Backslash,     // VK_OEM_5
		0xdd => Code::BracketRight,  // VK_OEM_6
		0xde => Code::Quote,         // VK_OEM_7
		0xe2 => Code::IntlBackslash, // VK_OEM_102
		_ => return None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Some of Microsoft's German layout (kbdgr.h), by scancode: Z and Y
	/// swap, `#` is VK_OEM_2, and ö is VK_OEM_3. The digit row keeps US
	/// virtual keys.
	fn german_vk(scancode: u32) -> u32 {
		match scancode {
			0x08 => 0x37,
			0x15 => 0x5a,
			0x2c => 0x59,
			0x2b => 0xbf,
			0x27 => 0xc0,
			other => panic!("no German virtual key for {other:#x} in this test"),
		}
	}

	#[test]
	fn windows_registers_what_the_layout_puts_there() {
		let windows = |scancode| vk_code(german_vk(scancode));
		// Shift+7 types `/` on German, which is Shift and VK_7, not VK_OEM_2.
		assert_eq!(windows(0x08), Some(Code::Digit7));
		// The key a US keyboard has Y on.
		assert_eq!(windows(0x15), Some(Code::KeyZ));
		assert_eq!(windows(0x2b), Some(Code::Slash));
		assert_eq!(windows(0x27), Some(Code::Backquote));
		assert_eq!(vk_code(0xdf), None);
		// The keypad keeps its virtual keys whatever the layout.
		assert!(relabeled(KeyCode::Digit1));
		assert!(!relabeled(KeyCode::Numpad1));
	}
}
