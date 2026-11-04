import { expect, test } from "bun:test";
import { parse } from "./link.ts";

const fragment = (params: Record<string, string>) => new URLSearchParams(params).toString();

test("a link dials the room with its token in the query", () => {
	const link = parse(fragment({ url: "https://relay.example:4443/show/guests", name: "alice", jwt: "a.b.c" }));
	expect(link.name).toBe("alice");
	expect(link.url.origin).toBe("https://relay.example:4443");
	expect(link.url.pathname).toBe("/show/guests");
	expect(link.url.searchParams.get("jwt")).toBe("a.b.c");
});

test("a name with spaces survives the fragment's encoding", () => {
	const link = parse(fragment({ url: "https://relay.example/room", name: "Alice Smith", jwt: "t" }));
	expect(link.name).toBe("Alice Smith");
});

test("an incomplete or malformed link is refused", () => {
	const valid = { url: "https://relay.example/room", name: "alice", jwt: "t" };
	for (const key of ["url", "name", "jwt"] as const) {
		const { [key]: _, ...missing } = valid;
		expect(() => parse(fragment(missing))).toThrow("incomplete");
	}
	expect(() => parse(fragment({ ...valid, url: "not a url" }))).toThrow("invalid relay");
	expect(() => parse(fragment({ ...valid, url: "moqt://relay.example/room" }))).toThrow("cannot dial");
	for (const name of ["a/b", "a\\b", " alice"]) {
		expect(() => parse(fragment({ ...valid, name }))).toThrow("invalid guest");
	}
});
