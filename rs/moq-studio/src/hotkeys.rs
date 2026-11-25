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

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, NativeKeyCode, PhysicalKey};

use crate::controls::{Action, Controls};

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
	pub fn same(&self, other: &Binding) -> bool {
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

	/// Each modifier and then the key, one per keycap.
	pub fn caps(&self, ctx: &egui::Context) -> Vec<String> {
		let mac = ctx.os().is_mac();
		// Each key held, by its own field: `contains` takes `command` for
		// either Ctrl or Cmd, which would show both for Cmd on a Mac.
		let held = &self.modifiers;
		let mut caps: Vec<String> = [
			(held.ctrl, egui::Modifiers::CTRL),
			(held.alt, egui::Modifiers::ALT),
			(held.shift, egui::Modifiers::SHIFT),
			(held.mac_cmd, egui::Modifiers::MAC_CMD),
		]
		.into_iter()
		.filter(|(on, _)| *on)
		.map(|(_, one)| egui::ModifierNames::NAMES.format(&one, mac))
		.collect();
		caps.push(self.label());
		caps
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
	/// The latest paste to reach egui since [`Keyboard::pasted`] last said so.
	pasted: Option<Pasted>,
}

/// How a paste pressed in the studio's window reads the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pasted {
	/// Read with the key, so egui reports any text on the same frame.
	Read,
	/// Asked of the window, which reports any text on a later frame.
	Asked,
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
	/// outside the window, unless `typing` into the studio's window: there
	/// they reach egui, while another window, the multiview, fires them.
	Firing {
		bound: Arc<Mutex<Bindings>>,
		fire: mpsc::Sender<Binding>,
		typing: bool,
	},
	/// The platform hears bound keys itself, wherever they are pressed, and
	/// fires them, so the window's own presses reach egui. One pressed for the
	/// window by `--control` rather than the desktop fires as the platform's
	/// would.
	Global {
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

	/// Hear `key` pressed in a window, the studio's own if `studio`, where
	/// the layout puts `name` on it, returning whether egui hears it too.
	pub fn pressed(&self, key: PhysicalKey, name: Key, repeat: bool, studio: bool) -> bool {
		let mut pressing = self.0.lock().unwrap();
		// A paste lands in the studio's window, never from the multiview's.
		let paste = studio && !repeat && paste(pressing.modifiers, key, &name);
		let heard = Self::hear(&mut pressing, key, name, repeat, studio);
		if heard && paste {
			pressing.pasted = Some(Pasted::Read);
		}
		heard
	}

	/// Hear `key`, where the layout puts `name`, pressed in the studio's window
	/// by `--control` rather than the desktop, returning whether egui hears it
	/// too: as [`pressed`](Self::pressed), but a bound key fires even where
	/// the platform would have fired it before the window heard it.
	pub fn injected(&self, key: PhysicalKey, name: Key) -> bool {
		let mut pressing = self.0.lock().unwrap();
		let paste = paste(pressing.modifiers, key, &name);
		let heard = match &pressing.taking {
			Taking::Global { bound, fire } => {
				let pressed = Binding {
					modifiers: pressing.modifiers,
					key,
					name,
				};
				!fired(bound, fire, pressed, false)
			}
			_ => Self::hear(&mut pressing, key, name, false, true),
		};
		// `--control` asks the window for the clipboard's text.
		if heard && paste {
			pressing.pasted = Some(Pasted::Asked);
		}
		heard
	}

	/// How the latest paste pressed since this last said so reads the
	/// clipboard, whose text egui reports only when there is some.
	pub fn pasted(&self) -> Option<Pasted> {
		self.0.lock().unwrap().pasted.take()
	}

	fn hear(pressing: &mut Pressing, key: PhysicalKey, name: Key, repeat: bool, studio: bool) -> bool {
		let pressed = Binding {
			modifiers: pressing.modifiers,
			key,
			name,
		};
		match &mut pressing.taking {
			Taking::Nothing | Taking::Global { .. } => true,
			// A modifier is held with the key it waits for.
			Taking::Binding(_) if modifier(key) => true,
			Taking::Binding(taken) => {
				if taken.is_none() && !repeat {
					*taken = Some(pressed);
				}
				false
			}
			Taking::Firing { typing: true, .. } if studio => true,
			Taking::Firing { bound, fire, .. } => !fired(bound, fire, pressed, repeat),
		}
	}

	fn take(&self, taking: Taking) {
		self.0.lock().unwrap().taking = taking;
	}
}

/// Fire `pressed` if it is bound, returning whether it was, so nothing else
/// hears it. A held key fires once, and its repeats are taken too.
fn fired(bound: &Mutex<Bindings>, fire: &mpsc::Sender<Binding>, pressed: Binding, repeat: bool) -> bool {
	if !bound.lock().unwrap().values().any(|bound| bound.same(&pressed)) {
		return false;
	}
	if !repeat {
		let _ = fire.send(pressed);
	}
	true
}

/// Whether `key`, where the layout puts `name`, held with `modifiers`
/// pastes, as egui-winit decides: by the name, or by the key V sits on for a
/// name egui has no key for, as on a layout without Latin letters.
fn paste(modifiers: egui::Modifiers, key: PhysicalKey, name: &Key) -> bool {
	match name {
		Key::Named(NamedKey::Paste) => true,
		Key::Named(NamedKey::Insert) => cfg!(windows) && modifiers.shift,
		Key::Character(text) => {
			modifiers.command
				&& match egui::Key::from_name(text) {
					Some(named) => named == egui::Key::V,
					None => key == PhysicalKey::Code(KeyCode::KeyV),
				}
		}
		_ => false,
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
	/// Those of [`bound`](Self::bound) whose mod is on, which the window's
	/// keyboard takes when only it hears them.
	live: Arc<Mutex<Bindings>>,
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
	/// Which mods are on, whose actions' keys are heard only then.
	controls: Arc<Controls>,
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
		let (held_off, reading, firing) = (suspended.clone(), bound.clone(), controls.clone());
		std::thread::spawn(move || {
			let controls = firing;
			for key in pressed {
				// A key the desktop holds on Wayland still fires while the panel
				// waits for one, and the panel never sees it.
				if held_off.load(Ordering::Relaxed) {
					continue;
				}
				// What a key does is read as it fires, under the lock the window
				// moves keys through renames under, so a key cleared since does
				// nothing and one whose scene was renamed takes it by its new name.
				let mut state = controls.engine.state.lock().unwrap();
				let action = reading
					.lock()
					.unwrap()
					.iter()
					.find(|(_, bound)| bound.same(&key))
					.map(|(action, _)| action.clone());
				// A key of a mod switched off since it was heard does nothing.
				let Some(action) = action.filter(|action| action.needs().is_none_or(|needs| controls.mods.on(needs)))
				else {
					continue;
				};
				match &action {
					Action::Scene(name) => {
						let result = controls.scene(&mut state, name);
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
			live: Arc::default(),
			backend,
			heard: Vec::new(),
			refused: Vec::new(),
			suspended,
			keyboard: window.keyboard,
			fire,
			controls,
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
		// A key bound to an action of a mod switched off is left to other
		// programs until it is back on.
		let mods = &self.controls.mods;
		let mut live = self.bindings();
		live.retain(|action, _| action.needs().is_none_or(|needs| mods.on(needs)));
		*self.live.lock().unwrap() = live.clone();
		let heard: Vec<Binding> = match self.suspended.load(Ordering::Relaxed) {
			true => Vec::new(),
			false => live.values().cloned().collect(),
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
			Backend::Portal(portal) => portal.set(live),
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

	/// Fire the actions bound to keys pressed in the studio's windows, where
	/// nothing hears them outside them, unless `typing` into the studio's
	/// own, as into a text field or a page the preview passes the keyboard
	/// to. A key that fires is taken, so nothing else in the window acts on
	/// it. Called each frame.
	pub fn focused(&self, typing: bool) {
		if self.suspended.load(Ordering::Relaxed) {
			return;
		}
		let (bound, fire) = (self.live.clone(), self.fire.clone());
		self.keyboard.take(match self.reach() {
			Reach::Focused(_) => Taking::Firing { bound, fire, typing },
			_ => Taking::Global { bound, fire },
		});
	}
}

impl Drop for Hotkeys {
	/// Leave the window's keyboard as the next show, while it opens, finds
	/// it: its keys act on nothing.
	fn drop(&mut self) {
		self.keyboard.take(Taking::Nothing);
	}
}

impl Backend {
	/// Pick where keys are heard from what the platform offers.
	fn new(window: &Window<'_>, runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Binding>) -> Self {
		#[cfg(target_os = "linux")]
		{
			use raw_window_handle::{HasWindowHandle, RawWindowHandle};
			let _ = window.ctx;
			match window.frame.window_handle().map(|handle| handle.as_raw()) {
				Ok(RawWindowHandle::Wayland(_)) => Self::Portal(portal::Portal::new(runtime, fire)),
				Ok(_) => match x11::X11::new(fire) {
					Ok(x11) => Self::X11(x11),
					Err(err) => Self::Focused(format!("cannot grab keys from the X server: {err:#}")),
				},
				// Such as a window drawn headless, with no display to hear keys on.
				Err(err) => Self::Focused(format!("the studio has no window: {err}")),
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
	fn a_keycap_for_each_key_held() {
		let ctx = egui::Context::default();
		ctx.set_os(egui::os::OperatingSystem::Mac);
		// Cmd on a Mac, which egui also marks as its `command`.
		let mut cmd = key(KeyCode::KeyL, "l");
		cmd.modifiers = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND;
		let caps = cmd.caps(&ctx);
		assert_eq!(caps.len(), 2, "{caps:?}");
		assert_eq!(caps[1], "L");
		ctx.set_os(egui::os::OperatingSystem::Nix);
		let mut both = key(KeyCode::KeyL, "l");
		both.modifiers = egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
		let caps = both.caps(&ctx);
		assert_eq!(caps.len(), 3, "{caps:?}");
	}

	#[test]
	fn the_panel_takes_the_key_pressed() {
		let keyboard = Keyboard::default();
		let press = |code, name: &str, repeat| {
			keyboard.pressed(PhysicalKey::Code(code), Key::Character(name.into()), repeat, true)
		};
		// Nothing is taken unless the panel waits for a key.
		assert!(press(KeyCode::KeyA, "a", false));
		keyboard.take(Taking::Binding(None));
		keyboard.modifiers(ModifiersState::CONTROL);
		// A modifier is held for the key, which egui hears no more of.
		assert!(keyboard.pressed(
			PhysicalKey::Code(KeyCode::ControlLeft),
			Key::Named(NamedKey::Control),
			false,
			true
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
		let bound = Arc::new(Mutex::new(Bindings::from([(
			Action::Panic,
			key(KeyCode::Numpad1, "1"),
		)])));
		keyboard.take(Taking::Firing {
			bound,
			fire,
			typing: false,
		});
		let press = |code, repeat| keyboard.pressed(PhysicalKey::Code(code), Key::Character("1".into()), repeat, true);
		assert!(press(KeyCode::Digit1, false), "an unbound key reaches egui");
		assert!(!press(KeyCode::Numpad1, false));
		assert!(!press(KeyCode::Numpad1, true));
		assert_eq!(fired.try_iter().count(), 1);
	}

	/// A paste pressed in the multiview pastes nothing into the studio.
	#[test]
	fn only_the_studios_window_pastes() {
		let keyboard = Keyboard::default();
		keyboard.modifiers(match cfg!(target_os = "macos") {
			true => ModifiersState::SUPER,
			false => ModifiersState::CONTROL,
		});
		let paste = |studio| {
			keyboard.pressed(
				PhysicalKey::Code(KeyCode::KeyV),
				Key::Character("v".into()),
				false,
				studio,
			)
		};
		assert!(paste(false));
		assert_eq!(keyboard.pasted(), None, "the multiview pasted");
		assert!(paste(true));
		assert_eq!(keyboard.pasted(), Some(Pasted::Read), "the studio's window pasted");
		let v = (PhysicalKey::Code(KeyCode::KeyV), Key::Character("v".into()));
		assert!(keyboard.injected(v.0, v.1));
		assert_eq!(keyboard.pasted(), Some(Pasted::Asked), "--control asked the window");
	}

	/// Typing into the studio's window holds its keys back, not those the
	/// multiview hears, whichever window drew last.
	#[test]
	fn typing_holds_back_only_the_studios_keys() {
		let keyboard = Keyboard::default();
		let (fire, fired) = mpsc::channel();
		let bound = Arc::new(Mutex::new(Bindings::from([(
			Action::Panic,
			key(KeyCode::Numpad1, "1"),
		)])));
		keyboard.take(Taking::Firing {
			bound,
			fire,
			typing: true,
		});
		let press = |studio| {
			keyboard.pressed(
				PhysicalKey::Code(KeyCode::Numpad1),
				Key::Character("1".into()),
				false,
				studio,
			)
		};
		assert!(press(true), "typed into the studio's field");
		assert_eq!(fired.try_iter().count(), 0);
		assert!(!press(false), "fired in the multiview");
		assert_eq!(fired.try_iter().count(), 1);
	}

	/// Where the platform hears bound keys, the window's own presses of them
	/// reach egui, while one `--control` presses fires as the platform's would.
	#[test]
	fn an_injected_key_fires_where_the_platform_hears_keys() {
		let keyboard = Keyboard::default();
		let (fire, fired) = mpsc::channel();
		let bound = Arc::new(Mutex::new(Bindings::from([(Action::Panic, key(KeyCode::F9, "F9"))])));
		keyboard.take(Taking::Global { bound, fire });
		let f9 = || (PhysicalKey::Code(KeyCode::F9), Key::Named(NamedKey::F9));
		let (code, name) = f9();
		assert!(
			keyboard.pressed(code, name, false, true),
			"the desktop's press reached egui"
		);
		assert_eq!(fired.try_iter().count(), 0);
		let (code, name) = f9();
		assert!(!keyboard.injected(code, name), "the injected press fired instead");
		assert_eq!(fired.try_iter().count(), 1);
		let unbound = keyboard.injected(PhysicalKey::Code(KeyCode::F8), Key::Named(NamedKey::F8));
		assert!(unbound, "an unbound key reaches egui");
	}

	#[test]
	fn bindings_follow_renames_at_once() {
		let scene = |name: &str| Action::Scene(name.into());
		let (f1, f2, f3) = (key(KeyCode::F1, "F1"), key(KeyCode::F2, "F2"), key(KeyCode::F3, "F3"));
		let bindings = Bindings::from([
			(scene("A"), f1.clone()),
			(scene("B"), f2.clone()),
			(Action::Panic, f3.clone()),
		]);
		// Swapping two names swaps their keys, and a removed scene keeps its.
		let renames = BTreeMap::from([
			("A".to_string(), Some("B".to_string())),
			("B".to_string(), Some("A".to_string())),
		]);
		let swapped = renamed(&bindings, &renames);
		assert_eq!(swapped[&scene("A")], f2);
		assert_eq!(swapped[&scene("B")], f1);
		assert_eq!(swapped[&Action::Panic], f3);
		let removed = BTreeMap::from([("A".to_string(), None)]);
		assert_eq!(renamed(&bindings, &removed), bindings);
		// B removed, then A renamed to B: A's key follows it over B's kept one.
		let onto = BTreeMap::from([("A".to_string(), Some("B".to_string()))]);
		let moved = renamed(&bindings, &onto);
		assert_eq!(moved[&scene("B")], f1);
		assert!(!moved.contains_key(&scene("A")));
	}

	/// The paste key is V by name, or the key V sits on where the layout
	/// names it nothing egui knows, as egui-winit falls back to.
	#[test]
	fn paste_as_egui_hears_it() {
		let command = egui::Modifiers::COMMAND;
		let v = PhysicalKey::Code(KeyCode::KeyV);
		let pastes = |modifiers, key, name: &str| paste(modifiers, key, &Key::Character(name.into()));
		assert!(pastes(command, v, "v"));
		assert!(pastes(command, v, "V"));
		// Russian puts `м` on V.
		assert!(pastes(command, v, "м"));
		// Dvorak puts `k` on V, and `v` on the key Qwerty's period is on.
		assert!(!pastes(command, v, "k"));
		assert!(pastes(command, PhysicalKey::Code(KeyCode::Period), "v"));
		assert!(!pastes(egui::Modifiers::NONE, v, "v"));
		assert!(paste(egui::Modifiers::NONE, v, &Key::Named(NamedKey::Paste)));
	}
}
