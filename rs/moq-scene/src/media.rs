//! Media files, such as an intro clip or a music bed, demuxed by moq-mux into a
//! broadcast of their own for a media source to decode.
//!
//! The whole file is imported when the source opens, still compressed, so each
//! play decodes from its first frame at once, as a stinger needs, and a loop
//! never reads the disk again. The source holds the file's size in memory for
//! as long as it runs.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use hang::catalog::{AudioConfig, VideoConfig};
use moq_mux::import::ContainerFormat;

use crate::Error;

/// A media file's video and audio, held in a broadcast of their own.
pub(crate) struct Clip {
	broadcast: moq_net::broadcast::Producer,
	/// Keeps the imported tracks readable.
	_import: moq_mux::import::container::Finished,
	_catalog: moq_mux::catalog::Producer,
	/// The video rendition, by catalog name.
	pub video: Option<(String, VideoConfig)>,
	/// The audio rendition, by catalog name.
	pub audio: Option<(String, AudioConfig)>,
}

impl Clip {
	/// Read and demux the file at `path`. Refuses a container moq-mux does not
	/// read, saying which it does, and a file with neither video nor audio in
	/// it that moq-mux reads.
	pub fn open(path: &Path) -> Result<Self, Error> {
		let failed = |err: String| Error::Media(format!("{}: {err}", path.display()));
		let bytes = std::fs::read(path).map_err(|err| failed(err.to_string()))?;
		let format = container(&mut std::io::Cursor::new(&bytes)).map_err(failed)?;

		let mut broadcast = moq_net::broadcast::Info::new().produce();
		// The file is written whole before anything reads it, so nothing in it
		// may go stale behind the newest frame.
		let config = moq_mux::catalog::Config::default().with_max_age(None);
		let catalog = moq_mux::catalog::Producer::new(&mut broadcast, config)?;
		let init = moq_mux::import::ContainerInit::new(format, bytes);
		let import = moq_mux::import::Container::new(broadcast.clone(), catalog.reserve(), &init)
			.map_err(|err| failed(err.to_string()))?
			.finish()
			.map_err(|err| failed(err.to_string()))?;

		let listed = catalog.snapshot();
		let video = crate::source::pick(listed.video.enabled())?;
		let audio = crate::source::pick(listed.audio.enabled())?;
		if video.is_none() && audio.is_none() {
			return Err(failed("it holds no video or audio the studio plays".into()));
		}
		Ok(Self {
			broadcast,
			_import: import,
			_catalog: catalog,
			video,
			audio,
		})
	}

	/// The clip's broadcast, to decode from its start.
	pub fn consume(&self) -> moq_net::broadcast::Consumer {
		self.broadcast.consume()
	}
}

/// The containers a media file may be in, as moq-mux reads them.
pub(crate) const READ: &str = "fragmented MP4, MKV or WebM, MPEG-TS, or FLV";

/// Which container `file` is in, by its first bytes and, for an MP4, its
/// top-level boxes, reading little else.
///
/// A regular MP4, with its samples indexed in a `moov` rather than in
/// fragments, is refused: moq-mux demuxes only fragmented MP4.
pub(crate) fn container(file: &mut (impl Read + Seek)) -> Result<ContainerFormat, String> {
	const TS_PACKET: usize = 188;
	let mut head = [0; TS_PACKET + 1];
	let read = fill(file, &mut head).map_err(|err| err.to_string())?;
	let bytes = &head[..read];
	if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
		return Ok(ContainerFormat::Mkv);
	}
	if bytes.starts_with(b"FLV") {
		return Ok(ContainerFormat::Flv);
	}
	if bytes.first() == Some(&0x47) && bytes.get(TS_PACKET).is_none_or(|sync| *sync == 0x47) {
		return Ok(ContainerFormat::Ts);
	}
	if bytes.get(4..8) == Some(b"ftyp") {
		return match fragmented(file).map_err(|err| err.to_string())? {
			true => Ok(ContainerFormat::Fmp4),
			false => Err(format!(
				"a regular MP4 is not read yet; remux it into one of {READ}, such as with \
				 `ffmpeg -i in.mp4 -c copy -movflags frag_keyframe+empty_moov out.mp4`"
			)),
		};
	}
	Err(format!("not a container the studio reads; it reads {READ}"))
}

/// Read into `buf` until it is full or the file ends; how much was read.
pub(crate) fn fill(file: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
	let mut read = 0;
	while read < buf.len() {
		match file.read(&mut buf[read..]) {
			Ok(0) => break,
			Ok(more) => read += more,
			Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
			Err(err) => return Err(err),
		}
	}
	Ok(read)
}

/// Whether the MP4 in `file` holds a fragment (`moof`) among its top-level
/// boxes, reading only their headers.
fn fragmented(file: &mut (impl Read + Seek)) -> std::io::Result<bool> {
	let end = file.seek(SeekFrom::End(0))?;
	let mut at = 0;
	while end - at >= 8 {
		file.seek(SeekFrom::Start(at))?;
		let mut header = [0; 16];
		let read = fill(file, &mut header)?;
		if &header[4..8] == b"moof" {
			return Ok(true);
		}
		let size = u32::from_be_bytes(header[..4].try_into().expect("four bytes"));
		let size = match size {
			// To the end of the file.
			0 => return Ok(false),
			// A 64-bit size follows the type.
			1 if read == 16 => u64::from_be_bytes(header[8..].try_into().expect("eight bytes")),
			1 => return Ok(false),
			size => u64::from(size),
		};
		match size >= 8 && size <= end - at {
			true => at += size,
			false => return Ok(false),
		}
	}
	Ok(false)
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;

	/// One second of 64x64 H.264 at 10 fps with a keyframe every half second,
	/// and stereo Opus, in MKV:
	///
	/// ```sh
	/// ffmpeg -f lavfi -i testsrc2=size=64x64:rate=10:duration=1 \
	///   -f lavfi -i sine=frequency=440:sample_rate=48000:duration=1 \
	///   -c:v libx264 -profile:v baseline -g 5 -bf 0 -c:a libopus -b:a 24k -ac 2 clip.mkv
	/// ```
	pub(crate) const CLIP: &[u8] = include_bytes!("test_data/clip.mkv");

	/// The same second, remuxed with `-c copy -movflags frag_keyframe+empty_moov`.
	const FRAGMENTED: &[u8] = include_bytes!("test_data/clip.mp4");

	/// Half a second of stereo Opus alone, in WebM.
	pub(crate) const TONE: &[u8] = include_bytes!("test_data/tone.webm");

	/// The first half second of [`CLIP`]'s pictures alone.
	pub(crate) const SILENT: &[u8] = include_bytes!("test_data/silent.mkv");

	/// [`CLIP`]'s second of pictures with [`TONE`]'s half second of sound.
	pub(crate) const UNEVEN: &[u8] = include_bytes!("test_data/uneven.mkv");

	fn container(bytes: &[u8]) -> Result<ContainerFormat, String> {
		super::container(&mut std::io::Cursor::new(bytes))
	}

	#[test]
	fn reads_by_the_first_bytes() {
		assert_eq!(container(CLIP), Ok(ContainerFormat::Mkv));
		assert_eq!(container(FRAGMENTED), Ok(ContainerFormat::Fmp4));
		assert_eq!(container(b"FLV\x01"), Ok(ContainerFormat::Flv));
		let mut ts = vec![0; 2 * 188];
		(ts[0], ts[188]) = (0x47, 0x47);
		assert_eq!(container(&ts), Ok(ContainerFormat::Ts));
		ts[188] = 0;
		assert!(container(&ts).is_err(), "one sync byte is not a stream");
		assert!(container(b"\x89PNG\r\n\x1a\n").unwrap_err().contains(READ));
	}

	/// A regular MP4 says how to make one moq-mux reads.
	#[test]
	fn a_regular_mp4_is_refused() {
		let mut mp4 = Vec::new();
		for (kind, body) in [(b"ftyp", 8), (b"moov", 32), (b"mdat", 64)] {
			mp4.extend_from_slice(&(8 + body as u32).to_be_bytes());
			mp4.extend_from_slice(kind);
			mp4.resize(mp4.len() + body, 0);
		}
		let err = container(&mp4).unwrap_err();
		assert!(err.contains("regular MP4") && err.contains("frag_keyframe"), "{err}");
		// Even when a box says it is larger than the file.
		mp4.truncate(mp4.len() - 1);
		assert!(container(&mp4).is_err());
	}

	#[test]
	fn opens_both_renditions() {
		let dir = tempfile::tempdir().unwrap();
		for (name, bytes) in [("clip.mkv", CLIP), ("clip.mp4", FRAGMENTED)] {
			let path = dir.path().join(name);
			std::fs::write(&path, bytes).unwrap();
			let clip = Clip::open(&path).unwrap();
			assert!(clip.video.is_some(), "{name} has no video");
			assert!(clip.audio.is_some(), "{name} has no audio");
		}

		let missing = dir.path().join("missing.mkv");
		let err = Clip::open(&missing).err().unwrap().to_string();
		assert!(err.contains("missing.mkv"), "{err}");
	}
}
