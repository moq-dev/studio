import { expect, test } from "bun:test";
import * as Net from "@moq/net";
import { Signal } from "@moq/signals";
import { follow, path } from "./renew.ts";

const encode = (text: string) => new TextEncoder().encode(text);

/** Resolve once `tokens` holds `count` entries. */
async function received(tokens: Signal<string[]>, count: number): Promise<string[]> {
	for (;;) {
		const now = tokens.peek();
		if (now.length >= count) return now;
		await tokens.changed();
	}
}

test("each token the studio publishes reaches the page, and only for its own name", async () => {
	const origin = new Net.Origin.Producer();
	const tokens = new Signal<string[]>([]);
	const renewal = follow(new Signal<Net.Origin.Reader | undefined>(origin), "alice", (token) =>
		tokens.update((all) => [...all, token]),
	);

	const other = origin.createBroadcast(path("bob"));
	other.broadcast.createTrack("token").writeFrame({ payload: encode("bob's") });
	other.announce();

	const publication = origin.createBroadcast(path("alice"));
	const track = publication.broadcast.createTrack("token");
	track.writeFrame({ payload: encode("first") });
	publication.announce();
	expect(await received(tokens, 1)).toEqual(["first"]);

	track.writeFrame({ payload: encode("second") });
	expect(await received(tokens, 2)).toEqual(["first", "second"]);

	renewal.close();
	origin.close();
});
