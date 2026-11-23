//! Going live from the window: the live control on the title bar, the check
//! going live waits on, run in the background, the Go live popover under the
//! control, the health dot and what it opens, and the Destinations settings.

use egui::{Align, Rect, Sense, Stroke, Vec2, pos2, vec2};
use moq_net::bandwidth::Rate;

use super::*;
use crate::controls::Probing;
use crate::engine::OnAir;
use crate::setup::{self, Encoding, Task};
use crate::stream::{Saved, Stream, Switched};
use crate::uplink;

/// The check going live waits on, run in the background off air: whether the
/// encoder keeps up with the program, and whether the connection to the
/// first relay carries what it sends.
pub(super) struct Ready {
	/// What it checks; a check of anything else is stale.
	of: Checked,
	/// How each canvas encodes, in the order of `of.canvases`.
	encoder: Task<Result<Vec<Encoding>, String>>,
	/// Started once the encoder is known, since what it sends is what the
	/// connection is measured toward.
	uplink: Option<Measuring>,
}

/// What a check is of: the program's canvases, where it goes, and the
/// network path there. A check goes stale when any of them changes, never
/// with age.
#[derive(Clone, PartialEq)]
struct Checked {
	/// The program's canvas, and its turned one while the show goes out in
	/// both formats, which are encoded together.
	canvases: Vec<Canvas>,
	/// The relay and the failover relay.
	relays: [Option<url::Url>; 2],
	/// The path the probe publishes beneath, which the relay's token covers.
	broadcast: String,
	/// How many saves set a relay's token.
	tokens: u64,
	/// How many sessions the relays watched connected: the first, and each
	/// reconnect, as on a new network path.
	connects: u64,
}

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
	/// Still checking.
	Waiting,
	Passed,
	/// Failed: going live still works, but viewers would notice.
	Failed,
}

/// What a failed check offers to do about it.
#[derive(Clone, Copy)]
enum Fix {
	/// Send the program at this canvas instead.
	Send(Canvas),
	/// Check again, such as once the relay is back.
	Again,
}

/// A check's line.
struct Check {
	mark: Outcome,
	icon: Icon,
	name: &'static str,
	/// What it found, or what is wrong and why, as one sentence.
	said: String,
	/// What the health dot reads while it fails, in a word or two.
	word: &'static str,
	fixes: Vec<(String, Fix)>,
}

/// The Go live popover, while open.
#[derive(Default)]
pub(super) struct Going {
	/// The relay's address and token typed, while the show has no relay.
	address: String,
	token: String,
	/// Why the address typed cannot be saved.
	invalid: Option<String>,
	/// Whether it goes live as soon as the check passes: from a press, until
	/// the check fails.
	armed: bool,
}

impl Ready {
	/// Start checking what `of` names, the encoder noted as `probing` until
	/// it finishes, which a go-live waits out before the program's open.
	fn new(gpu: crate::engine::Gpu, runtime: &tokio::runtime::Handle, of: Checked, probing: Probing) -> Self {
		let encoder = Task::spawn(runtime, {
			let canvases = of.canvases.clone();
			move || async move {
				let _probing = probing;
				setup::probe(gpu, &canvases).await
			}
		})
		.unwrap_or_else(|err| Task::Done(Err(format!("cannot start the check: {err}"))));
		Self {
			of,
			encoder,
			uplink: None,
		}
	}

	/// Collect what finished, and start measuring the connection once the
	/// encoder says what it sends.
	fn advance(&mut self, controls: &Controls, runtime: &tokio::runtime::Handle) {
		self.encoder.poll();
		if self.uplink.is_none()
			&& let Task::Done(encoder) = &self.encoder
		{
			self.uplink = Some(match encoder {
				Ok(encodings) => {
					// Up to past what they send, so a connection that carries
					// it says so rather than stopping at its rate.
					let ceiling = setup::total(encodings).scaled(1.0 / setup::headroom());
					measure(controls, runtime, ceiling)
				}
				Err(_) => Measuring {
					task: Task::Done(Err(uplink::Failure::Unmeasured(
						"the encoder failed, so what it would send is not known".into(),
					))),
					_stop: None,
				},
			});
		}
		if let Some(uplink) = &mut self.uplink {
			uplink.task.poll();
		}
	}

	/// The worst of its checks: failed if any did, else waiting if any is.
	fn outcome(&self) -> Outcome {
		let marks = self.checks().map(|check| check.mark);
		match () {
			_ if marks.contains(&Outcome::Failed) => Outcome::Failed,
			_ if marks.contains(&Outcome::Waiting) => Outcome::Waiting,
			_ => Outcome::Passed,
		}
	}

	/// The encoder and what it encodes, such as `nvenc 1080p at 60`, once it
	/// keeps up.
	fn encoded(&self) -> Option<String> {
		let Task::Done(Ok(encodings)) = &self.encoder else {
			return None;
		};
		let encoding = encodings.first()?;
		setup::keeps_up(&self.of.canvases, encodings)
			.then(|| format!("{} {}", encoding.encoder, named(self.of.canvases[0])))
	}

	/// The checks, as they stand: the encoders keep up with their canvases
	/// together, and the connection carries what they send.
	fn checks(&self) -> [Check; 2] {
		let encodes = self.of.canvases.len();
		let formats = match encodes {
			1 => String::new(),
			_ => " in both formats".to_string(),
		};
		let canvas = self.of.canvases[0];
		let send = smaller(canvas).map(|smaller| (format!("Send {} instead", named(smaller)), Fix::Send(smaller)));
		let encoder = match &self.encoder {
			Task::Running(_) => Check {
				mark: Outcome::Waiting,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Timing {}", named(canvas)),
				word: "",
				fixes: Vec::new(),
			},
			Task::Done(Ok(encodings)) if setup::keeps_up(&self.of.canvases, encodings) => Check {
				mark: Outcome::Passed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("Keeps up with {}{formats}", named(canvas)),
				word: "",
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
				word: "Encoder too slow",
				fixes: send.clone().into_iter().collect(),
			},
			Task::Done(Err(err)) => Check {
				mark: Outcome::Failed,
				icon: Icon::Encoder,
				name: "Encoder",
				said: format!("The encoder failed: {err}."),
				word: "Encoder failed",
				// A smaller picture frees no encode session, which a second
				// picture is likelier to be refused for.
				fixes: match encodes {
					1 => send.clone().into_iter().collect(),
					_ => Vec::new(),
				},
			},
		};

		// The connection carries the program. Published scenes and sources
		// share what is left, following the connection as it changes.
		let again = ("Check again".to_string(), Fix::Again);
		let task = self.uplink.as_ref().map(|uplink| &uplink.task);
		let connection = match (task, &self.encoder) {
			(Some(Task::Done(Ok(carries))), Task::Done(Ok(encodings))) => {
				let needs = setup::total(encodings);
				let sending = match encodes {
					1 => "the program",
					_ => "the program in both formats",
				};
				match carries.scaled(setup::headroom()) >= needs {
					true => Check {
						mark: Outcome::Passed,
						icon: Icon::Uplink,
						name: "Connection",
						said: format!(
							"Carries {}, and {sending} needs {}",
							health::rate(*carries),
							health::rate(needs)
						),
						word: "",
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
						word: "Connection too slow",
						fixes: send.into_iter().chain([again]).collect(),
					},
				}
			}
			(Some(Task::Done(Err(err))), _) => Check {
				mark: Outcome::Failed,
				icon: Icon::Uplink,
				name: "Connection",
				said: format!("The connection was not measured: {err}."),
				word: match err {
					uplink::Failure::Unreached(_) => "Relay not reached",
					uplink::Failure::Unmeasured(_) => "Connection not measured",
				},
				fixes: vec![again],
			},
			// Measuring, or about to once the encoder says what it sends.
			_ => Check {
				mark: Outcome::Waiting,
				icon: Icon::Uplink,
				name: "Connection",
				said: "Measuring the connection to the relay".into(),
				word: "",
				fixes: Vec::new(),
			},
		};
		[encoder, connection]
	}
}

/// The connection being measured, which stops, withdrawing its probe, when
/// this is dropped, and on its own once the stream starts going live, from
/// the popover or a hotkey, window shown or not, so it never competes with
/// the stream for the uplink.
struct Measuring {
	task: Task<Result<Rate, uplink::Failure>>,
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

	/// What a check would be of now, or `None` while the show has no relay.
	fn check_of(&self) -> Option<Checked> {
		let (relays, broadcast) = {
			let stream = self.controls.stream.lock().unwrap();
			let relays = [stream.relay().cloned(), stream.failover().cloned()];
			(relays, stream.broadcast().to_string())
		};
		if relays.iter().all(Option::is_none) {
			return None;
		}
		let canvases = program_canvases(self.engine.state.lock().unwrap().collection());
		Some(Checked {
			canvases,
			relays,
			broadcast,
			tokens: self.tokens,
			connects: self.engine.connects(),
		})
	}

	/// The check, unless it is stale.
	fn fresh(&self) -> Option<&Ready> {
		let of = self.check_of()?;
		self.ready.as_ref().filter(|ready| ready.of == of)
	}

	/// Keep the check fresh off air, before every frame and while the window
	/// is hidden, starting it over whenever what it is of changes; then go
	/// live from the popover once it passes.
	pub(super) fn follow_ready(&mut self) {
		let Some(of) = self.check_of() else {
			self.ready = None;
			self.follow_going();
			return;
		};
		let off = self.engine.on_air() == OnAir::Off && !self.controls.going();
		if !off {
			// One the stream cut short, as it would compete with it, starts
			// over once the stream ends.
			if self
				.ready
				.as_ref()
				.is_some_and(|ready| ready.outcome() == Outcome::Waiting)
			{
				self.ready = None;
			}
			self.follow_going();
			return;
		}
		// One encoder check at a time: a stale one still timing, even one
		// dropped, finishes first, as its thread runs on beside the next.
		let timing = self.controls.probes_running();
		// Not during the preferred relay's first dial, nor a save or follow
		// that would start one, since what it saves and the session it dials
		// count toward what a check is of: one checked before would go stale.
		// A dial that failed checks at once, and again once it connects; one
		// that hangs, or a save or follow that cannot finish, such as on a
		// token the credential store will not read, after a measurement's
		// length.
		let now = kio::clock::now();
		let dialing = self.saving.is_some() || self.controls.refollowing() || self.engine.dialing();
		let dialing = match (dialing, self.dialing) {
			(false, _) => {
				self.dialing = None;
				false
			}
			(true, None) => {
				self.dialing = Some(now);
				true
			}
			(true, Some(since)) => now.saturating_duration_since(since) < uplink::WINDOW,
		};
		if self.ready.as_ref().is_none_or(|ready| ready.of != of) && !timing && !dialing {
			// Not admitted while a go-live opens the program's encoders.
			self.ready = self
				.controls
				.probing()
				.map(|probing| Ready::new(self.engine.gpu().clone(), &self.runtime, of.clone(), probing));
			// A refusal from before is of what this checks again.
			if self.ready.is_some() && self.going.as_ref().is_some_and(|going| going.armed) {
				*self.controls.refused.lock().unwrap() = None;
			}
		}
		// A stale one waits, so it never measures what it is not of.
		if let Some(ready) = self.ready.as_mut().filter(|ready| ready.of == of) {
			ready.advance(&self.controls, &self.runtime);
		}
		self.follow_going();
	}

	/// GO LIVE pressed, from its button or the palette: on air at once when
	/// the check passed, or else the popover, which waits on the check while
	/// it runs, says why when it failed, and asks for a relay when the show
	/// has none.
	pub(super) fn press_go_live(&mut self) {
		*self.controls.refused.lock().unwrap() = None;
		let outcome = self.fresh().map(Ready::outcome);
		// Never to the relay before an edit to it saves.
		let unsaved = self.controls.unsaved.lock().unwrap().is_some();
		if outcome == Some(Outcome::Passed) && !unsaved {
			match self.controls.go_live() {
				Ok(_) => {
					self.going = None;
					return;
				}
				Err(err) => *self.controls.refused.lock().unwrap() = Some(format!("{err:#}")),
			}
		}
		let set = self.check_of().is_some();
		let refused = self.controls.refused.lock().unwrap().is_some();
		self.going = Some(Going {
			armed: set && outcome != Some(Outcome::Failed) && !refused,
			..Going::default()
		});
	}

	/// Go live from the popover once its press is armed and the check passed,
	/// or stop waiting once it failed, or once a relay typed into it could
	/// not be saved; close it once the stream is on its way.
	fn follow_going(&mut self) {
		let Some(going) = &self.going else { return };
		if self.engine.on_air() != OnAir::Off {
			self.going = None;
			return;
		}
		// An edit that will not save until it changes, such as an address
		// that is not one, or one refused, stops the wait with why, rather
		// than wait for an edit that may never come.
		let stream = self.controls.stream.lock().unwrap().clone();
		let stuck = self.saving.is_none() && self.stream_form.as_ref().is_some_and(|form| form.stuck(&stream));
		if going.armed && stuck {
			let why = self.controls.unsaved.lock().unwrap().clone();
			*self.controls.refused.lock().unwrap() = why;
			if let Some(going) = &mut self.going {
				going.armed = false;
			}
			return;
		}
		let unsaved = self.controls.unsaved.lock().unwrap().is_some();
		if !going.armed || unsaved || self.saving.is_some() || self.controls.going() {
			return;
		}
		let outcome = match self.check_of() {
			// The relay typed into the popover was not saved.
			None => Outcome::Failed,
			Some(_) => match self.fresh() {
				Some(ready) => ready.outcome(),
				None => Outcome::Waiting,
			},
		};
		match outcome {
			Outcome::Waiting => {}
			Outcome::Failed => {
				if let Some(going) = &mut self.going {
					going.armed = false;
				}
			}
			Outcome::Passed => match self.controls.go_live() {
				Ok(_) => self.going = None,
				Err(err) => {
					*self.controls.refused.lock().unwrap() = Some(format!("{err:#}"));
					if let Some(going) = &mut self.going {
						going.armed = false;
					}
				}
			},
		}
	}

	/// Apply a check's fix. One from the popover goes live once the check
	/// passes again.
	fn fix(&mut self, fix: Fix) {
		match fix {
			Fix::Send(canvas) => {
				let label = format!("Send {} instead", named(canvas));
				let mut state = self.engine.state.lock().unwrap();
				let result = super::edit(&mut state, label, None, |draft| draft.set_canvas(canvas));
				drop(state);
				self.check(result);
			}
			Fix::Again => self.ready = None,
		}
		if let Some(going) = &mut self.going {
			going.armed = true;
		}
	}

	/// The live control: GO LIVE off air, with its popover under it; LIVE and
	/// the clock on air, which asks before it ends the stream.
	pub(super) fn live_control(&mut self, ui: &mut Ui) {
		let on_air = self.engine.on_air();
		// Asked for from the palette, it holds only while live, so it never
		// opens on its own at a later stream.
		if on_air.live().is_none() {
			self.confirm_end = false;
		}
		let font = kit::display_bold(17.0);
		let clock_font = kit::mono(14.0);
		let painter = ui.painter().clone();
		match on_air {
			OnAir::Off => {
				let key = self.key(&Action::GoLive, ui.ctx());
				// Neither the accent nor red: those say the stream is on air.
				let word = kit::caps_galley(&painter, "Go live", font, kit::BG, None);
				let size = vec2(word.size().x + 36.0, 30.0);
				let (rect, response) = ui.allocate_exact_size(size, Sense::click());
				response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Go live"));
				let response = kit::tip(response, "Go live", key.as_deref());
				let open = self.going.is_some();
				let fill = match response.hovered() || open {
					true => Color32::WHITE,
					false => Color32::from_gray(0xe8),
				};
				painter.rect_filled(rect, 6, fill);
				if open {
					painter.rect_stroke(
						rect.expand(2.0),
						8,
						Stroke::new(1.5, kit::ACCENT),
						egui::StrokeKind::Outside,
					);
				}
				painter.galley(rect.center() - word.size() / 2.0, word, kit::BG);
				if response.clicked() {
					match open {
						true => self.going = None,
						false => self.press_go_live(),
					}
				}
				self.going_popover(&response);
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
				let live_word = kit::caps_galley(&painter, "Live", font, Color32::WHITE, None);
				let time = painter.layout_no_wrap(
					super::shell::clock(live.since.elapsed()),
					clock_font.clone(),
					Color32::WHITE,
				);
				let clock_width = painter
					.layout_no_wrap("00:00:00".into(), clock_font, Color32::WHITE)
					.size()
					.x;
				let width = 12.0 + live_word.size().x + 10.0 + clock_width + 12.0;
				let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
				response
					.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Live: end the stream"));
				let relays = live.relays.join(" and ");
				let note = format!("Live at {} on {relays}. Click to end the stream.", live.path);
				let response = kit::tip(response, &note, key.as_deref());
				let glow = egui::Shadow {
					offset: [0, 0],
					blur: 22,
					spread: 0,
					color: kit::ACCENT.gamma_multiply(0.4),
				};
				painter.add(glow.as_shape(rect, egui::CornerRadius::same(6)));
				let fill = match response.hovered() {
					true => kit::ACCENT.gamma_multiply(1.15),
					false => kit::ACCENT,
				};
				painter.rect_filled(rect, 6, fill);
				let x = rect.left() + 12.0;
				let after = x + live_word.size().x + 10.0;
				painter.galley(
					pos2(x, rect.center().y - live_word.size().y / 2.0),
					live_word,
					Color32::WHITE,
				);
				painter.galley(pos2(after, rect.center().y - time.size().y / 2.0), time, Color32::WHITE);
				// Asked for from the palette, which ends nothing without it.
				if std::mem::take(&mut self.confirm_end) {
					egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
				}
				let mut end = false;
				kit::popover(&response).width(280.0).show(|ui| {
					ui.label(
						egui::RichText::new("End the stream?")
							.font(kit::ui_strong(15.0))
							.color(kit::TEXT),
					);
					ui.add(
						egui::Label::new(
							egui::RichText::new("Viewers see it end. The relays get its last seconds first.")
								.font(kit::ui(12.5))
								.color(kit::DIM),
						)
						.wrap(),
					);
					ui.add_space(8.0);
					ui.horizontal(|ui| {
						let stop = egui::Button::new(
							egui::RichText::new("End stream")
								.font(kit::ui_strong(13.0))
								.color(Color32::WHITE),
						)
						.fill(kit::ERR);
						if ui.add(stop).clicked() {
							end = true;
							ui.close();
						}
						if ui.button("Keep live").clicked() {
							ui.close();
						}
					});
				});
				if end && let Err(err) = self.controls.end_stream() {
					self.report(Err(err));
				}
			}
		}
	}

	/// The Go live popover under `anchor`, while open: a relay to type in
	/// while the show has none; otherwise where the stream goes, the check as
	/// it comes in with a fix for what failed, and one button.
	fn going_popover(&mut self, anchor: &egui::Response) {
		let Some(mut going) = self.going.take() else { return };
		let (relays, broadcast) = {
			let stream = self.controls.stream.lock().unwrap();
			let relays = [
				("Relay", stream.relay().cloned()),
				("Failover relay", stream.failover().cloned()),
			];
			(relays, stream.broadcast().to_string())
		};
		let set = relays.iter().any(|(_, url)| url.is_some());
		let fresh = self.fresh();
		let checks = fresh.map(Ready::checks);
		let outcome = fresh.map(Ready::outcome);
		let encoded = fresh.and_then(Ready::encoded);
		let refused = self.controls.refused.lock().unwrap().clone();
		let pending = self.controls.unsaved.lock().unwrap().clone();
		let saving = self.saving.is_some();
		// Why the relay typed in here could not be saved.
		let unsaved = match &self.saved {
			Some((Setting::Destinations, note)) if !set => Some(note.clone()),
			_ => None,
		};
		let both = self.engine.state.lock().unwrap().collection().portrait.then(|| {
			format!(
				"Both formats go to each relay: landscape at {broadcast}, portrait at {broadcast}/{}.",
				moq_scene::PORTRAIT
			)
		});
		let watched = self.engine.watched();
		let mut open = !anchor.ctx.input(|input| input.key_pressed(egui::Key::Escape));
		let mut fixes = Vec::new();
		let mut now = false;
		let mut press = false;
		let mut settings = false;
		egui::Popup::from_response(anchor)
			.id(egui::Id::new("go live"))
			.open_bool(&mut open)
			.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
			.frame(kit::popover_frame())
			.align(egui::RectAlign::BOTTOM_END)
			.gap(10.0)
			.width(392.0)
			.show(|ui| {
				ui.set_width(392.0);
				ui.horizontal(|ui| {
					ui.label(
						egui::RichText::new("Go live")
							.font(kit::ui_strong(16.0))
							.color(kit::TEXT),
					);
					if let Some(encoded) = &encoded {
						ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
							ui.label(
								egui::RichText::new(format!("● {encoded} ready"))
									.font(kit::ui(12.0))
									.color(kit::OK),
							);
						});
					}
				});
				ui.add_space(6.0);
				if !set {
					// What was submitted stays as it was until it saves or fails.
					let waiting = saving || going.armed;
					ui.label(egui::RichText::new("Relay address").font(kit::ui(12.5)).color(kit::DIM));
					ui.add_enabled(
						!waiting,
						egui::TextEdit::singleline(&mut going.address)
							.id_salt("go live address")
							.hint_text("https://relay.example/anon")
							.desired_width(f32::INFINITY),
					);
					ui.add_enabled(
						!waiting,
						egui::TextEdit::singleline(&mut going.token)
							.id_salt("go live token")
							.password(true)
							.hint_text("Token (optional)")
							.desired_width(f32::INFINITY),
					);
					if let Some(why) = going.invalid.as_ref().or(unsaved.as_ref()) {
						ui.add(egui::Label::new(egui::RichText::new(why).font(kit::ui(12.0)).color(kit::ERR)).wrap());
					}
					ui.add_space(4.0);
					let enabled = !waiting && !going.address.trim().is_empty();
					now = wide_button(ui, "Go live now", enabled, waiting).clicked();
					ui.add_space(2.0);
					ui.vertical_centered(|ui| {
						ui.label(
							egui::RichText::new("Saved in Settings, Destinations. Next time GO LIVE starts at once.")
								.font(kit::ui(11.5))
								.color(kit::DIM),
						);
					});
					return;
				}
				let shown: Vec<_> = relays
					.iter()
					.filter_map(|(name, url)| Some((*name, url.as_ref()?)))
					.collect();
				for (name, url) in &shown {
					let endpoint = crate::engine::air::endpoint(url);
					let state = watched.iter().find(|(relay, _)| relay.endpoint == endpoint);
					let (dot, said) = match state {
						None => (kit::DIM, "Not dialed yet".to_string()),
						Some((relay, rtt)) => match &relay.state {
							health::State::Connected => (
								kit::OK,
								match rtt {
									Some(rtt) => format!("Connected, {} ms away", rtt.as_millis()),
									None => "Connected".into(),
								},
							),
							health::State::Connecting => (kit::DIM, "Connecting".into()),
							health::State::Disconnected(why) => (kit::ERR, format!("Cannot reach it: {why}")),
						},
					};
					kit::popover_row(ui, false, |ui| {
						let (at, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
						ui.painter().circle_filled(at.center(), 4.0, dot);
						ui.label(egui::RichText::new(*name).font(kit::ui_strong(13.0)).color(kit::TEXT))
							// The endpoint, never the address, whose query can
							// carry a token on a shared screen.
							.on_hover_text(endpoint.as_str());
						ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
							ui.add(egui::Label::new(egui::RichText::new(said).color(kit::DIM)).truncate());
						});
					});
				}
				if let Some(both) = &both {
					ui.add_space(2.0);
					ui.label(egui::RichText::new(both).font(kit::ui(12.0)).color(kit::DIM));
				}
				match &checks {
					Some(checks) => {
						for (index, check) in checks.iter().enumerate() {
							kit::popover_row(ui, index + 1 == checks.len(), |ui| {
								if let Some(fix) = check_row(ui, check) {
									fixes.push(fix);
								}
							});
						}
					}
					None => {
						kit::popover_row(ui, true, |ui| {
							ui.spinner();
							ui.label(egui::RichText::new("Starting the check").color(kit::DIM));
						});
					}
				}
				ui.add_space(8.0);
				if let Some(pending) = pending.as_ref().filter(|_| going.armed) {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.add(
							egui::Label::new(egui::RichText::new(format!("Waiting: {pending}.")).color(kit::DIM))
								.wrap(),
						);
					});
					ui.add_space(6.0);
				}
				if let Some(refused) = &refused {
					ui.horizontal(|ui| {
						ui.add(Icon::Warning.image(16.0, kit::ERR));
						ui.add(egui::Label::new(egui::RichText::new(refused).color(kit::TEXT)).wrap());
					});
					ui.add_space(6.0);
				}
				let failed = outcome == Some(Outcome::Failed);
				let (text, enabled, waiting) = match (going.armed, failed) {
					(true, _) => ("Going live once checked", false, true),
					(false, true) => ("Go live anyway", true, false),
					(false, false) => ("Go live", true, false),
				};
				press = wide_button(ui, text, enabled, waiting).clicked();
				ui.add_space(2.0);
				ui.vertical_centered(|ui| {
					settings = ui
						.add(
							egui::Button::new(
								egui::RichText::new("Change in Settings, Destinations")
									.font(kit::ui(11.5))
									.color(kit::DIM),
							)
							.frame(false),
						)
						.clicked();
				});
			});
		if now {
			going.invalid = None;
			match going.address.trim().parse::<url::Url>() {
				Err(err) => going.invalid = Some(format!("This is not an address: {err}.")),
				Ok(url) => {
					let stream = self.controls.stream.lock().unwrap().clone();
					let saved = Saved {
						relay: Some(url.clone()),
						..stream.saved().clone()
					};
					let token = going.token.trim();
					let tokens: Vec<Token> = match token.is_empty() {
						true => Vec::new(),
						false => vec![(url.clone(), Some(token.to_string()))],
					};
					let retokened = !tokens.is_empty() || crate::stream::carries_token(url.as_str());
					if self.refuse_overlap(Setting::Destinations) {
						going.invalid = Some("Another save is still running; try again once it finishes.".into());
					} else {
						self.saved = None;
						let mut saving = Saving::start(Setting::Destinations, stream, saved, tokens, &anchor.ctx);
						saving.retokened = retokened;
						self.saving = Some(saving);
						going.armed = true;
					}
				}
			}
		}
		if press {
			match outcome {
				// Going live anyway, past a failed check, as a hotkey would.
				Some(Outcome::Failed) => {
					*self.controls.refused.lock().unwrap() = None;
					match self.controls.go_live() {
						Ok(_) => open = false,
						Err(err) => *self.controls.refused.lock().unwrap() = Some(format!("{err:#}")),
					}
				}
				_ => {
					*self.controls.refused.lock().unwrap() = None;
					going.armed = true;
				}
			}
		}
		if settings {
			self.setting = Some(Setting::Destinations);
			open = false;
		}
		self.going = open.then_some(going);
		for fix in fixes {
			self.fix(fix);
		}
	}

	/// The stream's health as one dot, its name beside it unless `compact`,
	/// opening the worst problem, every destination, the viewers, and the
	/// encoder, and the details from there. Off air, a failed check going
	/// live waits on is the worst problem, with its fix.
	pub(super) fn health_dot(&mut self, ui: &mut Ui, fx: &mut Fx, compact: bool) {
		let on_air = self.engine.on_air();
		let watched = self.engine.watched();
		let failed: Vec<Check> = match on_air {
			OnAir::Off => self
				.fresh()
				.map(|ready| {
					ready
						.checks()
						.into_iter()
						.filter(|check| check.mark == Outcome::Failed)
						.collect()
				})
				.unwrap_or_default(),
			_ => Vec::new(),
		};
		let (name, dot) = match failed.first() {
			Some(check) => (check.word.to_string(), kit::CUE),
			None => health_word(&self.engine),
		};
		let name = name.as_str();
		let report = self.engine.health.lock().unwrap();
		let mark = crate::fx::Mark::new(ui.painter());
		let text = if compact { "" } else { name };
		let pill = ui.add(kit::Pill::new(text).dot(dot).clickable());

		let pill = kit::tip(pill, &format!("Health: {name}"), None);
		fx.health(&mark.around(pill.rect).inset(2.0));
		let mut details = false;
		let mut settings = false;
		let mut fixes = Vec::new();
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
			// What going live would run into, with its fix.
			for check in &failed {
				kit::popover_row(ui, false, |ui| {
					if let Some(fix) = check_row(ui, check) {
						fixes.push(fix);
					}
				});
			}
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
			self.setting = Some(Setting::Destinations);
		}
		for fix in fixes {
			self.fix(fix);
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
		let Saving {
			from,
			retokened,
			form: by_form,
			..
		} = self.saving.take().expect("polled above");
		let saved = match result {
			Ok(saved) => saved,
			Err(err) => {
				// A token it could not put back is in use, so a check made
				// with the one before is stale.
				self.tokens += u64::from(retokened);
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
		// A check made with the token before is stale.
		self.tokens += u64::from(retokened);
		let note = match (from, deferred) {
			(Setting::Destinations, true) => {
				"Saved. It applies the next time you go live; broadcast sources and guests follow the new relays \
				 once this stream ends."
			}
			(Setting::Destinations, false) => "Saved. It applies the next time you go live.",
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
		// The form whose save this was takes the file as saved once it holds
		// just what it sent; edited since, even back to what it held before,
		// it keeps its edit, which saves next. Another form holding no edit
		// takes the file too; one holding an edit tries it again, since what
		// refused it, such as a relay guests join through, may have changed.
		if let Some(form) = &mut self.stream_form {
			let own = by_form && from == Setting::Destinations;
			match (own && form.sent(&before)) || (!own && !form.edited(before.saved())) {
				true => self.stream_form = Some(Form::new(&saved)),
				false => form.tried = None,
			}
		}
		if let Some(form) = &mut self.guests_form {
			let own = by_form && from == Setting::Guests;
			match (own && form.sent(before.saved())) || (!own && !form.edited(before.saved())) {
				true => self.guests_form = Some(super::settings::GuestForm::new(saved.saved())),
				false => form.retry(),
			}
		}
		self.saved = Some((from, note));
	}

	/// Save the Destinations and Guests panes' edits, before every frame, once
	/// none of their fields has the keyboard, so a relay is never dialed at a
	/// half-typed address, and once Settings closes. An edit that cannot be
	/// saved says why in its pane, and is not tried again until it changes.
	pub(super) fn autosave_settings(&mut self, ctx: &egui::Context) {
		let stream = self.controls.stream.lock().unwrap().clone();
		self.publish_unsaved(&stream);
		if self.typing || self.saving.is_some() || self.picker.is_some() {
			return;
		}
		if let Some(form) = &mut self.stream_form
			&& let Some((saved, tokens)) = form.next(&stream)
		{
			let retokened = !tokens.is_empty()
				|| [saved.relay.as_ref(), saved.failover.as_ref()]
					.into_iter()
					.flatten()
					.any(|url| crate::stream::carries_token(url.as_str()));
			self.saved = None;
			let mut saving = Saving::start(Setting::Destinations, stream, saved, tokens, ctx);
			saving.retokened = retokened;
			saving.form = true;
			self.saving = Some(saving);
			return;
		}
		if let Some(form) = &mut self.guests_form
			&& let Some(saved) = form.next(stream.saved())
		{
			self.saved = None;
			let mut saving = Saving::start(Setting::Guests, stream, saved, Vec::new(), ctx);
			saving.form = true;
			self.saving = Some(saving);
		}
	}

	/// Save the Destinations and Guests edits still held, even one being
	/// typed, and wait for every save, on the way out of the show. Fails with
	/// why when an edit could not be saved.
	pub(super) fn flush_settings(&mut self) -> anyhow::Result<()> {
		// Nothing draws once the show closes; a save only asks it for a frame.
		let ctx = egui::Context::default();
		loop {
			if let Some(saving) = &mut self.saving {
				saving.task.wait();
			}
			self.follow_stream_save();
			self.typing = false;
			self.autosave_settings(&ctx);
			if self.saving.is_none() {
				break;
			}
		}
		let stream = self.controls.stream.lock().unwrap().clone();
		let saved = stream.saved();
		// Why each pane still holds its edit: it is not valid, or its last
		// save, the pane's own note, was refused.
		let refused = |pane: Setting| {
			self.saved
				.as_ref()
				.filter(|(from, _)| *from == pane)
				.map(|(_, note)| note.clone())
				.unwrap_or_else(|| "it was refused".into())
		};
		let held = [
			self.stream_form.as_ref().filter(|form| form.edited(saved)).map(|form| {
				(
					"Settings, Destinations",
					form.saved(saved)
						.err()
						.unwrap_or_else(|| refused(Setting::Destinations)),
				)
			}),
			self.guests_form.as_ref().filter(|form| form.edited(saved)).map(|form| {
				(
					"Settings, Guests",
					form.saved(saved).err().unwrap_or_else(|| refused(Setting::Guests)),
				)
			}),
		];
		match held.into_iter().flatten().next() {
			None => Ok(()),
			Some((pane, why)) => anyhow::bail!("{pane} has a change not saved: {why}"),
		}
	}

	/// Tell a go-live, from a hotkey too, whether a Destinations edit or
	/// save is still to land, which it waits for. Called as the pane is
	/// edited too, so a key pressed before the next frame sees the edit.
	fn publish_unsaved(&self, stream: &Stream) {
		let unsaved = match (&self.saving, &self.stream_form) {
			(Some(saving), _) if saving.from == Setting::Destinations => {
				Some("a change to Settings, Destinations is still saving".to_string())
			}
			(_, Some(form)) if form.edited(stream.saved()) => {
				Some("Settings, Destinations has a change not saved yet; finish or undo it there".to_string())
			}
			_ => None,
		};
		*self.controls.unsaved.lock().unwrap() = unsaved;
	}

	/// Settings, Destinations: the relay and its token and the broadcast's
	/// name, with a failover relay and on-demand encoding under Advanced,
	/// kept beside the show and saved as they change. A setting a flag
	/// overrides for this run says so, and is not editable.
	pub(super) fn destinations_pane(&mut self, ui: &mut Ui) {
		use super::settings::{field_row, row};
		let stream = self.controls.stream.lock().unwrap().clone();
		let overridden = stream.overridden();
		let form = self.stream_form.get_or_insert_with(|| Form::new(&stream));
		let addresses = [form.relay.clone(), form.failover.clone()];
		let mut typing = false;
		let token = |ui: &mut Ui, relay: &str, token: &mut String, forget: &mut bool, typing: &mut bool| {
			let field = ui.add(
				egui::TextEdit::singleline(token)
					.password(true)
					.hint_text("Token: leave empty to keep the saved one")
					.desired_width(f32::INFINITY),
			);
			*typing |= kit::named(field, &format!("{relay} token")).has_focus();
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
		// A flag locks only the address: the token stays the one kept for the
		// relay in effect, which the flag may name, unless the flag's address
		// carries its own.
		field_row(ui, "Relay", "Where the program goes live, and its token.", None, |ui| {
			let field = address(
				ui,
				&mut form.relay,
				"https://relay.example/anon",
				relay_flag.map(|url| ("--connect", url)),
			);
			typing |= kit::named(field, "Relay address").has_focus();
			ui.add_enabled_ui(!overridden.token && !relay_carries, |ui| {
				token(
					ui,
					"Relay",
					&mut form.relay_token,
					&mut form.forget_relay_token,
					&mut typing,
				);
			});
			if overridden.token {
				super::settings::flag_note(ui, "--connect-token");
			} else if relay_carries {
				super::settings::cue_note(ui, "The --connect address carries its own token, used over any kept");
			}
		});
		field_row(
			ui,
			"Broadcast name",
			"Where viewers find it on the relays.",
			overridden.broadcast.then_some("--broadcast"),
			|ui| {
				let field = ui.add(
					egui::TextEdit::singleline(&mut form.broadcast)
						.hint_text(crate::stream::BROADCAST)
						.desired_width(f32::INFINITY),
				);
				typing |= field.has_focus();
			},
		);
		// Rarely wanted: a second relay, and encoding only while watched.
		let id = ui.make_persistent_id("destinations advanced");
		let mut advanced = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
		// Found by the palette, it opens to show what it holds.
		if ADVANCED.iter().any(|name| super::settings::seeking(ui.ctx(), name)) {
			advanced.set_open(true);
		}
		let mut set = Vec::new();
		if stream.failover().is_some() {
			set.push("a failover relay");
		}
		if stream.on_demand() {
			set.push("on demand");
		}
		ui.add_space(8.0);
		advanced
			.show_header(ui, |ui| {
				ui.label(
					egui::RichText::new("Advanced")
						.font(kit::ui_strong(13.5))
						.color(kit::TEXT),
				);
				if !set.is_empty() {
					ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
						ui.label(
							egui::RichText::new(format!("With {}", set.join(" and ")))
								.font(kit::ui(12.0))
								.color(kit::DIM),
						);
					});
				}
			})
			.body(|ui| {
				field_row(
					ui,
					ADVANCED[0],
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
						typing |= kit::named(field, "Failover relay address").has_focus();
						ui.add_enabled_ui(!failover_carries, |ui| {
							token(
								ui,
								"Failover relay",
								&mut form.failover_token,
								&mut form.forget_failover_token,
								&mut typing,
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
				row(
					ui,
					ADVANCED[1],
					"Spares the encoder while nobody watches the full size. A late viewer then gets nothing from \
					 before they arrived.",
					|ui| {
						ui.add_enabled(!overridden.on_demand, kit::Toggle::new(&mut form.on_demand));
					},
				);
				if overridden.on_demand {
					super::settings::flag_note(ui, "--on-demand");
				}
			});
		ui.add_space(8.0);
		if let Err(err) = form.saved(stream.saved()) {
			ui.add(egui::Label::new(egui::RichText::new(err).font(kit::ui(12.0)).color(kit::ERR)).wrap());
		}
		if let Some(file) = stream.path().file_name() {
			ui.label(
				egui::RichText::new(format!("Kept in {}", file.to_string_lossy()))
					.font(kit::ui(11.5))
					.color(kit::DIM),
			);
		}
		if let Some(form) = &mut self.stream_form {
			form.readdressed([&addresses[0], &addresses[1]]);
		}
		self.typing |= typing;
		self.publish_unsaved(&stream);
		ui.add_space(6.0);
		self.saved_note(ui, Setting::Destinations);
		// On at once, as every setting here is.
		ui.add_space(16.0);
		self.camera(ui, None);
	}
}

/// The Destinations pane's settings under Advanced, by the names their rows
/// show, which the palette finds them by.
const ADVANCED: [&str; 2] = ["Failover relay", "Encode only while someone watches"];

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

/// Measure the connection to the first relay toward `ceiling`, reading its
/// token on the task's own thread, since the credential store blocks.
fn measure(controls: &Controls, runtime: &tokio::runtime::Handle, ceiling: Rate) -> Measuring {
	let stream = controls.stream.lock().unwrap().clone();
	let handle = runtime.clone();
	let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
	let mut on_air = controls.engine.on_air_changes();
	let task = Task::spawn(runtime, move || {
		let relays = {
			// Each client binds its socket on the runtime.
			let _runtime = handle.enter();
			// Only the one measured, so a failover's token the credential
			// store will not read fails going live, not this.
			let preferred = stream.relay().is_some();
			stream.relays(Switched {
				relay: preferred,
				failover: !preferred,
			})
		};
		let path = stream.broadcast().to_string();
		async move {
			// A relay that cannot be set up, such as on a token the credential
			// store would not read, is not dialed.
			let relays = relays.map_err(|err| uplink::Failure::Unreached(format!("{err:#}")))?;
			let relay = relays
				.into_iter()
				.next()
				.ok_or_else(|| uplink::Failure::Unreached("no relay is set".into()))?;
			let measured = uplink::measure(&relay.client, relay.url, &path, ceiling, uplink::WINDOW);
			let halted = |why: &str| Err(uplink::Failure::Unmeasured(why.into()));
			tokio::select! {
				biased;
				// Fails only once the engine is gone, which ends this anyway.
				_ = on_air.wait_for(|on_air| *on_air != OnAir::Off) => halted("stopped as the stream went live"),
				// Sent never: the sender is dropped to stop.
				_ = stopped => halted("stopped"),
				result = measured => result,
			}
		}
	});
	Measuring {
		task: task.unwrap_or_else(|err| {
			Task::Done(Err(uplink::Failure::Unmeasured(format!(
				"cannot start the measurement: {err}"
			))))
		}),
		_stop: Some(stop),
	}
}

/// A section's heading in a popover.
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

/// The popover's one button, wide and light: neither the accent nor red,
/// which say the stream is on air. A spinner beside it while `waiting`.
fn wide_button(ui: &mut Ui, text: &str, enabled: bool, waiting: bool) -> egui::Response {
	let size = vec2(ui.available_width(), 40.0);
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
	let galley = kit::caps_galley(painter, text, kit::display(17.0), ink, None);
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
/// asked for it, or the Go live popover, as Destinations.
pub(super) struct Saving {
	pub(super) from: Setting,
	/// Whether it sets a relay's token, which the relays are followed anew for.
	pub(super) retokened: bool,
	/// Whether it saves its pane's form, which takes the file once saved.
	pub(super) form: bool,
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
				stream.save(saved, &tokens)?;
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
			form: false,
			task,
		}
	}
}

/// The Destinations pane's form, holding edits until they save.
pub(super) struct Form {
	relay: String,
	relay_token: String,
	forget_relay_token: bool,
	failover: String,
	failover_token: String,
	forget_failover_token: bool,
	broadcast: String,
	on_demand: bool,
	/// The last save it started, which is not tried again until the form
	/// changes, so one refused waits for the next edit.
	tried: Option<(Saved, Vec<Token>)>,
}

pub(super) use crate::stream::Token;

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
			tried: None,
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

	/// Whether the form holds a change that will not save until it is edited
	/// again: one that cannot be, or one tried as it is and refused.
	fn stuck(&self, stream: &Stream) -> bool {
		if !self.edited(stream.saved()) {
			return false;
		}
		match self.saved(stream.saved()) {
			Err(_) => true,
			Ok(saved) => self.tried.as_ref() == Some(&(saved, self.tokens(stream))),
		}
	}

	/// What to save over `stream`'s settings, if the form holds a change
	/// that can be saved and was not tried as it is.
	fn next(&mut self, stream: &Stream) -> Option<(Saved, Vec<Token>)> {
		if !self.edited(stream.saved()) {
			return None;
		}
		let next = (self.saved(stream.saved()).ok()?, self.tokens(stream));
		if self.tried.as_ref() == Some(&next) {
			return None;
		}
		self.tried = Some(next.clone());
		Some(next)
	}

	/// Clear the token typed, or the forget box ticked, for a relay whose
	/// address was edited away from `before` (the relay's, then the
	/// failover's), so a token only ever goes to the address it was typed
	/// beside, even one a save in flight still sends.
	fn readdressed(&mut self, before: [&str; 2]) {
		for ((address, token, forget), before) in [
			(&self.relay, &mut self.relay_token, &mut self.forget_relay_token),
			(
				&self.failover,
				&mut self.failover_token,
				&mut self.forget_failover_token,
			),
		]
		.into_iter()
		.zip(before)
		{
			if address != before {
				token.clear();
				*forget = false;
			}
		}
	}

	/// Whether the form, after its save over `before`, still holds just
	/// what it sent.
	fn sent(&self, before: &Stream) -> bool {
		let now = self
			.saved(before.saved())
			.ok()
			.map(|saved| (saved, self.tokens(before)));
		now.is_some() && now == self.tried
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

	/// An edit saves once, and one the save refused waits for the next edit
	/// rather than being tried every frame. After a save, a form holding only
	/// what it sent settles, even when the save moved the address's token
	/// into the credential store, which would otherwise read as an edit and
	/// save again forever; one edited since keeps its edit, even one undone
	/// back to what the file held before.
	#[test]
	fn an_edit_saves_once_and_settles() {
		let dir = tempfile::tempdir().unwrap();
		let mut stream = Stream::load(
			&dir.path().join("show.toml"),
			Default::default(),
			Default::default(),
			Default::default(),
			crate::stream::Credentials::memory(),
		)
		.unwrap();
		let mut form = Form::new(&stream);
		assert!(form.next(&stream).is_none(), "nothing to save");

		form.relay = "not a url".into();
		assert!(
			form.next(&stream).is_none(),
			"an address that is not one is never saved"
		);

		form.relay = "https://relay.example/anon?jwt=secret".into();
		let (saved, tokens) = form.next(&stream).expect("the edit saves");
		assert!(form.next(&stream).is_none(), "and is not tried again as it is");
		let before = stream.clone();
		stream.save(saved, &tokens).unwrap();
		assert_ne!(
			stream.relay().unwrap().as_str(),
			form.relay,
			"the token moved out of the address"
		);
		assert!(form.sent(&before), "what was saved settles");

		let mut form = Form::new(&before);
		form.on_demand = true;
		form.next(&before).unwrap();
		form.on_demand = false;
		assert!(
			!form.sent(&before),
			"an edit made during the save is kept, back to what it was too"
		);
	}

	/// An edit that cannot save, or one refused as it is, is stuck until it
	/// changes; one not tried yet is not.
	#[test]
	fn an_edit_that_cannot_save_is_stuck() {
		let dir = tempfile::tempdir().unwrap();
		let stream = Stream::load(
			&dir.path().join("show.toml"),
			Default::default(),
			Default::default(),
			Default::default(),
			crate::stream::Credentials::memory(),
		)
		.unwrap();
		let mut form = Form::new(&stream);
		assert!(!form.stuck(&stream), "nothing to save");
		form.relay = "not a url".into();
		assert!(form.stuck(&stream), "an address that is not one");
		form.relay = "https://relay.example/".into();
		assert!(!form.stuck(&stream), "an edit not tried yet");
		form.next(&stream).expect("the edit saves");
		assert!(form.stuck(&stream), "tried as it is, and not landed");
		form.broadcast = "other".into();
		assert!(!form.stuck(&stream), "until it changes");
	}

	/// A token typed for one relay never goes to an address edited in
	/// after it, even while a save still sends it to the first.
	#[test]
	fn a_token_stays_with_the_relay_it_was_typed_for() {
		let dir = tempfile::tempdir().unwrap();
		let stream = Stream::load(
			&dir.path().join("show.toml"),
			Default::default(),
			Default::default(),
			Default::default(),
			crate::stream::Credentials::memory(),
		)
		.unwrap();
		let mut form = Form::new(&stream);
		form.relay = "https://a.example/".into();
		form.readdressed(["", ""]);
		form.relay_token = "secret".into();
		form.failover = "https://c.example/".into();
		form.readdressed(["https://a.example/", ""]);
		form.failover_token = "other".into();
		form.forget_relay_token = true;
		form.readdressed(["https://a.example/", "https://c.example/"]);
		let (_, tokens) = form.next(&stream).expect("the edit saves");
		assert_eq!(
			tokens,
			[
				("https://a.example/".parse().unwrap(), Some("secret".into())),
				("https://c.example/".parse().unwrap(), Some("other".into())),
			]
		);

		// While that save runs, or after it fails, the relay moves to B.
		form.relay = "https://b.example/".into();
		form.readdressed(["https://a.example/", "https://c.example/"]);
		assert!(form.relay_token.is_empty(), "A's token never goes to B");
		assert!(!form.forget_relay_token, "nor does forgetting A's");
		let (saved, tokens) = form.next(&stream).expect("the new address saves");
		assert_eq!(saved.relay.unwrap().as_str(), "https://b.example/");
		assert_eq!(
			tokens,
			[("https://c.example/".parse().unwrap(), Some("other".into()))],
			"with only the failover's token, its address untouched"
		);
	}
}
