//! Browser sources against the real helper: this package's binary, built with
//! `cef`, rendering `data:` pages with nothing fetched from the network.

#![cfg(all(target_os = "linux", feature = "cef"))]

use std::time::Duration;

use kio::clock::Instant;
use moq_scene::browser::{Button, Helper, Input, Modifiers};
use moq_scene::source::{Context, Sources, Status};
use moq_scene::{Canvas, Collection, Compositor, Item, Rect, Rgba, Scene, Source, wgpu};

/// Bounds each wait: CEF's first start on a cold disk takes seconds.
const TIMEOUT: Duration = Duration::from_secs(20);

const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A studio of one scene, "Main", drawing `sources` back to front over the
/// whole of a small canvas.
struct Studio {
	collection: Collection,
	sources: Sources,
	compositor: Compositor,
	device: wgpu::Device,
	queue: wgpu::Queue,
	bus: moq_audio::bus::Bus,
	_origin: moq_net::origin::Producer,
	_dir: tempfile::TempDir,
}

impl Studio {
	async fn new(sources: &[(&str, Source)]) -> Self {
		let instance = wgpu::Instance::default();
		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions::default())
			.await
			.expect("a GPU adapter");
		let (device, queue) = moq_video::render::request_device(&adapter, &Default::default())
			.await
			.expect("a GPU device");

		let mut collection = Collection::new();
		collection.canvas = Canvas {
			width: 64,
			height: 64,
			framerate: 30,
		};
		let fill = Rect::fill(&collection.canvas);
		collection.scenes = vec![Scene {
			name: "Main".into(),
			items: sources
				.iter()
				.map(|(name, _)| Item {
					source: name.to_string(),
					rect: fill,
					visible: true,
				})
				.collect(),
		}];
		for (name, source) in sources {
			collection.sources.insert(name.to_string(), source.clone());
		}
		collection.validate().unwrap();

		let dir = tempfile::tempdir().unwrap();
		let (origin, driver) = moq_net::origin::Producer::new(Default::default());
		tokio::spawn(moq_net::time::run(driver));
		let bus = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let mut sources = Sources::new(Context {
			bus: bus.clone(),
			origin: origin.consume(),
			dir: dir.path().to_path_buf(),
			output: moq_video::render::output(&device),
			browser: Some(Helper::new(env!("CARGO_BIN_EXE_moq-studio"), &adapter, &device, &queue)),
		});
		for driver in sources.sync(&collection) {
			tokio::task::spawn_local(driver.run());
		}
		let mut compositor = Compositor::new(&device, &queue, collection.canvas).unwrap();
		compositor.set_preview(Some("Main".into()));
		compositor.cut();
		Self {
			collection,
			sources,
			compositor,
			device,
			queue,
			bus,
			_origin: origin,
			_dir: dir,
		}
	}

	/// The program's center pixel, freshly drawn.
	fn center(&mut self) -> [u8; 4] {
		self.compositor
			.render(&self.collection, &self.sources, Instant::now())
			.unwrap();
		let texture = self.compositor.program().clone();
		let canvas = self.collection.canvas;
		let row = (canvas.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
		let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
			label: None,
			size: u64::from(row * canvas.height),
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mut encoder = self.device.create_command_encoder(&Default::default());
		encoder.copy_texture_to_buffer(
			texture.as_image_copy(),
			wgpu::TexelCopyBufferInfo {
				buffer: &buffer,
				layout: wgpu::TexelCopyBufferLayout {
					offset: 0,
					bytes_per_row: Some(row),
					rows_per_image: None,
				},
			},
			texture.size(),
		);
		self.queue.submit([encoder.finish()]);
		buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
		self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
		let mapped = buffer.slice(..).get_mapped_range().unwrap();
		let at = (canvas.height / 2 * row + canvas.width / 2 * 4) as usize;
		mapped[at..at + 4].try_into().unwrap()
	}

	/// Redraw until the center pixel is within a step of `expected`.
	/// Fails at once if a source other than `crashing` ones fails.
	async fn until_center(&mut self, expected: [u8; 4], crashing: &[&str]) {
		let deadline = Instant::now() + TIMEOUT;
		loop {
			for (name, feed) in self.sources.iter() {
				if let Status::Failed(reason) = feed.status()
					&& !crashing.contains(&name)
				{
					panic!("{name} failed: {reason}");
				}
			}
			let actual = self.center();
			if near(actual, expected) {
				return;
			}
			assert!(
				Instant::now() < deadline,
				"the center stayed {actual:?}, not {expected:?}"
			);
			kio::clock::sleep(Duration::from_millis(30)).await;
		}
	}

	/// Wait until `name` leaves `Starting` and `Live` for good, and say why.
	async fn until_failed(&self, name: &str) -> String {
		let deadline = Instant::now() + TIMEOUT;
		loop {
			if let Status::Failed(reason) = self.sources.get(name).unwrap().status() {
				return reason;
			}
			assert!(Instant::now() < deadline, "{name} never failed");
			kio::clock::sleep(Duration::from_millis(30)).await;
		}
	}
}

fn near(actual: [u8; 4], expected: [u8; 4]) -> bool {
	actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2)
}

fn page(html: &str) -> Source {
	Source::Browser {
		url: format!("data:text/html,{html}"),
		width: 320,
		height: 240,
	}
}

fn blue() -> Source {
	Source::Color { color: Rgba(BLUE) }
}

/// Run `f` on a local task set, as drivers need.
async fn local<F: Future>(f: F) -> F::Output {
	tokio::task::LocalSet::new().run_until(f).await
}

/// A page draws over what is under it with its own alpha, and its sound
/// joins the bus.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_page_draws_and_plays() {
	local(async {
		// Half-transparent red over blue, and a tone.
		let html = "<body style='margin:0;background:rgba(255,0,0,0.5)'><script>\
			const audio = new AudioContext();\
			const tone = audio.createOscillator();\
			tone.connect(audio.destination);\
			tone.start();\
			</script></body>";
		let mut studio = Studio::new(&[("blue", blue()), ("page", page(html))]).await;
		studio.until_center([128, 0, 127, 255], &[]).await;
		let feed = studio.sources.get("page").unwrap();
		assert_eq!(feed.status(), Status::Live);
		assert!(feed.page().unwrap().paint().is_some(), "the paint path shows");

		let strip = feed.strip().unwrap().clone();
		let deadline = Instant::now() + TIMEOUT;
		let mut samples = vec![0.0; 960];
		while strip.level().peak() <= 0.0 {
			assert!(Instant::now() < deadline, "the page's tone never reached the bus");
			studio.bus.read(&mut samples);
			kio::clock::sleep(Duration::from_millis(10)).await;
		}
	})
	.await;
}

/// A click reaches the page where the preview put it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_click_reaches_the_page() {
	local(async {
		let html = "<body style='margin:0;height:100vh;background:red' \
			onclick=\"document.body.style.background='blue'\"></body>";
		let mut studio = Studio::new(&[("page", page(html))]).await;
		studio.until_center([255, 0, 0, 255], &[]).await;

		let page = studio.sources.get("page").unwrap().page().unwrap().clone();
		for down in [true, false] {
			page.send(Input::Button {
				x: 160,
				y: 120,
				button: Button::Left,
				down,
				clicks: 1,
				modifiers: Modifiers::default(),
			});
		}
		studio.until_center(BLUE, &[]).await;
	})
	.await;
}

/// A page whose renderer crashes, or whose whole browser process does, fails
/// its own source and leaves the canvas, while everything else keeps drawing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_crash_drops_only_its_source() {
	local(async {
		let crash = Source::Browser {
			url: "chrome://crash".into(),
			width: 320,
			height: 240,
		};
		let red = page("<body style='background:red'></body>");
		let mut studio = Studio::new(&[("blue", blue()), ("browser", red), ("renderer", crash)]).await;

		let reason = studio.until_failed("renderer").await;
		assert!(reason.contains("renderer"), "{reason}");

		studio.until_center([255, 0, 0, 255], &["renderer"]).await;
		let helper = helper().expect("the page's helper is running");
		let killed = std::process::Command::new("kill")
			.args(["-KILL", &helper.to_string()])
			.status();
		assert!(killed.unwrap().success());
		let reason = studio.until_failed("browser").await;
		assert!(reason.contains("browser process exited"), "{reason}");

		assert_eq!(studio.sources.get("blue").unwrap().status(), Status::Live);
		studio.until_center(BLUE, &["renderer", "browser"]).await;
	})
	.await;
}

/// The one helper this test process still runs: a child started with the
/// helper's argument.
fn helper() -> Option<u32> {
	let ours = std::process::id().to_string();
	std::fs::read_dir("/proc").ok()?.flatten().find_map(|entry| {
		let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
		let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
		// The parent is the second field after the parenthesized command.
		let parent = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?;
		let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
		let helper = cmdline
			.split(|byte| *byte == 0)
			.any(|arg| arg == b"--moq-browser-helper");
		(parent == ours && helper).then_some(pid)
	})
}
