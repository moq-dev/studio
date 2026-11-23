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

/// Held while a rename moves a show's files, so quitting waits for them to
/// land together; true once the studio is quitting, when no rename starts.
static MOVING: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Wait for a rename moving a show's files, and start no other, so the
/// process never ends with a show split across two names.
pub fn settle() {
	*MOVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
}

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
		// The stream settings a new show had before this wrote its recordings
		// folder, `None` for no file, to put back if it cannot be made.
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
				&& stream.saved().recordings.is_none()
				&& let Some(videos) = videos()
			{
				let before = exists(stream.path())?.then(|| stream.saved().clone());
				let saved = stream::Saved {
					recordings: Some(videos),
					..stream.saved().clone()
				};
				stream.save(saved, &[])?;
				wrote = Some(before);
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
				let undone = match wrote {
					None => Ok(()),
					Some(Some(before)) => stream.save(before, &[]),
					Some(None) => std::fs::remove_file(stream.path()).map_err(Into::into),
				};
				if let Err(err) = undone {
					tracing::warn!(error = format!("{err:#}"), "cannot put back a show's stream settings");
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

/// Whether the system has a default microphone, the one a new show's `Mic`
/// opens: an input on another audio host alone is none. A listing that fails
/// is logged and counts as none, as a build without capture has.
async fn microphone() -> bool {
	#[cfg(feature = "capture")]
	match moq_audio::capture::devices().await {
		Ok(devices) => return devices.iter().any(|device| device.default),
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
pub fn videos() -> Option<PathBuf> {
	dirs::video_dir().or_else(dirs::home_dir)
}

/// A scene file in `dir` for a new show recording into `recordings`:
/// [`NAME`], numbered past any name a show there has or had, or that a
/// recording there is named after, created with the directory if need be.
pub fn new_path(dir: &Path, recordings: &[PathBuf]) -> anyhow::Result<PathBuf> {
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let claimed = claimed(dir, None)?;
	for n in 1.. {
		let name = match n {
			1 => NAME.to_string(),
			n => format!("{NAME} {n}"),
		};
		let path = dir.join(format!("{name}.toml"));
		let mut recorded = false;
		for recordings in recordings {
			recorded |= self::recorded(recordings, &name)?;
		}
		if !taken(&path)? && !claimed.contains(&name) && !recorded {
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
		// Through a link, and never a folder that happens to end in `.toml`.
		let metadata = std::fs::metadata(entry.path()).ok()?;
		if !metadata.is_file() {
			return None;
		}
		Some((metadata.modified().ok()?, entry.path()))
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
		if exists(&file)? {
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
/// stream settings, in the same directory. Its recordings and clips in
/// `recordings`, this run's folder for them, keep their names, which its stream settings note as an
/// earlier name of its, so REVIEW still lists them and media that reads one
/// still finds it. Refuses a name that is no file name, or that another show
/// there has, had, or recorded under; a rename that fails partway puts back
/// the files it moved. Where it is now.
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
	let old = self::name(path);
	let mut saved = saved(&stream::path(path))?;
	let ours = saved.earlier.iter().any(|earlier| earlier == name);
	let dir = path.parent().unwrap_or(Path::new(""));
	for (from, to) in files(path).into_iter().zip(files(&to)) {
		// A name only cased apart is the show's own on a case-insensitive
		// system; any other name for the same file, such as a hard link, is not.
		let recased = name.to_lowercase() == old.to_lowercase();
		let met = exists(&to)? && !(recased && same_file::is_same_file(&from, &to).unwrap_or(false));
		anyhow::ensure!(!met, "another show is called {name}");
	}
	anyhow::ensure!(
		!claimed(dir, Some(path))?.contains(name),
		"another show was called {name}"
	);
	// This run's folder, and the one the show saves, which a `--recordings`
	// flag only overrides for this run.
	for folder in [recordings.to_path_buf(), saved.folder(path)] {
		anyhow::ensure!(
			ours || !recorded(&folder, name)?,
			"{} holds recordings of another show called {name}",
			folder.display()
		);
	}
	// Taken while the file is there to resolve.
	let key = engine::grant_key(path).with_context(|| format!("reading {}", path.display()))?;
	let quitting = MOVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
	anyhow::ensure!(!*quitting, "the studio is quitting");

	let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
	let put_back = |moved: Vec<(PathBuf, PathBuf)>| {
		for (from, to) in moved.into_iter().rev() {
			if let Err(err) = std::fs::rename(&to, &from) {
				tracing::error!(%err, from = %to.display(), to = %from.display(), "cannot put a show's file back");
			}
		}
	};
	for (from, to) in files(path).into_iter().zip(files(&to)) {
		let result = exists(&from).and_then(|there| match there {
			true => std::fs::rename(&from, &to)
				.map(|()| true)
				.with_context(|| format!("renaming {}", from.display())),
			false => Ok(false),
		});
		match result {
			Ok(true) => moved.push((from, to)),
			Ok(false) => {}
			Err(err) => {
				put_back(moved);
				return Err(err);
			}
		}
	}
	saved.earlier.retain(|earlier| earlier != name && *earlier != old);
	saved.earlier.push(old);
	let text = toml::to_string(&saved).context("cannot write the stream settings")?;
	let settings = stream::path(&to);
	if let Err(err) = autosave::write_file(&settings, &text) {
		// It left the old file as it was: only a file this made is there to
		// take back, and the moves put back the rest.
		if !moved.iter().any(|(_, to)| *to == settings) {
			let _ = std::fs::remove_file(&settings);
		}
		put_back(moved);
		return Err(err).with_context(|| format!("writing {}", settings.display()));
	}
	// The picker asks again for each screen without them, which is no reason
	// to undo the rename.
	if let Err(err) = engine::move_grants(&key, &to) {
		tracing::warn!(error = format!("{err:#}"), "cannot keep the screens' grants");
	}
	Ok(to)
}

/// Whether `file` is there.
fn exists(file: &Path) -> anyhow::Result<bool> {
	file.try_exists().with_context(|| format!("reading {}", file.display()))
}

/// The stream settings in the file at `path`, or the defaults if it is not
/// there.
fn saved(path: &Path) -> anyhow::Result<stream::Saved> {
	match std::fs::read_to_string(path) {
		Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display())),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(stream::Saved::default()),
		Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
	}
}

/// The names the shows in `dir`, but the one at `except`, had before, whose
/// recordings are still theirs. A show whose stream settings cannot be read
/// claims none, and is logged.
fn claimed(dir: &Path, except: Option<&Path>) -> anyhow::Result<std::collections::HashSet<String>> {
	let entries = match std::fs::read_dir(if dir.as_os_str().is_empty() {
		Path::new(".")
	} else {
		dir
	}) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
		Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
	};
	let except = except.map(stream::path);
	let mut names = std::collections::HashSet::new();
	for entry in entries {
		let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
		let file = entry.path();
		let ours = except
			.as_deref()
			.is_some_and(|except| same_file::is_same_file(except, &file).unwrap_or(false));
		if ours || !entry.file_name().to_string_lossy().ends_with(".stream.toml") {
			continue;
		}
		match saved(&file) {
			Ok(saved) => names.extend(saved.earlier),
			Err(err) => tracing::warn!(error = format!("{err:#}"), "cannot read a show's earlier names"),
		}
	}
	Ok(names)
}

/// Whether `dir` holds a recording or clip of a show called `show`.
fn recorded(dir: &Path, show: &str) -> anyhow::Result<bool> {
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		// Nothing recorded yet.
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
		Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
	};
	for entry in entries {
		let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
		let file = entry.file_name();
		let file = file.to_string_lossy();
		// A marker a crash left names a recording, which a launch repairs.
		let recording = file.strip_suffix(crate::record::MARKER).unwrap_or(&file);
		if crate::shelf::parse(show, recording).is_some() {
			return Ok(true);
		}
	}
	Ok(false)
}

/// Write `collection`, the show at `path`'s, to `to` as a scene file of its
/// own. A copy in another directory names each file it reads from the root,
/// so it reads the same files, and is refused if an overlay loads a file,
/// which only its own scene file's directory holds. Its history and stream
/// settings stay with the show.
pub fn export(collection: &Collection, path: &Path, to: &Path) -> anyhow::Result<()> {
	let to_absolute = std::path::absolute(to)?;
	for file in files(path) {
		let own = to_absolute == std::path::absolute(&file)?
			// Through a link or a name cased apart, too.
			|| same_file::is_same_file(to, &file).unwrap_or(false);
		anyhow::ensure!(!own, "that is one of the show's own files");
	}
	let parent = |path: &Path| {
		let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
		std::path::absolute(dir.unwrap_or(Path::new(".")))
	};
	let dir = parent(path)?;
	// Opened as `to`, it reads beside the link; the file the write lands in,
	// through any links, may be read where it is. Beside the show both ways,
	// its paths hold as they are.
	let mut beside = true;
	for to in [to.to_path_buf(), autosave::resolve(to)?] {
		beside &= same_file::is_same_file(&dir, parent(&to)?).unwrap_or(false);
	}
	let mut copy = collection.clone();
	if !beside {
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

	/// A new show takes the first name no show in the directory has or had,
	/// counting a history or stream settings left behind, and no recording is
	/// named after.
	#[test]
	fn names_a_new_show_past_the_taken_ones() {
		let dir = tempfile::tempdir().unwrap();
		let shows = dir.path().join("shows");
		let videos = dir.path().join("videos");
		assert_eq!(
			new_path(&shows, std::slice::from_ref(&videos)).unwrap(),
			shows.join("My Show.toml")
		);
		std::fs::write(shows.join("My Show.toml"), "").unwrap();
		std::fs::write(shows.join("My Show 2.stream.toml"), "").unwrap();
		std::fs::write(shows.join("Podcast.stream.toml"), "earlier = [\"My Show 3\"]").unwrap();
		std::fs::create_dir(&videos).unwrap();
		std::fs::write(videos.join("My Show 4-clip-1.mp4"), "").unwrap();
		assert_eq!(
			new_path(&shows, std::slice::from_ref(&videos)).unwrap(),
			shows.join("My Show 5.toml")
		);
		assert_eq!(new_path(&shows, &[]).unwrap(), shows.join("My Show 4.toml"));
		// Every folder a new show's recordings may go to counts.
		let flagged = dir.path().join("flagged");
		std::fs::create_dir(&flagged).unwrap();
		std::fs::write(flagged.join("My Show 5-clip-1.mp4"), "").unwrap();
		assert_eq!(
			new_path(&shows, &[flagged, videos]).unwrap(),
			shows.join("My Show 6.toml")
		);
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
		// A folder is no show, however its name ends.
		std::fs::create_dir(dir.path().join("Backup.toml")).unwrap();
		assert_eq!(latest(dir.path()), Some(dir.path().join("My Show 2.toml")));
	}

	/// A rename moves the show's files, leaves its recordings where they are
	/// and notes its old name, so they stay its own, and refuses a name
	/// another show has, had, or recorded under, or that is no file name.
	#[test]
	fn renames_a_show_with_its_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("My Show.toml");
		std::fs::write(&path, "scene").unwrap();
		std::fs::write(autosave::history_path(&path), "history").unwrap();
		std::fs::write(stream::path(&path), "on_demand = true").unwrap();
		std::fs::write(dir.path().join("Taken.history.toml"), "").unwrap();
		std::fs::write(dir.path().join("Other.stream.toml"), "earlier = [\"Was\"]").unwrap();
		let videos = dir.path().join("videos");
		std::fs::create_dir(&videos).unwrap();
		let stamp = "2026-11-03T12-00-00Z";
		std::fs::write(videos.join(format!("My Show {stamp}.mp4")), "").unwrap();
		std::fs::write(videos.join(format!("Theirs {stamp}.mp4")), "").unwrap();

		let refused = [
			"",
			"  ",
			".hidden",
			"a/b",
			"a\\b",
			"Taken",
			"Was",
			"Theirs",
			"Talk.history",
			"Talk.STREAM",
		];
		for refused in refused {
			assert!(rename(&path, refused, &videos).is_err(), "{refused:?} was taken");
		}
		assert!(path.exists(), "a refused rename moves nothing");

		let to = rename(&path, " Podcast ", &videos).unwrap();
		assert_eq!(to, dir.path().join("Podcast.toml"));
		assert_eq!(std::fs::read_to_string(&to).unwrap(), "scene");
		let history = std::fs::read_to_string(autosave::history_path(&to)).unwrap();
		assert_eq!(history, "history");
		assert!(files(&path).iter().all(|file| !file.exists()));
		let settings = saved(&stream::path(&to)).unwrap();
		assert!(settings.on_demand, "the other settings stay");
		assert_eq!(settings.earlier, ["My Show"]);
		assert!(videos.join(format!("My Show {stamp}.mp4")).exists());
		assert_eq!(rename(&to, "Podcast", &videos).unwrap(), to, "the same name is no move");

		// Back to the name it recorded under, which is its own.
		let back = rename(&to, "My Show", &videos).unwrap();
		assert_eq!(saved(&stream::path(&back)).unwrap().earlier, ["Podcast"]);
	}

	/// A name recorded under in the folder the show saves is refused, even
	/// while this run records elsewhere, as `--recordings` makes it.
	#[test]
	fn a_rename_checks_the_saved_recordings_folder() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, "scene").unwrap();
		let saved = dir.path().join("saved");
		std::fs::create_dir(&saved).unwrap();
		std::fs::write(
			stream::path(&path),
			format!("recordings = {:?}", saved.display().to_string()),
		)
		.unwrap();
		std::fs::write(saved.join("Talk 2026-11-03T12-00-00Z.mp4"), "").unwrap();
		let flagged = dir.path().join("flagged");
		assert!(
			rename(&path, "Talk", &flagged).is_err(),
			"Talk recorded in the saved folder"
		);
		assert!(path.exists());
	}

	/// A show that never had a history, stream settings, or recordings
	/// renames too, gaining stream settings that note its old name.
	#[test]
	fn renames_a_show_without_its_other_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, "scene").unwrap();
		let to = rename(&path, "Talk", &dir.path().join("never made")).unwrap();
		assert_eq!(std::fs::read_to_string(&to).unwrap(), "scene");
		assert!(!autosave::history_path(&to).exists());
		assert_eq!(saved(&stream::path(&to)).unwrap().earlier, ["show"]);
	}

	/// Another name for the same file, such as a hard link, is another
	/// show's, unlike the show's own name cased apart.
	#[cfg(unix)]
	#[test]
	fn a_rename_onto_a_hard_link_is_refused() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, "scene").unwrap();
		std::fs::hard_link(&path, dir.path().join("Talk.toml")).unwrap();
		assert!(rename(&path, "Talk", &dir.path().join("never made")).is_err());
		assert!(path.exists());
	}

	/// A rename that cannot tell whether a file is there, such as a history
	/// linked into a folder it may not read, puts back what it moved.
	#[cfg(unix)]
	#[test]
	fn a_rename_that_cannot_read_a_file_moves_nothing() {
		use std::os::unix::fs::PermissionsExt;
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("show.toml");
		std::fs::write(&path, "scene").unwrap();
		let locked = dir.path().join("locked");
		std::fs::create_dir(&locked).unwrap();
		std::os::unix::fs::symlink(locked.join("history"), autosave::history_path(&path)).unwrap();
		std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
		// Root reads it anyway, which leaves nothing to test.
		let readable = std::fs::read_dir(&locked).is_ok();
		let result = (!readable).then(|| rename(&path, "Talk", &dir.path().join("never made")));
		std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
		let Some(result) = result else { return };
		assert!(result.is_err());
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "scene");
		assert!(!dir.path().join("Talk.toml").exists());
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
		for sidecar in [autosave::history_path(&show), stream::path(&show)] {
			assert!(export(&collection, &show, &sidecar).is_err(), "{}", sidecar.display());
		}
		#[cfg(unix)]
		{
			// A link beside the show to a file elsewhere roots, as the file is
			// read where it is.
			let elsewhere = dir.path().join("elsewhere.toml");
			let beside = dir.path().join("shows/linked.toml");
			std::fs::write(&elsewhere, "").unwrap();
			std::os::unix::fs::symlink(&elsewhere, &beside).unwrap();
			export(&collection, &show, &beside).unwrap();
			let copy = Collection::from_toml(&std::fs::read_to_string(&elsewhere).unwrap()).unwrap();
			assert!(matches!(&copy.sources["logo"], Source::Image { path } if path.is_absolute()));

			let link = dir.path().join("link.toml");
			std::os::unix::fs::symlink(&show, &link).unwrap();
			assert!(export(&collection, &link, &show).is_err(), "the show through a link");
			assert_eq!(std::fs::read_to_string(&show).unwrap(), "scene");
		}
	}
}
