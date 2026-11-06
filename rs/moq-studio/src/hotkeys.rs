//! Keys the operator binds to studio actions, heard system-wide, so a replay
//! fires from inside a game: through the platform's global hotkeys on macOS,
//! Windows, and X11, and the desktop's GlobalShortcuts portal on Wayland.
//! Where neither is there, they work only while the studio has focus.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use egui::{Key, KeyboardShortcut};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::controls::{Action, Controls};

#[cfg(target_os = "linux")]
mod portal;

/// The key bound to each action. Every action starts unbound, so a key
/// another app, such as a game, uses is never taken unasked.
pub type Bindings = BTreeMap<Action, KeyboardShortcut>;

/// The bound keys, heard wherever the platform lets the studio hear them.
pub struct Hotkeys {
	bindings: Bindings,
	backend: Backend,
	/// Why the platform refused each binding it would not register.
	refused: BTreeMap<Action, String>,
	/// Whether bindings are held off while the Hotkeys panel waits for a key,
	/// which the thread firing them reads too.
	suspended: Arc<AtomicBool>,
	fire: mpsc::Sender<Action>,
}

/// Where bound keys are heard.
enum Backend {
	/// The platform's global hotkeys: `RegisterEventHotKey` on macOS,
	/// `RegisterHotKey` on Windows, and a key grab on X11.
	Native {
		manager: GlobalHotKeyManager,
		/// The hotkeys registered now.
		held: Vec<HotKey>,
		/// The action each registered hotkey fires, by its id, which the
		/// platform's event handler reads.
		registered: Arc<Mutex<HashMap<u32, Action>>>,
	},
	/// The desktop's GlobalShortcuts portal, on Wayland.
	#[cfg(target_os = "linux")]
	Portal(portal::Portal),
	/// Only while the studio has focus, and why.
	Focused(String),
}

/// How far bound keys reach, as the Hotkeys panel says.
#[derive(Clone)]
pub enum Reach {
	/// Every app, through the platform.
	Global,
	/// Every app, through the desktop's shortcut settings, which may have
	/// bound different keys than asked: those it bound, by action.
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "only Wayland's portal binds through the desktop")
	)]
	Desktop(BTreeMap<Action, String>),
	/// Waiting on the desktop to bind them.
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "only Wayland's portal binds through the desktop")
	)]
	Asking,
	/// Only while the studio has focus, and why.
	Focused(String),
}

impl Hotkeys {
	/// Hear `bindings` from now on, firing each action on `controls` from a
	/// thread of its own. Called on the window's thread, which the platform's
	/// hotkeys must be registered from.
	pub fn new(
		window: &eframe::Frame,
		controls: Arc<Controls>,
		runtime: &tokio::runtime::Handle,
		bindings: Bindings,
	) -> Self {
		let (fire, fired) = mpsc::channel::<Action>();
		let suspended = Arc::new(AtomicBool::new(false));
		let held_off = suspended.clone();
		std::thread::spawn(move || {
			for action in fired {
				// A key the desktop holds on Wayland still fires while the panel
				// waits for one, and the panel never sees it.
				if held_off.load(Ordering::Relaxed) {
					continue;
				}
				let result = controls.fire(&action);
				controls.report(&action, result);
			}
		});
		let backend = Backend::new(window, runtime, fire.clone());
		let mut hotkeys = Self {
			bindings: Bindings::new(),
			backend,
			refused: BTreeMap::new(),
			suspended,
			fire,
		};
		hotkeys.set(bindings);
		hotkeys
	}

	pub fn bindings(&self) -> &Bindings {
		&self.bindings
	}

	/// Bind `action` to `key`, or unbind it, unless `key` is bound to another
	/// action, which is returned instead.
	pub fn bind(&mut self, action: &Action, key: Option<KeyboardShortcut>) -> Result<(), Action> {
		if let Some(key) = key
			&& let Some((other, _)) = self
				.bindings
				.iter()
				.find(|(other, bound)| **bound == key && *other != action)
		{
			return Err(other.clone());
		}
		let mut bindings = self.bindings.clone();
		match key {
			Some(key) => bindings.insert(action.clone(), key),
			None => bindings.remove(action),
		};
		self.set(bindings);
		Ok(())
	}

	/// Move each binding to a scene `renames` renamed, all at once, so a swap
	/// of two names swaps their keys. A removed scene keeps its key, which an
	/// undo may bring back.
	pub fn rename(&mut self, renames: &BTreeMap<String, Option<String>>) {
		let bindings = renamed(&self.bindings, renames);
		if bindings != self.bindings {
			self.set(bindings);
		}
	}

	/// Hold every binding off while the Hotkeys panel waits for a key, so the
	/// key pressed binds rather than fires; or take them up again.
	pub fn suspend(&mut self, suspended: bool) {
		if self.suspended.swap(suspended, Ordering::Relaxed) != suspended {
			self.set(self.bindings.clone());
		}
	}

	/// Hear `bindings` instead of the ones heard now.
	fn set(&mut self, bindings: Bindings) {
		self.refused.clear();
		let heard = match self.suspended.load(Ordering::Relaxed) {
			true => &Bindings::new(),
			false => &bindings,
		};
		match &mut self.backend {
			Backend::Native {
				manager,
				held,
				registered,
			} => {
				// `registered` is not locked while the manager works: on X11 it
				// waits on the thread that runs the event handler, which locks it.
				for hotkey in held.drain(..) {
					if let Err(err) = manager.unregister(hotkey) {
						tracing::warn!(%hotkey, error = %err, "cannot unregister a hotkey");
					}
				}
				let mut ids = HashMap::new();
				for (action, key) in heard {
					let Some(hotkey) = hotkey(key) else {
						let reason = "this key cannot be heard outside the studio".to_string();
						self.refused.insert(action.clone(), reason);
						continue;
					};
					match manager.register(hotkey) {
						Ok(()) => {
							held.push(hotkey);
							ids.insert(hotkey.id(), action.clone());
						}
						Err(err) => drop(self.refused.insert(action.clone(), refusal(err))),
					}
				}
				*registered.lock().unwrap() = ids;
			}
			// Held off, the desktop would be asked to bind them all again
			// after each key the panel takes.
			#[cfg(target_os = "linux")]
			Backend::Portal(portal) => portal.set(bindings.clone()),
			Backend::Focused(_) => {}
		}
		self.bindings = bindings;
	}

	/// Why the platform would not hear `action`'s key, if it refused it.
	pub fn refused(&self, action: &Action) -> Option<&str> {
		self.refused.get(action).map(String::as_str)
	}

	pub fn reach(&self) -> Reach {
		match &self.backend {
			Backend::Native { .. } => Reach::Global,
			#[cfg(target_os = "linux")]
			Backend::Portal(portal) => portal.reach(),
			Backend::Focused(reason) => Reach::Focused(reason.clone()),
		}
	}

	/// Fire the actions bound to keys pressed in the studio's window, where
	/// nothing hears them outside it. A text field being typed in keeps its
	/// keys, as the caller makes a page the preview passes the keyboard to.
	/// A key that fires is taken, so nothing else in the window acts on it.
	pub fn focused(&self, ctx: &egui::Context) {
		let suspended = self.suspended.load(Ordering::Relaxed);
		if suspended || ctx.text_edit_focused() || !matches!(self.reach(), Reach::Focused(_)) {
			return;
		}
		ctx.input_mut(|input| {
			input.events.retain(|event| {
				let egui::Event::Key {
					key,
					physical_key,
					pressed: true,
					repeat,
					modifiers,
				} = event
				else {
					return true;
				};
				let pressed = KeyboardShortcut::new(*modifiers, heard(*key, *physical_key));
				let Some((action, _)) = self.bindings.iter().find(|(_, key)| pressed == **key) else {
					return true;
				};
				// A held key fires once, and its repeats are taken too.
				if !repeat {
					let _ = self.fire.send(action.clone());
				}
				false
			});
		});
	}
}

impl Backend {
	/// Pick where keys are heard from what the platform offers.
	fn new(window: &eframe::Frame, runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Action>) -> Self {
		#[cfg(target_os = "linux")]
		{
			use raw_window_handle::{HasWindowHandle, RawWindowHandle};
			let wayland = window
				.window_handle()
				.is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
			if wayland {
				return Self::Portal(portal::Portal::new(runtime, fire));
			}
		}
		#[cfg(not(target_os = "linux"))]
		let _ = (window, runtime);
		let manager = match GlobalHotKeyManager::new() {
			Ok(manager) => manager,
			Err(err) => return Self::Focused(format!("the platform's global hotkeys are unavailable: {err}")),
		};
		let registered: Arc<Mutex<HashMap<u32, Action>>> = Arc::default();
		let heard = registered.clone();
		GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
			if event.state() != HotKeyState::Pressed {
				return;
			}
			if let Some(action) = heard.lock().unwrap().get(&event.id()) {
				let _ = fire.send(action.clone());
			}
		}));
		Self::Native {
			manager,
			held: Vec::new(),
			registered,
		}
	}
}

/// `bindings` with each scene `renames` renamed bound by its new name. A
/// renamed scene's key wins over one kept for a removed scene of that name.
fn renamed(bindings: &Bindings, renames: &BTreeMap<String, Option<String>>) -> Bindings {
	let mut renamed = Bindings::new();
	let mut kept = Vec::new();
	for (action, key) in bindings {
		let to = match action {
			Action::Scene(name) => renames.get(name).cloned().flatten(),
			_ => None,
		};
		match to {
			Some(to) => drop(renamed.insert(Action::Scene(to), *key)),
			None => kept.push((action.clone(), *key)),
		}
	}
	for (action, key) in kept {
		renamed.entry(action).or_insert(key);
	}
	renamed
}

/// What the platform said when it refused a key.
fn refusal(err: global_hotkey::Error) -> String {
	match err {
		global_hotkey::Error::AlreadyRegistered(_) => "another app holds this key".into(),
		err => format!("the platform refused this key: {err}"),
	}
}

/// The key `event` presses, with the modifiers held, ignoring repeats.
pub fn pressed(event: &egui::Event) -> Option<KeyboardShortcut> {
	match event {
		egui::Event::Key {
			key,
			physical_key,
			pressed: true,
			repeat: false,
			modifiers,
		} => Some(KeyboardShortcut::new(*modifiers, heard(*key, *physical_key))),
		_ => None,
	}
}

/// The key the platform's hotkeys hear for a press of `key`, as the layout
/// names it, at `physical`, where it sits on a US keyboard. macOS registers
/// a key by where it sits. Windows' virtual keys and the keysyms X11 and the
/// Wayland portal bind name what the layout prints on it, so a key only
/// Shift makes, such as `!`, is named by where it sits instead.
fn heard(key: Key, physical: Option<Key>) -> Key {
	let physical = physical.unwrap_or(key);
	match cfg!(target_os = "macos") || code(key).is_none() {
		true => physical,
		false => key,
	}
}

/// `key` as the platform's hotkeys name it, if they can hear it.
fn hotkey(key: &KeyboardShortcut) -> Option<HotKey> {
	let held = key.modifiers;
	let mut mods = Modifiers::empty();
	for (on, modifier) in [
		(held.shift, Modifiers::SHIFT),
		(held.alt, Modifiers::ALT),
		(held.ctrl, Modifiers::CONTROL),
		(held.mac_cmd, Modifiers::META),
	] {
		if on {
			mods |= modifier;
		}
	}
	Some(HotKey::new(Some(mods), code(key.logical_key)?))
}

/// `key` as the platforms name it: where it sits on a US keyboard, which
/// Windows and X11 take for the key the layout prints that on.
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

	#[test]
	fn keys_bind_as_the_platform_hears_them() {
		// On AZERTY, the key printed A sits where a US keyboard has Q.
		let a = heard(Key::A, Some(Key::Q));
		match cfg!(target_os = "macos") {
			true => assert_eq!(a, Key::Q),
			false => assert_eq!(a, Key::A),
		}
		// Shift and the key printed 1 make `!`, which no platform binds.
		assert_eq!(heard(Key::Exclamationmark, Some(Key::Num1)), Key::Num1);
		assert_eq!(heard(Key::F9, None), Key::F9);
	}

	#[test]
	fn bindings_follow_renames_at_once() {
		let key = |key| KeyboardShortcut::new(egui::Modifiers::NONE, key);
		let scene = |name: &str| Action::Scene(name.into());
		let bindings = Bindings::from([
			(scene("A"), key(Key::F1)),
			(scene("B"), key(Key::F2)),
			(Action::Cut, key(Key::F3)),
		]);
		// Swapping two names swaps their keys, and a removed scene keeps its.
		let renames = BTreeMap::from([
			("A".to_string(), Some("B".to_string())),
			("B".to_string(), Some("A".to_string())),
		]);
		let swapped = renamed(&bindings, &renames);
		assert_eq!(swapped[&scene("A")], key(Key::F2));
		assert_eq!(swapped[&scene("B")], key(Key::F1));
		assert_eq!(swapped[&Action::Cut], key(Key::F3));
		let removed = BTreeMap::from([("A".to_string(), None)]);
		assert_eq!(renamed(&bindings, &removed), bindings);
		// B removed, then A renamed to B: A's key follows it over B's kept one.
		let onto = BTreeMap::from([("A".to_string(), Some("B".to_string()))]);
		let moved = renamed(&bindings, &onto);
		assert_eq!(moved[&scene("B")], key(Key::F1));
		assert!(!moved.contains_key(&scene("A")));
	}
}
