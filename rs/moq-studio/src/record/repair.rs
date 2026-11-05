//! Finishing the recordings a crash or power loss left unfinished.
//!
//! Each file a recording writes has a sidecar marker beside it while it is
//! written (see [`Marker`](super::Marker)). A marker found at launch whose
//! lock nobody holds names a file that never finished, which is repaired into
//! a regular MP4 beside it, `<name>.repaired.mp4`, leaving the original as it
//! was. Removing the marker after the repair keeps the file from being
//! repaired again.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use moq_mux::container::mp4;

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
	/// It holds no recording that can be repaired, or writing the repair
	/// failed. A failed write leaves the marker, so the next launch tries again.
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
		let paths = unfinished(dir);
		let list: Vec<_> = paths
			.iter()
			.map(|path| Repair {
				path: path.clone(),
				outcome: None,
			})
			.collect();
		let list = Arc::new(Mutex::new(list));
		if !paths.is_empty() {
			let list = list.clone();
			runtime.spawn_blocking(move || {
				for (index, path) in paths.iter().enumerate() {
					let outcome = repair(path);
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

/// Each file in `dir` whose marker no running recording holds, in name
/// order. A marker whose file is gone is removed.
fn unfinished(dir: &Path) -> Vec<PathBuf> {
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		// Nothing was ever recorded there.
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
		Err(err) => {
			tracing::warn!(dir = %dir.display(), error = %err, "cannot look for unfinished recordings");
			return Vec::new();
		}
	};
	let mut paths: Vec<PathBuf> = entries
		.filter_map(|entry| {
			let marker = entry.ok()?.path();
			let name = marker.file_name()?.to_str()?.strip_suffix(super::MARKER)?;
			if !name.ends_with(".mp4") || held(&marker) {
				return None;
			}
			let recording = dir.join(name);
			if !recording.exists() {
				tracing::debug!(marker = %marker.display(), "removing the marker of a recording that is gone");
				remove(&marker);
				return None;
			}
			Some(recording)
		})
		.collect();
	paths.sort();
	paths
}

/// Whether a running recording, such as another studio's into the same
/// directory, holds `marker`'s lock.
fn held(marker: &Path) -> bool {
	let Ok(file) = std::fs::File::open(marker) else {
		return false;
	};
	matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// Repair the unfinished recording at `path`, reporting it in the log.
fn repair(path: &Path) -> Repaired {
	let outcome = write(path);
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

/// Write the repair of `path` beside it, and remove its marker unless writing
/// failed, which the next launch tries again.
fn write(path: &Path) -> Repaired {
	let outcome = if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == 0) {
		Repaired::Empty
	} else {
		match read(path) {
			Ok(Some(finish)) => {
				let repaired = repaired(path);
				if let Err(err) = copy(path, &repaired, &finish) {
					return Repaired::Failed(format!("{err:#}"));
				}
				Repaired::Saved(repaired)
			}
			Ok(None) => Repaired::Finished,
			Err(err) => Repaired::Failed(format!("{err:#}")),
		}
	};
	remove(&super::marker(path));
	outcome
}

/// The writes that finish `path`, or `None` when it is already a regular MP4.
fn read(path: &Path) -> anyhow::Result<Option<mp4::Finish>> {
	let mut file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
	mp4::repair(&mut file).context("not a recording the studio can repair")
}

/// Copy `path` to `repaired` with `finish` applied, through a temporary file
/// so a repair cut short never leaves a file under the repaired name.
fn copy(path: &Path, repaired: &Path, finish: &mp4::Finish) -> anyhow::Result<()> {
	let partial = partial(repaired);
	let written = apply(path, &partial, finish).and_then(|()| std::fs::rename(&partial, repaired));
	if written.is_err() {
		remove(&partial);
	}
	written.with_context(|| format!("cannot write {}", repaired.display()))
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

/// Where the repair of the recording `path` goes: `<name>.repaired.mp4`.
pub(super) fn repaired(path: &Path) -> PathBuf {
	path.with_extension(super::REPAIRED.trim_start_matches('.'))
}

/// Where the repair is written before it takes its name.
pub(super) fn partial(repaired: &Path) -> PathBuf {
	let mut name = repaired.as_os_str().to_owned();
	name.push(super::PARTIAL);
	name.into()
}

/// Remove `path`, which may already be gone.
fn remove(path: &Path) {
	if let Err(err) = std::fs::remove_file(path)
		&& err.kind() != std::io::ErrorKind::NotFound
	{
		tracing::warn!(path = %path.display(), error = %err, "cannot remove");
	}
}
