//! Scenes, live sources, and a GPU compositor for a MoQ streaming studio.
//!
//! The UI-free half of `moq-studio`, so the same scenes can be composed by a
//! windowed app, a headless renderer, or a test:
//!
//! - [`Collection`] is the scene graph: the program's [`Canvas`], named
//!   [`Source`]s, and [`Scene`]s placing them, each at its own [`Size`] or
//!   the program's, under a top layer that stays on across scene switches. A
//!   scene can draw another, or the program, and can be published or recorded
//!   on its own. [`AudioScene`]s hold sounds and other audio scenes, apart
//!   from cuts. It saves to a versioned TOML file that holds no credentials.
//! - [`Edit`] is every change to a collection, built with a [`Draft`]. Each
//!   one undoes itself, so a [`History`] undoes and redoes them and saves
//!   beside the scene file. A [`Clip`] carries items or filters between
//!   scenes and collections as text.
//! - [`source::Sources`] runs each source: a camera, display, window,
//!   microphone, or system-audio capture (with the `capture` feature), a still
//!   image, a video or audio file, a solid color, a broadcast subscribed over MoQ, or a web page
//!   rendered by a browser engine (the [`browser`] module). Pictures go to the
//!   compositor and sound goes to a `moq_audio::bus::Bus`, routed into a mix
//!   for each audio scene that reaches it.
//! - [`Compositor`] draws the scene in preview, the scene on program, and each
//!   scene an output reads on its own into textures on the caller's `wgpu`
//!   device, from one upload of each source's picture, cutting or running a
//!   [`Transition`] on program, and plays each item's [`Keyframe`]s from when
//!   its scene went on. A scene source shows another scene, drawn first into a
//!   texture of its own, or the program itself. A
//!   source whose [`Background`] is removed is cut out first, by a
//!   segmentation model on the CPU. `moq_video::render::export` hands the
//!   program to an encoder, on the GPU where the platform allows, and a
//!   [`Scaler`] draws it smaller for a smaller rendition.
//! - [`publish`] gives each local source its own track, in a broadcast the
//!   studio keeps to itself, to replay or record the source on its own.
//! - [`layout`] says where and when each source published independently
//!   shows over the program, and how its sound plays, for viewers to place
//!   its track themselves.
//!
//! Every picture is drawn on the program clock the caller ticks
//! [`Compositor::render`] with, from whatever each source delivered last: a
//! source that stalls repeats its last picture rather than holding up the
//! canvas.

pub mod browser;
pub mod clip;
mod collection;
mod compositor;
mod edit;
mod effect;
pub mod layout;
mod lut;
mod matte;
mod media;
mod overlay;
mod playout;
pub mod publish;
pub mod replay;
mod scale;
pub mod source;

pub use clip::Clip;
pub use collection::{
	Audio, AudioScene, Background, Blend, Canvas, Collection, Crop, Direction, Ease, Filter, Item, Keyframe, Layer,
	MaskMode, Nested, Rect, Rgba, Scene, Size, Source, Transition, Video,
};
pub use compositor::{Compositor, FORMAT, Output};
pub use edit::{Change, Draft, Edit, History};
pub use scale::Scaler;

/// The `wgpu` the compositor draws with: `moq_video`'s, so its textures and
/// the decoder's are the same type.
pub use moq_video::render::wgpu;

/// Errors from loading scenes, opening sources, and drawing them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	/// The scene file is not valid TOML of the expected shape.
	#[error("scene file: {0}")]
	Format(String),

	/// The scene file is from another version of the format.
	#[error("scene file version {found} is not supported; this build reads version {supported}")]
	Version {
		/// The version the file names.
		found: i64,
		/// The version this build reads.
		supported: u32,
	},

	/// The collection breaks a rule the types cannot express, such as an item
	/// naming a source that does not exist.
	#[error("invalid scene collection: {0}")]
	Invalid(String),

	/// An edit does not apply, since the collection no longer holds what the
	/// edit replaces.
	#[error("the edit no longer applies: {0}")]
	Conflict(String),

	/// An image source could not be read or decoded.
	#[error("image: {0}")]
	Image(String),

	/// A media source's file could not be read, or holds nothing the studio
	/// plays.
	#[error("media: {0}")]
	Media(String),

	/// A LUT source could not be read or parsed.
	#[error("LUT: {0}")]
	Lut(String),

	/// This build or platform cannot open the source.
	#[error("unsupported source: {0}")]
	Unsupported(String),

	/// The source stopped, with the reason, so it takes nothing more.
	#[error("the source stopped: {0}")]
	Stopped(String),

	/// Capturing, decoding, or drawing video failed.
	#[error(transparent)]
	Video(#[from] moq_video::Error),

	/// Capturing, decoding, or mixing audio failed.
	#[error(transparent)]
	Audio(#[from] moq_audio::Error),

	/// Reading a broadcast's catalog or media failed.
	#[error(transparent)]
	Mux(#[from] moq_mux::Error),

	/// The broadcast subscription failed.
	#[error(transparent)]
	Net(#[from] moq_net::Error),

	/// A browser source's page failed, or its helper process stopped.
	#[error("browser: {0}")]
	Browser(String),

	/// An overlay holds HTML or CSS outside what overlays draw, named, or
	/// failed to draw.
	#[error("overlay: {0}")]
	Overlay(String),

	/// Removing a source's background failed: running the model, or reading
	/// a picture back for it.
	#[error("background removal: {0}")]
	Matte(String),
}
