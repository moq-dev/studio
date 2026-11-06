//! Keys the operator binds to studio actions, heard system-wide, so a replay
//! fires from inside a game: through the platform's global hotkeys on macOS,
//! Windows, and X11, and the desktop's GlobalShortcuts portal on Wayland.
//! Where neither is there, they work only while the studio has focus.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use egui::{Key, KeyboardShortcut};
use global_hotkey::hotkey::{HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::controls::{self, Action, Controls};

mod keypad;
mod layout;
#[cfg(target_os = "linux")]
mod portal;

pub use keypad::Keypad;
use layout::Layout;

/// The key bound to each action. Every action starts unbound, so a key
/// another app, such as a game, uses is never taken unasked.
pub type Bindings = BTreeMap<Action, Binding>;

/// A key bound to an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Binding {
	/// The key with its modifiers as the layout names it, which the panel
	/// shows and the Wayland portal is asked for.
	pub key: KeyboardShortcut,
	/// Where the key sits, named for the key a US keyboard has there, which
	/// the platform's hotkeys register through the layout in use.
	pub position: Key,
}

impl Binding {
	/// Whether `self` and `other` are the same key, held with the same
	/// modifiers, whatever the layout called each.
	fn same(&self, other: &Binding) -> bool {
		self.position == other.position && self.key.modifiers == other.key.modifiers
	}
}

/// The bound keys, heard wherever the platform lets the studio hear them.
pub struct Hotkeys {
	/// The key bound to each action, which the thread firing them reads as
	/// each key fires.
	bound: Arc<Mutex<Bindings>>,
	backend: Backend,
	/// The keys last handed to the backend.
	heard: Vec<Binding>,
	/// Why the platform refused each key it would not register.
	refused: Vec<(Binding, String)>,
	/// Whether bindings are held off while the Hotkeys panel waits for a key,
	/// which the thread firing them reads too.
	suspended: Arc<AtomicBool>,
	fire: mpsc::Sender<Binding>,
}

/// Where bound keys are heard.
enum Backend {
	/// The platform's global hotkeys: `RegisterEventHotKey` on macOS,
	/// `RegisterHotKey` on Windows, and a key grab on X11.
	Native {
		manager: GlobalHotKeyManager,
		/// The hotkeys registered now, and the key each hears.
		held: Vec<(Binding, HotKey)>,
		/// The key each registered hotkey hears, by its id, which the
		/// platform's event handler reads.
		registered: Arc<Mutex<HashMap<u32, Binding>>>,
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
				let result = match &action {
					Action::Scene(name) => controls::preview(&mut state, name),
					action => {
						drop(state);
						controls.fire(action)
					}
				};
				controls.report(&action, result);
			}
		});
		let backend = Backend::new(window, runtime, fire.clone());
		let mut hotkeys = Self {
			bound,
			backend,
			heard: Vec::new(),
			refused: Vec::new(),
			suspended,
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
		if let Some(key) = key
			&& let Some((other, _)) = bindings
				.iter()
				.find(|(other, bound)| bound.same(&key) && *other != action)
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
		if self.suspended.swap(suspended, Ordering::Relaxed) != suspended {
			self.set(self.bindings());
		}
	}

	/// Hear `bindings` instead of the ones heard now.
	fn set(&mut self, bindings: Bindings) {
		*self.bound.lock().unwrap() = bindings;
		self.hear();
	}

	/// Have the platform hear the keys bound now. On X11, each key it takes
	/// or lets go of waits on the platform's event thread.
	pub fn hear(&mut self) {
		let bindings = self.bindings();
		let heard: Vec<Binding> = match self.suspended.load(Ordering::Relaxed) {
			true => Vec::new(),
			false => bindings.values().copied().collect(),
		};
		// A rename moves no key, so the platform is left alone.
		let moved =
			heard.len() != self.heard.len() || heard.iter().any(|key| !self.heard.iter().any(|was| was.same(key)));
		match &mut self.backend {
			Backend::Native {
				manager,
				held,
				registered,
			} if moved => {
				let layout = Layout::current();
				// Where the platform would let go of another key, or none, and keep
				// holding these, they stay as they are.
				let stuck = match &layout {
					Ok(layout) => held
						.iter()
						.any(|(key, hotkey)| !layout.releases(key.position, hotkey.key))
						.then(|| {
							"the keyboard layout changed since the studio took its keys; restart it to change them"
								.into()
						}),
					Err(err) => (!held.is_empty()).then(|| err.clone()),
				};
				if let Some(why) = stuck {
					self.refused = bindings.values().map(|key| (*key, why.clone())).collect();
					return;
				}
				self.refused.clear();
				// `registered` is not locked while the manager works: on X11 it
				// waits on the thread that runs the event handler, which locks it.
				for (_, hotkey) in held.drain(..) {
					if let Err(err) = manager.unregister(hotkey) {
						tracing::warn!(%hotkey, error = %err, "cannot unregister a hotkey");
					}
				}
				let mut ids = HashMap::new();
				for key in &heard {
					let hotkey = match &layout {
						Ok(layout) => {
							hotkey(layout, key).ok_or_else(|| "this key cannot be heard outside the studio".into())
						}
						Err(err) => Err(err.clone()),
					};
					let hotkey = match hotkey {
						Ok(hotkey) => hotkey,
						Err(reason) => {
							self.refused.push((*key, reason));
							continue;
						}
					};
					match manager.register(hotkey) {
						Ok(()) => {
							held.push((*key, hotkey));
							ids.insert(hotkey.id(), *key);
						}
						Err(err) => self.refused.push((*key, refusal(err))),
					}
				}
				*registered.lock().unwrap() = ids;
			}
			Backend::Native { .. } => {}
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
		let bindings = self.bound.lock().unwrap();
		ctx.input_mut(|input| {
			input.events.retain(|event| {
				let Some(pressed) = binding(event) else {
					return true;
				};
				if !bindings.values().any(|key| key.same(&pressed)) {
					return true;
				}
				// A held key fires once, and its repeats are taken too.
				if !matches!(event, egui::Event::Key { repeat: true, .. }) {
					let _ = self.fire.send(pressed);
				}
				false
			});
		});
	}
}

impl Backend {
	/// Pick where keys are heard from what the platform offers.
	fn new(window: &eframe::Frame, runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Binding>) -> Self {
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
		let registered: Arc<Mutex<HashMap<u32, Binding>>> = Arc::default();
		let heard = registered.clone();
		GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
			if event.state() != HotKeyState::Pressed {
				return;
			}
			if let Some(key) = heard.lock().unwrap().get(&event.id()) {
				let _ = fire.send(*key);
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

/// The key `event` presses, ignoring repeats.
pub fn pressed(event: &egui::Event) -> Option<Binding> {
	match event {
		egui::Event::Key { repeat: false, .. } => binding(event),
		_ => None,
	}
}

/// The key `event` presses or repeats, if it says where the key sits.
fn binding(event: &egui::Event) -> Option<Binding> {
	match event {
		egui::Event::Key {
			key,
			physical_key: Some(position),
			pressed: true,
			modifiers,
			..
		} => Some(Binding {
			key: KeyboardShortcut::new(*modifiers, *key),
			position: *position,
		}),
		_ => None,
	}
}

/// `key` as the platform's hotkeys register it in `layout`, if they can hear it.
fn hotkey(layout: &Layout, key: &Binding) -> Option<HotKey> {
	let held = key.key.modifiers;
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
	Some(HotKey::new(Some(mods), layout.code(key.position)?))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_key_is_the_same_wherever_the_layout_names_it() {
		let press = |key, physical_key, modifiers| egui::Event::Key {
			key,
			physical_key,
			pressed: true,
			repeat: false,
			modifiers,
		};
		// Shift and the key a US keyboard prints 7 on make `/` on German.
		let german = pressed(&press(Key::Slash, Some(Key::Num7), egui::Modifiers::SHIFT)).unwrap();
		assert_eq!(german.position, Key::Num7);
		assert_eq!(german.key.logical_key, Key::Slash);
		let us = pressed(&press(Key::Num7, Some(Key::Num7), egui::Modifiers::SHIFT)).unwrap();
		assert!(us.same(&german));
		assert!(!us.same(&pressed(&press(Key::Num7, Some(Key::Num7), egui::Modifiers::NONE)).unwrap()));
		// A key whose position is unknown cannot be registered by it.
		assert_eq!(pressed(&press(Key::Pipe, None, egui::Modifiers::NONE)), None);
	}

	#[test]
	fn bindings_follow_renames_at_once() {
		let key = |key| Binding {
			key: KeyboardShortcut::new(egui::Modifiers::NONE, key),
			position: key,
		};
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
