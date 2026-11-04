//! Instant replays and clips: the recent past of the program or a source,
//! played in a replay source or saved to disk.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use moq_scene::Source;
use moq_scene::replay::{Take, Target};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::engine::Engine;

/// The lengths the Replay panel offers.
pub const LENGTHS: [Duration; 4] = [
	Duration::from_secs(5),
	Duration::from_secs(10),
	Duration::from_secs(20),
	moq_scene::replay::MAX_LENGTH,
];

/// The speeds the Replay panel offers: real time, and slow motion.
pub const SPEEDS: [f32; 3] = [1.0, 0.5, 0.25];

/// What a replay or clip is of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Of {
	/// The program, as it was published.
	#[default]
	Program,
	/// A source, by name.
	Source(String),
}

impl std::fmt::Display for Of {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Program => write!(f, "the program"),
			Self::Source(name) => write!(f, "`{name}`"),
		}
	}
}

impl Engine {
	/// What a replay of `of` copies, ending now.
	///
	/// The program has a past only while it is published, a local source only
	/// from while it was on program, since nothing else encodes them, and a
	/// broadcast source once it shows a picture.
	pub fn target(&self, of: &Of) -> anyhow::Result<Target> {
		let output = || {
			self.output
				.as_ref()
				.context("nothing is encoded without --connect, so there is no past to replay")
		};
		let name = match of {
			Of::Program => return Ok(output()?.program()),
			Of::Source(name) => name,
		};
		let state = self.state.lock().unwrap();
		let capture = state.collection().capture(name);
		match state.collection().sources.get(capture) {
			Some(Source::Broadcast { .. }) => state
				.sources
				.get(capture)
				.and_then(|feed| feed.target())
				.with_context(|| format!("`{name}` has shown nothing yet")),
			Some(Source::Browser { .. }) => {
				anyhow::bail!(
					"`{name}` is a web page, which publishes no track of its own, so it has no past to replay"
				)
			}
			Some(source) if source.has_video() => output()?
				.source(capture)
				.with_context(|| format!("`{name}` publishes no video to replay")),
			Some(_) => anyhow::bail!("`{name}` has no picture to replay"),
			None => anyhow::bail!("there is no source named `{name}`"),
		}
	}
}

/// Hold the `length` of `of` up to now: to play in a replay source with
/// `play`, or as a clip to save.
pub fn take(engine: &Engine, of: &Of, length: Duration, play: bool) -> anyhow::Result<Take> {
	let target = engine.target(of)?;
	let take = match play {
		true => Take::new(&target, length),
		false => Take::clip(&target, length),
	};
	take.with_context(|| match of {
		Of::Source(name) => format!("cannot replay `{name}`, which keeps a past only while it is on program"),
		Of::Program => "cannot replay the program".into(),
	})
}

/// Play `take` at `speed` in the replay source `into`.
pub fn play(engine: &Engine, take: Take, speed: f32, into: &str) -> anyhow::Result<()> {
	let state = engine.state.lock().unwrap();
	let feed = state
		.sources
		.get(into)
		.with_context(|| format!("the replay source `{into}` is not running"))?;
	feed.replay(take, speed)?;
	Ok(())
}

/// Save `take`, in real time, as a new MP4 file beside `scenes`, the scene
/// file. Returns where.
pub async fn save(take: Take, scenes: &Path) -> anyhow::Result<PathBuf> {
	let (path, mut file) = create(scenes).await?;
	let written = async {
		let broadcast = take.consume();
		let catalog = moq_mux::catalog::Consumer::<()>::new(&broadcast, moq_mux::catalog::CatalogFormat::Hang).await?;
		// A take refuses renditions served from other broadcasts, so nothing
		// resolves through this origin.
		let source = moq_mux::Source::new(moq_tokio::origin::spawn().consume(), broadcast);
		// A take is history, so nothing in it is late.
		let mut export = moq_mux::container::mp4::Export::new(source, catalog).with_max_delay(Duration::MAX);
		while let Some(data) = export.next().await? {
			file.write_all(&data).await?;
		}
		let finish = export.finish()?.context("the replay held nothing to save")?;
		file.set_len(finish.moov_offset).await?;
		file.seek(std::io::SeekFrom::Start(finish.moov_offset)).await?;
		file.write_all(&finish.moov).await?;
		// The header points readers past the media at the moov, so the moov
		// must be on disk first.
		file.sync_data().await?;
		file.seek(std::io::SeekFrom::Start(finish.mdat_offset)).await?;
		file.write_all(&finish.mdat).await?;
		file.sync_all().await?;
		anyhow::Ok(())
	};
	if let Err(err) = written.await {
		// Half a clip is no clip.
		drop(file);
		let _ = tokio::fs::remove_file(&path).await;
		return Err(err.context(format!("cannot save {}", path.display())));
	}
	Ok(path)
}

/// A new file beside the scene file `scenes`: for `show.toml`,
/// `show-clip-1.mp4`, or the first number not taken.
async fn create(scenes: &Path) -> anyhow::Result<(PathBuf, tokio::fs::File)> {
	let dir = scenes.parent().unwrap_or(Path::new(""));
	let stem = scenes.file_stem().unwrap_or_default().to_string_lossy();
	for n in 1.. {
		let path = dir.join(format!("{stem}-clip-{n}.mp4"));
		match tokio::fs::File::create_new(&path).await {
			Ok(file) => return Ok((path, file)),
			Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
			Err(err) => return Err(err).with_context(|| format!("cannot create {}", path.display())),
		}
	}
	unreachable!("some clip number is free")
}

// The software encoder is the one every test machine has.
#[cfg(all(test, feature = "openh264"))]
mod tests {
	use std::io::Seek;

	use super::*;

	/// A clip is a finished MP4 of the span, named beside the scene file
	/// without overwriting an earlier one, that shows nothing after the span's
	/// end even when a picture inside it refers to one shown after.
	#[tokio::test]
	async fn a_clip_saves_its_span() {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let mut config = moq_video::encode::Config::new(64, 64, moq_video::FrameRate::new(30, 1).unwrap());
		config.kind = moq_video::encode::Kind::Software;
		let rendition = config.probe().await.unwrap();
		let reserved = catalog.reserve().with_hls_hint(config.gop.duration(config.framerate));
		let mut producer = moq_video::encode::Producer::new(broadcast.clone(), reserved, rendition).unwrap();
		let mut encoder = moq_video::encode::Encoder::new(&config).unwrap();
		let gray = vec![0x80; 64 * 64 * 4];
		let at = |index: u64| moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap();
		// Each second in the decode order of B-frames: I0 P2 B1 P4 B3 ... B27 P29.
		let shown = |index: u64| match index % 30 {
			0 | 29 => index,
			odd if odd % 2 == 1 => index + 1,
			_ => index - 1,
		};
		for index in 0..90u64 {
			if index % 30 == 0 {
				encoder.cut().unwrap();
			}
			let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
			let mut encoded = encoder.encode(&moq_video::Frame::new(surface, at(index))).unwrap();
			// The software encoder writes each picture as it takes it.
			let [unit] = encoded.as_mut_slice() else {
				panic!("one access unit per picture");
			};
			unit.timestamp = at(shown(index));
			producer.publish(&encoded).unwrap();
		}
		// Between the pictures shown at 2.433 and 2.467 s, which are decoded the
		// other way around.
		let end = Duration::from_millis(2_450);
		let target = Target {
			broadcast: broadcast.consume(),
			video: catalog.snapshot().video.renditions.into_iter().next(),
			audio: None,
			end: moq_net::Timestamp::from_micros(end.as_micros() as u64).unwrap(),
		};

		let dir = tempfile::tempdir().unwrap();
		let scenes = dir.path().join("show.toml");
		std::fs::write(dir.path().join("show-clip-1.mp4"), b"an earlier clip").unwrap();
		let take = Take::clip(&target, Duration::from_secs(1)).unwrap();
		let path = save(take, &scenes).await.unwrap();
		assert_eq!(path, dir.path().join("show-clip-2.mp4"));
		assert_eq!(
			std::fs::read(dir.path().join("show-clip-1.mp4")).unwrap(),
			b"an earlier clip"
		);
		// A regular MP4 needs no repair, unlike one a crash cut short.
		let mut file = std::fs::File::open(&path).unwrap();
		assert!(moq_mux::container::mp4::repair(&mut file).unwrap().is_none());

		// It shows from the keyframe at 1 s, before the span's start, to its
		// end at the latest, and at most a few pictures short of it.
		file.rewind().unwrap();
		let moov = std::iter::from_fn(|| mp4_atom::ReadFrom::read_from(&mut file).unwrap())
			.find_map(|atom| match atom {
				mp4_atom::Any::Moov(moov) => Some(moov),
				_ => None,
			})
			.expect("a moov");
		let shows = Duration::from_secs(1)
			+ Duration::from_secs_f64(moov.mvhd.duration as f64 / f64::from(moov.mvhd.timescale));
		assert!(shows <= end, "shows until {shows:?}, past {end:?}");
		assert!(shows >= end - Duration::from_millis(200), "shows only until {shows:?}");
	}
}
