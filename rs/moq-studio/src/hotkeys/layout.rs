//! What the keyboard layout in use puts where each key sits, so a binding
//! registers the key the operator pressed on every layout. macOS registers a
//! key by where it sits, Windows by the virtual key the layout gives it, and
//! X11 by a keysym, which global-hotkey looks up in the layout itself.

use egui::Key;
use global_hotkey::hotkey::Code;

/// The keyboard layout in use when a binding is registered.
pub struct Layout {
	#[cfg(windows)]
	layout: windows_sys::Win32::UI::Input::KeyboardAndMouse::HKL,
	#[cfg(target_os = "linux")]
	keymap: Keymap,
}

impl Layout {
	/// The layout in use now, read on the window's thread.
	pub fn current() -> Result<Self, String> {
		#[cfg(windows)]
		{
			// SAFETY: reads the calling thread's layout handle, and takes no pointers.
			let layout = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0) };
			Ok(Self { layout })
		}
		#[cfg(target_os = "linux")]
		{
			Ok(Self {
				keymap: Keymap::current().map_err(|err| format!("cannot read the X11 keyboard layout: {err}"))?,
			})
		}
		#[cfg(not(any(windows, target_os = "linux")))]
		Ok(Self {})
	}

	/// The key the platform's hotkeys register for the key at `position`,
	/// named for the key a US keyboard has there, if they can hear it.
	pub fn code(&self, position: Key) -> Option<Code> {
		#[cfg(windows)]
		if let Some(scancode) = scancode(position) {
			use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW};
			// SAFETY: a lookup in the layout `GetKeyboardLayout` returned, with no pointers.
			let vk = unsafe { MapVirtualKeyExW(scancode, MAPVK_VSC_TO_VK_EX, self.layout) };
			return vk_code(vk);
		}
		#[cfg(target_os = "linux")]
		if scancode(position).is_some() {
			return self.keymap.code(position);
		}
		code(position)
	}
}

/// The PC scancode of a key in the block whose keys a layout relabels,
/// named for the key a US keyboard has there. Linux's evdev codes for these
/// are the same, and X11's keycodes are those plus 8.
#[cfg(any(windows, target_os = "linux", test))]
fn scancode(position: Key) -> Option<u32> {
	Some(match position {
		Key::Num1 => 0x02,
		Key::Num2 => 0x03,
		Key::Num3 => 0x04,
		Key::Num4 => 0x05,
		Key::Num5 => 0x06,
		Key::Num6 => 0x07,
		Key::Num7 => 0x08,
		Key::Num8 => 0x09,
		Key::Num9 => 0x0a,
		Key::Num0 => 0x0b,
		Key::Minus => 0x0c,
		Key::Equals => 0x0d,
		Key::Q => 0x10,
		Key::W => 0x11,
		Key::E => 0x12,
		Key::R => 0x13,
		Key::T => 0x14,
		Key::Y => 0x15,
		Key::U => 0x16,
		Key::I => 0x17,
		Key::O => 0x18,
		Key::P => 0x19,
		Key::OpenBracket => 0x1a,
		Key::CloseBracket => 0x1b,
		Key::A => 0x1e,
		Key::S => 0x1f,
		Key::D => 0x20,
		Key::F => 0x21,
		Key::G => 0x22,
		Key::H => 0x23,
		Key::J => 0x24,
		Key::K => 0x25,
		Key::L => 0x26,
		Key::Semicolon => 0x27,
		Key::Quote => 0x28,
		Key::Backtick => 0x29,
		Key::Backslash => 0x2b,
		Key::Z => 0x2c,
		Key::X => 0x2d,
		Key::C => 0x2e,
		Key::V => 0x2f,
		Key::B => 0x30,
		Key::N => 0x31,
		Key::M => 0x32,
		Key::Comma => 0x33,
		Key::Period => 0x34,
		Key::Slash => 0x35,
		_ => return None,
	})
}

/// The key global-hotkey registers as the Windows virtual key `vk`, for the
/// keys a layout puts in the block `scancode` covers.
#[cfg(any(windows, test))]
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

/// The keysyms the X server's keyboard mapping puts on each keycode.
#[cfg(any(target_os = "linux", test))]
struct Keymap {
	min_keycode: u32,
	per_keycode: usize,
	keysyms: Vec<u32>,
}

#[cfg(any(target_os = "linux", test))]
impl Keymap {
	#[cfg(target_os = "linux")]
	fn current() -> anyhow::Result<Self> {
		use x11rb::connection::Connection;
		use x11rb::protocol::xproto::ConnectionExt;

		let (conn, _) = x11rb::connect(None)?;
		let setup = conn.setup();
		let (min, max) = (setup.min_keycode, setup.max_keycode);
		let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
		Ok(Self {
			min_keycode: min.into(),
			per_keycode: mapping.keysyms_per_keycode.into(),
			keysyms: mapping.keysyms,
		})
	}

	/// The key global-hotkey grabs at `position`: one whose keysym, which
	/// global-hotkey looks up as the first keycode carrying it, is on this
	/// key at any shift level.
	fn code(&self, position: Key) -> Option<Code> {
		let keycode = scancode(position)? + 8;
		self.keysyms(keycode)?
			.iter()
			.filter_map(|keysym| KEYSYMS.iter().find(|(_, named)| named == keysym))
			.find(|(_, keysym)| self.first(*keysym) == Some(keycode))
			.map(|(code, _)| *code)
	}

	fn keysyms(&self, keycode: u32) -> Option<&[u32]> {
		let index = keycode.checked_sub(self.min_keycode)? as usize;
		self.keysyms.chunks(self.per_keycode).nth(index)
	}

	/// The first keycode carrying `keysym`, as global-hotkey looks one up.
	fn first(&self, keysym: u32) -> Option<u32> {
		let index = self
			.keysyms
			.chunks(self.per_keycode)
			.position(|keysyms| keysyms.contains(&keysym))?;
		Some(self.min_keycode + index as u32)
	}
}

/// The keysym global-hotkey 0.9 grabs for each key `scancode` covers.
#[cfg(any(target_os = "linux", test))]
const KEYSYMS: [(Code, u32); 47] = [
	(Code::KeyA, 0x41),
	(Code::KeyB, 0x42),
	(Code::KeyC, 0x43),
	(Code::KeyD, 0x44),
	(Code::KeyE, 0x45),
	(Code::KeyF, 0x46),
	(Code::KeyG, 0x47),
	(Code::KeyH, 0x48),
	(Code::KeyI, 0x49),
	(Code::KeyJ, 0x4a),
	(Code::KeyK, 0x4b),
	(Code::KeyL, 0x4c),
	(Code::KeyM, 0x4d),
	(Code::KeyN, 0x4e),
	(Code::KeyO, 0x4f),
	(Code::KeyP, 0x50),
	(Code::KeyQ, 0x51),
	(Code::KeyR, 0x52),
	(Code::KeyS, 0x53),
	(Code::KeyT, 0x54),
	(Code::KeyU, 0x55),
	(Code::KeyV, 0x56),
	(Code::KeyW, 0x57),
	(Code::KeyX, 0x58),
	(Code::KeyY, 0x59),
	(Code::KeyZ, 0x5a),
	(Code::Digit0, 0x30),
	(Code::Digit1, 0x31),
	(Code::Digit2, 0x32),
	(Code::Digit3, 0x33),
	(Code::Digit4, 0x34),
	(Code::Digit5, 0x35),
	(Code::Digit6, 0x36),
	(Code::Digit7, 0x37),
	(Code::Digit8, 0x38),
	(Code::Digit9, 0x39),
	(Code::Comma, 0x2c),
	(Code::Minus, 0x2d),
	(Code::Period, 0x2e),
	(Code::Slash, 0x2f),
	(Code::Semicolon, 0x3b),
	(Code::Equal, 0x3d),
	(Code::BracketLeft, 0x5b),
	(Code::Backslash, 0x5c),
	(Code::BracketRight, 0x5d),
	(Code::Backquote, 0x60),
	// `leftsinglequotemark`, not `apostrophe`, so no layout's apostrophe
	// key grabs.
	(Code::Quote, 0xad0),
];

/// `key` as global-hotkey names the key at its position on a US keyboard.
fn code(key: Key) -> Option<Code> {
	Some(match key {
		Key::ArrowDown => Code::ArrowDown,
		Key::ArrowLeft => Code::ArrowLeft,
		Key::ArrowRight => Code::ArrowRight,
		Key::ArrowUp => Code::ArrowUp,
		Key::Escape => Code::Escape,
		Key::Tab => Code::Tab,
		Key::Backspace => Code::Backspace,
		Key::Enter => Code::Enter,
		Key::Space => Code::Space,
		Key::Insert => Code::Insert,
		Key::Delete => Code::Delete,
		Key::Home => Code::Home,
		Key::End => Code::End,
		Key::PageUp => Code::PageUp,
		Key::PageDown => Code::PageDown,
		Key::Comma => Code::Comma,
		Key::Backslash => Code::Backslash,
		Key::Slash => Code::Slash,
		Key::OpenBracket => Code::BracketLeft,
		Key::CloseBracket => Code::BracketRight,
		Key::Backtick => Code::Backquote,
		Key::Minus => Code::Minus,
		Key::Period => Code::Period,
		Key::Equals => Code::Equal,
		Key::Semicolon => Code::Semicolon,
		Key::Quote => Code::Quote,
		Key::Num0 => Code::Digit0,
		Key::Num1 => Code::Digit1,
		Key::Num2 => Code::Digit2,
		Key::Num3 => Code::Digit3,
		Key::Num4 => Code::Digit4,
		Key::Num5 => Code::Digit5,
		Key::Num6 => Code::Digit6,
		Key::Num7 => Code::Digit7,
		Key::Num8 => Code::Digit8,
		Key::Num9 => Code::Digit9,
		Key::A => Code::KeyA,
		Key::B => Code::KeyB,
		Key::C => Code::KeyC,
		Key::D => Code::KeyD,
		Key::E => Code::KeyE,
		Key::F => Code::KeyF,
		Key::G => Code::KeyG,
		Key::H => Code::KeyH,
		Key::I => Code::KeyI,
		Key::J => Code::KeyJ,
		Key::K => Code::KeyK,
		Key::L => Code::KeyL,
		Key::M => Code::KeyM,
		Key::N => Code::KeyN,
		Key::O => Code::KeyO,
		Key::P => Code::KeyP,
		Key::Q => Code::KeyQ,
		Key::R => Code::KeyR,
		Key::S => Code::KeyS,
		Key::T => Code::KeyT,
		Key::U => Code::KeyU,
		Key::V => Code::KeyV,
		Key::W => Code::KeyW,
		Key::X => Code::KeyX,
		Key::Y => Code::KeyY,
		Key::Z => Code::KeyZ,
		Key::F1 => Code::F1,
		Key::F2 => Code::F2,
		Key::F3 => Code::F3,
		Key::F4 => Code::F4,
		Key::F5 => Code::F5,
		Key::F6 => Code::F6,
		Key::F7 => Code::F7,
		Key::F8 => Code::F8,
		Key::F9 => Code::F9,
		Key::F10 => Code::F10,
		Key::F11 => Code::F11,
		Key::F12 => Code::F12,
		Key::F13 => Code::F13,
		Key::F14 => Code::F14,
		Key::F15 => Code::F15,
		Key::F16 => Code::F16,
		Key::F17 => Code::F17,
		Key::F18 => Code::F18,
		Key::F19 => Code::F19,
		Key::F20 => Code::F20,
		Key::F21 => Code::F21,
		Key::F22 => Code::F22,
		Key::F23 => Code::F23,
		Key::F24 => Code::F24,
		_ => return None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Some of Microsoft's German layout (kbdgr.h): Z and Y swap, `#` is
	/// VK_OEM_2, and ö is VK_OEM_3. The digit row keeps US virtual keys.
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
		let windows = |position| vk_code(german_vk(scancode(position).unwrap()));
		// Shift+7 types `/` on German, which is Shift and VK_7, not VK_OEM_2.
		assert_eq!(windows(Key::Num7), Some(Code::Digit7));
		assert_eq!(windows(Key::Y), Some(Code::KeyZ));
		assert_eq!(windows(Key::Backslash), Some(Code::Slash));
		assert_eq!(windows(Key::Semicolon), Some(Code::Backquote));
		assert_eq!(vk_code(0xdf), None);
	}

	/// A keymap with two keysyms per keycode from 8, with `keys` on theirs.
	fn keymap(keys: &[(Key, [u32; 2])]) -> Keymap {
		let mut keysyms = vec![0; 2 * 0x40];
		for (position, syms) in keys {
			let index = scancode(*position).unwrap() as usize;
			keysyms[2 * index..2 * index + 2].copy_from_slice(syms);
		}
		Keymap {
			min_keycode: 8,
			per_keycode: 2,
			keysyms,
		}
	}

	#[test]
	fn x11_grabs_what_the_layout_puts_there() {
		let german = keymap(&[
			(Key::Num7, [0x37, 0x2f]),      // 7, slash
			(Key::Y, [0x7a, 0x5a]),         // z, Z
			(Key::Z, [0x79, 0x59]),         // y, Y
			(Key::Semicolon, [0xf6, 0xd6]), // odiaeresis, Odiaeresis
			(Key::Slash, [0x2d, 0x5f]),     // minus, underscore
		]);
		assert_eq!(german.code(Key::Num7), Some(Code::Digit7));
		assert_eq!(german.code(Key::Y), Some(Code::KeyZ));
		assert_eq!(german.code(Key::Slash), Some(Code::Minus));
		// No key global-hotkey names makes ö, so it is refused rather than
		// grabbing the key that makes `;`.
		assert_eq!(german.code(Key::Semicolon), None);
		// AZERTY's digit row makes `&` unshifted, and 1 with Shift.
		let french = keymap(&[(Key::Num1, [0x26, 0x31])]);
		assert_eq!(french.code(Key::Num1), Some(Code::Digit1));
		// A keysym an earlier key carries grabs that key, so this one is refused.
		let twice = keymap(&[(Key::Num1, [0x31, 0x21]), (Key::Num2, [0x31, 0x40])]);
		assert_eq!(twice.code(Key::Num2), None);
	}
}
