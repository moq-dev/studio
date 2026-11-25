//! Add: a modal over the stage listing what can go in the edited scene, by
//! name, each card adding it in one click, or where it is dragged onto the
//! picture.

use egui::{Align2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::drop::Arrival;
use super::layers::{kind, unplaced, words};
use super::*;

/// A card's height.
const CARD: f32 = 104.0;
/// The narrowest a card gets before a row holds one fewer.
const CARD_MIN: f32 = 180.0;
/// Room between cards.
const GAP: f32 = 10.0;
/// Add's widest.
const WIDE: f32 = 780.0;

/// How much of the canvas's width a picture-in-picture takes.
const PIP: f32 = 0.3;
/// How far a picture-in-picture sits from the canvas's edges, as a part of
/// its width.
const MARGIN: f32 = 0.025;
/// How much of the canvas a layer must cover to hide what is behind it.
const COVERS: f32 = 0.9;

/// Add's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
	#[default]
	Devices,
	Media,
	Audio,
	Guests,
}

impl Tab {
	const ALL: [Self; 4] = [Self::Devices, Self::Media, Self::Audio, Self::Guests];

	fn name(self) -> &'static str {
		match self {
			Self::Devices => "Devices & Screens",
			Self::Media => "Media & Web",
			Self::Audio => "Audio",
			Self::Guests => "Guests",
		}
	}
}

/// Add's own state.
pub(super) struct Adding {
	/// Whether it is open over the stage.
	open: bool,
	pub(super) tab: Tab,
	/// The web address typed so far.
	url: String,
	/// The color the Color card adds.
	color: Color32,
	/// The microphones' levels while the Audio tab shows.
	meters: Meters,
}

impl Default for Adding {
	fn default() -> Self {
		Self {
			open: false,
			tab: Tab::default(),
			url: String::new(),
			color: Color32::from_rgb(0x1f, 0x24, 0x33),
			meters: Meters::default(),
		}
	}
}

impl Adding {
	/// Close it, letting go of the microphones its meters hold.
	pub(super) fn close(&mut self) {
		self.open = false;
		self.meters = Meters::default();
	}
}

/// What a card adds, which a drag onto the picture carries.
#[derive(Clone, Debug)]
pub(super) enum Card {
	/// A new source, named after the pick, numbered past any of that name.
	New(Pick),
	/// A source the show has that nothing shows or hears.
	Unused(String),
}

/// A card as it draws: its icon, its name and the line under it, and what
/// it adds, or why it cannot.
struct Entry {
	icon: Icon,
	name: String,
	line: String,
	card: Result<Card, String>,
	/// Its picture's color, for a color.
	swatch: Option<Color32>,
}

/// What a click in Add asks for.
enum Asked {
	Card(Card),
	/// Remove an unused source for good.
	Remove(String),
	/// The system's file dialog for an image or video.
	Media,
	Page(url::Url),
	/// List the devices again, after the user fixed what stopped them.
	Retry,
}

impl Panels {
	/// Open Add, listing the devices afresh.
	pub(super) fn open_add(&mut self) {
		self.adding.open = true;
		self.devices.renew(&self.runtime);
	}

	/// Close Add, if open.
	pub(super) fn close_add(&mut self) {
		self.adding.close();
	}

	/// Draw Add over `stage` while it is open, hidden while one of its cards
	/// is dragged so the picture under it can take the drop.
	pub(super) fn add_modal(&mut self, ui: &mut Ui, stage: Rect) {
		if !self.adding.open {
			return;
		}
		let ctx = ui.ctx().clone();
		self.devices.poll();
		if self.devices.pending.is_some() {
			ctx.request_repaint_after(Duration::from_millis(100));
		}
		if let Some(card) = egui::DragAndDrop::payload::<Card>(&ctx) {
			dragged(&ctx, &card);
			return;
		}
		let scrim = egui::Area::new(egui::Id::new("add scrim"))
			.order(egui::Order::Middle)
			.fixed_pos(stage.min)
			.show(&ctx, |ui| {
				let (rect, response) = ui.allocate_exact_size(stage.size(), Sense::click());
				ui.painter().rect_filled(rect, 0, kit::BG.gamma_multiply(0.7));
				kit::named(response, "Beside Add")
			});
		let width = (stage.width() - 48.0).clamp(320.0, WIDE);
		let tall = (stage.height() - 170.0).max(160.0);
		let frame = egui::Frame::new()
			.fill(kit::P1)
			.stroke(Stroke::new(1.0, kit::LINE))
			.corner_radius(14)
			.inner_margin(egui::Margin::same(18))
			.shadow(egui::Shadow {
				offset: [0, 24],
				blur: 60,
				spread: 0,
				color: Color32::from_black_alpha(200),
			});
		let title = {
			let state = self.engine.state.lock().unwrap();
			match self.edited(&state) {
				Some(scene) => format!("Add to {scene}"),
				None => "Add".to_string(),
			}
		};
		let mut closed = scrim.inner.clicked();
		let mut asked = None;
		// Centered on the stage by its size last frame, laid out from its top
		// so it has the stage's height to grow into.
		let id = egui::Id::new("add");
		let last = ctx
			.memory(|memory| memory.area_rect(id))
			.map_or(Vec2::ZERO, |rect| rect.size());
		let top = (stage.center().y - last.y / 2.0).max(stage.top() + 24.0);
		let left = stage.center().x - (width + 36.0) / 2.0;
		egui::Area::new(id)
			.order(egui::Order::Foreground)
			.fixed_pos(pos2(left, top))
			.show(&ctx, |ui| {
				// An area lays out in its size last frame, which would keep the
				// list as short as it first was.
				let room = Rect::from_min_size(ui.max_rect().min, vec2(width + 36.0, stage.height()));
				ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
					frame.show(ui, |ui| {
						ui.set_width(width);
						ui.horizontal(|ui| {
							ui.label(egui::RichText::new(&title).font(kit::ui_strong(17.0)).color(kit::TEXT));
							ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
								let close = ui.add(IconButton::new(Icon::Close, "Close").key(Some("Esc")));
								closed |= kit::named(close, "Close Add").clicked();
							});
						});
						ui.add_space(8.0);
						let mut tab = self.adding.tab;
						super::shell::tabs(ui, &mut tab, &Tab::ALL, Tab::name);
						self.adding.tab = tab;
						ui.add_space(10.0);
						if let Some(fault) = self.fault(&Spot::Add) {
							ui.colored_label(kit::ERR, fault);
							ui.add_space(6.0);
						}
						egui::ScrollArea::vertical()
							.id_salt(("add", tab))
							.max_height(tall)
							.auto_shrink([false, true])
							.show(ui, |ui| {
								asked = match tab {
									Tab::Devices => self.devices_tab(ui),
									Tab::Media => self.media_tab(ui),
									Tab::Audio => self.audio_tab(ui),
									Tab::Guests => self.guests_tab(ui),
								};
							});
					});
				});
			});
		if self.adding.tab != Tab::Audio {
			self.adding.meters = Meters::default();
		}
		if closed {
			self.close_add();
			return;
		}
		match asked {
			Some(Asked::Card(card)) => self.take_card(&card, None),
			Some(Asked::Remove(name)) => {
				let result = edit(
					&mut self.engine.state.lock().unwrap(),
					format!("Remove `{name}`"),
					None,
					|draft| draft.remove_source(&name),
				);
				self.fail(Spot::Add, result);
			}
			Some(Asked::Media) => {
				self.quick.dialog = Some(super::start::media_dialog(&self.runtime));
				self.close_add();
			}
			Some(Asked::Page(url)) => {
				self.adding.url.clear();
				self.dropping.arrive(Arrival::Page(url), None);
				self.close_add();
			}
			Some(Asked::Retry) => {
				self.devices.listing = None;
				self.devices.renew(&self.runtime);
			}
			None => {}
		}
	}

	/// The page's keys while Add is open: Escape closes it, and no other
	/// reaches the picture under it. Whether it is open.
	pub(super) fn add_keys(&mut self, ui: &Ui) -> bool {
		if !self.adding.open {
			return false;
		}
		if !ui.ctx().text_edit_focused()
			&& ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
		{
			self.close_add();
		}
		true
	}

	/// Add what `card` adds to the edited scene, centered on `at` in canvas
	/// pixels if given, then close Add with its layer selected.
	pub(super) fn take_card(&mut self, card: &Card, at: Option<[f32; 2]>) {
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		match card {
			Card::New(pick) => {
				// Two cameras of the same model share a name; number the later ones.
				let name = state.collection().unused_name(&pick.name);
				let layer = self.scene(&state).ok();
				let new = New {
					name,
					source: pick.source.clone(),
					layer,
					at,
				};
				let result = add(&mut state, new);
				drop(state);
				self.added(result);
			}
			Card::Unused(name) => {
				let result = self.place_unused(&mut state, name, at);
				drop(state);
				self.added(result.map(|()| name.clone()));
			}
		}
	}

	/// Put `name`, a source nothing shows, in the edited scene, centered on
	/// `at` if given, or in the next guest tile of a scene with slots if it
	/// is a guest's.
	fn place_unused(&mut self, state: &mut State, name: &str, at: Option<[f32; 2]>) -> anyhow::Result<()> {
		let layer = self.scene(state).map_err(|why| anyhow::anyhow!(why))?;
		let collection = state.collection();
		let source = collection.sources.get(name).context("the source is gone")?;
		let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
		let (mut index, rect) = place(collection, &layer, canvas, source, at);
		let mut item = Item::new(name.to_string(), rect);
		// A guest takes the next tile of a scene with slots.
		let room = self.engine.room();
		let guest = room
			.as_ref()
			.is_some_and(|room| crate::guests::plays_guest(room.path(), source));
		let tile = layer
			.scene
			.as_deref()
			.and_then(|scene| crate::guests::tile(collection, scene, name));
		if let Some(at) = tile.filter(|_| guest) {
			index = at;
			item = Item {
				slot: Some(moq_scene::Slot::Tile),
				..Item::new(name.to_string(), moq_scene::Rect::default())
			};
		}
		// Its sound joins the stream with it, as a new source's does.
		let heard = source.has_audio();
		edit(state, format!("Add `{name}` {}", super::place(&layer)), None, |draft| {
			draft.insert_item(&layer, index, item)?;
			if heard {
				draft.hear(name)?;
			}
			Ok(())
		})
	}

	/// Follow an added source to its layer and close Add, or show why it was
	/// not added.
	pub(super) fn added(&mut self, result: anyhow::Result<String>) {
		match result {
			Ok(name) => {
				let state = self.engine.state.lock().unwrap();
				let picked = self.scene(&state).ok().and_then(|layer| {
					let items = state.collection().items(&layer)?;
					let index = items.iter().rposition(|item| item.source == name)?;
					Some(Picked { layer, index })
				});
				drop(state);
				match picked {
					Some(picked) => {
						self.select(Some(picked));
						self.side = Side::Layers;
					}
					// A sound alone has no layer to show it.
					None => self.say(Ok(format!("added `{name}`"))),
				}
				self.fail(Spot::Add, Ok(()));
				self.close_add();
			}
			Err(err) => self.fail(Spot::Add, Err(err)),
		}
	}

	/// The sources nothing shows or hears, the guests' if `guests` and the
	/// rest otherwise, each as a card.
	fn unused(&self, guests: bool) -> Vec<Entry> {
		let state = self.engine.state.lock().unwrap();
		let collection = state.collection();
		let room = self.engine.room();
		let heard = |name: &str| collection.audio_scenes.iter().any(|scene| scene.sounds.contains(name));
		collection
			.sources
			.iter()
			.filter(|(name, source)| unplaced(collection, name) && !heard(name) && source.has_video())
			.filter(|(_, source)| {
				let guest = room
					.as_ref()
					.is_some_and(|room| crate::guests::plays_guest(room.path(), source));
				guest == guests
			})
			.map(|(name, _)| Entry {
				icon: kind(collection, name),
				name: name.clone(),
				line: words(collection, name),
				card: Ok(Card::Unused(name.clone())),
				swatch: None,
			})
			.collect()
	}

	/// The sources nothing shows, then this machine's cameras, screens, and
	/// windows.
	fn devices_tab(&mut self, ui: &mut Ui) -> Option<Asked> {
		let mut asked = None;
		let unused = self.unused(false);
		if !unused.is_empty() {
			caption(ui, "Not in any scene", Beside::Nothing);
			asked = asked.or(cards(ui, "unused", &unused, true));
		}
		// Never the studio's own virtual cameras, which would feed the
		// picture back into itself.
		let own: Vec<String> = self
			.engine
			.state
			.lock()
			.unwrap()
			.cameras()
			.map(|camera| camera.id)
			.collect();
		let Some(listing) = &self.devices.listing else {
			looking(ui);
			return asked;
		};
		let is_own = |pick: &Pick| matches!(&pick.source, Source::Camera { id: Some(id) } if own.contains(id));
		let kinds = [
			("Cameras", Icon::Camera, &listing.cameras),
			("Screens", Icon::Display, &listing.displays),
			("Windows", Icon::Window, &listing.windows),
		];
		for (heading, icon, picks) in kinds {
			caption(ui, heading, Beside::Nothing);
			match picks {
				Ok(picks) if picks.iter().all(is_own) => {
					ui.label(egui::RichText::new("None found").color(kit::FAINT));
				}
				Ok(picks) => {
					let shown: Vec<Entry> = picks
						.iter()
						.filter(|pick| !is_own(pick))
						.map(|pick| device(icon, pick))
						.collect();
					asked = asked.or(cards(ui, heading, &shown, true));
				}
				Err(err) => {
					// Listing displays on macOS needs the Screen Recording access
					// only opening one asks for, so the main display is offered
					// anyway.
					if heading == "Screens"
						&& let Some(pick) = super::start::display(picks)
					{
						let mut shown = device(icon, &pick);
						shown.name = "Main display".into();
						shown.line = "Asks for Screen Recording access first".into();
						asked = asked.or(cards(ui, heading, &[shown], true));
					}
					asked = asked.or(unavailable(ui, err));
				}
			}
		}
		asked
	}

	/// An image or video file, a web page, a text, a color, and what the show
	/// already has: the program, its other scenes, and with their mods on,
	/// the replay and a role.
	fn media_tab(&mut self, ui: &mut Ui) -> Option<Asked> {
		let mut asked = None;
		let file = ui.add_sized(
			vec2(ui.available_width(), 38.0),
			kit::button(Icon::Image, "Image or video file…"),
		);
		if file.clicked() && self.quick.dialog.is_none() {
			asked = Some(Asked::Media);
		}
		ui.add_space(8.0);
		ui.horizontal(|ui| {
			let url = url::Url::parse(self.adding.url.trim())
				.ok()
				.filter(|url| matches!(url.scheme(), "http" | "https"));
			let button = 120.0;
			let field = ui.add(
				egui::TextEdit::singleline(&mut self.adding.url)
					.hint_text("Web page URL: https://")
					.desired_width(ui.available_width() - button - 8.0),
			);
			let field = kit::named(field, "Web page URL");
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			let add = ui.add_enabled(
				url.is_some(),
				kit::button(Icon::Web, "Add page").min_size(vec2(button, 0.0)),
			);
			if let Some(url) = url.filter(|_| add.clicked() || entered) {
				asked = Some(Asked::Page(url));
			}
		});

		let canvas = {
			let state = self.engine.state.lock().unwrap();
			let collection = state.collection();
			let scene = self.edited(&state);
			collection.canvas(scene).unwrap_or(collection.canvas)
		};
		let color = self.adding.color;
		caption(ui, "Text and color", Beside::Color(&mut self.adding.color));
		let made = [
			Entry {
				icon: Icon::Text,
				name: "Text".into(),
				line: "A lower third to edit".into(),
				card: Ok(Card::New(Pick {
					name: "Text".into(),
					source: Source::Overlay {
						html: super::start::TEXT.into(),
						width: canvas.width,
						height: canvas.height,
						restart: true,
					},
				})),
				swatch: None,
			},
			Entry {
				icon: Icon::Color,
				name: "Color".into(),
				line: "Behind everything".into(),
				card: Ok(Card::New(Pick {
					name: "Color".into(),
					source: Source::Color {
						color: moq_scene::Rgba(color.to_srgba_unmultiplied()),
					},
				})),
				swatch: Some(color),
			},
		];
		asked = asked.or(cards(ui, "made", &made, true));

		let shown = self.the_show();
		caption(ui, "From the show", Beside::Nothing);
		asked.or(cards(ui, "show", &shown, true))
	}

	/// What the show has to put inside the edited scene: the program, every
	/// other scene, and with their mods on, the replay and an unfilled role.
	fn the_show(&self) -> Vec<Entry> {
		let state = self.engine.state.lock().unwrap();
		let edited = self.edited(&state);
		let new = |icon, name: &str, line: &str, source| Entry {
			icon,
			name: name.into(),
			line: line.into(),
			card: Ok(Card::New(Pick {
				name: name.into(),
				source,
			})),
			swatch: None,
		};
		let mut shown = vec![new(Icon::Live, "Program", "Whatever is on air", Source::Program)];
		for scene in &state.collection().scenes {
			if Some(scene.name.as_str()) != edited {
				let source = Source::Scene {
					scene: scene.name.clone(),
				};
				shown.push(new(Icon::Scene, &scene.name, "Scene", source));
			}
		}
		let mods = &self.controls.mods;
		if mods.on(Mod::Review) {
			shown.push(new(Icon::Replay, "Replay", "The replay as it plays", Source::Replay));
		}
		if mods.on(Mod::Layout) {
			shown.push(new(
				Icon::Layers,
				"Role",
				"A slot another source fills",
				Source::Role { source: None },
			));
		}
		shown
	}

	/// Each microphone with its level, so the one that moves is the one
	/// heard, and the system's sound where it can be captured.
	fn audio_tab(&mut self, ui: &mut Ui) -> Option<Asked> {
		let Some(listing) = &self.devices.listing else {
			looking(ui);
			return None;
		};
		let picks = match &listing.audio {
			Ok(picks) if picks.is_empty() => {
				ui.label(egui::RichText::new("No microphones found").color(kit::FAINT));
				return None;
			}
			Ok(picks) => picks.clone(),
			Err(err) => return unavailable(ui, err),
		};
		let ids: Vec<String> = picks
			.iter()
			.filter_map(|pick| match &pick.source {
				Source::Microphone { id: Some(id) } => Some(id.clone()),
				_ => None,
			})
			.collect();
		self.adding.meters.follow(&self.runtime, &ids);
		ui.ctx().request_repaint_after(Duration::from_millis(33));
		caption(ui, "Sound", Beside::Note("Talk, and watch which one moves"));
		let mut asked = None;
		for pick in picks {
			let level = match &pick.source {
				Source::Microphone { id: Some(id) } => self.adding.meters.level(id),
				_ => None,
			};
			if sound_row(ui, &pick, level).clicked() {
				asked = Some(Asked::Card(Card::New(pick)));
			}
		}
		asked
	}

	/// Invite guests and see who joined, then each guest's picture nothing
	/// shows yet.
	fn guests_tab(&mut self, ui: &mut Ui) -> Option<Asked> {
		self.guests(ui);
		let unused = self.unused(true);
		if unused.is_empty() {
			return None;
		}
		caption(ui, "Waiting for a scene", Beside::Nothing);
		cards(ui, "guests", &unused, true)
	}

	/// While a card is dragged over `picture`, which shows `canvas`, outline
	/// where it would land; the card and that place, in canvas pixels, once
	/// it is let go there.
	pub(super) fn card_on_picture(
		&self,
		ui: &Ui,
		state: &State,
		picture: &egui::Response,
		canvas: Canvas,
	) -> Option<(Card, Option<[f32; 2]>)> {
		let card = egui::DragAndDrop::payload::<Card>(ui.ctx())?;
		let pointer = ui.ctx().pointer_latest_pos()?;
		if !picture.contains_pointer() {
			return None;
		}
		let scale = picture.rect.size() / vec2(canvas.width.max(1) as f32, canvas.height.max(1) as f32);
		let at = (pointer - picture.rect.min) / scale;
		// Only the landscape picture places by where it is let go; the portrait
		// one lands it where it would land clicked.
		let at = (self.format == Format::Landscape).then_some([at.x, at.y]);
		let layer = self.scene(state).ok()?;
		let collection = state.collection();
		let landing = match &*card {
			Card::New(pick) => Some(&pick.source),
			Card::Unused(name) => collection.sources.get(name),
		};
		if let Some(source) = landing.filter(|source| source.has_video()) {
			let (_, rect) = place(collection, &layer, canvas, source, at);
			let outline = Rect::from_min_size(
				picture.rect.min + vec2(rect.x, rect.y) * scale,
				vec2(rect.width, rect.height) * scale,
			);
			ui.painter()
				.rect_stroke(outline, 4, Stroke::new(2.0, kit::ACCENT), egui::StrokeKind::Inside);
		}
		let card = picture.dnd_release_payload::<Card>()?;
		Some(((*card).clone(), at))
	}
}

impl Panels {
	/// Invite guests, show their links, and see who joined.
	fn guests(&mut self, ui: &mut Ui) {
		let engine = self.engine.clone();
		let Some(room) = engine.room() else {
			match self.controls.room_failure() {
				Some(failure) => ui.colored_label(kit::ERR, format!("Guests cannot join: {failure}")),
				None => ui.label("Guests join a room on your relay, which needs a room and its key."),
			};
			if ui.add(kit::button(Icon::Settings, "Set up the room")).clicked() {
				self.setting = Some(Setting::Guests);
			}
			return;
		};
		// A link into a room the studio has left joins nothing.
		self.shown.take_if(|shown| !shown.joins(&room));

		ui.horizontal(|ui| {
			ui.label("Name");
			let field = ui.text_edit_singleline(&mut self.guest);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			if ui.add(kit::button(Icon::Add, "Invite")).clicked() || entered {
				let name = self.guest.trim().to_string();
				match room.invite(&name) {
					Ok(invite) => {
						self.show(ui.ctx(), &room, name, invite);
						self.guest.clear();
					}
					Err(err) => self.fail(Spot::Add, Err(err)),
				}
			}
		});

		let now = kio::clock::wall();
		if let Some(shown) = &self.shown {
			let title = format!("Invite {}", shown.name);
			let close = kit::Sheet::new(egui::Id::new("join link"), &title).show(ui.ctx(), |ui| {
				ui.label(format!("Their join link {}.", expiry(shown.invite.expires, now)));
				ui.add_space(8.0);
				// The link is the guest's credential, so it shows only as a code to scan.
				match &shown.qr {
					Ok(qr) => {
						ui.vertical_centered(|ui| ui.image((qr.id(), egui::vec2(240.0, 240.0))));
					}
					Err(err) => {
						ui.colored_label(ui.visuals().error_fg_color, format!("{err}; copy the link instead."));
					}
				}
				ui.add_space(8.0);
				if ui.add(kit::button(Icon::Copy, "Copy link")).clicked() {
					ui.ctx().copy_text(shown.invite.link.to_string());
				}
			});
			if close {
				self.shown = None;
			}
		}

		ui.separator();
		if let Some(failure) = room.renewal_failure() {
			ui.colored_label(
				ui.visuals().error_fg_color,
				format!("Guests' tokens are not renewing, so each leaves when their link expires: {failure}"),
			);
		}
		let participants = room.participants();
		if participants.is_empty() {
			ui.weak("Nobody invited yet.");
			return;
		}
		let mut show = None;
		let mut invite = None;
		let mut remove = None;
		egui::ScrollArea::vertical().show(ui, |ui| {
			egui::Grid::new("guests").striped(true).show(ui, |ui| {
				for heading in ["Guest", "Camera", "Screen", "Link", ""] {
					ui.strong(heading);
				}
				ui.end_row();
				for (name, participant) in &participants {
					ui.label(name);
					for online in [participant.camera, participant.screen] {
						if online {
							ui.colored_label(kit::OK, "live");
						} else {
							ui.weak("offline");
						}
					}
					// A renewed guest no longer depends on their link's expiry.
					match &participant.invite {
						_ if participant.renewed() => ui.label("renewing"),
						_ if participant.removed => ui.weak("not renewed"),
						Some(link) => ui.label(expiry(link.expires, now)),
						None => ui.weak("invited before this session"),
					};
					ui.horizontal(|ui| {
						// Only a link that still works is worth sharing again.
						if let Some(link) = &participant.invite
							&& link.expires > now
							&& ui.small_button("Show link").clicked()
						{
							show = Some((name.clone(), link.clone()));
						}
						if ui.small_button("New link").clicked() {
							invite = Some(name.clone());
						}
						if ui
							.small_button("Remove from scenes")
							.on_hover_text(
								"Take their camera and screen, and any mask reading them, out of every scene and mute them. Their token stops renewing, so they leave within the link lifetime in Settings, Guests. Undo puts them back in the scenes; only New link renews them again.",
							)
							.clicked()
						{
							remove = Some(name.clone());
						}
					});
					ui.end_row();
				}
			});
		});

		if let Some((name, link)) = show {
			self.show(ui.ctx(), &room, name, link);
		}
		if let Some(name) = invite {
			match room.invite(&name) {
				Ok(link) => self.show(ui.ctx(), &room, name, link),
				Err(err) => self.fail(Spot::Add, Err(err)),
			}
		}
		if let Some(name) = remove {
			if let Err(err) = room.stop_renewing(&name) {
				self.report(Err(err));
				return;
			}
			let mut state = engine.state.lock().unwrap();
			let mut draft = state.collection().draft(format!("Remove `{name}` from scenes"));
			let result = match room.take_off(&mut draft, &name) {
				Ok(false) => Ok(()),
				Ok(true) => {
					// Removing items moves the ones after them.
					self.deselect_item();
					state.commit(draft, None)
				}
				Err(err) => Err(err.into()),
			};
			drop(state);
			self.report(result.map(|()| format!("removed {name} from every scene and muted them")));
		}
	}

	/// Put `invite`'s link for `name` on screen as a QR code.
	pub(super) fn show(
		&mut self,
		ctx: &egui::Context,
		room: &Arc<crate::guests::Room>,
		name: String,
		invite: crate::guests::Invite,
	) {
		let qr = qr(ctx, invite.link.as_str());
		let room = Arc::downgrade(room);
		self.shown = Some(Shown { name, invite, qr, room });
	}
}

/// Where a new layer showing `source` goes among `layer`'s on `canvas`: its
/// index, back first, and its place. Centered on `at` in canvas pixels, it
/// lands there at most half the canvas; otherwise the first picture fills
/// the canvas, behind any smaller layers, one over a layer already covering
/// it lands as a picture-in-picture in the first corner clear of the
/// others, a color goes behind everything, and a page or overlay lands at
/// its own size.
pub(super) fn place(
	collection: &Collection,
	layer: &Layer,
	canvas: Canvas,
	source: &Source,
	at: Option<[f32; 2]>,
) -> (usize, moq_scene::Rect) {
	let items = collection.items(layer).unwrap_or_default();
	let front = items.len();
	let shown = match source {
		Source::Clone { source } => collection.sources.get(source),
		source => Some(source),
	};
	let own = match shown {
		Some(Source::Browser { width, height, .. } | Source::Overlay { width, height, .. }) => {
			Some([*width as f32, *height as f32])
		}
		_ => None,
	};
	let (width, height) = (canvas.width as f32, canvas.height as f32);
	if let Some(at) = at {
		return (front, super::drop::land(canvas, own.unwrap_or([width, height]), at));
	}
	if let Some([w, h]) = own {
		let rect = moq_scene::Rect {
			x: 0.0,
			y: 0.0,
			width: w,
			height: h,
		};
		return (front, rect);
	}
	let fill = moq_scene::Rect::fill(&canvas);
	if matches!(shown, Some(Source::Color { .. })) {
		return (0, fill);
	}
	// An overlay is drawn see-through, so it hides nothing behind it.
	let opaque = |item: &&Item| {
		item.visible
			&& !matches!(
				collection.sources.get(collection.capture(&item.source)),
				Some(Source::Overlay { .. })
			)
	};
	let covers = |rect: &moq_scene::Rect| {
		let x = (rect.x + rect.width).min(width) - rect.x.max(0.0);
		let y = (rect.y + rect.height).min(height) - rect.y.max(0.0);
		x.max(0.0) * y.max(0.0) >= COVERS * width * height
	};
	if !items.iter().filter(opaque).any(|item| covers(&item.rect)) {
		// Behind what is there already, which it would hide.
		return (0, fill);
	}
	let w = (width * PIP).round();
	let h = (w * height / width).round();
	let margin = (width * MARGIN).round();
	let (left, top) = (margin, margin);
	let (right, bottom) = (width - margin - w, height - margin - h);
	let corners = [(right, bottom), (left, bottom), (right, top), (left, top)];
	let others: Vec<&moq_scene::Rect> = items
		.iter()
		.filter(|item| item.visible && !covers(&item.rect))
		.map(|item| &item.rect)
		.collect();
	let overlaps = |a: &moq_scene::Rect, b: &moq_scene::Rect| {
		a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
	};
	let pip = |(x, y)| moq_scene::Rect {
		x,
		y,
		width: w,
		height: h,
	};
	let rect = corners
		.into_iter()
		.map(pip)
		.find(|rect| others.iter().all(|other| !overlaps(rect, other)))
		.unwrap_or_else(|| pip(corners[0]));
	(front, rect)
}

/// A listed device as its card shows it.
fn device(icon: Icon, pick: &Pick) -> Entry {
	let line = match &pick.source {
		Source::Camera { .. } => "Camera",
		Source::Display { .. } => "Display",
		Source::Window { .. } => "Window",
		Source::PickedDisplay => "The system asks which display",
		Source::PickedWindow => "The system asks which window",
		_ => "",
	};
	let icon = match &pick.source {
		Source::PickedWindow => Icon::Window,
		_ => icon,
	};
	Entry {
		icon,
		name: pick.name.clone(),
		line: line.into(),
		card: Ok(Card::New(pick.clone())),
		swatch: None,
	}
}

/// A section's caption, with a note beside it or, given `color`, the color
/// to edit.
fn caption(ui: &mut Ui, text: &str, beside: Beside<'_>) {
	ui.add_space(12.0);
	ui.horizontal(|ui| {
		ui.label(kit::caps(text, kit::display(11.0)).color(kit::DIM));
		match beside {
			Beside::Nothing => {}
			Beside::Note(note) => {
				ui.label(egui::RichText::new(note).font(kit::ui(11.5)).color(kit::FAINT));
			}
			Beside::Color(color) => {
				let swatch = egui::color_picker::color_edit_button_srgba(ui, color, egui::color_picker::Alpha::Opaque);
				kit::named(swatch, "Card color");
			}
		}
	});
	ui.add_space(6.0);
}

/// What sits beside a section's caption.
enum Beside<'a> {
	Nothing,
	Note(&'a str),
	Color(&'a mut Color32),
}

/// While the listing is under way.
fn looking(ui: &mut Ui) {
	ui.add_space(12.0);
	ui.horizontal(|ui| {
		ui.spinner();
		ui.label(egui::RichText::new("Looking for cameras, screens, and microphones…").color(kit::DIM));
	});
}

/// Why a kind of device cannot be listed, and the way to list it again once
/// that is fixed.
fn unavailable(ui: &mut Ui, err: &str) -> Option<Asked> {
	let mut asked = None;
	egui::Frame::new()
		.fill(kit::P2)
		.stroke(Stroke::new(1.0, kit::LINE))
		.corner_radius(kit::RADIUS)
		.inner_margin(egui::Margin::same(10))
		.show(ui, |ui| {
			ui.set_width(ui.available_width());
			ui.horizontal(|ui| {
				let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
				Icon::Warning.paint(ui, icon, 15.0, kit::CUE);
				let text = ui.available_width() - 110.0;
				ui.add_sized(
					vec2(text.max(80.0), 0.0),
					egui::Label::new(egui::RichText::new(err).color(kit::DIM)).wrap(),
				);
				// A missing build feature is not fixed by asking again.
				if !err.contains("--features") && ui.add(kit::button(Icon::Reload, "Try again")).clicked() {
					asked = Some(Asked::Retry);
				}
			});
		});
	asked
}

/// `shown` as rows of cards, under `id`, draggable onto the picture if
/// `drag`; what a click asks for.
fn cards(ui: &mut Ui, id: &str, shown: &[Entry], drag: bool) -> Option<Asked> {
	let width = ui.available_width();
	let columns = ((width + GAP) / (CARD_MIN + GAP)).floor().clamp(1.0, 3.0) as usize;
	let card_width = (width - GAP * (columns - 1) as f32) / columns as f32;
	let mut asked = None;
	for row in shown.chunks(columns) {
		let (rect, _) = ui.allocate_exact_size(vec2(width, CARD), Sense::hover());
		for (column, shown) in row.iter().enumerate() {
			let at = Rect::from_min_size(
				pos2(rect.left() + column as f32 * (card_width + GAP), rect.top()),
				vec2(card_width, CARD),
			);
			let response = card(ui, at, (id, &shown.name), shown, drag);
			match &shown.card {
				Ok(card) => {
					if drag && response.drag_started() {
						egui::DragAndDrop::set_payload(ui.ctx(), card.clone());
					}
					if response.clicked() {
						asked = Some(Asked::Card(card.clone()));
					}
					if let Card::Unused(name) = card {
						response.context_menu(|ui| {
							if ui.add(kit::button(Icon::Remove, "Remove for good")).clicked() {
								asked = Some(Asked::Remove(name.clone()));
								ui.close();
							}
						});
					}
				}
				Err(why) => {
					response.on_hover_text(why);
				}
			}
		}
		ui.add_space(GAP);
	}
	asked
}

/// Draw `shown` in `rect`: an icon in a well, its name, and the line under
/// it, lit while hovered.
fn card(ui: &mut Ui, rect: Rect, id: (&str, &str), shown: &Entry, drag: bool) -> egui::Response {
	let enabled = shown.card.is_ok();
	let sense = match (enabled, drag) {
		(false, _) => Sense::hover(),
		(true, false) => Sense::click(),
		(true, true) => Sense::click_and_drag(),
	};
	let response = ui.interact(rect, ui.id().with(("add card", id)), sense);
	let label = format!("Add {}", shown.name);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
	let lit = enabled && response.hovered();
	let painter = ui.painter_at(rect);
	let stroke = if lit { kit::ACCENT } else { kit::LINE };
	painter.rect(rect, 10, kit::P2, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
	let well = Rect::from_min_max(rect.min + vec2(1.0, 1.0), pos2(rect.right() - 1.0, rect.top() + 56.0));
	let fill = shown.swatch.unwrap_or(kit::P3);
	painter.rect_filled(
		well,
		egui::CornerRadius {
			nw: 9,
			ne: 9,
			sw: 0,
			se: 0,
		},
		fill,
	);
	let ink = if enabled { kit::TEXT } else { kit::FAINT };
	if shown.swatch.is_none() {
		shown
			.icon
			.paint(ui, Rect::from_center_size(well.center(), Vec2::splat(22.0)), 22.0, ink);
	}
	let text = painter.with_clip_rect(rect.shrink2(vec2(12.0, 0.0)));
	text.text(
		pos2(rect.left() + 12.0, well.bottom() + 9.0),
		Align2::LEFT_TOP,
		&shown.name,
		kit::ui_strong(13.0),
		ink,
	);
	text.text(
		pos2(rect.left() + 12.0, well.bottom() + 28.0),
		Align2::LEFT_TOP,
		&shown.line,
		kit::ui(11.5),
		kit::DIM,
	);
	if lit {
		let pill = Rect::from_min_size(pos2(rect.right() - 58.0, rect.top() + 8.0), vec2(50.0, 20.0));
		painter.rect_filled(pill, 6, kit::ACCENT);
		painter.text(
			pill.center(),
			Align2::CENTER_CENTER,
			"+ Add",
			kit::ui_strong(11.0),
			Color32::WHITE,
		);
	}
	response
}

/// A microphone's row: its name, its level if metered, and a click adds it.
fn sound_row(ui: &mut Ui, pick: &Pick, level: Option<f32>) -> egui::Response {
	let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
	let label = format!("Add {}", pick.name);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
	let lit = response.hovered();
	let painter = ui.painter_at(rect);
	painter.rect(
		rect,
		8,
		if lit { kit::P3 } else { kit::P2 },
		Stroke::new(1.0, if lit { kit::ACCENT } else { kit::LINE }),
		egui::StrokeKind::Inside,
	);
	let icon = match pick.source {
		Source::SystemAudio => Icon::Sound,
		_ => Icon::Mic,
	};
	icon.paint(
		ui,
		Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(16.0)),
		15.0,
		kit::TEXT,
	);
	let meter = 160.0_f32.min(rect.width() * 0.35);
	let name = painter.with_clip_rect(rect.with_max_x(rect.right() - meter - 70.0));
	name.text(
		pos2(rect.left() + 40.0, rect.center().y),
		Align2::LEFT_CENTER,
		&pick.name,
		kit::ui_medium(13.0),
		kit::TEXT,
	);
	let bar = Rect::from_min_size(
		pos2(rect.right() - meter - 64.0, rect.center().y - 4.5),
		vec2(meter, 9.0),
	);
	if let Some(level) = level {
		let at = super::levels::position(level);
		ui.put(bar, kit::Meter::new(at, at).run(kit::Run::Across, meter, 9.0));
	}
	if lit {
		painter.text(
			pos2(rect.right() - 14.0, rect.center().y),
			Align2::RIGHT_CENTER,
			"+ Add",
			kit::ui_strong(11.0),
			kit::ACCENT,
		);
	}
	response
}

/// The card under the pointer while it is dragged out of Add.
fn dragged(ctx: &egui::Context, card: &Card) {
	let Some(pointer) = ctx.pointer_latest_pos() else {
		return;
	};
	let name = match card {
		Card::New(pick) => pick.name.as_str(),
		Card::Unused(name) => name.as_str(),
	};
	egui::Area::new(egui::Id::new("add dragged"))
		.order(egui::Order::Tooltip)
		.fixed_pos(pointer + vec2(14.0, 18.0))
		.interactable(false)
		.show(ctx, |ui| {
			egui::Frame::new()
				.fill(kit::POPOVER)
				.stroke(Stroke::new(1.0, kit::ACCENT))
				.corner_radius(8)
				.inner_margin(egui::Margin::symmetric(10, 6))
				.show(ui, |ui| {
					ui.label(egui::RichText::new(format!("Let go to add {name} here")).color(kit::TEXT));
				});
		});
}

/// Each listed microphone's level while the Audio tab shows it, each opened
/// on a thread of its own and let go once it is no longer shown.
#[derive(Default)]
struct Meters {
	#[cfg(feature = "capture")]
	open: std::collections::HashMap<String, Meter>,
}

#[cfg(feature = "capture")]
struct Meter {
	/// The loudest peak since the last look, as `f32` bits, which order as
	/// the levels do since none is negative.
	peak: Arc<std::sync::atomic::AtomicU32>,
	/// Whether the microphone could not be opened, which leaves no level.
	failed: Arc<std::sync::atomic::AtomicBool>,
	/// The level shown, falling back from each peak.
	shown: f32,
	/// Dropped to let go of the microphone.
	_stop: tokio::sync::oneshot::Sender<()>,
}

impl Meters {
	/// Meter exactly the microphones of `ids`.
	#[cfg(feature = "capture")]
	fn follow(&mut self, runtime: &tokio::runtime::Handle, ids: &[String]) {
		self.open.retain(|id, _| ids.contains(id));
		for id in ids {
			if self.open.contains_key(id) {
				continue;
			}
			match Meter::open(runtime, id) {
				Ok(meter) => {
					self.open.insert(id.clone(), meter);
				}
				Err(err) => tracing::warn!(%err, "cannot meter a microphone"),
			}
		}
	}

	#[cfg(not(feature = "capture"))]
	fn follow(&mut self, _: &tokio::runtime::Handle, _: &[String]) {}

	/// The level of the microphone `id`, from 0 to 1, unless it is not
	/// metered.
	#[cfg(feature = "capture")]
	fn level(&mut self, id: &str) -> Option<f32> {
		use std::sync::atomic::Ordering;
		let meter = self.open.get_mut(id)?;
		if meter.failed.load(Ordering::Relaxed) {
			return None;
		}
		let peak = f32::from_bits(meter.peak.swap(0, Ordering::Relaxed));
		meter.shown = peak.max(meter.shown * 0.85);
		Some(meter.shown)
	}

	#[cfg(not(feature = "capture"))]
	fn level(&mut self, _: &str) -> Option<f32> {
		None
	}
}

#[cfg(feature = "capture")]
impl Meter {
	/// Open the microphone `id` on a thread of its own: the capture backends'
	/// futures are `!Send` on macOS, as the device listing's are.
	fn open(runtime: &tokio::runtime::Handle, id: &str) -> std::io::Result<Self> {
		use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
		let peak = Arc::new(AtomicU32::new(0));
		let failed = Arc::new(AtomicBool::new(false));
		let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
		let source = moq_audio::capture::Source::Microphone(Some(id.to_string()));
		let (heard, failing) = (peak.clone(), failed.clone());
		let handle = runtime.clone();
		std::thread::Builder::new().name("meter".into()).spawn(move || {
			handle.block_on(async move {
				tokio::select! {
					_ = stopped => {}
					result = listen(source, &heard) => {
						if let Err(err) = result {
							tracing::info!(%err, "cannot meter a microphone");
						}
						failing.store(true, Ordering::Relaxed);
					}
				}
			});
		})?;
		Ok(Self {
			peak,
			failed,
			shown: 0.0,
			_stop: stop,
		})
	}
}

/// Capture `source`, keeping the loudest peak in `peak` until it fails.
#[cfg(feature = "capture")]
async fn listen(source: moq_audio::capture::Source, peak: &std::sync::atomic::AtomicU32) -> anyhow::Result<()> {
	let mut config = moq_audio::capture::Config::default();
	config.source = source;
	let capture = moq_audio::capture::Producer::open(config).await?;
	let consumer = capture.consume();
	tokio::select! {
		result = capture.run() => result?,
		result = peaks(consumer, peak) => match result? {},
	}
	anyhow::bail!("the microphone stopped")
}

/// Keep the loudest peak `consumer` reads in `peak`, until it fails.
#[cfg(feature = "capture")]
async fn peaks(
	mut consumer: moq_audio::capture::Consumer,
	peak: &std::sync::atomic::AtomicU32,
) -> Result<std::convert::Infallible, moq_audio::Error> {
	loop {
		let buffer = consumer.read().await?;
		let loudest = buffer
			.data
			.iter()
			.fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));
		peak.fetch_max(loudest.min(1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const SHOW: &str = r##"
version = 1

[canvas]
width = 1920
height = 1080
framerate = 30

[sources.screen]
kind = "display"

[sources.logo]
kind = "color"
color = "#ffffff"

[sources.text]
kind = "overlay"
html = "<p>hi</p>"
width = 1920
height = 1080

[[scenes]]
name = "Main"
"##;

	/// The scene `Main` showing `items`, each a source and its place.
	fn scene(items: &[(&str, [f32; 4])]) -> Collection {
		let mut collection = Collection::from_toml(SHOW).unwrap();
		let mut draft = collection.draft("items");
		for (index, (source, [x, y, width, height])) in items.iter().enumerate() {
			let rect = moq_scene::Rect {
				x: *x,
				y: *y,
				width: *width,
				height: *height,
			};
			draft
				.insert_item(&Layer::scene("Main"), index, Item::new(source.to_string(), rect))
				.unwrap();
		}
		collection = draft.collection().clone();
		collection
	}

	fn placed(collection: &Collection, source: Source, at: Option<[f32; 2]>) -> (usize, [f32; 4]) {
		let (index, rect) = place(collection, &Layer::scene("Main"), collection.canvas, &source, at);
		(index, [rect.x, rect.y, rect.width, rect.height])
	}

	const CAMERA: Source = Source::Camera { id: None };
	const FULL: [f32; 4] = [0.0, 0.0, 1920.0, 1080.0];

	/// The first picture fills the canvas, and one over a picture filling it
	/// lands in the first corner clear of the others, the bottom right first.
	#[test]
	fn a_camera_over_a_screen_lands_in_a_corner() {
		assert_eq!(placed(&scene(&[]), CAMERA, None), (0, FULL));
		let screen = scene(&[("screen", FULL)]);
		assert_eq!(placed(&screen, CAMERA, None), (1, [1296.0, 708.0, 576.0, 324.0]));
		let taken = scene(&[("screen", FULL), ("logo", [1296.0, 708.0, 576.0, 324.0])]);
		assert_eq!(placed(&taken, CAMERA, None), (2, [48.0, 708.0, 576.0, 324.0]));
	}

	/// With nothing covering the canvas, a new picture fills it behind what
	/// is there, which it would otherwise hide; a see-through overlay covers
	/// nothing.
	#[test]
	fn a_full_picture_goes_behind_smaller_ones() {
		let small = scene(&[("logo", [0.0, 0.0, 200.0, 100.0]), ("text", FULL)]);
		assert_eq!(placed(&small, CAMERA, None), (0, FULL));
	}

	/// A color goes behind everything, a page or overlay lands at its own
	/// size, and a drop lands centered where it was let go.
	#[test]
	fn colors_pages_and_drops() {
		let screen = scene(&[("screen", FULL)]);
		let color = Source::Color {
			color: moq_scene::Rgba([0, 0, 0, 255]),
		};
		assert_eq!(placed(&screen, color, None), (0, FULL));
		let page = Source::Browser {
			url: "https://example.com".into(),
			width: 800,
			height: 600,
		};
		assert_eq!(placed(&screen, page, None), (1, [0.0, 0.0, 800.0, 600.0]));
		assert_eq!(
			placed(&screen, CAMERA, Some([1500.0, 300.0])),
			(1, [960.0, 30.0, 960.0, 540.0]),
			"half the canvas, kept on it"
		);
	}
}
