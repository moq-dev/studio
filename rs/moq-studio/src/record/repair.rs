//! Finishing the recordings a crash or power loss left unfinished.
//!
//! Each file a recording writes has a sidecar marker beside it while it is
//! written (see [`Marker`](super::Marker)). A marker found at launch whose
//! lock nobody holds names a file that never finished. The launch takes the
//! lock for as long as it repairs the file into a regular MP4 beside it,
//! `<name>.repaired.mp4`, numbered rather than replacing a file already
//! there, leaving the original as it was, so neither a
//! recording nor another studio's launch touches it meanwhile. Removing the
//! marker after the repair keeps the file from being repaired again.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use moq_mux::container::mp4;

use super::Marker;

/// A recording found unfinished at launch.
#[derive(Clone, Debug)]
pub struct Repair {
	/// The unfinished file, which the repair leaves as it was.
	pub path: PathBuf,
	/// `None` while it is being repaired.
	pub outcome: Option<Repaired>,
}

/// How a repair ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Repaired {
	/// Written as a regular MP4 at this path.
	Saved(PathBuf),
	/// Already a regular MP4: the studio stopped just after finishing it.
	Finished,
	/// Nothing reached it before the studio stopped.
	Empty,
	/// It holds no recording that can be repaired, or reading it or writing
	/// the repair failed. A failure to read or write leaves the marker, so the
	/// next launch tries again.
	Failed(String),
}

/// The repairs a launch started, running in the background.
pub struct Repairs {
	list: Arc<Mutex<Vec<Repair>>>,
}

impl Repairs {
	/// Repair each recording in `dir` that a crash left unfinished, on
	/// `runtime`'s blocking pool, so a long file never holds up the launch.
	///
	/// Only the markers there now count, not those of recordings started later.
	pub fn start(dir: &Path, runtime: &tokio::runtime::Handle) -> Self {
		let claimed = unfinished(dir);
		let list: Vec<_> = claimed
			.iter()
			.map(|(path, _)| Repair {
				path: path.clone(),
				outcome: None,
			})
			.collect();
		let list = Arc::new(Mutex::new(list));
		if !claimed.is_empty() {
			let list = list.clone();
			runtime.spawn_blocking(move || {
				for (index, (path, marker)) in claimed.into_iter().enumerate() {
					let outcome = repair(&path, marker);
					list.lock().unwrap()[index].outcome = Some(outcome);
				}
			});
		}
		Self { list }
	}

	/// Every recording found unfinished at launch, in name order.
	pub fn list(&self) -> Vec<Repair> {
		self.list.lock().unwrap().clone()
	}
}

/// Each file in `dir` whose marker no running recording or repair holds, in
/// name order, with its marker claimed. A marker whose file is gone is removed.
fn unfinished(dir: &Path) -> Vec<(PathBuf, Marker)> {
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		// Nothing was ever recorded there.
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
		Err(err) => {
			tracing::warn!(dir = %dir.display(), error = %err, "cannot look for unfinished recordings");
			return Vec::new();
		}
	};
	let mut claimed: Vec<_> = entries
		.filter_map(|entry| {
			let path = entry.ok()?.path();
			let name = path.file_name()?.to_str()?.strip_suffix(super::MARKER)?;
			if !name.ends_with(".mp4") {
				return None;
			}
			let recording = dir.join(name);
			let marker = Marker::claim(path)?;
			if !recording.exists() {
				tracing::debug!(marker = %marker.path.display(), "removing the marker of a recording that is gone");
				marker.clear();
				return None;
			}
			Some((recording, marker))
		})
		.collect();
	claimed.sort_by(|(a, _), (b, _)| a.cmp(b));
	claimed
}

impl Marker {
	/// Claim the marker at `path` for a repair, unless a recording, such as
	/// another studio's into the same directory, or another launch's repair
	/// holds it.
	fn claim(path: PathBuf) -> Option<Self> {
		// Writable, as an exclusive lock needs on NFS.
		let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
			Ok(file) => file,
			// Its recording finished meanwhile.
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
			Err(err) => {
				tracing::warn!(path = %path.display(), error = %err, "cannot open a recording's marker");
				return None;
			}
		};
		match file.try_lock() {
			Ok(()) => {}
			Err(std::fs::TryLockError::WouldBlock) => return None,
			// A file system without locks: claimed, as the recording could not
			// lock it either.
			Err(std::fs::TryLockError::Error(err)) => {
				tracing::debug!(path = %path.display(), error = %err, "cannot lock a recording's marker");
			}
		}
		// Whoever held it before removed it, and a new recording may have
		// marked the same name since.
		match Self::named(&path, &file) {
			Ok(true) => {}
			Ok(false) => return None,
			Err(err) => {
				tracing::warn!(error = %format!("{err:#}"), "cannot claim a recording's marker");
				return None;
			}
		}
		Some(Self { path, file })
	}
}

/// Repair the unfinished recording at `path`, reporting it in the log, and
/// remove its `marker` unless the next launch should try again.
fn repair(path: &Path, marker: Marker) -> Repaired {
	let (outcome, again) = write(path);
	if !again {
		marker.clear();
	}
	match &outcome {
		Repaired::Saved(repaired) => tracing::warn!(
			path = %path.display(),
			repaired = %repaired.display(),
			"repaired a recording the studio left unfinished"
		),
		Repaired::Finished => tracing::info!(path = %path.display(), "an unfinished recording was already finished"),
		Repaired::Empty => tracing::info!(path = %path.display(), "an unfinished recording holds nothing"),
		Repaired::Failed(reason) => tracing::error!(
			path = %path.display(),
			error = %reason,
			"cannot repair a recording the studio left unfinished"
		),
	}
	outcome
}

/// Write the repair of `path` beside it. Also returns whether the next launch
/// should try again: after a failure to read or write rather than a file that
/// holds no recording.
fn write(path: &Path) -> (Repaired, bool) {
	if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == 0) {
		return (Repaired::Empty, false);
	}
	let failed = |err: anyhow::Error| Repaired::Failed(format!("{err:#}"));
	let finish = match read(path) {
		Ok(Some(finish)) => finish,
		Ok(None) => return (Repaired::Finished, false),
		Err(err) => {
			let again = io(&err);
			return (failed(err), again);
		}
	};
	match copy(path, &finish) {
		Ok(repaired) => (Repaired::Saved(repaired), false),
		Err(err) => (failed(err), true),
	}
}

/// Whether `err` is a failure to read or write rather than what was read.
fn io(err: &anyhow::Error) -> bool {
	err.chain().any(|err| {
		err.is::<std::io::Error>() || matches!(err.downcast_ref::<moq_mux::Error>(), Some(moq_mux::Error::Io(_)))
	})
}

/// The writes that finish `path`, or `None` when it is already a regular MP4.
fn read(path: &Path) -> anyhow::Result<Option<mp4::Finish>> {
	let mut file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
	mp4::repair(&mut file).context("not a recording the studio can repair")
}

/// Copy `path` with `finish` applied to `<name>.repaired.mp4` beside it,
/// numbered rather than replacing a file already there, such as the recording
/// of a source named like a repair.
fn copy(path: &Path, finish: &mp4::Finish) -> anyhow::Result<PathBuf> {
	let dir = path.parent().context("a recording has a directory")?;
	let stem = stem(path)?;
	// Written in a directory of its own first, which nothing else can name, so
	// a repair cut short never leaves a file under a repaired name. Dropping
	// it removes what is left.
	let staging = tempfile::Builder::new()
		.prefix(".repairing")
		.tempdir_in(dir)
		.with_context(|| format!("cannot create a directory in {}", dir.display()))?;
	let partial = staging.path().join("repair.mp4");
	apply(path, &partial, finish).with_context(|| format!("cannot copy {}", path.display()))?;
	let repaired = claim(dir, &stem)?;
	// Replaces only the empty file just claimed.
	std::fs::rename(&partial, &repaired)
		.inspect_err(|_| remove(&repaired))
		.with_context(|| format!("cannot write {}", repaired.display()))?;
	Ok(repaired)
}

/// The stem of the repair of the recording `path`: `<name>.repaired`.
pub(super) fn stem(path: &Path) -> anyhow::Result<String> {
	let name = path.file_stem().and_then(|name| name.to_str());
	let name = name.with_context(|| format!("{} has no name", path.display()))?;
	Ok(format!("{name}{}", super::REPAIRED))
}

/// Create an empty file named `stem` in `dir`, numbering it rather than
/// replacing one already there.
fn claim(dir: &Path, stem: &str) -> anyhow::Result<PathBuf> {
	let mut number: u32 = 1;
	loop {
		let path = dir.join(super::numbered(stem, number));
		match std::fs::File::create_new(&path) {
			Ok(_) => return Ok(path),
			Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
				number = number.checked_add(1).context("every numbered name is taken")?;
			}
			Err(err) => return Err(err).with_context(|| format!("cannot create {}", path.display())),
		}
	}
}

/// Copy `path` to `to` and finish the copy with `finish`, as a recording
/// finishes: cut where the moov goes, the moov, then the mdat header.
fn apply(path: &Path, to: &Path, finish: &mp4::Finish) -> std::io::Result<()> {
	// A clone on file systems that copy on write, so even a long recording
	// repairs at once there.
	std::fs::copy(path, to)?;
	let mut file = std::fs::OpenOptions::new().write(true).open(to)?;
	file.set_len(finish.moov_offset)?;
	file.seek(SeekFrom::Start(finish.moov_offset))?;
	file.write_all(&finish.moov)?;
	file.seek(SeekFrom::Start(finish.mdat_offset))?;
	file.write_all(&finish.mdat)?;
	file.sync_all()
}

/// Remove `path`, which may already be gone.
fn remove(path: &Path) {
	if let Err(err) = std::fs::remove_file(path)
		&& err.kind() != std::io::ErrorKind::NotFound
	{
		tracing::warn!(path = %path.display(), error = %err, "cannot remove");
	}
}
