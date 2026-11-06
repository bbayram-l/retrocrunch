// retrocrunch PSX material: PS1 semi-transparency, add (B + F), both faces drawn. The code lives in psx_common.gdshaderinc.
shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_opaque, blend_mix;
#define PSX_SEMI_ADD
#include "psx_common.gdshaderinc"
