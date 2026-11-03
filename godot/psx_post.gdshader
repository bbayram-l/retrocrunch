// psx-forge screen pass (used by psx_screen.gd): the PS1 dither table and colour depth,
// applied once per virtual pixel of the low-resolution render, like the real GPU did.
shader_type canvas_item;

uniform bool dither = true;
// Levels per channel: 32 = 15-bit RGB555, 256 = off.
uniform float color_levels = 32.0;

const float PS1_DITHER[16] = float[16](
	-4.0, 0.0, -3.0, 1.0,
	2.0, -2.0, 3.0, -1.0,
	-3.0, 1.0, -4.0, 0.0,
	3.0, -1.0, 2.0, -2.0);

void fragment() {
	// The viewport texture holds display (sRGB) values: work in 8-bit units.
	vec2 res = vec2(textureSize(TEXTURE, 0));
	ivec2 p = ivec2(floor(UV * res)) & ivec2(3);
	vec3 s = texture(TEXTURE, UV).rgb * 255.0;
	if (dither) {
		s += PS1_DITHER[p.y * 4 + p.x];
	}
	s = clamp(floor(s), 0.0, 255.0);
	float step_size = 256.0 / clamp(color_levels, 2.0, 256.0);
	COLOR = vec4(floor(s / step_size) * step_size / 255.0, 1.0);
}
