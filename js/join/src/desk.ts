/**
 * The desk: the show as the remote shows it, which the studio publishes at a
 * path only the remote's own token may read, and the presses the remote
 * publishes back. The shapes are `Desk` and `Press` in
 * `rs/moq-studio/src/guests/remote.rs`.
 *
 * @module
 */

import * as Net from "@moq/net";

/** Where the studio publishes `name`'s desk in `generation`, with its renewed tokens. */
export function path(name: string, generation: number): Net.Path.Valid {
	return Net.Path.from(`.remote/${name}/${generation}`);
}

/** Where the remote called `name` publishes its presses in `generation`. */
export function pressPath(name: string, generation: number): Net.Path.Valid {
	return Net.Path.from(`.press/${name}/${generation}`);
}

/** The desk's track, a JSON snapshot. */
export const TRACK = "desk";

/** The presses' track, a JSON snapshot. */
export const PRESS_TRACK = "press";

/** Whether the program is on air. */
export type Air = "off" | "starting" | "live" | "ending";

/** The show as the studio last published it. */
export interface Desk {
	air: Air;
	/** Whole seconds on air, while live. */
	clock: number | null;
	/** Whether a pad cues its scene rather than taking it. */
	studio: boolean;
	program: string | null;
	cued: string | null;
	scenes: { name: string; refused: string | null }[];
	/** Why TAKE and AUTO cannot run now, if they cannot. */
	take_refused: string | null;
	sounds: { name: string; muted: boolean }[];
	/** How this page's last press went, once the studio has read one. */
	done: Done | null;
}

/** How a press went. */
export interface Done {
	id: string;
	seq: number;
	ok: boolean;
	said: string;
}

/** What a remote may press: `Action` in `rs/moq-studio/src/controls.rs`, as serde writes it. */
export type Action = "Cut" | "Transition" | { Scene: string } | { Mute: string };

/** A press: this page, its number, and the action, or none to say the page is there. */
export interface Press {
	id: string;
	seq: number;
	action: Action | null;
}

/**
 * Whether `desk` says how the press numbered `seq` from the page `id` went:
 * until it does, the page sends nothing more, so the studio, which reads only
 * the newest press, never drops one.
 */
export function settled(desk: Desk | undefined, id: string, seq: number): boolean {
	return desk?.done?.id === id && desk.done.seq >= seq;
}

/** `seconds` as the studio's live clock shows it: hours, minutes, and seconds. */
export function clock(seconds: number): string {
	const whole = Math.max(0, Math.floor(seconds));
	const two = (value: number) => value.toString().padStart(2, "0");
	return `${two(Math.floor(whole / 3600))}:${two(Math.floor(whole / 60) % 60)}:${two(whole % 60)}`;
}

/** A pad's tally: on program in the accent, or next in amber. */
export type Tally = { tag?: string; on: "air" | "cue" } | undefined;

/**
 * The tally of the pad for `scene`, as the studio's own pads show it: the
 * scene on program is in the accent, tagged ON AIR while live and untagged
 * otherwise, as the one the stream opens on; the scene cued is NEXT.
 */
export function tally(desk: Desk, scene: string): Tally {
	if (desk.program === scene) {
		return desk.air === "live" || desk.air === "ending" ? { tag: "On air", on: "air" } : { on: "air" };
	}
	if (desk.cued === scene) return { tag: "Next", on: "cue" };
	return undefined;
}
