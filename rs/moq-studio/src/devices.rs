//! The capture devices this machine has, for Studio's Add and the quick start.

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

/// List every kind of device, logging what each listing found.
pub async fn list() -> Listing {
	fn found(devices: &Result<Vec<Pick>, String>) -> Result<usize, &str> {
		devices.as_ref().map(Vec::len).map_err(String::as_str)
	}

	let listing = find().await;
	tracing::info!(
		cameras = ?found(&listing.cameras),
		displays = ?found(&listing.displays),
		windows = ?found(&listing.windows),
		audio = ?found(&listing.audio),
		"listed devices"
	);
	listing
}

#[cfg(feature = "capture")]
async fn find() -> Listing {
	fn picks<T>(result: Result<Vec<T>, impl std::fmt::Display>, pick: impl Fn(T) -> Pick) -> Result<Vec<Pick>, String> {
		result
			.map(|devices| devices.into_iter().map(pick).collect())
			.map_err(|err| err.to_string())
	}

	let cameras = moq_video::capture::cameras().await;
	// Never the program's virtual camera, which the camera extension lists
	// even while it is off, and which Add or the quick start would feed what it shows
	// back into. Every other one of the studio's is listed only while on,
	// when Add leaves it out.
	#[cfg(target_os = "macos")]
	let cameras = cameras.map(|cameras| {
		cameras
			.into_iter()
			.filter(|camera| camera.id != moq_studio_camera::PROGRAM)
			.collect::<Vec<_>>()
	});
	let cameras = picks(cameras, |camera| Pick {
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
	#[cfg(all(target_os = "linux", feature = "pipewire"))]
	let (displays, windows) = {
		let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
		(
			with_picker(displays, Source::PickedDisplay, wayland),
			with_picker(windows, Source::PickedWindow, wayland),
		)
	};
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

/// Offer `picked`, the system picker, beside the X11 `listed`: first on a
/// Wayland desktop, whose screens X11 cannot capture, so the quick start
/// adds it, and last otherwise. A Wayland desktop may have no X server to list at all,
/// which leaves the picker.
#[cfg(all(target_os = "linux", feature = "pipewire"))]
fn with_picker(listed: Result<Vec<Pick>, String>, picked: Source, wayland: bool) -> Result<Vec<Pick>, String> {
	let name = match picked {
		Source::PickedDisplay => "Picked display",
		_ => "Picked window",
	};
	let pick = Pick {
		name: name.into(),
		source: picked,
	};
	let mut picks = match listed {
		Ok(picks) => picks,
		Err(err) if wayland => {
			tracing::info!(%err, "no X11 screens to list beside the system picker");
			Vec::new()
		}
		Err(err) => return Err(err),
	};
	match wayland {
		true => picks.insert(0, pick),
		false => picks.push(pick),
	}
	Ok(picks)
}

#[cfg(not(feature = "capture"))]
async fn find() -> Listing {
	let unsupported = || Err("this build cannot capture; rebuild with `--features capture`".to_string());
	Listing {
		cameras: unsupported(),
		displays: unsupported(),
		windows: unsupported(),
		audio: unsupported(),
	}
}
