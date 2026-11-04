//! Copy a page's shared texture onto the studio's device.

use std::os::fd::OwnedFd;

use super::Helper;
use super::protocol::Texture;
use crate::wgpu;

/// Whether the studio's device can import the helper's shared textures, so
/// the helper should paint into them.
pub(super) fn supported(helper: &Helper) -> bool {
	cfg!(feature = "cef")
		&& helper
			.device
			.features()
			.contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
}

/// Copy the DMA-BUF `texture` describes, whose plane descriptors are `fds`,
/// into a texture of the studio's own on `device`, waiting until the copy is
/// done.
#[cfg(feature = "cef")]
pub(super) fn texture(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	texture: &Texture,
	fds: Vec<OwnedFd>,
) -> Result<wgpu::Texture, String> {
	use std::os::fd::AsRawFd;

	use cef::osr_texture_import::SharedTextureHandle;

	use super::protocol::{Format, MAX_PLANES};

	if fds.len() != texture.planes.len() || fds.is_empty() || fds.len() > MAX_PLANES {
		return Err(format!(
			"a shared texture came with {} planes and {} descriptors",
			texture.planes.len(),
			fds.len()
		));
	}
	let mut info = cef::AcceleratedPaintInfo::default();
	for ((slot, plane), fd) in info.planes.iter_mut().zip(&texture.planes).zip(&fds) {
		slot.stride = plane.stride;
		slot.offset = plane.offset;
		slot.size = plane.size;
		slot.fd = fd.as_raw_fd();
	}
	info.plane_count = fds.len() as _;
	info.modifier = texture.modifier;
	info.format = match texture.format {
		Format::Rgba => cef::ColorType::RGBA_8888,
		Format::Bgra => cef::ColorType::BGRA_8888,
	};
	info.extra.coded_size.width = texture.width as _;
	info.extra.coded_size.height = texture.height as _;

	// SAFETY: the helper keeps the texture out of CEF's hands until it hears
	// back, and `fds` stay open until this returns. The device is the
	// studio's, opened by `moq_video::render::request_device` with DMA-BUF
	// import, on the GPU CEF paints with.
	unsafe { SharedTextureHandle::new(&info).import_texture(device, queue) }
		.map_err(|err| format!("cannot import the page's texture: {err}"))
}

#[cfg(not(feature = "cef"))]
pub(super) fn texture(
	_: &wgpu::Device,
	_: &wgpu::Queue,
	_: &Texture,
	_: Vec<OwnedFd>,
) -> Result<wgpu::Texture, String> {
	Err("this build cannot import shared textures (enable the `cef` feature)".into())
}
