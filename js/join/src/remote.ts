/**
 * The remote: a phone opens the studio's QR code and drives the show from
 * here, wherever it reaches the relay: the scene pads, TAKE and AUTO, the
 * mutes, the live clock, and a tally. Its token publishes only its presses and
 * reads only the desk the studio publishes it, with its renewed tokens.
 *
 * @module
 */

import { Duration } from "@moq/clock";
import { Snapshot } from "@moq/json";
import * as Net from "@moq/net";
import { Effect, Signal } from "@moq/signals";
import * as Desk from "./desk.ts";
import { format, type Link } from "./link.ts";
import { element, fail, read, stored } from "./page.ts";
import * as Renew from "./renew.ts";

/** Where the fragment is kept once it leaves the address bar, so a reload reconnects. */
const STORED = "moq-studio-remote";

/** Drive the studio until the page closes. */
function drive(link: Link): void {
	const signals = new Effect();
	// The token rides SETUP rather than the URL, and each renewal replaces it in band.
	const tokens = new Signal<readonly string[]>([link.jwt]);
	const connection = new Net.Connection({ url: link.url, versions: [Renew.VERSION], tokens, enabled: true });
	const desks = Desk.path(link.name, link.generation);

	Renew.follow(connection.origin, desks, (jwt) => {
		tokens.set([jwt]);
		// So a reload uses the newest token once the link's has expired.
		link.jwt = jwt;
		stored((storage) => storage.setItem(STORED, format(link)));
	});

	// This page, so a desk saying how another's press went, such as this one's before a reload, settles nothing.
	const id = crypto.randomUUID();
	const desk = new Signal<Desk.Desk | undefined>(undefined);
	// The number of the newest press sent, which the next waits for the desk to settle.
	const sent = new Signal<number | undefined>(undefined);
	const presses = new Signal<Snapshot.Producer<Desk.Press> | undefined>(undefined);
	let next = 0;
	const send = (producer: Snapshot.Producer<Desk.Press>, action: Desk.Action | null) => {
		const seq = next++;
		producer.update({ value: { id, seq, action }, at: Net.Time.Timestamp.unscaled(seq) });
		sent.set(seq);
	};
	const press = (action: Desk.Action) => {
		const producer = presses.peek();
		const last = sent.peek();
		if (!producer || last === undefined || !Desk.settled(desk.peek(), id, last)) return;
		send(producer, action);
	};

	signals.run((effect) => {
		const origin = effect.get(connection.origin);
		if (!origin) return;
		const route = origin.route(Desk.pressPath(link.name, link.generation));
		effect.cleanup(() => route.close());
		const producer = new Snapshot.Producer<Desk.Press>({
			track: route.broadcast.createTrack(Desk.PRESS_TRACK),
			// Each press whole, since the studio reads only the newest.
			deltas: false,
		});
		// The studio never does the first press it reads, which it may have done before it was
		// listening, so each route opens with the page saying it is there.
		send(producer, null);
		presses.set(producer);
		effect.cleanup(() => presses.set(undefined));
	});

	signals.run((effect) => {
		const origin = effect.get(connection.origin);
		if (!origin) return;
		const following = origin.follow(desks);
		effect.cleanup(() => following.close());
		effect.run((effect) => {
			const broadcast = effect.get(following.active);
			if (!broadcast) {
				desk.set(undefined);
				return;
			}
			const consumer = new Snapshot.Consumer<Desk.Desk>({ track: broadcast.track(Desk.TRACK) });
			effect.cleanup(() => consumer.close());
			effect.spawn(async () => {
				for (;;) {
					const latest = await effect.race(consumer.latest());
					if (!latest) return;
					desk.set(latest.value);
				}
			});
		});
	});

	signals.run((effect) => {
		const status = effect.get(connection.status);
		element("status").textContent = status.charAt(0).toUpperCase() + status.slice(1);
		element("status-dot").className = `dot ${status}`;
		const error = effect.get(connection.error);
		if (error) fail(error);
	});

	// The clock counts on from the whole seconds of the newest desk.
	const live = new Signal<{ seconds: number; at: number } | undefined>(undefined);
	signals.run((effect) => {
		const seconds = effect.get(desk)?.clock;
		live.set(seconds == null ? undefined : { seconds, at: performance.now() });
	});
	signals.run((effect) => {
		const current = effect.get(desk);
		const on = effect.get(live);
		const air = element("air");
		const tick = () => {
			const seconds = on ? on.seconds + (performance.now() - on.at) / 1000 : 0;
			air.textContent = current ? said(current.air, Desk.clock(seconds)) : "";
		};
		tick();
		if (on) effect.interval(tick, Duration.millis(250));
	});

	const show = view(press);
	signals.run((effect) => {
		const current = effect.get(desk);
		const last = effect.get(sent);
		const ready = last !== undefined && Desk.settled(current, id, last);
		show(current, ready, current?.done?.id === id ? current.done : undefined);
	});
}

/** What the live control says while the program is `air`, with its `clock` while live. */
function said(air: Desk.Air, clock: string): string {
	switch (air) {
		case "off":
			return "Off air";
		case "starting":
			return "Going live";
		case "live":
			return `Live ${clock}`;
		case "ending":
			return "Ending";
	}
}

/**
 * Show each desk, its controls pressable once `ready`, and how this page's
 * last press went. A control stays the same element from one desk to the
 * next, changed in place, since a desk arrives each second while live: a tap
 * that spans one still lands, and focus stays where it was.
 */
function view(
	press: (action: Desk.Action) => void,
): (desk: Desk.Desk | undefined, ready: boolean, done: Desk.Done | undefined) => void {
	// By the action each presses, which never changes for an element.
	const controls = new Map<string, HTMLButtonElement>();
	const control = (action: Desk.Action, className: string): HTMLButtonElement => {
		const key = JSON.stringify(action);
		let button = controls.get(key);
		if (!button) {
			button = document.createElement("button");
			button.type = "button";
			button.className = className;
			button.addEventListener("click", () => press(action));
			controls.set(key, button);
		}
		return button;
	};
	const take = control("Cut", "");
	const auto = control("Transition", "");
	take.textContent = "Take";
	auto.textContent = "Auto";
	element("take").replaceChildren(take, auto);
	const rows = new Map<string, HTMLDivElement>();

	return (desk, ready, done) => {
		element("waiting").hidden = desk !== undefined;
		element("desk").hidden = desk === undefined;
		document.body.dataset.air = desk?.air ?? "off";
		element("air").dataset.air = desk?.air ?? "off";
		if (!desk) return;

		const enable = (button: HTMLButtonElement, refused: string | null) => {
			button.disabled = !ready || refused !== null;
			if (refused) button.title = refused;
			else button.removeAttribute("title");
		};

		const pads = desk.scenes.map((scene) => {
			const pad = control({ Scene: scene.name }, "pad");
			if (!pad.firstChild) {
				const tag = document.createElement("span");
				tag.className = "tag";
				const name = document.createElement("span");
				name.textContent = scene.name;
				pad.append(tag, name);
			}
			const tally = Desk.tally(desk, scene.name);
			const tag = pad.firstChild as HTMLSpanElement;
			tag.hidden = !tally;
			text(tag, tally?.tag ?? "");
			if (tally) pad.dataset.tally = tally.on;
			else delete pad.dataset.tally;
			enable(pad, scene.refused);
			return pad;
		});
		children(element("scenes"), pads);
		text(element("mode"), desk.studio ? "Studio mode: a pad cues its scene" : "A pad takes its scene");

		// TAKE and AUTO put the cued scene on air, in Studio mode.
		element("take").hidden = !desk.studio;
		enable(take, desk.take_refused);
		enable(auto, desk.take_refused);

		const sounds = desk.sounds.map((sound) => {
			let row = rows.get(sound.name);
			const mute = control({ Mute: sound.name }, "");
			if (!row) {
				row = document.createElement("div");
				row.className = "sound";
				const name = document.createElement("span");
				name.textContent = sound.name;
				row.append(name, mute);
				rows.set(sound.name, row);
			}
			text(mute, sound.muted ? "Muted" : "Mute");
			mute.setAttribute("aria-pressed", String(sound.muted));
			mute.setAttribute("aria-label", `${sound.muted ? "Unmute" : "Mute"} ${sound.name}`);
			enable(mute, null);
			return row;
		});
		children(element("sounds"), sounds);
		element("no-sounds").hidden = sounds.length > 0;

		// Forget the controls of scenes and sounds the show no longer has.
		const kept = new Set([take, auto, ...pads, ...sounds.map((row) => row.lastChild)]);
		for (const [key, button] of controls) if (!kept.has(button)) controls.delete(key);
		for (const [name, row] of rows) if (!sounds.includes(row)) rows.delete(name);

		const message = element("said");
		message.hidden = !done;
		text(message, done?.said ?? "");
		message.classList.toggle("error", done?.ok === false);
	};
}

/** Set `node`'s text, unless it already says it, which would replace its text node. */
function text(node: HTMLElement, said: string): void {
	if (node.textContent !== said) node.textContent = said;
}

/** Make `nodes` the children of `parent`, in order, moving none that already are. */
function children(parent: HTMLElement, nodes: readonly Node[]): void {
	const same = parent.childNodes.length === nodes.length && nodes.every((node, i) => parent.childNodes[i] === node);
	if (!same) parent.replaceChildren(...nodes);
}

function main(): void {
	let link: Link;
	try {
		link = read(STORED);
	} catch (err) {
		fail(err);
		return;
	}
	element("name").textContent = link.name;
	drive(link);
}

main();
