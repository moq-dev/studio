//! Instant replays and clips: the recent past of the program or a source,
//! played in a replay source or saved to disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
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
	/// The program and the local sources have a past only while they are
	/// published, since nothing else encodes them, and a broadcast source once
	/// it shows a picture.
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
			Some(source) if source.has_video() => output()?
				.source(capture)
				.with_context(|| format!("`{name}` publishes no video to replay")),
			Some(_) => anyhow::bail!("`{name}` has no picture to replay"),
			None => anyhow::bail!("there is no source named `{name}`"),
		}
	}
}

/// Copy `length` of `target` and play it at `speed` in the replay source
/// `into`.
pub async fn play(
	engine: Arc<Engine>,
	target: Target,
	length: Duration,
	speed: f32,
	into: String,
) -> anyhow::Result<()> {
	let (take, copy) = Take::new(&target, length).await?;
	// A failed copy ends the take's tracks with the error, which the replay
	// source then reports.
	tokio::spawn(copy);
	let state = engine.state.lock().unwrap();
	let feed = state
		.sources
		.get(&into)
		.with_context(|| format!("the replay source `{into}` is not running"))?;
	feed.replay(take, speed)?;
	Ok(())
}

/// Copy `length` of `target` and save it, in real time, as a new MP4 file
/// beside `scenes`, the scene file. Returns where.
pub async fn save(target: Target, length: Duration, scenes: &Path) -> anyhow::Result<PathBuf> {
	let (take, copy) = Take::new(&target, length).await?;
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
	let (copied, written) = tokio::join!(copy, written);
	if let Err(err) = copied.map_err(anyhow::Error::from).and(written) {
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
	use super::*;

	/// A clip is a finished MP4 of the span, named beside the scene file
	/// without overwriting an earlier one.
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
		for index in 0..90u64 {
			if index % 30 == 0 {
				encoder.cut().unwrap();
			}
			let surface = moq_video::Surface::rgba(&gray, moq_video::Size::new(64, 64)).unwrap();
			let frame = moq_video::Frame::new(
				surface,
				moq_net::Timestamp::from_micros(index * 1_000_000 / 30).unwrap(),
			);
			producer.publish(&encoder.encode(&frame).unwrap()).unwrap();
		}
		let target = Target {
			broadcast: broadcast.consume(),
			video: catalog.snapshot().video.renditions.into_iter().next(),
			audio: None,
			end: moq_net::Timestamp::from_micros(2_500_000).unwrap(),
		};

		let dir = tempfile::tempdir().unwrap();
		let scenes = dir.path().join("show.toml");
		std::fs::write(dir.path().join("show-clip-1.mp4"), b"an earlier clip").unwrap();
		let length = Duration::from_secs(1);
		let path = save(target, length, &scenes).await.unwrap();
		assert_eq!(path, dir.path().join("show-clip-2.mp4"));
		assert_eq!(
			std::fs::read(dir.path().join("show-clip-1.mp4")).unwrap(),
			b"an earlier clip"
		);
		// A regular MP4 needs no repair, unlike one a crash cut short.
		let mut file = std::fs::File::open(&path).unwrap();
		assert!(moq_mux::container::mp4::repair(&mut file).unwrap().is_none());
	}
}
