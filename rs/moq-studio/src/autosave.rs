//! The scene file, written once the collection settles after a change.
//!
//! Each edit to the collection is recorded in [`Edits`], and the collection is
//! written [`QUIET`] after the last one, or [`LIMIT`] after the first unsaved
//! one if the edits never pause. A write compares the serialized collection
//! with what the file holds, so a change undone before it is written writes
//! nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use anyhow::Context;
use kio::clock::Instant;
use moq_scene::Collection;

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

pub struct Autosave {
	path: PathBuf,
	/// The collection the file holds, serialized as the studio writes it, or
	/// `None` until the file exists. Held across each read and write, so a
	/// reload and a write never interleave.
	written: Mutex<Option<String>>,
	edits: Edits,
	/// Why the last write failed, until one succeeds.
	error: Mutex<Option<String>>,
}

impl Autosave {
	/// Read the scene file at `path`, or start a new collection if there is
	/// none yet; the first save creates the file.
	pub fn open(path: PathBuf) -> anyhow::Result<(Self, Collection)> {
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
		let autosave = Self {
			path,
			written: Mutex::new(written),
			edits: Edits::default(),
			error: Mutex::new(None),
		};
		Ok((autosave, collection))
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

	/// Write the collection that `serialize` returns, unless the file already
	/// holds it. A failure stays in [`Self::error`] until a write succeeds.
	pub fn save(&self, serialize: impl FnOnce() -> anyhow::Result<String>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		// Before serializing, so an edit made after it is recorded anew.
		self.edits.0.lock().take();
		let result = serialize().and_then(|text| {
			if written.as_ref() != Some(&text) {
				write(&self.path, &text).with_context(|| format!("writing {}", self.path.display()))?;
				*written = Some(text);
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

	/// Write the collection that `serialize` returns now, which creates a
	/// missing file, and then whenever the recorded edits are due.
	pub async fn run(&self, serialize: impl Fn() -> anyhow::Result<String>) {
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

	/// Read the scene file and hand its collection to `replace`. Once it is
	/// accepted, the file counts as holding it.
	pub fn reload(&self, replace: impl FnOnce(Collection) -> anyhow::Result<()>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
		let text = std::fs::read_to_string(&self.path).with_context(|| format!("reading {}", self.path.display()))?;
		let collection = parse(&self.path, &text)?;
		let text = collection.to_toml()?;
		replace(collection)?;
		*written = Some(text);
		Ok(())
	}
}

fn parse(path: &Path, text: &str) -> anyhow::Result<Collection> {
	Collection::from_toml(text).with_context(|| format!("loading {}", path.display()))
}

/// Replace the file at `path` with `text` all at once, so a crash leaves
/// either the old file or the new one, never part of either.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
	let path = resolve(path)?;
	let existing = match std::fs::metadata(&path) {
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
	file.persist(&path)?;
	// And the rename itself, so a power loss cannot undo the save.
	#[cfg(unix)]
	std::fs::File::open(dir)?.sync_all()?;
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

	fn serialize(collection: &Collection) -> impl FnOnce() -> anyhow::Result<String> {
		let text = collection.to_toml();
		move || Ok(text?)
	}

	#[test]
	fn creates_a_missing_file_and_rewrites_only_a_change() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		let (autosave, opened) = Autosave::open(path.clone()).unwrap();
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
		assert_eq!(names, ["show.toml"], "the temporary file is renamed into place");
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
		let (autosave, opened) = Autosave::open(path.clone()).unwrap();
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

		let (autosave, opened) = Autosave::open(path.clone()).unwrap();
		autosave.save(serialize(&opened)).unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
	}

	/// An autosave running over a collection the test edits, as the engine's
	/// does over the studio's.
	struct Running {
		autosave: Arc<Autosave>,
		collection: Arc<Mutex<Collection>>,
		task: tokio::task::JoinHandle<()>,
	}

	impl Running {
		async fn start(path: PathBuf) -> Self {
			let (autosave, opened) = Autosave::open(path).unwrap();
			let autosave = Arc::new(autosave);
			let collection = Arc::new(Mutex::new(opened));
			let task = tokio::spawn({
				let autosave = autosave.clone();
				let collection = collection.clone();
				async move { autosave.run(|| Ok(collection.lock().unwrap().to_toml()?)).await }
			});
			// Its first save, at once.
			tokio::task::yield_now().await;
			Self {
				autosave,
				collection,
				task,
			}
		}

		/// Add a scene, as an edit in the studio would, and return the
		/// collection it leaves serialized.
		fn edit(&self, scene: &str) -> String {
			let mut collection = self.collection.lock().unwrap();
			collection.scenes.push(moq_scene::Scene::new(scene));
			self.autosave.edits().record();
			collection.to_toml().unwrap()
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
			.reload(|collection| {
				reloaded = Some(collection);
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

		let (autosave, opened) = Autosave::open(link.clone()).unwrap();
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

		let (autosave, _) = Autosave::open(path.clone()).unwrap();
		assert!(autosave.save(serialize(&collection("Edited"))).is_err());
		assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
	}
}
