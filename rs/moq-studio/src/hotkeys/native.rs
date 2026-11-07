//! Hotkeys registered with the system through global-hotkey: on macOS by
//! where the key sits, and on Windows by the virtual key the foreground
//! window's layout puts there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use winit::keyboard::{KeyCode, PhysicalKey};

use super::Binding;

/// The system's hotkeys, registered from the window's thread.
pub struct Native {
	manager: GlobalHotKeyManager,
	/// The hotkeys registered now.
	held: Vec<HotKey>,
	/// The key each registered hotkey hears, by its id, which the platform's
	/// event handler reads.
	registered: Arc<Mutex<HashMap<u32, Binding>>>,
	/// The layout Windows matches a hotkey's virtual key through.
	#[cfg(windows)]
	layout: super::layout::Foreground,
}

impl Native {
	/// Hear registered keys from now on, sending each pressed to `fire`.
	/// On Windows, `ctx` is woken when the foreground window's layout
	/// changes, so the keys register again through it.
	pub fn new(
		ctx: &egui::Context,
		runtime: &tokio::runtime::Handle,
		fire: mpsc::Sender<Binding>,
	) -> Result<Self, global_hotkey::Error> {
		let manager = GlobalHotKeyManager::new()?;
		let registered: Arc<Mutex<HashMap<u32, Binding>>> = Arc::default();
		let heard = registered.clone();
		GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
			if event.state() != HotKeyState::Pressed {
				return;
			}
			if let Some(key) = heard.lock().unwrap().get(&event.id()) {
				let _ = fire.send(key.clone());
			}
		}));
		#[cfg(not(windows))]
		let _ = (ctx, runtime);
		Ok(Self {
			manager,
			held: Vec::new(),
			registered,
			#[cfg(windows)]
			layout: super::layout::Foreground::new(ctx.clone(), runtime),
		})
	}

	/// Whether the layout the keys are matched through changed since they
	/// were registered.
	pub fn relaid(&self) -> bool {
		#[cfg(windows)]
		return self.layout.moved();
		#[cfg(not(windows))]
		false
	}

	/// Register `keys` in place of the keys registered now, returning why the
	/// system refused each it would not hear.
	pub fn hear(&mut self, keys: &[Binding]) -> Vec<(Binding, String)> {
		#[cfg(windows)]
		self.layout.follow();
		for hotkey in self.held.drain(..) {
			if let Err(err) = self.manager.unregister(hotkey) {
				tracing::warn!(%hotkey, error = %err, "cannot unregister a hotkey");
			}
		}
		let mut refused = Vec::new();
		let mut ids = HashMap::new();
		for key in keys {
			let registered = self.hotkey(key).and_then(|hotkey| {
				self.manager.register(hotkey).map_err(refusal)?;
				Ok(hotkey)
			});
			match registered {
				Ok(hotkey) => {
					self.held.push(hotkey);
					ids.insert(hotkey.id(), key.clone());
				}
				Err(why) => refused.push((key.clone(), why)),
			}
		}
		*self.registered.lock().unwrap() = ids;
		refused
	}

	/// `key` as global-hotkey registers it, or why it cannot.
	fn hotkey(&self, key: &Binding) -> Result<HotKey, String> {
		let PhysicalKey::Code(code) = key.key else {
			return Err("global-hotkey cannot name this key, so it cannot be heard outside the studio".into());
		};
		// RegisterHotKey matches virtual keys, and both Enter keys are VK_RETURN.
		#[cfg(windows)]
		if code == KeyCode::NumpadEnter {
			return Err("Windows cannot tell keypad Enter from Enter outside the studio".into());
		}
		#[cfg(windows)]
		let code = match super::layout::relabeled(code) {
			true => self.layout.code(code),
			false => self::code(code),
		};
		#[cfg(not(windows))]
		let code = self::code(code);
		let code = code.ok_or("this key cannot be heard outside the studio")?;
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
		Ok(HotKey::new(Some(mods), code))
	}
}

/// What the platform said when it refused a key.
fn refusal(err: global_hotkey::Error) -> String {
	match err {
		global_hotkey::Error::AlreadyRegistered(_) => "another app holds this key".into(),
		err => format!("the platform refused this key: {err}"),
	}
}

/// The key global-hotkey names `key`, where it sits. Both name keys by the
/// W3C UI Events codes.
fn code(key: KeyCode) -> Option<Code> {
	Some(match key {
		KeyCode::Backquote => Code::Backquote,
		KeyCode::Backslash => Code::Backslash,
		KeyCode::BracketLeft => Code::BracketLeft,
		KeyCode::BracketRight => Code::BracketRight,
		KeyCode::Comma => Code::Comma,
		KeyCode::Digit0 => Code::Digit0,
		KeyCode::Digit1 => Code::Digit1,
		KeyCode::Digit2 => Code::Digit2,
		KeyCode::Digit3 => Code::Digit3,
		KeyCode::Digit4 => Code::Digit4,
		KeyCode::Digit5 => Code::Digit5,
		KeyCode::Digit6 => Code::Digit6,
		KeyCode::Digit7 => Code::Digit7,
		KeyCode::Digit8 => Code::Digit8,
		KeyCode::Digit9 => Code::Digit9,
		KeyCode::Equal => Code::Equal,
		KeyCode::IntlBackslash => Code::IntlBackslash,
		KeyCode::IntlRo => Code::IntlRo,
		KeyCode::IntlYen => Code::IntlYen,
		KeyCode::KeyA => Code::KeyA,
		KeyCode::KeyB => Code::KeyB,
		KeyCode::KeyC => Code::KeyC,
		KeyCode::KeyD => Code::KeyD,
		KeyCode::KeyE => Code::KeyE,
		KeyCode::KeyF => Code::KeyF,
		KeyCode::KeyG => Code::KeyG,
		KeyCode::KeyH => Code::KeyH,
		KeyCode::KeyI => Code::KeyI,
		KeyCode::KeyJ => Code::KeyJ,
		KeyCode::KeyK => Code::KeyK,
		KeyCode::KeyL => Code::KeyL,
		KeyCode::KeyM => Code::KeyM,
		KeyCode::KeyN => Code::KeyN,
		KeyCode::KeyO => Code::KeyO,
		KeyCode::KeyP => Code::KeyP,
		KeyCode::KeyQ => Code::KeyQ,
		KeyCode::KeyR => Code::KeyR,
		KeyCode::KeyS => Code::KeyS,
		KeyCode::KeyT => Code::KeyT,
		KeyCode::KeyU => Code::KeyU,
		KeyCode::KeyV => Code::KeyV,
		KeyCode::KeyW => Code::KeyW,
		KeyCode::KeyX => Code::KeyX,
		KeyCode::KeyY => Code::KeyY,
		KeyCode::KeyZ => Code::KeyZ,
		KeyCode::Minus => Code::Minus,
		KeyCode::Period => Code::Period,
		KeyCode::Quote => Code::Quote,
		KeyCode::Semicolon => Code::Semicolon,
		KeyCode::Slash => Code::Slash,
		KeyCode::Backspace => Code::Backspace,
		KeyCode::CapsLock => Code::CapsLock,
		KeyCode::ContextMenu => Code::ContextMenu,
		KeyCode::Enter => Code::Enter,
		KeyCode::Space => Code::Space,
		KeyCode::Tab => Code::Tab,
		KeyCode::Convert => Code::Convert,
		KeyCode::KanaMode => Code::KanaMode,
		KeyCode::Lang1 => Code::Lang1,
		KeyCode::Lang2 => Code::Lang2,
		KeyCode::NonConvert => Code::NonConvert,
		KeyCode::Delete => Code::Delete,
		KeyCode::End => Code::End,
		KeyCode::Help => Code::Help,
		KeyCode::Home => Code::Home,
		KeyCode::Insert => Code::Insert,
		KeyCode::PageDown => Code::PageDown,
		KeyCode::PageUp => Code::PageUp,
		KeyCode::ArrowDown => Code::ArrowDown,
		KeyCode::ArrowLeft => Code::ArrowLeft,
		KeyCode::ArrowRight => Code::ArrowRight,
		KeyCode::ArrowUp => Code::ArrowUp,
		KeyCode::NumLock => Code::NumLock,
		KeyCode::Numpad0 => Code::Numpad0,
		KeyCode::Numpad1 => Code::Numpad1,
		KeyCode::Numpad2 => Code::Numpad2,
		KeyCode::Numpad3 => Code::Numpad3,
		KeyCode::Numpad4 => Code::Numpad4,
		KeyCode::Numpad5 => Code::Numpad5,
		KeyCode::Numpad6 => Code::Numpad6,
		KeyCode::Numpad7 => Code::Numpad7,
		KeyCode::Numpad8 => Code::Numpad8,
		KeyCode::Numpad9 => Code::Numpad9,
		KeyCode::NumpadAdd => Code::NumpadAdd,
		KeyCode::NumpadClear => Code::NumpadClear,
		KeyCode::NumpadComma => Code::NumpadComma,
		KeyCode::NumpadDecimal => Code::NumpadDecimal,
		KeyCode::NumpadDivide => Code::NumpadDivide,
		KeyCode::NumpadEnter => Code::NumpadEnter,
		KeyCode::NumpadEqual => Code::NumpadEqual,
		KeyCode::NumpadMultiply => Code::NumpadMultiply,
		KeyCode::NumpadSubtract => Code::NumpadSubtract,
		KeyCode::Escape => Code::Escape,
		KeyCode::PrintScreen => Code::PrintScreen,
		KeyCode::ScrollLock => Code::ScrollLock,
		KeyCode::Pause => Code::Pause,
		KeyCode::BrowserBack => Code::BrowserBack,
		KeyCode::BrowserFavorites => Code::BrowserFavorites,
		KeyCode::BrowserForward => Code::BrowserForward,
		KeyCode::BrowserHome => Code::BrowserHome,
		KeyCode::BrowserRefresh => Code::BrowserRefresh,
		KeyCode::BrowserSearch => Code::BrowserSearch,
		KeyCode::BrowserStop => Code::BrowserStop,
		KeyCode::LaunchApp1 => Code::LaunchApp1,
		KeyCode::LaunchApp2 => Code::LaunchApp2,
		KeyCode::LaunchMail => Code::LaunchMail,
		KeyCode::MediaPlayPause => Code::MediaPlayPause,
		KeyCode::MediaSelect => Code::MediaSelect,
		KeyCode::MediaStop => Code::MediaStop,
		KeyCode::MediaTrackNext => Code::MediaTrackNext,
		KeyCode::MediaTrackPrevious => Code::MediaTrackPrevious,
		KeyCode::Sleep => Code::Sleep,
		KeyCode::AudioVolumeDown => Code::AudioVolumeDown,
		KeyCode::AudioVolumeMute => Code::AudioVolumeMute,
		KeyCode::AudioVolumeUp => Code::AudioVolumeUp,
		KeyCode::F1 => Code::F1,
		KeyCode::F2 => Code::F2,
		KeyCode::F3 => Code::F3,
		KeyCode::F4 => Code::F4,
		KeyCode::F5 => Code::F5,
		KeyCode::F6 => Code::F6,
		KeyCode::F7 => Code::F7,
		KeyCode::F8 => Code::F8,
		KeyCode::F9 => Code::F9,
		KeyCode::F10 => Code::F10,
		KeyCode::F11 => Code::F11,
		KeyCode::F12 => Code::F12,
		KeyCode::F13 => Code::F13,
		KeyCode::F14 => Code::F14,
		KeyCode::F15 => Code::F15,
		KeyCode::F16 => Code::F16,
		KeyCode::F17 => Code::F17,
		KeyCode::F18 => Code::F18,
		KeyCode::F19 => Code::F19,
		KeyCode::F20 => Code::F20,
		KeyCode::F21 => Code::F21,
		KeyCode::F22 => Code::F22,
		KeyCode::F23 => Code::F23,
		KeyCode::F24 => Code::F24,
		_ => return None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keypad_keys_register_apart_from_the_main_row() {
		assert_eq!(code(KeyCode::Numpad1), Some(Code::Numpad1));
		assert_eq!(code(KeyCode::Digit1), Some(Code::Digit1));
		assert_eq!(code(KeyCode::NumpadSubtract), Some(Code::NumpadSubtract));
		assert_eq!(code(KeyCode::ShiftLeft), None);
	}
}
