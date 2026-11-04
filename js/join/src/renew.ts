/**
 * Renewal: while a guest is in the room, the studio publishes a fresh token for
 * them every half link lifetime, at a path only their own token may read. The
 * page presents each one on its live session, so the guest stays past the
 * link's expiry without reconnecting.
 *
 * @module
 */

import * as Net from "@moq/net";
import { Effect, type Getter } from "@moq/signals";

/** The version that presents a token on a live session, and the only one the page offers. */
export const VERSION = "moq-lite-07-wip";

/** Where the studio publishes `name`'s tokens: `RENEW` in `rs/moq-studio/src/guests.rs`. */
export function path(name: string): Net.Path.Valid {
	return Net.Path.from(`.renew/${name}`);
}

/** The track of those tokens, one group each: `TOKEN_TRACK` in `rs/moq-studio/src/guests.rs`. */
const TRACK = "token";

/**
 * Hand `renewed` each token the studio publishes for `name` in `origin`, across
 * the studio's restarts, until the returned effect is closed.
 */
export function follow(
	origin: Getter<Net.Origin.Reader | undefined>,
	name: string,
	renewed: (token: string) => void,
): Effect {
	const signals = new Effect();
	signals.run((effect) => {
		const reader = effect.get(origin);
		if (!reader) return;
		const request = reader.follow(path(name));
		effect.cleanup(() => request.close());

		effect.run((effect) => {
			const broadcast = effect.get(request.active);
			if (!broadcast) return;
			const track = broadcast.track(TRACK).subscribe();
			effect.cleanup(() => track.close());
			effect.spawn(async () => {
				for (;;) {
					const group = await effect.race(track.recvGroup());
					if (!group) return;
					const token = await effect.race(group.readString());
					if (token) renewed(token);
				}
			});
		});
	});
	return signals;
}
