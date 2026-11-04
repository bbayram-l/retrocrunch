//! Fresh UV atlas + texture rebake.
//!
//! * `unwrap_atlas` splits each piece into charts of faces whose normals stay within a
//!   crease angle of the chart's seed (so projection never folds), projects every chart
//!   onto its plane at a uniform texel density, and shelf-packs all charts into one
//!   square atlas with gutters.
//! * `BakeSource` holds the full-resolution source surface (including "baked loose
//!   pieces") in a BVH; `bake_atlas` shoots a ray per atlas texel from just outside the
//!   low-poly surface back along its normal and samples the outermost source hit, so
//!   detail floating on the surface (pockets, straps, decals) is painted into the texture.

use crate::mesh::{Material, Piece, Vertex};
use crate::texture::{self, BakeMaps};
use glam::{Vec2, Vec3, Vec4};
use image::RgbaImage;
use std::collections::HashMap;

// ------------------------------------------------------------------------- unwrap

/// Result of unwrapping: for every piece, the original vertex each new vertex came from
/// (so per-vertex data computed before the split, like lighting, can be carried over).
pub struct Unwrap {
    pub remap: Vec<Vec<u32>>,
    pub charts: usize,
    /// Texels per world unit actually used.
    pub texel_density: f32,
}

struct Chart {
    piece: usize,
    tris: Vec<usize>,
    /// Per corner (tri-local order) projected 2D position, world units.
    corners: Vec<[Vec2; 3]>,
    lo: Vec2,
    size: Vec2,
}

fn tri_normal(p: &Piece, t: usize) -> (Vec3, f32) {
    let i = &p.indices[t * 3..t * 3 + 3];
    let (a, b, c) = (
        p.vertices[i[0] as usize].pos,
        p.vertices[i[1] as usize].pos,
        p.vertices[i[2] as usize].pos,
    );
    let n = (b - a).cross(c - a);
    let len = n.length();
    (if len > 1e-12 { n / len } else { Vec3::Y }, len * 0.5)
}

/// Grow charts over shared (position-welded) edges while normals stay within `angle_deg`
/// of the seed normal (largest triangle first). Tiny leftover charts (slivers from
/// decimation) then merge into a neighbour whose seed normal is within 80 degrees, and
/// each chart is projected onto its average normal when that keeps every triangle
/// unflipped (else onto its seed normal, which always does).
fn build_charts(pieces: &[Piece], angle_deg: f32) -> Vec<Chart> {
    let cos_max = angle_deg.clamp(1.0, 89.0).to_radians().cos();
    let cos_merge = 80f32.to_radians().cos();
    let mut charts = Vec::new();
    for (pi, p) in pieces.iter().enumerate() {
        let nt = p.tri_count();
        if nt == 0 {
            continue;
        }
        let (lo, hi) = p.vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(l, h), v| (l.min(v.pos), h.max(v.pos)),
        );
        let q = 1e5 / (hi - lo).max_element().max(1e-6);
        let key = |v: &Vertex| ((v.pos - lo) * q).round().as_ivec3().to_array();
        let weld: Vec<[i32; 3]> = p.vertices.iter().map(key).collect();
        let edge = |t: usize, k: usize| {
            let (a, b) = (
                weld[p.indices[t * 3 + k] as usize],
                weld[p.indices[t * 3 + (k + 1) % 3] as usize],
            );
            (a.min(b), a.max(b))
        };
        let mut edges: HashMap<([i32; 3], [i32; 3]), Vec<usize>> = HashMap::new();
        for t in 0..nt {
            for k in 0..3 {
                edges.entry(edge(t, k)).or_default().push(t);
            }
        }
        let normals: Vec<(Vec3, f32)> = (0..nt).map(|t| tri_normal(p, t)).collect();
        let mut order: Vec<usize> = (0..nt).collect();
        order.sort_by(|&a, &b| normals[b].1.total_cmp(&normals[a].1));

        // 1. Grow.
        let mut chart_of = vec![usize::MAX; nt];
        let mut groups: Vec<(Vec3, Vec<usize>)> = Vec::new();
        for seed in order {
            if chart_of[seed] != usize::MAX {
                continue;
            }
            let id = groups.len();
            let n0 = normals[seed].0;
            let mut tris = vec![seed];
            chart_of[seed] = id;
            let mut head = 0;
            while head < tris.len() {
                let t = tris[head];
                head += 1;
                for k in 0..3 {
                    for &nb in &edges[&edge(t, k)] {
                        if chart_of[nb] == usize::MAX && normals[nb].0.dot(n0) >= cos_max {
                            chart_of[nb] = id;
                            tris.push(nb);
                        }
                    }
                }
            }
            groups.push((n0, tris));
        }

        // 2. Merge slivers into a neighbouring chart they can project onto.
        let mut small: Vec<usize> = (0..groups.len())
            .filter(|&g| groups[g].1.len() <= 2)
            .collect();
        small.sort_by_key(|&g| groups[g].1.len());
        for g in small {
            let tris = groups[g].1.clone();
            let mut best: Option<(f32, usize)> = None;
            for &t in &tris {
                for k in 0..3 {
                    for &nb in &edges[&edge(t, k)] {
                        let h = chart_of[nb];
                        if h == g || groups[h].1.is_empty() {
                            continue;
                        }
                        let host_n = groups[h].0;
                        let worst = tris
                            .iter()
                            .map(|&x| normals[x].0.dot(host_n))
                            .fold(f32::MAX, f32::min);
                        if worst >= cos_merge && best.is_none_or(|b| worst > b.0) {
                            best = Some((worst, h));
                        }
                    }
                }
            }
            if let Some((_, h)) = best {
                for &t in &tris {
                    chart_of[t] = h;
                }
                groups[h].1.extend(tris);
                groups[g].1.clear();
            }
        }

        // 3. Project.
        for (seed_n, tris) in groups.into_iter().filter(|g| !g.1.is_empty()) {
            let avg = tris
                .iter()
                .map(|&t| normals[t].0 * normals[t].1)
                .sum::<Vec3>()
                .normalize_or(seed_n);
            let axis = if tris.iter().all(|&t| normals[t].0.dot(avg) > 0.1) {
                avg
            } else {
                seed_n
            };
            let (tx, ty) = axis.any_orthonormal_pair();
            let corners: Vec<[Vec2; 3]> = tris
                .iter()
                .map(|&t| {
                    std::array::from_fn(|k| {
                        let v = p.vertices[p.indices[t * 3 + k] as usize].pos;
                        Vec2::new(v.dot(tx), v.dot(ty))
                    })
                })
                .collect();
            let mut chart = Chart {
                piece: pi,
                tris,
                corners,
                lo: Vec2::ZERO,
                size: Vec2::ZERO,
            };
            orient_chart(&mut chart);
            charts.push(chart);
        }
    }
    charts
}

/// Rotate a chart (15 degree steps) to the smallest bounding box, then measure it.
fn orient_chart(c: &mut Chart) {
    let bbox = |pts: &[[Vec2; 3]], rot: Vec2| -> (Vec2, Vec2) {
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        for p in pts.iter().flatten() {
            let r = rot.rotate(*p);
            lo = lo.min(r);
            hi = hi.max(r);
        }
        (lo, hi)
    };
    let best = (0..6)
        .map(|k| Vec2::from_angle((k as f32 * 15.0).to_radians()))
        .min_by(|a, b| {
            let (la, ha) = bbox(&c.corners, *a);
            let (lb, hb) = bbox(&c.corners, *b);
            ((ha - la).x * (ha - la).y).total_cmp(&((hb - lb).x * (hb - lb).y))
        })
        .unwrap_or(Vec2::X);
    for tri in &mut c.corners {
        for p in tri.iter_mut() {
            *p = best.rotate(*p);
        }
    }
    let (lo, hi) = bbox(&c.corners, Vec2::X);
    c.lo = lo;
    c.size = (hi - lo).max(Vec2::ZERO);
}

/// Shelf packing at `scale` texels per unit. Returns each chart's texel origin.
fn shelf_pack(
    charts: &[Chart],
    order: &[usize],
    scale: f32,
    n: u32,
    pad: u32,
) -> Option<Vec<(u32, u32)>> {
    let mut pos = vec![(0u32, 0u32); charts.len()];
    let (mut x, mut y, mut row_h) = (pad, pad, 0u32);
    for &ci in order {
        let w = (charts[ci].size.x * scale).ceil() as u32 + 1;
        let h = (charts[ci].size.y * scale).ceil() as u32 + 1;
        if x + w + pad > n {
            x = pad;
            y += row_h + pad;
            row_h = 0;
        }
        if x + w + pad > n || y + h + pad > n {
            return None;
        }
        pos[ci] = (x, y);
        x += w + pad;
        row_h = row_h.max(h);
    }
    Some(pos)
}

/// Replace every piece's UVs with a packed `n x n` atlas. Vertices are split along chart
/// seams (skin weights, colours and morph deltas are copied with them).
/// Returns None when the charts can't fit (the caller keeps the original UVs).
pub fn unwrap_atlas(pieces: &mut [Piece], n: u32, pad: u32, angle_deg: f32) -> Option<Unwrap> {
    unwrap_atlas_with_detail(pieces, n, pad, angle_deg, None)
}

/// `detail`: (field, strength). Charts in high-priority areas (the face) get up to
/// `strength` times the texel density of the rest.
pub fn unwrap_atlas_with_detail(
    pieces: &mut [Piece],
    n: u32,
    pad: u32,
    angle_deg: f32,
    detail: Option<(&crate::detail::DetailField, f32)>,
) -> Option<Unwrap> {
    let mut charts = build_charts(pieces, angle_deg);
    if charts.is_empty() {
        return None;
    }
    if let Some((field, strength)) = detail.filter(|(f, s)| !f.is_empty() && *s > 1.0) {
        for c in &mut charts {
            let p = &pieces[c.piece];
            let (mut sum, mut area) = (0.0f32, 0.0f32);
            for &t in &c.tris {
                let (_, a) = tri_normal(p, t);
                let i = &p.indices[t * 3..t * 3 + 3];
                let centre = (p.vertices[i[0] as usize].pos
                    + p.vertices[i[1] as usize].pos
                    + p.vertices[i[2] as usize].pos)
                    / 3.0;
                sum += field.at(centre) * a;
                area += a;
            }
            let b = 1.0 + (strength - 1.0) * (sum / area.max(1e-12));
            for corners in &mut c.corners {
                for q in corners.iter_mut() {
                    *q *= b;
                }
            }
            c.lo *= b;
            c.size *= b;
        }
    }
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| charts[b].size.y.total_cmp(&charts[a].size.y));
    let area: f32 = charts
        .iter()
        .map(|c| (c.size.x * c.size.y).max(1e-12))
        .sum();
    // Binary search the largest uniform density that still packs; drop the gutter
    // before giving up so dense meshes in tiny textures still unwrap.
    let mut found = None;
    for pad in [pad, 1, 0] {
        let mut hi = (n as f32 * n as f32 / area).sqrt();
        let mut lo = 0.0;
        let mut best = None;
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            match shelf_pack(&charts, &order, mid, n, pad) {
                Some(p) => {
                    best = Some((mid, p));
                    lo = mid;
                }
                None => hi = mid,
            }
        }
        if best.is_some() {
            found = best;
            break;
        }
    }
    let (scale, pos) = found?;

    let mut remap: Vec<Vec<u32>> = vec![Vec::new(); pieces.len()];
    let mut new_vertices: Vec<Vec<Vertex>> = vec![Vec::new(); pieces.len()];
    let mut new_morphs: Vec<Vec<(Vec<Vec3>, Vec<Vec3>)>> = pieces
        .iter()
        .map(|p| vec![(Vec::new(), Vec::new()); p.morphs.len()])
        .collect();
    let mut new_indices: Vec<Vec<u32>> = pieces.iter().map(|p| vec![0; p.indices.len()]).collect();
    for (ci, c) in charts.iter().enumerate() {
        let p = &pieces[c.piece];
        let origin = Vec2::new(pos[ci].0 as f32 + 0.5, pos[ci].1 as f32 + 0.5);
        let mut local: HashMap<u32, u32> = HashMap::new();
        for (k, &t) in c.tris.iter().enumerate() {
            for corner in 0..3 {
                let old = p.indices[t * 3 + corner];
                let id = *local.entry(old).or_insert_with(|| {
                    let mut v = p.vertices[old as usize];
                    v.uv = (origin + (c.corners[k][corner] - c.lo) * scale) / n as f32;
                    let verts = &mut new_vertices[c.piece];
                    verts.push(v);
                    remap[c.piece].push(old);
                    for (m, out) in p.morphs.iter().zip(new_morphs[c.piece].iter_mut()) {
                        out.0
                            .push(m.positions.get(old as usize).copied().unwrap_or_default());
                        out.1
                            .push(m.normals.get(old as usize).copied().unwrap_or_default());
                    }
                    verts.len() as u32 - 1
                });
                new_indices[c.piece][t * 3 + corner] = id;
            }
        }
    }
    for (pi, p) in pieces.iter_mut().enumerate() {
        if p.indices.is_empty() {
            continue;
        }
        p.vertices = std::mem::take(&mut new_vertices[pi]);
        p.indices = std::mem::take(&mut new_indices[pi]);
        for (m, (dp, dn)) in p.morphs.iter_mut().zip(std::mem::take(&mut new_morphs[pi])) {
            m.positions = dp;
            m.normals = dn;
        }
    }
    Some(Unwrap {
        remap,
        charts: charts.len(),
        texel_density: scale,
    })
}

// ------------------------------------------------------------------------- source BVH

struct SrcTri {
    p: [Vec3; 3],
    uv: [Vec2; 3],
    col: [Vec4; 3],
    mat: Option<usize>,
    n: Vec3,
}

#[derive(Clone, Copy)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// Leaf: first triangle (in `order`) and count. Inner: left child index, count = 0.
    first: u32,
    count: u32,
}

struct SourceMat {
    image: Option<RgbaImage>,
    base: Vec4,
}

/// The high-resolution surface the atlas is baked from.
pub struct BakeSource {
    tris: Vec<SrcTri>,
    nodes: Vec<Node>,
    order: Vec<u32>,
    mats: Vec<SourceMat>,
    pub diagonal: f32,
}

/// Maps baked into the source albedo before sampling (same options as the texture panel).
pub struct SourceMaps {
    pub ao: bool,
    pub ao_strength: f32,
    pub normal: bool,
    pub normal_strength: f32,
    pub emissive: bool,
}

impl BakeSource {
    pub fn new(pieces: &[Piece], materials: &[Material], maps: &SourceMaps) -> Self {
        let mut tris = Vec::new();
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in pieces {
            for t in p.indices.chunks_exact(3) {
                let v: [&Vertex; 3] = std::array::from_fn(|k| &p.vertices[t[k] as usize]);
                let pos = v.map(|v| v.pos);
                let n = (pos[1] - pos[0]).cross(pos[2] - pos[0]);
                if n.length_squared() < 1e-20 {
                    continue;
                }
                for q in pos {
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
                tris.push(SrcTri {
                    p: pos,
                    uv: v.map(|v| v.uv),
                    col: v.map(|v| Vec4::from(v.color)),
                    mat: p.material,
                    n: n.normalize(),
                });
            }
        }
        let mats = materials
            .iter()
            .map(|m| SourceMat {
                image: m.albedo.as_ref().map(|a| {
                    let mut img = a.clone();
                    texture::bake_maps(
                        &mut img,
                        &BakeMaps {
                            ao: m.ao.as_ref().filter(|_| maps.ao),
                            ao_strength: maps.ao_strength,
                            normal: m.normal.as_ref().filter(|_| maps.normal),
                            normal_strength: maps.normal_strength,
                            emissive: m.emissive.as_ref().filter(|_| maps.emissive),
                        },
                    );
                    img
                }),
                base: Vec4::from(m.base_color),
            })
            .collect();
        let mut s = BakeSource {
            order: (0..tris.len() as u32).collect(),
            tris,
            nodes: Vec::new(),
            mats,
            diagonal: (hi - lo).length().max(1e-6),
        };
        if !s.tris.is_empty() {
            let n = s.order.len();
            s.nodes.push(Node {
                lo: Vec3::ZERO,
                hi: Vec3::ZERO,
                first: 0,
                count: 0,
            });
            s.build(0, 0, n);
        }
        s
    }

    fn build(&mut self, node: usize, start: usize, end: usize) {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &i in &self.order[start..end] {
            let t = &self.tris[i as usize];
            for p in t.p {
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let c = (t.p[0] + t.p[1] + t.p[2]) / 3.0;
            clo = clo.min(c);
            chi = chi.max(c);
        }
        self.nodes[node].lo = lo;
        self.nodes[node].hi = hi;
        let count = end - start;
        let extent = chi - clo;
        if count <= 4 || extent.max_element() < 1e-9 {
            self.nodes[node].first = start as u32;
            self.nodes[node].count = count as u32;
            return;
        }
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mid = start + count / 2;
        let tris = &self.tris;
        self.order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
            let ca = tris[a as usize].p.iter().map(|p| p[axis]).sum::<f32>();
            let cb = tris[b as usize].p.iter().map(|p| p[axis]).sum::<f32>();
            ca.total_cmp(&cb)
        });
        let left = self.nodes.len();
        self.nodes.push(Node {
            lo,
            hi,
            first: 0,
            count: 0,
        });
        self.nodes.push(Node {
            lo,
            hi,
            first: 0,
            count: 0,
        });
        self.nodes[node].first = left as u32;
        self.nodes[node].count = 0;
        self.build(left, start, mid);
        self.build(left + 1, mid, end);
    }

    fn ray_box(n: &Node, o: Vec3, inv: Vec3, tmax: f32) -> bool {
        let t0 = (n.lo - o) * inv;
        let t1 = (n.hi - o) * inv;
        let tn = t0.min(t1).max_element();
        let tf = t0.max(t1).min_element();
        tf >= tn.max(0.0) && tn <= tmax
    }

    /// Nearest hit along the ray within [0, tmax]. `front_only` ignores triangles whose
    /// normal faces away from the ray origin.
    fn ray(&self, o: Vec3, d: Vec3, tmax: f32, front_only: bool) -> Option<(usize, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(f32, usize, Vec3)> = None;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            let limit = best.map_or(tmax, |b| b.0);
            if !Self::ray_box(&n, o, inv, limit) {
                continue;
            }
            if n.count == 0 {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
                continue;
            }
            for &ti in &self.order[n.first as usize..(n.first + n.count) as usize] {
                let t = &self.tris[ti as usize];
                if front_only && t.n.dot(d) >= 0.0 {
                    continue;
                }
                // Moller-Trumbore, two-sided.
                let (e1, e2) = (t.p[1] - t.p[0], t.p[2] - t.p[0]);
                let pv = d.cross(e2);
                let det = e1.dot(pv);
                if det.abs() < 1e-14 {
                    continue;
                }
                let inv_det = 1.0 / det;
                let tv = o - t.p[0];
                let u = tv.dot(pv) * inv_det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let qv = tv.cross(e1);
                let v = d.dot(qv) * inv_det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let dist = e2.dot(qv) * inv_det;
                if dist >= 0.0 && dist <= best.map_or(tmax, |b| b.0) {
                    best = Some((dist, ti as usize, Vec3::new(1.0 - u - v, u, v)));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// Closest point on the source surface within `max_d`.
    fn closest(&self, p: Vec3, max_d: f32) -> Option<(usize, Vec3)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(f32, usize, Vec3)> = None;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            let d2 = (p - p.clamp(n.lo, n.hi)).length_squared();
            if d2 > best.map_or(max_d * max_d, |b| b.0) {
                continue;
            }
            if n.count == 0 {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
                continue;
            }
            for &ti in &self.order[n.first as usize..(n.first + n.count) as usize] {
                let t = &self.tris[ti as usize];
                let (q, bary) = closest_on_triangle(p, t.p[0], t.p[1], t.p[2]);
                let d2 = (q - p).length_squared();
                if d2 <= best.map_or(max_d * max_d, |b| b.0) {
                    best = Some((d2, ti as usize, bary));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// Colour of the source surface at a barycentric point (texture x base colour x vertex colour).
    fn sample(&self, ti: usize, bary: Vec3) -> Vec4 {
        let t = &self.tris[ti];
        let uv = t.uv[0] * bary.x + t.uv[1] * bary.y + t.uv[2] * bary.z;
        let vcol = t.col[0] * bary.x + t.col[1] * bary.y + t.col[2] * bary.z;
        let mat = t.mat.and_then(|m| self.mats.get(m));
        let base = mat.map_or(Vec4::ONE, |m| m.base);
        // Triangles laid out inside 0..1 clamp (V = 1 at a pole must not wrap to the top
        // row); only genuinely tiling UVs wrap.
        let tiling =
            t.uv.iter()
                .any(|u| u.x < -1e-4 || u.y < -1e-4 || u.x > 1.0001 || u.y > 1.0001);
        let tex = mat
            .and_then(|m| m.image.as_ref())
            .map_or(Vec4::ONE, |img| bilinear(img, uv, tiling));
        tex * base * vcol
    }

    /// Outermost source colour seen from `p` along `-n`, starting `cage` above the surface.
    pub fn color_at(&self, p: Vec3, n: Vec3, cage: f32) -> Option<Vec4> {
        let o = p + n * cage;
        let hit = self
            .ray(o, -n, cage * 2.0, true)
            .or_else(|| self.ray(o, -n, cage * 2.0, false))
            .or_else(|| self.closest(p, cage * 2.0));
        hit.map(|(t, b)| self.sample(t, b))
    }
}

/// Ericson, Real-Time Collision Detection 5.1.5. Returns the point and its barycentrics.
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Vec3) {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, Vec3::X);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, Vec3::Y);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, Vec3::new(1.0 - v, v, 0.0));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, Vec3::Z);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, Vec3::new(1.0 - w, 0.0, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, Vec3::new(0.0, 1.0 - w, w));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (a + ab * v + ac * w, Vec3::new(1.0 - v - w, v, w))
}

fn bilinear(img: &RgbaImage, uv: Vec2, wrap: bool) -> Vec4 {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let uv = if wrap {
        uv - uv.floor()
    } else {
        uv.clamp(Vec2::ZERO, Vec2::ONE)
    };
    let x = uv.x * w as f32 - 0.5;
    let y = uv.y * h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let px = |xi: i32, yi: i32| {
        let (xi, yi) = if wrap {
            (xi.rem_euclid(w), yi.rem_euclid(h))
        } else {
            (xi.clamp(0, w - 1), yi.clamp(0, h - 1))
        };
        let p = img.get_pixel(xi as u32, yi as u32).0;
        Vec4::new(p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32) / 255.0
    };
    let (x0, y0) = (x0 as i32, y0 as i32);
    let top = px(x0, y0).lerp(px(x0 + 1, y0), fx);
    let bottom = px(x0, y0 + 1).lerp(px(x0 + 1, y0 + 1), fx);
    top.lerp(bottom, fy)
}

// ------------------------------------------------------------------------- bake

/// Point on triangle `t` of `p` for a texel-space UV point (barycentrics clamped to the
/// triangle so samples near chart edges stay on the surface).
fn surface_at(p: &Piece, t: usize, uv_px: [Vec2; 3], pt: Vec2) -> (Vec3, Vec3) {
    let v: [&Vertex; 3] = std::array::from_fn(|k| &p.vertices[p.indices[t * 3 + k] as usize]);
    let area = (uv_px[1] - uv_px[0]).perp_dot(uv_px[2] - uv_px[0]);
    let (mut w0, mut w1) = if area.abs() > 1e-12 {
        (
            (uv_px[1] - pt).perp_dot(uv_px[2] - pt) / area,
            (uv_px[2] - pt).perp_dot(uv_px[0] - pt) / area,
        )
    } else {
        (1.0 / 3.0, 1.0 / 3.0)
    };
    let mut w2 = 1.0 - w0 - w1;
    w0 = w0.max(0.0);
    w1 = w1.max(0.0);
    w2 = w2.max(0.0);
    let sum = (w0 + w1 + w2).max(1e-9);
    let face_n = (v[1].pos - v[0].pos)
        .cross(v[2].pos - v[0].pos)
        .normalize_or(Vec3::Y);
    let pos = (v[0].pos * w0 + v[1].pos * w1 + v[2].pos * w2) / sum;
    let nrm = (v[0].normal * w0 + v[1].normal * w1 + v[2].normal * w2).normalize_or(face_n);
    (pos, nrm)
}

impl BakeSource {
    /// Samples per texel side so one atlas texel averages the source texels it covers
    /// (a box filter, like the resize used in Keep mode) instead of aliasing.
    pub fn supersample_for(&self, atlas: u32) -> u32 {
        let src_px: f64 = self
            .mats
            .iter()
            .filter_map(|m| m.image.as_ref())
            .map(|i| i.width() as f64 * i.height() as f64)
            .sum();
        let ratio = src_px / (atlas as f64 * atlas as f64);
        (ratio.sqrt().ceil() as u32).clamp(2, 6)
    }
}

/// Bake the source into an `n x n` atlas laid out by the pieces' (atlas) UVs.
/// `cage` is how far above the low-poly surface rays start (world units); every texel
/// averages `ss x ss` samples.
pub fn bake_atlas(pieces: &[Piece], n: u32, src: &BakeSource, cage: f32, ss: u32) -> RgbaImage {
    // 1. Which triangle owns each covered texel.
    let mut owner = vec![false; (n * n) as usize];
    let mut jobs: Vec<(u32, u32, u32)> = Vec::new();
    for (pi, p) in pieces.iter().enumerate() {
        for (ti, t) in p.indices.chunks_exact(3).enumerate() {
            let uv: [Vec2; 3] = std::array::from_fn(|k| p.vertices[t[k] as usize].uv * n as f32);
            let area = (uv[1] - uv[0]).perp_dot(uv[2] - uv[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            let lo = uv[0].min(uv[1]).min(uv[2]).floor().max(Vec2::ZERO);
            let hi = uv[0]
                .max(uv[1])
                .max(uv[2])
                .ceil()
                .min(Vec2::splat(n as f32 - 1.0));
            let before = jobs.len();
            // Slightly conservative so chart edges are fully covered.
            let eps = -0.5 / (area.abs().sqrt().max(1.0));
            for y in lo.y as u32..=hi.y as u32 {
                for x in lo.x as u32..=hi.x as u32 {
                    let idx = (y * n + x) as usize;
                    if owner[idx] {
                        continue;
                    }
                    let pt = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = (uv[1] - pt).perp_dot(uv[2] - pt) / area;
                    let w1 = (uv[2] - pt).perp_dot(uv[0] - pt) / area;
                    if w0 < eps || w1 < eps || 1.0 - w0 - w1 < eps {
                        continue;
                    }
                    owner[idx] = true;
                    jobs.push((idx as u32, pi as u32, ti as u32));
                }
            }
            // Slivers smaller than a texel still claim the texel under their centroid.
            if jobs.len() == before {
                let c = ((uv[0] + uv[1] + uv[2]) / 3.0)
                    .floor()
                    .clamp(Vec2::ZERO, Vec2::splat(n as f32 - 1.0));
                let idx = (c.y as u32 * n + c.x as u32) as usize;
                if !owner[idx] {
                    owner[idx] = true;
                    jobs.push((idx as u32, pi as u32, ti as u32));
                }
            }
        }
    }
    // 2. Trace ss x ss samples per texel in parallel and average the hits.
    let ss = ss.max(1);
    let trace = |&(idx, pi, ti): &(u32, u32, u32)| -> Option<Vec4> {
        let p = &pieces[pi as usize];
        let t = ti as usize;
        let uv: [Vec2; 3] =
            std::array::from_fn(|k| p.vertices[p.indices[t * 3 + k] as usize].uv * n as f32);
        let (x, y) = ((idx % n) as f32, (idx / n) as f32);
        let mut acc = Vec4::ZERO;
        let mut hits = 0;
        for j in 0..ss {
            for i in 0..ss {
                let pt = Vec2::new(
                    x + (i as f32 + 0.5) / ss as f32,
                    y + (j as f32 + 0.5) / ss as f32,
                );
                let (pos, nrm) = surface_at(p, t, uv, pt);
                if let Some(c) = src.color_at(pos, nrm, cage) {
                    acc += c;
                    hits += 1;
                }
            }
        }
        (hits > 0).then(|| acc / hits as f32)
    };
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(32);
    let chunk = jobs.len().div_ceil(threads).max(1);
    let colors: Vec<Option<Vec4>> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(trace).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut img = RgbaImage::new(n, n);
    let mut filled = vec![false; (n * n) as usize];
    for ((idx, _, _), c) in jobs.iter().zip(colors) {
        if let Some(c) = c {
            let c = (c.clamp(Vec4::ZERO, Vec4::ONE) * 255.0).round();
            img.as_mut()[*idx as usize * 4..*idx as usize * 4 + 4]
                .copy_from_slice(&[c.x as u8, c.y as u8, c.z as u8, c.w as u8]);
            filled[*idx as usize] = true;
        }
    }
    dilate(&mut img, &mut filled, 8);
    img
}

/// Grow filled texels into empty ones (gutter bleed for nearest sampling), then fill
/// whatever is left with the mean colour so unused atlas space is never transparent.
fn dilate(img: &mut RgbaImage, filled: &mut [bool], passes: usize) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for _ in 0..passes {
        let src = img.clone();
        let was = filled.to_vec();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if was[i] {
                    continue;
                }
                let mut acc = [0u32; 4];
                let mut k = 0;
                for (dx, dy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, 1),
                    (-1, 1),
                    (1, -1),
                ] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < w && ny < h && was[(ny * w + nx) as usize] {
                        let p = src.get_pixel(nx as u32, ny as u32).0;
                        for c in 0..4 {
                            acc[c] += p[c] as u32;
                        }
                        k += 1;
                    }
                }
                if k > 0 {
                    img.put_pixel(x as u32, y as u32, image::Rgba(acc.map(|a| (a / k) as u8)));
                    filled[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let n = filled.iter().filter(|f| **f).count().max(1) as u64;
    let mut mean = [0u64; 4];
    for (i, p) in img.pixels().enumerate() {
        if filled[i] {
            for (m, &v) in mean.iter_mut().zip(&p.0) {
                *m += v as u64;
            }
        }
    }
    let mean = mean.map(|m| (m / n) as u8);
    for (i, p) in img.pixels_mut().enumerate() {
        if !filled[i] {
            p.0 = [mean[0], mean[1], mean[2], 255];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::uv_sphere;

    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    #[test]
    fn bvh_queries_match_brute_force() {
        let s = uv_sphere(24);
        let src = BakeSource::new(
            std::slice::from_ref(&s),
            &[],
            &SourceMaps {
                ao: false,
                ao_strength: 0.0,
                normal: false,
                normal_strength: 0.0,
                emissive: false,
            },
        );
        let mut seed = 7;
        for _ in 0..200 {
            let p = Vec3::new(
                lcg(&mut seed) * 4.0 - 2.0,
                lcg(&mut seed) * 4.0 - 2.0,
                lcg(&mut seed) * 4.0 - 2.0,
            );
            let (ti, bary) = src.closest(p, 100.0).unwrap();
            let t = &src.tris[ti];
            let q = t.p[0] * bary.x + t.p[1] * bary.y + t.p[2] * bary.z;
            let brute = src
                .tris
                .iter()
                .map(|t| (closest_on_triangle(p, t.p[0], t.p[1], t.p[2]).0 - p).length())
                .fold(f32::MAX, f32::min);
            assert!(((q - p).length() - brute).abs() < 1e-4);
            // Ray from outside toward the centre hits the sphere near radius 1.
            let d = (-p).normalize();
            let (ti, bary) = src.ray(p * 3.0, d, 100.0, true).unwrap();
            let t = &src.tris[ti];
            let hit = t.p[0] * bary.x + t.p[1] * bary.y + t.p[2] * bary.z;
            assert!(
                (hit.length() - 1.0).abs() < 0.02,
                "hit radius {}",
                hit.length()
            );
        }
    }

    #[test]
    fn unwrap_packs_non_overlapping_charts_inside_the_atlas() {
        let mut pieces = vec![uv_sphere(16)];
        let tris = pieces[0].tri_count();
        let u = unwrap_atlas(&mut pieces, 128, 2, 60.0).unwrap();
        assert!(
            u.charts >= 4,
            "a sphere needs several charts, got {}",
            u.charts
        );
        assert_eq!(pieces[0].tri_count(), tris);
        assert_eq!(u.remap[0].len(), pieces[0].vertices.len());
        // Rasterise each triangle's UV footprint: no texel may belong to two charts.
        let n = 128u32;
        let mut owner: Vec<i64> = vec![-1; (n * n) as usize];
        let p = &pieces[0];
        // Chart id per triangle = connected component in the new (split) vertex graph.
        let mut parent: Vec<usize> = (0..p.vertices.len()).collect();
        fn find(p: &mut Vec<usize>, i: usize) -> usize {
            if p[i] != i {
                let r = find(p, p[i]);
                p[i] = r;
            }
            p[i]
        }
        for t in p.indices.chunks_exact(3) {
            for k in 1..3 {
                let (a, b) = (
                    find(&mut parent, t[0] as usize),
                    find(&mut parent, t[k] as usize),
                );
                parent[a] = b;
            }
        }
        for t in p.indices.chunks_exact(3) {
            let chart = find(&mut parent, t[0] as usize) as i64;
            let uv: Vec<Vec2> = t
                .iter()
                .map(|&i| p.vertices[i as usize].uv * n as f32)
                .collect();
            for q in &uv {
                assert!(q.x >= 0.0 && q.y >= 0.0 && q.x <= n as f32 && q.y <= n as f32);
            }
            let area = (uv[1] - uv[0]).perp_dot(uv[2] - uv[0]);
            if area.abs() < 1e-6 {
                continue;
            }
            for y in 0..n {
                for x in 0..n {
                    let pt = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = (uv[1] - pt).perp_dot(uv[2] - pt) / area;
                    let w1 = (uv[2] - pt).perp_dot(uv[0] - pt) / area;
                    if w0 > 0.01 && w1 > 0.01 && 1.0 - w0 - w1 > 0.01 {
                        let o = &mut owner[(y * n + x) as usize];
                        assert!(*o == -1 || *o == chart, "charts overlap at texel {x},{y}");
                        *o = chart;
                    }
                }
            }
        }
        let used = owner.iter().filter(|o| **o >= 0).count() as f32 / (n * n) as f32;
        assert!(used > 0.25, "atlas should be reasonably full, used {used}");
    }

    #[test]
    fn rebake_reproduces_source_texture_colours() {
        // Source: sphere with red on top, blue below, by texture.
        let mut src_piece = uv_sphere(48);
        src_piece.material = Some(0);
        let tex = RgbaImage::from_fn(64, 64, |_, y| {
            if y < 32 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let mats = vec![Material {
            albedo: Some(tex),
            ..Default::default()
        }];
        let src = BakeSource::new(
            &[src_piece.clone()],
            &mats,
            &SourceMaps {
                ao: false,
                ao_strength: 0.0,
                normal: false,
                normal_strength: 0.0,
                emissive: false,
            },
        );
        let mut low = vec![crate::decimate::decimate(
            &src_piece,
            120,
            &Default::default(),
        )];
        unwrap_atlas(&mut low, 64, 2, 60.0).unwrap();
        let img = bake_atlas(&low, 64, &src, src.diagonal * 0.03, 2);
        // Sample at triangle centroids (vertices sit on chart edges, where gutters blend):
        // triangles entirely away from the equator must show their hemisphere colour.
        let p = &low[0];
        let mut checked = 0;
        for t in p.indices.chunks_exact(3) {
            let v: Vec<&Vertex> = t.iter().map(|&i| &p.vertices[i as usize]).collect();
            if !(v.iter().all(|v| v.pos.y > 0.3) || v.iter().all(|v| v.pos.y < -0.3)) {
                continue;
            }
            let uv = (v[0].uv + v[1].uv + v[2].uv) / 3.0;
            let px = img
                .get_pixel(
                    ((uv.x * 64.0) as u32).min(63),
                    ((uv.y * 64.0) as u32).min(63),
                )
                .0;
            checked += 1;
            if v[0].pos.y > 0.0 {
                assert!(px[0] > 200 && px[2] < 60, "top should be red, got {px:?}");
            } else {
                assert!(
                    px[2] > 200 && px[0] < 60,
                    "bottom should be blue, got {px:?}"
                );
            }
        }
        assert!(checked > 20, "only {checked} triangles checked");
    }

    #[test]
    fn loose_piece_floating_on_surface_is_baked_in() {
        // Grey floor quad (2x2) and a small red patch 1 cm above its centre.
        let quad = |size: f32, y: f32, color: [f32; 4]| {
            let v = |x: f32, z: f32| Vertex {
                pos: Vec3::new(x, y, z),
                normal: Vec3::Y,
                uv: Vec2::new(x, z),
                color,
                ..Default::default()
            };
            Piece {
                name: "q".into(),
                vertices: vec![
                    v(-size, -size),
                    v(size, -size),
                    v(size, size),
                    v(-size, size),
                ],
                indices: vec![0, 2, 1, 0, 3, 2],
                material: None,
                ..Default::default()
            }
        };
        let floor = quad(1.0, 0.0, [0.5, 0.5, 0.5, 1.0]);
        let patch = quad(0.25, 0.01, [1.0, 0.0, 0.0, 1.0]);
        let src = BakeSource::new(
            &[floor.clone(), patch],
            &[],
            &SourceMaps {
                ao: false,
                ao_strength: 0.0,
                normal: false,
                normal_strength: 0.0,
                emissive: false,
            },
        );
        let mut out = vec![floor];
        unwrap_atlas(&mut out, 32, 1, 60.0).unwrap();
        let img = bake_atlas(&out, 32, &src, 0.05, 2);
        // UV of the floor centre: interpolate from the quad's corners.
        let uv_center =
            out[0].vertices.iter().map(|v| v.uv).sum::<Vec2>() / out[0].vertices.len() as f32;
        let c = img
            .get_pixel((uv_center.x * 32.0) as u32, (uv_center.y * 32.0) as u32)
            .0;
        assert!(
            c[0] > 200 && c[1] < 60,
            "patch colour must be painted onto the floor, got {c:?}"
        );
        let corner = out[0]
            .vertices
            .iter()
            .map(|v| v.uv)
            .fold(Vec2::splat(9.0), Vec2::min)
            * 32.0
            + Vec2::splat(1.5);
        let g = img.get_pixel(corner.x as u32, corner.y as u32).0;
        assert!(
            (g[0] as i32 - 128).abs() < 20 && (g[1] as i32 - 128).abs() < 20,
            "floor stays grey away from the patch, got {g:?}"
        );
    }
}
