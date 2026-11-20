//! A small picture read back from the GPU without stalling a frame.
//!
//! A compute pass reduces a picture to a grid of RGBA8 tiles in a storage
//! buffer, which is copied to a buffer mapped for reading and comes back a
//! frame or so later, at most every [`INTERVAL`]. A read that fails is
//! dropped, as is one whose picture stopped mattering while under way.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kio::clock::Instant;

/// How often a picture is read, at most.
pub const INTERVAL: Duration = Duration::from_millis(250);

/// A read that came back: its tiles, row by row, when its picture was taken,
/// and what the reader said it was of.
pub(crate) struct Read<T> {
	pub tiles: Vec<[u8; 4]>,
	pub at: Instant,
	pub of: T,
}

/// A read under way.
struct Reading<T> {
	at: Instant,
	of: T,
	count: usize,
	mapped: Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>,
	/// Whether what it reads stopped mattering, so it is dropped once it
	/// comes back.
	stale: bool,
}

/// One reader's tiles on the GPU and the read under way, if any.
pub(crate) struct Readback<T> {
	queue: wgpu::Queue,
	tiles: wgpu::Buffer,
	mapped: wgpu::Buffer,
	reading: Option<Reading<T>>,
	/// When the last read was taken.
	last: Option<Instant>,
}

impl<T> Readback<T> {
	/// Room for a grid of up to `tiles` tiles.
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, tiles: u32) -> Self {
		let size = u64::from(tiles) * 4;
		let buffer = |usage| {
			device.create_buffer(&wgpu::BufferDescriptor {
				label: Some("moq-scene readback"),
				size,
				usage,
				mapped_at_creation: false,
			})
		};
		Self {
			queue: queue.clone(),
			tiles: buffer(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
			mapped: buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
			reading: None,
			last: None,
		}
	}

	/// The storage buffer the compute pass writes the tiles to.
	pub fn binding(&self) -> wgpu::BindingResource<'_> {
		self.tiles.as_entire_binding()
	}

	/// When the last read was taken, if one was since the last
	/// [`restart`](Self::restart).
	pub fn last(&self) -> Option<Instant> {
		self.last
	}

	/// Whether a read is under way.
	#[cfg(test)]
	pub fn busy(&self) -> bool {
		self.reading.is_some()
	}

	/// Whether a read may start at `now`: none is under way, and the last
	/// was at least [`INTERVAL`] ago.
	pub fn due(&self, now: Instant) -> bool {
		self.reading.is_none()
			&& self
				.last
				.is_none_or(|last| now.saturating_duration_since(last) >= INTERVAL)
	}

	/// Drop the read under way once it comes back.
	pub fn discard(&mut self) {
		if let Some(reading) = &mut self.reading {
			reading.stale = true;
		}
	}

	/// Forget the reads so far: the read under way is dropped, and the next
	/// is due at once.
	pub fn restart(&mut self) {
		self.discard();
		self.last = None;
	}

	/// Submit `encoder`, whose compute pass wrote `count` tiles of a picture
	/// taken at `at`, and read them back.
	pub fn submit(&mut self, mut encoder: wgpu::CommandEncoder, count: u32, at: Instant, of: T) {
		encoder.copy_buffer_to_buffer(&self.tiles, 0, &self.mapped, 0, u64::from(count) * 4);
		self.queue.submit([encoder.finish()]);
		let mapped = Arc::new(Mutex::new(None));
		let done = mapped.clone();
		// Called from a later submit or poll, once the GPU is done.
		self.mapped.map_async(wgpu::MapMode::Read, .., move |result| {
			*done.lock().unwrap() = Some(result);
		});
		self.reading = Some(Reading {
			at,
			of,
			count: count as usize,
			mapped,
			stale: false,
		});
		self.last = Some(at);
	}

	/// The read under way, once it came back; a stale one comes back as
	/// nothing.
	pub fn finish(&mut self) -> Option<Result<Read<T>, String>> {
		let reading = self.reading.as_ref()?;
		let mapped = reading.mapped.lock().unwrap().take()?;
		let Reading {
			at, of, count, stale, ..
		} = self.reading.take().expect("checked above");
		if stale {
			if mapped.is_ok() {
				self.mapped.unmap();
			}
			return None;
		}
		let tiles = mapped.map_err(|err| err.to_string()).and_then(|()| {
			let tiles = match self.mapped.slice(..).get_mapped_range() {
				Ok(view) => Ok(view.as_chunks::<4>().0[..count].to_vec()),
				Err(err) => Err(format!("{err:?}")),
			};
			self.mapped.unmap();
			tiles
		});
		Some(tiles.map(|tiles| Read { tiles, at, of }))
	}
}
