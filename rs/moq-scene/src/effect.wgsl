// Every draw the compositor makes: a quad sampling one picture, through a list
// of per-pixel filters, a blur, or a drop shadow. Colors are gamma-encoded
// sRGB, as CSS filters see them. Every picture is sampled premultiplied, and
// rasters between passes hold premultiplied alpha, so neither scaling nor a
// blur mixes in a transparent pixel's color.

const OPS: u32 = 8u;

// Kinds of `Op`.
const MATRIX: u32 = 0u;
const CHROMA_KEY: u32 = 1u;
const LUMA_KEY: u32 = 2u;
const LUT: u32 = 3u;
const MASK: u32 = 4u;

// Rec. 709 luma.
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// One per-pixel filter.
struct Op {
	kind: vec4<u32>,
	// MATRIX: unused. CHROMA_KEY: key Cb, key Cr, similarity, smoothness.
	// LUMA_KEY: min, max, smoothness. LUT: amount, size. MASK: mode, and 1 if
	// the mask is premultiplied.
	params: vec4<f32>,
	// MATRIX: applied to straight RGBA. LUT: the domain's max in column 0.
	matrix: mat4x4<f32>,
	// MATRIX: added after. CHROMA_KEY: spill in x. LUT: the domain's min.
	offset: vec4<f32>,
}

struct Draw {
	// The quad: top-left and bottom-right, in clip space.
	rect: vec4<f32>,
	// The input's texture coordinates at the quad's corners.
	input: vec4<f32>,
	// Where the input has a picture, in texture coordinates; zero outside.
	valid: vec4<f32>,
	// The item's own coordinates at the quad's corners, 0 to 1 over the item.
	item: vec4<f32>,
	// The aux texture's coordinates at the quad's corners.
	aux: vec4<f32>,
	// Ops in x, whether the input is premultiplied in y, the blend mode in z.
	mode: vec4<u32>,
	// A blur's step between taps in xy, its standard deviation in z, and its
	// taps each side in w, all in texels.
	blur: vec4<f32>,
	// A shadow's straight color.
	color: vec4<f32>,
	ops: array<Op, OPS>,
}

@group(0) @binding(0) var<uniform> draw: Draw;
@group(1) @binding(0) var input: texture_2d<f32>;
@group(1) @binding(1) var bilinear: sampler;
@group(2) @binding(0) var lut: texture_3d<f32>;
@group(2) @binding(1) var mask: texture_2d<f32>;
// A drop shadow's blurred shape, or the backdrop a blend mode mixes with.
@group(2) @binding(2) var aux: texture_2d<f32>;

struct Varyings {
	@builtin(position) position: vec4<f32>,
	@location(0) input: vec2<f32>,
	@location(1) item: vec2<f32>,
	@location(2) aux: vec2<f32>,
}

// A four-vertex triangle strip covering the quad.
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
	let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
	var out: Varyings;
	out.position = vec4<f32>(mix(draw.rect.xy, draw.rect.zw, corner), 0.0, 1.0);
	out.input = mix(draw.input.xy, draw.input.zw, corner);
	out.item = mix(draw.item.xy, draw.item.zw, corner);
	out.aux = mix(draw.aux.xy, draw.aux.zw, corner);
	return out;
}

fn within(uv: vec2<f32>, low: vec2<f32>, high: vec2<f32>) -> bool {
	return all(uv >= low) && all(uv <= high);
}

// One texel of a straight-alpha texture, premultiplied.
fn texel(texture: texture_2d<f32>, at: vec2<i32>) -> vec4<f32> {
	let color = textureLoad(texture, at, 0);
	return vec4<f32>(color.rgb * color.a, color.a);
}

// `texture` at `uv`, mixed bilinearly with premultiplied alpha. A straight
// texture's four nearest texels are premultiplied before they mix, so a
// transparent texel's color, which shows nowhere, bleeds into no neighbour.
fn filtered(texture: texture_2d<f32>, uv: vec2<f32>, premultiplied: bool) -> vec4<f32> {
	if premultiplied {
		return textureSampleLevel(texture, bilinear, uv, 0.0);
	}
	// The sampler's clamp to the edge, by hand.
	let size = vec2<i32>(textureDimensions(texture));
	let at = uv * vec2<f32>(size) - 0.5;
	let low = vec2<i32>(floor(at));
	let near = clamp(low, vec2<i32>(0), size - 1);
	let far = clamp(low + 1, vec2<i32>(0), size - 1);
	let fraction = at - floor(at);
	let top = mix(texel(texture, near), texel(texture, vec2<i32>(far.x, near.y)), fraction.x);
	let bottom = mix(texel(texture, vec2<i32>(near.x, far.y)), texel(texture, far), fraction.x);
	return mix(top, bottom, fraction.y);
}

// The input at `uv`, premultiplied, and transparent outside its picture, as
// CSS's edge mode none.
fn fetch(uv: vec2<f32>) -> vec4<f32> {
	if !within(uv, draw.valid.xy, draw.valid.zw) {
		return vec4<f32>(0.0);
	}
	return filtered(input, uv, draw.mode.y != 0u);
}

// The input at `uv`, with straight alpha.
fn straight(uv: vec2<f32>) -> vec4<f32> {
	let color = fetch(uv);
	if color.a <= 0.0 {
		return vec4<f32>(0.0);
	}
	return vec4<f32>(color.rgb / color.a, color.a);
}

fn chroma_key(color: vec4<f32>, op: Op) -> vec4<f32> {
	let chroma = vec2<f32>(
		dot(color.rgb, vec3<f32>(-0.1146, -0.3854, 0.5)),
		dot(color.rgb, vec3<f32>(0.5, -0.4542, -0.0458)),
	);
	let past = distance(chroma, op.params.xy) - op.params.z;
	let alpha = pow(clamp(past / max(op.params.w, 1e-4), 0.0, 1.0), 1.5);
	let kept = pow(clamp(past / max(op.offset.x, 1e-4), 0.0, 1.0), 1.5);
	let gray = vec3<f32>(dot(color.rgb, LUMA));
	return vec4<f32>(mix(gray, color.rgb, kept), color.a * alpha);
}

fn luma_key(color: vec4<f32>, op: Op) -> vec4<f32> {
	let luma = dot(color.rgb, LUMA);
	let soft = max(op.params.z, 1e-4);
	let low = clamp((luma - op.params.x + soft) / soft, 0.0, 1.0);
	let high = clamp((op.params.y + soft - luma) / soft, 0.0, 1.0);
	return vec4<f32>(color.rgb, color.a * low * high);
}

// The table's color for `rgb`, interpolated between its eight nearest points.
fn lookup(rgb: vec3<f32>, op: Op) -> vec3<f32> {
	let size = i32(op.params.y);
	let low = op.offset.xyz;
	let high = op.matrix[0].xyz;
	let at = clamp((rgb - low) / (high - low), vec3<f32>(0.0), vec3<f32>(1.0)) * f32(size - 1);
	let base = min(vec3<i32>(floor(at)), vec3<i32>(size - 2));
	let fraction = at - vec3<f32>(base);
	var out = vec3<f32>(0.0);
	for (var corner = 0u; corner < 8u; corner++) {
		let corner_step = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
		let weights = select(1.0 - fraction, fraction, corner_step == vec3<u32>(1u));
		let weight = weights.x * weights.y * weights.z;
		out += weight * textureLoad(lut, base + vec3<i32>(corner_step), 0).rgb;
	}
	return out;
}

// The mask's coverage at `item`, stretched over the item and empty outside it.
fn coverage(item: vec2<f32>, op: Op) -> f32 {
	if !within(item, vec2<f32>(0.0), vec2<f32>(1.0)) {
		return 0.0;
	}
	let color = filtered(mask, item, op.params.y > 0.5);
	if op.params.x > 0.5 {
		return dot(color.rgb, LUMA);
	}
	return color.a;
}

// Run the draw's ops, in order, on a straight-alpha color.
fn apply(start: vec4<f32>, item: vec2<f32>) -> vec4<f32> {
	var color = start;
	for (var index = 0u; index < draw.mode.x; index++) {
		let op = draw.ops[index];
		switch op.kind.x {
			case MATRIX: {
				color = clamp(op.matrix * color + op.offset, vec4<f32>(0.0), vec4<f32>(1.0));
			}
			case CHROMA_KEY: {
				color = chroma_key(color, op);
			}
			case LUMA_KEY: {
				color = luma_key(color, op);
			}
			case LUT: {
				color = vec4<f32>(mix(color.rgb, lookup(color.rgb, op), op.params.x), color.a);
			}
			case MASK: {
				color.a *= coverage(item, op);
			}
			default: {}
		}
	}
	return color;
}

// The input through the ops, premultiplied: drawn over a canvas or into a raster.
@fragment fn fs_effect(in: Varyings) -> @location(0) vec4<f32> {
	let color = apply(straight(in.input), in.item);
	return vec4<f32>(color.rgb * color.a, color.a);
}

fn hard_light(backdrop: f32, source: f32) -> f32 {
	if source <= 0.5 {
		return backdrop * 2.0 * source;
	}
	let screen = 2.0 * source - 1.0;
	return backdrop + screen - backdrop * screen;
}

fn soft_light(backdrop: f32, source: f32) -> f32 {
	if source <= 0.5 {
		return backdrop - (1.0 - 2.0 * source) * backdrop * (1.0 - backdrop);
	}
	var d = sqrt(backdrop);
	if backdrop <= 0.25 {
		d = ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop;
	}
	return backdrop + (2.0 * source - 1.0) * (d - backdrop);
}

fn color_dodge(backdrop: f32, source: f32) -> f32 {
	if backdrop <= 0.0 {
		return 0.0;
	}
	if source >= 1.0 {
		return 1.0;
	}
	return min(1.0, backdrop / (1.0 - source));
}

fn color_burn(backdrop: f32, source: f32) -> f32 {
	if backdrop >= 1.0 {
		return 1.0;
	}
	if source <= 0.0 {
		return 0.0;
	}
	return 1.0 - min(1.0, (1.0 - backdrop) / source);
}

// The non-separable modes' luminosity and saturation, from the Compositing
// and Blending spec.
const LUM: vec3<f32> = vec3<f32>(0.3, 0.59, 0.11);

fn clip_color(color: vec3<f32>) -> vec3<f32> {
	let l = dot(color, LUM);
	let n = min(color.r, min(color.g, color.b));
	let x = max(color.r, max(color.g, color.b));
	var out = color;
	if n < 0.0 {
		out = l + (out - l) * l / (l - n);
	}
	if x > 1.0 {
		out = l + (out - l) * (1.0 - l) / (x - l);
	}
	return out;
}

fn set_lum(color: vec3<f32>, l: f32) -> vec3<f32> {
	return clip_color(color + (l - dot(color, LUM)));
}

fn sat(color: vec3<f32>) -> f32 {
	return max(color.r, max(color.g, color.b)) - min(color.r, min(color.g, color.b));
}

fn set_sat(color: vec3<f32>, s: f32) -> vec3<f32> {
	let low = min(color.r, min(color.g, color.b));
	let range = sat(color);
	if range <= 0.0 {
		return vec3<f32>(0.0);
	}
	return (color - low) * s / range;
}

fn mix_blend(backdrop: vec3<f32>, source: vec3<f32>, mode: u32) -> vec3<f32> {
	switch mode {
		case 1u: { return backdrop * source; }
		case 2u: { return backdrop + source - backdrop * source; }
		case 3u: {
			return vec3<f32>(
				hard_light(source.r, backdrop.r),
				hard_light(source.g, backdrop.g),
				hard_light(source.b, backdrop.b),
			);
		}
		case 4u: { return min(backdrop, source); }
		case 5u: { return max(backdrop, source); }
		case 6u: {
			return vec3<f32>(
				color_dodge(backdrop.r, source.r),
				color_dodge(backdrop.g, source.g),
				color_dodge(backdrop.b, source.b),
			);
		}
		case 7u: {
			return vec3<f32>(
				color_burn(backdrop.r, source.r),
				color_burn(backdrop.g, source.g),
				color_burn(backdrop.b, source.b),
			);
		}
		case 8u: {
			return vec3<f32>(
				hard_light(backdrop.r, source.r),
				hard_light(backdrop.g, source.g),
				hard_light(backdrop.b, source.b),
			);
		}
		case 9u: {
			return vec3<f32>(
				soft_light(backdrop.r, source.r),
				soft_light(backdrop.g, source.g),
				soft_light(backdrop.b, source.b),
			);
		}
		case 10u: { return abs(backdrop - source); }
		case 11u: { return backdrop + source - 2.0 * backdrop * source; }
		case 12u: { return set_lum(set_sat(source, sat(backdrop)), dot(backdrop, LUM)); }
		case 13u: { return set_lum(set_sat(backdrop, sat(source)), dot(backdrop, LUM)); }
		case 14u: { return set_lum(source, dot(backdrop, LUM)); }
		case 15u: { return set_lum(backdrop, dot(source, LUM)); }
		default: { return source; }
	}
}

// The input through the ops, mixed with the opaque backdrop in `aux` by the
// draw's blend mode: (1 - αs) Cb + αs B(Cb, Cs).
@fragment fn fs_blend(in: Varyings) -> @location(0) vec4<f32> {
	let source = apply(straight(in.input), in.item);
	let backdrop = textureLoad(aux, vec2<i32>(in.position.xy), 0).rgb;
	let mixed = mix_blend(backdrop, source.rgb, draw.mode.z);
	return vec4<f32>(mix(backdrop, mixed, source.a), 1.0);
}

// One direction of a separable Gaussian blur of a premultiplied raster. With
// no taps, one bilinear sample: a 2x2 average when halving a raster.
@fragment fn fs_blur(in: Varyings) -> @location(0) vec4<f32> {
	let sigma = draw.blur.z;
	let taps = i32(draw.blur.w);
	var sum = vec4<f32>(0.0);
	var total = 0.0;
	for (var tap = -taps; tap <= taps; tap++) {
		let weight = exp(-f32(tap * tap) / (2.0 * sigma * sigma));
		sum += weight * fetch(in.input + f32(tap) * draw.blur.xy);
		total += weight;
	}
	return sum / total;
}

// The premultiplied input over its shadow: the blurred shape in `aux`, filled
// with the shadow's color.
@fragment fn fs_shadow(in: Varyings) -> @location(0) vec4<f32> {
	let picture = fetch(in.input);
	var shape = 0.0;
	if within(in.aux, vec2<f32>(0.0), vec2<f32>(1.0)) {
		shape = textureSampleLevel(aux, bilinear, in.aux, 0.0).a;
	}
	let shadow = vec4<f32>(draw.color.rgb, 1.0) * draw.color.a * shape;
	return picture + shadow * (1.0 - picture.a);
}
