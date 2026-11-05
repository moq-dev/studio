//! Interaction with browser sources through the preview: the pointer goes to
//! the page under it, and the keyboard to the page last clicked.

use std::borrow::Cow;
use std::cell::RefCell;
use std::time::Duration;

use kio::clock::Instant;
use moq_scene::browser::{Button, Input, Modifiers};
use moq_scene::{Collection, Crop, Item, Layer, Rect, Source};

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
	/// The pressed item, by its index among the scene's items then the top
	/// layer's, since the same page can be in several, and its keyframes move
	/// it during the drag.
	slot: usize,
	/// Where the pressed item placed the page, for when the slot no longer
	/// shows it.
	place: Place,
	/// The last point the page heard, where a release lands that comes from
	/// no pointer.
	at: (i32, i32),
	/// The buttons the page saw go down and not yet up.
	held: Vec<Button>,
}

/// A point on one page, placed on the canvas by `place`, the item at `slot`.
struct Hit {
	page: String,
	slot: usize,
	place: Place,
	x: i32,
	y: i32,
}

/// What part of a page an item shows, and where on the canvas.
#[derive(Clone, Copy)]
struct Place {
	rect: Rect,
	crop: Crop,
}

impl Interact {
	/// Turn this frame's input over `image`, which shows `scene` of
	/// `collection` under its top layer on `canvas`, the main one for `None`,
	/// into what each page it touches should receive, by page name. `scene` is
	/// the scene's name and how long it has been in the preview, which places
	/// its items where their keyframes have them.
	pub fn update(
		&mut self,
		ui: &egui::Ui,
		image: egui::Rect,
		collection: &Collection,
		canvas: Option<&str>,
		scene: Option<(&str, Duration)>,
	) -> Vec<(String, Input)> {
		let Some(size) = collection.canvas(canvas) else {
			return Vec::new();
		};
		let on_canvas = |pos: egui::Pos2| {
			let x = (pos.x - image.min.x) / image.width() * size.width as f32;
			let y = (pos.y - image.min.y) / image.height() * size.height as f32;
			(x, y)
		};
		let hit = |pos: egui::Pos2| -> Option<Hit> {
			if !image.contains(pos) {
				return None;
			}
			let (x, y) = on_canvas(pos);
			page_at(collection, canvas, scene, x, y)
		};
		// Where `pos` falls on the captured page, even off its edges, where the
		// pressed item shows it now.
		let on_page = |capture: &Capture, pos: egui::Pos2| -> Option<(i32, i32)> {
			let (x, y) = on_canvas(pos);
			let item = shown(collection, canvas, scene).into_iter().nth(capture.slot);
			let place = item
				.filter(|item| collection.capture(&item.source) == capture.page)
				.map_or(capture.place, |item| Place::of(collection, &item));
			scale(collection, &capture.page, place, x, y)
		};
		let out = RefCell::new(Vec::new());
		let send = |page: &str, input: Input| out.borrow_mut().push((page.to_string(), input));

		// The buttons and keys held now, for events that carry no modifiers
		// of their own.
		let (events, current) = ui.input(|input| (input.events.clone(), keys(input.modifiers, held(&input.pointer))));
		for event in events {
			match event {
				egui::Event::PointerMoved(pos) => {
					if let Some(capture) = &mut self.captured {
						if let Some((x, y)) = on_page(capture, pos) {
							capture.at = (x, y);
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
						// Every press hears its release, at the last point the page
						// heard if keyframes have since shrunk it to nothing.
						let (x, y) = on_page(capture, pos).unwrap_or(capture.at);
						capture.at = (x, y);
						let page = capture.page.clone();
						if capture.held.is_empty() {
							self.captured = None;
						}
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
						continue;
					}
					// Another press during a drag goes to the dragged page too.
					let target = match &self.captured {
						Some(capture) => on_page(capture, pos).map(|(x, y)| Hit {
							page: capture.page.clone(),
							slot: capture.slot,
							place: capture.place,
							x,
							y,
						}),
						None => hit(pos),
					};
					let Some(Hit {
						page,
						slot,
						place,
						x,
						y,
					}) = target
					else {
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
						slot,
						place,
						at: (x, y),
						held: Vec::new(),
					});
					capture.at = (x, y);
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
						egui::MouseWheelUnit::Page => size.height as f32,
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
				// egui swallows the copy and cut shortcuts too, which go to
				// the page as commands: only it knows its selection, which
				// comes back for the clipboard.
				egui::Event::Copy => {
					if let Some(page) = &self.focused {
						send(page, Input::Copy);
					}
				}
				egui::Event::Cut => {
					if let Some(page) = &self.focused {
						send(page, Input::Cut);
					}
				}
				// A release or a key typed in another app never arrives here,
				// so let go of the page as a native window does on losing
				// focus.
				egui::Event::WindowFocused(false) => {
					for (page, input) in self.release() {
						send(&page, input);
					}
				}
				_ => {}
			}
		}
		out.into_inner()
	}

	/// The pages this holds the pointer, the keyboard, or a drag for.
	pub fn pages(&self) -> impl Iterator<Item = &str> {
		let captured = self.captured.as_ref().map(|capture| &capture.page);
		[&self.hovered, &self.focused]
			.into_iter()
			.flatten()
			.chain(captured)
			.map(String::as_str)
	}

	/// Drop what this holds for `page`, which stopped or restarted, so it hears
	/// nothing more: a restarted page starts with nothing held.
	pub fn forget(&mut self, page: &str) {
		let is = |name: &String| name == page;
		self.hovered.take_if(|name| is(name));
		self.focused.take_if(|name| is(name));
		self.captured.take_if(|capture| is(&capture.page));
		self.last.take_if(|press| is(&press.page));
	}

	/// Whether a page has the keyboard, so the studio's own shortcuts leave
	/// it alone.
	pub fn has_keyboard(&self) -> bool {
		self.focused.is_some()
	}

	/// Let go of every page, as a native window does on losing focus or when
	/// interaction ends: release the buttons a page saw go down, tell the page
	/// under the pointer it left, and give the keyboard back. What each page
	/// should receive.
	pub fn release(&mut self) -> Vec<(String, Input)> {
		let mut out = Vec::new();
		if let Some(capture) = self.captured.take() {
			let (x, y) = capture.at;
			for button in capture.held {
				let up = Input::Button {
					x,
					y,
					button,
					down: false,
					clicks: 1,
					modifiers: Modifiers::default(),
				};
				out.push((capture.page.clone(), up));
			}
		}
		if let Some(left) = self.hovered.take() {
			let leave = Input::Leave {
				modifiers: Modifiers::default(),
			};
			out.push((left, leave));
		}
		if let Some(page) = self.focused.take() {
			out.push((page, Input::Focus(false)));
		}
		self.last = None;
		out
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

/// The items of `scene` on `canvas`, the main one for `None`, given as a
/// scene's name and how long it has been on, then of that canvas's top layer,
/// back to front, each where its keyframes have it.
fn shown<'a>(collection: &'a Collection, canvas: Option<&str>, scene: Option<(&str, Duration)>) -> Vec<Cow<'a, Item>> {
	let since = scene.map_or(Duration::ZERO, |(_, since)| since);
	let layer = |scene: Option<&str>| Layer {
		canvas: canvas.map(str::to_string),
		scene: scene.map(str::to_string),
	};
	let items = |layer: Layer| collection.items(&layer).unwrap_or_default();
	let scene = scene.map(|(name, _)| items(layer(Some(name)))).unwrap_or_default();
	scene
		.iter()
		.chain(items(layer(None)))
		.map(|item| item.posed(since))
		.collect()
}

/// The topmost visible browser source, or clone of one, at canvas point (`x`,
/// `y`) in `scene` and the top layer over it on `canvas`, as [`shown`] places
/// them, and the point on its page.
fn page_at(
	collection: &Collection,
	canvas: Option<&str>,
	scene: Option<(&str, Duration)>,
	x: f32,
	y: f32,
) -> Option<Hit> {
	let items = shown(collection, canvas, scene);
	items
		.iter()
		.enumerate()
		.rev()
		.filter(|(_, item)| item.visible)
		.find_map(|(slot, item)| {
			let rect = item.rect;
			let inside = x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height;
			if !inside {
				return None;
			}
			// A clone shows its source's page, which runs under that name.
			let page = collection.capture(&item.source);
			let place = Place::of(collection, item);
			let (x, y) = scale(collection, page, place, x, y)?;
			Some(Hit {
				page: page.to_string(),
				slot,
				place,
				x,
				y,
			})
		})
}

impl Place {
	/// Where `item` places its page.
	fn of(collection: &Collection, item: &Item) -> Self {
		Self {
			rect: item.rect,
			crop: collection.crop(item),
		}
	}
}

/// Canvas point (`x`, `y`) in the pixels of `page` placed by `place`, even off
/// the page's edges, if `page` is a browser source.
fn scale(collection: &Collection, page: &str, place: Place, x: f32, y: f32) -> Option<(i32, i32)> {
	let Some(Source::Browser { width, height, .. }) = collection.sources.get(page) else {
		return None;
	};
	let Place { rect, crop } = place;
	if rect.width <= 0.0 || rect.height <= 0.0 {
		return None;
	}
	// The crop's part of the page is stretched over the item.
	let u = crop.left + (x - rect.x) / rect.width * (1.0 - crop.left - crop.right);
	let v = crop.top + (y - rect.y) / rect.height * (1.0 - crop.top - crop.bottom);
	Some(((u * *width as f32) as i32, (v * *height as f32) as i32))
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
	use moq_scene::Scene;

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
			visible,
			..Item::new(source, rect)
		};
		let fill = Rect::fill(&collection.canvas);
		let corner = Rect {
			x: 1520.0,
			y: 280.0,
			width: 400.0,
			height: 800.0,
		};
		collection.scenes = vec![Scene {
			items: vec![item("alerts", fill, true), item("chat", corner, true)],
			..Scene::new("Main")
		}];
		let main = Some(("Main", Duration::ZERO));
		let at =
			|collection: &Collection, x, y| page_at(collection, None, main, x, y).map(|hit| (hit.page, hit.x, hit.y));

		assert_eq!(at(&collection, 1720.0, 680.0), Some(("chat".into(), 200, 400)));
		assert_eq!(at(&collection, 960.0, 540.0), Some(("alerts".into(), 400, 300)));

		collection.scenes[0].items[1].visible = false;
		assert_eq!(
			at(&collection, 1720.0, 680.0),
			Some(("alerts".into(), 716, 377)),
			"a hidden page takes no input"
		);
		assert!(page_at(&collection, None, Some(("Other", Duration::ZERO)), 1.0, 1.0).is_none());

		// The top layer draws over every scene, and a clone shows its source's
		// page, which runs under the source's name.
		collection
			.sources
			.insert("chat copy".into(), Source::Clone { source: "chat".into() });
		collection.top = vec![item("chat copy", corner, true)];
		assert_eq!(at(&collection, 1720.0, 680.0), Some(("chat".into(), 200, 400)));
		assert_eq!(
			page_at(&collection, None, None, 1720.0, 680.0).map(|hit| hit.page),
			Some("chat".into())
		);

		// A captured pointer maps onto its page past the edges.
		let place = Place {
			rect: corner,
			crop: Crop::default(),
		};
		assert_eq!(scale(&collection, "chat", place, 1420.0, 280.0), Some((-100, 0)));
		assert_eq!(scale(&collection, "chat", place, 960.0, 540.0), Some((-560, 260)));

		// A crop stretches what it leaves over the item: the source's right
		// half, then the item's bottom half of that.
		collection.top.clear();
		collection.video.insert(
			"chat".into(),
			moq_scene::Video {
				crop: Crop {
					left: 0.5,
					..Default::default()
				},
				..Default::default()
			},
		);
		collection.scenes[0].items[1].visible = true;
		collection.scenes[0].items[1].crop = Crop {
			top: 0.5,
			..Default::default()
		};
		assert_eq!(at(&collection, 1520.0, 280.0), Some(("chat".into(), 200, 400)));
		assert_eq!(at(&collection, 1720.0, 680.0), Some(("chat".into(), 300, 600)));

		// A keyframed item takes input where its keyframes have it.
		collection.scenes[0].items[1].keyframes = vec![moq_scene::Keyframe {
			x: -400.0,
			..moq_scene::Keyframe::new(Duration::ZERO)
		}];
		assert_eq!(at(&collection, 1320.0, 680.0), Some(("chat".into(), 300, 600)));
		let later = page_at(&collection, None, Some(("Main", Duration::from_secs(9))), 1320.0, 680.0);
		assert_eq!(
			later.map(|hit| hit.page),
			Some("chat".into()),
			"the last keyframe holds"
		);
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
			delivered = interact.update(ui, image, collection, None, Some(("Main", Duration::ZERO)));
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
		let item = |x| {
			let rect = Rect {
				x,
				y: 0.0,
				width: 100.0,
				height: 100.0,
			};
			Item::new("chat", rect)
		};
		collection.scenes = vec![Scene {
			items: vec![item(0.0), item(100.0)],
			..Scene::new("Main")
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

	/// A drag follows its item as the item's keyframes move it: a point held
	/// still on the moving page stays the same point on it, and a page shrunk
	/// to nothing still hears the release, where it last heard the pointer.
	#[test]
	fn a_drag_follows_its_moving_item() {
		let mut collection = Collection::new();
		collection.canvas.width = 400;
		collection.canvas.height = 100;
		collection.sources.insert(
			"chat".into(),
			Source::Browser {
				url: "https://example.com".into(),
				width: 100,
				height: 100,
			},
		);
		let rect = Rect {
			x: 200.0,
			y: 0.0,
			width: 100.0,
			height: 100.0,
		};
		let keyframes = vec![
			moq_scene::Keyframe {
				x: -200.0,
				..moq_scene::Keyframe::new(Duration::ZERO)
			},
			moq_scene::Keyframe {
				ease: moq_scene::Ease::Linear,
				..moq_scene::Keyframe::new(Duration::from_secs(2))
			},
			moq_scene::Keyframe {
				width: -100.0,
				..moq_scene::Keyframe::new(Duration::from_secs(3))
			},
		];
		collection.scenes = vec![Scene {
			items: vec![Item {
				keyframes,
				..Item::new("chat", rect)
			}],
			..Scene::new("Main")
		}];
		let ctx = egui::Context::default();
		let mut interact = Interact::default();
		let image = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 100.0));
		let mut run = |since, events| {
			let input = egui::RawInput {
				screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 200.0))),
				events,
				..Default::default()
			};
			let mut delivered = Vec::new();
			let mut output = ctx.run_ui(input, |ui| {
				delivered = interact.update(ui, image, &collection, None, Some(("Main", since)));
			});
			output.textures_delta.clear();
			delivered
		};
		let button = |x, pressed| egui::Event::PointerButton {
			pos: egui::pos2(x, 50.0),
			button: egui::PointerButton::Primary,
			pressed,
			modifiers: Default::default(),
		};

		// Pressed 50 px into the page while it is at x = 0.
		run(Duration::ZERO, vec![button(50.0, true)]);
		// A second later the page is at x = 100, and the pointer followed it.
		let delivered = run(
			Duration::from_secs(1),
			vec![egui::Event::PointerMoved(egui::pos2(150.0, 50.0)), button(150.0, false)],
		);
		let xs = |delivered: Vec<(String, Input)>| -> Vec<i32> {
			delivered
				.iter()
				.filter_map(|(_, input)| match input {
					Input::Move { x, .. } | Input::Button { x, .. } => Some(*x),
					_ => None,
				})
				.collect()
		};
		assert_eq!(xs(delivered), [50, 50]);

		// Pressed 50 px into the page where it rests, then released once it has
		// shrunk to nothing.
		run(Duration::from_secs(2), vec![button(250.0, true)]);
		let delivered = run(Duration::from_secs(3), vec![button(250.0, false)]);
		assert_eq!(xs(delivered), [50]);
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
		let item = |source: &str, x| {
			let rect = Rect {
				x,
				y: 0.0,
				width: 100.0,
				height: 100.0,
			};
			Item::new(source, rect)
		};
		collection.scenes = vec![Scene {
			items: vec![item("left", 0.0), item("right", 100.0)],
			..Scene::new("Main")
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
			items: vec![Item::new("chat", Rect::fill(&collection.canvas))],
			..Scene::new("Main")
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
				egui::Event::Cut,
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
				Input::Copy,
				Input::Cut,
			]
		);
	}

	/// Switching to another app mid-drag releases the page's button and its
	/// keyboard, since the release happens where the studio cannot see it.
	#[test]
	fn losing_the_window_lets_go_of_the_page() {
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
			items: vec![Item::new("chat", Rect::fill(&collection.canvas))],
			..Scene::new("Main")
		}];
		let ctx = egui::Context::default();
		let mut interact = Interact::default();
		let press = egui::Event::PointerButton {
			pos: egui::pos2(10.0, 10.0),
			button: egui::PointerButton::Primary,
			pressed: true,
			modifiers: Default::default(),
		};
		frame(&ctx, &mut interact, &collection, vec![press]);
		assert!(interact.has_keyboard());

		let delivered = frame(
			&ctx,
			&mut interact,
			&collection,
			vec![egui::Event::WindowFocused(false)],
		);
		let inputs: Vec<_> = delivered.into_iter().map(|(_, input)| input).collect();
		assert_eq!(
			inputs,
			[
				Input::Button {
					x: 10,
					y: 10,
					button: Button::Left,
					down: false,
					clicks: 1,
					modifiers: Modifiers::default(),
				},
				Input::Focus(false),
			]
		);
		assert!(!interact.has_keyboard());
		assert!(interact.captured.is_none());
	}

	/// A page that stopped or restarted is forgotten: it hears no release, and
	/// the studio's shortcuts come back.
	#[test]
	fn a_forgotten_page_hears_nothing() {
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
			items: vec![Item::new("chat", Rect::fill(&collection.canvas))],
			..Scene::new("Main")
		}];
		let ctx = egui::Context::default();
		let mut interact = Interact::default();
		let events = vec![
			egui::Event::PointerMoved(egui::pos2(10.0, 10.0)),
			egui::Event::PointerButton {
				pos: egui::pos2(10.0, 10.0),
				button: egui::PointerButton::Primary,
				pressed: true,
				modifiers: Default::default(),
			},
		];
		frame(&ctx, &mut interact, &collection, events);
		assert_eq!(interact.pages().collect::<Vec<_>>(), ["chat"; 3]);

		interact.forget("chat");
		assert!(!interact.has_keyboard());
		assert_eq!(interact.pages().count(), 0);
		assert!(interact.release().is_empty());
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
