//! The moments the operator marks as the show runs, by a key or the flag
//! beside the panic key: REVIEW shows them on its strip and in a list, and
//! cuts a clip around one. A session's markers are kept beside the
//! recordings, in `<show> <stamp>.markers.toml`, named after when the studio
//! opened the show, not in the scene file.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::Context;
use kio::clock::{Instant, Wall};

/// The most markers a session holds, which bounds what a key held down, or a
/// remote firing actions, can make the studio keep.
pub const MAX: usize = 255;

/// The longest name a marker takes, in characters.
pub const MAX_NAME: usize = 60;

/// A moment marked in the show.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
	/// Counted from 1 in the order they were dropped and never reused, so a
	/// marker keeps its number when another is removed.
	pub number: u32,
	/// When it was dropped, on the clock that stamps the program and every
	/// source's past.
	pub at: Instant,
	/// When it was dropped, by the wall clock, as the file says it.
	pub wall: Wall,
	pub name: Option<String>,
}

impl Marker {
	/// What the list and the strip call it: its name, or its number.
	pub fn title(&self) -> String {
		match &self.name {
			Some(name) => name.clone(),
			None => format!("Marker {}", self.number),
		}
	}
}

/// This session's markers.
pub struct Markers {
	/// The file's name in the recordings folder.
	file: String,
	list: Mutex<List>,
	writer: Writer,
}

#[derive(Default)]
struct List {
	markers: Vec<Marker>,
	/// The number the last marker took.
	last: u32,
}

/// The file's shape.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
	#[serde(default)]
	marker: Vec<SavedMarker>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedMarker {
	number: u32,
	/// UTC, to the millisecond, as RFC 3339.
	at: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	name: Option<String>,
}

impl Markers {
	/// No markers yet, for the show `show` opened now.
	pub fn new(show: &str) -> Self {
		// UTC, as a recording's name is stamped, since the studio does not read
		// the time zone; no colons, which Windows refuses in a file name.
		let stamp = humantime::format_rfc3339_seconds(kio::clock::wall().into())
			.to_string()
			.replace(':', "-");
		Self {
			file: format!("{show} {stamp}.markers.toml"),
			list: Mutex::default(),
			writer: Writer::default(),
		}
	}

	/// Where they are kept in the recordings folder `dir`.
	pub fn path(&self, dir: &Path) -> PathBuf {
		dir.join(&self.file)
	}

	/// Each marker, the oldest first.
	pub fn list(&self) -> Vec<Marker> {
		self.list.lock().unwrap().markers.clone()
	}

	/// Why another marker cannot be dropped, if it cannot.
	pub fn refused(&self) -> Option<String> {
		(self.list.lock().unwrap().markers.len() >= MAX).then(full)
	}

	/// Why the last write of the file failed, until one succeeds. The
	/// markers are kept for this session whatever the folder does, so their
	/// clips can still be cut.
	pub fn failure(&self) -> Option<String> {
		self.writer.shared.0.lock().unwrap().failure.clone()
	}

	/// Mark `at`, saving the markers into `dir` in the background, and return
	/// its number.
	pub fn mark(&self, dir: &Path, at: Instant) -> anyhow::Result<u32> {
		let mut list = self.list.lock().unwrap();
		anyhow::ensure!(list.markers.len() < MAX, "{}", full());
		list.last += 1;
		let number = list.last;
		list.markers.push(Marker {
			number,
			at,
			wall: kio::clock::wall(),
			name: None,
		});
		self.save(dir, &list);
		Ok(number)
	}

	/// Name marker `number` `name`, or no name when it is blank, saving the
	/// markers into `dir` in the background.
	pub fn rename(&self, dir: &Path, number: u32, name: &str) -> anyhow::Result<()> {
		let name = name.trim();
		anyhow::ensure!(
			name.chars().count() <= MAX_NAME,
			"a marker's name holds at most {MAX_NAME} characters"
		);
		let mut list = self.list.lock().unwrap();
		let marker = list
			.markers
			.iter_mut()
			.find(|marker| marker.number == number)
			.with_context(|| format!("there is no marker {number}"))?;
		marker.name = (!name.is_empty()).then(|| name.to_string());
		self.save(dir, &list);
		Ok(())
	}

	/// Remove marker `number`, saving the markers into `dir` in the
	/// background.
	pub fn remove(&self, dir: &Path, number: u32) -> anyhow::Result<()> {
		let mut list = self.list.lock().unwrap();
		let before = list.markers.len();
		list.markers.retain(|marker| marker.number != number);
		anyhow::ensure!(list.markers.len() < before, "there is no marker {number}");
		self.save(dir, &list);
		Ok(())
	}

	/// Marker `number`, if it is still there.
	pub fn get(&self, number: u32) -> Option<Marker> {
		let list = self.list.lock().unwrap();
		list.markers.iter().find(|marker| marker.number == number).cloned()
	}

	/// Queue `list` to be written over the file in `dir`, or the file's
	/// removal once the list is empty. Called under the list's lock, so the
	/// writes queue in the order of the changes.
	fn save(&self, dir: &Path, list: &List) {
		let text = (!list.markers.is_empty()).then(|| {
			let saved = Saved {
				marker: list
					.markers
					.iter()
					.map(|marker| SavedMarker {
						number: marker.number,
						at: humantime::format_rfc3339_millis(marker.wall.into()).to_string(),
						name: marker.name.clone(),
					})
					.collect(),
			};
			let text = toml::to_string(&saved).expect("numbers and strings serialize");
			format!("# The moments marked while the show ran, beside its recordings.\n{text}")
		});
		self.writer.queue(Write {
			dir: dir.to_path_buf(),
			path: self.path(dir),
			text,
		});
	}

	/// Wait until every write queued so far has been tried.
	#[cfg(test)]
	fn saved(&self) {
		let (queue, changed) = &*self.writer.shared;
		let queue = queue.lock().unwrap();
		drop(changed.wait_while(queue, |queue| queue.tried < queue.asked).unwrap());
	}
}

/// Why a full session refuses another marker.
fn full() -> String {
	format!("this session already holds {MAX} markers, the most it can; remove one on REVIEW")
}

/// Writes the file on a thread of its own, so a slow disk never holds up the
/// window or a hotkey. Only the latest list queued is written; one that
/// changed again before its turn is skipped. The last write finishes before
/// the markers drop, as the studio closes.
#[derive(Default)]
struct Writer {
	shared: Arc<(Mutex<Queue>, Condvar)>,
	thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct Queue {
	next: Option<Write>,
	/// How many writes were queued, and how many of them were tried, the
	/// skipped included.
	asked: u64,
	tried: u64,
	failure: Option<String>,
	/// No more writes are coming.
	closed: bool,
}

/// One write of the file.
struct Write {
	/// The recordings folder.
	dir: PathBuf,
	path: PathBuf,
	/// What it holds, or `None` to remove it.
	text: Option<String>,
}

impl Write {
	/// Write the file whole, so a crash leaves this list or the one before.
	fn run(&self) -> anyhow::Result<()> {
		let Some(text) = &self.text else {
			return match std::fs::remove_file(&self.path) {
				Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
					Err(err).with_context(|| format!("cannot remove {}", self.path.display()))
				}
				_ => Ok(()),
			};
		};
		let written = (|| {
			// A default folder, such as the videos folder the platform names, may
			// not exist yet.
			std::fs::create_dir_all(&self.dir)?;
			let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
			file.write_all(text.as_bytes())?;
			file.as_file().sync_all()?;
			file.persist(&self.path)?;
			std::io::Result::Ok(())
		})();
		written.with_context(|| format!("cannot write {}", self.path.display()))
	}
}

impl Writer {
	/// Write `write` once the thread is free, in place of any still waiting.
	fn queue(&self, write: Write) {
		let mut thread = self.thread.lock().unwrap();
		{
			let mut queue = self.shared.0.lock().unwrap();
			queue.next = Some(write);
			queue.asked += 1;
		}
		self.shared.1.notify_all();
		if thread.is_none() {
			let shared = self.shared.clone();
			let spawned = std::thread::Builder::new()
				.name("markers".into())
				.spawn(move || write_queued(&shared));
			match spawned {
				Ok(spawned) => *thread = Some(spawned),
				Err(err) => {
					let mut queue = self.shared.0.lock().unwrap();
					queue.failure = Some(format!("cannot start saving the markers: {err}"));
					queue.tried = queue.asked;
					drop(queue);
					self.shared.1.notify_all();
				}
			}
		}
	}
}

impl Drop for Writer {
	fn drop(&mut self) {
		self.shared.0.lock().unwrap().closed = true;
		self.shared.1.notify_all();
		if let Some(thread) = self.thread.get_mut().unwrap().take() {
			let _ = thread.join();
		}
	}
}

/// Write each list queued in `shared`, the latest each time, until closed.
fn write_queued(shared: &(Mutex<Queue>, Condvar)) {
	let (queue, changed) = shared;
	let mut guard = queue.lock().unwrap();
	loop {
		match guard.next.take() {
			Some(write) => {
				let asked = guard.asked;
				drop(guard);
				let result = write.run();
				guard = queue.lock().unwrap();
				guard.tried = asked;
				guard.failure = result.err().map(|err| {
					tracing::error!(error = format!("{err:#}"), "cannot save the markers");
					format!("{err:#}")
				});
				changed.notify_all();
			}
			None if guard.closed => return,
			None => guard = changed.wait(guard).unwrap(),
		}
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	fn read(path: &Path) -> Vec<(u32, Option<String>)> {
		let saved: Saved = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
		saved
			.marker
			.into_iter()
			.map(|marker| (marker.number, marker.name))
			.collect()
	}

	/// Each change is saved whole beside the recordings, a removed marker's
	/// number is never given again, and the file goes once the last marker
	/// does.
	#[test]
	fn markers_are_kept_beside_the_recordings() {
		let dir = tempfile::tempdir().unwrap();
		// The recordings folder may not exist until something is saved there.
		let folder = dir.path().join("videos");
		let markers = Markers::new("show");
		let path = markers.path(&folder);
		let name = path.file_name().unwrap().to_string_lossy().into_owned();
		assert!(name.starts_with("show ") && name.ends_with(".markers.toml"), "{name}");
		assert!(!name.contains(':'), "Windows refuses colons: {name}");

		let now = Instant::now();
		assert_eq!(markers.mark(&folder, now).unwrap(), 1);
		assert_eq!(
			markers
				.mark(&folder, now.saturating_add(Duration::from_secs(5)))
				.unwrap(),
			2
		);
		markers.rename(&folder, 1, "  The goal ").unwrap();
		markers.saved();
		assert_eq!(read(&path), [(1, Some("The goal".into())), (2, None)]);
		assert_eq!(markers.get(1).unwrap().title(), "The goal");
		assert_eq!(markers.get(2).unwrap().title(), "Marker 2");

		markers.rename(&folder, 1, "   ").unwrap();
		assert_eq!(markers.get(1).unwrap().name, None, "a blank name is none");
		markers.remove(&folder, 1).unwrap();
		assert_eq!(markers.mark(&folder, now).unwrap(), 3, "numbers are not reused");
		markers.saved();
		assert_eq!(read(&path), [(2, None), (3, None)]);

		assert!(markers.remove(&folder, 1).is_err(), "already removed");
		assert!(markers.rename(&folder, 9, "x").is_err(), "never dropped");
		assert!(markers.rename(&folder, 2, &"x".repeat(MAX_NAME + 1)).is_err());
		markers.rename(&folder, 2, &"é".repeat(MAX_NAME)).unwrap();

		markers.remove(&folder, 2).unwrap();
		markers.remove(&folder, 3).unwrap();
		markers.saved();
		assert!(!path.exists(), "nothing left to keep");
		assert_eq!(markers.failure(), None);
	}

	/// A session holds at most [`MAX`] markers, refused past it until one is
	/// removed.
	#[test]
	fn a_session_holds_a_bounded_number() {
		let dir = tempfile::tempdir().unwrap();
		let markers = Markers::new("show");
		for _ in 0..MAX {
			markers.mark(dir.path(), Instant::now()).unwrap();
		}
		assert!(markers.refused().is_some());
		assert!(markers.mark(dir.path(), Instant::now()).is_err());
		assert_eq!(markers.list().len(), MAX);
		markers.remove(dir.path(), 7).unwrap();
		assert!(markers.refused().is_none());
		assert_eq!(markers.mark(dir.path(), Instant::now()).unwrap(), MAX as u32 + 1);
	}

	/// A folder that cannot take the file keeps the marker for this session,
	/// and says why until a write succeeds.
	#[test]
	fn a_marker_the_folder_refuses_is_kept() {
		let dir = tempfile::tempdir().unwrap();
		// A file where the folder should be.
		let folder = dir.path().join("taken");
		std::fs::write(&folder, b"").unwrap();
		let markers = Markers::new("show");
		assert_eq!(markers.mark(&folder, Instant::now()).unwrap(), 1);
		markers.saved();
		let failure = markers.failure().expect("the folder is a file");
		assert!(failure.contains("cannot write"), "{failure}");
		assert_eq!(markers.list().len(), 1, "kept anyway");
		markers.mark(dir.path(), Instant::now()).unwrap();
		markers.saved();
		assert_eq!(markers.failure(), None, "saved once the folder takes it");
		assert_eq!(read(&markers.path(dir.path())), [(1, None), (2, None)]);
	}

	/// The last change queued is on disk once the markers drop, as the studio
	/// closes.
	#[test]
	fn the_last_change_is_written_before_closing() {
		let dir = tempfile::tempdir().unwrap();
		let markers = Markers::new("show");
		let path = markers.path(dir.path());
		for _ in 0..20 {
			markers.mark(dir.path(), Instant::now()).unwrap();
		}
		markers.rename(dir.path(), 20, "Last").unwrap();
		drop(markers);
		let saved = read(&path);
		assert_eq!(saved.len(), 20);
		assert_eq!(saved[19], (20, Some("Last".into())));
	}
}
