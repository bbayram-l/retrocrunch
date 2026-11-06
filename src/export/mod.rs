//! Exporters: OBJ+MTL, binary glTF (GLB) with nearest-filtered textures, and Godot 4 scenes.
//!
//! `godot` writes the Godot side (scenes, shader variants, import settings, sprite
//! scenes); `render` makes previews and sprite sheets with the CPU rasterizer.

mod godot;
mod render;

pub use godot::*;
use godot::{godot_common_for, is_hd};
pub use render::*;

use crate::mesh::Model;
use crate::rig::Property;
use crate::texture;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnginePreset {
    Godot,
    Unity,
    Unreal,
    Blender,
    Generic,
}

impl EnginePreset {
    pub const ALL: [EnginePreset; 5] = [
        Self::Godot,
        Self::Unity,
        Self::Unreal,
        Self::Blender,
        Self::Generic,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Godot => "Godot 4",
            Self::Unity => "Unity",
            Self::Unreal => "Unreal 5",
            Self::Blender => "Blender",
            Self::Generic => "Generic",
        }
    }

    pub fn settings(self) -> ExportSettings {
        let base = ExportSettings::default();
        match self {
            Self::Godot => ExportSettings {
                preset: self,
                glb: true,
                obj: false,
                godot_scene: true,
                ..base
            },
            // Unity has no built-in glTF importer: FBX via Blender keeps rigs and clips
            // (Humanoid/Mixamo-ready); OBJ is the fallback when Blender isn't installed.
            Self::Unity => ExportSettings {
                preset: self,
                glb: false,
                obj: false,
                fbx: true,
                godot_scene: false,
                ..base
            },
            Self::Unreal => ExportSettings {
                preset: self,
                glb: true,
                obj: false,
                godot_scene: false,
                scale: 100.0,
                ..base
            },
            Self::Blender => ExportSettings {
                preset: self,
                glb: true,
                obj: false,
                godot_scene: false,
                ..base
            },
            Self::Generic => ExportSettings {
                preset: self,
                glb: true,
                obj: true,
                godot_scene: false,
                ..base
            },
        }
    }

    pub fn import_notes(self) -> &'static str {
        match self {
            Self::Godot => {
                "Godot 4: copy the folder into your project. Open the .tscn: every mesh already uses a psx_*.gdshader material (vertex snapping, affine UVs, dither, 15-bit colour, nearest filtering): psx_opaque, psx_cutout (alpha-tested) or psx_blend (translucent), with _double variants for double-sided materials. Shared code is in psx_common.gdshaderinc. Tweak 'snap_lines' on a material for more or less jitter. Keep the Environment tonemap on Linear for colours that match the retrocrunch preview.\n\nScreen pass (optional, recommended): add a Control with psx_screen.gd as your main scene's root UI node and put the 3D level (with its camera) under it. It renders at 240 lines, upscales with nearest filtering and does the PS1 dither + 15-bit once per virtual pixel (psx_post.gdshader), switching those off in the materials.\n\nPS1 speed (optional): add a Node with psx_speed_limit.gd inside the 3D scene. It counts the triangles drawn each frame and caps the frame rate like the PS1 GPU (180 textured polygons per ms): 60 fps up to 3000, 30 up to 6000, 20 up to 9000. Turn on show_stats to see the count.\n\nDraw distance (optional, debug): add a Node with psx_draw_distance.gd inside the 3D scene. It finds how far the camera can see before the objects in view pass a polygon budget (6000 by default) and cuts everything farther, or tints it red, so you can see how far each scene can draw.\n\nThe .png.import files tell Godot to keep textures lossless and without VRAM compression, so the palette survives; HD textures (over 512 px) get mipmaps. The .glb.import files turn off Godot's automatic mesh LODs, so models are always drawn with the triangles you converted them to."
            }
            Self::Unity => {
                "Unity: drop the .fbx (or .obj) and .png into Assets. For rigged models set Rig > Animation Type = Humanoid (or Generic) and apply. Select the PNG: Filter Mode = Point, Compression = None, Generate Mip Maps = off. Use an Unlit/Texture (or URP Unlit) material; vertex colours need a vertex-colour shader."
            }
            Self::Unreal => {
                "Unreal 5: Import the .glb (Interchange). In the texture asset set Filter = Nearest, Mip Gen = NoMipmaps, Compression = UserInterface2D. Use an Unlit material multiplying Texture Sample by Vertex Color."
            }
            Self::Blender => {
                "Blender: File > Import > glTF. In the material's Image Texture node set interpolation to Closest."
            }
            Self::Generic => {
                "OBJ + MTL + PNG and GLB exported. Textures are meant for nearest-neighbour filtering without mipmaps."
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub preset: EnginePreset,
    pub glb: bool,
    pub obj: bool,
    pub godot_scene: bool,
    pub indexed_png: bool,
    pub unlit: bool,
    pub scale: f32,
    /// Also write a 320x240 PS1-style render of the model.
    #[serde(default = "yes")]
    pub preview: bool,
    /// Binary FBX converted from the GLB by headless Blender (rigs and clips kept).
    #[serde(default)]
    pub fbx: bool,
    /// Extra levels of detail, each with half the triangles of the previous one
    /// (`<name>_lod1.glb`, ...). Godot scenes of static models switch between them by
    /// distance (visibility ranges). 0 = off.
    #[serde(default)]
    pub lods: u32,
}

fn yes() -> bool {
    true
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            preset: EnginePreset::Godot,
            glb: true,
            obj: false,
            godot_scene: true,
            indexed_png: true,
            unlit: true,
            scale: 1.0,
            preview: true,
            fbx: false,
            lods: 0,
        }
    }
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "mesh".into() } else { s }
}

fn unique_names(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let base = sanitize(&n);
        let mut cand = base.clone();
        let mut k = 1;
        while out.contains(&cand) {
            cand = format!("{base}_{k}");
            k += 1;
        }
        out.push(cand);
    }
    out
}

/// Texture file name for material `i` (None if untextured).
pub fn texture_file(model: &Model, i: usize) -> Option<String> {
    model.materials.get(i)?.albedo.as_ref()?;
    let base = sanitize(&model.name);
    Some(
        if model
            .materials
            .iter()
            .filter(|m| m.albedo.is_some())
            .count()
            == 1
        {
            format!("{base}.png")
        } else {
            format!("{base}_{}.png", sanitize(&model.materials[i].name))
        },
    )
}

/// Write textures, models and engine helpers into `dir`. Returns written files.
pub fn export_model(model: &Model, dir: &Path, s: &ExportSettings) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // Scaling a rig means scaling translations, bind matrices and keys too.
    let scaled = model.scaled(s.scale);
    let model = &scaled;
    let mut written = Vec::new();
    for (i, m) in model.materials.iter().enumerate() {
        if let (Some(img), Some(name)) = (&m.albedo, texture_file(model, i)) {
            let p = dir.join(name);
            texture::save_png(&p, img, s.indexed_png)?;
            written.push(p);
        }
    }
    let base = sanitize(&model.name);
    if s.obj {
        let p = dir.join(format!("{base}.obj"));
        write_obj(model, &p)?;
        written.push(p.clone());
        written.push(p.with_extension("mtl"));
    }
    let mut warnings: Vec<String> = Vec::new();
    let lod_count = s.lods.min(4);
    if s.glb || s.godot_scene || s.fbx {
        let p = dir.join(format!("{base}.glb"));
        std::fs::write(&p, glb_bytes(model, s.unlit)?)?;
        for lod in lod_models(model, lod_count) {
            let lp = dir.join(format!("{}.glb", sanitize(&lod.name)));
            std::fs::write(&lp, glb_bytes(&lod, s.unlit)?)?;
            written.push(lp);
        }
        if s.fbx {
            let f = dir.join(format!("{base}.fbx"));
            match crate::blender::to_fbx(&p, &f) {
                Ok(()) => written.push(f),
                Err(e) => {
                    warnings.push(format!("FBX skipped ({e:#}); wrote OBJ instead."));
                    if !s.obj {
                        let o = dir.join(format!("{base}.obj"));
                        write_obj(model, &o)?;
                        written.push(o.clone());
                        written.push(o.with_extension("mtl"));
                    }
                }
            }
        }
        if s.glb || s.godot_scene {
            written.push(p);
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
    if s.godot_scene {
        let glbs: Vec<PathBuf> = written
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "glb"))
            .cloned()
            .collect();
        for g in glbs {
            let p = g.with_extension("glb.import");
            std::fs::write(&p, godot_model_import())?;
            written.push(p);
        }
        let common = dir.join("psx_common.gdshaderinc");
        std::fs::write(&common, godot_common_for(model))?;
        written.push(common);
        for v in godot_variants(model) {
            let p = dir.join(v.file_name());
            std::fs::write(&p, v.source())?;
            written.push(p);
        }
        for (name, text) in [
            ("psx_screen.gd", GODOT_SCREEN_SCRIPT),
            ("psx_post.gdshader", GODOT_POST_SHADER),
            ("psx_speed_limit.gd", GODOT_SPEED_SCRIPT),
            ("psx_draw_distance.gd", GODOT_DRAW_DISTANCE_SCRIPT),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, text)?;
            written.push(p);
        }
        for (i, m) in model.materials.iter().enumerate() {
            if let (Some(img), Some(name)) = (&m.albedo, texture_file(model, i)) {
                let p = dir.join(format!("{name}.import"));
                std::fs::write(&p, godot_texture_import(is_hd(img)))?;
                written.push(p);
            }
        }
        let p = dir.join(format!("{base}.tscn"));
        std::fs::write(&p, godot_scene_lods(model, lod_count))?;
        written.push(p);
        if model.is_rigged() {
            let gd = dir.join(GODOT_APPLY_SCRIPT_NAME);
            std::fs::write(&gd, GODOT_APPLY_SCRIPT)?;
            written.push(gd);
        }
    }
    if s.preview {
        let p = dir.join(format!("{base}_preview.png"));
        preview_image(model, 320, 240, 0.6).save(&p)?;
        written.push(p);
    }
    let notes = dir.join("IMPORT_NOTES.txt");
    let mut text = format!(
        "Made with retrocrunch {}.\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        s.preset.import_notes()
    );
    if let Some(r) = model.rig.as_ref().filter(|_| model.is_rigged()) {
        let clips: Vec<&str> = r.clips.iter().map(|c| c.name.as_str()).collect();
        text.push_str(&format!(
            "\nRig: {} joints in {} skin(s); clips: {}.\nThe GLB keeps the skeleton, skin weights (4 per vertex) and animations. The OBJ, if written, is static (bind pose).\n",
            r.joint_count(),
            r.skins.len(),
            if clips.is_empty() { "none".to_string() } else { clips.join(", ") }
        ));
    }
    for w in &warnings {
        text.push_str(&format!("\nWarning: {w}\n"));
    }
    text.push_str("\nGenerated by retrocrunch.\n");
    std::fs::write(&notes, text)?;
    written.push(notes);
    Ok(written)
}

/// Static OBJ (rigged models are written in their bind pose).
pub fn write_obj(model: &Model, path: &Path) -> Result<()> {
    let mtl_name = path
        .with_extension("mtl")
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("model.mtl")
        .to_string();
    let mut o = String::from("# retrocrunch\n");
    writeln!(o, "mtllib {mtl_name}")?;
    let names = unique_names(model.pieces.iter().map(|p| p.name.clone()));
    let mut base = 1usize;
    for (p, name) in model.pieces.iter().zip(&names) {
        writeln!(o, "o {name}")?;
        for v in &p.vertices {
            let q = v.pos;
            writeln!(
                o,
                "v {:.6} {:.6} {:.6} {:.4} {:.4} {:.4}",
                q.x, q.y, q.z, v.color[0], v.color[1], v.color[2]
            )?;
        }
        for v in &p.vertices {
            writeln!(o, "vt {:.6} {:.6}", v.uv.x, 1.0 - v.uv.y)?;
        }
        for v in &p.vertices {
            writeln!(
                o,
                "vn {:.5} {:.5} {:.5}",
                v.normal.x, v.normal.y, v.normal.z
            )?;
        }
        if let Some(m) = p.material.and_then(|m| model.materials.get(m)) {
            writeln!(o, "usemtl {}", sanitize(&m.name))?;
        }
        for t in p.indices.chunks_exact(3) {
            let (a, b, c) = (
                t[0] as usize + base,
                t[1] as usize + base,
                t[2] as usize + base,
            );
            writeln!(o, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}")?;
        }
        base += p.vertices.len();
    }
    std::fs::write(path, o)?;

    let mut m = String::from("# retrocrunch\n");
    for (i, mat) in model.materials.iter().enumerate() {
        writeln!(m, "newmtl {}", sanitize(&mat.name))?;
        writeln!(
            m,
            "Kd {:.4} {:.4} {:.4}",
            mat.base_color[0], mat.base_color[1], mat.base_color[2]
        )?;
        writeln!(m, "Ka 0 0 0\nKs 0 0 0\nillum 1")?;
        if let Some(t) = texture_file(model, i) {
            writeln!(m, "map_Kd {t}")?;
        }
    }
    std::fs::write(path.with_extension("mtl"), m)?;
    Ok(())
}

struct BinBuilder {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl BinBuilder {
    fn align(&mut self) {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let off = self.data.len();
        self.data.extend_from_slice(bytes);
        let mut v = json!({"buffer": 0, "byteOffset": off, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor_f32(&mut self, values: &[f32], comps: usize, kind: &str, minmax: bool) -> usize {
        let bytes: Vec<u8> = values.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        let mut a = json!({"bufferView": view, "componentType": 5126, "count": values.len() / comps, "type": kind});
        if minmax {
            let mut lo = vec![f32::MAX; comps];
            let mut hi = vec![f32::MIN; comps];
            for c in values.chunks_exact(comps) {
                for k in 0..comps {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    /// Float accessor without a buffer-view target (skins, animation data).
    fn accessor_plain_f32(
        &mut self,
        values: &[f32],
        comps: usize,
        kind: &str,
        minmax: bool,
    ) -> usize {
        let bytes: Vec<u8> = values.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, None);
        let mut a = json!({"bufferView": view, "componentType": 5126, "count": values.len() / comps, "type": kind});
        if minmax {
            let lo = values.iter().copied().fold(f32::MAX, f32::min);
            let hi = values.iter().copied().fold(f32::MIN, f32::max);
            a["min"] = json!([lo]);
            a["max"] = json!([hi]);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn accessor_u16x4(&mut self, values: &[u16]) -> usize {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        self.accessors.push(json!({"bufferView": view, "componentType": 5123, "count": values.len() / 4, "type": "VEC4"}));
        self.accessors.len() - 1
    }

    fn accessor_indices(&mut self, idx: &[u32]) -> usize {
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.accessors.push(json!({"bufferView": view, "componentType": 5125, "count": idx.len(), "type": "SCALAR"}));
        self.accessors.len() - 1
    }
}

pub fn glb_bytes(model: &Model, unlit: bool) -> Result<Vec<u8>> {
    let mut b = BinBuilder {
        data: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
    };
    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut materials = Vec::new();
    for m in &model.materials {
        let mut pbr =
            json!({"baseColorFactor": m.base_color, "metallicFactor": 0.0, "roughnessFactor": 1.0});
        let mut mat = json!({"name": m.name, "doubleSided": m.double_sided});
        if let Some(img) = &m.albedo {
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            let view = b.view(&png, None);
            images.push(json!({"bufferView": view, "mimeType": "image/png"}));
            textures.push(json!({"sampler": 0, "source": images.len() - 1}));
            pbr["baseColorTexture"] = json!({"index": textures.len() - 1});
            if img.pixels().any(|p| p.0[3] < 255) {
                mat["alphaMode"] = json!("MASK");
                mat["alphaCutoff"] = json!(0.5);
            }
        }
        mat["pbrMetallicRoughness"] = pbr;
        if unlit {
            mat["extensions"] = json!({"KHR_materials_unlit": {}});
        }
        materials.push(mat);
    }

    let rig = model.rig.as_ref().filter(|_| model.has_animation());
    let names = unique_names(model.pieces.iter().map(|p| p.name.clone()));
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    // Output node of every piece (None when skipped), for morph-weight channels.
    let mut piece_nodes: Vec<Option<usize>> = vec![None; model.pieces.len()];
    for (pi, (p, name)) in model.pieces.iter().zip(&names).enumerate() {
        if p.indices.is_empty() {
            continue;
        }
        let pos: Vec<f32> = p.vertices.iter().flat_map(|v| v.pos.to_array()).collect();
        let nrm: Vec<f32> = p
            .vertices
            .iter()
            .flat_map(|v| v.normal.normalize_or(glam::Vec3::Y).to_array())
            .collect();
        let uv: Vec<f32> = p.vertices.iter().flat_map(|v| v.uv.to_array()).collect();
        let col: Vec<f32> = p.vertices.iter().flat_map(|v| v.color).collect();
        let mut attrs = json!({
            "POSITION": b.accessor_f32(&pos, 3, "VEC3", true),
            "NORMAL": b.accessor_f32(&nrm, 3, "VEC3", false),
            "TEXCOORD_0": b.accessor_f32(&uv, 2, "VEC2", false),
            "COLOR_0": b.accessor_f32(&col, 4, "VEC4", false),
        });
        let skin = p.skin.filter(|&s| rig.is_some_and(|r| s < r.skins.len()));
        if skin.is_some() {
            let joints: Vec<u16> = p.vertices.iter().flat_map(|v| v.joints).collect();
            let weights: Vec<f32> = p.vertices.iter().flat_map(|v| v.weights).collect();
            attrs["JOINTS_0"] = json!(b.accessor_u16x4(&joints));
            attrs["WEIGHTS_0"] = json!(b.accessor_f32(&weights, 4, "VEC4", false));
        }
        let a_idx = b.accessor_indices(&p.indices);
        let mut prim = json!({"attributes": attrs, "indices": a_idx, "mode": 4});
        if let Some(m) = p.material.filter(|&m| m < materials.len()) {
            prim["material"] = json!(m);
        }
        let mut mesh = json!({"name": name});
        if p.has_morphs() {
            let targets: Vec<Value> = p
                .morphs
                .iter()
                .map(|m| {
                    let dp: Vec<f32> = m.positions.iter().flat_map(|d| d.to_array()).collect();
                    let dn: Vec<f32> = m.normals.iter().flat_map(|d| d.to_array()).collect();
                    json!({
                        "POSITION": b.accessor_f32(&dp, 3, "VEC3", true),
                        "NORMAL": b.accessor_f32(&dn, 3, "VEC3", false),
                    })
                })
                .collect();
            prim["targets"] = json!(targets);
            let mut w = p.morph_weights.clone();
            w.resize(p.morphs.len(), 0.0);
            mesh["weights"] = json!(w);
            // Godot, Blender and three.js read target names from here.
            mesh["extras"] =
                json!({"targetNames": p.morphs.iter().map(|m| m.name.clone()).collect::<Vec<_>>()});
        }
        mesh["primitives"] = json!([prim]);
        meshes.push(mesh);
        let mut node = json!({"name": name, "mesh": meshes.len() - 1});
        if let Some(s) = skin {
            node["skin"] = json!(s);
        }
        piece_nodes[pi] = Some(nodes.len());
        nodes.push(node);
    }

    // Rig nodes follow the mesh nodes; skins and animations point into them.
    let off = nodes.len();
    let mut scene_nodes: Vec<usize> = (0..off).collect();
    let mut skins = Vec::new();
    let mut animations = Vec::new();
    if let Some(r) = rig {
        for n in &r.nodes {
            let mut j = json!({
                "name": n.name,
                "translation": n.translation.to_array(),
                "rotation": n.rotation.normalize().to_array(),
                "scale": n.scale.to_array(),
            });
            if !n.children.is_empty() {
                j["children"] = json!(n.children.iter().map(|c| c + off).collect::<Vec<_>>());
            }
            nodes.push(j);
        }
        scene_nodes.extend(r.roots.iter().map(|i| i + off));
        for s in &r.skins {
            let ibm: Vec<f32> = s
                .inverse_bind
                .iter()
                .flat_map(|m| m.to_cols_array())
                .collect();
            let mut j = json!({
                "name": s.name,
                "joints": s.joints.iter().map(|j| j + off).collect::<Vec<_>>(),
                "inverseBindMatrices": b.accessor_plain_f32(&ibm, 16, "MAT4", false),
            });
            if let Some(sk) = s.skeleton {
                j["skeleton"] = json!(sk + off);
            }
            skins.push(j);
        }
        for clip in &r.clips {
            let mut samplers = Vec::new();
            let mut channels = Vec::new();
            for ch in clip.channels.iter().filter(|c| !c.times.is_empty()) {
                // Transform channels drive rig nodes; weight channels drive every piece
                // node of the morph group (one glTF mesh may have become several pieces).
                let targets: Vec<usize> = match ch.property {
                    Property::Weights(n) => model
                        .pieces
                        .iter()
                        .zip(&piece_nodes)
                        .filter(|(p, _)| p.morph_group == Some(ch.node) && p.morphs.len() == n)
                        .filter_map(|(_, &node)| node)
                        .collect(),
                    _ if ch.node < r.nodes.len() => vec![ch.node + off],
                    _ => Vec::new(),
                };
                if targets.is_empty() {
                    continue;
                }
                let comps = ch.property.components();
                let input = b.accessor_plain_f32(&ch.times, 1, "SCALAR", true);
                let output = match ch.property {
                    Property::Weights(_) => b.accessor_plain_f32(&ch.values, 1, "SCALAR", false),
                    _ => b.accessor_plain_f32(
                        &ch.values,
                        comps,
                        if comps == 4 { "VEC4" } else { "VEC3" },
                        false,
                    ),
                };
                samplers.push(json!({"input": input, "output": output, "interpolation": ch.interpolation.gltf_name()}));
                for node in targets {
                    channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": node, "path": ch.property.gltf_path()}}));
                }
            }
            if !channels.is_empty() {
                animations
                    .push(json!({"name": clip.name, "samplers": samplers, "channels": channels}));
            }
        }
    }
    b.align();

    let mut root = json!({
        "asset": {"version": "2.0", "generator": "retrocrunch"},
        "scene": 0,
        "scenes": [{"name": model.name, "nodes": scene_nodes}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": b.accessors,
        "bufferViews": b.views,
        "buffers": [{"byteLength": b.data.len()}],
    });
    if !skins.is_empty() {
        root["skins"] = json!(skins);
    }
    if !animations.is_empty() {
        root["animations"] = json!(animations);
    }
    if !images.is_empty() {
        root["images"] = json!(images);
        root["textures"] = json!(textures);
        root["samplers"] =
            json!([{"magFilter": 9728, "minFilter": 9728, "wrapS": 10497, "wrapT": 10497}]);
    }
    if unlit {
        root["extensionsUsed"] = json!(["KHR_materials_unlit"]);
    }
    let mut js = serde_json::to_vec(&root)?;
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + b.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    out.extend_from_slice(&(b.data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b.data);
    Ok(out)
}

/// Levels of detail for `model`: each halves the triangles of the previous one.
pub fn lod_models(model: &Model, count: u32) -> Vec<Model> {
    let mut out: Vec<Model> = Vec::new();
    for i in 1..=count.min(4) {
        let prev = out.last().unwrap_or(model);
        let mut m = prev.clone();
        m.name = format!("{}_lod{i}", model.name);
        for p in &mut m.pieces {
            let target = (p.tri_count() / 2).max(4);
            *p = crate::decimate::decimate(p, target, &crate::decimate::DecimateOptions::default());
        }
        out.push(m);
    }
    out
}

/// Distance (world units) where LOD `i` hands over to LOD `i + 1`.
pub fn lod_distance(model: &Model, i: u32) -> f32 {
    let (lo, hi) = model.bounds();
    (hi - lo).length().max(1e-3) * 6.0 * 2f32.powi(i as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Material;
    use crate::mesh::tests::uv_sphere;

    fn model() -> Model {
        let mut p = uv_sphere(12);
        p.material = Some(0);
        Model {
            name: "ball test".into(),
            pieces: vec![p],
            materials: vec![Material {
                albedo: Some(image::RgbaImage::from_pixel(
                    8,
                    8,
                    image::Rgba([200, 10, 10, 255]),
                )),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn sprite_sheet_has_a_cell_per_direction_and_transparent_background() {
        let m = model();
        let job = SpriteSheetJob {
            directions: 8,
            size: 32,
            ..Default::default()
        };
        let (sheet, info) = sprite_sheet(&m, &job).unwrap();
        assert_eq!(sheet.dimensions(), (32, 32 * 8));
        assert_eq!(info.frames, 1);
        assert_eq!(info.yaw_degrees.len(), 8);
        // Corners are transparent, centres hold the model, in every cell.
        for d in 0..8 {
            assert_eq!(sheet.get_pixel(0, d * 32).0[3], 0);
            assert_eq!(sheet.get_pixel(16, d * 32 + 16).0[3], 255);
        }
        assert!(
            sprite_sheet(
                &m,
                &SpriteSheetJob {
                    clip: Some("walk".into()),
                    ..job
                }
            )
            .is_err()
        );
    }

    #[test]
    fn lods_halve_triangles_and_godot_switches_by_distance() {
        let mut m = model();
        m.pieces[0] = crate::mesh::tests::uv_sphere(24);
        m.pieces[0].material = Some(0);
        let dir = std::env::temp_dir().join("retrocrunch_lod_test");
        let _ = std::fs::remove_dir_all(&dir);
        let s = ExportSettings {
            lods: 2,
            ..Default::default()
        };
        export_model(&m, &dir, &s).unwrap();
        let tris = |f: &str| Model::load(&dir.join(f)).unwrap().tri_count();
        let (t0, t1, t2) = (
            tris("ball_test.glb"),
            tris("ball_test_lod1.glb"),
            tris("ball_test_lod2.glb"),
        );
        assert!(t1 * 3 / 2 < t0 && t2 * 3 / 2 < t1, "{t0} > {t1} > {t2}");
        let scene = std::fs::read_to_string(dir.join("ball_test.tscn")).unwrap();
        assert!(scene.contains("path=\"ball_test_lod2.glb\""));
        assert!(
            scene.contains("[node name=\"LOD1\" parent=\".\" instance=ExtResource(\"lod_1\")]")
        );
        let d0 = lod_distance(&m, 0);
        assert!(scene.contains(&format!("visibility_range_end = {d0}")));
        assert!(scene.contains(&format!("visibility_range_begin = {d0}")));
    }

    #[test]
    fn glb_roundtrips_through_gltf_crate() {
        let m = model();
        let bytes = glb_bytes(&m, true).unwrap();
        let (doc, buffers, images) = gltf::import_slice(&bytes).unwrap();
        assert_eq!(doc.meshes().count(), 1);
        assert_eq!(images.len(), 1);
        let prim = doc.meshes().next().unwrap().primitives().next().unwrap();
        let r = prim.reader(|b| Some(&buffers[b.index()]));
        assert_eq!(
            r.read_indices().unwrap().into_u32().count(),
            m.pieces[0].indices.len()
        );
        let s = doc.samplers().next().unwrap();
        assert_eq!(s.mag_filter(), Some(gltf::texture::MagFilter::Nearest));
    }

    fn rigged_model() -> Model {
        let mut p = uv_sphere(16);
        p.material = Some(0);
        p.skin = Some(0);
        for v in &mut p.vertices {
            v.pos += glam::Vec3::Y; // sphere centred on the tip joint's bind position
            let t = ((v.pos.y - 0.8) / 0.4).clamp(0.0, 1.0);
            let (j, w) = crate::rig::normalize_influences([0, 1, 0, 0], [1.0 - t, t, 0.0, 0.0]);
            v.joints = j;
            v.weights = w;
        }
        Model {
            name: "rigged".into(),
            pieces: vec![p],
            materials: vec![Material::default()],
            rig: Some(crate::rig::tests::two_bone_rig()),
            ..Default::default()
        }
    }

    #[test]
    fn rigged_glb_roundtrips_skeleton_skin_and_clips() {
        let m = rigged_model();
        let bytes = glb_bytes(&m, true).unwrap();
        gltf::Gltf::from_slice(&bytes).expect("valid glTF");
        let dir = std::env::temp_dir().join("retrocrunch_rig_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rigged.glb");
        std::fs::write(&path, &bytes).unwrap();
        let back = Model::load(&path).unwrap();
        assert!(back.is_rigged());
        let r = back.rig.as_ref().unwrap();
        let names: Vec<&str> = r.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            ["root", "tip"],
            "mesh node must not leak into the rig"
        );
        assert_eq!(r.nodes[1].parent, Some(0));
        assert_eq!(r.skins[0].joints, vec![0, 1]);
        assert!(
            r.skins[0].inverse_bind[1]
                .abs_diff_eq(glam::Mat4::from_translation(-glam::Vec3::Y), 1e-6)
        );
        assert_eq!(r.clips.len(), 1);
        assert_eq!(r.clips[0].name, "bend");
        let ch = &r.clips[0].channels[0];
        assert_eq!(
            (ch.node, ch.property, ch.interpolation),
            (
                1,
                crate::rig::Property::Rotation,
                crate::rig::Interpolation::Linear
            )
        );
        let (a, b) = (&m.pieces[0], &back.pieces[0]);
        assert_eq!(b.skin, Some(0));
        assert_eq!(a.vertices.len(), b.vertices.len());
        for (va, vb) in a.vertices.iter().zip(&b.vertices) {
            assert!(
                (va.pos - vb.pos).length() < 1e-5,
                "skinned positions stay in bind space"
            );
            assert_eq!(va.joints[0], vb.joints[0]);
            assert!((va.weights[0] - vb.weights[0]).abs() < 1e-5);
        }
    }

    #[test]
    fn morph_targets_and_weight_clips_roundtrip() {
        use crate::mesh::Morph;
        use crate::rig::{Channel, Clip, Interpolation, Property, Rig};
        let mut p = uv_sphere(8);
        p.material = Some(0);
        let n = p.vertices.len();
        let up: Vec<glam::Vec3> = p
            .vertices
            .iter()
            .map(|v| glam::Vec3::Y * v.pos.y.max(0.0))
            .collect();
        p.morphs = vec![
            Morph {
                name: "stretch".into(),
                positions: up,
                normals: vec![glam::Vec3::ZERO; n],
            },
            Morph {
                name: "none".into(),
                positions: vec![glam::Vec3::ZERO; n],
                normals: vec![glam::Vec3::ZERO; n],
            },
        ];
        p.morph_group = Some(0);
        p.morph_weights = vec![0.25, 0.0];
        let clip = Clip {
            name: "pulse".into(),
            channels: vec![Channel {
                node: 0,
                property: Property::Weights(2),
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 1.0, 0.5],
            }],
        };
        let m = Model {
            name: "morph".into(),
            pieces: vec![p],
            materials: vec![Material::default()],
            rig: Some(Rig {
                clips: vec![clip],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(m.has_animation() && !m.is_rigged());
        let bytes = glb_bytes(&m, true).unwrap();
        let dir = std::env::temp_dir().join("retrocrunch_morph_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("morph.glb");
        std::fs::write(&path, &bytes).unwrap();
        let back = Model::load(&path).unwrap();
        let bp = &back.pieces[0];
        assert_eq!(
            bp.morphs
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["stretch", "none"]
        );
        assert_eq!(bp.morph_weights, vec![0.25, 0.0]);
        for (a, b) in m.pieces[0].morphs[0]
            .positions
            .iter()
            .zip(&bp.morphs[0].positions)
        {
            assert!((*a - *b).length() < 1e-5);
        }
        let r = back.rig.as_ref().expect("weight clip keeps a rig");
        let group = bp.morph_group.expect("morph group");
        assert_eq!(
            r.morph_weights(Some(0), 1.0, group).unwrap(),
            vec![1.0, 0.5]
        );
    }

    #[test]
    fn scaled_rig_export_scales_translations_and_bind_matrices() {
        let m = rigged_model();
        let dir = std::env::temp_dir().join("retrocrunch_rig_scale_test");
        let _ = std::fs::remove_dir_all(&dir);
        let s = ExportSettings {
            scale: 100.0,
            preview: false,
            ..Default::default()
        };
        let files = export_model(&m, &dir, &s).unwrap();
        assert!(files.iter().any(|f| f.ends_with(GODOT_APPLY_SCRIPT_NAME)));
        let scene = std::fs::read_to_string(dir.join("rigged.tscn")).unwrap();
        assert!(scene.contains("psx_materials = Array[Material]([SubResource(\"mat_0\")])"));
        assert!(
            !scene.contains("material_override"),
            "rigged scenes assign by material name"
        );
        let back = Model::load(&dir.join("rigged.glb")).unwrap();
        let r = back.rig.unwrap();
        assert!((r.nodes[1].translation - glam::Vec3::new(0.0, 100.0, 0.0)).length() < 1e-3);
        assert!((r.skins[0].inverse_bind[1].w_axis.y + 100.0).abs() < 1e-3);
        // Animated pose stays consistent after scaling.
        let mats = r.skin_matrices(&r.world(&r.pose(Some(0), 1.0)));
        let p = crate::rig::skin_point(
            glam::Vec3::new(0.0, 200.0, 0.0),
            [1, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
            &mats[0],
        );
        assert!(
            (p - glam::Vec3::new(-100.0, 100.0, 0.0)).length() < 1e-2,
            "{p}"
        );
    }

    #[test]
    fn godot_variant_follows_alpha_and_sides() {
        let mut m = Material::default();
        assert_eq!(
            GodotVariant::for_material(&m).file_name(),
            "psx_opaque.gdshader"
        );
        let mut img = image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255]));
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        m.albedo = Some(img.clone());
        assert_eq!(AlphaKind::of(&m), AlphaKind::Cutout);
        img.put_pixel(1, 0, image::Rgba([9, 9, 9, 128]));
        m.albedo = Some(img);
        m.double_sided = true;
        let v = GodotVariant::for_material(&m);
        assert_eq!(v.file_name(), "psx_blend_double.gdshader");
        let src = v.source();
        assert!(src.contains("cull_disabled") && src.contains("blend_mix"));
        assert!(
            src.contains("#define PSX_BLEND")
                && src.contains("#include \"psx_common.gdshaderinc\"")
        );
    }

    #[test]
    fn export_writes_godot_bundle_and_obj_reimports() {
        let m = model();
        let dir = std::env::temp_dir().join("retrocrunch_export_test");
        let _ = std::fs::remove_dir_all(&dir);
        let s = ExportSettings {
            obj: true,
            ..Default::default()
        };
        let files = export_model(&m, &dir, &s).unwrap();
        assert!(
            files
                .iter()
                .any(|f| f.extension().is_some_and(|e| e == "tscn"))
        );
        let scene = std::fs::read_to_string(dir.join("ball_test.tscn")).unwrap();
        assert!(scene.contains("material_override"));
        assert!(scene.contains("path=\"psx_opaque.gdshader\""));
        assert!(dir.join("psx_common.gdshaderinc").is_file());
        assert!(dir.join("psx_screen.gd").is_file());
        assert!(dir.join("psx_speed_limit.gd").is_file());
        assert!(dir.join("psx_draw_distance.gd").is_file());
        let import = std::fs::read_to_string(dir.join("ball_test.png.import")).unwrap();
        assert!(import.contains("compress/mode=0") && import.contains("mipmaps/generate=false"));
        let glb_import = std::fs::read_to_string(dir.join("ball_test.glb.import")).unwrap();
        assert!(glb_import.contains("\nmeshes/generate_lods=false\n"));
        assert!(
            glb_import.lines().all(|l| !l.starts_with(' ')),
            "no indented lines"
        );
        let back = Model::load(&dir.join("ball_test.obj")).unwrap();
        assert_eq!(back.tri_count(), m.tri_count());
        assert!(back.materials[0].albedo.is_some());
    }
}
