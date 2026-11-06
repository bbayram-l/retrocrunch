// retrocrunch: shared PlayStation-style material code for Godot 4 (psx_*.gdshader).
//
// The colour maths match retrocrunch's own preview: texture x vertex colour x tint are
// multiplied as 8-bit sRGB values (the PS1 GPU modulated texels by Gouraud colour that
// way), fog mixes in the same space, the PS1 4x4 dither table is added, and the result is
// truncated to 15-bit. Lighting is pre-baked into vertex colours, so the material is
// unshaded; psx_lights.gd adds moving lights per vertex (Gouraud), like the PS1's GTE.
// Keep the Environment's tonemap on Linear (Godot's default) for exact colours.
//
// Variants define PSX_CUTOUT, PSX_BLEND or one of the PS1 semi-transparency modes
// (PSX_SEMI_AVERAGE, PSX_SEMI_ADD, PSX_SEMI_ADD_QUARTER, PSX_SEMI_SUBTRACT) before
// including this file.

// Most dynamic lights one material takes (psx_lights.gd picks the nearest ones).
#define PSX_MAX_LIGHTS 8

// Raw sRGB values on purpose (no source_color): the maths above happen in sRGB.
uniform sampler2D albedo_texture : filter_nearest, repeat_enable;
uniform bool use_texture = true;
// Multiplies the sRGB values, like the vertex colour.
uniform vec4 tint = vec4(1.0);
uniform bool use_vertex_color = true;
// Texture repeats across the surface (for tiling floors and walls on plain meshes).
uniform vec2 uv_scale = vec2(1.0);

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
#if defined(PSX_SEMI_AVERAGE) || defined(PSX_SEMI_ADD) || defined(PSX_SEMI_ADD_QUARTER) || defined(PSX_SEMI_SUBTRACT)
#define PSX_SEMI
// What's already drawn behind: PS1 semi-transparency is computed on the stored 8-bit
// values, not in Godot's linear light, so the blend reads the screen and does it here.
uniform sampler2D psx_background : hint_screen_texture, filter_nearest;
#endif
// No z-buffer, like the PS1: every triangle gets one depth (its average with ps1_sort
// exports, else one corner's), so big polygons sort as a whole and can pop in front of
// nearby objects. The shader always writes depth, which costs the GPU's early depth test
// even when this is off.
uniform bool flat_depth = false;
// Moves this material's triangles nearer (negative) or farther in that sorting, like a
// PS1 game nudging an object's ordering-table slot.
uniform float depth_bias : hint_range(-0.01, 0.01) = 0.0;
// Optional PS1 accuracy switches (off = retrocrunch's usual look):
// PS1 dither rule: the GPU dithered only Gouraud-shaded or colour-modulated polygons, so
// unlit textures (vertex colour neutral) and flat untextured surfaces stay undithered.
uniform bool ps1_dither_rule = false;
// Drop triangles with a vertex nearer than this (world units, 0 = off). The PS1 had no
// near-plane clipping: games threw away polygons that came too close, so big floor and
// wall polygons vanish up close instead of being clipped.
uniform float near_drop = 0.0;
#ifdef PSX_SEMI
// PS1 STP bit: only texels with partial alpha (0.5..0.99) blend; fully opaque texels draw
// solid, as texels without the semi-transparency bit did. Off: every visible texel blends.
uniform bool stp_mode = false;
#endif

// Set every frame by psx_lights.gd; with no lights the baked vertex colours are used as
// they are. World space. pos.w: 0 directional (xyz = direction to the light), 1 point,
// 2 spot. color.rgb = colour x energy, color.a = range. spot.xyz = spot axis, spot.w =
// cosine of the cone's half angle.
group_uniforms lights;
uniform int psx_light_count = 0;
uniform vec4 psx_ambient = vec4(1.0);
uniform vec4 psx_light_pos[PSX_MAX_LIGHTS];
uniform vec4 psx_light_color[PSX_MAX_LIGHTS];
uniform vec4 psx_light_spot[PSX_MAX_LIGHTS];
// Per-object lights, the PS1 way (psx_lights.gd in Authentic mode): the GTE held three
// directional lights plus ambient, and games turned nearby point lights into directions
// per object on the CPU. psx_obj_lights = 1 uses these instead of the lights above.
instance uniform int psx_obj_lights = 0;
instance uniform vec4 psx_obj_ambient = vec4(1.0);
instance uniform vec4 psx_obj_dir0 = vec4(0.0);
instance uniform vec4 psx_obj_dir1 = vec4(0.0);
instance uniform vec4 psx_obj_dir2 = vec4(0.0);
instance uniform vec4 psx_obj_col0 = vec4(0.0);
instance uniform vec4 psx_obj_col1 = vec4(0.0);
instance uniform vec4 psx_obj_col2 = vec4(0.0);

group_uniforms fog;
uniform bool fog_enabled = false;
// sRGB, like everything else here.
uniform vec4 fog_color = vec4(0.05, 0.05, 0.08, 1.0);
uniform float fog_start = 8.0;
uniform float fog_end = 30.0;
// PS1 depth cueing: the GTE's fog follows 1/distance (from the perspective divide), so it
// thickens quickly near fog_start and slowly towards fog_end. Off: linear in distance.
uniform bool fog_ps1_curve = false;
group_uniforms;

// value * w and w are interpolated perspective-correctly; their ratio is screen-linear
// (affine) when w = clip.w and perspective-correct when w = 1. The hardware's Gouraud
// colour and fog were screen-linear too.
varying vec3 affine_uv;
varying vec4 affine_color;
varying vec2 fog_w;
varying flat float triangle_depth;

// Dynamic light reaching a vertex (world space): ambient plus every light, per vertex like
// the GTE. Up to 2x, as PS1 colour modulation could brighten a texture to double.
vec3 psx_lighting(vec3 p, vec3 n) {
	vec3 light = psx_ambient.rgb;
	for (int i = 0; i < min(psx_light_count, PSX_MAX_LIGHTS); i++) {
		vec4 lp = psx_light_pos[i];
		vec4 lc = psx_light_color[i];
		if (lp.w < 0.5) {
			light += lc.rgb * max(dot(n, normalize(lp.xyz)), 0.0);
			continue;
		}
		vec3 to_light = lp.xyz - p;
		float d = length(to_light);
		vec3 l = to_light / max(d, 1e-4);
		float falloff = clamp(1.0 - d / max(lc.a, 1e-4), 0.0, 1.0);
		if (lp.w > 1.5) {
			vec4 sp = psx_light_spot[i];
			falloff *= smoothstep(sp.w, mix(sp.w, 1.0, 0.1), dot(-l, sp.xyz));
		}
		light += lc.rgb * max(dot(n, l), 0.0) * falloff;
	}
	return clamp(light, 0.0, 2.0);
}

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
	if (near_drop > 0.0 && -view_pos.z < near_drop) {
		// A NaN vertex makes the GPU throw the whole triangle away, like the PS1 did.
		POSITION = vec4(uintBitsToFloat(0x7fc00000u));
	}
	// The depth a whole triangle sorts by, as the depth buffer stores it (reversed:
	// nearer = larger, in every renderer in Godot 4.7). The PS1 sorted by the AVERAGE depth
	// of a polygon's corners: meshes exported with ps1_sort carry each triangle's centre
	// (offset from the vertex) in CUSTOM0, so all three corners compute the same centre.
	// Without it, one corner's depth is used (the first in Forward+/Mobile, the last in
	// Compatibility). OpenGL's clip range is -1..1.
	vec4 sort_clip = clip;
	if (flat_depth) {
		sort_clip = PROJECTION_MATRIX * (MODELVIEW_MATRIX * vec4(VERTEX + CUSTOM0.xyz, 1.0));
	}
#if CURRENT_RENDERER == RENDERER_COMPATIBILITY
	triangle_depth = sort_clip.z / sort_clip.w * 0.5 + 0.5;
#else
	triangle_depth = sort_clip.z / sort_clip.w;
#endif
	float w = mix(1.0, clip.w, affine_mapping ? clamp(affine_weight, 0.0, 1.0) : 0.0);
	affine_uv = vec3(UV * w, w);
	vec4 base = use_vertex_color ? COLOR : vec4(1.0);
	if (psx_obj_lights == 1) {
		vec3 wn = normalize(MODEL_NORMAL_MATRIX * NORMAL);
		vec3 light = psx_obj_ambient.rgb
			+ psx_obj_col0.rgb * max(dot(wn, psx_obj_dir0.xyz), 0.0)
			+ psx_obj_col1.rgb * max(dot(wn, psx_obj_dir1.xyz), 0.0)
			+ psx_obj_col2.rgb * max(dot(wn, psx_obj_dir2.xyz), 0.0);
		base.rgb *= clamp(light, 0.0, 2.0);
	} else if (psx_light_count > 0) {
		vec3 wp = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
		vec3 wn = normalize(MODEL_NORMAL_MATRIX * NORMAL);
		base.rgb *= psx_lighting(wp, wn);
	}
	affine_color = base * w;
	float dist = -view_pos.z;
	float f;
	if (fog_ps1_curve) {
		// Linear in 1/distance, like the GTE's depth cueing (IR0 from H/SZ).
		float near_inv = 1.0 / max(fog_start, 0.001);
		float far_inv = 1.0 / max(fog_end, fog_start + 0.001);
		f = clamp((near_inv - 1.0 / max(dist, 0.001)) / max(near_inv - far_inv, 1e-6), 0.0, 1.0);
	} else {
		f = clamp((dist - fog_start) / max(fog_end - fog_start, 0.001), 0.0, 1.0);
	}
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

vec3 psx_linear_to_srgb(vec3 c) {
	c = max(c, vec3(0.0));
	return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

void fragment() {
	vec2 uv = affine_uv.xy / affine_uv.z * uv_scale;
	vec4 col = affine_color / affine_uv.z;
	vec4 tex = use_texture ? texture(albedo_texture, uv) : vec4(1.0);
	vec3 s = tex.rgb * col.rgb * tint.rgb * 255.0;
	s = mix(s, fog_color.rgb * 255.0, fog_w.x / max(fog_w.y, 1e-6));
	// The PS1 skipped dither on raw textures (no colour modulation) and flat untextured
	// polygons; neutral vertex colour (1.0) counts as unmodulated here.
	bool unmodulated = use_texture
		? all(lessThan(abs(col.rgb * tint.rgb - vec3(1.0)), vec3(0.004)))
		: !use_vertex_color && psx_light_count == 0 && psx_obj_lights == 0;
	if (dither && !(ps1_dither_rule && unmodulated)) {
		ivec2 p = ivec2(FRAGCOORD.xy) & ivec2(3);
		s += PS1_DITHER[p.y * 4 + p.x];
	}
	// Truncate like the hardware (and retrocrunch's preview), then drop to the colour depth.
	s = clamp(floor(s), 0.0, 255.0);
	float step_size = 256.0 / clamp(color_levels, 2.0, 256.0);
	s = floor(s / step_size) * step_size;
#ifdef PSX_SEMI
	// PS1 semi-transparency: clear texels (alpha < 0.5, the PS1's transparent black) are
	// skipped; the rest combine with what's behind (B) by a fixed rule on 8-bit values:
	// average B/2 + F/2, add B + F, add-quarter B + F/4, subtract B - F. Surfaces see the
	// opaque scene behind them, not each other (Godot's screen texture).
	float texel_alpha = tex.a * col.a * tint.a;
	if (texel_alpha < 0.5) {
		discard;
	}
	// In STP mode, fully opaque texels lack the semi-transparency bit: drawn solid.
	if (!stp_mode || texel_alpha < 0.999) {
		// The background as the 15-bit framebuffer holds it.
		vec3 b = floor(psx_linear_to_srgb(texture(psx_background, SCREEN_UV).rgb) * 255.0 + 0.5);
		b = floor(b / step_size) * step_size;
#if defined(PSX_SEMI_AVERAGE)
		s = floor((b + s) * 0.5);
#elif defined(PSX_SEMI_ADD)
		s = b + s;
#elif defined(PSX_SEMI_ADD_QUARTER)
		s = b + floor(s * 0.25);
#else
		s = b - s;
#endif
		s = floor(clamp(s, 0.0, 255.0) / step_size) * step_size;
	}
	ALPHA = 1.0;
#endif
	ALBEDO = psx_srgb_to_linear(s / 255.0);
	// Reversed depth: subtracting a negative bias brings the triangle nearer.
	DEPTH = flat_depth ? clamp(triangle_depth - depth_bias, 0.0, 1.0) : FRAGCOORD.z;
#if defined(PSX_CUTOUT)
	ALPHA = tex.a * col.a * tint.a;
	ALPHA_SCISSOR_THRESHOLD = alpha_cutoff;
#elif defined(PSX_BLEND)
	ALPHA = tex.a * col.a * tint.a;
#endif
}
