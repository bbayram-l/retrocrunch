//! OpenGL viewport renderer (through eframe's glow backend).
//!
//! Renders the scene into an offscreen framebuffer at the retro resolution, then blits it
//! up with nearest filtering, so PS1 pixelation is preserved at any window size. The
//! shaders mirror `retrocrunch::raster`: integer vertex snapping, the affine-weight UV
//! blend, nearest texturing, cutout alpha, fog, the PS1 dither table and 15-bit colour for
//! the PSX view, and the same GGX + studio-environment model for the PBR reference.
//! The CPU rasterizer remains the fallback (no GL, ordering-table mode, compile errors).

use eframe::glow::{self, HasContext};
use glam::{Mat4, Vec3};
use image::RgbaImage;
use std::sync::{Arc, Mutex};

const VS: &str = r#"#version 330 core
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_col;
layout(location = 3) in vec3 a_nrm;
layout(location = 4) in vec4 a_raw;
uniform mat4 u_view;
uniform mat4 u_proj;
uniform vec2 u_res;
uniform int u_snap_on;
uniform float u_snap;
uniform float u_affine;
uniform float u_gte;
uniform int u_mode;
out vec3 v_uvw;
out vec4 v_colw;
out vec3 v_pos;
out vec3 v_nrm;
out float v_viewz;
void main() {
    if (u_mode == 2 || u_mode == 4) { // background / CRT pass: full-screen triangle
        gl_Position = vec4(a_pos.xy, 0.0, 1.0);
        v_uvw = vec3(0.0, 0.0, 1.0); v_colw = vec4(1.0); v_pos = vec3(0.0); v_nrm = vec3(0.0); v_viewz = 0.0;
        return;
    }
    vec4 vp = u_view * vec4(a_pos, 1.0);
    if (u_gte > 0.0) { // fixed-point view space, like the PS1 GTE
        vp.xyz = floor(vp.xyz / u_gte + 0.5) * u_gte;
    }
    vec4 clip = u_proj * vp;
    if (u_snap_on == 1 && clip.w > 0.0) {
        vec2 ndc = clip.xy / clip.w;
        vec2 px = vec2((ndc.x * 0.5 + 0.5) * u_res.x, (0.5 - ndc.y * 0.5) * u_res.y);
        px = floor(px / u_snap + 0.5) * u_snap;
        clip.xy = vec2(px.x / u_res.x * 2.0 - 1.0, 1.0 - px.y / u_res.y * 2.0) * clip.w;
    }
    // Projection is 0..1 depth (glam perspective_rh); GL clips -w..w.
    gl_Position = vec4(clip.xy, clip.z * 2.0 - clip.w, clip.w);
    float w = mix(1.0, clip.w, u_affine);
    v_uvw = vec3(a_uv * w, w);
    v_colw = (u_mode == 1 ? a_raw : a_col) * w;
    v_pos = a_pos;
    v_nrm = a_nrm;
    v_viewz = -vp.z;
}
"#;

const FS: &str = r#"#version 330 core
in vec3 v_uvw;
in vec4 v_colw;
in vec3 v_pos;
in vec3 v_nrm;
in float v_viewz;
uniform int u_mode;
uniform vec2 u_res;
uniform sampler2D u_tex;
uniform int u_has_tex;
uniform int u_dither;
uniform int u_color15;
uniform int u_fog;
uniform vec3 u_fog_color;
uniform vec2 u_fog_range;
uniform vec3 u_bg_top;
uniform vec3 u_bg_bottom;
uniform vec4 u_base;
uniform float u_metal;
uniform float u_rough;
uniform vec3 u_emissive;
uniform sampler2D u_albedo;
uniform sampler2D u_mr;
uniform sampler2D u_normal;
uniform sampler2D u_emit;
uniform sampler2D u_ao;
uniform int u_has_albedo;
uniform int u_has_mr;
uniform int u_has_normal;
uniform int u_has_emit;
uniform int u_has_ao;
uniform vec3 u_eye;
uniform vec3 u_key;
uniform vec3 u_fill;
uniform vec3 u_wire;
uniform vec2 u_vp_origin;
uniform vec2 u_vp_size;
uniform vec2 u_src_res;
out vec4 frag;

const float PI = 3.14159265;
const float DITHER[16] = float[16](-4.0, 0.0, -3.0, 1.0, 2.0, -2.0, 3.0, -1.0, -3.0, 1.0, -4.0, 0.0, 3.0, -1.0, 2.0, -2.0);

vec3 lin(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec3 srgb(vec3 c) { return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }

vec3 environment(vec3 d, float blur) {
    vec3 sky = vec3(0.22, 0.25, 0.32);
    vec3 horizon = vec3(0.55, 0.55, 0.58);
    vec3 ground = vec3(0.05);
    float y = clamp(d.y, -1.0, 1.0);
    vec3 sharp = y >= 0.0 ? mix(horizon, sky, pow(y, 0.6)) : mix(horizon, ground, pow(-y, 0.4));
    vec3 avg = mix(vec3(0.30, 0.31, 0.34), mix(ground, sky, y * 0.5 + 0.5), 0.6);
    return mix(sharp, avg, blur);
}

vec3 aces(vec3 x) { return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0); }

void main() {
    if (u_mode == 4) { // CRT upscale (same recipe as retrocrunch::crt)
        vec2 o = gl_FragCoord.xy - u_vp_origin;
        vec2 uv = clamp(o / u_vp_size, 0.0, 0.9999);
        float sx = uv.x * u_src_res.x;
        float syf = uv.y * u_src_res.y;
        int sy = int(floor(syf));
        float fy = 1.0 - fract(syf); // texture rows run bottom-up; beam profile is symmetric
        int maxx = int(u_src_res.x) - 1;
        vec3 rgb;
        for (int c = 0; c < 3; c++) {
            float shift = float(c - 1) * 0.33;
            float acc = 0.0;
            for (int k = -1; k <= 1; k++) {
                float x = sx + shift + float(k) * 0.6;
                int xi = clamp(int(floor(x)), 0, maxx);
                acc += (k == 0 ? 0.5 : 0.25) * lin(texelFetch(u_tex, ivec2(xi, sy), 0).rgb)[c];
            }
            rgb[c] = acc;
        }
        float beam = 1.0 - 0.45 * (1.0 - sin(PI * fy));
        int phase = int(mod(floor(o.x), 3.0));
        for (int c = 0; c < 3; c++) {
            rgb[c] *= beam * 1.3 * (c == phase ? 1.0 : 0.78);
        }
        vec2 d = uv - 0.5;
        rgb *= 1.0 - 0.35 * dot(d, d);
        frag = vec4(srgb(clamp(rgb, 0.0, 1.0)), 1.0);
        return;
    }
    if (u_mode == 3) { // wireframe pass
        frag = vec4(u_wire, 1.0);
        return;
    }
    if (u_mode == 2) {
        frag = vec4(mix(u_bg_bottom, u_bg_top, gl_FragCoord.y / u_res.y), 1.0);
        return;
    }
    vec2 uv = v_uvw.xy / v_uvw.z;
    vec4 col = v_colw / v_uvw.z;
    vec3 rgb;
    if (u_mode == 1) {
        vec4 tex = u_has_albedo == 1 ? texture(u_albedo, uv) : vec4(1.0);
        vec4 base = u_base * col * vec4(lin(tex.rgb), tex.a);
        if (base.a < 0.5) discard;
        float rough = u_rough, metal = u_metal;
        if (u_has_mr == 1) { vec4 mr = texture(u_mr, uv); rough *= mr.g; metal *= mr.b; }
        rough = clamp(rough, 0.04, 1.0);
        vec3 v = normalize(u_eye - v_pos);
        vec3 n = normalize(length(v_nrm) > 1e-6 ? v_nrm : cross(dFdx(v_pos), dFdy(v_pos)));
        if (u_has_normal == 1) {
            vec3 dp1 = dFdx(v_pos), dp2 = dFdy(v_pos);
            vec2 du1 = dFdx(uv), du2 = dFdy(uv);
            vec3 t = dp1 * du2.y - dp2 * du1.y;
            vec3 b = dp2 * du1.x - dp1 * du2.x;
            t = normalize(t - n * dot(n, t));
            b = dot(b, cross(n, t)) < 0.0 ? -cross(n, t) : cross(n, t);
            vec3 tn = texture(u_normal, uv).rgb * 2.0 - 1.0;
            n = normalize(t * tn.x + b * tn.y + n * tn.z);
        }
        if (dot(n, v) < 0.0) n = -n;
        vec3 albedo = base.rgb;
        vec3 f0 = mix(vec3(0.04), albedo, metal);
        float nv = max(dot(n, v), 1e-4);
        float a = rough * rough, a2 = a * a, k = (rough + 1.0) * (rough + 1.0) / 8.0;
        vec3 c = vec3(0.0);
        vec3 dirs[2] = vec3[2](u_key, u_fill);
        float power[2] = float[2](2.6, 0.7);
        for (int i = 0; i < 2; i++) {
            float nl = dot(n, dirs[i]);
            if (nl <= 0.0) continue;
            vec3 h = normalize(dirs[i] + v);
            float nh = max(dot(n, h), 0.0);
            float d = a2 / max(PI * pow(nh * nh * (a2 - 1.0) + 1.0, 2.0), 1e-6);
            vec3 f = f0 + (1.0 - f0) * pow(1.0 - max(dot(v, h), 0.0), 5.0);
            float g = (nl / (nl * (1.0 - k) + k)) * (nv / (nv * (1.0 - k) + k));
            vec3 spec = f * (d * g / max(4.0 * nl * nv, 1e-4));
            vec3 kd = (1.0 - f) * (1.0 - metal);
            c += (kd * albedo / PI + spec) * nl * power[i] * PI;
        }
        float ao = u_has_ao == 1 ? texture(u_ao, uv).r : 1.0;
        vec3 fr = f0 + (1.0 - f0) * pow(1.0 - nv, 5.0);
        vec3 r = normalize(reflect(-v, n));
        c += (albedo * (1.0 - metal) * environment(n, 1.0) * (1.0 - fr) + environment(r, rough) * fr * (1.0 - rough * 0.5)) * ao;
        c += u_has_emit == 1 ? lin(texture(u_emit, uv).rgb) * u_emissive : u_emissive;
        rgb = srgb(aces(c)) * 255.0;
    } else {
        rgb = col.rgb * 255.0;
        if (u_has_tex == 1) {
            vec4 t = texture(u_tex, uv);
            if (t.a < 0.5) discard;
            rgb *= t.rgb;
        }
    }
    if (u_fog == 1) {
        float f = clamp((v_viewz - u_fog_range.x) / max(u_fog_range.y - u_fog_range.x, 1e-3), 0.0, 1.0);
        rgb = mix(rgb, u_fog_color, f);
    }
    if (u_dither == 1) {
        ivec2 p = ivec2(gl_FragCoord.xy);
        // Same table orientation as the CPU path (row 0 at the top of the frame).
        int y = int(u_res.y) - 1 - p.y;
        rgb += DITHER[(y & 3) * 4 + (p.x & 3)];
    }
    rgb = clamp(floor(rgb), 0.0, 255.0);
    if (u_color15 == 1) rgb = floor(rgb / 8.0) * 8.0;
    frag = vec4(rgb / 255.0, 1.0);
}
"#;

/// Geometry for one draw call, interleaved: pos(3) uv(2) col(4) nrm(3) raw(4).
pub struct MeshUpload {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub texture: Option<usize>,
    pub material: Option<usize>,
    pub double_sided: bool,
}

pub struct MaterialUpload {
    pub base: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_factor: [f32; 3],
    pub albedo: Option<RgbaImage>,
    pub metal_rough: Option<RgbaImage>,
    pub normal: Option<RgbaImage>,
    pub emissive: Option<RgbaImage>,
    pub ao: Option<RgbaImage>,
}

pub struct TextureUpload {
    /// Flat (PSX view) textures, sampled with nearest filtering.
    pub flat: Vec<Option<RgbaImage>>,
    pub materials: Vec<MaterialUpload>,
}

/// Per-frame draw parameters.
#[derive(Clone, Copy)]
pub struct DrawParams {
    pub view: Mat4,
    pub proj: Mat4,
    pub res: (u32, u32),
    pub reference: bool,
    pub snap: Option<f32>,
    pub affine: f32,
    pub dither: bool,
    pub color15: bool,
    pub textured: bool,
    pub cull_back: bool,
    pub fog: Option<([f32; 3], f32, f32)>,
    pub bg_top: [f32; 3],
    pub bg_bottom: [f32; 3],
    /// Edge colour for the wireframe overlay (0..1), None = off.
    pub wireframe: Option<[f32; 3]>,
    /// Fixed-point view-space grid (world units), 0 = off.
    pub gte_unit: f32,
    /// Upscale through the CRT pass instead of a plain nearest blit.
    pub crt: bool,
}

struct GpuMesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    count: i32,
    texture: Option<usize>,
    material: Option<usize>,
    double_sided: bool,
}

struct GpuMaterial {
    base: [f32; 4],
    metallic: f32,
    roughness: f32,
    emissive_factor: [f32; 3],
    maps: [Option<glow::Texture>; 5],
}

#[derive(Default)]
pub struct GpuState {
    program: Option<glow::Program>,
    bg_vao: Option<glow::VertexArray>,
    meshes: Vec<GpuMesh>,
    flat: Vec<Option<glow::Texture>>,
    materials: Vec<GpuMaterial>,
    target: Option<(
        glow::Framebuffer,
        glow::Texture,
        glow::Renderbuffer,
        u32,
        u32,
    )>,
    pending_meshes: Option<Vec<MeshUpload>>,
    pending_textures: Option<TextureUpload>,
    pub key: Option<(u64, usize)>,
    pub error: Option<String>,
}

pub type SharedGpu = Arc<Mutex<GpuState>>;

impl GpuState {
    pub fn queue(&mut self, meshes: Vec<MeshUpload>, textures: TextureUpload) {
        self.pending_meshes = Some(meshes);
        self.pending_textures = Some(textures);
    }

    unsafe fn compile(gl: &glow::Context) -> Result<glow::Program, String> {
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
                let sh = gl.create_shader(kind)?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!(
                        "shader compile failed: {}",
                        gl.get_shader_info_log(sh)
                    ));
                }
                gl.attach_shader(program, sh);
                shaders.push(sh);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!(
                    "shader link failed: {}",
                    gl.get_program_info_log(program)
                ));
            }
            for sh in shaders {
                gl.detach_shader(program, sh);
                gl.delete_shader(sh);
            }
            Ok(program)
        }
    }

    unsafe fn upload_texture(
        gl: &glow::Context,
        img: &RgbaImage,
        nearest: bool,
    ) -> Result<glow::Texture, String> {
        unsafe {
            let t = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                img.width() as i32,
                img.height() as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(img.as_raw()),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
            if nearest {
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
            } else {
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR_MIPMAP_LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
            }
            Ok(t)
        }
    }

    unsafe fn apply_uploads(&mut self, gl: &glow::Context) -> Result<(), String> {
        unsafe {
            if let Some(meshes) = self.pending_meshes.take() {
                for m in self.meshes.drain(..) {
                    gl.delete_vertex_array(m.vao);
                    gl.delete_buffer(m.vbo);
                    gl.delete_buffer(m.ebo);
                }
                for m in meshes.into_iter().filter(|m| !m.indices.is_empty()) {
                    let vao = gl.create_vertex_array()?;
                    gl.bind_vertex_array(Some(vao));
                    let vbo = gl.create_buffer()?;
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        bytemuck_f32(&m.vertices),
                        glow::STATIC_DRAW,
                    );
                    let ebo = gl.create_buffer()?;
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
                    gl.buffer_data_u8_slice(
                        glow::ELEMENT_ARRAY_BUFFER,
                        bytemuck_u32(&m.indices),
                        glow::STATIC_DRAW,
                    );
                    let stride = 16 * 4;
                    for (loc, size, offset) in [
                        (0u32, 3i32, 0i32),
                        (1, 2, 3),
                        (2, 4, 5),
                        (3, 3, 9),
                        (4, 4, 12),
                    ] {
                        gl.enable_vertex_attrib_array(loc);
                        gl.vertex_attrib_pointer_f32(
                            loc,
                            size,
                            glow::FLOAT,
                            false,
                            stride,
                            offset * 4,
                        );
                    }
                    gl.bind_vertex_array(None);
                    self.meshes.push(GpuMesh {
                        vao,
                        vbo,
                        ebo,
                        count: m.indices.len() as i32,
                        texture: m.texture,
                        material: m.material,
                        double_sided: m.double_sided,
                    });
                }
            }
            if let Some(tex) = self.pending_textures.take() {
                for t in self.flat.drain(..).flatten() {
                    gl.delete_texture(t);
                }
                for m in self.materials.drain(..) {
                    for t in m.maps.into_iter().flatten() {
                        gl.delete_texture(t);
                    }
                }
                for img in tex.flat {
                    self.flat.push(match img {
                        Some(i) => Some(Self::upload_texture(gl, &i, true)?),
                        None => None,
                    });
                }
                for m in tex.materials {
                    let mut maps: [Option<glow::Texture>; 5] = [None; 5];
                    for (slot, img) in [&m.albedo, &m.metal_rough, &m.normal, &m.emissive, &m.ao]
                        .into_iter()
                        .enumerate()
                    {
                        if let Some(i) = img {
                            maps[slot] = Some(Self::upload_texture(gl, i, false)?);
                        }
                    }
                    self.materials.push(GpuMaterial {
                        base: m.base,
                        metallic: m.metallic,
                        roughness: m.roughness,
                        emissive_factor: m.emissive_factor,
                        maps,
                    });
                }
            }
            Ok(())
        }
    }

    unsafe fn ensure_target(
        &mut self,
        gl: &glow::Context,
        w: u32,
        h: u32,
    ) -> Result<glow::Framebuffer, String> {
        unsafe {
            if let Some((fb, _, _, tw, th)) = self.target
                && (tw, th) == (w, h)
            {
                return Ok(fb);
            }
            if let Some((fb, tex, rb, _, _)) = self.target.take() {
                gl.delete_framebuffer(fb);
                gl.delete_texture(tex);
                gl.delete_renderbuffer(rb);
            }
            let fb = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            let rb = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
            gl.renderbuffer_storage(
                glow::RENDERBUFFER,
                glow::DEPTH_COMPONENT24,
                w as i32,
                h as i32,
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(rb),
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("offscreen framebuffer incomplete".into());
            }
            self.target = Some((fb, tex, rb, w, h));
            Ok(fb)
        }
    }

    /// Render into the offscreen target and blit to the egui framebuffer.
    pub fn paint(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        egui_fbo: Option<glow::Framebuffer>,
        p: &DrawParams,
    ) {
        if self.error.is_some() {
            return;
        }
        // SAFETY: called on the GL thread inside egui_glow's paint callback with a current
        // context; every object used here was created by this context.
        if let Err(e) = unsafe { self.paint_inner(gl, info, egui_fbo, p) } {
            self.error = Some(e);
        }
    }

    unsafe fn paint_inner(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        egui_fbo: Option<glow::Framebuffer>,
        p: &DrawParams,
    ) -> Result<(), String> {
        unsafe {
            if self.program.is_none() {
                self.program = Some(Self::compile(gl)?);
                let vao = gl.create_vertex_array()?;
                gl.bind_vertex_array(Some(vao));
                let vbo = gl.create_buffer()?;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                let tri: [f32; 48] = {
                    let mut v = [0.0f32; 48];
                    for (k, (x, y)) in [(-1.0f32, -1.0f32), (3.0, -1.0), (-1.0, 3.0)]
                        .iter()
                        .enumerate()
                    {
                        v[k * 16] = *x;
                        v[k * 16 + 1] = *y;
                    }
                    v
                };
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck_f32(&tri), glow::STATIC_DRAW);
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 64, 0);
                self.bg_vao = Some(vao);
            }
            self.apply_uploads(gl)?;
            let prog = self.program.ok_or("no program")?;
            let (w, h) = (p.res.0.max(1), p.res.1.max(1));
            let fb = self.ensure_target(gl, w, h)?;

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.viewport(0, 0, w as i32, h as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.use_program(Some(prog));
            let u = |name: &str| gl.get_uniform_location(prog, name);
            gl.uniform_2_f32(u("u_res").as_ref(), w as f32, h as f32);

            // Background gradient.
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.uniform_1_i32(u("u_mode").as_ref(), 2);
            gl.uniform_3_f32(
                u("u_bg_top").as_ref(),
                p.bg_top[0],
                p.bg_top[1],
                p.bg_top[2],
            );
            gl.uniform_3_f32(
                u("u_bg_bottom").as_ref(),
                p.bg_bottom[0],
                p.bg_bottom[1],
                p.bg_bottom[2],
            );
            gl.bind_vertex_array(self.bg_vao);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // Scene.
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.uniform_matrix_4_f32_slice(u("u_view").as_ref(), false, &p.view.to_cols_array());
            gl.uniform_matrix_4_f32_slice(u("u_proj").as_ref(), false, &p.proj.to_cols_array());
            gl.uniform_1_i32(u("u_snap_on").as_ref(), p.snap.is_some() as i32);
            gl.uniform_1_f32(u("u_snap").as_ref(), p.snap.unwrap_or(1.0).max(1.0));
            gl.uniform_1_f32(u("u_affine").as_ref(), p.affine);
            gl.uniform_1_f32(u("u_gte").as_ref(), p.gte_unit);
            gl.uniform_1_i32(u("u_dither").as_ref(), p.dither as i32);
            gl.uniform_1_i32(u("u_color15").as_ref(), p.color15 as i32);
            match p.fog {
                Some((c, start, end)) => {
                    gl.uniform_1_i32(u("u_fog").as_ref(), 1);
                    gl.uniform_3_f32(u("u_fog_color").as_ref(), c[0], c[1], c[2]);
                    gl.uniform_2_f32(u("u_fog_range").as_ref(), start, end);
                }
                None => gl.uniform_1_i32(u("u_fog").as_ref(), 0),
            }
            let inv = p.view.inverse();
            let (right, up, back) = (
                inv.x_axis.truncate(),
                inv.y_axis.truncate(),
                inv.z_axis.truncate(),
            );
            let eye = inv.w_axis.truncate();
            let key = (right * 0.55 + up * 0.75 + back * 0.6).normalize();
            let fill = (-right * 0.7 + up * 0.1 + back * 0.4).normalize();
            let set3 = |name: &str, v: Vec3| gl.uniform_3_f32(u(name).as_ref(), v.x, v.y, v.z);
            set3("u_eye", eye);
            set3("u_key", key);
            set3("u_fill", fill);
            for (i, name) in ["u_tex", "u_albedo", "u_mr", "u_normal", "u_emit", "u_ao"]
                .into_iter()
                .enumerate()
            {
                gl.uniform_1_i32(u(name).as_ref(), i as i32);
            }
            for m in &self.meshes {
                let mat = m
                    .material
                    .and_then(|i| self.materials.get(i))
                    .filter(|_| p.reference);
                // Pieces without a material keep flat shading in the reference view, as on the CPU.
                gl.uniform_1_i32(u("u_mode").as_ref(), mat.is_some() as i32);
                if p.cull_back && !m.double_sided {
                    gl.enable(glow::CULL_FACE);
                    gl.cull_face(glow::BACK);
                    gl.front_face(glow::CCW);
                } else {
                    gl.disable(glow::CULL_FACE);
                }
                let flat = m
                    .texture
                    .filter(|_| p.textured)
                    .and_then(|t| self.flat.get(t).copied().flatten());
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, flat);
                gl.uniform_1_i32(u("u_has_tex").as_ref(), flat.is_some() as i32);
                let (base, metal, rough, emit) = mat.map_or(([1.0; 4], 0.0, 1.0, [0.0; 3]), |m| {
                    (m.base, m.metallic, m.roughness, m.emissive_factor)
                });
                gl.uniform_4_f32(u("u_base").as_ref(), base[0], base[1], base[2], base[3]);
                gl.uniform_1_f32(u("u_metal").as_ref(), metal);
                gl.uniform_1_f32(u("u_rough").as_ref(), rough);
                gl.uniform_3_f32(u("u_emissive").as_ref(), emit[0], emit[1], emit[2]);
                for (slot, name) in [
                    "u_has_albedo",
                    "u_has_mr",
                    "u_has_normal",
                    "u_has_emit",
                    "u_has_ao",
                ]
                .into_iter()
                .enumerate()
                {
                    let t = mat
                        .and_then(|m| m.maps[slot])
                        .filter(|_| slot != 0 || p.textured);
                    gl.active_texture(glow::TEXTURE1 + slot as u32);
                    gl.bind_texture(glow::TEXTURE_2D, t);
                    gl.uniform_1_i32(u(name).as_ref(), t.is_some() as i32);
                }
                gl.bind_vertex_array(Some(m.vao));
                gl.draw_elements(glow::TRIANGLES, m.count, glow::UNSIGNED_INT, 0);
            }
            // Wireframe: the same meshes as lines, pulled slightly towards the camera so
            // they win the depth test against their own faces (hidden lines stay hidden).
            if let Some(w) = p.wireframe {
                gl.uniform_1_i32(u("u_mode").as_ref(), 3);
                gl.uniform_3_f32(u("u_wire").as_ref(), w[0], w[1], w[2]);
                gl.depth_func(glow::LEQUAL);
                gl.enable(glow::POLYGON_OFFSET_LINE);
                gl.polygon_offset(-1.0, -1.0);
                gl.polygon_mode(glow::FRONT_AND_BACK, glow::LINE);
                for m in &self.meshes {
                    if p.cull_back && !m.double_sided {
                        gl.enable(glow::CULL_FACE);
                    } else {
                        gl.disable(glow::CULL_FACE);
                    }
                    gl.bind_vertex_array(Some(m.vao));
                    gl.draw_elements(glow::TRIANGLES, m.count, glow::UNSIGNED_INT, 0);
                }
                gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
                gl.disable(glow::POLYGON_OFFSET_LINE);
                gl.depth_func(glow::LESS);
            }
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE0);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);

            // Upscale into egui's framebuffer: CRT pass, or plain nearest filtering.
            let vp = info.viewport_in_pixels();
            let clip = info.clip_rect_in_pixels();
            if p.crt {
                let tex = self.target.map(|t| t.1);
                gl.bind_framebuffer(glow::FRAMEBUFFER, egui_fbo);
                gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(
                    clip.left_px,
                    clip.from_bottom_px,
                    clip.width_px,
                    clip.height_px,
                );
                gl.use_program(Some(prog));
                gl.uniform_1_i32(u("u_mode").as_ref(), 4);
                gl.uniform_2_f32(
                    u("u_vp_origin").as_ref(),
                    vp.left_px as f32,
                    vp.from_bottom_px as f32,
                );
                gl.uniform_2_f32(
                    u("u_vp_size").as_ref(),
                    vp.width_px as f32,
                    vp.height_px as f32,
                );
                gl.uniform_2_f32(u("u_src_res").as_ref(), w as f32, h as f32);
                gl.uniform_1_i32(u("u_tex").as_ref(), 0);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, tex);
                gl.bind_vertex_array(self.bg_vao);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                gl.bind_vertex_array(None);
                return Ok(());
            }
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fb));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, egui_fbo);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.blit_framebuffer(
                0,
                0,
                w as i32,
                h as i32,
                vp.left_px,
                vp.from_bottom_px,
                vp.left_px + vp.width_px,
                vp.from_bottom_px + vp.height_px,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, egui_fbo);
            Ok(())
        }
    }
}

fn bytemuck_f32(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding or invalid bit patterns; the slice covers exactly the
    // same memory as `v` and lives as long as it.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

fn bytemuck_u32(v: &[u32]) -> &[u8] {
    // SAFETY: as above, for u32.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}
