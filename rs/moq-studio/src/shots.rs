//! Named views of the window, rendered to PNG with no display: `just shots`.
//!
//! Each view is a test that opens the show in `shots/show.toml` on a fresh
//! engine and draws one state of the window through egui_kittest, on the GPU
//! the compositor uses. A still view draws the same picture every time, which
//! its test checks by drawing it twice; a live one shows something that moves,
//! such as an encoder's timings. One more test drives the window the same way,
//! clicking as a person does, and draws nothing.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use eframe::egui_wgpu;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
use moq_scene::Source;
use moq_scene::source::Status;

use crate::mods::Mod;
use crate::ui::{AddTab, Inspect, MixView, Page, Setting, Side};
use crate::{App, Window, autosave, devices, engine, fx, hotkeys, recent, show, stream};

/// The window's size, the mock-ups'.
const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);

/// A window snapped to half of a 1440x900 screen, as the compact mock-up's.
const HALF: egui::Vec2 = egui::vec2(711.0, 872.0);

/// The longest a view waits for its sources and pictures to settle.
const SETTLE: Duration = Duration::from_secs(20);

/// What a view opens.
enum Open {
	/// The studio on the show in `shots/<show>`, in a window `size` large,
	/// turned to `view`, with a relay of its own to go live on if `relay`.
	Studio {
		view: View,
		size: egui::Vec2,
		show: &'static str,
		relay: bool,
	},
	/// The kit's gallery, with a sheet open over it or not.
	Kit { sheet: bool },
}

/// Where a view turns the studio.
enum View {
	Page(Page),
	/// Studio on a new show, as the first launch opens it: one empty scene,
	/// with a camera and a screen to add.
	FirstLaunch,
	/// The first launch after a file no source shows was picked: its refusal
	/// under the cards.
	Refused,
	/// Studio with its picture locked and the layers' rail closed.
	Locked,
	/// Studio with the options of the sound called this open from its gear.
	SoundOptions(&'static str),
	/// The MIX page in a view, with the strip of the sound named open.
	Mix(MixView, Option<&'static str>),
	/// MIX with Headphones' menu open, as a right click on its strip opens it.
	MixMenu,
	/// The Studio page with `side` and `inspect` shown, and the layer of the
	/// scene on program at `index`, front first, selected.
	Build {
		side: Side,
		inspect: Inspect,
		index: Option<usize>,
	},
	/// Add open on a tab over Studio, listing a few devices.
	Add(AddTab),
	/// Studio with the host's layer selected and its row's menu open.
	Menu,
	/// Studio with the host's layer selected and the inspector's More open.
	More,
	/// Studio with the host's layer selected, its inspector scrolled to how
	/// viewers may treat it, published independently.
	Published,
	/// The edit history, under the title bar's undo, after an edit.
	History,
	/// Studio with the pointer on a scene's pad, which shows its ⋯ button.
	PadHover,
	/// Studio asking whether to remove a scene holding two sources, from its
	/// pad's ⋯ menu.
	RemovePrompt,
	/// The show's menu, open from the title bar, with a few other shows.
	ShowMenu,
	/// On air, the show's menu with New show dimmed, saying why under the
	/// pointer.
	ShowMenuLive,
	/// Settings, at a pane.
	Setting(Setting),
	/// On air, Settings at a pane.
	OnAirSetting(Setting),
	/// On air, Settings' Show pane asking before 720p restarts the video.
	Restart,
	/// Settings' Show pane with Custom's editor open.
	CustomCanvas,
	/// Settings' Guests pane, its room unable to join at launch, as its key
	/// file is missing.
	GuestsFailed,
	/// Settings' Remote pane in a room, one phone paired and removed, and
	/// another just paired by its name and Pair a phone.
	Paired,
	/// The scene graph's window.
	Graph,
	/// The stream's health in detail.
	Health,
	/// Studio after a mute, with the health dot's popover open and the
	/// pointer on undo, which names the mute.
	Hover,
	/// GO LIVE pressed: on a show with a relay, its popover waiting on the
	/// check, started over; on one without, asking for a relay.
	Going,
	/// On air, at the Studio page.
	OnAir,
	/// On air, the clock clicked, asking before it ends the stream.
	EndConfirm,
	/// On air, with the health dot's popover open.
	OnAirHealth,
	/// REVIEW, with this show's recordings and clips, and if `picked`, the
	/// last 30 seconds held and a stretch picked. With `replays`, the show
	/// has two replay sources to pick between, and with `markers`, three
	/// markers were dropped, two still on the strip.
	Review {
		picked: bool,
		replays: bool,
		markers: bool,
	},
	/// Studio in Studio mode, with `Interview` cued.
	Cued,
	/// A scene, by name, open for editing off air.
	OffAir(&'static str),
	/// Studio with the front layer selected, once it kept out of the way:
	/// moved off what mattered and landed. If `dual`, with the show going
	/// out in both formats.
	KeepClear {
		dual: bool,
	},
	/// Studio with the game's window gone dark selected, and what to do about
	/// it open from its badge.
	Dark,
	/// Studio with the host's camera stalled selected, and what to do about it
	/// open from its badge.
	Stalled,
	/// Studio with no layer selected, after the guest `Ibis` appears on their
	/// own in the scene on program.
	Joined,
	/// Add's Guests tab in a room with `Robin`, in a scene and heard, and
	/// `Sam`, who is neither and shares a screen too, and the link just made
	/// for `Alex`, who has not joined.
	AddGuests,
	/// The multiview, as its own window draws it, with the host and the
	/// highlight picked beside the guests. If `live`, on air in Studio mode
	/// with `Interview` cued.
	Multiview {
		live: bool,
		/// Whether the host's picture stalled.
		stalled: bool,
	},
	/// Studio on the drop views' show, with a file dragged or dropped on the
	/// picture.
	Drop(Drop),
	/// On the dual-format show, the layer of the scene on program at
	/// `index`, front first, selected, Studio editing `format`'s picture, in
	/// Studio mode with the scene `cued` cued, if any, and the scene `off_air`
	/// open off air, if any.
	Dual {
		format: moq_scene::Format,
		index: Option<usize>,
		cued: Option<&'static str>,
		off_air: Option<&'static str>,
	},
	/// The portrait picture of Break, off air, with a picture just added.
	DualAdded,
	/// The command palette, open by its key over Studio with a few keys
	/// bound, with what is typed into it.
	Palette(&'static str),
	/// Settings, Mods, each switched on, as a power user has it.
	ModsOn,
	/// Settings, Mods, scrolled to the row of this name.
	ModsAt(&'static str),
}

/// The studio on the dual-format views' show, at the mock-ups' size, with
/// the camera's layer selected, editing `format`'s picture.
fn dual(format: moq_scene::Format) -> Open {
	Open::Studio {
		view: View::Dual {
			format,
			index: Some(2),
			cued: None,
			off_air: None,
		},
		size: SIZE,
		show: "dual.toml",
		relay: false,
	}
}

impl View {
	/// The mods a view switches on, beyond those its show uses.
	fn mods(&self) -> Vec<Mod> {
		match self {
			Self::Mix(..) | Self::MixMenu => vec![Mod::Audio],
			Self::Review { .. } => vec![Mod::Review],
			Self::Cued | Self::Dual { cued: Some(_), .. } => vec![Mod::Studio],
			Self::Multiview { live: false, .. } => vec![Mod::Multiview],
			Self::Multiview { live: true, .. } => vec![Mod::Multiview, Mod::Studio],
			Self::Setting(Setting::Remote) | Self::Paired => vec![Mod::Remote],
			Self::Graph => vec![Mod::Graph],
			Self::ModsOn => Mod::ALL.to_vec(),
			_ => Vec::new(),
		}
	}
}

/// A file over the picture, at the pointer.
#[derive(Clone, Copy)]
enum Drop {
	/// A logo dragged over the picture, away from every layer.
	Hover,
	/// A picture dropped on the host's picture, which asks whether to replace
	/// its file.
	Replace,
	/// A regular MP4 dropped, which says why it cannot play.
	Refused,
	/// A logo from outside the show's directory dropped away from every
	/// layer, copied beside the show, and placed there, selected.
	Placed,
}

/// The studio on the drop views' show, turned to Studio with `drop`.
fn dropping(drop: Drop) -> Open {
	Open::Studio {
		view: View::Drop(drop),
		size: SIZE,
		show: "drop.toml",
		relay: false,
	}
}

/// Studio with the host's layer, the middle of the scene on program's three,
/// selected, as the mock-up's.
const BUILD: View = View::Build {
	side: Side::Layers,
	inspect: Inspect::Layer,
	index: Some(1),
};

/// The studio on the shots' show, at the mock-ups' size, turned to `view`.
fn on_show(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "show.toml",
		relay: false,
	}
}

/// [`on_show`], with a relay to go live on.
fn relayed(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "show.toml",
		relay: true,
	}
}

/// The studio on the slot views' show, at the mock-ups' size, turned to
/// `view`.
fn slotted(view: View) -> Open {
	Open::Studio {
		view,
		size: SIZE,
		show: "slots.toml",
		relay: false,
	}
}

/// The studio on the MIX views' show, `size` large, turned to MIX in `view`
/// with the strip of the sound called `opened` open.
fn mixing(view: MixView, opened: Option<&'static str>, size: egui::Vec2) -> Open {
	Open::Studio {
		view: View::Mix(view, opened),
		size,
		show: "mix.toml",
		relay: false,
	}
}

/// Whether a view draws the same picture every time.
#[derive(PartialEq)]
enum Motion {
	Still,
	Live,
}

/// A test per view, named for `just shots <name>`.
macro_rules! views {
	($($(#[doc = $doc:literal])* $name:ident: $motion:ident, $open:expr;)*) => {$(
		$(#[doc = $doc])*
		#[test]
		#[ignore = "draws on a GPU; run with `just shots`"]
		fn $name() {
			shot(stringify!($name), Motion::$motion, $open);
		}
	)*};
}

// Every view. A quest that adds a page or a state adds its row, and an
// `Open` variant for a state no existing one reaches.
views! {
	/// Every widget of the kit in every state.
	kit: Still, Open::Kit { sheet: false };
	/// The kit's sheet, over the gallery.
	kit_sheet: Still, Open::Kit { sheet: true };
	/// A scene's pad with the pointer on it: its ⋯ button, which opens the menu
	/// a right click does.
	pad_hover: Still, on_show(View::PadHover);
	/// Removing a scene holding more than one source asks first.
	remove_prompt: Still, on_show(View::RemovePrompt);
	/// A new show, as the first launch opens it: the quick start on the empty
	/// picture, its cards naming the camera and screen they add.
	first_launch: Still, on_show(View::FirstLaunch);
	/// The first launch after the Media card's dialog picked a PDF: why it
	/// cannot show, under the cards.
	first_launch_refused: Still, on_show(View::Refused);
	/// The Studio page: the picture, editable, with the layers' rail beside
	/// it, and the slim strip of the pads, the panic key, and each sound's
	/// volume, meter, mute, and gear.
	studio: Still, on_show(View::Page(Page::Studio));
	/// Studio locked against stray edits with the layers' rail closed: the
	/// picture as large as the window allows.
	studio_locked: Still, on_show(View::Locked);
	/// A sound's options from its gear in the strip: monitoring it alone in
	/// headphones, its delay, and its pan.
	audio_popover: Still, on_show(View::SoundOptions("guest"));
	/// What the title bar's controls show under the pointer.
	studio_hover: Still, on_show(View::Hover);
	/// A portrait show: the scenes and the sound in columns beside the
	/// picture.
	studio_portrait: Still, Open::Studio { view: View::Page(Page::Studio), size: SIZE, show: "portrait.toml", relay: false };
	/// A window snapped beside a game stacks the picture, the scenes, and the
	/// sound.
	studio_compact: Still, Open::Studio { view: View::Page(Page::Studio), size: HALF, show: "show.toml", relay: false };
	/// Studio with the host's layer selected, as the `overhaul-edit` mock-up
	/// frame has it: its row in Layers, its handles and the toolbar of what
	/// acts on the picture, and the inspector's essentials with More closed.
	edit: Still, on_show(BUILD);
	/// The same with the inspector's More open: its place, crop, blend,
	/// filters, and its source's own.
	edit_more: Still, on_show(View::More);
	/// The selected layer's keyframes.
	build_animate: Still, on_show(View::Build { side: Side::Layers, inspect: Inspect::Animate, index: Some(1) });
	/// Add over the stage, as `A` opens it: the sources no layer shows, then
	/// this machine's cameras, screens, and windows, each a card adding it in
	/// one click.
	add: Still, on_show(View::Add(AddTab::Devices));
	/// Add's files, web page, text, color, and the show's own scenes.
	add_media: Still, on_show(View::Add(AddTab::Media));
	/// Add's microphones, each with its level.
	add_audio: Still, on_show(View::Add(AddTab::Audio));
	/// Add's guests with no room to join.
	add_guests_room: Still, on_show(View::Add(AddTab::Guests));
	/// Add's guests: the link just made, and who joined. Not still, as the
	/// code encodes a token signed afresh each run.
	add_guests: Live, on_show(View::AddGuests);
	/// A layer row's menu, as a right click opens it.
	build_menu: Still, on_show(View::Menu);
	/// A source published independently selected: under its switch in the
	/// inspector, whether viewers may switch it off and its priority for them.
	build_published: Still, Open::Studio { view: View::Published, size: SIZE, show: "published.toml", relay: false };
	/// Studio in a window snapped beside a game: the inspector is a tab beside
	/// Layers and Add.
	build_compact: Still, Open::Studio { view: BUILD, size: HALF, show: "show.toml", relay: false };
	/// A scene laid out by slots: each slot outlined with the role or guest
	/// in it, and the inspector's arrangements and slots, the main picture
	/// and camera filled by roles.
	build_slots: Still, slotted(View::Build { side: Side::Layers, inspect: Inspect::Layer, index: None });
	/// The same scene once a second guest appears on their own: the column
	/// shrinks to take them and the main picture grows, as the `autolayout`
	/// mock-up frame has it.
	build_slots_joined: Still, slotted(View::Joined);
	/// The camera's layer selected in its slot: no handles to resize it, and
	/// the source filling its role in the inspector.
	build_slots_layer: Still, slotted(View::Build { side: Side::Layers, inspect: Inspect::Layer, index: Some(2) });
	/// Studio mode: the cued scene's pad turns amber, and NEXT docks on the
	/// picture, edited on air, with TAKE and AUTO.
	build_studio: Still, on_show(View::Cued);
	/// A scene open off air: the frame turns amber, and the live picture
	/// shrinks to a corner beside the banner that takes it live or goes back.
	/// Layers lists the scene's own alone.
	build_off_air: Still, on_show(View::OffAir("Break"));
	/// A scene that cannot go on air opens off air too, its Take it live
	/// dimmed, saying why under the pointer.
	build_off_air_refused: Still, Open::Studio { view: View::OffAir("Vertical"), size: SIZE, show: "published.toml", relay: false };
	/// Too narrow for the banner beside the live picture, it goes under it.
	build_off_air_compact: Still, Open::Studio { view: View::OffAir("Break"), size: HALF, show: "show.toml", relay: false };
	/// A facecam keeping out of the way of a game: what matters glowing on the
	/// picture, the places it may go, where it was and the glide to where it
	/// went, the legend under the picture, and its settings in the
	/// inspector's More.
	build_keep_clear: Live, Open::Studio { view: View::KeepClear { dual: false }, size: SIZE, show: "keep.toml", relay: false };
	/// The same in the smallest window: the legend's labels shortened to fit
	/// beside the note, or the note left out.
	build_keep_clear_compact: Live, Open::Studio { view: View::KeepClear { dual: false }, size: crate::ui::MIN_SIZE, show: "keep.toml", relay: false };
	/// The same going out in both formats: the legend between the landscape
	/// picture, the only one it moves in, and the tabs, the portrait picture
	/// beside it showing the facecam where its rect rests, fitted.
	build_keep_clear_dual: Live, Open::Studio { view: View::KeepClear { dual: true }, size: SIZE, show: "keep.toml", relay: false };
	/// A window gone dark: its row, the layer on the picture, its scene's
	/// pad, and the health dot turn amber, and its badge opens what happened
	/// and the fixes.
	build_dark: Live, Open::Studio { view: View::Dark, size: SIZE, show: "dark.toml", relay: false };
	/// A camera that stopped sending pictures: the same amber badge, saying
	/// its picture froze, and offering to open it again.
	build_stalled: Live, Open::Studio { view: View::Stalled, size: SIZE, show: "stalled.toml", relay: false };
	/// A logo dragged over the picture: where it lands, outlined and named
	/// for the kind of layer it becomes, and Add's row for that kind lit.
	build_drop: Still, dropping(Drop::Hover);
	/// A picture dropped on an image layer asks whether to replace that
	/// layer's file or add a layer.
	build_drop_replace: Still, dropping(Drop::Replace);
	/// A regular MP4 dropped says, where it was dropped, why it cannot play
	/// and the remux that fixes it.
	build_drop_refused: Still, dropping(Drop::Refused);
	/// The logo dropped lands where it was let go, at its own size, selected.
	build_dropped: Still, dropping(Drop::Placed);
	/// A show going out in both formats: the landscape picture edited, the
	/// portrait one small beside it, and under them the tabs that pick which
	/// and where the selected camera is shown, which the inspector says too,
	/// and each pad showing the pair, as the `dual` mock-up frame has it.
	build_dual: Still, dual(moq_scene::Format::Landscape);
	/// It in a half-size window: the chip keeps clear of the tabs, or leaves
	/// the choice to the inspector where there is no room.
	build_dual_compact: Still, Open::Studio { view: View::Dual { format: moq_scene::Format::Landscape, index: Some(2), cued: None, off_air: None }, size: HALF, show: "dual.toml", relay: false };
	/// Studio mode on it: NEXT shows the cued scene in both formats.
	build_dual_studio: Still, Open::Studio { view: View::Dual { format: moq_scene::Format::Landscape, index: None, cued: Some("Guests"), off_air: None }, size: SIZE, show: "dual.toml", relay: false };
	/// Editing the portrait picture: the camera's handles at its portrait
	/// place, the landscape picture small beside it.
	build_dual_portrait: Still, dual(moq_scene::Format::Portrait);
	/// A scene laid out by slots on it, open off air in portrait: its own
	/// arrangement there, a row, which the inspector picks.
	build_dual_slots: Still, Open::Studio { view: View::Dual { format: moq_scene::Format::Portrait, index: None, cued: None, off_air: Some("Guests") }, size: SIZE, show: "dual.toml", relay: false };
	/// Editing the portrait picture of a scene off air, a 16:9 picture just
	/// added from Add: a picture-in-picture in each, cropped evenly to the
	/// portrait inset's shape rather than squeezed into it.
	build_dual_added: Still, Open::Studio { view: View::DualAdded, size: SIZE, show: "dual.toml", relay: false };
	/// Settings, Show on it: Both at once on.
	settings_show_dual: Still, Open::Studio { view: View::Setting(Setting::Show), size: SIZE, show: "dual.toml", relay: false };
	/// GO LIVE's popover on it: both broadcasts to each relay, and the
	/// encoder and connection checked for both pictures.
	golive_dual: Live, Open::Studio { view: View::Going, size: SIZE, show: "dual.toml", relay: true };
	/// On air on it: the landscape picture edited in its ON AIR frame, the
	/// portrait one small beside it.
	on_air_dual: Live, Open::Studio { view: View::OnAir, size: SIZE, show: "dual.toml", relay: true };
	/// MIX: a strip per sound and per audio scene, with their sends, and a
	/// small live picture in the sound's place.
	mix: Still, mixing(MixView::Strips, None, SIZE);
	/// A sound's strip opened to its gain, pan, delay, and where it is heard,
	/// its neighbours narrowed.
	mix_strip: Still, mixing(MixView::Strips, Some("mic"), SIZE);
	/// An audio scene's menu: rename, move, or remove it.
	mix_menu: Still, Open::Studio { view: View::MixMenu, size: SIZE, show: "mix.toml", relay: false };
	/// The same sends as a matrix.
	mix_routes: Still, mixing(MixView::Routes, None, SIZE);
	/// MIX in a stacked window, its strips scrolling, its live picture as
	/// tall as the sound's slot allows.
	mix_compact: Still, mixing(MixView::Strips, None, HALF);
	/// REVIEW off air: the program keeps no past until it goes live, which the
	/// strip and both actions say, beside this show's recordings and clips.
	review: Still, on_show(View::Review { picked: false, replays: false, markers: false });
	/// REVIEW with the last 30 seconds held: a stretch picked between the
	/// handles, the head inside it, the picture under the head large, and
	/// Replay saying why it cannot play without a replay source.
	review_picked: Still, on_show(View::Review { picked: true, replays: false, markers: false });
	/// REVIEW on a show with two replay sources: which one Replay plays in is
	/// picked under the speed, in place of its note.
	review_into: Still, on_show(View::Review { picked: true, replays: true, markers: false });
	/// REVIEW with markers: each still in the last 30 seconds a numbered
	/// pennant on the strip, and the list over the recordings, the newest
	/// first, one named, and the oldest too old to clip around.
	review_markers: Still, on_show(View::Review { picked: true, replays: false, markers: true });
	/// The edit history, open under undo.
	history: Still, on_show(View::History);
	/// The health dot's details off air: the encoders and their timings.
	health: Live, on_show(View::Health);
	/// Settings, Show: the format, the canvas size and frame rate by name,
	/// and the scene sizes behind their disclosure.
	settings_show: Still, on_show(View::Setting(Setting::Show));
	/// Settings, Show in the smallest window: each row's control under its
	/// name, and the categories as icons.
	settings_compact: Still, Open::Studio { view: View::Setting(Setting::Show), size: crate::ui::MIN_SIZE, show: "show.toml", relay: false };
	/// Settings, Show on air, saying what a new size does then.
	settings_show_on_air: Live, relayed(View::OnAirSetting(Setting::Show));
	/// On air, a new canvas size waits for a yes before it restarts the video.
	settings_restart: Live, relayed(View::Restart);
	/// Custom's editor: a width, height, and frame rate of the host's own.
	settings_custom: Still, on_show(View::CustomCanvas);
	/// Settings, Recording: the folder, and what a recording holds.
	settings_recording: Still, on_show(View::Setting(Setting::Recording));
	/// Settings, Hotkeys: each action by what it is for, on keycaps.
	settings_hotkeys: Still, on_show(View::Setting(Setting::Hotkeys));
	/// Settings, Guests, with no room yet.
	settings_guests: Still, on_show(View::Setting(Setting::Guests));
	/// Settings, Guests, when the room saved cannot be joined, such as when
	/// its key file is gone: why, and Try again.
	settings_guests_failed: Live, relayed(View::GuestsFailed);
	/// Settings, Remote with no room yet, which a remote joins.
	settings_remote: Still, on_show(View::Setting(Setting::Remote));
	/// Settings, Remote with a phone just paired: the code its camera scans,
	/// when its link expires, and every phone paired, one removed. Live, as
	/// each link carries a token of its own.
	settings_remote_paired: Live, on_show(View::Paired);
	/// Settings, Mods: each feature most streamers never use, off.
	settings_mods: Still, on_show(View::Setting(Setting::Mods));
	/// Settings, Mods with each switched on: Published scenes keeps its
	/// scenes and sources here, and Scene graph opens.
	settings_mods_on: Still, on_show(View::ModsOn);
	/// Settings, Mods on a show with a source published, which switched
	/// Published scenes on as it opened: how viewers may treat it, as a
	/// sound alone, with no layer to inspect, sets it.
	settings_mods_published: Still, Open::Studio { view: View::ModsAt("Priority"), size: SIZE, show: "published.toml", relay: false };
	/// The scene graph, in a window of its own.
	graph: Still, on_show(View::Graph);
	/// The command palette as its key opens it: every action that can run
	/// now, each with its key, before the pages, settings, and what can be
	/// added.
	palette: Still, on_show(View::Palette(""));
	/// The palette finding what `take` names: the scenes, as TAKE is Studio
	/// mode's, off.
	palette_found: Still, on_show(View::Palette("take"));
	/// The palette finding a setting by its name.
	palette_setting: Still, on_show(View::Palette("frame"));
	/// The show's menu: its file, the other recent shows to switch to, New
	/// show, Open show file, Rename, Export a copy, Reload, Copy, and Paste.
	show_menu: Still, on_show(View::ShowMenu);
	/// On air, switching shows is dimmed, and says to end the stream first.
	show_menu_live: Live, relayed(View::ShowMenuLive);
	/// GO LIVE on a show with no relay: its popover asks for one, and its
	/// token, to go live now.
	golive_inline: Still, on_show(View::Going);
	/// GO LIVE's popover while the check runs: the relay and its state, the
	/// encoder and the connection as they come in, and going live once they
	/// pass.
	golive_checking: Live, relayed(View::Going);
	/// On air: the accent frame, ON AIR, and the clock.
	on_air: Live, relayed(View::OnAir);
	/// On air, the clock clicked: End stream or Keep live.
	end_confirm: Live, relayed(View::EndConfirm);
	/// On air, the health dot's popover: the worst problem, every
	/// destination, the viewers, and the encoder.
	on_air_health: Live, relayed(View::OnAirHealth);
	/// Settings, Destinations: the relay, its token, and the broadcast's
	/// name, with the failover and on demand under Advanced.
	settings_destinations: Still, on_show(View::Setting(Setting::Destinations));
	/// Settings, Destinations with `--connect` set: the flag's relay, locked,
	/// and the token kept for it.
	settings_destinations_flagged: Live, relayed(View::Setting(Setting::Destinations));
	/// The multiview off air: every scene and the sources picked, the scene
	/// the stream opens on amber, and the clock, which keeps it from being
	/// still.
	multiview: Live, on_show(View::Multiview { live: false, stalled: false });
	/// The multiview with the host's camera stalled: its tile and each scene
	/// tile showing it carry the amber badge, and its picture says so.
	multiview_stalled: Live, on_show(View::Multiview { live: false, stalled: true });
	/// The multiview on air in Studio mode: the scene on air framed in the
	/// accent, the cued one amber, and the sources the live picture shows
	/// dotted.
	multiview_on_air: Live, relayed(View::Multiview { live: true, stalled: false });
}

/// Draw the view `name` opens into `$SHOTS_DIR/<name>.png`, twice if it is
/// still, failing if the two differ.
fn shot(name: &str, motion: Motion, open: Open) {
	let out = std::env::var_os("SHOTS_DIR").expect("SHOTS_DIR names where the PNGs go; run `just shots`");
	// Absolute, before the change of directory below.
	let out = std::path::absolute(out).unwrap();
	std::fs::create_dir_all(&out).unwrap();

	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let image = shoot(&open, &gpu, &runtime, &fixture.join("shots"));
	let path = out.join(format!("{name}.png"));
	if motion == Motion::Still {
		let again = shoot(&open, &gpu, &runtime, &fixture.join("shots"));
		if image != again {
			// Both, to see what moved.
			let again_path = out.join(format!("{name}.again.png"));
			image::DynamicImage::ImageRgba8(again)
				.to_rgb8()
				.save(&again_path)
				.unwrap();
			image::DynamicImage::ImageRgba8(image).to_rgb8().save(&path).unwrap();
			panic!(
				"the still view `{name}` drew a different picture twice: {}",
				again_path.display()
			);
		}
	}
	// The window is opaque, and a quarter smaller without the alpha.
	image::DynamicImage::ImageRgba8(image).to_rgb8().save(&path).unwrap();
	println!("{}", path.display());
}

/// A directory of its own to open the show from, made the current one, a
/// runtime, and Mesa's software GPU.
///
/// The show is opened as `show.toml` from there, so the window names it the
/// same on every machine. nextest runs each test in a process of its own,
/// which the change of directory relies on.
fn lavapipe() -> (tempfile::TempDir, tokio::runtime::Runtime, engine::Gpu) {
	let show = tempfile::tempdir().unwrap();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(
		fixture.join("../moq-scene/src/test_data/person.jpg"),
		show.path().join("person.jpg"),
	)
	.unwrap();
	std::env::set_current_dir(show.path()).unwrap();

	let runtime = tokio::runtime::Runtime::new().unwrap();
	// Vulkan only, where `just shots` names lavapipe the sole driver: any other
	// backend would find the host's hardware and draw a different picture.
	let gpu = pollster::block_on(engine::Gpu::new(wgpu::Backends::VULKAN)).expect("a GPU");
	let info = gpu.adapter.get_info();
	assert_eq!(
		info.device_type,
		wgpu::DeviceType::Cpu,
		"views draw on lavapipe, not {}",
		info.name
	);
	(show, runtime, gpu)
}

/// How a view starts the studio: recording beside the show.
fn launch(gpu: &engine::Gpu) -> engine::Launch {
	engine::Launch {
		gpu: gpu.clone(),
		recordings: ".".into(),
		sessions: Default::default(),
	}
}

/// `show.toml`'s stream settings, on `relay` if given, with tokens in a store
/// of the test's own.
fn stream(relay: Option<&Relay>) -> stream::Stream {
	let mut flags = stream::Flags::default();
	let mut client = moq_tokio::connect::Config::default();
	if let Some(relay) = relay {
		flags.relay = Some(relay.url.clone());
		client.tls.fingerprint = vec![relay.fingerprint.clone()];
	}
	let credentials = stream::Credentials::memory();
	stream::Stream::load(Path::new("show.toml"), flags, client, Default::default(), credentials).unwrap()
}

/// A relay of the view's own, running on `runtime`.
struct Relay {
	url: url::Url,
	fingerprint: String,
}

impl Relay {
	fn start(runtime: &tokio::runtime::Runtime) -> Self {
		runtime.block_on(async {
			let _ = moq_tokio::crypto::install_default();
			let fixture = moq_relay::test_relay().await.expect("test relay");
			let ready = fixture.relay.ready();
			tokio::spawn(fixture.relay.run());
			ready.wait().await.expect("relay ready");
			Self {
				url: fixture.url,
				fingerprint: fixture.fingerprint,
			}
		})
	}
}

/// Start the studio by `launch` on `show.toml`, without the history an
/// earlier draw's edits left, streaming as `stream` sets.
fn start(launch: engine::Launch, stream: stream::Stream, runtime: &tokio::runtime::Runtime) -> App {
	let _ = std::fs::remove_file("show.history.toml");
	let (autosave, loaded) = autosave::Autosave::open("show.toml".into()).unwrap();
	App::Starting(
		runtime
			.block_on(engine::Engine::start(launch, autosave, loaded))
			.unwrap(),
		Box::new(stream),
	)
}

/// A scene pad's ⋯ button, which shows while the pointer is on it, opens its
/// menu, and Remove asks first for a scene with more than one source and
/// removes a plain one at once; a pad the keyboard focused removes its scene
/// on Delete, leaving the selected layer alone; the scene on program goes
/// after the next takes its place; and the last one is emptied, so the show
/// keeps one; undo brings each back. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_scene_removes_from_its_pad() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.build(Side::Layers, Inspect::Layer, Some(1));
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let scenes = || {
		let state = engine.state.lock().unwrap();
		let names: Vec<String> = state
			.collection()
			.scenes
			.iter()
			.map(|scene| scene.name.clone())
			.collect();
		(names, state.compositor.program_scene().map(str::to_string))
	};
	let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect::<Vec<_>>();
	// The menu's or the sheet's, which comes after the layers' own Remove.
	let more = |harness: &mut Harness<'_, Window>, pad: &str| {
		harness.get_by_label(pad).hover();
		harness.run_steps(2);
		harness.get_by_label("Scene options").click();
		harness.run_steps(2);
		harness.get_all_by_label("Remove").last().unwrap().click();
		harness.run_steps(2);
	};

	// The button is only there while the pointer is on a pad.
	assert!(harness.query_by_label("Scene options").is_none());
	// Two sources: asked first, and left alone until the answer.
	more(&mut harness, "Break");
	harness.get_by_label_contains("Undo brings it back").hover();
	assert_eq!(scenes().0, names(&["Main", "Interview", "Break"]), "it asked first");
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert_eq!(scenes().0.len(), 3, "Escape left it");
	more(&mut harness, "Break");
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(scenes().0, names(&["Main", "Interview"]), "it asked, then removed");
	let undo = || {
		let mut state = engine.state.lock().unwrap();
		state.undo().unwrap();
	};
	undo();
	assert_eq!(scenes().0.len(), 3, "undo brought it back");

	// One source: removed at once.
	{
		let mut state = engine.state.lock().unwrap();
		let mut draft = state.collection().draft("Add `Solo`");
		let mut solo = moq_scene::Scene::new("Solo");
		solo.items = vec![moq_scene::Item::new(
			"backdrop",
			moq_scene::Rect::fill(&state.collection().canvas),
		)];
		draft.insert_scene(3, solo).unwrap();
		state.commit(draft, None).unwrap();
	}
	harness.run_steps(2);
	harness.get_by_label("Solo").click_secondary();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(scenes().0.len(), 3, "a scene of one source went without asking");

	// A focused pad takes Delete, not the selected layer, which a removal let go of.
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!()
	};
	studio.build(Side::Layers, Inspect::Layer, Some(1));
	harness.run_steps(2);
	let layers = || {
		engine
			.state
			.lock()
			.unwrap()
			.collection()
			.scene("Main")
			.unwrap()
			.items
			.len()
	};
	assert!(
		{
			let App::Studio(studio) = &harness.state().app else {
				unreachable!()
			};
			studio.selected()
		},
		"a layer is selected"
	);
	harness.get_by_label("Break").focus();
	harness.run_steps(2);
	harness.key_press(egui::Key::Delete);
	harness.run_steps(2);
	harness.get_by_label_contains("Undo brings it back").hover();
	assert_eq!(layers(), 3, "the layer stayed");
	assert_eq!(scenes().0.len(), 3, "the pad asked first");
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);

	// A scene only leaving program in a transition goes with the transition
	// finished on the scene it goes to.
	engine
		.state
		.lock()
		.unwrap()
		.take("Interview", kio::clock::Instant::now())
		.unwrap();
	assert!(engine.state.lock().unwrap().compositor.transitioning());
	more(&mut harness, "Main");
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(scenes(), (names(&["Interview", "Break"]), Some("Interview".into())));
	assert!(!engine.state.lock().unwrap().compositor.transitioning());

	// The scene on program goes after the next takes its place.
	more(&mut harness, "Interview");
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(scenes(), (names(&["Break"]), Some("Break".into())));

	// The last one is emptied.
	harness.get_by_label("Break").click_secondary();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	let (left, program) = scenes();
	assert_eq!(left, names(&["Break"]), "the show keeps one scene");
	assert_eq!(program.as_deref(), Some("Break"));
	let items = || {
		let state = engine.state.lock().unwrap();
		state.collection().scene("Break").unwrap().items.len()
	};
	assert_eq!(items(), 0, "and empties it");
	undo();
	assert_eq!(items(), 2, "undo fills it again");

	// With no other scene able to go on program, a new empty one takes its place.
	{
		let mut state = engine.state.lock().unwrap();
		let mut draft = state.collection().draft("Add `Wide`");
		let mut wide = moq_scene::Scene::new("Wide");
		wide.size = Some(moq_scene::Size {
			width: 640,
			height: 480,
		});
		draft.insert_scene(1, wide).unwrap();
		state.commit(draft, None).unwrap();
	}
	harness.run_steps(2);
	// The strip's pad, after any other label of the scene's name.
	harness.get_all_by_label("Break").last().unwrap().click_secondary();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(scenes(), (names(&["Wide", "Scene 3"]), Some("Scene 3".into())));
	quit(&engine, &runtime);
}

/// A show opened with its only scene of another size than the program has
/// nothing on program; removing that scene empties it and opens it off air,
/// where its quick start is. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_only_scene_of_another_size_empties_into_its_quick_start() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let mut show = show::first(moq_scene::Canvas::default(), false).unwrap();
	let name = show.scenes[0].name.clone();
	show.scenes[0].size = Some(moq_scene::Size {
		width: 640,
		height: 480,
	});
	std::fs::write("show.toml", show.to_toml().unwrap()).unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.seed_devices(first_devices());
	harness.run_steps(2);
	assert!(
		engine.state.lock().unwrap().compositor.program_scene().is_none(),
		"nothing is on program"
	);
	harness.get_all_by_label(&name).last().unwrap().click_secondary();
	harness.run_steps(2);
	harness.get_all_by_label("Remove").last().unwrap().click();
	harness.run_steps(2);
	assert_eq!(engine.state.lock().unwrap().collection().scenes.len(), 1);
	assert!(
		harness.query_by_label("Screen").is_some(),
		"its quick start is shown, the scene open off air"
	);
	quit(&engine, &runtime);
}

/// A click on a pad takes its scene on air, and the panic key mutes every
/// microphone and guest, in an edit undo takes back, and cuts to the safe
/// scene: the window as a person uses it, on a real engine. A guest of a room
/// the engine has since left is muted too; remote feeds that are not guests
/// stay on. Then the title bar's Close closes it. It draws no
/// view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_click_takes_and_the_panic_key_cuts() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let mut show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	// Failed in a build without `capture`, which leaves it a microphone to mute.
	show.push_str("\n[sources.mic]\nkind = \"microphone\"\n");
	// A guest in the room the show's `guest` and `caller` feeds are under,
	// which are not guests' broadcasts.
	show.push_str("\n[sources.alice]\nkind = \"broadcast\"\npath = \"room/alice/camera.hang\"\n");
	std::fs::write("show.toml", show).unwrap();
	let connect = url::Url::parse("https://relay.example/show").unwrap();
	let page = url::Url::parse("https://studio.example/join/").unwrap();
	let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
	let room = crate::guests::Room::new(&connect, "room", key, page.clone(), crate::guests::tests::EXPIRY).unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let App::Starting(engine, _) = &app else {
		unreachable!("the studio starts")
	};
	engine.join(Some(crate::guests::Joining::offline(room))).unwrap();
	// Moving to another room keeps `alice`, added from the first, a guest.
	let key = crate::guests::tests::key("show/other", &["**"], &["**"]);
	let other = crate::guests::Room::new(&connect, "other", key, page, crate::guests::tests::EXPIRY).unwrap();
	engine.join(Some(crate::guests::Joining::offline(other))).unwrap();
	let (mut harness, started, closed) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let program = || {
		engine
			.state
			.lock()
			.unwrap()
			.compositor
			.program_scene()
			.map(str::to_string)
	};
	let muted = |name: &str| {
		let state = engine.state.lock().unwrap();
		state.collection().audio.get(name).is_some_and(|audio| audio.muted)
	};
	assert_eq!(program().as_deref(), Some("Main"));

	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert_eq!(program().as_deref(), Some("Interview"), "the pad took its scene");

	harness.get_by_label("Panic").click();
	harness.run_steps(2);
	assert_eq!(program().as_deref(), Some("Break"), "cut to the safe scene");
	assert!(muted("mic"), "the microphone is muted");
	assert!(muted("alice"), "the guest is muted");
	assert!(!muted("guest") && !muted("caller"), "other remote feeds stay on");

	harness.get_by_label_contains("Undo: Panic: ").click();
	harness.run_steps(2);
	assert!(!muted("mic") && !muted("alice"), "undo takes the mute back");
	assert_eq!(program().as_deref(), Some("Break"), "and leaves program where it is");

	// The title bar's Close asks egui to close, which eframe passes back as a
	// close event the window counts as asked for, not as a failure.
	harness.get_by_label("Close").click();
	harness.step();
	let commands = &harness.output().viewport_output[&egui::ViewportId::ROOT].commands;
	assert!(commands.contains(&egui::ViewportCommand::Close), "{commands:?}");
	assert!(!closed.load(Ordering::Relaxed));
	let root = harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
	root.events.push(egui::ViewportEvent::Close);
	harness.step();
	assert!(
		closed.load(Ordering::Relaxed),
		"the window counts the close as asked for"
	);

	drop(harness);
	quit(&engine, &runtime);
}

/// A name typed into a marker's row on REVIEW names that marker: when
/// another marker's name is clicked next, and when a key drops a new marker
/// above it while it is being typed. Off air, the program keeps no past, so
/// no marker can be clipped around. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_marker_is_named_where_it_is_typed() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	enable(&mut harness, &[Mod::Review]);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	let controls = studio.controls();
	studio.turn(Page::Review);
	let dir = engine.recorder.dir();
	for _ in 0..2 {
		controls.markers.mark(&dir, kio::clock::Instant::now()).unwrap();
	}
	harness.run_steps(2);
	let name = |number| controls.markers.get(number).unwrap().name;
	// The newest first: marker 2's name, then marker 1's.
	let field = |harness: &Harness<'_, Window>, index: usize| {
		harness.get_all_by_role(Role::TextInput).nth(index).unwrap().focus();
	};

	field(&harness, 1);
	harness.run_steps(1);
	harness.event(egui::Event::Text("Opening".into()));
	harness.run_steps(1);
	field(&harness, 0);
	harness.run_steps(2);
	assert_eq!(
		name(1).as_deref(),
		Some("Opening"),
		"kept when marker 2 took the typing"
	);

	harness.event(egui::Event::Text("Come".into()));
	harness.run_steps(1);
	// A key drops marker 3 over the row being typed in.
	controls.press(&crate::controls::Action::Marker);
	harness.run_steps(2);
	harness.event(egui::Event::Text("back".into()));
	harness.run_steps(1);
	harness.key_press(egui::Key::Enter);
	harness.run_steps(2);
	assert_eq!(name(2).as_deref(), Some("Comeback"), "the typing stayed on marker 2");
	assert_eq!(name(3), None);

	// Until the strip's thread first samples the program, nothing refuses.
	let refused = |harness: &Harness<'_, Window>| {
		let scissors: Vec<_> = harness.get_all_by_label("Clip around it").collect();
		scissors.len() == 3 && scissors.iter().all(|clip| clip.accesskit_node().is_disabled())
	};
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while !refused(&harness) {
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("no marker is clipped around off air");

	drop(harness);
	quit(&engine, &runtime);
}

/// GO LIVE on a show with no relay asks for one in its popover, and Go
/// live now saves it, waits on the check, and goes live once it passes,
/// with no further press. A click on the
/// clock then asks before it ends the stream: Keep live keeps it on air, End
/// stream ends it. The window as a person uses it, on a real engine and
/// relay, with the check's results fed in, since timing the encoder and
/// measuring the connection fall short on a loaded host; `probe` and `uplink`
/// test those against a real encoder and relay. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn go_live_from_no_relay_and_end_on_confirm() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let relay = Relay::start(&runtime);
	// The relay's certificate is trusted, but no relay is set.
	let mut client = moq_tokio::connect::Config::default();
	client.tls.fingerprint = vec![relay.fingerprint.clone()];
	let credentials = stream::Credentials::memory();
	let stream = stream::Stream::load(
		Path::new("show.toml"),
		Default::default(),
		client,
		Default::default(),
		credentials,
	)
	.unwrap();
	let app = start(launch(&gpu), stream, &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	// An encoder that keeps up, on a connection measuring until told.
	let (carries, fed) = tokio::sync::watch::channel(None);
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	*studio.controls().fed.lock().unwrap() = Some(crate::controls::Fed {
		encodings: vec![crate::probe::Encoding {
			encoder: "openh264".into(),
			gpu: false,
			bitrate: moq_net::bandwidth::Rate::from_mbps(4),
			frame: Duration::from_millis(5),
		}],
		carries: fed,
	});
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");

	harness.get_by_label("Go live").click();
	harness.run_steps(2);
	harness
		.get_all_by_role(Role::TextInput)
		.next()
		.expect("the popover asks for a relay")
		.focus();
	harness.run_steps(2);
	harness
		.get_all_by_role(Role::TextInput)
		.next()
		.unwrap()
		.type_text(relay.url.as_str());
	harness.run_steps(2);
	harness.get_by_label("Go live now").click();
	// The save runs on a thread of its own; once it lands and the relay is
	// dialed, the check measures the connection, and the popover waits on it.
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while harness
				.query_by_label("Measuring the connection to the relay")
				.is_none()
			{
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the check measures the connection");
	harness.run_steps(4);
	assert!(
		harness.query_by_label("Going live once checked").is_some(),
		"it waits on the check"
	);
	assert_eq!(engine.on_air(), engine::OnAir::Off, "and not before it passes");
	carries.send_replace(Some(moq_net::bandwidth::Rate::from_mbps(100)));
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while engine.on_air().live().is_none() {
				harness.step();
				assert!(harness.query_by_label("Go live anyway").is_none(), "the check passes");
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the studio goes live once the check passes");
	let saved = std::fs::read_to_string("show.stream.toml").unwrap();
	assert!(
		saved.contains(relay.url.host_str().unwrap()),
		"the relay is saved: {saved}"
	);

	harness.run_steps(2);
	harness.get_by_label("Live: end the stream").click();
	harness.run_steps(2);
	harness.get_by_label("Keep live").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Keep live").is_none(), "the popover closes");
	assert!(engine.on_air().live().is_some(), "and the stream stays on air");
	harness.get_by_label("Live: end the stream").click();
	harness.run_steps(2);
	harness.get_by_label("End stream").click();
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while engine.on_air() != engine::OnAir::Off {
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the stream ends");
	drop(harness);
	quit(&engine, &runtime);
}

/// A marker dropped by the flag beside the panic key, on air, shows on
/// REVIEW, and a click on its row saves the seconds around it as a clip:
/// from [`replay::BEFORE`] it to [`replay::AFTER`] it, its video starting up
/// to a keyframe interval early. The window as a person uses it, on a real
/// engine going live to a relay. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_marker_is_clipped_around() {
	use crate::replay;

	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let relay = Relay::start(&runtime);
	let app = start(launch(&gpu), stream(Some(&relay)), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	enable(&mut harness, &[Mod::Review]);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	let controls = studio.controls();
	controls.go_live().expect("going live starts");
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while engine.on_air().live().is_none() {
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the studio goes live");
	// On air long enough before the marker for all of its clip to be held.
	let wait = |seconds: Duration| runtime.block_on(kio::clock::sleep(seconds));
	wait(replay::BEFORE + Duration::from_secs(1));

	harness.get_by_label("Drop a marker").click();
	harness.run_steps(2);
	assert_eq!(controls.markers.list().len(), 1, "the flag dropped a marker");
	// Past the seconds after it, so the clip is the whole span around it.
	wait(replay::AFTER + Duration::from_secs(1));
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("still the studio")
	};
	studio.turn(Page::Review);
	harness.run_steps(2);
	harness.get_by_label("Clip around it").click();
	harness.run_steps(2);
	let outcome = controls.replay.lock().unwrap().outcome.clone();
	let saved = runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			loop {
				match outcome.lock().unwrap().clone() {
					Some(Ok(saved)) if saved.starts_with("saved") => return Ok(saved),
					Some(Err(err)) => return Err(err),
					_ => {}
				}
				kio::clock::sleep(Duration::from_millis(50)).await;
			}
		}))
		.expect("the clip is written in real time");
	saved.expect("the clip saves");

	let mut file = std::fs::File::open("show-clip-1.mp4").expect("the clip is in the recordings folder");
	let moov = std::iter::from_fn(|| mp4_atom::ReadFrom::read_from(&mut file).unwrap())
		.find_map(|atom| match atom {
			mp4_atom::Any::Moov(moov) => Some(moov),
			_ => None,
		})
		.expect("a regular MP4");
	let length = Duration::from_secs_f64(moov.mvhd.duration as f64 / f64::from(moov.mvhd.timescale));
	let span = replay::BEFORE + replay::AFTER;
	assert!(
		length >= span - Duration::from_millis(200),
		"{length:?} is short of {span:?}"
	);
	assert!(
		length <= span + Duration::from_secs(3),
		"{length:?} is more than a keyframe past {span:?}"
	);
	// Sound, a keyframe every packet, holds the span itself.
	let sound = moov
		.trak
		.iter()
		.find(|trak| trak.mdia.hdlr.handler == b"soun".into())
		.expect("the program's sound");
	let mdhd = &sound.mdia.mdhd;
	let heard = Duration::from_secs_f64(mdhd.duration as f64 / f64::from(mdhd.timescale));
	assert!(
		heard.abs_diff(span) < Duration::from_millis(100),
		"{heard:?} heard of {span:?}"
	);

	drop(harness);
	quit(&engine, &runtime);
}

/// The command palette as a person uses it: its key opens it from Studio,
/// what is typed finds a scene, a sound, and a setting, and Enter runs the
/// one picked, taking the scene, muting the sound, and opening Settings at
/// the setting. An action that cannot run now says why and stays open.
/// Every setting the palette finds by name is a row of its pane. It draws
/// no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_palette_finds_and_runs_anything() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let mut show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	// Failed in a build without `capture`, which leaves it a sound to mute.
	show.push_str("\n[sources.mic]\nkind = \"microphone\"\n");
	std::fs::write("show.toml", show).unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let program = || {
		let state = engine.state.lock().unwrap();
		state.compositor.program_scene().map(str::to_string)
	};
	let muted = |name: &str| {
		let state = engine.state.lock().unwrap();
		state.collection().audio.get(name).is_some_and(|audio| audio.muted)
	};
	let run = |harness: &mut Harness<'_, Window>, query: &str| {
		harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
		harness.run_steps(2);
		harness.get_by_role(Role::TextInput).type_text(query);
		harness.run_steps(2);
		harness.key_press(egui::Key::Enter);
		harness.run_steps(2);
	};
	assert_eq!(program().as_deref(), Some("Main"));

	run(&mut harness, "interview");
	assert_eq!(program().as_deref(), Some("Interview"), "the palette took the scene");
	assert!(harness.query_by_role(Role::TextInput).is_none(), "and closed");

	// Typed and run in one frame, Enter runs what was typed.
	harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
	harness.run_steps(2);
	let enter = egui::Event::Key {
		key: egui::Key::Enter,
		physical_key: None,
		pressed: true,
		repeat: false,
		modifiers: egui::Modifiers::NONE,
	};
	let events = &mut harness.input_mut().events;
	events.extend([egui::Event::Text("take scene break".into()), enter]);
	harness.run_steps(2);
	assert_eq!(program().as_deref(), Some("Break"), "the palette ran what was typed");

	assert!(!muted("mic"));
	run(&mut harness, "mute mic");
	assert!(muted("mic"), "the palette muted the sound");
	run(&mut harness, "unmute mic");
	assert!(!muted("mic"), "and unmuted it");

	// TAKE with nothing cued says why, and Enter leaves it open, with
	// Studio mode's mod on, as it is not listed otherwise.
	run(&mut harness, "take the cued");
	assert!(harness.query_by_label("Take the cued scene").is_none(), "unlisted");
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	enable(&mut harness, &[Mod::Studio]);
	run(&mut harness, "take the cued");
	let take = harness.get_by_label("Take the cued scene");
	assert!(take.accesskit_node().is_disabled(), "it cannot run");
	assert_eq!(program().as_deref(), Some("Break"));
	assert!(harness.query_by_role(Role::TextInput).is_some(), "still open");
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(harness.query_by_role(Role::TextInput).is_none(), "Escape closes it");

	// Add a source from another page opens Add over Studio's picture.
	enable(&mut harness, &[Mod::Audio]);
	run(&mut harness, "go to mix");
	assert!(harness.query_by_label("Canvas").is_none(), "on MIX");
	run(&mut harness, "add a source");
	harness.run_steps(2);
	assert!(harness.query_by_label("Close Add").is_some(), "Add is open on Studio");
	harness.get_by_label("Close Add").click();
	harness.run_steps(2);

	// A setting under a disclosure opens it, its mod on.
	enable(&mut harness, &[Mod::Published]);
	run(&mut harness, "scene sizes");
	assert!(
		harness.query_by_label("Give a scene its own size").is_some(),
		"Scene sizes opened"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);

	run(&mut harness, "fps");
	assert!(
		harness.query_by_label("Frame rate").is_some(),
		"Settings opened at the frame rate"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);

	// A key bound to a source with no sound, which the MIX lists no strip for,
	// is still a sound of the show; only one the file lacks is no such sound.
	{
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!("the studio is open")
		};
		let key = |code, name: &str| hotkeys::Binding {
			modifiers: egui::Modifiers::NONE,
			key: winit::keyboard::PhysicalKey::Code(code),
			name: winit::keyboard::Key::Character(name.into()),
		};
		use crate::controls::Action;
		use winit::keyboard::KeyCode;
		studio.bind(&Action::Mute("host".into()), key(KeyCode::F7, "F7"));
		studio.bind(&Action::Mute("nope".into()), key(KeyCode::F8, "F8"));
		studio.open(Setting::Hotkeys);
	}
	harness.run_steps(2);
	assert!(
		harness.query_by_label("host").is_some(),
		"the silent source's key is listed"
	);
	assert_eq!(
		harness.query_all_by_label("no such sound").count(),
		1,
		"only the missing source is no such sound"
	);

	// Each setting the palette finds by name is a row of its pane while its
	// mod is on, and absent while off, as is a pane whose mod is off.
	{
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!("the studio is open")
		};
		for which in Mod::ALL {
			studio.switch(which, false);
		}
	}
	for all in [false, true] {
		if all {
			enable(&mut harness, &Mod::ALL);
		}
		for setting in Setting::ALL {
			let App::Studio(studio) = &mut harness.state_mut().app else {
				unreachable!("the studio is open")
			};
			studio.open(setting);
			harness.run_steps(2);
			let shown = all || setting.needs().is_none();
			assert_eq!(
				harness.query_all_by_label(setting.name()).next().is_some(),
				shown,
				"{setting:?} listed: {all}"
			);
			if !shown {
				continue;
			}
			let rows = setting
				.rows()
				.iter()
				.map(|(name, _, needs)| (*name, all || needs.is_none()));
			let mods = Mod::ALL
				.iter()
				.filter(|_| setting == Setting::Mods)
				.map(|which| (which.name(), true));
			for (name, on) in rows.chain(mods) {
				// Found as the palette finds it, which opens what holds it.
				let ctx = harness.ctx.clone();
				let App::Studio(studio) = &mut harness.state_mut().app else {
					unreachable!("the studio is open")
				};
				studio.seek(&ctx, name);
				harness.run_steps(2);
				assert_eq!(
					harness.query_all_by_label(name).next().is_some(),
					on,
					"{setting:?}'s row `{name}` shown, with every mod on: {all}"
				);
			}
		}
	}

	drop(harness);
	quit(&engine, &runtime);
}

/// Settings changes the show as a person would: a canvas size by name and
/// the format turn the program, keeping its frame rate. A show whose room
/// key is missing still opens, and once the key is back, Try again joins its
/// room. The Guests pane refuses a room guests could not join, saying why,
/// and moves to the room it saves at once, off air, dropping a phone's link
/// into the room it left. The recordings folder
/// moves only once no recording is under way. The scene
/// graph's Open, once switched on in Mods, closes Settings for the graph.
/// It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn settings_change_the_show_and_move_the_guests() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	// A canvas size no name has.
	let show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	let mut custom = show.replacen("width = 1920\nheight = 1080", "width = 1366\nheight = 768", 1);
	assert_ne!(custom, show);
	// A guest of the room, kept from an earlier run.
	custom.push_str("\n[sources.alice]\nkind = \"broadcast\"\npath = \"guests/alice/camera.hang\"\n");
	std::fs::write("show.toml", custom).unwrap();
	// A room whose key is not there yet.
	std::fs::write(
		"show.stream.toml",
		"recordings = \"elsewhere\"\nroom = \"guests\"\nroom_key = \"guests.jwk\"\n",
	)
	.unwrap();
	std::fs::create_dir("elsewhere").unwrap();
	let relay = Relay::start(&runtime);
	// Where the stream file puts them, as opening the show does.
	let launched = engine::Launch {
		recordings: "elsewhere".into(),
		..launch(&gpu)
	};
	let app = start(launched, stream(Some(&relay)), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let canvas = || engine.state.lock().unwrap().collection().canvas;
	let framerate = canvas().framerate;

	harness.get_by_label("Settings").click();
	harness.run_steps(2);
	// Custom is lit, and a click on it still opens its editor.
	assert!(harness.query_by_label("Cancel").is_none());
	harness.get_all_by_label("Custom").next().unwrap().click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Cancel").is_some(), "the editor is open");
	assert_eq!((canvas().width, canvas().height), (1366, 768), "and nothing changed");
	harness.get_by_label("720p").click();
	harness.run_steps(2);
	assert_eq!((canvas().width, canvas().height), (1280, 720));
	assert!(harness.query_by_label("Portrait").is_none(), "Both formats is off");
	enable(&mut harness, &[Mod::Formats]);
	harness.get_by_label("Portrait").click();
	harness.run_steps(2);
	assert_eq!((canvas().width, canvas().height), (720, 1280));
	assert_eq!(canvas().framerate, framerate, "the format keeps the frame rate");
	harness.get_by_label("1080p").click();
	harness.run_steps(2);
	assert_eq!(
		(canvas().width, canvas().height),
		(1080, 1920),
		"a size keeps the format"
	);

	harness.get_by_label("Guests").click();
	harness.run_steps(2);
	// Fill a field, replaced whole.
	let fill = |harness: &mut Harness<'_, Window>, index: usize, text: &str| {
		harness.get_all_by_role(Role::TextInput).nth(index).unwrap().focus();
		harness.run_steps(2);
		harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
		harness.key_press(egui::Key::Backspace);
		harness
			.get_all_by_role(Role::TextInput)
			.nth(index)
			.unwrap()
			.type_text(text);
		harness.run_steps(2);
	};
	let joined = |harness: &mut Harness<'_, Window>, room: &str| {
		runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while engine.room().is_none_or(|joined| joined.path().as_str() != room) {
					harness.step();
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.unwrap_or_else(|_| panic!("guests join `{room}`"));
	};
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while harness.query_by_label_contains("Guests cannot join").is_none() {
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the room failed to join at launch");
	assert!(engine.room().is_none());
	// Panic still knows the room, so it mutes the guest of it in a scene.
	let muted = engine.state.lock().unwrap().panic(&engine.rooms()).unwrap();
	assert!(
		engine.state.lock().unwrap().collection().audio["alice"].muted,
		"{muted}"
	);
	// Remote, with no room to pair in, sends the host to the room's pane.
	assert!(harness.query_by_label("Remote").is_none(), "Phone remote is off");
	enable(&mut harness, &[Mod::Remote]);
	harness.get_by_label("Remote").click();
	harness.run_steps(2);
	harness.get_by_label("Set up the room").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Try again").is_some(), "Guests is open");
	// The key is back in place, under the same name; the relay's path is the
	// room's parent.
	for room in ["guests", "other"] {
		crate::guests::tests::key(room, &["**"], &["**"])
			.to_file(format!("{room}.jwk"))
			.unwrap();
	}
	harness.get_by_label("Try again").click();
	joined(&mut harness, "guests");
	// A phone paired in this room, whose link stays on screen...
	harness.get_by_label("Remote").click();
	harness.run_steps(2);
	fill(&mut harness, 0, "phone");
	harness.get_by_label("Pair a phone").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Copy link").is_some(), "the phone's link shows");
	let left = engine.room().unwrap();
	harness.get_by_label("Guests").click();
	harness.run_steps(2);
	// Saved once no field has the keyboard, which a click away takes.
	fill(&mut harness, 0, "other");
	fill(&mut harness, 1, "");
	harness.get_by_label("Guests").click();
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while harness
				.query_by_label_contains("need both a room and a room key")
				.is_none()
			{
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("a room without its key is refused");
	assert_eq!(engine.room().unwrap().path().as_str(), "guests", "and the room stays");
	fill(&mut harness, 1, "other.jwk");
	harness.get_by_label("Guests").click();
	joined(&mut harness, "other");
	// The room left changes nothing the next could have missed.
	assert!(left.unpair("phone").is_err(), "the room left removes a remote");
	let saved = std::fs::read_to_string("show.stream.toml").unwrap();
	assert!(saved.contains("room = \"other\""), "{saved}");
	// ...until the room is another, which its link cannot join.
	harness.get_by_label("Remote").click();
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Copy link").is_none(),
		"a link into the room left shows"
	);

	// The recordings folder stays put while a recording is under way, then
	// moves with the file naming it.
	let recorder = &engine.recorder;
	let elsewhere = std::path::absolute("elsewhere").unwrap();
	assert_eq!(recorder.dir(), elsewhere);
	harness.get_by_label("Recording").click();
	harness.run_steps(2);
	recorder.start(false).unwrap();
	harness.run_steps(2);
	harness.get_by_label("Beside the show").click();
	harness.run_steps(2);
	assert_eq!(recorder.dir(), elsewhere, "not while recording");
	recorder.stop();
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while recorder.status().is_some_and(|status| !status.done) {
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the recording finishes");
	harness.get_by_label("Beside the show").click();
	let beside = std::path::absolute(".").unwrap();
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while harness
				.query_by_label_contains("The next recording goes there")
				.is_none()
			{
				harness.step();
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the folder moves");
	assert_eq!(recorder.dir(), beside);
	let saved = std::fs::read_to_string("show.stream.toml").unwrap();
	assert!(!saved.contains("recordings"), "{saved}");

	// The scene graph, switched on, opens in place of Settings, so it takes
	// the clicks.
	harness.get_by_label("Mods").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Open").is_none(), "Scene graph is off");
	harness
		.get_by_role_and_label(Role::CheckBox, "Scene graph")
		.scroll_to_me();
	harness.run_steps(2);
	harness
		.get_by_role_and_label(Role::CheckBox, "Scene graph")
		.click_accesskit();
	harness.run_steps(2);
	harness.get_by_label("Open").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Mods").is_none(), "Settings closed");
	assert!(harness.query_by_label("Scene graph").is_some(), "the graph is open");

	drop(harness);
	quit(&engine, &runtime);
}

/// A Destinations edit still being typed, which waits to save until no field
/// has the keyboard, saves on the way out of the show, as Settings has no
/// Save button. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn an_edit_being_typed_saves_on_the_way_out() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started");
	harness.get_by_label("Settings").click();
	harness.run_steps(2);
	harness.get_by_label("Destinations").click();
	harness.run_steps(2);
	harness.get_by_label("Relay address").focus();
	harness.run_steps(2);
	harness
		.get_by_label("Relay address")
		.type_text("https://flushed.example/");
	harness.run_steps(4);
	let saved = || std::fs::read_to_string("show.stream.toml").unwrap_or_default();
	assert!(!saved().contains("flushed"), "not while it is typed");

	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.flush_settings().expect("the edit saves");
	assert!(saved().contains("flushed.example"), "saved: {}", saved());

	// One that cannot save says why, not the note of the save before it.
	harness.run_steps(2);
	harness.get_by_label("Relay address").focus();
	harness.run_steps(2);
	harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
	harness.get_by_label("Relay address").type_text("not a url");
	harness.run_steps(2);
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("still the studio")
	};
	let refused = studio.flush_settings().expect_err("an address that is not one");
	assert!(format!("{refused:#}").contains("not a URL"), "says why: {refused:#}");
	drop(harness);
	quit(&engine, &runtime);
}

/// Add, opened by its key over a new show, adds a card in one click and
/// closes: the first picture fills the canvas, a camera over it lands as a
/// picture-in-picture in the bottom right corner, and a card dragged onto the
/// picture lands where it is let go. Escape closes it. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn add_places_a_card_in_one_click() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let text = show::first(moq_scene::Canvas::default(), false)
		.unwrap()
		.to_toml()
		.unwrap();
	// Both at once, for the portrait picture's placement at the end.
	let text = text.replacen("version = 1\n", "version = 1\nportrait = true\n", 1);
	assert!(text.contains("portrait = true"));
	std::fs::write("show.toml", text).unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started");
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.seed_devices(add_devices());
	harness.run_steps(2);
	let items = || engine.state.lock().unwrap().collection().scenes[0].items.clone();
	let canvas = engine.state.lock().unwrap().collection().canvas;
	let (width, height) = (canvas.width as f32, canvas.height as f32);

	harness.key_press(egui::Key::A);
	harness.run_steps(2);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(harness.query_by_label("Add Display 1").is_none(), "Escape closes Add");

	// What had the keyboard beside it, such as a scene's pad, which Enter
	// would press, lets go of it as Add opens.
	harness.key_press(egui::Key::Tab);
	harness.run_steps(2);
	let focused = harness.ctx.memory(|memory| memory.focused());
	assert!(focused.is_some(), "Tab focuses a control");
	harness.key_press(egui::Key::A);
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Add Display 1").is_some(),
		"A opens Add over a focused control"
	);
	assert_eq!(harness.ctx.memory(|memory| memory.focused()), None);
	harness.get_by_label("Add Display 1").click();
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Add Display 1").is_none(),
		"Add closes once it adds"
	);
	assert_eq!(
		items()[0].rect,
		moq_scene::Rect::fill(&canvas),
		"the first picture fills"
	);

	harness.key_press(egui::Key::A);
	harness.run_steps(2);
	harness.get_by_label("Add Sony A6400").click();
	harness.run_steps(2);
	let camera = items()[1].clone();
	assert_eq!(camera.source, "Sony A6400");
	assert_eq!(camera.rect.width, (width * 0.3).round(), "a picture-in-picture");
	assert!(
		camera.rect.x > width / 2.0 && camera.rect.y > height / 2.0,
		"in the bottom right corner: {:?}",
		camera.rect
	);

	// Dragged onto the top left of the picture, it lands there.
	harness.key_press(egui::Key::A);
	harness.run_steps(2);
	let from = harness.get_by_label("Add Laptop camera").rect().center();
	let picture = harness.get_by_label("Canvas").rect();
	let to = picture.lerp_inside(egui::vec2(0.25, 0.25));
	harness.hover_at(from);
	harness.run_steps(1);
	harness.drag_at(from);
	harness.run_steps(1);
	for step in 1..=4 {
		harness.hover_at(from.lerp(to, step as f32 / 4.0));
		harness.run_steps(1);
	}
	harness.input_mut().events.push(egui::Event::PointerButton {
		pos: to,
		button: egui::PointerButton::Primary,
		pressed: false,
		modifiers: egui::Modifiers::NONE,
	});
	harness.run_steps(2);
	let dropped = items().into_iter().find(|item| item.source == "Laptop camera");
	let dropped = dropped.expect("the dragged card is added");
	let center = [
		dropped.rect.x + dropped.rect.width / 2.0,
		dropped.rect.y + dropped.rect.height / 2.0,
	];
	assert!(
		center[0] < width / 2.0 && center[1] < height / 2.0,
		"lands where it was let go: {:?}",
		dropped.rect
	);

	// Display 1 small in the portrait picture: it covers the landscape one
	// but not the portrait one, so the two disagree. Added from either
	// editor, a source goes in front as a picture-in-picture in both.
	{
		let mut state = engine.state.lock().unwrap();
		let main = moq_scene::Layer::scene(&state.collection().scenes[0].name);
		let items = state.collection().items(&main).unwrap().to_vec();
		let index = items.iter().position(|item| item.source == "Display 1").unwrap();
		let small = moq_scene::Rect {
			x: 0.0,
			y: 0.0,
			width: 300.0,
			height: 200.0,
		};
		let mut draft = state.collection().draft("Small in portrait");
		let item = moq_scene::Item {
			portrait: Some(moq_scene::Placement {
				rect: small,
				crop: moq_scene::Crop::default(),
			}),
			..items[index].clone()
		};
		draft.set_item(&main, index, item).unwrap();
		state.commit(draft, None).unwrap();
	}
	let mut in_front_of_both = |format, card: &str, source: &str| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			panic!("still the studio")
		};
		studio.edit_format(format);
		harness.key_press(egui::Key::A);
		harness.run_steps(2);
		harness.get_by_label(card).click();
		harness.run_steps(2);
		let added = items().last().cloned().expect("a layer");
		assert_eq!(added.source, source, "in front, editing {format:?}");
		assert_eq!(added.rect.width, (width * 0.3).round(), "a picture-in-picture");
		let portrait = added.portrait.expect("placed in portrait too").rect;
		assert_eq!(portrait.width, (canvas.turned().width as f32 * 0.3).round());
	};
	in_front_of_both(moq_scene::Format::Landscape, "Add Display 2", "Display 2");
	in_front_of_both(
		moq_scene::Format::Portrait,
		"Add Season 4 (Neon Drift)",
		"Season 4 (Neon Drift)",
	);
	drop(harness);
	quit(&engine, &runtime);
}

/// On Studio, the quick start never draws over the program on air, where one
/// click would put a device straight on the stream: an empty show going live
/// hides it, and ending the stream brings it back. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_quick_start_waits_off_air() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let text = show::first(moq_scene::Canvas::default(), false)
		.unwrap()
		.to_toml()
		.unwrap();
	std::fs::write("show.toml", text).unwrap();
	let relay = Relay::start(&runtime);
	let app = start(launch(&gpu), stream(Some(&relay)), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started");
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.seed_devices(first_devices());
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Screen").is_some(),
		"the quick start is shown off air"
	);

	let App::Studio(studio) = &harness.state().app else {
		unreachable!("checked above")
	};
	studio.controls().go_live().expect("going live starts");
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while engine.on_air().live().is_none() {
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the studio goes live");
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Screen").is_none(),
		"the quick start draws over the program on air"
	);

	runtime.block_on(engine.end_stream()).expect("the stream ends");
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Screen").is_some(),
		"the quick start is back off air"
	);
}

/// The Show menu switches shows only off air. New show closes the open show
/// as quitting does and opens a new one beside the others, empty but for its
/// quick start, whose Camera card adds the first layer; a rename moves the
/// show's files and opens it again; and a recent show opens from the menu, a
/// new engine in place of the old each time, which nothing keeps alive. It
/// draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_show_menu_switches_shows() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let relay = Relay::start(&runtime);
	let app = start(launch(&gpu), stream(Some(&relay)), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started");
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let App::Studio(studio) = &harness.state().app else {
		panic!("the studio starts on its first frame")
	};
	studio.controls().go_live().expect("going live starts");
	runtime
		.block_on(kio::clock::timeout(SETTLE, async {
			while engine.on_air().live().is_none() {
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		}))
		.expect("the studio goes live");

	// On air, the row is dimmed and nothing happens.
	harness.get_by_label("The show").click();
	harness.run_steps(4);
	harness.get_by_label("New show").click();
	harness.run_steps(4);
	assert!(matches!(harness.state().app, App::Studio(_)), "switched on air");
	runtime.block_on(engine.end_stream()).expect("the stream ends");
	harness.run_steps(2);

	// Each switch ends the open show, and opens the next.
	let switch = |harness: &mut Harness<'_, Window>, closed: Arc<engine::Engine>| {
		// Outside the runtime, which ending the show blocks on.
		harness.run_steps(2);
		runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while started.get().is_none_or(|open| Arc::ptr_eq(&open, &closed)) {
					harness.step();
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.expect("the next show opens");
		harness.step();
		assert!(matches!(harness.state().app, App::Studio(_)));
		// Nothing of the show outlives it: not its tasks, its threads, its hotkeys.
		let weak = Arc::downgrade(&closed);
		drop(closed);
		runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while weak.upgrade().is_some() {
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.expect("something still holds the closed show's engine");
		started.get().unwrap()
	};

	harness.get_by_label("The show").click();
	harness.run_steps(4);
	harness.get_by_label("New show").click();
	let engine = switch(&mut harness, engine);
	let made = std::path::absolute("shows/My Show.toml").unwrap();
	assert_eq!(engine.autosave.path(), made);
	assert!(made.exists(), "a new show is saved at once");
	{
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		// Lavapipe has no hardware encoder.
		assert_eq!(collection.canvas, moq_scene::Canvas::default());
		assert_eq!(collection.scenes.len(), 1);
		assert!(collection.scenes[0].items.is_empty());
	}

	// One click on the quick start adds the camera as the scene's first layer.
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("checked above")
	};
	studio.seed_devices(first_devices());
	harness.run_steps(2);
	harness.get_by_label("Camera").click();
	harness.run_steps(2);
	{
		let state = engine.state.lock().unwrap();
		let items = &state.collection().scenes[0].items;
		assert_eq!(items.len(), 1);
		assert_eq!(items[0].source, "Sony A6400");
	}
	harness.run_steps(2);
	assert!(harness.query_by_label("Screen").is_none(), "the quick start is gone");

	// A rename moves the show's files and opens it again.
	harness.get_by_label("The show").click();
	harness.run_steps(4);
	harness.get_by_label("Rename show…").click();
	harness.run_steps(2);
	harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
	harness.get_by_role(Role::TextInput).type_text("Podcast");
	harness.run_steps(2);
	harness.get_by_label("Rename").click();
	let engine = switch(&mut harness, engine);
	let renamed = std::path::absolute("shows/Podcast.toml").unwrap();
	assert_eq!(engine.autosave.path(), renamed);
	assert!(!made.exists());
	assert_eq!(engine.state.lock().unwrap().collection().scenes[0].items.len(), 1);

	// The first show opens again from the menu.
	harness.get_by_label("The show").click();
	harness.run_steps(4);
	harness.get_by_label("show").click();
	let engine = switch(&mut harness, engine);
	assert_eq!(engine.autosave.path(), std::path::absolute("show.toml").unwrap());
	let shows: Vec<_> = harness
		.state()
		.recent
		.shows
		.iter()
		.map(|show| show.path.clone())
		.collect();
	assert_eq!(shows, [std::path::absolute("show.toml").unwrap(), renamed]);
	drop(harness);
	quit(&engine, &runtime);
}

/// On Studio, a layer row's menu hides, locks, reorders, and duplicates its
/// layer, offering a copy of its own only with Linked layers on; Delete
/// removes the selected layer but not a locked one, and Escape lets go of
/// it; a scene's pad's menu sets the transition and duplicates the scene
/// with its own copies; undo
/// takes each back; and a take lets go of a layer of the scene it leaves.
/// It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_layer_menu_edits_its_layer() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.build(Side::Layers, Inspect::Layer, None);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let main = || {
		let state = engine.state.lock().unwrap();
		state.collection().scene("Main").unwrap().items.clone()
	};
	let menu = |harness: &mut Harness<'_, Window>, row: &str, button: &str| {
		harness.get_all_by_label(row).next().unwrap().click_secondary();
		harness.run_steps(2);
		// The menu's, which comes after the row's own eye and lock and the
		// toolbar's buttons of the same names.
		harness.get_all_by_label(button).last().unwrap().click();
		harness.run_steps(2);
	};
	let selected = |harness: &mut Harness<'_, Window>| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!()
		};
		studio.selected()
	};

	menu(&mut harness, "bar", "Hide");
	assert!(!main()[2].visible, "the menu hid the bar");
	menu(&mut harness, "bar", "Lock");
	assert!(main()[2].locked, "and locked it");
	menu(&mut harness, "bar", "Send backward");
	assert_eq!(main()[1].source, "bar", "it went back one");
	harness.get_all_by_label("host").next().unwrap().click_secondary();
	harness.run_steps(2);
	assert!(
		harness.query_by_label("Duplicate as its own copy").is_none(),
		"a copy of its own waits for Linked layers"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	enable(&mut harness, &[Mod::Linked]);
	menu(&mut harness, "host", "Duplicate as its own copy");
	let items = main();
	assert_eq!(items.len(), 4);
	assert_eq!(items[3].source, "host copy", "the copy went in front of the host");
	let cloned = engine.state.lock().unwrap().collection().sources["host copy"].clone();
	assert_eq!(cloned, moq_scene::Source::Clone { source: "host".into() });

	harness.get_all_by_label("host copy").next().unwrap().click();
	harness.run_steps(2);
	harness.key_press(egui::Key::Delete);
	harness.run_steps(2);
	let shown: Vec<String> = main().into_iter().map(|item| item.source).collect();
	assert_eq!(shown, ["backdrop", "bar", "host"], "Delete removed the selected copy");
	harness.get_all_by_label("bar").next().unwrap().click();
	harness.run_steps(2);
	harness.key_press(egui::Key::Backspace);
	harness.run_steps(2);
	assert_eq!(main().len(), 3, "a locked layer stays");
	harness.get_by_label("`bar` is locked; unlock it to remove it");
	assert!(selected(&mut harness));
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(!selected(&mut harness), "Escape let go of it");

	// A pad's menu sets the show's transition.
	harness.get_by_label("Interview").click_secondary();
	harness.run_steps(2);
	// A submenu's button is named with its arrow.
	harness.get_by_label_contains("Transition").click();
	harness.run_steps(4);
	harness.get_by_label("wipe").click();
	harness.run_steps(2);
	let transition = engine.state.lock().unwrap().collection().transition.clone();
	assert!(
		matches!(transition, moq_scene::Transition::Wipe { .. }),
		"{transition:?}"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);

	harness.get_by_label("Interview").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Duplicate with its own copies").click();
	harness.run_steps(2);
	{
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		let copy = collection.scene("Interview copy").expect("the scene was duplicated");
		let shown: Vec<&str> = copy.items.iter().map(|item| item.source.as_str()).collect();
		assert_eq!(shown, ["backdrop copy", "host copy 2", "guest copy"]);
	}

	for _ in 0..7 {
		harness.get_by_label_contains("Undo: ").click();
		harness.run_steps(2);
	}
	let items = main();
	let shown: Vec<&str> = items.iter().map(|item| item.source.as_str()).collect();
	assert_eq!(shown, ["backdrop", "host", "bar"], "undo took each edit back");
	assert!(items[2].visible && !items[2].locked);
	assert!(
		engine
			.state
			.lock()
			.unwrap()
			.collection()
			.scene("Interview copy")
			.is_none()
	);

	// A take moves Studio to another scene, letting go of a layer of this one.
	harness.get_all_by_label("bar").next().unwrap().click();
	harness.run_steps(2);
	assert!(selected(&mut harness), "the click selected the bar");
	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert!(!selected(&mut harness), "the take let go of Main's bar");

	drop(harness);
	quit(&engine, &runtime);
}

/// A Layers row dragged onto a slot on the picture takes that slot, unless
/// a guest appears in the scene mid-drag, whose new tile may have taken the
/// row's index: the drop then does nothing rather than move another layer.
/// It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_row_dropped_on_a_slot_after_a_guest_appears_moves_nothing() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/slots.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.build(Side::Layers, Inspect::Layer, None);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let guests = || {
		let state = engine.state.lock().unwrap();
		state.collection().scene("Guests").unwrap().items.clone()
	};
	let slot_of = |items: &[moq_scene::Item], source: &str| {
		items
			.iter()
			.find(|item| item.source == source)
			.and_then(|item| item.slot)
	};
	let pointer = |harness: &mut Harness<'_, Window>, pos: egui::Pos2, pressed: Option<bool>| {
		let event = match pressed {
			None => egui::Event::PointerMoved(pos),
			Some(pressed) => egui::Event::PointerButton {
				pos,
				button: egui::PointerButton::Primary,
				pressed,
				modifiers: Default::default(),
			},
		};
		harness.input_mut().events.push(event);
		harness.run_steps(2);
	};
	// The logo's row, and the main slot on the picture.
	let row = egui::pos2(160.0, 147.0);
	let main = egui::pos2(574.0, 400.0);
	let grab = |harness: &mut Harness<'_, Window>| {
		pointer(harness, row, None);
		pointer(harness, row, Some(true));
		pointer(harness, row + egui::vec2(0.0, 12.0), None);
		pointer(harness, row + egui::vec2(40.0, 40.0), None);
	};
	let release = |harness: &mut Harness<'_, Window>| {
		pointer(harness, main, None);
		pointer(harness, main, Some(false));
		pointer(harness, egui::pos2(720.0, 880.0), None);
	};
	assert_eq!(slot_of(&guests(), "logo"), None, "the logo starts free");

	// Dropped on the main slot, the logo takes it.
	grab(&mut harness);
	release(&mut harness);
	assert_eq!(
		slot_of(&guests(), "logo"),
		Some(moq_scene::Slot::Main),
		"the row's drop put the logo in the main slot"
	);
	harness.get_by_label_contains("Undo: ").click();
	harness.run_steps(2);
	assert_eq!(slot_of(&guests(), "logo"), None, "undone");
	let before = guests();

	// Ibis appears mid-drag, in a tile at the logo's index.
	grab(&mut harness);
	{
		let mut state = engine.state.lock().unwrap();
		let connect = url::Url::parse("https://relay.example/show").unwrap();
		let page = url::Url::parse("https://studio.example/join/").unwrap();
		let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
		let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
		let appeared = crate::guests::appear(&mut state, &room, Some("Guests"), "Ibis").unwrap();
		assert!(appeared, "Ibis takes a tile");
	}
	// Drawn in its tile, the others glided to theirs, still held.
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the new tile settles");
	release(&mut harness);
	let after = guests();
	assert_eq!(after.len(), before.len() + 1);
	assert_eq!(
		slot_of(&after, "Ibis"),
		Some(moq_scene::Slot::Tile),
		"Ibis keeps the tile"
	);
	assert_eq!(
		slot_of(&after, "Main"),
		Some(moq_scene::Slot::Main),
		"Main keeps its slot"
	);
	assert_eq!(slot_of(&after, "logo"), None, "the logo stays free");

	drop(harness);
	quit(&engine, &runtime);
}

/// Studio mode, driven as a person does: its switch turns it on, a pad then
/// cues its scene rather than taking it, and TAKE puts it on air. NEXT's Edit
/// opens the cued scene off air on Studio, Back to live keeps it cued, and
/// the card's close clears it. Off again, a pad's menu opens a scene off air,
/// whose own layers Studio lists and selects, and the banner's Take it live
/// takes it, keeping the selection on the scene now live. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn studio_mode_cues_and_takes() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/show.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	enable(&mut harness, &[Mod::Studio]);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	// The scene on program, and the one cued.
	let staged = || {
		let state = engine.state.lock().unwrap();
		let program = state.compositor.program_scene().map(str::to_string);
		(program, state.cued().map(str::to_string))
	};
	let scenes = |program: &str, cued: Option<&str>| (Some(program.to_string()), cued.map(str::to_string));
	let selected = |harness: &mut Harness<'_, Window>| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!()
		};
		studio.selected()
	};
	assert_eq!(staged(), scenes("Main", None));

	harness.get_by_label("Studio mode").click();
	harness.run_steps(2);
	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Main", Some("Interview")), "the pad cued its scene");
	harness.get_by_label("TAKE").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None), "TAKE put it on air");

	harness.get_by_label("Main").click();
	harness.run_steps(2);
	harness.get_by_label("Edit").click();
	harness.run_steps(2);
	harness.get_by_label("Back to live").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", Some("Main")), "Studio mode keeps it cued");
	harness.get_by_label("Clear the cue").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None));

	harness.get_by_label("Studio mode").click();
	harness.run_steps(2);
	harness.get_by_label("Main").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Edit off air").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", Some("Main")), "open off air, not taken");
	// Studio lists the layers of the scene open off air.
	harness.get_all_by_label("bar").next().unwrap().click();
	harness.run_steps(2);
	assert!(selected(&mut harness), "the click selected Main's bar off air");
	harness.get_by_label("Take it live").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Main", None), "taken live");
	assert!(selected(&mut harness), "the bar stays selected, its scene now live");

	drop(harness);
	quit(&engine, &runtime);
}

/// Studio's title bar and strip as a person uses them: locked, a click or
/// double-click on the picture selects nothing and a file dropped on it is
/// refused, and unlocked a click selects and a double-click starts cropping,
/// which Escape or another double-click ends; off air with no safe
/// scene the panic key stays out of the strip; `+` adds a scene and takes it,
/// showing its quick start; and a sound's mute and gear act on that sound.
/// It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_studio_locks_adds_and_mutes() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	let show = show.replace("safe = true # the scene the panic key cuts to\n", "");
	std::fs::write("show.toml", show).unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let selected = |harness: &mut Harness<'_, Window>| {
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!()
		};
		studio.selected()
	};

	// At `at` across the picture, each click moved by its `apart` in points,
	// over `frames`: a click a frame a twentieth of a second apart, or both in
	// one slow frame. A second after any earlier click, so neither is its
	// double- or triple-click.
	let double_click = |harness: &mut Harness<'_, Window>, at: egui::Vec2, frames: usize, apart: [egui::Vec2; 2]| {
		let picture = harness.get_by_label("Canvas").rect();
		let points = apart.map(|apart| picture.min + at * picture.size() + apart);
		let start = harness.ctx.input(|input| input.time) + 1.0;
		for (frame, clicks) in points.chunks(2 / frames).enumerate() {
			let input = harness.input_mut();
			input.time = Some(start + 0.05 * (frame + 1) as f64);
			for &pos in clicks {
				input.events.push(egui::Event::PointerMoved(pos));
				for pressed in [true, false] {
					input.events.push(egui::Event::PointerButton {
						pos,
						button: egui::PointerButton::Primary,
						pressed,
						modifiers: egui::Modifiers::NONE,
					});
				}
			}
			harness.step();
		}
		// The harness's input keeps a time it is given, which would stop the clock.
		harness.input_mut().time = None;
		harness.run_steps(2);
	};
	let cropping = |harness: &Harness<'_, Window>| harness.query_by_label("Crop the layer from the top").is_some();
	let still = [egui::Vec2::ZERO; 2];
	let center = egui::vec2(0.5, 0.5);
	// The bar along the bottom, over the host filling the picture.
	let bar = egui::vec2(530.0 / 1920.0, 920.0 / 1080.0);

	harness.get_by_label("Editable").click();
	harness.run_steps(2);
	harness.get_by_label("Canvas").click();
	harness.run_steps(2);
	assert!(!selected(&mut harness), "locked, the picture selects nothing");
	double_click(&mut harness, center, 2, still);
	assert!(
		!selected(&mut harness) && !cropping(&harness),
		"locked, a double-click crops nothing"
	);
	// Dropped where the click left the pointer, on the picture.
	image::RgbaImage::new(4, 4).save("dropped.png").unwrap();
	let dropped = std::env::current_dir().unwrap().join("dropped.png");
	harness.input_mut().dropped_files.push(Arc::new(Dropped(dropped)));
	harness.run_steps(2);
	harness.get_by_label_contains("the picture is locked");
	assert!(harness.query_by_label_contains("Reading").is_none(), "nothing is read");
	harness.get_by_label("Locked").click();
	harness.run_steps(2);
	harness.get_by_label("Canvas").click();
	harness.run_steps(2);
	assert!(selected(&mut harness), "editable, a click selects the layer under it");
	assert!(!cropping(&harness));
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(!selected(&mut harness));
	// Its first click selects the bar, opening the inspector beside the
	// picture, which moves it out from under the pointer before the second.
	double_click(&mut harness, bar, 2, still);
	assert!(
		cropping(&harness) && selected(&mut harness),
		"a double-click starts cropping the layer"
	);
	let picture = harness.get_by_label("Canvas").rect();
	let top = harness.get_by_label("Crop the layer from the top").rect().center().y;
	assert!(
		top > picture.center().y,
		"it crops the bar the first click selected, not the host the second landed on"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(
		!cropping(&harness) && selected(&mut harness),
		"Escape ends the crop first"
	);
	double_click(&mut harness, bar, 2, still);
	double_click(&mut harness, bar, 2, still);
	assert!(
		!cropping(&harness) && selected(&mut harness),
		"a second double-click ends it"
	);
	// With the host selected, a slow frame holding both clicks on the bar.
	harness.get_by_label("Canvas").click();
	harness.run_steps(3);
	double_click(&mut harness, bar, 1, still);
	let picture = harness.get_by_label("Canvas").rect();
	let top = harness.get_by_label("Crop the layer from the top").rect().center().y;
	assert!(
		top > picture.center().y,
		"one frame's double-click crops the bar it is on"
	);
	// With nothing selected, a click just above the picture and one just
	// inside its top edge, which egui pairs, select the host as one click.
	harness.key_press(egui::Key::Escape);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(!selected(&mut harness));
	double_click(
		&mut harness,
		egui::vec2(0.5, 0.0),
		2,
		[egui::vec2(0.0, -3.0), egui::vec2(0.0, 2.0)],
	);
	assert!(
		selected(&mut harness) && !cropping(&harness),
		"a click paired with one off the picture selects without cropping"
	);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(!selected(&mut harness));
	double_click(
		&mut harness,
		egui::vec2(0.5, 0.0),
		1,
		[egui::vec2(0.0, -3.0), egui::vec2(0.0, 2.0)],
	);
	assert!(
		selected(&mut harness) && !cropping(&harness),
		"so do both in one slow frame"
	);
	// The host now selected, whose edges reach past the picture.
	double_click(
		&mut harness,
		egui::vec2(0.5, 0.0),
		1,
		[egui::vec2(0.0, -3.0), egui::vec2(0.0, 2.0)],
	);
	assert!(
		selected(&mut harness) && !cropping(&harness),
		"or with the layer already selected"
	);

	assert!(harness.query_by_label("Panic").is_none(), "no safe scene off air");
	harness.get_by_label("Add a scene").click();
	harness.run_steps(2);
	let program = engine
		.state
		.lock()
		.unwrap()
		.compositor
		.program_scene()
		.map(str::to_string);
	assert_eq!(program.as_deref(), Some("Scene 4"), "+ took the new scene");
	assert!(harness.query_by_label("Screen").is_some(), "its quick start shows");

	harness.get_by_label("Mute guest").click();
	harness.run_steps(2);
	let muted = |name: &str| {
		let state = engine.state.lock().unwrap();
		state.collection().audio.get(name).is_some_and(|audio| audio.muted)
	};
	assert!(muted("guest") && !muted("caller"), "the row's mute is its sound's");
	harness.get_by_label("guest options").click();
	harness.run_steps(2);
	harness.get_by_label("Monitor in headphones");
	harness.get_by_label("guest delay");

	drop(harness);
	quit(&engine, &runtime);
}

/// Add's Guests tab, in a room with a guest in a scene and heard, and one
/// waiting: a typed name makes a link shown as a code, hidden again; a card
/// places its guest in one click and sends their sound to the stream, which
/// the Hear button does alone; a menu makes a new link and removes the guest
/// from every scene. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn add_invites_and_places_guests() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let mut text = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	for guest in ["Robin", "Sam"] {
		text.push_str(&format!(
			"\n[sources.{guest}]\nkind = \"broadcast\"\npath = \"room/{guest}/camera.hang\"\n"
		));
	}
	std::fs::write("show.toml", text).unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	let connect = url::Url::parse("https://relay.example/show").unwrap();
	let page = url::Url::parse("https://join.studio.moq.dev/").unwrap();
	let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
	let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
	engine.join(Some(crate::guests::Joining::offline(room))).unwrap();
	let room = engine.room().expect("the room joined");
	for guest in ["Robin", "Sam"] {
		let path = moq_net::Path::new(&format!("{guest}/camera.hang")).unwrap();
		room.observe(&path, true, &moq_scene::Collection::new(), &moq_scene::History::new());
	}
	let heard = |name: &str| {
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		collection.heard(collection.program_audio().unwrap()).contains(name)
	};
	let shown = |name: &str| {
		let state = engine.state.lock().unwrap();
		let scenes = &state.collection().scenes;
		scenes
			.iter()
			.any(|scene| scene.items.iter().any(|item| item.source == name))
	};
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.open_add(AddTab::Guests);
	harness.run_steps(2);
	assert!(!heard("Sam") && !shown("Sam"), "a guest waits until placed");

	// A name makes a link, as a code to scan, until it is put away.
	harness.get_by_role(Role::TextInput).click();
	harness.run_steps(2);
	harness.get_by_role(Role::TextInput).type_text("Alex");
	harness.run_steps(2);
	harness.get_by_label("Invite").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Invite Alex").is_some(), "the link shows");
	assert!(
		harness.query_by_label("Add Alex").is_some(),
		"Alex has a card, not here yet"
	);
	harness.get_by_label("Hide link").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Invite Alex").is_none(), "the link is put away");
	harness.get_by_label("Add Robin").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("New link").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Invite Robin").is_some(), "a new link shows");

	// The Hear button sends the sound alone, as placing does.
	harness.get_by_label("Hear Sam").click();
	harness.run_steps(2);
	assert!(heard("Sam") && !shown("Sam"), "heard, though in no scene");
	assert!(harness.query_by_label("Hear Sam").is_none(), "nothing left to hear");
	assert!(harness.query_by_label("Hear Robin").is_some(), "only theirs was sent");
	harness.get_by_label("Add Sam").click();
	harness.run_steps(2);
	assert!(shown("Sam"), "a click places them in the edited scene");

	// Removed from every scene, and no longer heard.
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("the studio is open")
	};
	studio.open_add(AddTab::Guests);
	harness.run_steps(2);
	harness.get_by_label("Add Sam").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Remove from scenes").click();
	harness.run_steps(2);
	assert!(!shown("Sam") && !heard("Sam"), "they leave the picture and the mix");
	assert!(room.participants()["Sam"].removed, "and their token stops renewing");

	drop(harness);
	quit(&engine, &runtime);
}

/// While a mod is off, its feature is held back where no page shows it: a
/// key for it does nothing, a layer's menu offers no slot, a guest is not
/// placed on their own, though the settings ask, and a guest waiting to be
/// placed by hand, in no scene, does not switch Audio scenes & routes on.
/// It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_mod_off_holds_its_feature_back() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	// The only source in no scene: a guest who joined and waits.
	let show = show.replacen(
		"[sources.highlight]\nkind = \"color\"\ncolor = \"#f59e0b\"\n",
		"[sources.waiting]\nkind = \"broadcast\"\npath = \"room/waiting\"\n",
		1,
	);
	assert!(show.contains("room/waiting"));
	std::fs::write("show.toml", show).unwrap();
	std::fs::write("show.stream.toml", "guests_appear = true\n").unwrap();
	let app = start(launch(&gpu), stream(None), &runtime);
	let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
	harness.step();
	let engine = started.get().expect("the studio started").clone();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	let controls = studio.controls();
	studio.build(Side::Layers, Inspect::Layer, None);
	harness.run_steps(2);

	controls.press(&crate::controls::Action::SaveClip);
	let saved = controls.fired().expect("the key says how it went");
	assert!(
		saved.as_ref().is_err_and(|why| why.contains("Replay & review is off")),
		"{saved:?}"
	);
	assert!(!engine.guests_appear(), "with no slots to place them in");
	let offered = |harness: &mut Harness<'_, Window>| {
		harness.get_all_by_label("host").next().unwrap().click_secondary();
		harness.run_steps(2);
		let offered = harness.query_by_label("Put in a tile").is_some();
		harness.key_press(egui::Key::Escape);
		harness.run_steps(2);
		offered
	};
	assert!(!offered(&mut harness), "no slot in the layer's menu");
	assert!(
		crate::controls::sounds(&engine.state.lock().unwrap(), "waiting"),
		"the waiting guest makes a sound"
	);
	assert!(
		!controls.mods.on(Mod::Audio),
		"a guest waiting to be placed is not left out"
	);

	// Added from Add, where a guest waits for a scene, they are heard on the
	// stream.
	let tab = match engine.room() {
		Some(_) => AddTab::Guests,
		None => AddTab::Devices,
	};
	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("the studio is open")
	};
	studio.open_add(tab);
	harness.run_steps(2);
	harness.get_by_label("Add waiting").click();
	harness.run_steps(2);
	{
		let state = engine.state.lock().unwrap();
		let collection = state.collection();
		let program = collection.program_audio().unwrap();
		assert!(collection.heard(program).contains("waiting"), "heard once placed");
	}
	assert!(!controls.mods.on(Mod::Audio));

	enable(&mut harness, &[Mod::Layout]);
	assert!(engine.guests_appear(), "as the settings ask");
	assert!(offered(&mut harness));
	drop(harness);
	quit(&engine, &runtime);
}

/// Switching a mod off in Settings, Mods turns its feature off as one edit,
/// which undo takes back, switching the mod on again with it: on a show
/// using each, on a real engine. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn a_mod_switched_off_turns_its_feature_off() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	let read = |file: &str| std::fs::read_to_string(fixture.join("shots").join(file)).unwrap();
	// Sounds the stream leaves out: music heard only in the headphones, and
	// a microphone sent nowhere.
	let mix = read("mix.toml")
		.replacen(
			r#"sounds = ["caller", "guest", "mic", "music"]"#,
			r#"sounds = ["caller", "guest"]"#,
			1,
		)
		.replacen(r#"sounds = ["caller"]"#, r#"sounds = ["caller", "music"]"#, 1)
		.replacen(r#"sounds = ["guest", "mic"]"#, r#"sounds = ["guest"]"#, 1);
	// One mix, which leaves the microphone out.
	let one = read("mix.toml");
	let one = one[..one.find("[[audio_scenes]]\nname = \"Headphones\"").unwrap()].replacen(
		r#"sounds = ["caller", "guest", "mic", "music"]"#,
		r#"sounds = ["caller", "guest", "music"]"#,
		1,
	);
	// A tile whose role nothing fills, which its arrangement closes.
	let slots = read("slots.toml")
		.replacen(
			"[sources.game]",
			"[sources.Empty]\nkind = \"role\"\n\n[sources.game]",
			1,
		)
		.replacen(
			r#"{ source = "Dana", slot = "tile" },"#,
			r#"{ source = "Dana", slot = "tile" }, { source = "Empty", slot = "tile" },"#,
			1,
		);
	// Going out in portrait too, where keeping clear leaves the portrait
	// picture be.
	let keep = read("keep.toml").replacen("version = 1\n", "version = 1\nportrait = true\n", 1);
	// A guest shown through a role, their sound sent nowhere.
	let role = read("show.toml")
		.replacen(
			"[sources.caller]",
			"[sources.Guest]\nkind = \"role\"\nsource = \"guest\"\n\n[sources.caller]",
			1,
		)
		.replacen(r#"{ source = "guest", rect"#, r#"{ source = "Guest", rect"#, 1)
		.replacen(r#"sounds = ["caller", "guest"]"#, r#"sounds = ["caller"]"#, 1);
	// Slots in both pictures, one tile shown only in portrait.
	let dual_slots = read("dual.toml").replacen(
		r#"{ source = "Dana", slot = "tile" },"#,
		r#"{ source = "Dana", slot = "tile", format = "portrait" },"#,
		1,
	);
	let every: &[&str] = &["caller", "guest", "mic", "music"];
	let shows: [(String, Mod, &[&str]); 9] = [
		(mix, Mod::Audio, every),
		(one, Mod::Audio, every),
		(role, Mod::Audio, &["caller", "guest"]),
		(slots, Mod::Layout, &[]),
		(keep, Mod::Layout, &[]),
		(dual_slots, Mod::Layout, &[]),
		(read("dual.toml"), Mod::Formats, &[]),
		(read("portrait.toml"), Mod::Formats, &[]),
		(read("published.toml"), Mod::Published, &[]),
	];
	for (show, which, sounds) in shows {
		std::fs::write("show.toml", show).unwrap();
		let app = start(launch(&gpu), stream(None), &runtime);
		let (mut harness, started, _) = window(app, &gpu, &runtime, SIZE, recent::Recent::default());
		harness.step();
		let engine = started.get().expect("the studio started").clone();
		runtime
			.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
			.expect("the sources and pictures settle");
		let App::Studio(studio) = &mut harness.state_mut().app else {
			unreachable!("the studio is open")
		};
		let controls = studio.controls();
		studio.open(Setting::Mods);
		harness.run_steps(2);
		assert!(controls.mods.on(which), "{which:?} switched on, as the show uses it");
		if which == Mod::Layout {
			// A frame behind a reorder, held there by the lock, where each slot
			// is drawn is not known yet: switching off refuses rather than
			// saving a guess.
			let mut state = engine.state.lock().unwrap();
			let slotted = state
				.collection()
				.scenes
				.iter()
				.find(|scene| scene.items.len() > 1 && scene.items.iter().any(|item| item.slot.is_some()));
			if let Some(scene) = slotted.map(|scene| moq_scene::Layer::scene(&scene.name)) {
				let mut draft = state.collection().draft("Reorder");
				draft.move_item(&scene, 0, 1).unwrap();
				state.commit(draft, None).unwrap();
				let err = crate::ui::layout_off(&mut state, "Switch off".into()).unwrap_err();
				assert!(err.to_string().contains("still settling"), "{err:#}");
				state.undo().unwrap();
			}
		}
		let before = engine.state.lock().unwrap().collection().clone();

		harness
			.get_by_role_and_label(Role::CheckBox, which.name())
			.scroll_to_me();
		harness.run_steps(2);
		harness
			.get_by_role_and_label(Role::CheckBox, which.name())
			.click_accesskit();
		harness.run_steps(4);
		assert!(!controls.mods.on(which), "{which:?} switched off");
		let after = engine.state.lock().unwrap().collection().clone();
		match which {
			Mod::Audio => {
				let [program] = after.audio_scenes.as_slice() else {
					panic!("one audio scene: {:?}", after.audio_scenes)
				};
				assert!(program.program && !program.recorded && program.scenes.is_empty());
				let heard: std::collections::BTreeSet<&str> = before.heard(before.program_audio().unwrap());
				let all: Vec<&str> = program.sounds.iter().map(String::as_str).collect();
				assert_eq!(all, sounds, "every sound, those left out too");
				let muted = |name: &str| after.audio.get(name).is_some_and(|audio| audio.muted);
				for left in all.iter().filter(|name| !heard.contains(*name)) {
					assert!(muted(left), "{left} muted, as the stream left it out");
				}
				assert!(
					heard
						.iter()
						.all(|name| muted(name) == before.audio.get(*name).is_some_and(|a| a.muted))
				);
			}
			Mod::Layout => {
				assert_eq!(after.scenes.len(), before.scenes.len());
				for (scene, was) in after.scenes.iter().zip(&before.scenes) {
					assert_eq!(scene.items.len(), was.items.len(), "{}", scene.name);
					for (item, was) in scene.items.iter().zip(&was.items) {
						assert!(
							item.slot.is_none() && item.keep_clear.is_none(),
							"{} set free",
							item.source
						);
						if item.source == "Empty" {
							assert!(!item.visible, "a closed tile stays hidden");
						} else if was.slot.is_some() {
							assert!(!item.rect.is_empty(), "{} kept where it was drawn", item.source);
							assert!(item.visible, "{} still shown", item.source);
						}
						if was.keep_clear.is_some() {
							assert_eq!(
								after.placement(item, moq_scene::Format::Portrait),
								before.placement(was, moq_scene::Format::Portrait),
								"{} keeps its portrait place",
								item.source
							);
						}
					}
				}
			}
			Mod::Formats => {
				assert!(!after.portrait, "landscape only");
				assert!(after.canvas.width > after.canvas.height, "a landscape canvas");
			}
			Mod::Published => {
				assert!(after.published.is_empty(), "no source published independently");
				assert_eq!(after.scenes.len(), before.scenes.len());
				assert!(
					after
						.scenes
						.iter()
						.all(|scene| !scene.published && scene.size.is_none())
				);
			}
			_ => unreachable!(),
		}

		// Under Settings, the title bar's undo.
		harness.key_press(egui::Key::Escape);
		harness.run_steps(2);
		harness.get_by_label_contains("Undo: Switch off").click();
		harness.run_steps(4);
		assert_eq!(
			*engine.state.lock().unwrap().collection(),
			before,
			"undo brings it back"
		);
		assert!(controls.mods.on(which), "{which:?} back on with it");

		drop(harness);
		quit(&engine, &runtime);
	}
}

/// The show's menu opens the multiview. On it, a click on
/// a scene's tile takes it, or cues it in Studio mode, a source's menu leaves
/// it out, and the sources' popover picks one: on a real engine. It draws no
/// view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn the_multiview_takes_cues_and_picks() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	// A scene of its own size, which the program refuses.
	let published = std::fs::read_to_string(fixture.join("shots/published.toml")).unwrap();
	let vertical = &published[published.find("[[scenes]]\nname = \"Vertical\"").unwrap()..];
	let show = std::fs::read_to_string(fixture.join("shots/show.toml")).unwrap();
	std::fs::write("show.toml", format!("{show}\n{vertical}")).unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	enable(&mut harness, &[Mod::Multiview, Mod::Studio]);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let staged = || {
		let state = engine.state.lock().unwrap();
		let program = state.compositor.program_scene().map(str::to_string);
		(program, state.cued().map(str::to_string))
	};
	let scenes = |program: &str, cued: Option<&str>| (Some(program.to_string()), cued.map(str::to_string));

	// Embedded in the window here, as egui draws a viewport where it opens no
	// native windows.
	assert!(harness.query_by_label("Sources shown").is_none());
	harness.get_by_label("The show").click();
	harness.run_steps(2);
	harness.get_by_label("Open the multiview").click();
	harness.run_steps(4);
	assert!(harness.query_by_label("Sources shown").is_some(), "opened");

	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("the studio is open")
	};
	studio.multiview_alone(&[]);
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Main", None));
	harness.get_by_label("Interview").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None), "the tile took its scene");
	harness.get_by_label("Vertical").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", None), "a refused scene stays off");

	let App::Studio(studio) = &mut harness.state_mut().app else {
		unreachable!("the studio is open")
	};
	studio.studio_mode(true);
	harness.run_steps(2);
	harness.get_by_label("Break").click();
	harness.run_steps(2);
	assert_eq!(staged(), scenes("Interview", Some("Break")), "the tile cued its scene");

	assert!(harness.query_by_label("guest").is_some(), "a guest is shown by default");
	harness.get_by_label("guest").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Leave out of the multiview").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("guest").is_none(), "left out");
	harness.get_by_label("Sources shown").click();
	harness.run_steps(2);
	harness.get_by_label("host").click();
	harness.run_steps(2);
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);
	assert!(harness.query_by_label("host").is_some(), "picked");

	drop(harness);
	quit(&engine, &runtime);
}

/// MIX's send dots, an opened strip's switches, a mute, and the audio scene
/// tags edit the show as a person clicking them does, each an edit undo
/// names: on the MIX views' show, on a real engine. It draws no view.
#[test]
#[ignore = "draws on a GPU; run with `just shots`"]
fn mix_sends_and_mutes() {
	let (_show, runtime, gpu) = lavapipe();
	let _entered = runtime.enter();
	let fixture = Path::new(env!("CARGO_MANIFEST_DIR"));
	std::fs::copy(fixture.join("shots/mix.toml"), "show.toml").unwrap();
	let (mut harness, started, _) = window(
		start(launch(&gpu), stream(None), &runtime),
		&gpu,
		&runtime,
		SIZE,
		Default::default(),
	);
	harness.step();
	enable(&mut harness, &[Mod::Audio]);
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	studio.mix(MixView::Strips, None);
	let engine = started.get().expect("the studio started").clone();
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	let scene = |name: &str| {
		let state = engine.state.lock().unwrap();
		state.collection().audio_scene(name).cloned().expect("the audio scene")
	};
	let undo = || {
		let state = engine.state.lock().unwrap();
		let history = state.history();
		let at = history.position().checked_sub(1)?;
		history.entries().nth(at).map(|edit| edit.label.clone())
	};

	// A dot sends a sound in, and again takes it out.
	assert!(!scene("Recording").sounds.contains("music"));
	harness.get_by_label("Send music to Recording").click();
	harness.run_steps(2);
	assert!(scene("Recording").sounds.contains("music"), "sent");
	assert_eq!(undo().as_deref(), Some("Put `music` in `Recording`"));
	harness.get_by_label("Send music to Recording").click();
	harness.run_steps(2);
	assert!(!scene("Recording").sounds.contains("music"), "taken out");

	// A sound heard only through Stream, its dot a ring, is sent straight in.
	harness.get_by_label("Send guest to Headphones").click();
	harness.run_steps(2);
	assert!(scene("Headphones").sounds.contains("guest"));

	// An audio scene sends into another, but never into one it holds.
	harness.get_by_label("Send Recording to Headphones").click();
	harness.run_steps(2);
	assert!(scene("Headphones").scenes.contains("Recording"));
	assert!(
		harness
			.get_by_label("Send Headphones to Stream")
			.accesskit_node()
			.is_disabled(),
		"Headphones holds Stream"
	);

	// A strip's name opens it to its controls, and again closes it.
	assert!(harness.query_by_label("Delay").is_none());
	harness.get_by_label("Open mic").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Delay").is_some(), "opened");
	harness.get_by_label("Close mic").click();
	harness.run_steps(2);
	assert!(harness.query_by_label("Delay").is_none(), "closed");

	// The mute, and putting another audio scene on the stream.
	harness
		.get_all(by().label_contains("Mute "))
		.next()
		.expect("a mute")
		.click();
	harness.run_steps(2);
	assert_eq!(undo().as_deref(), Some("Mute `guest`"));
	harness
		.get_all_by_label("Put on stream")
		.next()
		.expect("one off the stream")
		.click();
	harness.run_steps(2);
	assert!(scene("Headphones").program && !scene("Stream").program);

	// A new audio scene, with nothing in it.
	harness.get_by_label("Add an audio scene").click();
	harness.run_steps(2);
	assert_eq!(scene("Audio scene 4").sounds.len(), 0);

	// Its strip's menu renames it in a sheet, moves it, and removes it.
	let names = || {
		let state = engine.state.lock().unwrap();
		let scenes = &state.collection().audio_scenes;
		scenes.iter().map(|scene| scene.name.clone()).collect::<Vec<_>>()
	};
	harness.get_by_label("Audio scene Audio scene 4").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Rename").click();
	harness.run_steps(2);
	harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
	harness.get_by_role(Role::TextInput).type_text("Talkback");
	harness.run_steps(2);
	harness.get_by_label("Rename").click();
	harness.run_steps(2);
	assert_eq!(names(), ["Stream", "Headphones", "Recording", "Talkback"]);
	assert_eq!(
		undo().as_deref(),
		Some("Rename audio scene `Audio scene 4` to `Talkback`")
	);
	harness.get_by_label("Audio scene Talkback").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Move left").click();
	harness.run_steps(2);
	assert_eq!(names(), ["Stream", "Headphones", "Talkback", "Recording"]);
	harness.get_by_label("Audio scene Talkback").click_secondary();
	harness.run_steps(2);
	harness.get_by_label("Remove").click();
	harness.run_steps(2);
	assert_eq!(names(), ["Stream", "Headphones", "Recording"]);
	// The one the stream hears cannot be removed.
	harness.get_by_label("Audio scene Headphones").click_secondary();
	harness.run_steps(2);
	assert!(harness.get_by_label("Remove").accesskit_node().is_disabled());
	harness.key_press(egui::Key::Escape);
	harness.run_steps(2);

	drop(harness);
	quit(&engine, &runtime);
}

/// Draw what `open` opens on a fresh engine, from a scene file in `shows`.
fn shoot(open: &Open, gpu: &engine::Gpu, runtime: &tokio::runtime::Runtime, shows: &Path) -> image::RgbaImage {
	if let &Open::Kit { sheet } = open {
		return gallery(gpu, sheet);
	}
	let app = match open {
		Open::Kit { .. } => unreachable!("drawn without an engine"),
		Open::Studio { show, view, relay, .. } => {
			let mut text = match view {
				View::FirstLaunch | View::Refused => show::first(moq_scene::Canvas::default(), true)
					.unwrap()
					.to_toml()
					.unwrap(),
				_ => std::fs::read_to_string(shows.join(show)).unwrap(),
			};
			if let View::KeepClear { dual: true } = view {
				text = text.replacen("version = 1\n", "version = 1\nportrait = true\n", 1);
			}
			if let View::AddGuests = view {
				text.push_str(
					"\n[sources.Robin]\nkind = \"broadcast\"\npath = \"room/Robin/camera.hang\"\n\
					 \n[sources.Sam]\nkind = \"broadcast\"\npath = \"room/Sam/camera.hang\"\n\
					 \n[sources.\"Sam screen\"]\nkind = \"broadcast\"\npath = \"room/Sam/screen.hang\"\n\
					 \n[[scenes]]\nname = \"Guest duo\"\nitems = [\n    \
					 { source = \"Robin\", rect = { x = 0, y = 0, width = 1920, height = 1080 } },\n]\n",
				);
			}
			if let View::Review { replays: true, .. } = view {
				text.push_str("\n[sources.replay]\nkind = \"replay\"\n\n[sources.slowmo]\nkind = \"replay\"\n");
			}
			std::fs::write("show.toml", text).unwrap();
			if let View::GuestsFailed = view {
				std::fs::write("show.stream.toml", "room = \"guests\"\nroom_key = \"guests.jwk\"\n").unwrap();
			}
			let relay = relay.then(|| Relay::start(runtime));
			start(launch(gpu), stream(relay.as_ref()), runtime)
		}
	};
	let size = match open {
		Open::Studio { size, .. } => *size,
		_ => SIZE,
	};
	let recent = match open {
		Open::Studio {
			view: View::ShowMenu, ..
		} => recent_shows(),
		_ => recent::Recent::default(),
	};
	let (mut harness, started, _) = window(app, gpu, runtime, size, recent);
	// What a view picks stays until the window has read it.
	let picks = tempfile::tempdir().unwrap();

	let Open::Studio { view, .. } = open else {
		unreachable!("drawn without an engine")
	};
	harness.step();
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio starts on its first frame")
	};
	let controls = studio.controls();
	for which in view.mods() {
		studio.switch(which, true);
	}
	match view {
		View::Page(page) => studio.turn(*page),
		View::FirstLaunch => studio.seed_devices(first_devices()),
		View::Refused => {
			studio.seed_devices(first_devices());
			let pdf = picks.path().join("Rider.pdf");
			std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
			studio.pick_file(pdf);
		}
		View::Locked => {
			studio.lock(true);
			studio.rail(false);
		}
		View::SoundOptions(name) => studio.sound_options(name),
		View::Mix(view, opened) => studio.mix(*view, *opened),
		View::MixMenu => studio.mix(MixView::Strips, None),
		View::Build { side, inspect, index } => studio.build(*side, *inspect, *index),
		View::Add(tab) => {
			studio.seed_devices(add_devices());
			studio.open_add(*tab);
		}
		View::AddGuests => {
			studio.seed_devices(add_devices());
			studio.open_add(AddTab::Guests);
		}
		View::KeepClear { .. } => {
			studio.build(Side::Layers, Inspect::Layer, Some(0));
			studio.more();
		}
		View::Menu => studio.build(Side::Layers, Inspect::Layer, Some(1)),
		View::More | View::Published => {
			studio.build(Side::Layers, Inspect::Layer, Some(1));
			studio.more();
		}
		View::Setting(setting) | View::OnAirSetting(setting) => studio.open(*setting),
		View::ModsOn | View::ModsAt(_) => studio.open(Setting::Mods),
		View::Restart | View::CustomCanvas => studio.open(Setting::Show),
		View::GuestsFailed => studio.open(Setting::Guests),
		View::Paired => studio.open(Setting::Remote),
		View::Graph => studio.open_graph(),
		View::Health => studio.details(),
		View::Hover
		| View::Going
		| View::History
		| View::PadHover
		| View::RemovePrompt
		| View::ShowMenu
		| View::ShowMenuLive
		| View::OnAir
		| View::EndConfirm
		| View::OnAirHealth => {}
		View::Cued => {
			studio.studio_mode(true);
			studio.cue("Interview");
		}
		View::OffAir(scene) => studio.edit_off_air(scene),
		// The game's window, the back one of three over the black.
		View::Dark => studio.build(Side::Layers, Inspect::Layer, Some(2)),
		View::Stalled => studio.build(Side::Layers, Inspect::Layer, Some(1)),
		View::Joined => studio.build(Side::Layers, Inspect::Layer, None),
		View::Multiview { live, .. } => {
			studio.multiview_alone(&["host", "highlight"]);
			if *live {
				studio.studio_mode(true);
				studio.cue("Interview");
			}
		}
		View::Drop(_) => studio.build(Side::Layers, Inspect::Layer, None),
		View::Dual {
			format,
			index,
			cued,
			off_air,
		} => {
			studio.build(Side::Layers, Inspect::Layer, *index);
			studio.edit_format(*format);
			if let Some(cued) = cued {
				studio.studio_mode(true);
				studio.cue(cued);
			}
			if let Some(scene) = off_air {
				studio.edit_off_air(scene);
			}
		}
		View::DualAdded => {
			studio.build(Side::Layers, Inspect::Layer, None);
			studio.edit_format(moq_scene::Format::Portrait);
			studio.edit_off_air("Break");
			let source = Source::Clone { source: "host".into() };
			studio.add_card("Person", source);
		}
		View::Palette(_) => {
			let key = |code, name: &str| hotkeys::Binding {
				modifiers: egui::Modifiers::NONE,
				key: winit::keyboard::PhysicalKey::Code(code),
				name: winit::keyboard::Key::Character(name.into()),
			};
			use crate::controls::Action;
			use winit::keyboard::KeyCode;
			studio.bind(&Action::Panic, key(KeyCode::F9, "F9"));
			studio.bind(&Action::Scene("Main".into()), key(KeyCode::F1, "F1"));
			studio.bind(&Action::Cut, key(KeyCode::Numpad0, "0"));
		}
		View::Review { picked, markers, .. } => {
			studio.turn(Page::Review);
			studio.seed_shelf(shelf());
			let secs = Duration::from_secs;
			let now = kio::clock::Instant::now();
			if *picked {
				studio.seed_past(past(now), now, (secs(14), secs(6)), secs(9));
			}
			if *markers {
				let dir = controls.engine.recorder.dir();
				for ago in [secs(3 * 60 + 5), secs(22), secs(4)] {
					controls.markers.mark(&dir, now.saturating_sub(ago)).unwrap();
				}
				controls.markers.rename(&dir, 2, "The comeback").unwrap();
			}
		}
	}
	let engine = started.get().expect("the studio started").clone();
	if let View::Dark = view {
		let dark = Duration::from_secs(12);
		engine.state.lock().unwrap().dark.insert("game".into(), dark);
	}
	if let View::Stalled | View::Multiview { stalled: true, .. } = view {
		let stalled = Duration::from_secs(4);
		engine.state.lock().unwrap().stalled.insert("host".into(), stalled);
	}
	if let View::Paired = view {
		let connect = url::Url::parse("https://relay.example/show").unwrap();
		let page = url::Url::parse("https://join.studio.moq.dev/").unwrap();
		let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
		let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
		room.pair("tablet").unwrap();
		room.unpair("tablet").unwrap();
		engine.join(Some(crate::guests::Joining::offline(room))).unwrap();
	}
	if let View::AddGuests = view {
		let connect = url::Url::parse("https://relay.example/show").unwrap();
		let page = url::Url::parse("https://join.studio.moq.dev/").unwrap();
		let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
		let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
		engine.join(Some(crate::guests::Joining::offline(room))).unwrap();
		let room = engine.room().expect("the room joined");
		let nothing = (&moq_scene::Collection::new(), &moq_scene::History::new());
		for broadcast in ["Robin/camera.hang", "Sam/camera.hang", "Sam/screen.hang"] {
			room.observe(&moq_net::Path::new(broadcast).unwrap(), true, nothing.0, nothing.1);
		}
		// Robin is heard once in a scene, as placing them does; Sam is not.
		let mut state = engine.state.lock().unwrap();
		let mut draft = state.collection().draft("Hear `Robin`");
		draft.hear("Robin").unwrap();
		state.commit(draft, None).unwrap();
	}
	if let View::Joined = view {
		let mut state = engine.state.lock().unwrap();
		let connect = url::Url::parse("https://relay.example/show").unwrap();
		let page = url::Url::parse("https://studio.example/join/").unwrap();
		let key = crate::guests::tests::key("show/room", &["**"], &["**"]);
		let room = crate::guests::Room::new(&connect, "room", key, page, crate::guests::tests::EXPIRY).unwrap();
		let appeared = crate::guests::appear(&mut state, &room, Some("Guests"), "Ibis").unwrap();
		assert!(appeared, "Ibis takes a tile");
	}
	if let View::OnAir
	| View::EndConfirm
	| View::OnAirHealth
	| View::OnAirSetting(_)
	| View::Restart
	| View::ShowMenuLive
	| View::Multiview { live: true, .. } = view
	{
		controls.go_live().expect("going live starts");
		runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while engine.on_air().live().is_none() {
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.unwrap_or_else(|_| panic!("the studio goes live: {:?}", controls.refused.lock().unwrap()));
	}
	runtime
		.block_on(kio::clock::timeout(SETTLE, settle(&mut harness, &engine)))
		.expect("the sources and pictures settle");
	match view {
		View::Hover => hover(&mut harness),
		// Pressed once the window settled, which the check, started over,
		// would otherwise pass in, going live.
		View::Going => {
			let App::Studio(studio) = &mut harness.state_mut().app else {
				panic!("the studio stays open")
			};
			studio.going();
			harness.run_steps(4);
		}
		View::EndConfirm => {
			harness.get_by_label("Live: end the stream").click();
			harness.run_steps(4);
		}
		View::Restart => {
			harness.get_by_label("720p").click();
			harness.run_steps(4);
		}
		View::Published => {
			harness.get_by_label("Priority").scroll_to_me();
			harness.run_steps(4);
		}
		View::ModsAt(row) => {
			harness.get_by_label(row).scroll_to_me();
			harness.run_steps(4);
		}
		View::CustomCanvas => {
			harness.get_all_by_label("Custom").next().unwrap().click();
			harness.run_steps(4);
		}
		View::AddGuests => {
			harness.get_by_role(Role::TextInput).click();
			harness.run_steps(2);
			harness.get_by_role(Role::TextInput).type_text("Alex");
			harness.run_steps(2);
			harness.get_by_label("Invite").click();
			harness.run_steps(4);
		}
		View::Paired => {
			harness.get_by_role(Role::TextInput).click();
			harness.run_steps(2);
			harness.get_by_role(Role::TextInput).type_text("Phone");
			harness.run_steps(2);
			harness.get_by_label("Pair a phone").click();
			harness.run_steps(4);
		}
		View::GuestsFailed => runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				while harness.query_by_label_contains("Guests cannot join").is_none() {
					harness.step();
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
			}))
			.expect("the room fails to join at launch"),
		View::KeepClear { .. } => runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				loop {
					let landed = {
						let state = engine.state.lock().unwrap();
						let item = &state.collection().scenes[0].items[1];
						let placement = state.compositor.placement(item.id);
						placement.is_some_and(|placement| {
							let slot = placement
								.slot
								.and_then(|slot| placement.slots.iter().find(|(fits, _)| *fits == slot));
							placement.left.is_some() && slot.is_some_and(|(_, rect)| *rect == placement.rect)
						})
					};
					if landed {
						break;
					}
					harness.step();
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
				harness.run_steps(2);
			}))
			.expect("the facecam moves off the minimap and lands"),
		View::OnAir if engine.state.lock().unwrap().collection().portrait => runtime
			.block_on(kio::clock::timeout(SETTLE, async {
				// Each picture's rate shows once health hears what its video
				// sent.
				let sending = |name: Option<&str>| {
					let report = engine.health.lock().unwrap();
					let broadcast = report.broadcasts.iter().find(|found| found.name.as_deref() == name);
					broadcast.is_some_and(|broadcast| {
						let mut tracks = broadcast.tracks.iter();
						tracks.any(|track| track.label.starts_with("video") && track.sent.is_some())
					})
				};
				while !(sending(None) && sending(Some(moq_scene::PORTRAIT))) {
					harness.step();
					kio::clock::sleep(Duration::from_millis(20)).await;
				}
				harness.run_steps(2);
			}))
			.expect("both pictures send"),
		View::OnAirHealth => {
			let pill = ["Healthy", "Starting", "Degraded", "Unhealthy"]
				.into_iter()
				.find_map(|name| harness.query_by_label(name))
				.expect("the health dot");
			pill.click();
			harness.run_steps(2);
		}
		View::MixMenu => {
			harness.get_by_label("Audio scene Headphones").click_secondary();
			harness.run_steps(4);
		}
		View::Dark => {
			// The health dot counts it once the stream is graded again.
			runtime
				.block_on(kio::clock::timeout(SETTLE, async {
					while engine.health.lock().unwrap().dark == 0 {
						kio::clock::sleep(Duration::from_millis(20)).await;
					}
				}))
				.expect("the health dot counts the dark window");
			harness.run_steps(2);
			harness.get_by_label("What happened to game").click();
			harness.run_steps(4);
		}
		View::Stalled => {
			runtime
				.block_on(kio::clock::timeout(SETTLE, async {
					while engine.health.lock().unwrap().stalled == 0 {
						kio::clock::sleep(Duration::from_millis(20)).await;
					}
				}))
				.expect("the health dot counts the stalled camera");
			harness.run_steps(2);
			harness.get_by_label("What happened to host").click();
			harness.run_steps(4);
		}
		View::Menu => {
			// The row comes before the inspector's tab of the same name.

			harness.get_all_by_label("host").next().unwrap().click_secondary();
			harness.run_steps(4);
		}
		View::ShowMenu => {
			harness.get_by_label("The show").click();
			harness.run_steps(4);
		}
		View::ShowMenuLive => {
			harness.get_by_label("The show").click();
			harness.run_steps(4);
			harness.get_by_label("New show").hover();
			// egui shows a tooltip once the pointer rests half a second.
			harness.run_steps(4);
		}
		View::Drop(drop) => drag(&mut harness, &engine, *drop),
		View::Palette(query) => {
			harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
			harness.run_steps(2);
			if !query.is_empty() {
				harness.get_by_role(Role::TextInput).type_text(query);
			}
			harness.run_steps(4);
		}
		View::PadHover => {
			harness.get_by_label("Interview").hover();
			harness.run_steps(4);
		}
		View::RemovePrompt => {
			harness.get_by_label("Break").hover();
			harness.run_steps(2);
			harness.get_by_label("Scene options").click();
			harness.run_steps(2);
			harness.get_by_label("Remove").click();
			harness.run_steps(4);
		}
		View::History => {
			harness
				.get_all(by().label_contains("Mute "))
				.next()
				.expect("a sound to mute")
				.click();
			harness.run_steps(2);
			harness.get_by_label("History").click();
			harness.run_steps(4);
		}
		_ => {}
	}
	fresh_glyphs(&mut harness);
	let image = harness.render().unwrap();
	drop(harness);
	quit(&engine, runtime);
	image
}

/// Pack egui's glyphs anew from the frame about to be drawn. They are packed
/// in the order text first appears, and a glyph's edge samples its
/// neighbour's, so a view whose text changed while it settled, such as the
/// health dot's on a show with a failed source, would otherwise draw a pixel
/// differently from run to run. New font definitions repack the atlas.
fn fresh_glyphs(harness: &mut Harness<'_, Window>) {
	let ctx = harness.ctx.clone();
	let fonts = ctx.fonts(|fonts| fonts.definitions().clone());
	let mut other = fonts.clone();
	other
		.families
		.insert(egui::FontFamily::Name("repack".into()), Vec::new());
	ctx.set_fonts(other);
	harness.step();
	ctx.set_fonts(fonts);
	harness.step();
}

/// A window on `app`, `size` large, where the studio it starts is kept, and
/// whether it was asked to close.
fn window<'a>(
	app: App,
	gpu: &engine::Gpu,
	runtime: &'a tokio::runtime::Runtime,
	size: egui::Vec2,
	recent: recent::Recent,
) -> (Harness<'a, Window>, Arc<crate::Started>, Arc<AtomicBool>) {
	let opener = show::Opener {
		gpu: gpu.clone(),
		runtime: runtime.handle().clone(),
		flags: Default::default(),
		client: Default::default(),
		quic: Default::default(),
		credentials: stream::Credentials::memory(),
	};
	let render = render_state(gpu);
	let started = Arc::<crate::Started>::default();
	let kept = started.clone();
	let closed = Arc::new(AtomicBool::new(false));
	let window_closed = closed.clone();
	let harness = Harness::builder()
		.with_size(size)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render.clone()))
		.build_eframe(move |cc| {
			crate::kit::install(&cc.egui_ctx);
			// Not the system's reduced motion, which differs between machines.
			let fx = fx::Fx::moving(&render);
			Window {
				app,
				render: render.clone(),
				opener,
				started: kept,
				recent,
				// The view's own, never the user's storage.
				shows: Some(std::path::absolute("shows").unwrap()),
				aired: None,
				keyboard: hotkeys::Keyboard::default(),
				fx,
				notice: None,
				failed: Default::default(),
				closed: window_closed,
				exiting: Default::default(),
				control: None,
			}
		});
	(harness, started, closed)
}

/// Stop `engine` as quitting does.
fn quit(engine: &engine::Engine, runtime: &tokio::runtime::Runtime) {
	// Now, while the scene file is this test's, so an edit made here is not
	// left for the autosave to lose when the engine stops.
	engine.save().expect("the show saves");
	engine.recorder.close();
	engine.stop();
	runtime.block_on(async { tokio::join!(engine.close(), engine.recorder.done()) });
}

/// Draw the kit's gallery, with a sheet over it if `sheet`.
fn gallery(gpu: &engine::Gpu, sheet: bool) -> image::RgbaImage {
	let render = render_state(gpu);
	let mut harness = Harness::builder()
		.with_size(SIZE)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render.clone()))
		.build_eframe(|cc| {
			crate::kit::install(&cc.egui_ctx);
			let fx = fx::Fx::moving(&render);
			crate::kit::gallery::Gallery::new(fx, sheet)
		});
	// Enough for the popover and the sheet to settle where they open.
	harness.run_steps(4);
	harness.render().unwrap()
}

/// A file the system drops, which the studio reads by its path.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
	fn path(&self) -> &Path {
		&self.0
	}

	fn bytes(&self) -> Result<Vec<u8>, String> {
		unreachable!("the studio reads the file itself")
	}
}

/// A file on the picture as `drop` has it, from the show's directory.
fn drag(harness: &mut Harness<'_, Window>, engine: &engine::Engine, drop: Drop) {
	// A white ring, as the mock-up's logo.
	let logo = image::RgbaImage::from_fn(400, 400, |x, y| {
		let distance = ((x as f32 - 200.0).powi(2) + (y as f32 - 200.0).powi(2)).sqrt();
		match (120.0..190.0).contains(&distance) {
			true => image::Rgba([255, 255, 255, 255]),
			false => image::Rgba([0, 0, 0, 0]),
		}
	});
	let dir = std::env::current_dir().unwrap();
	let outside = tempfile::tempdir().unwrap();
	// The drop that copies the logo in brings it from outside the show.
	match drop {
		Drop::Placed => logo.save(outside.path().join("logo.png")).unwrap(),
		_ => logo.save(dir.join("logo.png")).unwrap(),
	}
	// Boxes as a regular MP4 has them, its samples indexed in a `moov`.
	let mut mp4 = Vec::new();
	for (kind, body) in [(b"ftyp", 8), (b"moov", 32), (b"mdat", 64)] {
		mp4.extend_from_slice(&(8 + body as u32).to_be_bytes());
		mp4.extend_from_slice(kind);
		mp4.resize(mp4.len() + body, 0);
	}
	std::fs::write(dir.join("intro.mp4"), mp4).unwrap();

	// Away from every layer, or on the host's picture at the right.
	let (path, at) = match drop {
		Drop::Hover => (dir.join("logo.png"), egui::pos2(470.0, 330.0)),
		Drop::Replace => (dir.join("logo.png"), egui::pos2(985.0, 330.0)),
		Drop::Refused => (dir.join("intro.mp4"), egui::pos2(470.0, 330.0)),
		Drop::Placed => (outside.path().join("logo.png"), egui::pos2(470.0, 330.0)),
	};
	harness.input_mut().events.push(egui::Event::PointerMoved(at));
	match drop {
		Drop::Hover => harness.input_mut().hovered_files.push(egui::HoveredFile {
			path: Some(path.clone()),
			..Default::default()
		}),
		Drop::Replace | Drop::Refused | Drop::Placed => {
			harness.input_mut().dropped_files.push(Arc::new(Dropped(path.clone())))
		}
	}
	harness.run_steps(4);
	if let Drop::Placed = drop {
		// Read and copied on a thread of its own.
		let selected = |harness: &mut Harness<'_, Window>| match &harness.state().app {
			App::Studio(studio) => studio.selected(),
			_ => false,
		};
		let placed = async {
			while !selected(harness) {
				kio::clock::sleep(Duration::from_millis(20)).await;
				harness.step();
			}
		};
		tokio::runtime::Handle::current()
			.block_on(kio::clock::timeout(SETTLE, placed))
			.expect("the dropped logo is placed");
		assert_eq!(
			std::fs::read(dir.join("logo.png")).unwrap(),
			std::fs::read(&path).unwrap(),
			"copied beside the show"
		);
		// Away from the pointer, as a person moves on, and with the source's
		// picture opened and drawn.
		harness.input_mut().events.push(egui::Event::PointerGone);
		tokio::runtime::Handle::current()
			.block_on(kio::clock::timeout(SETTLE, settle(harness, engine)))
			.expect("the dropped logo's picture settles");
	}
}

/// Switch on `mods`, as their switches in Settings, Mods would.
fn enable(harness: &mut Harness<'_, Window>, mods: &[Mod]) {
	let App::Studio(studio) = &mut harness.state_mut().app else {
		panic!("the studio is open")
	};
	for which in mods {
		studio.switch(*which, true);
	}
	harness.run_steps(2);
}

/// Mute the first sound, open the health dot's popover, and rest the pointer
/// on undo until its tooltip shows.
fn hover(harness: &mut Harness<'_, Window>) {
	harness
		.get_all(by().label_contains("Mute "))
		.next()
		.expect("a sound to mute")
		.click();
	harness.run_steps(2);
	harness.get_by_label("No relay").click();
	harness.run_steps(2);
	harness.get_by_label_contains("Undo: Mute").hover();
	// egui shows a tooltip once the pointer rests half a second.
	harness.run_steps(4);
}

/// The other shows the Show menu lists: a few recent ones, beside the
/// view's directory, opened before it.
fn recent_shows() -> recent::Recent {
	let day = 24 * 3600;
	let now = recent::now();
	let shows = [
		("Friday Night Ranked", Some((3 * day, 3 * 3600 + 120))),
		("Sunday Chill", Some((6 * day, 6000))),
		("Podcast with Sam", None),
	];
	let mut recent = recent::Recent::default();
	// Oldest first, so the first is the latest opened.
	for (index, (name, live)) in shows.into_iter().enumerate().rev() {
		let path = std::path::absolute(format!("{name}.toml")).unwrap();
		std::fs::write(&path, "").unwrap();
		recent.opened(&path, now - 21 * day + (5 - index as u64) * 3600);
		if let Some((ago, lasted)) = live {
			recent.aired(&path, recent::Aired { at: now - ago, lasted });
		}
	}
	recent
}

/// The devices a first launch finds: a camera, and a screen.
fn first_devices() -> devices::Listing {
	let pick = |name: &str, source| devices::Pick {
		name: name.into(),
		source,
		size: None,
	};
	devices::Listing {
		cameras: Ok(vec![pick(
			"Sony A6400",
			Source::Camera {
				id: Some("a6400".into()),
			},
		)]),
		displays: Ok(vec![pick("Built-in display", Source::Display { id: Some("1".into()) })]),
		windows: Ok(Vec::new()),
		audio: Ok(Vec::new()),
	}
}

/// A desk's devices, as Add lists them: two cameras, two displays, two
/// windows, and two microphones.
fn add_devices() -> devices::Listing {
	let pick = |name: &str, source| devices::Pick {
		name: name.into(),
		source,
		size: None,
	};
	let camera = |id: &str| Source::Camera { id: Some(id.into()) };
	let display = |id: &str| Source::Display { id: Some(id.into()) };
	let window = |id: &str| Source::Window { id: id.into() };
	let mic = |id: &str| Source::Microphone { id: Some(id.into()) };
	devices::Listing {
		cameras: Ok(vec![
			pick("Sony A6400", camera("a6400")),
			pick("Laptop camera", camera("facetime")),
		]),
		displays: Ok(vec![pick("Display 1", display("1")), pick("Display 2", display("2"))]),
		windows: Ok(vec![
			pick("Season 4 (Neon Drift)", window("41")),
			pick("Run of show (Notes)", window("42")),
		]),
		audio: Ok(vec![
			pick("Desk mic (PipeWire)", mic("desk")),
			pick("Headset (PipeWire)", mic("headset")),
		]),
	}
}

/// The last 30 seconds as of `now`: a keyframe every 2.5 s, a sun setting
/// across a gradient, drawn rather than decoded so the view is the same on
/// every machine.
fn past(now: kio::clock::Instant) -> crate::past::View {
	let (width, height) = (480usize, 270usize);
	let pictures = (0..12u64)
		.map(|index| {
			let mut rgba = vec![0u8; width * height * 4];
			let sun = (60.0 + index as f32 * 30.0, 70.0 + index as f32 * 6.0);
			for (at, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
				let (x, y) = ((at % width) as f32, (at / width) as f32);
				let sky = y / height as f32;
				let mut color = [
					(40.0 + 160.0 * sky) as u8,
					(20.0 + 40.0 * sky) as u8,
					(90.0 - 40.0 * sky) as u8,
				];
				if (x - sun.0).powi(2) + (y - sun.1).powi(2) < 45.0_f32.powi(2) {
					color = [255, 170, 80];
				}
				pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
			}
			Arc::new(crate::past::Picture {
				id: index + 1,
				at: now.saturating_sub(Duration::from_millis(29_000 - index * 2_500)),
				size: [width, height],
				rgba,
			})
		})
		.collect::<Vec<_>>();
	// A packet each 20 ms, swelling and fading as speech does.
	let sound = (0..1450u64)
		.map(|index| {
			let at = now.saturating_sub(Duration::from_millis(29_000 - index * 20));
			let t = index as f32 / 50.0;
			let peak = ((t * 1.3).sin() * (t * 0.17).cos() * (t * 0.049 + 1.0).sin()).abs();
			(at, peak)
		})
		.collect();
	crate::past::View {
		of: crate::replay::Of::Program,
		held: Some(Ok(crate::past::Held {
			oldest: pictures[0].at,
			video: true,
		})),
		pictures: Ok(pictures),
		sound: Ok(sound),
	}
}

/// This show's recordings and clips, as the mock-up lists them.
fn shelf() -> crate::shelf::Listing {
	use crate::shelf::{Card, Kind, Listing};
	let wall = kio::clock::wall();
	let ago = |seconds: u64| Some(wall - Duration::from_secs(seconds));
	let length = |seconds: u64| Some(Duration::from_secs(seconds));
	let recording = |sources, repaired| Kind::Recording {
		stamp: String::new(),
		sources,
		repaired,
	};
	let card = |kind, modified, bytes, length| Card {
		kind,
		path: "show.mp4".into(),
		modified,
		bytes,
		length,
	};
	Listing {
		cards: vec![
			card(Kind::Clip(3), ago(150), 11_000_000, length(8)),
			card(Kind::Clip(2), ago(41 * 60), 18_000_000, length(12)),
			card(
				recording(0, true),
				ago(2 * 86_400 + 600),
				14_200_000_000,
				length(2 * 3600 + 10 * 60 + 44),
			),
			card(
				recording(4, false),
				ago(6 * 86_400 + 600),
				21_000_000_000,
				length(3 * 3600 + 2 * 60 + 10),
			),
			card(Kind::Clip(1), ago(6 * 86_400 + 900), 22_000_000, length(15)),
		],
		error: None,
		free: Some(212_000_000_000),
	}
}

/// Wait until every source has started and the program has drawn a second of
/// frames since, enough to redraw every thumbnail the window shows, then draw.
///
/// The window is not drawn while it waits: egui's clock moves a step a frame,
/// and the effects with it, so a still view takes the same steps however
/// long the program takes.
async fn settle(harness: &mut Harness<'_, Window>, engine: &engine::Engine) {
	// The thumbnails and preview the view shows, which the compositor draws
	// only once a frame asks for them.
	harness.step();
	let mut since: Option<u64> = None;
	loop {
		let rendered = engine.stats.rendered.load(Ordering::Relaxed);
		let (started, framerate) = {
			let state = engine.state.lock().unwrap();
			// A replay source's track starts with its first replay, so one that
			// idles has nothing to wait for.
			let started = state.sources.iter().all(|(_, feed)| {
				!matches!(feed.status(), Status::Starting)
					&& (matches!(feed.source(), Source::Replay) || !matches!(feed.track(), Some(Status::Starting)))
			});
			(started, u64::from(state.compositor.canvas().framerate))
		};
		match since {
			None if started => since = Some(rendered),
			Some(since) if rendered >= since + framerate => break,
			_ => {}
		}
		// The program draws on a clock of its own, at the canvas rate.
		kio::clock::sleep(Duration::from_millis(20)).await;
	}
	// Twice: once for egui to lay out what it learned, once to draw it.
	harness.run_steps(2);
}

/// A renderer on `gpu`'s device, as the window's is.
fn render_state(gpu: &engine::Gpu) -> egui_wgpu::RenderState {
	crate::control::headless::render_state(gpu).unwrap()
}
