//! Stream health: how the program actually reaches viewers, track by track,
//! and in plain words why it does not when it does not.
//!
//! Every [`INTERVAL`] the studio grades its stream with `hang::health`, the
//! grader the rest of the stack shares, from three points of view:
//! - its own: what each track of the broadcast wrote, each relay connection's
//!   state, and the serving one's round trip, estimated capacity, and loss;
//! - its viewers': the feedback each publishes under the broadcast's `echo`
//!   path, which the catalog solicits (`moq export --echo` answers it);
//! - the relay's: how many sessions watch the broadcast, read from the relay's
//!   stats broadcasts when it shares them with this connection.
//!
//! It adds what only the studio knows (program frames the encoder had no room
//! for, sources that stopped delivering) and names the cause of each problem.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use hang::health::Verdict;
use hang::health::{self, Health, Input, Observer, Thresholds};
use kio::clock::Instant;
use moq_net::bandwidth::Rate;
use moq_scene::source::{Stall, Status};

/// How often the health is graded: as often as reporters write.
pub const INTERVAL: Duration = Duration::from_secs(1);

/// What a stalled source has gone without, such as "no new picture for 3 s".
pub fn without(stall: Stall) -> String {
	let what = match stall {
		Stall::Picture(_) => "picture",
		Stall::Sound(_) => "sound",
	};
	format!("no new {what} for {:.0} s", stall.since().as_secs_f64())
}

/// The prefix, under the broadcast's own path, where viewers publish feedback.
pub(crate) const ECHO: &str = "viewers";

/// The most of one viewer's report the studio holds. A report covers the
/// broadcast's few tracks in well under a KiB, so this only stops a viewer
/// whose patches keep adding keys.
const MAX_REPORT: usize = 64 * 1024;

/// The most viewers whose feedback the studio reads at once. Their grades are
/// a sample, which a few dozen viewers show as well as thousands would, while
/// the readers, their subscriptions, and their reports stay a fixed cost
/// however many viewers announce feedback.
const MAX_VIEWERS: usize = 64;

/// What the Health panel draws, rebuilt every [`INTERVAL`].
#[derive(Debug, Default)]
pub struct Report {
	/// Each relay the broadcast goes to, the preferred first; empty when the
	/// studio publishes nothing.
	pub relays: Vec<Relay>,
	pub uplink: Uplink,
	pub viewers: Viewers,
	/// Each track of the broadcast in catalog order, then each source's own
	/// local track by source name.
	pub tracks: Vec<Track>,
	/// What is wrong, worst first, each naming its cause.
	pub problems: Vec<Problem>,
}

/// One relay connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Relay {
	/// Its scheme, host, and port.
	pub endpoint: String,
	pub state: State,
	/// The protocol its session negotiated, while connected.
	pub version: Option<moq_net::Version>,
}

/// A relay connection's state.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
	/// No session has connected yet.
	Connecting,
	Connected,
	/// The connection is redialing, with why the last session ended.
	Disconnected(String),
}

/// The link to the first connected relay, graded as the publisher's: viewers
/// who rank the relays as the studio does reach the stream through it.
#[derive(Debug, Default)]
pub struct Uplink {
	/// The relay it reaches, `None` while none is connected.
	pub relay: Option<String>,
	/// The worst of the grades below.
	pub verdict: Verdict,
	pub rtt: Option<Duration>,
	/// What the congestion controller estimates the connection can carry.
	pub capacity: Option<Rate>,
	/// The media the program's tracks wrote over the last interval. Source
	/// tracks are left out: they stay in the studio, and never reach a relay.
	pub media: Option<Rate>,
	/// Transport bytes lost per media byte written over the last interval.
	pub loss: Option<f64>,
	/// `hang::health`'s grade of each input. Loss and headroom are unknown
	/// while nobody watches: the relay pulls nothing then, so an idle link's
	/// estimate and loss say nothing about the stream.
	grades: Vec<(Input, Verdict)>,
}

impl Uplink {
	fn grade(&self, input: Input) -> Verdict {
		self.grades
			.iter()
			.find(|(graded, _)| *graded == input)
			.map_or(Verdict::Unknown, |(_, verdict)| *verdict)
	}
}

/// Who watches.
#[derive(Debug, Default)]
pub struct Viewers {
	/// Sessions subscribed to the broadcast, summed over the relay's nodes, or
	/// `None` when the relay shares no stats with this connection.
	pub watching: Option<u64>,
	/// The viewers whose feedback is read, by name: a sample of at most
	/// [`MAX_VIEWERS`].
	pub reports: Vec<Viewer>,
	/// Viewers reporting beyond the sample, whose feedback is not read.
	pub unread: usize,
}

/// One viewer's feedback.
#[derive(Debug)]
pub struct Viewer {
	/// The name it publishes feedback under.
	pub name: String,
	pub verdict: Verdict,
	/// Its round trip to its relay.
	pub rtt: Option<Duration>,
	/// The longest any track took from the program clock to this viewer.
	pub latency: Option<Duration>,
	/// The longest playout delay it reports.
	pub playout: Option<Duration>,
	/// Why its report cannot be read, if it cannot.
	pub error: Option<String>,
}

/// One track of the broadcast, or a source's own local track.
#[derive(Debug)]
pub struct Track {
	/// The source whose own track it is, or `None` for the program's.
	pub source: Option<String>,
	/// Its catalog rendition ID.
	pub id: String,
	/// What it carries, such as `video H.264 1920x1080`.
	pub label: String,
	/// Its share of the connection's bandwidth, which a program track follows:
	/// `None` for a source's track, or while the connection has no estimate.
	pub granted: Option<Rate>,
	/// Whether the catalog advertises it as encoding: not a rendition the
	/// grant cannot fund.
	pub enabled: bool,
	/// The bitrate it wrote over the last interval.
	pub sent: Option<Rate>,
	/// Frames per second it wrote over the last interval.
	pub framerate: Option<f64>,
	/// Viewers reporting this track.
	pub viewers: usize,
	/// The lowest bitrate any of them received over its last interval.
	pub received: Option<Rate>,
	/// The worst grade any of them gave it.
	pub verdict: Verdict,
	/// The longest it took any of them from the program clock to arrival.
	pub latency: Option<Duration>,
}

impl Track {
	/// How a problem names it.
	fn name(&self) -> String {
		match &self.source {
			Some(source) => format!("source `{source}`'s {}", self.label),
			None => self.label.clone(),
		}
	}
}

/// Something wrong with the stream, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
	/// Degraded or unhealthy.
	pub verdict: Verdict,
	pub cause: String,
}

/// What only the studio knows, read each interval: cumulative counters and
/// the state of its sources.
#[derive(Debug, Default, Clone)]
pub struct Local {
	/// Canvas frames drawn.
	pub rendered: u64,
	/// Program frames the encoder had no room for.
	pub dropped: u64,
	/// Program frames encoded.
	pub encoded: u64,
	/// Whether the program's canvas size should be encoding: always, or while
	/// watched when on demand, and while its grant funds it.
	pub encoding: bool,
	/// Each program track's share of the connection's bandwidth, by name.
	pub grants: BTreeMap<String, Option<Rate>>,
	/// Whether anyone subscribes to the program video.
	pub watched: bool,
	/// The encoder backend, once one opened.
	pub encoder: Option<String>,
	/// Why the output stopped, if it did.
	pub failed: Option<String>,
	/// Sources the program shows or plays that stopped delivering, and what.
	pub stalled: Vec<(String, Stall)>,
	/// Sources the program shows or plays that ended or failed.
	pub stopped: Vec<(String, Status)>,
}

/// What the studio publishes, for grading its delivery.
pub struct Publisher {
	/// The broadcast's path and the instance it was announced with.
	pub observer: Observer,
	pub catalog: moq_scene::publish::Catalog,
	/// The catalog's clock, which maps a track's timestamps to the wall time
	/// the program drew them.
	pub clock: moq_mux::Clock,
	/// Each relay the broadcast goes to, the preferred first.
	pub relays: Vec<Link>,
	pub feedback: Feedback,
	pub watching: Watching,
	/// Where the sources keep their own tracks, for what those wrote.
	pub sources: moq_scene::publish::Publish,
}

/// The connection to one relay.
#[derive(Clone)]
pub struct Link {
	/// Its scheme, host, and port.
	pub endpoint: String,
	pub monitor: moq_tokio::connection::Monitor,
}

/// Grades the stream every interval.
pub struct Sampler {
	publisher: Option<Publisher>,
	thresholds: Thresholds,
	previous: Option<Previous>,
}

/// The last interval's readings, which the next one is measured against.
struct Previous {
	at: Instant,
	local: Local,
	report: Option<health::Report<hang::stats::Snapshot>>,
	sources: Option<health::Report<hang::stats::Snapshot>>,
	/// Which relay's link the report's transport readings came from.
	serving: Option<usize>,
}

impl Sampler {
	/// Grade a studio publishing as `publisher`, or only its local work without one.
	pub fn new(publisher: Option<Publisher>) -> Self {
		Self {
			publisher,
			thresholds: Thresholds::default(),
			previous: None,
		}
	}

	/// Grade the interval ending `now`, given what the studio reads now.
	pub fn sample(&mut self, now: Instant, local: Local) -> Report {
		let previous = self.previous.take();
		let interval = previous
			.as_ref()
			.map(|previous| now.saturating_duration_since(previous.at));

		let mut report = Report::default();
		let mut graded = Graded::default();
		let (mut current, mut sources) = (None, None);
		let mut serving = None;
		if let Some(publisher) = &self.publisher {
			report.relays = publisher.relays.iter().map(relay).collect();
			serving = report.relays.iter().position(|relay| relay.state == State::Connected);
			let link = serving.map(|index| &publisher.relays[index]);
			let ours = health::Report::new(
				publisher.observer.clone(),
				snapshot(publisher, link.map(|link| &link.monitor)),
			);
			// Another relay's link counted its transport from another start.
			let previous_ours = previous
				.as_ref()
				.filter(|previous| previous.serving == serving)
				.and_then(|previous| previous.report.as_ref());
			let graded_ours = self.grade(&ours, previous_ours.zip(interval));
			report.uplink = uplink(&ours.snapshot, &graded_ours, local.watched);
			report.uplink.relay = link.map(|link| link.endpoint.clone());
			graded.publisher = Some(graded_ours);
			// Graded apart from the program's, so their media stays out of the
			// uplink's; see [`Uplink::media`].
			let mut snapshot = hang::stats::Snapshot::default();
			snapshot.renditions = publisher.sources.stats();
			let theirs = health::Report::new(publisher.observer.clone(), snapshot);
			let previous_theirs = previous.as_ref().and_then(|previous| previous.sources.as_ref());
			graded.sources = Some(self.grade(&theirs, previous_theirs.zip(interval)));
			graded.viewers = publisher.feedback.grade(now, &self.thresholds);
			report.viewers.unread = publisher.feedback.unread();
			report.viewers.watching = publisher.watching.get();
			let listed = Listed {
				program: publisher.catalog.snapshot(),
				sources: publisher.sources.catalog(),
				grants: &local.grants,
			};
			report.tracks = tracks(&listed, &publisher.clock, &graded);
			report.viewers.reports = viewers(publisher, &graded.viewers);
			current = Some(ours);
			sources = Some(theirs);
		}

		let drops = previous.as_ref().map(|previous| Drops {
			drawn: local.rendered.saturating_sub(previous.local.rendered),
			dropped: local.dropped.saturating_sub(previous.local.dropped),
			encoded: local.encoded.saturating_sub(previous.local.encoded),
		});
		report.problems = problems(&Facts {
			relays: &report.relays,
			local: &local,
			drops,
			uplink: &report.uplink,
			viewers: &graded.viewers,
			labels: &report.tracks,
			thresholds: &self.thresholds,
		});

		self.previous = Some(Previous {
			at: now,
			local,
			report: current,
			sources,
			serving,
		});
		report
	}

	/// Grade what the studio wrote, against `previous` and the interval since.
	fn grade(
		&self,
		current: &health::Report<hang::stats::Snapshot>,
		previous: Option<(&health::Report<hang::stats::Snapshot>, Duration)>,
	) -> Health {
		let mut sample = health::Sample::new(current, Duration::ZERO);
		if let Some((previous, interval)) = previous {
			sample = sample.with_previous(previous, interval);
		}
		health::publisher(&sample, &self.thresholds)
	}
}

fn relay(link: &Link) -> Relay {
	use moq_tokio::connection::Phase;
	let observed = link.monitor.state();
	let state = match observed.phase {
		Phase::Connecting => State::Connecting,
		Phase::Connected | Phase::Migrating => State::Connected,
		Phase::Disconnected | Phase::Closed => State::Disconnected(
			observed
				.failure
				.map_or_else(|| "the connection closed".to_string(), |err| err.to_string()),
		),
	};
	Relay {
		endpoint: link.endpoint.clone(),
		state,
		version: observed.version,
	}
}

/// What the studio's tracks have written, and the transport of the link
/// `monitor` watches.
fn snapshot(publisher: &Publisher, monitor: Option<&moq_tokio::connection::Monitor>) -> hang::stats::Snapshot {
	let mut snapshot = hang::stats::Snapshot::default();
	snapshot.renditions = publisher.catalog.stats();
	if let Some(stats) = monitor.and_then(moq_tokio::connection::Monitor::stats) {
		snapshot.transport.rtt = stats.rtt;
		snapshot.transport.rate = stats.estimated_send_rate.map(Rate::as_bps);
		snapshot.transport.lost_bytes = stats.bytes_lost;
		snapshot.transport.lost_packets = stats.packets_lost;
	}
	snapshot
}

/// The connection's readings and grades; see [`Uplink::grades`].
fn uplink(snapshot: &hang::stats::Snapshot, graded: &Health, watched: bool) -> Uplink {
	let evidence = &graded.transport.evidence;
	let grades: Vec<(Input, Verdict)> = evidence
		.iter()
		.map(|evidence| {
			let idle = !watched && evidence.input != Input::Rtt;
			(evidence.input, if idle { Verdict::Unknown } else { evidence.verdict })
		})
		.collect();
	let media = graded.interval.map(|_| {
		graded
			.renditions
			.values()
			.filter_map(|rendition| rendition.bitrate)
			.sum()
	});
	Uplink {
		relay: None,
		verdict: grades
			.iter()
			.map(|(_, verdict)| *verdict)
			.fold(Verdict::Unknown, Verdict::max),
		rtt: snapshot.transport.rtt,
		capacity: snapshot.transport.rate.map(Rate::from_bps),
		media: media.map(Rate::from_bps),
		loss: evidence
			.iter()
			.find(|evidence| evidence.input == Input::Loss)
			.and_then(|evidence| evidence.value),
		grades,
	}
}

/// What the studio's catalogs list now.
struct Listed<'a> {
	/// The program's broadcast.
	program: hang::Catalog<moq_scene::publish::Ext>,
	/// The sources' local one, nesting each source's catalog.
	sources: hang::Catalog<moq_scene::publish::Ext>,
	/// Each program track's share of the connection, by name.
	grants: &'a BTreeMap<String, Option<Rate>>,
}

/// Each media track `listed`, the program's and then each source's, with
/// what it sent and what its viewers received. A source's stills carry no
/// media to grade, so they are left out.
fn tracks(listed: &Listed, clock: &moq_mux::Clock, graded: &Graded) -> Vec<Track> {
	let program =
		renditions(&listed.program).map(|(id, label, enabled)| (None, id, label, enabled, graded.publisher.as_ref()));
	let sources = listed.sources.ext.studio.sources.iter().flat_map(|(name, catalog)| {
		renditions(catalog).map(move |(id, label, enabled)| (Some(name), id, label, enabled, graded.sources.as_ref()))
	});
	program
		.chain(sources)
		.map(|(source, id, label, enabled, sent)| {
			let ours = sent.and_then(|health| health.renditions.get(id));
			let interval = sent.and_then(|health| health.interval);
			// A source's track is in the local broadcast, whose names can repeat
			// the program's, and no relay viewer or grant reaches it.
			let program = source.is_none();
			let mut track = Track {
				source: source.cloned(),
				id: id.clone(),
				label,
				granted: listed.grants.get(id).copied().flatten().filter(|_| program),
				enabled,
				sent: ours.and_then(|rendition| rendition.bitrate).map(Rate::from_bps),
				framerate: ours
					.and_then(|rendition| rendition.frames)
					.zip(interval)
					.map(|(frames, interval)| frames as f64 / interval.as_secs_f64()),
				viewers: 0,
				received: None,
				verdict: Verdict::Unknown,
				latency: None,
			};
			for viewer in graded.viewers.iter().filter(|_| program) {
				let Some(health) = &viewer.health else { continue };
				let Some(rendition) = health.renditions.get(id) else {
					continue;
				};
				track.viewers += 1;
				track.verdict = track.verdict.max(rendition.verdict);
				if let Some(bitrate) = rendition.bitrate.map(Rate::from_bps) {
					track.received = Some(track.received.map_or(bitrate, |received| received.min(bitrate)));
				}
				let latency = viewer.latency(id, clock);
				track.latency = track.latency.max(latency);
			}
			track
		})
		.collect()
}

/// Each video and audio rendition `catalog` lists, by ID, with what it
/// carries and whether it is enabled.
fn renditions<E>(catalog: &hang::Catalog<E>) -> impl Iterator<Item = (&String, String, bool)> {
	let video = catalog.video.renditions.iter().map(|(id, config)| {
		let size = match (config.coded_width, config.coded_height) {
			(Some(width), Some(height)) => format!(" {width}x{height}"),
			_ => String::new(),
		};
		(id, format!("video {}{size}", config.codec), config.enabled)
	});
	let audio = catalog.audio.renditions.iter().map(|(id, config)| {
		let label = format!("audio {} {} ch", config.codec, config.channel_count);
		(id, label, config.enabled)
	});
	video.chain(audio)
}

fn viewers(publisher: &Publisher, graded: &[GradedViewer]) -> Vec<Viewer> {
	graded
		.iter()
		.map(|viewer| {
			let current = viewer.current.as_ref().map(|report| &report.snapshot);
			Viewer {
				name: viewer.name.clone(),
				verdict: viewer.health.as_ref().map_or(Verdict::Unknown, |health| health.verdict),
				rtt: current.and_then(|snapshot| snapshot.transport.rtt),
				latency: current.and_then(|snapshot| {
					snapshot
						.renditions
						.keys()
						.filter_map(|id| viewer.latency(id, &publisher.clock))
						.max()
				}),
				playout: current.and_then(|snapshot| {
					snapshot
						.renditions
						.values()
						.filter_map(|rendition| rendition.latency)
						.max()
				}),
				error: viewer.error.clone(),
			}
		})
		.collect()
}

/// The grades of one interval.
#[derive(Default)]
struct Graded {
	/// The program's tracks and the uplink.
	publisher: Option<Health>,
	/// The sources' own tracks.
	sources: Option<Health>,
	viewers: Vec<GradedViewer>,
}

/// One viewer's latest report and its grade.
struct GradedViewer {
	name: String,
	current: Option<health::Report<hang::echo::Snapshot>>,
	health: Option<Health>,
	error: Option<String>,
}

impl GradedViewer {
	/// How long rendition `id`'s newest frame took from the program clock to
	/// this viewer, by the two hosts' wall clocks. `None` when it reports no
	/// arrival, or arrived before it was drawn, as a viewer whose clock runs
	/// behind can claim.
	fn latency(&self, id: &str, clock: &moq_mux::Clock) -> Option<Duration> {
		let arrival = self.current.as_ref()?.snapshot.renditions.get(id)?.arrival.as_ref()?;
		let drawn = clock.wall_clock(arrival.timestamp).ok()?;
		arrival.wall.checked_duration_since(drawn)
	}
}

/// Program frames over one interval.
#[derive(Debug, Clone, Copy)]
struct Drops {
	drawn: u64,
	dropped: u64,
	encoded: u64,
}

/// Everything a problem can rest on.
struct Facts<'a> {
	relays: &'a [Relay],
	local: &'a Local,
	drops: Option<Drops>,
	uplink: &'a Uplink,
	viewers: &'a [GradedViewer],
	/// The tracks, for naming one a viewer reports on.
	labels: &'a [Track],
	thresholds: &'a Thresholds,
}

/// Name each problem the facts show, worst first.
fn problems(facts: &Facts<'_>) -> Vec<Problem> {
	let mut problems = Vec::new();
	let mut add = |verdict: Verdict, cause: String| {
		if verdict >= Verdict::Degraded {
			problems.push(Problem { verdict, cause });
		}
	};
	let local = facts.local;

	// While another relay serves, one lost is a lost fallback rather than an
	// outage.
	let serving = facts.relays.iter().find(|relay| relay.state == State::Connected);
	for relay in facts.relays {
		let endpoint = &relay.endpoint;
		match (&relay.state, serving) {
			(State::Disconnected(reason), Some(serving)) => add(
				Verdict::Degraded,
				format!(
					"The relay {endpoint} is unreachable ({reason}), so the stream reaches viewers only through {}.",
					serving.endpoint
				),
			),
			(State::Disconnected(reason), None) => add(
				Verdict::Unhealthy,
				format!(
					"The relay {endpoint} is unreachable ({reason}), so nothing reaches viewers until a relay reconnects."
				),
			),
			(State::Connecting, _) => add(Verdict::Degraded, format!("Connecting to the relay {endpoint}.")),
			(State::Connected, _) => {}
		}
	}
	if let Some(failed) = &local.failed {
		add(Verdict::Unhealthy, format!("The output stopped: {failed}."));
	}

	for (name, stall) in &local.stalled {
		let instead = match stall {
			Stall::Picture(_) => "repeats its last one",
			Stall::Sound(_) => "plays silence for it",
		};
		add(
			Verdict::Unhealthy,
			format!(
				"Source `{name}` stalled: {}, so the program {instead}.",
				without(*stall)
			),
		);
	}
	for (name, status) in &local.stopped {
		match status {
			Status::Ended => add(
				Verdict::Degraded,
				format!(
					"Source `{name}` played to its end, so the program holds its last picture and plays no sound for it."
				),
			),
			Status::Failed(reason) => add(Verdict::Unhealthy, format!("Source `{name}` failed: {reason}.")),
			_ => {}
		}
	}

	if let Some(drops) = facts.drops
		&& local.encoding
		&& local.failed.is_none()
	{
		let encoder = local.encoder.as_deref().unwrap_or("the encoder");
		let share = ratio(drops.dropped, drops.drawn);
		let limits = &facts.thresholds.publisher.skipped;
		if drops.drawn > 0 && drops.encoded == 0 && drops.dropped == 0 && local.encoder.is_some() {
			add(
				Verdict::Unhealthy,
				format!("The encoder is stuck: {encoder} produced nothing in the last second."),
			);
		} else if let Some(share) = share {
			add(
				grade_above(share, limits.degraded, limits.unhealthy),
				format!(
					"The encoder is behind: {encoder} had no room for {} of {} program frames in the last second.",
					drops.dropped, drops.drawn
				),
			);
		}
	}

	let uplink = facts.uplink;
	let relay = uplink
		.relay
		.as_ref()
		.map_or_else(String::new, |relay| format!(" {relay}"));
	if let (Some(capacity), Some(media)) = (uplink.capacity, uplink.media) {
		add(
			uplink.grade(Input::Headroom),
			format!(
				"The uplink to the relay{relay} is congested: the connection carries about {} and the stream needs {}.",
				rate(capacity),
				rate(media)
			),
		);
	}
	if let Some(loss) = uplink.loss {
		add(
			uplink.grade(Input::Loss),
			format!(
				"The uplink to the relay{relay} is losing packets: {:.1}% of the media sent had to be resent.",
				loss * 100.0
			),
		);
	}
	if let Some(rtt) = uplink.rtt {
		add(
			uplink.grade(Input::Rtt),
			format!(
				"The relay{relay} is far away: a round trip to it takes {} ms.",
				rtt.as_millis()
			),
		);
	}

	viewer_problems(facts, &mut add);

	problems.sort_by_key(|problem| std::cmp::Reverse(problem.verdict));
	problems
}

/// What viewers' reports show, one problem per kind of trouble across them.
fn viewer_problems(facts: &Facts<'_>, add: &mut impl FnMut(Verdict, String)) {
	let reporting = facts.viewers.iter().filter(|viewer| viewer.health.is_some()).count();
	// A congested or lossy uplink starves every viewer, and one measured
	// clear of both leaves the blame with the viewers' own links. With either
	// unmeasured, as headroom is under a controller without an estimate, the
	// cause goes unnamed.
	let uplink = facts.uplink;
	let (headroom, loss) = (uplink.grade(Input::Headroom), uplink.grade(Input::Loss));
	let cause = if headroom >= Verdict::Degraded || loss >= Verdict::Degraded {
		": the uplink cannot keep up"
	} else if headroom == Verdict::Healthy && loss == Verdict::Healthy {
		": their own connections cannot keep up"
	} else {
		""
	};

	// Each input's worst grade, how many viewers it troubles, and the track
	// the worst grade is on.
	let mut worst: Vec<(Input, Verdict, usize, Option<&str>)> = Vec::new();
	for viewer in facts.viewers {
		let Some(health) = &viewer.health else { continue };
		let transport = health.transport.evidence.iter().map(|evidence| (evidence, None));
		let renditions = health.renditions.iter().flat_map(|(id, rendition)| {
			rendition
				.evidence
				.iter()
				.map(move |evidence| (evidence, Some(id.as_str())))
		});
		let mut counted = Vec::new();
		for (evidence, id) in transport.chain(renditions) {
			if evidence.verdict < Verdict::Degraded {
				continue;
			}
			let index = match worst.iter().position(|entry| entry.0 == evidence.input) {
				Some(index) => index,
				None => {
					worst.push((evidence.input, Verdict::Unknown, 0, None));
					worst.len() - 1
				}
			};
			let entry = &mut worst[index];
			if !counted.contains(&evidence.input) {
				counted.push(evidence.input);
				entry.2 += 1;
			}
			if evidence.verdict > entry.1 {
				entry.1 = evidence.verdict;
				entry.3 = id;
			}
		}
	}

	let track = |id: Option<&str>| {
		id.and_then(|id| facts.labels.iter().find(|track| track.id == id))
			.map_or_else(|| "the stream".to_string(), Track::name)
	};
	for (input, verdict, count, id) in worst {
		let of = format!("{count} of {reporting} viewers");
		let cause = match input {
			Input::Late => format!("{of} receive {} too late to play{cause}.", track(id)),
			Input::Stalled => format!("{of} are stalling on {}{cause}.", track(id)),
			Input::Errors => format!("{of} cannot decode {}.", track(id)),
			Input::Rtt => format!("{of} are far from their relay."),
			// Graded for publishers only.
			Input::Loss | Input::Headroom | Input::Skipped => continue,
		};
		add(verdict, cause);
	}
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
	(denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn grade_above(value: f64, degraded: f64, unhealthy: f64) -> Verdict {
	if value >= unhealthy {
		Verdict::Unhealthy
	} else if value >= degraded {
		Verdict::Degraded
	} else {
		Verdict::Healthy
	}
}

/// A rate as people read it.
pub fn rate(rate: Rate) -> String {
	let bps = rate.as_bps() as f64;
	if bps >= 1e6 {
		format!("{:.1} Mb/s", bps / 1e6)
	} else {
		format!("{:.0} kb/s", bps / 1e3)
	}
}

/// The prefix viewers publish feedback under, relative to the broadcast at
/// `path` as the catalog's `echo` section spells it, and resolved.
pub fn echo(path: &moq_net::Path) -> anyhow::Result<(moq_net::path::Relative, moq_net::Path)> {
	// A reference resolves against the broadcast's parent, as a relative URL
	// does, so naming the broadcast's last segment puts the prefix beneath it.
	let relative = match path.parts().last() {
		Some(last) => format!("{last}/{ECHO}"),
		None => ECHO.to_string(),
	};
	let relative = moq_net::path::Relative::new(&relative)?;
	let resolved = path
		.try_resolve(&relative)
		.ok_or_else(|| anyhow::anyhow!("the feedback prefix of `{path}` escapes the root"))?;
	Ok((relative, resolved))
}

/// The feedback of a sample of viewers, kept by the readers
/// [`Feedback::follow`] runs.
#[derive(Clone, Default)]
pub struct Feedback(Arc<Mutex<Sample>>);

/// The viewers being read, and how many more wait for a reader.
#[derive(Default)]
struct Sample {
	slots: BTreeMap<String, Slot>,
	unread: usize,
}

/// Where one reader keeps its viewer's reports. A reader aborted mid-write
/// writes to a slot nobody reads any more, rather than bringing back a viewer
/// that left.
type Slot = Arc<Mutex<Reports>>;

/// One viewer's two latest reports, each with when it arrived.
#[derive(Default)]
struct Reports {
	previous: Option<(health::Report<hang::echo::Snapshot>, Instant)>,
	current: Option<(health::Report<hang::echo::Snapshot>, Instant)>,
	error: Option<String>,
}

impl Feedback {
	/// Read the feedback of viewers under `prefix` on `origin`, as viewers
	/// come and go, until dropped: at most [`MAX_VIEWERS`] at once. A viewer
	/// that arrives while that many are read is only counted, and a reader
	/// that frees up goes to the next viewer to arrive, so the studio holds a
	/// fixed number of readers and a count however many viewers announce.
	///
	/// Who may publish feedback is the relay's authorization, as for any
	/// broadcast, and a read viewer's report is held to [`MAX_REPORT`].
	pub async fn follow(self, origin: moq_net::origin::Consumer, prefix: moq_net::Path) -> anyhow::Result<()> {
		let pattern = moq_net::Pattern::subtree(prefix.as_str())?;
		let origin = origin.scope("", &moq_net::Patterns::from(pattern))?;
		let mut announced = origin.announced();
		let mut readers = BTreeMap::<moq_net::Path, Reader>::new();
		let mut unread = 0usize;
		while let Some(event) = announced.next().await {
			let (announce, restart) = match event {
				moq_net::announce::Event::Start(announce) => (announce, false),
				moq_net::announce::Event::Restart(announce) => (announce, true),
				moq_net::announce::Event::End(announce) => {
					if readers.remove(&announce.prefix).is_none() && hang::echo::is_echo(&announce.prefix) {
						unread = unread.saturating_sub(1);
					}
					self.0.lock().unwrap().unread = unread;
					continue;
				}
				// A takeover continues the same instance, and an update moves its route.
				_ => continue,
			};
			let path = announce.prefix;
			let Some(name) = name(&prefix, &path).filter(|_| hang::echo::is_echo(&path)) else {
				continue;
			};
			// A new instance of a viewer being read replaces its reader; one not
			// read was counted when it started.
			let read = if restart {
				readers.remove(&path).is_some()
			} else if readers.len() < MAX_VIEWERS {
				true
			} else {
				unread += 1;
				false
			};
			if read {
				let reader = self.read(
					origin.clone(),
					Observer::new(path.clone(), announce.route.instance),
					name,
				);
				readers.insert(path, reader);
			}
			self.0.lock().unwrap().unread = unread;
		}
		Ok(())
	}

	/// Start reading the viewer `observer` names.
	fn read(&self, origin: moq_net::origin::Consumer, observer: Observer, name: String) -> Reader {
		let slot = Slot::default();
		self.0.lock().unwrap().slots.insert(name.clone(), slot.clone());
		let task = tokio::spawn(read(origin, observer, slot.clone()));
		Reader {
			name,
			slot,
			task,
			feedback: self.clone(),
		}
	}

	fn unread(&self) -> usize {
		self.0.lock().unwrap().unread
	}

	/// Grade every viewer's latest report as of `now`.
	fn grade(&self, now: Instant, thresholds: &Thresholds) -> Vec<GradedViewer> {
		let sample = self.0.lock().unwrap();
		sample
			.slots
			.iter()
			.map(|(name, slot)| {
				let reports = slot.lock().unwrap();
				let health = reports.current.as_ref().map(|(current, at)| {
					let mut sample = health::Sample::new(current, now.saturating_duration_since(*at));
					if let Some((previous, then)) = &reports.previous {
						sample = sample.with_previous(previous, at.saturating_duration_since(*then));
					}
					health::viewer(&sample, thresholds)
				});
				GradedViewer {
					name: name.clone(),
					current: reports.current.as_ref().map(|(current, _)| current.clone()),
					health,
					error: reports.error.clone(),
				}
			})
			.collect()
	}
}

/// The name a viewer publishes feedback under: its path beneath `prefix`,
/// without the `.echo` suffix.
fn name(prefix: &moq_net::Path, path: &moq_net::Path) -> Option<String> {
	let name = path.strip_prefix(prefix)?;
	name.as_str().strip_suffix(hang::echo::SUFFIX).map(str::to_string)
}

/// Keep the latest reports of the viewer `observer` names in `slot`, until
/// its broadcast ends or the report cannot be read.
async fn read(origin: moq_net::origin::Consumer, observer: Observer, slot: Slot) {
	let read = async {
		let broadcast = origin.request(&observer.path, observer.instance.clone()).await?;
		let track = broadcast.track(hang::echo::TRACK)?.subscribe(None).await?;
		let mut config = moq_json::snapshot::consumer::Config::default();
		config.max_size = Some(MAX_REPORT);
		let mut reports = moq_json::snapshot::Consumer::<hang::echo::Snapshot>::new(track, config);
		while let Some(report) = reports.latest().await? {
			let report = health::Report::new(observer.clone(), report.value);
			let mut reports = slot.lock().unwrap();
			reports.previous = reports.current.take();
			reports.current = Some((report, kio::clock::now()));
		}
		anyhow::Ok(())
	};
	if let Err(err) = read.await {
		tracing::debug!(%err, viewer = %observer.path, "stopped reading feedback");
		slot.lock().unwrap().error = Some(format!("{err:#}"));
	}
}

/// A running feedback reader, which forgets its viewer when it stops.
struct Reader {
	name: String,
	slot: Slot,
	task: tokio::task::JoinHandle<()>,
	feedback: Feedback,
}

impl Drop for Reader {
	fn drop(&mut self) {
		self.task.abort();
		let slots = &mut self.feedback.0.lock().unwrap().slots;
		if slots.get(&self.name).is_some_and(|slot| Arc::ptr_eq(slot, &self.slot)) {
			slots.remove(&self.name);
		}
	}
}

/// How many sessions watch the broadcast, by the relay's own stats.
#[derive(Clone, Default)]
pub struct Watching(Arc<Mutex<Option<u64>>>);

impl Watching {
	fn get(&self) -> Option<u64> {
		*self.0.lock().unwrap()
	}

	/// Follow the broadcast at `path` in the stats every relay node publishes
	/// under `.stats` on `origin`, until dropped. Nothing is counted when the
	/// relay shares no stats with this connection.
	pub async fn follow(self, origin: moq_net::origin::Consumer, path: moq_net::Path) -> anyhow::Result<()> {
		let config = moq_stats::aggregate::Config::new();
		let pattern = moq_net::Pattern::subtree(config.prefix.as_str())?;
		let origin = origin.scope("", &moq_net::Patterns::from(pattern))?;
		let mut prefix = moq_stats::aggregate::Consumer::new(origin, config).prefix(path);
		while let Some(frame) = prefix.next().await? {
			*self.0.lock().unwrap() = Some(watching(&frame));
		}
		Ok(())
	}
}

/// Sessions subscribed to exactly the prefix's broadcast, over every tier.
fn watching(frame: &moq_stats::PrefixFrame) -> u64 {
	frame
		.values()
		.map(|tier| {
			let egress = &tier.own.publisher;
			egress.broadcasts_started.saturating_sub(egress.broadcasts_ended)
		})
		.sum()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn facts<'a>(local: &'a Local, uplink: &'a Uplink, viewers: &'a [GradedViewer], tracks: &'a [Track]) -> Facts<'a> {
		static THRESHOLDS: std::sync::LazyLock<Thresholds> = std::sync::LazyLock::new(Thresholds::default);
		Facts {
			relays: &[],
			local,
			drops: None,
			uplink,
			viewers,
			labels: tracks,
			thresholds: &THRESHOLDS,
		}
	}

	fn causes(problems: Vec<Problem>) -> Vec<String> {
		problems.into_iter().map(|problem| problem.cause).collect()
	}

	/// A viewer that spent `stalled` of the last second stalled on `id`.
	fn stalling(name: &str, id: &str, stalled: Duration) -> GradedViewer {
		let observer = Observer::new(moq_net::Path::new(&format!("show/viewers/{name}.echo")).unwrap(), None);
		let report = |frames: u64, stalled: Duration| {
			let mut snapshot = hang::echo::Snapshot::default();
			let mut rendition = hang::echo::Rendition::default();
			rendition.frames = frames;
			rendition.stalled = stalled;
			snapshot.renditions.insert(id.to_string(), rendition);
			health::Report::new(observer.clone(), snapshot)
		};
		let (previous, current) = (report(30, Duration::ZERO), report(60, stalled));
		let sample = health::Sample::new(&current, Duration::ZERO).with_previous(&previous, INTERVAL);
		let health = health::viewer(&sample, &Thresholds::default());
		GradedViewer {
			name: name.into(),
			current: Some(current),
			health: Some(health),
			error: None,
		}
	}

	fn track(id: &str, label: &str) -> Track {
		Track {
			source: None,
			id: id.into(),
			label: label.into(),
			granted: None,
			enabled: true,
			sent: None,
			framerate: None,
			viewers: 0,
			received: None,
			verdict: Verdict::Unknown,
			latency: None,
		}
	}

	/// An uplink that cannot carry the stream is named, with both rates, but
	/// only while the relay pulls the stream: an idle link's estimate says
	/// nothing about it.
	#[test]
	fn a_congested_uplink_names_both_rates() {
		let observer = Observer::new(moq_net::Path::new("show").unwrap(), None);
		let report = |bytes: u64| {
			let mut snapshot = hang::stats::Snapshot::default();
			snapshot.transport.rate = Some(2_500_000);
			let mut rendition = hang::stats::Rendition::default();
			rendition.bytes = bytes;
			snapshot.renditions.insert("video0".into(), rendition);
			health::Report::new(observer.clone(), snapshot)
		};
		// 375 kB in a second is 3 Mb/s.
		let (previous, current) = (report(0), report(375_000));
		let sample = health::Sample::new(&current, Duration::ZERO).with_previous(&previous, INTERVAL);
		let graded = health::publisher(&sample, &Thresholds::default());

		let mut local = Local {
			watched: true,
			..Default::default()
		};
		let mut watched = uplink(&current.snapshot, &graded, true);
		watched.relay = Some("https://a".into());
		assert_eq!(watched.verdict, Verdict::Unhealthy);
		assert_eq!(
			causes(problems(&facts(&local, &watched, &[], &[]))),
			[
				"The uplink to the relay https://a is congested: the connection carries about 2.5 Mb/s and the stream needs 3.0 Mb/s."
			]
		);
		local.watched = false;
		let idle = uplink(&current.snapshot, &graded, false);
		assert_eq!(idle.verdict, Verdict::Unknown);
		assert!(problems(&facts(&local, &idle, &[], &[])).is_empty());
	}

	/// Program frames the encoder had no room for, and an encoder that
	/// produces nothing at all, both blame the encoder by name.
	#[test]
	fn the_encoder_falling_behind_is_named() {
		let local = Local {
			encoding: true,
			encoder: Some("nvenc".into()),
			..Default::default()
		};
		let uplink = Uplink::default();
		let mut facts = facts(&local, &uplink, &[], &[]);
		facts.drops = Some(Drops {
			drawn: 30,
			dropped: 3,
			encoded: 27,
		});
		let problems = problems(&facts);
		assert_eq!(problems[0].verdict, Verdict::Unhealthy);
		assert_eq!(
			problems[0].cause,
			"The encoder is behind: nvenc had no room for 3 of 30 program frames in the last second."
		);

		facts.drops = Some(Drops {
			drawn: 30,
			dropped: 0,
			encoded: 0,
		});
		assert_eq!(
			causes(super::problems(&facts)),
			["The encoder is stuck: nvenc produced nothing in the last second."]
		);

		facts.drops = Some(Drops {
			drawn: 30,
			dropped: 0,
			encoded: 30,
		});
		assert!(super::problems(&facts).is_empty());
	}

	/// Stalling viewers are blamed on their own links while the uplink is
	/// measured clear, on the uplink when it is congested or lossy, and on
	/// nobody while it is unmeasured, counting each viewer once. A slow round
	/// trip alone blames nobody.
	#[test]
	fn stalling_viewers_name_whose_link_fails() {
		let tracks = [track("video0", "video h264 1920x1080")];
		let viewers = [
			stalling("alice", "video0", Duration::from_millis(100)),
			stalling("bob", "video0", Duration::ZERO),
		];
		let local = Local::default();
		let stalling = |grades: Vec<(Input, Verdict)>| {
			let uplink = Uplink {
				grades,
				..Default::default()
			};
			causes(problems(&facts(&local, &uplink, &viewers, &tracks)))
		};
		let clear = vec![(Input::Headroom, Verdict::Healthy), (Input::Loss, Verdict::Healthy)];
		assert_eq!(
			stalling(clear.clone()),
			["1 of 2 viewers are stalling on video h264 1920x1080: their own connections cannot keep up."]
		);
		let lossy = vec![(Input::Headroom, Verdict::Healthy), (Input::Loss, Verdict::Degraded)];
		assert_eq!(
			stalling(lossy),
			["1 of 2 viewers are stalling on video h264 1920x1080: the uplink cannot keep up."]
		);
		let mut far = clear;
		far.push((Input::Rtt, Verdict::Degraded));
		assert!(stalling(far)[0].ends_with("their own connections cannot keep up."));
		assert_eq!(
			stalling(Vec::new()),
			["1 of 2 viewers are stalling on video h264 1920x1080."]
		);
		let unestimated = vec![(Input::Headroom, Verdict::Unknown), (Input::Loss, Verdict::Healthy)];
		assert_eq!(
			stalling(unestimated),
			["1 of 2 viewers are stalling on video h264 1920x1080."]
		);
	}

	/// What the studio wrote over one interval, graded: `bytes` and 30 frames
	/// for each rendition.
	fn wrote(renditions: &[(&str, u64)]) -> Health {
		let observer = Observer::new(moq_net::Path::new("show").unwrap(), None);
		let report = |scale: u64| {
			let mut snapshot = hang::stats::Snapshot::<()>::default();
			for (id, bytes) in renditions {
				let mut rendition = hang::stats::Rendition::default();
				rendition.bytes = bytes * scale;
				rendition.frames = 30 * scale;
				snapshot.renditions.insert(id.to_string(), rendition);
			}
			health::Report::new(observer.clone(), snapshot)
		};
		let (previous, current) = (report(0), report(1));
		let sample = health::Sample::new(&current, Duration::ZERO).with_previous(&previous, INTERVAL);
		health::publisher(&sample, &Thresholds::default())
	}

	/// Each source's media tracks are graded like the program's, after them and
	/// by source name, from what the source wrote; a still has no media to
	/// grade. A source's track stays in the studio, so no viewer report or
	/// grant applies to it, even under a name the program's track shares.
	#[test]
	fn source_tracks_are_graded_by_source() {
		let video = |width: u32, height: u32| {
			let codec: hang::catalog::VideoCodec = "avc3.42c01f".parse().unwrap();
			let mut config = hang::catalog::VideoConfig::new(codec);
			config.coded_width = Some(width);
			config.coded_height = Some(height);
			config
		};
		let mut program = hang::Catalog::<moq_scene::publish::Ext>::default();
		program.video.renditions.insert("0.avc3".into(), video(1920, 1080));
		let mut half = video(960, 540);
		half.enabled = false;
		program.video.renditions.insert("3.avc3".into(), half);
		let mut camera = hang::Catalog::default();
		camera.video.renditions.insert("0.avc3".into(), video(1280, 720));
		let mut logo = hang::Catalog::default();
		let png = hang::catalog::BinaryConfig::new(hang::catalog::Mode::Snapshot);
		logo.binary.tracks.insert("1.png".into(), png);
		let mut local = hang::Catalog::<moq_scene::publish::Ext>::default();
		let sources = &mut local.ext.studio.sources;
		sources.insert("logo".into(), logo);
		sources.insert("camera".into(), camera);
		let grants = [("0.avc3".to_string(), Some(Rate::from_mbps(4)))].into();
		let listed = Listed {
			program,
			sources: local,
			grants: &grants,
		};

		// 375 kB in a second is 3 Mb/s, and 125 kB is 1 Mb/s.
		let graded = Graded {
			publisher: Some(wrote(&[("0.avc3", 375_000)])),
			sources: Some(wrote(&[("0.avc3", 125_000)])),
			viewers: vec![stalling("alice", "0.avc3", Duration::from_millis(100))],
		};
		let tracks = tracks(&listed, &moq_mux::Clock::new(), &graded);
		let rows: Vec<_> = tracks
			.iter()
			.map(|track| {
				let granted = track.granted.map(rate);
				(
					track.source.as_deref(),
					track.id.as_str(),
					track.sent.map(rate),
					granted,
					track.enabled,
				)
			})
			.collect();
		assert_eq!(
			rows,
			[
				(None, "0.avc3", Some("3.0 Mb/s".into()), Some("4.0 Mb/s".into()), true),
				(None, "3.avc3", None, None, false),
				(Some("camera"), "0.avc3", Some("1.0 Mb/s".into()), None, true),
			]
		);
		let camera = &tracks[2];
		assert_eq!(camera.framerate, Some(30.0));
		assert_eq!((camera.viewers, camera.verdict), (0, Verdict::Unknown));
		assert_eq!((tracks[0].viewers, tracks[0].verdict), (1, Verdict::Unhealthy));

		let local = Local::default();
		let uplink = Uplink::default();
		assert_eq!(
			causes(problems(&facts(&local, &uplink, &graded.viewers, &tracks))),
			["1 of 1 viewers are stalling on video avc3.42c01f 1920x1080."]
		);
	}

	/// Problems list worst first: an unreachable relay and a frozen or silent
	/// source above a slow round trip.
	#[test]
	fn problems_list_worst_first() {
		let local = Local {
			stalled: vec![
				("camera".into(), Stall::Picture(Duration::from_secs(3))),
				("mic".into(), Stall::Sound(Duration::from_secs(2))),
			],
			stopped: vec![
				("intro".into(), Status::Ended),
				("music".into(), Status::Failed("the file is gone".into())),
			],
			..Default::default()
		};
		let uplink = Uplink {
			rtt: Some(Duration::from_millis(400)),
			grades: vec![(Input::Rtt, Verdict::Degraded)],
			..Default::default()
		};
		let relays = [relay("https://a", State::Disconnected("timed out".into()))];
		let mut facts = facts(&local, &uplink, &[], &[]);
		facts.relays = &relays;
		assert_eq!(
			causes(problems(&facts)),
			[
				"The relay https://a is unreachable (timed out), so nothing reaches viewers until a relay reconnects.",
				"Source `camera` stalled: no new picture for 3 s, so the program repeats its last one.",
				"Source `mic` stalled: no new sound for 2 s, so the program plays silence for it.",
				"Source `music` failed: the file is gone.",
				"Source `intro` played to its end, so the program holds its last picture and plays no sound for it.",
				"The relay is far away: a round trip to it takes 400 ms.",
			]
		);
	}

	fn relay(endpoint: &str, state: State) -> Relay {
		Relay {
			endpoint: endpoint.into(),
			state,
			version: None,
		}
	}

	/// While another relay serves, an unreachable one costs viewers their
	/// fallback rather than the stream, whichever of the two it is; with none
	/// serving, nothing reaches viewers.
	#[test]
	fn a_lost_relay_is_an_outage_only_without_another() {
		let local = Local::default();
		let uplink = Uplink::default();
		let lost = |relays: &[Relay]| {
			let mut facts = facts(&local, &uplink, &[], &[]);
			facts.relays = relays;
			problems(&facts)
				.into_iter()
				.map(|problem| (problem.verdict, problem.cause))
				.collect::<Vec<_>>()
		};
		let down = || State::Disconnected("timed out".into());
		assert_eq!(
			lost(&[relay("https://a", down()), relay("https://b", State::Connected)]),
			[(
				Verdict::Degraded,
				"The relay https://a is unreachable (timed out), so the stream reaches viewers only through https://b."
					.to_string()
			)]
		);
		assert_eq!(
			lost(&[relay("https://a", State::Connected), relay("https://b", down())]),
			[(
				Verdict::Degraded,
				"The relay https://b is unreachable (timed out), so the stream reaches viewers only through https://a."
					.to_string()
			)]
		);
		let outage = lost(&[relay("https://a", down()), relay("https://b", State::Connecting)]);
		assert_eq!(
			outage.iter().map(|(verdict, _)| *verdict).collect::<Vec<_>>(),
			[Verdict::Unhealthy, Verdict::Degraded]
		);
	}

	/// The relay's count is the sessions on the broadcast itself, over every
	/// tier, not those on broadcasts nested beneath it.
	#[test]
	fn watching_counts_sessions_on_the_broadcast_itself() {
		let mut viewers = moq_stats::Prefix::default();
		viewers.own.publisher.broadcasts_started = 5;
		viewers.own.publisher.broadcasts_ended = 2;
		viewers.publisher.broadcasts_started = 9;
		let mut cluster = moq_stats::Prefix::default();
		cluster.own.publisher.broadcasts_started = 1;
		let frame = moq_stats::PrefixFrame::from([("".to_string(), viewers), ("eu".to_string(), cluster)]);
		assert_eq!(watching(&frame), 4);
	}

	/// The catalog's `echo` reference resolves beneath the broadcast, wherever
	/// the broadcast sits.
	#[test]
	fn feedback_lands_beneath_the_broadcast() {
		for (path, relative, resolved) in [
			("show", "show/viewers", "show/viewers"),
			("room/live", "live/viewers", "room/live/viewers"),
		] {
			let (rel, abs) = echo(&moq_net::Path::new(path).unwrap()).unwrap();
			assert_eq!(rel.as_str(), relative);
			assert_eq!(abs.as_str(), resolved);
		}
	}

	/// A viewer's feedback is read as it arrives, grades the track it
	/// reports, and times the track from the program clock to the viewer. The
	/// viewer is forgotten when its broadcast ends.
	#[tokio::test(start_paused = true)]
	async fn viewer_feedback_times_each_track() {
		let _ = moq_tokio::crypto::install_default();
		let origin = moq_tokio::origin::spawn();
		let publication = origin.create_broadcast("show").unwrap();
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
		let config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		let track = broadcast
			.create_track("audio", catalog.track_info(hang::catalog::PRIORITY.audio))
			.unwrap();
		let _audio = catalog
			.audio(
				track,
				moq_mux::catalog::hang::Container::Legacy(moq_mux::container::Kind::Audio),
				config,
			)
			.unwrap();
		let id = catalog.snapshot().audio.renditions.keys().next().unwrap().clone();

		let path = moq_net::Path::new("show").unwrap();
		let (_, prefix) = echo(&path).unwrap();
		let feedback = Feedback::default();
		let follow = tokio::spawn(feedback.clone().follow(origin.consume(), prefix));

		// Connecting nowhere, so the connection only ever redials.
		let client = moq_tokio::connect::Config::default().init(Default::default()).unwrap();
		let connection = client.connect("https://127.0.0.1:1".parse::<url::Url>().unwrap());
		let clock = catalog.clock();
		let mut sampler = Sampler::new(Some(Publisher {
			observer: Observer::new(path, None),
			catalog: catalog.clone(),
			clock: clock.clone(),
			relays: vec![Link {
				endpoint: "https://127.0.0.1:1".into(),
				monitor: connection.monitor(),
			}],
			feedback: feedback.clone(),
			watching: Watching::default(),
			sources: moq_scene::publish::Publish::new(clock.clone(), moq_video::encode::Kind::Auto, Default::default())
				.unwrap(),
		}));

		let alice = origin.create_broadcast("show/viewers/alice.echo").unwrap();
		let track = alice.create_track(hang::echo::TRACK, None).unwrap();
		let mut reports = moq_json::snapshot::Producer::<hang::echo::Snapshot>::new(track, Default::default());
		alice.announce(Default::default()).unwrap();

		let timestamp = moq_net::Timestamp::from_millis(20_000).unwrap();
		let wall = clock.wall_clock(timestamp).unwrap() + Duration::from_millis(250);
		let mut report = hang::echo::Snapshot::default();
		let mut rendition = hang::echo::Rendition::default();
		rendition.frames = 50;
		rendition.arrival = Some(hang::echo::Arrival::new(timestamp, wall));
		report.renditions.insert(id.clone(), rendition);
		reports.update(&report).unwrap();

		let sampled = until(&mut sampler, |report| {
			report
				.viewers
				.reports
				.first()
				.is_some_and(|viewer| viewer.latency.is_some())
		})
		.await;
		let viewer = &sampled.viewers.reports[0];
		assert_eq!(viewer.name, "alice");
		let audio = sampled.tracks.iter().find(|track| track.id == id).unwrap();
		assert_eq!(audio.label, "audio opus 2 ch");
		assert_eq!(audio.viewers, 1);
		// The report carries milliseconds, so the program clock's sub-millisecond
		// wall mapping can floor it by one.
		let latency = audio.latency.unwrap();
		assert!(
			(Duration::from_millis(249)..=Duration::from_millis(250)).contains(&latency),
			"{latency:?}"
		);
		assert_eq!(viewer.latency, Some(latency));

		alice.unannounce();
		until(&mut sampler, |report| report.viewers.reports.is_empty()).await;
		follow.abort();
	}

	/// A relay that shares its stats counts each session watching the
	/// broadcast, and only those on the broadcast itself.
	#[tokio::test(flavor = "multi_thread")]
	async fn the_relay_counts_who_watches() {
		let _ = moq_tokio::crypto::install_default();
		let mut config = moq_relay::Config::default();
		config.drain_timeout = Some(Duration::ZERO.into());
		config.listen.bind = Some("127.0.0.1:0".parse().unwrap());
		config.listen.tls.generate = vec!["localhost".into()];
		config.web.http.listen = Some("127.0.0.1:0".parse().unwrap());
		config.auth.public = vec![moq_auth::Pattern::all()];
		config.stats.enabled = true;
		let relay = moq_relay::Relay::load(config).await.unwrap().with_signals(false);
		let url: url::Url = format!("https://{}/", relay.quic_addr().unwrap()).parse().unwrap();
		let ready = relay.ready();
		tokio::spawn(relay.run());
		ready.wait().await.unwrap();
		// `moq_relay::test_relay` cannot turn stats on, so this relay's
		// generated certificate goes unchecked, on loopback.
		let mut client = moq_tokio::connect::Config::default();
		client.tls.insecure = Some(true);
		let client = client.init(Default::default()).unwrap();

		let published = moq_tokio::origin::spawn();
		let show = published.create_broadcast("show").unwrap();
		let track = show.create_track("video", None).unwrap();
		show.announce(Default::default()).unwrap();
		let subscribe = moq_tokio::origin::spawn();
		let watching = Watching::default();
		tokio::spawn(
			watching
				.clone()
				.follow(subscribe.consume(), moq_net::Path::new("show").unwrap()),
		);
		let _studio = client
			.clone()
			.with_publisher(&published)
			.with_subscriber(subscribe)
			.connect(url.clone());

		let viewer = moq_tokio::origin::spawn();
		let _session = client.with_subscriber(viewer.clone()).connect(url);
		let wait = async {
			let broadcast = viewer
				.consume()
				.follow("show", None)
				.unwrap()
				.available()
				.await
				.unwrap();
			let mut video = broadcast.track("video").unwrap().subscribe(None).await.unwrap();
			track.append_group().unwrap().finish().unwrap();
			video.recv_group().await.unwrap();
			while watching.get() != Some(1) {
				kio::clock::sleep(INTERVAL / 4).await;
			}
		};
		kio::clock::timeout(Duration::from_secs(30), wait)
			.await
			.expect("the relay never counted the viewer");
	}

	/// However many viewers announce feedback, only [`MAX_VIEWERS`] are read
	/// and the rest counted; a reader a departing viewer frees goes to the
	/// next to arrive.
	#[tokio::test(start_paused = true)]
	async fn feedback_reads_a_bounded_sample() {
		let origin = moq_tokio::origin::spawn();
		let (_, prefix) = echo(&moq_net::Path::new("show").unwrap()).unwrap();
		let feedback = Feedback::default();
		let follow = tokio::spawn(feedback.clone().follow(origin.consume(), prefix));

		let announce = |n: usize| {
			let viewer = origin.create_broadcast(format!("show/viewers/{n:03}.echo")).unwrap();
			viewer.announce(Default::default()).unwrap();
			viewer
		};
		let read = |feedback: &Feedback, name: &str| {
			let graded = feedback.grade(kio::clock::now(), &Thresholds::default());
			(
				graded.len(),
				feedback.unread(),
				graded.iter().any(|viewer| viewer.name == name),
			)
		};
		let settled = async {
			let viewers: Vec<_> = (0..MAX_VIEWERS + 2).map(announce).collect();
			while read(&feedback, "065") != (MAX_VIEWERS, 2, false) {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
			viewers[0].unannounce();
			viewers[MAX_VIEWERS].unannounce();
			while read(&feedback, "000") != (MAX_VIEWERS - 1, 1, false) {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
			let _late = announce(MAX_VIEWERS + 2);
			while read(&feedback, "066") != (MAX_VIEWERS, 1, true) {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		};
		kio::clock::timeout(Duration::from_secs(30), settled)
			.await
			.expect("the sample never settled");
		follow.abort();
	}

	/// Sample every interval of the paused clock until `done` holds.
	async fn until(sampler: &mut Sampler, done: impl Fn(&Report) -> bool) -> Report {
		let wait = async {
			loop {
				let report = sampler.sample(kio::clock::now(), Local::default());
				if done(&report) {
					return report;
				}
				kio::clock::sleep(INTERVAL).await;
			}
		};
		kio::clock::timeout(Duration::from_secs(30), wait)
			.await
			.expect("the report never arrived")
	}
}
