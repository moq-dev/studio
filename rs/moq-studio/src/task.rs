//! A result worked out off the window's thread.

use std::sync::mpsc;

/// A result being worked out in the background.
pub(crate) enum Task<T> {
	Running(mpsc::Receiver<T>),
	Done(T),
}

impl<T: Send + 'static> Task<T> {
	/// Run `task` in `runtime` on a thread of its own, since encoder probes and
	/// the engine's start block while they open encoders. Not on the runtime's
	/// blocking pool, which dropping the runtime waits out, so a probe that never
	/// ends, such as one waiting on a camera permission prompt, cannot hold the
	/// studio open once its window closes.
	pub(crate) fn spawn<F, Fut>(runtime: &tokio::runtime::Handle, task: F) -> std::io::Result<Self>
	where
		F: FnOnce() -> Fut + Send + 'static,
		Fut: Future<Output = T>,
	{
		let (send, recv) = mpsc::channel();
		let runtime = runtime.clone();
		std::thread::Builder::new().name("task".into()).spawn(move || {
			let _ = send.send(runtime.block_on(task()));
		})?;
		Ok(Self::Running(recv))
	}

	/// The result, once it arrived.
	///
	/// Panics if the task did, rather than waiting forever on a result that
	/// will never come.
	pub(crate) fn poll(&mut self) -> Option<&T> {
		if let Self::Running(recv) = self {
			match recv.try_recv() {
				Ok(done) => *self = Self::Done(done),
				Err(mpsc::TryRecvError::Empty) => {}
				Err(mpsc::TryRecvError::Disconnected) => panic!("a background task stopped without a result"),
			}
		}
		match self {
			Self::Running(_) => None,
			Self::Done(done) => Some(done),
		}
	}
}
