//! Stream health: how the program actually reaches viewers, track by track,
//! and in plain words why it does not when it does not.
//!
//! Every [`INTERVAL`] the studio grades its stream with `hang::health`, the
//! grader the rest of the stack shares, from three points of view:
//! - its own: what each track of every broadcast wrote, the program's and
//!   each published scene's, each relay connection's state, and the serving
//!   one's round trip, estimated capacity, and loss, which they all share;
//! - its viewers': the feedback each publishes under the broadcast's
//!   `echo` path, which the catalog solicits (`moq export --echo` answers it);
//! - the relay's: how many sessions watch each broadcast, read from the
//!   relay's stats broadcasts when it shares them with this connection.
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
	/// The connection every broadcast shares.
	pub uplink: Uplink,
	/// Each broadcast, the program's first; empty when the studio
	/// publishes nothing.
	pub broadcasts: Vec<Published>,
	/// Each source's own local track, by source name.
	pub sources: Vec<Track>,
	/// What is wrong, worst first, each naming its cause.
	pub problems: Vec<Problem>,
}

/// One broadcast: who watches it, and how each of its tracks reaches
/// them.
#[derive(Debug, Default)]
pub struct Published {
	/// The published scene's name, or `None` for the program's.
	pub name: Option<String>,
	pub viewers: Viewers,
	/// Each track it lists, in catalog order.
	pub tracks: Vec<Track>,
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
	/// The media every broadcast's tracks wrote over the last interval. Source
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
	/// Each broadcast's video: the program's as `None`, then each
	/// published scene's by name.
	pub broadcasts: BTreeMap<Option<String>, Video>,
	/// Each program track's share of the connection's bandwidth, by name: a
	/// published scene's prefixed with its name and a slash.
	pub grants: BTreeMap<String, Option<Rate>>,
	/// Sources whose stalled picture or sound a live broadcast carries, what
	/// stalled, and which broadcasts carry it.
	pub stalled: Vec<(String, Stall, Live)>,
	/// Sources a live broadcast shows or plays that ended or failed, and
	/// where.
	pub stopped: Vec<(String, Status, Reach)>,
	/// Browser sources a live broadcast shows whose pages are uploaded from
	/// memory, why, and where they are shown.
	pub uploaded: Vec<(String, Arc<str>, Live)>,
}

impl Local {
	/// Whether anyone subscribes to any broadcast's video.
	fn watched(&self) -> bool {
		self.broadcasts.values().any(|video| video.watched)
	}
}

/// Where a source's picture and sound reach live broadcasts.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reach {
	/// Where its picture is drawn.
	pub shown: Live,
	/// Where its sound is played.
	pub heard: Live,
}

impl Reach {
	/// Whether any live broadcast shows or plays it.
	pub fn any(&self) -> bool {
		self.shown.any() || self.heard.any()
	}
}

/// The live broadcasts something reaches.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Live {
	/// The program, the top layer and a stinger included.
	pub program: bool,
	/// Each published or recorded scene apart from program, by name.
	pub scenes: Vec<String>,
}

impl Live {
	/// Whether it reaches any.
	pub fn any(&self) -> bool {
		self.program || !self.scenes.is_empty()
	}

	/// The broadcasts as a sentence's subject, such as "the program and scene
	/// `tall`", and whether that is more than one.
	fn subject(&self) -> (String, bool) {
		let program = self.program.then(|| "the program".to_string());
		let scenes = self.scenes.iter().map(|name| format!("scene `{name}`"));
		let parts: Vec<String> = program.into_iter().chain(scenes).collect();
		let subject = match parts.split_last() {
			Some((last, [])) => last.clone(),
			Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
			None => String::new(),
		};
		(subject, parts.len() > 1)
	}
}

/// What only the studio knows about one broadcast's video.
#[derive(Debug, Default, Clone)]
pub struct Video {
	/// Program frames the encoder had no room for.
	pub dropped: u64,
	/// Program frames encoded.
	pub encoded: u64,
	/// Whether the full size should be encoding: always, or while watched
	/// when on demand, and while its grant funds it.
	pub encoding: bool,
	/// Whether anyone subscribes to the video.
	pub watched: bool,
	/// The encoder backend, once one opened.
	pub encoder: Option<String>,
	/// Why the output stopped, if it did.
	pub failed: Option<String>,
	/// Which video this is: a scene published again under its name counts
	/// afresh, as another.
	pub generation: u64,
}

/// What the studio publishes, for grading its delivery.
pub struct Publisher {
	/// The program's broadcast.
	pub main: Broadcast,
	/// Each published scene's, while it is published.
	pub scenes: Scenes,
	/// The catalogs' clock, which maps a track's timestamps to the wall time
	/// the program drew them.
	pub clock: moq_mux::Clock,
	/// Each relay the broadcasts go to, the preferred first.
	pub relays: Vec<Link>,
	/// Where the sources keep their own tracks, for what those wrote.
	pub sources: moq_scene::publish::Publish,
}

/// One broadcast, graded: what it lists, and what its viewers and
/// the relays say of it.
#[derive(Clone)]
pub struct Broadcast {
	/// Its path and the instance it was announced with.
	pub observer: Observer,
	pub catalog: moq_scene::publish::Catalog,
	pub feedback: Feedback,
	pub watching: Watching,
	/// Read for as long as any clone is held.
	followers: Arc<Followers>,
}

impl Broadcast {
	/// Grade the broadcast `observer` names, which `catalog` lists, reading
	/// its viewers' feedback beneath it and the relays' count of who watches
	/// it from `reports`, until the last clone drops. The catalog must solicit
	/// that feedback, as [`solicit`] has it.
	pub fn follow(
		observer: Observer,
		catalog: moq_scene::publish::Catalog,
		reports: &moq_net::origin::Consumer,
	) -> anyhow::Result<Self> {
		let (_, viewers) = echo(&observer.path)?;
		let feedback = Feedback::default();
		let watching = Watching::default();
		let followers = Followers(vec![
			tokio::spawn(log(
				"viewer feedback",
				feedback.clone().follow(reports.clone(), viewers),
			)),
			tokio::spawn(log(
				"relay stats",
				watching.clone().follow(reports.clone(), observer.path.clone()),
			)),
		]);
		Ok(Self {
			observer,
			catalog,
			feedback,
			watching,
			followers: Arc::new(followers),
		})
	}
}

/// Have the broadcast at `path`, which `catalog` lists, ask its viewers for
/// feedback beneath it.
pub fn solicit(catalog: &mut moq_scene::publish::Catalog, path: &moq_net::Path) -> anyhow::Result<()> {
	let (echo, _) = echo(path)?;
	let mut section = catalog.modify()?;
	section.echo = Some(hang::catalog::Echo::new(echo));
	section.commit()?;
	Ok(())
}

/// Tasks that stop when dropped.
struct Followers(Vec<tokio::task::JoinHandle<()>>);

impl Drop for Followers {
	fn drop(&mut self) {
		for task in &self.0 {
			task.abort();
		}
	}
}

/// Run a background reader, logging why it stopped.
async fn log(what: &'static str, task: impl Future<Output = anyhow::Result<()>>) {
	if let Err(err) = task.await {
		tracing::warn!(%err, "stopped reading {what}");
	}
}

/// The published scenes' broadcasts graded, by scene name, shared with what
/// publishes them.
#[derive(Clone)]
pub struct Scenes {
	graded: Arc<Mutex<BTreeMap<String, Broadcast>>>,
	/// Where viewers' reports and the relays' stats arrive.
	reports: moq_net::origin::Consumer,
	/// Where their readers run, rather than on the thread that publishes one,
	/// such as the program clock's.
	runtime: tokio::runtime::Handle,
}

impl Scenes {
	/// Published scenes graded from what arrives on `reports`, read on the
	/// tokio runtime this is called inside.
	pub fn new(reports: moq_net::origin::Consumer) -> Self {
		Self {
			graded: Arc::default(),
			reports,
			runtime: tokio::runtime::Handle::current(),
		}
	}

	/// Grade scene `name`'s broadcast, which `observer` names and `catalog`
	/// lists, as [`Broadcast::follow`] does, until the returned handle drops.
	pub fn follow(
		&self,
		name: &str,
		observer: Observer,
		catalog: moq_scene::publish::Catalog,
	) -> anyhow::Result<Graded> {
		let broadcast = {
			let _runtime = self.runtime.enter();
			Broadcast::follow(observer, catalog, &self.reports)?
		};
		let followers = broadcast.followers.clone();
		self.graded.lock().unwrap().insert(name.to_string(), broadcast);
		Ok(Graded {
			graded: self.graded.clone(),
			name: name.to_string(),
			followers,
		})
	}

	fn snapshot(&self) -> Vec<(String, Broadcast)> {
		let graded = self.graded.lock().unwrap();
		graded
			.iter()
			.map(|(name, broadcast)| (name.clone(), broadcast.clone()))
			.collect()
	}
}

/// A published scene's broadcast being graded, until dropped.
pub struct Graded {
	graded: Arc<Mutex<BTreeMap<String, Broadcast>>>,
	name: String,
	/// Its readers, which tell it from a scene published again under its name
	/// before it ended.
	followers: Arc<Followers>,
}

impl Drop for Graded {
	fn drop(&mut self) {
		let mut graded = self.graded.lock().unwrap();
		let ours = graded.get(&self.name);
		if ours.is_some_and(|broadcast| Arc::ptr_eq(&broadcast.followers, &self.followers)) {
			graded.remove(&self.name);
		}
	}
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
		// Each broadcast's viewers, as graded, beside `report.broadcasts`.
		let mut watched = Vec::new();
		let (mut current, mut sources) = (None, None);
		let mut serving = None;
		if let Some(publisher) = &self.publisher {
			report.relays = publisher.relays.iter().map(relay).collect();
			serving = report.relays.iter().position(|relay| relay.state == State::Connected);
			let link = serving.map(|index| &publisher.relays[index]);
			let extras = publisher.scenes.snapshot().into_iter();
			let broadcasts: Vec<(Option<String>, Broadcast)> = std::iter::once((None, publisher.main.clone()))
				.chain(extras.map(|(name, broadcast)| (Some(name), broadcast)))
				.collect();
			// Every broadcast's tracks share the connection, so the uplink is graded
			// on all of them.
			let ours = health::Report::new(
				publisher.main.observer.clone(),
				snapshot(&broadcasts, link.map(|link| &link.monitor)),
			);
			// Another relay's link counted its transport from another start.
			let previous_ours = previous
				.as_ref()
				.filter(|previous| previous.serving == serving)
				.and_then(|previous| previous.report.as_ref());
			let graded_ours = self.grade(&ours, previous_ours.zip(interval));
			report.uplink = uplink(&ours.snapshot, &graded_ours, local.watched());
			report.uplink.relay = link.map(|link| link.endpoint.clone());
			// Graded apart from the program's, so their media stays out of the
			// uplink's; see [`Uplink::media`].
			let mut snapshot = hang::stats::Snapshot::default();
			snapshot.renditions = publisher.sources.stats();
			let theirs = health::Report::new(publisher.main.observer.clone(), snapshot);
			let previous_theirs = previous.as_ref().and_then(|previous| previous.sources.as_ref());
			let graded_theirs = self.grade(&theirs, previous_theirs.zip(interval));

			for (name, broadcast) in broadcasts {
				let graded = broadcast.feedback.grade(now, &self.thresholds);
				let listed = Listed {
					renditions: renditions(&broadcast.catalog.snapshot()).collect(),
					prefix: prefix(name.as_deref()),
					source: None,
					sent: &graded_ours,
					viewers: &graded,
					grants: &local.grants,
				};
				report.broadcasts.push(Published {
					viewers: Viewers {
						watching: broadcast.watching.get(),
						reports: viewers(&graded, &publisher.clock),
						unread: broadcast.feedback.unread(),
					},
					tracks: tracks(&listed, &publisher.clock),
					name,
				});
				watched.push(graded);
			}
			let listed = publisher.sources.catalog();
			for (name, catalog) in &listed.ext.studio.sources {
				let listed = Listed {
					renditions: renditions(catalog).collect(),
					prefix: String::new(),
					source: Some(name),
					sent: &graded_theirs,
					viewers: &[],
					grants: &local.grants,
				};
				report.sources.extend(tracks(&listed, &publisher.clock));
			}
			current = Some(ours);
			sources = Some(theirs);
		}

		let drops = previous
			.as_ref()
			.map(|previous| {
				let drawn = local.rendered.saturating_sub(previous.local.rendered);
				let broadcasts = local.broadcasts.iter();
				broadcasts
					.filter_map(|(name, video)| {
						let before = previous.local.broadcasts.get(name)?;
						if before.generation != video.generation {
							return None;
						}
						let drops = Drops {
							drawn,
							dropped: video.dropped.saturating_sub(before.dropped),
							encoded: video.encoded.saturating_sub(before.encoded),
						};
						Some((name.clone(), drops))
					})
					.collect()
			})
			.unwrap_or_default();
		let seen = report.broadcasts.iter().zip(&watched).map(|(broadcast, viewers)| Seen {
			name: broadcast.name.as_deref(),
			viewers,
			tracks: &broadcast.tracks,
		});
		report.problems = problems(&Facts {
			relays: &report.relays,
			local: &local,
			drops,
			uplink: &report.uplink,
			broadcasts: seen.collect(),
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

/// What a published scene's tracks are graded under, before each ID, since
/// two broadcasts can list the same one: its name and a slash, as its grants
/// are listed; nothing for the program's.
fn prefix(scene: Option<&str>) -> String {
	scene.map(|name| format!("{name}/")).unwrap_or_default()
}

/// What every broadcast's tracks have written, each graded under its
/// [`prefix`], and the transport of the link `monitor` watches.
fn snapshot(
	broadcasts: &[(Option<String>, Broadcast)],
	monitor: Option<&moq_tokio::connection::Monitor>,
) -> hang::stats::Snapshot {
	let mut snapshot = hang::stats::Snapshot::default();
	for (name, broadcast) in broadcasts {
		let prefix = prefix(name.as_deref());
		let renditions = broadcast.catalog.stats().into_iter();
		snapshot
			.renditions
			.extend(renditions.map(|(id, rendition)| (format!("{prefix}{id}"), rendition)));
	}
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

/// What one catalog lists now, to grade each of its tracks.
struct Listed<'a> {
	/// Each video and audio rendition, as [`renditions`] reads them.
	renditions: Vec<Rendition>,
	/// What its tracks are graded and granted under, before each ID.
	prefix: String,
	/// The source whose own catalog it is, or `None` for a broadcast's.
	source: Option<&'a String>,
	/// What its tracks wrote, as graded.
	sent: &'a Health,
	/// Its viewers, as graded: a broadcast's alone.
	viewers: &'a [GradedViewer],
	/// Each program track's share of the connection, by name.
	grants: &'a BTreeMap<String, Option<Rate>>,
}

/// Each media track `listed`, with what it sent and what its viewers
/// received. A source's stills carry no media to grade, so they are left out.
fn tracks(listed: &Listed, clock: &moq_mux::Clock) -> Vec<Track> {
	listed
		.renditions
		.iter()
		.map(
			|Rendition {
			     id,
			     label,
			     enabled,
			     main,
			 }| {
				// One listed by reference to the program's broadcast, as a published
				// scene lists the audio, is graded and granted there.
				let graded = match main {
					Some(track) => track.clone(),
					None => format!("{}{id}", listed.prefix),
				};
				let ours = listed.sent.renditions.get(&graded);
				let interval = listed.sent.interval;
				// A source's track is in the local broadcast, whose names can repeat
				// the program's, and no relay viewer or grant reaches it.
				let program = listed.source.is_none();
				let mut track = Track {
					source: listed.source.cloned(),
					id: id.clone(),
					label: label.clone(),
					granted: listed.grants.get(&graded).copied().flatten().filter(|_| program),
					enabled: *enabled,
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
				for viewer in listed.viewers {
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
			},
		)
		.collect()
}

/// One video or audio rendition a catalog lists.
struct Rendition {
	id: String,
	/// What it carries, such as `video H.264 1920x1080`.
	label: String,
	enabled: bool,
	/// Its track in the main broadcast, when listed there by reference: the
	/// only broadcast the studio's catalogs refer to.
	main: Option<String>,
}

/// Each video and audio rendition `catalog` lists, by ID, with what it
/// carries and whether it is enabled.
fn renditions<E>(catalog: &hang::Catalog<E>) -> impl Iterator<Item = Rendition> {
	let main = |id: &String, broadcast: bool, track: &Option<String>| {
		broadcast.then(|| track.clone().unwrap_or_else(|| id.clone()))
	};
	let video = catalog.video.renditions.iter().map(move |(id, config)| {
		let size = match (config.coded_width, config.coded_height) {
			(Some(width), Some(height)) => format!(" {width}x{height}"),
			_ => String::new(),
		};
		Rendition {
			id: id.clone(),
			label: format!("video {}{size}", config.codec),
			enabled: config.enabled,
			main: main(id, config.broadcast.is_some(), &config.track),
		}
	});
	let audio = catalog.audio.renditions.iter().map(move |(id, config)| Rendition {
		id: id.clone(),
		label: format!("audio {} {} ch", config.codec, config.channel_count),
		enabled: config.enabled,
		main: main(id, config.broadcast.is_some(), &config.track),
	});
	video.chain(audio)
}

fn viewers(graded: &[GradedViewer], clock: &moq_mux::Clock) -> Vec<Viewer> {
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
						.filter_map(|id| viewer.latency(id, clock))
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
	/// Each broadcast's frames over the last interval, once there was
	/// one.
	drops: BTreeMap<Option<String>, Drops>,
	uplink: &'a Uplink,
	/// What each broadcast's viewers report, the program's first.
	broadcasts: Vec<Seen<'a>>,
	thresholds: &'a Thresholds,
}

/// One broadcast's viewers, as graded.
struct Seen<'a> {
	/// The published scene's name, or `None` for the program's.
	name: Option<&'a str>,
	viewers: &'a [GradedViewer],
	/// Its tracks, for naming one a viewer reports on.
	tracks: &'a [Track],
}

/// `cause` said of the broadcast `scene` names: a published scene's names it.
fn on(scene: Option<&str>, cause: String) -> String {
	let Some(name) = scene else { return cause };
	let mut chars = cause.chars();
	let first = chars.next().map(|first| first.to_lowercase().to_string());
	format!("On scene `{name}`, {}{}", first.unwrap_or_default(), chars.as_str())
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
	for (name, video) in &local.broadcasts {
		if let Some(failed) = &video.failed {
			add(
				Verdict::Unhealthy,
				on(name.as_deref(), format!("The output stopped: {failed}.")),
			);
		}
	}

	// A clause saying what `live` does instead, as a verb that agrees with it.
	let clause = |live: &Live, one: &str, many: &str, rest: &str| {
		let (subject, plural) = live.subject();
		format!("{subject} {} {rest}", if plural { many } else { one })
	};
	for (name, stall, live) in &local.stalled {
		let instead = match stall {
			Stall::Picture(_) => clause(live, "repeats", "repeat", "its last one"),
			Stall::Sound(_) => clause(live, "plays", "play", "silence for it"),
		};
		add(
			Verdict::Unhealthy,
			format!("Source `{name}` stalled: {}, so {instead}.", without(*stall)),
		);
	}
	for (name, status, reach) in &local.stopped {
		match status {
			Status::Ended => {
				let Reach { shown, heard } = reach;
				let instead = if shown == heard {
					let (subject, many) = shown.subject();
					let (hold, play) = if many { ("hold", "play") } else { ("holds", "plays") };
					format!("{subject} {hold} its last picture and {play} no sound for it")
				} else {
					let hold = shown.any().then(|| clause(shown, "holds", "hold", "its last picture"));
					let silent = heard.any().then(|| clause(heard, "plays", "play", "no sound for it"));
					let parts: Vec<String> = hold.into_iter().chain(silent).collect();
					parts.join(" and ")
				};
				add(
					Verdict::Degraded,
					format!("Source `{name}` played to its end, so {instead}."),
				);
			}
			Status::Failed(reason) => add(Verdict::Unhealthy, format!("Source `{name}` failed: {reason}.")),
			_ => {}
		}
	}
	for (name, why, live) in &local.uploaded {
		let (subject, many) = live.subject();
		let show = if many { "show" } else { "shows" };
		add(
			Verdict::Degraded,
			format!(
				"Source `{name}`, which {subject} {show}, is uploaded from memory every frame rather than copied on the GPU: {why}."
			),
		);
	}

	for (name, drops) in &facts.drops {
		let Some(video) = local.broadcasts.get(name) else {
			continue;
		};
		if !video.encoding || video.failed.is_some() {
			continue;
		}
		let encoder = video.encoder.as_deref().unwrap_or("the encoder");
		let share = ratio(drops.dropped, drops.drawn);
		let limits = &facts.thresholds.publisher.skipped;
		if drops.drawn > 0 && drops.encoded == 0 && drops.dropped == 0 && video.encoder.is_some() {
			add(
				Verdict::Unhealthy,
				on(
					name.as_deref(),
					format!("The encoder is stuck: {encoder} produced nothing in the last second."),
				),
			);
		} else if let Some(share) = share {
			add(
				grade_above(share, limits.degraded, limits.unhealthy),
				on(
					name.as_deref(),
					format!(
						"The encoder is behind: {encoder} had no room for {} of {} program frames in the last second.",
						drops.dropped, drops.drawn
					),
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

	for seen in &facts.broadcasts {
		viewer_problems(uplink, seen, &mut |verdict, cause| add(verdict, on(seen.name, cause)));
	}

	problems.sort_by_key(|problem| std::cmp::Reverse(problem.verdict));
	problems
}

/// What one broadcast's viewers report, one problem per kind of trouble across
/// them, beside the `uplink` they share.
fn viewer_problems(uplink: &Uplink, seen: &Seen<'_>, add: &mut impl FnMut(Verdict, String)) {
	let reporting = seen.viewers.iter().filter(|viewer| viewer.health.is_some()).count();
	// A congested or lossy uplink starves every viewer, and one measured
	// clear of both leaves the blame with the viewers' own links. With either
	// unmeasured, as headroom is under a controller without an estimate, the
	// cause goes unnamed.
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
	for viewer in seen.viewers {
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
		id.and_then(|id| seen.tracks.iter().find(|track| track.id == id))
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
			match event {
				moq_net::announce::Event::Present(announce) => {
					let path = announce.prefix;
					let Some(name) = name(&prefix, &path).filter(|_| hang::echo::is_echo(&path)) else {
						continue;
					};
					if readers.len() < MAX_VIEWERS {
						let reader = self.read(origin.clone(), path.clone(), name);
						readers.insert(path, reader);
					} else {
						unread += 1;
					}
				}
				moq_net::announce::Event::Absent(announce) => {
					if readers.remove(&announce.prefix).is_none() && hang::echo::is_echo(&announce.prefix) {
						unread = unread.saturating_sub(1);
					}
				}
			}
			self.0.lock().unwrap().unread = unread;
		}
		Ok(())
	}

	/// Start reading the viewer at `path`, named `name`.
	fn read(&self, origin: moq_net::origin::Consumer, path: moq_net::Path, name: String) -> Reader {
		let slot = Slot::default();
		self.0.lock().unwrap().slots.insert(name.clone(), slot.clone());
		let task = tokio::spawn(read(origin, path, slot.clone()));
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

/// Keep the latest reports of the viewer at `path` in `slot`, from each
/// instance that serves it in turn, until dropped. A new instance starts its
/// reports afresh.
async fn read(origin: moq_net::origin::Consumer, path: moq_net::Path, slot: Slot) {
	let mut following = match origin.follow(&path, None) {
		Ok(following) => following,
		Err(err) => return failed(&slot, &path, err.into()),
	};
	let mut next = following.available().await;
	loop {
		let broadcast = match next {
			Ok(Some(broadcast)) => broadcast,
			// Offline until another instance serves the path.
			Ok(None) => {
				next = following.available().await;
				continue;
			}
			// A closed origin answers every later call at once.
			Err(err @ moq_net::Error::Closed) => return failed(&slot, &path, err.into()),
			// The follow waits for the route to change before it asks again.
			Err(err) => {
				failed(&slot, &path, err.into());
				next = following.available().await;
				continue;
			}
		};
		*slot.lock().unwrap() = Reports::default();
		let observer = Observer::new(path.clone(), broadcast.info().instance.clone());
		tokio::select! {
			result = reports(&broadcast, observer, &slot) => {
				if let Err(err) = result {
					failed(&slot, &path, err);
				}
				next = following.available().await;
			}
			changed = following.available() => next = changed,
		}
	}
}

/// Keep the latest reports `broadcast` publishes in `slot`, until its track ends.
async fn reports(broadcast: &moq_net::broadcast::Consumer, observer: Observer, slot: &Slot) -> anyhow::Result<()> {
	let track = broadcast.track(hang::echo::TRACK)?;
	let mut config = moq_json::snapshot::consumer::Config::default();
	config.max_size = Some(MAX_REPORT);
	let mut reports = moq_json::snapshot::Consumer::<hang::echo::Snapshot>::new(track, config);
	while let Some(report) = reports.latest().await? {
		let report = health::Report::new(observer.clone(), report.value);
		let mut reports = slot.lock().unwrap();
		reports.previous = reports.current.take();
		reports.current = Some((report, kio::clock::now()));
	}
	Ok(())
}

/// Record why reading the viewer at `path` stopped.
fn failed(slot: &Slot, path: &moq_net::Path, err: anyhow::Error) {
	tracing::debug!(%err, viewer = %path, "stopped reading feedback");
	slot.lock().unwrap().error = Some(format!("{err:#}"));
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

	/// What the program's broadcast alone shows: `viewers` reporting on `tracks`.
	fn facts<'a>(local: &'a Local, uplink: &'a Uplink, viewers: &'a [GradedViewer], tracks: &'a [Track]) -> Facts<'a> {
		static THRESHOLDS: std::sync::LazyLock<Thresholds> = std::sync::LazyLock::new(Thresholds::default);
		Facts {
			relays: &[],
			local,
			drops: BTreeMap::new(),
			uplink,
			broadcasts: vec![Seen {
				name: None,
				viewers,
				tracks,
			}],
			thresholds: &THRESHOLDS,
		}
	}

	/// A studio whose program video is `main`, and the published scene
	/// `tall`'s, if any, `tall`.
	fn local(main: Video, tall: Option<Video>) -> Local {
		let tall = tall.map(|video| (Some("tall".to_string()), video));
		Local {
			broadcasts: std::iter::once((None, main)).chain(tall).collect(),
			..Default::default()
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

		let local = Local::default();
		let mut watched = uplink(&current.snapshot, &graded, true);
		watched.relay = Some("https://a".into());
		assert_eq!(watched.verdict, Verdict::Unhealthy);
		assert_eq!(
			causes(problems(&facts(&local, &watched, &[], &[]))),
			[
				"The uplink to the relay https://a is congested: the connection carries about 2.5 Mb/s and the stream needs 3.0 Mb/s."
			]
		);
		let idle = uplink(&current.snapshot, &graded, false);
		assert_eq!(idle.verdict, Verdict::Unknown);
		assert!(problems(&facts(&local, &idle, &[], &[])).is_empty());
	}

	/// Program frames the encoder had no room for, and an encoder that
	/// produces nothing at all, both blame the encoder by name, on the broadcast
	/// it encodes; one that stopped says why, and one not encoding is not
	/// blamed.
	#[test]
	fn the_encoder_falling_behind_is_named() {
		let encoding = |encoder: &str| Video {
			encoding: true,
			encoder: Some(encoder.into()),
			..Default::default()
		};
		let local = local(encoding("nvenc"), Some(encoding("vaapi")));
		let uplink = Uplink::default();
		let mut facts = facts(&local, &uplink, &[], &[]);
		let drops = |dropped, encoded| Drops {
			drawn: 30,
			dropped,
			encoded,
		};
		facts.drops = BTreeMap::from([(None, drops(3, 27)), (Some("tall".into()), drops(0, 0))]);
		let problems = problems(&facts);
		assert_eq!(problems[0].verdict, Verdict::Unhealthy);
		assert_eq!(
			causes(problems),
			[
				"The encoder is behind: nvenc had no room for 3 of 30 program frames in the last second.",
				"On scene `tall`, the encoder is stuck: vaapi produced nothing in the last second.",
			]
		);

		facts.drops = BTreeMap::from([(None, drops(0, 30)), (Some("tall".into()), drops(0, 30))]);
		assert!(super::problems(&facts).is_empty());

		let stopped = Video {
			failed: Some("video: the encoder failed".into()),
			..encoding("vaapi")
		};
		let local = super::tests::local(Video::default(), Some(stopped));
		let mut facts = super::tests::facts(&local, &uplink, &[], &[]);
		facts.drops = BTreeMap::from([(None, drops(0, 0)), (Some("tall".into()), drops(0, 0))]);
		assert_eq!(
			causes(super::problems(&facts)),
			["On scene `tall`, the output stopped: video: the encoder failed."]
		);
	}

	/// A scene published again under its name counts its frames afresh,
	/// rather than from the counters of the one before.
	#[test]
	fn a_scene_published_again_counts_afresh() {
		let mut sampler = Sampler::new(None);
		let start = kio::clock::now();
		let mut sample = |seconds: u32, generation: u64| {
			let tall = Video {
				encoding: true,
				encoder: Some("vaapi".into()),
				encoded: if generation == 0 { 300 } else { 0 },
				generation,
				..Default::default()
			};
			let mut local = local(Video::default(), Some(tall));
			local.rendered = u64::from(seconds) * 30;
			causes(sampler.sample(start + INTERVAL * seconds, local).problems)
		};
		sample(10, 0);
		let republished = sample(11, 1);
		assert!(republished.is_empty(), "{republished:?}");
		assert_eq!(
			sample(12, 1),
			["On scene `tall`, the encoder is stuck: vaapi produced nothing in the last second."]
		);
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

	/// Each broadcast's tracks are graded from what they wrote under the
	/// broadcast's prefix, beside its own viewers, and each source's media
	/// tracks the same way, by source name; a still has no media to grade. A
	/// source's track stays in the studio, so no viewer report or grant
	/// applies to it, and a published scene's tracks take its own grants and
	/// viewers, though each can share a track name with the program's,
	/// but the main broadcast's audio it lists by reference is graded where
	/// the main broadcast writes it.
	#[test]
	fn tracks_are_graded_by_broadcast_and_source() {
		let video = |width: u32, height: u32| {
			let codec: hang::catalog::VideoCodec = "avc3.42c01f".parse().unwrap();
			let mut config = hang::catalog::VideoConfig::new(codec);
			config.coded_width = Some(width);
			config.coded_height = Some(height);
			config
		};
		let mut main = hang::Catalog::<moq_scene::publish::Ext>::default();
		main.video.renditions.insert("0.avc3".into(), video(1920, 1080));
		let mut half = video(960, 540);
		half.enabled = false;
		main.video.renditions.insert("3.avc3".into(), half);
		let opus = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		main.audio.renditions.insert("audio".into(), opus.clone());
		let mut tall = hang::Catalog::<moq_scene::publish::Ext>::default();
		tall.video.renditions.insert("0.avc3".into(), video(1080, 1920));
		// The main broadcast's audio, listed by reference.
		let mut shared = opus;
		shared.broadcast = Some(moq_net::path::Relative::new("..").unwrap());
		shared.track = Some("audio".into());
		tall.audio.renditions.insert("audio".into(), shared);
		let mut camera = hang::Catalog::<()>::default();
		camera.video.renditions.insert("0.avc3".into(), video(1280, 720));
		let mut logo = hang::Catalog::<()>::default();
		let png = hang::catalog::BinaryConfig::new(hang::catalog::Mode::Snapshot);
		logo.binary.tracks.insert("1.png".into(), png);
		let grants = [
			("0.avc3".to_string(), Some(Rate::from_mbps(4))),
			("tall/0.avc3".to_string(), Some(Rate::from_mbps(2))),
		]
		.into();

		// 375 kB in a second is 3 Mb/s, 250 kB is 2 Mb/s, 125 kB is 1 Mb/s,
		// and 16 kB is 128 kb/s.
		let ours = wrote(&[("0.avc3", 375_000), ("tall/0.avc3", 250_000), ("audio", 16_000)]);
		let theirs = wrote(&[("0.avc3", 125_000)]);
		let main_viewers = [stalling("alice", "3.avc3", Duration::from_millis(100))];
		let tall_viewers = [stalling("bob", "0.avc3", Duration::from_millis(100))];
		let clock = moq_mux::Clock::new();
		let broadcast = |catalog: &hang::Catalog<moq_scene::publish::Ext>, prefix: &str, viewers| {
			let listed = Listed {
				renditions: renditions(catalog).collect(),
				prefix: prefix.into(),
				source: None,
				sent: &ours,
				viewers,
				grants: &grants,
			};
			tracks(&listed, &clock)
		};
		let main = broadcast(&main, "", &main_viewers);
		let tall = broadcast(&tall, "tall/", &tall_viewers);
		let source = |name: &String, catalog: &hang::Catalog<()>| {
			let listed = Listed {
				renditions: renditions(catalog).collect(),
				prefix: String::new(),
				source: Some(name),
				sent: &theirs,
				viewers: &[],
				grants: &grants,
			};
			tracks(&listed, &clock)
		};
		let sources: Vec<Track> = [("camera".to_string(), camera), ("logo".to_string(), logo)]
			.iter()
			.flat_map(|(name, catalog)| source(name, catalog))
			.collect();

		let rows: Vec<_> = main
			.iter()
			.chain(&tall)
			.chain(&sources)
			.map(|track| {
				(
					track.source.as_deref(),
					track.id.as_str(),
					track.sent.map(rate),
					track.granted.map(rate),
					track.enabled,
					track.viewers,
				)
			})
			.collect();
		assert_eq!(
			rows,
			[
				(
					None,
					"0.avc3",
					Some("3.0 Mb/s".into()),
					Some("4.0 Mb/s".into()),
					true,
					0
				),
				(None, "3.avc3", None, None, false, 1),
				(None, "audio", Some("128 kb/s".into()), None, true, 0),
				(
					None,
					"0.avc3",
					Some("2.0 Mb/s".into()),
					Some("2.0 Mb/s".into()),
					true,
					1
				),
				(None, "audio", Some("128 kb/s".into()), None, true, 0),
				(Some("camera"), "0.avc3", Some("1.0 Mb/s".into()), None, true, 0),
			]
		);
		assert_eq!(sources[0].framerate, Some(30.0));
		assert_eq!(tall[0].verdict, Verdict::Unhealthy);

		let local = Local::default();
		let uplink = Uplink::default();
		let mut facts = facts(&local, &uplink, &main_viewers, &main);
		facts.broadcasts.push(Seen {
			name: Some("tall"),
			viewers: &tall_viewers,
			tracks: &tall,
		});
		assert_eq!(
			causes(problems(&facts)),
			[
				"1 of 1 viewers are stalling on video avc3.42c01f 960x540.",
				"On scene `tall`, 1 of 1 viewers are stalling on video avc3.42c01f 1080x1920.",
			]
		);
	}

	/// Problems list worst first: an unreachable relay and a frozen or silent
	/// source above a slow round trip. A frozen, silent, ended, or uploaded
	/// source names the broadcasts that show or play it, including a published
	/// scene that alone plays it.
	#[test]
	fn problems_list_worst_first() {
		let program = Live {
			program: true,
			scenes: Vec::new(),
		};
		let tall = Live {
			program: false,
			scenes: vec!["tall".into()],
		};
		let both = Live {
			program: true,
			scenes: vec!["tall".into(), "wide".into()],
		};
		let reach = |shown: &Live, heard: &Live| Reach {
			shown: shown.clone(),
			heard: heard.clone(),
		};
		let local = Local {
			stalled: vec![
				("camera".into(), Stall::Picture(Duration::from_secs(3)), program.clone()),
				("logo".into(), Stall::Picture(Duration::from_secs(3)), tall.clone()),
				("mic".into(), Stall::Sound(Duration::from_secs(2)), tall.clone()),
			],
			stopped: vec![
				("intro".into(), Status::Ended, reach(&program, &program)),
				("outro".into(), Status::Ended, reach(&both, &both)),
				("clip".into(), Status::Ended, reach(&program, &tall)),
				("bed".into(), Status::Ended, reach(&Live::default(), &tall)),
				(
					"music".into(),
					Status::Failed("the file is gone".into()),
					Reach::default(),
				),
			],
			uploaded: vec![(
				"chat".into(),
				"there is no display to paint through".into(),
				tall.clone(),
			)],
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
				"Source `logo` stalled: no new picture for 3 s, so scene `tall` repeats its last one.",
				"Source `mic` stalled: no new sound for 2 s, so scene `tall` plays silence for it.",
				"Source `music` failed: the file is gone.",
				"Source `intro` played to its end, so the program holds its last picture and plays no sound for it.",
				"Source `outro` played to its end, so the program, scene `tall` and scene `wide` hold its last picture and play no sound for it.",
				"Source `clip` played to its end, so the program holds its last picture and scene `tall` plays no sound for it.",
				"Source `bed` played to its end, so scene `tall` plays no sound for it.",
				"Source `chat`, which scene `tall` shows, is uploaded from memory every frame rather than copied on the GPU: there is no display to paint through.",
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

	/// A broadcast at `path` on `origin` whose catalog lists one audio track,
	/// soliciting feedback, and that track's ID.
	fn listing(origin: &moq_net::origin::Producer, path: &str) -> (moq_scene::publish::Catalog, String, impl Sized) {
		let publication = origin.route(path, moq_net::origin::RouteOptions::hidden()).unwrap();
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let mut catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
		let config = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		let track = broadcast
			.create_track("audio", catalog.track_info(hang::catalog::PRIORITY.audio))
			.unwrap();
		let audio = catalog
			.audio(
				track,
				moq_mux::catalog::hang::Container::Legacy(moq_mux::container::Kind::Audio),
				config,
			)
			.unwrap();
		solicit(&mut catalog, &moq_net::Path::new(path).unwrap()).unwrap();
		let id = catalog.snapshot().audio.renditions.keys().next().unwrap().clone();
		(catalog, id, audio)
	}

	/// A viewer `name` reporting under `prefix` that rendition `id`'s frame at
	/// 20 s arrived 250 ms after the program drew it, by `clock`.
	fn report(
		origin: &moq_net::origin::Producer,
		prefix: &str,
		name: &str,
		id: &str,
		clock: &moq_mux::Clock,
	) -> (
		moq_net::origin::Route,
		moq_json::snapshot::Producer<hang::echo::Snapshot>,
	) {
		let viewer = origin
			.route(
				format!("{prefix}/viewers/{name}.echo"),
				moq_net::origin::RouteOptions::hidden(),
			)
			.unwrap();
		let track = viewer.broadcast().create_track(hang::echo::TRACK, None).unwrap();
		let mut reports = moq_json::snapshot::Producer::<hang::echo::Snapshot>::new(track, Default::default());
		viewer.announce().unwrap();
		let timestamp = moq_net::Timestamp::from_millis(20_000).unwrap();
		let wall = clock.wall_clock(timestamp).unwrap() + Duration::from_millis(250);
		let mut report = hang::echo::Snapshot::default();
		let mut rendition = hang::echo::Rendition::default();
		rendition.frames = 50;
		rendition.arrival = Some(hang::echo::Arrival::new(timestamp, wall));
		report.renditions.insert(id.to_string(), rendition);
		reports.update(&report).unwrap();
		(viewer, reports)
	}

	/// A viewer's feedback is read as it arrives, grades the track it
	/// reports, and times the track from the program clock to the viewer, on
	/// the broadcast it reports beneath. A viewer is forgotten
	/// when its broadcast ends, and a published scene when it is no longer
	/// graded.
	#[tokio::test(start_paused = true)]
	async fn viewer_feedback_times_each_track() {
		let _ = moq_tokio::crypto::install_default();
		let origin = moq_tokio::origin::spawn();
		let (catalog, id, _audio) = listing(&origin, "show");
		let (tall, tall_id, _tall_audio) = listing(&origin, "show/tall");
		let observer = |path: &str| Observer::new(moq_net::Path::new(path).unwrap(), None);
		let main = Broadcast::follow(observer("show"), catalog.clone(), &origin.consume()).unwrap();
		let scenes = Scenes::new(origin.consume());
		// Added back under its name before it ended, as an undo can, so the
		// one ending leaves the one added back.
		let ended = scenes.follow("tall", observer("show/tall"), tall.clone()).unwrap();
		let graded = scenes.follow("tall", observer("show/tall"), tall).unwrap();
		drop(ended);

		// Connecting nowhere, so the connection only ever redials.
		let client = moq_tokio::connect::Config::default().init(Default::default()).unwrap();
		let connection = client.connect("https://127.0.0.1:1".parse::<url::Url>().unwrap());
		let clock = catalog.clock();
		let mut sampler = Sampler::new(Some(Publisher {
			main,
			scenes,
			clock: clock.clone(),
			relays: vec![Link {
				endpoint: "https://127.0.0.1:1".into(),
				monitor: connection.monitor(),
			}],
			sources: moq_scene::publish::Publish::new(clock.clone(), moq_video::encode::Kind::Auto, Default::default())
				.unwrap(),
		}));

		let (alice, _reports) = report(&origin, "show", "alice", &id, &clock);
		let (_bob, _reports) = report(&origin, "show/tall", "bob", &tall_id, &clock);
		let timed = |broadcast: &Published| {
			broadcast
				.viewers
				.reports
				.first()
				.is_some_and(|viewer| viewer.latency.is_some())
		};
		let sampled = until(&mut sampler, |report| report.broadcasts.iter().all(timed)).await;
		let names: Vec<_> = sampled
			.broadcasts
			.iter()
			.map(|broadcast| broadcast.name.as_deref())
			.collect();
		assert_eq!(names, [None, Some("tall")]);
		for (broadcast, viewer, id) in [
			(&sampled.broadcasts[0], "alice", &id),
			(&sampled.broadcasts[1], "bob", &tall_id),
		] {
			let reported = &broadcast.viewers.reports;
			assert_eq!(reported.len(), 1, "{viewer} reports beneath its broadcast alone");
			assert_eq!(reported[0].name, viewer);
			let audio = broadcast.tracks.iter().find(|track| track.id == *id).unwrap();
			assert_eq!(audio.label, "audio opus 2 ch");
			assert_eq!(audio.viewers, 1);
			// The report carries milliseconds, so the program clock's
			// sub-millisecond wall mapping can floor it by one.
			let latency = audio.latency.unwrap();
			assert!(
				(Duration::from_millis(249)..=Duration::from_millis(250)).contains(&latency),
				"{latency:?}"
			);
			assert_eq!(reported[0].latency, Some(latency));
		}

		alice.withdraw();
		until(&mut sampler, |report| report.broadcasts[0].viewers.reports.is_empty()).await;
		drop(graded);
		let sampled = sampler.sample(kio::clock::now(), Local::default());
		assert_eq!(sampled.broadcasts.len(), 1, "a scene no longer graded is left out");
	}

	/// A viewer that restarts under a new instance is read from that
	/// instance, its reports starting afresh.
	#[tokio::test(start_paused = true)]
	async fn a_restarted_viewer_is_read_afresh() {
		let origin = moq_tokio::origin::spawn();
		let path = moq_net::Path::new("show/viewers/alice.echo").unwrap();
		let publish = || {
			let route = origin.route(&path, moq_net::origin::RouteOptions::hidden()).unwrap();
			let track = route.broadcast().create_track(hang::echo::TRACK, None).unwrap();
			let mut reports = moq_json::snapshot::Producer::<hang::echo::Snapshot>::new(track, Default::default());
			reports.update(&hang::echo::Snapshot::default()).unwrap();
			route.announce().unwrap();
			(route, reports)
		};
		let slot = Slot::default();
		let reading = tokio::spawn(read(origin.consume(), path.clone(), slot.clone()));
		let read_from = async |instance: moq_net::Instance| {
			let wait = async {
				loop {
					let read = {
						let reports = slot.lock().unwrap();
						let observer = reports.current.as_ref().map(|(report, _)| &report.observer);
						let read = observer.is_some_and(|observer| observer.instance.as_ref() == Some(&instance));
						assert!(
							!read || reports.previous.is_none(),
							"a new instance keeps no older report"
						);
						read
					};
					if read {
						return;
					}
					kio::clock::sleep(INTERVAL).await;
				}
			};
			kio::clock::timeout(Duration::from_secs(30), wait)
				.await
				.expect("the instance is read");
		};

		let (first, _first_reports) = publish();
		read_from(first.instance()).await;
		// Creating the path again supersedes the first route.
		let (second, _second_reports) = publish();
		assert_ne!(first.instance(), second.instance());
		read_from(second.instance()).await;
		reading.abort();
	}

	/// A reader stops, saying why, once its origin closes.
	#[tokio::test(start_paused = true)]
	async fn a_reader_stops_with_its_origin() {
		let (origin, driver) = moq_net::origin::Producer::new(Default::default());
		let slot = Slot::default();
		let path = moq_net::Path::new("show/viewers/alice.echo").unwrap();
		let reading = tokio::spawn(read(origin.consume(), path, slot.clone()));
		tokio::task::yield_now().await;
		drop(driver);
		kio::clock::timeout(Duration::from_secs(30), reading)
			.await
			.expect("the reader stops")
			.unwrap();
		assert!(slot.lock().unwrap().error.is_some());
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
		let show = published
			.route("show", moq_net::origin::RouteOptions::hidden())
			.unwrap();
		let track = show.broadcast().create_track("video", None).unwrap();
		show.announce().unwrap();
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
				.unwrap()
				.expect("an instance");
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
			let viewer = origin
				.route(
					format!("show/viewers/{n:03}.echo"),
					moq_net::origin::RouteOptions::hidden(),
				)
				.unwrap();
			viewer.announce().unwrap();
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
			viewers[0].withdraw();
			viewers[MAX_VIEWERS].withdraw();
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
