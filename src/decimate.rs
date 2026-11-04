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
    decimate_with_detail(piece, target_tris, opt, &[], 0.0)
}

/// Like `decimate`, with a 0..1 `priority` per vertex (e.g. the face). Triangles whose
/// corners average above 0.5 form the focus area, which gets a guaranteed share of the
/// budget: its surface area counts `strength` times. Empty `priority` = uniform.
///
/// Weighting the error alone isn't enough: quadric error measures shape change, and a
/// face's features are tiny next to spikes or armour, so they'd still go first.
/// Two passes instead: lock the focus area and reduce the rest to its share, then lock
/// the rest and reduce the focus area (with the strip along its border) to the total.
pub fn decimate_with_detail(
    piece: &Piece,
    target_tris: usize,
    opt: &DecimateOptions,
    priority: &[f32],
    strength: f32,
) -> Piece {
    if target_tris >= piece.tri_count() || piece.tri_count() == 0 {
        return piece.clone();
    }
    let flags = vertex_flags(piece);
    let mut indices = match focus_split(piece, target_tris, priority, strength) {
        Some((focus, focus_budget)) => {
            let in_focus_tri = |t: &[u32]| t.iter().filter(|&&i| focus[i as usize]).count() >= 2;
            let focus_tris = piece
                .indices
                .chunks_exact(3)
                .filter(|t| in_focus_tri(t))
                .count();
            // Pass 1: the focus area is frozen; everything else shrinks to its share.
            let locks: Vec<SimplifyVertex> = flags
                .iter()
                .zip(&focus)
                .map(|(&f, &k)| if k { SimplifyVertex::Lock } else { f })
                .collect();
            let rest_target = target_tris - focus_budget + focus_tris;
            let pass1 = simplify(piece, &piece.indices, &locks, rest_target, opt, false);
            // Pass 2: freeze the rest except where it touches the focus area, so the
            // border can still collapse and the two sides stay connected.
            let mut free = focus.clone();
            for t in pass1.chunks_exact(3) {
                if t.iter().any(|&i| focus[i as usize]) {
                    for &i in t {
                        free[i as usize] = true;
                    }
                }
            }
            let locks: Vec<SimplifyVertex> = flags
                .iter()
                .zip(&free)
                .map(|(&f, &k)| if k { f } else { SimplifyVertex::Lock })
                .collect();
            simplify(piece, &pass1, &locks, target_tris, opt, false)
        }
        None => simplify(piece, &piece.indices, &flags, target_tris, opt, true),
    };
    if indices.is_empty() {
        indices = piece.indices.clone();
    }
    // Clone keeps skin, morph targets and metadata; compact() re-aligns morph deltas.
    let mut out = piece.clone();
    out.indices = indices;
    out.compact();
    out
}

/// Focus vertices and the focus area's share of `target_tris`, when it applies.
fn focus_split(
    piece: &Piece,
    target_tris: usize,
    priority: &[f32],
    strength: f32,
) -> Option<(Vec<bool>, usize)> {
    if priority.len() != piece.vertices.len() || strength <= 1.0 {
        return None;
    }
    let focus: Vec<bool> = priority.iter().map(|&p| p > 0.5).collect();
    let (mut fa, mut ra, mut ft) = (0.0f32, 0.0f32, 0usize);
    for t in piece.indices.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|k| piece.vertices[t[k] as usize].pos);
        let area = (b - a).cross(c - a).length() * 0.5;
        if t.iter().filter(|&&i| focus[i as usize]).count() >= 2 {
            fa += area;
            ft += 1;
        } else {
            ra += area;
        }
    }
    let total = piece.tri_count();
    // Nothing to protect, or everything is the focus: plain decimation.
    if ft == 0 || ft == total || fa <= 0.0 || ra <= 0.0 {
        return None;
    }
    let share = fa * strength / (fa * strength + ra);
    let budget = ((target_tris as f32 * share).round() as usize).clamp(1, ft.min(target_tris - 1));
    Some((focus, budget))
}

/// One meshopt reduction of `indices` to about `target_tris`.
fn simplify(
    piece: &Piece,
    indices: &[u32],
    locks: &[SimplifyVertex],
    target_tris: usize,
    opt: &DecimateOptions,
    allow_sloppy: bool,
) -> Vec<u32> {
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
    let target_idx = (target_tris * 3).max(3);
    let run = |options: SimplifyOptions| {
        meshopt::simplify_with_attributes_and_locks_decoder(
            indices,
            &positions,
            &attrs,
            &weights,
            5 * std::mem::size_of::<f32>(),
            locks,
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
    let mut out = run(base);
    // Topology (UV seams, borders) can stall the collapse well above target:
    // relax seam protection, then fall back to sloppy clustering.
    if out.len() > target_idx * 5 / 4 {
        out = run(base | SimplifyOptions::Permissive);
    }
    // Sloppy clustering ignores locks and merges vertices across joints and morph
    // regions: never for those, nor in the focused passes.
    let deforming = piece.skin.is_some() || piece.has_morphs();
    if allow_sloppy && out.len() > target_idx * 3 / 2 && !opt.lock_borders && !deforming {
        let sloppy = meshopt::simplify_sloppy_decoder(indices, &positions, target_idx, 1.0, None);
        if !sloppy.is_empty() {
            out = sloppy;
        }
    }
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

    #[test]
    fn focus_area_keeps_its_share_of_the_budget() {
        let s = uv_sphere(64);
        // The cap above y = 0.8 is ~10% of the surface.
        let priority: Vec<f32> = s
            .vertices
            .iter()
            .map(|v| if v.pos.y > 0.8 { 1.0 } else { 0.0 })
            .collect();
        let cap_tris = |p: &Piece| {
            p.indices
                .chunks_exact(3)
                .filter(|t| t.iter().all(|&i| p.vertices[i as usize].pos.y > 0.75))
                .count()
        };
        let opt = DecimateOptions::default();
        let plain = decimate(&s, 300, &opt);
        let focused = decimate_with_detail(&s, 300, &opt, &priority, 3.0);
        let total = focused.tri_count();
        assert!((240..=360).contains(&total), "total budget kept: {total}");
        // Area x 3 => about a quarter of the budget, far more than uniform reduction.
        assert!(
            cap_tris(&focused) >= 2 * cap_tris(&plain).max(1),
            "cap {} vs plain {}",
            cap_tris(&focused),
            cap_tris(&plain)
        );
        // No new open edges along the focus border (edges used once, by position).
        let open_edges = |p: &Piece| {
            let mut edges = std::collections::HashMap::new();
            let key = |q: glam::Vec3| (q * 1e4).round().as_ivec3().to_array();
            for t in p.indices.chunks_exact(3) {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let (pa, pb) = (
                        key(p.vertices[a as usize].pos),
                        key(p.vertices[b as usize].pos),
                    );
                    if pa != pb {
                        *edges
                            .entry(if pa < pb { (pa, pb) } else { (pb, pa) })
                            .or_insert(0) += 1;
                    }
                }
            }
            edges.values().filter(|&&n| n == 1).count()
        };
        assert!(
            open_edges(&focused) <= open_edges(&plain),
            "no cracks along the focus border: {} open edges vs {} plain",
            open_edges(&focused),
            open_edges(&plain)
        );
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
