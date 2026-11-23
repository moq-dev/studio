//! The shows this user has opened, newest first, kept per user in the app's
//! storage rather than beside any show. The window reopens the first on
//! launch, and the title bar's Show menu lists them.

use std::path::{Path, PathBuf};

/// Where the list is kept in the app's storage.
const KEY: &str = "recent";

/// How many shows the list keeps.
const KEPT: usize = 30;

/// The shows this user has opened, the latest first.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Recent {
	pub shows: Vec<Show>,
}

/// A show in the list.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Show {
	/// Its scene file.
	pub path: PathBuf,
	/// When it was last opened, in seconds since the Unix epoch.
	pub opened: u64,
	/// Its last time on air, if it ever went live.
	#[serde(default)]
	pub live: Option<Aired>,
}

/// A time on air.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Aired {
	/// When it went live, in seconds since the Unix epoch.
	pub at: u64,
	/// How long it stayed live, in seconds.
	pub lasted: u64,
}

/// Now, in seconds since the Unix epoch, on the ambient clock.
pub fn now() -> u64 {
	kio::clock::wall()
		.saturating_duration_since(kio::clock::Wall::UNIX_EPOCH)
		.as_secs()
}

impl Recent {
	/// The list as the app's storage holds it, or none yet.
	pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
		storage
			.and_then(|storage| eframe::get_value(storage, KEY))
			.unwrap_or_default()
	}

	pub fn save(&self, storage: &mut dyn eframe::Storage) {
		eframe::set_value(storage, KEY, self);
	}

	/// The show at `path` was opened at `at`: it moves to the front.
	pub fn opened(&mut self, path: &Path, at: u64) {
		let path = absolute(path);
		let index = self.shows.iter().position(|show| show.path == path);
		let mut show = match index {
			Some(index) => self.shows.remove(index),
			None => Show {
				path,
				opened: at,
				live: None,
			},
		};
		show.opened = at;
		self.shows.insert(0, show);
		self.shows.truncate(KEPT);
	}

	/// The show at `path` was on air as `aired`.
	pub fn aired(&mut self, path: &Path, aired: Aired) {
		if let Some(show) = self.find(path) {
			show.live = Some(aired);
		}
	}

	/// The show at `from` is at `to` now, keeping its place and when it was
	/// live.
	pub fn moved(&mut self, from: &Path, to: &Path) {
		let to = absolute(to);
		self.shows.retain(|show| show.path != to);
		if let Some(show) = self.find(from) {
			show.path = to;
		}
	}

	/// The newest show whose scene file is still there, to reopen.
	pub fn last(&self) -> Option<&Path> {
		self.shows
			.iter()
			.map(|show| show.path.as_path())
			.find(|path| path.try_exists().unwrap_or(false))
	}

	fn find(&mut self, path: &Path) -> Option<&mut Show> {
		let path = absolute(path);
		self.shows.iter_mut().find(|show| show.path == path)
	}
}

/// `path` from the root, so the list names a show once however it was
/// opened.
fn absolute(path: &Path) -> PathBuf {
	std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What the Show menu says of a show: when it was last live and for how long, or
/// when it was opened if it never went live.
pub fn said(show: &Show, now: u64) -> String {
	match show.live {
		Some(aired) => format!(
			"Live {}, for {}",
			ago(now.saturating_sub(aired.at)),
			lasted(aired.lasted)
		),
		None => format!("Opened {}, never live", ago(now.saturating_sub(show.opened))),
	}
}

/// `seconds` ago, as a person would say it.
fn ago(seconds: u64) -> String {
	const MINUTE: u64 = 60;
	const HOUR: u64 = 60 * MINUTE;
	const DAY: u64 = 24 * HOUR;
	const WEEK: u64 = 7 * DAY;
	const MONTH: u64 = 30 * DAY;
	const YEAR: u64 = 365 * DAY;
	if seconds < MINUTE {
		return "just now".into();
	}
	// Each unit, below which the next takes over, and what one of it ago is.
	let units = [
		(MINUTE, HOUR, "minute", None),
		(HOUR, DAY, "hour", None),
		(DAY, WEEK, "day", Some("yesterday")),
		(WEEK, MONTH, "week", Some("last week")),
		(MONTH, YEAR, "month", Some("last month")),
		(YEAR, u64::MAX, "year", Some("last year")),
	];
	let (unit, _, name, last) = units
		.into_iter()
		.find(|(_, below, ..)| seconds < *below)
		.unwrap_or(units[units.len() - 1]);
	match (seconds / unit, last) {
		(1, Some(last)) => last.into(),
		(1, None) => format!("1 {name} ago"),
		(count, _) => format!("{count} {name}s ago"),
	}
}

/// `seconds` on air, to the minute past the first.
fn lasted(seconds: u64) -> String {
	let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
	match (hours, minutes) {
		(0, 0) => format!("{seconds} s"),
		(0, minutes) => format!("{minutes} min"),
		(hours, 0) => format!("{hours} h"),
		(hours, minutes) => format!("{hours} h {minutes} min"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A show opened again moves to the front rather than appearing twice, and
	/// keeps when it was last live.
	#[test]
	fn reopening_moves_a_show_to_the_front() {
		let mut recent = Recent::default();
		recent.opened(Path::new("/shows/a.toml"), 10);
		recent.opened(Path::new("/shows/b.toml"), 20);
		recent.aired(Path::new("/shows/a.toml"), Aired { at: 12, lasted: 60 });
		recent.opened(Path::new("/shows/a.toml"), 30);
		let paths: Vec<_> = recent.shows.iter().map(|show| show.path.clone()).collect();
		assert_eq!(paths, [PathBuf::from("/shows/a.toml"), PathBuf::from("/shows/b.toml")]);
		assert_eq!(recent.shows[0].opened, 30);
		assert_eq!(recent.shows[0].live, Some(Aired { at: 12, lasted: 60 }));
	}

	/// A card says when the show was last live and for how long, as a person
	/// would.
	#[test]
	fn says_when_a_show_was_live() {
		let day = 24 * 3600;
		let show = |live| Show {
			path: "/shows/a.toml".into(),
			opened: 0,
			live,
		};
		let now = 400 * day;
		let aired = |ago: u64, lasted: u64| Some(Aired { at: now - ago, lasted });
		assert_eq!(
			said(&show(aired(3 * day, 3 * 3600 + 120)), now),
			"Live 3 days ago, for 3 h 2 min"
		);
		assert_eq!(said(&show(aired(8 * day, 6000)), now), "Live last week, for 1 h 40 min");
		assert_eq!(
			said(&show(aired(14 * day, 55 * 60)), now),
			"Live 2 weeks ago, for 55 min"
		);
		assert_eq!(said(&show(aired(40 * day, 7200)), now), "Live last month, for 2 h");
		assert_eq!(said(&show(None), now), "Opened last year, never live");
	}

	/// A renamed show keeps its place, and replaces any entry already at its
	/// new path; the newest still there is the one to reopen.
	#[test]
	fn a_moved_show_keeps_its_place() {
		let dir = tempfile::tempdir().unwrap();
		let [a, b, c] = ["a.toml", "b.toml", "c.toml"].map(|name| dir.path().join(name));
		let mut recent = Recent::default();
		recent.opened(&c, 5);
		recent.opened(&b, 10);
		recent.opened(&a, 20);
		recent.aired(&a, Aired { at: 12, lasted: 60 });
		recent.moved(&a, &c);
		let paths: Vec<_> = recent.shows.iter().map(|show| show.path.clone()).collect();
		assert_eq!(paths, [c.clone(), b.clone()]);
		assert_eq!(recent.shows[0].live, Some(Aired { at: 12, lasted: 60 }));

		assert_eq!(recent.last(), None, "neither file is there");
		std::fs::write(&b, "").unwrap();
		assert_eq!(recent.last(), Some(b.as_path()));
	}
}
