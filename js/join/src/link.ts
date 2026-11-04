/**
 * The join link's fragment: the room's relay URL, the guest's name, and the
 * token the studio signed for that name. A fragment never reaches a server, so
 * the token stays out of every request log on the way here.
 *
 * @module
 */

/** A guest's way into the studio's room. */
export interface Link {
	/** The relay at the room, with the token in its query, as `@moq/net` dials it. */
	url: URL;
	/** The guest's identity in the room: one path segment. */
	name: string;
}

/** Parse a join link's fragment, without its `#`. Throws on a link the studio did not make. */
export function parse(fragment: string): Link {
	const params = new URLSearchParams(fragment);
	const relay = params.get("url");
	const name = params.get("name");
	const jwt = params.get("jwt");
	if (!relay || !name || !jwt) throw new Error("this join link is incomplete; ask the host for a new one");

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

	url.searchParams.set("jwt", jwt);
	return { url, name };
}
