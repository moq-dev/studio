//! Interaction with browser sources through the preview: the pointer goes to
//! the page under it, and the keyboard to the page last clicked.

use std::time::Duration;

use kio::clock::Instant;
use moq_scene::browser::{Button, Input, Modifiers};
use moq_scene::source::Sources;
use moq_scene::{Collection, Source};

/// How close in time and space a press must follow the last to count toward a
/// double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(500);
const MULTI_CLICK_SLOP: i32 = 4;

/// Which page has the pointer and the keyboard.
#[derive(Default)]
pub struct Interact {
	/// The page under the pointer, so it hears when the pointer leaves.
	hovered: Option<String>,
	/// The page last clicked, which receives the keyboard.
	focused: Option<String>,
	/// The last press, to count multi-clicks.
	last: Option<Press>,
}

struct Press {
	page: String,
	button: Button,
	x: i32,
	y: i32,
	at: Instant,
	clicks: u32,
}

/// A point on one page.
struct Hit {
	page: String,
	x: i32,
	y: i32,
}

impl Interact {
	/// Forward this frame's input over `image`, which shows `scene` of
	/// `collection` on `canvas`, to the pages it touches.
	pub fn update(
		&mut self,
		ui: &egui::Ui,
		image: egui::Rect,
		collection: &Collection,
		scene: Option<&str>,
		sources: &Sources,
	) {
		let canvas = collection.canvas;
		let hit = |pos: egui::Pos2| -> Option<Hit> {
			if !image.contains(pos) {
				return None;
			}
			let x = (pos.x - image.min.x) / image.width() * canvas.width as f32;
			let y = (pos.y - image.min.y) / image.height() * canvas.height as f32;
			page_at(collection, scene, x, y)
		};
		let send = |page: &str, input: Input| {
			if let Some(page) = sources.get(page).and_then(|feed| feed.page()) {
				page.send(input);
			}
		};

		let (events, buttons) = ui.input(|input| (input.events.clone(), held(&input.pointer)));
		for event in events {
			match event {
				egui::Event::PointerMoved(pos) => {
					let hit = hit(pos);
					let page = hit.as_ref().map(|hit| hit.page.clone());
					if let Some(left) = self.hovered.take().filter(|left| Some(left) != page.as_ref()) {
						send(&left, Input::Leave { modifiers: buttons });
					}
					if let Some(Hit { page, x, y }) = hit {
						send(
							&page,
							Input::Move {
								x,
								y,
								modifiers: buttons,
							},
						);
						self.hovered = Some(page);
					}
				}
				egui::Event::PointerGone => {
					if let Some(left) = self.hovered.take() {
						send(&left, Input::Leave { modifiers: buttons });
					}
				}
				egui::Event::PointerButton {
					pos,
					button,
					pressed,
					modifiers,
				} => {
					let Some(button) = self::button(button) else { continue };
					let Some(Hit { page, x, y }) = hit(pos) else {
						if pressed {
							self.blur(&send);
						}
						continue;
					};
					let modifiers = keys(modifiers, buttons);
					let clicks = if pressed {
						self.press(&page, button, x, y)
					} else {
						self.last.as_ref().map_or(1, |last| last.clicks)
					};
					if pressed && self.focused.as_ref() != Some(&page) {
						self.blur(&send);
						send(&page, Input::Focus(true));
						self.focused = Some(page.clone());
					}
					send(
						&page,
						Input::Button {
							x,
							y,
							button,
							down: pressed,
							clicks,
							modifiers,
						},
					);
				}
				egui::Event::MouseWheel {
					unit, delta, modifiers, ..
				} => {
					let Some(Hit { page, x, y }) = ui.input(|input| input.pointer.hover_pos()).and_then(hit) else {
						continue;
					};
					// Pixels, as a page scrolls; a line is what a browser
					// scrolls per wheel notch.
					let scale = match unit {
						egui::MouseWheelUnit::Point => 1.0,
						egui::MouseWheelUnit::Line => 40.0,
						egui::MouseWheelUnit::Page => canvas.height as f32,
					};
					send(
						&page,
						Input::Wheel {
							x,
							y,
							dx: (delta.x * scale) as i32,
							dy: (delta.y * scale) as i32,
							modifiers: keys(modifiers, buttons),
						},
					);
				}
				egui::Event::Key {
					key,
					pressed,
					modifiers,
					..
				} => {
					let (Some(page), Some(code)) = (&self.focused, code(key)) else {
						continue;
					};
					let modifiers = keys(modifiers, buttons);
					send(
						page,
						Input::Key {
							code,
							down: pressed,
							modifiers,
						},
					);
					// A page sees Enter as typed text too, which is what
					// submits a form or breaks a line.
					if pressed && key == egui::Key::Enter {
						send(page, Input::Text("\r".into()));
					}
				}
				egui::Event::Text(text) => {
					if let Some(page) = &self.focused {
						send(page, Input::Text(text));
					}
				}
				_ => {}
			}
		}
	}

	/// Give the keyboard back, as when interaction ends.
	pub fn release(&mut self, sources: &Sources) {
		self.hovered = None;
		self.last = None;
		if let Some(page) = self.focused.take().and_then(|name| sources.get(&name)?.page().cloned()) {
			page.send(Input::Focus(false));
		}
	}

	fn blur(&mut self, send: &impl Fn(&str, Input)) {
		if let Some(page) = self.focused.take() {
			send(&page, Input::Focus(false));
		}
	}

	/// Count this press toward a multi-click, returning the count.
	fn press(&mut self, page: &str, button: Button, x: i32, y: i32) -> u32 {
		let at = Instant::now();
		let clicks = match &self.last {
			Some(last)
				if last.page == page
					&& last.button == button
					&& at.saturating_duration_since(last.at) <= MULTI_CLICK
					&& (last.x - x).abs() <= MULTI_CLICK_SLOP
					&& (last.y - y).abs() <= MULTI_CLICK_SLOP =>
			{
				last.clicks % 3 + 1
			}
			_ => 1,
		};
		self.last = Some(Press {
			page: page.to_string(),
			button,
			x,
			y,
			at,
			clicks,
		});
		clicks
	}
}

/// The topmost visible browser source in `scene` at canvas point (`x`, `y`),
/// and the point on its page.
fn page_at(collection: &Collection, scene: Option<&str>, x: f32, y: f32) -> Option<Hit> {
	let scene = collection.scene(scene?)?;
	scene.items.iter().rev().filter(|item| item.visible).find_map(|item| {
		let Some(Source::Browser { width, height, .. }) = collection.sources.get(&item.source) else {
			return None;
		};
		let rect = item.rect;
		let inside = x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height;
		if !inside || rect.width <= 0.0 || rect.height <= 0.0 {
			return None;
		}
		Some(Hit {
			page: item.source.clone(),
			x: ((x - rect.x) / rect.width * *width as f32) as i32,
			y: ((y - rect.y) / rect.height * *height as f32) as i32,
		})
	})
}

fn button(button: egui::PointerButton) -> Option<Button> {
	match button {
		egui::PointerButton::Primary => Some(Button::Left),
		egui::PointerButton::Middle => Some(Button::Middle),
		egui::PointerButton::Secondary => Some(Button::Right),
		_ => None,
	}
}

/// The mouse buttons held now.
fn held(pointer: &egui::PointerState) -> Modifiers {
	Modifiers {
		left: pointer.primary_down(),
		middle: pointer.middle_down(),
		right: pointer.secondary_down(),
		..Default::default()
	}
}

/// `buttons` plus the keys `modifiers` holds.
fn keys(modifiers: egui::Modifiers, buttons: Modifiers) -> Modifiers {
	Modifiers {
		shift: modifiers.shift,
		ctrl: modifiers.ctrl,
		alt: modifiers.alt,
		meta: modifiers.mac_cmd,
		..buttons
	}
}

/// The Windows virtual-key code for `key`, which CEF takes on every platform.
fn code(key: egui::Key) -> Option<i32> {
	use egui::Key;

	let letter = |offset: u8| Some(i32::from(b'A' + offset));
	let digit = |offset: u8| Some(i32::from(b'0' + offset));
	match key {
		Key::Backspace => Some(0x08),
		Key::Tab => Some(0x09),
		Key::Enter => Some(0x0d),
		Key::Escape => Some(0x1b),
		Key::Space => Some(0x20),
		Key::PageUp => Some(0x21),
		Key::PageDown => Some(0x22),
		Key::End => Some(0x23),
		Key::Home => Some(0x24),
		Key::ArrowLeft => Some(0x25),
		Key::ArrowUp => Some(0x26),
		Key::ArrowRight => Some(0x27),
		Key::ArrowDown => Some(0x28),
		Key::Insert => Some(0x2d),
		Key::Delete => Some(0x2e),
		Key::Num0 => digit(0),
		Key::Num1 => digit(1),
		Key::Num2 => digit(2),
		Key::Num3 => digit(3),
		Key::Num4 => digit(4),
		Key::Num5 => digit(5),
		Key::Num6 => digit(6),
		Key::Num7 => digit(7),
		Key::Num8 => digit(8),
		Key::Num9 => digit(9),
		Key::A => letter(0),
		Key::B => letter(1),
		Key::C => letter(2),
		Key::D => letter(3),
		Key::E => letter(4),
		Key::F => letter(5),
		Key::G => letter(6),
		Key::H => letter(7),
		Key::I => letter(8),
		Key::J => letter(9),
		Key::K => letter(10),
		Key::L => letter(11),
		Key::M => letter(12),
		Key::N => letter(13),
		Key::O => letter(14),
		Key::P => letter(15),
		Key::Q => letter(16),
		Key::R => letter(17),
		Key::S => letter(18),
		Key::T => letter(19),
		Key::U => letter(20),
		Key::V => letter(21),
		Key::W => letter(22),
		Key::X => letter(23),
		Key::Y => letter(24),
		Key::Z => letter(25),
		Key::F1 => Some(0x70),
		Key::F2 => Some(0x71),
		Key::F3 => Some(0x72),
		Key::F4 => Some(0x73),
		Key::F5 => Some(0x74),
		Key::F6 => Some(0x75),
		Key::F7 => Some(0x76),
		Key::F8 => Some(0x77),
		Key::F9 => Some(0x78),
		Key::F10 => Some(0x79),
		Key::F11 => Some(0x7a),
		Key::F12 => Some(0x7b),
		Key::Semicolon => Some(0xba),
		Key::Equals | Key::Plus => Some(0xbb),
		Key::Comma => Some(0xbc),
		Key::Minus => Some(0xbd),
		Key::Period => Some(0xbe),
		Key::Slash => Some(0xbf),
		Key::Backtick => Some(0xc0),
		Key::OpenBracket => Some(0xdb),
		Key::Backslash => Some(0xdc),
		Key::CloseBracket => Some(0xdd),
		Key::Quote => Some(0xde),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use moq_scene::{Item, Rect, Scene};

	use super::*;

	/// The pointer lands on the topmost visible page, scaled into its own
	/// pixels however it is stretched on the canvas.
	#[test]
	fn the_pointer_lands_on_the_topmost_page() {
		let mut collection = Collection::new();
		let page = |width, height| Source::Browser {
			url: "https://example.com".into(),
			width,
			height,
		};
		collection.sources.insert("chat".into(), page(400, 800));
		collection.sources.insert("alerts".into(), page(800, 600));
		let item = |source: &str, rect, visible| Item {
			source: source.into(),
			rect,
			visible,
		};
		let fill = Rect::fill(&collection.canvas);
		let corner = Rect {
			x: 1520.0,
			y: 280.0,
			width: 400.0,
			height: 800.0,
		};
		collection.scenes = vec![Scene {
			name: "Main".into(),
			items: vec![item("alerts", fill, true), item("chat", corner, true)],
		}];
		let at =
			|collection: &Collection, x, y| page_at(collection, Some("Main"), x, y).map(|hit| (hit.page, hit.x, hit.y));

		assert_eq!(at(&collection, 1720.0, 680.0), Some(("chat".into(), 200, 400)));
		assert_eq!(at(&collection, 960.0, 540.0), Some(("alerts".into(), 400, 300)));

		collection.scenes[0].items[1].visible = false;
		assert_eq!(
			at(&collection, 1720.0, 680.0),
			Some(("alerts".into(), 716, 377)),
			"a hidden page takes no input"
		);
		assert!(page_at(&collection, Some("Other"), 1.0, 1.0).is_none());
	}

	/// Presses close in time and place count up to a triple click, then start
	/// over; a press elsewhere is a single click.
	#[test]
	fn presses_count_multi_clicks() {
		let mut interact = Interact::default();
		assert_eq!(interact.press("chat", Button::Left, 10, 10), 1);
		assert_eq!(interact.press("chat", Button::Left, 11, 10), 2);
		assert_eq!(interact.press("chat", Button::Left, 11, 11), 3);
		assert_eq!(interact.press("chat", Button::Left, 11, 11), 1);
		assert_eq!(interact.press("chat", Button::Left, 100, 11), 1);
		assert_eq!(interact.press("chat", Button::Right, 100, 11), 1);
	}
}
