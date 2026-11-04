import { expect, test } from "bun:test";
import { format, parse } from "./link.ts";

const fragment = (params: Record<string, string>) => new URLSearchParams(params).toString();

const KEY = "00ff".repeat(16);

test("a link names the room and keeps its token out of the URL", () => {
	const link = parse(
		fragment({ url: "https://relay.example:4443/show/guests", name: "alice", jwt: "a.b.c", key: KEY }),
	);
	expect(link.name).toBe("alice");
	expect(link.url.origin).toBe("https://relay.example:4443");
	expect(link.url.pathname).toBe("/show/guests");
	expect(link.url.search).toBe("");
	expect(link.jwt).toBe("a.b.c");
	expect(Array.from(link.key.slice(0, 2))).toEqual([0, 255]);
	expect(link.key.length).toBe(32);
	// A renewed token replaces the link's when the page stores it.
	expect(parse(format({ ...link, jwt: "d.e.f" }))).toEqual({ ...link, jwt: "d.e.f" });
});

test("a name with spaces survives the fragment's encoding", () => {
	const link = parse(fragment({ url: "https://relay.example/room", name: "Alice Smith", jwt: "t", key: KEY }));
	expect(link.name).toBe("Alice Smith");
	expect(parse(format(link)).name).toBe("Alice Smith");
});

test("an incomplete or malformed link is refused", () => {
	const valid = { url: "https://relay.example/room", name: "alice", jwt: "t", key: KEY };
	for (const key of ["url", "name", "jwt", "key"] as const) {
		const { [key]: _, ...missing } = valid;
		expect(() => parse(fragment(missing))).toThrow("incomplete");
	}
	expect(() => parse(fragment({ ...valid, url: "not a url" }))).toThrow("invalid relay");
	expect(() => parse(fragment({ ...valid, url: "moqt://relay.example/room" }))).toThrow("cannot dial");
	for (const key of ["00", KEY.toUpperCase(), `${KEY}00`, "zz".repeat(32)]) {
		expect(() => parse(fragment({ ...valid, key }))).toThrow("key is invalid");
	}
	for (const name of ["a/b", "a\\b", " alice"]) {
		expect(() => parse(fragment({ ...valid, name }))).toThrow("invalid guest");
	}
});
