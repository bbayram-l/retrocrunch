//! In-memory model representation. File import lives in `load`.

mod load;

#[cfg(test)]
use load::percent_decode;
pub(crate) use load::safe_local_path;
use load::{load_gltf, load_obj, load_via_blender};

use crate::rig::{self, Rig};
use anyhow::{Result, bail};
use glam::{Vec2, Vec3};
use image::{GrayImage, RgbaImage};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
    /// Linear 0..1 vertex colour (Gouraud). 1.0 = unmodulated texture.
    pub color: [f32; 4],
    /// Skin joint slots (indices into the piece's `Skin::joints`) and normalised weights.
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl Vertex {
    /// Midpoint of every attribute, skin influences included. Symmetric arithmetic, so
    /// `midpoint(a, b) == midpoint(b, a)` bit for bit (seam duplicates stay welded).
    pub fn midpoint(a: &Vertex, b: &Vertex) -> Vertex {
        let mid4 = |x: [f32; 4], y: [f32; 4]| std::array::from_fn(|k| (x[k] + y[k]) * 0.5);
        let (joints, weights) =
            rig::blend_influences((a.joints, a.weights), (b.joints, b.weights), 0.5);
        Vertex {
            pos: (a.pos + b.pos) * 0.5,
            normal: (a.normal + b.normal).normalize_or(a.normal),
            uv: (a.uv + b.uv) * 0.5,
            color: mid4(a.color, b.color),
            joints,
            weights,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Piece {
    pub name: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// Index into `Model::rig.skins` when skinned (positions are then in bind space).
    pub skin: Option<usize>,
    /// Morph targets (blend shapes), one delta per vertex, aligned with `vertices`.
    pub morphs: Vec<Morph>,
    /// Pieces from the same source mesh share a group; weight animation channels
    /// (`rig::Property::Weights`) address this id.
    pub morph_group: Option<usize>,
    /// Default morph weights (rest expression).
    pub morph_weights: Vec<f32>,
    /// Hand-painted detail priority per vertex (0..1, empty = none); see `detail.rs`.
    pub detail: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Morph {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
}

impl Morph {
    fn remap(&mut self, order: &[usize]) {
        self.positions = order
            .iter()
            .map(|&i| self.positions.get(i).copied().unwrap_or_default())
            .collect();
        self.normals = order
            .iter()
            .map(|&i| self.normals.get(i).copied().unwrap_or_default())
            .collect();
    }

    fn push_midpoint(&mut self, a: usize, b: usize) {
        let mid = |v: &[Vec3]| {
            (v.get(a).copied().unwrap_or_default() + v.get(b).copied().unwrap_or_default()) * 0.5
        };
        let (p, n) = (mid(&self.positions), mid(&self.normals));
        self.positions.push(p);
        self.normals.push(n);
    }

    pub fn is_zero_at(&self, i: usize) -> bool {
        self.positions
            .get(i)
            .is_none_or(|d| d.length_squared() < 1e-12)
    }
}

impl Piece {
    /// One bone per vertex (no blending), like PS1 vertex skinning: each vertex follows
    /// only its strongest joint. Joints move stiffly but the mesh stays closed.
    pub fn single_bone_skinning(&mut self) -> bool {
        if self.skin.is_none() {
            return false;
        }
        for v in &mut self.vertices {
            let k = (0..4)
                .max_by(|&a, &b| v.weights[a].total_cmp(&v.weights[b]))
                .unwrap_or(0);
            v.joints = [v.joints[k], 0, 0, 0];
            v.weights = [1.0, 0.0, 0.0, 0.0];
        }
        true
    }

    /// Split a skinned piece into rigid segments, like early PS1 characters: each
    /// triangle follows the one joint with the most weight across its corners, and
    /// corners shared by triangles of different joints are duplicated, so the segments
    /// move as solid blocks (no stretching, visible seams at the joints). Returns the
    /// number of segments.
    pub fn rigid_segments(&mut self) -> usize {
        if self.skin.is_none() {
            return 0;
        }
        let mut map: std::collections::HashMap<(u32, u16), u32> = Default::default();
        let mut order: Vec<usize> = Vec::new();
        let mut verts: Vec<Vertex> = Vec::new();
        let mut used: std::collections::BTreeSet<u16> = Default::default();
        let mut indices = Vec::with_capacity(self.indices.len());
        for t in self.indices.chunks_exact(3) {
            let mut acc: Vec<(u16, f32)> = Vec::new();
            for &i in t {
                let v = &self.vertices[i as usize];
                for k in 0..4 {
                    if v.weights[k] > 0.0 {
                        match acc.iter_mut().find(|a| a.0 == v.joints[k]) {
                            Some(a) => a.1 += v.weights[k],
                            None => acc.push((v.joints[k], v.weights[k])),
                        }
                    }
                }
            }
            let joint = acc
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map_or(self.vertices[t[0] as usize].joints[0], |a| a.0);
            used.insert(joint);
            for &i in t {
                let id = *map.entry((i, joint)).or_insert_with(|| {
                    let mut v = self.vertices[i as usize];
                    v.joints = [joint, 0, 0, 0];
                    v.weights = [1.0, 0.0, 0.0, 0.0];
                    verts.push(v);
                    order.push(i as usize);
                    verts.len() as u32 - 1
                });
                indices.push(id);
            }
        }
        self.vertices = verts;
        self.indices = indices;
        for m in &mut self.morphs {
            m.remap(&order);
        }
        self.detail.clear();
        used.len()
    }

    /// Remove triangles that reference vertices past the end of `vertices`.
    pub fn drop_invalid_triangles(&mut self) {
        let n = self.vertices.len();
        if self.indices.iter().all(|&i| (i as usize) < n) && self.indices.len().is_multiple_of(3) {
            return;
        }
        self.indices = self
            .indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| (i as usize) < n))
            .flatten()
            .copied()
            .collect();
    }

    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn surface_area(&self) -> f32 {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    self.vertices[t[0] as usize].pos,
                    self.vertices[t[1] as usize].pos,
                    self.vertices[t[2] as usize].pos,
                );
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    pub fn uv_area(&self) -> f32 {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    self.vertices[t[0] as usize].uv,
                    self.vertices[t[1] as usize].uv,
                    self.vertices[t[2] as usize].uv,
                );
                ((b - a).perp_dot(c - a)).abs() * 0.5
            })
            .sum()
    }

    /// Drop vertices no index refers to and remap indices.
    pub fn compact(&mut self) {
        let mut remap = vec![u32::MAX; self.vertices.len()];
        let mut verts = Vec::new();
        let mut order = Vec::new();
        for i in self.indices.iter_mut() {
            let r = &mut remap[*i as usize];
            if *r == u32::MAX {
                *r = verts.len() as u32;
                verts.push(self.vertices[*i as usize]);
                order.push(*i as usize);
            }
            *i = *r;
        }
        let painted = self.detail.len() == self.vertices.len();
        self.vertices = verts;
        for m in &mut self.morphs {
            m.remap(&order);
        }
        self.detail = if painted {
            order.iter().map(|&o| self.detail[o]).collect()
        } else {
            Vec::new()
        };
    }

    pub fn has_morphs(&self) -> bool {
        !self.morphs.is_empty()
    }

    /// Vertices any morph target moves (used to protect expression detail).
    pub fn morphed_vertices(&self) -> Vec<bool> {
        (0..self.vertices.len())
            .map(|i| self.morphs.iter().any(|m| !m.is_zero_at(i)))
            .collect()
    }

    /// Positions with morph `weights` applied (missing weights count as 0).
    pub fn morphed_positions(&self, weights: &[f32]) -> Vec<Vec3> {
        let mut out: Vec<Vec3> = self.vertices.iter().map(|v| v.pos).collect();
        for (m, &w) in self.morphs.iter().zip(weights) {
            if w == 0.0 {
                continue;
            }
            for (o, d) in out.iter_mut().zip(&m.positions) {
                *o += *d * w;
            }
        }
        out
    }

    /// Split triangles until no edge is longer than `max_edge` (red-green refinement).
    /// Edges are split globally, so neighbours always agree and no T-junction cracks
    /// appear. Stops early once the piece reaches `max_tris`.
    pub fn subdivide(&mut self, max_edge: f32, max_tris: usize) {
        if max_edge <= 0.0 {
            return;
        }
        let limit2 = max_edge * max_edge;
        for _round in 0..12 {
            if self.tri_count() >= max_tris {
                return;
            }
            let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
            for t in self.indices.chunks_exact(3) {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    let long = (self.vertices[a as usize].pos - self.vertices[b as usize].pos)
                        .length_squared()
                        > limit2;
                    if long && !mids.contains_key(&key) {
                        let m = Vertex::midpoint(
                            &self.vertices[key.0 as usize],
                            &self.vertices[key.1 as usize],
                        );
                        mids.insert(key, self.vertices.len() as u32);
                        self.vertices.push(m);
                        for mo in &mut self.morphs {
                            mo.push_midpoint(key.0 as usize, key.1 as usize);
                        }
                    }
                }
            }
            if mids.is_empty() {
                return;
            }
            let mid = |a: u32, b: u32| mids.get(&(a.min(b), a.max(b))).copied();
            let mut out = Vec::with_capacity(self.indices.len() * 2);
            for t in self.indices.chunks_exact(3) {
                let v = [t[0], t[1], t[2]];
                let m = [mid(v[0], v[1]), mid(v[1], v[2]), mid(v[2], v[0])];
                match m {
                    [None, None, None] => out.extend_from_slice(&v),
                    [Some(m01), Some(m12), Some(m20)] => out.extend_from_slice(&[
                        v[0], m01, m20, m01, v[1], m12, m20, m12, v[2], m01, m12, m20,
                    ]),
                    _ => {
                        let split = m.iter().filter(|x| x.is_some()).count();
                        if split == 1 {
                            // Rotate so the split edge is a-b.
                            let r = m.iter().position(Option::is_some).unwrap_or(0);
                            let (a, b, c) = (v[r], v[(r + 1) % 3], v[(r + 2) % 3]);
                            let mab = m[r].unwrap_or(a);
                            out.extend_from_slice(&[a, mab, c, mab, b, c]);
                        } else {
                            // Rotate so edges a-b and b-c are split and c-a is not.
                            let r = (m.iter().position(Option::is_none).unwrap_or(0) + 1) % 3;
                            let (a, b, c) = (v[r], v[(r + 1) % 3], v[(r + 2) % 3]);
                            let (mab, mbc) = (m[r].unwrap_or(a), m[(r + 1) % 3].unwrap_or(b));
                            out.extend_from_slice(&[mab, b, mbc, a, mab, mbc, a, mbc, c]);
                        }
                    }
                }
            }
            self.indices = out;
        }
    }

    pub fn recompute_normals(&mut self) {
        for v in &mut self.vertices {
            v.normal = Vec3::ZERO;
        }
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (
                self.vertices[t[0] as usize].pos,
                self.vertices[t[1] as usize].pos,
                self.vertices[t[2] as usize].pos,
            );
            let n = (b - a).cross(c - a);
            for &i in t {
                self.vertices[i as usize].normal += n;
            }
        }
        for v in &mut self.vertices {
            v.normal = v.normal.normalize_or(Vec3::Y);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub albedo: Option<RgbaImage>,
    pub ao: Option<GrayImage>,
    pub normal: Option<RgbaImage>,
    pub emissive: Option<RgbaImage>,
    /// PBR inputs, used by the reference ("Original") viewport.
    pub metallic: f32,
    pub roughness: f32,
    /// glTF layout: roughness in G, metalness in B.
    pub metal_rough: Option<RgbaImage>,
    pub emissive_factor: [f32; 3],
    /// Draw both faces (glTF doubleSided): no back-face culling.
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "material".into(),
            base_color: [1.0; 4],
            albedo: None,
            ao: None,
            normal: None,
            emissive: None,
            metallic: 0.0,
            roughness: 1.0,
            metal_rough: None,
            // glTF semantics: emission = factor x texture, black by default.
            emissive_factor: [0.0; 3],
            double_sided: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub name: String,
    pub source: PathBuf,
    pub pieces: Vec<Piece>,
    pub materials: Vec<Material>,
    /// Skeleton, skins and animation clips (glTF/GLB sources only).
    pub rig: Option<Rig>,
}

impl Model {
    /// Copy with every length multiplied by `s` (vertices, rig translations, bind matrices, keys).
    pub fn scaled(&self, s: f32) -> Model {
        let mut m = self.clone();
        if s == 1.0 {
            return m;
        }
        for p in &mut m.pieces {
            for v in &mut p.vertices {
                v.pos *= s;
            }
            for mo in &mut p.morphs {
                mo.positions.iter_mut().for_each(|d| *d *= s);
            }
        }
        if let Some(r) = &mut m.rig {
            r.scale(s);
        }
        m
    }

    /// Anything to export or preview as animation: skins, morphs or clips.
    pub fn has_animation(&self) -> bool {
        self.rig.is_some() && (self.is_rigged() || self.pieces.iter().any(Piece::has_morphs))
    }

    /// A copy posed at time `t` of `clip` (skinning and morph weights applied to the
    /// positions). Without a rig or clip it's an unchanged copy.
    pub fn posed(&self, clip: Option<usize>, t: f32) -> Model {
        let mut m = self.clone();
        let Some(rig) = self.rig.as_ref() else {
            return m;
        };
        let Some(ci) = clip.filter(|&c| c < rig.clips.len()) else {
            return m;
        };
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(ci), t)));
        for p in &mut m.pieces {
            let weights = p
                .morph_group
                .and_then(|g| rig.morph_weights(Some(ci), t, g))
                .unwrap_or_else(|| p.morph_weights.clone());
            let base = p.morphed_positions(&weights);
            let skin = p.skin.and_then(|s| mats.get(s));
            for (v, b) in p.vertices.iter_mut().zip(base) {
                v.pos = match skin {
                    Some(mt) => crate::rig::skin_point(b, v.joints, v.weights, mt),
                    None => b,
                };
            }
        }
        m
    }

    /// Clip index by name or number.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        let rig = self.rig.as_ref()?;
        rig.clips
            .iter()
            .position(|c| c.name == name)
            .or_else(|| name.parse::<usize>().ok().filter(|&i| i < rig.clips.len()))
    }

    pub fn is_rigged(&self) -> bool {
        self.rig.as_ref().is_some_and(|r| !r.skins.is_empty())
            && self.pieces.iter().any(|p| p.skin.is_some())
    }

    pub fn tri_count(&self) -> usize {
        self.pieces.iter().map(Piece::tri_count).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.pieces.iter().map(|p| p.vertices.len()).sum()
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in &self.pieces {
            for v in &p.vertices {
                lo = lo.min(v.pos);
                hi = hi.max(v.pos);
            }
        }
        if lo.x > hi.x {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (lo, hi)
        }
    }

    pub fn load(path: &Path) -> Result<Model> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut model = match ext.as_str() {
            "obj" => load_obj(path)?,
            "glb" | "gltf" => load_gltf(path)?,
            // Native ufbx first; Blender as a fallback for files ufbx rejects.
            "fbx" => crate::fbx::load_fbx(path).or_else(|e| {
                load_via_blender(path).map_err(|b| {
                    anyhow::anyhow!(
                        "{e:#}
(Blender fallback: {b:#})"
                    )
                })
            })?,
            e if crate::blender::IMPORT_EXTS.contains(&e) => load_via_blender(path)?,
            other => bail!(
                "unsupported model format '.{other}' (use .obj, .glb, .gltf, .fbx, or a Blender format: {})",
                crate::blender::IMPORT_EXTS.join(", ")
            ),
        };
        model.source = path.to_path_buf();
        model.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        // Untrusted files: drop triangles whose indices point past the vertex array, so
        // no consumer (CPU raster, GPU buffers, exporters) reads out of bounds.
        for p in &mut model.pieces {
            p.drop_invalid_triangles();
        }
        model.pieces.retain(|p| !p.indices.is_empty());
        if model.pieces.is_empty() {
            bail!("{} contains no triangles", path.display());
        }
        Ok(model)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Unit-sphere-ish test mesh (UV sphere) with `seg` segments.
    pub fn uv_sphere(seg: u32) -> Piece {
        let mut vertices = Vec::new();
        let rings = seg / 2;
        for r in 0..=rings {
            let v = r as f32 / rings as f32;
            let phi = v * std::f32::consts::PI;
            for s in 0..=seg {
                let u = s as f32 / seg as f32;
                let th = u * std::f32::consts::TAU;
                let p = Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin());
                vertices.push(Vertex {
                    pos: p,
                    normal: p,
                    uv: Vec2::new(u, v),
                    color: [1.0; 4],
                    ..Default::default()
                });
            }
        }
        let mut indices = Vec::new();
        let row = seg + 1;
        for r in 0..rings {
            for s in 0..seg {
                let a = r * row + s;
                let b = a + row;
                indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        Piece {
            name: "sphere".into(),
            vertices,
            indices,
            material: None,
            skin: None,
            ..Default::default()
        }
    }

    #[test]
    fn rigid_segments_bind_every_vertex_to_one_joint() {
        let mut p = uv_sphere(12);
        p.skin = Some(0);
        for v in &mut p.vertices {
            // Blend between joints 0 (bottom) and 1 (top) across the equator.
            let t = (v.pos.y * 2.0 + 0.5).clamp(0.0, 1.0);
            v.joints = [0, 1, 0, 0];
            v.weights = [1.0 - t, t, 0.0, 0.0];
        }
        let tris = p.tri_count();
        let before = p.vertices.len();
        assert_eq!(p.rigid_segments(), 2);
        assert_eq!(p.tri_count(), tris);
        assert!(p.vertices.len() > before, "border vertices duplicated");
        assert!(p.vertices.iter().all(|v| v.weights == [1.0, 0.0, 0.0, 0.0]));
        // A triangle never mixes joints.
        for t in p.indices.chunks_exact(3) {
            let j = p.vertices[t[0] as usize].joints[0];
            assert!(t.iter().all(|&i| p.vertices[i as usize].joints[0] == j));
        }
        let mut stat = uv_sphere(4);
        assert_eq!(stat.rigid_segments(), 0, "unskinned pieces are left alone");
        // One bone per vertex: same vertices, no blending.
        let mut q = uv_sphere(12);
        q.skin = Some(0);
        for v in &mut q.vertices {
            v.joints = [0, 1, 0, 0];
            v.weights = [0.3, 0.7, 0.0, 0.0];
        }
        let n = q.vertices.len();
        assert!(q.single_bone_skinning());
        assert_eq!(q.vertices.len(), n);
        assert!(
            q.vertices
                .iter()
                .all(|v| v.joints[0] == 1 && v.weights[0] == 1.0)
        );
    }

    #[test]
    fn invalid_indices_are_dropped_with_their_triangle() {
        let mut p = uv_sphere(4);
        let n = p.vertices.len() as u32;
        let good = p.indices.len();
        p.indices.extend_from_slice(&[0, 1, n + 7, 2]);
        p.drop_invalid_triangles();
        assert_eq!(p.indices.len(), good);
        assert!(p.indices.iter().all(|&i| i < n));
    }

    #[test]
    fn sphere_area_close_to_4pi() {
        let s = uv_sphere(64);
        assert!((s.surface_area() - 4.0 * std::f32::consts::PI).abs() < 0.1);
    }

    #[test]
    fn compact_removes_unused() {
        let mut s = uv_sphere(8);
        s.indices.truncate(3);
        s.compact();
        assert_eq!(s.vertices.len(), 3);
    }

    fn quad(size: f32) -> Piece {
        let v = |x: f32, z: f32| Vertex {
            pos: Vec3::new(x, 0.0, z),
            normal: Vec3::Y,
            uv: Vec2::new(x, z),
            color: [1.0; 4],
            ..Default::default()
        };
        Piece {
            name: "q".into(),
            vertices: vec![v(0.0, 0.0), v(size, 0.0), v(size, size), v(0.0, size)],
            indices: vec![0, 2, 1, 0, 3, 2],
            material: None,
            skin: None,
            ..Default::default()
        }
    }

    #[test]
    fn subdivide_bounds_edges_keeps_area_and_has_no_cracks() {
        let mut q = quad(2.0);
        q.subdivide(0.75, 100_000);
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for t in q.indices.chunks_exact(3) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let len = (q.vertices[a as usize].pos - q.vertices[b as usize].pos).length();
                assert!(len <= 0.75 + 1e-5, "edge {len}");
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
            let p = |i: u32| q.vertices[i as usize].pos;
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            assert!(n.y > 0.0, "winding must be preserved");
        }
        assert!((q.surface_area() - 4.0).abs() < 1e-4);
        // Crack-free: edges used by only one triangle add up to the outer perimeter.
        let boundary: f32 = edges
            .iter()
            .filter(|(_, c)| **c == 1)
            .map(|((a, b), _)| (q.vertices[*a as usize].pos - q.vertices[*b as usize].pos).length())
            .sum();
        assert!((boundary - 8.0).abs() < 1e-4, "boundary {boundary}");
    }

    #[test]
    fn morph_deltas_stay_aligned_through_compact_and_subdivide() {
        let mut q = quad(2.0);
        let lift: Vec<Vec3> = q
            .vertices
            .iter()
            .map(|v| Vec3::new(0.0, v.pos.x, 0.0))
            .collect();
        q.morphs = vec![Morph {
            name: "lift".into(),
            positions: lift,
            normals: vec![Vec3::ZERO; 4],
        }];
        q.subdivide(0.75, 10_000);
        assert_eq!(q.morphs[0].positions.len(), q.vertices.len());
        // Linear morph: every vertex is lifted by its own x, including midpoints.
        for (v, d) in q.vertices.iter().zip(&q.morphs[0].positions) {
            assert!((d.y - v.pos.x).abs() < 1e-5);
        }
        let moved = q.morphed_positions(&[1.0]);
        assert!(
            moved
                .iter()
                .zip(&q.vertices)
                .all(|(m, v)| (m.y - v.pos.x).abs() < 1e-5)
        );
        q.indices.truncate(6);
        q.compact();
        assert_eq!(q.morphs[0].positions.len(), q.vertices.len());
        for (v, d) in q.vertices.iter().zip(&q.morphs[0].positions) {
            assert!((d.y - v.pos.x).abs() < 1e-5, "compact must remap deltas");
        }
    }

    #[test]
    fn subdivide_unit_face_into_grids() {
        let mut q = quad(1.0);
        q.subdivide(1.5 / 2.0, 10_000);
        assert_eq!(q.tri_count(), 8);
        let mut q = quad(1.0);
        q.subdivide(1.5 / 4.0, 10_000);
        assert_eq!(q.tri_count(), 32);
        let mut q = quad(1.0);
        q.subdivide(1.5 / 4.0, 5);
        assert!(q.tri_count() < 32, "triangle cap stops refinement");
    }

    #[test]
    fn file_references_never_leave_the_model_folder() {
        let dir = std::env::temp_dir().join("retrocrunch_safe_paths");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            safe_local_path(&dir, "tex.png").unwrap(),
            dir.join("tex.png")
        );
        assert_eq!(
            safe_local_path(&dir, "sub/tex.png").unwrap(),
            dir.join("sub/tex.png")
        );
        // Absolute, UNC, drive-relative and parent references collapse to the file name.
        for evil in [
            "C:\\Users\\victim\\tex.png",
            "\\\\attacker\\share\\tex.png",
            "//attacker/share/tex.png",
            "/etc/tex.png",
            "../../tex.png",
            "C:tex.png",
            "file://host/tex.png",
        ] {
            assert_eq!(
                safe_local_path(&dir, evil).unwrap(),
                dir.join("tex.png"),
                "{evil}"
            );
        }
        assert!(safe_local_path(&dir, "").is_none());
    }

    #[test]
    fn gltf_with_external_absolute_uri_is_rejected() {
        let dir = std::env::temp_dir().join("retrocrunch_gltf_uri");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evil.gltf");
        std::fs::write(&p, r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"\\\\attacker\\share\\b.bin","byteLength":4}]}"#).unwrap();
        let err = Model::load(&p).unwrap_err().to_string();
        assert!(err.contains("outside its folder"), "{err}");
        std::fs::write(
            &p,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"..%2F..%2Fb.bin","byteLength":4}]}"#,
        )
        .unwrap();
        assert!(
            Model::load(&p)
                .unwrap_err()
                .to_string()
                .contains("outside its folder")
        );
    }

    #[test]
    fn percent_decode_handles_crafted_input_without_panicking() {
        assert_eq!(percent_decode("..%2F..%2Fb.bin"), "../../b.bin");
        assert_eq!(percent_decode("a%2fb"), "a/b");
        // Multi-byte UTF-8 right after '%' used to panic on a char-boundary slice.
        assert_eq!(percent_decode("%é"), "%é");
        assert_eq!(percent_decode("%aé"), "%aé"); // old slice [1..3] split 'é' here
        assert_eq!(percent_decode("x%a"), "x%a");
        assert_eq!(percent_decode("x%e9%"), "x\u{FFFD}%");
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("%zz"), "%zz");
        // And the loader rejects such a URI instead of crashing.
        let dir = std::env::temp_dir().join("retrocrunch_gltf_utf8");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("utf8.gltf");
        std::fs::write(
            &p,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"%aé/../../b.bin","byteLength":4}]}"#,
        )
        .unwrap();
        assert!(Model::load(&p).is_err());
    }

    #[test]
    fn obj_mtllib_outside_folder_is_not_followed() {
        let root = std::env::temp_dir().join("retrocrunch_obj_mtl");
        let dir = root.join("model");
        std::fs::create_dir_all(&dir).unwrap();
        // A real MTL sits one folder up; the OBJ asks for it with "..".
        std::fs::write(root.join("outside.mtl"), "newmtl leaked\nKd 1 0 0\n").unwrap();
        let p = dir.join("m.obj");
        std::fs::write(
            &p,
            "mtllib ../outside.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl leaked\nf 1 2 3\n",
        )
        .unwrap();
        let m = Model::load(&p).unwrap();
        assert!(m.materials.iter().all(|m| m.name != "leaked"));
    }

    #[test]
    fn obj_import_reads_triangles() {
        let dir = std::env::temp_dir().join("retrocrunch_obj_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("quad.obj");
        std::fs::write(&p, "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\n").unwrap();
        let m = Model::load(&p).unwrap();
        assert_eq!(m.tri_count(), 2);
        assert_eq!(m.name, "quad");
    }
}
