//! Dropping and pasting onto the picture: a file dragged from a file manager
//! becomes the kind of layer it holds where it lands, an image, a video or
//! sound file, or a color grade on the layer under it, and a pasted file,
//! picture, or web address does the same at the pointer. While a drag is
//! over the window, the picture shows where the layer will land and what it
//! will become.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use moq_scene::file::{self, Picture};

use super::*;

/// What arrived on the window.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Arrival {
	/// A file dropped, or pasted from a file manager.
	File(PathBuf),
	/// Whatever picture the clipboard holds, pasted.
	Clipboard,
	/// A web address pasted.
	Page(url::Url),
}

impl Arrival {
	/// What pasted `text` brings, unless it is something else, such as layers:
	/// each line a file, as a file manager copies them, or a web address.
	pub(super) fn of_text(text: &str) -> Option<Vec<Self>> {
		let text = text.trim();
		let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
		let file = |line: &str| match url::Url::parse(line) {
			Ok(url) if url.scheme() == "file" => url.to_file_path().ok(),
			_ => Some(PathBuf::from(line)).filter(|path| path.is_absolute() && path.is_file()),
		};
		if let Some(files) = lines.iter().map(|line| file(line)).collect::<Option<Vec<_>>>()
			&& !files.is_empty()
		{
			return Some(files.into_iter().map(Self::File).collect());
		}
		match url::Url::parse(text) {
			Ok(url) if matches!(url.scheme(), "http" | "https") && !text.contains(char::is_whitespace) => {
				Some(vec![Self::Page(url)])
			}
			_ => None,
		}
	}
}

/// Where an arrival lands.
#[derive(Clone, Debug)]
struct Landing {
	/// The layers it joins.
	layer: Layer,
	/// The picture it lands in, Studio's while it edits a scene drawn in both,
	/// and that picture's canvas.
	format: Format,
	canvas: Canvas,
	/// Its center, in canvas pixels.
	at: [f32; 2],
	/// The layer under it on the picture, which a file can replace the file
	/// of or a color grade grades, with that layer as it was.
	onto: Option<(Picked, Item)>,
}

/// What a file read in the background becomes.
#[derive(Clone, Debug)]
enum Intent {
	/// A new layer at `landing`, or a sound alone, heard.
	Add(Landing),
	/// The new file of the source called this, which was the source given
	/// when the read began.
	Replace(String, Source),
	/// A grade on this layer, which was the item given when the read began.
	Grade(Picked, Item),
}

/// A source read and copied beside the scene file, ready to add.
struct Prepared {
	source: Source,
	picture: Picture,
	/// The name it is added under, unless taken.
	name: String,
}

/// A file to read in the background: its name, where it was dropped in
/// canvas pixels, and what it is for.
struct Job {
	name: String,
	at: [f32; 2],
	intent: Intent,
}

/// A file read in the background, and what it is for.
struct Done {
	job: u64,
	intent: Intent,
	result: anyhow::Result<Prepared>,
}

/// A drop on a layer that takes a file of its kind, which asks whether to
/// replace that file or add a layer.
struct Offer {
	source: Source,
	landing: Landing,
	/// The name of the source shown by the layer dropped on, and that source.
	shown: String,
	was: Source,
}

/// What the picture knows of a file dragged over it, read once per drag.
#[derive(Clone)]
struct Guess {
	/// The source it would become, or why it cannot.
	source: Result<Source, String>,
	/// The picture's size, where its header says it; a media file's needs the
	/// whole file read, which waits for the drop.
	size: Option<[f32; 2]>,
}

/// The drops and pastes under way.
pub(super) struct Dropping {
	/// Where the pointer is during a drag, which the window cannot hear.
	pointer: crate::pointer::Pointer,
	/// Where the pointer is this frame, while files are dragged over the
	/// window.
	dragging: Option<egui::Pos2>,
	/// What each file dragged over the window would become.
	guesses: HashMap<PathBuf, Guess>,
	/// Arrivals this frame, each with where the pointer was on screen, which
	/// the picture places where they arrived on it, or the window at the
	/// edited scene's center once it has drawn.
	arrived: Vec<(Arrival, Option<egui::Pos2>)>,
	/// Where a menu's Paste lands, on the picture or (`None`) the scene's
	/// center, while it waits on the clipboard's text, and the frames waited.
	menu_paste: Option<(Option<egui::Pos2>, u32)>,
	/// Where the picture's menu was last opened.
	menu_at: Option<egui::Pos2>,
	/// The files being read, by job, with their names and where they land.
	busy: Vec<(u64, String, [f32; 2])>,
	jobs: u64,
	done: (mpsc::Sender<Done>, mpsc::Receiver<Done>),
	offer: Option<Offer>,
	/// Where the last failure at a drop happened, in canvas pixels.
	failed_at: [f32; 2],
}

impl Default for Dropping {
	fn default() -> Self {
		Self {
			pointer: Default::default(),
			dragging: None,
			guesses: HashMap::new(),
			arrived: Vec::new(),
			menu_paste: None,
			menu_at: None,
			busy: Vec::new(),
			jobs: 0,
			done: mpsc::channel(),
			offer: None,
			failed_at: [0.0; 2],
		}
	}
}

/// How far each file after the first lands from the one before, as a part
/// of the canvas.
const CASCADE: f32 = 0.04;

/// The frames a menu's Paste waits for the clipboard's text before taking
/// it to hold none. eframe 0.36 runs a frame's viewport commands after it
/// takes the next frame's input, so the text a menu asks for in one frame
/// arrives two frames on; a third spares a slow clipboard.
const MENU_PASTE_FRAMES: u32 = 3;

/// Where a chip sits from the pointer it labels, clear of the cursor.
const CHIP: egui::Vec2 = egui::vec2(14.0, 18.0);

impl Dropping {
	/// Take this frame's drag and drops from `ctx`, asking the windowing
	/// system where the pointer is while a drag holds it.
	pub(super) fn follow(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
		let (hovered, dropped) =
			ctx.input(|input| (!input.raw.hovered_files.is_empty(), input.raw.dropped_files.clone()));
		if hovered || !dropped.is_empty() {
			// egui's own pointer, where the windowing system cannot say, as in a
			// test, which hovers it.
			let at = self.pointer.at(frame, ctx).or_else(|| ctx.pointer_latest_pos());
			self.dragging = hovered.then_some(at).flatten();
			for file in dropped {
				self.arrived.push((Arrival::File(file.path().to_path_buf()), at));
			}
		} else {
			self.dragging = None;
		}
		if hovered {
			// Nothing tells the window the pointer moved while a drag holds it.
			ctx.request_repaint();
		} else {
			self.guesses.clear();
		}
	}

	/// Take `arrival` at `at` on screen.
	pub(super) fn arrive(&mut self, arrival: Arrival, at: Option<egui::Pos2>) {
		self.arrived.push((arrival, at));
	}

	/// Paste what the clipboard holds besides text at `at` on screen: the
	/// files of a file list, as Windows Explorer and macOS Finder copy them,
	/// or else a picture. Read here, with nothing locked, as egui reads the
	/// clipboard's text.
	pub(super) fn paste_clipboard(&mut self, at: Option<egui::Pos2>) {
		match clipboard_files() {
			Some(files) => self
				.arrived
				.extend(files.into_iter().map(|path| (Arrival::File(path), at))),
			None => self.arrived.push((Arrival::Clipboard, at)),
		}
	}

	/// Take this frame's arrivals, and whether they are several files, which
	/// no single layer's file can stand for.
	fn take_arrived(&mut self) -> (Vec<(Arrival, Option<egui::Pos2>)>, bool) {
		let arrived = std::mem::take(&mut self.arrived);
		let files = arrived
			.iter()
			.filter(|(arrival, _)| matches!(arrival, Arrival::File(_)))
			.count();
		(arrived, files > 1)
	}

	/// Paste at the edited scene's center, from a menu off the picture.
	pub(super) fn paste_from_window(&mut self) {
		self.menu_paste = Some((None, 0));
	}

	/// Note where the picture's menu opened, which its Paste pastes at.
	fn menu_opened(&mut self, at: Option<egui::Pos2>) {
		self.menu_at = at;
	}

	/// Paste at where the picture's menu opened.
	pub(super) fn paste_from_menu(&mut self) {
		self.menu_paste = Some((self.menu_at, 0));
	}

	/// Paste at `at` on screen once the text the window was asked for comes,
	/// as a menu's Paste does.
	pub(super) fn paste_asked(&mut self, at: Option<egui::Pos2>) {
		self.menu_paste = Some((at, 0));
	}

	/// Where a menu's Paste lands, on the frame it lands: with the clipboard's
	/// text, if `text` arrived this frame, or else, once none has arrived for
	/// `MENU_PASTE_FRAMES`, with what the clipboard holds besides text
	/// (`true`).
	pub(super) fn menu_pasted(&mut self, ctx: &egui::Context, text: bool) -> Option<(Option<egui::Pos2>, bool)> {
		let (at, frames) = self.menu_paste.as_mut()?;
		let at = *at;
		*frames += 1;
		if text {
			self.menu_paste = None;
			return Some((at, false));
		}
		if *frames < MENU_PASTE_FRAMES {
			ctx.request_repaint();
			return None;
		}
		self.menu_paste = None;
		Some((at, true))
	}

	/// Whether files are dragged over the window.
	fn hovering(&self, ctx: &egui::Context) -> bool {
		ctx.input(|input| !input.raw.hovered_files.is_empty())
	}

	/// What `path` would become, read once per drag.
	fn guess(&mut self, path: &Path) -> Guess {
		self.guesses
			.entry(path.to_path_buf())
			.or_insert_with(|| {
				let source = file::source(path).map_err(|err| reason(&err, path));
				let size = match &source {
					Ok(source @ Source::Image { .. }) => match file::picture(source) {
						Ok(Picture::Sized(size)) => Some([size.width as f32, size.height as f32]),
						_ => None,
					},
					_ => None,
				};
				Guess { source, size }
			})
			.clone()
	}
}

/// Why the file at `path` cannot be shown, without the kind of failure and
/// the path, since the file's name is said beside it.
fn reason(err: &moq_scene::Error, path: &Path) -> String {
	let text = err.to_string();
	let named = format!("{}: ", path.display());
	match text.find(&named) {
		Some(at) => text[at + named.len()..].to_string(),
		None => text,
	}
}

/// Where a picture `size` canvas pixels large lands centered on `at`: at its
/// own size, shrunk to half of `canvas` at most keeping its shape, and moved
/// wholly onto the canvas.
pub(super) fn land(canvas: Canvas, size: [f32; 2], at: [f32; 2]) -> Rect {
	let (cw, ch) = (canvas.width as f32, canvas.height as f32);
	let [mut width, mut height] = size.map(|side| side.max(1.0));
	let shrink = (cw / 2.0 / width).min(ch / 2.0 / height).min(1.0);
	width = (width * shrink).round().max(1.0);
	height = (height * shrink).round().max(1.0);
	let x = (at[0] - width / 2.0).clamp(0.0, (cw - width).max(0.0)).round();
	let y = (at[1] - height / 2.0).clamp(0.0, (ch - height).max(0.0)).round();
	Rect { x, y, width, height }
}

/// The path to name `path` by from the scene file's directory `dir`: its
/// place under `dir`, or else a copy put there, under its own name or the
/// first free one after it (`logo 2.png`). A file there already that holds
/// the same bytes is reused rather than copied again.
fn beside(dir: &Path, path: &Path) -> anyhow::Result<PathBuf> {
	let dir = dir
		.canonicalize()
		.with_context(|| format!("cannot read {}", dir.display()))?;
	let path = path
		.canonicalize()
		.with_context(|| format!("cannot read {}", path.display()))?;
	if let Ok(under) = path.strip_prefix(&dir) {
		return Ok(under.to_path_buf());
	}
	let name = path.file_name().context("the file has no name")?;
	save(
		&dir,
		Path::new(name),
		|copy| {
			let mut from = std::fs::File::open(&path)?;
			std::io::copy(&mut from, copy).map(drop)
		},
		|there| same(&path, there),
	)
}

/// Write a file named `name`, or the first free name after it, into `dir`
/// with `write`, unless a file there `matches`; its name.
fn save(
	dir: &Path,
	name: &Path,
	write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
	matches: impl Fn(&Path) -> bool,
) -> anyhow::Result<PathBuf> {
	let stem = name.file_stem().unwrap_or(name.as_os_str()).to_string_lossy();
	let extension = name.extension().map(|extension| extension.to_string_lossy());
	let named = |n: u32| {
		let stem = match n {
			1 => stem.to_string(),
			n => format!("{stem} {n}"),
		};
		match &extension {
			Some(extension) => PathBuf::from(format!("{stem}.{extension}")),
			None => PathBuf::from(stem),
		}
	};
	// A copy already there is reused before anything is written, so a large
	// file's second drop needs no room for a third copy.
	let mut checked = 0;
	while dir.join(named(checked + 1)).exists() {
		checked += 1;
		if matches(&dir.join(named(checked))) {
			return Ok(named(checked));
		}
	}
	// Written whole under a name of its own, so nothing reads it half done,
	// then moved to a name nothing holds.
	let mut temp =
		tempfile::NamedTempFile::new_in(dir).with_context(|| format!("cannot write in {}", dir.display()))?;
	write(temp.as_file_mut()).with_context(|| format!("cannot write in {}", dir.display()))?;
	for n in 1.. {
		let name = named(n);
		let there = dir.join(&name);
		if there.exists() {
			// Taken while it was written, by the same bytes or others.
			if n > checked && matches(&there) {
				return Ok(name);
			}
			continue;
		}
		match temp.persist_noclobber(&there) {
			Ok(_) => return Ok(name),
			// Taken since it was looked at.
			Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => temp = err.file,
			Err(err) => return Err(err.error).with_context(|| format!("cannot write {}", there.display())),
		}
	}
	unreachable!("a free name")
}

/// Whether the files at `a` and `b` hold the same bytes.
fn same(a: &Path, b: &Path) -> bool {
	use std::io::Read;
	let open = |path: &Path| std::fs::File::open(path).map(std::io::BufReader::new);
	let (Ok(mut a), Ok(mut b)) = (open(a), open(b)) else {
		return false;
	};
	let (mut left, mut right) = ([0; 8192], [0; 8192]);
	loop {
		let (Ok(read), Ok(other)) = (fill(&mut a, &mut left), fill(&mut b, &mut right)) else {
			return false;
		};
		if left[..read] != right[..other] {
			return false;
		}
		if read == 0 {
			return true;
		}
	}

	fn fill(file: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
		let mut read = 0;
		while read < buf.len() {
			match file.read(&mut buf[read..])? {
				0 => break,
				more => read += more,
			}
		}
		Ok(read)
	}
}

/// Read `source`'s file whole, as the source will, and put it beside the
/// scene file in `dir`, named for the file.
fn prepare(source: Source, dir: &Path) -> anyhow::Result<Prepared> {
	let mut source = source;
	let read = file::picture(&source);
	let (Source::Image { path } | Source::Media { path, .. } | Source::Lut { path }) = &mut source else {
		anyhow::bail!("{source:?} reads no file");
	};
	let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
	let picture = read.map_err(|err| anyhow::anyhow!("{file_name}: {}", reason(&err, path)))?;
	let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
	*path = beside(dir, path)?;
	Ok(Prepared { source, picture, name })
}

/// The files the clipboard holds as a file list, if any. Read where the
/// paste key was pressed, as egui reads the clipboard's text.
fn clipboard_files() -> Option<Vec<PathBuf>> {
	let files = arboard::Clipboard::new().ok()?.get().file_list().ok()?;
	(!files.is_empty()).then_some(files)
}

/// Save the picture the clipboard holds as a PNG beside the scene file in
/// `dir`.
fn pasted(dir: &Path) -> anyhow::Result<Prepared> {
	let mut clipboard = arboard::Clipboard::new().context("cannot read the clipboard")?;
	let image = clipboard
		.get_image()
		.map_err(|_| anyhow::anyhow!("the clipboard holds no layers, filters, files, picture, or web address"))?;
	let (width, height) = (u32::try_from(image.width)?, u32::try_from(image.height)?);
	let mut png = Vec::new();
	{
		let mut encoder = png::Encoder::new(&mut png, width, height);
		encoder.set_color(png::ColorType::Rgba);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header()?;
		writer.write_image_data(&image.bytes)?;
	}
	let path = save(
		dir,
		Path::new("Pasted picture.png"),
		|file| std::io::Write::write_all(file, &png),
		|_| false,
	)?;
	Ok(Prepared {
		source: Source::Image { path },
		picture: Picture::Sized(moq_video::Size::new(width, height)),
		name: "Pasted picture".into(),
	})
}

impl Panels {
	/// Place what arrived this frame on the picture at `view`, edited as
	/// `canvas`, where it arrived on it, and draw what a drag over the window
	/// would become, what is being read, the offer to replace a file, and
	/// the last failure at a drop.
	pub(super) fn drops_on_picture(&mut self, ui: &mut Ui, state: &mut State, rect: egui::Rect, canvas: Canvas) {
		if !rect.is_positive() {
			return;
		}
		let view = View::new(rect, canvas);
		let opened = ui.input(|input| {
			input.pointer.secondary_clicked() && input.pointer.interact_pos().is_some_and(|at| rect.contains(at))
		});
		if opened {
			self.dropping
				.menu_opened(ui.input(|input| input.pointer.interact_pos()));
		}
		let (arrived, several) = self.dropping.take_arrived();
		for (arrival, at) in arrived {
			let at = at.filter(|at| rect.contains(*at)).map(|at| view.point(at));
			self.land(ui.ctx(), state, arrival, at, several);
		}
		let painter = ui.painter_at(rect);
		if self.dropping.hovering(ui.ctx()) {
			self.preview(ui, state, &view, &painter);
		}
		for (_, name, at) in &self.dropping.busy {
			let pos = view.pos(at[0], at[1]);
			chip(
				ui,
				&painter,
				pos + CHIP,
				&format!("Reading {name}…"),
				Icon::Reload,
				kit::ACCENT,
			);
		}
		if let Some(fault) = self.fault(&Spot::Dropped) {
			let [x, y] = self.dropping.failed_at;
			chip(ui, &painter, view.pos(x, y) + CHIP, fault, Icon::Close, kit::ERR);
		}
		self.offer(ui, state, &view);
	}

	/// Refuse the files dropped this frame onto the locked picture at
	/// `rect`, which a drop elsewhere would otherwise place on the scene,
	/// keeping a paste and a drop off the picture.
	pub(super) fn drops_on_locked(&mut self, ui: &Ui, rect: egui::Rect) {
		let dropped: Vec<std::path::PathBuf> = ui.input(|input| {
			input
				.raw
				.dropped_files
				.iter()
				.map(|file| file.path().to_path_buf())
				.collect()
		});
		let before = self.dropping.arrived.len();
		self.dropping.arrived.retain(|(arrival, at)| {
			let on = at.is_some_and(|at| rect.contains(at));
			!(on && matches!(arrival, Arrival::File(path) if dropped.contains(path)))
		});
		if self.dropping.arrived.len() < before {
			self.say(Err("the picture is locked: unlock it to drop onto it".into()));
		}
	}

	/// Place what arrived on no picture at the edited scene's center.
	pub(super) fn drops_elsewhere(&mut self, ctx: &egui::Context) {
		if self.dropping.arrived.is_empty() {
			return;
		}
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		let (arrived, several) = self.dropping.take_arrived();
		for (arrival, _) in arrived {
			self.land(ctx, &mut state, arrival, None, several);
		}
	}

	/// Where on the picture a drop at `at`, in canvas pixels, lands: there,
	/// or the edited scene's center for `None`, over the layer it is on.
	fn landing(&self, state: &State, at: Option<[f32; 2]>) -> Result<Landing, String> {
		let layer = self.scene(state)?;
		let collection = state.collection();
		let format = collection.format_of(&layer, self.format);
		let canvas = match collection.dual(&layer) {
			true => collection.canvas_of(format),
			false => collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas),
		};
		let center = [canvas.width as f32 / 2.0, canvas.height as f32 / 2.0];
		let onto = at.and_then(|at| {
			let shown = self.shown(state);
			let items: Vec<Item> = shown.iter().map(|(_, item)| item.clone()).collect();
			let (index, _) = snap::hit(&items, None, at, 0.0)?;
			Some(shown[index].clone())
		});
		Ok(Landing {
			layer,
			format,
			canvas,
			at: at.unwrap_or(center),
			onto,
		})
	}

	/// Take `arrival` at `at` on the picture, in canvas pixels, or at the
	/// edited scene's center; one of `several` files is added, never offered
	/// as a layer's new file.
	fn land(&mut self, ctx: &egui::Context, state: &mut State, arrival: Arrival, at: Option<[f32; 2]>, several: bool) {
		let landing = match self.landing(state, at) {
			Ok(landing) => landing,
			Err(why) => {
				self.dropped_failed(at.unwrap_or_default(), Err(anyhow::anyhow!(why)));
				return;
			}
		};
		// Each file after the first lands a little further along.
		let cascade = self.dropping.busy.len() as f32 * CASCADE;
		let mut landing = landing;
		landing.at = [
			landing.at[0] + cascade * landing.canvas.width as f32,
			landing.at[1] + cascade * landing.canvas.height as f32,
		];
		let dir = state.sources.context().dir.clone();
		match arrival {
			Arrival::Page(url) => {
				let result = self.add_page(state, &landing, url);
				self.dropped_failed(landing.at, result);
			}
			Arrival::Clipboard => {
				let job = Job {
					name: "the pasted picture".into(),
					at: landing.at,
					intent: Intent::Add(landing),
				};
				self.read(ctx, job, move || pasted(&dir));
			}
			Arrival::File(path) => {
				let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
				let source = match file::source(&path) {
					Ok(source) => source,
					Err(err) => {
						let err = anyhow::anyhow!("{name}: {}", reason(&err, &path));
						self.dropped_failed(landing.at, Err(err));
						return;
					}
				};
				let onto = landing.onto.clone();
				let landing_at = landing.at;
				let intent = match (&source, onto) {
					// As the scene file has it, not as posed for the picture.
					(Source::Lut { .. }, Some((picked, _))) => match picked.item(state.collection()) {
						Some(stored) => Intent::Grade(picked.clone(), stored.clone()),
						None => {
							let err = anyhow::anyhow!("the layer dropped on is gone");
							self.dropped_failed(landing.at, Err(err));
							return;
						}
					},
					(Source::Lut { .. }, None) => {
						let err = anyhow::anyhow!("drop a color grade on the layer it grades");
						self.dropped_failed(landing.at, Err(err));
						return;
					}
					(_, Some((_, item))) if !several && same_kind(state.collection(), &item.source, &source) => {
						let collection = state.collection();
						let shown = collection.capture(&item.source).to_string();
						let was = collection.sources.get(&shown).cloned().expect("a source of its kind");
						self.dropping.offer = Some(Offer {
							source,
							landing,
							shown,
							was,
						});
						return;
					}
					_ => Intent::Add(landing),
				};
				let at = landing_at;
				self.read(ctx, Job { name, at, intent }, move || prepare(source, &dir));
			}
		}
	}

	/// Read `job`'s file in the background with `read`, then do what it is
	/// for.
	fn read(
		&mut self,
		ctx: &egui::Context,
		job: Job,
		read: impl FnOnce() -> anyhow::Result<Prepared> + Send + 'static,
	) {
		let Job { name, at, intent } = job;
		self.dropping.jobs += 1;
		let job = self.dropping.jobs;
		let send = self.dropping.done.0.clone();
		let ctx = ctx.clone();
		let spawned = std::thread::Builder::new().name("drop".into()).spawn(move || {
			let result = read();
			let _ = send.send(Done { job, intent, result });
			ctx.request_repaint();
		});
		match spawned {
			Ok(_) => self.dropping.busy.push((job, name, at)),
			Err(err) => self.dropped_failed(at, Err(err).context("cannot read the file")),
		}
	}

	/// Add or replace what each file read in the background became.
	pub(super) fn drops_read(&mut self) {
		let done: Vec<Done> = self.dropping.done.1.try_iter().collect();
		if done.is_empty() {
			return;
		}
		let engine = self.engine.clone();
		let mut state = engine.state.lock().unwrap();
		self.follow_reloads(&state);
		for Done { job, intent, result } in done {
			let at = self
				.dropping
				.busy
				.iter()
				.find(|(busy, ..)| *busy == job)
				.map(|(.., at)| *at);
			self.dropping.busy.retain(|(busy, ..)| *busy != job);
			let at = at.unwrap_or_default();
			let result = result.and_then(|prepared| match intent {
				Intent::Add(landing) => self.add_prepared(&mut state, &landing, prepared),
				Intent::Replace(name, was) => replace(&mut state, &name, &was, prepared.source),
				Intent::Grade(picked, item) => grade(&mut state, &picked, &item, prepared),
			});
			self.dropped_failed(at, result);
		}
	}

	/// Show a failure at a drop at `at` in canvas pixels, on the picture if
	/// it shows, or else as the window's message.
	fn dropped_failed(&mut self, at: [f32; 2], result: anyhow::Result<()>) {
		if self.page != Page::Studio {
			self.check(result);
			return;
		}
		if result.is_err() {
			self.dropping.failed_at = at;
		}
		self.fail(Spot::Dropped, result);
	}

	/// Add `prepared` as a layer at `landing`, sent to the audio scene on
	/// program if it has sound, and select it; a sound alone is only heard.
	fn add_prepared(&mut self, state: &mut State, landing: &Landing, prepared: Prepared) -> anyhow::Result<()> {
		let Prepared { source, picture, name } = prepared;
		let name = state.collection().unused_name(&name);
		let canvas = landing.canvas;
		let rect = match picture {
			Picture::None => None,
			Picture::Sized(size) => Some(land(canvas, [size.width as f32, size.height as f32], landing.at)),
			Picture::Unsized => Some(land(canvas, [canvas.width as f32, canvas.height as f32], landing.at)),
		};
		self.add_layer(state, landing, name, source, rect)
	}

	/// Add a web page showing `url` at `landing`, as large as the canvas it
	/// lands on.
	fn add_page(&mut self, state: &mut State, landing: &Landing, url: url::Url) -> anyhow::Result<()> {
		let canvas = landing.canvas;
		let name = state.collection().unused_name(url.host_str().unwrap_or("Web page"));
		let source = Source::Browser {
			url: url.to_string(),
			width: canvas.width,
			height: canvas.height,
		};
		let rect = land(canvas, [canvas.width as f32, canvas.height as f32], landing.at);
		self.add_layer(state, landing, name, source, Some(rect))
	}

	/// Add `source` as `name`, in front of `landing`'s layers at `rect` if
	/// given, heard on program if it has sound, and select its layer.
	fn add_layer(
		&mut self,
		state: &mut State,
		landing: &Landing,
		name: String,
		source: Source,
		rect: Option<Rect>,
	) -> anyhow::Result<()> {
		let layer = &landing.layer;
		let heard = source.has_audio();
		let index = state.collection().items(layer).map_or(0, <[Item]>::len);
		// Landed in the portrait picture, it sits there; in the landscape
		// one, it lands centered at the same size until moved there. One that
		// landed in portrait before its layers stopped being drawn in both,
		// such as while its file was read, lands centered in their one.
		let collection = state.collection();
		let centered = |canvas: Canvas, rect: Rect| {
			let center = [canvas.width as f32 / 2.0, canvas.height as f32 / 2.0];
			land(canvas, [rect.width, rect.height], center)
		};
		let item = rect.map(|rect| match (landing.format, collection.dual(layer)) {
			(Format::Landscape, _) => Item::new(name.clone(), rect),
			(Format::Portrait, true) => Item {
				portrait: Some(Placement {
					rect,
					crop: Crop::default(),
				}),
				..Item::new(name.clone(), centered(collection.canvas, rect))
			},
			(Format::Portrait, false) => {
				let canvas = collection.canvas(layer.scene.as_deref()).unwrap_or(collection.canvas);
				Item::new(name.clone(), centered(canvas, rect))
			}
		});
		edit(state, format!("Drop `{name}` {}", place(layer)), None, |draft| {
			draft.set_source(&name, Some(source))?;
			if heard {
				draft.hear(&name)?;
			}
			if let Some(item) = item {
				draft.insert_item(layer, index, item)?;
			}
			Ok(())
		})?;
		match rect {
			Some(_) => {
				self.select(Some(Picked {
					layer: layer.clone(),
					index,
				}));
				self.side = Side::Layers;
			}
			None => self.say(Ok(format!("`{name}` has no picture, so it is heard on program alone"))),
		}
		Ok(())
	}

	/// What a drag over the picture would become, drawn where it would land.
	fn preview(&mut self, ui: &mut Ui, state: &State, view: &View, painter: &egui::Painter) {
		let paths: Vec<PathBuf> = ui.input(|input| {
			input
				.raw
				.hovered_files
				.iter()
				.filter_map(|file| file.path.clone())
				.collect()
		});
		let pointer = self.dropping.dragging;
		let at = pointer.filter(|at| view.rect.contains(*at)).map(|at| view.point(at));
		let Ok(landing) = self.landing(state, at) else { return };
		let accent = kit::ACCENT;
		let stroke = egui::Stroke::new(2.0, accent);
		let chip_at = pointer.unwrap_or_else(|| view.pos(landing.at[0], landing.at[1]));
		let Some(first) = paths.first() else { return };
		let guess = self.dropping.guess(first);
		let mut name = first.file_name().unwrap_or_default().to_string_lossy().into_owned();
		if paths.len() > 1 {
			name = format!("{name} and {} more", paths.len() - 1);
		}
		let collection = state.collection();
		let (rect, tag, icon) = match &guess.source {
			Err(why) => {
				chip(
					ui,
					painter,
					chip_at + CHIP,
					&format!("{name}: {why}"),
					Icon::Close,
					kit::ERR,
				);
				return;
			}
			Ok(Source::Lut { .. }) => match &landing.onto {
				Some((_, item)) => (item.rect, format!("Color grade for {}", item.source), Icon::Color),
				None => {
					let why = "Drop a color grade on the layer it grades";
					chip(ui, painter, chip_at + CHIP, why, Icon::Color, kit::ERR);
					return;
				}
			},
			Ok(source) => {
				let icon = match source {
					Source::Image { .. } => Icon::Image,
					_ => Icon::Media,
				};
				let kind = match source {
					Source::Image { .. } => "Image layer",
					_ => "Media layer",
				};
				match &landing.onto {
					Some((_, item)) if paths.len() == 1 && same_kind(collection, &item.source, source) => {
						(item.rect, format!("{kind}, or replace {}'s file", item.source), icon)
					}
					_ => {
						let canvas = landing.canvas;
						let size = guess.size.unwrap_or([canvas.width as f32, canvas.height as f32]);
						(land(canvas, size, landing.at), kind.to_string(), icon)
					}
				}
			}
		};
		let rect = view.screen(&rect);
		painter.rect_filled(rect, 4.0, accent.gamma_multiply(0.14));
		super::dashed(painter, rect, stroke);
		let galley = kit::caps_galley(painter, &tag, kit::display_bold(11.0), Color32::WHITE, None);
		let size = galley.size() + egui::vec2(28.0, 8.0);
		let tag_rect = egui::Rect::from_min_size(rect.left_top() - egui::vec2(0.0, size.y + 4.0), size);
		painter.rect_filled(tag_rect, 4.0, accent);
		icon.paint(
			ui,
			egui::Rect::from_min_size(tag_rect.min + egui::vec2(4.0, 2.0), egui::Vec2::splat(size.y - 4.0)),
			12.0,
			Color32::WHITE,
		);
		painter.galley(tag_rect.min + egui::vec2(22.0, 4.0), galley, Color32::WHITE);
		chip(ui, painter, chip_at + CHIP, &name, icon, accent);
	}

	/// Ask whether a file dropped on a layer showing a file of its kind
	/// replaces that file or adds a layer, beside where it was dropped.
	fn offer(&mut self, ui: &mut Ui, state: &mut State, view: &View) {
		let Some(offer) = &self.dropping.offer else { return };
		let at = view.pos(offer.landing.at[0], offer.landing.at[1]);
		if let Some((_, item)) = &offer.landing.onto {
			let painter = ui.painter_at(view.rect);
			super::dashed(&painter, view.screen(&item.rect), egui::Stroke::new(2.0, kit::ACCENT));
		}
		let name = offer.source_name();
		// The layers a replaced file changes, counted as the offer shows, so
		// one added meanwhile is named too.
		let collection = state.collection();
		let linked = collection
			.layers()
			.flat_map(|(_, items)| items)
			.filter(|item| collection.capture(&item.source) == offer.shown)
			.count();
		let mut chosen = None;
		let area = egui::Area::new(egui::Id::new("drop offer"))
			.order(egui::Order::Foreground)
			.fixed_pos(at + CHIP)
			.show(ui.ctx(), |ui| {
				kit::popover_frame().show(ui, |ui| {
					ui.label(egui::RichText::new(&name).font(kit::ui_medium(13.0)).color(kit::TEXT));
					ui.add_space(4.0);
					let label = match linked {
						0 | 1 => format!("Replace {}'s file", offer.shown),
						n => format!("Replace {}'s file, on all {n} layers showing it", offer.shown),
					};
					if ui.add(kit::button(Icon::Image, &label)).clicked() {
						chosen = Some(true);
					}
					if ui.add(kit::button(Icon::Add, "Add a layer")).clicked() {
						chosen = Some(false);
					}
				});
			});
		let away = ui.input(|input| {
			input.key_pressed(egui::Key::Escape)
				|| (input.pointer.any_pressed()
					&& input
						.pointer
						.interact_pos()
						.is_some_and(|pos| !area.response.rect.contains(pos)))
		});
		let Some(replace) = chosen else {
			if away {
				ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
				self.dropping.offer = None;
			}
			return;
		};
		let Offer {
			source,
			landing,
			shown,
			was,
			..
		} = self.dropping.offer.take().expect("an offer");
		let dir = state.sources.context().dir.clone();
		let at = landing.at;
		let intent = match replace {
			true => Intent::Replace(shown, was),
			false => Intent::Add(landing),
		};
		self.read(ui.ctx(), Job { name, at, intent }, move || prepare(source, &dir));
	}
}

impl Offer {
	/// The dropped file's name.
	fn source_name(&self) -> String {
		match &self.source {
			Source::Image { path } | Source::Media { path, .. } | Source::Lut { path } => {
				path.file_name().unwrap_or_default().to_string_lossy().into_owned()
			}
			_ => String::new(),
		}
	}
}

/// Whether the source a layer shows as `shown` reads a file of the same kind
/// as `dropped`, which a drop on it can replace.
fn same_kind(collection: &Collection, shown: &str, dropped: &Source) -> bool {
	let shown = collection.sources.get(collection.capture(shown));
	matches!(
		(shown, dropped),
		(Some(Source::Image { .. }), Source::Image { .. }) | (Some(Source::Media { .. }), Source::Media { .. })
	)
}

/// Point the source called `name`, if it is still `was`, at `source`'s
/// file, keeping how a media file plays.
fn replace(state: &mut State, name: &str, was: &Source, source: Source) -> anyhow::Result<()> {
	let old = state
		.collection()
		.sources
		.get(name)
		.filter(|old| *old == was)
		.context("the layer's source changed while the file was read")?;
	let source = match (old, source) {
		(&Source::Media { looping, restart, .. }, Source::Media { path, .. }) => {
			Source::Media { path, looping, restart }
		}
		(Source::Image { .. }, source @ Source::Image { .. }) => source,
		_ => anyhow::bail!("`{name}` no longer shows a file of that kind"),
	};
	edit(state, format!("Replace `{name}`'s file"), None, |draft| {
		draft.set_source(name, Some(source))
	})
}

/// Grade the layer `picked`, if it is still `item`, with the LUT `prepared`,
/// reusing a LUT source of the same file, in place of any grade it has.
fn grade(state: &mut State, picked: &Picked, item: &Item, prepared: Prepared) -> anyhow::Result<()> {
	let collection = state.collection();
	let shown = &item.source;
	let item = picked
		.item(collection)
		.filter(|now| *now == item)
		.cloned()
		.context("the layer dropped on changed while the file was read")?;
	let existing = collection
		.sources
		.iter()
		.find(|(_, source)| **source == prepared.source);
	let (lut, added) = match existing {
		Some((name, _)) => (name.clone(), None),
		None => {
			let name = collection.unused_name(&prepared.name);
			(name, Some(prepared.source))
		}
	};
	let mut filters = item.filters.clone();
	let grade = Filter::Lut {
		source: lut.clone(),
		amount: 1.0,
	};
	match filters.iter().position(|filter| matches!(filter, Filter::Lut { .. })) {
		Some(at) => filters[at] = grade,
		None => filters.push(grade),
	}
	edit(state, format!("Grade `{shown}` with `{lut}`"), None, |draft| {
		if let Some(source) = added {
			draft.set_source(&lut, Some(source))?;
		}
		draft.set_item(&picked.layer, picked.index, Item { filters, ..item })
	})
}

/// A rounded label at `at` with `icon`, outlined in `color`, such as the
/// file a drag holds or why it cannot land.
fn chip(ui: &Ui, painter: &egui::Painter, at: egui::Pos2, text: &str, icon: Icon, color: Color32) {
	let galley = painter.layout(text.to_string(), kit::ui_medium(12.0), kit::TEXT, 320.0);
	let size = galley.size() + egui::vec2(36.0, 12.0);
	// Kept on the picture.
	let clip = painter.clip_rect();
	let min = egui::pos2(
		at.x.min(clip.right() - size.x).max(clip.left()),
		at.y.min(clip.bottom() - size.y).max(clip.top()),
	);
	let rect = egui::Rect::from_min_size(min, size);
	painter.rect(
		rect,
		8.0,
		kit::POPOVER,
		egui::Stroke::new(1.0, color),
		egui::StrokeKind::Inside,
	);
	let icon_rect = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 6.0), egui::Vec2::splat(18.0));
	icon.paint(ui, icon_rect, 14.0, color);
	painter.galley(rect.min + egui::vec2(28.0, 6.0), galley, kit::TEXT);
}

#[cfg(test)]
mod tests {
	use super::*;

	const CANVAS: Canvas = Canvas {
		width: 1920,
		height: 1080,
		framerate: 30,
	};

	/// A paste asked of the window lands once: with its text when that comes
	/// in time, or else once with what the clipboard holds besides text.
	#[test]
	fn an_asked_paste_lands_once() {
		let ctx = egui::Context::default();
		let at = Some(egui::pos2(10.0, 20.0));
		let mut dropping = Dropping::default();
		dropping.paste_asked(at);
		assert_eq!(dropping.menu_pasted(&ctx, false), None);
		assert_eq!(dropping.menu_pasted(&ctx, true), Some((at, false)), "the text came");
		assert_eq!(dropping.menu_pasted(&ctx, false), None, "and nothing else");

		dropping.paste_asked(at);
		let landed: Vec<_> = (0..MENU_PASTE_FRAMES + 2)
			.filter_map(|_| dropping.menu_pasted(&ctx, false))
			.collect();
		assert_eq!(landed, [(at, true)], "no text came");
	}

	/// A picture lands at its own size where it is dropped, no larger than
	/// half the canvas, and never off it.
	#[test]
	fn lands_where_dropped() {
		let small = land(CANVAS, [200.0, 100.0], [500.0, 500.0]);
		assert_eq!(
			small,
			Rect {
				x: 400.0,
				y: 450.0,
				width: 200.0,
				height: 100.0
			}
		);
		let large = land(CANVAS, [3840.0, 2160.0], [960.0, 540.0]);
		assert_eq!(
			large,
			Rect {
				x: 480.0,
				y: 270.0,
				width: 960.0,
				height: 540.0
			}
		);
		let tall = land(CANVAS, [1000.0, 4000.0], [0.0, 0.0]);
		assert_eq!((tall.x, tall.y, tall.height), (0.0, 0.0, 540.0), "{tall:?}");
		assert!((tall.width - 135.0).abs() < 1.0, "keeps its shape: {tall:?}");
		let corner = land(CANVAS, [200.0, 100.0], [1920.0, 1080.0]);
		assert_eq!((corner.x, corner.y), (1720.0, 980.0));
	}

	/// A file under the scene file's directory is named from there; any other
	/// is copied in, beside one of the same name that differs, and a copy
	/// already there is reused.
	#[test]
	fn copies_beside_the_scene_file() {
		let show = tempfile::tempdir().unwrap();
		let outside = tempfile::tempdir().unwrap();
		std::fs::create_dir(show.path().join("art")).unwrap();
		std::fs::write(show.path().join("art/logo.png"), b"inside").unwrap();
		assert_eq!(
			beside(show.path(), &show.path().join("art/logo.png")).unwrap(),
			Path::new("art/logo.png")
		);

		std::fs::write(show.path().join("logo.png"), b"another").unwrap();
		std::fs::write(outside.path().join("logo.png"), b"dropped").unwrap();
		let copied = beside(show.path(), &outside.path().join("logo.png")).unwrap();
		assert_eq!(copied, Path::new("logo 2.png"));
		assert_eq!(std::fs::read(show.path().join(&copied)).unwrap(), b"dropped");
		assert_eq!(
			std::fs::read(show.path().join("logo.png")).unwrap(),
			b"another",
			"left alone"
		);

		let again = beside(show.path(), &outside.path().join("logo.png")).unwrap();
		assert_eq!(again, copied, "the same bytes are not copied twice");
		let left: Vec<_> = std::fs::read_dir(show.path())
			.unwrap()
			.map(|entry| entry.unwrap().file_name())
			.collect();
		assert_eq!(left.len(), 3, "no temporary file is left: {left:?}");
	}

	/// Pasted text brings files, as a file manager copies them, or a web
	/// address, and nothing for anything else, such as copied layers.
	#[test]
	fn pasted_text() {
		let dir = tempfile::tempdir().unwrap();
		let logo = dir.path().join("my logo.png");
		std::fs::write(&logo, b"png").unwrap();
		let uri = url::Url::from_file_path(&logo).unwrap();
		assert_eq!(
			Arrival::of_text(&format!("{uri}\r\n")),
			Some(vec![Arrival::File(logo.clone())])
		);
		let both = format!("{}\n{uri}", logo.display());
		assert_eq!(Arrival::of_text(&both), Some(vec![Arrival::File(logo.clone()); 2]));
		assert_eq!(
			Arrival::of_text(&dir.path().join("missing.png").display().to_string()),
			None
		);

		let page = url::Url::parse("https://example.com/overlay?id=1").unwrap();
		assert_eq!(
			Arrival::of_text(" https://example.com/overlay?id=1 "),
			Some(vec![Arrival::Page(page)])
		);
		assert_eq!(Arrival::of_text("ftp://example.com/file"), None);
		assert_eq!(Arrival::of_text("see https://example.com"), None);
		assert_eq!(Arrival::of_text("[[items]]\nsource = \"host\""), None);
	}
}
