//! Model file loading: OBJ (+MTL), glTF/GLB (meshes, materials, skins, morphs and
//! animation), and Blender-readable formats through the Blender bridge. Files are
//! untrusted: referenced textures and buffers must stay in the model's folder.

use super::{Material, Model, Morph, Piece, Vertex};
use crate::rig::{self, Rig};
use anyhow::{Context, Result, bail};
use glam::{Mat4, Quat, Vec2, Vec3};
use image::{GrayImage, RgbaImage};
use std::path::{Path, PathBuf};

/// Convert through headless Blender to a temporary GLB, then load that.
pub(super) fn load_via_blender(path: &Path) -> Result<Model> {
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
    // Judge the reference the same way on every OS. Models made on Windows use '\' and
    // drive letters, which macOS and Linux would otherwise read as part of a file name.
    let norm = name.replace('\\', "/");
    let b = norm.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let p = Path::new(&norm);
    let unsafe_ref = norm.contains("://")
        || norm.starts_with('/')
        || drive
        || p.components().any(|c| {
            matches!(
                c,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        });
    let rel = if unsafe_ref {
        // Only the last segment, without a drive ("C:tex.png" -> "tex.png").
        let last = norm.rsplit('/').next().unwrap_or("");
        let last = if drive && !norm.contains('/') {
            &last[2..]
        } else {
            last
        };
        if matches!(last, "" | "." | "..") {
            return None;
        }
        PathBuf::from(last)
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
        let as_given = dir.join(decoded.replace('\\', "/"));
        let safe =
            safe_local_path(dir, &decoded).is_some_and(|p| p == as_given) && !decoded.contains(':');
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
pub(super) fn percent_decode(s: &str) -> String {
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

pub(super) fn load_obj(path: &Path) -> Result<Model> {
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

pub(super) fn load_gltf(path: &Path) -> Result<Model> {
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
                double_sided: m.double_sided(),
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
                detail: Vec::new(),
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
