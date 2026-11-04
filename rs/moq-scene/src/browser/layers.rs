//! What a page shows: its view and, while one is open, the popup CEF paints
//! apart from it, such as a `<select>`'s list, drawn over it.

use super::protocol::{Area, Layer};
use crate::wgpu;

/// A page's newest pictures.
#[derive(Default)]
pub(super) struct Layers {
	view: Option<wgpu::Texture>,
	/// The open popup's area of the view, and its picture once painted.
	popup: Option<(Area, Option<wgpu::Texture>)>,
}

impl Layers {
	/// Take `texture` as the newest picture of `layer`. A popup painted while
	/// none is open is dropped.
	pub fn paint(&mut self, layer: Layer, texture: wgpu::Texture) {
		match layer {
			Layer::View => self.view = Some(texture),
			Layer::Popup => {
				if let Some((_, picture)) = &mut self.popup {
					*picture = Some(texture);
				}
			}
		}
	}

	/// The popup opened or moved to `area`, or closed with `None`. One that
	/// moved keeps its picture until it paints again.
	pub fn place(&mut self, area: Option<Area>) {
		let picture = self.popup.take().and_then(|(_, picture)| picture);
		self.popup = area.map(|area| (area, picture));
	}

	/// What the page shows, once its view has painted: the view itself, or a
	/// copy of it with the popup over it, so the view stays clean for when the
	/// popup changes or closes.
	pub fn picture(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Option<wgpu::Texture>, String> {
		let Some(view) = &self.view else { return Ok(None) };
		let Some((area, Some(popup))) = &self.popup else {
			return Ok(Some(view.clone()));
		};
		let Some((from, to, size)) = clip(*area, popup.size(), view.size()) else {
			return Ok(Some(view.clone()));
		};
		if popup.format() != view.format() {
			return Err(format!(
				"the page painted its popup as {:?} over a {:?} page",
				popup.format(),
				view.format()
			));
		}
		let texture = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("moq-scene page with popup"),
			size: view.size(),
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: view.format(),
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let mut encoder = device.create_command_encoder(&Default::default());
		encoder.copy_texture_to_texture(view.as_image_copy(), texture.as_image_copy(), view.size());
		encoder.copy_texture_to_texture(
			wgpu::TexelCopyTextureInfo {
				origin: from,
				..popup.as_image_copy()
			},
			wgpu::TexelCopyTextureInfo {
				origin: to,
				..texture.as_image_copy()
			},
			size,
		);
		queue.submit([encoder.finish()]);
		Ok(Some(texture))
	}
}

/// Where a `popup`-sized picture placed at `area` lands on a `view`-sized
/// one: the corner copied from, the corner copied to, and how much, or `None`
/// if none of it does. The area comes from the page, so it may reach past the
/// view, and the picture may not match it.
fn clip(
	area: Area,
	popup: wgpu::Extent3d,
	view: wgpu::Extent3d,
) -> Option<(wgpu::Origin3d, wgpu::Origin3d, wgpu::Extent3d)> {
	let (x, y) = (i64::from(area.x), i64::from(area.y));
	let (left, top) = (x.max(0), y.max(0));
	let right = (x + i64::from(area.width.min(popup.width))).min(view.width.into());
	let bottom = (y + i64::from(area.height.min(popup.height))).min(view.height.into());
	if left >= right || top >= bottom {
		return None;
	}
	let origin = |x: i64, y: i64| wgpu::Origin3d {
		x: x as u32,
		y: y as u32,
		z: 0,
	};
	let size = wgpu::Extent3d {
		width: (right - left) as u32,
		height: (bottom - top) as u32,
		depth_or_array_layers: 1,
	};
	Some((origin(left - x, top - y), origin(left, top), size))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn extent(width: u32, height: u32) -> wgpu::Extent3d {
		wgpu::Extent3d {
			width,
			height,
			depth_or_array_layers: 1,
		}
	}

	fn area(x: i32, y: i32, width: u32, height: u32) -> Area {
		Area { x, y, width, height }
	}

	/// A copy as `((from x, from y), (to x, to y), (width, height))`.
	type Copied = ((u32, u32), (u32, u32), (u32, u32));

	fn clipped(area: Area, popup: (u32, u32), view: (u32, u32)) -> Option<Copied> {
		clip(area, extent(popup.0, popup.1), extent(view.0, view.1))
			.map(|(from, to, size)| ((from.x, from.y), (to.x, to.y), (size.width, size.height)))
	}

	/// A popup reaching past any edge of the view, or a picture that does not
	/// match its area, copies only what lands on the view, from the matching
	/// part of the picture, and one entirely off the view copies nothing.
	#[test]
	fn a_popup_copies_only_what_lands_on_the_view() {
		let view = (320, 240);
		assert_eq!(
			clipped(area(10, 20, 100, 50), (100, 50), view),
			Some(((0, 0), (10, 20), (100, 50)))
		);
		assert_eq!(
			clipped(area(300, 200, 100, 100), (100, 100), view),
			Some(((0, 0), (300, 200), (20, 40)))
		);
		assert_eq!(
			clipped(area(-30, -40, 100, 100), (100, 100), view),
			Some(((30, 40), (0, 0), (70, 60)))
		);
		assert_eq!(
			clipped(area(i32::MIN, i32::MIN, u32::MAX, u32::MAX), (5000, 5000), view),
			None,
			"far off the view, however large"
		);
		assert_eq!(
			clipped(area(-10, 0, u32::MAX, u32::MAX), (4096, 4096), view),
			Some(((10, 0), (0, 0), (320, 240)))
		);
		assert_eq!(
			clipped(area(10, 10, 100, 100), (40, 300), view),
			Some(((0, 0), (10, 10), (40, 100))),
			"the smaller of the picture and its area"
		);
		assert_eq!(clipped(area(320, 0, 10, 10), (10, 10), view), None);
		assert_eq!(clipped(area(-10, 0, 10, 10), (10, 10), view), None);
		assert_eq!(clipped(area(0, 0, 0, 10), (10, 10), view), None);
	}
}
