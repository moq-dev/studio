//! Commands for pages not yet carried out, taken in batches: what the studio
//! has yet to tell the helper, which its writer takes, and what the helper
//! has yet to apply, which CEF's UI thread takes.
//!
//! Each page's open, frame rate, and close coalesce, and its pending input
//! goes with it once it closes, so what waits scales with the pages open,
//! never with how often they changed, even while the taker is stuck. A page
//! closed before its open is taken never opens. Input past the backlog drops,
//! except a release, which is owed instead, so a taker that catches up never
//! leaves a page holding a button or key that was let go.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use super::protocol::{Command, Id, Open};
use super::{Button, Input};

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
	/// The backlog is full, so the input dropped.
	Full,
	/// The writer stopped, and so did the helper.
	Gone,
}

/// What an input lets go of, which a later release of the same supersedes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Release {
	Button(Button),
	Key(i32),
	Focus,
	Pointer,
}

impl Release {
	fn of(input: &Input) -> Option<Self> {
		match *input {
			Input::Button {
				button, down: false, ..
			} => Some(Self::Button(button)),
			Input::Key { code, down: false, .. } => Some(Self::Key(code)),
			Input::Focus(false) => Some(Self::Focus),
			Input::Leave { .. } => Some(Self::Pointer),
			_ => None,
		}
	}
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
	/// Releases past the backlog, oldest first, at most one per page and
	/// button, key, focus, and pointer, taken after the input before them.
	owed: Vec<(Id, Input)>,
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
			outbox.owed.retain(|(page, _)| *page != id);
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

	/// Queue input for page `id`. Past the backlog, or behind an owed release,
	/// a release is owed and other input drops.
	pub(crate) fn input(&self, id: Id, input: Input) -> Result<(), Refused> {
		let (outbox, ready) = &*self.0;
		let mut outbox = outbox.lock().unwrap();
		if outbox.stopped {
			return Err(Refused::Gone);
		}
		if outbox.owed.is_empty() && outbox.input.len() < BACKLOG {
			outbox.input.push_back((id, input));
		} else if let Some(release) = Release::of(&input) {
			outbox
				.owed
				.retain(|(page, owed)| *page != id || Release::of(owed) != Some(release));
			outbox.owed.push((id, input));
		} else {
			return Err(Refused::Full);
		}
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

	/// Whether nothing waits to be taken.
	#[cfg(test)]
	pub(crate) fn is_empty(&self) -> bool {
		let outbox = self.0.0.lock().unwrap();
		outbox.opens.is_empty()
			&& outbox.rates.is_empty()
			&& outbox.input.is_empty()
			&& outbox.owed.is_empty()
			&& outbox.painted == 0
			&& outbox.closes.is_empty()
	}

	/// The commands waiting, without waiting for any.
	#[cfg(any(test, feature = "cef"))]
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
		batch.extend(self.owed.drain(..).map(|(id, input)| Command::Input(id, input)));
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

	fn button(down: bool) -> Input {
		Input::Button {
			x: 1,
			y: 1,
			button: Button::Left,
			down,
			clicks: 1,
			modifiers: Default::default(),
		}
	}

	/// Past the backlog, a release is owed rather than dropped, once per
	/// button, and goes ahead of newer input, so the page never keeps holding
	/// what was let go.
	#[test]
	fn a_release_past_the_backlog_is_owed() {
		let commands = Commands::default();
		commands.input(0, button(true)).ok().unwrap();
		while commands.input(0, Input::Text("x".into())).is_ok() {}
		for input in [button(false), button(true), button(false), Input::Text("y".into())] {
			let _ = commands.input(0, input);
		}

		let received: Vec<Input> = commands
			.take()
			.into_iter()
			.map(|command| match command {
				Command::Input(0, input) => input,
				other => panic!("only input is queued, not {other:?}"),
			})
			.collect();
		assert_eq!(received.first(), Some(&button(true)));
		assert_eq!(received.last(), Some(&button(false)));
		assert_eq!(received.iter().filter(|input| **input == button(false)).count(), 1);
		assert!(!received.contains(&Input::Text("y".into())));

		commands.input(0, Input::Text("z".into())).ok().unwrap();
		assert!(matches!(&commands.take()[..], [Command::Input(0, Input::Text(text))] if text == "z"));
	}

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
