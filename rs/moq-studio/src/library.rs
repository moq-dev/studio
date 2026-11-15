//! The library: the shows this user has opened, newest first, each with the
//! last picture of its program, kept per user in the app's storage rather than
//! beside any show. The window opens on it when no show is named.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use egui::{Align, Color32, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};

use crate::engine::{Engine, Gpu};
use crate::kit::{self, Icon};
use crate::setup::Task;

/// Where the list is kept in the app's storage.
const KEY: &str = "recent";

/// How many shows the list keeps.
const KEPT: usize = 30;

/// How wide a saved picture is at most, enough for a card on a large screen.
const PICTURE: u32 = 640;

/// The shows this user has opened, the latest first.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Recent {
	pub shows: Vec<Show>,
}

/// A show in the list.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Show {
	/// Its scene file.
	pub path: PathBuf,
	/// When it was last opened, in seconds since the Unix epoch.
	pub opened: u64,
	/// Its last time on air, if it ever went live.
	#[serde(default)]
	pub live: Option<Aired>,
	/// Whether its canvas is taller than wide, as of its last picture.
	#[serde(default)]
	pub portrait: bool,
}

/// A time on air.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Aired {
	/// When it went live, in seconds since the Unix epoch.
	pub at: u64,
	/// How long it stayed live, in seconds.
	pub lasted: u64,
}

/// Now, in seconds since the Unix epoch, on the ambient clock.
pub fn now() -> u64 {
	kio::clock::wall()
		.saturating_duration_since(kio::clock::Wall::UNIX_EPOCH)
		.as_secs()
}

impl Recent {
	/// The list as the app's storage holds it, or none yet.
	pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
		storage
			.and_then(|storage| eframe::get_value(storage, KEY))
			.unwrap_or_default()
	}

	pub fn save(&self, storage: &mut dyn eframe::Storage) {
		eframe::set_value(storage, KEY, self);
	}

	/// The show at `path` was opened at `at`: it moves to the front.
	pub fn opened(&mut self, path: &Path, at: u64) {
		let path = absolute(path);
		let index = self.shows.iter().position(|show| show.path == path);
		let mut show = match index {
			Some(index) => self.shows.remove(index),
			None => Show {
				path,
				opened: at,
				live: None,
				portrait: false,
			},
		};
		show.opened = at;
		self.shows.insert(0, show);
		self.shows.truncate(KEPT);
	}

	/// The show at `path` was on air as `aired`.
	pub fn aired(&mut self, path: &Path, aired: Aired) {
		if let Some(show) = self.find(path) {
			show.live = Some(aired);
		}
	}

	/// The show at `path` was last drawn `portrait` or not.
	pub fn shaped(&mut self, path: &Path, portrait: bool) {
		if let Some(show) = self.find(path) {
			show.portrait = portrait;
		}
	}

	/// Take the show at `path` off the list, and its picture in `pictures`
	/// with it.
	pub fn forget(&mut self, path: &Path, pictures: Option<&Path>) {
		self.shows.retain(|show| show.path != path);
		if let Some(pictures) = pictures {
			let _ = std::fs::remove_file(picture_path(pictures, path));
		}
	}

	fn find(&mut self, path: &Path) -> Option<&mut Show> {
		let path = absolute(path);
		self.shows.iter_mut().find(|show| show.path == path)
	}
}

/// `path` from the root, so the list names a show once however it was
/// opened.
fn absolute(path: &Path) -> PathBuf {
	std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Where the shows' pictures are kept: the app's storage.
pub fn pictures() -> Option<PathBuf> {
	eframe::storage_dir("moq-studio").map(|dir| dir.join("shows"))
}

/// Where the picture of the show at `path` is kept in `pictures`, by a hash of
/// the show's path, so moving the show starts it without one.
pub fn picture_path(pictures: &Path, path: &Path) -> PathBuf {
	let path = absolute(path);
	// FNV-1a, which unlike std's hasher stays the same across releases.
	let hash = path
		.as_os_str()
		.as_encoded_bytes()
		.iter()
		.fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
			(hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
		});
	pictures.join(format!("{hash:016x}.png"))
}

/// A picture in memory, as RGBA rows.
pub struct Picture {
	pub width: u32,
	pub height: u32,
	pub rgba: Vec<u8>,
}

impl Picture {
	/// Whether it is taller than wide.
	pub fn portrait(&self) -> bool {
		self.height > self.width
	}

	/// This picture, shrunk to `width` at most, by averaging each block of
	/// pixels that becomes one.
	fn shrink(self, width: u32) -> Self {
		let step = self.width.div_ceil(width.max(1)).max(1);
		if step == 1 {
			return self;
		}
		let (out_width, out_height) = (self.width / step, self.height / step);
		let mut rgba = Vec::with_capacity((out_width * out_height * 4) as usize);
		for y in 0..out_height {
			for x in 0..out_width {
				let mut sum = [0u32; 4];
				for dy in 0..step {
					for dx in 0..step {
						let at = (((y * step + dy) * self.width + x * step + dx) * 4) as usize;
						for (channel, total) in sum.iter_mut().enumerate() {
							*total += u32::from(self.rgba[at + channel]);
						}
					}
				}
				rgba.extend(sum.map(|total| (total / (step * step)) as u8));
			}
		}
		Self {
			width: out_width,
			height: out_height,
			rgba,
		}
	}
}

/// Read the program's last frame back from the GPU, shrunk for a card.
pub fn capture(gpu: &Gpu, engine: &Engine) -> anyhow::Result<Picture> {
	let (buffer, width, height, row) = {
		let state = engine.state.lock().unwrap();
		let texture = state.compositor.program();
		let (width, height) = (texture.width(), texture.height());
		let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
		let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("show picture"),
			size: u64::from(row) * u64::from(height),
			usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
			mapped_at_creation: false,
		});
		let mut encoder = gpu.device.create_command_encoder(&Default::default());
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
		gpu.queue.submit([encoder.finish()]);
		(buffer, width, height, row)
	};
	let (send, recv) = std::sync::mpsc::channel();
	buffer.map_async(wgpu::MapMode::Read, .., move |result| {
		let _ = send.send(result);
	});
	gpu.device
		.poll(wgpu::PollType::wait_indefinitely())
		.context("cannot wait for the GPU")?;
	recv.recv()
		.context("the GPU dropped the read")?
		.context("cannot read the program back")?;
	let mapped = buffer
		.slice(..)
		.get_mapped_range()
		.map_err(|err| anyhow::anyhow!("cannot read the program back: {err:?}"))?;
	let mut rgba = Vec::with_capacity((width * height * 4) as usize);
	for line in mapped.chunks(row as usize).take(height as usize) {
		rgba.extend_from_slice(&line[..(width * 4) as usize]);
	}
	drop(mapped);
	buffer.unmap();
	Ok(Picture { width, height, rgba }.shrink(PICTURE))
}

/// Keep `picture` as the show at `path`'s, in `pictures`.
pub fn save_picture(pictures: &Path, path: &Path, picture: &Picture) -> anyhow::Result<()> {
	let file = picture_path(pictures, path);
	let dir = pictures;
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let mut out = tempfile::NamedTempFile::new_in(dir).with_context(|| format!("writing {}", file.display()))?;
	{
		let mut encoder = png::Encoder::new(std::io::BufWriter::new(&mut out), picture.width, picture.height);
		encoder.set_color(png::ColorType::Rgba);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header().context("cannot encode the picture")?;
		writer
			.write_image_data(&picture.rgba)
			.context("cannot encode the picture")?;
	}
	out.persist(&file)
		.with_context(|| format!("writing {}", file.display()))?;
	Ok(())
}

/// The picture kept for the show at `path` in `pictures`, if there is one.
fn load_picture(pictures: &Path, path: &Path) -> Option<egui::ColorImage> {
	let file = std::fs::File::open(picture_path(pictures, path)).ok()?;
	let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
	decoder.set_transformations(png::Transformations::normalize_to_color8());
	let mut reader = decoder.read_info().ok()?;
	let mut buffer = vec![0; reader.output_buffer_size()?];
	let info = reader.next_frame(&mut buffer).ok()?;
	let size = [info.width as usize, info.height as usize];
	let pixels = &buffer[..info.buffer_size()];
	match info.color_type {
		png::ColorType::Rgba => Some(egui::ColorImage::from_rgba_unmultiplied(size, pixels)),
		png::ColorType::Rgb => Some(egui::ColorImage::from_rgb(size, pixels)),
		_ => None,
	}
}

/// What the library asks the window for.
pub enum Pick {
	/// Open the show at this path, or set it up if there is no file there.
	Open(PathBuf),
}

/// The library page.
pub struct Library {
	/// Where the shows' pictures are kept, if anywhere.
	dir: Option<PathBuf>,
	/// Each show's picture, loaded the first time it is drawn; `None` for one
	/// without.
	pictures: HashMap<PathBuf, Option<egui::TextureHandle>>,
	/// Whether each show's file is still there, checked once.
	found: HashMap<PathBuf, bool>,
	/// The system's file dialog, while open.
	dialog: Option<Task<Option<PathBuf>>>,
	/// Why the last show failed to open.
	pub error: Option<String>,
	/// The show being opened, by name, while it opens.
	pub opening: Option<String>,
}

impl Library {
	/// The library, with the shows' pictures from `dir`.
	pub fn new(dir: Option<PathBuf>) -> Self {
		Self {
			dir,
			pictures: HashMap::new(),
			found: HashMap::new(),
			dialog: None,
			error: None,
			opening: None,
		}
	}
}

/// Room between cards.
const GAP: f32 = 22.0;
/// The height of a card's name and line under its picture.
const LABEL: f32 = 70.0;

impl Library {
	/// Draw the page; a show to open once one is picked.
	pub fn ui(&mut self, ui: &mut Ui, recent: &mut Recent, runtime: &tokio::runtime::Handle) -> Option<Pick> {
		let mut pick = None;
		if let Some(dialog) = &mut self.dialog {
			ui.ctx().request_repaint_after(Duration::from_millis(100));
			if let Some(picked) = dialog.poll() {
				pick = picked.clone().map(Pick::Open);
				self.dialog = None;
			}
		}
		let window = ui.max_rect();
		kit::wash(ui.painter(), window);
		let width = (window.width() - 2.0 * 48.0).min(1220.0);
		let column = Rect::from_center_size(pos2(window.center().x, window.center().y), vec2(width, window.height()))
			.with_min_y(window.top() + 56.0);
		let builder = UiBuilder::new()
			.max_rect(column)
			.layout(egui::Layout::top_down(Align::Min));
		ui.scope_builder(builder, |ui| {
			egui::ScrollArea::vertical().show(ui, |ui| {
				ui.set_width(width);
				if self.heading(ui, runtime) {
					self.dialog(runtime, false);
				}
				ui.add_space(20.0);
				if let Some(error) = &self.error {
					ui.colored_label(kit::ERR, error);
					ui.add_space(12.0);
				}
				if let Some(name) = &self.opening {
					ui.horizontal(|ui| {
						ui.spinner();
						ui.label(format!("Opening {name}…"));
					});
					ui.add_space(12.0);
				}
				let busy = self.opening.is_some() || self.dialog.is_some();
				ui.add_enabled_ui(!busy, |ui| {
					if let Some(picked) = self.cards(ui, recent, width, runtime) {
						pick = Some(picked);
					}
				});
				ui.add_space(40.0);
			});
		});
		pick
	}

	/// YOUR SHOWS, and Open a show file beside it; whether that was clicked.
	fn heading(&mut self, ui: &mut Ui, _runtime: &tokio::runtime::Handle) -> bool {
		ui.horizontal(|ui| {
			ui.vertical(|ui| {
				ui.label(kit::caps("Your shows", kit::display(44.0)).color(kit::TEXT));
				ui.label(
					egui::RichText::new("Open one to pick up where it ended. Nothing goes live until you say so.")
						.font(kit::ui(14.0))
						.color(kit::DIM),
				);
			});
			ui.with_layout(egui::Layout::right_to_left(Align::Max), |ui| {
				let open = kit::button(Icon::Open, "Open a show file").min_size(vec2(0.0, 38.0));
				ui.add_enabled(self.dialog.is_none(), open).clicked()
			})
			.inner
		})
		.inner
	}

	/// Open the system's dialog: for a new show's file if `new`, or an
	/// existing one's.
	fn dialog(&mut self, runtime: &tokio::runtime::Handle, new: bool) {
		let dialog = Task::spawn(runtime, move || async move {
			let dialog = rfd::AsyncFileDialog::new().add_filter("Show", &["toml"]);
			let picked = match new {
				true => {
					dialog
						.set_title("Where to keep the new show")
						.set_file_name("show.toml")
						.save_file()
						.await
				}
				false => dialog.set_title("Open a show").pick_file().await,
			};
			picked.map(|file| file.path().to_path_buf())
		});
		match dialog {
			Ok(dialog) => self.dialog = Some(dialog),
			Err(err) => self.error = Some(format!("cannot open the file dialog: {err}")),
		}
	}

	/// The New show card and a card for each show, three to a row.
	fn cards(
		&mut self,
		ui: &mut Ui,
		recent: &mut Recent,
		width: f32,
		runtime: &tokio::runtime::Handle,
	) -> Option<Pick> {
		let columns = if width > 900.0 { 3 } else { 2 };
		let card = (width - GAP * (columns - 1) as f32) / columns as f32;
		let size = vec2(card, card * 9.0 / 16.0 + LABEL);
		let mut pick = None;
		let mut forget = None;
		let count = recent.shows.len() + 1;
		for row in 0..count.div_ceil(columns) {
			ui.horizontal(|ui| {
				ui.spacing_mut().item_spacing.x = GAP;
				for column in 0..columns {
					let index = row * columns + column;
					if index == 0 {
						if self.new_card(ui, size).clicked() {
							self.dialog(runtime, true);
						}
						continue;
					}
					let Some(show) = recent.shows.get(index - 1) else { break };
					let response = self.show_card(ui, show, size);
					if response.clicked() && self.found(&show.path) {
						pick = Some(Pick::Open(show.path.clone()));
					}
					response.context_menu(|ui| {
						if ui.add(kit::button(Icon::Remove, "Forget this show")).clicked() {
							forget = Some(show.path.clone());
						}
					});
				}
			});
			ui.add_space(GAP);
		}
		if let Some(path) = forget {
			recent.forget(&path, self.dir.as_deref());
			self.pictures.remove(&path);
		}
		pick
	}

	/// Whether the show at `path` is still there, checked the first time.
	fn found(&mut self, path: &Path) -> bool {
		*self
			.found
			.entry(path.to_path_buf())
			.or_insert_with(|| path.try_exists().unwrap_or(false))
	}

	/// The card that makes a new show.
	fn new_card(&mut self, ui: &mut Ui, size: Vec2) -> egui::Response {
		let (rect, response) = ui.allocate_exact_size(size, Sense::click());
		response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "New show"));
		let hovered = response.hovered();
		let painter = ui.painter();
		let stroke = Stroke::new(1.5, if hovered { kit::DIM } else { kit::LINE });
		crate::ui::dashed(painter, rect.shrink(1.0), stroke);
		let center = rect.center() - vec2(0.0, 26.0);
		painter.circle_filled(center, 30.0, if hovered { Color32::WHITE } else { kit::TEXT });
		Icon::Add.paint(ui, Rect::from_center_size(center, Vec2::splat(22.0)), 22.0, kit::BG);
		let name = kit::caps_galley(painter, "New show", kit::display(24.0), kit::TEXT, None);
		let at = pos2(rect.center().x - name.size().x / 2.0, center.y + 44.0);
		painter.galley(at, name, kit::TEXT);
		painter.text(
			pos2(rect.center().x, at.y + 38.0),
			egui::Align2::CENTER_TOP,
			"Pick a shape, a camera, and a layout",
			kit::ui(13.0),
			kit::DIM,
		);
		response
	}

	/// A show's card: its picture, its name, and when it was last live.
	fn show_card(&mut self, ui: &mut Ui, show: &Show, size: Vec2) -> egui::Response {
		let found = self.found(&show.path);
		let picture = self
			.pictures
			.entry(show.path.clone())
			.or_insert_with(|| {
				let image = load_picture(self.dir.as_deref()?, &show.path)?;
				let name = format!("show picture {}", show.path.display());
				Some(ui.ctx().load_texture(name, image, egui::TextureOptions::LINEAR))
			})
			.clone();
		let name = show
			.path
			.file_stem()
			.unwrap_or(show.path.as_os_str())
			.to_string_lossy()
			.into_owned();
		let (rect, response) = ui.allocate_exact_size(size, Sense::click());
		response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name));
		let response = response.on_hover_text(show.path.display().to_string());
		let painter = ui.painter();
		let hovered = response.hovered() && found;
		painter.rect_filled(rect, 12, kit::P1);
		let top = rect.with_max_y(rect.bottom() - LABEL);
		let clip = painter.with_clip_rect(top.intersect(painter.clip_rect()));
		clip.rect_filled(
			top,
			egui::CornerRadius {
				nw: 12,
				ne: 12,
				sw: 0,
				se: 0,
			},
			Color32::BLACK,
		);
		match &picture {
			Some(texture) => {
				let shape = texture.aspect_ratio();
				let fit = crate::ui::fit(top, shape);
				let image = egui::Image::new(texture).corner_radius(0).tint(match found {
					true => Color32::WHITE,
					false => Color32::from_gray(90),
				});
				image.paint_at(&ui.new_child(UiBuilder::new().max_rect(top)), fit);
			}
			None => {
				let icon = if show.portrait { Icon::Portrait } else { Icon::Landscape };
				icon.paint(
					ui,
					Rect::from_center_size(top.center(), Vec2::splat(36.0)),
					36.0,
					kit::FAINT,
				);
			}
		}
		let painter = ui.painter();
		let label = rect.with_min_y(top.bottom());
		let ink = if found { kit::TEXT } else { kit::DIM };
		let title = kit::caps_galley(painter, &name, kit::display_bold(19.0), ink, Some(label.width() - 60.0));
		painter.galley(pos2(label.left() + 14.0, label.top() + 14.0), title, ink);
		let line = match found {
			true => said(show, crate::library::now()),
			false => "Not where it was; right-click to forget it".to_string(),
		};
		painter.text(
			pos2(label.left() + 14.0, label.top() + 44.0),
			egui::Align2::LEFT_TOP,
			line,
			kit::ui(12.5),
			kit::DIM,
		);
		let icon = if show.portrait { Icon::Portrait } else { Icon::Landscape };
		let at = Rect::from_center_size(pos2(label.right() - 26.0, label.center().y), Vec2::splat(18.0));
		icon.paint(ui, at, 18.0, kit::DIM);
		let stroke = match hovered {
			true => Stroke::new(1.5, kit::DIM),
			false => Stroke::new(1.0, kit::LINE),
		};
		ui.painter().rect_stroke(rect, 12, stroke, egui::StrokeKind::Inside);
		response
	}
}

/// What a show's card says of it: when it was last live and for how long, or
/// when it was opened if it never went live.
pub fn said(show: &Show, now: u64) -> String {
	match show.live {
		Some(aired) => format!(
			"Live {}, for {}",
			ago(now.saturating_sub(aired.at)),
			lasted(aired.lasted)
		),
		None => format!("Opened {}, never live", ago(now.saturating_sub(show.opened))),
	}
}

/// `seconds` ago, as a person would say it.
fn ago(seconds: u64) -> String {
	const MINUTE: u64 = 60;
	const HOUR: u64 = 60 * MINUTE;
	const DAY: u64 = 24 * HOUR;
	const WEEK: u64 = 7 * DAY;
	const MONTH: u64 = 30 * DAY;
	const YEAR: u64 = 365 * DAY;
	if seconds < MINUTE {
		return "just now".into();
	}
	// Each unit, below which the next takes over, and what one of it ago is.
	let units = [
		(MINUTE, HOUR, "minute", None),
		(HOUR, DAY, "hour", None),
		(DAY, WEEK, "day", Some("yesterday")),
		(WEEK, MONTH, "week", Some("last week")),
		(MONTH, YEAR, "month", Some("last month")),
		(YEAR, u64::MAX, "year", Some("last year")),
	];
	let (unit, _, name, last) = units
		.into_iter()
		.find(|(_, below, ..)| seconds < *below)
		.unwrap_or(units[units.len() - 1]);
	match (seconds / unit, last) {
		(1, Some(last)) => last.into(),
		(1, None) => format!("1 {name} ago"),
		(count, _) => format!("{count} {name}s ago"),
	}
}

/// `seconds` on air, to the minute past the first.
fn lasted(seconds: u64) -> String {
	let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
	match (hours, minutes) {
		(0, 0) => format!("{seconds} s"),
		(0, minutes) => format!("{minutes} min"),
		(hours, 0) => format!("{hours} h"),
		(hours, minutes) => format!("{hours} h {minutes} min"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A show opened again moves to the front rather than appearing twice, and
	/// keeps when it was last live.
	#[test]
	fn reopening_moves_a_show_to_the_front() {
		let mut recent = Recent::default();
		recent.opened(Path::new("/shows/a.toml"), 10);
		recent.opened(Path::new("/shows/b.toml"), 20);
		recent.aired(Path::new("/shows/a.toml"), Aired { at: 12, lasted: 60 });
		recent.opened(Path::new("/shows/a.toml"), 30);
		let paths: Vec<_> = recent.shows.iter().map(|show| show.path.clone()).collect();
		assert_eq!(paths, [PathBuf::from("/shows/a.toml"), PathBuf::from("/shows/b.toml")]);
		assert_eq!(recent.shows[0].opened, 30);
		assert_eq!(recent.shows[0].live, Some(Aired { at: 12, lasted: 60 }));
	}

	/// A card says when the show was last live and for how long, as a person
	/// would.
	#[test]
	fn says_when_a_show_was_live() {
		let day = 24 * 3600;
		let show = |live| Show {
			path: "/shows/a.toml".into(),
			opened: 0,
			live,
			portrait: false,
		};
		let now = 400 * day;
		let aired = |ago: u64, lasted: u64| Some(Aired { at: now - ago, lasted });
		assert_eq!(
			said(&show(aired(3 * day, 3 * 3600 + 120)), now),
			"Live 3 days ago, for 3 h 2 min"
		);
		assert_eq!(said(&show(aired(8 * day, 6000)), now), "Live last week, for 1 h 40 min");
		assert_eq!(
			said(&show(aired(14 * day, 55 * 60)), now),
			"Live 2 weeks ago, for 55 min"
		);
		assert_eq!(said(&show(aired(40 * day, 7200)), now), "Live last month, for 2 h");
		assert_eq!(said(&show(None), now), "Opened last year, never live");
	}

	/// A picture shrinks by averaging, to the width asked for at most.
	#[test]
	fn shrinks_a_picture() {
		let picture = Picture {
			width: 4,
			height: 2,
			rgba: [[0, 0, 0, 255], [200, 100, 50, 255]].repeat(4).concat(),
		};
		let small = picture.shrink(2);
		assert_eq!((small.width, small.height), (2, 1));
		assert_eq!(&small.rgba[..4], &[100, 50, 25, 255]);
	}
}
