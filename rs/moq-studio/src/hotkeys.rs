//! Keys the operator binds to studio actions, heard system-wide, so a replay
//! fires from inside a game: through the platform's global hotkeys on macOS
//! and Windows, a key grab on X11, and the desktop's GlobalShortcuts portal on
//! Wayland. Where neither is there, they work only while the studio has focus.
//!
//! A key is where it sits, as winit names it from the window's own key event:
//! egui names a keypad key as its main keyboard twin and drops a key it cannot
//! name, so the window hears keys before egui does.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use winit::keyboard::{Key, KeyCode, ModifiersState, NativeKeyCode, PhysicalKey};

use crate::controls::{self, Action, Controls};

// Tested on macOS too, which global-hotkey builds for.
#[cfg(any(windows, all(test, target_os = "macos")))]
mod layout;
#[cfg(any(target_os = "macos", windows))]
mod native;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod x11;

/// The key bound to each action. Every action starts unbound, so a key
/// another app, such as a game, uses is never taken unasked.
pub type Bindings = BTreeMap<Action, Binding>;

/// A key bound to an action.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Binding {
	/// The modifiers held with the key.
	pub modifiers: egui::Modifiers,
	/// Where the key sits, which the platform hears whatever the layout.
	pub key: PhysicalKey,
	/// What the layout put on the key when it was bound, ignoring modifiers,
	/// which the panel shows and the Wayland portal is asked for.
	pub name: Key,
}

impl Binding {
	/// Whether `self` and `other` are the same key, held with the same
	/// modifiers, whatever the layout called each.
	fn same(&self, other: &Binding) -> bool {
		self.key == other.key && self.modifiers == other.modifiers
	}

	/// The key with its modifiers, as the panel shows it.
	pub fn shown(&self, ctx: &egui::Context) -> String {
		let modifiers = egui::ModifierNames::NAMES.format(&self.modifiers, ctx.os().is_mac());
		match modifiers.is_empty() {
			true => self.label(),
			false => format!("{modifiers}+{}", self.label()),
		}
	}

	/// The key alone, as the layout printed it, and a keypad key by its place
	/// on the keypad, which NumLock relabels.
	fn label(&self) -> String {
		if let Some((_, label, _)) = keypad(self.key) {
			return format!("Keypad {label}");
		}
		match (&self.name, self.key) {
			(Key::Character(text), _) => text.to_uppercase(),
			(Key::Named(named), _) => format!("{named:?}"),
			(_, PhysicalKey::Code(code)) => format!("{code:?}"),
			(_, PhysicalKey::Unidentified(native)) => match native {
				NativeKeyCode::Xkb(code) => format!("Key {code}"),
				NativeKeyCode::Windows(code) => format!("Key {code}"),
				NativeKeyCode::MacOS(code) => format!("Key {code}"),
				NativeKeyCode::Android(code) => format!("Key {code}"),
				NativeKeyCode::Unidentified => "Unknown key".into(),
			},
		}
	}
}

/// The keypad keys, each with what the panel calls it and its XKB keysym.
const KEYPAD: [(KeyCode, &str, &str); 18] = [
	(KeyCode::Numpad0, "0", "KP_0"),
	(KeyCode::Numpad1, "1", "KP_1"),
	(KeyCode::Numpad2, "2", "KP_2"),
	(KeyCode::Numpad3, "3", "KP_3"),
	(KeyCode::Numpad4, "4", "KP_4"),
	(KeyCode::Numpad5, "5", "KP_5"),
	(KeyCode::Numpad6, "6", "KP_6"),
	(KeyCode::Numpad7, "7", "KP_7"),
	(KeyCode::Numpad8, "8", "KP_8"),
	(KeyCode::Numpad9, "9", "KP_9"),
	(KeyCode::NumpadAdd, "+", "KP_Add"),
	(KeyCode::NumpadSubtract, "-", "KP_Subtract"),
	(KeyCode::NumpadMultiply, "*", "KP_Multiply"),
	(KeyCode::NumpadDivide, "/", "KP_Divide"),
	(KeyCode::NumpadDecimal, ".", "KP_Decimal"),
	(KeyCode::NumpadComma, ",", "KP_Separator"),
	(KeyCode::NumpadEqual, "=", "KP_Equal"),
	(KeyCode::NumpadEnter, "Enter", "KP_Enter"),
];

/// `key`'s entry in [`KEYPAD`], if it is on the keypad.
fn keypad(key: PhysicalKey) -> Option<&'static (KeyCode, &'static str, &'static str)> {
	KEYPAD.iter().find(|(code, ..)| key == PhysicalKey::Code(*code))
}

/// The keys pressed in the studio's window, which winit hands it before egui.
#[derive(Clone, Default)]
pub struct Keyboard(Arc<Mutex<Pressing>>);

#[derive(Default)]
struct Pressing {
	/// The modifiers held now, as egui names them.
	modifiers: egui::Modifiers,
	taking: Taking,
}

/// What a key pressed in the window does besides reaching egui.
#[derive(Default)]
enum Taking {
	#[default]
	Nothing,
	/// The Hotkeys panel waits for a key: the first pressed, other than a
	/// modifier, is kept for it, and no key reaches egui meanwhile, so the
	/// key binds rather than acts.
	Binding(Option<Binding>),
	/// Bound keys fire instead of reaching egui, where nothing hears them
	/// outside the window.
	Firing {
		bound: Arc<Mutex<Bindings>>,
		fire: mpsc::Sender<Binding>,
	},
}

impl Keyboard {
	/// Note the modifiers held from now on.
	pub fn modifiers(&self, state: ModifiersState) {
		let mac = cfg!(target_os = "macos");
		self.0.lock().unwrap().modifiers = egui::Modifiers {
			alt: state.alt_key(),
			ctrl: state.control_key(),
			shift: state.shift_key(),
			mac_cmd: mac && state.super_key(),
			command: match mac {
				true => state.super_key(),
				false => state.control_key(),
			},
		};
	}

	/// Hear `key` pressed in the window, where the layout puts `name` on it,
	/// returning whether egui hears it too.
	pub fn pressed(&self, key: PhysicalKey, name: Key, repeat: bool) -> bool {
		let mut pressing = self.0.lock().unwrap();
		let pressed = Binding {
			modifiers: pressing.modifiers,
			key,
			name,
		};
		match &mut pressing.taking {
			Taking::Nothing => true,
			// A modifier is held with the key it waits for.
			Taking::Binding(_) if modifier(key) => true,
			Taking::Binding(taken) => {
				if taken.is_none() && !repeat {
					*taken = Some(pressed);
				}
				false
			}
			Taking::Firing { bound, fire } => {
				if !bound.lock().unwrap().values().any(|bound| bound.same(&pressed)) {
					return true;
				}
				// A held key fires once, and its repeats are taken too.
				if !repeat {
					let _ = fire.send(pressed);
				}
				false
			}
		}
	}

	fn take(&self, taking: Taking) {
		self.0.lock().unwrap().taking = taking;
	}
}

/// Whether `key` is a modifier, held with a key rather than bound itself.
fn modifier(key: PhysicalKey) -> bool {
	matches!(
		key,
		PhysicalKey::Code(
			KeyCode::ShiftLeft
				| KeyCode::ShiftRight
				| KeyCode::ControlLeft
				| KeyCode::ControlRight
				| KeyCode::AltLeft
				| KeyCode::AltRight
				| KeyCode::SuperLeft
				| KeyCode::SuperRight
				| KeyCode::Meta
				| KeyCode::Hyper
				| KeyCode::Fn
				| KeyCode::FnLock
		)
	)
}

/// The bound keys, heard wherever the platform lets the studio hear them.
pub struct Hotkeys {
	/// The key bound to each action, which the thread firing them reads as
	/// each key fires.
	bound: Arc<Mutex<Bindings>>,
	backend: Backend,
	/// The keys last handed to the backend.
	heard: Vec<Binding>,
	/// Why the platform refused each key it would not hear.
	refused: Vec<(Binding, String)>,
	/// Whether bindings are held off while the Hotkeys panel waits for a key,
	/// which the thread firing them reads too.
	suspended: Arc<AtomicBool>,
	keyboard: Keyboard,
	fire: mpsc::Sender<Binding>,
}

/// Where bound keys are heard.
enum Backend {
	/// The system's global hotkeys: `RegisterEventHotKey` on macOS, and
	/// `RegisterHotKey` on Windows.
	#[cfg(any(target_os = "macos", windows))]
	Native(native::Native),
	/// Key grabs from the X server.
	#[cfg(target_os = "linux")]
	X11(x11::X11),
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

/// What `Hotkeys::new` takes from the window it hears keys for.
pub struct Window<'a> {
	pub frame: &'a eframe::Frame,
	pub ctx: &'a egui::Context,
	/// The keys the window hears before egui.
	pub keyboard: Keyboard,
}

impl Hotkeys {
	/// Hear `bindings` from now on, firing each action on `controls` from a
	/// thread of its own. Called on the window's thread, which the platform's
	/// hotkeys must be registered from.
	pub fn new(
		window: Window<'_>,
		controls: Arc<Controls>,
		runtime: &tokio::runtime::Handle,
		bindings: Bindings,
	) -> Self {
		let (fire, pressed) = mpsc::channel::<Binding>();
		let bound: Arc<Mutex<Bindings>> = Arc::default();
		let suspended = Arc::new(AtomicBool::new(false));
		let (held_off, reading) = (suspended.clone(), bound.clone());
		std::thread::spawn(move || {
			for key in pressed {
				// A key the desktop holds on Wayland still fires while the panel
				// waits for one, and the panel never sees it.
				if held_off.load(Ordering::Relaxed) {
					continue;
				}
				// What a key does is read as it fires, under the lock the window
				// moves keys through renames under, so a key cleared since does
				// nothing and one whose scene was renamed previews it by its new name.
				let mut state = controls.engine.state.lock().unwrap();
				let action = reading
					.lock()
					.unwrap()
					.iter()
					.find(|(_, bound)| bound.same(&key))
					.map(|(action, _)| action.clone());
				let Some(action) = action else {
					continue;
				};
				match &action {
					Action::Scene(name) => {
						let result = controls::preview(&mut state, name);
						drop(state);
						controls.report(&action, result.map_err(|err| format!("{err:#}")));
					}
					action => {
						drop(state);
						controls.press(action);
					}
				}
			}
		});
		let backend = Backend::new(&window, runtime, fire.clone());
		let mut hotkeys = Self {
			bound,
			backend,
			heard: Vec::new(),
			refused: Vec::new(),
			suspended,
			keyboard: window.keyboard,
			fire,
		};
		hotkeys.set(bindings);
		hotkeys
	}

	pub fn bindings(&self) -> Bindings {
		self.bound.lock().unwrap().clone()
	}

	/// Bind `action` to `key`, or unbind it, unless `key` is bound to another
	/// action, which is returned instead.
	pub fn bind(&mut self, action: &Action, key: Option<Binding>) -> Result<(), Action> {
		let mut bindings = self.bindings();
		if let Some(key) = &key
			&& let Some((other, _)) = bindings
				.iter()
				.find(|(other, bound)| bound.same(key) && *other != action)
		{
			return Err(other.clone());
		}
		match key {
			Some(key) => bindings.insert(action.clone(), key),
			None => bindings.remove(action),
		};
		self.set(bindings);
		Ok(())
	}

	/// Move each binding to a scene `renames` renamed, all at once, so a swap
	/// of two names swaps their keys. A removed scene keeps its key, which an
	/// undo may bring back. Called under the state lock `renames` was taken
	/// under, so a key fired meanwhile reads the names on one side of them;
	/// [`hear`](Self::hear) then follows outside it.
	pub fn rename(&mut self, renames: &BTreeMap<String, Option<String>>) {
		let mut bound = self.bound.lock().unwrap();
		*bound = renamed(&bound, renames);
	}

	/// Hold every binding off while the Hotkeys panel waits for a key, so the
	/// key pressed binds rather than fires; or take them up again.
	pub fn suspend(&mut self, suspended: bool) {
		self.keyboard.take(match suspended {
			true => Taking::Binding(None),
			false => Taking::Nothing,
		});
		if self.suspended.swap(suspended, Ordering::Relaxed) != suspended {
			self.set(self.bindings());
		}
	}

	/// The key pressed in the window since the Hotkeys panel began waiting
	/// for one, if any.
	pub fn taken(&self) -> Option<Binding> {
		match &mut self.keyboard.0.lock().unwrap().taking {
			Taking::Binding(taken) => taken.take(),
			_ => None,
		}
	}

	/// Hear `bindings` instead of the ones heard now.
	fn set(&mut self, bindings: Bindings) {
		*self.bound.lock().unwrap() = bindings;
		self.hear();
	}

	/// Have the platform hear the keys bound now, through the keyboard layout
	/// in use where the platform matches keys through one. Called before each
	/// frame too, so the keys follow a layout change.
	pub fn hear(&mut self) {
		let bindings = self.bindings();
		let heard: Vec<Binding> = match self.suspended.load(Ordering::Relaxed) {
			true => Vec::new(),
			false => bindings.values().cloned().collect(),
		};
		// A rename moves no key, so the platform is left alone.
		let moved =
			heard.len() != self.heard.len() || heard.iter().any(|key| !self.heard.iter().any(|was| was.same(key)));
		match &mut self.backend {
			#[cfg(any(target_os = "macos", windows))]
			Backend::Native(native) => {
				if moved || native.relaid() {
					self.refused = native.hear(&heard);
				}
			}
			#[cfg(target_os = "linux")]
			Backend::X11(x11) => {
				if moved {
					self.refused = x11.hear(&heard);
				}
			}
			// Held off, the desktop would be asked to bind them all again
			// after each key the panel takes.
			#[cfg(target_os = "linux")]
			Backend::Portal(portal) => portal.set(bindings),
			Backend::Focused(_) => {}
		}
		self.heard = heard;
	}

	/// Why the platform would not hear `action`'s key, if it refused it.
	pub fn refused(&self, action: &Action) -> Option<&str> {
		let bindings = self.bound.lock().unwrap();
		let key = bindings.get(action)?;
		self.refused
			.iter()
			.find(|(refused, _)| refused.same(key))
			.map(|(_, why)| why.as_str())
	}

	pub fn reach(&self) -> Reach {
		match &self.backend {
			#[cfg(any(target_os = "macos", windows))]
			Backend::Native(_) => Reach::Global,
			#[cfg(target_os = "linux")]
			Backend::X11(_) => Reach::Global,
			#[cfg(target_os = "linux")]
			Backend::Portal(portal) => portal.reach(),
			Backend::Focused(reason) => Reach::Focused(reason.clone()),
		}
	}

	/// Fire the actions bound to keys pressed in the studio's window, where
	/// nothing hears them outside it, unless `typing`, as into a text field
	/// or a page the preview passes the keyboard to. A key that fires is
	/// taken, so nothing else in the window acts on it. Called each frame.
	pub fn focused(&self, typing: bool) {
		if self.suspended.load(Ordering::Relaxed) {
			return;
		}
		let firing = !typing && matches!(self.reach(), Reach::Focused(_));
		self.keyboard.take(match firing {
			true => Taking::Firing {
				bound: self.bound.clone(),
				fire: self.fire.clone(),
			},
			false => Taking::Nothing,
		});
	}
}

impl Backend {
	/// Pick where keys are heard from what the platform offers.
	fn new(window: &Window<'_>, runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Binding>) -> Self {
		#[cfg(target_os = "linux")]
		{
			use raw_window_handle::{HasWindowHandle, RawWindowHandle};
			let _ = window.ctx;
			let wayland = window
				.frame
				.window_handle()
				.is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
			if wayland {
				return Self::Portal(portal::Portal::new(runtime, fire));
			}
			match x11::X11::new(fire) {
				Ok(x11) => Self::X11(x11),
				Err(err) => Self::Focused(format!("cannot grab keys from the X server: {err:#}")),
			}
		}
		#[cfg(any(target_os = "macos", windows))]
		{
			let _ = window.frame;
			match native::Native::new(window.ctx, runtime, fire) {
				Ok(native) => Self::Native(native),
				Err(err) => Self::Focused(format!("the platform's global hotkeys are unavailable: {err}")),
			}
		}
		#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
		{
			let _ = (window, runtime, fire);
			Self::Focused("this platform has no global hotkeys".into())
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
			Some(to) => drop(renamed.insert(Action::Scene(to), key.clone())),
			None => kept.push((action.clone(), key.clone())),
		}
	}
	for (action, key) in kept {
		renamed.entry(action).or_insert(key);
	}
	renamed
}

#[cfg(test)]
mod tests {
	use winit::keyboard::NamedKey;

	use super::*;

	fn key(code: KeyCode, name: &str) -> Binding {
		Binding {
			modifiers: egui::Modifiers::NONE,
			key: PhysicalKey::Code(code),
			name: Key::Character(name.into()),
		}
	}

	#[test]
	fn a_key_is_where_it_sits() {
		// Shift and the key a US keyboard prints 7 on make `/` on German.
		let mut german = key(KeyCode::Digit7, "7");
		german.modifiers = egui::Modifiers::SHIFT;
		let mut us = key(KeyCode::Digit7, "7");
		us.modifiers = egui::Modifiers::SHIFT;
		us.name = Key::Character("&".into());
		assert!(us.same(&german));
		assert!(!us.same(&key(KeyCode::Digit7, "7")));
		// Keypad 1 is not the 1 above the letters, whatever NumLock makes it.
		let keypad = key(KeyCode::Numpad1, "1");
		assert!(!keypad.same(&key(KeyCode::Digit1, "1")));
		assert_eq!(keypad.label(), "Keypad 1");
		let mut end = keypad.clone();
		end.name = Key::Named(NamedKey::End);
		assert!(end.same(&keypad));
		assert_eq!(end.label(), "Keypad 1");
		assert_eq!(key(KeyCode::Digit1, "1").label(), "1");
		// A letter outside A to Z shows as the layout prints it.
		assert_eq!(key(KeyCode::Semicolon, "ö").label(), "Ö");
		let unnamed = Binding {
			modifiers: egui::Modifiers::NONE,
			key: PhysicalKey::Unidentified(NativeKeyCode::Xkb(200)),
			name: Key::Unidentified(winit::keyboard::NativeKey::Unidentified),
		};
		assert_eq!(unnamed.label(), "Key 200");
	}

	#[test]
	fn the_panel_takes_the_key_pressed() {
		let keyboard = Keyboard::default();
		let press =
			|code, name: &str, repeat| keyboard.pressed(PhysicalKey::Code(code), Key::Character(name.into()), repeat);
		// Nothing is taken unless the panel waits for a key.
		assert!(press(KeyCode::KeyA, "a", false));
		keyboard.take(Taking::Binding(None));
		keyboard.modifiers(ModifiersState::CONTROL);
		// A modifier is held for the key, which egui hears no more of.
		assert!(keyboard.pressed(
			PhysicalKey::Code(KeyCode::ControlLeft),
			Key::Named(NamedKey::Control),
			false
		));
		assert!(!press(KeyCode::Numpad1, "1", false));
		assert!(!press(KeyCode::Digit1, "1", false));
		let Taking::Binding(Some(taken)) = &keyboard.0.lock().unwrap().taking else {
			panic!("no key was taken");
		};
		assert_eq!(taken.key, PhysicalKey::Code(KeyCode::Numpad1));
		assert!(taken.modifiers.ctrl);
	}

	#[test]
	fn focused_keys_fire_once_and_are_taken() {
		let keyboard = Keyboard::default();
		let (fire, fired) = mpsc::channel();
		let bound = Arc::new(Mutex::new(Bindings::from([(Action::Cut, key(KeyCode::Numpad1, "1"))])));
		keyboard.take(Taking::Firing { bound, fire });
		let press = |code, repeat| keyboard.pressed(PhysicalKey::Code(code), Key::Character("1".into()), repeat);
		assert!(press(KeyCode::Digit1, false), "an unbound key reaches egui");
		assert!(!press(KeyCode::Numpad1, false));
		assert!(!press(KeyCode::Numpad1, true));
		assert_eq!(fired.try_iter().count(), 1);
	}

	#[test]
	fn bindings_follow_renames_at_once() {
		let scene = |name: &str| Action::Scene(name.into());
		let (f1, f2, f3) = (key(KeyCode::F1, "F1"), key(KeyCode::F2, "F2"), key(KeyCode::F3, "F3"));
		let bindings = Bindings::from([
			(scene("A"), f1.clone()),
			(scene("B"), f2.clone()),
			(Action::Cut, f3.clone()),
		]);
		// Swapping two names swaps their keys, and a removed scene keeps its.
		let renames = BTreeMap::from([
			("A".to_string(), Some("B".to_string())),
			("B".to_string(), Some("A".to_string())),
		]);
		let swapped = renamed(&bindings, &renames);
		assert_eq!(swapped[&scene("A")], f2);
		assert_eq!(swapped[&scene("B")], f1);
		assert_eq!(swapped[&Action::Cut], f3);
		let removed = BTreeMap::from([("A".to_string(), None)]);
		assert_eq!(renamed(&bindings, &removed), bindings);
		// B removed, then A renamed to B: A's key follows it over B's kept one.
		let onto = BTreeMap::from([("A".to_string(), Some("B".to_string()))]);
		let moved = renamed(&bindings, &onto);
		assert_eq!(moved[&scene("B")], f1);
		assert!(!moved.contains_key(&scene("A")));
	}
}
