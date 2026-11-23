//! Opening a show, creating it first when its scene file does not exist yet,
//! and the shows kept in the app's storage: naming a new one, renaming one,
//! and exporting a copy.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use moq_scene::{Canvas, Collection, History, Source};

use crate::autosave::{self, Autosave};
use crate::engine::{self, Engine, Launch};
use crate::probe;
use crate::stream::{self, Stream};

/// What a new show is called, numbered past any show already called that.
const NAME: &str = "My Show";

/// What no file name holds on some system.
const RESERVED: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// What the microphone a new show hears is called.
const MIC: &str = "Mic";

/// What opening a show takes besides its path.
#[derive(Clone)]
pub struct Opener {
	pub gpu: engine::Gpu,
	pub runtime: tokio::runtime::Handle,
	/// What the command line sets for every show this run opens.
	pub flags: stream::Flags,
	/// Every other client flag a relay is dialed with.
	pub client: moq_tokio::connect::Config,
	pub quic: moq_tokio::quic::Config,
	pub credentials: stream::Credentials,
}

impl Opener {
	/// Open the show at `path` and start its engine, creating the show when
	/// `create` and there is no scene file there yet. A new show starts on the
	/// canvas this machine encodes smoothly, hearing the system's microphone
	/// if it has one, and is saved at once. Nothing goes live, and its guests'
	/// room opens with the window, which follows the show's relays.
	pub async fn open(&self, path: PathBuf, create: bool) -> anyhow::Result<(Arc<Engine>, Stream)> {
		let mut stream = Stream::load(
			&path,
			self.flags.clone(),
			self.client.clone(),
			self.quic.clone(),
			self.credentials.clone(),
		)?;
		let (autosave, mut loaded) = Autosave::open(path.clone())?;
		let new = !autosave.exists();
		// The stream settings this writes for a new show, to take back if it
		// cannot be made, so its name stays free.
		let mut wrote = None;
		anyhow::ensure!(create || !new, "{} is no longer there", autosave.path().display());
		if new {
			let (canvas, mic) = tokio::join!(probe::first_canvas(self.gpu.clone()), microphone());
			// Nothing to undo, whatever history was left beside a removed file.
			loaded = autosave::Loaded {
				collection: first(canvas, mic)?,
				history: History::new(),
				dropped: 0,
			};
			// Kept out of the app's storage, where nobody would find them.
			if in_storage(autosave.path())
				&& let Some(videos) = videos()
			{
				let file = stream::path(&path);
				let there = file
					.try_exists()
					.with_context(|| format!("reading {}", file.display()))?;
				let saved = stream::Saved {
					recordings: Some(videos),
					..stream.saved().clone()
				};
				stream.save(saved)?;
				wrote = (!there).then_some(file);
			}
		}
		let launch = Launch {
			gpu: self.gpu.clone(),
			recordings: stream.recordings(autosave.path()),
			sessions: Default::default(),
		};
		let started = async {
			let engine = Engine::start(launch, autosave, loaded).await?;
			// Now, so the show's name is taken and a failure says so here.
			if new && let Err(err) = engine.save() {
				engine.recorder.close();
				engine.stop();
				tokio::join!(engine.close(), engine.recorder.done());
				return Err(err.context("cannot create the show"));
			}
			Ok(engine)
		};
		match started.await {
			Ok(engine) => Ok((engine, stream)),
			Err(err) => {
				if let Some(file) = wrote
					&& let Err(err) = std::fs::remove_file(&file)
				{
					tracing::warn!(%err, file = %file.display(), "cannot remove a show's stream settings");
				}
				Err(err)
			}
		}
	}
}

/// A new show's collection: one empty scene on `canvas`, hearing the
/// system's microphone on program if there is a `mic`.
pub fn first(canvas: Canvas, mic: bool) -> anyhow::Result<Collection> {
	let mut collection = Collection::new();
	collection.canvas = canvas;
	if mic {
		let mut draft = collection.draft("Hear the microphone");
		draft.set_source(MIC, Some(Source::Microphone { id: None }))?;
		draft.hear(MIC)?;
		let edit = draft.finish()?;
		collection.apply(&edit)?;
	}
	Ok(collection)
}

/// Whether the system has a microphone to hear. A listing that fails is
/// logged and counts as none, as a build without capture has.
async fn microphone() -> bool {
	#[cfg(feature = "capture")]
	match moq_audio::capture::devices().await {
		Ok(devices) => return !devices.is_empty(),
		Err(err) => tracing::warn!(%err, "cannot list the microphones"),
	}
	false
}

/// Where new shows are kept: the app's storage.
pub fn dir() -> Option<PathBuf> {
	crate::data_dir().map(|dir| dir.join("shows"))
}

/// Whether the scene file at `path` is one of the app's own, in [`dir`].
fn in_storage(path: &Path) -> bool {
	let parent = path.parent().and_then(|parent| std::path::absolute(parent).ok());
	let dir = dir().and_then(|dir| std::path::absolute(dir).ok());
	parent.is_some() && parent == dir
}

/// The system's folder for videos, where a show in the app's storage
/// records.
fn videos() -> Option<PathBuf> {
	dirs::video_dir().or_else(dirs::home_dir)
}

/// A scene file in `dir` for a new show: [`NAME`], numbered past any show
/// already there, created with the directory if need be.
pub fn new_path(dir: &Path) -> anyhow::Result<PathBuf> {
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let named = |n: u32| match n {
		1 => dir.join(format!("{NAME}.toml")),
		n => dir.join(format!("{NAME} {n}.toml")),
	};
	for n in 1.. {
		let path = named(n);
		if !taken(&path)? {
			return Ok(path);
		}
	}
	unreachable!("a free name")
}

/// The show in `dir` whose scene file was written last, for a launch whose
/// recent list lost track of it, such as one after a crash before the list
/// was saved.
pub fn latest(dir: &Path) -> Option<PathBuf> {
	let entries = std::fs::read_dir(dir).ok()?;
	let scenes = entries.filter_map(|entry| {
		let entry = entry.ok()?;
		let name = entry.file_name().into_string().ok()?;
		let sibling = name.ends_with(".history.toml") || name.ends_with(".stream.toml");
		if !name.ends_with(".toml") || sibling {
			return None;
		}
		let written = entry.metadata().ok()?.modified().ok()?;
		Some((written, entry.path()))
	});
	scenes.max().map(|(_, path)| path)
}

/// The files a show keeps at `path`: its scene file, then the history and
/// the stream settings beside it.
fn files(path: &Path) -> [PathBuf; 3] {
	[path.to_path_buf(), autosave::history_path(path), stream::path(path)]
}

/// Whether a show at `path` would meet any file already there.
fn taken(path: &Path) -> anyhow::Result<bool> {
	for file in files(path) {
		if file
			.try_exists()
			.with_context(|| format!("reading {}", file.display()))?
		{
			return Ok(true);
		}
	}
	Ok(false)
}

/// The show's name: its scene file's, without the extension.
pub fn name(path: &Path) -> String {
	path.file_stem()
		.unwrap_or(path.as_os_str())
		.to_string_lossy()
		.into_owned()
}

/// Rename the closed show at `path` to `name`: its scene file, history, and
/// stream settings, in the same directory, and the recordings and clips in
/// `recordings` named after it, so REVIEW still finds them. Refuses a name
/// that is no file name, one another show there has, or one a recording
/// would meet a file under; a rename that fails partway puts back the files
/// it moved. Where it is now.
pub fn rename(path: &Path, name: &str, recordings: &Path) -> anyhow::Result<PathBuf> {
	let name = name.trim();
	anyhow::ensure!(!name.is_empty(), "a show needs a name");
	anyhow::ensure!(
		!name.starts_with('.') && !name.contains(RESERVED) && !name.contains(char::is_control),
		"a show's name cannot start with a dot or hold any of {}",
		String::from_iter(RESERVED)
	);
	let lower = name.to_lowercase();
	anyhow::ensure!(
		![".history", ".stream"].iter().any(|end| lower.ends_with(end)),
		"a show's name cannot end in .history or .stream, as its other files' names do"
	);
	let to = path.with_file_name(format!("{name}.toml"));
	if to == path {
		return Ok(to);
	}
	let there = |file: &Path| file.try_exists().with_context(|| format!("reading {}", file.display()));
	// A name only cased apart is the show's own on a case-insensitive system.
	let met = |from: &Path, to: &Path| -> anyhow::Result<bool> {
		Ok(there(to)? && !same_file::is_same_file(from, to).unwrap_or(false))
	};
	let mut moves = Vec::new();
	for (from, to) in files(path).into_iter().zip(files(&to)) {
		anyhow::ensure!(!met(&from, &to)?, "another show is called {name}");
		if there(&from)? {
			moves.push((from, to));
		}
	}
	for (from, to) in named_after(recordings, &self::name(path), name)? {
		anyhow::ensure!(!met(&from, &to)?, "{} is already there", to.display());
		moves.push((from, to));
	}
	let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
	for (from, to) in moves {
		if let Err(err) = std::fs::rename(&from, &to) {
			for (from, to) in moved.into_iter().rev() {
				if let Err(err) = std::fs::rename(&to, &from) {
					tracing::error!(%err, from = %to.display(), to = %from.display(), "cannot put a show's file back");
				}
			}
			return Err(err).with_context(|| format!("renaming {}", from.display()));
		}
		moved.push((from, to));
	}
	Ok(to)
}

/// The recordings and clips in `dir` of the show `old`, and the marker left
/// beside one a crash cut short, each with where it goes for the show `new`.
fn named_after(dir: &Path, old: &str, new: &str) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		// Nothing recorded yet.
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
	};
	let mut moves = Vec::new();
	for entry in entries {
		let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
		// The recorder names its files in UTF-8, after the show's name.
		let Ok(file) = entry.file_name().into_string() else {
			continue;
		};
		let recording = file.strip_suffix(crate::record::MARKER).unwrap_or(&file);
		if crate::shelf::parse(old, recording).is_some() {
			moves.push((entry.path(), dir.join(format!("{new}{}", &file[old.len()..]))));
		}
	}
	Ok(moves)
}

/// Write `collection`, the show at `path`'s, to `to` as a scene file of its
/// own. A copy in another directory names each file it reads from the root,
/// so it reads the same files, and is refused if an overlay loads a file,
/// which only its own scene file's directory holds. Its history and stream
/// settings stay with the show.
pub fn export(collection: &Collection, path: &Path, to: &Path) -> anyhow::Result<()> {
	let own = std::path::absolute(to)? == std::path::absolute(path)?
		// Through a link or a name cased apart, too.
		|| same_file::is_same_file(to, path).unwrap_or(false);
	anyhow::ensure!(!own, "that is the show's own file");
	let parent = |path: &Path| {
		let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
		std::path::absolute(dir.unwrap_or(Path::new(".")))
	};
	let (dir, into) = (parent(path)?, parent(to)?);
	let mut copy = collection.clone();
	if !same_file::is_same_file(&dir, &into).unwrap_or(false) {
		copy.root(&dir)?;
	}
	let text = copy.to_toml()?;
	autosave::write_file(to, &text).with_context(|| format!("writing {}", to.display()))?;
	Ok(())
}

/// Follow broadcast sources, guests, and viewers' reports through `stream`'s
/// relays, read with their tokens. Each relay is set up on its own, so one
/// that cannot be is logged and the other still followed; going live says
/// why. Blocks on the credential store: call it off the window's thread,
/// outside an async task, inside the tokio runtime, and through
/// [`Controls`](crate::controls::Controls), which orders the follows.
/// Says whether every relay was set up, so a follow that was not can be
/// tried again.
#[must_use]
pub fn follow(engine: &Engine, stream: &Stream) -> bool {
	let each = [(true, false), (false, true)].map(|(relay, failover)| stream::Switched { relay, failover });
	let mut relays = Vec::new();
	let mut failed = false;
	for switched in each {
		match stream.relays(switched) {
			Ok(set) => relays.extend(set),
			Err(err) => {
				tracing::warn!(error = format!("{err:#}"), "cannot follow a relay");
				failed = true;
			}
		}
	}
	// None could be set up, such as a new relay on a fixed --connect-bind
	// port the sessions followed now still hold: those keep the broadcast
	// sources and guests rather than none.
	if relays.is_empty() && failed {
		tracing::warn!("keeping the relays followed before");
		return false;
	}
	engine.watch(&relays);
	!failed
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A new show takes the first name no show in the directory has, counting
	/// a history or stream settings left behind.
	#[test]
	fn names_a_new_show_past_the_taken_ones() {
		let dir = tempfile::tempdir().unwrap();
		let shows = dir.path().join("shows");
		assert_eq!(new_path(&shows).unwrap(), shows.join("My Show.toml"));
		std::fs::write(shows.join("My Show.toml"), "").unwrap();
		std::fs::write(shows.join("My Show 2.stream.toml"), "").unwrap();
		assert_eq!(new_path(&shows).unwrap(), shows.join("My Show 3.toml"));
	}

	/// The latest show is the scene file written last, never a history or
	/// stream settings beside one.
	#[test]
	#[expect(clippy::disallowed_types, reason = "a file's modified time, which std sets")]
	fn finds_the_latest_show() {
		let dir = tempfile::tempdir().unwrap();
		assert_eq!(latest(dir.path()), None);
		let at = |secs| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
		for (name, secs) in [
			("My Show.toml", 10),
			("My Show 2.toml", 20),
			("My Show.history.toml", 30),
			("My Show.stream.toml", 30),
			("logo.png", 40),
		] {
			let file = std::fs::File::create(dir.path().join(name)).unwrap();
			file.set_modified(at(secs)).unwrap();
		}
		assert_eq!(latest(dir.path()), Some(dir.path().join("My Show 2.toml")));
	}

	/// A rename moves the show's files, and its recordings and clips with any
	/// marker beside one, but no other show's, and refuses a name another
	/// show has or that is no file name.
	#[test]
	fn renames_a_show_with_its_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("My Show.toml");
		for (file, text) in files(&path).iter().zip(["scene", "history", "stream"]) {
			std::fs::write(file, text).unwrap();
		}
		std::fs::write(dir.path().join("Taken.history.toml"), "").unwrap();
		let videos = dir.path().join("videos");
		std::fs::create_dir(&videos).unwrap();
		let stamp = "2026-11-03T12-00-00Z";
		let ours = [
			format!("My Show {stamp}.mp4"),
			format!("My Show {stamp} mic (2).mp4"),
			format!("My Show {stamp} mic (2).mp4.unfinished"),
			"My Show-clip-3.mp4".to_string(),
		];
		let theirs = [
			format!("My Show 2 {stamp}.mp4"),
			"My Show 2-clip-1.mp4".into(),
			"My Show.mp4".into(),
		];
		for file in ours.iter().chain(&theirs) {
			std::fs::write(videos.join(file), file).unwrap();
		}

		for refused in [
			"",
			"  ",
			".hidden",
			"a/b",
			"a\\b",
			"Taken",
			"Talk.history",
			"Talk.STREAM",
		] {
			assert!(rename(&path, refused, &videos).is_err(), "{refused:?} was taken");
		}
		assert!(path.exists(), "a refused rename moves nothing");

		let to = rename(&path, " Podcast ", &videos).unwrap();
		assert_eq!(to, dir.path().join("Podcast.toml"));
		let read = files(&to).map(|file| std::fs::read_to_string(file).unwrap());
		assert_eq!(read, ["scene", "history", "stream"]);
		assert!(files(&path).iter().all(|file| !file.exists()));
		for file in &ours {
			let moved = videos.join(file.replacen("My Show", "Podcast", 1));
			assert_eq!(std::fs::read_to_string(moved).unwrap(), *file);
		}
		assert!(theirs.iter().all(|file| videos.join(file).exists()));
		assert_eq!(rename(&to, "Podcast", &videos).unwrap(), to, "the same name is no move");
	}

	/// A rename a recording's new name would meet a file under moves nothing.
	#[test]
	fn a_rename_onto_a_recording_moves_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("My Show.toml");
		std::fs::write(&path, "scene").unwrap();
		let stamp = "2026-11-03T12-00-00Z";
		std::fs::write(dir.path().join(format!("My Show {stamp}.mp4")), "ours").unwrap();
		std::fs::write(dir.path().join(format!("Talk {stamp}.mp4")), "theirs").unwrap();
		assert!(rename(&path, "Talk", dir.path()).is_err());
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "scene");
		let ours = dir.path().join(format!("My Show {stamp}.mp4"));
		assert_eq!(std::fs::read_to_string(ours).unwrap(), "ours");
	}

	/// A show that never had a history, stream settings, or recordings
	/// renames too.
	#[test]
	fn renames_a_show_without_its_other_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, "scene").unwrap();
		let to = rename(&path, "Talk", &dir.path().join("never made")).unwrap();
		assert_eq!(std::fs::read_to_string(to).unwrap(), "scene");
	}

	/// An exported copy reads the show's files from wherever it is kept, and
	/// is never written over the show itself.
	#[test]
	fn exports_a_copy_that_reads_the_same_files() {
		let dir = tempfile::tempdir().unwrap();
		let show = dir.path().join("shows/My Show.toml");
		std::fs::create_dir_all(show.parent().unwrap()).unwrap();
		std::fs::write(&show, "scene").unwrap();
		let mut collection = Collection::new();
		let mut draft = collection.draft("Add a logo");
		draft
			.set_source(
				"logo",
				Some(Source::Image {
					path: "logo.png".into(),
				}),
			)
			.unwrap();
		draft
			.set_source(
				"lower third",
				Some(Source::Image {
					path: "/elsewhere/third.png".into(),
				}),
			)
			.unwrap();
		collection.apply(&draft.finish().unwrap()).unwrap();

		let to = dir.path().join("copy.toml");
		export(&collection, &show, &to).unwrap();
		let copy = Collection::from_toml(&std::fs::read_to_string(&to).unwrap()).unwrap();
		let path = |name: &str| match &copy.sources[name] {
			Source::Image { path } => path.clone(),
			other => panic!("{other:?}"),
		};
		assert_eq!(path("logo"), dir.path().join("shows/logo.png"));
		assert_eq!(path("lower third"), PathBuf::from("/elsewhere/third.png"));

		// Beside the show, its paths hold as they are.
		let beside = dir.path().join("shows/copy.toml");
		export(&collection, &show, &beside).unwrap();
		let copy = Collection::from_toml(&std::fs::read_to_string(&beside).unwrap()).unwrap();
		assert_eq!(copy, collection);

		assert!(export(&collection, &show, &show).is_err());
		#[cfg(unix)]
		{
			let link = dir.path().join("link.toml");
			std::os::unix::fs::symlink(&show, &link).unwrap();
			assert!(export(&collection, &link, &show).is_err(), "the show through a link");
			assert_eq!(std::fs::read_to_string(&show).unwrap(), "scene");
		}
	}
}
