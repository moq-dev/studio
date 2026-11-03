//! The scene file and its history, written once they settle after a change.
//!
//! Each edit to the collection or its history is recorded in [`Edits`], and
//! both are written [`QUIET`] after the last one, or [`LIMIT`] after the first
//! unsaved one if the edits never pause. A write compares each serialized file
//! with what it holds, so a change undone before it is written rewrites only
//! the history, and an undo that leaves the history as written writes nothing.

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
}

struct Written {
	/// `None` until the scene file exists.
	collection: Option<String>,
	/// An empty history when there is no history file, so none is created
	/// until there is an edit to undo.
	history: String,
}

impl Autosave {
	/// Read the scene file at `path` and its history, or start a new
	/// collection if there is no scene file yet; the first save creates it, and
	/// fails rather than replace one another program created meanwhile.
	pub fn open(path: PathBuf) -> anyhow::Result<(Self, Loaded)> {
		let (collection, written) = match std::fs::read_to_string(&path) {
			Ok(text) => {
				let collection = parse(&path, &text)?;
				// Kept as the studio would write it, not as the file spells it, so
				// a hand-written file is rewritten only once the collection changes.
				let written = collection.to_toml()?;
				(collection, Some(written))
			}
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Collection::new(), None),
			Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
		};
		let history = history_path(&path);
		let loaded = load_history(&history, collection)?;
		let written = Written {
			collection: written,
			history: loaded.history.to_toml()?,
		};
		let autosave = Self {
			path,
			history,
			written: Mutex::new(written),
			edits: Edits::default(),
			error: Mutex::new(None),
		};
		Ok((autosave, loaded))
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Where the studio records its edits to the collection.
	pub fn edits(&self) -> Edits {
		self.edits.clone()
	}

	/// Why the last write failed, until one succeeds.
	pub fn error(&self) -> Option<String> {
		self.error.lock().unwrap().clone()
	}

	/// Write each file `serialize` returns that does not already hold it: the
	/// scene file, then the history, so a history never describes edits the
	/// scene file lacks. A failure stays in [`Self::error`] until a write
	/// succeeds.
	pub fn save(&self, serialize: impl FnOnce() -> anyhow::Result<Text>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		// Before serializing, so an edit made after it is recorded anew.
		self.edits.0.lock().take();
		let result = serialize().and_then(|text| {
			if written.collection.as_ref() != Some(&text.collection) {
				let new = written.collection.is_none();
				write(&self.path, &text.collection, new).with_context(|| format!("writing {}", self.path.display()))?;
				written.collection = Some(text.collection);
			}
			if written.history != text.history {
				write(&self.history, &text.history, false).with_context(|| format!("writing {}", self.history.display()))?;
				written.history = text.history;
			}
			Ok(())
		});

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

	/// Write the files that `serialize` returns now, which creates a missing
	/// scene file, and then whenever the recorded edits are due.
	pub async fn run(&self, serialize: impl Fn() -> anyhow::Result<Text>) {
		loop {
			// Kept for the status bar until a save succeeds.
			let _ = self.save(&serialize);
			let mut deadline = self.edits.next().await;
			// Another edit only moves the deadline later, so sleep until it holds.
			loop {
				kio::clock::sleep_until(deadline).await;
				match self.edits.deadline() {
					Some(next) if next > deadline => deadline = next,
					_ => break,
				}
			}
		}
	}

	/// Read the scene file and its history and hand them to `replace`. Once
	/// they are accepted, the files count as holding them.
	pub fn reload(&self, replace: impl FnOnce(Loaded) -> anyhow::Result<()>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		let text = std::fs::read_to_string(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
		let collection = parse(&self.path, &text)?;
		let loaded = load_history(&self.history, collection)?;
		let text = Text::new(&loaded.collection, &loaded.history)?;
		replace(loaded)?;
		*written = Written {
			collection: Some(text.collection),
			history: text.history,
		};
		Ok(())
	}
}

fn parse(path: &Path, text: &str) -> anyhow::Result<Collection> {
	Collection::from_toml(text).with_context(|| format!("loading {}", path.display()))
}

/// Where the history of the scene file at `scene` is saved: beside it, as
/// `show.history.toml` for `show.toml`.
fn history_path(scene: &Path) -> PathBuf {
	let mut name = scene.file_stem().unwrap_or_default().to_os_string();
	name.push(".history.toml");
	scene.with_file_name(name)
}

/// Read the history at `path` for `collection`, keeping the entries that
/// still apply to it, or start a new one if there is no history file.
fn load_history(path: &Path, collection: Collection) -> anyhow::Result<Loaded> {
	let (history, dropped) = match std::fs::read_to_string(path) {
		Ok(text) => History::from_toml(&text, &collection)
			.with_context(|| format!("loading {}; delete it to start a new history", path.display()))?,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => (History::new(), 0),
		Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
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
/// either the old file or the new one, never part of either.
///
/// A `new` file must not exist yet: one another program created since the
/// studio found none is left alone rather than replaced.
fn write(path: &Path, text: &str, new: bool) -> std::io::Result<()> {
	let path = resolve(path)?;
	let existing = match std::fs::metadata(&path) {
		Ok(_) if new => return Err(created_since()),
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
		Some(dir) if !dir.as_os_str().is_empty() => dir,
		_ => Path::new("."),
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
	let mut file = builder.tempfile_in(dir)?;
	// It replaces the old file, so it takes its permissions.
	if let Some(permissions) = existing {
		file.as_file().set_permissions(permissions)?;
	}
	file.write_all(text.as_bytes())?;
	// On disk before the rename, so a power loss cannot leave the new name
	// pointing at an empty file.
	file.as_file().sync_all()?;
	match new {
		// Fails if the file appeared since the check above.
		true => file.persist_noclobber(&path).map_err(|err| match err.error.kind() {
			std::io::ErrorKind::AlreadyExists => created_since(),
			_ => err.error,
		})?,
		false => file.persist(&path)?,
	};
	// And the rename itself, so a power loss cannot undo the save.
	#[cfg(unix)]
	std::fs::File::open(dir)?.sync_all()?;
	Ok(())
}

fn created_since() -> std::io::Error {
	std::io::Error::new(
		std::io::ErrorKind::AlreadyExists,
		"another program created it since the studio started; restart the studio to open it",
	)
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

	/// A new collection with scene `scene` added, and the history of that edit.
	fn edited(scene: &str) -> (Collection, History) {
		let mut collection = Collection::new();
		let mut history = History::new();
		let mut draft = collection.draft(format!("Add scene `{scene}`"));
		draft.insert_scene(1, moq_scene::Scene::new(scene)).unwrap();
		let edit = draft.finish().unwrap();
		collection.apply(&edit).unwrap();
		history.record(edit, None);
		(collection, history)
	}

	/// `collection` with no history, which leaves the history file unwritten.
	fn serialize(collection: &Collection) -> impl FnOnce() -> anyhow::Result<Text> {
		let text = Text::new(collection, &History::new());
		move || text
	}

	#[test]
	fn creates_a_missing_file_and_rewrites_only_a_change() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();
		assert_eq!(opened, Collection::new());

		autosave.save(serialize(&opened)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), opened.to_toml().unwrap());

		// The file is not touched while the collection matches what was written.
		std::fs::write(&path, "elsewhere").unwrap();
		autosave.save(serialize(&opened)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "elsewhere");

		let edited = collection("Second");
		autosave.save(serialize(&edited)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), edited.to_toml().unwrap());
		let names: Vec<_> = std::fs::read_dir(dir.path())
			.unwrap()
			.map(|e| e.unwrap().file_name())
			.collect();
		assert_eq!(
			names,
			["show.toml"],
			"the temporary file is renamed into place, and an empty history is not written"
		);
	}

	/// A scene file another program creates after the studio found none, such
	/// as a second studio finishing setup first, is never replaced.
	#[test]
	fn leaves_a_file_created_since_it_found_none() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();

		std::fs::write(&path, "elsewhere").unwrap();
		let err = format!("{:#}", autosave.save(serialize(&opened)).unwrap_err());
		assert!(err.contains("restart the studio"), "{err}");
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "elsewhere");
		assert!(autosave.error().is_some(), "shown until a restart");
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
		autosave.save(serialize(&opened)).unwrap();

		let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode(&path), mode(&plain));
	}

	/// A hand-written file keeps its comments and layout until the collection
	/// it describes changes.
	#[test]
	fn leaves_a_hand_written_file_alone_until_an_edit() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let text = "# my show\nversion = 1\ncanvas = { width = 1280, height = 720, framerate = 30 }\n\n[[scenes]]\nname = \"Main\"\n";
		std::fs::write(&path, text).unwrap();

		let (autosave, Loaded { collection: opened, .. }) = Autosave::open(path.clone()).unwrap();
		autosave.save(serialize(&opened)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
	}

	/// An autosave running over a collection the test edits, as the engine's
	/// does over the studio's.
	struct Running {
		autosave: Arc<Autosave>,
		state: Arc<Mutex<Loaded>>,
		task: tokio::task::JoinHandle<()>,
	}

	impl Running {
		async fn start(path: PathBuf) -> Self {
			let (autosave, opened) = Autosave::open(path).unwrap();
			let autosave = Arc::new(autosave);
			let state = Arc::new(Mutex::new(opened));
			let task = tokio::spawn({
				let autosave = autosave.clone();
				let state = state.clone();
				async move {
					autosave
						.run(|| {
							let state = state.lock().unwrap();
							Text::new(&state.collection, &state.history)
						})
						.await
				}
			});
			// Its first save, at once.
			tokio::task::yield_now().await;
			Self { autosave, state, task }
		}

		/// Add a scene, as an edit in the studio would, and return the
		/// collection it leaves serialized.
		fn edit(&self, scene: &str) -> String {
			let mut state = self.state.lock().unwrap();
			let Loaded {
				collection, history, ..
			} = &mut *state;
			let mut draft = collection.draft(format!("Add scene `{scene}`"));
			draft
				.insert_scene(collection.scenes.len(), moq_scene::Scene::new(scene))
				.unwrap();
			let edit = draft.finish().unwrap();
			collection.apply(&edit).unwrap();
			history.record(edit, None);
			self.autosave.edits().record();
			collection.to_toml().unwrap()
		}

		/// Undo the newest edit, as the studio would, and return the files it
		/// leaves serialized.
		fn undo(&self) -> Text {
			let mut state = self.state.lock().unwrap();
			let Loaded {
				collection, history, ..
			} = &mut *state;
			history.undo(collection).unwrap().expect("an edit to undo");
			self.autosave.edits().record();
			Text::new(collection, history).unwrap()
		}
	}

	impl Drop for Running {
		fn drop(&mut self) {
			self.task.abort();
		}
	}

	fn read(path: &Path) -> String {
		std::fs::read_to_string(path).unwrap()
	}

	/// A burst of edits is written once, a quiet second after the last.
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

	/// A reload the studio refuses leaves the file as it is, so the next save
	/// does not overwrite the hand edit that was refused.
	#[test]
	fn a_refused_reload_keeps_the_file() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let original = Collection::new();
		std::fs::write(&path, original.to_toml().unwrap()).unwrap();
		let (autosave, _) = Autosave::open(path.clone()).unwrap();

		let edited = collection("Edited");
		std::fs::write(&path, edited.to_toml().unwrap()).unwrap();
		assert!(autosave.reload(|_| anyhow::bail!("refused")).is_err());
		autosave.save(serialize(&original)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), edited.to_toml().unwrap());

		// Accepted, the reloaded collection is what the file holds.
		let mut reloaded = None;
		autosave
			.reload(|loaded| {
				reloaded = Some(loaded.collection);
				Ok(())
			})
			.unwrap();
		std::fs::write(&path, "elsewhere").unwrap();
		autosave.save(serialize(&reloaded.unwrap())).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "elsewhere");
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
		let edited = collection("Edited");
		autosave.save(serialize(&edited)).unwrap();

		assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
		assert_eq!(std::fs::read_to_string(&target).unwrap(), edited.to_toml().unwrap());
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
		autosave.save(serialize(&opened)).unwrap();

		assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
		assert!(std::fs::symlink_metadata(&middle).unwrap().file_type().is_symlink());
		assert_eq!(std::fs::read_to_string(&target).unwrap(), opened.to_toml().unwrap());
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
		let (collection, history) = edited("Edited");
		assert!(autosave.save(|| Text::new(&collection, &history)).is_err());
		assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
		assert!(!history_path(&path).exists());
	}

	/// An edit undone before it is written changes only the history, which is
	/// written alone, leaving the scene file as it is.
	#[tokio::test(start_paused = true)]
	async fn writes_a_change_to_the_history_alone() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let history = dir.path().join("show.history.toml");
		let running = Running::start(path.clone()).await;
		assert!(!history.exists(), "no history until an edit");

		// The file is not touched while the collection matches what was written.
		std::fs::write(&path, "elsewhere").unwrap();
		running.edit("A");
		let undone = running.undo();
		kio::clock::sleep(QUIET + Duration::from_millis(100)).await;
		assert_eq!(read(&path), "elsewhere");
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

		let (collection, saved) = edited("Two");
		std::fs::write(&path, collection.to_toml().unwrap()).unwrap();
		std::fs::write(&history, saved.to_toml().unwrap()).unwrap();
		let (autosave, mut loaded) = Autosave::open(path.clone()).unwrap();
		assert_eq!((&loaded.collection, loaded.dropped), (&collection, 0));
		loaded.history.undo(&mut loaded.collection).unwrap();
		assert_eq!(loaded.collection, Collection::new());

		std::fs::write(&history, "not a history").unwrap();
		let err = format!("{:#}", autosave.reload(|_| Ok(())).unwrap_err());
		assert!(err.contains("show.history.toml"), "{err}");
		let err = format!("{:#}", Autosave::open(path).err().unwrap());
		assert!(err.contains("show.history.toml"), "{err}");
	}
}
