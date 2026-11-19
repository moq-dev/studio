// The picture under an item keeping clear, as a grid of tiles, against the
// read before: each tile's share of pixels whose brightness moved, that are
// edges, and that are edges that held still, and its mean brightness. One
// workgroup reduces each tile, a thread to a pixel.

@group(0) @binding(0) var picture: texture_2d<f32>;
// The read before, or this one when there was none.
@group(0) @binding(1) var before: texture_2d<f32>;
// Each tile, row by row, as packed RGBA8: moved, edges, still, brightness.
@group(0) @binding(2) var<storage, read_write> tiles: array<u32>;

// A tile's side, as place.rs's TILE.
const SIDE = 16u;
// How far brightness steps to a neighbor at an edge, out of 1.
const EDGE = 0.1;
// How far a pixel's brightness changes when it moved.
const MOVED = 0.06;
// How little an edge's brightness changes when it held still.
const STILL = 0.02;

var<workgroup> moved: atomic<u32>;
var<workgroup> edges: atomic<u32>;
var<workgroup> still: atomic<u32>;
var<workgroup> light: atomic<u32>;

fn luma(texture: texture_2d<f32>, at: vec2<i32>, size: vec2<i32>) -> f32 {
	let color = textureLoad(texture, clamp(at, vec2<i32>(0), size - 1), 0).rgb;
	return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// How far brightness steps from `at` to its right and lower neighbors, the
// larger of the two.
fn rise(texture: texture_2d<f32>, at: vec2<i32>, size: vec2<i32>, here: f32) -> f32 {
	let right = luma(texture, at + vec2<i32>(1, 0), size);
	let below = luma(texture, at + vec2<i32>(0, 1), size);
	return max(abs(right - here), abs(below - here));
}

@compute @workgroup_size(16, 16)
fn reduce(
	@builtin(workgroup_id) tile: vec3<u32>,
	@builtin(num_workgroups) grid: vec3<u32>,
	@builtin(global_invocation_id) pixel: vec3<u32>,
	@builtin(local_invocation_index) index: u32,
) {
	let size = vec2<i32>(textureDimensions(picture));
	let at = vec2<i32>(pixel.xy);
	if all(at < size) {
		let now = luma(picture, at, size);
		let then = luma(before, at, size);
		let change = abs(now - then);
		if change > MOVED {
			atomicAdd(&moved, 1u);
		}
		if rise(picture, at, size, now) > EDGE {
			atomicAdd(&edges, 1u);
			if change < STILL && rise(before, at, size, then) > EDGE {
				atomicAdd(&still, 1u);
			}
		}
		atomicAdd(&light, u32(round(now * 255.0)));
	}
	workgroupBarrier();
	if index == 0u {
		let start = vec2<i32>(tile.xy * SIDE);
		let end = min(start + vec2<i32>(i32(SIDE)), size);
		let count = f32((end.x - start.x) * (end.y - start.y));
		let shares = vec4<f32>(
			f32(atomicLoad(&moved)),
			f32(atomicLoad(&edges)),
			f32(atomicLoad(&still)),
			f32(atomicLoad(&light)) / 255.0,
		) / count;
		tiles[tile.y * grid.x + tile.x] = pack4x8unorm(shares);
	}
}
