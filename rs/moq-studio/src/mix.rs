//! Outputs that read one of the collection's mixes: the program audio and
//! the headphone monitor.
//!
//! Each output follows a [`Pick`], which the program clock sets from the
//! collection, and reads the picked mix on its own thread or task. A mix's
//! reader never holds the bus up, so an output that falls behind loses only
//! its own audio.

use std::sync::{Arc, Mutex};
use std::task::{Poll, ready};

use moq_audio::bus::{Block, Mix, Reader};

/// Which mix an output reads, as last picked.
#[derive(Clone, Default)]
pub struct Pick(kio::Producer<Picked>);

#[derive(Default)]
struct Picked {
	/// How many times a mix was picked, so a reader sees each pick once.
	generation: u64,
	/// The mix, and a reader starting at the first block after the pick.
	mix: Option<(Mix, Reader)>,
}

impl Pick {
	/// Read `mix` from its next block on. Picking the mix already read
	/// changes nothing a reader hears.
	pub fn set(&self, mix: Mix) {
		let reader = mix.reader();
		// Fails only once every handle is gone, when nothing reads it.
		if let Ok(mut picked) = self.0.write() {
			picked.generation += 1;
			picked.mix = Some((mix, reader));
		}
	}

	/// Pick nothing more, as the bus stops: a reader that never got a mix
	/// ends, and one that did reads it to its end.
	pub fn close(&self) {
		let _ = self.0.close();
	}

	/// A reader of whichever mix is picked, starting with the next pick or
	/// the current one.
	pub fn follow(&self) -> Follow {
		Follow {
			pick: self.0.consume(),
			seen: 0,
			end: 0,
			current: None,
			next: None,
		}
	}
}

/// Reads the blocks of the [`Pick`]ed mix, switching between mixes on a block
/// boundary, so a new pick neither drops nor repeats a block.
pub struct Follow {
	pick: kio::Consumer<Picked>,
	/// The generation of the pick read last.
	seen: u64,
	/// Where the block after the last one read starts, which a new pick
	/// starts at or after.
	end: u64,
	/// The mix being read, held so it outlives a pick that drops it from the
	/// collection until the next one takes over.
	current: Option<(Mix, Reader)>,
	/// The mix picked since, with its first block once read, until the
	/// current one has caught up to it.
	next: Option<(Mix, Reader, Option<Block>)>,
}

impl Follow {
	/// The next block of the picked mix, or `None` once the bus has stopped and
	/// every block kept for this reader has been read. Waits for a first pick.
	pub fn poll_read(&mut self, waiter: &kio::Waiter) -> Poll<Option<Block>> {
		let block = ready!(self.poll_next(waiter));
		if let Some(block) = &block {
			self.end = block.position + block.frames() as u64;
		}
		Poll::Ready(block)
	}

	fn poll_next(&mut self, waiter: &kio::Waiter) -> Poll<Option<Block>> {
		let seen = self.seen;
		let picked = self.pick.poll(waiter, |picked| match &picked.mix {
			Some(mix) if picked.generation != seen => Poll::Ready((picked.generation, mix.clone())),
			_ => Poll::Pending,
		});
		let picked = picked.map(Result::ok);
		// Closed once the bus stops, leaving no pick to wait for.
		let mut ended = matches!(picked, Poll::Ready(None));
		if let Poll::Ready(Some((generation, (mix, reader)))) = picked {
			self.seen = generation;
			self.next = Some((mix, reader, None));
		}

		while let Some((_, reader, first @ None)) = &mut self.next {
			match reader.poll_read(waiter) {
				// Already read from the current mix, which the bus advanced
				// since this mix was picked.
				Poll::Ready(Some(block)) if block.position < self.end => {}
				Poll::Ready(Some(block)) => *first = Some(block),
				// The bus stopped, so the current mix ends too.
				Poll::Ready(None) => {
					self.next = None;
					ended = true;
				}
				Poll::Pending => break,
			}
		}
		let first = match &self.next {
			Some((_, _, Some(first))) => Some(first.position),
			_ => None,
		};

		let block = match &mut self.current {
			Some((_, reader)) => ready!(reader.poll_read(waiter)),
			// Nothing read yet, so the next mix starts at its first block.
			None if first.is_some() => return Poll::Ready(self.switch()),
			None if ended && self.next.is_none() => return Poll::Ready(None),
			None => return Poll::Pending,
		};
		match (block, first) {
			// Every mix advances on the one clock, so the current mix reaches
			// the next one's first block, which takes over from there.
			(Some(block), Some(first)) if block.position < first => Poll::Ready(Some(block)),
			(_, Some(_)) => Poll::Ready(self.switch()),
			(block, None) => Poll::Ready(block),
		}
	}

	/// Read the next mix from now on, starting with its first block.
	fn switch(&mut self) -> Option<Block> {
		let (mix, reader, first) = self.next.take()?;
		self.current = Some((mix, reader));
		first
	}

	/// The next block of the picked mix, or `None` once the bus has stopped.
	pub async fn read(&mut self) -> Option<Block> {
		kio::wait(|waiter| self.poll_read(waiter)).await
	}
}

/// Whether this build can play a mix to the speakers: the `capture` feature
/// brings the device's audio library.
pub const MONITOR: bool = cfg!(feature = "capture");

/// What to listen to, and where.
#[derive(Clone, Copy, Debug)]
pub struct Listen<'a> {
	/// The mix, by name.
	pub mix: &'a str,
	/// The [`Speaker::id`] to play on, or `None` for the system default.
	pub speaker: Option<&'a str>,
}

/// An audio output this machine has, to listen on.
#[derive(Clone, Debug)]
pub struct Speaker {
	/// What [`Monitor::start`] opens it by.
	pub id: String,
	/// The device's name and the host API it is reached through.
	pub name: String,
}

/// This machine's audio outputs, or why they cannot be listed.
#[cfg(feature = "capture")]
pub async fn speakers() -> Result<Vec<Speaker>, String> {
	let devices = moq_audio::playback::devices().await.map_err(|err| err.to_string())?;
	let speakers = devices.into_iter().map(|device| Speaker {
		name: format!("{} ({})", device.name, device.host),
		id: device.id,
	});
	Ok(speakers.collect())
}

#[cfg(not(feature = "capture"))]
pub async fn speakers() -> Result<Vec<Speaker>, String> {
	Err("listening needs a build with the `capture` feature".into())
}

/// Plays one mix to an audio output, for headphones.
///
/// Stops when dropped.
pub struct Monitor {
	/// The mix it plays, by name.
	name: String,
	/// The [`Speaker::id`] it plays on, or `None` for the system default.
	speaker: Option<String>,
	pick: Pick,
	failed: Arc<Mutex<Option<String>>>,
	task: tokio::task::JoinHandle<()>,
}

impl Monitor {
	/// Open the output `speaker`, or the system default, on `runtime`, to play
	/// the mix called `name` once it is [given](Self::play). Fails in a build
	/// without [`MONITOR`].
	pub fn start(runtime: &tokio::runtime::Handle, name: &str, speaker: Option<&str>) -> anyhow::Result<Self> {
		anyhow::ensure!(MONITOR, "listening needs a build with the `capture` feature");
		let pick = Pick::default();
		let follow = pick.follow();
		let failed = Arc::new(Mutex::new(None));
		let reason = failed.clone();
		let device = speaker.map(str::to_string);
		let task = runtime.spawn(async move {
			if let Err(err) = play(follow, device).await {
				tracing::warn!(error = format!("{err:#}"), "the monitor stopped");
				*reason.lock().unwrap() = Some(format!("{err:#}"));
			}
		});
		Ok(Self {
			name: name.to_string(),
			speaker: speaker.map(str::to_string),
			pick,
			failed,
			task,
		})
	}

	/// The name of the mix it plays.
	pub fn name(&self) -> &str {
		&self.name
	}

	/// The output it plays on, or `None` for the system default.
	pub fn speaker(&self) -> Option<&str> {
		self.speaker.as_deref()
	}

	/// Play the mix called `name` once it is given.
	pub fn set_name(&mut self, name: &str) {
		self.name = name.to_string();
	}

	/// Play `mix`, the one [named](Self::name), from its next block on.
	pub fn play(&self, mix: Mix) {
		self.pick.set(mix);
	}

	/// Why it stopped playing, if it did.
	pub fn failed(&self) -> Option<String> {
		self.failed.lock().unwrap().clone()
	}
}

impl Drop for Monitor {
	fn drop(&mut self) {
		self.task.abort();
	}
}

/// Play what `follow` reads out the output `device`, or the system default,
/// until the bus stops.
#[cfg(feature = "capture")]
async fn play(mut follow: Follow, device: Option<String>) -> anyhow::Result<()> {
	use anyhow::Context;

	let mut config = moq_audio::playback::Config::default();
	config.device = device;
	let device = moq_audio::playback::Engine::open(config)
		.await
		.context("cannot open the output device")?;
	let config = moq_audio::bus::Config::default();
	let mut input = moq_audio::playback::Input::default();
	input.format = moq_audio::Format::F32;
	input.sample_rate = config.sample_rate;
	input.layout = config.layout;
	let mut sink = device.sink(input)?;
	while let Some(block) = follow.read().await {
		let data: Vec<u8> = block.data.iter().flat_map(|sample| sample.to_le_bytes()).collect();
		// What the device cannot take is dropped rather than played late.
		let _ = sink.write(&data)?;
	}
	Ok(())
}

#[cfg(not(feature = "capture"))]
async fn play(_follow: Follow, _device: Option<String>) -> anyhow::Result<()> {
	unreachable!("refused by Monitor::start")
}

#[cfg(test)]
mod tests {
	use moq_audio::bus::{Bus, Channel, Driver, Strip};

	use super::*;

	/// The default bus's 10 ms block.
	const BLOCK: usize = 480;

	/// A stereo source the test feeds `value` one block per advance.
	fn source(bus: &Bus) -> (Channel, Strip) {
		let strip = Strip::default();
		let mut input = moq_audio::encode::Input::new(48_000, moq_audio::Layout::Stereo);
		input.format = moq_audio::Format::F32;
		(bus.channel(input, strip.clone()).unwrap(), strip)
	}

	fn feed(channel: &mut Channel, value: f32) {
		let data: Vec<u8> = std::iter::repeat_n(value, BLOCK * 2)
			.flat_map(f32::to_le_bytes)
			.collect();
		channel.write(&data).unwrap();
	}

	/// The block `follow` already has waiting.
	fn ready(follow: &mut Follow) -> Option<Block> {
		match follow.poll_read(&kio::Waiter::noop()) {
			Poll::Ready(block) => block,
			Poll::Pending => panic!("no block waiting"),
		}
	}

	/// Two mixes of one source each, `a` playing 0.25 and `b` 0.5, run past
	/// the bus's latency so each mix carries its source.
	fn mixes() -> (Driver, Mix, Mix, [Channel; 2]) {
		let (bus, mut driver) = Bus::new(Default::default()).unwrap();
		let (mut quiet, quiet_strip) = source(&bus);
		let (mut loud, loud_strip) = source(&bus);
		let a = bus.mix();
		a.route(&quiet_strip);
		let b = bus.mix();
		b.route(&loud_strip);
		driver.advance(BLOCK);
		for _ in 0..20 {
			feed(&mut quiet, 0.25);
			feed(&mut loud, 0.5);
			driver.advance(BLOCK);
		}
		(driver, a, b, [quiet, loud])
	}

	/// Switching mixes, even while the output lags, plays every position once.
	#[test]
	fn a_new_pick_neither_drops_nor_repeats_a_block() {
		let (mut driver, a, b, [mut quiet, mut loud]) = mixes();
		let pick = Pick::default();
		let mut follow = pick.follow();
		assert!(
			follow.poll_read(&kio::Waiter::noop()).is_pending(),
			"nothing picked yet"
		);

		pick.set(a.clone());
		let mut step = |driver: &mut Driver| {
			feed(&mut quiet, 0.25);
			feed(&mut loud, 0.5);
			driver.advance(BLOCK);
		};
		step(&mut driver);
		let first = ready(&mut follow).unwrap();
		assert_eq!(first.data[0], 0.25);

		// Two blocks behind when the pick changes: both still come from `a`.
		step(&mut driver);
		step(&mut driver);
		pick.set(b.clone());
		step(&mut driver);
		let mut heard = Vec::new();
		while let Poll::Ready(Some(block)) = follow.poll_read(&kio::Waiter::noop()) {
			heard.push((block.position, block.data[0]));
		}
		let at = |n: u64| first.position + n * BLOCK as u64;
		assert_eq!(heard, [(at(1), 0.25), (at(2), 0.25), (at(3), 0.5)]);

		// Picking the mix already read changes nothing.
		pick.set(b.clone());
		step(&mut driver);
		assert_eq!(
			ready(&mut follow).map(|block| (block.position, block.data[0])),
			Some((at(4), 0.5))
		);

		drop(driver);
		assert!(ready(&mut follow).is_none(), "the bus stopped");
	}

	/// A pick whose reader starts at a block the follower already read, as one
	/// made while the bus advances can, starts after it instead.
	#[test]
	fn a_pick_never_replays_a_block() {
		let (mut driver, a, b, [mut quiet, mut loud]) = mixes();
		let pick = Pick::default();
		let mut follow = pick.follow();
		let mut step = |driver: &mut Driver| {
			feed(&mut quiet, 0.25);
			feed(&mut loud, 0.5);
			driver.advance(BLOCK);
		};
		pick.set(a.clone());
		step(&mut driver);
		ready(&mut follow).unwrap();
		let late = b.reader();
		step(&mut driver);
		let read = ready(&mut follow).unwrap();
		{
			let mut picked = pick.0.write().ok().unwrap();
			picked.generation += 1;
			picked.mix = Some((b.clone(), late));
		}
		step(&mut driver);
		let next = ready(&mut follow).unwrap();
		assert_eq!(next.position, read.position + BLOCK as u64, "read once");
		assert_eq!(next.data[0], 0.5);
	}

	/// A follower that stops reading holds up neither the bus nor another reader.
	#[test]
	fn a_stalled_output_loses_only_its_own_audio() {
		let (mut driver, a, _b, [mut quiet, _loud]) = mixes();
		let pick = Pick::default();
		pick.set(a.clone());
		let mut stalled = pick.follow();
		let mut live = pick.follow();
		feed(&mut quiet, 0.25);
		driver.advance(BLOCK);
		let start = ready(&mut stalled).unwrap().position;
		assert_eq!(ready(&mut live).unwrap().position, start);

		// A second's worth, read by `live` only.
		for index in 1..=100u64 {
			feed(&mut quiet, 0.25);
			driver.advance(BLOCK);
			let block = ready(&mut live).unwrap();
			assert_eq!(block.position, start + index * BLOCK as u64, "never held up");
		}
		let resumed = ready(&mut stalled).unwrap();
		assert!(
			resumed.position > start + BLOCK as u64,
			"the stalled output skipped ahead"
		);
	}
}
