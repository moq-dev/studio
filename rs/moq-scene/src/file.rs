//! Which source shows a file, such as one dropped on the picture, read the
//! way that source reads it.

use std::path::Path;

use crate::{Error, Source};

/// The file extensions of the pictures and videos [`source`] shows, for a file
/// dialog to list: the PNG and JPEG stills it reads, and the video containers
/// it reads. A file under another name may still show,
/// since [`source`] reads the bytes, so this narrows a picker, not a drop.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "mp4", "m4v", "mkv", "webm", "ts", "flv"];

/// The source that shows the file at `path`: a PNG or JPEG still, a video or
/// sound file, or a `.cube` LUT. Reads only the first bytes, and an MP4's
/// top-level box headers, so a drag can ask on every frame.
///
/// Refuses a file no source shows, saying what the studio does show, and a
/// regular MP4 with the remux that makes it readable. Refuses anything but a
/// regular file before opening it, since opening a named pipe waits for a
/// writer.
pub fn source(path: &Path) -> Result<Source, Error> {
	let failed = |err: String| Error::Unsupported(format!("{}: {err}", path.display()));
	let metadata = std::fs::metadata(path).map_err(|err| failed(err.to_string()))?;
	if !metadata.is_file() {
		return Err(failed("not a file".into()));
	}
	let cube = path
		.extension()
		.is_some_and(|extension| extension.eq_ignore_ascii_case("cube"));
	if cube {
		return Ok(Source::Lut { path: path.into() });
	}
	let mut file = std::fs::File::open(path).map_err(|err| failed(err.to_string()))?;
	let mut head = [0; 64];
	let read = crate::media::fill(&mut file, &mut head).map_err(|err| failed(err.to_string()))?;
	match image::guess_format(&head[..read]) {
		Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg) => return Ok(Source::Image { path: path.into() }),
		Ok(format) => {
			let name = format
				.extensions_str()
				.first()
				.copied()
				.unwrap_or("this")
				.to_uppercase();
			return Err(failed(format!(
				"a {name} image is not shown; the studio shows PNG or JPEG stills"
			)));
		}
		Err(_) => {}
	}
	std::io::Seek::rewind(&mut file).map_err(|err| failed(err.to_string()))?;
	match crate::media::container(&mut file) {
		Ok(_) => Ok(Source::Media {
			path: path.into(),
			looping: false,
			restart: false,
		}),
		// The remux is the fix for a file that is media after all.
		Err(err) if err.contains("regular MP4") => Err(failed(err)),
		Err(_) => Err(failed(format!(
			"not a file the studio shows; it shows PNG or JPEG stills, video or sound in {}, and .cube color \
			 grades",
			crate::media::READ
		))),
	}
}

/// What a file shows on the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
	/// Nothing to place: a sound alone, or a LUT.
	None,
	/// A picture this large.
	Sized(moq_video::Size),
	/// A picture whose size the file does not say before it plays.
	Unsized,
}

/// The picture `source`, as [`source`] made it, shows: an image's size from
/// its header, or a media file's video, read whole as the source would, which
/// also checks it holds something the studio plays. A LUT is read whole to
/// check it parses.
pub fn picture(source: &Source) -> Result<Picture, Error> {
	match source {
		Source::Image { path } => {
			let failed = |err: String| Error::Image(format!("{}: {err}", path.display()));
			let reader = image::ImageReader::open(path).map_err(|err| failed(err.to_string()))?;
			let reader = reader.with_guessed_format().map_err(|err| failed(err.to_string()))?;
			let (width, height) = reader.into_dimensions().map_err(|err| failed(err.to_string()))?;
			Ok(Picture::Sized(moq_video::Size::new(width, height)))
		}
		Source::Media { path, .. } => {
			let clip = crate::media::Clip::open(path)?;
			Ok(
				match clip
					.video
					.as_ref()
					.map(|(_, config)| (config.coded_width, config.coded_height))
				{
					None => Picture::None,
					Some((Some(width), Some(height))) if width > 0 && height > 0 => {
						Picture::Sized(moq_video::Size::new(width, height))
					}
					Some(_) => Picture::Unsized,
				},
			)
		}
		Source::Lut { path } => {
			let failed = |err: String| Error::Lut(format!("{}: {err}", path.display()));
			let text = std::fs::read_to_string(path).map_err(|err| failed(err.to_string()))?;
			crate::lut::Cube::parse(&text).map_err(failed)?;
			Ok(Picture::None)
		}
		other => Err(Error::Unsupported(format!("{other:?} reads no file"))),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::media::tests::{CLIP, TONE};

	fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
		let path = dir.join(name);
		std::fs::write(&path, bytes).unwrap();
		path
	}

	/// The picker's stills are the ones [`source`] shows, and its names are
	/// the lowercase ones a dialog matches.
	#[test]
	fn extensions_list_what_shows() {
		for extension in EXTENSIONS {
			assert_eq!(*extension, extension.to_lowercase());
			if let Some(format) = image::ImageFormat::from_extension(extension) {
				assert!(
					matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg),
					"{extension}"
				);
			}
		}
		for still in ["png", "jpg", "jpeg"] {
			assert!(EXTENSIONS.contains(&still));
		}
	}

	/// Each kind is found by what the file holds, not its name, except a LUT,
	/// which is text with no mark of its own.
	#[test]
	fn each_kind_by_its_bytes() {
		let dir = tempfile::tempdir().unwrap();
		let person = write(dir.path(), "person.dat", include_bytes!("test_data/person.jpg"));
		let source = super::source(&person).unwrap();
		assert_eq!(source, Source::Image { path: person.clone() });
		let Picture::Sized(size) = picture(&source).unwrap() else {
			panic!("no size")
		};
		let decoded = image::load_from_memory(include_bytes!("test_data/person.jpg")).unwrap();
		assert_eq!((size.width, size.height), (decoded.width(), decoded.height()));

		let clip = write(dir.path(), "clip.png", CLIP);
		let source = super::source(&clip).unwrap();
		assert!(matches!(source, Source::Media { .. }), "{source:?}");
		assert_eq!(picture(&source).unwrap(), Picture::Sized(moq_video::Size::new(64, 64)));

		let tone = write(dir.path(), "tone.webm", TONE);
		assert_eq!(
			picture(&super::source(&tone).unwrap()).unwrap(),
			Picture::None,
			"a sound has no picture"
		);

		let grade = write(
			dir.path(),
			"grade.CUBE",
			b"LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
		);
		let source = super::source(&grade).unwrap();
		assert_eq!(source, Source::Lut { path: grade });
		assert_eq!(picture(&source).unwrap(), Picture::None);
	}

	/// What the studio cannot show says why, with the fix where it has one.
	#[test]
	fn refusals_say_why() {
		let dir = tempfile::tempdir().unwrap();
		let mut mp4 = Vec::new();
		for (kind, body) in [(b"ftyp", 8), (b"moov", 32), (b"mdat", 64)] {
			mp4.extend_from_slice(&(8 + body as u32).to_be_bytes());
			mp4.extend_from_slice(kind);
			mp4.resize(mp4.len() + body, 0);
		}
		let regular = write(dir.path(), "intro.mp4", &mp4);
		let err = source(&regular).unwrap_err().to_string();
		assert!(err.contains("frag_keyframe") && err.contains("intro.mp4"), "{err}");

		let gif = write(dir.path(), "cat.gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;");
		let err = source(&gif).unwrap_err().to_string();
		assert!(err.contains("GIF") && err.contains("PNG or JPEG"), "{err}");

		let notes = write(dir.path(), "notes.txt", b"run of show");
		let err = source(&notes).unwrap_err().to_string();
		assert!(err.contains("not a file the studio shows"), "{err}");

		let empty = write(dir.path(), "empty", b"");
		assert!(source(&empty).is_err());
		assert!(source(&dir.path().join("missing.png")).is_err());

		let broken = write(dir.path(), "broken.cube", b"LUT_1D_SIZE 2\n");
		assert!(
			picture(&source(&broken).unwrap()).is_err(),
			"a LUT is parsed before it is placed"
		);

		let err = source(dir.path()).unwrap_err().to_string();
		assert!(err.contains("not a file"), "{err}");
	}

	/// A named pipe with no writer is refused, not opened, which would wait
	/// for one.
	#[cfg(target_os = "linux")]
	#[test]
	fn a_pipe_is_refused_unopened() {
		use rustix::fs::{CWD, FileType, Mode, mknodat};
		let dir = tempfile::tempdir().unwrap();
		for name in ["pipe.png", "pipe.cube"] {
			let pipe = dir.path().join(name);
			mknodat(CWD, &pipe, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).unwrap();
			let err = source(&pipe).unwrap_err().to_string();
			assert!(err.contains("not a file"), "{err}");
		}
	}
}
