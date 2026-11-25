//! The strip's sound: a row per sound the stream carries, each with its mute,
//! its volume on a slider over its meter, and a gear that opens its options,
//! so everyday audio never needs a page of its own.

use egui::{Align, Rect, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};

use super::mixer::{self, Change};
use super::*;

/// A sound as its row shows it.
pub(super) struct Sound {
	pub(super) name: String,
	pub(super) microphone: bool,
	pub(super) audio: moq_scene::Audio,
}

/// The room a row's slider leaves its knob at each end.
const KNOB: f32 = 7.0;

impl Panels {
	/// A row for each of `sounds` in `rows`, scrolling past as many as fit,
	/// and the options of the one whose gear is open.
	pub(super) fn sound_rows(&mut self, ui: &mut Ui, sounds: &[Sound], rows: Rect) {
		let mut changes = Vec::new();
		// A sound since removed or renamed closes its options.
		if let Some(open) = &self.sound_options
			&& !sounds.iter().any(|sound| sound.name == *open)
		{
			self.sound_options = None;
		}
		ui.scope_builder(UiBuilder::new().max_rect(rows), |ui| {
			egui::ScrollArea::vertical().id_salt("sound").show(ui, |ui| {
				ui.spacing_mut().item_spacing.y = 0.0;
				if sounds.is_empty() {
					ui.label(egui::RichText::new("Nothing on the stream makes a sound yet").color(kit::DIM));
				}
				for sound in sounds {
					let size = vec2(ui.available_width(), layout::SOUND_ROW);
					ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(Align::Center), |ui| {
						ui.spacing_mut().item_spacing.x = 8.0;
						self.sound_row(ui, sound, &mut changes);
					});
				}
			});
		});
		self.apply(changes);
	}

	/// A sound's mute, name, volume over its meter, decibels, and gear.
	fn sound_row(&mut self, ui: &mut Ui, sound: &Sound, changes: &mut Vec<Change>) {
		let name = &sound.name;
		let audio = &sound.audio;
		let muted = audio.muted;
		let icon = match (sound.microphone, muted) {
			(true, false) => Icon::Mic,
			(true, true) => Icon::MicMuted,
			(false, false) => Icon::Sound,
			(false, true) => Icon::Muted,
		};
		let verb = if muted { "Unmute" } else { "Mute" };
		let button = IconButton::new(icon, verb)
			.lit(muted, kit::CUE)
			.look(kit::Look::Boxed)
			.small();
		if kit::named(ui.add(button), &format!("{verb} {name}")).clicked() {
			changes.push(Change::Audio {
				name: name.clone(),
				set: mixer::Setting::Muted(!muted),
				label: format!("{verb} `{name}`"),
				gesture: None,
			});
		}
		let ink = if muted { kit::DIM } else { kit::TEXT };
		let label = egui::Label::new(egui::RichText::new(name).font(kit::ui(12.5)).color(ink)).truncate();
		let width = (ui.available_width() * 0.24).clamp(48.0, 96.0);
		ui.allocate_ui_with_layout(
			vec2(width, layout::SOUND_ROW),
			egui::Layout::left_to_right(Align::Center),
			|ui| {
				ui.set_min_width(width);
				ui.add(label);
			},
		);
		let db = decibels(audio.gain);
		// The decibels and the gear take their room at the right first.
		let along = (ui.available_width() - 44.0 - 24.0 - 2.0 * 8.0).max(48.0);
		let (rect, response) = ui.allocate_exact_size(vec2(along, 18.0), Sense::drag());
		let response = kit::named(response, &format!("{name} volume"));
		let gesture = self.gestures.of(&response);
		let track = egui::Rangef::new(rect.left() + KNOB, rect.right() - KNOB);
		let mut at = mixer::taper(db);
		let mut moved = false;
		if (response.dragged() || response.drag_stopped())
			&& let Some(pointer) = response.interact_pointer_pos()
		{
			at = ((pointer.x - track.min) / track.span()).clamp(0.0, 1.0);
			moved = true;
		}
		let reset = mixer::double_clicked(ui, &response);
		if moved || reset {
			let db = if reset { 0.0 } else { mixer::untaper(at) };
			changes.push(Change::Audio {
				name: name.clone(),
				set: mixer::Setting::Gain(gain(db)),
				label: format!("Set `{name}` volume"),
				gesture,
			});
		}
		response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(db), format!("{name} volume")));
		response.on_hover_text("Drag to set its volume; double-click for 0 dB");
		if ui.is_rect_visible(rect) {
			self.volume(ui, name, muted, rect, track, at);
		}
		let readout = match db.round() {
			_ if db <= MUTE_DB => "-inf dB".to_string(),
			0.0 => "0 dB".to_string(),
			db if db > 0.0 => format!("+{db} dB"),
			db => format!("{db} dB"),
		};
		let readout = egui::RichText::new(readout).font(kit::mono(11.5)).color(kit::DIM);
		ui.allocate_ui_with_layout(
			vec2(44.0, layout::SOUND_ROW),
			egui::Layout::right_to_left(Align::Center),
			|ui| {
				ui.set_min_width(44.0);
				ui.label(readout);
			},
		);
		let open = self.sound_options.as_deref() == Some(name.as_str());
		let gear = IconButton::new(Icon::Settings, "Options").lit(open, kit::P3).small();
		let gear = kit::named(ui.add(gear), &format!("{name} options"));
		if gear.clicked() {
			self.sound_options = match open {
				true => None,
				false => Some(name.clone()),
			};
		}
		let mut shown = self.sound_options.as_deref() == Some(name.as_str());
		egui::Popup::from_response(&gear)
			.id(egui::Id::new(("sound options", name)))
			.open_bool(&mut shown)
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.frame(kit::popover_frame())
			.align(egui::RectAlign::TOP_END)
			.align_alternatives(&[egui::RectAlign::BOTTOM_END, egui::RectAlign::LEFT_END])
			.gap(8.0)
			.width(300.0)
			.show(|ui| self.sound_popover(ui, sound, changes));
		if !shown && self.sound_options.as_deref() == Some(name.as_str()) {
			self.sound_options = None;
		}
	}

	/// The volume slider in `rect`: the meter from silence to unity under a
	/// thin rail, and the knob at `at` on the desk's taper.
	fn volume(&self, ui: &Ui, name: &str, muted: bool, rect: Rect, track: egui::Rangef, at: f32) {
		let painter = ui.painter();
		let y = rect.center().y;
		let rail = Rect::from_min_max(pos2(track.min, y - 2.0), pos2(track.max, y + 2.0));
		painter.rect_filled(rail, 2, kit::P3);
		let (level, peak) = match muted {
			true => (0.0, 0.0),
			false => mixer::reading(self.levels.sound(name)),
		};
		// Full scale sits at unity, so a sound turned up past it reads hot.
		let unity = egui::lerp(track.min..=track.max, mixer::UNITY);
		let meter = Rect::from_min_max(pos2(track.min, y - 2.0), pos2(unity, y + 2.0));
		let painter = ui.painter_at(meter);
		let lit = |amount: f32| egui::lerp(meter.left()..=meter.right(), amount.clamp(0.0, 1.0));
		let hot = mixer::lit(gain(HOT_DB));
		let fill = |from: f32, to: f32, color| {
			if to > from {
				painter.rect_filled(
					Rect::from_min_max(pos2(lit(from), meter.top()), pos2(lit(to), meter.bottom())),
					2,
					color,
				);
			}
		};
		fill(0.0, level.min(hot), kit::OK);
		fill(hot, level, kit::CUE);
		if peak > 0.0 {
			let color = match peak >= 1.0 {
				true => kit::ERR,
				false if peak >= hot => kit::CUE,
				false => kit::OK,
			};
			let x = lit(peak);
			painter.vline(x, meter.y_range(), Stroke::new(2.0, color));
		}
		let x = egui::lerp(track.min..=track.max, at);
		let knob = if muted { kit::DIM } else { egui::Color32::WHITE };
		ui.painter().circle_filled(pos2(x, y), 6.5, knob);
	}

	/// A sound's options: hearing it alone in headphones, its delay, and its
	/// pan, each saved as it changes.
	fn sound_popover(&mut self, ui: &mut Ui, sound: &Sound, changes: &mut Vec<Change>) {
		let name = &sound.name;
		let audio = &sound.audio;
		ui.horizontal(|ui| {
			let icon = if sound.microphone { Icon::Mic } else { Icon::Sound };
			let (at, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
			icon.paint(ui, at, 15.0, kit::TEXT);
			ui.label(egui::RichText::new(format!("{name} options")).font(kit::ui_strong(14.0)));
			ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
				ui.label(egui::RichText::new("Auto-saved").font(kit::ui(11.5)).color(kit::DIM));
			});
		});
		ui.add_space(6.0);
		let soloing = self.engine.soloing();
		let mut heard = soloing.as_ref().is_some_and(|(solo, _)| solo == name);
		let note = match mix::MONITOR {
			true => "Hear it alone on the Listen output, in place of anything else Listen plays",
			false => "Needs a build with the `capture` feature",
		};
		let toggle = ui
			.add_enabled(
				mix::MONITOR,
				kit::Toggle::new(&mut heard).label("Monitor in headphones"),
			)
			.on_hover_text(note)
			.on_disabled_hover_text(note);
		if let Some((solo, Some(why))) = &soloing
			&& solo == name
		{
			ui.label(egui::RichText::new(format!("Stopped: {why}")).color(kit::ERR));
		}
		if toggle.changed() {
			let engine = self.engine.clone();
			let mut state = engine.state.lock().unwrap();
			let source = heard.then_some(name.as_str());
			let result = engine.monitor_source(&mut state, &self.runtime, source, self.speaker.as_deref());
			drop(state);
			self.check(result);
		}
		ui.add_space(4.0);
		egui::Grid::new(("sound options", name))
			.num_columns(3)
			.spacing(vec2(10.0, 8.0))
			.show(ui, |ui| {
				ui.label("Delay");
				let max = moq_audio::bus::MAX_DELAY.as_millis() as u64;
				let mut at = audio.delay.as_millis() as f32 / max as f32;
				let slid = ui
					.add(kit::Slider::new(&mut at).width(150.0))
					.on_hover_text("Hold the sound back to line it up with a camera that lags it");
				let slid = kit::named(slid, &format!("{name} delay"));
				let gesture = self.gestures.of(&slid);
				if slid.changed() {
					let millis = (at * max as f32).round() as u64;
					changes.push(Change::Audio {
						name: name.clone(),
						set: mixer::Setting::Delay(Duration::from_millis(millis)),
						label: format!("Set `{name}` delay"),
						gesture,
					});
				}
				let millis = audio.delay.as_millis();
				ui.label(
					egui::RichText::new(format!("{millis} ms"))
						.font(kit::mono(11.5))
						.color(kit::DIM),
				);
				ui.end_row();

				ui.label("Pan");
				let mut at = (audio.pan + 1.0) / 2.0;
				let slid = ui
					.add(kit::Slider::new(&mut at).width(150.0))
					.on_hover_text("Place the sound between left and right; double-click to center");
				let slid = kit::named(slid, &format!("{name} pan"));
				let gesture = self.gestures.of(&slid);
				let reset = mixer::double_clicked(ui, &slid);
				if slid.changed() || reset {
					let pan = if reset { 0.0 } else { at * 2.0 - 1.0 };
					changes.push(Change::Audio {
						name: name.clone(),
						set: mixer::Setting::Pan(pan),
						label: format!("Set `{name}` pan"),
						gesture,
					});
				}
				ui.label(
					egui::RichText::new(mixer::pan_words(audio.pan))
						.font(kit::mono(11.5))
						.color(kit::DIM),
				);
				ui.end_row();
			});
	}
}
