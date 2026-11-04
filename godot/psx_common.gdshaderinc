// psx-forge: shared PlayStation-style material code for Godot 4 (psx_*.gdshader).
//
// The colour maths match psx-forge's own preview: texture x vertex colour x tint are
// multiplied as 8-bit sRGB values (the PS1 GPU modulated texels by Gouraud colour that
// way), fog mixes in the same space, the PS1 4x4 dither table is added, and the result is
// truncated to 15-bit. Lighting is pre-baked into vertex colours, so the material is
// unshaded. Keep the Environment's tonemap on Linear (Godot's default) for exact colours.
//
// Variants define PSX_CUTOUT or PSX_BLEND before including this file.

// Raw sRGB values on purpose (no source_color): the maths above happen in sRGB.
uniform sampler2D albedo_texture : filter_nearest, repeat_enable;
uniform bool use_texture = true;
// Multiplies the sRGB values, like the vertex colour.
uniform vec4 tint = vec4(1.0);
uniform bool use_vertex_color = true;

group_uniforms ps1;
uniform bool vertex_snap = true;
// Virtual screen height the vertices snap to; the width follows the viewport's aspect.
uniform float snap_lines = 240.0;
// Fixed-point (GTE) precision: view-space positions round to this grid in world units
// before projection, so vertices shift against each other as the camera moves, like
// the PS1's geometry coprocessor. Try ~1/1000 of a character's height. 0 = off.
uniform float gte_unit = 0.0;
uniform bool affine_mapping = true;
// 0 = perspective-correct, 1 = full PS1 warping. In between tames big polygons.
uniform float affine_weight : hint_range(0.0, 1.0) = 1.0;
// Dither and colour depth per screen pixel. psx_screen.gd turns both off and does them
// once per virtual (240-line) pixel instead.
uniform bool dither = true;
// Levels per channel: 32 = 15-bit RGB555, 256 = off.
uniform float color_levels = 32.0;
#ifdef PSX_CUTOUT
uniform float alpha_cutoff : hint_range(0.0, 1.0) = 0.5;
#endif

group_uniforms fog;
uniform bool fog_enabled = false;
// sRGB, like everything else here.
uniform vec4 fog_color = vec4(0.05, 0.05, 0.08, 1.0);
uniform float fog_start = 8.0;
uniform float fog_end = 30.0;
group_uniforms;

// value * w and w are interpolated perspective-correctly; their ratio is screen-linear
// (affine) when w = clip.w and perspective-correct when w = 1. The hardware's Gouraud
// colour and fog were screen-linear too.
varying vec3 affine_uv;
varying vec4 affine_color;
varying vec2 fog_w;

void vertex() {
	vec4 view_pos = MODELVIEW_MATRIX * vec4(VERTEX, 1.0);
	if (gte_unit > 0.0) {
		view_pos.xyz = floor(view_pos.xyz / gte_unit + 0.5) * gte_unit;
	}
	vec4 clip = PROJECTION_MATRIX * view_pos;
	if (vertex_snap && clip.w > 0.0) {
		vec2 res = vec2(round(snap_lines * VIEWPORT_SIZE.x / max(VIEWPORT_SIZE.y, 1.0)), snap_lines);
		vec2 half_res = res * 0.5;
		clip.xy = round(clip.xy / clip.w * half_res) / half_res * clip.w;
	}
	POSITION = clip;
	float w = mix(1.0, clip.w, affine_mapping ? clamp(affine_weight, 0.0, 1.0) : 0.0);
	affine_uv = vec3(UV * w, w);
	affine_color = (use_vertex_color ? COLOR : vec4(1.0)) * w;
	float f = clamp((-view_pos.z - fog_start) / max(fog_end - fog_start, 0.001), 0.0, 1.0);
	fog_w = vec2((fog_enabled ? f : 0.0) * clip.w, clip.w);
}

const float PS1_DITHER[16] = float[16](
	-4.0, 0.0, -3.0, 1.0,
	2.0, -2.0, 3.0, -1.0,
	-3.0, 1.0, -4.0, 0.0,
	3.0, -1.0, 2.0, -2.0);

vec3 psx_srgb_to_linear(vec3 c) {
	return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

void fragment() {
	vec2 uv = affine_uv.xy / affine_uv.z;
	vec4 col = affine_color / affine_uv.z;
	vec4 tex = use_texture ? texture(albedo_texture, uv) : vec4(1.0);
	vec3 s = tex.rgb * col.rgb * tint.rgb * 255.0;
	s = mix(s, fog_color.rgb * 255.0, fog_w.x / max(fog_w.y, 1e-6));
	if (dither) {
		ivec2 p = ivec2(FRAGCOORD.xy) & ivec2(3);
		s += PS1_DITHER[p.y * 4 + p.x];
	}
	// Truncate like the hardware (and psx-forge's preview), then drop to the colour depth.
	s = clamp(floor(s), 0.0, 255.0);
	float step_size = 256.0 / clamp(color_levels, 2.0, 256.0);
	s = floor(s / step_size) * step_size;
	ALBEDO = psx_srgb_to_linear(s / 255.0);
#if defined(PSX_CUTOUT)
	ALPHA = tex.a * col.a * tint.a;
	ALPHA_SCISSOR_THRESHOLD = alpha_cutoff;
#elif defined(PSX_BLEND)
	ALPHA = tex.a * col.a * tint.a;
#endif
}
