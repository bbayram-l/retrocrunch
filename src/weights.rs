//! Skin-weight editing: brush strokes, smoothing and normalisation on a piece's
//! 4-influence skin, plus the heat-map colours the rig editor shows.
//!
//! Joint ids here are *slots* in the piece's skin (`Skin::joints`), like `Vertex::joints`.

use crate::mesh::{Piece, Vertex};
use crate::rig::{Rig, top4};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrushMode {
    Add,
    Subtract,
    /// Pull the weight towards the brush strength.
    Set,
    /// Average with neighbouring vertices.
    Smooth,
}

impl BrushMode {
    pub const ALL: [BrushMode; 4] = [Self::Add, Self::Subtract, Self::Set, Self::Smooth];
    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Set => "Set",
            Self::Smooth => "Smooth",
        }
    }
}

/// The skin slot of rig node `node` in `piece`'s skin, if the piece is skinned to it.
pub fn joint_slot(rig: &Rig, piece: &Piece, node: usize) -> Option<u16> {
    let skin = rig.skins.get(piece.skin?)?;
    skin.joints
        .iter()
        .position(|&n| n == node)
        .map(|i| i as u16)
}

/// Weight of joint slot `j` on `v` (0 when it doesn't influence the vertex).
pub fn weight_of(v: &Vertex, j: u16) -> f32 {
    (0..4)
        .filter(|&k| v.joints[k] == j && v.weights[k] > 0.0)
        .map(|k| v.weights[k])
        .sum()
}

/// Give joint `j` the weight `w` (0..1) on `v`. The other influences share the rest in
/// their current proportions; the 4 strongest are kept, normalised. A vertex's only
/// influence can't be reduced (there is nothing to hand its weight to).
pub fn set_weight(v: &mut Vertex, j: u16, w: f32) {
    let w = w.clamp(0.0, 1.0);
    let others: Vec<(u16, f32)> = (0..4)
        .filter(|&k| v.joints[k] != j && v.weights[k] > 0.0)
        .map(|k| (v.joints[k], v.weights[k]))
        .collect();
    let rest: f32 = others.iter().map(|o| o.1).sum();
    if rest <= 1e-6 && w < 1.0 {
        if weight_of(v, j) > 0.0 {
            return;
        }
        if w <= 0.0 {
            return;
        }
    }
    let mut pairs = vec![(j, w)];
    if rest > 1e-6 {
        pairs.extend(others.iter().map(|&(oj, ow)| (oj, ow / rest * (1.0 - w))));
    }
    let (joints, weights) = top4(&pairs);
    v.joints = joints;
    v.weights = weights;
}

/// Groups of vertices sharing a position (UV seams split vertices; weights must not).
fn welded_ids(piece: &Piece) -> (Vec<u32>, usize) {
    let mut map: HashMap<[i32; 3], u32> = HashMap::new();
    let lo = piece
        .vertices
        .iter()
        .fold(glam::Vec3::splat(f32::MAX), |a, v| a.min(v.pos));
    let hi = piece
        .vertices
        .iter()
        .fold(glam::Vec3::splat(f32::MIN), |a, v| a.max(v.pos));
    let q = ((hi - lo).max_element() * 1e-5).max(1e-9);
    let ids = piece
        .vertices
        .iter()
        .map(|v| {
            let k = ((v.pos - lo) / q).round().as_ivec3().to_array();
            let n = map.len() as u32;
            *map.entry(k).or_insert(n)
        })
        .collect();
    (ids, map.len())
}

/// Vertex neighbourhoods across UV seams, for smoothing.
pub struct Neighbours {
    weld: Vec<u32>,
    /// Neighbouring weld groups per weld group.
    adj: Vec<Vec<u32>>,
    /// Vertices per weld group.
    members: Vec<Vec<u32>>,
}

impl Neighbours {
    pub fn new(piece: &Piece) -> Self {
        let (weld, n) = welded_ids(piece);
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
        for t in piece.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (weld[a as usize], weld[b as usize]);
                if a != b {
                    if !adj[a as usize].contains(&b) {
                        adj[a as usize].push(b);
                    }
                    if !adj[b as usize].contains(&a) {
                        adj[b as usize].push(a);
                    }
                }
            }
        }
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (i, &g) in weld.iter().enumerate() {
            members[g as usize].push(i as u32);
        }
        Self { weld, adj, members }
    }

    /// Average weight of joint `j` around vertex `i` (its weld group's neighbours).
    fn neighbour_average(&self, piece: &Piece, i: usize, j: u16) -> Option<f32> {
        let around = &self.adj[self.weld[i] as usize];
        if around.is_empty() {
            return None;
        }
        let sum: f32 = around
            .iter()
            .map(|&g| weight_of(&piece.vertices[self.members[g as usize][0] as usize], j))
            .sum();
        Some(sum / around.len() as f32)
    }
}

/// One brush dab: `hits` are (vertex, falloff 0..1). Returns whether anything changed.
pub fn paint(
    piece: &mut Piece,
    j: u16,
    hits: &[(usize, f32)],
    mode: BrushMode,
    strength: f32,
    nb: Option<&Neighbours>,
) -> bool {
    let strength = strength.clamp(0.0, 1.0);
    // Targets first, so smoothing reads the weights from before this dab.
    let targets: Vec<(usize, f32)> = hits
        .iter()
        .filter(|(i, f)| *i < piece.vertices.len() && *f > 0.0)
        .filter_map(|&(i, f)| {
            let cur = weight_of(&piece.vertices[i], j);
            let new = match mode {
                BrushMode::Add => cur + strength * f,
                BrushMode::Subtract => cur - strength * f,
                BrushMode::Set => cur + (strength - cur) * f,
                BrushMode::Smooth => {
                    let avg = nb?.neighbour_average(piece, i, j)?;
                    cur + (avg - cur) * f * strength.max(0.1)
                }
            };
            ((new - cur).abs() > 1e-5).then_some((i, new))
        })
        .collect();
    for &(i, w) in &targets {
        set_weight(&mut piece.vertices[i], j, w);
    }
    !targets.is_empty()
}

/// Smooth joint `j` over the whole piece (`passes` times).
pub fn smooth_all(piece: &mut Piece, j: u16, passes: usize) -> bool {
    let nb = Neighbours::new(piece);
    let all: Vec<(usize, f32)> = (0..piece.vertices.len()).map(|i| (i, 1.0)).collect();
    let mut changed = false;
    for _ in 0..passes {
        changed |= paint(piece, j, &all, BrushMode::Smooth, 0.5, Some(&nb));
    }
    changed
}

/// Re-normalise every vertex's influences (sum 1, strongest first, dead slots cleared).
pub fn normalize_all(piece: &mut Piece) {
    for v in &mut piece.vertices {
        let (j, w) = crate::rig::normalize_influences(v.joints, v.weights);
        v.joints = j;
        v.weights = w;
    }
}

/// Heat-map colour for a weight: dark blue (0) -> cyan -> green -> yellow -> red (1).
pub fn heat(w: f32) -> [f32; 4] {
    if w <= 1e-4 {
        return [0.06, 0.07, 0.25, 1.0];
    }
    let stops = [
        [0.0, 0.25, 1.0],
        [0.0, 0.9, 0.9],
        [0.1, 0.9, 0.2],
        [1.0, 0.9, 0.0],
        [1.0, 0.1, 0.05],
    ];
    let x = w.clamp(0.0, 1.0) * 4.0;
    let i = (x.floor() as usize).min(3);
    let t = x - i as f32;
    let (a, b) = (stops[i], stops[i + 1]);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        1.0,
    ]
}

/// Skin influences of every vertex, for undo.
pub type WeightSnapshot = Vec<Vec<([u16; 4], [f32; 4])>>;

pub fn snapshot(pieces: &[Piece]) -> WeightSnapshot {
    pieces
        .iter()
        .map(|p| p.vertices.iter().map(|v| (v.joints, v.weights)).collect())
        .collect()
}

pub fn restore(pieces: &mut [Piece], snap: &WeightSnapshot) {
    for (p, s) in pieces.iter_mut().zip(snap) {
        for (v, &(j, w)) in p.vertices.iter_mut().zip(s) {
            v.joints = j;
            v.weights = w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::uv_sphere;

    fn two_joint(v: &mut Vertex, a: f32) {
        v.joints = [0, 1, 0, 0];
        v.weights = [a, 1.0 - a, 0.0, 0.0];
    }

    fn sum(v: &Vertex) -> f32 {
        v.weights.iter().sum()
    }

    #[test]
    fn set_weight_keeps_sum_and_proportions() {
        let mut v = Vertex {
            joints: [0, 1, 2, 0],
            weights: [0.5, 0.3, 0.2, 0.0],
            ..Default::default()
        };
        set_weight(&mut v, 0, 0.0);
        assert!((sum(&v) - 1.0).abs() < 1e-5);
        assert!((weight_of(&v, 1) - 0.6).abs() < 1e-4, "others keep 3:2");
        assert!((weight_of(&v, 2) - 0.4).abs() < 1e-4);
        assert_eq!(weight_of(&v, 0), 0.0);
        // A new joint takes its share from the others.
        set_weight(&mut v, 7, 0.5);
        assert!((weight_of(&v, 7) - 0.5).abs() < 1e-4);
        assert!((sum(&v) - 1.0).abs() < 1e-5);
        // The only influence can't be removed.
        let mut solo = Vertex {
            joints: [3, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        set_weight(&mut solo, 3, 0.2);
        assert_eq!(weight_of(&solo, 3), 1.0);
    }

    #[test]
    fn brush_modes_move_weights_and_smooth_blends() {
        let mut p = uv_sphere(8);
        for v in &mut p.vertices {
            let a = if v.pos.y > 0.0 { 0.8 } else { 0.0 };
            two_joint(v, a);
        }
        let i = p.vertices.iter().position(|v| v.pos.y > 0.5).unwrap();
        assert!(paint(
            &mut p,
            0,
            &[(i, 1.0)],
            BrushMode::Subtract,
            0.4,
            None
        ));
        assert!((weight_of(&p.vertices[i], 0) - 0.4).abs() < 1e-4);
        paint(&mut p, 0, &[(i, 0.5)], BrushMode::Set, 0.2, None);
        assert!((weight_of(&p.vertices[i], 0) - 0.3).abs() < 1e-4);
        paint(&mut p, 1, &[(i, 1.0)], BrushMode::Add, 1.0, None);
        assert_eq!(weight_of(&p.vertices[i], 1), 1.0);
        // Smoothing turns the hard top/bottom split into a gradient near the equator.
        // The lowest ring above the equator sits next to the 0-weight half.
        let eq = (0..p.vertices.len())
            .filter(|&k| p.vertices[k].pos.y > 1e-3)
            .min_by(|&a, &b| p.vertices[a].pos.y.total_cmp(&p.vertices[b].pos.y))
            .unwrap();
        assert!(smooth_all(&mut p, 0, 2));
        let w = weight_of(&p.vertices[eq], 0);
        assert!(w > 0.05 && w < 0.95, "equator weight {w}");
        for v in &p.vertices {
            assert!((sum(v) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn seams_share_weights_when_smoothing() {
        let p = uv_sphere(8);
        let nb = Neighbours::new(&p);
        // The sphere's UV seam duplicates vertices: they must land in one weld group.
        assert!(nb.members.iter().any(|m| m.len() > 1));
        assert!(heat(0.0)[2] > heat(1.0)[2] && heat(1.0)[0] > 0.9);
    }
}
