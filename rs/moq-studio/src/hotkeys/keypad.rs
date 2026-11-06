//! The keypad keys egui-winit 0.36 reports as their main keyboard twins
//! (`Numpad1` as `Key::Num1`), so binding one would register the other. The
//! window's event loop notes them before egui hears them, and each frame
//! forgets where they sit, so they are neither bound nor matched.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use egui::Key;
use winit::keyboard::{KeyCode, PhysicalKey};

/// The twins of the keypad keys pressed or released since the last frame.
#[derive(Clone, Default)]
pub struct Keypad(Arc<Mutex<BTreeSet<Key>>>);

impl Keypad {
	/// Note a key the window heard, before egui hears it.
	pub fn note(&self, key: PhysicalKey) {
		if let Some(twin) = twin(key) {
			self.0.lock().unwrap().insert(twin);
		}
	}

	/// Forget where the keypad keys noted since the last frame sit, in that
	/// frame's `events`. A twin pressed in the same frame is forgotten too,
	/// which refuses it rather than binding the wrong key.
	pub fn forget(&self, events: &mut [egui::Event]) {
		let keypad = std::mem::take(&mut *self.0.lock().unwrap());
		for event in events {
			if let egui::Event::Key { physical_key, .. } = event
				&& physical_key.is_some_and(|key| keypad.contains(&key))
			{
				*physical_key = None;
			}
		}
	}
}

/// The key egui-winit reports for the keypad key `key`, where it folds one.
fn twin(key: PhysicalKey) -> Option<Key> {
	let PhysicalKey::Code(code) = key else {
		return None;
	};
	Some(match code {
		KeyCode::Numpad0 => Key::Num0,
		KeyCode::Numpad1 => Key::Num1,
		KeyCode::Numpad2 => Key::Num2,
		KeyCode::Numpad3 => Key::Num3,
		KeyCode::Numpad4 => Key::Num4,
		KeyCode::Numpad5 => Key::Num5,
		KeyCode::Numpad6 => Key::Num6,
		KeyCode::Numpad7 => Key::Num7,
		KeyCode::Numpad8 => Key::Num8,
		KeyCode::Numpad9 => Key::Num9,
		KeyCode::NumpadEnter => Key::Enter,
		KeyCode::NumpadDivide => Key::Slash,
		KeyCode::NumpadSubtract => Key::Minus,
		KeyCode::NumpadAdd => Key::Plus,
		_ => return None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keypad_keys_are_not_bound_as_their_twins() {
		// egui reports keypad 1 and the top row's 1 alike, with NumLock on.
		let press = || {
			vec![egui::Event::Key {
				key: Key::Num1,
				physical_key: Some(Key::Num1),
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::CTRL,
			}]
		};
		let keypad = Keypad::default();

		keypad.note(PhysicalKey::Code(KeyCode::Numpad1));
		let mut events = press();
		keypad.forget(&mut events);
		assert_eq!(crate::hotkeys::pressed(&events[0]), None);

		// The next frame's top-row 1 binds where it sits.
		keypad.note(PhysicalKey::Code(KeyCode::Digit1));
		let mut events = press();
		keypad.forget(&mut events);
		let bound = crate::hotkeys::pressed(&events[0]).unwrap();
		assert_eq!(bound.position, Key::Num1);
	}
}
