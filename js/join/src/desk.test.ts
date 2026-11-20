import { expect, test } from "bun:test";
import { clock, type Desk, settled, tally } from "./desk.ts";

const desk = (change: Partial<Desk>): Desk => ({
	air: "off",
	clock: null,
	studio: false,
	program: "Gameplay",
	cued: null,
	scenes: [],
	take_refused: null,
	sounds: [],
	done: null,
	...change,
});

test("the clock reads hours, minutes, and seconds, past a day too", () => {
	expect(clock(0)).toBe("00:00:00");
	expect(clock(5047)).toBe("01:24:07");
	expect(clock(100 * 3600 + 59)).toBe("100:00:59");
	expect(clock(-3)).toBe("00:00:00");
});

test("the scene on program is on air only while live, and first before", () => {
	expect(tally(desk({ air: "live" }), "Gameplay")).toEqual({ tag: "On air", on: "air" });
	expect(tally(desk({ air: "ending" }), "Gameplay")?.on).toBe("air");
	expect(tally(desk({ air: "off" }), "Gameplay")).toEqual({ tag: "First", on: "cue" });
	expect(tally(desk({ air: "starting" }), "Gameplay")?.tag).toBe("First");
	expect(tally(desk({ cued: "Chatting" }), "Chatting")).toEqual({ tag: "Next", on: "cue" });
	expect(tally(desk({ cued: "Chatting" }), "BRB")).toBeUndefined();
});

test("a press is settled only by the desk naming this page and it or a later one", () => {
	const done = (id: string, seq: number) => desk({ done: { id, seq, ok: true, said: "" } });
	expect(settled(undefined, "a", 0)).toBe(false);
	expect(settled(desk({}), "a", 0)).toBe(false);
	expect(settled(done("a", 0), "a", 0)).toBe(true);
	expect(settled(done("a", 3), "a", 2)).toBe(true);
	expect(settled(done("a", 2), "a", 3)).toBe(false);
	// Another page, such as this one before a reload.
	expect(settled(done("b", 9), "a", 3)).toBe(false);
});
