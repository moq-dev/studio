//! Where a show streams: its relay, a failover relay, the broadcast's name, and
//! the options, kept per show in `<show>.stream.toml` beside its scene file
//! rather than in it, since the scene file is meant to be shared and names no
//! relay. A relay's token goes in the operating system's credential store,
//! never in a file. Each command-line flag wins over its setting for the run it
//! is given to, and is never written back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use url::Url;

use crate::engine::{Destination, Relay};
use crate::guests;

/// The broadcast path a show without one goes live at.
pub const BROADCAST: &str = "studio";

/// A token to keep for a relay, or `None` to forget its saved one.
pub type Token = (Url, Option<String>);

/// What `<show>.stream.toml` holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Saved {
	/// The relay the program goes live on.
	pub relay: Option<Url>,
	/// A second relay to publish the same broadcast to, so a viewer linked to
	/// both resumes from it when the first fails.
	pub failover: Option<Url>,
	/// The broadcast path on the relays, [`BROADCAST`] when unset.
	pub broadcast: Option<String>,
	/// Whether the program video's canvas size encodes only while someone
	/// watches it.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub on_demand: bool,
	/// Where recordings are saved, the scene file's directory when unset.
	pub recordings: Option<PathBuf>,
	/// The room guests join, a path beneath the relay's.
	pub room: Option<String>,
	/// The key that signs guest tokens, a file `moq auth generate` writes.
	pub room_key: Option<PathBuf>,
	/// The join page a guest opens.
	pub join_page: Option<Url>,
	/// How long a join link works.
	#[serde(with = "humantime_serde")]
	pub guest_expiry: Option<Duration>,
	/// Whether a guest who joins takes the next tile of the scene on
	/// program, rather than waiting under Add.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub guests_appear: bool,
}

impl Saved {
	/// Where these settings save recordings for the show at `scene`, with no
	/// flag over them.
	pub fn folder(&self, scene: &Path) -> PathBuf {
		match &self.recordings {
			Some(dir) => dir.clone(),
			None => scene
				.parent()
				.filter(|dir| !dir.as_os_str().is_empty())
				.unwrap_or(Path::new("."))
				.to_path_buf(),
		}
	}
}

/// What the command line sets for one run, over the show's saved settings.
#[derive(Clone, Debug, Default)]
pub struct Flags {
	pub relay: Option<Url>,
	/// Credentials for the relay, over the one in the credential store.
	pub tokens: Vec<moq_net::auth::Token>,
	pub failover: Option<Url>,
	pub broadcast: Option<String>,
	/// Set only to turn it on; off leaves the show's setting.
	pub on_demand: bool,
	pub recordings: Option<PathBuf>,
	pub guests: guests::Config,
}

/// Which settings a flag sets for this run, so the window shows them as the
/// flags' and does not offer to change them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overridden {
	pub relay: bool,
	pub token: bool,
	pub failover: bool,
	pub broadcast: bool,
	pub on_demand: bool,
	pub recordings: bool,
	pub room: bool,
	pub room_key: bool,
	pub join_page: bool,
	pub guest_expiry: bool,
}

/// Which of a stream's relays to dial: both to go live, and each on its own to
/// follow them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switched {
	pub relay: bool,
	pub failover: bool,
}

impl Default for Switched {
	fn default() -> Self {
		Self {
			relay: true,
			failover: true,
		}
	}
}

/// A show's stream settings, as saved and as this run's flags override them.
#[derive(Clone)]
pub struct Stream {
	/// `<show>.stream.toml`.
	path: PathBuf,
	saved: Saved,
	flags: Flags,
	/// Every other client flag a relay is dialed with.
	client: moq_tokio::connect::Config,
	quic: moq_tokio::quic::Config,
	credentials: Credentials,
	/// Each relay's client, shared by every clone, so watching, measuring,
	/// and going live dial from one socket per relay, which a fixed
	/// `--connect-bind` port can be bound for only once.
	clients: Arc<Mutex<Vec<Cached>>>,
}

/// A relay's client, by the address and tokens it was set up with.
struct Cached {
	url: Url,
	tokens: Vec<moq_net::auth::Token>,
	client: moq_tokio::Client,
}

impl Stream {
	/// The settings of the show at `scene`, as its stream file holds them, or
	/// none yet, under `flags`. Each relay is dialed as `client` and `quic`
	/// configure, with its token from `credentials`.
	pub fn load(
		scene: &Path,
		flags: Flags,
		client: moq_tokio::connect::Config,
		quic: moq_tokio::quic::Config,
		credentials: Credentials,
	) -> anyhow::Result<Self> {
		let path = path(scene);
		let saved = match std::fs::read_to_string(&path) {
			Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => Saved::default(),
			Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
		};
		Ok(Self {
			path,
			saved,
			flags,
			client,
			quic,
			credentials,
			clients: Default::default(),
		})
	}

	/// The stream file.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// What the stream file holds.
	pub fn saved(&self) -> &Saved {
		&self.saved
	}

	/// Which settings this run's flags set.
	pub fn overridden(&self) -> Overridden {
		let flags = &self.flags;
		Overridden {
			relay: flags.relay.is_some(),
			token: !flags.tokens.is_empty(),
			failover: flags.failover.is_some(),
			broadcast: flags.broadcast.is_some(),
			on_demand: flags.on_demand,
			recordings: flags.recordings.is_some(),
			room: flags.guests.room.is_some(),
			room_key: flags.guests.room_key.is_some(),
			join_page: flags.guests.join_page.is_some(),
			guest_expiry: flags.guests.guest_expiry.is_some(),
		}
	}

	/// Keep `tokens` and write `saved` to the stream file, all at once, so a
	/// crash leaves the old file or the new one. A token in either relay's
	/// address moves to the credential store first, so the file never holds
	/// one. A failure puts back every token it replaced, so a save never
	/// half lands.
	///
	/// Blocks on the credential store: call it off the window's thread, and
	/// outside an async task.
	pub fn save(&mut self, mut saved: Saved, tokens: &[Token]) -> anyhow::Result<()> {
		let mut tokens = tokens.to_vec();
		for url in [&mut saved.relay, &mut saved.failover].into_iter().flatten() {
			if let Some(token) = take_token(url) {
				tokens.push((url.clone(), Some(token)));
			}
		}
		let mut kept = Vec::new();
		let result = (|| {
			for (url, token) in &tokens {
				kept.push((url, self.credentials.get(url)?));
				self.credentials.set(url, token.as_deref())?;
			}
			self.write(&saved)
		})();
		if let Err(err) = result {
			// Every one is tried, so one the store refuses leaves the rest put back.
			let lost: Vec<String> = kept
				.iter()
				.rev()
				.filter_map(|(url, token)| {
					let restore = self.credentials.set(url, token.as_deref()).err()?;
					Some(format!("{url} ({restore:#})"))
				})
				.collect();
			return Err(match lost.is_empty() {
				true => err,
				false => err.context(format!(
					"and the tokens kept before could not be put back for {}",
					lost.join(", ")
				)),
			});
		}
		self.saved = saved;
		Ok(())
	}

	/// Write `saved` to the stream file, all at once.
	fn write(&self, saved: &Saved) -> anyhow::Result<()> {
		let text = toml::to_string(saved).context("cannot write the stream settings")?;
		let dir = self
			.path
			.parent()
			.filter(|dir| !dir.as_os_str().is_empty())
			.unwrap_or(Path::new("."));
		let mut file =
			tempfile::NamedTempFile::new_in(dir).with_context(|| format!("writing {}", self.path.display()))?;
		std::io::Write::write_all(&mut file, text.as_bytes())
			.with_context(|| format!("writing {}", self.path.display()))?;
		file.persist(&self.path)
			.with_context(|| format!("writing {}", self.path.display()))?;
		Ok(())
	}

	/// The relay the program goes live on.
	pub fn relay(&self) -> Option<&Url> {
		self.flags.relay.as_ref().or(self.saved.relay.as_ref())
	}

	/// The failover relay.
	pub fn failover(&self) -> Option<&Url> {
		self.flags.failover.as_ref().or(self.saved.failover.as_ref())
	}

	/// The broadcast path on the relays.
	pub fn broadcast(&self) -> &str {
		let set = self.flags.broadcast.as_deref().or(self.saved.broadcast.as_deref());
		set.unwrap_or(BROADCAST)
	}

	/// Whether the canvas size encodes only while someone watches.
	pub fn on_demand(&self) -> bool {
		self.flags.on_demand || self.saved.on_demand
	}

	/// Where recordings are saved for the show at `scene`.
	pub fn recordings(&self, scene: &Path) -> PathBuf {
		match &self.flags.recordings {
			Some(dir) => dir.clone(),
			None => self.saved.folder(scene),
		}
	}

	/// Whether guests appear in the scene on program on their own.
	pub fn guests_appear(&self) -> bool {
		self.saved.guests_appear
	}

	/// The guests' room, as the flags and the show set it.
	pub fn guests(&self) -> guests::Config {
		self.guests_with(&self.saved)
	}

	/// Refuse `saved` unless guests could join the room it sets, under the
	/// flags, on its relay: with both a room and its key, a key that reads,
	/// and a relay and join page a browser can reach.
	pub fn check_guests(&self, saved: &Saved) -> anyhow::Result<()> {
		let relay = self.flags.relay.as_ref().or(saved.relay.as_ref());
		self.guests_with(saved).load(relay)?;
		Ok(())
	}

	/// The guests' room, as the flags and `saved` set it.
	fn guests_with(&self, saved: &Saved) -> guests::Config {
		let flags = &self.flags.guests;
		guests::Config {
			room: flags.room.clone().or(saved.room.clone()),
			room_key: flags.room_key.clone().or(saved.room_key.clone()),
			join_page: flags.join_page.clone().or(saved.join_page.clone()),
			guest_expiry: flags.guest_expiry.or(saved.guest_expiry.map(Into::into)),
		}
	}

	/// The client flags with the relay's address, for the guests' room, which
	/// takes none of its credentials.
	pub fn client(&self) -> moq_tokio::connect::Config {
		let mut client = self.client.clone();
		client.url = self.relay().cloned();
		client
	}

	/// The QUIC tuning every relay is dialed with.
	pub fn quic(&self) -> &moq_tokio::quic::Config {
		&self.quic
	}

	/// Each relay `switched` on, the preferred first, with its token, as a
	/// client ready to dial it: the one set up before for the same address
	/// and tokens, if any.
	///
	/// Blocks on the credential store: call it off the window's thread, and
	/// outside an async task. Call it inside the tokio runtime, where each
	/// client binds its socket.
	pub fn relays(&self, switched: Switched) -> anyhow::Result<Vec<Relay>> {
		// Each relay's client binds its own socket, so a fixed port would be
		// bound twice, by the one watched and the one gone live to.
		anyhow::ensure!(
			self.client.bind.is_none_or(|bind| bind.port() == 0) || self.relay().is_none() || self.failover().is_none(),
			"a failover relay cannot share a fixed --connect-bind port with the relay; leave the port 0, or remove the failover"
		);
		let dials = self.dials(switched)?;
		let mut clients = self.clients.lock().unwrap();
		// A relay no longer set, or set with other tokens, lets go of its
		// socket before another binds one.
		let current = [self.relay(), self.failover()];
		clients.retain(|cached| current.contains(&Some(&cached.url)));
		dials
			.into_iter()
			.map(|(url, tokens)| {
				if let Some(cached) = clients
					.iter()
					.find(|cached| cached.url == url && cached.tokens == tokens)
				{
					return Ok(Relay {
						client: cached.client.clone(),
						url,
					});
				}
				clients.retain(|cached| cached.url != url);
				let mut config = self.client.clone();
				config.url = Some(url.clone());
				config.tokens = tokens.clone();
				let client = config
					.init(self.quic.clone())
					.with_context(|| format!("cannot set up the relay {}", crate::engine::air::endpoint(&url)))?;
				clients.push(Cached {
					url: url.clone(),
					tokens,
					client: client.clone(),
				});
				Ok(Relay { client, url })
			})
			.collect()
	}

	/// Each relay `switched` on, the preferred first, with the tokens it is
	/// dialed with: the flags' for the relay they name, none for an address
	/// that carries its own `?jwt=`, or its own from the credential store, so
	/// one relay's never reach another.
	fn dials(&self, switched: Switched) -> anyhow::Result<Vec<(Url, Vec<moq_net::auth::Token>)>> {
		let picked = [
			(switched.relay, self.relay(), !self.flags.tokens.is_empty()),
			(switched.failover, self.failover(), false),
		];
		let picked: Vec<(&Url, bool)> = picked
			.into_iter()
			.filter_map(|(on, url, flagged)| Some((on.then_some(url?)?, flagged)))
			.collect();
		picked
			.into_iter()
			.map(|(url, flagged)| {
				let carried = url.query_pairs().any(|(key, _)| key == "jwt");
				let tokens = match (flagged, carried) {
					(true, _) => self.flags.tokens.clone(),
					(false, true) => Vec::new(),
					(false, false) => match self.credentials.get(url)? {
						Some(token) => vec![token.parse().map_err(|err| anyhow::anyhow!("{err}"))?],
						None => Vec::new(),
					},
				};
				Ok((url.clone(), tokens))
			})
			.collect()
	}

	/// Where going live with the relays `switched` on sends the program.
	/// Refuses a failover beside a relay a viewer could not move between
	/// without a restart.
	///
	/// Blocks as [`relays`](Self::relays) does.
	pub fn destination(&self, switched: Switched) -> anyhow::Result<Destination> {
		if switched.relay && switched.failover && self.relay().is_some() && self.failover().is_some() {
			self.check_failover()?;
		}
		let relays = self.relays(switched)?;
		anyhow::ensure!(!relays.is_empty(), "no relay is set; set one in Settings, Destinations");
		Ok(Destination {
			relays,
			broadcast: self.broadcast().to_string(),
			on_demand: self.on_demand(),
		})
	}

	/// Refuse relays a viewer could not move between without a restart: a
	/// copy resumes mid-group only under the same instance, which needs both
	/// links on one protocol version that carries it.
	fn check_failover(&self) -> anyhow::Result<()> {
		let [version] = self.client.version.as_slice() else {
			anyhow::bail!(
				"a failover relay needs exactly one --connect-version, so both relays negotiate the same protocol"
			);
		};
		anyhow::ensure!(
			matches!(version, moq_net::Version::Lite(lite) if lite.has_instance()),
			"a failover relay needs a --connect-version with broadcast instances (moq-lite-07-wip today), not {version}, \
			 or a viewer moving between relays restarts the stream rather than resuming it"
		);
		Ok(())
	}
}

/// `show.stream.toml` for `show.toml`.
fn path(scene: &Path) -> PathBuf {
	let mut name = scene.file_stem().unwrap_or(scene.as_os_str()).to_os_string();
	name.push(".stream.toml");
	scene.with_file_name(name)
}

/// Whether the address `url` carries a token that [saving](Stream::save)
/// takes out of it, its query's `jwt` as decoded, however it is written.
pub fn carries_token(url: &str) -> bool {
	Url::parse(url.trim()).is_ok_and(|url| url.query_pairs().any(|(key, _)| key == "jwt"))
}

/// `url` without the token its `?jwt=` carries, to show.
pub fn redacted(url: &Url) -> String {
	let mut url = url.clone();
	take_token(&mut url);
	url.to_string()
}

/// Take the token out of `url`'s `?jwt=`, if it has one, leaving the rest of
/// its query.
fn take_token(url: &mut Url) -> Option<String> {
	let mut token = None;
	let kept: Vec<(String, String)> = url
		.query_pairs()
		.filter_map(|(key, value)| match key == "jwt" {
			true => {
				token = Some(value.into_owned());
				None
			}
			false => Some((key.into_owned(), value.into_owned())),
		})
		.collect();
	token.as_ref()?;
	match kept.is_empty() {
		true => url.set_query(None),
		false => {
			url.query_pairs_mut().clear().extend_pairs(kept);
		}
	}
	token
}

/// What the credential store keeps tokens under.
const SERVICE: &str = "live.kixel.studio";

/// The operating system's credential store, where relay tokens are kept, one
/// per relay address.
#[derive(Clone)]
pub struct Credentials(Result<Arc<keyring_core::CredentialStore>, String>);

impl Credentials {
	/// This system's store: the Secret Service on Linux, the login keychain on
	/// macOS, the Credential Manager on Windows. One that cannot open says why
	/// whenever it is used.
	pub fn system() -> Self {
		Self(open().map_err(|err| format!("{err:#}")))
	}

	/// A store in memory, for tests.
	#[cfg(test)]
	pub fn memory() -> Self {
		let store: Arc<keyring_core::CredentialStore> = keyring_core::mock::Store::new().expect("a mock store");
		Self(Ok(store))
	}

	/// The token kept for the relay at `url`, if any. Without a store, none:
	/// a relay that needs no token still connects, one that does refuses the
	/// connection, and saving a token says why it cannot.
	///
	/// Blocks on the store, which may ask the user to unlock it: call it off
	/// the window's thread, and outside an async task.
	pub fn get(&self, url: &Url) -> anyhow::Result<Option<String>> {
		if let Err(err) = &self.0 {
			tracing::warn!(%err, "no credential store, so the relay is dialed without a token");
			return Ok(None);
		}
		match self.entry(url)?.get_password() {
			Ok(token) => Ok(Some(token)),
			Err(keyring_core::Error::NoEntry) => Ok(None),
			Err(err) => Err(err).context("cannot read the relay's token from the credential store"),
		}
	}

	/// Keep `token` for the relay at `url`, or forget its token with `None`.
	///
	/// Blocks as [`get`](Self::get) does.
	pub fn set(&self, url: &Url, token: Option<&str>) -> anyhow::Result<()> {
		let entry = self.entry(url)?;
		match token {
			Some(token) => entry
				.set_password(token)
				.context("cannot save the relay's token in the credential store"),
			None => match entry.delete_credential() {
				Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
				Err(err) => Err(err).context("cannot forget the relay's token in the credential store"),
			},
		}
	}

	/// The entry for the relay at `url`: its address without a query or a
	/// fragment, which a token could ride.
	fn entry(&self, url: &Url) -> anyhow::Result<keyring_core::Entry> {
		let store = self
			.0
			.as_ref()
			.map_err(|err| anyhow::anyhow!("no credential store: {err}"))?;
		let mut user = url.clone();
		user.set_query(None);
		user.set_fragment(None);
		let modifiers: Option<&HashMap<&str, &str>> = None;
		store
			.build(SERVICE, user.as_str(), modifiers)
			.context("cannot use the credential store")
	}
}

/// Open this system's credential store.
fn open() -> anyhow::Result<Arc<keyring_core::CredentialStore>> {
	#[cfg(target_os = "linux")]
	let store: Arc<keyring_core::CredentialStore> = zbus_secret_service_keyring_store::Store::new()?;
	#[cfg(target_os = "macos")]
	let store: Arc<keyring_core::CredentialStore> = apple_native_keyring_store::keychain::Store::new()?;
	#[cfg(windows)]
	let store: Arc<keyring_core::CredentialStore> = windows_native_keyring_store::Store::new()?;
	#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
	anyhow::bail!("this system has no credential store the studio knows");
	#[cfg(any(target_os = "linux", target_os = "macos", windows))]
	Ok(store)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn stream(dir: &Path, flags: Flags) -> Stream {
		Stream::load(
			&dir.join("show.toml"),
			flags,
			Default::default(),
			Default::default(),
			Credentials::memory(),
		)
		.unwrap()
	}

	/// A save that cannot write its file leaves every token as it was, the
	/// ones given and the one an address carries alike.
	#[test]
	fn a_failed_save_puts_back_the_tokens() {
		let dir = tempfile::tempdir().unwrap();
		let mut stream = stream(dir.path(), Flags::default());
		let a: Url = "https://a.example/".parse().unwrap();
		let b: Url = "https://b.example/".parse().unwrap();
		stream.credentials.set(&a, Some("old")).unwrap();
		// No file can be written where a directory stands.
		std::fs::create_dir(&stream.path).unwrap();
		let saved = Saved {
			relay: Some("https://b.example/?jwt=new".parse().unwrap()),
			..Default::default()
		};
		stream
			.save(saved, &[(a.clone(), Some("newer".into()))])
			.expect_err("the file cannot be written");
		assert_eq!(stream.credentials.get(&a).unwrap().as_deref(), Some("old"));
		assert_eq!(stream.credentials.get(&b).unwrap(), None);
		assert_eq!(stream.saved, Saved::default(), "and the settings are as they were");
	}

	/// The settings round-trip through the file beside the scene file, which
	/// a token in a relay's address never reaches: the credential store takes
	/// it, and the address keeps the rest of its query.
	#[test]
	fn saves_beside_the_scene_file_without_a_token() {
		let dir = tempfile::tempdir().unwrap();
		let mut first = stream(dir.path(), Flags::default());
		assert_eq!(first.path(), dir.path().join("show.stream.toml"));
		assert_eq!(first.broadcast(), BROADCAST);
		let saved = Saved {
			relay: Some("https://relay.example/anon?jwt=secret&keep=1".parse().unwrap()),
			broadcast: Some("friday".into()),
			on_demand: true,
			guest_expiry: Some(Duration::from_secs(3600)),
			..Default::default()
		};
		first.save(saved, &[]).unwrap();
		let text = std::fs::read_to_string(first.path()).unwrap();
		assert!(!text.contains("secret"), "the token reached the file: {text}");

		let second = Stream::load(
			&dir.path().join("show.toml"),
			Flags::default(),
			Default::default(),
			Default::default(),
			first.credentials.clone(),
		)
		.unwrap();
		let relay = second.relay().unwrap().clone();
		assert_eq!(relay.as_str(), "https://relay.example/anon?keep=1");
		assert_eq!(second.broadcast(), "friday");
		assert!(second.on_demand());
		assert_eq!(
			second.guests().guest_expiry.map(Duration::from),
			Some(Duration::from_secs(3600))
		);
		assert_eq!(second.credentials.get(&relay).unwrap().as_deref(), Some("secret"));
	}

	/// A flag wins over the saved setting for its run, and saving another
	/// setting never writes the flag's value back.
	#[test]
	fn flags_win_and_are_never_saved() {
		let dir = tempfile::tempdir().unwrap();
		let saved = Saved {
			relay: Some("https://saved.example/".parse().unwrap()),
			broadcast: Some("saved".into()),
			..Default::default()
		};
		stream(dir.path(), Flags::default()).save(saved.clone(), &[]).unwrap();
		let flags = Flags {
			relay: Some("https://flag.example/".parse().unwrap()),
			on_demand: true,
			..Default::default()
		};
		let mut flagged = stream(dir.path(), flags);
		assert_eq!(flagged.relay().unwrap().as_str(), "https://flag.example/");
		assert_eq!(flagged.broadcast(), "saved");
		assert!(flagged.on_demand());
		let overridden = flagged.overridden();
		assert!(overridden.relay && overridden.on_demand && !overridden.broadcast);

		flagged
			.save(
				Saved {
					broadcast: Some("renamed".into()),
					..flagged.saved().clone()
				},
				&[],
			)
			.unwrap();
		let reread = stream(dir.path(), Flags::default());
		assert_eq!(reread.relay().unwrap().as_str(), "https://saved.example/");
		assert_eq!(reread.broadcast(), "renamed");
		assert!(!reread.on_demand());
	}

	/// An unknown key is refused rather than dropped on the next save.
	#[test]
	fn refuses_an_unknown_setting() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("show.stream.toml"), "relays = []\n").unwrap();
		let loaded = Stream::load(
			&dir.path().join("show.toml"),
			Flags::default(),
			Default::default(),
			Default::default(),
			Credentials::memory(),
		);
		assert!(loaded.is_err());
	}

	/// Each relay switched on is dialed with its own token: the flags' for the
	/// relay they name, the store's for the failover, and a failover is
	/// refused unless both relays resume the same broadcast.
	#[test]
	fn each_relay_gets_its_own_token() {
		let dir = tempfile::tempdir().unwrap();
		let relay: Url = "https://a.example/".parse().unwrap();
		let failover: Url = "https://b.example/".parse().unwrap();
		let mut client = moq_tokio::connect::Config::default();
		let credentials = Credentials::memory();
		credentials.set(&failover, Some("b-token")).unwrap();
		let flags = Flags {
			relay: Some(relay.clone()),
			tokens: vec!["a-token".parse().unwrap()],
			failover: Some(failover.clone()),
			..Default::default()
		};
		let load = |client: moq_tokio::connect::Config| {
			Stream::load(
				&dir.path().join("show.toml"),
				flags.clone(),
				client,
				Default::default(),
				credentials.clone(),
			)
			.unwrap()
		};
		let unversioned = load(client.clone());
		assert!(
			unversioned.destination(Switched::default()).is_err(),
			"going live to both needs a single version"
		);
		assert_eq!(
			unversioned.dials(Switched::default()).unwrap().len(),
			2,
			"watching both does not"
		);
		client.version = vec!["moq-lite-07-wip".parse().unwrap()];
		let stream = load(client);
		let token = |token: &str| -> Vec<moq_net::auth::Token> { vec![token.parse().unwrap()] };
		let dials = stream.dials(Switched::default()).unwrap();
		assert_eq!(dials, [(relay, token("a-token")), (failover.clone(), token("b-token"))]);
		let alone = Switched {
			relay: false,
			failover: true,
		};
		assert_eq!(stream.dials(alone).unwrap(), [(failover, token("b-token"))]);
		let none = Switched {
			relay: false,
			failover: false,
		};
		assert!(stream.destination(none).is_err(), "nothing to send");
	}

	/// An address carries a token wherever saving would take one out of it,
	/// its key percent-encoded included, and only there.
	#[test]
	fn a_token_is_found_as_saving_finds_it() {
		for url in [
			"https://relay.example/anon?jwt=abc",
			" https://relay.example/anon?%6awt=abc ",
			"https://relay.example/anon?x=1&jwt=abc",
		] {
			assert!(carries_token(url), "{url}");
			let mut parsed: Url = url.trim().parse().unwrap();
			assert!(!redacted(&parsed).contains("abc"), "shown without it: {url}");
			assert_eq!(take_token(&mut parsed).as_deref(), Some("abc"), "{url}");
		}
		for url in [
			"https://relay.example/anon",
			"https://relay.example/anon?notjwt=abc",
			"https://relay.example/jwt=abc",
			"not a url ?jwt=abc",
		] {
			assert!(!carries_token(url), "{url}");
		}
	}

	/// An address carrying its own `?jwt=` dials with it, without the
	/// credential store, which may not open at all.
	#[test]
	fn a_token_in_the_address_needs_no_store() {
		let dir = tempfile::tempdir().unwrap();
		let relay: Url = "https://a.example/?jwt=x".parse().unwrap();
		let flags = Flags {
			relay: Some(relay.clone()),
			..Default::default()
		};
		let broken = Credentials(Err("no Secret Service".into()));
		let stream = Stream::load(
			&dir.path().join("show.toml"),
			flags,
			Default::default(),
			Default::default(),
			broken,
		)
		.unwrap();
		assert_eq!(stream.dials(Switched::default()).unwrap(), [(relay, Vec::new())]);
	}

	/// Without a credential store, a relay that needs no token still dials,
	/// tokenless, while saving a token says why it cannot.
	#[test]
	fn a_relay_without_a_token_needs_no_store() {
		let dir = tempfile::tempdir().unwrap();
		let relay: Url = "https://a.example/".parse().unwrap();
		let flags = Flags {
			relay: Some(relay.clone()),
			..Default::default()
		};
		let broken = Credentials(Err("no Secret Service".into()));
		let stream = Stream::load(
			&dir.path().join("show.toml"),
			flags,
			Default::default(),
			Default::default(),
			broken,
		)
		.unwrap();
		assert_eq!(
			stream.dials(Switched::default()).unwrap(),
			[(relay.clone(), Vec::new())]
		);
		let refused = stream.credentials.set(&relay, Some("secret")).unwrap_err();
		assert!(format!("{refused:#}").contains("no Secret Service"), "{refused:#}");
	}

	/// Watching, measuring, and going live share one client per relay, so a
	/// fixed `--connect-bind` port is bound once, and a failover, which
	/// would bind it again, is refused.
	#[tokio::test]
	async fn a_relay_is_set_up_once() {
		let dir = tempfile::tempdir().unwrap();
		let relay: Url = "https://a.example/".parse().unwrap();
		let free = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
		let mut client = moq_tokio::connect::Config::default();
		client.bind = Some(free.local_addr().unwrap());
		drop(free);
		let mut stream = Stream::load(
			&dir.path().join("show.toml"),
			Flags::default(),
			client,
			Default::default(),
			Credentials::memory(),
		)
		.unwrap();
		stream
			.save(
				Saved {
					relay: Some(relay.clone()),
					..Default::default()
				},
				&[],
			)
			.unwrap();
		let only = Switched {
			relay: true,
			failover: false,
		};
		let _watching = stream.relays(Switched::default()).unwrap();
		stream.relays(only).expect("the bound port is reused");
		stream.clone().destination(only).expect("so is going live's");
		assert_eq!(stream.clients.lock().unwrap().len(), 1);

		// A failover beside it would bind the port a second time, so the
		// pair is refused, whichever is switched on.
		let mut paired = stream.clone();
		paired
			.save(
				Saved {
					relay: Some(relay),
					failover: Some("https://b.example/".parse().unwrap()),
					..Default::default()
				},
				&[],
			)
			.unwrap();
		let refused = paired.relays(only).err().expect("a fixed port with a failover");
		assert!(format!("{refused:#}").contains("--connect-bind"), "{refused:#}");
	}
}
