//! The icons, named by what they do: Lucide's, in one stroke weight, drawn
//! from their SVGs (`assets/icons`, ISC) at whatever size and scale they are
//! shown. Add one with `sh/kit-assets.sh <lucide name>` and a variant here.

use egui::{Color32, ImageSource, include_image};

/// An icon, by its role in the studio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
	Undo,
	Redo,
	Settings,
	Mic,
	Sound,
	Muted,
	Listen,
	Clip,
	Shown,
	Hidden,
	Panic,
	StudioMode,
	Add,
	Close,
	Rename,
	Up,
	Down,
	Remove,
	Copy,
	Hotkeys,
	Play,
	Stop,
	Reload,
	#[cfg_attr(not(test), expect(dead_code, reason = "a guest's invite: quest/m0/ux/guests.md"))]
	Link,
	#[cfg_attr(
		not(test),
		expect(dead_code, reason = "the selection's toolbar: quest/m0/ux/build.md")
	)]
	More,
	MicMuted,
	Minimize,
	Maximize,
	Live,
	Build,
	Mix,
	Review,
}

impl Icon {
	/// Every icon, for the gallery.
	#[cfg(test)]
	pub const ALL: [Self; 32] = [
		Self::Undo,
		Self::Redo,
		Self::Settings,
		Self::Mic,
		Self::Sound,
		Self::Muted,
		Self::Listen,
		Self::Clip,
		Self::Shown,
		Self::Hidden,
		Self::Panic,
		Self::StudioMode,
		Self::Add,
		Self::Close,
		Self::Rename,
		Self::Up,
		Self::Down,
		Self::Remove,
		Self::Copy,
		Self::Hotkeys,
		Self::Play,
		Self::Stop,
		Self::Reload,
		Self::Link,
		Self::More,
		Self::MicMuted,
		Self::Minimize,
		Self::Maximize,
		Self::Live,
		Self::Build,
		Self::Mix,
		Self::Review,
	];

	fn source(self) -> ImageSource<'static> {
		match self {
			Self::Undo => include_image!("../../assets/icons/undo-2.svg"),
			Self::Redo => include_image!("../../assets/icons/redo-2.svg"),
			Self::Settings => include_image!("../../assets/icons/settings.svg"),
			Self::Mic => include_image!("../../assets/icons/mic.svg"),
			Self::Sound => include_image!("../../assets/icons/volume-2.svg"),
			Self::Muted => include_image!("../../assets/icons/volume-x.svg"),
			Self::Listen => include_image!("../../assets/icons/headphones.svg"),
			Self::Clip => include_image!("../../assets/icons/scissors.svg"),
			Self::Shown => include_image!("../../assets/icons/eye.svg"),
			Self::Hidden => include_image!("../../assets/icons/eye-off.svg"),
			Self::Panic => include_image!("../../assets/icons/shield.svg"),
			Self::StudioMode => include_image!("../../assets/icons/columns-2.svg"),
			Self::Add => include_image!("../../assets/icons/plus.svg"),
			Self::Close => include_image!("../../assets/icons/x.svg"),
			Self::Rename => include_image!("../../assets/icons/pencil.svg"),
			Self::Up => include_image!("../../assets/icons/arrow-up.svg"),
			Self::Down => include_image!("../../assets/icons/arrow-down.svg"),
			Self::Remove => include_image!("../../assets/icons/trash-2.svg"),
			Self::Copy => include_image!("../../assets/icons/copy.svg"),
			Self::Hotkeys => include_image!("../../assets/icons/keyboard.svg"),
			Self::Play => include_image!("../../assets/icons/play.svg"),
			Self::Stop => include_image!("../../assets/icons/square.svg"),
			Self::Reload => include_image!("../../assets/icons/rotate-ccw.svg"),
			Self::Link => include_image!("../../assets/icons/link.svg"),
			Self::More => include_image!("../../assets/icons/chevron-down.svg"),
			Self::MicMuted => include_image!("../../assets/icons/mic-off.svg"),
			Self::Minimize => include_image!("../../assets/icons/minus.svg"),
			Self::Maximize => include_image!("../../assets/icons/square.svg"),
			Self::Live => include_image!("../../assets/icons/video.svg"),
			Self::Build => include_image!("../../assets/icons/layers.svg"),
			Self::Mix => include_image!("../../assets/icons/sliders-vertical.svg"),
			Self::Review => include_image!("../../assets/icons/film.svg"),
		}
	}

	/// The icon `size` points square, in `color`.
	pub fn image(self, size: f32, color: Color32) -> egui::Image<'static> {
		egui::Image::new(self.source())
			.fit_to_exact_size(egui::Vec2::splat(size))
			.tint(color)
	}

	/// Draw the icon `size` points square, centered in `rect`, in `color`.
	pub fn paint(self, ui: &egui::Ui, rect: egui::Rect, size: f32, color: Color32) {
		let at = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(size));
		self.image(size, color).paint_at(ui, at);
	}
}
