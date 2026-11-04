//! A small CPU rasterizer that reproduces PlayStation-style rendering:
//! integer vertex snapping (jitter), affine texture mapping, nearest sampling,
//! ordering-table depth sorting (no z-buffer), GPU dither and 15-bit colour.
//! Also used for ID/depth renders (hidden-face removal, ambient occlusion).

use crate::palette::{linear_to_srgb, srgb_to_linear};
use crate::texture::PS1_DITHER;
use glam::{Mat4, Vec2, Vec3, Vec4};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};

pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub color: Vec<[u8; 4]>,
    pub depth: Vec<f32>,
    /// Triangle id + 1 per pixel (0 = background). Only filled when requested.
    pub ids: Vec<u32>,
}

impl Frame {
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w * h) as usize;
        Self {
            w,
            h,
            color: vec![[0, 0, 0, 255]; n],
            depth: vec![f32::INFINITY; n],
            ids: vec![0; n],
        }
    }

    pub fn clear(&mut self, c: [u8; 4]) {
        self.color.fill(c);
        self.depth.fill(f32::INFINITY);
        self.ids.fill(0);
    }

    /// Vertical gradient background, like many PS1 menus.
    pub fn clear_gradient(&mut self, top: [u8; 3], bottom: [u8; 3]) {
        for y in 0..self.h {
            let t = y as f32 / self.h.max(1) as f32;
            let c = [
                (top[0] as f32 * (1.0 - t) + bottom[0] as f32 * t) as u8,
                (top[1] as f32 * (1.0 - t) + bottom[1] as f32 * t) as u8,
                (top[2] as f32 * (1.0 - t) + bottom[2] as f32 * t) as u8,
                255,
            ];
            let row = (y * self.w) as usize;
            self.color[row..row + self.w as usize].fill(c);
        }
        self.depth.fill(f32::INFINITY);
        self.ids.fill(0);
    }

    pub fn to_rgba(&self) -> RgbaImage {
        RgbaImage::from_raw(
            self.w,
            self.h,
            self.color.iter().flatten().copied().collect(),
        )
        .expect("frame size")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RasterOptions {
    pub affine: bool,
    /// 0 = perspective-correct, 1 = full PS1 affine warping (only used when `affine`).
    #[serde(default = "one")]
    pub affine_weight: f32,
    pub vertex_snap: bool,
    /// Snap granularity in pixels (1 = integer coordinates, higher = wobblier).
    pub snap_size: f32,
    pub dither: bool,
    pub color15: bool,
    pub zbuffer: bool,
    pub cull_back: bool,
    pub write_ids: bool,
    pub textured: bool,
    /// Physically based shading for draw calls that carry a `PbrMaterial` (reference view).
    #[serde(default)]
    pub pbr: bool,
    pub fog: Option<FogSettings>,
    /// Draw triangle edges (hidden-line: only visible edges) in this colour.
    #[serde(default)]
    pub wireframe: Option<[u8; 3]>,
    /// Fixed-point (GTE) precision: view-space positions round to this grid (world
    /// units) before projection, so vertices shift against each other as the camera
    /// moves. 0 = off.
    #[serde(default)]
    pub gte_unit: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FogSettings {
    pub color: [u8; 3],
    pub start: f32,
    pub end: f32,
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            affine: true,
            affine_weight: 1.0,
            vertex_snap: true,
            snap_size: 1.0,
            dither: true,
            color15: true,
            zbuffer: true,
            cull_back: true,
            write_ids: false,
            textured: true,
            pbr: false,
            fog: None,
            wireframe: None,
            gte_unit: 0.0,
        }
    }
}

impl RasterOptions {
    /// Plain modern rendering (for ID buffers and "original" comparison views).
    pub fn clean() -> Self {
        Self {
            affine: false,
            affine_weight: 1.0,
            vertex_snap: false,
            snap_size: 1.0,
            dither: false,
            color15: false,
            ..Default::default()
        }
    }

    /// Modern reference rendering: perspective-correct, z-buffered, physically shaded.
    pub fn reference() -> Self {
        Self {
            pbr: true,
            ..Self::clean()
        }
    }
}

/// PBR inputs for the reference ("Original") view. Built from a `mesh::Material`.
#[derive(Clone, Copy)]
pub struct PbrMaterial<'a> {
    pub base: Vec4,
    pub albedo: Option<&'a RgbaImage>,
    pub metallic: f32,
    pub roughness: f32,
    pub metal_rough: Option<&'a RgbaImage>,
    pub normal: Option<&'a RgbaImage>,
    pub emissive: Option<&'a RgbaImage>,
    pub emissive_factor: Vec3,
    pub ao: Option<&'a GrayImage>,
}

impl<'a> PbrMaterial<'a> {
    pub fn from_material(m: &'a crate::mesh::Material) -> Self {
        Self {
            base: Vec4::from(m.base_color),
            albedo: m.albedo.as_ref(),
            metallic: m.metallic,
            roughness: m.roughness,
            metal_rough: m.metal_rough.as_ref(),
            normal: m.normal.as_ref(),
            emissive: m.emissive.as_ref(),
            emissive_factor: Vec3::from(m.emissive_factor),
            ao: m.ao.as_ref(),
        }
    }
}

pub struct DrawCall<'a> {
    pub positions: &'a [Vec3],
    pub uvs: &'a [Vec2],
    pub colors: &'a [[f32; 4]],
    pub indices: &'a [u32],
    pub texture: Option<&'a RgbaImage>,
    /// Added to the local triangle index when writing ids.
    pub id_base: u32,
    pub double_sided: bool,
    /// Per-vertex normals (only needed for PBR shading).
    pub normals: &'a [Vec3],
    /// Use physically based shading for this call when `RasterOptions::pbr` is on.
    pub pbr: Option<PbrMaterial<'a>>,
}

#[derive(Clone, Copy)]
struct ClipVert {
    clip: Vec4,
    uv: Vec2,
    col: Vec4,
    view_z: f32,
    pos: Vec3,
    nrm: Vec3,
}

#[derive(Clone, Copy)]
struct ScreenVert {
    x: f32,
    y: f32,
    z: f32,
    inv_w: f32,
    uv: Vec2,
    col: Vec4,
    view_z: f32,
    pos: Vec3,
    nrm: Vec3,
}

struct Tri<'a> {
    v: [ScreenVert; 3],
    tex: Option<&'a RgbaImage>,
    id: u32,
    sort_key: f32,
    pbr: Option<(PbrMaterial<'a>, Vec3, Vec3)>,
}

fn lerp_cv(a: &ClipVert, b: &ClipVert, t: f32) -> ClipVert {
    ClipVert {
        clip: a.clip.lerp(b.clip, t),
        uv: a.uv.lerp(b.uv, t),
        col: a.col.lerp(b.col, t),
        view_z: a.view_z + (b.view_z - a.view_z) * t,
        pos: a.pos.lerp(b.pos, t),
        nrm: a.nrm.lerp(b.nrm, t),
    }
}

/// Clip a polygon against the near plane (z >= 0 in 0..1 depth clip space).
fn clip_near(poly: &[ClipVert]) -> Vec<ClipVert> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = &poly[i];
        let b = &poly[(i + 1) % poly.len()];
        let da = a.clip.z;
        let db = b.clip.z;
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(lerp_cv(a, b, t));
        }
    }
    out
}

/// Camera-relative lighting rig for PBR shading.
#[derive(Clone, Copy)]
struct Lights {
    eye: Vec3,
    key: Vec3,
    fill: Vec3,
}

/// Rows `y0..y1` of a frame, borrowed mutably so bands can render in parallel.
struct Band<'a> {
    w: u32,
    y0: i32,
    y1: i32,
    color: &'a mut [[u8; 4]],
    depth: &'a mut [f32],
    ids: &'a mut [u32],
}

/// Render several draw calls into `frame` with a single view-projection.
/// `view` is needed separately for fog distance and PBR lighting.
pub fn render(frame: &mut Frame, view: Mat4, proj: Mat4, calls: &[DrawCall], o: &RasterOptions) {
    let threads = if frame.w * frame.h >= 160_000 {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(16)
    } else {
        1
    };
    render_with_threads(frame, view, proj, calls, o, threads);
}

fn render_with_threads(
    frame: &mut Frame,
    view: Mat4,
    proj: Mat4,
    calls: &[DrawCall],
    o: &RasterOptions,
    threads: usize,
) {
    let vp = proj * view;
    let (fw, fh) = (frame.w as f32, frame.h as f32);
    let inv_view = view.inverse();
    let (right, up, back) = (
        inv_view.x_axis.truncate(),
        inv_view.y_axis.truncate(),
        inv_view.z_axis.truncate(),
    );
    let lights = Lights {
        eye: inv_view.w_axis.truncate(),
        key: (right * 0.55 + up * 0.75 + back * 0.6).normalize(),
        fill: (-right * 0.7 + up * 0.1 + back * 0.4).normalize(),
    };
    let mut tris: Vec<Tri> = Vec::new();
    for call in calls {
        let has_uv = call.uvs.len() == call.positions.len();
        let has_col = call.colors.len() == call.positions.len();
        let pbr = call.pbr.filter(|_| o.pbr);
        let has_n = call.normals.len() == call.positions.len();
        for (ti, t) in call.indices.chunks_exact(3).enumerate() {
            let cv: [ClipVert; 3] = std::array::from_fn(|k| {
                let i = t[k] as usize;
                let p = call.positions[i];
                ClipVert {
                    clip: if o.gte_unit > 0.0 {
                        let q = view.transform_point3(p);
                        proj * ((q / o.gte_unit).round() * o.gte_unit).extend(1.0)
                    } else {
                        vp * p.extend(1.0)
                    },
                    uv: if has_uv { call.uvs[i] } else { Vec2::ZERO },
                    col: if has_col {
                        Vec4::from(call.colors[i])
                    } else {
                        Vec4::ONE
                    },
                    view_z: -(view.transform_point3(p)).z,
                    pos: p,
                    nrm: if has_n { call.normals[i] } else { Vec3::ZERO },
                }
            });
            // Trivial reject when all three are outside the same frustum side.
            let outside = |f: &dyn Fn(&Vec4) -> bool| cv.iter().all(|v| f(&v.clip));
            if outside(&|c| c.x > c.w)
                || outside(&|c| c.x < -c.w)
                || outside(&|c| c.y > c.w)
                || outside(&|c| c.y < -c.w)
                || outside(&|c| c.z < 0.0)
                || outside(&|c| c.z > c.w)
            {
                continue;
            }
            // Tangent frame for normal mapping (per triangle, from positions and UVs).
            let frame_tb = pbr.map(|m| {
                let (e1, e2) = (cv[1].pos - cv[0].pos, cv[2].pos - cv[0].pos);
                let (d1, d2) = (cv[1].uv - cv[0].uv, cv[2].uv - cv[0].uv);
                let r = d1.x * d2.y - d2.x * d1.y;
                let (tt, bb) = if r.abs() > 1e-12 {
                    ((e1 * d2.y - e2 * d1.y) / r, (e2 * d1.x - e1 * d2.x) / r)
                } else {
                    let n = e1.cross(e2).normalize_or(Vec3::Y);
                    n.any_orthonormal_pair()
                };
                (m, tt.normalize_or_zero(), bb.normalize_or_zero())
            });
            let poly = if cv.iter().any(|v| v.clip.z < 0.0) {
                clip_near(&cv)
            } else {
                cv.to_vec()
            };
            if poly.len() < 3 {
                continue;
            }
            let sv: Vec<ScreenVert> = poly
                .iter()
                .map(|v| {
                    let inv_w = 1.0 / v.clip.w.max(1e-6);
                    let ndc = v.clip.truncate() * inv_w;
                    let mut x = (ndc.x * 0.5 + 0.5) * fw;
                    let mut y = (0.5 - ndc.y * 0.5) * fh;
                    if o.vertex_snap {
                        let s = o.snap_size.max(1.0);
                        x = (x / s).round() * s;
                        y = (y / s).round() * s;
                    }
                    ScreenVert {
                        x,
                        y,
                        z: ndc.z,
                        inv_w,
                        uv: v.uv,
                        col: v.col,
                        view_z: v.view_z,
                        pos: v.pos,
                        nrm: v.nrm,
                    }
                })
                .collect();
            for k in 1..sv.len() - 1 {
                let v = [sv[0], sv[k], sv[k + 1]];
                let area =
                    (v[1].x - v[0].x) * (v[2].y - v[0].y) - (v[1].y - v[0].y) * (v[2].x - v[0].x);
                // CCW (front) triangles have negative area in y-down screen space.
                if area == 0.0 || (o.cull_back && !call.double_sided && area > 0.0) {
                    continue;
                }
                let sort_key = (v[0].z + v[1].z + v[2].z) / 3.0;
                tris.push(Tri {
                    v,
                    tex: if o.textured { call.texture } else { None },
                    id: call.id_base + ti as u32 + 1,
                    sort_key,
                    pbr: frame_tb,
                });
            }
        }
    }
    if !o.zbuffer {
        // PS1 ordering table: far to near by average depth.
        tris.sort_by(|a, b| b.sort_key.total_cmp(&a.sort_key));
    }
    // Big frames (native resolution, PBR) render in horizontal bands on all cores.
    let w = frame.w as usize;
    let threads = threads.max(1);
    let rows = (frame.h as usize).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let parts = frame
            .color
            .chunks_mut(rows * w)
            .zip(frame.depth.chunks_mut(rows * w))
            .zip(frame.ids.chunks_mut(rows * w));
        for (bi, ((color, depth), ids)) in parts.enumerate() {
            let y0 = (bi * rows) as i32;
            let mut band = Band {
                w: frame.w,
                y0,
                y1: y0 + (color.len() / w) as i32,
                color,
                depth,
                ids,
            };
            let tris = &tris;
            let mut work = move || {
                for t in tris {
                    raster_tri(&mut band, t, o, &lights);
                }
            };
            if threads == 1 {
                work();
            } else {
                scope.spawn(work);
            }
        }
    });
}

fn bilinear(img: &RgbaImage, uv: Vec2) -> Vec4 {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x = (uv.x - uv.x.floor()) * w as f32 - 0.5;
    let y = (uv.y - uv.y.floor()) * h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let px = |xi: i32, yi: i32| {
        let p = img
            .get_pixel(xi.rem_euclid(w) as u32, yi.rem_euclid(h) as u32)
            .0;
        Vec4::new(p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32) / 255.0
    };
    let (x0, y0) = (x0 as i32, y0 as i32);
    px(x0, y0)
        .lerp(px(x0 + 1, y0), fx)
        .lerp(px(x0, y0 + 1).lerp(px(x0 + 1, y0 + 1), fx), fy)
}

/// Sky/ground gradient used for ambient light and reflections.
fn environment(d: Vec3, blur: f32) -> Vec3 {
    // A dim studio: bright enough for diffuse, dark enough that mirror-like metals and
    // glass read dark with highlights, like typical asset viewers.
    let sky = Vec3::new(0.22, 0.25, 0.32);
    let horizon = Vec3::new(0.55, 0.55, 0.58);
    let ground = Vec3::new(0.05, 0.05, 0.05);
    let y = d.y.clamp(-1.0, 1.0);
    // Rough surfaces see a smoother, darker average of the environment.
    let sharp = if y >= 0.0 {
        horizon.lerp(sky, y.powf(0.6))
    } else {
        horizon.lerp(ground, (-y).powf(0.4))
    };
    let avg = Vec3::new(0.30, 0.31, 0.34).lerp(ground.lerp(sky, y * 0.5 + 0.5), 0.6);
    sharp.lerp(avg, blur)
}

/// Cook-Torrance GGX with a hemisphere environment; returns linear RGB and alpha.
fn shade_pbr(
    m: &PbrMaterial,
    uv: Vec2,
    vcol: Vec4,
    pos: Vec3,
    nrm: Vec3,
    tb: (Vec3, Vec3),
    l: &Lights,
) -> Option<Vec4> {
    let lin = |c: Vec4| {
        Vec4::new(
            srgb_to_linear(c.x),
            srgb_to_linear(c.y),
            srgb_to_linear(c.z),
            c.w,
        )
    };
    let tex = m.albedo.map_or(Vec4::ONE, |t| lin(bilinear(t, uv)));
    let base = m.base * vcol * tex;
    if base.w < 0.5 {
        return None;
    }
    let albedo = base.truncate();
    let (mut rough, mut metal) = (m.roughness, m.metallic);
    if let Some(mr) = m.metal_rough {
        let s = bilinear(mr, uv);
        rough *= s.y;
        metal *= s.z;
    }
    let rough = rough.clamp(0.04, 1.0);
    let v = (l.eye - pos).normalize_or(Vec3::Z);
    let mut n = nrm.normalize_or(Vec3::Y);
    if let Some(nt) = m.normal {
        let t = bilinear(nt, uv);
        let tn = Vec3::new(t.x * 2.0 - 1.0, t.y * 2.0 - 1.0, t.z * 2.0 - 1.0);
        let (tt, bb) = tb;
        // Gram-Schmidt against the interpolated normal.
        let tt = (tt - n * n.dot(tt)).normalize_or_zero();
        let bb = if bb.dot(n.cross(tt)) < 0.0 {
            -n.cross(tt)
        } else {
            n.cross(tt)
        };
        n = (tt * tn.x + bb * tn.y + n * tn.z).normalize_or(n);
    }
    if n.dot(v) < 0.0 {
        n = -n; // double-sided look for open meshes
    }
    let f0 = Vec3::splat(0.04).lerp(albedo, metal);
    let nv = n.dot(v).max(1e-4);
    let a = rough * rough;
    let a2 = a * a;
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let g1 = |x: f32| x / (x * (1.0 - k) + k);
    let mut color = Vec3::ZERO;
    for (dir, intensity) in [(l.key, 2.6f32), (l.fill, 0.7)] {
        let nl = n.dot(dir);
        if nl <= 0.0 {
            continue;
        }
        let h = (dir + v).normalize();
        let nh = n.dot(h).max(0.0);
        let d = a2 / (std::f32::consts::PI * (nh * nh * (a2 - 1.0) + 1.0).powi(2)).max(1e-6);
        let f = f0 + (Vec3::ONE - f0) * (1.0 - v.dot(h).max(0.0)).powi(5);
        let spec = f * (d * g1(nl) * g1(nv) / (4.0 * nl * nv).max(1e-4));
        let kd = (Vec3::ONE - f) * (1.0 - metal);
        color +=
            (kd * albedo / std::f32::consts::PI + spec) * nl * intensity * std::f32::consts::PI;
    }
    let ao = m.ao.map_or(1.0, |ao| {
        let (w, h) = (ao.width() as f32, ao.height() as f32);
        let x = ((uv.x - uv.x.floor()) * w) as u32;
        let y = ((uv.y - uv.y.floor()) * h) as u32;
        ao.get_pixel(x.min(ao.width() - 1), y.min(ao.height() - 1))
            .0[0] as f32
            / 255.0
    });
    let fr = f0 + (Vec3::ONE.max(Vec3::splat(1.0 - rough)) - f0) * (1.0 - nv).powi(5);
    let r = (n * 2.0 * n.dot(v) - v).normalize();
    let ambient_d = albedo * (1.0 - metal) * environment(n, 1.0) * (Vec3::ONE - fr);
    let ambient_s = environment(r, rough) * fr * (1.0 - rough * 0.5);
    color += (ambient_d + ambient_s) * ao;
    if let Some(e) = m.emissive {
        color += lin(bilinear(e, uv)).truncate() * m.emissive_factor;
    } else {
        color += m.emissive_factor;
    }
    Some(color.extend(base.w))
}

/// ACES filmic approximation (Narkowicz) and sRGB encoding to 8 bits.
fn tonemap(c: Vec3) -> Vec3 {
    let f = |x: f32| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    Vec3::new(
        linear_to_srgb(f(c.x)),
        linear_to_srgb(f(c.y)),
        linear_to_srgb(f(c.z)),
    ) * 255.0
}

fn raster_tri(band: &mut Band, t: &Tri, o: &RasterOptions, lights: &Lights) {
    let [a, b, c] = &t.v;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let inv_area = 1.0 / area;
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(band.w as f32 - 1.0) as i32;
    let min_y = (a.y.min(b.y).min(c.y).floor() as i32).max(band.y0);
    let max_y = (a.y.max(b.y).max(c.y).ceil() as i32).min(band.y1 - 1);
    if min_x > max_x || min_y > max_y {
        return;
    }
    let tex = t.tex;
    let (tw, th) = tex.map_or((1, 1), |t| (t.width() as i32, t.height() as i32));
    let aw = if o.affine {
        o.affine_weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = |inv_w: f32| (1.0 - aw) * inv_w + aw;
    let (qa, qb, qc) = (q(a.inv_w), q(b.inv_w), q(c.inv_w));
    // Wireframe: barycentric weight -> distance in pixels to the opposite edge.
    let edge_px = o.wireframe.map(|_| {
        let len =
            |p: &ScreenVert, q: &ScreenVert| ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt();
        let a2 = area.abs();
        [
            a2 / len(b, c).max(1e-6),
            a2 / len(c, a).max(1e-6),
            a2 / len(a, b).max(1e-6),
        ]
    });
    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        for px in min_x..=max_x {
            let fx = px as f32 + 0.5;
            let w0 = ((b.x - fx) * (c.y - fy) - (b.y - fy) * (c.x - fx)) * inv_area;
            let w1 = ((c.x - fx) * (a.y - fy) - (c.y - fy) * (a.x - fx)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let idx = ((py - band.y0) as u32 * band.w + px as u32) as usize;
            let z = w0 * a.z + w1 * b.z + w2 * c.z;
            if o.zbuffer && z >= band.depth[idx] {
                continue;
            }
            if let (Some(wire), Some(e)) = (o.wireframe, edge_px)
                && (w0 * e[0]).min(w1 * e[1]).min(w2 * e[2]) < 0.6
            {
                band.color[idx] = [wire[0], wire[1], wire[2], 255];
                band.depth[idx] = z;
                if o.write_ids {
                    band.ids[idx] = t.id;
                }
                continue;
            }
            // Same blend as psx_common.gdshaderinc: q = 1 is affine, q = 1/w perspective-correct.
            let (l0, l1, l2) = (w0 * qa, w1 * qb, w2 * qc);
            let k = 1.0 / (l0 + l1 + l2);
            let uv = (a.uv * l0 + b.uv * l1 + c.uv * l2) * k;
            let col = (a.col * l0 + b.col * l1 + c.col * l2) * k;
            let mut rgb = if let Some((m, tt, bb)) = &t.pbr {
                let pos = (a.pos * l0 + b.pos * l1 + c.pos * l2) * k;
                let nrm = (a.nrm * l0 + b.nrm * l1 + c.nrm * l2) * k;
                let nrm = if nrm.length_squared() < 1e-12 {
                    (b.pos - a.pos).cross(c.pos - a.pos)
                } else {
                    nrm
                };
                match shade_pbr(m, uv, col, pos, nrm, (*tt, *bb), lights) {
                    Some(c) => tonemap(c.truncate()),
                    None => continue,
                }
            } else {
                let mut rgb = Vec3::new(col.x, col.y, col.z) * 255.0;
                if let Some(tex) = tex {
                    let tx = ((uv.x - uv.x.floor()) * tw as f32) as i32;
                    let ty = ((uv.y - uv.y.floor()) * th as f32) as i32;
                    let p = tex
                        .get_pixel(tx.clamp(0, tw - 1) as u32, ty.clamp(0, th - 1) as u32)
                        .0;
                    if p[3] < 128 {
                        continue;
                    }
                    rgb *= Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0;
                }
                rgb
            };
            if let Some(f) = o.fog {
                let vz = w0 * a.view_z + w1 * b.view_z + w2 * c.view_z;
                let k = ((vz - f.start) / (f.end - f.start).max(1e-3)).clamp(0.0, 1.0);
                rgb = rgb.lerp(
                    Vec3::new(f.color[0] as f32, f.color[1] as f32, f.color[2] as f32),
                    k,
                );
            }
            if o.dither {
                rgb += Vec3::splat(PS1_DITHER[(py & 3) as usize][(px & 3) as usize] as f32);
            }
            let mut out = [
                rgb.x.clamp(0.0, 255.0) as u8,
                rgb.y.clamp(0.0, 255.0) as u8,
                rgb.z.clamp(0.0, 255.0) as u8,
                255,
            ];
            if o.color15 {
                for ch in out.iter_mut().take(3) {
                    *ch &= 0xF8;
                }
            }
            band.color[idx] = out;
            band.depth[idx] = z;
            if o.write_ids {
                band.ids[idx] = t.id;
            }
        }
    }
}

fn one() -> f32 {
    1.0
}

/// Blender-like orbit camera.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            distance: 3.0,
            fov_y: 50f32.to_radians(),
        }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        self.target + dir * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(0.001);
        Mat4::perspective_rh(self.fov_y, aspect, near, self.distance * 50.0 + 100.0)
    }

    pub fn frame_bounds(&mut self, lo: Vec3, hi: Vec3) {
        self.target = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(1e-3);
        self.distance = radius / (self.fov_y * 0.5).sin() * 1.1;
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.55, 1.55);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let view = self.view();
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
        let scale = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += (-right * dx + up * dy) * scale;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (1.0 - scroll * 0.0015)).clamp(0.01, 1e5);
    }

    /// World-space ray through a point in normalised device coords (-1..1, y up).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = (self.proj(aspect) * self.view()).inverse();
        let near = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        (near, (far - near).normalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_call<'a>(pos: &'a [Vec3], idx: &'a [u32]) -> DrawCall<'a> {
        DrawCall {
            positions: pos,
            uvs: &[],
            colors: &[],
            indices: idx,
            texture: None,
            id_base: 0,
            double_sided: false,
            normals: &[],
            pbr: None,
        }
    }

    #[test]
    fn wireframe_draws_edges_and_the_diagonal_but_not_the_inside() {
        let pos = [
            Vec3::new(-1., -1., 0.),
            Vec3::new(1., -1., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(-1., 1., 0.),
        ];
        let cam = OrbitCamera {
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
            ..Default::default()
        };
        let wire = [255, 0, 255];
        let o = RasterOptions {
            wireframe: Some(wire),
            ..RasterOptions::clean()
        };
        let mut f = Frame::new(64, 64);
        f.clear([0, 0, 0, 255]);
        render(
            &mut f,
            cam.view(),
            cam.proj(1.0),
            &[quad_call(&pos, &[0, 1, 2, 0, 2, 3])],
            &o,
        );
        let at = |x: u32, y: u32| f.color[(y * 64 + x) as usize];
        let wired = f.color.iter().filter(|c| c[..3] == wire).count();
        assert!(wired > 64, "edges drawn: {wired}");
        // The shared diagonal (bottom-left to top-right) runs through the centre.
        assert!((30..34).any(|d| at(d, 63 - d)[..3] == wire || at(d, 64 - d)[..3] == wire));
        // Inside a triangle, away from every edge: shaded, not wire.
        assert_ne!(at(40, 40)[..3], wire);
    }

    #[test]
    fn front_facing_quad_covers_center_and_back_is_culled() {
        let pos = [
            Vec3::new(-1., -1., 0.),
            Vec3::new(1., -1., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(-1., 1., 0.),
        ];
        let ccw = [0, 1, 2, 0, 2, 3];
        let cam = OrbitCamera {
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
            ..Default::default()
        };
        let mut f = Frame::new(64, 64);
        let o = RasterOptions {
            write_ids: true,
            ..RasterOptions::clean()
        };
        render(
            &mut f,
            cam.view(),
            cam.proj(1.0),
            &[quad_call(&pos, &ccw)],
            &o,
        );
        assert_ne!(f.ids[(32 * 64 + 32) as usize], 0, "front face must render");

        let cw = [0, 2, 1, 0, 3, 2];
        let mut f = Frame::new(64, 64);
        render(
            &mut f,
            cam.view(),
            cam.proj(1.0),
            &[quad_call(&pos, &cw)],
            &o,
        );
        assert_eq!(
            f.ids[(32 * 64 + 32) as usize],
            0,
            "back face must be culled"
        );
    }

    /// Textured floor seen at a grazing angle; returns the rendered colours.
    fn floor_render(affine: bool, weight: f32) -> Vec<[u8; 4]> {
        let pos = [
            Vec3::new(-4., -1., -8.),
            Vec3::new(4., -1., -8.),
            Vec3::new(4., -1., 2.),
            Vec3::new(-4., -1., 2.),
        ];
        let uvs = [
            Vec2::new(0., 0.),
            Vec2::new(0.999, 0.),
            Vec2::new(0.999, 0.999),
            Vec2::new(0., 0.999),
        ];
        let idx = [0, 3, 2, 0, 2, 1];
        // UV gradient: red = u, green = v, so colour error measures UV error.
        let tex = RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let cam = OrbitCamera {
            yaw: 0.0,
            pitch: 0.2,
            distance: 3.0,
            ..Default::default()
        };
        let mut f = Frame::new(64, 64);
        let o = RasterOptions {
            affine,
            affine_weight: weight,
            cull_back: false,
            ..RasterOptions::clean()
        };
        let call = DrawCall {
            positions: &pos,
            uvs: &uvs,
            colors: &[],
            indices: &idx,
            texture: Some(&tex),
            id_base: 0,
            double_sided: true,
            normals: &[],
            pbr: None,
        };
        render(&mut f, cam.view(), cam.proj(1.0), &[call], &o);
        f.color
    }

    #[test]
    fn affine_weight_zero_matches_perspective_and_one_warps() {
        let persp = floor_render(false, 1.0);
        assert_eq!(
            floor_render(true, 0.0),
            persp,
            "weight 0 must equal perspective-correct"
        );
        let full = floor_render(true, 1.0);
        let half = floor_render(true, 0.5);
        let diff = |a: &[[u8; 4]], b: &[[u8; 4]]| -> u64 {
            a.iter()
                .zip(b)
                .map(|(x, y)| (x[0].abs_diff(y[0]) as u64) + (x[1].abs_diff(y[1]) as u64))
                .sum()
        };
        assert!(diff(&full, &persp) > 0, "affine must warp a grazing floor");
        assert!(
            diff(&half, &persp) < diff(&full, &persp),
            "partial weight warps less"
        );
    }

    fn sphere_call<'a>(
        p: &'a crate::mesh::Piece,
        pos: &'a [Vec3],
        nrm: &'a [Vec3],
        m: Option<PbrMaterial<'a>>,
    ) -> DrawCall<'a> {
        DrawCall {
            positions: pos,
            uvs: &[],
            colors: &[],
            indices: &p.indices,
            texture: None,
            id_base: 0,
            double_sided: false,
            normals: nrm,
            pbr: m,
        }
    }

    #[test]
    fn banded_parallel_render_matches_single_thread() {
        let s = crate::mesh::tests::uv_sphere(24);
        let pos: Vec<Vec3> = s.vertices.iter().map(|v| v.pos).collect();
        let nrm: Vec<Vec3> = s.vertices.iter().map(|v| v.normal).collect();
        let mat = crate::mesh::Material {
            base_color: [0.8, 0.3, 0.2, 1.0],
            metallic: 0.5,
            roughness: 0.4,
            ..Default::default()
        };
        let cam = OrbitCamera {
            distance: 3.0,
            ..Default::default()
        };
        for opts in [RasterOptions::default(), RasterOptions::reference()] {
            let render_n = |threads| {
                let mut f = Frame::new(320, 200);
                let call = sphere_call(&s, &pos, &nrm, Some(PbrMaterial::from_material(&mat)));
                render_with_threads(&mut f, cam.view(), cam.proj(1.6), &[call], &opts, threads);
                f.color
            };
            assert_eq!(render_n(1), render_n(7), "bands must not change the image");
        }
    }

    #[test]
    fn pbr_shading_responds_to_metal_and_emission() {
        let s = crate::mesh::tests::uv_sphere(32);
        let pos: Vec<Vec3> = s.vertices.iter().map(|v| v.pos).collect();
        let nrm: Vec<Vec3> = s.vertices.iter().map(|v| v.normal).collect();
        let cam = OrbitCamera {
            distance: 3.0,
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        let center = |m: &crate::mesh::Material, o: &RasterOptions| {
            let mut f = Frame::new(64, 64);
            render(
                &mut f,
                cam.view(),
                cam.proj(1.0),
                &[sphere_call(
                    &s,
                    &pos,
                    &nrm,
                    Some(PbrMaterial::from_material(m)),
                )],
                o,
            );
            let p = f.color[(32 * 64 + 32) as usize];
            p[0] as u32 + p[1] as u32 + p[2] as u32
        };
        let o = RasterOptions::reference();
        let plastic = crate::mesh::Material {
            base_color: [0.8, 0.8, 0.8, 1.0],
            roughness: 0.6,
            ..Default::default()
        };
        let metal = crate::mesh::Material {
            metallic: 1.0,
            ..plastic.clone()
        };
        let black = crate::mesh::Material {
            base_color: [0.0, 0.0, 0.0, 1.0],
            ..plastic.clone()
        };
        let glowing = crate::mesh::Material {
            emissive_factor: [0.0, 2.0, 0.0],
            ..black.clone()
        };
        // Facing the camera, a rough metal loses its diffuse term and reads darker.
        assert!(
            center(&metal, &o) < center(&plastic, &o),
            "metal vs plastic"
        );
        assert!(
            center(&glowing, &o) > center(&black, &o) + 100,
            "emission must light a black surface"
        );
        // Without the pbr flag the call falls back to flat vertex colour (white).
        assert_eq!(center(&black, &RasterOptions::clean()), 765);
    }

    #[test]
    fn near_plane_clipping_keeps_visible_part() {
        // A floor quad that passes underneath/behind the camera.
        let pos = [
            Vec3::new(-10., -1., -10.),
            Vec3::new(10., -1., -10.),
            Vec3::new(10., -1., 10.),
            Vec3::new(-10., -1., 10.),
        ];
        let idx = [0, 3, 2, 0, 2, 1];
        let cam = OrbitCamera {
            yaw: 0.0,
            pitch: 0.0,
            distance: 1.0,
            target: Vec3::ZERO,
            ..Default::default()
        };
        let mut f = Frame::new(64, 64);
        let o = RasterOptions {
            write_ids: true,
            cull_back: false,
            ..RasterOptions::clean()
        };
        render(
            &mut f,
            cam.view(),
            cam.proj(1.0),
            &[quad_call(&pos, &idx)],
            &o,
        );
        assert!(
            f.ids[(60 * 64 + 32) as usize] != 0,
            "floor below horizon should be visible"
        );
        assert_eq!(f.ids[(4 * 64 + 32) as usize], 0, "sky should be empty");
    }
}
