//! Block ("bloxel") level builder: a sparse grid of shaped 1x1x1 blocks,
//! hidden-face culling, box-mapped atlas UVs, voxel raycasting and project save/load.

use crate::mesh::{Material, Model, Piece, Vertex};
use anyhow::{Context, Result};
use glam::{IVec3, Vec2, Vec3};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Shape {
    Cube,
    Slab,
    Wedge,
    Corner,
    InnerCorner,
    Pillar,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Self::Cube,
        Self::Slab,
        Self::Wedge,
        Self::Corner,
        Self::InnerCorner,
        Self::Pillar,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Slab => "Slab",
            Self::Wedge => "Ramp",
            Self::Corner => "Outer corner",
            Self::InnerCorner => "Inner corner",
            Self::Pillar => "Pillar",
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub shape: Shape,
    /// Quarter turns around +Y.
    pub rot: u8,
    /// Upside-down (ceiling ramps).
    pub flip: bool,
    /// Atlas tile index.
    pub tile: u16,
}

impl Default for Block {
    fn default() -> Self {
        Self {
            shape: Shape::Cube,
            rot: 0,
            flip: false,
            tile: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BlockWorld {
    #[serde(with = "cells")]
    pub cells: HashMap<IVec3, Block>,
    pub atlas_path: Option<String>,
    pub tile_size: u32,
}

mod cells {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Cell {
        p: [i32; 3],
        b: Block,
    }

    pub fn serialize<S: Serializer>(m: &HashMap<IVec3, Block>, s: S) -> Result<S::Ok, S::Error> {
        let mut v: Vec<Cell> = m
            .iter()
            .map(|(k, b)| Cell {
                p: k.to_array(),
                b: *b,
            })
            .collect();
        v.sort_by_key(|c| c.p);
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<HashMap<IVec3, Block>, D::Error> {
        let v: Vec<Cell> = Vec::deserialize(d)?;
        Ok(v.into_iter()
            .map(|c| (IVec3::from_array(c.p), c.b))
            .collect())
    }
}

/// One face of a shape template in unit-cube space.
#[derive(Clone, Debug)]
struct Face {
    verts: Vec<Vec3>,
    /// Cube side the face lies on (0..6 = +X,-X,+Y,-Y,+Z,-Z) or None for interior faces.
    side: Option<usize>,
    /// Covers the entire cube side (can hide / be hidden by neighbours).
    full: bool,
    normal: Vec3,
}

pub const SIDES: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

fn opposite(side: usize) -> usize {
    side ^ 1
}

fn template(shape: Shape) -> (Vec<Vec<Vec3>>, Vec3) {
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let cube_faces = |lo: Vec3, hi: Vec3| -> Vec<Vec<Vec3>> {
        vec![
            vec![
                v(hi.x, lo.y, lo.z),
                v(hi.x, hi.y, lo.z),
                v(hi.x, hi.y, hi.z),
                v(hi.x, lo.y, hi.z),
            ],
            vec![
                v(lo.x, lo.y, lo.z),
                v(lo.x, lo.y, hi.z),
                v(lo.x, hi.y, hi.z),
                v(lo.x, hi.y, lo.z),
            ],
            vec![
                v(lo.x, hi.y, lo.z),
                v(lo.x, hi.y, hi.z),
                v(hi.x, hi.y, hi.z),
                v(hi.x, hi.y, lo.z),
            ],
            vec![
                v(lo.x, lo.y, lo.z),
                v(hi.x, lo.y, lo.z),
                v(hi.x, lo.y, hi.z),
                v(lo.x, lo.y, hi.z),
            ],
            vec![
                v(lo.x, lo.y, hi.z),
                v(hi.x, lo.y, hi.z),
                v(hi.x, hi.y, hi.z),
                v(lo.x, hi.y, hi.z),
            ],
            vec![
                v(lo.x, lo.y, lo.z),
                v(lo.x, hi.y, lo.z),
                v(hi.x, hi.y, lo.z),
                v(hi.x, lo.y, lo.z),
            ],
        ]
    };
    match shape {
        Shape::Cube => (cube_faces(Vec3::ZERO, Vec3::ONE), Vec3::splat(0.5)),
        Shape::Slab => (cube_faces(Vec3::ZERO, v(1.0, 0.5, 1.0)), v(0.5, 0.25, 0.5)),
        Shape::Pillar => (
            cube_faces(v(0.3, 0.0, 0.3), v(0.7, 1.0, 0.7)),
            v(0.5, 0.5, 0.5),
        ),
        // Ramp rising toward -Z: full bottom and back (-Z) faces.
        Shape::Wedge => (
            vec![
                vec![v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
                vec![v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.), v(1., 0., 0.)],
                vec![v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 0.)],
                vec![v(1., 0., 0.), v(1., 1., 0.), v(1., 0., 1.)],
                vec![v(0., 1., 0.), v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 0.)],
            ],
            v(0.5, 0.3, 0.3),
        ),
        // Pyramid with apex above the (-X,-Z) corner.
        Shape::Corner => (
            vec![
                vec![v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
                vec![v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 0.)],
                vec![v(0., 0., 0.), v(0., 1., 0.), v(1., 0., 0.)],
                vec![v(1., 0., 0.), v(0., 1., 0.), v(1., 0., 1.)],
                vec![v(0., 0., 1.), v(1., 0., 1.), v(0., 1., 0.)],
            ],
            v(0.35, 0.25, 0.35),
        ),
        // Cube with the (+X,+Z) top corner cut off.
        Shape::InnerCorner => (
            vec![
                vec![v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
                vec![v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.)],
                vec![v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.), v(1., 0., 0.)],
                vec![v(0., 1., 0.), v(0., 1., 1.), v(1., 1., 0.)],
                vec![v(1., 0., 0.), v(1., 1., 0.), v(1., 0., 1.)],
                vec![v(0., 0., 1.), v(1., 0., 1.), v(0., 1., 1.)],
                vec![v(1., 1., 0.), v(0., 1., 1.), v(1., 0., 1.)],
            ],
            v(0.4, 0.4, 0.4),
        ),
    }
}

fn polygon_area(p: &[Vec3]) -> f32 {
    let mut n = Vec3::ZERO;
    for i in 1..p.len() - 1 {
        n += (p[i] - p[0]).cross(p[i + 1] - p[0]);
    }
    n.length() * 0.5
}

fn build_faces(shape: Shape, rot: u8, flip: bool) -> Vec<Face> {
    let (polys, centroid) = template(shape);
    let c = Vec3::splat(0.5);
    let xf = |p: Vec3| {
        let mut q = p - c;
        if flip {
            q.y = -q.y;
        }
        for _ in 0..(rot % 4) {
            q = Vec3::new(q.z, q.y, -q.x);
        }
        q + c
    };
    let centroid = xf(centroid);
    polys
        .into_iter()
        .map(|poly| {
            let mut verts: Vec<Vec3> = poly.into_iter().map(xf).collect();
            let mut n = (verts[1] - verts[0])
                .cross(verts[2] - verts[0])
                .normalize_or_zero();
            let center = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
            if n.dot(center - centroid) < 0.0 {
                verts.reverse();
                n = -n;
            }
            let eps = 1e-4;
            let side = (0..6).find(|&s| {
                let d = SIDES[s].as_vec3();
                let axis = if d.x != 0.0 {
                    0
                } else if d.y != 0.0 {
                    1
                } else {
                    2
                };
                let target = if d[axis] > 0.0 { 1.0 } else { 0.0 };
                verts.iter().all(|v| (v[axis] - target).abs() < eps) && n.dot(d) > 0.99
            });
            let full = side.is_some() && (polygon_area(&verts) - 1.0).abs() < 1e-3;
            Face {
                verts,
                side,
                full,
                normal: n,
            }
        })
        .collect()
}

fn faces_for(b: &Block) -> &'static [Face] {
    static CACHE: OnceLock<Vec<Vec<Face>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let mut all = Vec::new();
        for s in Shape::ALL {
            for flip in [false, true] {
                for rot in 0..4u8 {
                    all.push(build_faces(s, rot, flip));
                }
            }
        }
        all
    });
    &cache[b.shape.index() * 8 + (b.flip as usize) * 4 + (b.rot % 4) as usize]
}

fn has_full_side(b: &Block, side: usize) -> bool {
    faces_for(b).iter().any(|f| f.full && f.side == Some(side))
}

#[derive(Clone, Copy, Debug)]
pub struct Atlas {
    pub cols: u32,
    pub rows: u32,
    /// Shrink tile UVs by this many texels to avoid bleeding.
    pub inset: f32,
    pub tex_w: u32,
    pub tex_h: u32,
}

impl Atlas {
    pub fn from_image(img: &RgbaImage, tile: u32) -> Self {
        let tile = tile.max(1);
        Self {
            cols: (img.width() / tile).max(1),
            rows: (img.height() / tile).max(1),
            inset: 0.0,
            tex_w: img.width(),
            tex_h: img.height(),
        }
    }

    pub fn tile_count(&self) -> u32 {
        self.cols * self.rows
    }

    fn uv(&self, tile: u16, local: Vec2) -> Vec2 {
        let t = tile as u32 % self.tile_count().max(1);
        let (cx, cy) = ((t % self.cols) as f32, (t / self.cols) as f32);
        let iu = self.inset / self.tex_w.max(1) as f32 * self.cols as f32;
        let iv = self.inset / self.tex_h.max(1) as f32 * self.rows as f32;
        let l = Vec2::new(
            iu + local.x * (1.0 - 2.0 * iu),
            iv + local.y * (1.0 - 2.0 * iv),
        );
        Vec2::new((cx + l.x) / self.cols as f32, (cy + l.y) / self.rows as f32)
    }
}

impl Default for Atlas {
    fn default() -> Self {
        Self {
            cols: 1,
            rows: 1,
            inset: 0.0,
            tex_w: 1,
            tex_h: 1,
        }
    }
}

/// Box-mapped UV inside a tile for a point on a face with normal `n`.
fn local_uv(p: Vec3, n: Vec3) -> Vec2 {
    let a = n.abs();
    let fr = |v: f32| {
        if (v - v.floor()).abs() < 1e-5 && v > 0.0 {
            1.0
        } else {
            v - v.floor()
        }
    };
    if a.y >= a.x && a.y >= a.z {
        Vec2::new(fr(p.x), if n.y > 0.0 { fr(p.z) } else { 1.0 - fr(p.z) })
    } else if a.x >= a.z {
        Vec2::new(
            if n.x > 0.0 { 1.0 - fr(p.z) } else { fr(p.z) },
            1.0 - fr(p.y),
        )
    } else {
        Vec2::new(
            if n.z > 0.0 { fr(p.x) } else { 1.0 - fr(p.x) },
            1.0 - fr(p.y),
        )
    }
}

/// Simple directional face shading so the editor and untextured exports read well.
pub fn face_shade(n: Vec3) -> f32 {
    let l = Vec3::new(0.45, 0.8, 0.35).normalize();
    0.55 + 0.45 * n.dot(l).max(0.0)
}

impl BlockWorld {
    pub fn get(&self, p: IVec3) -> Option<&Block> {
        self.cells.get(&p)
    }

    pub fn set(&mut self, p: IVec3, b: Option<Block>) {
        match b {
            Some(b) => {
                self.cells.insert(p, b);
            }
            None => {
                self.cells.remove(&p);
            }
        }
    }

    pub fn fill_box(&mut self, a: IVec3, b: IVec3, block: Option<Block>) {
        let (lo, hi) = (a.min(b), a.max(b));
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    self.set(IVec3::new(x, y, z), block);
                }
            }
        }
    }

    pub fn bounds(&self) -> Option<(IVec3, IVec3)> {
        let mut it = self.cells.keys();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), &p| {
            (lo.min(p), hi.max(p + IVec3::ONE))
        }))
    }

    /// Build a single mesh with culled hidden faces. `shade` bakes face lighting into vertex colours.
    pub fn build_piece(&self, atlas: &Atlas, shade: bool) -> Piece {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut keys: Vec<&IVec3> = self.cells.keys().collect();
        keys.sort_by_key(|k| k.to_array());
        for &pos in keys {
            let b = &self.cells[&pos];
            for f in faces_for(b) {
                if let Some(side) = f.side
                    && let Some(nb) = self.cells.get(&(pos + SIDES[side]))
                    && has_full_side(nb, opposite(side))
                {
                    continue;
                }
                let base = vertices.len() as u32;
                let s = if shade { face_shade(f.normal) } else { 1.0 };
                for v in &f.verts {
                    let wp = *v + pos.as_vec3();
                    vertices.push(Vertex {
                        pos: wp,
                        normal: f.normal,
                        uv: atlas.uv(b.tile, local_uv(*v, f.normal)),
                        color: [s, s, s, 1.0],
                        ..Default::default()
                    });
                }
                for i in 1..f.verts.len() as u32 - 1 {
                    indices.extend_from_slice(&[base, base + i, base + i + 1]);
                }
            }
        }
        Piece {
            name: "level".into(),
            vertices,
            indices,
            material: Some(0),
            ..Default::default()
        }
    }

    /// `subdivisions` splits every block face into an n x n grid (1 = off) so affine
    /// texture warping stays small, like PS1 games did with large polygons.
    pub fn to_model(
        &self,
        name: &str,
        atlas_img: Option<&RgbaImage>,
        shade: bool,
        godot_collision: bool,
        subdivisions: u32,
    ) -> Model {
        let atlas = atlas_img
            .map(|i| Atlas::from_image(i, self.tile_size))
            .unwrap_or_default();
        let mut piece = self.build_piece(&atlas, shade);
        piece.subdivide(face_grid_edge(subdivisions), MAX_SUBDIVIDED_TRIS);
        // Godot import hint: "-col" suffix generates a static trimesh collider.
        piece.name = if godot_collision {
            format!("{name}-col")
        } else {
            name.to_string()
        };
        Model {
            name: name.to_string(),
            pieces: vec![piece],
            materials: vec![Material {
                name: "blocks".into(),
                albedo: atlas_img.cloned(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).context("parsing block project")
    }
}

/// Procedural 4x4 starter atlas so the editor works without any assets.
pub fn default_atlas(tile: u32) -> RgbaImage {
    let base: [[u8; 3]; 16] = [
        [150, 75, 60],
        [120, 120, 125],
        [80, 140, 60],
        [140, 100, 60],
        [170, 160, 120],
        [90, 90, 110],
        [60, 100, 140],
        [180, 180, 175],
        [110, 60, 50],
        [70, 70, 70],
        [150, 130, 70],
        [100, 80, 120],
        [60, 120, 110],
        [200, 120, 50],
        [40, 40, 50],
        [190, 60, 60],
    ];
    let hash = |x: u32, y: u32, s: u32| {
        let mut h =
            x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263) ^ s.wrapping_mul(2246822519);
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        (h ^ (h >> 16)) & 0xff
    };
    RgbaImage::from_fn(tile * 4, tile * 4, |x, y| {
        let t = (y / tile) * 4 + x / tile;
        let (lx, ly) = (x % tile, y % tile);
        let c = base[t as usize];
        let n = hash(x, y, t) as i32 / 16 - 8;
        let line = match t % 4 {
            0 => {
                ly % (tile / 2).max(1) == 0
                    || (lx
                        + if (ly / (tile / 2).max(1)).is_multiple_of(2) {
                            0
                        } else {
                            tile / 2
                        })
                        % tile
                        == 0
            }
            1 => lx == 0 || ly == 0,
            2 => false,
            _ => ly % (tile / 4).max(1) == 0,
        };
        let k = if line { -35 } else { n };
        let ch = |v: u8| (v as i32 + k).clamp(0, 255) as u8;
        image::Rgba([ch(c[0]), ch(c[1]), ch(c[2]), 255])
    })
}

/// Safety cap for subdivided level meshes.
pub const MAX_SUBDIVIDED_TRIS: usize = 400_000;

/// Edge-length threshold that turns a unit block face into an n x n grid
/// (sides 1/n and diagonals sqrt(2)/n both fall under 1.5/n). 0 = no subdivision.
pub fn face_grid_edge(n: u32) -> f32 {
    if n <= 1 { 0.0 } else { 1.5 / n as f32 }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub cell: IVec3,
    /// Face normal of the entry face (points toward the empty neighbour).
    pub normal: IVec3,
    pub t: f32,
}

/// Amanatides-Woo voxel traversal. Also hits the ground plane y = `ground` (cell below).
pub fn raycast(
    world: &BlockWorld,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
    ground: Option<i32>,
) -> Option<Hit> {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let mut cell = origin.floor().as_ivec3();
    let step = IVec3::new(
        dir.x.signum() as i32,
        dir.y.signum() as i32,
        dir.z.signum() as i32,
    );
    let next_boundary = |o: f32, c: i32, s: i32| {
        if s > 0 {
            (c + 1) as f32 - o
        } else {
            o - c as f32
        }
    };
    let inv = |d: f32| {
        if d.abs() < 1e-9 {
            f32::INFINITY
        } else {
            1.0 / d.abs()
        }
    };
    let t_delta = Vec3::new(inv(dir.x), inv(dir.y), inv(dir.z));
    let mut t_max = Vec3::new(
        next_boundary(origin.x, cell.x, step.x) * t_delta.x,
        next_boundary(origin.y, cell.y, step.y) * t_delta.y,
        next_boundary(origin.z, cell.z, step.z) * t_delta.z,
    );
    let ground_hit = ground.and_then(|g| {
        let t = (g as f32 - origin.y) / dir.y;
        (dir.y < 0.0 && t > 0.0 && t <= max_t).then(|| {
            let p = origin + dir * t;
            Hit {
                cell: IVec3::new(p.x.floor() as i32, g - 1, p.z.floor() as i32),
                normal: IVec3::Y,
                t,
            }
        })
    });
    let mut t = 0.0;
    let mut normal = IVec3::ZERO;
    while t <= max_t {
        if world.cells.contains_key(&cell) && t > 0.0 {
            let hit = Hit { cell, normal, t };
            return match ground_hit {
                Some(g) if g.t < t => Some(g),
                _ => Some(hit),
            };
        }
        if t_max.x < t_max.y && t_max.x < t_max.z {
            cell.x += step.x;
            t = t_max.x;
            t_max.x += t_delta.x;
            normal = IVec3::new(-step.x, 0, 0);
        } else if t_max.y < t_max.z {
            cell.y += step.y;
            t = t_max.y;
            t_max.y += t_delta.y;
            normal = IVec3::new(0, -step.y, 0);
        } else {
            cell.z += step.z;
            t = t_max.z;
            t_max.z += t_delta.z;
            normal = IVec3::new(0, 0, -step.z);
        }
    }
    ground_hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdivided_level_has_grid_faces_and_stays_closed() {
        let mut w = BlockWorld::default();
        w.set(IVec3::ZERO, Some(Block::default()));
        w.set(IVec3::X, Some(Block::default()));
        let m = w.to_model("lvl", None, true, false, 2);
        let p = &m.pieces[0];
        // 10 visible faces, each split into a 2x2 grid of 8 triangles.
        assert_eq!(p.tri_count(), 10 * 8);
        // Closed surface: every edge (by welded position) is shared by exactly two triangles.
        let key = |v: Vec3| (v * 1000.0).round().as_ivec3().to_array();
        let mut edges: HashMap<([i32; 3], [i32; 3]), u32> = HashMap::new();
        for t in p.indices.chunks_exact(3) {
            for k in 0..3 {
                let (a, b) = (
                    key(p.vertices[t[k] as usize].pos),
                    key(p.vertices[t[(k + 1) % 3] as usize].pos),
                );
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(
            edges.values().all(|&c| c == 2),
            "subdivision must not open cracks"
        );
        assert_eq!(face_grid_edge(1), 0.0);
    }

    #[test]
    fn adjacent_cubes_cull_shared_faces() {
        let mut w = BlockWorld::default();
        w.set(IVec3::ZERO, Some(Block::default()));
        assert_eq!(w.build_piece(&Atlas::default(), false).tri_count(), 12);
        w.set(IVec3::X, Some(Block::default()));
        assert_eq!(w.build_piece(&Atlas::default(), false).tri_count(), 20);
    }

    #[test]
    fn ramp_against_wall_hides_back_face_only() {
        let mut w = BlockWorld::default();
        w.set(
            IVec3::ZERO,
            Some(Block {
                shape: Shape::Wedge,
                ..Default::default()
            }),
        );
        let alone = w.build_piece(&Atlas::default(), false).tri_count();
        assert_eq!(alone, 2 + 2 + 1 + 1 + 2);
        // Cube behind the ramp's full back face (-Z).
        w.set(IVec3::NEG_Z, Some(Block::default()));
        let both = w.build_piece(&Atlas::default(), false).tri_count();
        assert_eq!(both, alone - 2 + 10);
    }

    #[test]
    fn every_shape_has_outward_faces_and_full_bottom() {
        for s in Shape::ALL {
            for rot in 0..4 {
                let b = Block {
                    shape: s,
                    rot,
                    flip: false,
                    tile: 0,
                };
                let faces = faces_for(&b);
                assert!(!faces.is_empty());
                if s != Shape::Pillar {
                    assert!(has_full_side(&b, 3), "{s:?} rot {rot} bottom must be full");
                }
                let flipped = Block { flip: true, ..b };
                if s != Shape::Pillar {
                    assert!(has_full_side(&flipped, 2), "{s:?} flipped top must be full");
                }
            }
        }
    }

    #[test]
    fn raycast_hits_block_and_ground() {
        let mut w = BlockWorld::default();
        w.set(IVec3::new(0, 0, -5), Some(Block::default()));
        let h = raycast(&w, Vec3::new(0.5, 0.5, 0.5), Vec3::NEG_Z, 100.0, None).unwrap();
        assert_eq!(h.cell, IVec3::new(0, 0, -5));
        assert_eq!(h.normal, IVec3::Z);
        let g = raycast(&w, Vec3::new(3.5, 5.0, 3.5), Vec3::NEG_Y, 100.0, Some(0)).unwrap();
        assert_eq!(g.cell, IVec3::new(3, -1, 3));
        assert_eq!(g.cell + g.normal, IVec3::new(3, 0, 3));
    }

    #[test]
    fn project_roundtrip() {
        let mut w = BlockWorld {
            tile_size: 16,
            ..Default::default()
        };
        w.set(
            IVec3::new(1, 2, 3),
            Some(Block {
                shape: Shape::Corner,
                rot: 2,
                flip: true,
                tile: 7,
            }),
        );
        let p = std::env::temp_dir().join("psxforge_blocks.json");
        w.save(&p).unwrap();
        let back = BlockWorld::load(&p).unwrap();
        assert_eq!(back.cells, w.cells);
        let _ = std::fs::remove_file(p);
    }
}
