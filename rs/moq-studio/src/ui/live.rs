//! Going live from the window: the live control on the title bar, the go-live
//! sheet with where the stream goes and the checks before it does, the health
//! dot and what it opens, and the Stream settings.

use std::collections::BTreeMap;

use egui::{Align, Rect, Sense, Stroke, Vec2, pos2, vec2};
use moq_net::bandwidth::Rate;

use super::levels::reading;
use super::*;
use crate::engine::OnAir;
use crate::setup::{self, Encoding, Task};
use crate::stream::{Saved, Stream};
use crate::{show, uplink};

/// How long END STREAM is held down to end the stream.
const HOLD: Duration = Duration::from_millis(1200);

/// How loud a microphone gets, on the meter's scale, to count as moving.
const MOVING: f32 = 0.2;

/// How long the microphone check listens before saying it heard nothing.
const LISTEN: Duration = Duration::from_secs(4);

/// The go-live sheet's checks, run while it is open.
pub(super) struct Going {
	/// The canvas the encoder was timed at; another starts the checks over.
	canvas: Canvas,
	encoder: Task<Result<Encoding, String>>,
	/// Started once the encoder is known, since what it sends is what the
	/// connection is measured toward.
	uplink: Option<Task<Result<Rate, String>>>,
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
	/// Start the checks for the program at `canvas`.
	fn new(gpu: crate::engine::Gpu, runtime: &tokio::runtime::Handle, canvas: Canvas) -> Self {
		let encoder = Task::spawn(runtime, move || setup::probe(gpu, canvas))
			.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the check: {err}"))));
		Self {
			canvas,
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
		matches!(self.encoder, Task::Done(_)) && matches!(self.uplink, Some(Task::Done(_)))
	}
}

/// `canvas` as a broadcaster names it, such as `1080p at 60`.
fn named(canvas: Canvas) -> String {
	format!("{}p at {}", canvas.width.min(canvas.height), canvas.framerate)
}

/// A smaller canvas to send, if there is one worth offering: 720p from
/// anything larger, then 30 frames a second rather than more.
fn smaller(canvas: Canvas) -> Option<Canvas> {
	let short = canvas.width.min(canvas.height);
	if short > 720 {
		let scale = |side: u32| (u64::from(side) * 720 / u64::from(short)) as u32 & !1;
		return Some(Canvas {
			width: scale(canvas.width),
			height: scale(canvas.height),
			framerate: canvas.framerate,
		});
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
		let canvas = self.engine.state.lock().unwrap().collection().canvas;
		*self.controls.refused.lock().unwrap() = None;
		self.going = Some(Going::new(self.engine.gpu().clone(), &self.runtime, canvas));
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
		let (show, program, canvas) = {
			let state = self.engine.state.lock().unwrap();
			let path = self.engine.autosave.path();
			let show = path
				.file_stem()
				.unwrap_or(path.as_os_str())
				.to_string_lossy()
				.into_owned();
			let program = state.compositor.program_scene().map(str::to_string);
			(show, program, state.collection().canvas)
		};
		if self.going.as_ref().is_some_and(|going| going.canvas != canvas) {
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
			let waiting = confirmed && matches!(on_air, OnAir::Starting);
			let text = match (waiting, count) {
				(true, _) => "Going live".to_string(),
				(false, 0) => "Switch a destination on".to_string(),
				(false, count) => format!("Go live to {count}"),
			};
			let enabled = count > 0 && on_air == OnAir::Off;
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
				Ok(encoding) => {
					// Up to past what it sends, so a connection that carries
					// it says so rather than stopping at its rate.
					let ceiling = encoding.total().scaled(1.0 / setup::headroom());
					measure(&self.controls, &self.runtime, ceiling)
				}
				Err(_) => Task::Done(Err("nothing encodes, so nothing would be sent".into())),
			});
		}
		if let Some(uplink) = &mut going.uplink {
			uplink.poll();
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
					.on_hover_text(url.as_str());
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
		let moving = microphones
			.iter()
			.find(|name| going.heard.get(*name).is_some_and(|heard| *heard >= MOVING));
		let listened = now.saturating_duration_since(going.opened) >= LISTEN;
		let shown = moving.or(microphones.first());
		let level = shown.map(|name| reading(self.levels.sound(name)));
		checks.push(match (shown, moving, listened) {
			(None, ..) => Check {
				mark: Outcome::Skipped,
				icon: Icon::Mic,
				name: "Microphone",
				said: "This show has no microphone".into(),
				level: None,
				fixes: Vec::new(),
			},
			(Some(name), Some(_), _) => Check {
				mark: Outcome::Passed,
				icon: Icon::Mic,
				name: "Microphone",
				said: format!("{name} is moving"),
				level,
				fixes: Vec::new(),
			},
			(Some(name), None, false) => Check {
				mark: Outcome::Waiting,
				icon: Icon::Mic,
				name: "Microphone",
				said: format!("Listening to {name}; say something"),
				level,
				fixes: Vec::new(),
			},
			(Some(name), None, true) => Check {
				mark: Outcome::Failed,
				icon: Icon::Mic,
				name: "Microphone",
				said: format!("{name} has been silent. Speak, or check it is not muted in the sound panel."),
				level,
				fixes: Vec::new(),
			},
		});

		// The encoder keeps up with the canvas.
		let canvas = going.canvas;
		let send = smaller(canvas).map(|smaller| (format!("Send {} instead", named(smaller)), Fix::Send(smaller)));
		let encoding = match &going.encoder {
			Task::Done(Ok(encoding)) => Some(encoding.clone()),
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
			Task::Done(Ok(encoding)) if encoding.keeps_up(canvas) => Check {
				mark: Outcome::Passed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Keeps up with {}", named(canvas)),
				level: None,
				fixes: Vec::new(),
			},
			Task::Done(Ok(encoding)) => Check {
				mark: Outcome::Failed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!(
					"The encoder takes {} ms a frame, too slow for {}. Viewers would see it stutter.",
					encoding.frame.as_millis(),
					named(canvas)
				),
				level: None,
				fixes: send.clone().into_iter().collect(),
			},
			Task::Done(Err(err)) => Check {
				mark: Outcome::Failed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Nothing encodes {} here: {err}.", named(canvas)),
				level: None,
				fixes: send.clone().into_iter().collect(),
			},
		});

		// The connection carries what the stream sends.
		let again = ("Check again".to_string(), Fix::Again);
		checks.push(match (&going.uplink, &encoding) {
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
			(Some(Task::Done(Ok(carries))), Some(encoding)) => {
				let needs = encoding.total();
				match carries.scaled(setup::headroom()) >= needs {
					true => Check {
						mark: Outcome::Passed,
						icon: Icon::Uplink,
						name: "Connection",
						said: format!(
							"Carries {}, and this stream needs {}",
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
							"The connection carries {} and this stream needs {}. Viewers would see it stall.",
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
		let report = self.engine.health.lock().unwrap();
		let worst = report
			.problems
			.first()
			.map_or(Verdict::Healthy, |problem| problem.verdict);
		// A problem shows even off air, such as a failed source.
		let (name, dot) = match (worst, on_air.live().is_some(), watched.is_empty()) {
			(Verdict::Unhealthy, ..) => ("Unhealthy", kit::ERR),
			(Verdict::Degraded, ..) => ("Degraded", kit::CUE),
			(_, false, true) => ("No relay", kit::DIM),
			(_, false, false) => ("Ready", kit::OK),
			(Verdict::Healthy, true, _) => ("Healthy", kit::OK),
			(Verdict::Unknown, true, _) => ("Starting", kit::DIM),
		};
		let mark = fx.on().then(|| crate::fx::Mark::new(ui.painter()));
		let text = if compact { "" } else { name };
		let pill = ui.add(kit::Pill::new(text).dot(dot).clickable());
		let pill = kit::tip(pill, &format!("Health: {name}"), None);
		if let Some(mark) = mark {
			fx.health(&mark.around(pill.rect).inset(2.0));
		}
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

	/// Settings, Stream: the relay and its token, a failover relay, the
	/// broadcast's name, and on-demand encoding, kept beside the show. A
	/// setting a flag overrides for this run says so.
	pub(super) fn stream_settings(&mut self, ui: &mut Ui) {
		let stream = self.controls.stream.lock().unwrap().clone();
		let overridden = stream.overridden();
		let form = self.stream_form.get_or_insert_with(|| Form::new(&stream));
		if let Some(saving) = &mut form.saving
			&& let Some(saved) = saving.poll()
		{
			match saved.clone() {
				Ok(saved) => {
					*self.controls.stream.lock().unwrap() = saved.clone();
					show::watch(&self.engine, &saved, &self.runtime);
					*form = Form::new(&saved);
					form.note = Some("Saved. It applies the next time you go live.".into());
				}
				Err(err) => {
					form.saving = None;
					form.note = Some(err);
				}
			}
		}
		let busy = form.saving.is_some();
		if busy {
			ui.ctx().request_repaint_after(Duration::from_millis(100));
		}
		ui.label(
			egui::RichText::new(format!(
				"Kept beside the show, in {}. Tokens go in the system's credential store, never in a file.",
				stream.path().display()
			))
			.color(kit::DIM),
		);
		ui.add_space(8.0);
		let flagged = |ui: &mut Ui, flag: &str| {
			ui.label(egui::RichText::new(format!("{flag} overrides this for this run")).color(kit::CUE));
		};
		ui.add_enabled_ui(!busy, |ui| {
			egui::Grid::new("stream settings")
				.num_columns(2)
				.spacing(vec2(14.0, 10.0))
				.show(ui, |ui| {
					ui.label("Relay");
					ui.vertical(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut form.relay)
								.hint_text("https://relay.example/anon")
								.desired_width(360.0),
						);
						if overridden.relay {
							flagged(ui, "--connect");
						}
					});
					ui.end_row();
					ui.label("Token");
					ui.vertical(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut form.relay_token)
								.password(true)
								.hint_text("Leave empty to keep the saved one")
								.desired_width(360.0),
						);
						ui.checkbox(&mut form.forget_relay_token, "Forget the saved token");
						if overridden.token {
							flagged(ui, "--connect-token");
						}
					});
					ui.end_row();
					ui.label("Failover relay");
					ui.vertical(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut form.failover)
								.hint_text("A second relay viewers resume from")
								.desired_width(360.0),
						);
						if overridden.failover {
							flagged(ui, "--failover");
						}
					});
					ui.end_row();
					ui.label("Its token");
					ui.vertical(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut form.failover_token)
								.password(true)
								.hint_text("Leave empty to keep the saved one")
								.desired_width(360.0),
						);
						ui.checkbox(&mut form.forget_failover_token, "Forget the saved token");
					});
					ui.end_row();
					ui.label("Broadcast name");
					ui.vertical(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut form.broadcast)
								.hint_text(crate::stream::BROADCAST)
								.desired_width(360.0),
						);
						if overridden.broadcast {
							flagged(ui, "--broadcast");
						}
					});
					ui.end_row();
					ui.label("On demand");
					ui.vertical(|ui| {
						ui.add(kit::Toggle::new(&mut form.on_demand).label("Encode the full size only while watched"));
						ui.add(
							egui::Label::new(
								egui::RichText::new(
									"Saves the work, but a late viewer gets nothing from before they arrived.",
								)
								.color(kit::DIM),
							)
							.wrap(),
						);
						if overridden.on_demand {
							flagged(ui, "--on-demand");
						}
					});
					ui.end_row();
				});
		});
		ui.add_space(10.0);
		ui.horizontal(|ui| {
			let edited = form.edited(stream.saved());
			if ui.add_enabled(edited && !busy, egui::Button::new("Save")).clicked() {
				match form.saved(stream.saved()) {
					Ok(saved) => {
						let tokens = form.tokens();
						form.saving = Some(save(stream, saved, tokens, &self.runtime));
						form.note = None;
					}
					Err(err) => form.note = Some(err),
				}
			}
			if ui.add_enabled(edited && !busy, egui::Button::new("Revert")).clicked() {
				let stream = self.controls.stream.lock().unwrap().clone();
				*form = Form::new(&stream);
			}
			if busy {
				ui.spinner();
			}
			if let Some(note) = &form.note {
				ui.label(egui::RichText::new(note).color(kit::DIM));
			}
		});
	}
}

/// Measure the connection to the first relay switched on toward `ceiling`,
/// reading its token on the task's own thread, since the credential store
/// blocks.
fn measure(controls: &Controls, runtime: &tokio::runtime::Handle, ceiling: Rate) -> Task<Result<Rate, String>> {
	let stream = controls.stream.lock().unwrap().clone();
	let switched = *controls.switched.lock().unwrap();
	let handle = runtime.clone();
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
			uplink::measure(&relay.client, relay.url, &path, ceiling, uplink::WINDOW)
				.await
				.map_err(|err| format!("{err:#}"))
		}
	});
	task.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the measurement: {err}"))))
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

/// The Stream tab's form.
pub(super) struct Form {
	relay: String,
	relay_token: String,
	forget_relay_token: bool,
	failover: String,
	failover_token: String,
	forget_failover_token: bool,
	broadcast: String,
	on_demand: bool,
	saving: Option<Task<Result<Stream, String>>>,
	/// How the last save went, or why it was refused.
	note: Option<String>,
}

/// A token to keep for a relay, or `None` to forget its saved one.
type Token = (url::Url, Option<String>);

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
			saving: None,
			note: None,
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

	/// The tokens typed or forgotten, by the relay they go with.
	fn tokens(&self) -> Vec<Token> {
		let mut tokens = Vec::new();
		for (url, token, forget) in [
			(&self.relay, &self.relay_token, self.forget_relay_token),
			(&self.failover, &self.failover_token, self.forget_failover_token),
		] {
			let Ok(url) = url.trim().parse::<url::Url>() else {
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

/// Keep `tokens` and save `saved` as `stream`'s, on a thread of their own,
/// since the credential store blocks.
fn save(
	mut stream: Stream,
	saved: Saved,
	tokens: Vec<Token>,
	runtime: &tokio::runtime::Handle,
) -> Task<Result<Stream, String>> {
	let task = Task::spawn(runtime, move || {
		let result = (|| -> anyhow::Result<Stream> {
			for (url, token) in &tokens {
				stream.credentials().set(url, token.as_deref())?;
			}
			stream.save(saved)?;
			Ok(stream)
		})();
		let result = result.map_err(|err| format!("{err:#}"));
		async move { result }
	});
	task.unwrap_or_else(|err| Task::Done(Err(format!("cannot save: {err}"))))
}
