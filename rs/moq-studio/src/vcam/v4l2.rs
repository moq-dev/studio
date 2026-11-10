//! The fallback camera: an existing v4l2loopback device, written to as I420.
//!
//! v4l2loopback is a kernel module, so the studio never creates a device; it
//! takes the first one it finds that accepts the picture's size and that none
//! of its other cameras holds.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use moq_v4l::video::Output;
use moq_video::{I420, Size};

use super::Sink;

/// The driver name v4l2loopback devices report.
const DRIVER: &str = "v4l2 loopback";

/// I420, which every V4L2 camera reader takes.
const FOURCC: &[u8; 4] = b"YU12";

/// The devices this process's cameras write to. The kernel lets a second
/// writer set the same format, which would show two cameras' pictures in turn.
static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

pub struct Loopback {
	device: moq_v4l::Device,
	path: PathBuf,
	size: Size,
}

impl Loopback {
	/// Open the first v4l2loopback device that takes `size` I420 frames and
	/// that no other camera holds.
	pub fn open(size: Size, framerate: u32) -> anyhow::Result<Self> {
		let mut refused = Vec::new();
		// Held across the search, so two cameras opening at once never pick
		// the same device.
		let mut held = HELD.lock().unwrap();
		for path in devices()? {
			let Ok(device) = moq_v4l::Device::with_path(&path) else {
				continue;
			};
			match device.query_caps() {
				Ok(caps) if caps.driver == DRIVER => {}
				_ => continue,
			}
			if held.contains(&path) {
				refused.push(format!("{}: another virtual camera holds it", path.display()));
				continue;
			}
			match configure(&device, size, framerate) {
				Ok(()) => {
					held.insert(path.clone());
					return Ok(Self { device, path, size });
				}
				Err(err) => refused.push(format!("{}: {err:#}", path.display())),
			}
		}
		match refused.is_empty() {
			true => anyhow::bail!("no device; load the v4l2loopback module to create one"),
			false => anyhow::bail!("no device takes the picture ({})", refused.join("; ")),
		}
	}

	pub fn name(&self) -> String {
		self.path.display().to_string()
	}
}

impl Drop for Loopback {
	fn drop(&mut self) {
		HELD.lock().unwrap().remove(&self.path);
	}
}

impl Sink for Loopback {
	fn write(&mut self, rgba: &[u8]) -> anyhow::Result<()> {
		let i420 = moq_video::Surface::rgba(rgba, self.size)?.into_i420()?;
		// v4l2loopback takes one frame per write, so a short one would split it.
		let written = self
			.device
			.write(i420.data())
			.with_context(|| format!("writing to {}", self.path.display()))?;
		anyhow::ensure!(
			written == i420.data().len(),
			"{} took {written} of a frame's {} bytes",
			self.path.display(),
			i420.data().len()
		);
		Ok(())
	}
}

/// Every `/dev/videoN`, in number order.
fn devices() -> anyhow::Result<Vec<PathBuf>> {
	let mut devices: Vec<(u32, PathBuf)> = std::fs::read_dir("/dev")
		.context("listing /dev")?
		.filter_map(|entry| {
			let path = entry.ok()?.path();
			let number = path.file_name()?.to_str()?.strip_prefix("video")?.parse().ok()?;
			Some((number, path))
		})
		.collect();
	devices.sort();
	Ok(devices.into_iter().map(|(_, path)| path).collect())
}

/// Set `device` to take `size` I420 frames at `framerate`, refusing any other
/// layout it answers with.
fn configure(device: &moq_v4l::Device, size: Size, framerate: u32) -> anyhow::Result<()> {
	let fourcc = moq_v4l::FourCC::new(FOURCC);
	let mut want = moq_v4l::Format::new(size.width, size.height, fourcc);
	want.stride = size.width;
	let len = I420::len(size)?;
	want.size = len as u32;
	let set = device.set_format(&want).context("setting the format")?;
	anyhow::ensure!(
		set.width == size.width && set.height == size.height && set.fourcc == fourcc,
		"it took {}x{} {} rather than {size} {}",
		set.width,
		set.height,
		set.fourcc,
		fourcc
	);
	anyhow::ensure!(
		set.stride == size.width && set.size as usize == len,
		"it pads I420 rows to {} bytes",
		set.stride
	);
	device
		.set_params(&moq_v4l::video::output::Parameters::with_fps(framerate))
		.context("setting the frame rate")?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use std::io::Read;

	use super::*;

	/// A V4L2 app reading the loopback device gets the picture written to it,
	/// and another camera never writes to the same device.
	#[test]
	#[ignore = "needs a v4l2loopback device; run by just v4l2loopback"]
	fn a_v4l2_app_reads_the_program() {
		let size = Size::new(320, 180);
		let mut loopback = Loopback::open(size, 30).expect("a v4l2loopback device");
		let red = [255, 0, 0, 255].repeat((size.width * size.height) as usize);
		// A reader opens the device once a writer has set it up, as an app does.
		loopback.write(&red).expect("write");
		let mut reader = moq_v4l::Device::with_path(&loopback.path).expect("the reader opens");
		let mut frame = vec![0; I420::len(size).unwrap()];
		loopback.write(&red).expect("write");
		let read = reader.read(&mut frame).expect("read");
		assert_eq!(read, frame.len());

		let i420 = I420::new(size, frame).expect("an I420 frame");
		let center = |plane: &[u8], width: u32, height: u32| plane[(height / 2 * width + width / 2) as usize];
		let (y, u, v) = (
			center(i420.y(), size.width, size.height),
			center(i420.u(), size.width / 2, size.height / 2),
			center(i420.v(), size.width / 2, size.height / 2),
		);
		// Red, in any matrix: dim luma, blue-difference below gray, and
		// red-difference well above it.
		assert!(
			y < 100 && u < 128 && v > 200,
			"the reader got y={y} u={u} v={v}, not red"
		);

		if let Ok(other) = Loopback::open(size, 30) {
			assert_ne!(other.path, loopback.path, "two cameras share a device");
		}
		let path = loopback.path.clone();
		drop(loopback);
		assert_eq!(Loopback::open(size, 30).expect("the device is free again").path, path);
	}
}
