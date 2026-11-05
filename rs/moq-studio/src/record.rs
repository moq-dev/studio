//! Recording the program, and optionally each source, to disk beside any live
//! output.
//!
//! A recording reads the studio's broadcast the way a relay viewer does, so it
//! shares the live encoders and starts and stops without touching what the
//! relay serves, and each source's track from the studio's local broadcast,
//! which its reading keeps encoding. Each file is an MP4 that plays while it
//! grows, so a crash still leaves a playable file, and becomes a regular MP4
//! when the recording stops. A sidecar marks each file while it is written, so
//! the next launch finds one a crash left unfinished and repairs it (see
//! [`repair`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_mux::catalog::{CatalogFormat, Stream};
use moq_mux::container::mp4;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;

mod repair;
pub use repair::{Repair, Repaired};

/// How far a file may fall behind the broadcast before it skips ahead: only a
/// disk stalled this long loses media, and the studio never holds more for it.
/// A recording also starts with what the broadcast still holds from this far
/// before it started, since moq-mux reads from the oldest group within it.
pub const MAX_DELAY: Duration = Duration::from_secs(2);

/// Records the program into a directory, one recording at a time.
pub struct Recorder {
	program: moq_mux::Source,
	sources: moq_mux::Source,
	dir: PathBuf,
	/// Each file's name starts with this: the scene file's.
	name: String,
	runtime: tokio::runtime::Handle,
	/// The recording in progress, or the last one.
	session: Mutex<Option<Session>>,
	/// The recordings found unfinished in `dir` at launch.
	repairs: repair::Repairs,
}

struct Session {
	started: Instant,
	stop: Arc<watch::Sender<bool>>,
	/// Set by [`Recorder::close`].
	closing: kio::Producer<bool>,
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
	/// Marks the file unfinished until it is a regular MP4 or removed. A file
	/// that fails before then keeps it, so the next launch repairs it.
	marker: Mutex<Option<Marker>>,
}

/// How a file ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Finished {
	/// A regular MP4.
	Saved,
	/// Nothing reached it before the recording stopped, so it was removed.
	Empty,
	/// Writing stopped early. A file that holds anything still plays as far as
	/// it got, as a fragmented MP4 unless it finished, and one left unfinished
	/// keeps its marker, so the next launch repairs it.
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
	/// Record `program`, the studio's broadcast, and from `sources`, the local
	/// one, each source's own track, into `dir`, naming each file after the
	/// `scenes` file, and repair in the background each file in `dir` a crash
	/// left unfinished.
	///
	/// Call inside the tokio runtime the recordings write on.
	pub fn new(program: moq_mux::Source, sources: moq_mux::Source, dir: &Path, scenes: &Path) -> anyhow::Result<Self> {
		let name = scenes
			.file_stem()
			.context("the scene file has no name")?
			.to_string_lossy()
			.into_owned();
		let dir = std::path::absolute(dir)?;
		let runtime = tokio::runtime::Handle::try_current().context("not inside a tokio runtime")?;
		Ok(Self {
			program,
			sources,
			repairs: repair::Repairs::start(&dir, &runtime),
			dir,
			name,
			runtime,
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
		let stem = program_stem(&self.name, &stamp);
		// Here rather than on the runtime, so a directory that cannot take it
		// fails the click that started it.
		let (path, file, marker) = create(&self.dir, &stem)?;
		let program = Arc::new(File::new(path, Some(marker)));
		let files: Files = Arc::new(Mutex::new(vec![program.clone()]));
		let stop = Arc::new(watch::channel(false).0);
		let closing = kio::Producer::new(false);
		let (finished, done) = watch::channel(false);

		let run = Run {
			program: self.program.clone(),
			sources: self.sources.clone(),
			dir: self.dir.clone(),
			stem,
			files: files.clone(),
			stop: stop.clone(),
			closing: closing.consume(),
		};
		let file = tokio::fs::File::from_std(file);
		self.runtime.spawn(async move {
			run.record(program, file, sources).await;
			finished.send_replace(true);
		});

		*session = Some(Session {
			started: kio::clock::now(),
			stop,
			closing,
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
	#[cfg(test)]
	pub async fn finish(&self) {
		self.stop();
		self.done().await;
	}

	/// Let the recording read the program until the program's own tracks end,
	/// as stopping the studio ends them, and then finish, rather than stop where
	/// it is or go on in another file. So a recording still running when the
	/// studio quits holds what the encoders publish on the way out.
	pub fn close(&self) {
		if let Some(session) = self.session.lock().unwrap().as_ref() {
			// Fails only once the recording has finished.
			if let Ok(mut closing) = session.closing.write() {
				*closing = true;
			}
		}
	}

	/// Wait until the recording, if any, has finished every file.
	pub async fn done(&self) {
		let Some(mut done) = self
			.session
			.lock()
			.unwrap()
			.as_ref()
			.map(|session| session.done.clone())
		else {
			return;
		};
		// Fails only once the recording's task is gone, having finished.
		let _ = done.wait_for(|done| *done).await;
	}

	/// Each recording found unfinished at launch, and how its repair went.
	pub fn repairs(&self) -> Vec<Repair> {
		self.repairs.list()
	}

	/// Wait until every repair started at launch has ended.
	#[cfg(test)]
	pub async fn repaired(&self) {
		while self.repairs().iter().any(|repair| repair.outcome.is_none()) {
			kio::clock::sleep(Duration::from_millis(10)).await;
		}
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
	fn new(path: PathBuf, marker: Option<Marker>) -> Self {
		Self {
			path,
			written: AtomicU64::new(0),
			finished: Mutex::new(None),
			marker: Mutex::new(marker),
		}
	}

	/// The file is a regular MP4, or removed, so nothing is left to repair.
	fn settle(&self) {
		if let Some(marker) = self.marker.lock().unwrap().take() {
			marker.clear();
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
	program: moq_mux::Source,
	sources: moq_mux::Source,
	dir: PathBuf,
	/// Each file's name starts with this.
	stem: String,
	files: Files,
	stop: Arc<watch::Sender<bool>>,
	/// The recording reads the program to its end; see [`Recorder::close`].
	closing: kio::Consumer<bool>,
}

/// A recording's file writers, each ending with the file it wrote.
type Writers = tokio::task::JoinSet<Ended>;

/// A file that ended.
struct Ended {
	/// The source whose file it was, or `None` for the program's.
	source: Option<String>,
	/// Its tracks were replaced, so it goes on in another file.
	replaced: bool,
}

impl Run {
	/// Record the program into `file`, and with `sources` each source too,
	/// until asked to stop or the program ends, and finish every file.
	async fn record(self, program: Arc<File>, file: tokio::fs::File, sources: bool) {
		let mut writers = tokio::task::JoinSet::new();
		self.spawn(None, program, file, &mut writers);
		if let Err(err) = self.follow(&mut writers, sources).await {
			tracing::error!(error = format!("{err:#}"), "a recording's writer failed");
			self.stop.send_replace(true);
		}
		// Whatever ends the program's file ends the recording.
		while let Some(joined) = writers.join_next().await {
			if matches!(joined, Ok(Ended { source: None, .. })) {
				self.stop.send_replace(true);
			}
		}
	}

	/// Go on in a new file whenever the program's tracks are replaced, and with
	/// `sources`, give each source that publishes video or audio a file of its
	/// own while it is listed, until asked to stop or the program ends.
	///
	/// A source removed while recording finishes its file; one added, or added
	/// back, starts another, as does one whose tracks were replaced. One whose
	/// file failed while it stays listed gets no other.
	async fn follow(&self, writers: &mut Writers, sources: bool) -> anyhow::Result<()> {
		let mut asked = self.stop.subscribe();
		// A catalog that fails ends following it, without stopping the files:
		// each reads the same catalog, and its outcome says why.
		let mut catalog = tokio::select! {
			catalog = self.sources.catalog::<moq_scene::publish::Ext>(CatalogFormat::Hang) => match catalog {
				Ok(catalog) => catalog,
				Err(_) => return Ok(()),
			},
			() = stopped(&mut asked) => return Ok(()),
		};
		let mut listed = BTreeSet::new();
		// Each source with a file being written, and whether its listing has
		// gone since that file started.
		let mut writing: BTreeMap<String, bool> = BTreeMap::new();
		// Sources whose file ended, or could not be created, while their listing
		// stayed, held back until they leave the catalog.
		let mut ended = BTreeSet::new();
		loop {
			tokio::select! {
				snapshot = catalog.next() => {
					let Ok(Some(snapshot)) = snapshot else { return Ok(()) };
					if !sources {
						continue;
					}
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
					// A listing that went away and came back, or whose tracks were
					// replaced, is recorded afresh.
					Ended { source: Some(name), replaced } => {
						let gone = writing.remove(&name).unwrap_or_default();
						if !gone && !replaced && listed.contains(&name) {
							ended.insert(name);
						}
					}
					// A new canvas size or frame rate moves the program video to a
					// new track, which goes on in the next file.
					Ended { source: None, replaced: true } if !*asked.borrow() && !*self.closing.read() => {
						if !self.file(None, writers) {
							self.stop.send_replace(true);
							return Ok(());
						}
					}
					// The program's writer ended, so the recording is over.
					Ended { source: None, .. } => {
						self.stop.send_replace(true);
						return Ok(());
					}
				},
				() = stopped(&mut asked) => return Ok(()),
			}
			if *asked.borrow() || *self.closing.read() {
				continue;
			}
			let start: Vec<String> = listed
				.iter()
				.filter(|name| !writing.contains_key(*name) && !ended.contains(*name))
				.cloned()
				.collect();
			for name in start {
				if self.file(Some(&name), writers) {
					writing.insert(name, false);
				} else {
					ended.insert(name);
				}
			}
		}
	}

	/// Start recording the program, or the source named, into a new file,
	/// returning whether it started rather than failing to create the file.
	fn file(&self, source: Option<&str>, writers: &mut Writers) -> bool {
		let stem = match source {
			None => self.stem.clone(),
			Some(name) => source_stem(&self.stem, name),
		};
		let (path, file, marker) = match create(&self.dir, &stem) {
			Ok(created) => created,
			Err(err) => {
				let entry = File::new(self.dir.join(format!("{stem}.mp4")), None);
				entry.finish(Err(err));
				self.files.lock().unwrap().push(Arc::new(entry));
				return false;
			}
		};
		let entry = Arc::new(File::new(path, Some(marker)));
		self.files.lock().unwrap().push(entry.clone());
		self.spawn(
			source.map(str::to_string),
			entry,
			tokio::fs::File::from_std(file),
			writers,
		);
		true
	}

	/// Write the program, or the source named, into `file`.
	fn spawn(&self, source: Option<String>, entry: Arc<File>, file: tokio::fs::File, writers: &mut Writers) {
		let broadcast = match source {
			Some(_) => self.sources.clone(),
			None => self.program.clone(),
		};
		let asked = self.stop.subscribe();
		let closing = self.closing.clone();
		writers.spawn(async move {
			let (finished, replaced) = write(broadcast, source.as_deref(), file, &entry, asked, closing).await;
			entry.finish(finished);
			Ended { source, replaced }
		});
	}
}

/// The longest file name, in bytes of UTF-8, that common file systems take.
/// Windows counts UTF-16 units instead, which are never more.
const MAX_NAME: usize = 255;

/// The most a recording's files add to a stem: the largest number [`create`]
/// gives, and the longest name a file takes from there, its repair's, which
/// is numbered too.
const SUFFIX: usize = 2 * NUMBER.len() + REPAIRED.len() + ".mp4".len();

/// The largest number a taken name is given.
const NUMBER: &str = " (4294967295)";

/// What a file's marker adds to its name. Specific to the studio, since a
/// launch removes a marker whose recording is gone.
const MARKER: &str = ".unfinished";

/// What a repaired file's stem adds to the recording's.
const REPAIRED: &str = ".repaired";

/// What a long scene file name leaves of each source's file name, in bytes,
/// so the Output panel can still tell the source files apart.
const SOURCE: usize = 64;

/// The stem of a recording's program file: the scene file's, cut to fit
/// beside a source's name, and when it started.
fn program_stem(scenes: &str, stamp: &str) -> String {
	let budget = (MAX_NAME - SUFFIX).saturating_sub(stamp.len() + 1 + SOURCE + 1);
	format!("{} {stamp}", truncate(scenes, budget))
}

/// The stem of the source `name`'s file beside the program's, `program`,
/// holding as much of its name as fits.
fn source_stem(program: &str, name: &str) -> String {
	// The scene file names sources, and a name may hold a path separator.
	let safe: String = name
		.chars()
		.map(|c| match c {
			c if c.is_alphanumeric() || " -_.()".contains(c) => c,
			_ => '_',
		})
		.collect();
	let budget = (MAX_NAME - SUFFIX).saturating_sub(program.len() + 1);
	format!("{program} {}", truncate(&safe, budget))
}

/// The longest start of `text` within `bytes`, cut between characters.
fn truncate(text: &str, bytes: usize) -> &str {
	let mut end = bytes.min(text.len());
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	&text[..end]
}

/// The name of the `number`th file named `stem`.
fn numbered(stem: &str, number: u32) -> String {
	match number {
		1 => format!("{stem}.mp4"),
		_ => format!("{stem} ({number}).mp4"),
	}
}

/// Create a file named `stem` in `dir`, numbering it rather than replacing
/// one already there, and mark it unfinished before it exists.
fn create(dir: &Path, stem: &str) -> anyhow::Result<(PathBuf, std::fs::File, Marker)> {
	let mut number: u32 = 1;
	loop {
		let path = dir.join(numbered(stem, number));
		if let Some(marker) = Marker::create(&path)? {
			match std::fs::File::create_new(&path) {
				Ok(file) => return Ok((path, file, marker)),
				Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => marker.clear(),
				Err(err) => {
					marker.clear();
					return Err(err).with_context(|| format!("cannot create {}", path.display()));
				}
			}
		}
		number = number.checked_add(1).context("every numbered name is taken")?;
	}
}

/// The sidecar `<name>.mp4.unfinished` beside a file being recorded, until the file
/// is a regular MP4 or removed. A crash leaves it, which tells the next launch
/// to repair the file. Its lock, held while recording, tells a launch apart
/// from another studio still recording into the same directory.
struct Marker {
	path: PathBuf,
	/// Holds the lock; closing it releases the lock.
	file: std::fs::File,
}

impl Marker {
	/// Mark the recording at `path`, created next, unfinished, or `None` when
	/// the name is taken: a marker is already there, or a launch claimed this
	/// one before it was locked.
	///
	/// Locked before the recording exists, so a launch that claims it first
	/// finds no recording and removes it rather than repairing a live one.
	fn create(path: &Path) -> anyhow::Result<Option<Self>> {
		let path = marker(path);
		let file = match std::fs::File::create_new(&path) {
			Ok(file) => file,
			Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
			Err(err) => return Err(err).with_context(|| format!("cannot create {}", path.display())),
		};
		match file.try_lock() {
			Ok(()) => {}
			// A launch holds it, and removes it once it finds no recording.
			Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
			// A file system without locks still marks the file; only another studio
			// recording into the same directory as it launches goes unnoticed.
			Err(std::fs::TryLockError::Error(err)) => {
				tracing::warn!(path = %path.display(), error = %err, "cannot lock a recording's marker");
			}
		}
		// A launch that held it before it was locked removed it.
		if !path.exists() {
			return Ok(None);
		}
		Ok(Some(Self { path, file }))
	}

	/// Remove the marker, and only then let go of its lock, so a launch that
	/// takes the lock next finds it gone.
	fn clear(self) {
		if let Err(err) = std::fs::remove_file(&self.path)
			&& err.kind() != std::io::ErrorKind::NotFound
		{
			// The next launch finds the file finished or gone, and removes it.
			tracing::warn!(path = %self.path.display(), error = %err, "cannot remove a recording's marker");
		}
		drop(self.file);
	}
}

/// The marker of the recording at `path`.
fn marker(path: &Path) -> PathBuf {
	let mut name = path.as_os_str().to_owned();
	name.push(MARKER);
	name.into()
}

/// Record the program's own tracks from `source`, or with `name` that
/// source's, into `file` until they leave the catalog or are replaced, or
/// `stop` is set, then finish the file. Also returns whether they were
/// replaced.
async fn write(
	source: moq_mux::Source,
	name: Option<&str>,
	file: tokio::fs::File,
	entry: &File,
	mut stop: watch::Receiver<bool>,
	closing: kio::Consumer<bool>,
) -> (anyhow::Result<Finished>, bool) {
	let catalog = tokio::select! {
		// A failure already there is reported rather than lost to the stop.
		biased;
		catalog = source.catalog::<moq_scene::publish::Ext>(CatalogFormat::Hang) => catalog.context("the studio's catalog"),
		() = stopped(&mut stop) => return (remove(file, entry).await, false),
	};
	let catalog = match catalog {
		Ok(catalog) => catalog,
		Err(err) => return (Err(err), false),
	};
	let replaced = Arc::new(AtomicBool::new(false));
	let listing = Listing {
		catalog,
		name: name.map(str::to_string),
		last: None,
		replaced: replaced.clone(),
		closing,
	};
	let finished = record(mp4::Export::new(source, listing), file, entry, stop).await;
	(finished, replaced.load(Ordering::Relaxed))
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
		let next = if !stopping && *stop.borrow() {
			// Checked on every chunk, so media that keeps arriving never holds a
			// stop off. One look first, which reports a failure already waiting
			// rather than losing it to the stop; what was read is still written.
			stopping = true;
			let next = export.poll_next(&kio::Waiter::noop());
			export.stop();
			match next {
				Poll::Ready(next) => next,
				Poll::Pending => continue,
			}
		} else {
			tokio::select! {
				next = export.next() => next,
				() = stopped(&mut stop), if !stopping => continue,
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
		// Removed either way, but a broadcast that failed says why.
		let removed = remove(file, entry).await;
		ended.context("reading the broadcast")?;
		return removed;
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
	entry.settle();
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
	entry.settle();
	Ok(Finished::Empty)
}

/// The program's own tracks in the studio's catalog, or one source's catalog
/// nested in it, which ends once the source leaves it or the tracks are
/// replaced.
///
/// A new canvas size or frame rate, or a capture that changes size or color
/// space, moves the video to a new track, which an MP4 would hold as a second
/// track that players skip, so the new one goes into another file instead.
struct Listing {
	catalog: moq_mux::catalog::Consumer<moq_scene::publish::Ext>,
	/// The source, or `None` for the program.
	name: Option<String>,
	/// The source's catalog as last recorded, once it was listed.
	last: Option<hang::Catalog>,
	/// Set as the listing ends because its tracks were replaced.
	replaced: Arc<AtomicBool>,
	/// Set by [`Recorder::close`]: the program's listing then ends once the
	/// tracks it recorded have all ended, adding and replacing none.
	closing: kio::Consumer<bool>,
}

impl Stream for Listing {
	type Ext = ();

	fn poll_next(&mut self, waiter: &kio::Waiter) -> Poll<moq_mux::Result<Option<hang::Catalog>>> {
		if self.replaced.load(Ordering::Relaxed) {
			return Poll::Ready(Ok(None));
		}
		// Polled for the program's listing, so closing wakes it.
		let closing = self.name.is_none()
			&& matches!(
				self.closing.poll(waiter, |closing| if **closing {
					Poll::Ready(())
				} else {
					Poll::Pending
				}),
				Poll::Ready(Ok(()))
			);
		loop {
			let studio = match self.catalog.poll_next(waiter) {
				Poll::Ready(studio) => studio?,
				// With nothing new, a closing program listing with nothing left to
				// read ends without waiting for another catalog update. A catalog
				// that failed is reported above instead.
				Poll::Pending => {
					let empty =
						|last: &hang::Catalog| last.video.renditions.is_empty() && last.audio.renditions.is_empty();
					if closing && self.last.as_ref().is_some_and(empty) {
						return Poll::Ready(Ok(None));
					}
					return Poll::Pending;
				}
			};
			let Some(studio) = studio else {
				return Poll::Ready(Ok(None));
			};
			let listing = match &self.name {
				None => Some(canvas(studio.media())),
				Some(name) => studio.ext.studio.sources.get(name).cloned(),
			};
			let Some(listing) = listing else {
				if self.last.is_some() {
					return Poll::Ready(Ok(None));
				}
				continue;
			};
			if closing {
				// Each track a publisher ended leaves the catalog, and the export
				// reads every one to its end before it finishes. Before the first
				// snapshot, the tracks listed are the ones left to read.
				let Some(last) = &self.last else {
					if listing.video.renditions.is_empty() && listing.audio.renditions.is_empty() {
						return Poll::Ready(Ok(None));
					}
					self.last = Some(listing.clone());
					return Poll::Ready(Ok(Some(listing)));
				};
				let listed = |track: &String| {
					listing.video.renditions.contains_key(track) || listing.audio.renditions.contains_key(track)
				};
				if !last
					.video
					.renditions
					.keys()
					.chain(last.audio.renditions.keys())
					.any(listed)
				{
					return Poll::Ready(Ok(None));
				}
				continue;
			}
			if let Some(last) = &self.last {
				let video = replaced(&last.video.renditions, &listing.video.renditions);
				let audio = replaced(&last.audio.renditions, &listing.audio.renditions);
				if video || audio {
					// The replaced tracks stay to the end their publisher gives
					// them, and the rest go on in the next file.
					let mut tail = last.clone();
					if !video {
						tail.video.renditions.clear();
					}
					if !audio {
						tail.audio.renditions.clear();
					}
					self.replaced.store(true, Ordering::Relaxed);
					return Poll::Ready(Ok(Some(tail)));
				}
			}
			self.last = Some(listing.clone());
			return Poll::Ready(Ok(Some(listing)));
		}
	}
}

/// The program's `media` narrowed to its canvas size, the largest video
/// rendition, which stays in the file while the bandwidth grant disables it
/// for viewers: its pictures stop, and resume, on the same track.
fn canvas(mut media: hang::Catalog) -> hang::Catalog {
	let pixels = |config: &hang::catalog::VideoConfig| {
		u64::from(config.coded_width.unwrap_or(0)) * u64::from(config.coded_height.unwrap_or(0))
	};
	let largest = media
		.video
		.renditions
		.iter()
		.max_by_key(|(_, config)| pixels(config))
		.map(|(name, _)| name.clone());
	media.video.renditions.retain(|name, _| Some(name) == largest.as_ref());
	for config in media.video.renditions.values_mut() {
		config.enabled = true;
	}
	media
}

/// Whether `now` replaced the renditions of a kind the file already records.
/// A kind that first appears joins the file instead, and a rendition's config
/// changing under the same name stays on its track.
fn replaced<T>(last: &BTreeMap<String, T>, now: &BTreeMap<String, T>) -> bool {
	!last.is_empty() && !last.keys().eq(now.keys())
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
	/// sources listed in a local broadcast as the studio lists them.
	struct Studio {
		origin: moq_net::origin::Producer,
		publication: moq_net::origin::Publication,
		catalog: moq_scene::publish::Catalog,
		/// The local broadcast the sources' tracks are in, and its catalog.
		local: moq_net::broadcast::Producer,
		listed: moq_scene::publish::Catalog,
		/// `None` once [ended](Self::end_program).
		program: Option<Tone>,
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
			let program = Some(Tone::new(&mut broadcast, catalog.clone()));
			publication.announce(Default::default()).unwrap();
			let mut local = moq_net::broadcast::Info::new().produce();
			let config = moq_mux::catalog::Config::default()
				.with_catalog(Default::default())
				.with_clock(catalog.clock());
			let listed = moq_scene::publish::Catalog::new(&mut local, config).unwrap();
			Self {
				origin,
				publication,
				catalog,
				local,
				listed,
				program,
				sources: Vec::new(),
				played: Duration::ZERO,
			}
		}

		fn recorder(&self, dir: &Path) -> Recorder {
			let program = moq_mux::Source::new(self.origin.consume(), self.publication.broadcast().consume());
			let sources = moq_mux::Source::new(moq_tokio::origin::spawn().consume(), self.local.consume());
			Recorder::new(program, sources, dir, Path::new("show.toml")).unwrap()
		}

		/// List a source `name` publishing a tone of its own.
		fn add(&mut self, name: &str) {
			let mut scratch = moq_net::broadcast::Info::new().produce();
			let config = moq_mux::catalog::Config::default().with_clock(self.catalog.clock());
			let listing = moq_mux::catalog::Producer::new(&mut scratch, config).unwrap();
			let mut broadcast = self.local.clone();
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

		/// Move the source `name` onto a new track, as a capture that changes
		/// size is.
		fn replace(&mut self, name: &str) {
			let mut broadcast = self.local.clone();
			let (_, tone, _, listing) = self.sources.iter_mut().find(|(source, ..)| source == name).unwrap();
			let old = listing.snapshot().audio.renditions;
			let mut old_tone = std::mem::replace(tone, Tone::new(&mut broadcast, listing.clone()));
			old_tone.audio.finish().unwrap();
			drop(old_tone);
			listing
				.mutate(|catalog| catalog.audio.renditions.retain(|track, _| !old.contains_key(track)))
				.unwrap();
			self.relist();
		}

		/// Move the program onto a new track, as a new canvas size or frame rate
		/// moves its video.
		fn replace_program(&mut self) {
			let mut broadcast = self.publication.broadcast().clone();
			let old = self.catalog.snapshot().audio.renditions;
			let tone = Tone::new(&mut broadcast, self.catalog.clone());
			let mut old_tone = self.program.replace(tone).unwrap();
			old_tone.audio.finish().unwrap();
			drop(old_tone);
			self.catalog
				.mutate(|root| root.audio.renditions.retain(|track, _| !old.contains_key(track)))
				.unwrap();
		}

		/// End the program's track, as stopping the studio does.
		fn end_program(&mut self) {
			let old = self.catalog.snapshot().audio.renditions;
			let mut tone = self.program.take().unwrap();
			tone.audio.finish().unwrap();
			drop(tone);
			self.catalog
				.mutate(|root| root.audio.renditions.retain(|track, _| !old.contains_key(track)))
				.unwrap();
		}

		/// List what each source's own catalog holds, as the studio mirrors it.
		fn relist(&mut self) {
			let listings: Vec<_> = self
				.sources
				.iter()
				.map(|(name, _, _, listing)| {
					let mut listing = listing.snapshot();
					listing.media = Default::default();
					(name.clone(), listing)
				})
				.collect();
			self.listed
				.mutate(|root| root.ext.studio.sources = listings.into_iter().collect())
				.unwrap();
		}

		/// Play every tone for `duration`, as the program clock would.
		async fn play(&mut self, duration: Duration) {
			let end = self.played + duration;
			while self.played < end {
				if let Some(program) = &mut self.program {
					program.write(self.played);
				}
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
			.listed
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

	/// A source moved onto a new track, as a capture that changes size is, goes
	/// on in another file, so neither holds a track that players skip.
	#[tokio::test(start_paused = true)]
	async fn a_replaced_track_goes_on_in_another_file() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.add("camera");
		let recorder = studio.recorder(dir.path());

		recorder.start(true).unwrap();
		studio.play(Duration::from_secs(1)).await;
		studio.replace("camera");
		studio.play(Duration::from_secs(2)).await;
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
		assert_eq!(names, [".mp4", " camera.mp4", " camera (2).mp4"]);
		for ((_, duration), expected) in files.iter().zip([3, 1, 2]) {
			assert!(about(*duration, Duration::from_secs(expected)), "{files:?}");
		}
	}

	/// A program moved onto a new track, as a new canvas moves its video, goes
	/// on in another file, with or without the sources.
	#[tokio::test(start_paused = true)]
	async fn a_replaced_program_goes_on_in_another_file() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		let recorder = studio.recorder(dir.path());

		recorder.start(false).unwrap();
		studio.play(Duration::from_secs(1)).await;
		studio.replace_program();
		studio.play(Duration::from_secs(2)).await;
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
		assert_eq!(names, [".mp4", " (2).mp4"]);
		for ((_, duration), expected) in files.iter().zip([1, 2]) {
			assert!(about(*duration, Duration::from_secs(expected)), "{files:?}");
		}
	}

	/// Closed as the studio quits, a recording reads the program until the
	/// program's track ends, rather than stopping where it was, and goes on in
	/// no other file even as a new track appears; the sources finish with it.
	#[tokio::test(start_paused = true)]
	async fn a_closed_recording_reads_the_program_to_its_end() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.add("mic");
		let recorder = studio.recorder(dir.path());

		recorder.start(true).unwrap();
		studio.play(Duration::from_secs(1)).await;
		recorder.close();
		studio.play(Duration::from_secs(1)).await;
		studio.replace_program();
		kio::clock::timeout(Duration::from_secs(5), async {
			tokio::join!(recorder.done(), studio.play(Duration::from_millis(500)))
		})
		.await
		.expect("the recording finishes once the program's track ends");

		let status = recorder.status().unwrap();
		let files: Vec<_> = status
			.files
			.iter()
			.map(|file| {
				assert_eq!(file.finished, Some(Finished::Saved), "{}", file.path.display());
				let name = file.path.file_name().unwrap().to_string_lossy().into_owned();
				let [(_, duration)] = tracks(&file.path).try_into().unwrap();
				(name.rsplit_once("Z").unwrap().1.to_string(), duration)
			})
			.collect();
		let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
		assert_eq!(names, [".mp4", " mic.mp4"]);
		let program = files[0].1;
		assert!(about(program, Duration::from_secs(2)), "{files:?}");
	}

	/// Closed and its program ended before its file read anything, a recording
	/// still finishes, with nothing left to wait for.
	#[tokio::test(start_paused = true)]
	async fn a_recording_closed_before_it_read_anything_finishes() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		let recorder = studio.recorder(dir.path());
		recorder.start(false).unwrap();
		recorder.close();
		studio.end_program();
		kio::clock::timeout(Duration::from_secs(5), recorder.done())
			.await
			.expect("the recording finishes without another catalog update");
		let status = recorder.status().unwrap();
		let [file] = status.files.as_slice() else {
			panic!("{} files", status.files.len());
		};
		assert_eq!(file.finished, Some(Finished::Empty));
	}

	/// A recording that read the program with no tracks left, such as one whose
	/// video never opened and whose audio failed, finishes once closed, without
	/// waiting for a catalog update that will not come. A catalog that failed
	/// just before is what it reports.
	#[tokio::test(start_paused = true)]
	async fn a_recording_of_an_empty_program_finishes_once_closed() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.end_program();
		let recorder = studio.recorder(dir.path());
		let file = async |abort: bool| {
			recorder.start(false).unwrap();
			kio::clock::sleep(Duration::from_millis(500)).await;
			assert!(!recorder.status().unwrap().done, "nothing ends it before closing");
			if abort {
				// With no chance for the file to read the failure before closing.
				studio.catalog.clone().abort(moq_net::StreamError::Internal.into());
			}
			recorder.close();
			kio::clock::timeout(Duration::from_secs(5), recorder.done())
				.await
				.expect("closing wakes the recording");
			let status = recorder.status().unwrap();
			let [file] = status.files.as_slice() else {
				panic!("{} files", status.files.len());
			};
			file.finished.clone()
		};
		assert_eq!(file(false).await, Some(Finished::Empty));
		let failed = file(true).await;
		assert!(
			matches!(&failed, Some(Finished::Failed(reason)) if reason.contains("reading the broadcast")),
			"{failed:?}"
		);
	}

	/// A failure already waiting as a recording stops is what each file
	/// reports, the program's and every source's, rather than lost to the stop.
	#[tokio::test(start_paused = true)]
	async fn a_failure_wins_over_a_stop() {
		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.add("mic");
		let recorder = studio.recorder(dir.path());
		recorder.start(true).unwrap();
		studio.play(Duration::from_secs(1)).await;
		// Together, with no chance for either file to read the failure first:
		// the program's file reads the program's catalog, and the source's the
		// local one.
		studio.catalog.clone().abort(moq_net::StreamError::Internal.into());
		studio.listed.clone().abort(moq_net::StreamError::Internal.into());
		recorder.stop();
		kio::clock::timeout(Duration::from_secs(5), recorder.done())
			.await
			.expect("the recording finishes");

		let status = recorder.status().unwrap();
		assert_eq!(status.files.len(), 2);
		for file in &status.files {
			assert!(
				matches!(&file.finished, Some(Finished::Failed(reason)) if reason.contains("reading the broadcast")),
				"{}: {:?}",
				file.path.display(),
				file.finished
			);
		}
	}

	/// A source whose file cannot be created fails without stopping the rest,
	/// and once removed and added back, records again.
	#[cfg(unix)]
	#[tokio::test(start_paused = true)]
	async fn a_source_that_failed_to_start_records_once_added_back() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		let recorder = studio.recorder(dir.path());
		recorder.start(true).unwrap();
		studio.play(Duration::from_millis(500)).await;

		let mode = |mode| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
		mode(0o555);
		studio.add("mic");
		studio.play(Duration::from_secs(1)).await;
		studio.remove("mic");
		studio.play(Duration::from_millis(500)).await;
		mode(0o755);
		// Fixed, it still waits to be listed again.
		studio.play(Duration::from_millis(500)).await;
		assert_eq!(recorder.status().unwrap().files.len(), 2);
		studio.add("mic");
		studio.play(Duration::from_secs(1)).await;
		recorder.finish().await;

		let status = recorder.status().unwrap();
		let outcomes: Vec<_> = status
			.files
			.iter()
			.map(|file| {
				let name = file.path.file_name().unwrap().to_string_lossy();
				(name.rsplit_once('Z').unwrap().1.to_string(), file.finished.clone())
			})
			.collect();
		let [program, failed, added] = outcomes.as_slice() else {
			panic!("{outcomes:?}");
		};
		assert_eq!(program, &(".mp4".to_string(), Some(Finished::Saved)));
		assert_eq!(failed.0, " mic.mp4");
		assert!(
			matches!(&failed.1, Some(Finished::Failed(reason)) if reason.contains("cannot create")),
			"{failed:?}"
		);
		assert_eq!(added, &(" mic.mp4".to_string(), Some(Finished::Saved)));
		let [(_, duration)] = tracks(&status.files[2].path).try_into().unwrap();
		assert!(about(duration, Duration::from_secs(1)), "{duration:?}");
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
		let local = moq_mux::Source::new(origin.consume(), moq_net::broadcast::Info::new().produce().consume());
		let recorder = Recorder::new(source, local, dir.path(), Path::new("show.toml")).unwrap();
		recorder.start(true).unwrap();
		kio::clock::sleep(Duration::from_secs(1)).await;
		recorder.finish().await;

		let status = recorder.status().unwrap();
		assert_eq!(status.files.len(), 1);
		assert_eq!(status.files[0].finished, Some(Finished::Empty));
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
	}

	/// A broadcast that fails before anything reached the file still says why,
	/// though the empty file is removed.
	#[tokio::test(start_paused = true)]
	async fn a_failure_before_any_media_says_why() {
		let dir = tempfile::tempdir().unwrap();
		let origin = moq_tokio::origin::spawn();
		let publication = origin.create_broadcast("studio").unwrap();
		let mut broadcast = publication.broadcast().clone();
		let config = moq_mux::catalog::Config::default().with_catalog(Default::default());
		let catalog = moq_scene::publish::Catalog::new(&mut broadcast, config).unwrap();
		publication.announce(Default::default()).unwrap();
		let source = moq_mux::Source::new(origin.consume(), publication.broadcast().consume());
		let local = moq_mux::Source::new(origin.consume(), moq_net::broadcast::Info::new().produce().consume());
		let recorder = Recorder::new(source, local, dir.path(), Path::new("show.toml")).unwrap();
		recorder.start(false).unwrap();
		kio::clock::sleep(Duration::from_millis(100)).await;
		catalog.abort(moq_net::StreamError::Internal.into());
		// Not stopped, which would only race the failure: it ends on its own.
		kio::clock::timeout(Duration::from_secs(5), async {
			while !recorder.status().unwrap().done {
				kio::clock::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the recording ends with its program");

		let status = recorder.status().unwrap();
		let [file] = status.files.as_slice() else {
			panic!("{} files", status.files.len());
		};
		assert!(
			matches!(&file.finished, Some(Finished::Failed(reason)) if reason.contains("reading the broadcast")),
			"{:?}",
			file.finished
		);
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
	}

	/// The names in `dir`, sorted.
	fn names(dir: &Path) -> Vec<String> {
		let mut names: Vec<_> = std::fs::read_dir(dir)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	/// A recording a crash left unfinished, the program's file and each
	/// source's, is repaired at the next launch into a regular MP4 beside it
	/// holding what was written, leaving the original as it was, and only once.
	/// One that finished before the crash removed its marker is left alone.
	#[tokio::test(start_paused = true)]
	async fn launch_repairs_a_crashed_recording() {
		let dir = tempfile::tempdir().unwrap();
		let crashed = tempfile::tempdir().unwrap();
		let mut studio = Studio::new();
		studio.add("mic");
		// Its file is named as the mic's repair would be.
		studio.add("mic.repaired");
		let recorder = studio.recorder(dir.path());
		recorder.start(true).unwrap();
		studio.play(Duration::from_secs(1)).await;

		// What a crash leaves: each file as written so far, and its marker.
		let names_while_recording = names(dir.path());
		assert_eq!(names_while_recording.len(), 6, "{names_while_recording:?}");
		for name in &names_while_recording {
			std::fs::copy(dir.path().join(name), crashed.path().join(name)).unwrap();
		}
		recorder.finish().await;
		assert!(
			names(dir.path()).iter().all(|name| name.ends_with(".mp4")),
			"a finished recording leaves no marker: {:?}",
			names(dir.path())
		);
		// A file finished just before a crash, whose marker stayed.
		let program = &recorder.status().unwrap().files[0].path;
		std::fs::copy(program, crashed.path().join("finished.mp4")).unwrap();
		std::fs::write(crashed.path().join("finished.mp4.unfinished"), b"").unwrap();

		let originals: Vec<_> = names(crashed.path())
			.into_iter()
			.map(|name| {
				let data = std::fs::read(crashed.path().join(&name)).unwrap();
				(name, data)
			})
			.collect();
		let launch = studio.recorder(crashed.path());
		kio::clock::timeout(Duration::from_secs(5), launch.repaired())
			.await
			.expect("the repairs end");

		// The name after the timestamp, or all of it.
		let end = |path: &Path| {
			let name = path.file_name().unwrap().to_string_lossy().into_owned();
			name.rsplit_once('Z').map_or(name.clone(), |(_, end)| end.to_string())
		};
		let repairs = launch.repairs();
		let outcomes: Vec<_> = repairs
			.iter()
			.map(|repair| {
				let outcome = match &repair.outcome {
					Some(Repaired::Saved(repaired)) => Some(end(repaired)),
					Some(Repaired::Finished) => None,
					outcome => panic!("{}: {outcome:?}", repair.path.display()),
				};
				(end(&repair.path), outcome)
			})
			.collect();
		// In name order, where a space comes before a dot. The repair of the
		// mic's file is numbered rather than replacing the other source's.
		let expected = [
			("finished.mp4", None),
			(" mic.mp4", Some(" mic.repaired (2).mp4")),
			(" mic.repaired.mp4", Some(" mic.repaired.repaired.mp4")),
			(".mp4", Some(".repaired.mp4")),
		]
		.map(|(name, repaired)| (name.to_string(), repaired.map(str::to_string)));
		assert_eq!(outcomes, expected);
		for repair in &repairs[1..] {
			let Some(Repaired::Saved(repaired)) = &repair.outcome else {
				unreachable!();
			};
			let [(handler, duration)] = tracks(repaired).try_into().unwrap();
			assert_eq!(handler, "soun");
			assert!(
				about(duration, Duration::from_secs(1)),
				"{}: {duration:?}",
				repaired.display()
			);
		}

		// Each original is as it was, and only the repairs were added.
		for (name, data) in &originals {
			let path = crashed.path().join(name);
			if name.ends_with(MARKER) {
				assert!(!path.exists(), "{name} is removed once repaired");
			} else {
				assert!(&std::fs::read(&path).unwrap() == data, "{name} is left as it was");
			}
		}
		assert_eq!(names(crashed.path()).len(), originals.len() - 4 + 3);

		// The next launch has nothing to repair.
		let again = studio.recorder(crashed.path());
		assert!(again.repairs().is_empty());
	}

	/// What else a launch may find: a file nothing reached and one that is no
	/// recording each give up their marker, as does a marker whose file is
	/// gone, and a file another studio is still recording is left alone.
	#[tokio::test(start_paused = true)]
	async fn launch_leaves_what_it_cannot_repair() {
		let dir = tempfile::tempdir().unwrap();
		let studio = Studio::new();
		std::fs::write(dir.path().join("empty.mp4"), b"").unwrap();
		std::fs::write(dir.path().join("empty.mp4.unfinished"), b"").unwrap();
		std::fs::write(dir.path().join("junk.mp4"), b"not a recording").unwrap();
		std::fs::write(dir.path().join("junk.mp4.unfinished"), b"").unwrap();
		std::fs::write(dir.path().join("gone.mp4.unfinished"), b"").unwrap();
		let (_, _file, _marker) = create(dir.path(), "recording").unwrap();

		let launch = studio.recorder(dir.path());
		kio::clock::timeout(Duration::from_secs(5), launch.repaired())
			.await
			.expect("the repairs end");
		let outcomes: Vec<_> = launch
			.repairs()
			.into_iter()
			.map(|repair| {
				(
					repair.path.file_name().unwrap().to_string_lossy().into_owned(),
					repair.outcome,
				)
			})
			.collect();
		let [(empty, Some(Repaired::Empty)), (junk, Some(Repaired::Failed(reason)))] = outcomes.as_slice() else {
			panic!("{outcomes:?}");
		};
		assert_eq!([empty.as_str(), junk], ["empty.mp4", "junk.mp4"]);
		assert!(reason.contains("not a recording the studio can repair"), "{reason}");
		assert_eq!(
			names(dir.path()),
			["empty.mp4", "junk.mp4", "recording.mp4", "recording.mp4.unfinished"]
		);
	}

	/// A file the launch cannot read keeps its marker, so the next launch tries
	/// again, unlike one it reads and finds no recording in.
	#[cfg(unix)]
	#[tokio::test(start_paused = true)]
	async fn a_repair_that_cannot_read_tries_again() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let studio = Studio::new();
		let path = dir.path().join("locked.mp4");
		std::fs::write(&path, b"not a recording").unwrap();
		std::fs::write(dir.path().join("locked.mp4.unfinished"), b"").unwrap();
		let launch = async |mode| {
			std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
			let launch = studio.recorder(dir.path());
			kio::clock::timeout(Duration::from_secs(5), launch.repaired())
				.await
				.expect("the repairs end");
			let [repair] = launch.repairs().try_into().unwrap();
			let Some(Repaired::Failed(reason)) = repair.outcome else {
				panic!("{:?}", repair.outcome);
			};
			(reason, names(dir.path()))
		};

		let (reason, names) = launch(0o000).await;
		assert!(reason.contains("cannot open"), "{reason}");
		assert_eq!(names, ["locked.mp4", "locked.mp4.unfinished"]);
		let (reason, names) = launch(0o644).await;
		assert!(reason.contains("not a recording the studio can repair"), "{reason}");
		assert_eq!(names, ["locked.mp4"]);
	}

	/// A long scene file or source name is cut, between characters, so every
	/// file name fits, numbered ones included.
	#[test]
	fn a_long_name_is_cut_to_fit() {
		let dir = tempfile::tempdir().unwrap();
		// The first leaves an odd budget for the source's two-byte characters,
		// and the second only the source's reserve.
		for scenes in ["shows".to_string(), "s".repeat(250)] {
			let program = program_stem(&scenes, "2026-11-03T12-00-00Z");
			assert!(program.ends_with(" 2026-11-03T12-00-00Z"), "{program}");
			let source = source_stem(&program, &"é".repeat(300));
			assert!(source.len() > MAX_NAME - SUFFIX - 2, "{} bytes", source.len());
			assert!(source.len() >= program.len() + 1 + SOURCE - 1, "{source}");
			for stem in [&program, &source, &source] {
				let (path, ..) = create(dir.path(), stem).unwrap();
				// So do its marker and its repair, numbered too.
				let repaired = dir.path().join(numbered(&repair::stem(&path).unwrap(), u32::MAX));
				for path in [marker(&path), repaired, path] {
					let name = path.file_name().unwrap().len();
					assert!(name <= MAX_NAME, "{name} bytes: {}", path.display());
				}
			}
		}
	}

	/// The program's file holds its canvas size alone, kept on while the grant
	/// disables it for viewers, with the audio.
	#[test]
	fn the_program_records_its_canvas_size() {
		let video = |width: u32, height: u32, enabled: bool| {
			let mut config =
				hang::catalog::VideoConfig::new("avc3.42c01f".parse::<hang::catalog::VideoCodec>().unwrap());
			config.coded_width = Some(width);
			config.coded_height = Some(height);
			config.enabled = enabled;
			config
		};
		let mut media: hang::Catalog = Default::default();
		media.video.renditions.insert("0.avc3".into(), video(960, 540, true));
		media.video.renditions.insert("1.avc3".into(), video(1920, 1080, false));
		let audio = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		media.audio.renditions.insert("audio".into(), audio);
		let recorded = canvas(media);
		let video: Vec<_> = recorded
			.video
			.renditions
			.iter()
			.map(|(name, config)| (name.as_str(), config.enabled))
			.collect();
		assert_eq!(video, [("1.avc3", true)]);
		assert_eq!(recorded.audio.renditions.len(), 1);
	}

	/// A name already taken, such as by a recording started in the same
	/// second, is numbered rather than replaced.
	#[test]
	fn a_taken_name_is_numbered() {
		let dir = tempfile::tempdir().unwrap();
		let (first, ..) = create(dir.path(), "show").unwrap();
		let (second, ..) = create(dir.path(), "show").unwrap();
		let (third, ..) = create(dir.path(), "show").unwrap();
		assert_eq!(first, dir.path().join("show.mp4"));
		assert_eq!(second, dir.path().join("show (2).mp4"));
		assert_eq!(third, dir.path().join("show (3).mp4"));
	}

	/// A marker already there, such as one a launch holds while it removes it,
	/// takes its name too, so a new recording is never marked by a marker that
	/// someone else may remove.
	#[test]
	fn a_taken_marker_is_numbered() {
		let dir = tempfile::tempdir().unwrap();
		let held = std::fs::File::create(dir.path().join("show.mp4.unfinished")).unwrap();
		held.try_lock().unwrap();
		let (path, _file, created) = create(dir.path(), "show").unwrap();
		assert_eq!(path, dir.path().join("show (2).mp4"));
		assert_eq!(created.path, marker(&path));
		assert_eq!(
			names(dir.path()),
			["show (2).mp4", "show (2).mp4.unfinished", "show.mp4.unfinished"]
		);
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
