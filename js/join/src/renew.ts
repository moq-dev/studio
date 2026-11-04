/**
 * Renewal: while a guest is in the room, the studio publishes a fresh token for
 * them every half link lifetime, encrypted with `@moq/e2ee` to the key in their
 * link, at a path only their own token may read. The page presents each one on
 * its live session, so the guest stays past the link's expiry without
 * reconnecting.
 *
 * @module
 */

import * as E2ee from "@moq/e2ee";
import * as Net from "@moq/net";
import { Effect, type Getter, Signal } from "@moq/signals";

/** The version that presents a token on a live session, and the only one the page offers. */
export const VERSION = "moq-lite-07-wip";

/** The prefix of every guest's renewals: `RENEW` in `rs/moq-studio/src/guests.rs`. */
const PREFIX = ".renew";

/** The semantic broadcast and track names: `RENEW_BROADCAST` and `TOKEN_TRACK` there. */
const BROADCAST = "renew";
const TRACK = "token";

/** The credential that decrypts `name`'s renewals, from the 32-byte `key` in their link. */
export function credential(name: string, key: Uint8Array): E2ee.Credential {
	return new E2ee.Credential({ context: new TextEncoder().encode(name), kid: 0, secret: key });
}

/** Where the renewals `credential` decrypts are published. */
export async function path(credential: E2ee.Credential): Promise<Net.Path.Valid> {
	return Net.Path.join(Net.Path.from(PREFIX), await credential.path(BROADCAST));
}

/**
 * Hand `renewed` each token the studio publishes with `credential` in `origin`,
 * across the studio's restarts, until the returned effect is closed.
 */
export function follow(
	origin: Getter<Net.Origin.Reader | undefined>,
	credential: E2ee.Credential,
	renewed: (token: string) => void,
): Effect {
	const signals = new Effect();
	const at = new Signal<Net.Path.Valid | undefined>(undefined);
	signals.spawn(async () => at.set(await path(credential)));

	signals.run((effect) => {
		const reader = effect.get(origin);
		const renewals = effect.get(at);
		if (!reader || !renewals) return;
		const request = reader.follow(renewals);
		effect.cleanup(() => request.close());

		effect.run((effect) => {
			const broadcast = effect.get(request.active);
			// moq-lite-07 carries the publisher's epoch, which keys its encryption.
			const epoch = broadcast?.epoch;
			if (!broadcast || !epoch) return;
			const decrypted = new E2ee.Broadcast.Consumer(broadcast, credential.consume(epoch));
			effect.spawn(async () => {
				const track = await effect.race(decrypted.subscribe(TRACK));
				if (!track) return;
				try {
					for (;;) {
						const group = await effect.race(track.recvGroup());
						if (!group) return;
						const frame = await effect.race(group.readFrame());
						if (frame) renewed(new TextDecoder().decode(frame.plaintext));
					}
				} finally {
					track.close();
				}
			});
		});
	});
	return signals;
}
