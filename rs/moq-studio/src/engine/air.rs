//! Going live and ending the stream.
//!
//! Off air the studio composes, previews, and records with no relay session
//! that publishes. Going live dials a session to each relay, announces the
//! program's broadcast on them, and opens its encoders; ending the stream
//! ends every track it lists, so each encoder publishes what it holds, and
//! closes the sessions once the relays have the broadcast's tail, as quitting
//! does. Each time on air is a broadcast of its own, with its own instance.
//!
//! Broadcast sources, guests, and viewers' reports arrive through sessions
//! of their own, which only subscribe, so they are seen off air too.

use super::*;

/// The longest going live waits for a relay to take the session.
const DIAL: Duration = Duration::from_secs(15);

/// Where the program goes live.
#[derive(Clone)]
pub struct Destination {
	/// The relays to publish to, the preferred first. Their clients are the
	/// show's, which outlive the stream, so going live again dials from them.
	pub relays: Vec<Relay>,
	/// The broadcast path on them.
	pub broadcast: String,
	/// Whether the program video's canvas size is copied and encoded only
	/// while someone watches it, rather than all the time. The half size
	/// always is.
	pub on_demand: bool,
}

/// Whether the program is on air.
#[derive(Clone, Debug, PartialEq)]
pub enum OnAir {
	Off,
	/// Dialing the relays and opening the encoders.
	Starting,
	Live(Live),
	/// Handing the relays the broadcast's tail.
	Ending,
}

impl OnAir {
	/// The stream on air, if it is.
	pub fn live(&self) -> Option<&Live> {
		match self {
			Self::Live(live) => Some(live),
			_ => None,
		}
	}
}

/// The stream on air.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
	/// The broadcast path on the relays.
	pub path: String,
	/// Whether the canvas size encodes only while someone watches.
	pub on_demand: bool,
	/// When it went live.
	pub since: Instant,
	/// Each relay's scheme, host, and port, the preferred first.
	pub relays: Vec<String>,
}

/// A stream that has gone off air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aired {
	/// When it went live.
	pub since: Instant,
	/// How long it was on air, until END STREAM.
	pub lasted: Duration,
}

/// The engine's stream.
pub(super) enum Slot {
	Off,
	Starting,
	Live(Box<Air>),
	Ending,
}

/// A stream on air: the sessions publishing it and what its broadcast lists
/// beside the video, which the program clock encodes ([`Lines`]).
pub(super) struct Air {
	live: Live,
	/// A session to each relay, which reconnects on its own. Their clients
	/// are the show's, which its watching sessions and the next stream share.
	connections: Vec<moq_tokio::Connection>,
	/// The program's broadcast, which closes once dropped.
	publication: moq_net::origin::Route,
	catalog: moq_scene::publish::Catalog,
	/// Ends the program audio, which then finishes its track.
	audio_stop: tokio::sync::oneshot::Sender<()>,
	audio: tokio::task::JoinHandle<()>,
	/// Its audio's claim on the connection's bandwidth, which it does not
	/// follow: Opus at this rate is never what congests the uplink. Listed
	/// under its track's name in [`Stats::grants`].
	_audio_grant: bandwidth::Reservation,
	audio_track: String,
	/// Copies the sources published independently into the broadcast.
	forward: tokio::task::JoinHandle<()>,
	/// Dropped, the copies finish their tracks and the forwarding ends.
	forward_stop: tokio::sync::watch::Sender<()>,
	/// Where the broadcasts are announced, which the sessions publish.
	_origin: moq_net::origin::Producer,
}

impl Drop for Air {
	fn drop(&mut self) {
		self.forward.abort();
		self.publication.broadcast().close();
	}
}

/// What the program clock feeds while on air.
pub(super) struct Lines {
	/// The program's.
	pub main: Line,
	/// What starts the line of each published scene.
	pub extras: Extras,
	/// The published scenes' lines.
	pub lines: super::Lines,
	/// Where and when the broadcast's viewers show the sources published
	/// independently over the program.
	pub composition: Composed,
}

impl Lines {
	/// Stop handing every line frames, so each encode thread publishes what
	/// it holds and finishes its track.
	pub fn end(self) -> Ending {
		let mut ended = vec![self.main.end(None)];
		ended.extend(
			self.lines
				.into_values()
				.flatten()
				.map(|(line, broadcast)| line.end(Some(broadcast))),
		);
		Ending {
			ended,
			composition: Some(self.composition),
		}
	}
}

/// What ending a stream leaves running, to wait on before the sessions close.
#[derive(Default)]
pub(super) struct Ending {
	pub ended: Vec<Ended>,
	/// Finished once the video it composes has ended.
	pub composition: Option<Composed>,
}

/// What the program clock takes from going live and gives back on ending,
/// at its next tick.
#[derive(Default)]
pub(super) struct Handoff {
	start: Option<Lines>,
	end: Option<tokio::sync::oneshot::Sender<Ending>>,
	/// Set once the program clock stops, after which nothing is taken.
	closed: bool,
}

impl Handoff {
	/// What to start and whether to end, this tick.
	pub fn take(&mut self) -> (Option<Lines>, Option<tokio::sync::oneshot::Sender<Ending>>) {
		(self.start.take(), self.end.take())
	}

	/// Take nothing more, handing back what was waiting.
	pub fn close(&mut self) -> (Option<Lines>, Option<tokio::sync::oneshot::Sender<Ending>>) {
		self.closed = true;
		self.take()
	}
}

/// A session to a relay that only subscribes.
pub(super) struct Watch {
	pub(super) connection: moq_tokio::Connection,
	pub(super) client: moq_tokio::Client,
	pub(super) link: health::Link,
}

/// Dial a session to each of `relays` that publishes what `origin` announces,
/// such as the broadcast at `path`, with each one's link.
pub(super) fn publish(
	relays: Vec<Relay>,
	origin: &moq_net::origin::Producer,
	path: &moq_net::Path,
) -> (Vec<moq_tokio::Connection>, Vec<health::Link>) {
	relays
		.into_iter()
		.map(|Relay { client, url }| {
			tracing::info!(url = %moq_tokio::RedactedUrl::new(&url), broadcast = %path, "going live");
			let endpoint = endpoint(&url);
			let connection = client.with_publisher(origin).connect(url);
			let monitor = connection.monitor();
			(connection, health::Link { endpoint, monitor })
		})
		.unzip()
}

/// Dial a session to each of `relays` that only subscribes, into
/// `subscribe`, the first relay's routes preferred.
pub(super) fn subscribe(relays: &[Relay], subscribe: &moq_net::origin::Producer) -> Vec<Watch> {
	relays
		.iter()
		.enumerate()
		.map(|(rank, relay)| {
			// The first relay's route to a broadcast wins while it has one,
			// whether it carries the same instance as the failover's or another.
			let preference = match rank {
				0 => moq_net::origin::Preference::default(),
				_ => moq_net::origin::Preference::new(-1),
			};
			let subscribe = subscribe.clone().with_preference(preference);
			let connection = relay
				.client
				.clone()
				.with_subscriber(subscribe)
				.connect(relay.url.clone());
			let link = health::Link {
				endpoint: endpoint(&relay.url),
				monitor: connection.monitor(),
			};
			Watch {
				connection,
				client: relay.client.clone(),
				link,
			}
		})
		.collect()
}

/// Close each of `connections` once its relay has what it was sent, bounded
/// at [`CLOSE`]. Their clients stay open: the show's watching sessions share
/// them, and its endpoint, still running, sends each close to the relay.
pub(super) async fn close(connections: &[moq_tokio::Connection]) -> anyhow::Result<()> {
	let mut closing = tokio::task::JoinSet::new();
	for connection in connections {
		closing.spawn(connection.clone().close());
	}
	let drained = kio::clock::timeout(CLOSE, closing.join_all())
		.await
		.map_err(|_| anyhow::anyhow!("the relays did not take the stream's end within {CLOSE:?}"))?;
	for drained in drained {
		drained.context("a relay session closed without taking the stream's end")?;
	}
	Ok(())
}

impl Engine {
	/// Whether the program is on air.
	pub fn on_air(&self) -> OnAir {
		self.on_air.borrow().clone()
	}

	/// The last stream to go off air, kept as it ends rather than as the
	/// window sees it, since hotkeys go live and end while it draws nothing.
	pub fn aired(&self) -> Option<Aired> {
		*self.aired.lock().unwrap()
	}

	/// Whether the program is on air, as it changes.
	pub fn on_air_changes(&self) -> tokio::sync::watch::Receiver<OnAir> {
		self.on_air.subscribe()
	}

	/// The GPU the program is drawn on, which going live's encoder check
	/// times encoders on.
	pub fn gpu(&self) -> &Gpu {
		&self.gpu
	}

	/// Move the stream to `slot`, and say so.
	fn set_air(&self, slot: &mut Slot, to: Slot) {
		let on_air = match &to {
			Slot::Off => OnAir::Off,
			Slot::Starting => OnAir::Starting,
			Slot::Live(air) => OnAir::Live(air.live.clone()),
			Slot::Ending => OnAir::Ending,
		};
		*slot = to;
		self.on_air.send_replace(on_air);
	}

	/// Go live to `destination`: dial each relay, failing with why when none
	/// takes the session, then open the program's encoders and announce its
	/// broadcast, and each published scene's beneath it. The program audio
	/// follows the audio scene on program, and guests' return feeds show the
	/// program from now on.
	///
	/// Call inside the tokio runtime that serves the relay connections.
	pub async fn go_live(&self, destination: Destination) -> anyhow::Result<()> {
		{
			let mut slot = self.air.lock().unwrap();
			match &*slot {
				Slot::Off => {}
				Slot::Starting => anyhow::bail!("already going live"),
				Slot::Live(_) => anyhow::bail!("already live"),
				Slot::Ending => anyhow::bail!("the last stream is still ending"),
			}
			anyhow::ensure!(!*self.stopped.borrow(), "the studio is quitting");
			self.set_air(&mut slot, Slot::Starting);
		}
		let lifted = self.lift(destination).await;
		let mut slot = self.air.lock().unwrap();
		match lifted {
			Ok(air) => {
				self.set_air(&mut slot, Slot::Live(Box::new(air)));
				Ok(())
			}
			Err(err) => {
				self.set_air(&mut slot, Slot::Off);
				Err(err)
			}
		}
	}

	/// Dial `destination`'s relays and start the stream on them.
	async fn lift(&self, destination: Destination) -> anyhow::Result<Air> {
		let Destination {
			relays,
			broadcast,
			on_demand,
		} = destination;
		anyhow::ensure!(!relays.is_empty(), "no relay to go live on");
		let path =
			moq_net::Path::new(broadcast.as_str()).with_context(|| format!("cannot publish at `{broadcast}`"))?;
		let origin = moq_tokio::origin::spawn();
		let (connections, links) = publish(relays, &origin, &path);
		dial(&connections, &links).await?;

		// Each track reserves its ceiling on the first relay's estimate, which
		// that connection then probes to fund: the bandwidth the stream is
		// granted. An allocator spans one connection, so the failover's link is
		// never probed. The catalog claims nothing itself, which would count
		// each track twice.
		let bandwidth = connections[0].allocator();
		let publication = origin
			.route(&path, moq_net::origin::RouteOptions::hidden())
			.context("cannot create the broadcast")?;
		let mut broadcast = publication.broadcast().clone();
		let clock = self.program.clock.clone();
		let config = moq_mux::catalog::Config::default()
			.with_catalog(Default::default())
			.with_clock(clock.clone());
		let catalog = moq_scene::publish::Catalog::new(&mut broadcast, config)?;
		// Held until the video and audio are listed, so the first catalog
		// carries both and the solicitation.
		let reserved = catalog.reserve();
		// Viewers report back beneath the broadcast.
		health::solicit(&mut catalog.clone(), &path)?;
		let renditions = Renditions {
			gpu: self.gpu.clone(),
			broadcast: broadcast.clone(),
			timebase: catalog.timebase(),
			record: false,
			reclaim: self.program.reclaim.clone(),
			bandwidth: bandwidth.clone(),
			on_demand,
			live: true,
			stats: self.stats.clone(),
			video: self.stats.video.clone(),
			grants: String::new(),
		};
		// Opened before announcing, so a canvas this machine cannot encode
		// fails going live rather than going live without a picture. The
		// program clock probes again if the canvas changes meanwhile.
		let canvas = self.state.lock().unwrap().compositor.canvas();
		let ladder = renditions.open(canvas).await;
		let ladder = ladder.inspect_err(|err| self.stats.video.fail(format!("{err:#}")))?;
		let main = Line::new(renditions, canvas, Some(ladder), true);

		let (input, options) = audio_options();
		let rate = input.sample_rate;
		let mut audio = moq_audio::encode::Producer::new(&mut broadcast, catalog.clone(), input, &options)?;
		let audio_grant = bandwidth.reserve(&audio.demand(), AUDIO_BITRATE);
		let audio_track = audio.track_name().to_string();
		self.stats
			.grants
			.lock()
			.unwrap()
			.insert(audio_track.clone(), audio_grant.consume());
		drop(reserved);

		// One instance on every relay, so a viewer moving between them resumes
		// mid-group rather than restarting.
		let instance = publication.instance();
		if let Err(err) = publication.announce() {
			// Taken back, as ending does, before saying why.
			self.stats.grants.lock().unwrap().remove(&audio_track);
			drop(main.end(None));
			return Err(err).context("cannot announce the broadcast");
		}

		let composition = match Composed::new(&origin, &path, instance.clone(), clock.clone(), catalog.clone()) {
			Ok(composition) => composition,
			Err(err) => {
				self.stats.grants.lock().unwrap().remove(&audio_track);
				drop(main.end(None));
				return Err(err);
			}
		};
		let (forward_stop, stop) = tokio::sync::watch::channel(());
		let forward = {
			let sources = self.program.sources.clone();
			let forward = moq_scene::publish::Forward {
				published: self.program.published.subscribe(),
				origin: origin.clone(),
				base: composition.sources.clone(),
				bandwidth: bandwidth.clone(),
				stop,
			};
			tokio::spawn(async move {
				if let Err(err) = sources.forward(forward).await {
					tracing::warn!(%err, "stopped publishing sources independently");
				}
			})
		};
		let reports = self.subscribe.consume();
		let observer = hang::health::Observer::new(path.clone(), Some(instance));
		let publisher = health::Publisher {
			main: health::Broadcast::follow(observer, catalog.clone(), &reports)?,
			scenes: health::Scenes::new(reports),
			clock: clock.clone(),
			relays: links.clone(),
			sources: self.program.sources.clone(),
		};
		let extras = Extras {
			origin: origin.clone(),
			path: path.clone(),
			clock,
			epoch: self.program.epoch,
			runtime: tokio::runtime::Handle::current(),
			gpu: self.gpu.clone(),
			bandwidth,
			on_demand,
			live: true,
			stats: self.stats.clone(),
			reclaim: self.program.reclaim.clone(),
			graded: Some(publisher.scenes.clone()),
		};
		// The audio scene on program, encoded on the runtime, never the
		// program clock's thread.
		let mut follow = self.program_audio.follow();
		// A stream before this one whose audio failed is not this one's.
		*self.stats.audio_error.lock().unwrap() = None;
		let (audio_stop, stopped) = tokio::sync::oneshot::channel::<()>();
		let (stats, epoch) = (self.stats.clone(), self.program.epoch);
		let audio = tokio::spawn(async move {
			let encoded = tokio::select! {
				encoded = mix::encode(&mut follow, &mut audio, epoch, rate) => encoded,
				_ = stopped => Ok(()),
			};
			match encoded {
				// Fails only once the broadcast closed, which ended the track.
				Ok(()) => drop(audio.finish()),
				Err(err) => {
					stats.fail_audio(format!("{err:#}"));
					// Readers see the audio fail rather than go away unfinished.
					audio.abort(moq_net::StreamError::Internal.into());
				}
			}
		});

		let lines = Lines {
			main,
			extras,
			lines: super::Lines::new(),
			composition,
		};
		{
			let mut handoff = self.handoff.lock().unwrap();
			if handoff.closed {
				drop(handoff);
				// The program clock stopped meanwhile, so nothing will feed them.
				let ending = lines.end();
				for ended in ending.ended {
					ended.join();
				}
				anyhow::bail!("the studio is quitting");
			}
			handoff.start = Some(lines);
		}
		self.on_air_program
			.send_replace(Some(publication.broadcast().consume()));
		*self.sampler.lock().unwrap() = health::Sampler::new(Some(publisher));

		Ok(Air {
			live: Live {
				path: path.to_string(),
				on_demand,
				since: kio::clock::now(),
				relays: links.into_iter().map(|link| link.endpoint).collect(),
			},
			connections,
			publication,
			catalog,
			audio_stop,
			audio,
			_audio_grant: audio_grant,
			audio_track,
			forward,
			forward_stop,
			_origin: origin,
		})
	}

	/// End the stream: each encoder publishes what it holds and finishes its
	/// track, the broadcast's catalog finishes, and each relay session closes
	/// once the relay has received what it was sent, so a viewer or a
	/// recording on the relay reaches the end rather than a cut. Bounded at
	/// [`CLOSE`] for the sessions. Fails when nothing is on air.
	pub async fn end_stream(&self) -> anyhow::Result<()> {
		let air = {
			let mut slot = self.air.lock().unwrap();
			let air = match std::mem::replace(&mut *slot, Slot::Off) {
				Slot::Live(air) => air,
				other => {
					let why = match &other {
						Slot::Off => "not live",
						Slot::Starting => "still going live",
						Slot::Live(_) => unreachable!("matched above"),
						Slot::Ending => "already ending",
					};
					*slot = other;
					anyhow::bail!(why);
				}
			};
			self.set_air(&mut slot, Slot::Ending);
			*self.aired.lock().unwrap() = Some(Aired {
				since: air.live.since,
				lasted: air.live.since.elapsed(),
			});
			air
		};
		let ending = {
			let mut handoff = self.handoff.lock().unwrap();
			match handoff.closed {
				// The program clock ended the encoders as it stopped.
				true => None,
				false => {
					let (reply, ending) = tokio::sync::oneshot::channel();
					handoff.end = Some(reply);
					Some(ending)
				}
			}
		};
		let ending = match ending {
			// The program clock stopping meanwhile drops the reply, having
			// ended the encoders itself.
			Some(ending) => ending.await.unwrap_or_default(),
			None => Ending::default(),
		};
		let landed = self.land(*air, ending).await;
		let mut slot = self.air.lock().unwrap();
		self.set_air(&mut slot, Slot::Off);
		landed
	}

	/// Finish `air` once its encoders, `ending`, have, then close its
	/// sessions.
	async fn land(&self, mut air: Air, ending: Ending) -> anyhow::Result<()> {
		let Ending { ended, composition } = ending;
		let joined = tokio::task::spawn_blocking(move || {
			for ended in ended {
				ended.join();
			}
		});
		if joined.await.is_err() {
			tracing::error!("waiting on the encoders panicked");
		}
		if let Some(mut composition) = composition {
			composition.finish();
		}
		// Dropped, so the audio finishes its track.
		let (stop, _) = tokio::sync::oneshot::channel();
		drop(std::mem::replace(&mut air.audio_stop, stop));
		if (&mut air.audio).await.is_err() {
			tracing::error!("the program audio panicked");
		}
		// Dropped, so each source's copied track finishes after what it has.
		drop(std::mem::replace(
			&mut air.forward_stop,
			tokio::sync::watch::channel(()).0,
		));
		if (&mut air.forward).await.is_err() {
			tracing::error!("publishing the sources independently panicked");
		}
		self.stats.grants.lock().unwrap().remove(&air.audio_track);
		// Fails only once the catalog failed, which ended its tracks already.
		let _ = air.catalog.clone().finish();
		self.on_air_program.send_replace(None);
		*self.sampler.lock().unwrap() = health::Sampler::new(None);

		close(&air.connections).await
	}

	/// What a replay of the program could copy: its video at each size,
	/// largest first, with its audio, up to now. It has a past only on air,
	/// as the relay's copy of it.
	pub fn program_targets(&self) -> anyhow::Result<Vec<moq_scene::replay::Target>> {
		let slot = self.air.lock().unwrap();
		let Slot::Live(air) = &*slot else {
			anyhow::bail!("the program has a past to replay only while on air");
		};
		let catalog = air.catalog.snapshot();
		let broadcast = air.publication.broadcast().consume();
		let end = self.program.clock.now();
		let target = |video, audio| moq_scene::replay::Target {
			broadcast: broadcast.clone(),
			video,
			audio,
			end,
		};
		let audio = catalog.audio.renditions.into_iter().next();
		let mut video: Vec<_> = catalog.video.renditions.into_iter().collect();
		video.sort_by_key(|(_, config)| {
			let pixels = u64::from(config.coded_width.unwrap_or(0)) * u64::from(config.coded_height.unwrap_or(0));
			std::cmp::Reverse(pixels)
		});
		Ok(match video.is_empty() {
			true => vec![target(None, audio)],
			false => video
				.into_iter()
				.map(|video| target(Some(video), audio.clone()))
				.collect(),
		})
	}

	/// Subscribe to broadcast sources, guests, and viewers' reports through
	/// `relays`, the preferred first, instead of those followed before. Not
	/// once the engine stops, as a follow still under way at a close would.
	pub fn watch(&self, relays: &[Relay]) {
		// Under the lock `unwatch` takes, which a close does once `stop` has
		// run, so these are either closed with the rest or never dialed.
		let mut current = self.watching.lock().unwrap();
		if *self.stopped.borrow() {
			return;
		}
		let watching = subscribe(relays, &self.subscribe);
		// Each dropped stops dialing, and its session goes.
		drop(std::mem::replace(&mut *current, watching));
	}

	/// Each relay [watched](Self::watch), with its state and round trip.
	pub fn watched(&self) -> Vec<(health::Relay, Option<Duration>)> {
		let watching = self.watching.lock().unwrap();
		watching
			.iter()
			.map(|watch| {
				let rtt = watch.link.monitor.stats().and_then(|stats| stats.rtt);
				(health::relay(&watch.link), rtt)
			})
			.collect()
	}

	/// Close each watching session, so its relay hears the close.
	pub(super) async fn unwatch(&self) {
		let watching = std::mem::take(&mut *self.watching.lock().unwrap());
		let mut closing = tokio::task::JoinSet::new();
		for Watch { connection, client, .. } in watching {
			closing.spawn(async move {
				// A session that only subscribes has nothing to drain.
				let _ = connection.close().await;
				client.close().await;
			});
		}
		closing.join_all().await;
	}
}

/// Wait until any of `connections` has a session, or fail with why the first
/// one could not, within [`DIAL`]. A connection keeps redialing until its
/// loop gives up, so a relay whose first dial fails may still answer in time.
async fn dial(connections: &[moq_tokio::Connection], links: &[health::Link]) -> anyhow::Result<()> {
	let mut dialing: tokio::task::JoinSet<(usize, anyhow::Result<()>)> = tokio::task::JoinSet::new();
	for (index, connection) in connections.iter().enumerate() {
		let connection = connection.clone();
		dialing.spawn(async move {
			let established = connection.established().await.map(drop).map_err(anyhow::Error::from);
			(index, established)
		});
	}
	let mut failures: Vec<(usize, anyhow::Error)> = Vec::new();
	let first = async {
		while let Some(joined) = dialing.join_next().await {
			match joined {
				Ok((_, Ok(()))) => return true,
				Ok((index, Err(err))) => failures.push((index, err)),
				Err(err) => failures.push((usize::MAX, anyhow::anyhow!("the dial panicked: {err}"))),
			}
		}
		false
	};
	let connected = kio::clock::timeout(DIAL, first).await;
	match connected {
		Ok(true) => Ok(()),
		Ok(false) => {
			failures.sort_by_key(|(index, _)| *index);
			let (index, err) = failures.into_iter().next().expect("every dial failed");
			let endpoint = links.get(index).map_or("the relay", |link| link.endpoint.as_str());
			Err(err.context(format!("cannot reach {endpoint}")))
		}
		Err(_) => {
			// Each relay, and its latest failed dial where one failed rather
			// than hung. A relay whose loop gave up, such as on an auth
			// rejection, is closed, so its monitor no longer says why; its
			// dial did.
			let relays: Vec<String> = connections
				.iter()
				.zip(links)
				.enumerate()
				.map(|(index, (connection, link))| {
					let gave_up = failures.iter().find(|(failed, _)| *failed == index);
					match (gave_up, connection.monitor().failure()) {
						(Some((_, err)), _) => format!("{}: {err:#}", link.endpoint),
						(None, Some(err)) => format!("{}: {err}", link.endpoint),
						(None, None) => format!("{}: no answer", link.endpoint),
					}
				})
				.collect();
			anyhow::bail!(
				"no relay took the session within {} s ({})",
				DIAL.as_secs(),
				relays.join("; ")
			)
		}
	}
}

/// The relay at `url` as the window names it: only the scheme, host, and
/// port, since a path can carry a credential, and the window may be on a
/// shared screen. A URL without a host, such as a unix socket's, keeps its
/// path, which is the address.
pub fn endpoint(url: &url::Url) -> String {
	match url.host_str() {
		Some(host) => match url.port() {
			Some(port) => format!("{}://{host}:{port}", url.scheme()),
			None => format!("{}://{host}", url.scheme()),
		},
		None => format!("{}:{}", url.scheme(), url.path()),
	}
}

/// The composition's broadcast, announced beneath the program's, and where it
/// finds what it lists.
pub(super) struct Composed {
	/// The composition's broadcast, which closes once dropped.
	route: moq_net::origin::Route,
	publisher: moq_scene::composition::Publisher,
	/// The composition's path, which each child's is relative to.
	path: moq_net::Path,
	/// The program's path and stream.
	program: moq_net::Path,
	instance: moq_net::Instance,
	/// The program's catalog, whose renditions the composition's lists.
	catalog: moq_scene::publish::Catalog,
	/// Where each local source published independently has its broadcast.
	sources: moq_net::Path,
}

impl Composed {
	/// Announce the composition beneath the program's broadcast at `program`,
	/// the stream `instance`, on `origin`, its catalog on the program's
	/// `clock` over the renditions the program's `catalog` lists.
	fn new(
		origin: &moq_net::origin::Producer,
		program: &moq_net::Path,
		instance: moq_net::Instance,
		clock: moq_mux::Clock,
		catalog: moq_scene::publish::Catalog,
	) -> anyhow::Result<Self> {
		let below = |name: &'static str| program.join(&moq_net::Path::from_static(name));
		let path = below(moq_scene::composition::PATH)?;
		let route = origin
			.route(&path, moq_net::origin::RouteOptions::hidden())
			.context("cannot create the composition's broadcast")?;
		let publisher = moq_scene::composition::Publisher::new(&mut route.broadcast().clone(), clock)?;
		route.announce().context("cannot announce the composition")?;
		Ok(Self {
			route,
			publisher,
			path,
			program: program.clone(),
			instance,
			catalog,
			sources: below(moq_scene::composition::SOURCES)?,
		})
	}

	/// Where the source called `name` is, relative to the composition: the
	/// broadcast it plays at `path` for a broadcast source, else the local
	/// one it is published to. `None` for a path no reference can name.
	fn child(&self, name: &str, path: Option<&str>) -> Option<moq_net::path::Relative> {
		let path = match path {
			Some(path) => moq_net::Path::new(path).ok()?,
			None => self.sources.join(&moq_net::Path::new(name).ok()?).ok()?,
		};
		path.relative(&self.path)
	}

	/// Publish the composition of what `state` drew on program at `at` on the
	/// media clock, at `now`: the program, and each source published
	/// independently, a broadcast source while it shows its stream, where
	/// what it showed lands on the program's clock.
	pub(super) fn compose(&mut self, state: &State, at: Duration, now: Instant) {
		// Past what a timestamp holds, the frame goes unstamped too.
		let Ok(stamp) = moq_net::Timestamp::try_from(at) else {
			return;
		};
		let collection = &state.collection;
		let mut children = BTreeMap::new();
		for name in &collection.published {
			let child = match collection.sources.get(name) {
				Some(moq_scene::Source::Broadcast { path }) => {
					// While it shows nothing of a stream, such as offline, the
					// studio shows nothing of it, so viewers drop it too.
					let Some(feed) = state.sources.get(name) else { continue };
					let Some(landing) = feed.lands(at, now) else { continue };
					let Some(broadcast) = self.child(name, Some(path)) else {
						continue;
					};
					moq_scene::composition::Child::new(broadcast, landing.instance, landing.offset)
				}
				Some(_) => match self.child(name, None) {
					Some(broadcast) => moq_scene::composition::Child::new(broadcast, None, 0),
					None => continue,
				},
				None => continue,
			};
			children.insert(name.clone(), child);
		}
		let program = moq_scene::composition::Program {
			broadcast: self.program.relative(&self.path).expect("the program is above it"),
			instance: Some(self.instance.clone()),
		};
		// To the whole microsecond as the frame's stamp is, rather than
		// rounded as the wire would, so a cut's composition starts on its frame.
		let start = at.saturating_sub(state.compositor.program_since()).as_micros();
		let start = Duration::from_micros(start as u64);
		let scene = state.compositor.program_scene();
		let composition = moq_scene::composition::Composition::new(collection, scene, start, program, children);
		if let Err(err) = self.publisher.update(composition, &self.catalog.snapshot(), stamp) {
			tracing::warn!(%err, "cannot publish the composition");
		}
	}

	/// End the composition, once the video it composes has ended.
	pub(super) fn finish(&mut self) {
		if let Err(err) = self.publisher.finish() {
			tracing::warn!(%err, "cannot finish the composition");
		}
	}
}

impl Drop for Composed {
	fn drop(&mut self) {
		self.route.broadcast().close();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// When no relay takes the session in time, the error names why each
	/// one did not: a relay whose loop already gave up keeps the reason it
	/// gave up with, beside the one that never answered, rather than reading
	/// as unanswered because its closed monitor no longer holds it.
	#[tokio::test(start_paused = true)]
	async fn a_relay_that_gave_up_keeps_its_reason_past_the_timeout() {
		let _ = moq_tokio::crypto::install_default();
		let client = moq_tokio::connect::Config::default()
			.init(Default::default())
			.expect("client");
		// No backend dials this scheme, so its loop gives up at once.
		let primary: url::Url = "ftp://127.0.0.1:1/".parse().unwrap();
		// Bound but never read, so the failover never answers.
		let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
		let failover: url::Url = format!("https://{}/", silent.local_addr().unwrap()).parse().unwrap();
		let relays = vec![
			Relay {
				client: client.clone(),
				url: primary,
			},
			Relay { client, url: failover },
		];
		let origin = moq_tokio::origin::spawn();
		let (connections, links) = publish(relays, &origin, &moq_net::Path::new("studio").unwrap());
		let err = dial(&connections, &links).await.expect_err("no relay answers");
		let err = format!("{err:#}");
		assert!(err.contains("no relay took the session"), "{err}");
		assert!(err.contains("ftp://127.0.0.1:1: url scheme must be"), "{err}");
		assert!(err.contains("no answer"), "the failover hung: {err}");
	}
}
