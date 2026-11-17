//! A camera, display, or window gone dark: the amber badge on its layer's
//! row, over it on the picture, and on its scenes' pads, and the popover
//! that says in plain words what happened and offers the fix.

use egui::{Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::*;

/// What fixes a capture gone dark, as its popover offers.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mend {
	/// Ask the system picker afresh, forgetting the grant it holds.
	PickAgain,
	/// Open the device again.
	Reopen,
	/// Capture this in its place, under the same name.
	Replace(Source),
	/// It is meant to go dark: stop watching it.
	Expected,
}

/// What a dark capture's popover says and offers, by its kind.
struct Advice {
	title: &'static str,
	/// What may have happened, and what viewers see.
	cause: String,
	/// What the streamer must do where the studio cannot, and where.
	hint: Option<&'static str>,
	/// Each fix the studio can take itself, the first the likeliest.
	fixes: Vec<(&'static str, Mend)>,
	/// Whether to offer the windows this machine lists, to capture one in
	/// its place: a window chosen by id, which the system cannot ask again.
	windows: bool,
}

/// What to say of `source`, called `name`, gone dark; `None` for a kind
/// never watched.
fn advice(name: &str, source: &Source) -> Option<Advice> {
	let whole = ("Capture the whole screen instead", Mend::Replace(whole_screen()));
	let advice = match source {
		Source::PickedWindow | Source::Window { .. } => Advice {
			title: "This window went black.",
			cause: format!(
				"`{name}` may have closed, been minimized, or changed how it draws. Viewers see black where it was."
			),
			hint: match () {
				_ if cfg!(target_os = "macos") => Some(SCREEN_PERMISSION),
				_ => Some(
					"A game in exclusive full screen refuses capture: switch it to borderless or windowed in its display settings.",
				),
			},
			fixes: match source {
				Source::PickedWindow => vec![("Pick it again", Mend::PickAgain), whole],
				_ => vec![whole],
			},
			windows: !matches!(source, Source::PickedWindow),
		},
		Source::PickedDisplay | Source::Display { .. } => Advice {
			title: "This screen went black.",
			cause: format!(
				"`{name}` may be locked or asleep, or showing something that refuses capture, such as protected video. Viewers see black where it was."
			),
			hint: cfg!(target_os = "macos").then_some(SCREEN_PERMISSION),
			fixes: match source {
				Source::PickedDisplay => vec![("Pick it again", Mend::PickAgain)],
				_ => vec![("Open it again", Mend::Reopen)],
			},
			windows: false,
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
			},
			fixes: vec![("Open it again", Mend::Reopen)],
			windows: false,
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

/// "No picture for 12 s", what a capture dark for `dark` shows.
pub(super) fn words(dark: Duration) -> String {
	format!("No picture for {:.0} s", dark.as_secs_f64())
}

/// The amber badge in `rect`: a warning sign on amber.
pub(super) fn badge(ui: &Ui, rect: Rect) {
	let painter = ui.painter();
	painter.circle_filled(rect.center(), rect.width() / 2.0, kit::CUE);
	Icon::Warning.paint(ui, rect, rect.width() * 0.6, kit::ON_CUE);
}

/// The badge for the capture `source`, called `name`, dark for `dark`, in
/// `rect`: a click opens its popover. The fix asked for, if one was; with
/// `windows`, the windows to offer in its place, if listed.
pub(super) fn badge_button(
	ui: &mut Ui,
	rect: Rect,
	id: egui::Id,
	name: &str,
	source: &Source,
	windows: Option<&Result<Vec<Pick>, String>>,
) -> Option<Mend> {
	let response = ui.interact(rect, id, Sense::click());
	let label = format!("What happened to {name}");
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
	badge(ui, rect);
	let advice = advice(name, source)?;
	let mut mend = None;
	kit::popover(&response).width(300.0).show(|ui| {
		mend = popover(ui, &advice, windows);
	});
	mend
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
	if let Some(hint) = advice.hint {
		ui.add_space(4.0);
		ui.add(egui::Label::new(egui::RichText::new(hint).font(kit::ui(11.0)).color(kit::DIM)).wrap());
	}
	ui.add_space(4.0);
	let expected = ui
		.add(egui::Button::new(egui::RichText::new("It is meant to go dark").color(kit::DIM)).frame(false))
		.on_hover_text("Stop watching it for going dark; its inspector watches it again");
	if expected.clicked() {
		mend = Some(Mend::Expected);
	}
	mend
}

/// Mark the dark layer at `at` on the picture: an amber dashed outline, and
/// its name on an amber pill in its middle.
pub(super) fn on_picture(ui: &Ui, at: Rect, name: &str) {
	let painter = ui.painter();
	super::dashed(painter, at.shrink(1.5), Stroke::new(2.0, kit::CUE));

	let text = format!("{name} · no picture");
	let galley = painter.layout_no_wrap(text.to_uppercase(), kit::display(13.0), kit::ON_CUE);
	let size = galley.size() + vec2(44.0, 14.0);
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
			let advice = advice("game", &source).expect("advice");
			assert!(advice.cause.contains("`game`"));
			assert!(!advice.fixes.is_empty() || advice.windows, "{source:?}");
		}
		let picked = advice("game", &Source::PickedWindow).unwrap();
		assert_eq!(picked.fixes[0].1, Mend::PickAgain);
		assert!(
			advice(
				"logo",
				&Source::Image {
					path: "logo.png".into()
				}
			)
			.is_none()
		);
	}
}
