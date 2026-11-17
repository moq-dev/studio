//! A camera, display, or window gone dark, or a camera or broadcast that
//! stopped sending pictures: the amber badge on its layer's row, over it on
//! the picture, and on its scenes' pads, and the popover that says in plain
//! words what happened and offers the fix.

use egui::{Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::*;

/// What fixes a capture gone dark, as its popover offers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mend {
	/// Ask the system picker afresh, forgetting the grant it holds.
	PickAgain,
	/// Open the device, or subscribe to the broadcast, again.
	Reopen,
	/// Capture this in its place, under the same name.
	Replace(Source),
	/// It is meant to go dark: stop watching it.
	Expected,
}

/// What a lost picture's popover says and offers, by its kind.
struct Advice {
	title: &'static str,
	/// What may have happened, and what viewers see.
	cause: String,
	/// What the streamer must do where the studio cannot, and where.
	hint: Option<String>,
	/// Each fix the studio can take itself, the first the likeliest.
	fixes: Vec<(&'static str, Mend)>,
	/// Whether to offer the windows this machine lists, to capture one in
	/// its place: a window chosen by id, which the system cannot ask again.
	windows: bool,
	/// Whether to offer to stop watching it: a capture meant to go dark.
	expected: bool,
}

/// What to say of `source`, called `name`, that `lost` its picture; `None`
/// for a kind never watched for it.
fn advice(name: &str, source: &Source, lost: Lost) -> Option<Advice> {
	match lost {
		Lost::Dark(_) => dark(name, source),
		Lost::Stalled(_) => stalled(name, source),
	}
}

/// What to say of `source`, called `name`, that stopped sending pictures.
fn stalled(name: &str, source: &Source) -> Option<Advice> {
	let again = ("Subscribe again", Mend::Reopen);
	let advice = match source {
		Source::Camera { .. } => Advice {
			title: "This camera stopped sending pictures.",
			cause: format!(
				"`{name}` may have been unplugged, or another app may have taken it. Viewers see its last picture, frozen."
			),
			hint: Some("If it stays frozen, check its cable, and quit any other app using the camera.".into()),
			fixes: vec![("Open it again", Mend::Reopen)],
			windows: false,
			expected: false,
		},
		Source::Broadcast { path } => Advice {
			title: "This broadcast stopped sending pictures.",
			cause: format!(
				"`{name}`'s sender may have stopped, or lost its connection to the relay. Viewers see its last picture, frozen."
			),
			hint: Some(format!(
				"If it stays frozen, whoever sends `{path}` should check that it is still live and its connection keeps up."
			)),
			fixes: vec![again],
			windows: false,
			expected: false,
		},
		Source::Layer { broadcast, .. } => Advice {
			title: "This layer stopped sending pictures.",
			cause: format!(
				"`{name}` comes from the broadcast `{broadcast}`, whose sender may have stopped or lost its connection. Viewers see its last picture, frozen."
			),
			hint: None,
			fixes: vec![again],
			windows: false,
			expected: false,
		},
		_ => return None,
	};
	Some(advice)
}

/// What to say of `source`, called `name`, gone dark.
fn dark(name: &str, source: &Source) -> Option<Advice> {
	let whole = ("Capture the whole screen instead", Mend::Replace(whole_screen()));
	let advice = match source {
		Source::PickedWindow | Source::Window { .. } => Advice {
			title: "This window went black.",
			cause: format!(
				"`{name}` may have closed, been minimized, or changed how it draws. Viewers see black where it was."
			),
			hint: Some(match () {
				_ if cfg!(target_os = "macos") => SCREEN_PERMISSION.into(),
				_ => "A game in exclusive full screen refuses capture: switch it to borderless or windowed in its display settings.".into(),
			}),
			fixes: match source {
				Source::PickedWindow => vec![("Pick it again", Mend::PickAgain), whole],
				_ => vec![whole],
			},
			windows: !matches!(source, Source::PickedWindow),
			expected: true,
		},
		Source::PickedDisplay | Source::Display { .. } => Advice {
			title: "This screen went black.",
			cause: format!(
				"`{name}` may be locked or asleep, or showing something that refuses capture, such as protected video. Viewers see black where it was."
			),
			hint: cfg!(target_os = "macos").then(|| SCREEN_PERMISSION.into()),
			fixes: match source {
				Source::PickedDisplay => vec![("Pick it again", Mend::PickAgain)],
				_ => vec![("Open it again", Mend::Reopen)],
			},
			windows: false,
			expected: true,
		},
		Source::Camera { .. } => Advice {
			title: "This camera went black.",
			cause: format!(
				"`{name}` may have its lens covered or its privacy shutter closed, or another app may have taken it. Viewers see black where it was."
			),
			hint: match () {
				_ if cfg!(target_os = "macos") => Some(
					"If it stays black, macOS may have taken away the camera: allow kixel.live Studio in System Settings > Privacy & Security > Camera.",
				),
				_ if cfg!(windows) => Some(
					"If it stays black, Windows may have taken away the camera: allow desktop apps in Settings > Privacy & security > Camera.",
				),
				_ => None,
			}
			.map(str::to_string),
			fixes: vec![("Open it again", Mend::Reopen)],
			windows: false,
			expected: true,
		},
		_ => return None,
	};
	Some(advice)
}

/// What macOS asks of a display or window capture that it stopped allowing.
const SCREEN_PERMISSION: &str = "If it stays black, macOS may have taken away screen recording: allow kixel.live Studio in System Settings > Privacy & Security > Screen & System Audio Recording.";

/// What captures the whole screen here: the system picker on a Wayland
/// desktop, which captures no screen without it, else the main display.
fn whole_screen() -> Source {
	if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
		return Source::PickedDisplay;
	}
	Source::Display { id: None }
}

/// Say whether the capture called `capture` is `expected` to go dark, so it
/// is never flagged, or is watched again.
pub(super) fn expect(state: &mut State, capture: &str, expected: bool) -> anyhow::Result<()> {
	let video = state.collection().video.get(capture).cloned().unwrap_or_default();
	let video = Video {
		expect_dark: expected,
		..video
	};
	let label = match expected {
		true => format!("Expect `{capture}` to go dark"),
		false => format!("Warn when `{capture}` goes dark"),
	};
	edit(state, label, None, |draft| draft.set_video(capture, Some(video)))
}

/// "No picture for 12 s" for a capture gone dark, "No new picture for 3 s"
/// for one stalled: what its row says under its name.
pub(super) fn words(lost: Lost) -> String {
	let what = match lost {
		Lost::Dark(_) => "No picture",
		Lost::Stalled(_) => "No new picture",
	};
	format!("{what} for {:.0} s", lost.since().as_secs_f64())
}

/// What the health dot says of `dark` layers gone dark and `stalled` ones
/// stalled, such as "1 layer dark"; `None` for none.
pub(super) fn counted(dark: usize, stalled: usize) -> Option<String> {
	let layers = |count: usize| match count {
		1 => "1 layer".to_string(),
		count => format!("{count} layers"),
	};
	match (dark, stalled) {
		(0, 0) => None,
		(dark, 0) => Some(format!("{} dark", layers(dark))),
		(0, stalled) => Some(format!("{} stalled", layers(stalled))),
		(dark, stalled) => Some(format!("{} dark or stalled", layers(dark + stalled))),
	}
}

/// Whether the scene called `scene` draws a layer that `lost` its picture,
/// as `lost` says of a source: through the scenes it nests, and the masks
/// its layers read, and not one it hides.
pub(super) fn draws_lost(collection: &Collection, scene: &str, lost: impl Fn(&str) -> bool) -> bool {
	collection.drawn([Layer::scene(scene)]).iter().any(|item| {
		let masks = collection.filters(item).filter_map(|filter| match filter {
			Filter::Mask { source, .. } => Some(source.as_str()),
			_ => None,
		});
		std::iter::once(item.source.as_str()).chain(masks).any(&lost)
	})
}

/// The amber badge in `rect`: a warning sign on amber.
pub(super) fn badge(ui: &Ui, rect: Rect) {
	let painter = ui.painter();
	painter.circle_filled(rect.center(), rect.width() / 2.0, kit::CUE);
	Icon::Warning.paint(ui, rect, rect.width() * 0.6, kit::ON_CUE);
}

/// The badge for the source `source`, called `name`, that `lost` its
/// picture, in `rect`: a click opens its popover. Whether it was clicked
/// open, and the fix asked for, if one was; with `windows`, the windows to
/// offer in its place, if listed.
pub(super) fn badge_button(
	ui: &mut Ui,
	rect: Rect,
	id: egui::Id,
	name: &str,
	source: &Source,
	lost: Lost,
	windows: Option<&Result<Vec<Pick>, String>>,
) -> (bool, Option<Mend>) {
	let response = ui.interact(rect, id, Sense::click());
	let label = format!("What happened to {name}");
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
	badge(ui, rect);
	let opened = response.clicked();
	let Some(advice) = advice(name, source, lost) else {
		return (opened, None);
	};
	let mut mend = None;
	kit::popover(&response).width(300.0).show(|ui| {
		mend = popover(ui, &advice, windows);
	});
	(opened, mend)
}

/// The popover's body: what happened, the fixes, and where the streamer acts
/// when the studio cannot.
fn popover(ui: &mut Ui, advice: &Advice, windows: Option<&Result<Vec<Pick>, String>>) -> Option<Mend> {
	let mut mend = None;
	ui.add_space(6.0);
	ui.horizontal_top(|ui| {
		ui.add(Icon::Warning.image(18.0, kit::CUE));
		ui.vertical(|ui| {
			ui.add(
				egui::Label::new(
					egui::RichText::new(advice.title)
						.font(kit::ui_strong(14.0))
						.color(kit::TEXT),
				)
				.wrap(),
			);
			ui.add(egui::Label::new(egui::RichText::new(&advice.cause).color(kit::DIM)).wrap());
		});
	});
	ui.add_space(8.0);
	let wide = vec2(ui.available_width(), 30.0);
	if advice.windows {
		ui.menu_button("Pick another window", |ui| match windows {
			Some(Ok(windows)) if !windows.is_empty() => {
				for window in windows {
					if ui.button(&window.name).clicked() {
						mend = Some(Mend::Replace(window.source.clone()));
						ui.close();
					}
				}
			}
			Some(Ok(_)) => {
				ui.label("No windows to capture");
			}
			Some(Err(err)) => {
				ui.colored_label(kit::ERR, err);
			}
			None => {
				ui.spinner();
			}
		});
	}
	for (index, (label, fix)) in advice.fixes.iter().enumerate() {
		// The likeliest fix stands out, as a light button.
		let button = match index {
			0 if !advice.windows => egui::Button::new(egui::RichText::new(*label).color(kit::BG)).fill(kit::TEXT),
			_ => egui::Button::new(*label),
		};
		if ui.add(button.min_size(wide)).clicked() {
			mend = Some(fix.clone());
		}
	}
	if let Some(hint) = &advice.hint {
		ui.add_space(4.0);
		ui.add(egui::Label::new(egui::RichText::new(hint).font(kit::ui(11.0)).color(kit::DIM)).wrap());
	}
	if advice.expected {
		ui.add_space(4.0);
		let expected = ui
			.add(egui::Button::new(egui::RichText::new("It is meant to go dark").color(kit::DIM)).frame(false))
			.on_hover_text("Stop watching it for going dark; its inspector watches it again");
		if expected.clicked() {
			mend = Some(Mend::Expected);
		}
	}
	mend
}

/// Mark the layer called `name` at `at`, which `lost` its picture, on the
/// picture: an amber dashed outline, and its name on an amber pill in its
/// middle.
pub(super) fn on_picture(ui: &Ui, at: Rect, name: &str, lost: Lost) {
	// Nothing spills onto what is beside it, such as a narrow multiview tile's
	// neighbors.
	let painter = ui.painter().with_clip_rect(at.intersect(ui.clip_rect()));
	super::dashed(&painter, at.shrink(1.5), Stroke::new(2.0, kit::CUE));

	let what = match lost {
		Lost::Dark(_) => "no picture",
		Lost::Stalled(_) => "no new picture",
	};
	// Its name goes first where the pill would not fit, then the pill.
	let pill = |text: String| {
		let galley = painter.layout_no_wrap(text.to_uppercase(), kit::display(13.0), kit::ON_CUE);
		let size = galley.size() + vec2(44.0, 14.0);
		(size.x <= at.width() - 8.0 && size.y <= at.height() - 8.0).then_some((galley, size))
	};
	let Some((galley, size)) = pill(format!("{name} · {what}")).or_else(|| pill(what.to_string())) else {
		return;
	};
	let pill = Rect::from_center_size(at.center(), size);
	painter.rect_filled(pill, size.y / 2.0, kit::CUE);
	let icon = Rect::from_center_size(pos2(pill.left() + 18.0, pill.center().y), Vec2::splat(18.0));
	painter.circle_filled(icon.center(), 9.0, kit::ON_CUE);
	Icon::Warning.paint(ui, icon, 11.0, kit::CUE);
	painter.galley(
		pos2(icon.right() + 6.0, pill.center().y - galley.size().y / 2.0),
		galley,
		kit::ON_CUE,
	);
}

#[cfg(test)]
mod tests {
	use super::*;

	const DARK: Lost = Lost::Dark(Duration::from_secs(12));
	const STALLED: Lost = Lost::Stalled(Duration::from_secs(3));

	/// Every watched kind says what happened and offers a fix, and a window
	/// or display the picker granted offers to pick it again first.
	#[test]
	fn every_watched_kind_has_advice() {
		let kinds = [
			Source::Camera { id: None },
			Source::Display { id: None },
			Source::Window { id: "1".into() },
			Source::PickedDisplay,
			Source::PickedWindow,
		];
		for source in kinds {
			let advice = advice("game", &source, DARK).expect("advice");
			assert!(advice.cause.contains("`game`"));
			assert!(!advice.fixes.is_empty() || advice.windows, "{source:?}");
			assert!(advice.expected, "{source:?}");
		}
		let picked = advice("game", &Source::PickedWindow, DARK).unwrap();
		assert_eq!(picked.fixes[0].1, Mend::PickAgain);
		let logo = Source::Image {
			path: "logo.png".into(),
		};
		assert!(advice("logo", &logo, DARK).is_none());
		assert!(advice("logo", &logo, STALLED).is_none());
	}

	/// Every kind whose picture can stall says so and offers to open or
	/// subscribe again, and never to stop watching it.
	#[test]
	fn every_stalling_kind_has_advice() {
		let kinds = [
			Source::Camera { id: None },
			Source::Broadcast { path: "guest".into() },
			Source::Layer {
				broadcast: "remote".into(),
				source: "cam".into(),
			},
		];
		for source in kinds {
			let advice = advice("cam", &source, STALLED).expect("advice");
			assert!(advice.cause.contains("`cam`"), "{source:?}");
			assert!(advice.cause.contains("frozen"), "{source:?}");
			assert_eq!(advice.fixes[0].1, Mend::Reopen, "{source:?}");
			assert!(!advice.expected, "{source:?}");
		}
		let broadcast = advice("cam", &Source::Broadcast { path: "guest".into() }, STALLED).unwrap();
		assert!(broadcast.hint.unwrap().contains("`guest`"));
		// A window that stands still is not stalled.
		assert!(advice("game", &Source::PickedWindow, STALLED).is_none());
	}

	#[test]
	fn rows_say_how_long() {
		assert_eq!(words(DARK), "No picture for 12 s");
		assert_eq!(words(STALLED), "No new picture for 3 s");
	}

	/// A scene pad is marked for a lost picture it draws, nested in a scene
	/// it shows, but not for one it hides.
	#[test]
	fn a_pad_marks_what_it_draws() {
		let fill = moq_scene::Rect::fill(&Canvas::default());
		let mut collection = Collection::new();
		collection.sources.insert("cam".into(), Source::Camera { id: None });
		collection
			.sources
			.insert("inner".into(), Source::Scene { scene: "Inner".into() });
		let mut inner = Scene::new("Inner");
		inner.items.push(Item::new("cam", fill));
		collection.scenes.push(inner);
		let mut hidden = Scene::new("Hidden");
		hidden.items.push(Item {
			visible: false,
			..Item::new("cam", fill)
		});
		collection.scenes.push(hidden);
		collection.scenes[0].items.push(Item::new("inner", fill));
		// A color cut to the camera's shape reads the camera's picture.
		collection.sources.insert(
			"red".into(),
			Source::Color {
				color: moq_scene::Rgba([255, 0, 0, 255]),
			},
		);
		let mut masked = Scene::new("Masked");
		masked.items.push(Item {
			filters: vec![Filter::Mask {
				source: "cam".into(),
				mode: MaskMode::default(),
			}],
			..Item::new("red", fill)
		});
		collection.scenes.push(masked);
		let outer = collection.scenes[0].name.clone();
		collection.validate().unwrap();
		let lost = |name: &str| name == "cam";
		assert!(draws_lost(&collection, "Inner", lost));
		assert!(draws_lost(&collection, &outer, lost), "through the scene it nests");
		assert!(!draws_lost(&collection, "Hidden", lost), "not what it hides");
		assert!(draws_lost(&collection, "Masked", lost), "through the mask it reads");
	}

	/// The health dot names what the layers lost, and counts them.
	#[test]
	fn the_dot_counts_layers_without_a_picture() {
		assert_eq!(counted(0, 0), None);
		assert_eq!(counted(1, 0).as_deref(), Some("1 layer dark"));
		assert_eq!(counted(2, 0).as_deref(), Some("2 layers dark"));
		assert_eq!(counted(0, 1).as_deref(), Some("1 layer stalled"));
		assert_eq!(counted(1, 1).as_deref(), Some("2 layers dark or stalled"));
	}
}
