//! The Show menu's shows: switching to a recent one, a new one, one from a
//! file, renaming this one, and exporting a copy of it. Each but the export
//! closes this show, which the window does once [`Studio::leaving`] says so.

use std::path::PathBuf;

use super::*;
use crate::recent;
use crate::task::Task;

/// The most room the recent shows take in the Show menu: about eight rows.
const RECENT_HEIGHT: f32 = 240.0;

/// What the Show menu asks the window for; each closes the open show first.
#[derive(Clone, Debug, PartialEq)]
pub enum Leave {
	/// Open the show at this path.
	Open(PathBuf),
	/// Make a new show in the app's storage and open it.
	New,
	/// Rename the show to this and open it again.
	Rename(String),
}

/// A system file dialog the Show menu opened, and what it is for.
pub(super) enum Dialog {
	/// A show file to open.
	Open(Task<Option<PathBuf>>),
	/// Where to write a copy of the show.
	Export(Task<Option<PathBuf>>),
}

impl Panels {
	/// The Show menu's shows, under the open show's file: the other recent
	/// ones, then New show, Open show file, Rename, and Export a copy. Only
	/// the export works on air, since the rest close the show.
	pub(super) fn show_list(&mut self, ui: &mut Ui) {
		let off = matches!(self.engine.on_air(), crate::engine::OnAir::Off) && !self.controls.going();
		let why = |response: egui::Response| match off {
			true => response,
			false => kit::tip(response, "End the stream first", None),
		};
		let now = recent::now();
		let mut leave = None;
		ui.add_enabled_ui(off, |ui| {
			// Up to 30 shows, which scroll so the rest stay in reach in a short
			// window.
			let height = (ui.ctx().content_rect().height() / 3.0).min(RECENT_HEIGHT);
			egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
				for show in &self.shows {
					let name = crate::show::name(&show.path);
					let row = ui.add(kit::button(Icon::Open, &name));
					let row = match off {
						true => row.on_hover_text(format!("{}\n{}", recent::said(show, now), show.path.display())),
						false => why(row),
					};
					if row.clicked() {
						leave = Some(Leave::Open(show.path.clone()));
					}
				}
			});
			if !self.shows.is_empty() {
				ui.separator();
			}
			if why(ui.add(kit::button(Icon::Add, "New show"))).clicked() {
				leave = Some(Leave::New);
			}
			let busy = self.show_dialog.is_some();
			let open = ui.add_enabled(!busy, kit::button(Icon::Folder, "Open show file…"));
			if why(open).clicked() {
				self.show_dialog = Some(Dialog::Open(file_dialog(&self.runtime, None)));
			}
			if why(ui.add(kit::button(Icon::Rename, "Rename show…"))).clicked() {
				let name = crate::show::name(self.engine.autosave.path());
				self.renaming_show = Some((name.clone(), name));
				ui.close();
			}
		});
		let busy = self.show_dialog.is_some();
		let export = ui
			.add_enabled(!busy, kit::button(Icon::Save, "Export show copy…"))
			.on_hover_text("Save the scenes as a scene file of their own, to share or keep elsewhere");
		if export.clicked() {
			let name = crate::show::name(self.engine.autosave.path());
			self.show_dialog = Some(Dialog::Export(file_dialog(&self.runtime, Some(name))));
		}
		if leave.is_some() {
			self.leaving = leave;
		}
	}

	/// Follow the Show menu's file dialog and the rename sheet, which outlive
	/// the menu.
	pub(super) fn follow_shows(&mut self, ctx: &egui::Context) {
		if let Some((from, to)) = super::shell::rename_sheet(ctx, "rename show", &mut self.renaming_show)
			&& to != from
		{
			self.leaving = Some(Leave::Rename(to));
		}
		let picked = match &mut self.show_dialog {
			Some(Dialog::Open(task) | Dialog::Export(task)) => task.poll().cloned(),
			None => return,
		};
		ctx.request_repaint_after(Duration::from_millis(100));
		let Some(picked) = picked else { return };
		let dialog = self.show_dialog.take();
		let Some(path) = picked else { return };
		match dialog {
			Some(Dialog::Open(_)) => self.leaving = Some(Leave::Open(path)),
			Some(Dialog::Export(_)) => {
				let collection = self.engine.state.lock().unwrap().collection().clone();
				let result = crate::show::export(&collection, self.engine.autosave.path(), &path)
					.map(|()| format!("Exported a copy to {}", path.display()))
					.with_context(|| format!("cannot export to {}", path.display()));
				self.report(result);
			}
			None => {}
		}
	}
}

impl Studio {
	/// What the Show menu asked for since this last said so, which the window
	/// answers by closing the show and opening the next, or
	/// [`refuse`](Self::refuse).
	pub fn leaving(&mut self) -> Option<Leave> {
		self.panels.leaving.take()
	}

	/// List the shows in `recent` other than this one, which are still there,
	/// in the Show menu.
	pub fn list(&mut self, recent: &recent::Recent) {
		let open = std::path::absolute(self.panels.engine.autosave.path()).ok();
		self.panels.shows = recent
			.shows
			.iter()
			.filter(|show| Some(&show.path) != open.as_ref() && show.path.try_exists().unwrap_or(false))
			.cloned()
			.collect();
	}
}

/// The system's dialog for a show file: one to open, or with `export`, the
/// show's name, where to save a copy.
fn file_dialog(runtime: &tokio::runtime::Handle, export: Option<String>) -> Task<Option<PathBuf>> {
	let dialog = Task::spawn(runtime, move || async move {
		let dialog = rfd::AsyncFileDialog::new().add_filter("Show", &["toml"]);
		let picked = match export {
			Some(name) => {
				dialog
					.set_title("Export a copy of the show")
					.set_file_name(format!("{name}.toml"))
					.save_file()
					.await
			}
			None => dialog.set_title("Open a show").pick_file().await,
		};
		picked.map(|file| file.path().to_path_buf())
	});
	dialog.unwrap_or_else(|err| {
		tracing::warn!(%err, "cannot open the file dialog");
		Task::Done(None)
	})
}
