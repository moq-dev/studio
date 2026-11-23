//! `--headless`: the whole studio with no window, drawn off-screen on the
//! compositor's GPU and driven only through its control socket.
//!
//! A frame is drawn whenever the studio asks for one, as a window's would be,
//! but no more often than a 60 Hz display draws them, and the picture only
//! when a screenshot asks for it. Animations are off, as in `just shots`, so
//! what a request changes is there on the next frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use eframe::egui_wgpu;
use egui_kittest::Harness;

use super::Socket;
use crate::{App, Started, Window, engine, fx, hotkeys, kit, library, show};

/// The window's size, its default on a desktop.
const SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);

/// The shortest time between frames.
const FRAME: Duration = Duration::from_micros(16_667);

/// Run `app` off-screen, serving `socket`, until it is told to close; then
/// end the studio as quitting a window does.
pub fn run(app: App, opener: show::Opener, socket: Socket) -> anyhow::Result<()> {
	let render = render_state(&opener.gpu)?;
	let started = Arc::<Started>::default();
	let closed = Arc::new(AtomicBool::new(false));
	let (wake, woken) = std::sync::mpsc::channel::<Duration>();
	let runtime = opener.runtime.clone();
	let framed = matches!(app, App::Starting(..));
	let window = {
		let render = render.clone();
		let started = started.clone();
		let closed = closed.clone();
		let runtime = runtime.clone();
		move |cc: &mut eframe::CreationContext<'_>| {
			kit::install(&cc.egui_ctx);
			cc.egui_ctx.set_request_repaint_callback(move |info| {
				let _ = wake.send(info.delay);
			});
			let control = socket.serve(&cc.egui_ctx, &runtime);
			Window {
				app,
				fx: fx::Fx::new(&render, &cc.egui_ctx, &runtime),
				render,
				opener,
				started,
				recent: library::Recent::default(),
				// None, so a headless run keeps nothing in the user's own
				// storage, which a window would.
				pictures: None,
				aired: None,
				keyboard: hotkeys::Keyboard::default(),
				framed,
				closed,
				control: Some(control),
			}
		}
	};
	let mut harness = Harness::builder()
		.with_size(SIZE)
		.renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render))
		.build_eframe(window);

	let begun = kio::clock::now();
	loop {
		let last = kio::clock::now();
		harness.input_mut().time = Some(last.saturating_duration_since(begun).as_secs_f64());
		harness.step();
		// What the desktop answers a close asked for with.
		if root(&harness).commands.contains(&egui::ViewportCommand::Close) {
			let root = harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
			root.events.push(egui::ViewportEvent::Close);
			harness.step();
		}
		if closed.load(Ordering::Relaxed) {
			break;
		}
		// egui's "never" is a delay too long to add.
		let mut next = last
			.checked_add(root(&harness).repaint_delay)
			.unwrap_or(last.saturating_add(Duration::from_secs(3600)));
		// Until the frame due, or sooner if asked; never before the next
		// frame a display would draw.
		loop {
			let now = kio::clock::now();
			let due = next.max(last.saturating_add(FRAME));
			let Some(left) = due.checked_duration_since(now).filter(|left| !left.is_zero()) else {
				break;
			};
			match woken.recv_timeout(left) {
				Ok(delay) => next = next.min(now.saturating_add(delay)),
				Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
				Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("the window's context is gone"),
			}
		}
	}

	drop(harness);
	if let Some(engine) = started.take() {
		crate::shutdown(&engine, &runtime)?;
	}
	Ok(())
}

/// The last frame's output for the window.
fn root<'a>(harness: &'a Harness<'_, Window>) -> &'a egui::ViewportOutput {
	&harness.output().viewport_output[&egui::ViewportId::ROOT]
}

/// A renderer on `gpu`'s device, as the window's is.
pub fn render_state(gpu: &engine::Gpu) -> anyhow::Result<egui_wgpu::RenderState> {
	let config = egui_wgpu::WgpuConfiguration {
		wgpu_setup: egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting {
			instance: gpu.instance.clone(),
			adapter: gpu.adapter.clone(),
			device: gpu.device.clone(),
			queue: gpu.queue.clone(),
		}),
		..Default::default()
	};
	Ok(pollster::block_on(egui_wgpu::RenderState::create(
		&config,
		&gpu.instance,
		None,
		egui_wgpu::RendererOptions::PREDICTABLE,
	))?)
}
