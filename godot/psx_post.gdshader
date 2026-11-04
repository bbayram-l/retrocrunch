// psx-forge screen pass (used by psx_screen.gd): the PS1 dither table and colour depth,
// applied once per virtual pixel of the low-resolution render, like the real GPU did.
// Optional CRT look (experimental): composite blur with colour bleed, scanlines and an
// aperture grille, the same recipe as psx-forge's viewport.
shader_type canvas_item;

uniform bool dither = true;
// Levels per channel: 32 = 15-bit RGB555, 256 = off.
uniform float color_levels = 32.0;
uniform bool crt = false;
// Darkness between scanlines (0 = flat, 1 = black gaps).
uniform float scanline = 0.45;
// The two "other" phosphors of each screen column are dimmed to this.
uniform float mask = 0.78;

const float PS1_DITHER[16] = float[16](
	-4.0, 0.0, -3.0, 1.0,
	2.0, -2.0, 3.0, -1.0,
	-3.0, 1.0, -4.0, 0.0,
	3.0, -1.0, 2.0, -2.0);

// One virtual pixel after dither and colour depth (sRGB 0..1).
vec3 ps1_texel(sampler2D tex, ivec2 t) {
	ivec2 res = textureSize(tex, 0);
	t = clamp(t, ivec2(0), res - 1);
	vec3 s = texelFetch(tex, t, 0).rgb * 255.0;
	if (dither) {
		ivec2 p = t & ivec2(3);
		s += PS1_DITHER[p.y * 4 + p.x];
	}
	s = clamp(floor(s), 0.0, 255.0);
	float step_size = 256.0 / clamp(color_levels, 2.0, 256.0);
	return floor(s / step_size) * step_size / 255.0;
}

vec3 to_linear(vec3 c) {
	return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

vec3 to_srgb(vec3 c) {
	return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

void fragment() {
	vec2 res = vec2(textureSize(TEXTURE, 0));
	vec2 src = UV * res;
	if (!crt) {
		COLOR = vec4(ps1_texel(TEXTURE, ivec2(floor(src))), 1.0);
	} else {
		int y = int(floor(src.y));
		vec3 rgb;
		for (int c = 0; c < 3; c++) {
			float shift = float(c - 1) * 0.33;
			float acc = 0.0;
			for (int k = -1; k <= 1; k++) {
				float x = src.x + shift + float(k) * 0.6;
				acc += (k == 0 ? 0.5 : 0.25) * to_linear(ps1_texel(TEXTURE, ivec2(int(floor(x)), y)))[c];
			}
			rgb[c] = acc;
		}
		float beam = 1.0 - scanline * (1.0 - sin(PI * fract(src.y)));
		int phase = int(mod(floor(FRAGCOORD.x), 3.0));
		for (int c = 0; c < 3; c++) {
			rgb[c] *= beam * 1.3 * (c == phase ? 1.0 : mask);
		}
		vec2 d = UV - 0.5;
		rgb *= 1.0 - 0.35 * dot(d, d);
		COLOR = vec4(to_srgb(clamp(rgb, 0.0, 1.0)), 1.0);
	}
}
