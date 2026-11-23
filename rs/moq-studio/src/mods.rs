//! Mods: the features most streamers never use, each off until switched on
//! in Settings, Mods. While one is off, no page, menu, hotkey list, palette,
//! or Settings pane shows it, its hotkeys and a phone's presses do nothing,
//! and the show does not use it. They are kept per user, beside the hotkeys,
//! since most are about how the operator works rather than the show.

use std::sync::atomic::{AtomicU16, Ordering};

/// A feature most streamers never use, built in but hidden until switched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mod {
	/// Audio scenes, their sends, and the MIX page.
	Audio,
	/// The REVIEW page, the replay buffer, clips, and markers.
	Review,
	/// Scene slots and layers keeping out of the way.
	Layout,
	/// Portrait shows, and going out in both formats at once.
	Formats,
	/// Cue a scene, then take it.
	Studio,
	/// The multiview window.
	Multiview,
	/// Phones paired as remotes.
	Remote,
	/// Scenes and sources published on their own, and scenes of their own size.
	Published,
	/// The scene graph's window.
	Graph,
}

impl Mod {
	/// Each mod, in the order Settings lists them.
	pub const ALL: [Self; 9] = [
		Self::Audio,
		Self::Review,
		Self::Studio,
		Self::Layout,
		Self::Formats,
		Self::Multiview,
		Self::Remote,
		Self::Published,
		Self::Graph,
	];

	pub fn name(self) -> &'static str {
		match self {
			Self::Audio => "Audio scenes & routes",
			Self::Review => "Replay & review",
			Self::Layout => "Smart layout",
			Self::Formats => "Both formats",
			Self::Studio => "Studio mode",
			Self::Multiview => "Multiview",
			Self::Remote => "Phone remote",
			Self::Published => "Published scenes",
			Self::Graph => "Scene graph",
		}
	}

	/// What it adds, in a line under its name.
	pub fn help(self) -> &'static str {
		match self {
			Self::Audio => {
				"Separate mixes for the stream, headphones, and the recording, on a MIX page with sends and routes."
			}
			Self::Review => "A REVIEW page: replay the last moments, save clips, and mark moments of the stream.",
			Self::Layout => "Slots that tile a scene's guests, and layers that keep out of the way of what matters.",
			Self::Formats => "A portrait show, or a portrait picture for phones beside the landscape one.",
			Self::Studio => "Cue a scene off air, then TAKE it or AUTO it live by the transition.",
			Self::Multiview => "A window of every scene and source, for a second monitor.",
			Self::Remote => "Phones paired by a QR code drive the scenes, TAKE, and the mutes.",
			Self::Published => {
				"Scenes and sources as broadcasts of their own, at their own size, with their own virtual camera."
			}
			Self::Graph => "Every source, scene, and audio scene, drawn as one map you can wire.",
		}
	}

	/// What else the palette finds it by.
	pub fn words(self) -> &'static [&'static str] {
		match self {
			Self::Audio => &["mix", "bus", "sends", "headphones", "monitor"],
			Self::Review => &["replay", "clip", "marker", "instant"],
			Self::Layout => &["slots", "keep out of the way", "dodge", "tile"],
			Self::Formats => &["portrait", "vertical", "dual", "phones"],
			Self::Studio => &["cue", "take", "preview", "next"],
			Self::Multiview => &["monitor", "tiles"],
			Self::Remote => &["phone", "pair", "qr"],
			Self::Published => &["publish", "broadcast", "scene size", "virtual camera"],
			Self::Graph => &["wires", "nodes", "map"],
		}
	}

	fn bit(self) -> u16 {
		1 << self as u16
	}
}

/// The mods switched on, which hotkeys read from outside the window's frames.
#[derive(Default)]
pub struct Mods(AtomicU16);

impl Mods {
	/// Whether `which` is on.
	pub fn on(&self, which: Mod) -> bool {
		self.0.load(Ordering::Relaxed) & which.bit() != 0
	}

	/// Switch `which` on or off.
	pub fn set(&self, which: Mod, on: bool) {
		match on {
			true => self.0.fetch_or(which.bit(), Ordering::Relaxed),
			false => self.0.fetch_and(!which.bit(), Ordering::Relaxed),
		};
	}

	/// Each mod on, as this user's settings keep them: by name, so a mod
	/// added or reordered leaves the others as they were.
	pub fn saved(&self) -> Vec<Mod> {
		Mod::ALL.into_iter().filter(|which| self.on(*which)).collect()
	}

	/// Switch on exactly the mods `saved` names.
	pub fn load(&self, saved: &[Mod]) {
		let bits = saved.iter().fold(0, |bits, which| bits | which.bit());
		self.0.store(bits, Ordering::Relaxed);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn each_mod_switches_alone() {
		let mods = Mods::default();
		for which in Mod::ALL {
			assert!(!mods.on(which), "{which:?} starts off");
		}
		mods.set(Mod::Review, true);
		mods.set(Mod::Graph, true);
		mods.set(Mod::Review, false);
		let on: Vec<Mod> = Mod::ALL.into_iter().filter(|which| mods.on(*which)).collect();
		assert_eq!(on, [Mod::Graph]);
	}

	#[test]
	fn load_replaces() {
		let mods = Mods::default();
		mods.set(Mod::Studio, true);
		mods.load(&[Mod::Remote, Mod::Audio]);
		assert_eq!(
			mods.saved(),
			[Mod::Audio, Mod::Remote],
			"only those, in the pane's order"
		);
	}
}
