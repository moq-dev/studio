//! Global shortcuts on Wayland, which lets no app hear keys meant for
//! another: the studio asks the desktop's GlobalShortcuts portal for each
//! binding, and the desktop's shortcut settings decide the keys.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use tokio::sync::watch;
use winit::keyboard::{Key, NamedKey, PhysicalKey};

use super::{Binding, Bindings, Reach};
use crate::controls::Action;

/// A session with the portal, kept by a task of its own.
pub struct Portal {
	/// The bindings the task asks the desktop for.
	bindings: watch::Sender<Bindings>,
	reach: Arc<Mutex<Reach>>,
}

impl Portal {
	pub fn new(runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Binding>) -> Self {
		let (bindings, wanted) = watch::channel(Bindings::new());
		let reach = Arc::new(Mutex::new(Reach::Asking));
		let task = reach.clone();
		runtime.spawn(async move {
			if let Err(err) = run(wanted, fire, &task).await {
				*task.lock().unwrap() = Reach::Focused(format!("{err:#}"));
			}
		});
		Self { bindings, reach }
	}

	/// Ask the desktop for `bindings`, unless it already holds them: a new
	/// session brings its dialog back, and declining it loses every key.
	pub fn set(&self, bindings: Bindings) {
		self.bindings.send_if_modified(|wanted| {
			let changed = *wanted != bindings;
			*wanted = bindings;
			changed
		});
	}

	pub fn reach(&self) -> Reach {
		self.reach.lock().unwrap().clone()
	}
}

/// Ask the desktop for each set of `wanted` bindings in turn, in a session of
/// its own, and fire the keys of the shortcuts it reports.
async fn run(
	mut wanted: watch::Receiver<Bindings>,
	fire: mpsc::Sender<Binding>,
	reach: &Mutex<Reach>,
) -> anyhow::Result<()> {
	let portal = GlobalShortcuts::new()
		.await
		.context("the desktop has no global shortcuts portal")?;
	let mut activated = portal.receive_activated().await?;
	loop {
		let bindings = wanted.borrow_and_update().clone();
		let session = portal.create_session(Default::default()).await?;
		// No session is bound without bindings, so the desktop asks nothing
		// until the first key is.
		*reach.lock().unwrap() = Reach::Desktop(BTreeMap::new());
		if !bindings.is_empty() {
			*reach.lock().unwrap() = Reach::Asking;
			let shortcuts: Vec<NewShortcut> = bindings
				.iter()
				.map(|(action, binding)| {
					NewShortcut::new(id(action, binding), action.to_string())
						.preferred_trigger(trigger(binding).as_deref())
				})
				.collect();
			let bound = portal
				.bind_shortcuts(&session, &shortcuts, None, Default::default())
				.await?
				.response();
			*reach.lock().unwrap() = match bound {
				Ok(bound) => Reach::Desktop(
					bound
						.shortcuts()
						.iter()
						.filter_map(|shortcut| {
							let (action, _) = by_id(&bindings, shortcut.id())?;
							Some((action.clone(), shortcut.trigger_description().to_string()))
						})
						.collect(),
				),
				Err(err) => Reach::Focused(format!("the desktop did not bind them: {err}")),
			};
		}
		loop {
			tokio::select! {
				event = activated.next() => {
					let event = event.context("the global shortcuts portal went away")?;
					// Its key fires whatever it is bound to by then, if anything.
					if let Some((_, key)) = by_id(&bindings, event.shortcut_id()) {
						let _ = fire.send(key.clone());
					}
				}
				changed = wanted.changed() => {
					if changed.is_err() {
						return Ok(());
					}
					break;
				}
			}
		}
		session.close().await?;
	}
}

/// The id the desktop's settings keep `action`'s shortcut under, the same
/// across runs so a key the operator picked there stays. A scene's is named
/// for the key bound to it, which follows the scene through renames.
fn id(action: &Action, binding: &Binding) -> String {
	match action {
		Action::Cut => "cut".into(),
		Action::Transition => "transition".into(),
		Action::Scene(_) => {
			let held = binding.modifiers;
			let mut id = "scene/".to_string();
			for (on, name) in [
				(held.ctrl, "ctrl+"),
				(held.alt, "alt+"),
				(held.shift, "shift+"),
				(held.mac_cmd, "logo+"),
			] {
				if on {
					id.push_str(name);
				}
			}
			match binding.key {
				PhysicalKey::Code(code) => id.push_str(&format!("{code:?}")),
				PhysicalKey::Unidentified(native) => id.push_str(&format!("{native:?}")),
			}
			id
		}
		Action::Replay => "replay".into(),
		Action::SaveClip => "save-clip".into(),
		Action::Record => "record".into(),
	}
}

/// The binding among `bindings` whose shortcut is kept under `id`.
fn by_id<'a>(bindings: &'a Bindings, id: &str) -> Option<(&'a Action, &'a Binding)> {
	bindings
		.iter()
		.find(|(action, binding)| self::id(action, binding) == id)
}

/// `binding` as a trigger for the desktop to offer first, its key named by
/// its XKB keysym.
fn trigger(binding: &Binding) -> Option<String> {
	let held = binding.modifiers;
	let mut trigger = String::new();
	for (on, name) in [
		(held.ctrl, "CTRL+"),
		(held.alt, "ALT+"),
		(held.shift, "SHIFT+"),
		(held.mac_cmd, "LOGO+"),
	] {
		if on {
			trigger.push_str(name);
		}
	}
	trigger.push_str(&keysym(binding)?);
	Some(trigger)
}

/// The XKB keysym of `binding`'s key: a keypad key's whatever NumLock makes
/// it, and otherwise the one the layout put on the key when it was bound.
fn keysym(binding: &Binding) -> Option<String> {
	if let Some((_, _, keysym)) = super::keypad(binding.key) {
		return Some((*keysym).into());
	}
	let name = match &binding.name {
		Key::Named(named) => match named {
			NamedKey::ArrowDown => "Down",
			NamedKey::ArrowLeft => "Left",
			NamedKey::ArrowRight => "Right",
			NamedKey::ArrowUp => "Up",
			NamedKey::Escape => "Escape",
			NamedKey::Tab => "Tab",
			NamedKey::Backspace => "BackSpace",
			NamedKey::Enter => "Return",
			NamedKey::Space => "space",
			NamedKey::Insert => "Insert",
			NamedKey::Delete => "Delete",
			NamedKey::Home => "Home",
			NamedKey::End => "End",
			NamedKey::PageUp => "Page_Up",
			NamedKey::PageDown => "Page_Down",
			NamedKey::PrintScreen => "Print",
			NamedKey::Pause => "Pause",
			// A function key is its name, as winit names them.
			named => {
				let name = format!("{named:?}");
				let function = name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok();
				return function.then_some(name);
			}
		},
		Key::Character(text) => {
			let mut chars = text.chars();
			let (Some(char), None) = (chars.next(), chars.next()) else {
				return None;
			};
			match char {
				',' => "comma",
				'\\' => "backslash",
				'/' => "slash",
				'[' => "bracketleft",
				']' => "bracketright",
				'`' => "grave",
				'-' => "minus",
				'.' => "period",
				'=' => "equal",
				';' => "semicolon",
				'\'' => "apostrophe",
				// A letter or digit is its lower-case self, and any other
				// character its Unicode keysym, as XKB names one.
				char if char.is_ascii_alphanumeric() => return Some(char.to_ascii_lowercase().into()),
				char => return Some(format!("U{:04X}", u32::from(char))),
			}
		}
		_ => return None,
	};
	Some(name.into())
}

#[cfg(test)]
mod tests {
	use winit::keyboard::{KeyCode, NativeKeyCode};

	use super::*;

	fn key(modifiers: egui::Modifiers, code: KeyCode, name: Key) -> Binding {
		Binding {
			modifiers,
			key: PhysicalKey::Code(code),
			name,
		}
	}

	fn prints(text: &str) -> Key {
		Key::Character(text.into())
	}

	#[test]
	fn unchanged_bindings_keep_the_session() {
		let (bindings, mut wanted) = watch::channel(Bindings::new());
		let portal = Portal {
			bindings,
			reach: Arc::new(Mutex::new(Reach::Asking)),
		};
		let key = key(egui::Modifiers::CTRL, KeyCode::KeyR, prints("r"));
		portal.set(Bindings::from([(Action::Cut, key.clone())]));
		assert!(wanted.has_changed().unwrap());
		wanted.mark_unchanged();
		// What the Hotkeys panel sets when it starts and stops waiting for a key.
		portal.set(Bindings::from([(Action::Cut, key)]));
		assert!(!wanted.has_changed().unwrap());
	}

	#[test]
	fn a_scene_keeps_its_id_through_renames() {
		let scene = |name: &str| Action::Scene(name.into());
		let f1 = key(egui::Modifiers::NONE, KeyCode::F1, Key::Named(NamedKey::F1));
		assert_eq!(id(&scene("Intro"), &f1), "scene/F1");
		assert_eq!(id(&scene("Intro"), &f1), id(&scene("Opening"), &f1));
		let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
		assert_eq!(
			id(&scene("Intro"), &key(ctrl_shift, KeyCode::Digit1, prints("1"))),
			"scene/ctrl+shift+Digit1"
		);
		// Keypad 1 is its own shortcut.
		assert_eq!(
			id(&scene("Intro"), &key(ctrl_shift, KeyCode::Numpad1, prints("1"))),
			"scene/ctrl+shift+Numpad1"
		);
		// An activation reads the bindings the session was opened with.
		let cut = key(egui::Modifiers::CTRL, KeyCode::F1, Key::Named(NamedKey::F1));
		let bindings = Bindings::from([(scene("Opening"), f1.clone()), (Action::Cut, cut)]);
		assert_eq!(by_id(&bindings, "scene/F1"), Some((&scene("Opening"), &f1)));
		assert_eq!(by_id(&bindings, "cut").map(|(action, _)| action), Some(&Action::Cut));
		assert_eq!(by_id(&bindings, "unknown"), None);
	}

	#[test]
	fn triggers_name_keysyms() {
		let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
		let trigger = |modifiers, code, name| trigger(&key(modifiers, code, name));
		assert_eq!(
			trigger(ctrl_shift, KeyCode::KeyR, prints("r")).as_deref(),
			Some("CTRL+SHIFT+r")
		);
		let none = egui::Modifiers::NONE;
		assert_eq!(
			trigger(none, KeyCode::F9, Key::Named(NamedKey::F9)).as_deref(),
			Some("F9")
		);
		assert_eq!(
			trigger(egui::Modifiers::ALT, KeyCode::Digit1, prints("1")).as_deref(),
			Some("ALT+1")
		);
		assert_eq!(
			trigger(none, KeyCode::PageUp, Key::Named(NamedKey::PageUp)).as_deref(),
			Some("Page_Up")
		);
		// A keypad key is asked for as one, whatever NumLock makes it.
		assert_eq!(
			trigger(none, KeyCode::Numpad1, Key::Named(NamedKey::End)).as_deref(),
			Some("KP_1")
		);
		assert_eq!(
			trigger(none, KeyCode::NumpadEnter, Key::Named(NamedKey::Enter)).as_deref(),
			Some("KP_Enter")
		);
		// German's ö, by its Unicode keysym.
		assert_eq!(trigger(none, KeyCode::Semicolon, prints("ö")).as_deref(), Some("U00F6"));
		let unnamed = Binding {
			modifiers: none,
			key: PhysicalKey::Unidentified(NativeKeyCode::Xkb(200)),
			name: Key::Unidentified(winit::keyboard::NativeKey::Unidentified),
		};
		assert_eq!(super::trigger(&unnamed), None);
	}
}
