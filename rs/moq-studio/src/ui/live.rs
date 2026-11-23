//! Going live from the window: the live control on the title bar, the go-live
//! sheet with where the stream goes and the checks before it does, the health
//! dot and what it opens, and the Stream settings.

use std::collections::BTreeMap;

use egui::{Align, Rect, Sense, Stroke, Vec2, pos2, vec2};
use moq_net::bandwidth::Rate;

use super::levels::reading;
use super::*;
use crate::engine::OnAir;
use crate::probe::{self, Encoding};
use crate::stream::{Saved, Stream};
use crate::task::Task;
use crate::uplink;

/// How long END STREAM is held down to end the stream.
const HOLD: Duration = Duration::from_millis(1200);

/// How loud a microphone gets, on the meter's scale, to count as moving.
const MOVING: f32 = 0.2;

/// How long the microphone check listens before saying it heard nothing.
const LISTEN: Duration = Duration::from_secs(4);

/// The go-live sheet's checks, run while it is open.
pub(super) struct Going {
	/// The canvases the encoders were timed at together, the program's and
	/// its turned one while the show goes out in both formats; others start
	/// the checks over.
	canvases: Vec<Canvas>,
	/// How each encodes, in the canvases' order.
	encoder: Task<Result<Vec<Encoding>, String>>,
	/// Started once the encoder is known, since what it sends is what the
	/// connection is measured toward.
	uplink: Option<Measuring>,
	/// The loudest each microphone was since the sheet opened, on the meter's
	/// scale.
	heard: BTreeMap<String, f32>,
	opened: kio::clock::Instant,
	/// Whether the confirm was pressed, so the sheet waits on going live.
	confirmed: bool,
}

/// How a check came out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
	/// Still checking.
	Waiting,
	Passed,
	/// Failed: going live still works, but viewers would notice.
	Failed,
	/// Nothing to check, such as a show without a camera.
	Skipped,
}

/// What a failed check offers to do about it.
#[derive(Clone, Copy)]
enum Fix {
	/// Send the program at this canvas instead.
	Send(Canvas),
	/// Measure the connection again.
	Again,
	/// Open BUILD, where the sources are.
	Build,
}

/// A check's line in the sheet.
struct Check {
	mark: Outcome,
	icon: Icon,
	name: &'static str,
	/// What it found, or what is wrong and why, as one sentence.
	said: String,
	/// The microphone's level, for its meter.
	level: Option<(f32, f32)>,
	fixes: Vec<(String, Fix)>,
}

impl Going {
	/// Start the checks for the program at `canvases`, the encoders' noted as
	/// `probing` until it ends, even once the sheet closes, and not run
	/// without it, while a go-live opens the program's encoders.
	fn new(
		gpu: crate::engine::Gpu,
		runtime: &tokio::runtime::Handle,
		canvases: Vec<Canvas>,
		probing: Option<crate::controls::Probing>,
	) -> Self {
		let encoder = match probing {
			Some(probing) => Task::spawn(runtime, {
				let canvases = canvases.clone();
				move || async move {
					let _probing = probing;
					probe::probe(gpu, &canvases).await
				}
			})
			.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the check: {err}")))),
			None => Task::Done(Err("not checked while a stream goes live".into())),
		};
		Self {
			canvases,
			encoder,
			uplink: None,
			heard: BTreeMap::new(),
			opened: kio::clock::now(),
			confirmed: false,
		}
	}
}

impl Going {
	/// Whether every check that runs in the background has finished.
	#[cfg(test)]
	pub(super) fn done(&self) -> bool {
		matches!(self.encoder, Task::Done(_))
			&& matches!(
				self.uplink,
				Some(Measuring {
					task: Task::Done(_),
					..
				})
			)
	}
}

/// The connection being measured, which stops, withdrawing its probe, when
/// this is dropped, and on its own once the stream starts going live, from
/// the sheet or a hotkey, window shown or not, so it never competes with the
/// stream for the uplink.
struct Measuring {
	task: Task<Result<Rate, String>>,
	/// Dropping it ends the measurement.
	_stop: Option<tokio::sync::oneshot::Sender<()>>,
}

/// The pictures the program encodes at once: its canvas, and the turned one
/// while the show goes out in both formats.
fn program_canvases(collection: &moq_scene::Collection) -> Vec<Canvas> {
	std::iter::once(collection.canvas)
		.chain(collection.portrait_canvas())
		.collect()
}

/// `canvas` as a broadcaster names it, such as `1080p at 60`.
fn named(canvas: Canvas) -> String {
	format!("{}p at {}", canvas.width.min(canvas.height), canvas.framerate)
}

/// A smaller canvas to send, if there is one worth offering: 720p from
/// anything larger, keeping it wider than tall, or taller, as it was, then
/// 30 frames a second rather than more.
fn smaller(canvas: Canvas) -> Option<Canvas> {
	let short = canvas.width.min(canvas.height);
	if short > 720 {
		let scale = |side: u32| (u64::from(side) * 720 / u64::from(short)) as u32 & !1;
		let (width, height) = (scale(canvas.width), scale(canvas.height));
		// Rounding a nearly square canvas can make it square, which a show
		// going out in both formats cannot turn.
		if width.cmp(&height) == canvas.width.cmp(&canvas.height) {
			return Some(Canvas {
				width,
				height,
				framerate: canvas.framerate,
			});
		}
	}
	(canvas.framerate > 30).then_some(Canvas {
		framerate: 30,
		..canvas
	})
}

impl Panels {
	/// The live frame's tally and tag: ON AIR in the accent while live, OFF
	/// AIR in amber on a neutral frame otherwise.
	pub(super) fn program_frame(&self) -> TallyFrame<'static> {
		match self.engine.on_air() {
			OnAir::Live(_) => TallyFrame::new(Tally::Air).tag("On air"),
			OnAir::Ending => TallyFrame::new(Tally::Air).tag("Ending"),
			OnAir::Starting => TallyFrame::new(Tally::Off).tag_as("Going live", Tally::Cue),
			OnAir::Off => TallyFrame::new(Tally::Off).tag_as("Off air", Tally::Cue),
		}
	}

	/// The live control: GO LIVE off air, which opens the go-live sheet; the
	/// clock on air, which reads END STREAM under the pointer and ends the
	/// stream once held down.
	pub(super) fn live_control(&mut self, ui: &mut Ui) {
		let on_air = self.engine.on_air();
		let font = kit::display_bold(17.0);
		let clock_font = kit::mono(14.0);
		let painter = ui.painter().clone();
		let end = kit::caps_galley(&painter, "End stream", font.clone(), Color32::WHITE, None);
		let live_word = kit::caps_galley(&painter, "Live", font.clone(), Color32::WHITE, None);
		let clock_width = painter
			.layout_no_wrap("00:00:00".into(), clock_font.clone(), Color32::WHITE)
			.size()
			.x;
		match on_air {
			OnAir::Off => {
				let key = self.key(&Action::GoLive, ui.ctx());
				// Neither the accent nor red: those say the stream is on air.
				let word = kit::caps_galley(&painter, "Go live", font, kit::BG, None);
				let size = vec2(word.size().x + 36.0, 30.0);
				let (rect, response) = ui.allocate_exact_size(size, Sense::click());
				response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Go live"));
				let response = kit::tip(response, "Go live", key.as_deref());
				let fill = match response.hovered() {
					true => Color32::WHITE,
					false => Color32::from_gray(0xe8),
				};
				painter.rect_filled(rect, 6, fill);
				painter.galley(rect.center() - word.size() / 2.0, word, kit::BG);
				if response.clicked() {
					self.open_going();
				}
			}
			OnAir::Starting | OnAir::Ending => {
				let text = match on_air {
					OnAir::Starting => "Going live",
					_ => "Ending",
				};
				let word = kit::caps_galley(&painter, text, font, kit::DIM, None);
				let size = vec2(word.size().x + 24.0 + 20.0, 30.0);
				let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
				let tip = match on_air {
					OnAir::Starting => "Dialing the relays and opening the encoders",
					_ => "Handing the relays the stream's tail before the sessions close",
				};
				kit::tip(response, tip, None);
				painter.rect(rect, 6, kit::P2, Stroke::new(1.0, kit::LINE), egui::StrokeKind::Inside);
				let spinner = Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(12.0));
				egui::Spinner::new().size(12.0).color(kit::DIM).paint_at(ui, spinner);
				painter.galley(
					pos2(rect.left() + 30.0, rect.center().y - word.size().y / 2.0),
					word,
					kit::DIM,
				);
			}
			OnAir::Live(live) => {
				let key = self.key(&Action::EndStream, ui.ctx());
				let width = (12.0 + live_word.size().x + 10.0 + clock_width + 12.0).max(end.size().x + 24.0);
				let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click_and_drag());
				response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "End stream"));
				let relays = live.relays.join(" and ");
				let note = format!("Live at {} on {relays}. Hold to end the stream.", live.path);
				let held = response.is_pointer_button_down_on();
				let response = kit::tip(response, &note, key.as_deref());
				let ending = match (held, self.ending) {
					(true, Some(since)) => Some(since),
					(true, None) => Some(kio::clock::now()),
					(false, _) => None,
				};
				self.ending = ending;
				let progress = ending.map_or(0.0, |since| since.elapsed().as_secs_f32() / HOLD.as_secs_f32());
				if progress >= 1.0 {
					self.ending = None;
					if let Err(err) = self.controls.end_stream() {
						self.report(Err(err));
					}
				} else if ending.is_some() {
					ui.ctx().request_repaint();
				}
				let glow = egui::Shadow {
					offset: [0, 0],
					blur: 22,
					spread: 0,
					color: kit::ACCENT.gamma_multiply(0.4),
				};
				painter.add(glow.as_shape(rect, egui::CornerRadius::same(6)));
				if response.hovered() || held {
					painter.rect_filled(rect, 6, kit::ERR.gamma_multiply(0.35));
					let filled = rect.with_max_x(rect.left() + rect.width() * progress.min(1.0));
					painter.rect_filled(filled, 6, kit::ERR);
					painter.rect_stroke(rect, 6, Stroke::new(1.0, kit::ERR), egui::StrokeKind::Inside);
					painter.galley(rect.center() - end.size() / 2.0, end, Color32::WHITE);
				} else {
					painter.rect_filled(rect, 6, kit::ACCENT);
					let elapsed = live.since.elapsed();
					let time = painter.layout_no_wrap(super::shell::clock(elapsed), clock_font, Color32::WHITE);
					let x = rect.left() + 12.0;
					let after = x + live_word.size().x + 10.0;
					painter.galley(
						pos2(x, rect.center().y - live_word.size().y / 2.0),
						live_word,
						Color32::WHITE,
					);
					let x = after;
					painter.galley(pos2(x, rect.center().y - time.size().y / 2.0), time, Color32::WHITE);
				}
			}
		}
	}

	/// Open the go-live sheet, starting its checks.
	pub(super) fn open_going(&mut self) {
		let canvases = program_canvases(self.engine.state.lock().unwrap().collection());
		*self.controls.refused.lock().unwrap() = None;
		let probing = self.controls.probing();
		self.going = Some(Going::new(self.engine.gpu().clone(), &self.runtime, canvases, probing));
	}

	/// The go-live sheet, while open: where the stream goes, each with a
	/// switch and its state, the checks as they come in, and one confirm.
	pub(super) fn going_sheet(&mut self, ctx: &egui::Context) {
		if self.going.is_none() {
			return;
		}
		let on_air = self.engine.on_air();
		// Live: the sheet has done its job.
		if on_air.live().is_some() {
			self.going = None;
			return;
		}
		ctx.request_repaint_after(Duration::from_millis(100));
		let broadcast = self.controls.stream.lock().unwrap().broadcast().to_string();
		let (show, program, canvases, both) = {
			let state = self.engine.state.lock().unwrap();
			let path = self.engine.autosave.path();
			let show = path
				.file_stem()
				.unwrap_or(path.as_os_str())
				.to_string_lossy()
				.into_owned();
			let program = state.compositor.program_scene().map(str::to_string);
			let collection = state.collection();
			// Each relay carries both pictures, each its own broadcast.
			let both = collection.portrait.then(|| {
				format!(
					"Both formats go to each relay: landscape at {broadcast}, portrait at {broadcast}/{}.",
					moq_scene::PORTRAIT
				)
			});
			(show, program, program_canvases(collection), both)
		};
		// Not while going live, which keeps the sheet waiting on it, and
		// whose encoders a new check would compete with: once it ends.
		if self.going.as_ref().is_some_and(|going| going.canvases != canvases) && !self.controls.going() {
			self.open_going();
		}
		self.advance_going();
		let checks = self.checks();
		let mut close = false;
		let mut fixes = Vec::new();
		let mut confirm = false;
		let mut settings = false;
		let refused = self.controls.refused.lock().unwrap().clone();
		let confirmed = self.going.as_ref().is_some_and(|going| going.confirmed);
		// The encoder check holds a hardware encode session until it finishes,
		// which would count against the program's.
		let probing = self
			.going
			.as_ref()
			.is_some_and(|going| matches!(going.encoder, Task::Running(_)));
		let id = egui::Id::new("go live");
		let asked = kit::Sheet::new(id, "Go live").show(ctx, |ui| {
			let opens = match &program {
				Some(scene) => format!("{show} opens on {scene}"),
				None => format!("{show} opens on an empty picture"),
			};
			ui.label(egui::RichText::new(opens).color(kit::DIM));
			ui.add_space(10.0);
			heading(ui, "Where it goes");
			let count = self.destinations(ui, &mut settings);
			if let Some(both) = &both {
				ui.add_space(4.0);
				ui.label(egui::RichText::new(both).font(kit::ui(12.0)).color(kit::DIM));
			}
			ui.add_space(10.0);
			heading(ui, "Checks");
			for (index, check) in checks.iter().enumerate() {
				kit::popover_row(ui, index + 1 == checks.len(), |ui| {
					if let Some(fix) = check_row(ui, check) {
						fixes.push(fix);
					}
				});
			}
			ui.add_space(12.0);
			if let Some(refused) = &refused {
				ui.horizontal(|ui| {
					ui.add(Icon::Warning.image(16.0, kit::ERR));
					ui.add(egui::Label::new(egui::RichText::new(refused).color(kit::TEXT)).wrap());
				});
				ui.add_space(8.0);
			}
			let going = self.controls.going();
			let waiting = confirmed && (going || matches!(on_air, OnAir::Starting));
			let text = match (waiting, count) {
				(true, _) => "Going live".to_string(),
				(false, 0) => "Switch a destination on".to_string(),
				(false, _) if probing => "Checking the encoder".to_string(),
				(false, count) => format!("Go live to {count}"),
			};
			let enabled = count > 0 && on_air == OnAir::Off && !going && !probing;
			confirm = confirm_button(ui, &text, enabled, waiting).clicked();
		});
		close |= asked;
		if settings {
			self.setting = Some(Setting::Stream);
			close = true;
		}
		for fix in fixes {
			match fix {
				Fix::Send(canvas) => {
					let label = format!("Send {} instead", named(canvas));
					let mut state = self.engine.state.lock().unwrap();
					let result = super::edit(&mut state, label, None, |draft| draft.set_canvas(canvas));
					drop(state);
					self.check(result);
				}
				Fix::Again => {
					if let Some(going) = &mut self.going {
						going.uplink = None;
					}
				}
				Fix::Build => {
					self.page = Page::Build;
					close = true;
				}
			}
		}
		if confirm {
			match self.controls.go_live() {
				Ok(_) => {
					if let Some(going) = &mut self.going {
						going.confirmed = true;
					}
				}
				Err(err) => *self.controls.refused.lock().unwrap() = Some(format!("{err:#}")),
			}
		}
		if close {
			self.going = None;
		}
	}

	/// Collect the checks that finished, and start measuring the connection
	/// once the encoder says what it sends.
	fn advance_going(&mut self) {
		let Some(going) = &mut self.going else { return };
		going.encoder.poll();
		if going.uplink.is_none()
			&& let Task::Done(encoder) = &going.encoder
		{
			going.uplink = Some(match encoder {
				Ok(encodings) => {
					// Up to past what they send, so a connection that carries
					// it says so rather than stopping at its rate.
					let ceiling = probe::total(encodings).scaled(1.0 / probe::headroom());
					measure(&self.controls, &self.runtime, ceiling)
				}
				Err(_) => Measuring {
					task: Task::Done(Err("the encoder failed, so what it would send is not known".into())),
					_stop: None,
				},
			});
		}
		if let Some(uplink) = &mut going.uplink {
			uplink.task.poll();
		}
	}

	/// Each destination, with its switch and its state; how many are on.
	fn destinations(&mut self, ui: &mut Ui, settings: &mut bool) -> usize {
		let (relay, failover) = {
			let stream = self.controls.stream.lock().unwrap();
			(stream.relay().cloned(), stream.failover().cloned())
		};
		if relay.is_none() && failover.is_none() {
			kit::popover_row(ui, true, |ui| {
				ui.label("No relay yet. Add the one the stream goes to.");
				ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
					*settings = ui.button("Set one up").clicked();
				});
			});
			return 0;
		}
		let watched = self.engine.watched();
		let mut switched = *self.controls.switched.lock().unwrap();
		let rows = [
			("Relay", relay, &mut switched.relay),
			("Failover relay", failover, &mut switched.failover),
		];
		let rows: Vec<_> = rows
			.into_iter()
			.filter_map(|(name, url, on)| Some((name, url?, on)))
			.collect();
		let count = rows.len();
		let mut on_count = 0;
		for (index, (name, url, on)) in rows.into_iter().enumerate() {
			let endpoint = crate::engine::air::endpoint(&url);
			let state = watched.iter().find(|(relay, _)| relay.endpoint == endpoint);
			let (dot, said) = match (*on, state) {
				(false, _) => (None, "Off".to_string()),
				(true, None) => (Some(kit::DIM), "Not dialed yet".to_string()),
				(true, Some((relay, rtt))) => match &relay.state {
					health::State::Connected => (
						Some(kit::OK),
						match rtt {
							Some(rtt) => format!("Connected, {} ms away", rtt.as_millis()),
							None => "Connected".into(),
						},
					),
					health::State::Connecting => (Some(kit::DIM), "Connecting".into()),
					health::State::Disconnected(why) => (Some(kit::ERR), format!("Cannot reach it: {why}")),
				},
			};
			if *on {
				on_count += 1;
			}
			kit::popover_row(ui, index + 1 == count, |ui| {
				ui.add(kit::Toggle::new(on));
				let (at, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
				match dot {
					Some(dot) => ui.painter().circle_filled(at.center(), 4.0, dot),
					None => ui
						.painter()
						.circle_stroke(at.center(), 3.5, Stroke::new(1.0, kit::FAINT)),
				};
				ui.label(egui::RichText::new(name).font(kit::ui_strong(13.0)).color(kit::TEXT))
					// The endpoint, never the address, whose query can carry a token
					// on a shared screen.
					.on_hover_text(endpoint.as_str());
				ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
					ui.add(egui::Label::new(egui::RichText::new(said).color(kit::DIM)).truncate());
				});
			});
		}
		let changed = switched != *self.controls.switched.lock().unwrap();
		*self.controls.switched.lock().unwrap() = switched;
		// The connection is measured to the first relay switched on.
		if changed && let Some(going) = &mut self.going {
			going.uplink = None;
		}
		on_count
	}

	/// The checks, as they stand.
	fn checks(&mut self) -> Vec<Check> {
		let engine = self.engine.clone();
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		let now = kio::clock::now();
		let Some(going) = &mut self.going else {
			return Vec::new();
		};
		let mut checks = Vec::new();

		// Each camera in the show has a picture.
		let cameras: Vec<_> = collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Camera { .. }))
			.map(|(name, _)| {
				let failed = state.compositor.error(name).map(|err| err.to_string());
				let status = state.sources.get(name).map(|feed| feed.status());
				(name.clone(), failed, status)
			})
			.collect();
		let blank = cameras
			.iter()
			.find(|(_, failed, status)| failed.is_some() || !matches!(status, Some(Status::Live)));
		checks.push(match (cameras.first(), blank) {
			(None, _) => Check {
				mark: Outcome::Skipped,
				icon: Icon::Camera,
				name: "Camera",
				said: "This show has no camera".into(),
				level: None,
				fixes: Vec::new(),
			},
			(Some((name, ..)), None) => Check {
				mark: Outcome::Passed,
				icon: Icon::Camera,
				name: "Camera",
				said: format!("{name} has a picture"),
				level: None,
				fixes: Vec::new(),
			},
			(Some(_), Some((name, failed, status))) => {
				let why = match (failed, status) {
					(Some(failed), _) => format!("{name} has no picture: {failed}."),
					(None, Some(Status::Failed(why))) => format!("{name} has no picture: {why}."),
					(None, Some(Status::Starting)) | (None, None) => format!("{name} is still starting."),
					(None, _) => format!("{name} has no picture. Check it is plugged in and not in use."),
				};
				Check {
					mark: Outcome::Failed,
					icon: Icon::Camera,
					name: "Camera",
					said: why,
					level: None,
					fixes: vec![("Open BUILD".into(), Fix::Build)],
				}
			}
		});

		// Each microphone's level moves.
		let microphones: Vec<String> = collection
			.sources
			.iter()
			.filter(|(_, source)| matches!(source, Source::Microphone { .. }))
			.map(|(name, _)| name.clone())
			.collect();
		for name in &microphones {
			let (level, _) = reading(self.levels.sound(name));
			let heard = going.heard.entry(name.clone()).or_default();
			*heard = heard.max(level);
		}
		// The first that has not moved yet, so one silent beside another that
		// moves is still named.
		let silent = microphones
			.iter()
			.find(|name| going.heard.get(*name).is_none_or(|heard| *heard < MOVING));
		let listened = now.saturating_duration_since(going.opened) >= LISTEN;
		let shown = silent.or(microphones.first());
		let level = shown.map(|name| reading(self.levels.sound(name)));
		checks.push(match (shown, silent, listened) {
			(None, ..) => Check {
				mark: Outcome::Skipped,
				icon: Icon::Mic,
				name: "Microphone",
				said: "This show has no microphone".into(),
				level: None,
				fixes: Vec::new(),
			},
			(Some(name), None, _) => Check {
				mark: Outcome::Passed,
				icon: Icon::Mic,
				name: "Microphone",
				said: match microphones.len() {
					1 => format!("{name} is moving"),
					count => format!("All {count} microphones are moving"),
				},
				level,
				fixes: Vec::new(),
			},
			(Some(_), Some(name), false) => Check {
				mark: Outcome::Waiting,
				icon: Icon::Mic,
				name: "Microphone",
				said: format!("Listening to {name}; say something"),
				level,
				fixes: Vec::new(),
			},
			(Some(_), Some(name), true) => Check {
				mark: Outcome::Failed,
				icon: Icon::Mic,
				name: "Microphone",
				said: format!("{name} has been silent. Speak, or check it is not muted in the sound panel."),
				level,
				fixes: Vec::new(),
			},
		});

		// The encoders keep up with their canvases together: a picture in each
		// format, timed at once, while the show goes out in both.
		let encodes = going.canvases.len();
		let formats = match encodes {
			1 => String::new(),
			_ => " in both formats".to_string(),
		};
		let canvas = going.canvases[0];
		let send = smaller(canvas).map(|smaller| (format!("Send {} instead", named(smaller)), Fix::Send(smaller)));
		let encoding = match &going.encoder {
			Task::Done(Ok(encodings)) => Some(encodings.clone()),
			_ => None,
		};
		checks.push(match &going.encoder {
			Task::Running(_) => Check {
				mark: Outcome::Waiting,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Timing {}", named(canvas)),
				level: None,
				fixes: Vec::new(),
			},
			Task::Done(Ok(encodings)) if probe::keeps_up(&going.canvases, encodings) => Check {
				mark: Outcome::Passed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Keeps up with {}{formats}", named(canvas)),
				level: None,
				fixes: Vec::new(),
			},
			Task::Done(Ok(encodings)) => Check {
				mark: Outcome::Failed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!(
					"The encoder takes {} ms a frame, too slow for {}{formats}. Viewers would see it stutter.",
					encodings
						.iter()
						.map(|encoding| encoding.frame)
						.max()
						.unwrap_or_default()
						.as_millis(),
					named(canvas)
				),
				level: None,
				fixes: send.clone().into_iter().collect(),
			},
			Task::Done(Err(err)) => Check {
				mark: Outcome::Failed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("The encoder failed: {err}."),
				level: None,
				// A smaller picture frees no encode session, which a second
				// picture is likelier to be refused for.
				fixes: match encodes {
					1 => send.clone().into_iter().collect(),
					_ => Vec::new(),
				},
			},
		});

		// The connection carries the program. Published scenes and sources
		// share what is left, following the connection as it changes.
		let again = ("Check again".to_string(), Fix::Again);
		checks.push(match (going.uplink.as_ref().map(|uplink| &uplink.task), &encoding) {
			(None, _) | (Some(Task::Running(_)), _) => Check {
				mark: Outcome::Waiting,
				icon: Icon::Uplink,
				name: "Connection",
				said: "Measuring the connection to the relay".into(),
				level: None,
				fixes: Vec::new(),
			},
			(Some(Task::Done(Err(err))), _) => Check {
				mark: Outcome::Failed,
				icon: Icon::Uplink,
				name: "Connection",
				said: format!("The connection was not measured: {err}."),
				level: None,
				fixes: vec![again],
			},
			(Some(Task::Done(Ok(_))), None) => Check {
				mark: Outcome::Skipped,
				icon: Icon::Uplink,
				name: "Connection",
				said: "Measured, but nothing encodes to compare it with".into(),
				level: None,
				fixes: Vec::new(),
			},
			(Some(Task::Done(Ok(carries))), Some(encodings)) => {
				let needs = probe::total(encodings);
				let sending = match encodes {
					1 => "the program",
					_ => "the program in both formats",
				};
				match carries.scaled(probe::headroom()) >= needs {
					true => Check {
						mark: Outcome::Passed,
						icon: Icon::Uplink,
						name: "Connection",
						said: format!(
							"Carries {}, and {sending} needs {}",
							health::rate(*carries),
							health::rate(needs)
						),
						level: None,
						fixes: Vec::new(),
					},
					false => Check {
						mark: Outcome::Failed,
						icon: Icon::Uplink,
						name: "Connection",
						said: format!(
							"The connection carries {} and {sending} needs {}. Viewers would see it stall.",
							health::rate(*carries),
							health::rate(needs)
						),
						level: None,
						fixes: send.into_iter().chain([again]).collect(),
					},
				}
			}
		});
		checks
	}

	/// The stream's health as one dot, its name beside it unless `compact`,
	/// opening the worst problem, every destination, the viewers, and the
	/// encoder, and the details from there.
	pub(super) fn health_dot(&mut self, ui: &mut Ui, fx: &mut Fx, compact: bool) {
		let on_air = self.engine.on_air();
		let watched = self.engine.watched();
		let (name, dot) = health_word(&self.engine);
		let name = name.as_str();
		let report = self.engine.health.lock().unwrap();
		let mark = crate::fx::Mark::new(ui.painter());
		let text = if compact { "" } else { name };
		let pill = ui.add(kit::Pill::new(text).dot(dot).clickable());

		let pill = kit::tip(pill, &format!("Health: {name}"), None);
		fx.health(&mark.around(pill.rect).inset(2.0));
		let mut details = false;
		let mut settings = false;
		kit::popover(&pill).show(|ui| {
			ui.set_width(400.0);
			let row = |ui: &mut Ui, dot: Option<Color32>, name: &str, note: &str, value: &str| {
				kit::popover_row(ui, false, |ui| {
					let (at, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
					match dot {
						Some(dot) => ui.painter().circle_filled(at.center(), 4.0, dot),
						None => ui
							.painter()
							.circle_stroke(at.center(), 3.5, Stroke::new(1.0, kit::FAINT)),
					};
					ui.label(egui::RichText::new(name).color(kit::TEXT));
					ui.add(egui::Label::new(egui::RichText::new(note).color(kit::DIM)).truncate());
					ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
						ui.label(egui::RichText::new(value).font(kit::mono(12.0)).color(kit::TEXT))
					});
				});
			};
			// The worst problem first, on a card of its own.
			if let Some(problem) = report.problems.first() {
				let color = super::color(problem.verdict);
				egui::Frame::new()
					.fill(color.gamma_multiply(0.08))
					.stroke(Stroke::new(1.0, color))
					.corner_radius(10)
					.inner_margin(12)
					.show(ui, |ui| {
						ui.set_width(ui.available_width());
						ui.horizontal_top(|ui| {
							ui.add(Icon::Warning.image(18.0, color));
							ui.add(
								egui::Label::new(
									egui::RichText::new(&problem.cause)
										.font(kit::ui_strong(14.0))
										.color(kit::TEXT),
								)
								.wrap(),
							);
						});
					});
				for problem in report.problems.iter().skip(1) {
					row(ui, Some(super::color(problem.verdict)), &problem.cause, "", "");
				}
			}
			heading(ui, "Destinations");
			match on_air.live() {
				Some(_) => {
					for relay in &report.relays {
						let (dot, state) = match &relay.state {
							health::State::Connecting => (kit::DIM, "connecting"),
							health::State::Connected => (kit::OK, "connected"),
							health::State::Disconnected(_) => (kit::ERR, "unreachable"),
						};
						let rtt = match &report.uplink.relay {
							Some(endpoint) if *endpoint == relay.endpoint => report.uplink.rtt,
							_ => None,
						};
						let rtt = rtt.map(|rtt| format!("{} ms", rtt.as_millis())).unwrap_or_default();
						row(ui, Some(dot), &relay.endpoint, state, &rtt);
					}
				}
				None if watched.is_empty() => {
					kit::popover_row(ui, false, |ui| {
						ui.label("No relay yet.");
						ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
							settings = ui.button("Set one up").clicked();
						});
					});
				}
				None => {
					for (relay, rtt) in &watched {
						let (dot, state) = match &relay.state {
							health::State::Connected => (Some(kit::OK), "standing by"),
							health::State::Connecting => (None, "connecting"),
							health::State::Disconnected(_) => (Some(kit::ERR), "unreachable"),
						};
						let rtt = rtt.map(|rtt| format!("{} ms", rtt.as_millis())).unwrap_or_default();
						row(ui, dot, &relay.endpoint, state, &rtt);
					}
				}
			}
			heading(ui, "Tracks and viewers");
			let uplink = &report.uplink;
			let media = uplink.media.map(health::rate).unwrap_or_default();
			row(ui, None, "Sent", "every track, to the relay", &media);
			let watching = report
				.broadcasts
				.first()
				.and_then(|program| program.viewers.watching)
				.map(|watching| watching.to_string());
			let watching_note = match on_air.live() {
				Some(_) => "",
				None => "off air",
			};
			row(ui, None, "Watching", watching_note, watching.as_deref().unwrap_or("-"));
			let encoder = self.engine.stats.video.encoder();
			let encoder_note = match &encoder {
				Some(_) => "",
				None => "opens on air",
			};
			row(ui, None, "Encoder", encoder_note, encoder.as_deref().unwrap_or(""));
			ui.add_space(4.0);
			details = ui.button("Details").clicked();
		});
		drop(report);
		if details {
			self.details = true;
		}
		if settings {
			self.setting = Some(Setting::Stream);
		}
	}

	/// Apply a save of the stream file once it finished, before every frame
	/// and while the window is hidden, so one still running when Settings
	/// closes, the pane changes, or the window hides applies before a hotkey
	/// goes live.
	pub(super) fn follow_stream_save(&mut self) {
		let Some(saving) = &mut self.saving else { return };
		// The save asks for a frame once it finishes, which polls it again.
		let Some(result) = saving.task.poll().cloned() else {
			return;
		};
		let Saving { from, retokened, .. } = self.saving.take().expect("polled above");
		let saved = match result {
			Ok(saved) => saved,
			Err(err) => {
				self.saved = Some((from, err));
				return;
			}
		};
		let before = self.controls.stream.lock().unwrap().clone();
		// Only a new address, token, or room follows the relays anew, which
		// cuts the broadcast sources and guests they carry, so not on air:
		// then once the stream ends.
		let moved = before.relay() != saved.relay() || before.failover() != saved.failover();
		let rejoined = before.guests() != saved.guests();
		let deferred = self.controls.set_stream(saved.clone(), moved || retokened || rejoined);
		// A measurement under way is of the relay as it was.
		if (moved || retokened)
			&& let Some(going) = &mut self.going
		{
			going.uplink = None;
		}
		let note = match (from, deferred) {
			(Setting::Stream, true) => {
				"Saved. It applies the next time you go live; broadcast sources and guests follow the new relays \
				 once this stream ends."
			}
			(Setting::Stream, false) => "Saved. It applies the next time you go live.",
			(Setting::Guests, true) => "Saved. Guests move to the new room once this stream ends.",
			(Setting::Guests, false) => "Saved. Guests join the new room now; links made before stop renewing.",
			(_, _) => "Saved. The next recording goes there.",
		}
		.to_string();
		// The recorder moved with the save, which REVIEW's shelf follows.
		let scene = self.engine.autosave.path();
		if before.recordings(scene) != saved.recordings(scene) {
			self.review.shelve(&self.engine, &self.runtime);
		}
		match from {
			Setting::Stream => self.stream_form = Some(Form::new(&saved)),
			Setting::Guests => self.guests_form = Some(super::settings::GuestForm::new(saved.saved())),
			_ => {}
		}
		self.saved = Some((from, note));
	}

	/// Settings, Stream: the relay and its token, a failover relay, the
	/// broadcast's name, and on-demand encoding, kept beside the show. A
	/// setting a flag overrides for this run says so, and is not editable.
	pub(super) fn stream_settings(&mut self, ui: &mut Ui) {
		use super::settings::{field_row, row};
		let stream = self.controls.stream.lock().unwrap().clone();
		let overridden = stream.overridden();
		let busy = self.saving.is_some() || self.picker.is_some();
		let form = self.stream_form.get_or_insert_with(|| Form::new(&stream));
		let token = |ui: &mut Ui, relay: &str, token: &mut String, forget: &mut bool| {
			let field = ui.add(
				egui::TextEdit::singleline(token)
					.password(true)
					.hint_text("Token: leave empty to keep the saved one")
					.desired_width(f32::INFINITY),
			);
			kit::named(field, &format!("{relay} token"));
			kit::named(
				ui.checkbox(forget, "Forget the saved token"),
				&format!("Forget the saved {relay} token"),
			);
		};
		// The relays flags set over the saved ones this run, which the token
		// controls key to, and whether each flag's address carries a token of
		// its own, which the stream dials with over any kept.
		let relay_flag = overridden.relay.then(|| stream.relay()).flatten();
		let failover_flag = overridden.failover.then(|| stream.failover()).flatten();
		let carries = |flag: Option<&url::Url>| flag.is_some_and(|url| crate::stream::carries_token(url.as_str()));
		let relay_carries = !overridden.token && carries(relay_flag);
		let failover_carries = carries(failover_flag);
		ui.add_enabled_ui(!busy, |ui| {
			// A flag locks only the address: the token stays the one kept for
			// the relay in effect, which the flag may name, unless the flag's
			// address carries its own.
			field_row(ui, "Relay", "Where the program goes live, and its token.", None, |ui| {
				let field = address(
					ui,
					&mut form.relay,
					"https://relay.example/anon",
					relay_flag.map(|url| ("--connect", url)),
				);
				kit::named(field, "Relay address");
				ui.add_enabled_ui(!overridden.token && !relay_carries, |ui| {
					token(ui, "Relay", &mut form.relay_token, &mut form.forget_relay_token);
				});
				if overridden.token {
					super::settings::flag_note(ui, "--connect-token");
				} else if relay_carries {
					super::settings::cue_note(ui, "The --connect address carries its own token, used over any kept");
				}
			});
			field_row(
				ui,
				"Failover relay",
				"A second relay the same broadcast goes to, which viewers resume from if the first fails. Each is \
				 another upload of the whole stream.",
				None,
				|ui| {
					let field = address(
						ui,
						&mut form.failover,
						"https://backup.example/anon",
						failover_flag.map(|url| ("--failover", url)),
					);
					kit::named(field, "Failover relay address");
					ui.add_enabled_ui(!failover_carries, |ui| {
						token(
							ui,
							"Failover relay",
							&mut form.failover_token,
							&mut form.forget_failover_token,
						);
					});
					if failover_carries {
						super::settings::cue_note(
							ui,
							"The --failover address carries its own token, used over any kept",
						);
					}
				},
			);
			field_row(
				ui,
				"Broadcast name",
				"Where viewers find it on the relays.",
				overridden.broadcast.then_some("--broadcast"),
				|ui| {
					ui.add(
						egui::TextEdit::singleline(&mut form.broadcast)
							.hint_text(crate::stream::BROADCAST)
							.desired_width(f32::INFINITY),
					);
				},
			);
			row(
				ui,
				"Encode only while someone watches",
				"Spares the encoder while nobody watches the full size. A late viewer then gets nothing from before \
				 they arrived.",
				|ui| {
					ui.add_enabled(!overridden.on_demand, kit::Toggle::new(&mut form.on_demand));
				},
			);
			if overridden.on_demand {
				super::settings::flag_note(ui, "--on-demand");
			}
		});
		let edited = form.edited(stream.saved());
		let mut save = None;
		let mut revert = false;
		ui.horizontal(|ui| {
			if ui.add_enabled(edited && !busy, egui::Button::new("Save")).clicked() {
				save = Some(form.saved(stream.saved()));
			}
			revert = ui.add_enabled(edited && !busy, egui::Button::new("Revert")).clicked();
			if let Some(file) = stream.path().file_name() {
				ui.add_space(8.0);
				ui.label(
					egui::RichText::new(format!("Kept in {}", file.to_string_lossy()))
						.font(kit::ui(11.5))
						.color(kit::DIM),
				);
			}
		});
		let tokens = form.tokens(&stream);
		let retokened = !tokens.is_empty()
			|| [&form.relay, &form.failover]
				.iter()
				.any(|url| crate::stream::carries_token(url));
		ui.add_space(6.0);
		self.saved_note(ui, Setting::Stream);
		match save {
			Some(Ok(saved)) => {
				self.saved = None;
				let mut saving = Saving::start(Setting::Stream, stream, saved, tokens, ui.ctx());
				saving.retokened = retokened;
				self.saving = Some(saving);
			}
			Some(Err(err)) => self.saved = Some((Setting::Stream, err)),
			None => {}
		}
		if revert {
			let stream = self.controls.stream.lock().unwrap().clone();
			self.stream_form = Some(Form::new(&stream));
		}
		// On at once, rather than saved with the form above.
		ui.add_space(16.0);
		self.camera(ui, None);
	}
}

/// A relay's address, or the one `flag` sets over it for this run, without
/// its token, locked with a note; its field. The saved address stays in
/// `text`, as a flag is never saved.
fn address(ui: &mut Ui, text: &mut String, hint: &str, flag: Option<(&str, &url::Url)>) -> egui::Response {
	match flag {
		None => ui.add(
			egui::TextEdit::singleline(text)
				.hint_text(hint)
				.desired_width(f32::INFINITY),
		),
		Some((flag, url)) => {
			let shown = crate::stream::redacted(url);
			let field = ui.add_enabled(
				false,
				egui::TextEdit::singleline(&mut shown.as_str()).desired_width(f32::INFINITY),
			);
			super::settings::flag_note(ui, flag);
			field
		}
	}
}

/// Measure the connection to the first relay switched on toward `ceiling`,
/// reading its token on the task's own thread, since the credential store
/// blocks.
fn measure(controls: &Controls, runtime: &tokio::runtime::Handle, ceiling: Rate) -> Measuring {
	let stream = controls.stream.lock().unwrap().clone();
	let switched = *controls.switched.lock().unwrap();
	let handle = runtime.clone();
	let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
	let mut on_air = controls.engine.on_air_changes();
	let task = Task::spawn(runtime, move || {
		let relays = {
			// Each client binds its socket on the runtime.
			let _runtime = handle.enter();
			stream.relays(switched)
		};
		let path = stream.broadcast().to_string();
		async move {
			let relays = relays.map_err(|err| format!("{err:#}"))?;
			let relay = relays.into_iter().next().ok_or("no relay is switched on")?;
			let measured = uplink::measure(&relay.client, relay.url, &path, ceiling, uplink::WINDOW);
			tokio::select! {
				biased;
				// Fails only once the engine is gone, which ends this anyway.
				_ = on_air.wait_for(|on_air| *on_air != OnAir::Off) => Err("stopped as the stream went live".into()),
				// Sent never: the sender is dropped to stop.
				_ = stopped => Err("stopped".into()),
				result = measured => result.map_err(|err| format!("{err:#}")),
			}
		}
	});
	Measuring {
		task: task.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the measurement: {err}")))),
		_stop: Some(stop),
	}
}

/// A section's heading in a sheet or popover.
fn heading(ui: &mut Ui, text: &str) {
	ui.add_space(6.0);
	ui.label(kit::caps(text, kit::display_bold(11.0)).color(kit::FAINT));
	ui.add_space(2.0);
}

/// A check's line: its mark, its icon and name, what it found, and a button
/// for each fix; the fix pressed, if any.
fn check_row(ui: &mut Ui, check: &Check) -> Option<Fix> {
	let mut pressed = None;
	ui.vertical(|ui| {
		ui.horizontal(|ui| {
			let (at, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
			let painter = ui.painter();
			match check.mark {
				Outcome::Waiting => {
					egui::Spinner::new()
						.size(14.0)
						.color(kit::DIM)
						.paint_at(ui, at.shrink(3.0));
				}
				Outcome::Passed => {
					painter.circle_filled(at.center(), 10.0, kit::OK.gamma_multiply(0.2));
					Icon::Check.paint(ui, at.shrink(4.0), 12.0, kit::OK);
				}
				Outcome::Failed => {
					painter.circle_filled(at.center(), 10.0, kit::CUE);
					Icon::Close.paint(ui, at.shrink(4.0), 12.0, kit::ON_CUE);
				}
				Outcome::Skipped => {
					painter.circle_stroke(at.center(), 9.0, Stroke::new(1.0, kit::FAINT));
				}
			}
			if check.mark == Outcome::Failed {
				ui.add(
					egui::Label::new(
						egui::RichText::new(&check.said)
							.font(kit::ui_strong(13.0))
							.color(kit::TEXT),
					)
					.wrap(),
				);
				return;
			}
			ui.add(check.icon.image(14.0, kit::DIM));
			ui.label(
				egui::RichText::new(check.name)
					.font(kit::ui_strong(13.0))
					.color(kit::TEXT),
			);
			ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
				ui.add(egui::Label::new(egui::RichText::new(&check.said).color(kit::DIM)).truncate());
				if let Some((level, peak)) = check.level {
					ui.add(kit::Meter::new(level, peak).run(kit::Run::Across, 80.0, 8.0));
				}
			});
		});
		if !check.fixes.is_empty() {
			ui.horizontal(|ui| {
				ui.add_space(28.0);
				for (index, (label, fix)) in check.fixes.iter().enumerate() {
					let button = match index {
						0 => egui::Button::new(egui::RichText::new(label).color(kit::ON_CUE)).fill(kit::CUE),
						_ => egui::Button::new(label),
					};
					if ui.add(button).clicked() {
						pressed = Some(*fix);
					}
				}
			});
		}
	});
	pressed
}

/// The sheet's confirm: one wide light button, neither the accent nor red.
fn confirm_button(ui: &mut Ui, text: &str, enabled: bool, waiting: bool) -> egui::Response {
	let size = vec2(ui.available_width(), 54.0);
	let sense = if enabled { Sense::click() } else { Sense::hover() };
	let (rect, response) = ui.allocate_exact_size(size, sense);
	response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
	let (fill, ink) = match (enabled, response.hovered()) {
		(true, true) => (Color32::WHITE, kit::BG),
		(true, false) => (Color32::from_gray(0xe8), kit::BG),
		(false, _) => (kit::P3, kit::DIM),
	};
	let painter = ui.painter();
	painter.rect_filled(rect, 8, fill);
	let galley = kit::caps_galley(painter, text, kit::display(22.0), ink, None);
	let mut at = rect.center() - galley.size() / 2.0;
	if waiting {
		let spinner = Rect::from_center_size(pos2(at.x - 14.0, rect.center().y), Vec2::splat(16.0));
		egui::Spinner::new().size(16.0).color(kit::DIM).paint_at(ui, spinner);
		at.x += 6.0;
	}
	ui.painter().galley(at, galley, ink);
	response
}

/// A save of the show's stream file under way, from the Settings pane that
/// asked for it.
pub(super) struct Saving {
	pub(super) from: Setting,
	/// Whether it sets a relay's token, which the relays are followed anew for.
	pub(super) retokened: bool,
	pub(super) task: Task<Result<Stream, String>>,
}

impl Saving {
	/// Keep `tokens` and save `saved` as `stream`'s, on a thread of their own,
	/// since the credential store blocks, asking `ctx` for a frame once the
	/// result is in, which applies it even while the window is hidden. A save
	/// from the Guests pane is refused unless guests could join its room.
	pub(super) fn start(
		from: Setting,
		mut stream: Stream,
		saved: Saved,
		tokens: Vec<Token>,
		ctx: &egui::Context,
	) -> Self {
		let ctx = ctx.clone();
		let (send, recv) = std::sync::mpsc::channel();
		let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
			let result = (|| -> anyhow::Result<Stream> {
				if from == Setting::Guests {
					stream.check_guests(&saved)?;
				}
				for (url, token) in &tokens {
					stream.credentials().set(url, token.as_deref())?;
				}
				stream.save(saved)?;
				Ok(stream)
			})();
			let _ = send.send(result.map_err(|err| format!("{err:#}")));
			// After the send, so the frame it asks for finds the result.
			ctx.request_repaint();
		});
		let task = match spawned {
			Ok(_) => Task::Running(recv),
			Err(err) => Task::Done(Err(format!("cannot save: {err}"))),
		};
		Self {
			from,
			retokened: false,
			task,
		}
	}
}

/// The Stream pane's form.
pub(super) struct Form {
	relay: String,
	relay_token: String,
	forget_relay_token: bool,
	failover: String,
	failover_token: String,
	forget_failover_token: bool,
	broadcast: String,
	on_demand: bool,
}

/// A token to keep for a relay, or `None` to forget its saved one.
pub(super) type Token = (url::Url, Option<String>);

impl Form {
	/// The form holding what `stream` saved, not what the flags set.
	fn new(stream: &Stream) -> Self {
		let saved = stream.saved();
		let text = |url: &Option<url::Url>| url.as_ref().map(url::Url::to_string).unwrap_or_default();
		Self {
			relay: text(&saved.relay),
			relay_token: String::new(),
			forget_relay_token: false,
			failover: text(&saved.failover),
			failover_token: String::new(),
			forget_failover_token: false,
			broadcast: saved.broadcast.clone().unwrap_or_default(),
			on_demand: saved.on_demand,
		}
	}

	/// What the form would save over `saved`, or why it cannot.
	fn saved(&self, saved: &Saved) -> Result<Saved, String> {
		let url = |text: &str, name: &str| -> Result<Option<url::Url>, String> {
			match text.trim() {
				"" => Ok(None),
				text => text
					.parse()
					.map(Some)
					.map_err(|err| format!("The {name} address is not a URL: {err}.")),
			}
		};
		let broadcast = self.broadcast.trim();
		Ok(Saved {
			relay: url(&self.relay, "relay")?,
			failover: url(&self.failover, "failover relay")?,
			broadcast: (!broadcast.is_empty()).then(|| broadcast.to_string()),
			on_demand: self.on_demand,
			..saved.clone()
		})
	}

	/// Whether the form holds anything to save over `saved`.
	fn edited(&self, saved: &Saved) -> bool {
		let tokens = !self.relay_token.is_empty()
			|| !self.failover_token.is_empty()
			|| self.forget_relay_token
			|| self.forget_failover_token;
		tokens || self.saved(saved).is_ok_and(|form| form != *saved) || self.saved(saved).is_err()
	}

	/// The tokens typed or forgotten, by the relay they go with: the one
	/// the form names, or the one a flag of `stream` sets over it.
	fn tokens(&self, stream: &Stream) -> Vec<Token> {
		let overridden = stream.overridden();
		let mut tokens = Vec::new();
		for (flagged, url, token, forget) in [
			(
				overridden.relay.then(|| stream.relay()).flatten(),
				&self.relay,
				&self.relay_token,
				self.forget_relay_token,
			),
			(
				overridden.failover.then(|| stream.failover()).flatten(),
				&self.failover,
				&self.failover_token,
				self.forget_failover_token,
			),
		] {
			let Some(url) = flagged.cloned().or_else(|| url.trim().parse::<url::Url>().ok()) else {
				continue;
			};
			match (token.trim(), forget) {
				("", true) => tokens.push((url, None)),
				("", false) => {}
				(token, _) => tokens.push((url, Some(token.to_string()))),
			}
		}
		tokens
	}
}

/// The stream's health in a word, and its dot's color, or how many layers
/// went dark or stalled when they are among the worst problems. A problem
/// shows even off air, such as a failed source.
pub(super) fn health_word(engine: &Engine) -> (String, Color32) {
	let live = engine.on_air().live().is_some();
	let unwatched = engine.watched().is_empty();
	let report = engine.health.lock().unwrap();
	let worst = report
		.problems
		.first()
		.map_or(Verdict::Healthy, |problem| problem.verdict);
	let (word, dot) = match (worst, live, unwatched) {
		(Verdict::Unhealthy, ..) => ("Unhealthy", kit::ERR),
		(Verdict::Degraded, ..) => ("Degraded", kit::CUE),
		(_, false, true) => ("No relay", kit::DIM),
		(_, false, false) => ("Ready", kit::OK),
		(Verdict::Healthy, true, _) => ("Healthy", kit::OK),
		(Verdict::Unknown, true, _) => ("Starting", kit::DIM),
	};
	// When layers gone dark or stalled are among the worst problems, the
	// word counts them, as the streamer can see and fix them on BUILD. A
	// stalled picture is unhealthy, a dark one degraded.
	let among = match worst {
		Verdict::Unhealthy => report.stalled > 0,
		Verdict::Degraded => report.dark > 0,
		_ => false,
	};
	let counted = among
		.then(|| super::dark::counted(report.dark, report.stalled))
		.flatten();
	(counted.unwrap_or_else(|| word.to_string()), dot)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// 720p from anything larger, keeping its shape, unless rounding would
	/// make a canvas wider than tall square; then a lower frame rate.
	#[test]
	fn a_smaller_canvas_keeps_its_orientation() {
		let canvas = |width, height, framerate| Canvas {
			width,
			height,
			framerate,
		};
		assert_eq!(smaller(canvas(1920, 1080, 60)), Some(canvas(1280, 720, 60)));
		assert_eq!(smaller(canvas(1080, 1920, 30)), Some(canvas(720, 1280, 30)));
		assert_eq!(smaller(canvas(1082, 1080, 60)), Some(canvas(1082, 1080, 30)));
		assert_eq!(smaller(canvas(1082, 1080, 30)), None);
	}
}
