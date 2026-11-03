//! The capture devices this machine has, for the Devices panel and setup.

use moq_scene::Source;

/// A device the user can add as a source.
#[derive(Clone, Debug)]
pub struct Pick {
	pub name: String,
	pub source: Source,
}

/// One listing of every kind of device, each of which can fail on its own.
pub struct Listing {
	pub cameras: Result<Vec<Pick>, String>,
	pub displays: Result<Vec<Pick>, String>,
	pub windows: Result<Vec<Pick>, String>,
	/// Microphones and other inputs, plus system audio where it can be captured.
	pub audio: Result<Vec<Pick>, String>,
}

impl Listing {
	/// Each kind with its heading, in the order the Devices panel shows them.
	pub fn kinds(&mut self) -> [(&'static str, &mut Result<Vec<Pick>, String>); 4] {
		[
			("Cameras", &mut self.cameras),
			("Displays", &mut self.displays),
			("Windows", &mut self.windows),
			("Audio inputs", &mut self.audio),
		]
	}
}

#[cfg(feature = "capture")]
pub async fn list() -> Listing {
	fn picks<T>(result: Result<Vec<T>, impl std::fmt::Display>, pick: impl Fn(T) -> Pick) -> Result<Vec<Pick>, String> {
		result
			.map(|devices| devices.into_iter().map(pick).collect())
			.map_err(|err| err.to_string())
	}

	let cameras = picks(moq_video::capture::cameras().await, |camera| Pick {
		name: camera.name,
		source: Source::Camera { id: Some(camera.id) },
	});
	let displays = picks(moq_video::capture::displays().await, |display| Pick {
		name: display.name,
		source: Source::Display { id: Some(display.id) },
	});
	let windows = picks(moq_video::capture::windows().await, |window| Pick {
		name: format!("{} ({})", window.title, window.app),
		source: Source::Window { id: window.id },
	});
	let mut audio = picks(moq_audio::capture::devices().await, |device| Pick {
		name: format!("{} ({})", device.name, device.host),
		source: Source::Microphone { id: Some(device.id) },
	});
	if cfg!(target_os = "macos")
		&& let Ok(audio) = &mut audio
	{
		audio.push(Pick {
			name: "System audio".into(),
			source: Source::SystemAudio,
		});
	}

	Listing {
		cameras,
		displays,
		windows,
		audio,
	}
}

#[cfg(not(feature = "capture"))]
pub async fn list() -> Listing {
	let unsupported = || Err("this build cannot capture; rebuild with `--features capture`".to_string());
	Listing {
		cameras: unsupported(),
		displays: unsupported(),
		windows: unsupported(),
		audio: unsupported(),
	}
}
