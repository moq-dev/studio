//! What the studio has yet to tell the helper, which the writer takes in
//! batches.
//!
//! Each page's open, frame rate, and close coalesce, and its pending input
//! goes with it once it closes, so what waits scales with the pages open,
//! never with how often they changed, even while the helper reads nothing.
//! A page closed before its open was written never opens.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use super::Input;
use super::protocol::{Command, Id, Open};

/// Input waiting for the helper to read it. A helper this far behind is hung,
/// and [`Page::send`](super::Page::send) drops new input rather than wait for
/// it.
const BACKLOG: usize = 256;

/// A handle on the commands waiting for one helper. Nothing here waits on the
/// helper.
#[derive(Clone, Default)]
pub(crate) struct Commands(Arc<(Mutex<Outbox>, Condvar)>);

/// Why input was not queued.
pub(crate) enum Refused {
	/// The backlog is full, and the input is handed back.
	Full(Input),
	/// The writer stopped, and so did the helper.
	Gone,
}

#[derive(Default)]
struct Outbox {
	/// Pages to open, in order.
	opens: Vec<(Id, Open)>,
	/// Pages whose open is taken and whose close is not.
	open: HashSet<Id>,
	/// The newest frame rate per page.
	rates: HashMap<Id, u32>,
	/// Input, at most [`BACKLOG`].
	input: VecDeque<(Id, Input)>,
	/// Answers owed for pictures copied.
	painted: usize,
	/// Open pages to close.
	closes: Vec<Id>,
	/// Whether the writer stopped, so nothing more is taken.
	stopped: bool,
}

impl Commands {
	/// Queue `f`'s change, unless the writer stopped.
	fn push(&self, f: impl FnOnce(&mut Outbox)) -> bool {
		let (outbox, ready) = &*self.0;
		let mut outbox = outbox.lock().unwrap();
		if outbox.stopped {
			return false;
		}
		f(&mut outbox);
		ready.notify_one();
		true
	}

	/// Open page `id`.
	pub(crate) fn open(&self, id: Id, open: Open) {
		self.push(|outbox| outbox.opens.push((id, open)));
	}

	/// Close page `id`, which drops what it has waiting, its open too if
	/// that is still waiting.
	pub(crate) fn close(&self, id: Id) {
		self.push(|outbox| {
			outbox.input.retain(|(page, _)| *page != id);
			outbox.rates.remove(&id);
			match outbox.opens.iter().position(|(page, _)| *page == id) {
				Some(at) => _ = outbox.opens.remove(at),
				None if outbox.open.contains(&id) => outbox.closes.push(id),
				None => {}
			}
		});
	}

	/// Paint page `id` at `rate`; false once the writer stopped.
	pub(crate) fn rate(&self, id: Id, rate: u32) -> bool {
		self.push(|outbox| _ = outbox.rates.insert(id, rate))
	}

	/// Answer the picture the helper waits on.
	pub(crate) fn painted(&self) {
		self.push(|outbox| outbox.painted += 1);
	}

	/// Queue input for page `id`, unless the backlog is full.
	pub(crate) fn input(&self, id: Id, input: Input) -> Result<(), Refused> {
		let (outbox, ready) = &*self.0;
		let mut outbox = outbox.lock().unwrap();
		if outbox.stopped {
			return Err(Refused::Gone);
		}
		if outbox.input.len() >= BACKLOG {
			return Err(Refused::Full(input));
		}
		outbox.input.push_back((id, input));
		ready.notify_one();
		Ok(())
	}

	/// Stop taking commands, so [`next`](Self::next) returns `None`.
	pub(crate) fn stop(&self) {
		let (outbox, ready) = &*self.0;
		outbox.lock().unwrap().stopped = true;
		ready.notify_all();
	}

	/// Wait for commands to write, or `None` once stopped.
	pub(crate) fn next(&self) -> Option<Vec<Command>> {
		let (outbox, ready) = &*self.0;
		let mut outbox = outbox.lock().unwrap();
		loop {
			if outbox.stopped {
				return None;
			}
			let batch = outbox.take();
			if !batch.is_empty() {
				return Some(batch);
			}
			outbox = ready.wait(outbox).unwrap();
		}
	}

	/// The commands waiting, without waiting for any.
	#[cfg(test)]
	pub(crate) fn take(&self) -> Vec<Command> {
		self.0.0.lock().unwrap().take()
	}
}

impl Outbox {
	/// Everything waiting, each page's open ahead of anything else for it.
	fn take(&mut self) -> Vec<Command> {
		let mut batch = Vec::new();
		for (id, open) in self.opens.drain(..) {
			self.open.insert(id);
			batch.push(Command::Open(id, open));
		}
		batch.extend(self.rates.drain().map(|(id, rate)| Command::FrameRate(id, rate)));
		batch.extend(self.input.drain(..).map(|(id, input)| Command::Input(id, input)));
		batch.extend(std::iter::repeat_n(Command::Painted, std::mem::take(&mut self.painted)));
		for id in self.closes.drain(..) {
			self.open.remove(&id);
			batch.push(Command::Close(id));
		}
		batch
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn open(url: &str) -> Open {
		Open {
			url: url.into(),
			..Default::default()
		}
	}

	/// Reloads while the helper reads nothing leave one page waiting, not one
	/// per reload: a page closed before its open is taken never opens, and its
	/// input and frame rate go with it.
	#[test]
	fn a_page_closed_before_it_opens_never_opens() {
		let commands = Commands::default();
		commands.open(0, open("first"));
		assert!(matches!(commands.take()[..], [Command::Open(0, _)]));

		for id in 1..100 {
			commands.open(id, open(&"x".repeat(1 << 16)));
			commands.rate(id, 60);
			let _ = commands.input(id, Input::Text("x".into()));
			commands.close(id);
		}
		commands.open(100, open("last"));
		commands.close(0);

		let batch = commands.take();
		assert_eq!(batch.len(), 2, "{batch:?}");
		assert!(matches!(&batch[0], Command::Open(100, open) if open.url == "last"));
		assert!(matches!(batch[1], Command::Close(0)));
	}

	/// A page's open goes ahead of its input and close, even in one batch,
	/// and frame rates coalesce to the newest.
	#[test]
	fn a_batch_opens_before_anything_else() {
		let commands = Commands::default();
		commands.open(1, open("page"));
		commands.rate(1, 30);
		commands.rate(1, 60);
		let _ = commands.input(1, Input::Text("x".into()));
		commands.painted();

		let batch = commands.take();
		assert!(matches!(
			batch[..],
			[
				Command::Open(1, _),
				Command::FrameRate(1, 60),
				Command::Input(1, _),
				Command::Painted
			]
		));
		commands.close(1);
		assert!(matches!(commands.take()[..], [Command::Close(1)]));
	}
}
