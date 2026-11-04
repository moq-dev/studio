/**
 * The join link's fragment: the room's relay URL, the guest's name, and the
 * token the studio signed for that name. A fragment never reaches a server, so
 * the token stays out of every request log on the way here.
 *
 * @module
 */

/** A guest's way into the studio's room. */
export interface Link {
	/** The relay at the room. */
	url: URL;
	/** The guest's identity in the room: one path segment. */
	name: string;
	/** The token the studio signed for the guest: the link's, or a renewed one. */
	jwt: string;
	/** How often the host let the guest back in after a removal, which names where their renewals are. */
	generation: number;
}

/** Parse a join link's fragment, without its `#`. Throws on a link the studio did not make. */
export function parse(fragment: string): Link {
	const params = new URLSearchParams(fragment);
	const relay = params.get("url");
	const name = params.get("name");
	const jwt = params.get("jwt");
	const generation = params.get("generation");
	if (!relay || !name || !jwt || !generation)
		throw new Error("this join link is incomplete; ask the host for a new one");

	let url: URL;
	try {
		url = new URL(relay);
	} catch {
		throw new Error(`this join link names an invalid relay: ${relay}`);
	}
	if (url.protocol !== "https:" && url.protocol !== "http:") {
		throw new Error(`a browser cannot dial ${url.protocol}// relays`);
	}
	if (name.includes("/") || name.includes("\\") || name.trim() !== name) {
		throw new Error(`this join link names an invalid guest: ${name}`);
	}

	if (!/^(0|[1-9][0-9]*)$/.test(generation) || !Number.isSafeInteger(Number(generation))) {
		throw new Error(`this join link names an invalid generation: ${generation}`);
	}

	return { url, name, jwt, generation: Number(generation) };
}

/** The fragment {@link parse} reads `link` back from. */
export function format(link: Link): string {
	const { url, name, jwt, generation } = link;
	return new URLSearchParams({ url: url.href, name, jwt, generation: generation.toString() }).toString();
}
