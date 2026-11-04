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
	/** The 32-byte key that decrypts the guest's renewed tokens. */
	key: Uint8Array;
}

/** Parse a join link's fragment, without its `#`. Throws on a link the studio did not make. */
export function parse(fragment: string): Link {
	const params = new URLSearchParams(fragment);
	const relay = params.get("url");
	const name = params.get("name");
	const jwt = params.get("jwt");
	const hex = params.get("key");
	if (!relay || !name || !jwt || !hex) throw new Error("this join link is incomplete; ask the host for a new one");

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

	const key = hex.match(/^[0-9a-f]{64}$/)
		? Uint8Array.from(hex.match(/../g) ?? [], (byte) => parseInt(byte, 16))
		: undefined;
	if (!key) throw new Error("this join link's key is invalid; ask the host for a new one");

	return { url, name, jwt, key };
}

/** The fragment {@link parse} reads `link` back from. */
export function format(link: Link): string {
	const key = Array.from(link.key, (byte) => byte.toString(16).padStart(2, "0")).join("");
	return new URLSearchParams({ url: link.url.href, name: link.name, jwt: link.jwt, key }).toString();
}
