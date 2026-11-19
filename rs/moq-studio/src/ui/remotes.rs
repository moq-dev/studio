//! Settings, Remote: pair a phone by its QR code, see which are connected,
//! and remove one.

use std::sync::Arc;

use egui::{RichText, Ui};

use super::{Panels, Setting, Shown, expiry, kit, qr};
use crate::kit::Icon;

impl Panels {
	/// Pair phones as remotes of the guests' room, and list those paired.
	pub(super) fn remote_settings(&mut self, ui: &mut Ui) {
		let Some(room) = self.engine.room() else {
			let said = match self.controls.room_failure() {
				Some(failure) => format!("The room cannot be joined: {failure}"),
				None => "A remote joins the guests' room on your relay, which needs a room and its key.".into(),
			};
			ui.label(RichText::new(said).font(kit::ui(12.5)).color(kit::TEXT));
			ui.add_space(8.0);
			if ui.add(kit::button(Icon::Guests, "Set up the room")).clicked() {
				self.setting = Some(Setting::Guests);
			}
			return;
		};
		// A link into a room the studio has left joins nothing.
		self.paired.take_if(|shown| !shown.joins(&room));

		let mut pair = None;
		ui.horizontal(|ui| {
			ui.label("Name");
			let field = ui.add(
				egui::TextEdit::singleline(&mut self.phone)
					.id_salt("remote name")
					.hint_text("phone"),
			);
			let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
			if ui.add(kit::button(Icon::Phone, "Pair a phone")).clicked() || entered {
				pair = Some(self.phone.trim().to_string());
			}
		});

		let now = kio::clock::wall();
		if let Some(shown) = &self.paired {
			ui.add_space(10.0);
			ui.label(format!(
				"Scan with {}'s camera. The link {}.",
				shown.name,
				expiry(shown.invite.expires, now)
			));
			ui.add_space(8.0);
			// The link is the phone's credential, so it shows only as a code to scan.
			match &shown.qr {
				Ok(code) => {
					ui.image((code.id(), egui::vec2(240.0, 240.0)));
				}
				Err(err) => {
					ui.colored_label(kit::ERR, format!("{err}; copy the link instead."));
				}
			}
			ui.add_space(8.0);
			let done = ui.horizontal(|ui| {
				if ui.add(kit::button(Icon::Copy, "Copy link")).clicked() {
					ui.ctx().copy_text(shown.invite.link.to_string());
				}
				ui.add(kit::button(Icon::Close, "Done")).clicked()
			});
			if done.inner {
				self.paired = None;
			}
		}

		ui.add_space(10.0);
		ui.separator();
		if let Some(failure) = room.renewal_failure() {
			ui.colored_label(
				kit::ERR,
				format!("The studio is not in the room, so no remote connects: {failure}"),
			);
		}
		let remotes = room.remotes();
		if remotes.is_empty() {
			ui.weak("No phone paired yet.");
		}
		let mut show = None;
		let mut remove = None;
		egui::Grid::new("remotes").striped(true).show(ui, |ui| {
			if remotes.is_empty() {
				return;
			}
			for heading in ["Phone", "Status", "Link", ""] {
				ui.strong(heading);
			}
			ui.end_row();
			for (name, remote) in &remotes {
				ui.label(name);
				match (remote.removed, remote.connected) {
					(true, _) => ui.weak("removed"),
					(false, true) => ui.colored_label(kit::OK, "connected"),
					(false, false) => ui.weak("offline"),
				};
				// A connected remote no longer depends on its link's expiry.
				match &remote.invite {
					_ if remote.connected => ui.label("renewing"),
					_ if remote.removed => ui.weak("not renewed"),
					Some(link) => ui.label(expiry(link.expires, now)),
					None => ui.weak("paired before this session"),
				};
				ui.horizontal(|ui| {
					// Only a link that still works is worth scanning again.
					if let Some(link) = &remote.invite
						&& !remote.removed
						&& link.expires > now
						&& ui.small_button("Show link").clicked()
					{
						show = Some((name.clone(), link.clone()));
					}
					if ui.small_button("New link").clicked() {
						pair = Some(name.clone());
					}
					if !remote.removed
						&& ui
							.small_button("Remove")
							.on_hover_text(
								"Stop reading its presses at once. Its token stops renewing, so it is disconnected within the link lifetime in Settings, Guests. Only New link pairs it again.",
							)
							.clicked()
					{
						remove = Some(name.clone());
					}
				});
				ui.end_row();
			}
		});

		if let Some((name, invite)) = show {
			self.show_remote(ui.ctx(), &room, name, invite);
		}
		if let Some(name) = pair {
			match room.pair(&name) {
				Ok(invite) => {
					self.show_remote(ui.ctx(), &room, name, invite);
					self.phone.clear();
				}
				Err(err) => self.report(Err(err)),
			}
		}
		if let Some(name) = remove {
			let result = room.unpair(&name).map(|()| format!("removed the remote {name}"));
			if result.is_ok() && self.paired.as_ref().is_some_and(|shown| shown.name == name) {
				self.paired = None;
			}
			self.report(result);
		}
	}

	/// Put `invite`'s link for the remote `name` on screen as a QR code.
	fn show_remote(
		&mut self,
		ctx: &egui::Context,
		room: &Arc<crate::guests::Room>,
		name: String,
		invite: crate::guests::Invite,
	) {
		let qr = qr(ctx, invite.link.as_str());
		let room = Arc::downgrade(room);
		self.paired = Some(Shown { name, invite, qr, room });
	}
}
