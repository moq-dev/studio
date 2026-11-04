import { expect, test } from "bun:test";
import * as E2ee from "@moq/e2ee";
import * as Net from "@moq/net";
import { Signal } from "@moq/signals";
import { credential, follow, path } from "./renew.ts";

const encode = (text: string) => new TextEncoder().encode(text);

/** Resolve once `tokens` holds `count` entries. */
async function received(tokens: Signal<string[]>, count: number): Promise<string[]> {
	for (;;) {
		const now = tokens.peek();
		if (now.length >= count) return now;
		await tokens.changed();
	}
}

/** Publish `name`'s renewals as the studio does, encrypted to `key`. */
async function publish(origin: Net.Origin.Producer, name: string, key: Uint8Array) {
	const studio = credential(name, key);
	const generation = studio.produce();
	const publication = origin.createBroadcast(await path(studio));
	const track = await new E2ee.Broadcast.Producer(publication.broadcast, generation).createTrack("token");
	publication.announce({ epoch: generation.epoch });
	return async (token: string) => {
		const group = track.appendGroup();
		await group.writeFrame(undefined, encode(token));
		group.finish();
	};
}

test("each token the studio publishes reaches the page, decrypted with its own key", async () => {
	const origin = new Net.Origin.Producer();
	const alice = new Uint8Array(32).fill(1);
	const tokens = new Signal<string[]>([]);
	const renewal = follow(new Signal<Net.Origin.Reader | undefined>(origin), credential("alice", alice), (token) =>
		tokens.update((all) => [...all, token]),
	);

	await (await publish(origin, "bob", new Uint8Array(32).fill(2)))("bob's");
	const write = await publish(origin, "alice", alice);
	await write("first");
	expect(await received(tokens, 1)).toEqual(["first"]);
	await write("second");
	expect(await received(tokens, 2)).toEqual(["first", "second"]);

	renewal.close();
	origin.close();
});

test("a key that is not the guest's names another broadcast", async () => {
	const right = await path(credential("alice", new Uint8Array(32).fill(1)));
	// The same vector as `renewals_name_the_same_broadcast_as_the_join_page` in the studio.
	expect(right).toBe(".renew/8W8-sbdtrkZZF_byr3vEcQ");
	expect(await path(credential("alice", new Uint8Array(32).fill(1)))).toBe(right);
	expect(await path(credential("alice", new Uint8Array(32).fill(2)))).not.toBe(right);
	expect(await path(credential("bob", new Uint8Array(32).fill(1)))).not.toBe(right);
});
