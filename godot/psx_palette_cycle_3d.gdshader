// psx-forge palette cycling (experimental), 3D: an unshaded material whose texture is
// stored as palette indices; entries in [cycle_start, cycle_end] rotate over time.
// index_texture: <name>_index.png (index = red * 255); palette_texture: <name>_palette.png.
shader_type spatial;
render_mode unshaded, cull_back;

uniform sampler2D index_texture : filter_nearest;
uniform sampler2D palette_texture : filter_nearest;
uniform int cycle_start = 0;
uniform int cycle_end = 0;
// Palette steps per second (negative = the other way).
uniform float speed = 8.0;

vec3 srgb_to_linear(vec3 c) {
	return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

void fragment() {
	int i = int(round(texture(index_texture, UV).r * 255.0));
	int n = cycle_end - cycle_start + 1;
	if (n > 1 && i >= cycle_start && i <= cycle_end) {
		int shift = int(floor(TIME * speed));
		// Integer modulo (float mod can land on n itself), kept positive for negative speeds.
		i = cycle_start + ((i - cycle_start + shift) % n + n) % n;
	}
	vec4 c = texelFetch(palette_texture, ivec2(i, 0), 0);
	if (c.a < 0.5) {
		discard;
	}
	// The palette holds sRGB values: convert for the unshaded output.
	ALBEDO = srgb_to_linear(c.rgb * COLOR.rgb);
}
