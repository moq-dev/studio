//! Guests: people who join the studio's room from a web page, whose camera and
//! screen become broadcast sources.
//!
//! The room is a moq-room prefix on the studio's relay. Inviting a guest signs
//! a short-lived token with [`moq_room::claims`], so the guest publishes only
//! under its own `<identity>/`, and hands it to the join page in a link's
//! fragment, which no server logs. The guest publishes moq-room's
//! `<identity>/camera.hang` and `<identity>/screen.hang`, and the studio adds
//! each as a muted [`Source::Broadcast`] when it is announced. Only the room key
//! mints those tokens, so guests can make the studio hold at most two sources
//! per name it signed for.
//!
//! A token cannot be revoked, and the relay ends a session at its expiry. So
//! while a guest is in the room, the studio signs them a fresh token every half
//! `--guest-expiry` and publishes it under `.renew/`, encrypted with moq-e2ee to
//! a key only their link carries, at a path only their token may subscribe to.
//! The join page presents each one in band. Removing a guest from scenes stops
//! their renewals, across restarts, until a new link, so their last token
//! bounds how long they stay.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context;
use moq_net::Path;
use moq_room::Kind;
use moq_scene::{Collection, Draft, Filter, History, Item, Layer, Source, Video};
use url::Url;

use crate::engine::Engine;

/// The domain `.github/workflows/join.yml` deploys `js/join` to.
const JOIN_PAGE: &str = "https://join.studio.moq.dev/";

/// The prefix of each guest's renewals, which moq-room's announcements hide.
/// Beneath it, moq-e2ee names the broadcast after the guest's key.
const RENEW: &str = ".renew";

/// The semantic broadcast and track names of a guest's renewals, one token per
/// group. `js/join` reads the same names.
const RENEW_BROADCAST: &str = "renew";
const TOKEN_TRACK: &str = "token";

/// The shortest `--guest-expiry`: half of it must leave room for a renewal to
/// reach the guest and the relay before the token it replaces expires.
const MIN_EXPIRY: Duration = Duration::from_secs(60);

/// The guest flags.
#[derive(usage_rs::Args, Clone, Debug)]
#[usage(unknown_flags = "error", args_override_self = false)]
pub struct Config {
	/// The room guests join: a path under the `--connect` URL's, which the
	/// studio's own connection must be allowed to subscribe to. Needs
	/// `--room-key`.
	#[usage(long)]
	pub room: Option<String>,

	/// The key that signs guest tokens, as `moq auth generate` writes it. Its
	/// scope must stay inside the room, so a stolen key cannot sign for the rest
	/// of the relay; an unscoped key is refused.
	#[usage(long, value_hint = usage_rs::ValueHint::FilePath, extensions("jwk", "json"))]
	pub room_key: Option<PathBuf>,

	/// The join page a guest opens, `js/join` hosted anywhere. Its link carries
	/// the room's relay URL, the guest's name, and its token in the fragment.
	/// Defaults to `https://join.studio.moq.dev/`.
	#[usage(long)]
	pub join_page: Option<Url>,

	/// How long a join link, and each token the studio renews it with, works. A
	/// guest in the room is renewed every half of this, so a guest removed from
	/// scenes is disconnected within it. At least a minute.
	#[usage(long, default = "4h")]
	pub guest_expiry: moq_tokio::cli::Duration,
}

impl Config {
	/// The room these flags describe on the relay at `connect`, or `None` without
	/// `--room`, remembering who was removed from scenes beside the scene file
	/// at `scenes`.
	pub fn load(&self, connect: Option<&Url>, scenes: &std::path::Path) -> anyhow::Result<Option<Room>> {
		let (room, key) = match (&self.room, &self.room_key, &self.join_page) {
			(None, None, None) => return Ok(None),
			(Some(room), Some(key), _) => (room, key),
			_ => anyhow::bail!("guests need both --room and --room-key"),
		};
		let page = self
			.join_page
			.clone()
			.unwrap_or_else(|| Url::parse(JOIN_PAGE).expect("the default join page is a URL"));
		let connect = connect.context("guests join through the relay; add --connect")?;
		let key = moq_auth::Key::from_file(key).with_context(|| format!("reading the room key {}", key.display()))?;
		let room = Room::new(connect, room, key, page, self.guest_expiry.into())?;
		Ok(Some(room.remember(removals_path(scenes))?))
	}
}

/// The moq-e2ee credential of `identity`'s renewals under `secret`, as
/// `credential` in `js/join/src/renew.ts` builds it.
fn credential(identity: &str, secret: [u8; moq_e2ee::SECRET_LEN]) -> anyhow::Result<moq_e2ee::Credential> {
	Ok(moq_e2ee::Credential::new(moq_e2ee::credential::Config {
		context: bytes::Bytes::copy_from_slice(identity.as_bytes()),
		kid: 0,
		secret,
	})?)
}

/// Where the guests removed from scenes are kept: `show.guests.toml` for
/// `show.toml`, beside its history.
fn removals_path(scenes: &std::path::Path) -> PathBuf {
	let mut name = scenes.file_stem().unwrap_or_default().to_os_string();
	name.push(".guests.toml");
	scenes.with_file_name(name)
}

/// The file at [`removals_path`].
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Removals {
	/// The guests whose tokens are not renewed until the host makes them a new link.
	removed: BTreeSet<String>,
}

/// The studio's room: who was invited, and what each guest publishes.
pub struct Room {
	/// The room, relative to the studio's connection.
	path: Path,
	/// The room on the relay: what guest tokens are rooted at.
	root: Path,
	/// Where a guest dials: the relay at the room, without the studio's credentials.
	url: Url,
	key: moq_auth::Key,
	/// Derives each guest's renewal key, from the room key so it survives a restart.
	renewals: rind::hmac::Key,
	page: Url,
	expiry: Duration,
	participants: Mutex<BTreeMap<String, Participant>>,
	/// Where removals are kept, or `None` to keep them in memory alone.
	removals: Option<PathBuf>,
	/// Wakes the renewals when who is renewed changes.
	changed: tokio::sync::Notify,
	/// The session publishing the renewals, once it dials.
	session: OnceLock<moq_tokio::connection::Monitor>,
}

/// One guest the studio knows of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Participant {
	/// The newest link the host made for this guest this session, if any.
	pub invite: Option<Invite>,
	/// Whether the guest's camera broadcast is announced.
	pub camera: bool,
	/// Whether the guest's screen broadcast is announced.
	pub screen: bool,
	/// Whether the host removed them from scenes since their newest link, which
	/// stops renewing their token, across restarts.
	pub removed: bool,
}

impl Participant {
	/// Whether the studio renews their token: they are in the room, and the host
	/// has not removed them.
	pub fn renewed(&self) -> bool {
		// The join page announces the camera broadcast for as long as it is joined.
		self.camera && !self.removed
	}
}

/// A join link for one guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
	/// The join page, with the relay, name, and token in its fragment.
	pub link: Url,
	/// When the token, and so the link, stops working.
	pub expires: kio::clock::Wall,
}

impl Room {
	/// The room at `room` under the `connect` URL's path, whose guests `key`
	/// signs for and who join at `page` within `expiry`.
	///
	/// Refuses a key whose scope reaches outside the room, or that cannot sign a
	/// guest's claims, and a relay, page, or expiry no guest could join with.
	pub fn new(connect: &Url, room: &str, key: moq_auth::Key, page: Url, expiry: Duration) -> anyhow::Result<Self> {
		anyhow::ensure!(
			matches!(connect.scheme(), "https" | "http"),
			"guests join from a browser, which cannot dial {}://; connect over https://",
			connect.scheme()
		);
		anyhow::ensure!(
			connect.scheme() == "https" || local(connect),
			"a guest's browser blocks http:// to {} from a secure page; \
			 connect over https://, or over http:// only to this machine or the local network",
			connect.host_str().unwrap_or_default()
		);
		anyhow::ensure!(
			page.scheme() == "https" || (page.scheme() == "http" && loopback(&page)),
			"browsers open the camera only on a secure page, so the join page {page} must be https://, \
			 or http:// on localhost"
		);
		anyhow::ensure!(
			expiry >= MIN_EXPIRY,
			"--guest-expiry must be at least a minute, so a renewal reaches each guest before their token expires"
		);
		let path = Path::new(room).with_context(|| format!("invalid room `{room}`"))?;
		anyhow::ensure!(!path.is_empty(), "the room needs a path");
		let base = Path::new(connect.path().trim_matches('/')).context("invalid --connect path")?;
		let root = base.join(&path).context("the room path is too long")?;

		// Only the relay and the room: never the studio's own `?jwt=`.
		let mut url = connect.clone();
		url.set_query(None);
		url.set_fragment(None);
		let _ = url.set_username("");
		let _ = url.set_password(None);
		url.set_path(root.as_str());
		// The relay compares a token's root with the dialed path as sent, so a room
		// the URL would percent-encode could never match its guests' tokens.
		anyhow::ensure!(
			url.path().strip_prefix('/') == Some(root.as_str()),
			"the room `{room}` changes when written in a URL; use only letters, digits, and `-_.~`"
		);

		check_scope(&key, &root)?;
		// The private key, which only the studio holds, so no one else can derive
		// a guest's renewal key.
		let secret = match &key.material {
			moq_auth::KeyMaterial::EC { d: Some(d), .. } | moq_auth::KeyMaterial::OKP { d: Some(d), .. } => d,
			moq_auth::KeyMaterial::RSA {
				private: Some(private), ..
			} => &private.d,
			moq_auth::KeyMaterial::OCT { secret } => secret,
			_ => anyhow::bail!("the room key has no private half, so it cannot sign guests' tokens"),
		};
		let renewals = rind::hmac::Key::new(&rind::hmac::HMAC_SHA256, secret);
		let room = Self {
			path,
			root,
			url,
			key,
			renewals,
			page,
			expiry,
			participants: Mutex::default(),
			removals: None,
			changed: Default::default(),
			session: Default::default(),
		};
		// Sign once now, so a key that cannot sign for guests or for the studio's
		// own session fails at startup.
		room.sign("guest")?;
		room.sign_session()?;
		Ok(room)
	}

	/// Keep removals in the file at `path`, starting from those it holds, so a
	/// restart does not renew a guest the host removed.
	fn remember(mut self, path: PathBuf) -> anyhow::Result<Self> {
		let removals: Removals = match std::fs::read_to_string(&path) {
			Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => Removals::default(),
			Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
		};
		let participants = self.participants.get_mut().unwrap();
		for identity in removals.removed {
			participants.entry(identity).or_default().removed = true;
		}
		self.removals = Some(path);
		Ok(self)
	}

	/// Mark `identity` removed or not, once the file holds it, so a failed
	/// write leaves both as they were.
	fn set_removed(
		&self,
		participants: &mut BTreeMap<String, Participant>,
		identity: &str,
		removed: bool,
	) -> anyhow::Result<()> {
		let participant = participants.entry(identity.to_string()).or_default();
		let before = std::mem::replace(&mut participant.removed, removed);
		let saved = self.save_removals(participants);
		if saved.is_err() {
			participants.entry(identity.to_string()).or_default().removed = before;
		}
		saved
	}

	/// Write the removals to their file, if they are kept in one.
	fn save_removals(&self, participants: &BTreeMap<String, Participant>) -> anyhow::Result<()> {
		let Some(path) = &self.removals else { return Ok(()) };
		let removed = participants.iter().filter(|(_, participant)| participant.removed);
		let removals = Removals {
			removed: removed.map(|(identity, _)| identity.clone()).collect(),
		};
		let context = || format!("writing {}", path.display());
		let dir = match path.parent() {
			Some(dir) if !dir.as_os_str().is_empty() => dir,
			_ => std::path::Path::new("."),
		};
		// Replaced whole, so a crash never leaves half a file.
		let mut file = tempfile::NamedTempFile::new_in(dir).with_context(context)?;
		std::io::Write::write_all(&mut file, toml::to_string(&removals)?.as_bytes()).with_context(context)?;
		file.persist(path).map_err(|err| err.error).with_context(context)?;
		Ok(())
	}

	/// The room, relative to the studio's connection.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Every guest the studio knows of, by name.
	pub fn participants(&self) -> BTreeMap<String, Participant> {
		self.participants.lock().unwrap().clone()
	}

	/// Make a join link for the guest called `name`, replacing any earlier one.
	/// An earlier link keeps working until it expires: a signed token cannot be
	/// taken back.
	pub fn invite(&self, name: &str) -> anyhow::Result<Invite> {
		let name = name.trim();
		anyhow::ensure!(
			!name.contains(['/', '\\']),
			"a guest's name is one path segment, without `/` or `\\`"
		);
		// The room's announcements hide a path segment starting with `.`, so the
		// studio would never see this guest join.
		anyhow::ensure!(!name.starts_with('.'), "a guest's name cannot start with `.`");
		let (token, expires) = self.sign(name)?;
		let secret: String = self.secret(name).iter().map(|byte| format!("{byte:02x}")).collect();
		let mut link = self.page.clone();
		let fragment = url::form_urlencoded::Serializer::new(String::new())
			.append_pair("url", self.url.as_str())
			.append_pair("name", name)
			.append_pair("jwt", &token)
			.append_pair("key", &secret)
			.finish();
		link.set_fragment(Some(&fragment));
		let invite = Invite { link, expires };
		let mut participants = self.participants.lock().unwrap();
		// A new link lets a removed guest back in.
		if participants.get(name).is_some_and(|participant| participant.removed) {
			self.set_removed(&mut participants, name, false)?;
		}
		participants.entry(name.to_string()).or_default().invite = Some(invite.clone());
		drop(participants);
		self.changed.notify_one();
		Ok(invite)
	}

	/// Stop renewing the token of the guest called `identity`, across restarts,
	/// until the host makes them a new link.
	pub fn stop_renewing(&self, identity: &str) -> anyhow::Result<()> {
		let mut participants = self.participants.lock().unwrap();
		if !participants
			.get(identity)
			.is_some_and(|participant| participant.removed)
		{
			self.set_removed(&mut participants, identity, true)?;
		}
		drop(participants);
		self.changed.notify_one();
		Ok(())
	}

	/// The key that encrypts `identity`'s renewals, which only their links carry.
	fn secret(&self, identity: &str) -> [u8; moq_e2ee::SECRET_LEN] {
		let tag = rind::hmac::sign(&self.renewals, identity.as_bytes());
		tag.as_ref()
			.try_into()
			.expect("HMAC-SHA256 is as long as a moq-e2ee secret")
	}

	/// The moq-e2ee credential of `identity`'s renewals.
	fn credential(&self, identity: &str) -> anyhow::Result<moq_e2ee::Credential> {
		credential(identity, self.secret(identity))
	}

	/// Where `identity`'s renewals are published, in the room: a name only their
	/// key derives.
	fn renewal_path(credential: &moq_e2ee::Credential) -> anyhow::Result<Path> {
		Ok(Path::new(RENEW)?.join(&credential.path(RENEW_BROADCAST)?)?)
	}

	/// Why the session publishing guests' renewed tokens is not connected, if it
	/// is not.
	pub fn renewal_failure(&self) -> Option<String> {
		let failure = self.session.get()?.state().failure?;
		Some(format!("{:#}", anyhow::Error::from(failure)))
	}

	/// A token for `identity`, and when it expires. It publishes moq-room's
	/// paths but subscribes only to its own renewals, so no guest reads
	/// another's.
	fn sign(&self, identity: &str) -> anyhow::Result<(String, kio::clock::Wall)> {
		let invalid = || format!("cannot invite `{identity}`");
		let renewals = Self::renewal_path(&self.credential(identity).with_context(invalid)?).with_context(invalid)?;
		let claims = moq_room::claims(self.root.as_str(), identity)
			.with_context(invalid)?
			.with_subscribe([moq_auth::Pattern::literal(renewals.as_str())?]);
		self.sign_claims(claims)
			.context("the room key cannot sign a guest's token")
	}

	/// A token for the studio's own session in the room, which publishes the
	/// renewals and nothing else.
	fn sign_session(&self) -> anyhow::Result<(String, kio::clock::Wall)> {
		let claims = moq_auth::Claims::default()
			.with_root(self.root.as_str())
			.with_publish([moq_auth::Pattern::subtree(RENEW)?]);
		self.sign_claims(claims)
			.context("the room key cannot sign the studio's own session in the room")
	}

	/// Sign `claims` to expire no sooner than one `--guest-expiry` from now.
	fn sign_claims(&self, claims: moq_auth::Claims) -> anyhow::Result<(String, kio::clock::Wall)> {
		// Whole seconds, as the token carries them: issued rounded down and
		// expiring rounded up, so a renewal every half expiry always comes first.
		let now = kio::clock::wall().saturating_duration_since(kio::clock::Wall::UNIX_EPOCH);
		let issued = kio::clock::Wall::UNIX_EPOCH + Duration::from_secs(now.as_secs());
		let end = now + self.expiry;
		let expires =
			kio::clock::Wall::UNIX_EPOCH + Duration::from_secs(end.as_secs() + u64::from(end.subsec_nanos() > 0));
		let token = self.key.sign(&claims.with_issued(issued).with_expires(expires))?;
		Ok((token, expires))
	}

	/// The guests whose tokens are renewed.
	fn renewed(&self) -> Vec<String> {
		let participants = self.participants.lock().unwrap();
		let renewed = participants.iter().filter(|(_, participant)| participant.renewed());
		renewed.map(|(identity, _)| identity.clone()).collect()
	}

	/// Record that the broadcast at `path` in the room went online or offline,
	/// returning the source to add for it, if the collection lacks one. Its
	/// name is one `history` never edited, so the source can be added outside it.
	///
	/// Every guest is tracked, including one invited before this session whose
	/// link still works, so a restart finds the guests already connected.
	fn observe(
		&self,
		path: &Path,
		online: bool,
		collection: &Collection,
		history: &History,
	) -> Option<(String, Source)> {
		let parsed = moq_room::parse(path)?;
		// Only the two paths a link's one-segment identity publishes, so the
		// `<identity>/**` its token grants cannot mint more guests or sources.
		let canonical = moq_room::broadcast_path(parsed.identity.clone(), parsed.kind).ok()?;
		if parsed.identity.parts().count() != 1 || canonical != *path {
			tracing::debug!(%path, "ignoring a broadcast that is not a guest's camera or screen");
			return None;
		}
		let source = self.path.join(path).ok()?.to_string();
		let existing = collection
			.sources
			.values()
			.any(|existing| matches!(existing, Source::Broadcast { path } if *path == source));

		let identity = parsed.identity.to_string();
		let mut participants = self.participants.lock().unwrap();
		let participant = participants.entry(identity.clone()).or_default();
		match parsed.kind {
			Kind::Camera => participant.camera = online,
			Kind::Screen => participant.screen = online,
		}
		drop(participants);
		self.changed.notify_one();
		if !online || existing {
			return None;
		}

		let base = match parsed.kind {
			Kind::Camera => identity,
			Kind::Screen => format!("{identity} screen"),
		};
		Some((
			history.unused_name(collection, &base),
			Source::Broadcast { path: source },
		))
	}

	/// Take the guest called `identity` off the program in `draft`: their
	/// items, the items showing a clone of their sources, and the masks
	/// reading them leave every layer, and their sources are muted. Returns
	/// whether that changed anything.
	pub fn take_off(&self, draft: &mut Draft, identity: &str) -> Result<bool, moq_scene::Error> {
		let before = draft.collection().clone();
		let sources = self.sources_of(&before, identity);
		let ours = |name: &str| sources.iter().any(|source| source == name);
		// A clone shows the same capture, and a mask puts their picture on air.
		let shows = |item: &Item| ours(before.capture(&item.source));
		let reads = |filter: &Filter| filter.source().is_some_and(ours);

		let placed: Vec<(Layer, usize)> = before
			.layers()
			.flat_map(|(layer, items)| {
				let at = items.iter().enumerate().filter(|(_, item)| shows(item));
				at.map(move |(index, _)| (layer.clone(), index)).collect::<Vec<_>>()
			})
			.collect();
		// Back to front, so each index still names the item found.
		for (layer, index) in placed.into_iter().rev() {
			draft.remove_item(&layer, index)?;
		}

		let masked: Vec<(Layer, usize, Item)> = draft
			.collection()
			.layers()
			.flat_map(|(layer, items)| {
				let at = items
					.iter()
					.enumerate()
					.filter(|(_, item)| item.filters.iter().any(reads));
				at.map(move |(index, item)| (layer.clone(), index, item.clone()))
					.collect::<Vec<_>>()
			})
			.collect();
		for (layer, index, mut item) in masked {
			item.filters.retain(|filter| !reads(filter));
			draft.set_item(&layer, index, item)?;
		}
		let masked: Vec<(String, Video)> = before
			.video
			.iter()
			.filter(|(_, video)| video.filters.iter().any(reads))
			.map(|(name, video)| (name.clone(), video.clone()))
			.collect();
		for (name, mut video) in masked {
			video.filters.retain(|filter| !reads(filter));
			draft.set_video(&name, Some(video))?;
		}

		for source in &sources {
			let audio = before.audio.get(source).copied().unwrap_or_default();
			draft.set_audio(source, Some(moq_scene::Audio { muted: true, ..audio }))?;
		}
		Ok(*draft.collection() != before)
	}

	/// The sources in `collection` that show the guest called `identity`.
	pub fn sources_of(&self, collection: &Collection, identity: &str) -> Vec<String> {
		collection
			.sources
			.iter()
			.filter(|(_, source)| {
				let Source::Broadcast { path } = source else {
					return false;
				};
				let Ok(path) = Path::new(path) else { return false };
				path.strip_prefix(&self.path)
					.and_then(moq_room::parse)
					.is_some_and(|parsed| parsed.identity.as_str() == identity)
			})
			.map(|(name, _)| name.clone())
			.collect()
	}
}

/// Whether `url` names this machine, so a browser treats an `http://` page
/// there as secure.
fn loopback(url: &Url) -> bool {
	match url.host() {
		Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
		Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
		Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
		None => false,
	}
}

/// Whether a browser on an `https://` page may reach `url` over `http://`:
/// this machine, or, after a local network prompt, a private address or a
/// `.local` name. Anything else is mixed content it blocks.
fn local(url: &Url) -> bool {
	loopback(url)
		|| match url.host() {
			Some(url::Host::Domain(host)) => host.ends_with(".local"),
			Some(url::Host::Ipv4(ip)) => ip.is_private() || ip.is_link_local(),
			Some(url::Host::Ipv6(ip)) => ip.is_unique_local() || ip.is_unicast_link_local(),
			None => false,
		}
}

/// Refuse a key that could sign beyond `room`: one with no scope, or a scope
/// granting any path outside it.
fn check_scope(key: &moq_auth::Key, room: &Path) -> anyhow::Result<()> {
	let Some(scope) = &key.scope else {
		anyhow::bail!(
			"the room key is unscoped, so it could sign for anywhere on the relay; \
			 generate one with `moq auth generate --root {room} --publish '**' --subscribe '**'`"
		);
	};
	let inside: moq_auth::Patterns = moq_auth::Pattern::subtree(room.as_str())?.into();
	let confined =
		|patterns: &moq_auth::Patterns| patterns.rooted(&scope.root).is_ok_and(|rooted| inside.covers(&rooted));
	anyhow::ensure!(
		confined(&scope.publish) && confined(&scope.subscribe),
		"the room key's scope reaches outside the room `{room}`; generate one with \
		 `moq auth generate --root {room} --publish '**' --subscribe '**'`"
	);
	Ok(())
}

/// Follow the room's announcements for as long as the studio runs, adding each
/// guest's broadcasts as sources.
pub async fn run(engine: Arc<Engine>, origin: moq_net::origin::Consumer) {
	let room = engine.room.as_ref().expect("run only with a room");
	let mut announced = moq_room::Room::new(&origin, None);
	while let Some(event) = announced.next().await {
		let mut state = engine.state.lock().unwrap();
		let online = event.broadcast.is_some();
		let Some((name, source)) = room.observe(&event.path, online, state.collection(), state.history()) else {
			continue;
		};
		// Not the host's edit, so it stays out of what they undo and redo.
		let mut draft = state.collection().draft(format!("Add guest `{name}`"));
		let added = admit(&mut draft, &name, source)
			.map_err(anyhow::Error::from)
			.and_then(|()| state.apply_outside(draft));
		match added {
			Ok(()) => tracing::info!(source = %name, identity = %event.identity, "a guest joined"),
			Err(err) => tracing::error!(source = %name, error = format!("{err:#}"), "cannot add a guest"),
		}
	}
	tracing::warn!("the room's announcements ended");
}

/// The studio's own session in the room, which publishes each renewed guest's
/// tokens, and renews its own.
pub struct Renewal {
	renewals: Renewals,
	connection: moq_tokio::Connection,
}

impl Renewal {
	/// Dial `room`'s relay with `client`'s settings but none of its credentials,
	/// as a session the room key signs for. It offers only moq-lite-07, the
	/// version that renews a token without reconnecting.
	pub fn connect(
		room: Arc<Room>,
		client: &moq_tokio::connect::Config,
		quic: moq_tokio::quic::Config,
	) -> anyhow::Result<Self> {
		let mut client = client.clone();
		// The main connection's socket, if `--connect-bind` fixed its port.
		anyhow::ensure!(
			client.bind.is_none_or(|bind| bind.port() == 0),
			"guests cannot share a fixed --connect-bind port with the studio's session in the room; leave the port 0"
		);
		// The studio's own credentials stay on its own connection: this session
		// holds only what the room key signs, as a guest does.
		client.url = None;
		client.tls.cert = None;
		client.tls.key = None;
		client.tls.identity = None;
		let (token, _) = room.sign_session()?;
		client.tokens = vec![token.try_into()?];
		client.version = vec![LITE_07.parse().map_err(anyhow::Error::msg)?];
		let client = client
			.init(quic)
			.context("cannot set up the studio's session in the room")?;
		let origin = moq_tokio::origin::spawn();
		let connection = client.with_publisher(&origin).connect(room.url.clone());
		let _ = room.session.set(connection.monitor());
		let renewals = Renewals::new(room, origin);
		Ok(Self { renewals, connection })
	}

	/// Publish a token to each guest as they join, and renew every token, the
	/// session's own included, every half `--guest-expiry`, for as long as the
	/// studio runs.
	pub async fn run(mut self) {
		let room = self.renewals.room.clone();
		let period = room.expiry / 2;
		let mut renew = kio::clock::interval_at(kio::clock::now() + period, period);
		loop {
			self.renewals.sync();
			tokio::select! {
				_ = renew.tick() => {
					match room.sign_session().and_then(|(token, _)| Ok(token.try_into()?)) {
						Ok(token) => self.connection.tokens().set(vec![token]),
						Err(err) => tracing::error!(error = format!("{err:#}"), "cannot renew the studio's session in the room"),
					}
					self.renewals.renew();
				}
				() = room.changed.notified() => {}
			}
		}
	}
}

/// The moq-lite version that carries a token on a live session.
const LITE_07: &str = "moq-lite-07-wip";

/// Each renewed guest's token track, in the origin the studio's session in the
/// room publishes.
struct Renewals {
	room: Arc<Room>,
	origin: moq_net::origin::Producer,
	tracks: BTreeMap<String, Renewed>,
}

/// One guest's token track, encrypted to their key, ended when dropped.
struct Renewed {
	publication: moq_net::origin::Publication,
	track: Option<moq_e2ee::track::Producer>,
}

impl Drop for Renewed {
	fn drop(&mut self) {
		if let Some(track) = self.track.take() {
			let _ = track.finish();
		}
		self.publication.close();
	}
}

impl Renewals {
	fn new(room: Arc<Room>, origin: moq_net::origin::Producer) -> Self {
		Self {
			room,
			origin,
			tracks: BTreeMap::new(),
		}
	}

	/// Publish a token to each guest who is newly renewed, and end the tracks
	/// of those who are not anymore.
	fn sync(&mut self) {
		let renewed = self.room.renewed();
		self.tracks.retain(|identity, _| renewed.contains(identity));
		for identity in renewed {
			if self.tracks.contains_key(&identity) {
				continue;
			}
			match self.publish(&identity) {
				Ok(track) => {
					self.tracks.insert(identity, track);
				}
				Err(err) => tracing::error!(%identity, error = format!("{err:#}"), "cannot renew a guest's token"),
			}
		}
	}

	/// The token track for `identity`, holding a fresh token.
	fn publish(&self, identity: &str) -> anyhow::Result<Renewed> {
		let credential = self.room.credential(identity)?;
		let generation = credential.produce();
		let publication = self.origin.create_broadcast(Room::renewal_path(&credential)?)?;
		let protected = moq_e2ee::broadcast::Producer::new(publication.broadcast().clone(), generation.clone());
		let track = protected.create_track(TOKEN_TRACK, None)?;
		let mut renewed = Renewed {
			publication,
			track: Some(track),
		};
		self.write(identity, &mut renewed)?;
		let route = moq_net::origin::Route::default().with_epoch(generation.epoch().clone());
		renewed.publication.announce(route)?;
		Ok(renewed)
	}

	/// A fresh token for every renewed guest.
	fn renew(&mut self) {
		let mut tracks = std::mem::take(&mut self.tracks);
		for (identity, renewed) in &mut tracks {
			if let Err(err) = self.write(identity, renewed) {
				tracing::error!(%identity, error = format!("{err:#}"), "cannot renew a guest's token");
			}
		}
		self.tracks = tracks;
	}

	fn write(&self, identity: &str, renewed: &mut Renewed) -> anyhow::Result<()> {
		let (token, _) = self.room.sign(identity)?;
		let track = renewed.track.as_mut().expect("held until dropped");
		let mut group = track.append_group()?;
		group.write_frame(None, token.as_bytes())?;
		group.finish()?;
		Ok(())
	}
}

/// Add a guest's broadcast as the source `name`, muted: it is in no scene, so
/// the guest reaches the program only once the host places or unmutes them.
fn admit(draft: &mut Draft, name: &str, source: Source) -> Result<(), moq_scene::Error> {
	let muted = moq_scene::Audio {
		muted: true,
		..Default::default()
	};
	draft.set_source(name, Some(source))?;
	draft.set_audio(name, Some(muted))
}

#[cfg(test)]
mod tests {
	use super::*;

	const EXPIRY: Duration = Duration::from_secs(3600);

	fn key(root: &str, publish: &[&str], subscribe: &[&str]) -> moq_auth::Key {
		let patterns = |texts: &[&str]| texts.iter().map(|text| text.parse().unwrap()).collect();
		moq_auth::Key::generate(moq_auth::Algorithm::ES256, Some(moq_auth::KeyId::random()))
			.unwrap()
			.with_scope(moq_auth::Scope {
				root: root.into(),
				publish: patterns(publish),
				subscribe: patterns(subscribe),
			})
			.unwrap()
	}

	fn room(connect: &str, key: moq_auth::Key) -> anyhow::Result<Room> {
		let page = Url::parse("https://studio.example/join/").unwrap();
		Room::new(&Url::parse(connect).unwrap(), "guests", key, page, EXPIRY)
	}

	fn fragment(link: &Url) -> BTreeMap<String, String> {
		url::form_urlencoded::parse(link.fragment().unwrap().as_bytes())
			.into_owned()
			.collect()
	}

	#[test]
	fn the_guest_flags_come_together_and_need_a_relay() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("room.jwk");
		key("show/guests", &["**"], &["**"]).to_file(&path).unwrap();
		let scenes = dir.path().join("show.toml");
		let connect = Url::parse("https://relay.example/show").unwrap();
		let config = |room: bool, key: bool, page: bool| Config {
			room: room.then(|| "guests".into()),
			room_key: key.then(|| path.clone()),
			join_page: page.then(|| Url::parse("https://studio.example/join/").unwrap()),
			guest_expiry: EXPIRY.into(),
		};

		assert!(
			config(false, false, false)
				.load(Some(&connect), &scenes)
				.unwrap()
				.is_none()
		);
		for partial in [
			config(true, false, true),
			config(false, true, true),
			config(false, false, true),
		] {
			let err = partial.load(Some(&connect), &scenes).err().unwrap();
			assert!(err.to_string().contains("need both"), "{err}");
		}
		let err = config(true, true, true).load(None, &scenes).err().unwrap();
		assert!(err.to_string().contains("--connect"), "{err}");
		let room = config(true, true, true).load(Some(&connect), &scenes).unwrap().unwrap();
		assert_eq!(room.path().as_str(), "guests");
		let link = room.invite("alice").unwrap().link;
		assert!(link.as_str().starts_with("https://studio.example/join/#"), "{link}");

		let room = config(true, true, false)
			.load(Some(&connect), &scenes)
			.unwrap()
			.unwrap();
		let link = room.invite("alice").unwrap().link;
		assert!(link.as_str().starts_with(JOIN_PAGE), "{link}");
	}

	#[test]
	fn refuses_a_key_that_reaches_outside_the_room() {
		let unscoped = moq_auth::Key::generate(moq_auth::Algorithm::ES256, None).unwrap();
		let err = room("https://relay.example/show", unscoped).err().unwrap();
		assert!(err.to_string().contains("unscoped"), "{err}");

		// The whole relay, the show around the room, and a sibling room.
		for (root, pattern) in [("", "**"), ("show", "**"), ("show/other", "**"), ("", "show/**")] {
			let err = room("https://relay.example/show", key(root, &[pattern], &[pattern]))
				.err()
				.unwrap();
			assert!(err.to_string().contains("outside the room"), "{root} {pattern}: {err}");
		}
	}

	#[test]
	fn accepts_a_key_scoped_to_the_room_however_it_is_spelled() {
		room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		room(
			"https://relay.example/show",
			key("", &["show/guests/**"], &["show/guests/**"]),
		)
		.unwrap();
		room("https://relay.example/", key("guests", &["**"], &["**"])).unwrap();
	}

	#[test]
	fn refuses_a_key_too_narrow_to_sign_for_guests() {
		let err = room("https://relay.example/show", key("show/guests/alice", &["**"], &["**"]))
			.err()
			.unwrap();
		assert!(err.to_string().contains("cannot sign"), "{err:#}");
	}

	#[test]
	fn refuses_a_room_a_url_would_spell_differently() {
		for room in ["guest room", "gäste"] {
			let key = key(&format!("show/{room}"), &["**"], &["**"]);
			let page = Url::parse("https://studio.example/join/").unwrap();
			let connect = Url::parse("https://relay.example/show").unwrap();
			let err = Room::new(&connect, room, key, page, EXPIRY).err().unwrap();
			assert!(
				err.to_string().contains("changes when written in a URL"),
				"{room}: {err}"
			);
		}
	}

	#[test]
	fn refuses_a_relay_a_browser_cannot_dial() {
		let key = || key("show/guests", &["**"], &["**"]);
		for connect in [
			"moqt://relay.example/show",
			"http://relay.example/show",
			"http://8.8.8.8/show",
		] {
			let err = room(connect, key()).err().unwrap();
			assert!(err.to_string().contains("browser"), "{connect}: {err}");
		}
		for connect in [
			"http://localhost:4443/show",
			"http://127.0.0.1:4443/show",
			"http://[::1]:4443/show",
			"http://192.168.1.20:4443/show",
			"http://[fd00::20]:4443/show",
			"http://studio.local:4443/show",
		] {
			room(connect, key()).unwrap_or_else(|err| panic!("{connect}: {err}"));
		}
	}

	#[test]
	fn refuses_a_join_page_that_cannot_open_the_camera() {
		let connect = Url::parse("https://relay.example/show").unwrap();
		let key = || key("show/guests", &["**"], &["**"]);
		for page in [
			"http://join.example/",
			"http://192.168.1.20/",
			"file:///join/index.html",
		] {
			let err = Room::new(&connect, "guests", key(), Url::parse(page).unwrap(), EXPIRY)
				.err()
				.unwrap();
			assert!(err.to_string().contains("secure page"), "{page}: {err}");
		}
		for page in ["http://localhost:5173/", "http://127.0.0.1:5173/"] {
			Room::new(&connect, "guests", key(), Url::parse(page).unwrap(), EXPIRY)
				.unwrap_or_else(|err| panic!("{page}: {err}"));
		}
	}

	#[test]
	fn refuses_an_expiry_too_short_to_renew() {
		let connect = Url::parse("https://relay.example/show").unwrap();
		let page = Url::parse("https://studio.example/join/").unwrap();
		for expiry in [
			Duration::ZERO,
			Duration::from_millis(500),
			MIN_EXPIRY - Duration::from_millis(1),
		] {
			let key = key("show/guests", &["**"], &["**"]);
			let err = Room::new(&connect, "guests", key, page.clone(), expiry).err().unwrap();
			assert!(err.to_string().contains("--guest-expiry"), "{expiry:?}: {err}");
		}
	}

	#[test]
	fn a_link_carries_the_room_and_a_token_for_one_guest_in_its_fragment() {
		let key = key("show/guests", &["**"], &["**"]);
		let room = room(
			"https://user:pass@relay.example:4443/show?jwt=studio-secret",
			key.clone(),
		)
		.unwrap();
		let invite = room.invite(" alice ").unwrap();

		assert_eq!(invite.link.path(), "/join/");
		assert_eq!(
			invite.link.query(),
			None,
			"the token must stay out of what a server sees"
		);
		let fragment = fragment(&invite.link);
		assert_eq!(fragment["url"], "https://relay.example:4443/show/guests");
		assert_eq!(fragment["name"], "alice");

		let claims = key.verify(&fragment["jwt"]).unwrap();
		assert_eq!(claims.root, "show/guests");
		assert_eq!(claims.publish, ["alice/**".parse().unwrap()].into_iter().collect());
		let renewals = Room::renewal_path(&room.credential("alice").unwrap()).unwrap();
		assert_eq!(
			claims.subscribe,
			[moq_auth::Pattern::literal(renewals.as_str()).unwrap()]
				.into_iter()
				.collect(),
			"a guest reads only its own renewals"
		);
		assert_eq!(
			fragment["key"].len(),
			moq_e2ee::SECRET_LEN * 2,
			"the renewal key, in hex"
		);
		let (issued, expires) = (claims.issued.unwrap(), claims.expires.unwrap());
		let lifetime = expires.saturating_duration_since(issued);
		assert!(
			lifetime == EXPIRY || lifetime == EXPIRY + Duration::from_secs(1),
			"{lifetime:?}"
		);
		assert_eq!(invite.expires, expires);
		assert!(room.participants()["alice"].invite.is_some());
	}

	#[test]
	fn refuses_a_name_that_is_not_one_literal_segment() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		for name in ["", "  ", "a/b", r"a\/b", "*", "al*ce", ".alice"] {
			assert!(room.invite(name).is_err(), "`{name}` was accepted");
		}
		assert!(room.participants().is_empty());
	}

	#[test]
	fn a_guest_becomes_a_muted_source_once() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		let mut collection = Collection::new();
		let history = History::new();
		let path = |text: &str| Path::new(text).unwrap();

		// Anything else under a guest's own path is not a guest.
		for other in ["alice/1/camera.hang", "alice/camera", "alice/mic.hang"] {
			assert_eq!(room.observe(&path(other), true, &collection, &history), None, "{other}");
		}
		assert!(room.participants().is_empty());

		// Invited before this session, so the roster has no link for them.
		let (name, source) = room
			.observe(&path("alice/camera.hang"), true, &collection, &history)
			.unwrap();
		assert_eq!(name, "alice");
		assert_eq!(
			source,
			Source::Broadcast {
				path: "guests/alice/camera.hang".into()
			}
		);
		let mut draft = collection.draft("Add guest");
		admit(&mut draft, &name, source).unwrap();
		history
			.apply_outside(&mut collection, &draft.finish().unwrap())
			.unwrap();
		assert!(collection.audio["alice"].muted);
		assert_eq!(
			room.participants()["alice"],
			Participant {
				invite: None,
				camera: true,
				screen: false,
				removed: false,
			}
		);

		// The same broadcast again, after a restart or a reconnect, is the same source.
		assert_eq!(
			room.observe(&path("alice/camera.hang"), true, &collection, &history),
			None
		);

		// A source already named after the guest pushes the screen's name along.
		collection.sources.insert(
			"alice screen".into(),
			Source::Color {
				color: moq_scene::Rgba([0, 0, 0, 255]),
			},
		);
		let (name, _) = room
			.observe(&path("alice/screen.hang"), true, &collection, &history)
			.unwrap();
		assert_eq!(name, "alice screen 2");

		assert_eq!(
			room.observe(&path("alice/camera.hang"), false, &collection, &history),
			None
		);
		assert!(!room.participants()["alice"].camera);
		assert!(room.participants()["alice"].screen);
	}

	/// A guest who opens the link reaches the relay with its token, and the
	/// studio's own connection sees its camera in the room. The studio's session
	/// in the room then hands the guest a fresh token, which the relay accepts in
	/// band, and which no other guest may read.
	#[tokio::test(flavor = "multi_thread")]
	async fn a_guest_joins_and_renews_through_a_relay_that_verifies_tokens() {
		const TIMEOUT: Duration = Duration::from_secs(10);
		let _ = moq_tokio::crypto::install_default();

		// The operator's key for the studio's own connection, and the room's.
		let dir = tempfile::tempdir().unwrap();
		let admin = moq_auth::Key::generate(moq_auth::Algorithm::ES256, Some(moq_auth::KeyId::random())).unwrap();
		let scoped = key("show/guests", &["**"], &["**"]);
		for key in [&admin, &scoped] {
			let kid = key.kid.as_ref().unwrap().encode();
			key.to_public()
				.unwrap()
				.to_file(dir.path().join(format!("{kid}.jwk")))
				.unwrap();
		}
		let mut policy = moq_auth::serve::Policy::default();
		policy.keys = Some(moq_auth::serve::Keys::Dir(dir.path().into()));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let auth = format!("http://{}/", listener.local_addr().unwrap()).parse().unwrap();
		let server = moq_auth::serve::Server::new(policy).unwrap();
		tokio::spawn(async move { server.serve(listener).await });

		let mut config = moq_relay::Config::default();
		config.drain_timeout = Some(Duration::ZERO.into());
		config.listen.bind = Some("127.0.0.1:0".parse().unwrap());
		config.listen.tls.generate = vec!["localhost".into()];
		config.auth.url = Some(auth);
		// Every version, as a relay serving the studio and its join page opts into
		// the one that renews tokens in band.
		config.listen.version = moq_net::Versions::all()
			.iter()
			.copied()
			.chain([LITE_07.parse().unwrap()])
			.collect();
		let relay = moq_relay::Relay::load(config).await.unwrap().with_signals(false);
		let addr = relay.quic_addr().unwrap();
		let ready = relay.ready();
		tokio::spawn(relay.run());
		ready.wait().await.unwrap();

		let mut settings = moq_tokio::connect::Config::default();
		// The relay's certificate is generated for this test alone.
		settings.tls.insecure = Some(true);
		let client = settings.clone().init(Default::default()).unwrap();

		let studio = moq_auth::Claims::default()
			.with_root("show")
			.with_subscribe([moq_auth::Pattern::all()]);
		let mut connect = Url::parse(&format!("https://localhost:{}/show", addr.port())).unwrap();
		connect
			.query_pairs_mut()
			.append_pair("jwt", &admin.sign(&studio).unwrap());
		let subscribe = moq_tokio::origin::spawn();
		let all = moq_auth::Pattern::all().into();
		let mut announced = moq_room::Room::new(&subscribe.consume().scope("guests", &all).unwrap(), None);
		let _studio = client.with_subscriber(subscribe).connect(connect.clone());

		let page = Url::parse("https://studio.example/join/").unwrap();
		let room = Arc::new(Room::new(&connect, "guests", scoped.clone(), page, EXPIRY).unwrap());
		let renewal = Renewal::connect(room.clone(), &settings, Default::default()).unwrap();
		tokio::spawn(renewal.run());
		// Another guest in the room, whose renewals alice must not read.
		room.observe(
			&Path::new("bob/camera.hang").unwrap(),
			true,
			&Collection::new(),
			&History::new(),
		);

		// As the join page dials: its token kept out of the URL, on the version
		// that renews it in band.
		let fragment = fragment(&room.invite("alice").unwrap().link);
		let mut page = settings.clone();
		page.tokens = vec![fragment["jwt"].clone().try_into().unwrap()];
		page.version = vec![LITE_07.parse().unwrap()];
		let guest = moq_tokio::origin::spawn();
		let camera = guest.create_broadcast("alice/camera.hang").unwrap();
		camera.announce(moq_net::origin::Route::default()).unwrap();
		let renewals = moq_tokio::origin::spawn();
		let alice = page
			.init(Default::default())
			.unwrap()
			.with_publisher(&guest)
			.with_subscriber(renewals.clone())
			.connect(Url::parse(&fragment["url"]).unwrap());

		let event = kio::clock::timeout(TIMEOUT, announced.next())
			.await
			.expect("the guest's camera reaches the studio")
			.expect("an announcement");
		assert_eq!(event.path.as_str(), "alice/camera.hang");
		assert!(event.broadcast.is_some());
		let (name, source) = room
			.observe(&event.path, true, &Collection::new(), &History::new())
			.unwrap();
		assert_eq!(name, "alice");
		assert_eq!(
			source,
			Source::Broadcast {
				path: "guests/alice/camera.hang".into()
			}
		);

		let token = kio::clock::timeout(TIMEOUT, next_token(&renewals.consume(), &room, "alice"))
			.await
			.expect("the studio renews the guest")
			.expect("a token");
		alice.tokens().set(vec![token.try_into().unwrap()]);
		kio::clock::timeout(TIMEOUT, alice.tokens().answered())
			.await
			.expect("the relay answers the renewal")
			.expect("the relay accepts the renewed token in band");

		// Published before alice's, over the same session, so the relay holds it.
		let bob = room.credential("bob").unwrap();
		let other = kio::clock::timeout(TIMEOUT, async {
			let broadcast = renewals.consume().request(Room::renewal_path(&bob)?, None).await?;
			read_token(broadcast, &bob).await
		})
		.await
		.expect("the relay answers");
		assert!(other.is_err(), "alice read bob's token");

		// A link signed before renewals existed lets its holder subscribe to the
		// whole room, and so reach alice's renewals, but without alice's key
		// it reads nothing.
		let old = moq_room::claims("show/guests", "mallory")
			.unwrap()
			.with_expires(kio::clock::wall() + EXPIRY);
		let mut mallory = settings.clone();
		mallory.tokens = vec![scoped.sign(&old).unwrap().try_into().unwrap()];
		mallory.version = vec![LITE_07.parse().unwrap()];
		let stolen = moq_tokio::origin::spawn();
		let _mallory = mallory
			.init(Default::default())
			.unwrap()
			.with_subscriber(stolen.clone())
			.connect(Url::parse(&fragment["url"]).unwrap());
		let alices = Room::renewal_path(&room.credential("alice").unwrap()).unwrap();
		let broadcast = kio::clock::timeout(TIMEOUT, async {
			stolen.consume().follow(alices.clone(), None)?.available().await
		})
		.await
		.expect("the relay answers")
		.expect("a room-wide grant reaches alice's renewals");
		let guess = credential("alice", [0; moq_e2ee::SECRET_LEN]).unwrap();
		let read = kio::clock::timeout(TIMEOUT, read_token(broadcast, &guess)).await;
		assert!(!matches!(read, Ok(Ok(_))), "a room-wide grant read alice's token");
	}

	/// The newest token published for `identity` in `origin`, once there is one.
	async fn next_token(origin: &moq_net::origin::Consumer, room: &Room, identity: &str) -> anyhow::Result<String> {
		let credential = room.credential(identity)?;
		let broadcast = origin
			.follow(Room::renewal_path(&credential)?, None)?
			.available()
			.await?;
		read_token(broadcast, &credential).await
	}

	/// The newest token in `broadcast`, decrypted with `credential`.
	async fn read_token(
		broadcast: moq_net::broadcast::Consumer,
		credential: &moq_e2ee::Credential,
	) -> anyhow::Result<String> {
		let epoch = broadcast.info().epoch.clone().context("no epoch")?;
		let broadcast = moq_e2ee::broadcast::Consumer::new(broadcast, credential.consume(epoch));
		let mut track = broadcast.subscribe(TOKEN_TRACK, None)?.await?;
		let mut group = track.recv_group().await?.context("the track ended")?;
		let frame = group.read_frame().await?.context("the group is empty")?;
		Ok(String::from_utf8(frame.plaintext.to_vec())?)
	}

	/// The tokens a renewed guest holds, as they read them: fresh while they are
	/// in the room, each at least `--guest-expiry` long, and none once the host
	/// removes them, until a new link.
	#[tokio::test]
	async fn a_guest_in_the_room_is_renewed_until_removed() {
		let clock = kio::clock::Manual::new().install(&tokio::runtime::Handle::current());
		// Partway through a second, which the tokens round to.
		clock.advance(Duration::from_millis(750));
		let key = key("show/guests", &["**"], &["**"]);
		let room = Arc::new(room("https://relay.example/show", key.clone()).unwrap());
		let origin = moq_tokio::origin::spawn();
		let mut renewals = Renewals::new(room.clone(), origin.clone());
		let consumer = origin.consume();
		let path = |text: &str| Path::new(text).unwrap();
		let join = |online: bool| room.observe(&path("alice/camera.hang"), online, &Collection::new(), &History::new());

		// Invited, or sharing a screen without the page joined, is not in the room.
		room.invite("alice").unwrap();
		room.observe(&path("alice/screen.hang"), true, &Collection::new(), &History::new());
		renewals.sync();
		assert!(renewals.tracks.is_empty());

		join(true);
		renewals.sync();
		let first = key
			.verify(&next_token(&consumer, &room, "alice").await.unwrap())
			.unwrap();
		assert_eq!(first.root, "show/guests");
		assert_eq!(first.publish, ["alice/**".parse().unwrap()].into_iter().collect());
		// Each renewal comes half an expiry later, so a token must outlast that.
		let ahead = |claims: &moq_auth::Claims| claims.expires.unwrap().saturating_duration_since(kio::clock::wall());
		assert!(ahead(&first) >= EXPIRY, "{:?}", ahead(&first));

		clock.advance(EXPIRY / 2);
		assert!(ahead(&first) >= EXPIRY / 2, "renewed before the token expires");
		renewals.renew();
		let second = key
			.verify(&next_token(&consumer, &room, "alice").await.unwrap())
			.unwrap();
		assert!(ahead(&second) >= EXPIRY, "{:?}", ahead(&second));

		room.stop_renewing("alice").unwrap();
		assert!(room.participants()["alice"].removed);
		renewals.sync();
		assert!(renewals.tracks.is_empty(), "a removed guest's renewals end");

		room.invite("alice").unwrap();
		renewals.sync();
		next_token(&consumer, &room, "alice").await.unwrap();

		join(false);
		renewals.sync();
		assert!(renewals.tracks.is_empty(), "a guest who left is not renewed");
	}

	/// The studio publishes renewals where the join page looks for them: the
	/// same vector as `renew.test.ts`.
	#[test]
	fn renewals_name_the_same_broadcast_as_the_join_page() {
		let alice = credential("alice", [1; moq_e2ee::SECRET_LEN]).unwrap();
		assert_eq!(
			Room::renewal_path(&alice).unwrap().as_str(),
			".renew/8W8-sbdtrkZZF_byr3vEcQ"
		);
	}

	/// A removed guest stays unrenewed when the studio restarts, until a new
	/// link, and their renewal key is the same across the restart.
	#[test]
	fn a_removal_outlasts_a_restart() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("room.jwk");
		key("show/guests", &["**"], &["**"]).to_file(&path).unwrap();
		let scenes = dir.path().join("show.toml");
		let connect = Url::parse("https://relay.example/show").unwrap();
		let config = Config {
			room: Some("guests".into()),
			room_key: Some(path),
			join_page: None,
			guest_expiry: EXPIRY.into(),
		};
		let start = || config.load(Some(&connect), &scenes).unwrap().unwrap();
		let camera = Path::new("alice/camera.hang").unwrap();

		let room = start();
		let key = fragment(&room.invite("alice").unwrap().link)["key"].clone();
		room.stop_renewing("alice").unwrap();
		assert!(dir.path().join("show.guests.toml").exists());

		let room = start();
		room.observe(&camera, true, &Collection::new(), &History::new());
		assert!(room.participants()["alice"].removed);
		assert!(room.renewed().is_empty(), "a removed guest is renewed after a restart");
		assert_eq!(fragment(&room.invite("alice").unwrap().link)["key"], key);
		assert_eq!(room.renewed(), ["alice"]);

		let room = start();
		room.observe(&camera, true, &Collection::new(), &History::new());
		assert_eq!(room.renewed(), ["alice"], "a new link is remembered too");

		// A removal the file cannot hold is not made, so trying again saves it.
		let file = dir.path().join("show.guests.toml");
		std::fs::remove_file(&file).unwrap();
		std::fs::create_dir(&file).unwrap();
		assert!(room.stop_renewing("alice").is_err());
		assert_eq!(room.renewed(), ["alice"]);
		std::fs::remove_dir(&file).unwrap();
		room.stop_renewing("alice").unwrap();
		assert!(start().participants()["alice"].removed);
	}

	/// A guest joining is not the host's edit: what the host undid can still
	/// be redone, and undo never takes the guest out. A guest whose source the
	/// host removed comes back under a name of its own, so undoing the removal
	/// still applies.
	#[test]
	fn a_guest_joining_keeps_what_the_host_can_undo_and_redo() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		let path = |text: &str| Path::new(text).unwrap();
		let mut collection = Collection::new();
		let mut history = History::new();
		let join = |collection: &mut Collection, history: &History, guest: &str| {
			let (name, source) = room
				.observe(&path(&format!("{guest}/camera.hang")), true, collection, history)
				.unwrap();
			let mut draft = collection.draft("Add guest");
			admit(&mut draft, &name, source).unwrap();
			history.apply_outside(collection, &draft.finish().unwrap()).unwrap();
			name
		};
		let commit = |collection: &mut Collection, history: &mut History, draft: Draft| {
			let edit = draft.finish().unwrap();
			collection.apply(&edit).unwrap();
			history.record(edit, None);
		};

		assert_eq!(join(&mut collection, &history, "bob"), "bob");
		let mut draft = collection.draft("Remove bob");
		draft.remove_source("bob").unwrap();
		commit(&mut collection, &mut history, draft);
		let mut draft = collection.draft("Add a color");
		let black = Source::Color {
			color: moq_scene::Rgba([0, 0, 0, 255]),
		};
		draft.set_source("black", Some(black)).unwrap();
		commit(&mut collection, &mut history, draft);
		history.undo(&mut collection).unwrap();

		assert_eq!(join(&mut collection, &history, "alice"), "alice");
		assert_eq!(join(&mut collection, &history, "bob"), "bob 2");
		assert_eq!(history.entries().len(), 2);
		history.redo(&mut collection).unwrap();
		assert!(collection.sources.contains_key("black"));

		history.seek(&mut collection, 0).unwrap();
		for name in ["alice", "bob", "bob 2"] {
			assert!(collection.sources.contains_key(name), "{name}");
		}
	}

	/// Remove from scenes takes the guest off the program however they reach
	/// it, leaves everything else, and is one edit that undoes whole.
	#[test]
	fn taking_a_guest_off_leaves_nothing_of_them_on_air() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		let mut collection = Collection::new();
		let fill = moq_scene::Rect::fill(&collection.canvas);
		let mask = Filter::Mask {
			source: "alice".into(),
			mode: Default::default(),
		};
		let gray = Filter::Grayscale { amount: 1.0 };
		collection.sources.insert(
			"alice".into(),
			Source::Broadcast {
				path: "guests/alice/camera.hang".into(),
			},
		);
		collection
			.sources
			.insert("alice clone".into(), Source::Clone { source: "alice".into() });
		collection.sources.insert(
			"bg".into(),
			Source::Color {
				color: moq_scene::Rgba([0, 0, 0, 255]),
			},
		);
		let mut bg = Item::new("bg", fill);
		bg.filters = vec![mask.clone(), gray.clone()];
		collection.scenes[0].items = vec![bg, Item::new("alice", fill), Item::new("alice clone", fill)];
		collection.top.push(Item::new("alice", fill));
		let video = Video {
			filters: vec![mask],
			..Default::default()
		};
		collection.video.insert("bg".into(), video);
		collection.validate().unwrap();
		let start = collection.clone();

		let mut draft = collection.draft("Remove from scenes");
		assert!(room.take_off(&mut draft, "alice").unwrap());
		let edit = draft.finish().unwrap();
		collection.apply(&edit).unwrap();
		let bg = &collection.scenes[0].items;
		assert_eq!(bg.len(), 1);
		assert_eq!(
			(bg[0].source.as_str(), bg[0].filters.as_slice()),
			("bg", [gray].as_slice())
		);
		assert!(collection.top.is_empty());
		assert!(!collection.video.contains_key("bg"));
		assert!(collection.audio["alice"].muted);
		assert!(collection.sources.contains_key("alice clone"), "a clone stays a source");

		let mut again = collection.draft("Remove from scenes");
		assert!(
			!room.take_off(&mut again, "alice").unwrap(),
			"nothing is left to take off"
		);
		collection.apply(&edit.inverse()).unwrap();
		assert_eq!(collection, start);
	}

	#[test]
	fn a_guest_already_in_the_collection_is_tracked_without_being_added() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		let mut collection = Collection::new();
		collection.sources.insert(
			"bob".into(),
			Source::Broadcast {
				path: "guests/bob/camera.hang".into(),
			},
		);
		let path = Path::new("bob/camera.hang").unwrap();
		assert_eq!(room.observe(&path, true, &collection, &History::new()), None);
		assert!(room.participants()["bob"].camera);
		assert_eq!(room.sources_of(&collection, "bob"), ["bob"]);
		assert!(room.sources_of(&collection, "alice").is_empty());
	}
}
