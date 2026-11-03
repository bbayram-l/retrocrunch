//! Native FBX import through ufbx: meshes, materials and textures, skin clusters,
//! blend shapes and baked animation stacks. Converted to metres, Y-up, right-handed.

use crate::mesh::{Material, Model, Morph, Piece, Vertex};
use crate::rig::{self, Channel, Clip, Interpolation, Property, Rig, RigNode, Skin};
use anyhow::{Result, anyhow};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;

fn v3(v: ufbx::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

fn quat(q: ufbx::Quat) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32).normalize()
}

fn mat(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(m.m00 as f32, m.m10 as f32, m.m20 as f32, 0.0),
        Vec4::new(m.m01 as f32, m.m11 as f32, m.m21 as f32, 0.0),
        Vec4::new(m.m02 as f32, m.m12 as f32, m.m22 as f32, 0.0),
        Vec4::new(m.m03 as f32, m.m13 as f32, m.m23 as f32, 1.0),
    )
}

fn load_texture(tex: &ufbx::Texture, dir: &Path) -> Option<RgbaImage> {
    if !tex.content.is_empty()
        && let Ok(i) = image::load_from_memory(&tex.content)
    {
        return Some(i.to_rgba8());
    }
    let names = [
        tex.absolute_filename.as_ref(),
        tex.relative_filename.as_ref(),
        tex.filename.as_ref(),
    ];
    // FBX files often carry the author's absolute paths: only look inside the model's
    // folder (and its textures/ sub-folder), never at the path as written.
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for n in names.iter().filter(|n| !n.is_empty()) {
        candidates.extend(crate::mesh::safe_local_path(dir, n));
        if let Some(file) = Path::new(n).file_name().and_then(|f| f.to_str()) {
            candidates.extend(crate::mesh::safe_local_path(&dir.join("textures"), file));
        }
    }
    candidates
        .iter()
        .find_map(|c| image::open(c).ok())
        .map(|i| i.to_rgba8())
}

fn map_texture<'a>(maps: &[&'a ufbx::MaterialMap]) -> Option<&'a ufbx::Texture> {
    maps.iter().find_map(|m| m.texture.as_deref())
}

fn convert_material(m: &ufbx::Material, dir: &Path) -> Material {
    let color = if m.pbr.base_color.has_value {
        m.pbr.base_color.value_vec4
    } else {
        m.fbx.diffuse_color.value_vec4
    };
    let base = if m.pbr.base_color.has_value || m.fbx.diffuse_color.has_value {
        [color.x as f32, color.y as f32, color.z as f32, 1.0]
    } else {
        [1.0; 4]
    };
    let albedo =
        map_texture(&[&m.pbr.base_color, &m.fbx.diffuse_color]).and_then(|t| load_texture(t, dir));
    Material {
        name: m.element.name.to_string(),
        // A texture already carries the colour; FBX exporters often leave diffuse at grey.
        base_color: if albedo.is_some() { [1.0; 4] } else { base },
        albedo,
        ao: map_texture(&[&m.pbr.ambient_occlusion])
            .and_then(|t| load_texture(t, dir))
            .map(|i| image::DynamicImage::ImageRgba8(i).to_luma8()),
        normal: map_texture(&[&m.pbr.normal_map, &m.fbx.normal_map])
            .and_then(|t| load_texture(t, dir)),
        emissive: map_texture(&[&m.pbr.emission_color, &m.fbx.emission_color])
            .and_then(|t| load_texture(t, dir)),
        metallic: if m.pbr.metalness.has_value {
            m.pbr.metalness.value_vec4.x as f32
        } else {
            0.0
        },
        roughness: if m.pbr.roughness.has_value {
            m.pbr.roughness.value_vec4.x as f32
        } else {
            0.8
        },
        // An emission texture is meant to glow at full strength.
        emissive_factor: if m.pbr.emission_color.texture.is_some()
            || m.fbx.emission_color.texture.is_some()
        {
            [1.0; 3]
        } else {
            [0.0; 3]
        },
        ..Default::default()
    }
}

/// Rig node index per FBX node (the scene root is dropped).
struct NodeMap {
    rig_of: Vec<Option<usize>>,
}

fn build_rig_nodes(scene: &ufbx::Scene) -> (Vec<RigNode>, Vec<usize>, NodeMap) {
    let mut rig_of = vec![None; scene.nodes.len()];
    let mut nodes = Vec::new();
    for n in scene.nodes.iter() {
        if n.is_root {
            continue;
        }
        rig_of[n.element.typed_id as usize] = Some(nodes.len());
        let t = &n.local_transform;
        nodes.push(RigNode {
            name: n.element.name.to_string(),
            parent: None,
            children: Vec::new(),
            translation: v3(t.translation),
            rotation: quat(t.rotation),
            scale: v3(t.scale),
        });
    }
    let mut roots = Vec::new();
    for n in scene.nodes.iter() {
        let Some(me) = rig_of[n.element.typed_id as usize] else {
            continue;
        };
        match n
            .parent
            .as_ref()
            .and_then(|p| rig_of[p.element.typed_id as usize])
        {
            Some(p) => {
                nodes[me].parent = Some(p);
                nodes[p].children.push(me);
            }
            None => roots.push(me),
        }
    }
    (nodes, roots, NodeMap { rig_of })
}

fn bake_clips(
    scene: &ufbx::Scene,
    map: &NodeMap,
    blend_targets: &HashMap<u32, (usize, usize)>,
    group_sizes: &HashMap<usize, usize>,
) -> Vec<Clip> {
    let mut clips = Vec::new();
    for stack in scene.anim_stacks.iter() {
        let opts = ufbx::BakeOpts {
            trim_start_time: true,
            ..Default::default()
        };
        let Ok(baked) = ufbx::bake_anim(scene, &stack.anim, opts) else {
            continue;
        };
        let mut channels = Vec::new();
        for bn in baked.nodes.iter() {
            let Some(node) = map.rig_of.get(bn.typed_id as usize).copied().flatten() else {
                continue;
            };
            let push =
                |channels: &mut Vec<Channel>, property, times: Vec<f32>, values: Vec<f32>| {
                    if !times.is_empty() {
                        channels.push(Channel {
                            node,
                            property,
                            interpolation: Interpolation::Linear,
                            times,
                            values,
                        });
                    }
                };
            push(
                &mut channels,
                Property::Translation,
                bn.translation_keys.iter().map(|k| k.time as f32).collect(),
                bn.translation_keys
                    .iter()
                    .flat_map(|k| v3(k.value).to_array())
                    .collect(),
            );
            push(
                &mut channels,
                Property::Rotation,
                bn.rotation_keys.iter().map(|k| k.time as f32).collect(),
                bn.rotation_keys
                    .iter()
                    .flat_map(|k| quat(k.value).to_array())
                    .collect(),
            );
            push(
                &mut channels,
                Property::Scale,
                bn.scale_keys.iter().map(|k| k.time as f32).collect(),
                bn.scale_keys
                    .iter()
                    .flat_map(|k| v3(k.value).to_array())
                    .collect(),
            );
        }
        // Blend-shape weights: one baked prop per blend channel ("DeformPercent", 0-100).
        // Merge them per morph group onto a shared timeline.
        // (target index, [(time, weight)]) per morph group.
        type WeightTrack = (usize, Vec<(f32, f32)>);
        let mut per_group: HashMap<usize, Vec<WeightTrack>> = HashMap::new();
        for el in baked.elements.iter() {
            let Some(&(group, target)) = blend_targets.get(&el.element_id) else {
                continue;
            };
            for prop in el
                .props
                .iter()
                .filter(|p| p.name.as_ref() == "DeformPercent")
            {
                let keys: Vec<(f32, f32)> = prop
                    .keys
                    .iter()
                    .map(|k| (k.time as f32, k.value.x as f32 / 100.0))
                    .collect();
                per_group.entry(group).or_default().push((target, keys));
            }
        }
        for (group, tracks) in per_group {
            let n = group_sizes.get(&group).copied().unwrap_or(0);
            if n == 0 {
                continue;
            }
            let mut times: Vec<f32> = tracks
                .iter()
                .flat_map(|(_, k)| k.iter().map(|x| x.0))
                .collect();
            times.sort_by(f32::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            let sample = |keys: &[(f32, f32)], t: f32| -> f32 {
                match keys.iter().position(|k| k.0 >= t) {
                    None => keys.last().map_or(0.0, |k| k.1),
                    Some(0) => keys[0].1,
                    Some(i) => {
                        let (a, b) = (keys[i - 1], keys[i]);
                        a.1 + (b.1 - a.1) * ((t - a.0) / (b.0 - a.0).max(1e-9))
                    }
                }
            };
            let mut values = Vec::with_capacity(times.len() * n);
            for &t in &times {
                for target in 0..n {
                    values.push(
                        tracks
                            .iter()
                            .find(|(i, _)| *i == target)
                            .map_or(0.0, |(_, k)| sample(k, t)),
                    );
                }
            }
            channels.push(Channel {
                node: group,
                property: Property::Weights(n),
                interpolation: Interpolation::Linear,
                times,
                values,
            });
        }
        if !channels.is_empty() {
            clips.push(Clip {
                name: stack.element.name.to_string(),
                channels,
            });
        }
    }
    clips
}

pub fn load_fbx(path: &Path) -> Result<Model> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        ignore_missing_external_files: true,
        ..Default::default()
    };
    let path_str = path.to_str().ok_or_else(|| anyhow!("non UTF-8 path"))?;
    let scene = ufbx::load_file(path_str, opts)
        .map_err(|e| anyhow!("FBX load failed: {:?}", e.description.as_ref() as &str))?;
    let dir = path.parent().unwrap_or(Path::new("."));

    let mut materials: Vec<Material> = scene
        .materials
        .iter()
        .map(|m| convert_material(m, dir))
        .collect();
    let default_mat = materials.len();
    let mut used_default = false;

    let (nodes, roots, map) = build_rig_nodes(&scene);
    let mut rig = Rig {
        nodes,
        roots,
        ..Default::default()
    };

    // Morph bookkeeping for animation baking: blend channel element id -> (group, target).
    let mut blend_targets: HashMap<u32, (usize, usize)> = HashMap::new();
    let mut group_sizes: HashMap<usize, usize> = HashMap::new();
    for n in scene.nodes.iter() {
        let Some(mesh) = n.mesh.as_ref() else {
            continue;
        };
        let group = n.element.typed_id as usize;
        let mut k = 0;
        for bd in mesh.blend_deformers.iter() {
            for ch in bd.channels.iter() {
                blend_targets.insert(ch.element.element_id, (group, k));
                k += 1;
            }
        }
        if k > 0 {
            group_sizes.insert(group, k);
        }
    }
    rig.clips = bake_clips(&scene, &map, &blend_targets, &group_sizes);

    let mut skin_index: HashMap<u32, usize> = HashMap::new();
    let mut pieces = Vec::new();
    for n in scene.nodes.iter() {
        let Some(mesh) = n.mesh.as_ref() else {
            continue;
        };
        let deformer = mesh.skin_deformers.iter().find(|d| !d.clusters.is_empty());
        // Skin: positions in bind-world space, IBM_j = geometry_to_bone_j * G^-1.
        let (skin, geom_xf) = match deformer {
            Some(d) => {
                let g = mat(&d.clusters[0].geometry_to_world);
                let si = *skin_index.entry(d.element.typed_id).or_insert_with(|| {
                    let joints = d
                        .clusters
                        .iter()
                        .map(|c| {
                            c.bone_node
                                .as_ref()
                                .and_then(|b| map.rig_of[b.element.typed_id as usize])
                                .unwrap_or(0)
                        })
                        .collect();
                    let inverse_bind = d
                        .clusters
                        .iter()
                        .map(|c| mat(&c.geometry_to_bone) * g.inverse())
                        .collect();
                    rig.skins.push(Skin {
                        name: d.element.name.to_string(),
                        joints,
                        inverse_bind,
                        skeleton: None,
                    });
                    rig.skins.len() - 1
                });
                (Some(si), g)
            }
            None => (None, mat(&n.geometry_to_world)),
        };
        // Plain meshes under joints or animated nodes follow them rigidly.
        let attach = if skin.is_none() {
            map.rig_of[n.element.typed_id as usize].and_then(|r| rig.follow_target(r))
        } else {
            None
        };
        let xf = match attach {
            Some((si, j)) => {
                let rest = rig.skin_matrices(&rig.world(&rig.pose(None, 0.0)));
                rest[si][j].inverse() * geom_xf
            }
            None => geom_xf,
        };
        let normal_mat = xf.inverse().transpose();

        // Per-vertex (control point) skin influences.
        let influences: Option<Vec<([u16; 4], [f32; 4])>> = deformer.map(|d| {
            (0..mesh.num_vertices)
                .map(|vi| {
                    let sv = d.vertices.get(vi);
                    let pairs: Vec<(u16, f32)> = sv
                        .map(|sv| {
                            (sv.weight_begin..sv.weight_begin + sv.num_weights)
                                .filter_map(|w| d.weights.get(w as usize))
                                .map(|w| (w.cluster_index as u16, w.weight as f32))
                                .collect()
                        })
                        .unwrap_or_default();
                    rig::top4(&pairs)
                })
                .collect()
        });

        // Blend shapes: dense per-control-point deltas.
        let mut shapes: Vec<(String, f32, Vec<Vec3>, Vec<Vec3>)> = Vec::new();
        for bd in mesh.blend_deformers.iter() {
            for ch in bd.channels.iter() {
                let shape = ch
                    .target_shape
                    .as_ref()
                    .or_else(|| ch.keyframes.last().map(|k| &k.shape));
                let mut dp = vec![Vec3::ZERO; mesh.num_vertices];
                let mut dn = vec![Vec3::ZERO; mesh.num_vertices];
                if let Some(sh) = shape {
                    for k in 0..sh.num_offsets {
                        let vi = sh.offset_vertices[k] as usize;
                        if vi < dp.len() {
                            dp[vi] = xf.transform_vector3(v3(sh.position_offsets[k]));
                            if let Some(nn) = sh.normal_offsets.get(k) {
                                dn[vi] = normal_mat.transform_vector3(v3(*nn));
                            }
                        }
                    }
                }
                shapes.push((ch.element.name.to_string(), ch.weight as f32, dp, dn));
            }
        }

        let mats: Vec<usize> = (if n.materials.is_empty() {
            mesh.materials.iter().collect::<Vec<_>>()
        } else {
            n.materials.iter().collect()
        })
        .iter()
        .map(|m| m.element.typed_id as usize)
        .collect();
        let mut tri = Vec::new();
        for part in mesh.material_parts.iter() {
            if part.num_triangles == 0 {
                continue;
            }
            let material = match mats.get(part.index as usize) {
                Some(&m) => Some(m),
                None => {
                    used_default = true;
                    Some(default_mat)
                }
            };
            let mut dedup: HashMap<(u32, [u32; 9]), u32> = HashMap::new();
            let mut vertices: Vec<Vertex> = Vec::new();
            let mut source: Vec<usize> = Vec::new();
            let mut indices = Vec::new();
            for &fi in part.face_indices.iter() {
                let face = mesh.faces[fi as usize];
                ufbx::triangulate_face_vec(&mut tri, mesh, face);
                for &ix in &tri {
                    let ix = ix as usize;
                    let vi = mesh.vertex_indices[ix];
                    let nrm = if mesh.vertex_normal.exists {
                        v3(mesh.vertex_normal[ix])
                    } else {
                        Vec3::ZERO
                    };
                    let uv = if mesh.vertex_uv.exists {
                        let t = mesh.vertex_uv[ix];
                        Vec2::new(t.x as f32, 1.0 - t.y as f32)
                    } else {
                        Vec2::ZERO
                    };
                    let col = if mesh.vertex_color.exists {
                        let c = mesh.vertex_color[ix];
                        [c.x as f32, c.y as f32, c.z as f32, c.w as f32]
                    } else {
                        [1.0; 4]
                    };
                    let key = (
                        vi,
                        [
                            nrm.x.to_bits(),
                            nrm.y.to_bits(),
                            nrm.z.to_bits(),
                            uv.x.to_bits(),
                            uv.y.to_bits(),
                            col[0].to_bits(),
                            col[1].to_bits(),
                            col[2].to_bits(),
                            col[3].to_bits(),
                        ],
                    );
                    let id = *dedup.entry(key).or_insert_with(|| {
                        let (joints, weights) = match (&influences, attach) {
                            (Some(inf), _) => inf[vi as usize],
                            (None, Some((_, j))) => ([j as u16, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                            _ => ([0; 4], [0.0; 4]),
                        };
                        vertices.push(Vertex {
                            pos: xf.transform_point3(v3(mesh.vertices[vi as usize])),
                            normal: normal_mat.transform_vector3(nrm).normalize_or_zero(),
                            uv,
                            color: col,
                            joints,
                            weights,
                        });
                        source.push(vi as usize);
                        vertices.len() as u32 - 1
                    });
                    indices.push(id);
                }
            }
            if xf.determinant() < 0.0 {
                for t in indices.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            let morphs: Vec<Morph> = shapes
                .iter()
                .map(|(name, _, dp, dn)| Morph {
                    name: name.clone(),
                    positions: source.iter().map(|&s| dp[s]).collect(),
                    normals: source.iter().map(|&s| dn[s]).collect(),
                })
                .collect();
            let name = if mesh.material_parts.len() > 1 {
                format!("{}.{}", n.element.name.as_ref() as &str, part.index)
            } else {
                n.element.name.to_string()
            };
            let mut p = Piece {
                name,
                vertices,
                indices,
                material,
                skin: skin.or(attach.map(|a| a.0)),
                morph_group: (!morphs.is_empty()).then_some(n.element.typed_id as usize),
                morph_weights: shapes.iter().map(|s| s.1).collect(),
                morphs,
            };
            if p.vertices.iter().all(|v| v.normal == Vec3::ZERO) {
                p.recompute_normals();
            }
            pieces.push(p);
        }
    }
    if used_default {
        materials.push(Material::default());
    }
    let keep_rig = !rig.skins.is_empty() || !rig.clips.is_empty();
    Ok(Model {
        pieces,
        materials,
        rig: keep_rig.then_some(rig),
        ..Default::default()
    })
}
