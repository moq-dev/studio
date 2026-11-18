//! Copy a page's shared texture onto the studio's device.

#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;

#[cfg(target_os = "macos")]
use objc2_io_surface::IOSurfaceRef;

use super::protocol::Texture;

/// Copy the DMA-BUF `texture` describes, whose plane descriptors are `fds`,
/// into a texture of the studio's own on `device`, waiting until the copy is
/// done.
#[cfg(all(target_os = "linux", feature = "cef"))]
pub(super) fn texture(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	texture: &Texture,
	fds: Vec<OwnedFd>,
) -> Result<wgpu::Texture, String> {
	use std::os::fd::AsRawFd;

	use cef::osr_texture_import::SharedTextureHandle;

	use super::protocol::MAX_PLANES;

	if fds.len() != texture.planes.len() || fds.is_empty() || fds.len() > MAX_PLANES {
		return Err(format!(
			"a shared texture came with {} planes and {} descriptors",
			texture.planes.len(),
			fds.len()
		));
	}
	let mut info = paint_info(texture);
	for ((slot, plane), fd) in info.planes.iter_mut().zip(&texture.planes).zip(&fds) {
		slot.stride = plane.stride;
		slot.offset = plane.offset;
		slot.size = plane.size;
		slot.fd = fd.as_raw_fd();
	}
	info.plane_count = fds.len() as _;
	info.modifier = texture.modifier;

	// SAFETY: the helper keeps the texture out of CEF's hands until it hears
	// back, and `fds` stay open until this returns. The device is the
	// studio's, opened by `moq_video::render::request_device` with DMA-BUF
	// import, on the GPU CEF paints with.
	unsafe { SharedTextureHandle::new(&info).import_texture(device, queue) }
		.map_err(|err| format!("cannot import the page's texture: {err}"))
}

/// Copy the part of `surface` that `texture` describes into a texture of the
/// studio's own on `device`, waiting until the copy is done.
#[cfg(all(target_os = "macos", feature = "cef"))]
pub(super) fn texture(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	texture: &Texture,
	surface: &IOSurfaceRef,
) -> Result<wgpu::Texture, String> {
	use cef::osr_texture_import::SharedTextureHandle;

	use super::protocol::Format;

	// Metal reads only what the surface holds.
	let (width, height) = (surface.width(), surface.height());
	if texture.width as usize > width || texture.height as usize > height {
		return Err(format!(
			"a {}x{} picture arrived on a {width}x{height} surface",
			texture.width, texture.height
		));
	}
	let format = match texture.format {
		Format::Rgba => *b"RGBA",
		Format::Bgra => *b"BGRA",
	};
	if surface.pixel_format() != u32::from_be_bytes(format) {
		return Err(format!(
			"a {:?} picture arrived on a surface of format {:#x}",
			texture.format,
			surface.pixel_format()
		));
	}
	let mut info = paint_info(texture);
	info.shared_texture_io_surface = std::ptr::from_ref(surface).cast_mut().cast();

	// SAFETY: the helper keeps the surface out of CEF's hands until it hears
	// back, and `surface` stays alive until this returns. The device is the
	// studio's, a Metal one, as `super::gpu` checked.
	unsafe { SharedTextureHandle::new(&info).import_texture(device, queue) }
		.map_err(|err| format!("cannot import the page's texture: {err}"))
}

/// CEF's description of `texture`, without its handles.
#[cfg(feature = "cef")]
fn paint_info(texture: &Texture) -> cef::AcceleratedPaintInfo {
	use super::protocol::Format;

	let mut info = cef::AcceleratedPaintInfo {
		format: match texture.format {
			Format::Rgba => cef::ColorType::RGBA_8888,
			Format::Bgra => cef::ColorType::BGRA_8888,
		},
		..Default::default()
	};
	info.extra.coded_size.width = texture.width as _;
	info.extra.coded_size.height = texture.height as _;
	info
}

#[cfg(all(target_os = "linux", not(feature = "cef")))]
pub(super) fn texture(
	_: &wgpu::Device,
	_: &wgpu::Queue,
	_: &Texture,
	_: Vec<OwnedFd>,
) -> Result<wgpu::Texture, String> {
	Err(UNSUPPORTED.into())
}

#[cfg(all(target_os = "macos", not(feature = "cef")))]
pub(super) fn texture(
	_: &wgpu::Device,
	_: &wgpu::Queue,
	_: &Texture,
	_: &IOSurfaceRef,
) -> Result<wgpu::Texture, String> {
	Err(UNSUPPORTED.into())
}

#[cfg(not(feature = "cef"))]
const UNSUPPORTED: &str = "this build cannot import shared textures (enable the `cef` feature)";
