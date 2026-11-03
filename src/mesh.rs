//! In-memory model representation plus OBJ and glTF/GLB import.

use crate::rig::{self, Rig};
use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec2, Vec3};
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
    /// Remove triangles that reference vertices past the end of `vertices`.
    pub fn drop_invalid_triangles(&mut self) {
        let n = self.vertices.len();
        if self.indices.iter().all(|&i| (i as usize) < n) && self.indices.len() % 3 == 0 {
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
        self.vertices = verts;
        for m in &mut self.morphs {
            m.remap(&order);
        }
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

/// Convert through headless Blender to a temporary GLB, then load that.
fn load_via_blender(path: &Path) -> Result<Model> {
    let tmp = crate::blender::temp_glb(path);
    crate::blender::to_glb(path, &tmp)?;
    let model = load_gltf(&tmp);
    let _ = std::fs::remove_file(&tmp);
    model
}

/// Resolve a file name referenced *inside* a model file (texture, MTL, buffer) against
/// the model's folder. Absolute, drive-relative, UNC (`\\host\share`), URI and `..`
/// references are never followed as given: only their file name is looked up in `dir`.
/// Following them would let a crafted file read arbitrary paths or, on Windows, make
/// SMB connections that leak NTLM credentials.
pub(crate) fn safe_local_path(dir: &Path, name: &str) -> Option<PathBuf> {
    use std::path::Component;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let p = Path::new(name);
    let unsafe_ref = name.contains("://")
        || name.starts_with("\\\\")
        || name.starts_with("//")
        || p.components().any(|c| {
            matches!(
                c,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        });
    let rel = if unsafe_ref {
        PathBuf::from(p.file_name()?)
    } else {
        p.to_path_buf()
    };
    let full = dir.join(rel);
    // Symlinks must not lead outside the folder either.
    if let (Ok(c), Ok(d)) = (full.canonicalize(), dir.canonicalize())
        && !c.starts_with(&d)
    {
        return None;
    }
    Some(full)
}

/// Reject glTF files whose buffers/images point outside their folder (absolute paths,
/// UNC shares, other URI schemes, `..`). Embedded `data:` URIs and plain relative names are fine.
fn check_gltf_uris(path: &Path) -> Result<()> {
    let gltf = gltf::Gltf::open(path).with_context(|| format!("loading {}", path.display()))?;
    let uris = gltf
        .buffers()
        .filter_map(|b| match b.source() {
            gltf::buffer::Source::Uri(u) => Some(u.to_string()),
            gltf::buffer::Source::Bin => None,
        })
        .chain(gltf.images().filter_map(|i| match i.source() {
            gltf::image::Source::Uri { uri, .. } => Some(uri.to_string()),
            gltf::image::Source::View { .. } => None,
        }));
    for uri in uris {
        if uri.starts_with("data:") {
            continue;
        }
        let decoded = percent_decode(&uri);
        let dir = path.parent().unwrap_or(Path::new("."));
        let safe = safe_local_path(dir, &decoded).is_some_and(|p| p == dir.join(&decoded))
            && !decoded.contains(':');
        if !safe {
            bail!(
                "{} references '{uri}' outside its folder; refusing to follow it",
                path.display()
            );
        }
    }
    Ok(())
}

/// Byte-wise %XX decoding. Never slices the `str`, so multi-byte UTF-8 after a `%`
/// (e.g. `"%é"` in a crafted URI) can't hit a char boundary and panic.
fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn load_image_rgba(path: &Path) -> Option<RgbaImage> {
    match image::open(path) {
        Ok(i) => Some(i.to_rgba8()),
        Err(e) => {
            eprintln!("warning: could not load texture {}: {e}", path.display());
            None
        }
    }
}

fn load_obj(path: &Path) -> Result<Model> {
    let opts = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    // mtllib paths come from the file: resolve them only inside the model's folder.
    let file = std::fs::File::open(path).with_context(|| format!("loading {}", path.display()))?;
    let (models, mats) = tobj::load_obj_buf(&mut std::io::BufReader::new(file), &opts, |mtl| {
        let p =
            safe_local_path(dir, &mtl.to_string_lossy()).ok_or(tobj::LoadError::OpenFileFailed)?;
        tobj::load_mtl(p)
    })
    .with_context(|| format!("loading {}", path.display()))?;
    let mats = mats.unwrap_or_else(|e| {
        eprintln!("warning: MTL not loaded: {e}");
        Vec::new()
    });
    let tex_path = |s: &str| -> PathBuf {
        // Strip MTL options like "-bm 1.0 file.png": keep the last token.
        let file = s.split_whitespace().last().unwrap_or(s);
        safe_local_path(dir, file).unwrap_or_else(|| dir.join("__invalid__"))
    };
    let materials = mats
        .iter()
        .map(|m| {
            let d = m.diffuse.unwrap_or([1.0, 1.0, 1.0]);
            Material {
                name: m.name.clone(),
                base_color: [d[0], d[1], d[2], m.dissolve.unwrap_or(1.0)],
                albedo: m
                    .diffuse_texture
                    .as_deref()
                    .and_then(|t| load_image_rgba(&tex_path(t))),
                ao: m
                    .ambient_texture
                    .as_deref()
                    .and_then(|t| load_image_rgba(&tex_path(t)))
                    .map(|i| image::DynamicImage::ImageRgba8(i).to_luma8()),
                normal: m
                    .normal_texture
                    .as_deref()
                    .and_then(|t| load_image_rgba(&tex_path(t))),
                // OBJ shininess (Ns, 0..1000) mapped to roughness.
                roughness: m
                    .shininess
                    .map_or(1.0, |ns| (1.0 - (ns / 1000.0).sqrt()).clamp(0.05, 1.0)),
                ..Default::default()
            }
        })
        .collect();

    let pieces = models
        .into_iter()
        .map(|m| {
            let mesh = m.mesh;
            let n = mesh.positions.len() / 3;
            let has_n = mesh.normals.len() == n * 3;
            let has_uv = mesh.texcoords.len() == n * 2;
            let has_c = mesh.vertex_color.len() == n * 3;
            let vertices = (0..n)
                .map(|i| Vertex {
                    pos: Vec3::new(
                        mesh.positions[i * 3],
                        mesh.positions[i * 3 + 1],
                        mesh.positions[i * 3 + 2],
                    ),
                    normal: if has_n {
                        Vec3::new(
                            mesh.normals[i * 3],
                            mesh.normals[i * 3 + 1],
                            mesh.normals[i * 3 + 2],
                        )
                    } else {
                        Vec3::ZERO
                    },
                    // Internal UV convention is glTF (origin top-left): flip OBJ's V.
                    uv: if has_uv {
                        Vec2::new(mesh.texcoords[i * 2], 1.0 - mesh.texcoords[i * 2 + 1])
                    } else {
                        Vec2::ZERO
                    },
                    color: if has_c {
                        [
                            mesh.vertex_color[i * 3],
                            mesh.vertex_color[i * 3 + 1],
                            mesh.vertex_color[i * 3 + 2],
                            1.0,
                        ]
                    } else {
                        [1.0; 4]
                    },
                    ..Default::default()
                })
                .collect();
            let mut p = Piece {
                name: m.name,
                vertices,
                indices: mesh.indices,
                material: mesh.material_id,
                skin: None,
                ..Default::default()
            };
            if !has_n {
                p.recompute_normals();
            }
            p
        })
        .collect();
    Ok(Model {
        pieces,
        materials,
        ..Default::default()
    })
}

fn gltf_image_to_rgba(img: &gltf::image::Data) -> Option<RgbaImage> {
    use gltf::image::Format;
    let (w, h) = (img.width, img.height);
    let px = &img.pixels;
    let rgba: Vec<u8> = match img.format {
        Format::R8G8B8A8 => px.clone(),
        Format::R8G8B8 => px
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        Format::R8G8 => px
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[1], 0, 255])
            .collect(),
        Format::R8 => px.iter().flat_map(|&c| [c, c, c, 255]).collect(),
        Format::R16G16B16A16 => px.chunks_exact(2).map(|c| c[1]).collect(),
        Format::R16G16B16 => px
            .chunks_exact(6)
            .flat_map(|c| [c[1], c[3], c[5], 255])
            .collect(),
        _ => return None,
    };
    RgbaImage::from_raw(w, h, rgba)
}

fn load_gltf(path: &Path) -> Result<Model> {
    check_gltf_uris(path)?;
    let (doc, buffers, images) =
        gltf::import(path).with_context(|| format!("loading {}", path.display()))?;
    let tex = |t: Option<gltf::texture::Texture>| {
        t.and_then(|t| images.get(t.source().index()))
            .and_then(gltf_image_to_rgba)
    };

    let mut materials: Vec<Material> = doc
        .materials()
        .map(|m| {
            let pbr = m.pbr_metallic_roughness();
            Material {
                name: m.name().unwrap_or("material").to_string(),
                base_color: pbr.base_color_factor(),
                albedo: tex(pbr.base_color_texture().map(|i| i.texture())),
                // glTF occlusion lives in the red channel.
                ao: tex(m.occlusion_texture().map(|i| i.texture())).map(|i| {
                    GrayImage::from_fn(i.width(), i.height(), |x, y| {
                        image::Luma([i.get_pixel(x, y).0[0]])
                    })
                }),
                normal: tex(m.normal_texture().map(|i| i.texture())),
                emissive: tex(m.emissive_texture().map(|i| i.texture())),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                metal_rough: tex(pbr.metallic_roughness_texture().map(|i| i.texture())),
                emissive_factor: m.emissive_factor(),
            }
        })
        .collect();
    let default_mat = materials.len();
    let mut used_default = false;

    let mut rig = load_rig(&doc, &buffers);
    let mut parent = vec![None; doc.nodes().count()];
    for n in doc.nodes() {
        for c in n.children() {
            parent[c.index()] = Some(n.index());
        }
    }
    // Rest-pose world matrix of every node.
    let mut world = vec![Mat4::IDENTITY; parent.len()];
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let mut stack: Vec<(gltf::Node, Mat4)> = match &scene {
        Some(s) => s.nodes().map(|n| (n, Mat4::IDENTITY)).collect(),
        None => doc
            .nodes()
            .filter(|n| parent[n.index()].is_none())
            .map(|n| (n, Mat4::IDENTITY))
            .collect(),
    };
    let mut mesh_nodes = Vec::new();
    while let Some((node, p)) = stack.pop() {
        let w = p * Mat4::from_cols_array_2d(&node.transform().matrix());
        world[node.index()] = w;
        for child in node.children() {
            stack.push((child, w));
        }
        if node.mesh().is_some() {
            mesh_nodes.push(node);
        }
    }
    mesh_nodes.sort_by_key(|n| n.index());

    let mut pieces = Vec::new();
    for node in mesh_nodes {
        let Some(mesh) = node.mesh() else { continue };
        let skin = node.skin().map(|s| s.index());
        // Rigid attachment: a plain mesh below a joint, or below an animated node, follows
        // that node with weight 1 (animated nodes join a synthetic rigid skin).
        let rig_node = match (&rig, skin) {
            (Some(r), None) => rig_index_of(&doc, r, node.index(), &parent),
            _ => None,
        };
        let attach = rig_node.and_then(|n| rig.as_mut().and_then(|r| r.follow_target(n)));
        // Skinned meshes ignore their node transform (glTF spec) and stay in bind space.
        // Attachments are moved from rest-world into their joint's bind space.
        let xf = match (skin, attach, &rig) {
            (Some(_), _, _) => Mat4::IDENTITY,
            (None, Some((si, j)), Some(r)) => {
                let rest = r.skin_matrices(&r.world(&r.pose(None, 0.0)));
                rest[si][j].inverse() * world[node.index()]
            }
            _ => world[node.index()],
        };
        let target_names = morph_target_names(&mesh);
        let normal_mat = xf.inverse().transpose();
        for (pi, prim) in mesh.primitives().enumerate() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(pos) = reader.read_positions() else {
                continue;
            };
            let pos: Vec<Vec3> = pos.map(|p| xf.transform_point3(Vec3::from(p))).collect();
            let influences = if skin.is_some() {
                read_influences(&reader, pos.len())
            } else {
                None
            };
            let nrm: Option<Vec<Vec3>> = reader.read_normals().map(|n| {
                n.map(|n| {
                    normal_mat
                        .transform_vector3(Vec3::from(n))
                        .normalize_or_zero()
                })
                .collect()
            });
            let uvs: Option<Vec<Vec2>> = reader
                .read_tex_coords(0)
                .map(|t| t.into_f32().map(Vec2::from).collect());
            let cols: Option<Vec<[f32; 4]>> =
                reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            let flip = xf.determinant() < 0.0;
            let indices = if flip {
                indices
                    .chunks_exact(3)
                    .flat_map(|t| [t[0], t[2], t[1]])
                    .collect()
            } else {
                indices
            };
            let vertices = (0..pos.len())
                .map(|i| {
                    let (joints, weights) = match (&influences, attach) {
                        (Some(inf), _) => inf[i],
                        (None, Some((_, j))) => ([j as u16, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                        _ => ([0; 4], [0.0; 4]),
                    };
                    Vertex {
                        pos: pos[i],
                        normal: nrm.as_ref().map_or(Vec3::ZERO, |n| n[i]),
                        uv: uvs.as_ref().map_or(Vec2::ZERO, |u| u[i]),
                        color: cols.as_ref().map_or([1.0; 4], |c| c[i]),
                        joints,
                        weights,
                    }
                })
                .collect();
            // Names: mesh.extras.targetNames, else the target accessors' names (Blender does the same).
            let names: Vec<String> = prim
                .morph_targets()
                .enumerate()
                .map(|(i, t)| {
                    target_names
                        .get(i)
                        .cloned()
                        .or_else(|| t.positions().and_then(|a| a.name().map(str::to_string)))
                        .unwrap_or_else(|| format!("target{i}"))
                })
                .collect();
            let morphs = read_morphs(&reader, pos.len(), &xf, &normal_mat, &names);
            let piece_skin = match (skin, influences.is_some(), attach) {
                (Some(s), true, _) => Some(s),
                (None, _, Some((s, _))) => Some(s),
                _ => None,
            };
            let material = match prim.material().index() {
                Some(i) => Some(i),
                None => {
                    used_default = true;
                    Some(default_mat)
                }
            };
            let base = node.name().or(mesh.name()).unwrap_or("mesh");
            let name = if mesh.primitives().len() > 1 {
                format!("{base}.{pi}")
            } else {
                base.to_string()
            };
            let mut p = Piece {
                name,
                vertices,
                indices,
                material,
                skin: piece_skin,
                morph_group: (!morphs.is_empty()).then_some(node.index()),
                morph_weights: if morphs.is_empty() {
                    Vec::new()
                } else {
                    let mut w = mesh.weights().map(<[f32]>::to_vec).unwrap_or_default();
                    w.resize(morphs.len(), 0.0);
                    w
                },
                morphs,
            };
            if nrm.is_none() {
                p.recompute_normals();
            }
            pieces.push(p);
        }
    }
    if used_default {
        materials.push(Material::default());
    }
    Ok(Model {
        pieces,
        materials,
        rig,
        ..Default::default()
    })
}

/// Rig node index for a glTF node: itself if kept, else its nearest kept ancestor.
fn rig_index_of(
    doc: &gltf::Document,
    rig: &Rig,
    node: usize,
    parent: &[Option<usize>],
) -> Option<usize> {
    let remap = rig_remap(doc);
    let mut cur = Some(node);
    while let Some(n) = cur {
        if let Some(r) = remap[n] {
            return (r < rig.nodes.len()).then_some(r);
        }
        cur = parent[n];
    }
    None
}

/// Which glTF nodes survive into the rig: everything except pure mesh leaves
/// (meshes become pieces), so armature parents, joints and animated nodes stay.
fn rig_remap(doc: &gltf::Document) -> Vec<Option<usize>> {
    use std::collections::HashSet;
    // Only transform channels keep a node: morph-weight channels target the mesh itself.
    let animated: HashSet<usize> = doc
        .animations()
        .flat_map(|a| {
            a.channels()
                .filter(|c| c.target().property() != gltf::animation::Property::MorphTargetWeights)
                .map(|c| c.target().node().index())
                .collect::<Vec<_>>()
        })
        .collect();
    let joints: HashSet<usize> = doc
        .skins()
        .flat_map(|s| s.joints().map(|j| j.index()).collect::<Vec<_>>())
        .collect();
    let mut next = 0;
    doc.nodes()
        .map(|n| {
            let i = n.index();
            let keep = n.mesh().is_none()
                || n.children().len() > 0
                || joints.contains(&i)
                || animated.contains(&i);
            keep.then(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// Morph target names from the common `mesh.extras.targetNames` convention.
fn morph_target_names(mesh: &gltf::Mesh) -> Vec<String> {
    mesh.extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
        .and_then(|v| v.get("targetNames").cloned())
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

/// Morph targets of a primitive, transformed like the base mesh (deltas are vectors).
fn read_morphs<'a, 's, F>(
    reader: &gltf::mesh::Reader<'a, 's, F>,
    n: usize,
    xf: &Mat4,
    normal_mat: &Mat4,
    names: &[String],
) -> Vec<Morph>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    reader
        .read_morph_targets()
        .enumerate()
        .map(|(i, (pos, nrm, _tan))| {
            let positions: Vec<Vec3> = pos
                .map(|it| it.map(|d| xf.transform_vector3(Vec3::from(d))).collect())
                .unwrap_or_else(|| vec![Vec3::ZERO; n]);
            let normals: Vec<Vec3> = nrm
                .map(|it| {
                    it.map(|d| normal_mat.transform_vector3(Vec3::from(d)))
                        .collect()
                })
                .unwrap_or_else(|| vec![Vec3::ZERO; n]);
            Morph {
                name: names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("target{i}")),
                positions,
                normals,
            }
        })
        .filter(|m| m.positions.len() == n)
        .collect()
}

/// JOINTS_0/WEIGHTS_0 (+ JOINTS_1/WEIGHTS_1), reduced to the 4 strongest, normalised.
fn read_influences<'a, 's, F>(
    reader: &gltf::mesh::Reader<'a, 's, F>,
    n: usize,
) -> Option<Vec<([u16; 4], [f32; 4])>>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    type InfluenceSet = (Vec<[u16; 4]>, Vec<[f32; 4]>);
    let sets: Vec<InfluenceSet> = (0..2)
        .filter_map(|set| {
            let j: Vec<[u16; 4]> = reader.read_joints(set)?.into_u16().collect();
            let w: Vec<[f32; 4]> = reader.read_weights(set)?.into_f32().collect();
            (j.len() == n && w.len() == n).then_some((j, w))
        })
        .collect();
    if sets.is_empty() {
        return None;
    }
    Some(
        (0..n)
            .map(|i| {
                let pairs: Vec<(u16, f32)> = sets
                    .iter()
                    .flat_map(|(j, w)| (0..4).map(move |k| (j[i][k], w[i][k])))
                    .collect();
                rig::top4(&pairs)
            })
            .collect(),
    )
}

/// Node hierarchy (rest TRS), skins and animation clips.
fn load_rig(doc: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Option<Rig> {
    // Skins, node animation and morph animation all need the node tree and clips.
    if doc.skins().count() == 0 && doc.animations().count() == 0 {
        return None;
    }
    let remap = rig_remap(doc);
    let mut nodes: Vec<rig::RigNode> = Vec::new();
    for n in doc.nodes().filter(|n| remap[n.index()].is_some()) {
        let (t, r, s) = n.transform().decomposed();
        nodes.push(rig::RigNode {
            name: n
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("node{}", n.index())),
            parent: None,
            children: n.children().filter_map(|c| remap[c.index()]).collect(),
            translation: Vec3::from(t),
            rotation: Quat::from_array(r).normalize(),
            scale: Vec3::from(s),
        });
    }
    for i in 0..nodes.len() {
        for c in nodes[i].children.clone() {
            nodes[c].parent = Some(i);
        }
    }
    let roots = (0..nodes.len())
        .filter(|&i| nodes[i].parent.is_none())
        .collect();
    let get = |b: gltf::Buffer| buffers.get(b.index()).map(|d| &d.0[..]);
    let skins = doc
        .skins()
        .map(|s| {
            let joints: Vec<usize> = s.joints().filter_map(|j| remap[j.index()]).collect();
            let inverse_bind: Vec<Mat4> = s
                .reader(get)
                .read_inverse_bind_matrices()
                .map(|m| m.map(|c| Mat4::from_cols_array_2d(&c)).collect())
                .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
            rig::Skin {
                name: s.name().unwrap_or("skin").to_string(),
                joints,
                inverse_bind,
                skeleton: s.skeleton().and_then(|n| remap[n.index()]),
            }
        })
        .collect();
    let clips = doc
        .animations()
        .enumerate()
        .map(|(ai, a)| rig::Clip {
            name: a
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("clip{ai}")),
            channels: a
                .channels()
                .filter_map(|c| read_channel(&c, &remap, get))
                .collect(),
        })
        .collect();
    Some(Rig {
        nodes,
        roots,
        skins,
        clips,
    })
}

fn read_channel<'a, 's>(
    c: &gltf::animation::Channel<'a>,
    remap: &[Option<usize>],
    get: impl Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
) -> Option<rig::Channel> {
    use gltf::animation::util::ReadOutputs;
    let gltf_node = c.target().node().index();
    let r = c.reader(get);
    let times: Vec<f32> = r.read_inputs()?.collect();
    let interpolation = match c.sampler().interpolation() {
        gltf::animation::Interpolation::Linear => rig::Interpolation::Linear,
        gltf::animation::Interpolation::Step => rig::Interpolation::Step,
        gltf::animation::Interpolation::CubicSpline => rig::Interpolation::CubicSpline,
    };
    let (property, values): (rig::Property, Vec<f32>) = match r.read_outputs()? {
        ReadOutputs::Translations(v) => (rig::Property::Translation, v.flatten().collect()),
        ReadOutputs::Rotations(v) => (rig::Property::Rotation, v.into_f32().flatten().collect()),
        ReadOutputs::Scales(v) => (rig::Property::Scale, v.flatten().collect()),
        ReadOutputs::MorphTargetWeights(v) => {
            let values: Vec<f32> = v.into_f32().collect();
            let keys = times.len()
                * if interpolation == rig::Interpolation::CubicSpline {
                    3
                } else {
                    1
                };
            if keys == 0 || !values.len().is_multiple_of(keys) {
                return None;
            }
            // Weight channels address the mesh's morph group (its glTF node index).
            return Some(rig::Channel {
                node: gltf_node,
                property: rig::Property::Weights(values.len() / keys),
                interpolation,
                times,
                values,
            });
        }
    };
    let node = remap[gltf_node]?;
    Some(rig::Channel {
        node,
        property,
        interpolation,
        times,
        values,
    })
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
        let dir = std::env::temp_dir().join("psxforge_safe_paths");
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
        let dir = std::env::temp_dir().join("psxforge_gltf_uri");
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
        let dir = std::env::temp_dir().join("psxforge_gltf_utf8");
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
        let root = std::env::temp_dir().join("psxforge_obj_mtl");
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
        let dir = std::env::temp_dir().join("psxforge_obj_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("quad.obj");
        std::fs::write(&p, "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\n").unwrap();
        let m = Model::load(&p).unwrap();
        assert_eq!(m.tri_count(), 2);
        assert_eq!(m.name, "quad");
    }
}
