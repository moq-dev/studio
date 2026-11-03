//! MoQ Studio: compose scenes from cameras, screens, images, and other
//! broadcasts on the GPU, and go live over MoQ.
//!
//! The scenes, sources, and compositor live in `moq-scene`; this binary is the
//! window around them and the program clock that publishes the result.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

mod autosave;
mod engine;
mod health;
mod ui;

#[derive(usage_rs::Cli, Clone)]
#[usage(unknown_flags = "error", args_override_self = false)]
#[usage(name = "moq-studio")]
#[usage(completion, settings)]
pub struct Config {
	/// The scene collection to open, saved as it changes. Created if it does not exist.
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

	/// A second relay to publish the same broadcast to, so a viewer linked to
	/// both resumes from it when the `--connect` relay fails. Both must speak
	/// the one `--connect-version`, which must carry publisher epochs. Its
	/// credential rides its URL's `?jwt=`: `--connect-token` goes to the
	/// `--connect` relay only.
	#[usage(long)]
	pub failover: Option<url::Url>,

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

	/// The relays to publish to, the `--connect` one first; none without it.
	fn relays(&self) -> anyhow::Result<Vec<engine::Relay>> {
		let Some(url) = self.client.url.clone() else {
			anyhow::ensure!(self.failover.is_none(), "--failover needs --connect");
			return Ok(Vec::new());
		};
		if self.failover.is_some() {
			self.check_failover()?;
		}
		let client = self.client.clone().init(self.quic.clone())?;
		let mut relays = vec![engine::Relay { client, url }];
		if let Some(url) = self.failover.clone() {
			let mut client = self.client.clone();
			// Its own credentials ride its URL; another operator's never reach it.
			client.tokens.clear();
			let client = client
				.init(self.quic.clone())
				.context("cannot set up the failover relay")?;
			relays.push(engine::Relay { client, url });
		}
		Ok(relays)
	}

	/// Refuse relays a viewer could not move between without a restart: a copy
	/// resumes mid-group only under the same epoch, which needs both links on
	/// one protocol version that carries it.
	fn check_failover(&self) -> anyhow::Result<()> {
		let [version] = self.client.version.as_slice() else {
			anyhow::bail!("--failover needs exactly one --connect-version, so both relays negotiate the same protocol");
		};
		anyhow::ensure!(
			matches!(version, moq_net::Version::Lite(lite) if lite.has_epoch()),
			"--failover needs a --connect-version with publisher epochs (moq-lite-07 or newer), not {version}, \
			 or a viewer moving between relays restarts the stream rather than resuming it"
		);
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
	let relays = config.relays()?;
	let gpu = pollster::block_on(engine::Gpu::new())?;
	let (autosave, loaded) = autosave::Autosave::open(config.scenes.clone())?;
	let engine = runtime.block_on(engine::Engine::start(
		gpu.clone(),
		autosave,
		loaded,
		relays,
		&config.broadcast,
		config.on_demand,
	))?;

	let ran = run(engine.clone(), gpu, runtime.handle().clone());
	// The edits since the last autosave, so quitting loses none.
	let saved = engine.save();
	if let (Err(err), Err(_)) = (&saved, &ran) {
		tracing::error!(error = format!("{err:#}"), "cannot save on quit");
	}
	ran?;
	saved
}

/// Show the window until it is closed.
fn run(engine: Arc<engine::Engine>, gpu: engine::Gpu, runtime: tokio::runtime::Handle) -> anyhow::Result<()> {
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
		Box::new(move |cc| Ok(Box::new(ui::Studio::new(cc, engine, runtime)?))),
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

	/// A failover relay is accepted only on one protocol version that carries
	/// publisher epochs, so a viewer moving between the relays resumes rather
	/// than restarts.
	#[test]
	fn failover_needs_one_version_with_epochs() {
		let check = |versions: &[&str]| {
			let mut args = vec!["show.toml", "--connect", "https://a/", "--failover", "https://b/"];
			for version in versions {
				args.extend(["--connect-version", version]);
			}
			let args: Vec<_> = args.into_iter().map(std::ffi::OsStr::new).collect();
			Config::parse_from(&args).unwrap().check_failover()
		};
		check(&["moq-lite-07-wip"]).expect("one version with epochs");
		for refused in [
			&[][..],
			&["moq-lite-06"],
			&["moq-transport-22"],
			&["moq-lite-07-wip", "moq-lite-06"],
		] {
			assert!(check(refused).is_err(), "{refused:?} was accepted");
		}
	}
}
