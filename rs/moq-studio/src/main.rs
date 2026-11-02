//! MoQ Studio: compose scenes from cameras, screens, images, and other
//! broadcasts on the GPU, and go live over MoQ.
//!
//! The scenes, sources, and compositor live in `moq-scene`; this binary is the
//! window around them and the program clock that publishes the result.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

mod devices;
mod engine;
mod setup;
mod ui;
mod uplink;

#[derive(usage_rs::Cli, Clone)]
#[usage(unknown_flags = "error", args_override_self = false)]
#[usage(name = "moq-studio")]
#[usage(completion, settings)]
pub struct Config {
	/// The scene collection to open. If it does not exist, guided setup proposes
	/// one and writes it here.
	#[usage(value_hint = usage_rs::ValueHint::FilePath, extensions("toml"))]
	pub scenes: PathBuf,

	/// The broadcast path the program is published at.
	#[usage(long, default = "studio")]
	pub broadcast: String,

	/// Copy and encode the program video only while someone watches it. This
	/// saves the work while nobody watches, but leaves a late viewer nothing
	/// from before they arrived. Audio is always encoded.
	#[usage(long)]
	pub on_demand: bool,

	/// The MoQ client configuration. Without `--connect`, nothing is published
	/// and broadcast sources stay offline.
	#[usage(flatten)]
	pub client: moq_tokio::connect::Config,

	/// QUIC transport tuning (`--quic-*`).
	#[usage(flatten)]
	pub quic: moq_tokio::quic::Config,

	/// The log configuration.
	#[usage(flatten)]
	pub log: moq_tokio::Log,
}

impl Config {
	/// Refuse a config parsed from released spellings, naming what replaced each.
	fn check_deprecated(&self) -> anyhow::Result<()> {
		let mut deprecated = self.client.deprecated();
		deprecated.extend(self.quic.deprecated());
		anyhow::ensure!(deprecated.is_empty(), "{deprecated}");
		Ok(())
	}
}

fn main() -> anyhow::Result<()> {
	let config = Config::parse();
	config.log.init()?;
	config.check_deprecated()?;
	moq_tokio::cli::check_env(&[Config::spec()], &[])?;

	let runtime = tokio::runtime::Runtime::new()?;
	// The QUIC client binds its socket on the current runtime.
	let _runtime = runtime.enter();
	let gpu = pollster::block_on(engine::Gpu::new())?;
	let connect = match config.client.url.clone() {
		Some(url) => Some((config.client.clone().init(config.quic.clone())?, url)),
		None => None,
	};
	let launch = engine::Launch {
		gpu: gpu.clone(),
		path: config.scenes.clone(),
		connect,
		broadcast: config.broadcast.clone(),
		on_demand: config.on_demand,
	};
	let exists = config
		.scenes
		.try_exists()
		.with_context(|| format!("reading {}", config.scenes.display()))?;
	let app = match exists {
		true => {
			let collection = engine::load(&config.scenes)?;
			App::Starting(runtime.block_on(engine::Engine::start(launch, collection))?)
		}
		false => App::Setup(Box::new(setup::Setup::new(launch, runtime.handle().clone()))),
	};

	run(app, gpu, runtime.handle().clone())
}

/// What the window shows.
enum App {
	/// First launch: guided setup, until it starts the studio.
	Setup(Box<setup::Setup>),
	/// An engine whose studio is built once the window can draw it.
	Starting(Arc<engine::Engine>),
	Studio(Box<ui::Studio>),
}

/// The window's contents, and what building the studio takes.
struct Window {
	app: App,
	render: eframe::egui_wgpu::RenderState,
	runtime: tokio::runtime::Handle,
}

impl eframe::App for Window {
	fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		let started = match &mut self.app {
			App::Studio(studio) => return studio.ui(ui, frame),
			App::Setup(setup) => match setup.ui(ui) {
				Some(started) => started,
				None => return,
			},
			App::Starting(engine) => setup::Started {
				engine: engine.clone(),
				message: None,
			},
		};
		let studio = ui::Studio::new(&self.render, started.engine, self.runtime.clone(), started.message);
		self.app = App::Studio(Box::new(studio));
		ui.ctx().request_repaint();
	}
}

/// Show the window until it is closed.
fn run(app: App, gpu: engine::Gpu, runtime: tokio::runtime::Handle) -> anyhow::Result<()> {
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_title("MoQ Studio")
			.with_app_id("moq-studio")
			.with_inner_size([1440.0, 900.0]),
		wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
			// The window draws on the device the compositor does, so preview and
			// program reach the screen without leaving the GPU.
			wgpu_setup: eframe::egui_wgpu::WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
				instance: gpu.instance,
				adapter: gpu.adapter,
				device: gpu.device,
				queue: gpu.queue,
			}),
			..Default::default()
		},
		..Default::default()
	};
	eframe::run_native(
		"moq-studio",
		options,
		Box::new(move |cc| {
			let render = cc
				.wgpu_render_state
				.clone()
				.context("the window is not drawn with wgpu")?;
			Ok(Box::new(Window { app, render, runtime }))
		}),
	)
	.map_err(|err| anyhow::anyhow!("{err}"))
	.context("the window failed")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_a_scene_file_and_a_relay() {
		let config = Config::parse_from(&[
			std::ffi::OsStr::new("show.toml"),
			std::ffi::OsStr::new("--connect"),
			std::ffi::OsStr::new("http://localhost:4443"),
		])
		.expect("a scene file and a relay should parse");
		config.check_deprecated().unwrap();
		assert_eq!(config.scenes, PathBuf::from("show.toml"));
		assert_eq!(config.broadcast, "studio");
		assert!(!config.on_demand, "the program encodes whatever the demand by default");
		assert_eq!(config.client.url.unwrap().port(), Some(4443));
	}
}
