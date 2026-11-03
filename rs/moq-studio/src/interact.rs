//! Interaction with browser sources through the preview: the pointer goes to
//! the page under it, and the keyboard to the page last clicked.

use std::cell::RefCell;
use std::time::Duration;

use kio::clock::Instant;
use moq_scene::browser::{Button, Input, Modifiers};
use moq_scene::{Collection, Rect, Source};

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
	/// The page a button went down on, which gets the pointer wherever it
	/// goes until every button it saw go down comes up, so a drag never
	/// leaves a page with a button stuck down.
	captured: Option<Capture>,
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

struct Capture {
	page: String,
	/// Where the pressed item places the page, since the same page can be in
	/// several.
	rect: Rect,
	/// The buttons the page saw go down and not yet up.
	held: Vec<Button>,
}

/// A point on one page, placed on the canvas at `rect`.
struct Hit {
	page: String,
	rect: Rect,
	x: i32,
	y: i32,
}

impl Interact {
	/// Turn this frame's input over `image`, which shows `scene` of
	/// `collection`, into what each page it touches should receive, by page
	/// name.
	pub fn update(
		&mut self,
		ui: &egui::Ui,
		image: egui::Rect,
		collection: &Collection,
		scene: Option<&str>,
	) -> Vec<(String, Input)> {
		let canvas = collection.canvas;
		let on_canvas = |pos: egui::Pos2| {
			let x = (pos.x - image.min.x) / image.width() * canvas.width as f32;
			let y = (pos.y - image.min.y) / image.height() * canvas.height as f32;
			(x, y)
		};
		let hit = |pos: egui::Pos2| -> Option<Hit> {
			if !image.contains(pos) {
				return None;
			}
			let (x, y) = on_canvas(pos);
			page_at(collection, scene, x, y)
		};
		// Where `pos` falls on the captured page, even off its edges.
		let on_page = |capture: &Capture, pos: egui::Pos2| -> Option<(i32, i32)> {
			let (x, y) = on_canvas(pos);
			scale(collection, &capture.page, capture.rect, x, y)
		};
		let out = RefCell::new(Vec::new());
		let send = |page: &str, input: Input| out.borrow_mut().push((page.to_string(), input));

		// The buttons and keys held now, for events that carry no modifiers
		// of their own.
		let (events, current) = ui.input(|input| (input.events.clone(), keys(input.modifiers, held(&input.pointer))));
		for event in events {
			match event {
				egui::Event::PointerMoved(pos) => {
					if let Some(capture) = &self.captured {
						if let Some((x, y)) = on_page(capture, pos) {
							send(
								&capture.page,
								Input::Move {
									x,
									y,
									modifiers: current,
								},
							);
						}
						continue;
					}
					let hit = hit(pos);
					let page = hit.as_ref().map(|hit| hit.page.clone());
					if let Some(left) = self.hovered.take().filter(|left| Some(left) != page.as_ref()) {
						send(&left, Input::Leave { modifiers: current });
					}
					if let Some(Hit { page, x, y, .. }) = hit {
						send(
							&page,
							Input::Move {
								x,
								y,
								modifiers: current,
							},
						);
						self.hovered = Some(page);
					}
				}
				egui::Event::PointerGone => {
					if let Some(left) = self.hovered.take() {
						send(&left, Input::Leave { modifiers: current });
					}
				}
				egui::Event::PointerButton {
					pos,
					button,
					pressed,
					modifiers,
				} => {
					let Some(button) = self::button(button) else { continue };
					let modifiers = keys(modifiers, current);
					if !pressed {
						// Only the page that saw the press hears the release.
						let Some(capture) = self.captured.as_mut().filter(|capture| capture.held.contains(&button))
						else {
							continue;
						};
						capture.held.retain(|held| *held != button);
						let point = on_page(capture, pos);
						let page = capture.page.clone();
						if capture.held.is_empty() {
							self.captured = None;
						}
						if let Some((x, y)) = point {
							let clicks = self.last.as_ref().map_or(1, |last| last.clicks);
							send(
								&page,
								Input::Button {
									x,
									y,
									button,
									down: false,
									clicks,
									modifiers,
								},
							);
						}
						continue;
					}
					// Another press during a drag goes to the dragged page too.
					let target = match &self.captured {
						Some(capture) => on_page(capture, pos).map(|(x, y)| Hit {
							page: capture.page.clone(),
							rect: capture.rect,
							x,
							y,
						}),
						None => hit(pos),
					};
					let Some(Hit { page, rect, x, y }) = target else {
						if self.captured.is_none() {
							self.blur(&send);
						}
						continue;
					};
					let clicks = self.press(&page, button, x, y);
					if self.focused.as_ref() != Some(&page) {
						self.blur(&send);
						send(&page, Input::Focus(true));
						self.focused = Some(page.clone());
					}
					let capture = self.captured.get_or_insert_with(|| Capture {
						page: page.clone(),
						rect,
						held: Vec::new(),
					});
					if !capture.held.contains(&button) {
						capture.held.push(button);
					}
					send(
						&page,
						Input::Button {
							x,
							y,
							button,
							down: true,
							clicks,
							modifiers,
						},
					);
				}
				egui::Event::MouseWheel {
					unit, delta, modifiers, ..
				} => {
					let Some(Hit { page, x, y, .. }) = ui.input(|input| input.pointer.hover_pos()).and_then(hit) else {
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
							modifiers: keys(modifiers, current),
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
					let modifiers = keys(modifiers, current);
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
				// egui turns a paste into its own event with the clipboard's
				// text, which the page takes as typed.
				egui::Event::Text(text) | egui::Event::Paste(text) => {
					if let Some(page) = &self.focused {
						send(page, Input::Text(text));
					}
				}
				// egui swallows the copy and cut shortcuts too, so the page
				// gets them back as keys: only it knows its selection.
				egui::Event::Copy | egui::Event::Cut => {
					let Some(page) = &self.focused else { continue };
					let code = i32::from(if event == egui::Event::Copy { b'C' } else { b'X' });
					for down in [true, false] {
						send(
							page,
							Input::Key {
								code,
								down,
								modifiers: current,
							},
						);
					}
				}
				_ => {}
			}
		}
		out.into_inner()
	}

	/// Give the keyboard back, as when interaction ends: what the page that
	/// had it should receive.
	pub fn release(&mut self) -> Option<(String, Input)> {
		self.hovered = None;
		self.captured = None;
		self.last = None;
		Some((self.focused.take()?, Input::Focus(false)))
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
		let rect = item.rect;
		let inside = x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height;
		if !inside {
			return None;
		}
		let (x, y) = scale(collection, &item.source, rect, x, y)?;
		Some(Hit {
			page: item.source.clone(),
			rect,
			x,
			y,
		})
	})
}

/// Canvas point (`x`, `y`) in the pixels of `page` placed at `rect`, even off
/// the page's edges, if `page` is a browser source.
fn scale(collection: &Collection, page: &str, rect: Rect, x: f32, y: f32) -> Option<(i32, i32)> {
	let Some(Source::Browser { width, height, .. }) = collection.sources.get(page) else {
		return None;
	};
	if rect.width <= 0.0 || rect.height <= 0.0 {
		return None;
	}
	Some((
		((x - rect.x) / rect.width * *width as f32) as i32,
		((y - rect.y) / rect.height * *height as f32) as i32,
	))
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
	use moq_scene::{Item, Scene};

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

		// A captured pointer maps onto its page past the edges.
		assert_eq!(scale(&collection, "chat", corner, 1420.0, 280.0), Some((-100, 0)));
		assert_eq!(scale(&collection, "chat", corner, 960.0, 540.0), Some((-560, 260)));
	}

	/// One frame of `events` over a preview showing the canvas at its own
	/// size, and what each page receives.
	fn frame(
		ctx: &egui::Context,
		interact: &mut Interact,
		collection: &Collection,
		events: Vec<egui::Event>,
	) -> Vec<(String, Input)> {
		let canvas = egui::vec2(collection.canvas.width as f32, collection.canvas.height as f32);
		let image = egui::Rect::from_min_size(egui::Pos2::ZERO, canvas);
		let input = egui::RawInput {
			screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, canvas * 2.0)),
			events,
			..Default::default()
		};
		let mut delivered = Vec::new();
		let mut output = ctx.run_ui(input, |ui| {
			delivered = interact.update(ui, image, collection, Some("Main"));
		});
		// Nothing renders, so the font atlas egui uploaded goes nowhere.
		output.textures_delta.clear();
		delivered
	}

	/// A page placed twice keeps a drag in the item it was pressed in, even
	/// under the other one.
	#[test]
	fn a_drag_stays_with_its_item() {
		let mut collection = Collection::new();
		collection.canvas.width = 200;
		collection.canvas.height = 100;
		collection.sources.insert(
			"chat".into(),
			Source::Browser {
				url: "https://example.com".into(),
				width: 100,
				height: 100,
			},
		);
		let item = |x| Item {
			source: "chat".into(),
			rect: Rect {
				x,
				y: 0.0,
				width: 100.0,
				height: 100.0,
			},
			visible: true,
		};
		collection.scenes = vec![Scene {
			name: "Main".into(),
			items: vec![item(0.0), item(100.0)],
		}];
		let ctx = egui::Context::default();
		let mut interact = Interact::default();
		let button = |x, pressed| egui::Event::PointerButton {
			pos: egui::pos2(x, 50.0),
			button: egui::PointerButton::Primary,
			pressed,
			modifiers: Default::default(),
		};

		let delivered = frame(
			&ctx,
			&mut interact,
			&collection,
			vec![
				button(50.0, true),
				egui::Event::PointerMoved(egui::pos2(60.0, 50.0)),
				button(60.0, false),
			],
		);
		let xs: Vec<_> = delivered
			.iter()
			.filter_map(|(_, input)| match input {
				Input::Move { x, .. } | Input::Button { x, .. } => Some(*x),
				_ => None,
			})
			.collect();
		assert_eq!(xs, [50, 60, 60]);
	}

	/// A drag that starts on a page stays with it: the moves and the release
	/// go to that page off its edges and over another page, and a release
	/// nobody pressed for goes nowhere.
	#[test]
	fn a_drag_stays_with_its_page() {
		let mut collection = Collection::new();
		collection.canvas.width = 200;
		collection.canvas.height = 100;
		let page = Source::Browser {
			url: "https://example.com".into(),
			width: 100,
			height: 100,
		};
		collection.sources.insert("left".into(), page.clone());
		collection.sources.insert("right".into(), page);
		let item = |source: &str, x| Item {
			source: source.into(),
			rect: Rect {
				x,
				y: 0.0,
				width: 100.0,
				height: 100.0,
			},
			visible: true,
		};
		collection.scenes = vec![Scene {
			name: "Main".into(),
			items: vec![item("left", 0.0), item("right", 100.0)],
		}];
		let ctx = egui::Context::default();
		let mut interact = Interact::default();
		let mut frame = |events| self::frame(&ctx, &mut interact, &collection, events);
		let press = |x, y, button, pressed| egui::Event::PointerButton {
			pos: egui::pos2(x, y),
			button,
			pressed,
			modifiers: Default::default(),
		};
		let left = egui::PointerButton::Primary;
		let right = egui::PointerButton::Secondary;
		let button = move |x, y, pressed| press(x, y, left, pressed);
		let pages =
			|delivered: &[(String, Input)]| -> Vec<String> { delivered.iter().map(|(page, _)| page.clone()).collect() };

		let pressed = frame(vec![
			egui::Event::PointerMoved(egui::pos2(50.0, 50.0)),
			button(50.0, 50.0, true),
		]);
		assert_eq!(pages(&pressed), ["left", "left", "left"], "move, focus, press");

		// Over the other page and then off the preview, still the left page's.
		let dragged = frame(vec![
			egui::Event::PointerMoved(egui::pos2(150.0, 50.0)),
			egui::Event::PointerMoved(egui::pos2(300.0, 50.0)),
			button(300.0, 50.0, false),
		]);
		assert_eq!(pages(&dragged), ["left", "left", "left"]);
		assert!(
			matches!(
				dragged[2].1,
				Input::Button {
					x: 300,
					down: false,
					..
				}
			),
			"{:?}",
			dragged[2].1
		);

		// A release with no press behind it is dropped.
		assert!(frame(vec![button(150.0, 50.0, false)]).is_empty());

		// Every button the page saw go down comes up there, whichever order
		// they are released in and wherever the pointer is.
		let chord = frame(vec![
			press(50.0, 50.0, left, true),
			press(150.0, 50.0, right, true),
			press(150.0, 50.0, left, false),
			press(150.0, 50.0, right, false),
		]);
		let buttons: Vec<_> = chord
			.iter()
			.filter_map(|(page, input)| match input {
				Input::Button { button, down, .. } => Some((page.as_str(), *button, *down)),
				_ => None,
			})
			.collect();
		assert_eq!(
			buttons,
			[
				("left", Button::Left, true),
				("left", Button::Right, true),
				("left", Button::Left, false),
				("left", Button::Right, false),
			]
		);
		assert!(
			frame(vec![press(150.0, 50.0, right, false)]).is_empty(),
			"the drag is over"
		);
	}

	/// What egui keeps for itself still reaches the page: the keys held during
	/// a drag, and the clipboard shortcuts it turns into events of their own.
	#[test]
	fn held_keys_and_the_clipboard_reach_the_page() {
		let mut collection = Collection::new();
		collection.canvas.width = 100;
		collection.canvas.height = 100;
		collection.sources.insert(
			"chat".into(),
			Source::Browser {
				url: "https://example.com".into(),
				width: 100,
				height: 100,
			},
		);
		collection.scenes = vec![Scene {
			name: "Main".into(),
			items: vec![Item {
				source: "chat".into(),
				rect: Rect::fill(&collection.canvas),
				visible: true,
			}],
		}];
		let delivered = frame(
			&egui::Context::default(),
			&mut Interact::default(),
			&collection,
			vec![
				egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
				egui::Event::PointerButton {
					pos: egui::pos2(10.0, 10.0),
					button: egui::PointerButton::Primary,
					pressed: true,
					modifiers: egui::Modifiers::SHIFT,
				},
				egui::Event::PointerMoved(egui::pos2(20.0, 10.0)),
				egui::Event::Paste("pasted".into()),
				egui::Event::Copy,
			],
		);
		let inputs: Vec<_> = delivered.into_iter().map(|(_, input)| input).collect();
		let shift = Modifiers {
			shift: true,
			left: true,
			..Default::default()
		};
		assert_eq!(
			inputs[2..],
			[
				Input::Move {
					x: 20,
					y: 10,
					modifiers: shift
				},
				Input::Text("pasted".into()),
				Input::Key {
					code: i32::from(b'C'),
					down: true,
					modifiers: shift
				},
				Input::Key {
					code: i32::from(b'C'),
					down: false,
					modifiers: shift
				},
			]
		);
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
