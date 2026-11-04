//! The scene file and its history, written once they settle after a change.
//!
//! Each edit to the collection or its history is recorded in [`Edits`], and
//! both are written [`QUIET`] after the last one, or [`LIMIT`] after the first
//! unsaved one if the edits never pause. A write compares each serialized file
//! with what it holds, so a change undone before it is written rewrites only
//! the history, and an undo that leaves the history as written writes nothing.
//!
//! The scene file can also change outside the studio, by hand or by another
//! studio. The studio looks for that every [`POLL`], before each write, and
//! again right before each write's rename. It takes such a change when it has
//! nothing unsaved, and otherwise holds it as a [`Conflict`], writing nothing
//! until the user keeps the studio's version or takes the file's. A file
//! deleted outside the studio is written again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_scene::{Collection, History};

/// How long the collection must go unchanged before it is written, so a burst
/// such as a slider drag becomes one write.
pub const QUIET: Duration = Duration::from_secs(1);

/// The longest an edit waits for the collection to settle, so a continuous
/// drag is still written: the most edits a crash loses.
pub const LIMIT: Duration = Duration::from_secs(5);

/// How often the scene file is read for a change made outside the studio.
pub const POLL: Duration = Duration::from_secs(1);

/// The edits not yet written, shared by the studio's edit path and the autosave.
#[derive(Clone, Default)]
pub struct Edits(kio::Shared<Option<Unsaved>>);

#[derive(Clone, Copy)]
struct Unsaved {
	first: Instant,
	last: Instant,
}

impl Unsaved {
	/// [`QUIET`] after the last edit, but no later than [`LIMIT`] after the
	/// first. Another edit only moves it later.
	fn deadline(&self) -> Instant {
		(self.last + QUIET).min(self.first + LIMIT)
	}
}

impl Edits {
	/// Note an edit to the collection, to be written once it settles.
	pub fn record(&self) {
		let now = kio::clock::now();
		let mut unsaved = self.0.lock();
		match unsaved.as_mut() {
			Some(unsaved) => unsaved.last = now,
			None => *unsaved = Some(Unsaved { first: now, last: now }),
		}
	}

	/// Ask for a save [`QUIET`] from now, without delaying one already
	/// waiting, which a repeated failure would otherwise put off until
	/// [`LIMIT`].
	fn retry(&self) {
		let mut unsaved = self.0.lock();
		if unsaved.is_none() {
			let now = kio::clock::now();
			*unsaved = Some(Unsaved { first: now, last: now });
		}
	}

	/// When the unsaved edits are due, if there are any.
	fn deadline(&self) -> Option<Instant> {
		self.0.read().as_ref().map(Unsaved::deadline)
	}

	/// Wait for an unsaved edit, and return when it is due.
	async fn next(&self) -> Instant {
		let unsaved = self
			.0
			.wait(|unsaved| match unsaved.is_some() {
				true => Poll::Ready(()),
				false => Poll::Pending,
			})
			.await;
		unsaved.expect("waited for an edit").deadline()
	}

	/// Wait until the unsaved edits are due.
	async fn due(&self) {
		let mut deadline = self.next().await;
		// Another edit only moves the deadline later, so sleep until it holds.
		loop {
			kio::clock::sleep_until(deadline).await;
			match self.deadline() {
				Some(next) if next > deadline => deadline = next,
				_ => return,
			}
		}
	}
}

/// The scene file and the history beside it, serialized as the studio writes
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
	pub collection: String,
	pub history: String,
}

impl Text {
	pub fn new(collection: &Collection, history: &History) -> anyhow::Result<Self> {
		Ok(Self {
			collection: collection.to_toml()?,
			history: history.to_toml()?,
		})
	}
}

/// What the scene file and the history beside it hold.
pub struct Loaded {
	pub collection: Collection,
	pub history: History,
	/// History entries left out since they no longer apply to the collection.
	pub dropped: usize,
}

/// The collection and history the autosave writes, and replaces with what the
/// files hold on a reload.
pub trait Document {
	fn text(&self) -> anyhow::Result<Text>;

	/// Swap in what the files hold, or refuse it and keep what is there.
	fn replace(&mut self, loaded: Loaded) -> anyhow::Result<()>;
}

/// A change to the scene file made outside the studio that it has not taken,
/// since it has unsaved edits of its own or cannot load the file. The
/// autosave writes nothing until the user picks a version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
	/// Why the file's version cannot be loaded, if it cannot.
	pub refused: Option<String>,
}

pub struct Autosave {
	path: PathBuf,
	/// Where the history is saved, beside the scene file.
	history: PathBuf,
	/// What the files hold, serialized as the studio writes them. Held across
	/// each read and write, so a reload and a write never interleave.
	written: Mutex<Written>,
	edits: Edits,
	/// Why the last write failed, until one succeeds.
	error: Mutex<Option<String>>,
	conflict: Mutex<Option<Conflict>>,
}

struct Written {
	/// `None` until the scene file exists, or when it must be rewritten.
	collection: Option<String>,
	/// An empty history when there is no history file, so none is created
	/// until there is an edit to undo. `None` when it must be rewritten.
	history: Option<String>,
	disk: Disk,
}

/// Exactly what the scene file and its history held when the studio last read
/// or wrote them, so anything else there is an outside edit. Compared by
/// content, not time, so the studio's own write never reads as one. `None`
/// for a missing file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Disk {
	scene: Option<Vec<u8>>,
	history: Option<Vec<u8>>,
}

impl Disk {
	fn read(scene: &Path, history: &Path) -> std::io::Result<Self> {
		Ok(Self {
			scene: read(scene)?,
			history: read(history)?,
		})
	}

	/// `now`, what the files hold, if an edit outside the studio changed them
	/// since they held `self`. The history counts on its own, since another
	/// studio writes it after the scene file. A deleted file is no edit to
	/// take, as the studio writes it again.
	fn changed(&self, now: Self) -> Option<Self> {
		let changed = |now: &Option<Vec<u8>>, then: &Option<Vec<u8>>| now.is_some() && now != then;
		let changed = now.scene.is_some() && (changed(&now.scene, &self.scene) || changed(&now.history, &self.history));
		changed.then_some(now)
	}
}

impl Written {
	/// Whether the files hold `text`, so the studio has nothing unsaved.
	fn holds(&self, text: &Text) -> bool {
		self.collection.as_ref() == Some(&text.collection) && self.history.as_ref() == Some(&text.history)
	}
}

impl Autosave {
	/// Read the scene file at `path` and its history, or start a new
	/// collection if there is no scene file yet; the first save creates it,
	/// and one another program created meanwhile is an outside edit.
	pub fn open(path: PathBuf) -> anyhow::Result<(Self, Loaded)> {
		let history = history_path(&path);
		let disk = Disk::read(&path, &history)?;
		let (collection, written) = match &disk.scene {
			Some(bytes) => {
				let collection = parse(&path, bytes)?;
				// Kept as the studio would write it, not as the file spells it, so
				// a hand-written file is rewritten only once the collection changes.
				let written = collection.to_toml()?;
				(collection, Some(written))
			}
			None => (Collection::new(), None),
		};
		let loaded = load_history(&history, disk.history.as_deref(), collection)?;
		let written = Written {
			collection: written,
			history: Some(loaded.history.to_toml()?),
			disk,
		};
		let autosave = Self {
			path,
			history,
			written: Mutex::new(written),
			edits: Edits::default(),
			error: Mutex::new(None),
			conflict: Mutex::new(None),
		};
		Ok((autosave, loaded))
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Whether the scene file exists, as the studio last read or wrote it.
	pub fn exists(&self) -> bool {
		self.written.lock().unwrap().disk.scene.is_some()
	}

	/// Where the studio records its edits to the collection.
	pub fn edits(&self) -> Edits {
		self.edits.clone()
	}

	/// Why the last write failed, until one succeeds.
	pub fn error(&self) -> Option<String> {
		self.error.lock().unwrap().clone()
	}

	/// The outside edit holding up the autosave, until the user picks a
	/// version or the studio can take it.
	pub fn conflict(&self) -> Option<Conflict> {
		self.conflict.lock().unwrap().clone()
	}

	/// Write each file of `doc` that does not already hold it: the scene file,
	/// then the history, so a history never describes edits the scene file
	/// lacks. An outside edit to the scene file is taken instead when `doc`
	/// has nothing unsaved, and otherwise fails the save as a [`Conflict`]. A
	/// failure stays in [`Self::error`] until a write succeeds.
	pub fn save<D: Document>(&self, doc: &Mutex<D>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		self.save_locked(&mut written, doc)
	}

	fn save_locked<D: Document>(&self, written: &mut Written, doc: &Mutex<D>) -> anyhow::Result<()> {
		// Before serializing, so an edit made after it is recorded anew.
		self.edits.0.lock().take();
		let result = self.write(written, doc);

		let error = result.as_ref().err().map(|err| format!("{err:#}"));
		let mut last = self.error.lock().unwrap();
		// Logged once, not on every failed attempt.
		if error.is_some() && *last != error {
			tracing::error!(error = error.as_deref(), "autosave failed");
		}
		*last = error;
		if result.is_err() {
			// Still unsaved, so tried again once the collection is quiet.
			self.edits.record();
		}
		result
	}

	fn write<D: Document>(&self, written: &mut Written, doc: &Mutex<D>) -> anyhow::Result<()> {
		loop {
			self.look(written, doc)?;
			let text = doc.lock().unwrap().text()?;
			if written.collection.as_ref() != Some(&text.collection) {
				// Checked again right before the rename, since another studio may
				// have written the files since the look above.
				let unchanged = || Ok(self.outside(&written.disk)?.is_none());
				let context = || format!("writing {}", self.path.display());
				let renamed = write(&self.path, &text.collection, written.disk.scene.is_none(), unchanged)
					.with_context(context)?;
				let Some(dir) = renamed else {
					// Taken or held as a conflict by the next look.
					continue;
				};
				// Renamed into place, so the studio's own file never reads as an
				// outside edit, but written again until its rename is synced.
				written.disk.scene = Some(text.collection.clone().into_bytes());
				written.collection = None;
				sync(&dir).with_context(context)?;
				written.collection = Some(text.collection);
			}
			if written.history.as_ref() != Some(&text.history) {
				let unchanged = || Ok(self.outside(&written.disk)?.is_none());
				let context = || format!("writing {}", self.history.display());
				let renamed = write(&self.history, &text.history, false, unchanged).with_context(context)?;
				let Some(dir) = renamed else {
					continue;
				};
				written.disk.history = Some(text.history.clone().into_bytes());
				written.history = None;
				sync(&dir).with_context(context)?;
				written.history = Some(text.history);
			}
			return Ok(());
		}
	}

	/// Look for an outside edit to the files now, without writing, and ask
	/// for a save if a file must be written again, such as one deleted.
	pub fn check<D: Document>(&self, doc: &Mutex<D>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		self.look(&mut written, doc)?;
		if written.collection.is_none() || written.history.is_none() {
			self.edits.retry();
		}
		Ok(())
	}

	/// Take an outside edit to the files if `doc` has nothing unsaved and
	/// accepts it, or else hold it as a [`Conflict`] and fail. A file deleted
	/// outside the studio holds nothing to keep, so it is written again.
	fn look<D: Document>(&self, written: &mut Written, doc: &Mutex<D>) -> anyhow::Result<()> {
		let now = Disk::read(&self.path, &self.history)?;
		if now.scene.is_none() && written.disk.scene.is_some() {
			tracing::warn!(path = %self.path.display(), "the scene file was deleted outside the studio; writing it again");
			written.disk.scene = None;
			written.collection = None;
		}
		if now.history.is_none() && written.disk.history.is_some() {
			tracing::warn!(path = %self.history.display(), "the history was deleted outside the studio; writing it again");
			written.disk.history = None;
			written.history = None;
		}
		let Some(disk) = written.disk.changed(now) else {
			*self.conflict.lock().unwrap() = None;
			return Ok(());
		};

		let refused = match self.load(&disk) {
			Ok(loaded) => {
				let mut doc = doc.lock().unwrap();
				if written.holds(&doc.text()?) {
					match self.take(written, &mut *doc, loaded, disk) {
						Ok(()) => return Ok(()),
						Err(err) => Some(err),
					}
				} else {
					None
				}
			}
			Err(err) => Some(err),
		};

		let conflict = Conflict {
			refused: refused.map(|err| format!("{err:#}")),
		};
		let mut last = self.conflict.lock().unwrap();
		if last.as_ref() != Some(&conflict) {
			tracing::warn!(refused = conflict.refused.as_deref(), path = %self.path.display(), "the scene file changed outside the studio");
		}
		*last = Some(conflict);
		anyhow::bail!("{} changed outside the studio", self.path.display())
	}

	/// What the files hold now, if an edit outside the studio changed them
	/// since `disk`.
	fn outside(&self, disk: &Disk) -> std::io::Result<Option<Disk>> {
		Ok(disk.changed(Disk::read(&self.path, &self.history)?))
	}

	/// The collection and history `disk` holds.
	fn load(&self, disk: &Disk) -> anyhow::Result<Loaded> {
		let scene = disk
			.scene
			.as_deref()
			.with_context(|| format!("{} does not exist", self.path.display()))?;
		let collection = parse(&self.path, scene)?;
		load_history(&self.history, disk.history.as_deref(), collection)
	}

	/// Hand `loaded`, read from `disk`, to `doc`. Once it is accepted, the
	/// files count as holding it.
	fn take(&self, written: &mut Written, doc: &mut impl Document, loaded: Loaded, disk: Disk) -> anyhow::Result<()> {
		let text = Text::new(&loaded.collection, &loaded.history)?;
		doc.replace(loaded)?;
		*written = Written {
			collection: Some(text.collection),
			history: Some(text.history),
			disk,
		};
		*self.conflict.lock().unwrap() = None;
		Ok(())
	}

	/// Replace `doc` with what the scene file and its history hold, whatever
	/// it has unsaved: the user taking the file's version.
	pub fn reload<D: Document>(&self, doc: &Mutex<D>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		let disk = Disk::read(&self.path, &self.history)?;
		let loaded = self.load(&disk)?;
		self.take(&mut written, &mut *doc.lock().unwrap(), loaded, disk)
	}

	/// Write `doc` over whatever the scene file and its history hold now: the
	/// user keeping the studio's version.
	pub fn keep<D: Document>(&self, doc: &Mutex<D>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		written.disk = Disk::read(&self.path, &self.history)?;
		// The history beside an outside edit may be another studio's.
		written.collection = None;
		written.history = None;
		self.save_locked(&mut written, doc)
	}

	/// Write `doc`'s files now, which creates a missing scene file, and then
	/// whenever the recorded edits are due, looking for an outside edit every
	/// [`POLL`] in between.
	pub async fn run<D: Document>(&self, doc: &Mutex<D>) {
		// Kept for the status bar until a save succeeds.
		let _ = self.save(doc);
		loop {
			tokio::select! {
				() = self.edits.due() => {
					let _ = self.save(doc);
				}
				() = kio::clock::sleep(POLL) => {
					if self.check(doc).is_err() {
						// Retried as a save, so a file that cannot be read shows in the
						// status bar until a save succeeds.
						self.edits.retry();
					}
				}
			}
		}
	}
}

/// Read the file at `path`, or `None` if there is none.
fn read(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
	match std::fs::read(path) {
		Ok(bytes) => Ok(Some(bytes)),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
		Err(err) => Err(std::io::Error::new(
			err.kind(),
			format!("reading {}: {err}", path.display()),
		)),
	}
}

fn parse(path: &Path, bytes: &[u8]) -> anyhow::Result<Collection> {
	let text = std::str::from_utf8(bytes).with_context(|| format!("loading {}", path.display()))?;
	Collection::from_toml(text).with_context(|| format!("loading {}", path.display()))
}

/// Where the history of the scene file at `scene` is saved: beside it, as
/// `show.history.toml` for `show.toml`.
fn history_path(scene: &Path) -> PathBuf {
	let mut name = scene.file_stem().unwrap_or_default().to_os_string();
	name.push(".history.toml");
	scene.with_file_name(name)
}

/// Load the history file at `path`, which holds `bytes`, for `collection`,
/// keeping the entries that still apply to it, or start a new one if there is
/// no history file.
fn load_history(path: &Path, bytes: Option<&[u8]>, collection: Collection) -> anyhow::Result<Loaded> {
	let (history, dropped) = match bytes {
		Some(bytes) => std::str::from_utf8(bytes)
			.map_err(anyhow::Error::from)
			.and_then(|text| Ok(History::from_toml(text, &collection)?))
			.with_context(|| format!("loading {}; delete it to start a new history", path.display()))?,
		None => (History::new(), 0),
	};
	if dropped > 0 {
		tracing::warn!(dropped, path = %path.display(), "history entries no longer apply to the scene file");
	}
	Ok(Loaded {
		collection,
		history,
		dropped,
	})
}

/// Replace the file at `path` with `text` all at once, so a crash leaves
/// either the old file or the new one, never part of either. Returns the
/// directory to [`sync`] once the caller notes the rename, or `None`, leaving
/// the file as it is, if the files changed outside the studio: a `new` file,
/// one the studio last found missing, exists, or `unchanged`, asked right
/// before the rename, says otherwise.
fn write(
	path: &Path,
	text: &str,
	new: bool,
	unchanged: impl FnOnce() -> std::io::Result<bool>,
) -> std::io::Result<Option<PathBuf>> {
	let path = resolve(path)?;
	let existing = match std::fs::metadata(&path) {
		Ok(_) if new => return Ok(None),
		Ok(metadata) => {
			// Refused as a write in place would be, so a read-only scene file
			// is not replaced.
			std::fs::OpenOptions::new().write(true).open(&path)?;
			Some(metadata.permissions())
		}
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
		Err(err) => return Err(err),
	};
	let dir = match path.parent() {
		Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
		_ => PathBuf::from("."),
	};

	// Named uniquely and created exclusively, so another studio saving the
	// same file never writes into this one.
	let mut prefix = std::ffi::OsString::from(".");
	prefix.push(path.file_name().unwrap_or_default());
	prefix.push(".");
	let mut builder = tempfile::Builder::new();
	builder.prefix(&prefix).suffix(".autosave");
	// A new file gets the umask's permissions, like any other.
	#[cfg(unix)]
	if existing.is_none() {
		use std::os::unix::fs::PermissionsExt;
		builder.permissions(std::fs::Permissions::from_mode(0o666));
	}
	let mut file = builder.tempfile_in(&dir)?;
	// It replaces the old file, so it takes its permissions.
	if let Some(permissions) = existing {
		file.as_file().set_permissions(permissions)?;
	}
	file.write_all(text.as_bytes())?;
	// On disk before the rename, so a power loss cannot leave the new name
	// pointing at an empty file.
	file.as_file().sync_all()?;
	// As late as it can be. An edit landing between this and the rename is
	// still lost, since nothing binds every editor to a lock, except that a
	// new file is never replaced.
	if !unchanged()? {
		return Ok(None);
	}
	match new {
		true => match file.persist_noclobber(&path) {
			Ok(_) => {}
			Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
			Err(err) => return Err(err.error),
		},
		false => {
			file.persist(&path)?;
		}
	}
	Ok(Some(dir))
}

/// Sync the directory `dir` a file was renamed into, so a power loss cannot
/// undo the rename.
fn sync(dir: &Path) -> std::io::Result<()> {
	#[cfg(unix)]
	std::fs::File::open(dir)?.sync_all()?;
	#[cfg(not(unix))]
	let _ = dir;
	Ok(())
}

/// The file a chain of symlinks at `path` ends in, which need not exist yet,
/// so a save replaces that file and every link survives.
fn resolve(path: &Path) -> std::io::Result<PathBuf> {
	let mut path = path.to_path_buf();
	// Linux's own limit on links followed in one lookup.
	for _ in 0..40 {
		match std::fs::symlink_metadata(&path) {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				let target = std::fs::read_link(&path)?;
				// Relative to the link's directory; an absolute target replaces it.
				path = path.parent().unwrap_or(Path::new("")).join(target);
			}
			Ok(_) => return Ok(path),
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(path),
			Err(err) => return Err(err),
		}
	}
	Err(std::io::Error::other(format!(
		"{}: too many levels of symbolic links",
		path.display()
	)))
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use super::*;

	fn collection(scene: &str) -> Collection {
		let mut collection = Collection::new();
		collection.scenes.push(moq_scene::Scene::new(scene));
		collection
	}

	/// A collection and its history as the studio holds them, which can be
	/// told to refuse a reload as the studio refuses a new canvas.
	struct Doc {
		collection: Collection,
		history: History,
		refuse: bool,
	}

	impl Doc {
		fn new(collection: Collection) -> Mutex<Self> {
			Mutex::new(Self {
				collection,
				history: History::new(),
				refuse: false,
			})
		}

		/// Add scene `scene`, as an edit in the studio would.
		fn edit(&mut self, scene: &str) {
			let mut draft = self.collection.draft(format!("Add scene `{scene}`"));
			draft
				.insert_scene(self.collection.scenes.len(), moq_scene::Scene::new(scene))
				.unwrap();
			let edit = draft.finish().unwrap();
			self.collection.apply(&edit).unwrap();
			self.history.record(edit, None);
		}
	}

	impl Document for Doc {
		fn text(&self) -> anyhow::Result<Text> {
			Text::new(&self.collection, &self.history)
		}

		fn replace(&mut self, loaded: Loaded) -> anyhow::Result<()> {
			anyhow::ensure!(!self.refuse, "refused");
			self.collection = loaded.collection;
			self.history = loaded.history;
			Ok(())
		}
	}

	fn read(path: &Path) -> String {
		std::fs::read_to_string(path).unwrap()
	}

	fn names(dir: &Path) -> Vec<std::ffi::OsString> {
		let mut names: Vec<_> = std::fs::read_dir(dir)
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		names.sort();
		names
	}

	#[test]
	fn creates_a_missing_file_and_rewrites_only_a_change() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();
		assert_eq!(opened, Collection::new());

		let doc = Doc::new(opened);
		autosave.save(&doc).unwrap();
		assert_eq!(read(&path), Collection::new().to_toml().unwrap());

		doc.lock().unwrap().collection = collection("Second");
		autosave.save(&doc).unwrap();
		assert_eq!(read(&path), collection("Second").to_toml().unwrap());
		assert_eq!(
			names(dir.path()),
			["show.toml"],
			"the temporary file is renamed into place, and an empty history is not written"
		);
	}

	/// A scene file another program creates after the studio found none, such
	/// as a second studio finishing setup first, is an outside edit, not
	/// replaced by the collection setup proposed.
	#[test]
	fn leaves_a_file_created_since_it_found_none() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, _) = Autosave::open(path.clone()).unwrap();
		assert!(!autosave.exists());

		std::fs::write(&path, HAND_WRITTEN).unwrap();
		let err = format!("{:#}", autosave.save(&Doc::new(collection("Proposed"))).unwrap_err());
		assert!(err.contains("changed outside the studio"), "{err}");
		assert_eq!(read(&path), HAND_WRITTEN);
		assert_eq!(autosave.conflict(), Some(Conflict { refused: None }));
		assert!(!autosave.exists(), "the studio has not read it");
	}

	/// A new file gets the permissions any new file would, not a temporary
	/// file's owner-only ones.
	#[cfg(unix)]
	#[test]
	fn creates_a_file_with_the_umask_permissions() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let plain = dir.path().join("plain");
		std::fs::File::create(&plain).unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();
		autosave.save(&Doc::new(opened)).unwrap();

		let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode(&path), mode(&plain));
	}

	const HAND_WRITTEN: &str = "# my show\nversion = 1\ncanvas = { width = 1280, height = 720, framerate = 30 }\n\n[[scenes]]\nname = \"Main\"\n";

	/// A hand-written file keeps its comments and layout until the collection
	/// it describes changes.
	#[test]
	fn leaves_a_hand_written_file_alone_until_an_edit() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, HAND_WRITTEN).unwrap();

		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();
		autosave.save(&Doc::new(opened)).unwrap();
		assert_eq!(read(&path), HAND_WRITTEN);
	}

	/// An autosave running over a collection the test edits, as the engine's
	/// does over the studio's.
	struct Running {
		autosave: Arc<Autosave>,
		doc: Arc<Mutex<Doc>>,
		task: tokio::task::JoinHandle<()>,
	}

	impl Running {
		async fn start(path: PathBuf) -> Self {
			let (autosave, opened) = Autosave::open(path).unwrap();
			let autosave = Arc::new(autosave);
			let doc = Arc::new(Mutex::new(Doc {
				collection: opened.collection,
				history: opened.history,
				refuse: false,
			}));
			let task = tokio::spawn({
				let autosave = autosave.clone();
				let doc = doc.clone();
				async move { autosave.run(&doc).await }
			});
			// Its first save, at once.
			tokio::task::yield_now().await;
			Self { autosave, doc, task }
		}

		/// Add a scene, as an edit in the studio would, and return the
		/// collection it leaves serialized.
		fn edit(&self, scene: &str) -> String {
			let mut doc = self.doc.lock().unwrap();
			doc.edit(scene);
			self.autosave.edits().record();
			doc.collection.to_toml().unwrap()
		}

		/// Undo the newest edit, as the studio would, and return the files it
		/// leaves serialized.
		fn undo(&self) -> Text {
			let mut doc = self.doc.lock().unwrap();
			let Doc {
				collection, history, ..
			} = &mut *doc;
			history.undo(collection).unwrap().expect("an edit to undo");
			self.autosave.edits().record();
			doc.text().unwrap()
		}

		fn text(&self) -> Text {
			self.doc.lock().unwrap().text().unwrap()
		}
	}

	impl Drop for Running {
		fn drop(&mut self) {
			self.task.abort();
		}
	}

	/// A burst of edits is written once, a quiet second after the last, and
	/// the studio's own writes never read as an outside edit.
	#[tokio::test(start_paused = true)]
	async fn writes_once_the_edits_pause() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let running = Running::start(path.clone()).await;
		let created = Collection::new().to_toml().unwrap();
		assert_eq!(read(&path), created, "a missing file is created at once");

		running.edit("A");
		kio::clock::sleep(QUIET / 2).await;
		let last = running.edit("B");
		kio::clock::sleep(QUIET - Duration::from_millis(100)).await;
		assert_eq!(read(&path), created, "written before the edits paused");
		kio::clock::sleep(Duration::from_millis(200)).await;
		assert_eq!(read(&path), last);

		let last = running.edit("C");
		kio::clock::sleep(QUIET + POLL * 2).await;
		assert_eq!(read(&path), last);
		assert_eq!(running.autosave.conflict(), None);
		assert_eq!(running.autosave.error(), None);
	}

	/// Edits that never pause are still written, [`LIMIT`] after the first.
	#[tokio::test(start_paused = true)]
	async fn writes_continuous_edits_within_the_limit() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let running = Running::start(path.clone()).await;
		let created = read(&path);

		// Off the limit's beat, so no edit lands on it.
		let every = QUIET * 2 / 5;
		let mut elapsed = Duration::ZERO;
		let mut last = None;
		while elapsed < LIMIT {
			assert_eq!(read(&path), created, "written before the limit");
			last = Some(running.edit(&elapsed.as_millis().to_string()));
			kio::clock::sleep(every).await;
			elapsed += every;
		}
		// Past the limit, and less than a quiet second after the last edit.
		assert_eq!(Some(read(&path)), last);
	}

	/// A failed write shows until a retry, a quiet second later, succeeds.
	#[tokio::test(start_paused = true)]
	async fn retries_a_failed_write_until_one_succeeds() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("missing").join("show.toml");
		let running = Running::start(path.clone()).await;
		assert!(running.autosave.error().unwrap().contains("show.toml"));

		std::fs::create_dir(path.parent().unwrap()).unwrap();
		kio::clock::sleep(QUIET - Duration::from_millis(100)).await;
		assert!(!path.exists(), "retried before the collection was quiet");
		kio::clock::sleep(Duration::from_millis(200)).await;
		assert_eq!(running.autosave.error(), None);
		assert!(path.exists());
	}

	/// With nothing unsaved, an outside edit is taken within a poll, its
	/// history with it even when it lands a poll later, and left as written
	/// until the next edit.
	#[tokio::test(start_paused = true)]
	async fn takes_an_outside_edit_with_nothing_unsaved() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let running = Running::start(path.clone()).await;

		std::fs::write(&path, HAND_WRITTEN).unwrap();
		kio::clock::sleep(POLL + Duration::from_millis(100)).await;
		let hand = Collection::from_toml(HAND_WRITTEN).unwrap();
		assert_eq!(running.text().collection, hand.to_toml().unwrap());

		// Another studio's history, written after its scene file.
		let mut theirs = Doc::new(hand).into_inner().unwrap();
		theirs.edit("Theirs");
		theirs.history.undo(&mut theirs.collection).unwrap();
		std::fs::write(history_path(&path), theirs.history.to_toml().unwrap()).unwrap();
		kio::clock::sleep(POLL).await;
		assert_eq!(running.text(), theirs.text().unwrap());
		assert_eq!(running.autosave.conflict(), None);
		assert_eq!(read(&path), HAND_WRITTEN, "taken, not rewritten");

		let edited = running.edit("Mine");
		kio::clock::sleep(QUIET + Duration::from_millis(100)).await;
		assert_eq!(read(&path), edited);
	}

	/// When both the studio and the file changed, nothing is written until
	/// the user keeps the studio's version or takes the file's.
	#[tokio::test(start_paused = true)]
	async fn holds_an_outside_edit_against_unsaved_edits() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let history = history_path(&path);
		let running = Running::start(path.clone()).await;

		let mine = running.edit("Mine");
		std::fs::write(&path, HAND_WRITTEN).unwrap();
		kio::clock::sleep(QUIET * 3).await;
		assert_eq!(read(&path), HAND_WRITTEN);
		assert!(!history.exists());
		assert_eq!(running.autosave.conflict(), Some(Conflict { refused: None }));
		assert!(running.autosave.error().unwrap().contains("changed outside the studio"));

		// Keeping the studio's version writes it, and its history, over the file.
		running.autosave.keep(&*running.doc).unwrap();
		assert_eq!(read(&path), mine);
		assert_eq!(read(&history), running.text().history);
		assert_eq!((running.autosave.conflict(), running.autosave.error()), (None, None));

		// Taking the file's version replaces the studio's unsaved edit.
		running.edit("Again");
		std::fs::write(&path, HAND_WRITTEN).unwrap();
		kio::clock::sleep(POLL + Duration::from_millis(100)).await;
		assert!(running.autosave.conflict().is_some());
		running.autosave.reload(&*running.doc).unwrap();
		assert_eq!(
			running.text().collection,
			Collection::from_toml(HAND_WRITTEN).unwrap().to_toml().unwrap()
		);
		assert_eq!(running.autosave.conflict(), None);
		kio::clock::sleep(QUIET * 2).await;
		assert_eq!(read(&path), HAND_WRITTEN);
	}

	/// An outside edit the studio cannot load is held even with nothing
	/// unsaved, so the file is not overwritten, and taken once it loads.
	#[tokio::test(start_paused = true)]
	async fn holds_an_outside_edit_it_cannot_load() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let running = Running::start(path.clone()).await;
		let created = running.text();

		std::fs::write(&path, "not a scene file").unwrap();
		kio::clock::sleep(POLL + Duration::from_millis(100)).await;
		let refused = running.autosave.conflict().unwrap().refused.unwrap();
		assert!(refused.contains("show.toml"), "{refused}");
		assert!(running.autosave.reload(&*running.doc).is_err());
		assert_eq!(running.text(), created);
		assert_eq!(read(&path), "not a scene file");

		// One the studio refuses, as it does a new canvas, is held the same way.
		running.doc.lock().unwrap().refuse = true;
		std::fs::write(&path, HAND_WRITTEN).unwrap();
		kio::clock::sleep(POLL + Duration::from_millis(100)).await;
		let conflict = running.autosave.conflict().unwrap();
		assert_eq!(conflict.refused.as_deref(), Some("refused"));
		assert_eq!(read(&path), HAND_WRITTEN);

		running.doc.lock().unwrap().refuse = false;
		kio::clock::sleep(POLL + Duration::from_millis(100)).await;
		assert_eq!(running.autosave.conflict(), None);
		assert_eq!(
			running.text().collection,
			Collection::from_toml(HAND_WRITTEN).unwrap().to_toml().unwrap()
		);
	}

	/// A scene file that cannot be read shows as a failed save until it can.
	#[cfg(unix)]
	#[tokio::test(start_paused = true)]
	async fn shows_a_file_it_cannot_read() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let running = Running::start(path.clone()).await;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
		if std::fs::File::open(&path).is_ok() {
			return; // Running as root, which may read it anyway.
		}

		kio::clock::sleep(POLL + QUIET + Duration::from_millis(100)).await;
		let error = running.autosave.error().unwrap();
		assert!(error.contains("show.toml"), "{error}");

		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
		kio::clock::sleep(QUIET + Duration::from_millis(100)).await;
		assert_eq!(running.autosave.error(), None);
	}

	/// A file deleted outside the studio holds nothing to keep, so it is
	/// written again within a poll, or by the save on quit, without waiting
	/// for an edit.
	#[tokio::test(start_paused = true)]
	async fn writes_a_deleted_file_again() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let history = history_path(&path);
		let running = Running::start(path.clone()).await;
		running.edit("A");
		kio::clock::sleep(QUIET + Duration::from_millis(100)).await;
		let saved = running.text();
		assert_eq!(read(&history), saved.history);

		std::fs::remove_file(&path).unwrap();
		std::fs::remove_file(&history).unwrap();
		kio::clock::sleep(POLL + QUIET + Duration::from_millis(100)).await;
		assert_eq!(read(&path), saved.collection);
		assert_eq!(read(&history), saved.history);

		// Saved on quit before a poll sees it.
		std::fs::remove_file(&path).unwrap();
		running.autosave.save(&*running.doc).unwrap();
		assert_eq!(read(&path), saved.collection);
		assert_eq!(running.autosave.conflict(), None);
	}

	/// A rename whose directory cannot be synced still replaced the file, so
	/// once the directory can be synced, the studio's own file is no outside
	/// edit, and it is written again so the save is durable.
	#[cfg(unix)]
	#[test]
	fn remembers_a_rename_it_could_not_sync() {
		use std::os::unix::fs::{MetadataExt, PermissionsExt};

		let dir = tempfile::tempdir().unwrap();
		let show = dir.path().join("show");
		std::fs::create_dir(&show).unwrap();
		let path = show.join("show.toml");
		let (autosave, Loaded { collection, .. }) = Autosave::open(path.clone()).unwrap();
		let doc = Doc::new(collection);
		autosave.save(&doc).unwrap();

		// Files can be created and renamed in it, but it cannot be opened to sync.
		let mode = |mode| std::fs::set_permissions(&show, std::fs::Permissions::from_mode(mode)).unwrap();
		mode(0o333);
		if std::fs::File::open(&show).is_ok() {
			return mode(0o755); // Running as root, which may open it anyway.
		}
		doc.lock().unwrap().edit("Mine");
		let err = autosave.save(&doc);
		mode(0o755);
		let err = format!("{:#}", err.unwrap_err());
		assert!(err.contains("show.toml"), "{err}");
		let mine = doc.lock().unwrap().text().unwrap();
		assert_eq!(read(&path), mine.collection, "renamed before the sync failed");

		autosave.check(&doc).unwrap();
		assert_eq!(autosave.conflict(), None);
		let inode = || std::fs::metadata(&path).unwrap().ino();
		let renamed = inode();
		autosave.save(&doc).unwrap();
		assert_eq!(autosave.error(), None);
		assert_ne!(inode(), renamed, "written again, so its rename is synced");
		assert_eq!(read(&path), mine.collection);
		assert_eq!(read(&history_path(&path)), mine.history);
	}

	/// A retry asks for a save without putting off one already waiting, so a
	/// failure repeating every poll cannot hold it back until [`LIMIT`].
	#[tokio::test(start_paused = true)]
	async fn a_retry_does_not_put_off_a_waiting_save() {
		let edits = Edits::default();
		edits.record();
		let deadline = edits.deadline();
		kio::clock::sleep(QUIET / 2).await;
		edits.retry();
		assert_eq!(edits.deadline(), deadline);
	}

	/// The file is checked again after the save serializes the studio's
	/// version, so an outside edit landing as the save starts is not lost.
	#[test]
	fn checks_again_before_replacing_the_file() {
		/// Another studio writes the file just as this one serializes.
		struct Racing {
			doc: Doc,
			path: PathBuf,
		}

		impl Document for Racing {
			fn text(&self) -> anyhow::Result<Text> {
				std::fs::write(&self.path, HAND_WRITTEN).unwrap();
				self.doc.text()
			}

			fn replace(&mut self, loaded: Loaded) -> anyhow::Result<()> {
				self.doc.replace(loaded)
			}
		}

		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, Collection::new().to_toml().unwrap()).unwrap();
		let (autosave, Loaded { collection, .. }) = Autosave::open(path.clone()).unwrap();
		let mut doc = Doc::new(collection).into_inner().unwrap();
		doc.edit("Mine");
		let racing = Mutex::new(Racing {
			doc,
			path: path.clone(),
		});

		let err = format!("{:#}", autosave.save(&racing).unwrap_err());
		assert!(err.contains("changed outside the studio"), "{err}");
		assert_eq!(read(&path), HAND_WRITTEN);
		assert_eq!(names(dir.path()), ["show.toml"], "the temporary file is removed");
		assert_eq!(autosave.conflict(), Some(Conflict { refused: None }));
	}

	#[cfg(unix)]
	#[test]
	fn writes_through_a_symlink_and_keeps_permissions() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let target = dir.path().join("target.toml");
		std::fs::write(&target, Collection::new().to_toml().unwrap()).unwrap();
		std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
		let link = dir.path().join("show.toml");
		std::os::unix::fs::symlink(&target, &link).unwrap();

		let (autosave, _) = Autosave::open(link.clone()).unwrap();
		let doc = Doc::new(collection("Edited"));
		autosave.save(&doc).unwrap();
		// Written through the link, so its own write reads as no outside edit.
		autosave.check(&doc).unwrap();

		assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
		assert_eq!(read(&target), collection("Edited").to_toml().unwrap());
		let mode = std::fs::metadata(&target).unwrap().permissions().mode();
		assert_eq!(mode & 0o777, 0o640);
	}

	/// A chain of links to a file that does not exist yet creates that file,
	/// as a write through the links would.
	#[cfg(unix)]
	#[test]
	fn creates_the_file_a_dangling_symlink_chain_names() {
		let dir = tempfile::tempdir().unwrap();
		let target = dir.path().join("target.toml");
		let middle = dir.path().join("middle.toml");
		let link = dir.path().join("show.toml");
		std::os::unix::fs::symlink("target.toml", &middle).unwrap();
		std::os::unix::fs::symlink(&middle, &link).unwrap();

		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(link.clone()).unwrap();
		autosave.save(&Doc::new(opened)).unwrap();

		assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
		assert!(std::fs::symlink_metadata(&middle).unwrap().file_type().is_symlink());
		assert_eq!(read(&target), Collection::new().to_toml().unwrap());
	}

	#[cfg(unix)]
	#[test]
	fn refuses_to_replace_a_read_only_file() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let text = Collection::new().to_toml().unwrap();
		std::fs::write(&path, &text).unwrap();
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
		if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
			return; // Running as root, which may write it anyway.
		}

		// Nor is a history of edits the scene file lacks written beside it.
		let (autosave, _) = Autosave::open(path.clone()).unwrap();
		let doc = Doc::new(Collection::new());
		doc.lock().unwrap().edit("Edited");
		assert!(autosave.save(&doc).is_err());
		assert_eq!(read(&path), text);
		assert!(!history_path(&path).exists());
	}

	/// An edit undone before it is written changes only the history, which is
	/// written alone, leaving the scene file as it is.
	#[tokio::test(start_paused = true)]
	async fn writes_a_change_to_the_history_alone() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let history = dir.path().join("show.history.toml");
		std::fs::write(&path, HAND_WRITTEN).unwrap();
		let running = Running::start(path.clone()).await;
		assert!(!history.exists(), "no history until an edit");

		running.edit("A");
		let undone = running.undo();
		kio::clock::sleep(QUIET + Duration::from_millis(100)).await;
		assert_eq!(read(&path), HAND_WRITTEN);
		assert_eq!(read(&history), undone.history);
	}

	/// The history saved beside a scene file loads with it, and one that
	/// cannot be read stops the studio, naming the file, rather than being
	/// overwritten on the next save.
	#[test]
	fn loads_the_history_beside_the_scene_file() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let history = dir.path().join("show.history.toml");
		assert_eq!(history_path(&path), history);

		let saved = Doc::new(Collection::new());
		saved.lock().unwrap().edit("Two");
		let saved = saved.into_inner().unwrap();
		std::fs::write(&path, saved.collection.to_toml().unwrap()).unwrap();
		std::fs::write(&history, saved.history.to_toml().unwrap()).unwrap();
		let (autosave, mut loaded) = Autosave::open(path.clone()).unwrap();
		assert_eq!((&loaded.collection, loaded.dropped), (&saved.collection, 0));
		loaded.history.undo(&mut loaded.collection).unwrap();
		assert_eq!(loaded.collection, Collection::new());

		std::fs::write(&history, "not a history").unwrap();
		let err = format!("{:#}", autosave.reload(&Doc::new(Collection::new())).unwrap_err());
		assert!(err.contains("show.history.toml"), "{err}");
		let err = format!("{:#}", Autosave::open(path).err().unwrap());
		assert!(err.contains("show.history.toml"), "{err}");
	}
}
