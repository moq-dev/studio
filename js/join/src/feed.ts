/**
 * The return feed: while a guest is in the room, the studio publishes the show
 * back to them, the program with everyone's sound but their own, at a path only
 * their own token may read.
 *
 * @module
 */

import * as Net from "@moq/net";

/**
 * Where the studio publishes `name`'s return feed in `generation`: `Room::feed_path` in
 * `rs/moq-studio/src/guests.rs`.
 */
export function path(name: string, generation: number): Net.Path.Valid {
	return Net.Path.from(`.return/${name}/${generation}`);
}
