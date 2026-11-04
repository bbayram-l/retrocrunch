//! Godot 4 output: the scene that instances the GLB with PSX materials (and LOD
//! bands), shader variants per material, texture import settings, and sprite scenes.

use super::{SpriteSheetInfo, lod_distance, sanitize, texture_file, unique_names};
use crate::mesh::Model;
use std::fmt::Write as _;

/// Godot 4 scene instancing the GLB with the PSX shader on every mesh.
pub fn godot_scene(model: &Model) -> String {
    godot_scene_lods(model, 0)
}

/// Godot scene; with `lods` > 0 (static models), LOD0..LODn instances switch by distance.
pub fn godot_scene_lods(model: &Model, lods: u32) -> String {
    let base = sanitize(&model.name);
    let names = unique_names(model.pieces.iter().map(|p| p.name.clone()));
    let tex_files: Vec<Option<String>> = (0..model.materials.len())
        .map(|i| texture_file(model, i))
        .collect();
    let n_tex = tex_files.iter().flatten().count();
    let variants = godot_variants(model);
    let mut s = format!(
        "[gd_scene load_steps={} format=3]\n\n",
        2 + variants.len() + n_tex + model.materials.len().max(1) + model.is_rigged() as usize
    );
    writeln!(
        s,
        "[ext_resource type=\"PackedScene\" path=\"{base}.glb\" id=\"1_glb\"]"
    )
    .ok();
    // LOD scenes only for static models (skinned meshes move under the skeleton).
    let lods = if model.is_rigged() { 0 } else { lods };
    for level in 1..=lods {
        writeln!(
            s,
            "[ext_resource type=\"PackedScene\" path=\"{base}_lod{level}.glb\" id=\"lod_{level}\"]"
        )
        .ok();
    }
    for v in &variants {
        writeln!(
            s,
            "[ext_resource type=\"Shader\" path=\"{}\" id=\"{}\"]",
            v.file_name(),
            v.resource_id()
        )
        .ok();
    }
    if model.is_rigged() {
        writeln!(
            s,
            "[ext_resource type=\"Script\" path=\"{GODOT_APPLY_SCRIPT_NAME}\" id=\"3_apply\"]"
        )
        .ok();
    }
    for (i, t) in tex_files.iter().enumerate() {
        if let Some(t) = t {
            writeln!(
                s,
                "[ext_resource type=\"Texture2D\" path=\"{t}\" id=\"tex_{i}\"]"
            )
            .ok();
        }
    }
    s.push('\n');
    let mut mats = model.materials.len();
    if mats == 0 {
        mats = 1;
    }
    for i in 0..mats {
        let mat_name = model
            .materials
            .get(i)
            .map_or("material".to_string(), |m| m.name.replace('"', "'"));
        let shader = model
            .materials
            .get(i)
            .map_or(GodotVariant::default(), GodotVariant::for_material)
            .resource_id();
        writeln!(s, "[sub_resource type=\"ShaderMaterial\" id=\"mat_{i}\"]\nresource_name = \"{mat_name}\"\nrender_priority = 0\nshader = ExtResource(\"{shader}\")").ok();
        match tex_files.get(i).cloned().flatten() {
            Some(_) => writeln!(s, "shader_parameter/albedo_texture = ExtResource(\"tex_{i}\")\nshader_parameter/use_texture = true").ok(),
            None => writeln!(s, "shader_parameter/use_texture = false").ok(),
        };
        // Colour is baked into the texture / vertex colours; translucency isn't.
        let alpha = model.materials.get(i).map_or(1.0, |m| m.base_color[3]);
        if alpha < 0.999 {
            writeln!(s, "shader_parameter/tint = Color(1, 1, 1, {alpha})").ok();
        }
        s.push('\n');
    }
    if model.is_rigged() {
        // Skinned meshes get re-parented under Skeleton3D on import: assign by material name.
        let list: Vec<String> = (0..mats)
            .map(|i| format!("SubResource(\"mat_{i}\")"))
            .collect();
        writeln!(
            s,
            "[node name=\"{base}\" instance=ExtResource(\"1_glb\")]\nscript = ExtResource(\"3_apply\")\npsx_materials = Array[Material]([{}])\n",
            list.join(", ")
        )
        .ok();
        return s;
    }
    if lods == 0 {
        writeln!(
            s,
            "[node name=\"{base}\" instance=ExtResource(\"1_glb\")]\n"
        )
        .ok();
        for (p, name) in model.pieces.iter().zip(&names) {
            let mi = p.material.unwrap_or(0).min(mats - 1);
            writeln!(
                s,
                "[node name=\"{name}\" parent=\".\"]\nmaterial_override = SubResource(\"mat_{mi}\")\n"
            )
            .ok();
        }
        return s;
    }
    // LODs: every level is an instance; its meshes show only within their distance band.
    writeln!(s, "[node name=\"{base}\" type=\"Node3D\"]\n").ok();
    for level in 0..=lods {
        let resource = if level == 0 {
            "1_glb".to_string()
        } else {
            format!("lod_{level}")
        };
        writeln!(
            s,
            "[node name=\"LOD{level}\" parent=\".\" instance=ExtResource(\"{resource}\")]\n"
        )
        .ok();
        let begin = if level == 0 {
            0.0
        } else {
            lod_distance(model, level - 1)
        };
        let end = if level == lods {
            0.0
        } else {
            lod_distance(model, level)
        };
        for (p, name) in model.pieces.iter().zip(&names) {
            let mi = p.material.unwrap_or(0).min(mats - 1);
            writeln!(
                s,
                "[node name=\"{name}\" parent=\"LOD{level}\"]\nmaterial_override = SubResource(\"mat_{mi}\")\nvisibility_range_begin = {begin}\nvisibility_range_end = {end}\n"
            )
            .ok();
        }
    }
    s
}

pub const GODOT_BILLBOARD_SCRIPT_NAME: &str = "psx_billboard.gd";
pub const GODOT_BILLBOARD_SCRIPT: &str = include_str!("../../godot/psx_billboard.gd");

/// Godot 4 scene for a sprite sheet: an AnimatedSprite3D (Y billboard, nearest
/// filtering, alpha cut) with one animation per direction (`dir0`..), driven by
/// `psx_billboard.gd`, which picks the row facing the camera.
pub fn godot_sprite_scene(name: &str, png: &str, info: &SpriteSheetInfo) -> String {
    let mut s = String::new();
    let atlases = (info.directions * info.frames) as usize;
    writeln!(
        s,
        "[gd_scene load_steps={} format=3]
",
        atlases + 4
    )
    .ok();
    writeln!(
        s,
        "[ext_resource type=\"Texture2D\" path=\"{png}\" id=\"1_sheet\"]"
    )
    .ok();
    writeln!(
        s,
        "[ext_resource type=\"Script\" path=\"{GODOT_BILLBOARD_SCRIPT_NAME}\" id=\"2_billboard\"]
"
    )
    .ok();
    for d in 0..info.directions {
        for f in 0..info.frames {
            writeln!(
                s,
                "[sub_resource type=\"AtlasTexture\" id=\"cell_{d}_{f}\"]
atlas = ExtResource(\"1_sheet\")
region = Rect2({}, {}, {}, {})
",
                f * info.cell,
                d * info.cell,
                info.cell,
                info.cell
            )
            .ok();
        }
    }
    let anims: Vec<String> = (0..info.directions)
        .map(|d| {
            let frames: Vec<String> = (0..info.frames)
                .map(|f| {
                    format!(
                        "{{
\"duration\": 1.0,
\"texture\": SubResource(\"cell_{d}_{f}\")
}}"
                    )
                })
                .collect();
            format!(
                "{{
\"frames\": [{}],
\"loop\": true,
\"name\": &\"dir{d}\",
\"speed\": {}
}}",
                frames.join(", "),
                info.fps
            )
        })
        .collect();
    writeln!(
        s,
        "[sub_resource type=\"SpriteFrames\" id=\"frames\"]
animations = [{}]
",
        anims.join(", ")
    )
    .ok();
    let c = info.center;
    writeln!(
        s,
        "[node name=\"{}\" type=\"Node3D\"]
",
        sanitize(name)
    )
    .ok();
    writeln!(
        s,
        "[node name=\"Sprite\" type=\"AnimatedSprite3D\" parent=\".\"]
transform = Transform3D(1, 0, 0, 0, 1, 0, 0, 0, 1, {}, {}, {})
pixel_size = {}
billboard = 2
alpha_cut = 1
texture_filter = 0
sprite_frames = SubResource(\"frames\")
animation = &\"dir0\"
autoplay = \"dir0\"
script = ExtResource(\"2_billboard\")
directions = {}",
        c[0], c[1], c[2], info.units_per_pixel, info.directions
    )
    .ok();
    s
}

pub const GODOT_APPLY_SCRIPT_NAME: &str = "psx_apply_materials.gd";
pub const GODOT_APPLY_SCRIPT: &str = include_str!("../../godot/psx_apply_materials.gd");

pub const GODOT_COMMON_SHADER: &str = include_str!("../../godot/psx_common.gdshaderinc");
pub const GODOT_SCREEN_SCRIPT: &str = include_str!("../../godot/psx_screen.gd");
pub const GODOT_POST_SHADER: &str = include_str!("../../godot/psx_post.gdshader");

/// How a material's alpha is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlphaKind {
    #[default]
    Opaque,
    /// Texels are fully opaque or fully transparent: alpha test, stays in the opaque pass.
    Cutout,
    /// Partial alpha (texture or base colour): alpha blending.
    Blend,
}

impl AlphaKind {
    pub fn of(m: &crate::mesh::Material) -> Self {
        if m.base_color[3] < 0.999 {
            return Self::Blend;
        }
        let Some(img) = &m.albedo else {
            return Self::Opaque;
        };
        let (mut clear, mut partial) = (false, false);
        for p in img.pixels() {
            match p.0[3] {
                0 => clear = true,
                255 => {}
                _ => partial = true,
            }
        }
        if partial {
            Self::Blend
        } else if clear {
            Self::Cutout
        } else {
            Self::Opaque
        }
    }
}

/// One `psx_*.gdshader` variant: alpha handling x culling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct GodotVariant {
    pub alpha: AlphaKind,
    pub double_sided: bool,
}

impl GodotVariant {
    pub fn for_material(m: &crate::mesh::Material) -> Self {
        Self {
            alpha: AlphaKind::of(m),
            double_sided: m.double_sided,
        }
    }

    fn stem(self) -> String {
        let a = match self.alpha {
            AlphaKind::Opaque => "opaque",
            AlphaKind::Cutout => "cutout",
            AlphaKind::Blend => "blend",
        };
        format!("psx_{a}{}", if self.double_sided { "_double" } else { "" })
    }

    pub fn file_name(self) -> String {
        format!("{}.gdshader", self.stem())
    }

    fn resource_id(self) -> String {
        format!("sh_{}", self.stem())
    }

    pub fn source(self) -> String {
        let (what, define, blend) = match self.alpha {
            AlphaKind::Opaque => ("opaque", "", ""),
            AlphaKind::Cutout => ("alpha-tested cutout", "#define PSX_CUTOUT\n", ""),
            AlphaKind::Blend => ("alpha-blended", "#define PSX_BLEND\n", ", blend_mix"),
        };
        let (sides, cull) = if self.double_sided {
            ("both faces drawn", "cull_disabled")
        } else {
            ("back faces culled", "cull_back")
        };
        format!(
            "// retrocrunch PSX material: {what}, {sides}. The code lives in psx_common.gdshaderinc.\n\
             shader_type spatial;\n\
             render_mode unshaded, {cull}, depth_draw_opaque{blend};\n\
             {define}#include \"psx_common.gdshaderinc\"\n"
        )
    }
}

/// The shader variants a model's materials need (at least one).
pub fn godot_variants(model: &Model) -> Vec<GodotVariant> {
    let mut v: Vec<GodotVariant> = model
        .materials
        .iter()
        .map(GodotVariant::for_material)
        .collect();
    if v.is_empty() {
        v.push(GodotVariant::default());
    }
    v.sort();
    v.dedup();
    v
}

pub(super) fn is_hd(img: &image::RgbaImage) -> bool {
    img.width().max(img.height()) > 512
}

/// HD textures (over 512 px) sample with nearest *mipmapped* filtering: texels stay sharp
/// up close without shimmering at a distance. PS1-sized textures keep plain nearest.
pub(super) fn godot_common_for(model: &Model) -> String {
    let hd = model
        .materials
        .iter()
        .filter_map(|m| m.albedo.as_ref())
        .any(is_hd);
    if hd {
        GODOT_COMMON_SHADER.replacen("filter_nearest,", "filter_nearest_mipmap,", 1)
    } else {
        GODOT_COMMON_SHADER.to_string()
    }
}

/// Godot 4 texture import settings: lossless, no VRAM compression (Godot's "Detect 3D"
/// would otherwise switch to it and smear the palette), exact transparent texels.
/// Mipmaps only for HD textures, whose shader samples them.
pub fn godot_texture_import(mipmaps: bool) -> String {
    format!(
        "[remap]\n\n\
         importer=\"texture\"\n\
         type=\"CompressedTexture2D\"\n\n\
         [params]\n\n\
         compress/mode=0\n\
         compress/high_quality=false\n\
         compress/lossy_quality=0.7\n\
         compress/hdr_compression=1\n\
         compress/normal_map=0\n\
         compress/channel_pack=0\n\
         mipmaps/generate={mipmaps}\n\
         mipmaps/limit=-1\n\
         roughness/mode=0\n\
         roughness/src_normal=\"\"\n\
         process/fix_alpha_border=false\n\
         process/premult_alpha=false\n\
         process/normal_map_invert_y=false\n\
         process/hdr_as_srgb=false\n\
         process/hdr_clamp_exposure=false\n\
         process/size_limit=0\n\
         detect_3d/compress_to=0\n"
    )
}
