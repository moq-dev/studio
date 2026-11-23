//! A user's journey through a headless studio, driven as an agent drives it:
//! `moq-studio --headless --control <socket>` and `moq-studio ctl` calls,
//! naming widgets by their accessible names. Needs a GPU, or lavapipe as
//! `just render-sw` sets up.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::Duration;

use serde_json::Value;

/// Bounds the studio's start: lavapipe compiles every shader on a cold run.
const START: Duration = Duration::from_secs(60);

fn program() -> PathBuf {
	PathBuf::from(env!("CARGO_BIN_EXE_moq-studio"))
}

/// A headless studio on an empty show, killed if the test fails before it
/// quits.
struct Studio {
	child: Child,
	socket: PathBuf,
	dir: tempfile::TempDir,
}

impl Studio {
	async fn start() -> Self {
		let dir = tempfile::tempdir().unwrap();
		let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../moq-scene/src/test_data/tone.webm");
		std::fs::copy(data, dir.path().join("tone.webm")).unwrap();
		std::fs::write(
			dir.path().join("show.toml"),
			"version = 1\n\n[canvas]\nwidth = 1280\nheight = 720\nframerate = 30\n\n[[scenes]]\nname = \"Main\"\n",
		)
		.unwrap();
		let socket = dir.path().join("studio.sock");
		let child = Command::new(program())
			.current_dir(dir.path())
			.arg("--headless")
			.arg("--control")
			.arg(&socket)
			.arg("show.toml")
			// Keeps the studio's settings and recent shows out of the user's.
			.env("XDG_CONFIG_HOME", dir.path().join("config"))
			.env("XDG_DATA_HOME", dir.path().join("data"))
			.env("XDG_STATE_HOME", dir.path().join("state"))
			.spawn()
			.unwrap();
		let mut studio = Self { child, socket, dir };
		studio.bound().await;
		studio
	}

	/// Waits for the studio to bind its socket. A connection made then waits
	/// in the backlog until the window serves it, so binding is enough.
	async fn bound(&mut self) {
		let waited = kio::clock::timeout(START, async {
			while !self.socket.exists() {
				if let Some(status) = self.child.try_wait().unwrap() {
					panic!("the studio exited before binding its socket: {status}");
				}
				kio::clock::sleep(Duration::from_millis(20)).await;
			}
		})
		.await;
		waited.expect("the studio binds its socket");
	}

	/// Runs `moq-studio ctl` with `args`.
	fn run(&self, args: &[&str]) -> Output {
		Command::new(program())
			.arg("ctl")
			.arg("--socket")
			.arg(&self.socket)
			.args(["--reply", "60s"])
			.args(args)
			.output()
			.unwrap()
	}

	/// Runs `moq-studio ctl` with `args`, which must succeed, and returns
	/// what it printed.
	fn ctl(&self, args: &[&str]) -> String {
		let output = self.run(args);
		assert!(
			output.status.success(),
			"ctl {args:?}: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		String::from_utf8(output.stdout).unwrap()
	}

	/// The window's tree and the studio's state.
	fn snapshot(&self) -> Value {
		let printed: Value = serde_json::from_str(&self.ctl(&["snapshot"])).unwrap();
		printed["snapshot"].clone()
	}

	/// Waits for the state at `pointer` to be `value`.
	fn until(&self, pointer: &str, value: &str) {
		self.ctl(&["wait", "--state", &format!("{pointer}={value}"), "--timeout", "20s"]);
	}

	/// Fails on every control in view that an agent cannot name.
	fn audit(&self, page: &str) {
		let snapshot = self.snapshot();
		let mut unnamed = Vec::new();
		unnamed_controls(&snapshot["tree"], false, &mut unnamed);
		assert!(unnamed.is_empty(), "unnamed controls on {page}: {unnamed:#?}");
	}
}

impl Drop for Studio {
	fn drop(&mut self) {
		if self.child.try_wait().unwrap().is_none() {
			let _ = self.child.kill();
			let _ = self.child.wait();
		}
	}
}

fn control(node: &Value) -> bool {
	let actions = node["actions"].as_array().map(Vec::as_slice).unwrap_or_default();
	node["role"] != "Label" && actions.iter().any(|action| action == "click" || action == "focus")
}

/// The controls under `node`, not counting those inside other controls.
fn controls(node: &Value) -> Vec<&Value> {
	let mut found = Vec::new();
	for child in node["children"].as_array().into_iter().flatten() {
		match control(child) {
			true => found.push(child),
			false => found.extend(controls(child)),
		}
	}
	found
}

/// Collects the controls with no name of their own, nor from a named group
/// holding only them. egui's scroll bars are left out, and so are containers
/// that hold other controls, which an agent targets through those.
fn unnamed_controls(node: &Value, grouped: bool, unnamed: &mut Vec<String>) {
	let named = |node: &Value| node["label"].as_str().is_some_and(|label| !label.is_empty());
	let grouped = match node["role"] == "Group" && named(node) {
		true => controls(node).len() == 1,
		false => grouped,
	};
	if control(node) && !named(node) && !grouped && node["role"] != "ScrollBar" && controls(node).is_empty() {
		unnamed.push(format!("{} {} at {}", node["role"], node["id"], node["rect"]));
	}
	for child in node["children"].as_array().into_iter().flatten() {
		unnamed_controls(child, grouped, unnamed);
	}
}

/// The node `label` names, searching the whole tree.
fn find<'a>(node: &'a Value, label: &str) -> Option<&'a Value> {
	if node["label"] == label {
		return Some(node);
	}
	node["children"]
		.as_array()
		.into_iter()
		.flatten()
		.find_map(|child| find(child, label))
}

#[tokio::test]
#[ignore = "needs a GPU: `just render-sw` runs it on lavapipe"]
async fn an_agent_builds_a_show() {
	let studio = Studio::start().await;
	studio.until("/app", "\"studio\"");
	studio.audit("the live page");

	// Add a media source, as a user would, from the Build page's Add tab.
	studio.ctl(&["click", "Build"]);
	studio.ctl(&["click", "Add"]);
	studio.audit("the add tab");
	studio.ctl(&["click", "Name"]);
	studio.ctl(&["type", "tone"]);
	studio.ctl(&["click", "Kind"]);
	studio.ctl(&["click", "Media file"]);
	// The name, through the run's own clipboard.
	studio.ctl(&["click", "Name"]);
	studio.ctl(&["key", "ctrl+a"]);
	studio.ctl(&["key", "ctrl+c"]);
	studio.ctl(&["click", "file"]);
	studio.ctl(&["key", "ctrl+v"]);
	studio.ctl(&["type", ".webm"]);
	studio.ctl(&["click", "Add source"]);
	studio.ctl(&["wait", "--settled", "--timeout", "20s"]);
	let state = &studio.snapshot()["state"]["studio"];
	assert_eq!(state["show"]["sources"]["tone"]["path"], "tone.webm", "{state:#}");
	assert_eq!(state["show"]["scenes"][0]["items"][0]["source"], "tone", "{state:#}");
	assert_eq!(state["sources"]["tone"]["track"], "live", "{state:#}");

	// Select it on the canvas, and inspect it.
	studio.ctl(&["key", "Escape"]);
	studio.until("/studio/selection", "null");
	studio.ctl(&["click", "Canvas"]);
	studio.until("/studio/selection/source", "\"tone\"");
	studio.ctl(&["wait", "--shown", "Slot"]);
	studio.audit("the inspector");

	// Pull its fader down to near the bottom of its travel.
	studio.ctl(&["click", "Mix"]);
	studio.audit("the mix page");
	let snapshot = studio.snapshot();
	let fader = find(&snapshot["tree"], "tone fader").expect("tone's fader");
	let [x0, y0, x1, y1] = [0, 1, 2, 3].map(|i| fader["rect"][i].as_f64().unwrap());
	let to = format!("at:{},{}", (x0 + x1) / 2.0, y0 + (y1 - y0) * 0.9);
	studio.ctl(&["drag", "tone fader", &to]);
	let gain = studio.snapshot()["state"]["studio"]["show"]["audio"]["tone"]["gain"]
		.as_f64()
		.expect("tone has a gain");
	assert!(gain < 0.5, "the fader set a gain of {gain}");

	// A picture of the window, as PNG.
	let shot = studio.dir.path().join("shot.png");
	studio.ctl(&["screenshot", shot.to_str().unwrap()]);
	let png = std::fs::read(&shot).unwrap();
	assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
	assert!(png.len() > 10_000, "a {} byte picture holds no window", png.len());

	studio.ctl(&["click", "Settings"]);
	studio.ctl(&["wait", "--shown", "Scene sizes"]);
	studio.audit("the settings");

	// A key pressed binds a hotkey, as the desktop's would.
	studio.ctl(&["click", "Hotkeys"]);
	studio.ctl(&["click", "Bind Go live"]);
	studio.ctl(&["key", "F9"]);
	studio.ctl(&["wait", "--shown", "Rebind Go live: F9"]);
	studio.audit("the hotkeys");

	// Hotkeys runs past the dialog's foot, so a scroll moves it up.
	let top = |studio: &Studio| {
		find(&studio.snapshot()["tree"], "Bind Panic").expect("a Panic hotkey")["rect"][1]
			.as_f64()
			.unwrap()
	};
	let before = top(&studio);
	studio.ctl(&["scroll", "Bind Panic", "--dy", "40"]);
	assert_eq!(top(&studio), before - 40.0, "scrolled by the points asked");

	// A target that names nothing fails, and says so.
	let missing = studio.run(&["click", "No such button"]);
	assert!(!missing.status.success());
	assert!(String::from_utf8_lossy(&missing.stderr).contains("No such button"));

	let mut studio = studio;
	studio.ctl(&["quit"]);
	let status = studio.child.wait().unwrap();
	assert!(status.success(), "the studio exited with {status}");
	assert!(!studio.socket.exists(), "the studio left its socket behind");
}
