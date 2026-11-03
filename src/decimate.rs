//! Triangle budget distribution and UV-aware mesh decimation (meshoptimizer).

use crate::mesh::Piece;
use meshopt::{SimplifyOptions, SimplifyVertex};

/// Water-filling distribution of a triangle budget across pieces,
/// proportional to `area * weight`, never exceeding a piece's own count.
pub fn distribute_budget(budget: usize, pieces: &[(usize, f32, f32)]) -> Vec<usize> {
    let n = pieces.len();
    let mut out = vec![0usize; n];
    let mut active: Vec<usize> = (0..n).filter(|&i| pieces[i].0 > 0).collect();
    let mut remaining = budget as f64;
    loop {
        let total: f64 = active
            .iter()
            .map(|&i| (pieces[i].1.max(1e-9) * pieces[i].2.max(0.0)) as f64)
            .sum();
        if active.is_empty() {
            break;
        }
        let mut saturated = Vec::new();
        for &i in &active {
            let share = if total > 0.0 {
                remaining * (pieces[i].1.max(1e-9) * pieces[i].2.max(0.0)) as f64 / total
            } else {
                0.0
            };
            if share >= pieces[i].0 as f64 {
                saturated.push(i);
            }
        }
        if saturated.is_empty() {
            for &i in &active {
                let share = if total > 0.0 {
                    remaining * (pieces[i].1.max(1e-9) * pieces[i].2.max(0.0)) as f64 / total
                } else {
                    0.0
                };
                out[i] = share.round() as usize;
            }
            break;
        }
        for i in saturated {
            out[i] = pieces[i].0;
            remaining -= pieces[i].0 as f64;
            active.retain(|&a| a != i);
        }
        remaining = remaining.max(0.0);
    }
    // Keep every piece recognisable: at least a few triangles if it had them.
    for i in 0..n {
        out[i] = out[i].max(pieces[i].0.min(2));
    }
    out
}

pub struct DecimateOptions {
    pub lock_borders: bool,
    /// How strongly UV distortion is penalised relative to geometric error.
    pub uv_weight: f32,
    pub normal_weight: f32,
}

impl Default for DecimateOptions {
    fn default() -> Self {
        Self {
            lock_borders: false,
            uv_weight: 1.0,
            normal_weight: 0.3,
        }
    }
}

/// Simplify `piece` toward `target_tris`. Returns a new compacted piece.
pub fn decimate(piece: &Piece, target_tris: usize, opt: &DecimateOptions) -> Piece {
    if target_tris >= piece.tri_count() || piece.tri_count() == 0 {
        return piece.clone();
    }
    let positions: Vec<[f32; 3]> = piece.vertices.iter().map(|v| v.pos.to_array()).collect();
    let attrs: Vec<f32> = piece
        .vertices
        .iter()
        .flat_map(|v| [v.uv.x, v.uv.y, v.normal.x, v.normal.y, v.normal.z])
        .collect();
    let weights = [
        opt.uv_weight,
        opt.uv_weight,
        opt.normal_weight,
        opt.normal_weight,
        opt.normal_weight,
    ];
    let locks = vertex_flags(piece);
    let target_idx = (target_tris * 3).max(3);

    let run = |options: SimplifyOptions| {
        meshopt::simplify_with_attributes_and_locks_decoder(
            &piece.indices,
            &positions,
            &attrs,
            &weights,
            5 * std::mem::size_of::<f32>(),
            &locks,
            target_idx,
            1.0,
            options,
            None,
        )
    };
    let base = if opt.lock_borders {
        SimplifyOptions::LockBorder
    } else {
        SimplifyOptions::None
    };
    let mut indices = run(base);
    // Topology (UV seams, borders) can stall the collapse well above target:
    // relax seam protection, then fall back to sloppy clustering.
    if indices.len() > target_idx * 5 / 4 {
        indices = run(base | SimplifyOptions::Permissive);
    }
    // Sloppy clustering merges vertices across joints and morph regions: never for those.
    let deforming = piece.skin.is_some() || piece.has_morphs();
    if indices.len() > target_idx * 3 / 2 && !opt.lock_borders && !deforming {
        let sloppy =
            meshopt::simplify_sloppy_decoder(&piece.indices, &positions, target_idx, 1.0, None);
        if !sloppy.is_empty() {
            indices = sloppy;
        }
    }
    // Clone keeps skin, morph targets and metadata; compact() re-aligns morph deltas.
    let mut out = piece.clone();
    out.indices = indices;
    out.compact();
    out
}

/// Dominant weight below this marks a vertex in a joint's bend zone.
const BEND_ZONE: f32 = 0.85;

/// Higher priority for vertices that deform: joint bend zones (several joints share
/// influence) and anything a morph target moves, so elbows, knees and facial
/// expressions keep enough edge loops.
fn vertex_flags(piece: &Piece) -> Vec<SimplifyVertex> {
    let morphed = piece.morphed_vertices();
    piece
        .vertices
        .iter()
        .zip(&morphed)
        .map(|(v, &m)| {
            if (piece.skin.is_some() && v.weights[0] < BEND_ZONE) || m {
                SimplifyVertex::Priority
            } else {
                SimplifyVertex::None
            }
        })
        .collect()
}

/// Rough lower bound for a rig: every joint that really deforms the mesh needs a few
/// edge loops (PS1 characters typically use ~15-25 triangles per bone).
pub fn recommended_min_tris(pieces: &[Piece]) -> usize {
    const TRIS_PER_JOINT: usize = 20;
    let mut used = std::collections::HashSet::new();
    for p in pieces.iter().filter(|p| p.skin.is_some()) {
        for v in &p.vertices {
            for k in 0..4 {
                if v.weights[k] >= 0.2 {
                    used.insert((p.skin, v.joints[k]));
                }
            }
        }
    }
    used.len() * TRIS_PER_JOINT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::uv_sphere;

    #[test]
    fn budget_respects_caps_and_total() {
        // Piece 0 is large but only has 10 triangles: it saturates and the rest is redistributed.
        let b = distribute_budget(1000, &[(10, 10.0, 1.0), (5000, 1.0, 1.0), (5000, 1.0, 1.0)]);
        assert_eq!(b[0], 10);
        assert_eq!(b[1], 495);
        assert_eq!(b.iter().sum::<usize>(), 1000);
    }

    #[test]
    fn budget_weight_shifts_triangles() {
        let b = distribute_budget(1000, &[(5000, 1.0, 3.0), (5000, 1.0, 1.0)]);
        assert!(b[0] > b[1] * 2);
    }

    #[test]
    fn decimation_hits_target_roughly() {
        let s = uv_sphere(64); // ~4k tris
        let d = decimate(&s, 300, &DecimateOptions::default());
        assert!(d.tri_count() <= 360, "got {}", d.tri_count());
        assert!(d.tri_count() >= 150, "got {}", d.tri_count());
        assert!(d.vertices.len() < s.vertices.len());
    }

    /// Sphere skinned to two joints split at the equator, blended in a band.
    pub(crate) fn skinned_sphere() -> Piece {
        let mut s = uv_sphere(64);
        s.skin = Some(0);
        for v in &mut s.vertices {
            let t = ((v.pos.y + 0.2) / 0.4).clamp(0.0, 1.0);
            let (j, w) = crate::rig::normalize_influences([0, 1, 0, 0], [1.0 - t, t, 0.0, 0.0]);
            v.joints = j;
            v.weights = w;
        }
        s
    }

    #[test]
    fn skinned_decimation_keeps_bend_zone_and_valid_weights() {
        let s = skinned_sphere();
        let d = decimate(&s, 300, &DecimateOptions::default());
        assert_eq!(d.skin, Some(0));
        let mixed = |p: &Piece| {
            p.vertices
                .iter()
                .filter(|v| v.weights[0] < BEND_ZONE)
                .count()
        };
        let kept_mixed = mixed(&d) as f32 / mixed(&s) as f32;
        let kept_all = d.vertices.len() as f32 / s.vertices.len() as f32;
        assert!(
            kept_mixed > kept_all * 1.5,
            "bend zone {kept_mixed} vs overall {kept_all}"
        );
        for v in &d.vertices {
            assert!((v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-4);
            assert!(v.joints.iter().all(|&j| j < 2));
        }
        assert_eq!(recommended_min_tris(&[s]), 40);
    }
}
