/**
 * The join page: a guest opens the studio's link and publishes their camera,
 * microphone, and screen into the studio's room, following moq-room's
 * `{name}/camera.hang` and `{name}/screen.hang`, and plays the show the studio
 * sends back.
 *
 * @module
 */

import * as Net from "@moq/net";
import * as Publish from "@moq/publish";
import { Local } from "@moq/room";
import { Effect, Signal } from "@moq/signals";
import * as Watch from "@moq/watch";
import * as Feed from "./feed.ts";
import { format, type Link } from "./link.ts";
import { element, fail, read, stored } from "./page.ts";
import * as Renew from "./renew.ts";

/** Where the fragment is kept once it leaves the address bar, so a reload rejoins. */
const STORED = "moq-studio-join";

/** Publish into the room until the guest leaves. Returns how to leave. */
function join(link: Link): () => void {
	const signals = new Effect();
	// The token rides SETUP rather than the URL, and each renewal replaces it in band.
	const tokens = new Signal<readonly string[]>([link.jwt]);
	const connection = new Net.Connection({ url: link.url, versions: [Renew.VERSION], tokens, enabled: true });
	signals.cleanup(() => connection.close());

	const renewal = Renew.follow(connection.origin, Renew.path(link.name, link.generation), (jwt) => {
		tokens.set([jwt]);
		// So joining again, or a reload, uses the newest token once the link's has expired.
		link.jwt = jwt;
		stored((storage) => storage.setItem(STORED, format(link)));
	});
	signals.cleanup(() => renewal.close());

	// The program, with everyone's sound but the guest's own, so they hear no echo of themselves.
	const show = new Watch.Player({
		origin: connection.origin,
		name: Feed.path(link.name, link.generation),
		// Claim the guest's receive rate, so the player drops to the half size their link can carry.
		viewer: { viewers: connection.viewers },
		video: { canvas: element<HTMLCanvasElement>("show") },
	});
	signals.cleanup(() => show.close());

	const local = new Local({
		connection,
		identity: Net.Path.from(link.name),
		enabled: true,
		cameraEnabled: true,
		microphoneEnabled: true,
		user: { id: link.name, name: link.name },
	});
	signals.cleanup(() => local.close());

	const preview = new Publish.Preview.Renderer({
		canvas: element<HTMLCanvasElement>("preview"),
		frames: local.cameraCapture.out.frames,
		display: local.cameraCapture.out.display,
		flip: true,
	});
	signals.cleanup(() => preview.close());

	// A refused permission or a missing device leaves the toggle on with nothing sent, so say so.
	signals.run((effect) => {
		const failures = [
			["camera", effect.get(local.webcam.out.error)],
			["microphone", effect.get(local.microphone.out.error)],
		] as const;
		const messages = failures.flatMap(([device, error]) => (error ? [`No ${device}: ${error.message}`] : []));
		// The screen picker gives no reason when it is closed or refused.
		if (effect.get(local.screenEnabled) && !effect.get(local.share.out.source)) {
			messages.push("No screen shared: pick one in the browser's prompt, or turn Share screen off.");
		}
		const capture = element("capture");
		capture.textContent = messages.join(" ");
		capture.hidden = messages.length === 0;
	});

	signals.run((effect) => {
		const status = effect.get(connection.status);
		element("status").textContent = status.charAt(0).toUpperCase() + status.slice(1);
		element("status-dot").className = `dot ${status}`;
		const error = effect.get(connection.error);
		if (error) fail(error);
	});

	const toggles = [
		["camera", local.cameraEnabled],
		["microphone", local.microphoneEnabled],
		["screen", local.screenEnabled],
	] as const;
	for (const [id, enabled] of toggles) {
		const button = element<HTMLButtonElement>(id);
		signals.event(button, "click", () => enabled.set(!enabled.peek()));
		signals.run((effect) => {
			button.setAttribute("aria-pressed", String(effect.get(enabled)));
		});
	}

	return () => signals.close();
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
	element("ready").hidden = false;

	// Capture starts on a click, which some phones require before they open the camera.
	let leave: (() => void) | undefined;
	element("join").addEventListener("click", () => {
		element("ready").hidden = true;
		element("error").hidden = true;
		element("joined").hidden = false;
		leave = join(link);
	});
	element("leave").addEventListener("click", () => {
		leave?.();
		leave = undefined;
		stored((storage) => storage.removeItem(STORED));
		element("joined").hidden = true;
		element("ready").hidden = false;
	});
}

main();
