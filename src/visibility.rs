//! Multi-view visibility using the software rasterizer:
//! hidden-face removal (ID buffers) and per-vertex ambient occlusion / sun shadows (depth maps).

use crate::mesh::Piece;
use crate::raster::{self, DrawCall, Frame, RasterOptions};
use glam::{Mat4, Vec3};

/// Evenly distributed unit directions on a sphere.
pub fn fibonacci_dirs(n: usize) -> Vec<Vec3> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let th = golden * i as f32;
            Vec3::new(r * th.cos(), y, r * th.sin())
        })
        .collect()
}

struct OrthoView {
    view: Mat4,
    proj: Mat4,
}

fn ortho_view(center: Vec3, radius: f32, dir: Vec3) -> OrthoView {
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let eye = center + dir * radius * 2.0;
    let view = Mat4::look_at_rh(eye, center, up);
    let proj = Mat4::orthographic_rh(-radius, radius, -radius, radius, radius * 0.5, radius * 3.5);
    OrthoView { view, proj }
}

fn bounds(pieces: &[Piece]) -> (Vec3, f32) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in pieces {
        for v in &p.vertices {
            lo = lo.min(v.pos);
            hi = hi.max(v.pos);
        }
    }
    let c = (lo + hi) * 0.5;
    (c, ((hi - lo).length() * 0.5).max(1e-4) * 1.02)
}

/// Per-piece list of triangle visibility flags, from `views` directions.
/// With `ignore_below`, views from underneath don't count (parked vehicles, props on floors).
pub fn visible_triangles(
    pieces: &[Piece],
    views: usize,
    resolution: u32,
    ignore_below: bool,
) -> Vec<Vec<bool>> {
    let (center, radius) = bounds(pieces);
    let positions: Vec<Vec<Vec3>> = pieces
        .iter()
        .map(|p| p.vertices.iter().map(|v| v.pos).collect())
        .collect();
    let mut bases = Vec::with_capacity(pieces.len());
    let mut acc = 0u32;
    for p in pieces {
        bases.push(acc);
        acc += p.tri_count() as u32;
    }
    let mut seen = vec![false; acc as usize + 1];
    let calls: Vec<DrawCall> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| DrawCall {
            positions: &positions[i],
            uvs: &[],
            colors: &[],
            indices: &p.indices,
            texture: None,
            id_base: bases[i],
            double_sided: false,
            normals: &[],
            pbr: None,
        })
        .collect();
    let opts = RasterOptions {
        write_ids: true,
        textured: false,
        ..RasterOptions::clean()
    };
    let mut frame = Frame::new(resolution, resolution);
    for d in fibonacci_dirs(views) {
        if ignore_below && d.y < -0.2 {
            continue;
        }
        let v = ortho_view(center, radius, d);
        frame.clear([0, 0, 0, 255]);
        raster::render(&mut frame, v.view, v.proj, &calls, &opts);
        for &id in &frame.ids {
            seen[id as usize] = true;
        }
    }
    pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            (0..p.tri_count())
                .map(|t| seen[bases[i] as usize + t + 1])
                .collect()
        })
        .collect()
}

/// Remove triangles never seen from outside. Returns how many were removed.
/// Pieces flagged in `protect` still occlude others but never lose triangles.
pub fn remove_hidden(
    pieces: &mut [Piece],
    protect: &[bool],
    views: usize,
    resolution: u32,
    ignore_below: bool,
) -> usize {
    let vis = visible_triangles(pieces, views, resolution, ignore_below);
    let mut removed = 0;
    for (i, (p, v)) in pieces.iter_mut().zip(vis).enumerate() {
        if protect.get(i).copied().unwrap_or(false) {
            continue;
        }
        let before = p.tri_count();
        let idx: Vec<u32> = p
            .indices
            .chunks_exact(3)
            .zip(&v)
            .filter(|(_, keep)| **keep)
            .flat_map(|(t, _)| t.to_vec())
            .collect();
        p.indices = idx;
        removed += before - p.tri_count();
        p.compact();
    }
    removed
}

struct DepthMap {
    vp: Mat4,
    depth: Vec<f32>,
    res: u32,
    dir: Vec3,
}

impl DepthMap {
    fn render(pieces: &[Piece], center: Vec3, radius: f32, dir: Vec3, res: u32) -> Self {
        let positions: Vec<Vec<Vec3>> = pieces
            .iter()
            .map(|p| p.vertices.iter().map(|v| v.pos).collect())
            .collect();
        let calls: Vec<DrawCall> = pieces
            .iter()
            .enumerate()
            .map(|(i, p)| DrawCall {
                positions: &positions[i],
                uvs: &[],
                colors: &[],
                indices: &p.indices,
                texture: None,
                id_base: 0,
                double_sided: true,
                normals: &[],
                pbr: None,
            })
            .collect();
        let v = ortho_view(center, radius, dir);
        let mut frame = Frame::new(res, res);
        let opts = RasterOptions {
            textured: false,
            cull_back: false,
            ..RasterOptions::clean()
        };
        raster::render(&mut frame, v.view, v.proj, &calls, &opts);
        Self {
            vp: v.proj * v.view,
            depth: frame.depth,
            res,
            dir,
        }
    }

    /// Is `p` (offset along `n`) the closest surface along this view direction?
    fn lit(&self, p: Vec3, n: Vec3, bias: f32) -> bool {
        let q = self.vp.project_point3(p + n * bias);
        let x = ((q.x * 0.5 + 0.5) * self.res as f32) as i32;
        let y = ((0.5 - q.y * 0.5) * self.res as f32) as i32;
        let mut far = f32::MIN;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (sx, sy) = (x + dx, y + dy);
                if sx < 0 || sy < 0 || sx >= self.res as i32 || sy >= self.res as i32 {
                    return true;
                }
                far = far.max(self.depth[(sy as u32 * self.res + sx as u32) as usize]);
            }
        }
        q.z <= far + 1e-3
    }
}

pub struct ShadeSettings {
    pub ao: bool,
    pub ao_strength: f32,
    pub ao_views: usize,
    pub sun: Option<(Vec3, f32)>,
    pub ambient: f32,
}

/// Per-vertex lighting factors (0..~1.5) for every piece.
pub fn vertex_shading(pieces: &[Piece], s: &ShadeSettings) -> Vec<Vec<f32>> {
    let (center, radius) = bounds(pieces);
    let res = 384;
    let bias = radius * 2.0 / res as f32 * 1.5;
    let ao_maps: Vec<DepthMap> = if s.ao {
        fibonacci_dirs(s.ao_views)
            .into_iter()
            .map(|d| DepthMap::render(pieces, center, radius, d, res))
            .collect()
    } else {
        Vec::new()
    };
    let sun_map = s
        .sun
        .map(|(d, _)| DepthMap::render(pieces, center, radius, d.normalize(), res));
    pieces
        .iter()
        .map(|p| {
            p.vertices
                .iter()
                .map(|v| {
                    let n = v.normal.normalize_or(Vec3::Y);
                    let mut ao = 1.0;
                    if !ao_maps.is_empty() {
                        let (mut vis, mut tot) = (0.0, 0.0);
                        for m in &ao_maps {
                            let w = n.dot(m.dir);
                            if w <= 0.0 {
                                continue;
                            }
                            tot += w;
                            if m.lit(v.pos, n, bias) {
                                vis += w;
                            }
                        }
                        let occl = if tot > 0.0 { vis / tot } else { 1.0 };
                        ao = 1.0 - s.ao_strength * (1.0 - occl);
                    }
                    match (&sun_map, s.sun) {
                        (Some(m), Some((_, strength))) => {
                            let lambert = n.dot(m.dir).max(0.0);
                            let shadow = if lambert > 0.0 && m.lit(v.pos, n, bias) {
                                1.0
                            } else {
                                0.0
                            };
                            ao * (s.ambient + strength * lambert * shadow)
                        }
                        _ => ao,
                    }
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Piece;
    use crate::mesh::tests::uv_sphere;

    fn inner_and_outer() -> Vec<Piece> {
        let outer = uv_sphere(24);
        let mut inner = uv_sphere(12);
        for v in &mut inner.vertices {
            v.pos *= 0.5;
        }
        inner.name = "inner".into();
        vec![outer, inner]
    }

    #[test]
    fn enclosed_mesh_is_removed() {
        let mut ps = inner_and_outer();
        let outer_before = ps[0].tri_count();
        remove_hidden(&mut ps, &[], 32, 192, false);
        assert_eq!(ps[1].tri_count(), 0, "inner sphere is never visible");
        assert!(
            ps[0].tri_count() as f32 > outer_before as f32 * 0.9,
            "outer sphere survives"
        );
    }

    #[test]
    fn protected_pieces_keep_hidden_faces() {
        let mut ps = inner_and_outer();
        let inner_before = ps[1].tri_count();
        remove_hidden(&mut ps, &[false, true], 32, 192, false);
        assert_eq!(ps[1].tri_count(), inner_before);
    }

    #[test]
    fn ao_darkens_enclosed_vertices() {
        let ps = inner_and_outer();
        let s = ShadeSettings {
            ao: true,
            ao_strength: 1.0,
            ao_views: 32,
            sun: None,
            ambient: 0.0,
        };
        let sh = vertex_shading(&ps, &s);
        let outer_avg: f32 = sh[0].iter().sum::<f32>() / sh[0].len() as f32;
        let inner_avg: f32 = sh[1].iter().sum::<f32>() / sh[1].len() as f32;
        assert!(outer_avg > 0.8, "outer {outer_avg}");
        assert!(inner_avg < 0.2, "inner {inner_avg}");
    }
}
