//! What the studio and its macOS camera extension agree on.
//!
//! The extension (this crate's binary) is a CoreMediaIO Camera Extension the
//! app bundle carries. It always lists one camera, the program's, and adds a
//! camera for a published scene when the studio asks. Each camera has a source
//! stream, which apps read, and a sink stream, which only the studio may feed:
//! the extension passes each picture the studio puts on the sink to the source.
//!
//! The studio talks to the extension through two custom properties on a
//! camera, the only channel CoreMediaIO offers a sandboxed extension:
//! [`CONFIG`], which reads as what the camera shows and, written on the
//! program's camera, asks for a camera; and [`READING`], whether an app reads
//! the camera, so the studio copies pictures only then.

use std::fmt;
use std::str::FromStr;

/// The app's bundle identifier, the only client allowed to feed a camera.
pub const APP: &str = "live.kixel.studio";

/// The extension's bundle identifier, which the app asks the system to
/// activate.
pub const EXTENSION: &str = "live.kixel.studio.camera";

/// What apps list the program's camera as. A scene's camera adds `: <scene>`.
pub const NAME: &str = "kixel.live Studio";

/// The program camera's device id, fixed so apps remember it across launches
/// and updates.
pub const PROGRAM: &str = "6B1D0F4E-3C52-4E8B-9A57-2D0C1F6E8A10";

/// The custom property holding a camera's [`Config`] as text.
pub const CONFIG: Property = Property(*b"kcfg");

/// The custom property saying whether an app reads the camera: `1` or `0`.
pub const READING: Property = Property(*b"krdg");

/// A custom CoreMediaIO property, named by its four-character selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Property(pub [u8; 4]);

impl Property {
	/// The selector the studio addresses it by through the CoreMediaIO
	/// hardware API.
	pub fn selector(self) -> u32 {
		u32::from_be_bytes(self.0)
	}

	/// The name the extension declares it by: a global property of the
	/// object it is on, as CoreMediaIO spells custom properties.
	pub fn name(self) -> String {
		format!("4cc_{}_glob_0000", String::from_utf8_lossy(&self.0))
	}
}

/// The largest picture a camera shows on either side: 8K.
const MAX_SIZE: u32 = 8192;

/// The highest frame rate a camera shows.
const MAX_FRAMERATE: u32 = 240;

/// The longest scene name a camera is named after, in bytes.
const MAX_SCENE: usize = 256;

/// What a camera shows: the program or a scene, at a size and frame rate.
///
/// As text, `1920x1080@30` for the program and `1920x1080@30 Scene` for the
/// scene called `Scene`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
	/// The published scene, or `None` for the program.
	pub scene: Option<String>,
	pub width: u32,
	pub height: u32,
	pub framerate: u32,
}

impl Config {
	/// The program at 1080p30, what its camera shows before the studio asks
	/// for anything else.
	pub fn program() -> Self {
		Self {
			scene: None,
			width: 1920,
			height: 1080,
			framerate: 30,
		}
	}

	/// The device id of the camera showing it: [`PROGRAM`], or one derived
	/// from the scene's name, so an app remembers a scene's camera too.
	pub fn device(&self) -> String {
		match &self.scene {
			None => PROGRAM.to_string(),
			Some(scene) => scene_device(scene),
		}
	}

	/// What apps list the camera as.
	pub fn name(&self) -> String {
		match &self.scene {
			None => NAME.to_string(),
			Some(scene) => format!("{NAME}: {scene}"),
		}
	}
}

impl fmt::Display for Config {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}x{}@{}", self.width, self.height, self.framerate)?;
		if let Some(scene) = &self.scene {
			write!(f, " {scene}")?;
		}
		Ok(())
	}
}

/// Why a [`Config`]'s text was refused.
#[derive(Debug, PartialEq, Eq)]
pub struct Invalid(String);

impl fmt::Display for Invalid {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "invalid camera config {:?}", self.0)
	}
}

impl std::error::Error for Invalid {}

impl FromStr for Config {
	type Err = Invalid;

	/// Parses what [`Display`](fmt::Display) writes. Any app can write the
	/// property, so this refuses anything the studio would not ask for.
	fn from_str(text: &str) -> Result<Self, Self::Err> {
		let invalid = || Invalid(text.chars().take(64).collect());
		let (format, scene) = match text.split_once(' ') {
			Some((format, scene)) => (format, Some(scene)),
			None => (text, None),
		};
		let (size, framerate) = format.split_once('@').ok_or_else(invalid)?;
		let (width, height) = size.split_once('x').ok_or_else(invalid)?;
		let number = |text: &str, max: u32| {
			// `parse` takes a leading `+`, which `Display` never writes.
			text.bytes()
				.all(|byte| byte.is_ascii_digit())
				.then(|| text.parse::<u32>().ok())
				.flatten()
				.filter(|&value| (1..=max).contains(&value))
				.ok_or_else(invalid)
		};
		let scene = match scene {
			Some(scene) if scene.is_empty() || scene.len() > MAX_SCENE || scene.chars().any(char::is_control) => {
				return Err(invalid());
			}
			scene => scene.map(str::to_string),
		};
		Ok(Self {
			scene,
			width: number(width, MAX_SIZE)?,
			height: number(height, MAX_SIZE)?,
			framerate: number(framerate, MAX_FRAMERATE)?,
		})
	}
}

/// A scene camera's device id: a UUID (version 8, custom) from the 128-bit
/// FNV-1a hash of its name. Never change it: apps remember cameras by it.
fn scene_device(scene: &str) -> String {
	const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
	const PRIME: u128 = 0x0000000001000000000000000000013b;
	let mut hash = OFFSET;
	for byte in scene.bytes() {
		hash ^= u128::from(byte);
		hash = hash.wrapping_mul(PRIME);
	}
	// The version and variant bits RFC 9562 puts in every UUID.
	hash = (hash & !(0xf << 76)) | (0x8 << 76);
	hash = (hash & !(0x3 << 62)) | (0x2 << 62);
	let hex = format!("{hash:032X}");
	format!(
		"{}-{}-{}-{}-{}",
		&hex[..8],
		&hex[8..12],
		&hex[12..16],
		&hex[16..20],
		&hex[20..]
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn configs_round_trip() {
		for text in ["1920x1080@30", "1280x720@60 Main", "1x1@1 a scene: with spaces"] {
			let config: Config = text.parse().unwrap();
			assert_eq!(config.to_string(), text);
		}
		let config: Config = "640x480@25 Interview".parse().unwrap();
		assert_eq!(
			config,
			Config {
				scene: Some("Interview".to_string()),
				width: 640,
				height: 480,
				framerate: 25,
			}
		);
	}

	#[test]
	fn refuses_what_the_studio_never_asks_for() {
		for text in [
			"",
			"1920x1080",
			"1920@30",
			"0x1080@30",
			"1920x0@30",
			"1920x1080@0",
			"8193x1080@30",
			"1920x1080@241",
			"+1920x1080@30",
			"1920x1080@30 ",
			"1920x1080@30 a\nb",
			"99999999999999999999x1080@30",
		] {
			assert!(text.parse::<Config>().is_err(), "{text:?}");
		}
		let long = format!("1920x1080@30 {}", "a".repeat(MAX_SCENE + 1));
		assert!(long.parse::<Config>().is_err());
	}

	#[test]
	fn scene_cameras_keep_their_ids() {
		let scene = |name: &str| Config {
			scene: Some(name.to_string()),
			..Config::program()
		};
		// Pinned: apps remember a camera by its id, so it must never change.
		assert_eq!(scene("Main").device(), "6828B994-E175-8277-B806-E911DE48E2B8");
		assert_eq!(scene("Main").device(), scene("Main").device());
		assert_ne!(scene("Main").device(), scene("main").device());
		assert_eq!(Config::program().device(), PROGRAM);
		for id in [scene("Main").device(), scene("").device(), PROGRAM.to_string()] {
			let groups: Vec<_> = id.split('-').map(str::len).collect();
			assert_eq!(groups, [8, 4, 4, 4, 12], "{id}");
			assert!(
				id.chars()
					.all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
			);
		}
	}

	#[test]
	fn names_properties_as_coremediaio_does() {
		assert_eq!(CONFIG.name(), "4cc_kcfg_glob_0000");
		assert_eq!(CONFIG.selector(), 0x6b636667);
		assert_eq!(READING.name(), "4cc_krdg_glob_0000");
	}
}
