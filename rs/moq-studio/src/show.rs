//! Opening a show: its stream settings and its engine, or guided setup when
//! its scene file does not exist yet.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use crate::autosave::Autosave;
use crate::engine::{self, Engine, Launch};
use crate::setup;
use crate::stream::{self, Stream};

/// What opening a show takes besides its path.
#[derive(Clone)]
pub struct Opener {
	pub gpu: engine::Gpu,
	pub runtime: tokio::runtime::Handle,
	/// What the command line sets for every show this run opens.
	pub flags: stream::Flags,
	/// Every other client flag a relay is dialed with.
	pub client: moq_tokio::connect::Config,
	pub quic: moq_tokio::quic::Config,
	pub credentials: stream::Credentials,
}

/// A show, opened.
pub enum Opened {
	/// Its engine, started.
	Studio(Arc<Engine>, Stream),
	/// Guided setup, for a scene file that does not exist yet.
	Setup(Box<setup::Setup>, Stream),
}

impl Opener {
	/// Open the show at `path`: start its engine, or guided setup when there
	/// is no scene file there yet. Nothing goes live, and its guests' room
	/// opens with the window, which follows the show's relays.
	pub async fn open(&self, path: PathBuf) -> anyhow::Result<Opened> {
		let stream = Stream::load(
			&path,
			self.flags.clone(),
			self.client.clone(),
			self.quic.clone(),
			self.credentials.clone(),
		)?;
		let launch = Launch {
			gpu: self.gpu.clone(),
			recordings: stream.recordings(&path),
		};
		let exists = path
			.try_exists()
			.with_context(|| format!("reading {}", path.display()))?;
		if exists {
			let (autosave, loaded) = Autosave::open(path)?;
			let engine = Engine::start(launch, autosave, loaded).await?;
			return Ok(Opened::Studio(engine, stream));
		}
		// The relay setup measures the uplink to, with its token, read off
		// this task since the credential store blocks.
		let measured = stream.clone();
		let relay = tokio::task::spawn_blocking(move || {
			let only = stream::Switched {
				relay: true,
				failover: false,
			};
			measured.relays(only)
		})
		.await
		.context("reading the relay's token panicked")?;
		let uplink = match relay {
			Ok(relays) => relays
				.into_iter()
				.next()
				.map(|relay| (relay, stream.broadcast().to_string())),
			Err(err) => {
				tracing::warn!(error = format!("{err:#}"), "cannot measure the uplink");
				None
			}
		};
		let setup = setup::Setup::new(launch, uplink, path, self.runtime.clone())?;
		Ok(Opened::Setup(Box::new(setup), stream))
	}
}

/// Follow broadcast sources, guests, and viewers' reports through `stream`'s
/// relays, read with their tokens. Each relay is set up on its own, so one
/// that cannot be is logged and the other still followed; going live says
/// why. Blocks on the credential store: call it off the window's thread,
/// outside an async task, inside the tokio runtime, and through
/// [`Controls`](crate::controls::Controls), which orders the follows.
/// Says whether every relay was set up, so a follow that was not can be
/// tried again.
#[must_use]
pub fn follow(engine: &Engine, stream: &Stream) -> bool {
	let each = [(true, false), (false, true)].map(|(relay, failover)| stream::Switched { relay, failover });
	let mut relays = Vec::new();
	let mut failed = false;
	for switched in each {
		match stream.relays(switched) {
			Ok(set) => relays.extend(set),
			Err(err) => {
				tracing::warn!(error = format!("{err:#}"), "cannot follow a relay");
				failed = true;
			}
		}
	}
	// None could be set up, such as a new relay on a fixed --connect-bind
	// port the sessions followed now still hold: those keep the broadcast
	// sources and guests rather than none.
	if relays.is_empty() && failed {
		tracing::warn!("keeping the relays followed before");
		return false;
	}
	engine.watch(&relays);
	!failed
}
