//! This show's recordings and clips, as REVIEW's cards list them: the files in
//! the recordings folder whose names start with the show's, read in the
//! background so a large folder or file never holds up the page.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kio::clock::{Instant, Wall};
use mp4_atom::{Atom, ReadAtom, ReadFrom};

/// How often the folder is listed again while REVIEW shows it.
const PERIOD: Duration = Duration::from_secs(2);

/// What a recording's start stamp looks like in its files' names:
/// `2026-11-03T12-00-00Z`.
const STAMP: usize = "2026-11-03T12-00-00Z".len();

/// What a repaired file's name adds.
const REPAIRED: &str = ".repaired";

/// The show's recordings and clips, listed again while watched.
pub struct Shelf {
	dir: PathBuf,
	/// The show's name, then the names it had before: a file named after any
	/// is its own.
	names: Vec<String>,
	runtime: tokio::runtime::Handle,
	shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
	/// When the folder was last listed.
	listed: Option<Instant>,
	/// Whether a listing runs now, so a slow disk never has two at once.
	listing: bool,
	cards: Arc<Listing>,
	lengths: Lengths,
	/// Never listed again, for the window's views.
	seeded: bool,
}

/// Each file's length, by path, with its size and when it last changed, so a
/// file is read once until it changes.
type Lengths = BTreeMap<PathBuf, (u64, Option<Wall>, Option<Duration>)>;

/// What a listing found.
#[derive(Default)]
pub struct Listing {
	/// The newest first.
	pub cards: Vec<Card>,
	/// Why the folder could not be listed.
	pub error: Option<String>,
	/// The room left on the folder's disk, in bytes.
	pub free: Option<u64>,
}

/// A recording, all its files together, or a clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
	pub kind: Kind,
	/// The file a click on it shows: the program's, or the clip.
	pub path: PathBuf,
	/// When it was last written to.
	pub modified: Option<Wall>,
	/// Every file's size together.
	pub bytes: u64,
	/// How long the program's files play together, once any is a regular MP4.
	pub length: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
	Recording {
		/// The UTC start stamp its files are named with.
		stamp: String,
		/// How many sources have files of their own beside the program's.
		sources: usize,
		/// Whether a file of it was repaired after a crash.
		repaired: bool,
	},
	/// The `n`th clip.
	Clip(u32),
}

impl Shelf {
	pub fn new(dir: &Path, names: Vec<String>, runtime: tokio::runtime::Handle) -> Self {
		Self {
			dir: dir.to_path_buf(),
			names,
			runtime,
			shared: Arc::default(),
		}
	}

	/// The folder.
	pub fn dir(&self) -> &Path {
		&self.dir
	}

	/// The show's names its files are named after, the current one first.
	pub fn names(&self) -> &[String] {
		&self.names
	}

	/// What the last listing found, listing again in the background once it
	/// is a [`PERIOD`] old.
	pub fn cards(&self) -> Arc<Listing> {
		let mut shared = self.shared.lock().unwrap();
		if !shared.seeded && !shared.listing && shared.listed.is_none_or(|listed| listed.elapsed() >= PERIOD) {
			shared.listing = true;
			let (dir, names, kept) = (self.dir.clone(), self.names.clone(), self.shared.clone());
			let lengths = shared.lengths.clone();
			self.runtime.spawn_blocking(move || {
				let (listing, lengths) = list(&dir, &names, lengths);
				let mut shared = kept.lock().unwrap();
				shared.cards = Arc::new(listing);
				shared.lengths = lengths;
				shared.listing = false;
				// Counted from when it finished, so a slow disk is not asked again at once.
				shared.listed = Some(Instant::now());
			});
		}
		shared.cards.clone()
	}

	/// Show `listing` as if listed, from now on, for the window's views.
	#[cfg(test)]
	pub fn seed(&self, listing: Listing) {
		let mut shared = self.shared.lock().unwrap();
		shared.seeded = true;
		shared.cards = Arc::new(listing);
	}
}

/// Open `path`, a folder or a recording, as the system opens it: a folder in
/// the file manager, a recording in the video player.
pub fn open(path: &Path) -> anyhow::Result<()> {
	let program = match () {
		_ if cfg!(target_os = "macos") => "open",
		_ if cfg!(windows) => "explorer",
		_ => "xdg-open",
	};
	let mut child = std::process::Command::new(program)
		.arg(path)
		.spawn()
		.map_err(|err| anyhow::anyhow!("cannot run {program} to open {}: {err}", path.display()))?;
	// Reaped once it exits, so it leaves no zombie.
	std::thread::spawn(move || child.wait());
	Ok(())
}

/// List the files in `dir` named after any of the show's `names` as cards,
/// reading the length of any file not in `lengths` or changed since.
fn list(dir: &Path, names: &[String], mut lengths: Lengths) -> (Listing, Lengths) {
	let mut listing = Listing {
		free: free(dir),
		..Listing::default()
	};
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) => {
			listing.error = Some(format!("cannot list {}: {err}", dir.display()));
			return (listing, lengths);
		}
	};
	let mut seen = BTreeMap::new();
	let mut recordings: BTreeMap<String, Gathered> = BTreeMap::new();
	for entry in entries.flatten() {
		let name = entry.file_name().to_string_lossy().into_owned();
		let Some(file) = names.iter().find_map(|show| parse(show, &name)) else {
			continue;
		};
		let Ok(metadata) = entry.metadata() else { continue };
		let path = entry.path();
		let modified = metadata
			.modified()
			.ok()
			.and_then(|modified| Wall::try_from(modified).ok());
		let bytes = metadata.len();
		let length = match lengths.remove(&path) {
			Some((size, changed, length)) if size == bytes && changed == modified => length,
			_ => length(&path),
		};
		seen.insert(path.clone(), (bytes, modified, length));
		match file {
			File::Clip(n) => listing.cards.push(Card {
				kind: Kind::Clip(n),
				path,
				modified,
				bytes,
				length,
			}),
			File::Recording {
				stamp,
				source,
				repaired,
			} => {
				let gathered = recordings.entry(stamp).or_default();
				gathered.bytes += bytes;
				gathered.modified = gathered.modified.max(modified);
				gathered.repaired |= repaired;
				match source {
					// A source's files count once, however many it was split
					// or repaired into.
					Some(source) => drop(gathered.sources.insert(source)),
					None => {
						if let Some(length) = length {
							*gathered.length.get_or_insert_default() += length;
						}
						gathered.programs.push((!repaired, path.as_os_str().len(), path));
					}
				}
			}
		}
	}
	listing
		.cards
		.extend(recordings.into_iter().filter_map(|(stamp, gathered)| {
			// The first program file, its repair over it, is what a click shows.
			let (_, _, path) = gathered.programs.into_iter().min()?;
			Some(Card {
				kind: Kind::Recording {
					stamp,
					sources: gathered.sources.len(),
					repaired: gathered.repaired,
				},
				path,
				modified: gathered.modified,
				bytes: gathered.bytes,
				length: gathered.length,
			})
		}));
	listing
		.cards
		.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
	(listing, seen)
}

/// A recording's files, as the folder is listed.
#[derive(Default)]
struct Gathered {
	bytes: u64,
	modified: Option<Wall>,
	length: Option<Duration>,
	repaired: bool,
	sources: std::collections::BTreeSet<String>,
	/// Each program file, after whether it is not a repair and how long its
	/// name is, so the first file and its repair sort first.
	programs: Vec<(bool, usize, PathBuf)>,
}

/// What a file in the recordings folder is to the show `show`.
#[derive(Debug, PartialEq)]
pub enum File {
	/// A file of the recording that started at `stamp`: the program's, or
	/// the `source`'s.
	Recording {
		stamp: String,
		source: Option<String>,
		repaired: bool,
	},
	Clip(u32),
}

/// Read a file's name as the recorder and clips name them: `<show>
/// <stamp>[ <source>][ (n)][.repaired][ (n)].mp4`, or `<show>-clip-<n>.mp4`.
pub fn parse(show: &str, name: &str) -> Option<File> {
	let stem = name.strip_suffix(".mp4")?;
	if let Some(n) = stem.strip_prefix(show).and_then(|rest| rest.strip_prefix("-clip-")) {
		return n.parse().ok().filter(|n| *n > 0).map(File::Clip);
	}
	let rest = stem.strip_prefix(show)?.strip_prefix(' ')?;
	let stamp = rest.get(..STAMP)?;
	let shaped = stamp.bytes().enumerate().all(|(at, byte)| match at {
		4 | 7 | 13 | 16 => byte == b'-',
		10 => byte == b'T',
		19 => byte == b'Z',
		_ => byte.is_ascii_digit(),
	});
	if !shaped {
		return None;
	}
	let mut tail = &rest[STAMP..];
	let repaired = match tail.find(REPAIRED) {
		Some(at) => {
			tail = &tail[..at];
			true
		}
		None => false,
	};
	let tail = strip_number(tail);
	let source = match tail {
		"" => None,
		source => Some(source.strip_prefix(' ')?.to_string()),
	};
	Some(File::Recording {
		stamp: stamp.to_string(),
		source,
		repaired,
	})
}

/// `text` without a trailing ` (n)`, the number a taken name is given.
fn strip_number(text: &str) -> &str {
	let Some(open) = text.rfind(" (") else { return text };
	let number = &text[open + 2..];
	match number.strip_suffix(')') {
		Some(digits) if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) => &text[..open],
		_ => text,
	}
}

/// How long the MP4 at `path` plays, from its movie header alone, skipping
/// its media. `None` for a fragmented one, whose header holds no length, and
/// for anything unreadable.
fn length(path: &Path) -> Option<Duration> {
	let mut file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
	loop {
		let header = <Option<mp4_atom::Header> as ReadFrom>::read_from(&mut file).ok()??;
		if header.kind != mp4_atom::Moov::KIND {
			file.seek(SeekFrom::Current(i64::try_from(header.size?).ok()?)).ok()?;
			continue;
		}
		let mut moov = (&mut file).take(header.size? as u64);
		loop {
			let child = mp4_atom::Header::read_from(&mut moov).ok()?;
			if child.kind != mp4_atom::Mvhd::KIND {
				std::io::copy(&mut (&mut moov).take(child.size? as u64), &mut std::io::sink()).ok()?;
				continue;
			}
			let mvhd = mp4_atom::Mvhd::read_atom(&child, &mut moov).ok()?;
			if mvhd.duration == 0 || mvhd.timescale == 0 {
				return None;
			}
			return Some(Duration::from_secs_f64(
				mvhd.duration as f64 / f64::from(mvhd.timescale),
			));
		}
	}
}

/// The room left for an unprivileged user on the disk holding `dir`, in bytes.
#[cfg(unix)]
fn free(dir: &Path) -> Option<u64> {
	let stat = rustix::fs::statvfs(dir).ok()?;
	Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
fn free(dir: &Path) -> Option<u64> {
	use std::os::windows::ffi::OsStrExt;
	let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
	let mut available = 0u64;
	// SAFETY: `wide` is NUL-terminated and outlives the call, and the one
	// out-pointer is a valid u64.
	let ok = unsafe {
		windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
			wide.as_ptr(),
			&mut available,
			std::ptr::null_mut(),
			std::ptr::null_mut(),
		)
	};
	(ok != 0).then_some(available)
}

#[cfg(not(any(unix, windows)))]
fn free(_dir: &Path) -> Option<u64> {
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every name the recorder, a repair, and a clip give is read back as the
	/// recording or clip it belongs to, and another show's, or a stranger's,
	/// is not.
	#[test]
	fn names_read_back() {
		let stamp = "2026-11-03T12-00-00Z";
		let program = |repaired| File::Recording {
			stamp: stamp.into(),
			source: None,
			repaired,
		};
		let source = |name: &str, repaired| File::Recording {
			stamp: stamp.into(),
			source: Some(name.into()),
			repaired,
		};
		let cases = [
			(format!("show {stamp}.mp4"), Some(program(false))),
			(format!("show {stamp} (2).mp4"), Some(program(false))),
			(format!("show {stamp}.repaired.mp4"), Some(program(true))),
			(format!("show {stamp}.repaired (3).mp4"), Some(program(true))),
			(format!("show {stamp} (2).repaired.mp4"), Some(program(true))),
			(format!("show {stamp} mic.mp4"), Some(source("mic", false))),
			(
				format!("show {stamp} desk mic (2).mp4"),
				Some(source("desk mic", false)),
			),
			(format!("show {stamp} mic.repaired.mp4"), Some(source("mic", true))),
			("show-clip-3.mp4".into(), Some(File::Clip(3))),
			("show-clip-0.mp4".into(), None),
			("show-clip-x.mp4".into(), None),
			(format!("show 2 {stamp}.mp4"), None),
			(format!("showtime {stamp}.mp4"), None),
			(format!("show {stamp}.mp4.unfinished"), None),
			("show notes.mp4".into(), None),
		];
		for (name, file) in cases {
			assert_eq!(parse("show", &name), file, "{name}");
		}
	}

	/// The folder lists each recording once with every file's size and its
	/// program files' length together, marks one repaired after a crash, and
	/// lists clips on their own, newest first.
	#[test]
	fn the_folder_lists_cards() {
		let dir = tempfile::tempdir().unwrap();
		let stamp = "2026-11-03T12-00-00Z";
		let write = |name: &str, seconds: u64, extra: usize| {
			let path = dir.path().join(name);
			std::fs::write(&path, mp4(seconds, extra)).unwrap();
			path
		};
		write(&format!("show {stamp}.mp4"), 60, 0);
		write(&format!("show {stamp} (2).mp4"), 30, 0);
		write(&format!("show {stamp} mic.mp4"), 90, 100);
		write(&format!("show {stamp} mic (2).mp4"), 90, 100);
		let crashed = "2026-11-04T12-00-00Z";
		write(&format!("show {crashed}.repaired.mp4"), 10, 0);
		std::fs::write(dir.path().join(format!("show {crashed}.mp4")), b"a fragment").unwrap();
		let clip = write("show-clip-1.mp4", 8, 0);
		write(&format!("other {stamp}.mp4"), 1, 0);
		// Recorded before the show was renamed.
		let before = "2026-11-02T12-00-00Z";
		write(&format!("earlier {before}.mp4"), 20, 0);
		// The clip is the newest.
		let later = kio::clock::wall() + Duration::from_secs(60);
		std::fs::File::options()
			.write(true)
			.open(&clip)
			.unwrap()
			.set_modified(later.into())
			.unwrap();

		let names = ["show".to_string(), "earlier".to_string()];
		let (listing, lengths) = list(dir.path(), &names, BTreeMap::new());
		assert!(listing.error.is_none());
		assert_eq!(lengths.len(), 8, "every file of the show's is remembered");
		let kinds: Vec<_> = listing.cards.iter().map(|card| (&card.kind, card.length)).collect();
		assert_eq!(kinds[0], (&Kind::Clip(1), Some(Duration::from_secs(8))));
		let recording = listing
			.cards
			.iter()
			.find(|card| matches!(&card.kind, Kind::Recording { stamp: theirs, .. } if theirs == stamp))
			.unwrap();
		assert_eq!(
			recording.kind,
			Kind::Recording {
				stamp: stamp.into(),
				sources: 1,
				repaired: false
			}
		);
		assert_eq!(recording.length, Some(Duration::from_secs(90)));
		let sizes: u64 = ["", " (2)", " mic", " mic (2)"]
			.iter()
			.map(|tail| {
				std::fs::metadata(dir.path().join(format!("show {stamp}{tail}.mp4")))
					.unwrap()
					.len()
			})
			.sum();
		assert_eq!(recording.bytes, sizes);
		assert_eq!(recording.path, dir.path().join(format!("show {stamp}.mp4")));
		let crash = listing
			.cards
			.iter()
			.find(|card| matches!(&card.kind, Kind::Recording { stamp: theirs, .. } if theirs == crashed))
			.unwrap();
		assert!(matches!(crash.kind, Kind::Recording { repaired: true, .. }));
		assert_eq!(
			crash.length,
			Some(Duration::from_secs(10)),
			"the fragment adds no length"
		);
		assert_eq!(crash.path, dir.path().join(format!("show {crashed}.repaired.mp4")));
		let earlier = listing
			.cards
			.iter()
			.find(|card| card.path.ends_with(format!("earlier {before}.mp4")));
		assert!(earlier.is_some(), "a file under an earlier name is the show's");
		assert_eq!(listing.cards.len(), 4);
	}

	/// A regular MP4's boxes, holding `seconds` in its movie header after
	/// `extra` bytes of media.
	fn mp4(seconds: u64, extra: usize) -> Vec<u8> {
		use mp4_atom::Encode;
		let mut out = Vec::new();
		mp4_atom::Ftyp {
			major_brand: b"isom".into(),
			minor_version: 0,
			compatible_brands: vec![b"isom".into()],
		}
		.encode(&mut out)
		.unwrap();
		mp4_atom::Mdat { data: vec![0; extra] }.encode(&mut out).unwrap();
		mp4_atom::Moov {
			mvhd: mp4_atom::Mvhd {
				timescale: 1000,
				duration: seconds * 1000,
				..Default::default()
			},
			..Default::default()
		}
		.encode(&mut out)
		.unwrap();
		out
	}
}
