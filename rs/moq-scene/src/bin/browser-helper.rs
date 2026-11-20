//! CEF's subprocesses (renderer, GPU, utility) on macOS, where each kind runs
//! from a helper app of its own inside the app bundle, as Chromium requires,
//! rather than from the studio's executable as on Linux. The bundle copies
//! this binary into each helper app.

use std::process::ExitCode;

fn main() -> ExitCode {
	moq_scene::browser::child().unwrap_or_else(|| {
		eprintln!("moq-browser-helper runs only as one of CEF's subprocesses");
		ExitCode::FAILURE
	})
}
