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

test("each token the studio publishes reaches the page, and only for its own name and generation", async () => {
	const origin = new Net.Origin.Producer();
	const tokens = new Signal<string[]>([]);
	const renewal = follow(new Signal<Net.Origin.Reader | undefined>(origin), path("alice", 0), (token) =>
		tokens.update((all) => [...all, token]),
	);

	for (const [name, generation] of [
		["bob", 0],
		["alice", 1],
	] as const) {
		const other = origin.route(path(name, generation), { hidden: true });
		other.broadcast.createTrack("token").writeFrame({ payload: encode(`${name} ${generation}`) });
		other.announce();
	}

	const publication = origin.route(path("alice", 0), { hidden: true });
	const track = publication.broadcast.createTrack("token");
	track.writeFrame({ payload: encode("first") });
	publication.announce();
	expect(await received(tokens, 1)).toEqual(["first"]);

	track.writeFrame({ payload: encode("second") });
	expect(await received(tokens, 2)).toEqual(["first", "second"]);

	renewal.close();
	origin.close();
});

test("a group superseded before it completes does not end the renewals", async () => {
	const origin = new Net.Origin.Producer();
	const tokens = new Signal<string[]>([]);
	const renewal = follow(new Signal<Net.Origin.Reader | undefined>(origin), path("alice", 0), (token) =>
		tokens.update((all) => [...all, token]),
	);

	const publication = origin.route(path("alice", 0), { hidden: true });
	const track = publication.broadcast.createTrack("token");
	track.writeFrame({ payload: encode("first") });
	publication.announce();
	expect(await received(tokens, 1)).toEqual(["first"]);
	// Opened but never written, as when the studio stops partway through one,
	// so the page is reading it when the next arrives.
	const partial = track.appendGroup();
	for (let turn = 0; turn < 10; turn++) await new Promise((resolve) => setTimeout(resolve));
	track.writeFrame({ payload: encode("next") });
	expect(await received(tokens, 2)).toEqual(["first", "next"]);
	track.writeFrame({ payload: encode("last") });
	expect(await received(tokens, 3)).toEqual(["first", "next", "last"]);

	partial.finish();
	renewal.close();
	origin.close();
});
