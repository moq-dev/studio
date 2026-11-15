//! The screen-sharing grants this machine holds, so a display or window the
//! system picker granted reopens without the picker, across restarts.
//!
//! A grant is the xdg-desktop-portal's restore token: a credential for this
//! user on this machine, so it lives in a per-machine state file, never in the
//! scene file, which stays portable. The file is TOML, one table per scene
//! file, keyed by source name, readable by its owner alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::Error;

/// Every scene file's grants, by source name.
type Tables = BTreeMap<String, BTreeMap<String, String>>;

/// The grants one scene file's picked sources hold, kept in a state file
/// shared with every other scene file and studio.
#[derive(Clone)]
pub struct Grants {
	file: PathBuf,
	/// The scene file, as the table's key.
	collection: String,
	owners: Arc<Mutex<Owners>>,
}

/// Which [`Claim`] may write each source's grant: the newest, until the
/// source is removed, so a capture that outlives its source writes nothing.
#[derive(Default)]
struct Owners {
	claims: BTreeMap<String, u64>,
	next: u64,
}

impl Grants {
	/// The grants of the scene file at `collection`, kept in `file`, which is
	/// created when the first grant is stored.
	pub fn new(file: PathBuf, collection: &Path) -> Self {
		Self {
			file,
			collection: collection.to_string_lossy().into_owned(),
			owners: Arc::default(),
		}
	}

	/// Take over writing the grant of the source called `source`, from any
	/// earlier claim on it.
	#[cfg_attr(
		not(any(test, all(target_os = "linux", feature = "pipewire"))),
		expect(dead_code, reason = "only the portal opens with a grant")
	)]
	pub(crate) fn claim(&self, source: &str) -> Claim {
		let mut owners = self.owners.lock().unwrap();
		let generation = owners.next;
		owners.next += 1;
		owners.claims.insert(source.to_string(), generation);
		Claim {
			grants: self.clone(),
			source: source.to_string(),
			generation,
		}
	}

	/// Forget the grant of every source not in `keep`, such as a removed one,
	/// and stop its claims from writing one again.
	pub(crate) fn retain(&self, keep: &BTreeSet<String>) -> Result<(), Error> {
		let mut owners = self.owners.lock().unwrap();
		owners.claims.retain(|source, _| keep.contains(source));
		self.change(|grants| {
			let before = grants.len();
			grants.retain(|source, _| keep.contains(source));
			grants.len() != before
		})
	}

	/// Apply `change` to this scene file's table, writing the file back only
	/// when `change` says it changed something.
	fn change(&self, change: impl FnOnce(&mut BTreeMap<String, String>) -> bool) -> Result<(), Error> {
		let _lock = self.lock(true)?;
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

	/// Lock the file against other studios, `exclusive` to change it, until
	/// the returned file drops. The lock is a file of its own, since a write
	/// replaces the grants file.
	fn lock(&self, exclusive: bool) -> Result<std::fs::File, Error> {
		let lock = self.file.with_extension("lock");
		if let Some(dir) = lock.parent().filter(|dir| !dir.as_os_str().is_empty()) {
			std::fs::create_dir_all(dir).map_err(|err| self.failed(err))?;
		}
		let file = std::fs::File::options()
			.create(true)
			.truncate(false)
			.write(true)
			.open(&lock)
			.map_err(|err| self.failed(err))?;
		match exclusive {
			true => file.lock(),
			false => file.lock_shared(),
		}
		.map_err(|err| self.failed(err))?;
		Ok(file)
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
		let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|err| self.failed(err))?;
		temp.write_all(text.as_bytes()).map_err(|err| self.failed(err))?;
		temp.as_file().sync_all().map_err(|err| self.failed(err))?;
		temp.persist(&self.file).map_err(|err| self.failed(err.error))?;
		// The rename is durable only once its directory is synced.
		#[cfg(unix)]
		std::fs::File::open(dir)
			.and_then(|dir| dir.sync_all())
			.map_err(|err| self.failed(err))?;
		Ok(())
	}

	fn failed(&self, err: impl std::fmt::Display) -> Error {
		Error::Grants(format!("{}: {err}", self.file.display()))
	}
}

/// The right to read and write one source's grant, which a newer claim on the
/// source, or the source's removal, takes away.
#[cfg_attr(
	not(any(test, all(target_os = "linux", feature = "pipewire"))),
	expect(dead_code, reason = "only the portal opens with a grant")
)]
pub(crate) struct Claim {
	grants: Grants,
	source: String,
	generation: u64,
}

#[cfg_attr(
	not(any(test, all(target_os = "linux", feature = "pipewire"))),
	expect(dead_code, reason = "only the portal opens with a grant")
)]
impl Claim {
	/// The grant stored for the source.
	pub(crate) fn get(&self) -> Result<Option<String>, Error> {
		let grants = &self.grants;
		let _lock = grants.lock(false)?;
		let tables = grants.read()?;
		Ok(tables
			.get(&grants.collection)
			.and_then(|table| table.get(&self.source))
			.cloned())
	}

	/// Store `grant` for the source, or forget its grant on `None`, unless
	/// the claim was taken away.
	pub(crate) fn set(&self, grant: Option<&str>) -> Result<(), Error> {
		let owners = self.grants.owners.lock().unwrap();
		if owners.claims.get(&self.source) != Some(&self.generation) {
			return Ok(());
		}
		let source = &self.source;
		self.grants.change(|grants| match grant {
			Some(grant) => grants.insert(source.clone(), grant.to_string()).as_deref() != Some(grant),
			None => grants.remove(source).is_some(),
		})
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
	use super::*;

	#[test]
	fn each_scene_file_keeps_its_own_grants() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("state/portal.toml");
		let one = Grants::new(file.clone(), Path::new("/scenes/one.toml")).claim("Screen");
		let two = Grants::new(file.clone(), Path::new("/scenes/two.toml")).claim("Screen");

		assert_eq!(one.get().unwrap(), None);
		assert!(!file.exists(), "reading creates nothing");
		one.set(Some("a")).unwrap();
		two.set(Some("b")).unwrap();
		assert_eq!(one.get().unwrap().as_deref(), Some("a"));
		assert_eq!(two.get().unwrap().as_deref(), Some("b"));

		// Another process reads what this one wrote.
		let again = Grants::new(file.clone(), Path::new("/scenes/one.toml")).claim("Screen");
		assert_eq!(again.get().unwrap().as_deref(), Some("a"));

		one.set(None).unwrap();
		assert_eq!(one.get().unwrap(), None);
		assert_eq!(two.get().unwrap().as_deref(), Some("b"));
		let text = std::fs::read_to_string(&file).unwrap();
		assert!(!text.contains("one.toml"), "an empty table is dropped: {text}");
	}

	#[test]
	fn retaining_forgets_removed_sources() {
		let dir = tempfile::tempdir().unwrap();
		let grants = Grants::new(dir.path().join("portal.toml"), Path::new("/scene.toml"));
		let kept = grants.claim("Kept");
		let removed = grants.claim("Removed");
		kept.set(Some("a")).unwrap();
		removed.set(Some("b")).unwrap();
		grants.retain(&BTreeSet::from(["Kept".to_string()])).unwrap();
		assert_eq!(kept.get().unwrap().as_deref(), Some("a"));
		assert_eq!(removed.get().unwrap(), None);
	}

	#[test]
	fn only_the_newest_claim_of_a_present_source_writes() {
		let dir = tempfile::tempdir().unwrap();
		let grants = Grants::new(dir.path().join("portal.toml"), Path::new("/scene.toml"));

		// A capture still running after its source was removed.
		let removed = grants.claim("Screen");
		grants.retain(&BTreeSet::new()).unwrap();
		removed.set(Some("resurrected")).unwrap();
		assert_eq!(removed.get().unwrap(), None);

		// A capture superseded by its source's restart.
		let old = grants.claim("Screen");
		let new = grants.claim("Screen");
		new.set(Some("new")).unwrap();
		old.set(None).unwrap();
		old.set(Some("old")).unwrap();
		assert_eq!(new.get().unwrap().as_deref(), Some("new"));
	}

	#[test]
	fn an_unchanged_grant_leaves_the_file_alone() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		let grants = Grants::new(file.clone(), Path::new("/scene.toml"));
		let claim = grants.claim("Screen");
		grants.retain(&BTreeSet::from(["Screen".to_string()])).unwrap();
		claim.set(None).unwrap();
		assert!(!file.exists(), "nothing to write");
		claim.set(Some("a")).unwrap();
		std::fs::write(&file, "edited = {}\n[\"/scene.toml\"]\nScreen = \"a\"\n").unwrap();
		claim.set(Some("a")).unwrap();
		assert!(std::fs::read_to_string(&file).unwrap().starts_with("edited"));
	}

	#[test]
	fn studios_sharing_the_file_lose_no_grant() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		// Each its own `Grants`, as each studio process has.
		let threads: Vec<_> = (0..4)
			.map(|studio| {
				let file = file.clone();
				std::thread::spawn(move || {
					let scene = format!("/scenes/{studio}.toml");
					let claim = Grants::new(file, Path::new(&scene)).claim("Screen");
					for round in 0..8 {
						claim.set(Some(&format!("{studio}-{round}"))).unwrap();
					}
				})
			})
			.collect();
		for thread in threads {
			thread.join().unwrap();
		}
		for studio in 0..4 {
			let scene = format!("/scenes/{studio}.toml");
			let claim = Grants::new(file.clone(), Path::new(&scene)).claim("Screen");
			assert_eq!(claim.get().unwrap(), Some(format!("{studio}-7")));
		}
	}

	#[cfg(unix)]
	#[test]
	fn only_its_owner_reads_the_file() {
		use std::os::unix::fs::PermissionsExt;

		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		Grants::new(file.clone(), Path::new("/scene.toml"))
			.claim("Screen")
			.set(Some("a"))
			.unwrap();
		let mode = std::fs::metadata(&file).unwrap().permissions().mode();
		assert_eq!(mode & 0o077, 0, "mode {mode:o}");
	}

	#[test]
	fn a_malformed_file_fails_naming_it() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("portal.toml");
		std::fs::write(&file, "not toml [").unwrap();
		let claim = Grants::new(file, Path::new("/scene.toml")).claim("Screen");
		let err = claim.get().unwrap_err().to_string();
		assert!(err.contains("portal.toml"), "{err}");
		assert!(claim.set(Some("a")).is_err(), "never overwrites what it cannot read");
	}
}
