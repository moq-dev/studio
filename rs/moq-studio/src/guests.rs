//! Guests: people who join the studio's room from a web page, whose camera and
//! screen become broadcast sources.
//!
//! The room is a moq-room prefix on the studio's relay. Inviting a guest signs
//! a short-lived token with [`moq_room::claims`], so the guest publishes only
//! under its own `<identity>/`, and hands it to the join page in a link's
//! fragment, which no server logs. The guest publishes moq-room's
//! `<identity>/camera.hang` and `<identity>/screen.hang`, and the studio adds
//! each as a [`Source::Broadcast`] when it is announced.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use moq_net::Path;
use moq_room::Kind;
use moq_scene::{Collection, Source};
use url::Url;

use crate::engine::Engine;

/// The guest flags.
#[derive(usage_rs::Args, Clone, Debug)]
#[usage(unknown_flags = "error", args_override_self = false)]
pub struct Config {
	/// The room guests join: a path under the `--connect` URL's, which the
	/// studio's own connection must be allowed to subscribe to. Needs
	/// `--room-key` and `--join-page`.
	#[usage(long)]
	pub room: Option<String>,

	/// The key that signs guest tokens, as `moq auth generate` writes it. Its
	/// scope must stay inside the room, so a stolen key cannot sign for the rest
	/// of the relay; an unscoped key is refused.
	#[usage(long, value_hint = usage_rs::ValueHint::FilePath, extensions("jwk", "json"))]
	pub room_key: Option<PathBuf>,

	/// The join page a guest opens. Its link carries the room's relay URL, the
	/// guest's name, and its token in the fragment.
	#[usage(long)]
	pub join_page: Option<Url>,

	/// How long a join link works. The relay disconnects a guest when its link
	/// expires, so send a new one before then for a longer show.
	#[usage(long, default = "4h")]
	pub guest_expiry: moq_tokio::cli::Duration,
}

impl Config {
	/// The room these flags describe on the relay at `connect`, or `None` without
	/// `--room`.
	pub fn load(&self, connect: Option<&Url>) -> anyhow::Result<Option<Room>> {
		let (room, key, page) = match (&self.room, &self.room_key, &self.join_page) {
			(None, None, None) => return Ok(None),
			(Some(room), Some(key), Some(page)) => (room, key, page),
			_ => anyhow::bail!("guests need all of --room, --room-key, and --join-page"),
		};
		let connect = connect.context("guests join through the relay; add --connect")?;
		let key = moq_auth::Key::from_file(key).with_context(|| format!("reading the room key {}", key.display()))?;
		let room = Room::new(connect, room, key, page.clone(), self.guest_expiry.into())?;
		Ok(Some(room))
	}
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
	page: Url,
	expiry: Duration,
	participants: Mutex<BTreeMap<String, Participant>>,
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
	/// guest's claims.
	pub fn new(connect: &Url, room: &str, key: moq_auth::Key, page: Url, expiry: Duration) -> anyhow::Result<Self> {
		anyhow::ensure!(
			matches!(connect.scheme(), "https" | "http"),
			"guests join from a browser, which cannot dial {}://; connect over https://",
			connect.scheme()
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

		check_scope(&key, &root)?;
		let room = Self {
			path,
			root,
			url,
			key,
			page,
			expiry,
			participants: Mutex::default(),
		};
		// Sign once now, so a key that cannot sign for guests fails at startup.
		room.sign("guest")?;
		Ok(room)
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
		let (token, expires) = self.sign(name)?;
		let mut link = self.page.clone();
		let fragment = url::form_urlencoded::Serializer::new(String::new())
			.append_pair("url", self.url.as_str())
			.append_pair("name", name)
			.append_pair("jwt", &token)
			.finish();
		link.set_fragment(Some(&fragment));
		let invite = Invite { link, expires };
		self.participants
			.lock()
			.unwrap()
			.entry(name.to_string())
			.or_default()
			.invite = Some(invite.clone());
		Ok(invite)
	}

	/// A token for `identity`, and when it expires.
	fn sign(&self, identity: &str) -> anyhow::Result<(String, kio::clock::Wall)> {
		// Whole seconds, as the token carries them.
		let now = kio::clock::wall().saturating_duration_since(kio::clock::Wall::UNIX_EPOCH);
		let issued = kio::clock::Wall::UNIX_EPOCH + Duration::from_secs(now.as_secs());
		let expires = issued + self.expiry;
		let claims = moq_room::claims(self.root.as_str(), identity)
			.with_context(|| format!("cannot invite `{identity}`"))?
			.with_issued(issued)
			.with_expires(expires);
		let token = self.key.sign(&claims).context("the room key cannot sign a guest's token")?;
		Ok((token, expires))
	}

	/// Record that the broadcast at `path` in the room went online or offline,
	/// returning the source to add for it, if the collection lacks one.
	///
	/// Only guests the host invited this session, or whose broadcast is already
	/// a source, are tracked: anyone else with a token for the room cannot make
	/// the studio hold anything.
	fn observe(&self, path: &Path, online: bool, collection: &Collection) -> Option<(String, Source)> {
		let parsed = moq_room::parse(path)?;
		let source = self.path.join(path).ok()?.to_string();
		let existing = collection
			.sources
			.values()
			.any(|existing| matches!(existing, Source::Broadcast { path } if *path == source));

		let identity = parsed.identity.to_string();
		let mut participants = self.participants.lock().unwrap();
		let participant = match participants.get_mut(&identity) {
			Some(participant) => participant,
			None if existing => participants.entry(identity.clone()).or_default(),
			None => {
				tracing::debug!(%path, "ignoring a broadcast from someone the studio did not invite");
				return None;
			}
		};
		match parsed.kind {
			Kind::Camera => participant.camera = online,
			Kind::Screen => participant.screen = online,
		}
		if !online || existing {
			return None;
		}

		let base = match parsed.kind {
			Kind::Camera => identity,
			Kind::Screen => format!("{identity} screen"),
		};
		let name = std::iter::once(base.clone())
			.chain((2..).map(|n| format!("{base} {n}")))
			.find(|name| !collection.sources.contains_key(name))
			.expect("an unused name");
		Some((name, Source::Broadcast { path: source }))
	}

	/// The sources in `collection` that show the guest called `identity`.
	pub fn sources_of(&self, collection: &Collection, identity: &str) -> Vec<String> {
		collection
			.sources
			.iter()
			.filter(|(_, source)| {
				let Source::Broadcast { path } = source else { return false };
				let Ok(path) = Path::new(path) else { return false };
				path.strip_prefix(&self.path)
					.and_then(moq_room::parse)
					.is_some_and(|parsed| parsed.identity.as_str() == identity)
			})
			.map(|(name, _)| name.clone())
			.collect()
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
	let confined = |patterns: &moq_auth::Patterns| {
		patterns
			.rooted(&scope.root)
			.is_ok_and(|rooted| inside.covers(&rooted))
	};
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
		if let Some((name, source)) = room.observe(&event.path, event.broadcast.is_some(), &state.collection) {
			tracing::info!(source = %name, identity = %event.identity, "a guest joined");
			state.edit().sources.insert(name, source);
		}
	}
	tracing::warn!("the room's announcements ended");
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
		let connect = Url::parse("https://relay.example/show").unwrap();
		let config = |room: bool, key: bool, page: bool| Config {
			room: room.then(|| "guests".into()),
			room_key: key.then(|| path.clone()),
			join_page: page.then(|| Url::parse("https://studio.example/join/").unwrap()),
			guest_expiry: EXPIRY.into(),
		};

		assert!(config(false, false, false).load(Some(&connect)).unwrap().is_none());
		for partial in [config(true, false, false), config(true, true, false), config(false, true, true)] {
			let err = partial.load(Some(&connect)).err().unwrap();
			assert!(err.to_string().contains("need all of"), "{err}");
		}
		let err = config(true, true, true).load(None).err().unwrap();
		assert!(err.to_string().contains("--connect"), "{err}");
		let room = config(true, true, true).load(Some(&connect)).unwrap().unwrap();
		assert_eq!(room.path().as_str(), "guests");
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
		room("https://relay.example/show", key("", &["show/guests/**"], &["show/guests/**"])).unwrap();
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
	fn refuses_a_relay_a_browser_cannot_dial() {
		let err = room("moqt://relay.example/show", key("show/guests", &["**"], &["**"]))
			.err()
			.unwrap();
		assert!(err.to_string().contains("browser"), "{err}");
	}

	#[test]
	fn a_link_carries_the_room_and_a_token_for_one_guest_in_its_fragment() {
		let key = key("show/guests", &["**"], &["**"]);
		let room = room("https://user:pass@relay.example:4443/show?jwt=studio-secret", key.clone()).unwrap();
		let invite = room.invite(" alice ").unwrap();

		assert_eq!(invite.link.path(), "/join/");
		assert_eq!(invite.link.query(), None, "the token must stay out of what a server sees");
		let fragment = fragment(&invite.link);
		assert_eq!(fragment["url"], "https://relay.example:4443/show/guests");
		assert_eq!(fragment["name"], "alice");

		let claims = key.verify(&fragment["jwt"]).unwrap();
		assert_eq!(claims.root, "show/guests");
		assert_eq!(claims.publish, ["alice/**".parse().unwrap()].into_iter().collect());
		assert_eq!(claims.subscribe, [moq_auth::Pattern::all()].into_iter().collect());
		let (issued, expires) = (claims.issued.unwrap(), claims.expires.unwrap());
		assert_eq!(expires.saturating_duration_since(issued), EXPIRY);
		assert_eq!(invite.expires, expires);
		assert!(room.participants()["alice"].invite.is_some());
	}

	#[test]
	fn refuses_a_name_that_is_not_one_literal_segment() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		for name in ["", "  ", "a/b", r"a\/b", "*", "al*ce"] {
			assert!(room.invite(name).is_err(), "`{name}` was accepted");
		}
		assert!(room.participants().is_empty());
	}

	#[test]
	fn an_invited_guest_becomes_a_source_once_and_a_stranger_never_does() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		room.invite("alice").unwrap();
		let mut collection = Collection::new();
		let path = |text: &str| Path::new(text).unwrap();

		assert_eq!(room.observe(&path("mallory/camera.hang"), true, &collection), None);
		assert!(!room.participants().contains_key("mallory"));

		let (name, source) = room.observe(&path("alice/camera.hang"), true, &collection).unwrap();
		assert_eq!(name, "alice");
		assert_eq!(
			source,
			Source::Broadcast {
				path: "guests/alice/camera.hang".into()
			}
		);
		collection.sources.insert(name, source);
		assert!(room.participants()["alice"].camera);

		// The same broadcast again, after a restart or a reconnect, is the same source.
		assert_eq!(room.observe(&path("alice/camera.hang"), true, &collection), None);

		// A source already named after the guest pushes the screen's name along.
		collection.sources.insert(
			"alice screen".into(),
			Source::Color {
				color: moq_scene::Rgba([0, 0, 0, 255]),
			},
		);
		let (name, _) = room.observe(&path("alice/screen.hang"), true, &collection).unwrap();
		assert_eq!(name, "alice screen 2");

		assert_eq!(room.observe(&path("alice/camera.hang"), false, &collection), None);
		assert!(!room.participants()["alice"].camera);
		assert!(room.participants()["alice"].screen);
	}

	/// A guest who opens the link reaches the relay with its token, and the
	/// studio's own connection sees its camera in the room.
	#[tokio::test(flavor = "multi_thread")]
	async fn a_guest_joins_through_a_relay_that_verifies_tokens() {
		const TIMEOUT: Duration = Duration::from_secs(10);
		let _ = moq_tokio::crypto::install_default();

		// The operator's key for the studio's own connection, and the room's.
		let dir = tempfile::tempdir().unwrap();
		let admin = moq_auth::Key::generate(moq_auth::Algorithm::ES256, Some(moq_auth::KeyId::random())).unwrap();
		let scoped = key("show/guests", &["**"], &["**"]);
		for key in [&admin, &scoped] {
			let kid = key.kid.as_ref().unwrap().encode();
			key.to_public().unwrap().to_file(dir.path().join(format!("{kid}.jwk"))).unwrap();
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
		let relay = moq_relay::Relay::load(config).await.unwrap().with_signals(false);
		let addr = relay.quic_addr().unwrap();
		let ready = relay.ready();
		tokio::spawn(relay.run());
		ready.wait().await.unwrap();

		let mut client = moq_tokio::connect::Config::default();
		// The relay's certificate is generated for this test alone.
		client.tls.insecure = Some(true);
		let client = client.init(Default::default()).unwrap();

		let studio = moq_auth::Claims::default()
			.with_root("show")
			.with_subscribe([moq_auth::Pattern::all()]);
		let mut connect = Url::parse(&format!("https://localhost:{}/show", addr.port())).unwrap();
		connect.query_pairs_mut().append_pair("jwt", &admin.sign(&studio).unwrap());
		let subscribe = moq_tokio::origin::spawn();
		let all = moq_auth::Pattern::all().into();
		let mut announced = moq_room::Room::new(&subscribe.consume().scope("guests", &all).unwrap(), None);
		let _studio = client.clone().with_subscriber(subscribe).connect(connect.clone());

		let page = Url::parse("https://studio.example/join/").unwrap();
		let room = Room::new(&connect, "guests", scoped, page, EXPIRY).unwrap();
		let fragment = fragment(&room.invite("alice").unwrap().link);
		let mut dial = Url::parse(&fragment["url"]).unwrap();
		dial.query_pairs_mut().append_pair("jwt", &fragment["jwt"]);
		let guest = moq_tokio::origin::spawn();
		let camera = guest.create_broadcast("alice/camera.hang").unwrap();
		camera.announce(moq_net::origin::Route::default()).unwrap();
		let _guest = client.with_publisher(&guest).connect(dial);

		let event = kio::clock::timeout(TIMEOUT, announced.next())
			.await
			.expect("the guest's camera reaches the studio")
			.expect("an announcement");
		assert_eq!(event.path.as_str(), "alice/camera.hang");
		assert!(event.broadcast.is_some());
		let (name, source) = room.observe(&event.path, true, &Collection::new()).unwrap();
		assert_eq!(name, "alice");
		assert_eq!(
			source,
			Source::Broadcast {
				path: "guests/alice/camera.hang".into()
			}
		);
	}

	#[test]
	fn a_guest_already_in_the_collection_is_tracked_without_an_invite() {
		let room = room("https://relay.example/show", key("show/guests", &["**"], &["**"])).unwrap();
		let mut collection = Collection::new();
		collection.sources.insert(
			"bob".into(),
			Source::Broadcast {
				path: "guests/bob/camera.hang".into(),
			},
		);
		let path = Path::new("bob/camera.hang").unwrap();
		assert_eq!(room.observe(&path, true, &collection), None);
		assert!(room.participants()["bob"].camera);
		assert_eq!(room.sources_of(&collection, "bob"), ["bob"]);
		assert!(room.sources_of(&collection, "alice").is_empty());
	}
}
