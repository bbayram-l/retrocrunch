//! Detail focus: where a model should keep more triangles and texels (faces first).
//!
//! A `DetailField` gives a 0..1 priority for any point in model space. It combines an
//! automatic head region (from head bones on rigs, else from the shape of an upright
//! character) with detail painted by hand on the source mesh (`Piece::detail`).
//! Decimation, the triangle budget and the atlas all sample it by position, so it
//! doesn't have to be carried through every mesh operation.

use crate::mesh::{Model, Piece};
use glam::Vec3;
use std::collections::HashMap;

/// An axis-aligned ellipsoid with a soft edge.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub center: Vec3,
    pub radii: Vec3,
}

impl Region {
    /// 1 inside, fading to 0 between 1x and 1.5x the radii.
    fn at(&self, p: Vec3) -> f32 {
        let d = ((p - self.center) / self.radii.max(Vec3::splat(1e-6))).length();
        (1.0 - (d - 1.0) / 0.5).clamp(0.0, 1.0)
    }
}

/// Painted detail as a point cloud on a hash grid.
struct Painted {
    cell: f32,
    grid: HashMap<[i32; 3], Vec<(Vec3, f32)>>,
}

impl Painted {
    fn key(&self, p: Vec3) -> [i32; 3] {
        (p / self.cell).floor().as_ivec3().to_array()
    }

    /// Value of the nearest painted point within one cell, fading with distance.
    fn at(&self, p: Vec3) -> f32 {
        let k = self.key(p);
        let mut best = 0.0f32;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(pts) = self.grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &(q, v) in pts {
                        let d = (q - p).length() / self.cell;
                        if d < 1.0 {
                            best = best.max(v * (1.0 - d));
                        }
                    }
                }
            }
        }
        best
    }
}

#[derive(Default)]
pub struct DetailField {
    pub regions: Vec<Region>,
    painted: Option<Painted>,
}

/// Bone names that mean "head" (lower-case substrings).
const HEAD_BONES: [&str; 6] = ["head", "face", "jaw", "skull", "eye", "brow"];

impl DetailField {
    /// `auto_head`: find the head automatically. Painted detail on the source pieces is
    /// always included.
    pub fn for_model(model: &Model, auto_head: bool) -> Self {
        let mut f = Self::default();
        if auto_head && let Some(r) = head_from_rig(model).or_else(|| head_from_shape(model)) {
            f.regions.push(r);
        }
        f.painted = painted(model);
        f
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty() && self.painted.is_none()
    }

    /// Priority 0..1 at `p` (model space; bind space for skinned pieces).
    pub fn at(&self, p: Vec3) -> f32 {
        let r = self.regions.iter().map(|r| r.at(p)).fold(0.0, f32::max);
        let q = self.painted.as_ref().map_or(0.0, |s| s.at(p));
        r.max(q)
    }

    /// Mean priority over a piece's vertices.
    pub fn mean(&self, piece: &Piece) -> f32 {
        if self.is_empty() || piece.vertices.is_empty() {
            return 0.0;
        }
        piece.vertices.iter().map(|v| self.at(v.pos)).sum::<f32>() / piece.vertices.len() as f32
    }
}

fn painted(model: &Model) -> Option<Painted> {
    let pts: Vec<(Vec3, f32)> = model
        .pieces
        .iter()
        .filter(|p| p.detail.len() == p.vertices.len())
        .flat_map(|p| {
            p.vertices
                .iter()
                .zip(&p.detail)
                .filter(|(_, d)| **d > 0.01)
                .map(|(v, &d)| (v.pos, d))
        })
        .collect();
    if pts.is_empty() {
        return None;
    }
    let (lo, hi) = model.bounds();
    // About 1/40 of the model: wide enough to cover gaps after decimation.
    let cell = ((hi - lo).max_element() / 40.0).max(1e-5);
    let mut s = Painted {
        cell,
        grid: HashMap::new(),
    };
    for (p, v) in pts {
        let k = s.key(p);
        s.grid.entry(k).or_default().push((p, v));
    }
    Some(s)
}

/// Head region from the vertices skinned mostly to head bones.
fn head_from_rig(model: &Model) -> Option<Region> {
    let rig = model.rig.as_ref()?;
    let head_nodes: Vec<usize> = (0..rig.nodes.len())
        .filter(|&n| {
            let name = rig.nodes[n].name.to_lowercase();
            HEAD_BONES.iter().any(|h| name.contains(h))
        })
        .collect();
    if head_nodes.is_empty() {
        return None;
    }
    let mut pts: Vec<Vec3> = Vec::new();
    for p in &model.pieces {
        let Some(skin) = p.skin.and_then(|s| rig.skins.get(s)) else {
            continue;
        };
        let slots: Vec<u16> = skin
            .joints
            .iter()
            .enumerate()
            .filter(|(_, n)| head_nodes.contains(n))
            .map(|(i, _)| i as u16)
            .collect();
        for v in &p.vertices {
            let w: f32 = (0..4)
                .filter(|&k| slots.contains(&v.joints[k]))
                .map(|k| v.weights[k])
                .sum();
            if w > 0.5 {
                pts.push(v.pos);
            }
        }
    }
    if pts.len() < 8 {
        return None;
    }
    let (lo, hi) = pts.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(a, b), &p| (a.min(p), b.max(p)),
    );
    Some(Region {
        center: (lo + hi) * 0.5,
        radii: ((hi - lo) * 0.5).max(Vec3::splat((hi - lo).max_element() * 0.25)),
    })
}

/// Head of an upright (Y-up) character, from proportions: the highest point near the
/// vertical centre line, about 1/7.5 of the height. None for shapes that aren't
/// character-like (wider or deeper than tall).
fn head_from_shape(model: &Model) -> Option<Region> {
    let (lo, hi) = model.bounds();
    let size = hi - lo;
    let h = size.y;
    if h <= 0.0 || h < 2.0 * size.z || h < 0.8 * size.x {
        return None;
    }
    // Centre line from the upper body, robust to arms and shoulder pieces.
    let upper: Vec<Vec3> = model
        .pieces
        .iter()
        .flat_map(|p| p.vertices.iter().map(|v| v.pos))
        .filter(|p| p.y > lo.y + 0.55 * h)
        .collect();
    if upper.len() < 16 {
        return None;
    }
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let cx = median(upper.iter().map(|p| p.x).collect());
    let cz = median(upper.iter().map(|p| p.z).collect());
    // The top of the head: highest vertex close to the centre line (horns and spikes
    // off to the side don't count).
    let near = 0.07 * h;
    let top = upper
        .iter()
        .filter(|p| (p.x - cx).abs() < near && (p.z - cz).abs() < near * 1.5)
        .map(|p| p.y)
        .fold(f32::MIN, f32::max);
    if top == f32::MIN {
        return None;
    }
    let head_h = h / 7.5;
    // Face depth: the head's own vertices, forward of the centre line.
    let head: Vec<Vec3> = upper
        .iter()
        .copied()
        .filter(|p| p.y > top - head_h && (p.x - cx).abs() < near)
        .collect();
    let z_mid = if head.len() >= 8 {
        median(head.iter().map(|p| p.z).collect())
    } else {
        cz
    };
    Some(Region {
        center: Vec3::new(cx, top - head_h * 0.5, z_mid),
        radii: Vec3::new(head_h * 0.55, head_h * 0.6, head_h * 0.65),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::uv_sphere;

    /// A crude upright figure: body capsule + head sphere on top + two shoulder spikes
    /// that reach as high as the head.
    fn figure() -> Model {
        let mut body = uv_sphere(16);
        for v in &mut body.vertices {
            v.pos = Vec3::new(v.pos.x * 0.25, v.pos.y * 0.75 + 0.75, v.pos.z * 0.15);
        }
        let mut head = uv_sphere(16);
        for v in &mut head.vertices {
            v.pos = v.pos * 0.12 + Vec3::new(0.0, 1.68, 0.02);
        }
        let mut spikes = Vec::new();
        for side in [-1.0f32, 1.0] {
            let mut s = uv_sphere(6);
            for v in &mut s.vertices {
                v.pos = Vec3::new(
                    v.pos.x * 0.03 + side * 0.32,
                    v.pos.y * 0.2 + 1.6,
                    v.pos.z * 0.03,
                );
            }
            spikes.push(s);
        }
        let mut pieces = vec![body, head];
        pieces.extend(spikes);
        Model {
            name: "figure".into(),
            pieces,
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_head_of_an_upright_figure_not_the_spikes() {
        let m = figure();
        let f = DetailField::for_model(&m, true);
        assert_eq!(f.regions.len(), 1);
        assert!(f.at(Vec3::new(0.0, 1.68, 0.1)) > 0.9, "face");
        assert!(f.at(Vec3::new(0.0, 0.8, 0.0)) < 0.01, "belly");
        assert!(f.at(Vec3::new(0.32, 1.65, 0.0)) < 0.01, "shoulder spike");
        // Not character-like: no head.
        let crate_like = Model {
            pieces: vec![uv_sphere(8)],
            ..Default::default()
        };
        assert!(DetailField::for_model(&crate_like, true).is_empty());
        assert!(DetailField::for_model(&m, false).is_empty());
    }

    #[test]
    fn painted_detail_is_sampled_by_position() {
        let mut m = figure();
        let n = m.pieces[0].vertices.len();
        m.pieces[0].detail = (0..n)
            .map(|i| {
                if m.pieces[0].vertices[i].pos.y < 0.3 {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let f = DetailField::for_model(&m, false);
        let foot = m.pieces[0]
            .vertices
            .iter()
            .find(|v| v.pos.y < 0.2)
            .unwrap()
            .pos;
        assert!(f.at(foot) > 0.9);
        assert!(f.at(Vec3::new(0.0, 1.0, 0.15)) < 0.01);
    }
}
