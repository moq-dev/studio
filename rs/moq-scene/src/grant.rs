//! The screen-sharing grants this machine holds, so a display or window the
//! system picker granted reopens without the picker, across restarts.
//!
//! A grant is the xdg-desktop-portal's restore token: a credential for this
//! user on this machine, so it lives in a per-machine state file, never in the
//! scene file, which stays portable. The file is TOML, one table per scene
//! file, keyed by source name, readable by its owner alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::Error;

/// Every scene file's grants, by source name.
type Tables = BTreeMap<String, BTreeMap<String, String>>;

/// The grants one scene file's picked sources hold, kept in a state file
/// shared with every other scene file.
#[derive(Clone)]
pub struct Grants {
	file: PathBuf,
	/// The scene file, as the table's key.
	collection: String,
	/// Read, change, and write happen as one within this process.
	lock: Arc<Mutex<()>>,
}

impl Grants {
	/// The grants of the scene file at `collection`, kept in `file`, which is
	/// created when the first grant is stored.
	pub fn new(file: PathBuf, collection: &Path) -> Self {
		Self {
			file,
			collection: collection.to_string_lossy().into_owned(),
			lock: Arc::default(),
		}
	}

	/// The grant stored for the source called `source`.
	#[cfg_attr(
		not(any(test, all(target_os = "linux", feature = "pipewire"))),
		expect(dead_code, reason = "only the portal opens with a grant")
	)]
	pub(crate) fn get(&self, source: &str) -> Result<Option<String>, Error> {
		let _lock = self.lock.lock().unwrap();
		let tables = self.read()?;
		Ok(tables
			.get(&self.collection)
			.and_then(|grants| grants.get(source))
			.cloned())
	}

	/// Store `grant` for the source called `source`, or forget its grant on `None`.
	#[cfg_attr(
		not(any(test, all(target_os = "linux", feature = "pipewire"))),
		expect(dead_code, reason = "only the portal opens with a grant")
	)]
	pub(crate) fn set(&self, source: &str, grant: Option<&str>) -> Result<(), Error> {
		self.change(|grants| match grant {
			Some(grant) => grants.insert(source.to_string(), grant.to_string()).as_deref() != Some(grant),
			None => grants.remove(source).is_some(),
		})
	}

	/// Forget the grant of every source not in `keep`, such as a removed one.
	pub(crate) fn retain(&self, keep: &std::collections::BTreeSet<String>) -> Result<(), Error> {
		self.change(|grants| {
			let before = grants.len();
			grants.retain(|source, _| keep.contains(source));
			grants.len() != before
		})
	}

	/// Apply `change` to this scene file's table, writing the file back only
	/// when `change` says it changed something.
	fn change(&self, change: impl FnOnce(&mut BTreeMap<String, String>) -> bool) -> Result<(), Error> {
		let _lock = self.lock.lock().unwrap();
		let mut tables = self.read()?;
		let grants = tables.entry(self.collection.clone()).or_default();
		if !change(grants) {
			return Ok(());
		}
		if grants.is_empty() {
			tables.remove(&self.collection);
		}
		self.write(&tables)
	}

	fn read(&self) -> Result<Tables, Error> {
		let text = match std::fs::read_to_string(&self.file) {
			Ok(text) => text,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Tables::new()),
			Err(err) => return Err(self.failed(err)),
		};
		toml::from_str(&text).map_err(|err| self.failed(err))
	}

	/// Replace the file whole, so a crash leaves the old one or the new one,
	/// created readable by its owner alone, since the grants are credentials.
	fn write(&self, tables: &Tables) -> Result<(), Error> {
		use std::io::Write;

		let text = toml::to_string(tables).map_err(|err| self.failed(err))?;
		let dir = self.file.parent().filter(|dir| !dir.as_os_str().is_empty());
		let dir = dir.unwrap_or(Path::new("."));
		std::fs::create_dir_all(dir).map_err(|err| self.failed(err))?;
		let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|err| self.failed(err))?;
		temp.write_all(text.as_bytes()).map_err(|err| self.failed(err))?;
		temp.as_file().sync_all().map_err(|err| self.failed(err))?;
		temp.persist(&self.file).map_err(|err| self.failed(err.error))?;
		Ok(())
	}

	fn failed(&self, err: impl std::fmt::Display) -> Error {
		Error::Grants(format!("{}: {err}", self.file.display()))
	}
}

impl std::fmt::Debug for Grants {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Grants")
			.field("file", &self.file)
			.field("collection", &self.collection)
			.finish_non_exhaustive()
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use super::*;

	#[test]
	fn each_scene_file_keeps_its_own_grants() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("state/portal.toml");
		let one = Grants::new(file.clone(), Path::new("/scenes/one.toml"));
		let two = Grants::new(file.clone(), Path::new("/scenes/two.toml"));

		assert_eq!(one.get("Screen").unwrap(), None);
		assert!(!file.exists(), "reading creates nothing");
		one.set("Screen", Some("a")).unwrap();
		two.set("Screen", Some("b")).unwrap();
		assert_eq!(one.get("Screen").unwrap().as_deref(), Some("a"));
		assert_eq!(two.get("Screen").unwrap().as_deref(), Some("b"));

		// Another process reads what this one wrote.
		let again = Grants::new(file.clone(), Path::new("/scenes/one.toml"));
		assert_eq!(again.get("Screen").unwrap().as_deref(), Some("a"));

		one.set("Screen", None).unwrap();
		assert_eq!(one.get("Screen").unwrap(), None);
		assert_eq!(two.get("Screen").unwrap().as_deref(), Some("b"));
		let text = std::fs::read_to_string(&file).unwrap();
		assert!(!text.contains("one.toml"), "an empty table is dropped: {text}");
	}

	#[test]
	fn retaining_forgets_removed_sources() {
		let dir = tempfile::tempdir().unwrap();
		let grants = Grants::new(dir.path().join("portal.toml"), Path::new("/scene.toml"));
		grants.set("Kept", Some("a")).unwrap();
		grants.set("Removed", Some("b")).unwrap();
		grants.retain(&BTreeSet::from(["Kept".to_string()])).unwrap();
		assert_eq!(grants.get("Kept").unwrap().as_deref(), Some("a"));
		assert_eq!(grants.get("Removed").unwrap(), None);
	}

	#[test]
	fn an_unchanged_grant_leaves_the_file_alone() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		let grants = Grants::new(file.clone(), Path::new("/scene.toml"));
		grants.retain(&BTreeSet::new()).unwrap();
		grants.set("Gone", None).unwrap();
		assert!(!file.exists(), "nothing to write");
		grants.set("Screen", Some("a")).unwrap();
		std::fs::write(&file, "edited = {}\n[\"/scene.toml\"]\nScreen = \"a\"\n").unwrap();
		grants.set("Screen", Some("a")).unwrap();
		assert!(std::fs::read_to_string(&file).unwrap().starts_with("edited"));
	}

	#[cfg(unix)]
	#[test]
	fn only_its_owner_reads_the_file() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		Grants::new(file.clone(), Path::new("/scene.toml"))
			.set("Screen", Some("a"))
			.unwrap();
		let mode = std::fs::metadata(&file).unwrap().permissions().mode();
		assert_eq!(mode & 0o077, 0, "mode {mode:o}");
	}

	#[test]
	fn a_malformed_file_fails_naming_it() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		std::fs::write(&file, "not toml [").unwrap();
		let grants = Grants::new(file, Path::new("/scene.toml"));
		let err = grants.get("Screen").unwrap_err().to_string();
		assert!(err.contains("portal.toml"), "{err}");
		assert!(
			grants.set("Screen", Some("a")).is_err(),
			"never overwrites what it cannot read"
		);
	}
}
