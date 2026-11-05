//! Quad counting. PS1 artists budgeted in polygons, where a flat quad (POLY_FT4/GT4) is
//! one primitive, so a triangle count overstates the cost of quad-modelled meshes.

use super::{Model, Piece};
use glam::Vec3;
use std::collections::HashMap;

/// Two triangles count as one quad when their normals are within this angle.
const QUAD_MAX_ANGLE_DEG: f32 = 10.0;

impl Piece {
    /// Number of triangle pairs that can be sent as one quad: they share an edge (same
    /// corner positions and UVs on both sides, so not across a UV seam), are nearly
    /// coplanar and form a convex quad. Each triangle joins at most one pair; the
    /// flattest pairs win.
    pub fn quad_count(&self) -> usize {
        let pos = |i: u32| self.vertices[i as usize].pos;
        // Weld copies that only differ in normal (flat-shaded exports split every corner).
        let mut first: HashMap<([u32; 3], [u32; 2]), u32> = HashMap::new();
        let weld: Vec<u32> = (0..self.vertices.len() as u32)
            .map(|i| {
                let v = &self.vertices[i as usize];
                let key = (
                    v.pos.to_array().map(f32::to_bits),
                    v.uv.to_array().map(f32::to_bits),
                );
                *first.entry(key).or_insert(i)
            })
            .collect();
        let tris: Vec<[u32; 3]> = self
            .indices
            .chunks_exact(3)
            .map(|t| {
                [
                    weld[t[0] as usize],
                    weld[t[1] as usize],
                    weld[t[2] as usize],
                ]
            })
            .collect();
        let normals: Vec<Option<Vec3>> = tris
            .iter()
            .map(|t| {
                (pos(t[1]) - pos(t[0]))
                    .cross(pos(t[2]) - pos(t[0]))
                    .try_normalize()
            })
            .collect();
        // Undirected edge -> (triangle, vertex opposite the edge).
        let mut edges: HashMap<(u32, u32), Vec<(usize, u32)>> = HashMap::new();
        for (ti, t) in tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edges
                    .entry((a.min(b), a.max(b)))
                    .or_default()
                    .push((ti, t[(k + 2) % 3]));
            }
        }
        let min_dot = QUAD_MAX_ANGLE_DEG.to_radians().cos();
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (&(a, b), users) in &edges {
            // Only plain manifold edges between two different triangles.
            let [(t1, c), (t2, d)] = users[..] else {
                continue;
            };
            let (Some(n1), Some(n2)) = (normals[t1], normals[t2]) else {
                continue;
            };
            let dot = n1.dot(n2);
            if t1 == t2 || dot < min_dot {
                continue;
            }
            // Convex: the other diagonal (c-d) must cross the shared edge (a-b).
            let n = n1 + n2;
            let side = |p: Vec3| (pos(d) - pos(c)).cross(p - pos(c)).dot(n);
            if side(pos(a)) * side(pos(b)) >= 0.0 {
                continue;
            }
            pairs.push((dot, t1.min(t2), t1.max(t2)));
        }
        // Flattest first; ties broken by triangle order so the count is deterministic
        // (edge-map iteration order is random).
        pairs.sort_by(|x, y| y.0.total_cmp(&x.0).then((x.1, x.2).cmp(&(y.1, y.2))));
        let mut used = vec![false; tris.len()];
        let mut quads = 0;
        for (_, t1, t2) in pairs {
            if !used[t1] && !used[t2] {
                used[t1] = true;
                used[t2] = true;
                quads += 1;
            }
        }
        quads
    }
}

impl Model {
    pub fn quad_count(&self) -> usize {
        self.pieces.iter().map(Piece::quad_count).sum()
    }

    /// PS1-style polygon count: each quad counts once, leftover triangles once each.
    pub fn polygon_count(&self) -> usize {
        self.tri_count() - self.quad_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Vertex;
    use glam::Vec2;

    fn piece(pos: &[[f32; 3]], indices: &[u32]) -> Piece {
        Piece {
            vertices: pos
                .iter()
                .map(|p| Vertex {
                    pos: Vec3::from(*p),
                    ..Default::default()
                })
                .collect(),
            indices: indices.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn flat_square_is_one_quad() {
        let p = piece(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        assert_eq!(p.quad_count(), 1);
    }

    #[test]
    fn folded_pair_is_not_a_quad() {
        // Same square with one corner lifted 90 degrees out of the plane.
        let p = piece(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        assert_eq!(p.quad_count(), 0);
    }

    #[test]
    fn concave_pair_is_not_a_quad() {
        // Coplanar and consistently wound, but the corner at (2, 0) is reflex.
        let p = piece(
            &[
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [0.0, 2.0, 0.0],
                [3.0, -0.5, 0.0],
            ],
            &[0, 1, 2, 1, 3, 2],
        );
        assert_eq!(p.quad_count(), 0);
    }

    #[test]
    fn uv_seam_split_vertices_do_not_pair() {
        // Coplanar, but the second triangle's copies of the shared edge have other UVs.
        let mut p = split_square();
        p.vertices[3].uv = Vec2::new(0.5, 0.0);
        p.vertices[4].uv = Vec2::new(0.5, 0.5);
        assert_eq!(p.quad_count(), 0);
    }

    #[test]
    fn flat_shaded_split_vertices_still_pair() {
        // Same corners, only the normals differ (flat-shaded exports): still one quad.
        let mut p = split_square();
        p.vertices[3].normal = Vec3::Z;
        assert_eq!(p.quad_count(), 1);
    }

    #[test]
    fn flat_grid_count_is_stable_across_calls() {
        // Every pair ties on flatness, so only the tie-break decides the matching.
        let n = 6u32;
        let pos: Vec<[f32; 3]> = (0..=n)
            .flat_map(|y| (0..=n).map(move |x| [x as f32, y as f32, 0.0]))
            .collect();
        let idx: Vec<u32> = (0..n)
            .flat_map(|y| (0..n).map(move |x| y * (n + 1) + x))
            .flat_map(|a| {
                let (b, c, d) = (a + 1, a + n + 2, a + n + 1);
                [a, b, c, a, c, d]
            })
            .collect();
        let p = piece(&pos, &idx);
        let first = p.quad_count();
        assert!(first >= (n * n) as usize * 3 / 4, "quads {first}");
        assert!((0..20).all(|_| p.quad_count() == first));
    }

    /// A square whose two triangles each have their own copies of the shared corners.
    fn split_square() -> Piece {
        piece(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            &[0, 1, 2, 3, 4, 5],
        )
    }

    #[test]
    fn cube_counts_six_polygons() {
        let c = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        // Outward-facing faces as quads (a, b, c, d), split into (a, b, c) + (a, c, d).
        let faces = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [1, 2, 6, 5],
            [0, 4, 7, 3],
        ];
        let idx: Vec<u32> = faces
            .iter()
            .flat_map(|f| [f[0], f[1], f[2], f[0], f[2], f[3]])
            .collect();
        let model = Model {
            pieces: vec![piece(&c, &idx)],
            ..Default::default()
        };
        assert_eq!(model.tri_count(), 12);
        assert_eq!(model.quad_count(), 6);
        assert_eq!(model.polygon_count(), 6);
    }
}
