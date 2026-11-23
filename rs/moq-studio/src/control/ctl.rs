//! `moq-studio ctl`: send one request to a studio's `--control` socket and
//! print its answer.

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;

use super::{Button, Request, Target, Until};

/// The studio's own command line, when it starts with `ctl`.
#[derive(usage_rs::Cli)]
#[usage(name = "moq-studio", unknown_flags = "error")]
pub struct Driver {
	#[usage(subcommand)]
	command: Top,
}

#[derive(usage_rs::Subcommands)]
enum Top {
	Ctl(Ctl),
}

/// Drive a studio started with `--control <socket>`.
///
/// A target is a widget's exact name as `snapshot` lists it, `contains:<text>`
/// for one whose name holds the text, `id:<n>` for a node ID, `at:<x>,<y>`
/// for a point in logical pixels, or `label:<text>` for a name that starts
/// like one of those.
#[derive(usage_rs::Args)]
struct Ctl {
	/// The socket the studio was started with `--control` on.
	#[usage(long, value_hint = usage_rs::ValueHint::FilePath)]
	socket: PathBuf,

	/// How long to wait for the studio to answer, past a `wait`'s own
	/// timeout.
	#[usage(long, default = "30s")]
	reply: humantime::Duration,

	#[usage(subcommand)]
	command: Command,
}

#[derive(usage_rs::Subcommands)]
enum Command {
	Snapshot(Snapshot),
	Screenshot(Screenshot),
	Click(Click),
	RightClick(Click),
	DoubleClick(Click),
	Hover(Click),
	Drag(Drag),
	Scroll(Scroll),
	Type(Type),
	Key(Key),
	Wait(Wait),
	Quit(Quit),
}

/// Print the window's accessibility tree and the studio's state as JSON.
#[derive(usage_rs::Args)]
struct Snapshot;

/// Save the window's picture as PNG.
#[derive(usage_rs::Args)]
struct Screenshot {
	/// Where to write it.
	#[usage(value_hint = usage_rs::ValueHint::FilePath)]
	out: PathBuf,
}

/// Act with the pointer on a target.
#[derive(usage_rs::Args)]
struct Click {
	target: String,
}

/// Press on one target, move to another, and let go.
#[derive(usage_rs::Args)]
struct Drag {
	from: String,
	to: String,
}

/// Scroll what is under a target, by points: positive `dy` brings what is
/// below into view.
#[derive(usage_rs::Args)]
struct Scroll {
	target: String,
	#[usage(long, default = "0")]
	dx: f32,
	#[usage(long, default = "0")]
	dy: f32,
}

/// Type text into the focused widget.
#[derive(usage_rs::Args)]
struct Type {
	text: String,
}

/// Press a key, such as `Escape`, `Enter`, or `ctrl+z`.
#[derive(usage_rs::Args)]
struct Key {
	combo: String,
}

/// Block until a widget shows or goes, the sources settle, or the state
/// holds a value.
#[derive(usage_rs::Args)]
struct Wait {
	#[usage(arg_group)]
	until: Condition,
	/// How long to wait before failing.
	#[usage(long, default = "10s")]
	timeout: humantime::Duration,
}

#[derive(usage_rs::ArgGroup)]
#[usage(name = "until")]
enum Condition {
	/// Until a target is in the window.
	Shown(String),
	/// Until no widget is the target.
	Gone(String),
	/// Until every source has started and the program drew a second since.
	Settled,
	/// Until the snapshot's state holds a value: `<JSON pointer>=<JSON>`,
	/// such as `/studio/program="Main"`.
	State(String),
}

/// Close the window, as its Close button does, ending the studio.
#[derive(usage_rs::Args)]
struct Quit;

impl Driver {
	/// Run `moq-studio ctl`, as the process's arguments say.
	pub fn run() -> anyhow::Result<()> {
		let Top::Ctl(ctl) = Self::parse().command;
		let (request, out) = ctl.command.request()?;
		let mut timeout = *ctl.reply;
		if let Request::Wait { timeout: waiting, .. } = &request {
			timeout += *waiting;
		}
		let stream = std::os::unix::net::UnixStream::connect(&ctl.socket)
			.with_context(|| format!("cannot reach a studio at {}", ctl.socket.display()))?;
		stream.set_read_timeout(Some(timeout))?;
		let mut line = serde_json::to_vec(&request)?;
		line.push(b'\n');
		(&stream).write_all(&line)?;

		let mut read = std::io::BufReader::new(&stream);
		let mut answer = String::new();
		match read.read_line(&mut answer) {
			Ok(0) => anyhow::bail!("the studio hung up without answering"),
			Ok(_) => {}
			Err(err)
				if matches!(
					err.kind(),
					std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
				) =>
			{
				anyhow::bail!("no answer within {}", humantime::format_duration(timeout))
			}
			Err(err) => return Err(err).context("cannot read the studio's answer"),
		}
		let answer: serde_json::Value = serde_json::from_str(&answer).context("the studio's answer is not JSON")?;
		if let Some(err) = answer.get("error") {
			anyhow::bail!("{}", err.as_str().unwrap_or("the studio failed"));
		}
		match out {
			Some(out) => {
				let length = answer["screenshot"]["png"]
					.as_u64()
					.context("the answer is no screenshot")?;
				let mut png = Vec::new();
				read.take(length).read_to_end(&mut png)?;
				anyhow::ensure!(png.len() as u64 == length, "the screenshot ended early");
				std::fs::write(&out, png).with_context(|| format!("cannot write {}", out.display()))?;
			}
			None if answer == "done" => {}
			None => println!("{}", serde_json::to_string_pretty(&answer)?),
		}
		Ok(())
	}
}

impl Command {
	/// The request this command sends, and where a screenshot goes.
	fn request(self) -> anyhow::Result<(Request, Option<PathBuf>)> {
		let click = |click: Click, button, double| -> anyhow::Result<Request> {
			Ok(Request::Click {
				target: Target::parse(&click.target)?,
				button,
				double,
			})
		};
		let request = match self {
			Self::Snapshot(_) => Request::Snapshot {},
			Self::Screenshot(screenshot) => return Ok((Request::Screenshot {}, Some(screenshot.out))),
			Self::Click(target) => click(target, Button::Primary, false)?,
			Self::RightClick(target) => click(target, Button::Secondary, false)?,
			Self::DoubleClick(target) => click(target, Button::Primary, true)?,
			Self::Hover(hover) => Request::Hover {
				target: Target::parse(&hover.target)?,
			},
			Self::Drag(drag) => Request::Drag {
				from: Target::parse(&drag.from)?,
				to: Target::parse(&drag.to)?,
			},
			Self::Scroll(scroll) => Request::Scroll {
				target: Target::parse(&scroll.target)?,
				dx: scroll.dx,
				dy: scroll.dy,
			},
			Self::Type(text) => Request::Type { text: text.text },
			Self::Key(key) => Request::Key { combo: key.combo },
			Self::Wait(wait) => Request::Wait {
				until: match wait.until {
					Condition::Shown(target) => Until::Shown(Target::parse(&target)?),
					Condition::Gone(target) => Until::Gone(Target::parse(&target)?),
					Condition::Settled => Until::Settled,
					Condition::State(state) => {
						let (pointer, value) = state.split_once('=').context("a state is `<JSON pointer>=<JSON>`")?;
						Until::State {
							pointer: pointer.to_string(),
							value: serde_json::from_str(value)
								.with_context(|| format!("`{value}` is not JSON; quote a string"))?,
						}
					}
				},
				timeout: Duration::from(wait.timeout),
			},
			Self::Quit(_) => Request::Quit {},
		};
		Ok((request, None))
	}
}
