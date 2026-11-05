//! Browser sources against the real helper: this package's binary, built with
//! `cef`, rendering `data:` pages, or pages served on loopback, with nothing
//! fetched from the network.

#![cfg(all(target_os = "linux", feature = "cef"))]

use std::path::Path;
use std::time::Duration;

use kio::clock::Instant;
use moq_scene::browser::{Button, Helper, Input, Modifiers};
use moq_scene::source::{Context, Sources, Status};
use moq_scene::{Canvas, Collection, Compositor, Item, Rect, Rgba, Scene, Source, wgpu};

/// Bounds each wait: CEF's first start on a cold disk takes seconds.
const TIMEOUT: Duration = Duration::from_secs(20);

const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A studio of one scene, "Main", drawing `sources` back to front over the
/// whole of a small canvas. Dropping it stops its helper.
struct Studio {
	collection: Collection,
	sources: Sources,
	compositor: Compositor,
	device: wgpu::Device,
	queue: wgpu::Queue,
	/// Advances the bus, which meters each source's strip.
	mixer: moq_audio::bus::Driver,
	helper: Helper,
	_origin: moq_net::origin::Producer,
	_dir: tempfile::TempDir,
	_profile: Option<tempfile::TempDir>,
}

impl Studio {
	/// A studio with a fresh browser profile.
	async fn new(sources: &[(&str, Source)]) -> Self {
		let profile = tempfile::tempdir().unwrap();
		let mut studio = Self::on(profile.path(), sources).await;
		studio._profile = Some(profile);
		studio
	}

	/// A studio whose pages keep their cookies in `profile`.
	async fn on(profile: &Path, sources: &[(&str, Source)]) -> Self {
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
			items: sources.iter().map(|(name, _)| Item::new(*name, fill)).collect(),
			..Scene::new("Main")
		}];
		for (name, source) in sources {
			collection.sources.insert(name.to_string(), source.clone());
		}
		collection.validate().unwrap();

		let dir = tempfile::tempdir().unwrap();
		let (origin, driver) = moq_net::origin::Producer::new(Default::default());
		tokio::spawn(moq_net::time::run(driver));
		let (bus, mixer) = moq_audio::bus::Bus::new(Default::default()).unwrap();
		let helper = Helper::new(env!("CARGO_BIN_EXE_moq-studio"), profile, &adapter, &device, &queue);
		let mut sources = Sources::new(Context {
			bus,
			origin: origin.consume(),
			dir: dir.path().to_path_buf(),
			output: moq_video::render::output(&device),
			publish: None,
			browser: Some(helper.clone()),
		});
		for driver in sources.sync(&collection) {
			tokio::task::spawn_local(driver.run());
		}
		let mut compositor = Compositor::new(&device, &queue, collection.canvas).unwrap();
		compositor.switcher_mut(None).set_preview(Some("Main".into()));
		compositor.switcher_mut(None).cut();
		Self {
			collection,
			sources,
			compositor,
			device,
			queue,
			mixer,
			helper,
			_origin: origin,
			_dir: dir,
			_profile: None,
		}
	}

	/// Restart the source `name`, as its Reload button does.
	fn reload(&mut self, name: &str) {
		self.sources.restart(name);
		for driver in self.sources.sync(&self.collection) {
			tokio::task::spawn_local(driver.run());
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

impl Drop for Studio {
	fn drop(&mut self) {
		self.helper.stop();
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
		while strip.level().peak() <= 0.0 {
			assert!(Instant::now() < deadline, "the page's tone never reached the bus");
			studio.mixer.advance(480);
			kio::clock::sleep(Duration::from_millis(10)).await;
		}
	})
	.await;
}

/// A page gets WebGL and WebGL2 contexts, and what it draws with them shows.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_webgl_page_draws() {
	local(async {
		// Red unless both contexts exist, and then green from WebGL2.
		let html = "<body style='margin:0;background:red'>\
			<canvas id=c width=320 height=240 style='display:block'></canvas><script>\
			const gl = document.getElementById('c').getContext('webgl2');\
			const gl1 = document.createElement('canvas').getContext('webgl');\
			if (gl && gl1) { gl.clearColor(0, 1, 0, 1); gl.clear(gl.COLOR_BUFFER_BIT); }\
			</script></body>";
		let mut studio = Studio::new(&[("page", page(html))]).await;
		studio.until_center(GREEN, &[]).await;
	})
	.await;
}

/// A click reaches the page where the preview put it, and a popup it asks for
/// opens nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_click_reaches_the_page() {
	local(async {
		let html = "<body style='margin:0;height:100vh;background:red' onclick=\"\
			const popup = window.open('about:blank');\
			document.body.style.background = popup ? 'green' : 'blue'\"></body>";
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
		assert_eq!(studio.sources.get("page").unwrap().status(), Status::Live);
	})
	.await;
}

/// A `<select>`'s list, which CEF paints apart from the page, draws over the
/// page, and a click on it picks an option like a click on the page would.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_dropdown_draws_and_takes_clicks() {
	local(async {
		// A select across the top, whose list of lime options covers the
		// center once open; picking one turns the page blue.
		let options = "<option style='background:lime'>option</option>".repeat(12);
		let html = format!(
			"<body style='margin:0;height:100vh;background:red'>\
			<select style='width:320px;font-size:24px' onchange=\"document.body.style.background='blue'\">\
			{options}</select></body>"
		);
		let mut studio = Studio::new(&[("page", page(&html))]).await;
		studio.until_center([255, 0, 0, 255], &[]).await;

		let page = studio.sources.get("page").unwrap().page().unwrap().clone();
		let click = |x, y| {
			for down in [true, false] {
				page.send(Input::Button {
					x,
					y,
					button: Button::Left,
					down,
					clicks: 1,
					modifiers: Modifiers::default(),
				});
			}
		};
		page.send(Input::Focus(true));
		click(160, 10);
		studio.until_center(GREEN, &[]).await;
		click(160, 120);
		studio.until_center(BLUE, &[]).await;
	})
	.await;
}

/// A page that closes itself fails its own source and leaves the canvas,
/// while the page beside it in the same helper keeps drawing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_page_that_closes_itself_fails_alone() {
	local(async {
		let closer = page("<body><script>addEventListener('load', () => window.close())</script></body>");
		let red = page("<body style='background:red'></body>");
		let mut studio = Studio::new(&[("red", red), ("closer", closer)]).await;
		let reason = studio.until_failed("closer").await;
		assert!(reason.contains("closed itself"), "{reason}");
		studio.until_center([255, 0, 0, 255], &["closer"]).await;
	})
	.await;
}

/// A page whose renderer crashes on its first navigation fails its own
/// source, while everything else keeps drawing. A crash of the helper every
/// page shares fails them all and nothing else, and a reload starts a new one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_crash_while_loading_drops_only_its_source() {
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

		studio.reload("browser");
		studio.until_center([255, 0, 0, 255], &["renderer"]).await;
	})
	.await;
}

/// A page whose renderer crashes after it has drawn fails its source and
/// leaves the canvas, and a reload draws it again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn a_crash_after_drawing_leaves_the_canvas() {
	local(async {
		let red = page("<body style='background:red'></body>");
		let mut studio = Studio::new(&[("blue", blue()), ("page", red)]).await;
		studio.until_center([255, 0, 0, 255], &[]).await;

		// Abort the page's renderer, and any spare one CEF keeps ready, which
		// shows no page, as a failed assertion would: a crash that dumps core.
		// (V8 takes a SIGSEGV that no fault raised and carries on.)
		let helper = helper().expect("the page's helper is running");
		let renderers = renderers(helper);
		assert!(!renderers.is_empty(), "the page has a renderer");
		for renderer in renderers {
			let crashed = std::process::Command::new("kill")
				.args(["-ABRT", &renderer.to_string()])
				.status();
			assert!(crashed.unwrap().success());
		}
		let reason = studio.until_failed("page").await;
		assert!(reason.contains("renderer"), "{reason}");
		studio.until_center(BLUE, &["page"]).await;

		studio.reload("page");
		studio.until_center([255, 0, 0, 255], &[]).await;
	})
	.await;
}

/// Cookies, even one without an expiry, survive a reload of the page and a
/// restart of the studio, since every page keeps them in one profile on disk.
/// A second studio on the profile while it is in use fails its pages saying
/// why, and leaves the first one's drawing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs CEF and a Vulkan device; run by just render-sw"]
async fn cookies_survive_a_restart() {
	local(async {
		// Red until it has seen its cookie, then green.
		let url = serve(
			"<body style='margin:0'><script>\
			document.body.style.background = document.cookie.includes('login=1') ? 'lime' : 'red';\
			document.cookie = 'login=1';\
			</script></body>",
		)
		.await;
		let page = Source::Browser {
			url,
			width: 320,
			height: 240,
		};
		let profile = tempfile::tempdir().unwrap();
		let mut studio = Studio::on(profile.path(), &[("page", page.clone())]).await;
		studio.until_center([255, 0, 0, 255], &[]).await;
		studio.reload("page");
		studio.until_center(GREEN, &[]).await;

		let second = Studio::on(profile.path(), &[("page", page.clone())]).await;
		let reason = second.until_failed("page").await;
		assert!(reason.contains("another studio"), "{reason}");
		drop(second);
		studio.until_center(GREEN, &[]).await;
		drop(studio);

		let mut studio = Studio::on(profile.path(), &[("page", page)]).await;
		studio.until_center(GREEN, &[]).await;
	})
	.await;
}

const GREEN: [u8; 4] = [0, 255, 0, 255];

/// Serve `html` over HTTP on loopback to every request, returning its URL,
/// since a cookie needs an origin a `data:` page lacks.
async fn serve(html: &'static str) -> String {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/", listener.local_addr().unwrap());
	tokio::spawn(async move {
		loop {
			let Ok((mut stream, _)) = listener.accept().await else {
				return;
			};
			tokio::spawn(async move {
				// The request, up to the blank line that ends its headers.
				let mut request = Vec::new();
				let mut buffer = [0; 1024];
				while !request.ends_with(b"\r\n\r\n") {
					match stream.read(&mut buffer).await {
						Ok(0) | Err(_) => return,
						Ok(read) => request.extend_from_slice(&buffer[..read]),
					}
				}
				let response = format!(
					"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
					html.len()
				);
				let _ = stream.write_all(response.as_bytes()).await;
			});
		}
	});
	url
}

/// The one helper this test process still runs: a child started with the
/// helper's argument.
fn helper() -> Option<u32> {
	let ours = std::process::id();
	processes()
		.into_iter()
		.find(|process| process.parent == ours && process.args.iter().any(|arg| arg == b"--moq-browser-helper"))
		.map(|process| process.pid)
}

/// The renderers under `helper`. CEF forks them from its zygote, whose
/// arguments they keep, so they are told apart by the compositor thread every
/// renderer runs.
fn renderers(helper: u32) -> Vec<u32> {
	let processes = processes();
	let mut family = vec![helper];
	let mut at = 0;
	while let Some(&parent) = family.get(at) {
		family.extend(
			processes
				.iter()
				.filter(|process| process.parent == parent)
				.map(|process| process.pid),
		);
		at += 1;
	}
	family
		.into_iter()
		.filter(|pid| {
			let threads = std::fs::read_dir(format!("/proc/{pid}/task"))
				.into_iter()
				.flatten()
				.flatten();
			threads
				.filter_map(|thread| std::fs::read_to_string(thread.path().join("comm")).ok())
				.any(|name| name.trim_end() == "Compositor")
		})
		.collect()
}

struct Process {
	pid: u32,
	parent: u32,
	args: Vec<Vec<u8>>,
}

/// Every process running.
fn processes() -> Vec<Process> {
	let Ok(entries) = std::fs::read_dir("/proc") else {
		return Vec::new();
	};
	entries
		.flatten()
		.filter_map(|entry| {
			let pid = entry.file_name().to_str()?.parse().ok()?;
			let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
			// The parent is the second field after the parenthesized command.
			let parent = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
			let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
			let args = cmdline.split(|byte| *byte == 0).map(<[u8]>::to_vec).collect();
			Some(Process { pid, parent, args })
		})
		.collect()
}
