//! The camera extension in the macOS app bundle. The system's camera service
//! starts it once the user allows it, and apps list the cameras it adds.

#[cfg(target_os = "macos")]
mod extension;

#[cfg(target_os = "macos")]
fn main() {
	extension::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
	eprintln!("moq-studio-camera is a macOS camera extension");
	std::process::ExitCode::FAILURE
}
