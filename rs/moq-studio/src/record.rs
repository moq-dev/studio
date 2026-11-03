//! Recording the program, and optionally each source, to disk beside any live
//! output.
//!
//! A recording reads the studio's broadcast the way a relay viewer does, so it
//! shares the live encoders and starts and stops without touching what the
//! relay serves. Each file is an MP4 that plays while it grows, so a crash
//! still leaves a playable file, and becomes a regular MP4 when the recording
//! stops.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, ready};
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_mux::catalog::{CatalogFormat, Stream};
use moq_mux::container::mp4;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;

/// How far a file may fall behind the broadcast before it skips ahead: only a
/// disk stalled this long loses media, and the studio never holds more for it.
/// A recording also starts with what the broadcast still holds from this far
/// before it started, since moq-mux reads from the oldest group within it.
pub const MAX_DELAY: Duration = Duration::from_secs(2);

/// Records the program into a directory, one recording at a time.
pub struct Recorder {
	source: moq_mux::Source,
	dir: PathBuf,
	/// Each file's name starts with this: the scene file's.
	name: String,
	runtime: tokio::runtime::Handle,
	/// The recording in progress, or the last one.
	session: Mutex<Option<Session>>,
}

struct Session {
	started: Instant,
	stop: Arc<watch::Sender<bool>>,
	done: watch::Receiver<bool>,
	files: Files,
}

/// Every file a recording has opened, in the order it opened them.
type Files = Arc<Mutex<Vec<Arc<File>>>>;

/// One file a recording writes.
struct File {
	path: PathBuf,
	written: AtomicU64,
	/// `None` while it is being written.
	finished: Mutex<Option<Finished>>,
}

/// How a file ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Finished {
	/// A regular MP4.
	Saved,
	/// Nothing reached it before the recording stopped, so it was removed.
	Empty,
	/// Writing stopped early. A file that holds anything still plays as far as
	/// it got, as a fragmented MP4.
	Failed(String),
}

/// A recording, for the Output panel.
pub struct Status {
	pub started: Instant,
	/// Asked to stop, and finishing its files.
	pub stopping: bool,
	/// Every file has finished.
	pub done: bool,
	pub files: Vec<FileStatus>,
}

pub struct FileStatus {
	pub path: PathBuf,
	/// Bytes written so far.
	pub written: u64,
	/// `None` while it is being written.
	pub finished: Option<Finished>,
}

impl Recorder {
	/// Record `source`, the studio's broadcast, into `dir`, naming each file
	/// after the `scenes` file.
	///
	/// Call inside the tokio runtime the recordings write on.
	pub fn new(source: moq_mux::Source, dir: &Path, scenes: &Path) -> anyhow::Result<Self> {
		let name = scenes
			.file_stem()
			.context("the scene file has no name")?
			.to_string_lossy()
			.into_owned();
		Ok(Self {
			source,
			dir: std::path::absolute(dir)?,
			name,
			runtime: tokio::runtime::Handle::try_current().context("not inside a tokio runtime")?,
			session: Mutex::new(None),
		})
	}

	/// The directory recordings go to.
	pub fn dir(&self) -> &Path {
		&self.dir
	}

	/// Start recording the program, and with `sources`, each source that
	/// publishes video or audio into a file of its own, unless a recording is
	/// still running.
	pub fn start(&self, sources: bool) -> anyhow::Result<()> {
		let mut session = self.session.lock().unwrap();
		anyhow::ensure!(
			session.as_ref().is_none_or(|session| *session.done.borrow()),
			"already recording"
		);

		// UTC, since the studio does not read the time zone; no colons, which
		// Windows refuses in a file name.
		let stamp = humantime::format_rfc3339_seconds(kio::clock::wall().into())
			.to_string()
			.replace(':', "-");
		let stem = format!("{} {stamp}", self.name);
		// Here rather than on the runtime, so a directory that cannot take it
		// fails the click that started it.
		let (path, file) = create(&self.dir, &stem)?;
		let program = Arc::new(File::new(path));
		let files: Files = Arc::new(Mutex::new(vec![program.clone()]));
		let stop = Arc::new(watch::channel(false).0);
		let (finished, done) = watch::channel(false);

		let run = Run {
			source: self.source.clone(),
			dir: self.dir.clone(),
			stem,
			files: files.clone(),
			stop: stop.clone(),
		};
		let file = tokio::fs::File::from_std(file);
		self.runtime.spawn(async move {
			run.record(program, file, sources).await;
			finished.send_replace(true);
		});

		*session = Some(Session {
			started: kio::clock::now(),
			stop,
			done,
			files,
		});
		Ok(())
	}

	/// Ask the recording to stop. Its files finish in the background.
	pub fn stop(&self) {
		if let Some(session) = self.session.lock().unwrap().as_ref() {
			session.stop.send_replace(true);
		}
	}

	/// Stop recording and wait until every file is finished.
	pub async fn finish(&self) {
		let mut done = {
			let session = self.session.lock().unwrap();
			let Some(session) = session.as_ref() else { return };
			session.stop.send_replace(true);
			session.done.clone()
		};
		// Fails only once the recording's task is gone, having finished.
		let _ = done.wait_for(|done| *done).await;
	}

	/// The recording in progress, or the last one, or `None` before the first.
	pub fn status(&self) -> Option<Status> {
		let session = self.session.lock().unwrap();
		let session = session.as_ref()?;
		let files = session.files.lock().unwrap();
		Some(Status {
			started: session.started,
			stopping: *session.stop.borrow(),
			done: *session.done.borrow(),
			files: files
				.iter()
				.map(|file| FileStatus {
					path: file.path.clone(),
					written: file.written.load(Ordering::Relaxed),
					finished: file.finished.lock().unwrap().clone(),
				})
				.collect(),
		})
	}
}

impl File {
	fn new(path: PathBuf) -> Self {
		Self {
			path,
			written: AtomicU64::new(0),
			finished: Mutex::new(None),
		}
	}

	fn finish(&self, result: anyhow::Result<Finished>) {
		let finished = result.unwrap_or_else(|err| {
			tracing::error!(path = %self.path.display(), error = format!("{err:#}"), "recording failed");
			Finished::Failed(format!("{err:#}"))
		});
		*self.finished.lock().unwrap() = Some(finished);
	}
}

/// One recording, running on the runtime.
struct Run {
	source: moq_mux::Source,
	dir: PathBuf,
	/// Each file's name starts with this.
	stem: String,
	files: Files,
	stop: Arc<watch::Sender<bool>>,
}

impl Run {
	/// Record the program into `file`, and with `sources` each source too,
	/// until asked to stop or the program ends, and finish every file.
	async fn record(self, program: Arc<File>, file: tokio::fs::File, sources: bool) {
		let mut writers = tokio::task::JoinSet::new();
		let source = self.source.clone();
		let asked = self.stop.subscribe();
		writers.spawn(async move {
			program.finish(write(source, file, &program, asked).await);
			None
		});
		if sources && let Err(err) = self.sources(&mut writers).await {
			tracing::error!(error = format!("{err:#}"), "cannot record the sources");
			self.stop.send_replace(true);
		}
		while writers.join_next().await.is_some() {}
	}

	/// Give each source that publishes video or audio a file of its own while
	/// it is listed, until asked to stop or the program's writer ends.
	///
	/// A source removed while recording finishes its file; one added, or added
	/// back, starts another. One whose file failed while it stays listed gets
	/// no other.
	async fn sources(&self, writers: &mut tokio::task::JoinSet<Option<String>>) -> anyhow::Result<()> {
		let mut asked = self.stop.subscribe();
		let mut catalog = tokio::select! {
			catalog = self.source.catalog::<moq_scene::publish::Ext>(CatalogFormat::Hang) => catalog?,
			() = stopped(&mut asked) => return Ok(()),
		};
		let mut listed = BTreeSet::new();
		// Each source with a file being written, and whether its listing has
		// gone since that file started.
		let mut writing: BTreeMap<String, bool> = BTreeMap::new();
		// Sources whose file ended while their listing stayed, such as on a
		// failure, held back until they leave the catalog.
		let mut ended = BTreeSet::new();
		loop {
			tokio::select! {
				snapshot = catalog.next() => {
					let Some(snapshot) = snapshot? else { return Ok(()) };
					listed = snapshot
						.ext
						.studio
						.sources
						.into_iter()
						.filter(|(_, listing)| !listing.video.renditions.is_empty() || !listing.audio.renditions.is_empty())
						.map(|(name, _)| name)
						.collect();
					ended.retain(|name| listed.contains(name));
					for (name, gone) in &mut writing {
						*gone |= !listed.contains(name);
					}
				}
				joined = writers.join_next() => match joined.context("no writer left")?? {
					// A listing that went away and came back is recorded afresh.
					Some(name) => {
						let gone = writing.remove(&name).unwrap_or_default();
						if !gone && listed.contains(&name) {
							ended.insert(name);
						}
					}
					// The program's writer ended, so the recording is over.
					None => {
						self.stop.send_replace(true);
						return Ok(());
					}
				},
				() = stopped(&mut asked) => return Ok(()),
			}
			let start: Vec<String> = listed
				.iter()
				.filter(|name| !writing.contains_key(*name) && !ended.contains(*name))
				.cloned()
				.collect();
			for name in start {
				self.source_file(&name, writers);
				writing.insert(name, false);
			}
		}
	}

	/// Start recording the source `name` into a file of its own.
	fn source_file(&self, name: &str, writers: &mut tokio::task::JoinSet<Option<String>>) {
		// The scene file names sources, and a name may hold a path separator.
		let safe: String = name
			.chars()
			.map(|c| match c {
				c if c.is_alphanumeric() || " -_.()".contains(c) => c,
				_ => '_',
			})
			.collect();
		let stem = format!("{} {safe}", self.stem);
		let (path, file) = match create(&self.dir, &stem) {
			Ok(created) => created,
			Err(err) => {
				let entry = File::new(self.dir.join(format!("{stem}.mp4")));
				entry.finish(Err(err));
				self.files.lock().unwrap().push(Arc::new(entry));
				return;
			}
		};
		let entry = Arc::new(File::new(path));
		self.files.lock().unwrap().push(entry.clone());
		let (source, name) = (self.source.clone(), name.to_string());
		let asked = self.stop.subscribe();
		let file = tokio::fs::File::from_std(file);
		writers.spawn(async move {
			entry.finish(write_source(source, &name, file, &entry, asked).await);
			Some(name)
		});
	}
}

/// Create a file named `stem` in `dir`, numbering it rather than replacing
/// one already there.
fn create(dir: &Path, stem: &str) -> anyhow::Result<(PathBuf, std::fs::File)> {
	let mut number = 1;
	loop {
		let name = match number {
			1 => format!("{stem}.mp4"),
			_ => format!("{stem} ({number}).mp4"),
		};
		let path = dir.join(name);
		match std::fs::File::create_new(&path) {
			Ok(file) => return Ok((path, file)),
			Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => number += 1,
			Err(err) => return Err(err).with_context(|| format!("cannot create {}", path.display())),
		}
	}
}

/// Record the program from `source` into `file` until it ends or `stop` is
/// set, then finish the file with everything it received.
async fn write(
	source: moq_mux::Source,
	file: tokio::fs::File,
	entry: &File,
	mut stop: watch::Receiver<bool>,
) -> anyhow::Result<Finished> {
	let catalog = tokio::select! {
		catalog = source.catalog::<()>(CatalogFormat::Hang) => catalog.context("the program's catalog")?,
		() = stopped(&mut stop) => return remove(file, entry).await,
	};
	record(mp4::Export::new(source, catalog), file, entry, stop).await
}

/// Record the source `name`'s own tracks from `source` into `file` until it
/// leaves the catalog or `stop` is set, then finish the file.
async fn write_source(
	source: moq_mux::Source,
	name: &str,
	file: tokio::fs::File,
	entry: &File,
	mut stop: watch::Receiver<bool>,
) -> anyhow::Result<Finished> {
	let catalog = tokio::select! {
		catalog = source.catalog::<moq_scene::publish::Ext>(CatalogFormat::Hang) => catalog.context("the studio's catalog")?,
		() = stopped(&mut stop) => return remove(file, entry).await,
	};
	let listing = Listing {
		catalog,
		name: name.to_string(),
		listed: false,
	};
	record(mp4::Export::new(source, listing), file, entry, stop).await
}

/// Write `export` into `file` until it ends or `stop` is set, then finish it.
///
/// A failed export still finishes what it recorded; a failed write leaves the
/// file as it is.
async fn record<S: Stream>(
	export: mp4::Export<S>,
	mut file: tokio::fs::File,
	entry: &File,
	mut stop: watch::Receiver<bool>,
) -> anyhow::Result<Finished> {
	let mut export = export.with_max_delay(MAX_DELAY);
	let mut stopping = false;
	let ended = loop {
		let next = tokio::select! {
			next = export.next() => next,
			// What was read is still written.
			() = stopped(&mut stop), if !stopping => {
				export.stop();
				stopping = true;
				continue;
			}
		};
		match next {
			Ok(Some(data)) => {
				file.write_all(&data).await.context("writing the recording")?;
				entry.written.fetch_add(data.len() as u64, Ordering::Relaxed);
			}
			Ok(None) => break Ok(()),
			Err(err) => break Err(err),
		}
	};

	let Some(finish) = export.finish().context("finishing the recording")? else {
		return remove(file, entry).await;
	};
	// The moov where the media ends, then the mdat header over the reserved box,
	// which points readers past the media at the moov, so the moov must be on
	// disk first.
	file.set_len(finish.moov_offset).await?;
	file.seek(std::io::SeekFrom::Start(finish.moov_offset)).await?;
	file.write_all(&finish.moov).await?;
	file.sync_data().await?;
	file.seek(std::io::SeekFrom::Start(finish.mdat_offset)).await?;
	file.write_all(&finish.mdat).await?;
	file.sync_all().await?;
	entry
		.written
		.store(finish.moov_offset + finish.moov.len() as u64, Ordering::Relaxed);

	ended.context("reading the broadcast")?;
	Ok(Finished::Saved)
}

/// Wait until the recording is asked to stop, or its recorder is gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
	let _ = stop.wait_for(|stop| *stop).await;
}

/// Remove a file nothing was recorded into.
async fn remove(file: tokio::fs::File, entry: &File) -> anyhow::Result<Finished> {
	drop(file);
	tokio::fs::remove_file(&entry.path).await?;
	Ok(Finished::Empty)
}

/// One source's own catalog, nested in the studio's, which ends once the
/// source leaves it.
struct Listing {
	catalog: moq_mux::catalog::Consumer<moq_scene::publish::Ext>,
	name: String,
	/// Whether the source was listed yet.
	listed: bool,
}

impl Stream for Listing {
	type Ext = ();

	fn poll_next(&mut self, waiter: &kio::Waiter) -> Poll<moq_mux::Result<Option<hang::Catalog>>> {
		loop {
			let Some(studio) = ready!(self.catalog.poll_next(waiter))? else {
				return Poll::Ready(Ok(None));
			};
			match studio.ext.studio.sources.get(&self.name) {
				Some(listing) => {
					self.listed = true;
					return Poll::Ready(Ok(Some(listing.clone())));
				}
				None if self.listed => return Poll::Ready(Ok(None)),
				None => {}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use bytes::Bytes;
	use moq_mux::container::fmp4::mp4_atom::{self, DecodeMaybe};

	use super::*;

	const RATE: u32 = 48_000;
	/// One write's worth of audio, as the program's audio thread hands over.
	const PERIOD: Duration = Duration::from_millis(10);

	/// A tone, published as an Opus track in `broadcast` and listed in `catalog`.
	struct Tone {
		audio: moq_audio::encode::Producer,
	}

	impl Tone {
		fn new<E: moq_mux::catalog::hang::CatalogExt>(
			broadcast: &mut moq_net::broadcast::Producer,
			catalog: moq_mux::catalog::Producer<E>,
		) -> Self {
			let mut input = moq_audio::encode::Input::new(RATE, moq_audio::Layout::Stereo);
			input.format = moq_audio::Format::F32;
			let mut options = moq_audio::encode::Options::default();
			options.config = moq_audio::encode::Config::from_input(moq_audio::encode::Codec::Opus, &input);
			let audio = moq_audio::encode::Producer::new(broadcast, catalog, input, &options).unwrap();
			Self { audio }
		}

		/// Write the 10 ms of tone starting at `at`.
		fn write(&mut self, at: Duration) {
			let frames = (u128::from(RATE) * PERIOD.as_nanos() / 1_000_000_000) as usize;
			let start = at.as_secs_f32();
			let data: Vec<u8> = (0..frames)
				.map(|frame| (start + frame as f32 / RATE as f32) * 440.0 * std::f32::consts::TAU)
				.flat_map(|phase| [phase.sin() * 0.5; 2])
				.flat_map(f32::to_le_bytes)
				.collect();
			let timestamp = moq_net::Timestamp::try_from(at).unwrap();
			self.audio
				.write(&moq_audio::Frame::new(Bytes::from(data), timestamp))
				.unwrap();
		}
	}

	/// The studio's broadcast with only program audio, which needs no GPU, and
	/// sources listed beside it as the studio lists them.
	struct Studio {
		origin: moq_net::origin::Producer,
		publication: moq_net::origin::Publication,
		catalog: moq_scene::publish::Catalog,
		program: Tone,
		/// Each listed source's tone, and the scratch broadcast its own catalog
		/// writes into.
		sources: Vec<(String, Tone, moq_net::broadcast::Producer, moq_mux::catalog::Producer)>,
		/// How much audio was written.
		played: Duration,
	}

	impl Studio {
		fn new() -> Self {
			let origin = moq_tokio::origin::spawn();
			let publication = origin.create_broadcast("studio").unwrap();
			let mut broadcast = publication.broadcast().clone();
			let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
			let catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
			let program = Tone::new(&mut broadcast, catalog.clone());
			publication.announce(Default::default()).unwrap();
			Self {
				origin,
				publication,
				catalog,
				program,
				sources: Vec::new(),
				played: Duration::ZERO,
			}
		}

		fn recorder(&self, dir: &Path) -> Recorder {
			let source = moq_mux::Source::new(self.origin.consume(), self.publication.broadcast().consume());
			Recorder::new(source, dir, Path::new("show.toml")).unwrap()
		}

		/// List a source `name` publishing a tone of its own.
		fn add(&mut self, name: &str) {
			let mut scratch = moq_net::broadcast::Info::new().produce();
			let config = moq_mux::catalog::Config::default().with_clock(self.catalog.clock());
			let listing = moq_mux::catalog::Producer::new(&mut scratch, config).unwrap();
			let mut broadcast = self.publication.broadcast().clone();
			let tone = Tone::new(&mut broadcast, listing.clone());
			self.sources.push((name.to_string(), tone, scratch, listing));
			self.relist();
		}

		/// Stop listing the source `name`.
		fn remove(&mut self, name: &str) {
			// A removed source ends its track cleanly, as the studio's do.
			for (_, tone, ..) in self.sources.iter_mut().filter(|(source, ..)| source == name) {
				tone.audio.finish().unwrap();
			}
			self.sources.retain(|(source, ..)| source != name);
			self.relist();
		}

		/// List what each source's own catalog holds, as the studio mirrors it.
		fn relist(&mut self) {
			let listings: Vec<_> = self
				.sources
				.iter()
				.map(|(name, _, _, listing)| {
					let mut listing = listing.snapshot();
					listing.clock = None;
					(name.clone(), listing)
				})
				.collect();
			self.catalog
				.mutate(|root| root.ext.studio.sources = listings.into_iter().collect())
				.unwrap();
		}

		/// Play every tone for `duration`, as the program clock would.
		async fn play(&mut self, duration: Duration) {
			let end = self.played + duration;
			while self.played < end {
				self.program.write(self.played);
				for (_, tone, ..) in &mut self.sources {
					tone.write(self.played);
				}
				self.played += PERIOD;
				kio::clock::sleep(PERIOD).await;
			}
		}
	}

	/// Every track in `path`, by handler, and its duration.
	fn tracks(path: &Path) -> Vec<(String, Duration)> {
		let data = std::fs::read(path).unwrap();
		let mut cursor = std::io::Cursor::new(data.as_slice());
		let mut kinds = Vec::new();
		let mut tracks = Vec::new();
		while let Some(atom) = mp4_atom::Any::decode_maybe(&mut cursor).unwrap() {
			kinds.push(atom.kind().to_string());
			if let mp4_atom::Any::Moov(moov) = atom {
				for trak in moov.trak {
					let mdhd = trak.mdia.mdhd;
					let duration = Duration::from_secs_f64(mdhd.duration as f64 / f64::from(mdhd.timescale));
					tracks.push((trak.mdia.hdlr.handler.to_string(), duration));
				}
			}
		}
		assert_eq!(kinds, ["ftyp", "mdat", "moov"], "a regular MP4");
		tracks
	}

	/// Whether `duration` is about `expected`: within a group either way, for
	/// where the recording joined and the fragment it finished.
	fn about(duration: Duration, expected: Duration) -> bool {
		duration.abs_diff(expected) <= Duration::from_millis(100)
	}

	/// A recording started mid-program and stopped while it plays on is a
	/// regular MP4 of what played in between, from [`MAX_DELAY`] before it
	/// started.
	#[tokio::test(start_paused = true)]
	async fn records_a_regular_mp4_until_stopped() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		let recorder = studio.recorder(dir.path());
		assert!(recorder.status().is_none());

		studio.play(Duration::from_secs(5)).await;
		recorder.start(false).unwrap();
		assert!(recorder.start(false).is_err(), "one recording at a time");
		studio.play(Duration::from_secs(1)).await;
		recorder.stop();
		let ((), ()) = tokio::join!(recorder.finish(), studio.play(Duration::from_millis(500)));

		let status = recorder.status().unwrap();
		assert!(status.stopping && status.done);
		let [file] = status.files.as_slice() else {
			panic!("{} files", status.files.len());
		};
		assert_eq!(file.finished, Some(Finished::Saved));
		let name = file.path.file_name().unwrap().to_string_lossy();
		assert!(name.starts_with("show ") && name.ends_with("Z.mp4"), "{name}");
		assert_eq!(file.written, std::fs::metadata(&file.path).unwrap().len());
		let [(handler, duration)] = tracks(&file.path).try_into().unwrap();
		assert_eq!(handler, "soun");
		assert!(about(duration, MAX_DELAY + Duration::from_secs(1)), "{duration:?}");

		// The next recording starts fresh.
		recorder.start(false).unwrap();
		assert_eq!(recorder.status().unwrap().files.len(), 1);
		recorder.finish().await;
	}

	/// Each source with media of its own records into its own file while it is
	/// listed, beside the program, and into another when listed again; a still
	/// has nothing to record.
	#[tokio::test(start_paused = true)]
	async fn sources_record_into_their_own_files() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.add("mic/1");
		studio
			.catalog
			.mutate(|root| {
				let mut still = hang::Catalog::default();
				still.binary.tracks.insert(
					"logo.png".into(),
					hang::catalog::BinaryConfig::new(hang::catalog::Mode::Snapshot),
				);
				root.ext.studio.sources.insert("logo".into(), still);
			})
			.unwrap();
		let recorder = studio.recorder(dir.path());

		recorder.start(true).unwrap();
		studio.play(Duration::from_secs(1)).await;
		studio.add("guitar");
		studio.play(Duration::from_secs(1)).await;
		// Its file finishes while the rest record on.
		studio.remove("mic/1");
		studio.play(Duration::from_secs(1)).await;
		// Added back, it records into another file.
		studio.add("mic/1");
		studio.play(Duration::from_secs(1)).await;
		recorder.finish().await;

		let status = recorder.status().unwrap();
		let files: Vec<_> = status
			.files
			.iter()
			.map(|file| {
				assert_eq!(file.finished, Some(Finished::Saved), "{}", file.path.display());
				let name = file.path.file_name().unwrap().to_string_lossy().into_owned();
				let [(handler, duration)] = tracks(&file.path).try_into().unwrap();
				assert_eq!(handler, "soun");
				(name.rsplit_once("Z").unwrap().1.to_string(), duration)
			})
			.collect();
		let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
		assert_eq!(names, [".mp4", " mic_1.mp4", " guitar.mp4", " mic_1 (2).mp4"]);
		for ((_, duration), expected) in files.iter().zip([4, 2, 3, 1]) {
			assert!(about(*duration, Duration::from_secs(expected)), "{files:?}");
		}
	}

	/// A recording of a broadcast with nothing to describe yet, stopped, leaves
	/// no file behind.
	#[tokio::test(start_paused = true)]
	async fn an_empty_recording_is_removed() {
		let dir = tempfile::tempdir().unwrap();
		let origin = moq_tokio::origin::spawn();
		let publication = origin.create_broadcast("studio").unwrap();
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let _catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
		let source = moq_mux::Source::new(origin.consume(), publication.broadcast().consume());
		let recorder = Recorder::new(source, dir.path(), Path::new("show.toml")).unwrap();
		recorder.start(true).unwrap();
		kio::clock::sleep(Duration::from_secs(1)).await;
		recorder.finish().await;

		let status = recorder.status().unwrap();
		assert_eq!(status.files.len(), 1);
		assert_eq!(status.files[0].finished, Some(Finished::Empty));
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
	}

	/// A name already taken, such as by a recording started in the same
	/// second, is numbered rather than replaced.
	#[test]
	fn a_taken_name_is_numbered() {
		let dir = tempfile::tempdir().unwrap();
		let (first, _) = create(dir.path(), "show").unwrap();
		let (second, _) = create(dir.path(), "show").unwrap();
		let (third, _) = create(dir.path(), "show").unwrap();
		assert_eq!(first, dir.path().join("show.mp4"));
		assert_eq!(second, dir.path().join("show (2).mp4"));
		assert_eq!(third, dir.path().join("show (3).mp4"));
	}

	/// Starting into a directory that does not exist fails at once.
	#[tokio::test(start_paused = true)]
	async fn a_missing_directory_fails_to_start() {
		let dir = tempfile::tempdir().unwrap();
		let studio = Studio::new();
		let recorder = studio.recorder(&dir.path().join("missing"));
		let err = recorder.start(false).unwrap_err();
		assert!(format!("{err:#}").contains("cannot create"), "{err:#}");
		assert!(recorder.status().is_none());
	}
}
