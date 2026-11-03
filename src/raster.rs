//! A small CPU rasterizer that reproduces PlayStation-style rendering:
//! integer vertex snapping (jitter), affine texture mapping, nearest sampling,
//! ordering-table depth sorting (no z-buffer), GPU dither and 15-bit colour.
//! Also used for ID/depth renders (hidden-face removal, ambient occlusion).

use crate::texture::PS1_DITHER;
use glam::{Mat4, Vec2, Vec3, Vec4};
use image::RgbaImage;
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
    pub fog: Option<FogSettings>,
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
            fog: None,
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
}

#[derive(Clone, Copy)]
struct ClipVert {
    clip: Vec4,
    uv: Vec2,
    col: Vec4,
    view_z: f32,
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
}

struct Tri<'a> {
    v: [ScreenVert; 3],
    tex: Option<&'a RgbaImage>,
    id: u32,
    sort_key: f32,
}

fn lerp_cv(a: &ClipVert, b: &ClipVert, t: f32) -> ClipVert {
    ClipVert {
        clip: a.clip.lerp(b.clip, t),
        uv: a.uv.lerp(b.uv, t),
        col: a.col.lerp(b.col, t),
        view_z: a.view_z + (b.view_z - a.view_z) * t,
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

/// Render several draw calls into `frame` with a single view-projection.
/// `view` is needed separately for fog distance.
pub fn render(frame: &mut Frame, view: Mat4, proj: Mat4, calls: &[DrawCall], o: &RasterOptions) {
    let vp = proj * view;
    let (fw, fh) = (frame.w as f32, frame.h as f32);
    let mut tris: Vec<Tri> = Vec::new();
    for call in calls {
        let has_uv = call.uvs.len() == call.positions.len();
        let has_col = call.colors.len() == call.positions.len();
        for (ti, t) in call.indices.chunks_exact(3).enumerate() {
            let cv: [ClipVert; 3] = std::array::from_fn(|k| {
                let i = t[k] as usize;
                let p = call.positions[i];
                ClipVert {
                    clip: vp * p.extend(1.0),
                    uv: if has_uv { call.uvs[i] } else { Vec2::ZERO },
                    col: if has_col {
                        Vec4::from(call.colors[i])
                    } else {
                        Vec4::ONE
                    },
                    view_z: -(view.transform_point3(p)).z,
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
                });
            }
        }
    }
    if !o.zbuffer {
        // PS1 ordering table: far to near by average depth.
        tris.sort_by(|a, b| b.sort_key.total_cmp(&a.sort_key));
    }
    for t in &tris {
        raster_tri(frame, t, o);
    }
}

fn raster_tri(frame: &mut Frame, t: &Tri, o: &RasterOptions) {
    let [a, b, c] = &t.v;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let inv_area = 1.0 / area;
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(frame.w as f32 - 1.0) as i32;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(frame.h as f32 - 1.0) as i32;
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
            let idx = (py as u32 * frame.w + px as u32) as usize;
            let z = w0 * a.z + w1 * b.z + w2 * c.z;
            if o.zbuffer && z >= frame.depth[idx] {
                continue;
            }
            // Same blend as psx_lit.gdshader: q = 1 is affine, q = 1/w perspective-correct.
            let (l0, l1, l2) = (w0 * qa, w1 * qb, w2 * qc);
            let k = 1.0 / (l0 + l1 + l2);
            let uv = (a.uv * l0 + b.uv * l1 + c.uv * l2) * k;
            let col = (a.col * l0 + b.col * l1 + c.col * l2) * k;
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
            frame.color[idx] = out;
            frame.depth[idx] = z;
            if o.write_ids {
                frame.ids[idx] = t.id;
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
        }
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
