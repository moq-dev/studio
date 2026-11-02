//! The scene file, written shortly after every change to the collection.
//!
//! Autosave compares the serialized collection with what the file holds rather
//! than tracking edits, so whatever changes the collection is saved, and a
//! change that is undone before the next write writes nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use moq_scene::Collection;

/// How often the collection is compared with the file: the most edits a crash
/// loses, and what a burst such as a slider drag coalesces into one write.
pub const PERIOD: Duration = Duration::from_secs(1);

pub struct Autosave {
	path: PathBuf,
	/// The collection the file holds, serialized as the studio writes it, or
	/// `None` until the file exists. Held across each read and write, so a
	/// reload and a write never interleave.
	written: Mutex<Option<String>>,
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
			error: Mutex::new(None),
		};
		Ok((autosave, collection))
	}

	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Why the last write failed, until one succeeds.
	pub fn error(&self) -> Option<String> {
		self.error.lock().unwrap().clone()
	}

	/// Write the collection that `serialize` returns, unless the file already
	/// holds it. A failure stays in [`Self::error`] until a write succeeds.
	pub fn save(&self, serialize: impl FnOnce() -> anyhow::Result<String>) -> anyhow::Result<()> {
		let mut written = self.written.lock().unwrap();
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
		result
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
	// Through a symlink to the file it names, so the link survives.
	let path = match std::fs::canonicalize(path) {
		Ok(path) => path,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
		Err(err) => return Err(err),
	};
	let mut name = std::ffi::OsString::from(".");
	name.push(path.file_name().unwrap_or_default());
	name.push(".autosave");
	let temp = path.with_file_name(name);

	let result = (|| {
		let mut file = std::fs::File::create(&temp)?;
		// The temporary file replaces the old one, so it takes its permissions.
		match std::fs::metadata(&path) {
			Ok(metadata) => file.set_permissions(metadata.permissions())?,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
			Err(err) => return Err(err),
		}
		file.write_all(text.as_bytes())?;
		// On disk before the rename, so a power loss cannot leave the new name
		// pointing at an empty file.
		file.sync_all()?;
		std::fs::rename(&temp, &path)
	})();
	if result.is_err() {
		let _ = std::fs::remove_file(&temp);
	}
	result
}

#[cfg(test)]
mod tests {
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

	#[test]
	fn reports_a_failed_write_until_one_succeeds() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("missing").join("show.toml");
		let (autosave, opened) = Autosave::open(path.clone()).unwrap();

		assert!(autosave.save(serialize(&opened)).is_err());
		assert!(autosave.error().unwrap().contains("show.toml"));

		// Still unwritten, so the next save tries again.
		std::fs::create_dir(path.parent().unwrap()).unwrap();
		autosave.save(serialize(&opened)).unwrap();
		assert_eq!(autosave.error(), None);
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
}
