// psx-forge: PlayStation-style material for Godot 4.
// Vertex snapping (jitter), affine texture mapping, nearest filtering,
// 15-bit colour with the PS1 dither table, optional distance fog.
// Lighting is pre-baked into vertex colours by psx-forge, so the material is unshaded.
shader_type spatial;
render_mode unshaded, cull_back, depth_draw_opaque, diffuse_lambert;

uniform sampler2D albedo_texture : source_color, filter_nearest, repeat_enable;
uniform bool use_texture = true;
uniform vec4 tint : source_color = vec4(1.0);
uniform bool use_vertex_color = true;

group_uniforms ps1;
uniform bool vertex_snap = true;
// Virtual screen resolution the vertices snap to (320x240 = original hardware).
uniform vec2 snap_resolution = vec2(320.0, 240.0);
uniform bool affine_mapping = true;
// 0 = perspective-correct, 1 = full PS1 warping. In between tames big polygons.
uniform float affine_weight : hint_range(0.0, 1.0) = 1.0;
uniform bool dither = true;
// Levels per channel: 32 = 15-bit RGB555.
uniform float color_levels = 32.0;
uniform float alpha_cutoff = 0.5;

group_uniforms fog;
uniform bool fog_enabled = false;
uniform vec4 fog_color : source_color = vec4(0.05, 0.05, 0.08, 1.0);
uniform float fog_start = 8.0;
uniform float fog_end = 30.0;

varying vec3 affine_uv;
varying float fog_amount;

void vertex() {
	vec4 view_pos = MODELVIEW_MATRIX * vec4(VERTEX, 1.0);
	vec4 clip = PROJECTION_MATRIX * view_pos;
	if (vertex_snap && clip.w > 0.0) {
		vec2 half_res = snap_resolution * 0.5;
		clip.xy = round(clip.xy / clip.w * half_res) / half_res * clip.w;
	}
	POSITION = clip;
	// UV * w and w are interpolated perspective-correctly; their ratio is screen-linear
	// (affine) when w = clip.w, and the normal perspective-correct UV when w = 1.
	float w = mix(1.0, clip.w, affine_mapping ? clamp(affine_weight, 0.0, 1.0) : 0.0);
	affine_uv = vec3(UV * w, w);
	fog_amount = clamp((-view_pos.z - fog_start) / max(fog_end - fog_start, 0.001), 0.0, 1.0);
}

const float PS1_DITHER[16] = float[16](
	-4.0, 0.0, -3.0, 1.0,
	2.0, -2.0, 3.0, -1.0,
	-3.0, 1.0, -4.0, 0.0,
	3.0, -1.0, 2.0, -2.0);

void fragment() {
	vec2 uv = affine_uv.xy / affine_uv.z;
	vec4 tex = use_texture ? texture(albedo_texture, uv) : vec4(1.0);
	if (tex.a < alpha_cutoff) {
		discard;
	}
	vec3 c = tex.rgb * tint.rgb;
	if (use_vertex_color) {
		c *= COLOR.rgb;
	}
	if (fog_enabled) {
		c = mix(c, fog_color.rgb, fog_amount);
	}
	// Quantise in display (sRGB-ish) space like the hardware did.
	vec3 s = pow(max(c, vec3(0.0)), vec3(1.0 / 2.2));
	float d = 0.0;
	if (dither) {
		ivec2 p = ivec2(FRAGCOORD.xy) % 4;
		d = PS1_DITHER[p.y * 4 + p.x] / 255.0;
	}
	float lv = max(color_levels - 1.0, 1.0);
	s = floor((s + d) * lv + 0.5) / lv;
	ALBEDO = pow(clamp(s, 0.0, 1.0), vec3(2.2));
}
