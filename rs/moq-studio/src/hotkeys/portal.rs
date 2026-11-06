//! Global shortcuts on Wayland, which lets no app hear keys meant for
//! another: the studio asks the desktop's GlobalShortcuts portal for each
//! binding, and the desktop's shortcut settings decide the keys.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use egui::{Key, KeyboardShortcut};
use futures_util::StreamExt;
use tokio::sync::watch;

use super::{Bindings, Reach};
use crate::controls::Action;

/// A session with the portal, kept by a task of its own.
pub struct Portal {
	/// The bindings the task asks the desktop for.
	bindings: watch::Sender<Bindings>,
	reach: Arc<Mutex<Reach>>,
}

impl Portal {
	pub fn new(runtime: &tokio::runtime::Handle, fire: mpsc::Sender<Action>) -> Self {
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
/// its own, and fire the actions it reports.
async fn run(
	mut wanted: watch::Receiver<Bindings>,
	fire: mpsc::Sender<Action>,
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
				.map(|(action, key)| {
					NewShortcut::new(id(action), action.to_string()).preferred_trigger(trigger(key).as_deref())
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
							Some((action(shortcut.id())?, shortcut.trigger_description().to_string()))
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
					// Only this session is open, so every activation is its own.
					if let Some(action) = action(event.shortcut_id()) {
						let _ = fire.send(action);
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
/// across runs so a key the operator picked there stays.
fn id(action: &Action) -> String {
	match action {
		Action::Cut => "cut".into(),
		Action::Transition => "transition".into(),
		Action::Scene(name) => format!("scene/{name}"),
		Action::Replay => "replay".into(),
		Action::SaveClip => "save-clip".into(),
		Action::Record => "record".into(),
	}
}

/// The action a shortcut `id` is kept under.
fn action(id: &str) -> Option<Action> {
	if let Some(name) = id.strip_prefix("scene/") {
		return Some(Action::Scene(name.into()));
	}
	Action::FIXED.into_iter().find(|action| self::id(action) == id)
}

/// `shortcut` as a trigger for the desktop to offer first, its key named by
/// its XKB keysym.
fn trigger(shortcut: &KeyboardShortcut) -> Option<String> {
	let held = shortcut.modifiers;
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
	trigger.push_str(&keysym(shortcut.logical_key)?);
	Some(trigger)
}

fn keysym(key: Key) -> Option<String> {
	let name = match key {
		Key::ArrowDown => "Down",
		Key::ArrowLeft => "Left",
		Key::ArrowRight => "Right",
		Key::ArrowUp => "Up",
		Key::Escape => "Escape",
		Key::Tab => "Tab",
		Key::Backspace => "BackSpace",
		Key::Enter => "Return",
		Key::Space => "space",
		Key::Insert => "Insert",
		Key::Delete => "Delete",
		Key::Home => "Home",
		Key::End => "End",
		Key::PageUp => "Page_Up",
		Key::PageDown => "Page_Down",
		Key::Comma => "comma",
		Key::Backslash => "backslash",
		Key::Slash => "slash",
		Key::OpenBracket => "bracketleft",
		Key::CloseBracket => "bracketright",
		Key::Backtick => "grave",
		Key::Minus => "minus",
		Key::Period => "period",
		Key::Equals => "equal",
		Key::Semicolon => "semicolon",
		Key::Quote => "apostrophe",
		// A letter or digit is its lower-case self, and a function key its
		// name, as egui names them.
		key => {
			let name = key.name();
			let alphanumeric = name.len() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric());
			let function = name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok();
			return match (alphanumeric, function) {
				(true, _) => Some(name.to_lowercase()),
				(_, true) => Some(name.into()),
				_ => None,
			};
		}
	};
	Some(name.into())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unchanged_bindings_keep_the_session() {
		let (bindings, mut wanted) = watch::channel(Bindings::new());
		let portal = Portal {
			bindings,
			reach: Arc::new(Mutex::new(Reach::Asking)),
		};
		let key = KeyboardShortcut::new(egui::Modifiers::CTRL, Key::R);
		portal.set(Bindings::from([(Action::Cut, key)]));
		assert!(wanted.has_changed().unwrap());
		wanted.mark_unchanged();
		// What the Hotkeys panel sets when it starts and stops waiting for a key.
		portal.set(Bindings::from([(Action::Cut, key)]));
		assert!(!wanted.has_changed().unwrap());
	}

	#[test]
	fn ids_round_trip() {
		for action in Action::FIXED.into_iter().chain([Action::Scene("Live/Intro".into())]) {
			assert_eq!(super::action(&id(&action)), Some(action));
		}
		assert_eq!(super::action("unknown"), None);
	}

	#[test]
	fn triggers_name_keysyms() {
		let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
		let trigger = |modifiers, key| trigger(&KeyboardShortcut::new(modifiers, key));
		assert_eq!(trigger(ctrl_shift, Key::R).as_deref(), Some("CTRL+SHIFT+r"));
		assert_eq!(trigger(egui::Modifiers::NONE, Key::F9).as_deref(), Some("F9"));
		assert_eq!(trigger(egui::Modifiers::ALT, Key::Num1).as_deref(), Some("ALT+1"));
		assert_eq!(trigger(egui::Modifiers::NONE, Key::PageUp).as_deref(), Some("Page_Up"));
		assert_eq!(trigger(egui::Modifiers::NONE, Key::Colon), None);
	}
}
