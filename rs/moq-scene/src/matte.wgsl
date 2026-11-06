// Background removal's GPU half: hold a picture, shrink it for the model, and
// cut it out with the model's matte.

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(1) @binding(0) var matte: texture_2d<f32>;

struct Varyings {
	@builtin(position) position: vec4<f32>,
	@location(0) uv: vec2<f32>,
}

// One triangle covering the whole target.
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
	let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
	var out: Varyings;
	out.position = vec4<f32>(corner * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
	out.uv = corner;
	return out;
}

// Same size, so each pixel samples its own texel's center.
@fragment fn fs_copy(in: Varyings) -> @location(0) vec4<f32> {
	return textureSample(picture, linear, in.uv);
}

// Sixteen bilinear taps spread over the target pixel's footprint: a box over
// 8x8 texels, enough for 1080p shrunk to the model's 256x144.
@fragment fn fs_shrink(in: Varyings) -> @location(0) vec4<f32> {
	let step = fwidth(in.uv) * 0.25;
	var sum = vec4<f32>(0.0);
	for (var y = 0; y < 4; y++) {
		for (var x = 0; x < 4; x++) {
			sum += textureSample(picture, linear, in.uv + (vec2<f32>(f32(x), f32(y)) - 1.5) * step);
		}
	}
	return sum / 16.0;
}

// The picture is premultiplied, so scaling every channel by the matte keeps it so.
@fragment fn fs_cut(in: Varyings) -> @location(0) vec4<f32> {
	return textureSample(picture, linear, in.uv) * textureSample(matte, linear, in.uv).r;
}
