//! Whether the system asks apps to reduce motion, which the Effects setting
//! follows until one is picked.

/// The system's reduced-motion preference, followed while the window is open.
pub struct System {
	/// Set by the desktop portal's task.
	#[cfg(target_os = "linux")]
	reduced: std::sync::Arc<std::sync::atomic::AtomicBool>,
	#[cfg(target_os = "linux")]
	task: tokio::task::AbortHandle,
}

#[cfg(target_os = "linux")]
impl System {
	/// Read the preference from the desktop portal, and follow its changes.
	pub fn new(ctx: &egui::Context, runtime: &tokio::runtime::Handle) -> Self {
		let reduced = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
		let task = runtime
			.spawn(portal(reduced.clone(), ctx.clone()))
			.abort_handle();
		Self { reduced, task }
	}

	pub fn reduced(&self) -> bool {
		self.reduced.load(std::sync::atomic::Ordering::Relaxed)
	}
}

#[cfg(target_os = "linux")]
impl Drop for System {
	fn drop(&mut self) {
		self.task.abort();
	}
}

/// Keep `reduced` as the portal's Settings report it. A desktop without the
/// portal, or whose portal predates the setting, expresses no preference.
#[cfg(target_os = "linux")]
async fn portal(reduced: std::sync::Arc<std::sync::atomic::AtomicBool>, ctx: egui::Context) {
	use ashpd::desktop::settings::{ReducedMotion, Settings};
	use futures_util::StreamExt;

	let set = |motion: ReducedMotion| {
		reduced.store(motion == ReducedMotion::ReducedMotion, std::sync::atomic::Ordering::Relaxed);
		ctx.request_repaint();
	};
	let followed = async {
		let settings = Settings::new().await?;
		let mut changes = settings.receive_reduced_motion_changed().await?;
		set(settings.reduced_motion().await?);
		while let Some(motion) = changes.next().await {
			set(motion);
		}
		anyhow::Ok(())
	};
	if let Err(err) = followed.await {
		tracing::debug!(error = format!("{err:#}"), "not following the system's reduced motion");
	}
}

#[cfg(not(target_os = "linux"))]
impl System {
	pub fn new(_ctx: &egui::Context, _runtime: &tokio::runtime::Handle) -> Self {
		Self {}
	}

	/// Read each frame: a system setting the OS keeps in memory.
	#[cfg(target_os = "macos")]
	pub fn reduced(&self) -> bool {
		objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
	}

	/// Read each frame: a system setting the OS keeps in memory. Windows
	/// turns client-area animations off to reduce motion.
	#[cfg(windows)]
	pub fn reduced(&self) -> bool {
		use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

		let mut animated: windows_sys::core::BOOL = 1;
		// SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the
		// pointer, which points at one.
		let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animated).cast(), 0) };
		read != 0 && animated == 0
	}

	#[cfg(not(any(target_os = "macos", windows)))]
	pub fn reduced(&self) -> bool {
		false
	}
}
