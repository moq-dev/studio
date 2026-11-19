// A capture's picture as a grid of tiles: each tile's mean color, and its
// brightest channel of any texel, so one lit pixel in a tile counts. One
// workgroup reduces each tile, its threads reading every texel of it once,
// side by side, so the reads are spread over the whole GPU.

@group(0) @binding(0) var picture: texture_2d<f32>;
// Each tile, row by row, as packed RGBA8: the mean color, and the brightest
// channel as alpha.
@group(0) @binding(1) var<storage, read_write> tiles: array<u32>;

// The grid, as dark.rs's TILES.
const TILES = vec2<u32>(64u, 36u);
const SIDE = 16u;

var<workgroup> sums: array<vec4<f32>, 256>;

@compute @workgroup_size(16, 16)
fn reduce(@builtin(workgroup_id) tile: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) index: u32) {
	let size = textureDimensions(picture);
	let start = tile.xy * size / TILES;
	// A picture smaller than the grid gives a tile one texel at least.
	let end = max(min((tile.xy + 1u) * size / TILES, size), start + 1u);
	var sum = vec3<f32>(0.0);
	var brightest = 0.0;
	for (var y = start.y + local.y; y < end.y; y += SIDE) {
		for (var x = start.x + local.x; x < end.x; x += SIDE) {
			let color = textureLoad(picture, vec2<u32>(x, y), 0).rgb;
			sum += color;
			brightest = max(brightest, max(color.r, max(color.g, color.b)));
		}
	}
	sums[index] = vec4<f32>(sum, brightest);
	workgroupBarrier();
	for (var half = 128u; half > 0u; half >>= 1u) {
		if index < half {
			let other = sums[index + half];
			sums[index] = vec4<f32>(sums[index].rgb + other.rgb, max(sums[index].a, other.a));
		}
		workgroupBarrier();
	}
	if index == 0u {
		let count = f32((end.x - start.x) * (end.y - start.y));
		tiles[tile.y * TILES.x + tile.x] = pack4x8unorm(vec4<f32>(sums[0].rgb / count, sums[0].a));
	}
}
