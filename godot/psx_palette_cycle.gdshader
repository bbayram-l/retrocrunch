// psx-forge palette cycling (experimental), 2D: the texture is stored as palette
// indices, and the entries in [cycle_start, cycle_end] rotate over time, the classic
// way to animate water, lava, lights and conveyor belts without new pixels.
// index_texture: <name>_index.png (index = red * 255); palette_texture: <name>_palette.png.
shader_type canvas_item;

uniform sampler2D index_texture : filter_nearest;
uniform sampler2D palette_texture : filter_nearest;
uniform int cycle_start = 0;
uniform int cycle_end = 0;
// Palette steps per second (negative = the other way).
uniform float speed = 8.0;

void fragment() {
	int i = int(round(texture(index_texture, UV).r * 255.0));
	int n = cycle_end - cycle_start + 1;
	if (n > 1 && i >= cycle_start && i <= cycle_end) {
		int shift = int(floor(TIME * speed));
		// Integer modulo (float mod can land on n itself), kept positive for negative speeds.
		i = cycle_start + ((i - cycle_start + shift) % n + n) % n;
	}
	COLOR = texelFetch(palette_texture, ivec2(i, 0), 0);
}
