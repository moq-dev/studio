/**
 * What the join page and the remote share: finding their elements, saying what
 * failed, and reading the link they were opened with out of the address bar.
 *
 * @module
 */

import { type Link, parse } from "./link.ts";

/** The element with `id`, which the page's HTML always has. */
export function element<T extends HTMLElement>(id: string): T {
	const found = document.getElementById(id);
	if (!found) throw new Error(`missing #${id}`);
	return found as T;
}

/** Show `err` in the page's `#error`. */
export function fail(err: unknown): void {
	const error = element("error");
	error.textContent = err instanceof Error ? err.message : String(err);
	error.hidden = false;
}

/**
 * `use` on the page's session storage, or `undefined` where the browser denies
 * it, which only costs rejoining after a reload.
 */
export function stored<T>(use: (storage: Storage) => T): T | undefined {
	try {
		return use(sessionStorage);
	} catch {
		return undefined;
	}
}

/**
 * The link this page was opened with, moved out of the address bar and kept
 * under `key` in session storage, so a reload rejoins.
 */
export function read(key: string): Link {
	const fragment = location.hash.slice(1) || stored((storage) => storage.getItem(key));
	if (!fragment) throw new Error("open the link the host sent you");
	const link = parse(fragment);
	// Out of the address bar, history, and anything that shows them, such as a screen share.
	stored((storage) => storage.setItem(key, fragment));
	history.replaceState(null, "", location.pathname + location.search);
	return link;
}
