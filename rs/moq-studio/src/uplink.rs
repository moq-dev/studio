//! Measure the uplink to the relay, for guided setup.
//!
//! The bandwidth grant alone measures nothing: a congestion controller only
//! learns what the path carries while something is sent. So this publishes a
//! short probe broadcast, subscribes to it back through the relay so the relay
//! pulls it, reserves the most setup could propose so the transport probes up
//! to that, and reads the congestion controller's send-rate estimate.

use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use moq_net::bandwidth::Rate;

/// How long the estimate is watched once the probe flows. The transport's probe
/// reaches its target within about five seconds of the path opening up.
pub const WINDOW: Duration = Duration::from_secs(6);

/// The least filler sent per second, until the estimate says more fits.
const FLOOR: Rate = Rate::from_kbps(500);

/// Filler frames per second.
const RATE: u32 = 30;

/// The probe's one track.
const TRACK: &str = "probe";

/// Publish a probe at `path` on the relay at `url` for `window` once it flows,
/// probing up to `ceiling`, and return the send-rate estimate it ends with.
///
/// Fails, rather than guessing, when either connection fails, the relay never
/// delivers the probe back, or the transport reports no estimate (WebSocket
/// has none).
pub async fn measure(
	client: &moq_tokio::Client,
	url: url::Url,
	path: &str,
	ceiling: Rate,
	window: Duration,
) -> anyhow::Result<Rate> {
	let publish = moq_tokio::origin::spawn();
	let publication = publish
		.create_broadcast(path)
		.context("cannot create the probe broadcast")?;
	let track = publication
		.broadcast()
		.create_track(TRACK, None)
		.context("cannot create the probe track")?;
	publication
		.announce(moq_net::origin::Route::default())
		.context("cannot announce the probe")?;

	// One-shot, so a relay that refuses either side ends the probe with its reason
	// instead of being redialed.
	let client = client.clone().with_reconnect(false);
	let publisher = client.clone().with_publisher(&publish).connect(url.clone());
	let subscribe = moq_tokio::origin::spawn();
	let viewer = client.with_subscriber(subscribe.clone()).connect(url);

	// The transport probes up to the reservations' sum, so the estimate can rise
	// past what the filler sends.
	let reservation = publisher.allocator().reserve(&track.demand(), ceiling);

	let measured = async {
		// The window starts when the probe comes back, so a slow dial does not eat it.
		let Ok(received) = kio::clock::timeout(window, receive(subscribe.consume(), path)).await else {
			anyhow::bail!(match publisher.connected() && viewer.connected() {
				true => format!("the relay did not deliver the probe back within {} s", window.as_secs()),
				false => format!("cannot connect to the relay within {} s", window.as_secs()),
			});
		};
		let mut groups = received?;
		// Drain the probe so the relay keeps pulling it; only the window ends this.
		let drained: Result<anyhow::Result<std::convert::Infallible>, _> = kio::clock::timeout(window, async {
			loop {
				groups.recv_group().await?.context("the probe ended early")?;
			}
		})
		.await;
		if let Ok(Err(err)) = drained {
			return Err(err);
		}
		// The controller's estimate is already a maximum over its recent rounds.
		publisher
			.send_bandwidth()
			.peek()
			.context("the relay connection reports no bandwidth estimate, as over WebSocket")
	};

	let result = tokio::select! {
		result = publisher.closed() => Err(anyhow::anyhow!("the relay connection ended: {}", describe(result))),
		result = viewer.closed() => Err(anyhow::anyhow!("the probe's viewer connection ended: {}", describe(result))),
		Err(err) = fill(track, &reservation, ceiling) => Err(err),
		result = measured => result,
	};
	publication.close();
	result
}

/// Wait for the probe through the relay, and subscribe to its track.
async fn receive(origin: moq_net::origin::Consumer, path: &str) -> anyhow::Result<moq_net::track::Subscriber> {
	let broadcast = origin.follow(path, None)?.available().await?;
	let mut track = broadcast.track(TRACK)?.subscribe(None).await?;
	// Wait for the first group, so the window measures data that flows.
	track.recv_group().await?.context("the probe ended before it began")?;
	Ok(track)
}

/// Send filler at the rate the estimate grants, between [`FLOOR`] and `ceiling`.
///
/// Returns only on an error, since the probe ends by dropping it.
async fn fill(
	track: moq_net::track::Producer,
	reservation: &moq_net::bandwidth::Reservation,
	ceiling: Rate,
) -> anyhow::Result<()> {
	let largest = (ceiling.as_bps() / 8 / u64::from(RATE)) as usize;
	let filler = Bytes::from(vec![0u8; largest.max(1)]);
	let mut clock = kio::clock::interval(Duration::from_secs(1) / RATE);
	clock.set_missed_tick_behavior(kio::clock::MissedTickBehavior::Skip);
	let mut group = None;
	let mut frames = 0;

	loop {
		clock.tick().await;
		if frames % RATE == 0 {
			if let Some(mut done) = group.take() {
				moq_net::group::Producer::finish(&mut done)?;
			}
			group = Some(track.append_group()?);
		}
		frames += 1;
		let rate = reservation.rate().unwrap_or(FLOOR).clamp(FLOOR, ceiling);
		let size = (rate.as_bps() / 8 / u64::from(RATE)) as usize;
		let group = group.as_mut().expect("opened above");
		group.write_frame(None, filler.slice(..size.clamp(1, filler.len())))?;
	}
}

fn describe(result: moq_tokio::Result<()>) -> String {
	match result {
		Ok(()) => "closed".into(),
		Err(err) => err.to_string(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The probe reaches a relay and comes back with the congestion
	/// controller's estimate.
	#[tokio::test(flavor = "multi_thread")]
	async fn measures_the_uplink_to_a_relay() {
		let _ = moq_tokio::crypto::install_default();
		let fixture = moq_relay::test_relay().await.expect("test relay");
		let ready = fixture.relay.ready();
		tokio::spawn(fixture.relay.run());
		ready.wait().await.expect("relay ready");

		let mut client = moq_tokio::connect::Config::default();
		client.tls.fingerprint = vec![fixture.fingerprint.clone()];
		let client = client.init(Default::default()).expect("client");

		let rate = measure(
			&client,
			fixture.url.clone(),
			"studio",
			Rate::from_mbps(10),
			Duration::from_secs(2),
		)
		.await
		.expect("a measurement");
		assert!(rate > Rate::ZERO, "an estimate of {rate:?}");
	}

	/// A relay that cannot be reached fails the probe rather than reporting a rate.
	#[tokio::test(flavor = "multi_thread")]
	async fn reports_an_unreachable_relay() {
		let _ = moq_tokio::crypto::install_default();
		let client = moq_tokio::connect::Config::default()
			.init(Default::default())
			.expect("client");
		// Nothing listens on the discard port.
		let url = url::Url::parse("https://127.0.0.1:9").unwrap();
		let err = measure(&client, url, "studio", Rate::from_mbps(10), Duration::from_secs(2))
			.await
			.expect_err("no relay answers");
		assert!(err.to_string().contains("connect"), "names the connection: {err:#}");
	}
}
