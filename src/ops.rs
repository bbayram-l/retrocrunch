//! High-level operations shared by the CLI and the MCP server. Every operation takes a
//! serde-deserialisable options struct (so agents can pass JSON) and returns a JSON
//! report: written files, stats and warnings.

use crate::blocks::{Block, BlockWorld, Shape};
use crate::export::{self, EnginePreset, ExportSettings};
use crate::mesh::Model;
use crate::palette::{self, Rgb};
use crate::process::{self, ConvertSettings, PieceSettings, TextureSizing};
use crate::scale::{self, Scaler};
use crate::sprite::{self, SpriteSettings};
use crate::texture::{self, BakeMaps, CrunchSettings, DitherMode, PaletteMode};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out")
        .to_string()
}

fn paths(v: &[PathBuf]) -> Vec<String> {
    v.iter().map(|p| p.display().to_string()).collect()
}

/// Texture options shared by models and standalone textures.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureOptions {
    /// Longest texture side in pixels (4-2048).
    pub tex_size: u32,
    /// Palette size (2-256).
    pub colors: usize,
    /// Palette file (.gpl .pal .act .hex .txt or swatch image).
    pub palette: Option<PathBuf>,
    /// "best" (best N entries of the palette) or "all" (every entry, shared CLUT).
    pub palette_mode: String,
    /// "bayer", "ps1" or "off".
    pub dither: String,
    pub dither_amount: f32,
    pub brightness: f32,
    pub rgb555: bool,
}

impl Default for TextureOptions {
    fn default() -> Self {
        Self {
            tex_size: 128,
            colors: 16,
            palette: None,
            palette_mode: "best".into(),
            dither: "bayer".into(),
            dither_amount: 12.0,
            brightness: 0.0,
            rgb555: true,
        }
    }
}

impl TextureOptions {
    pub fn crunch(&self) -> Result<CrunchSettings> {
        Ok(CrunchSettings {
            max_size: self.tex_size.clamp(4, 2048),
            colors: self.colors.clamp(2, 256),
            dither: match self.dither.to_ascii_lowercase().as_str() {
                "off" | "none" => DitherMode::None,
                "bayer" | "bayer4" => DitherMode::Bayer4,
                "bayer8" => DitherMode::Bayer8,
                "floyd" | "floyd-steinberg" | "fs" | "diffusion" => DitherMode::FloydSteinberg,
                "ps1" | "psx" => DitherMode::Ps1Table,
                other => bail!("unknown dither '{other}' (bayer, ps1, off)"),
            },
            dither_amount: self.dither_amount,
            brightness: self.brightness,
            rgb555: self.rgb555,
            palette_mode: match (
                &self.palette,
                self.palette_mode.to_ascii_lowercase().as_str(),
            ) {
                (None, _) => PaletteMode::Auto,
                (Some(_), "all") => PaletteMode::ImportedAll,
                (Some(_), "best") => PaletteMode::ImportedBestN,
                (Some(_), other) => bail!("unknown palette_mode '{other}' (best, all)"),
            },
            ..Default::default()
        })
    }

    pub fn load_palette(&self) -> Result<Option<Vec<Rgb>>> {
        self.palette
            .as_deref()
            .map(palette::load_palette)
            .transpose()
    }
}

pub fn parse_engine(s: &str) -> Result<EnginePreset> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "godot" => EnginePreset::Godot,
        "unity" => EnginePreset::Unity,
        "unreal" => EnginePreset::Unreal,
        "blender" => EnginePreset::Blender,
        "generic" => EnginePreset::Generic,
        other => bail!("unknown engine '{other}' (godot, unity, unreal, blender, generic)"),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelOptions {
    pub input: PathBuf,
    /// A sub-folder named after the model is created inside.
    pub output_dir: PathBuf,
    pub tris: usize,
    /// Target as a percentage of the source instead of `tris`.
    pub percent: Option<f32>,
    #[serde(flatten)]
    pub texture: TextureOptions,
    /// Physical texel size in cm (enables Pixel size mode).
    pub texel_cm: Option<f32>,
    pub units_per_meter: f32,
    pub engine: String,
    pub remove_hidden: bool,
    pub grounded: bool,
    pub ao: bool,
    pub snap: f32,
    /// Split polygons with edges longer than this (world units). 0 = off.
    pub subdivide: f32,
    /// Override the engine preset's export scale.
    pub scale: Option<f32>,
    /// Force FBX on/off (FBX needs Blender).
    pub fbx: Option<bool>,
    pub preview: bool,
    /// "atlas" (fresh UV atlas + rebake into one texture) or "keep" (original UVs).
    pub uv: String,
    /// Names of pieces to bake into the texture instead of keeping as geometry.
    pub bake_pieces: Vec<String>,
    /// Rebake search distance, % of the model size (default 3).
    pub bake_distance: Option<f32>,
    /// Atlas chart crease angle in degrees (default 60).
    pub chart_angle: Option<f32>,
    /// Preset name (built-in or user) or .json file used as the base settings.
    pub preset: Option<String>,
    /// HD texture: source texel density and full colour on the low-poly model.
    pub hd_texture: bool,
    /// Remesh static models in Blender (voxel + QuadriFlow) before decimating.
    pub remesh: bool,
    /// Rigged models: "off", "one-bone" (one bone per vertex) or "segments".
    pub rigid: String,
    /// Extra half-resolution levels of detail (0-4).
    pub lods: u32,
    /// Near-camera tessellation levels (0-3): versions with long edges split that
    /// Godot scenes of static models show up close.
    pub tessellate: u32,
    /// Keep more triangles/texels on the automatically found head (default on).
    pub focus_face: bool,
    /// Focus strength (default 4; 1 = off).
    pub focus_strength: Option<f32>,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            output_dir: PathBuf::from("psx_out"),
            tris: 600,
            percent: None,
            texture: TextureOptions::default(),
            texel_cm: None,
            units_per_meter: 1.0,
            engine: "godot".into(),
            remove_hidden: true,
            grounded: false,
            ao: true,
            snap: 0.0,
            subdivide: 0.0,
            scale: None,
            fbx: None,
            preview: true,
            uv: "atlas".into(),
            bake_pieces: Vec::new(),
            bake_distance: None,
            chart_angle: None,
            preset: None,
            hd_texture: false,
            remesh: false,
            rigid: "off".into(),
            lods: 0,
            tessellate: 0,
            focus_face: true,
            focus_strength: None,
        }
    }
}

/// Machine-readable summary of a loaded model.
pub fn describe(model: &Model) -> Value {
    let (lo, hi) = model.bounds();
    let rig = model.rig.as_ref().filter(|_| model.has_animation()).map(|r| {
        json!({
            "joints": r.joint_count(),
            "skins": r.skins.iter().map(|s| json!({"name": s.name, "joints": s.joints.len()})).collect::<Vec<_>>(),
            "clips": r.clips.iter().map(|c| json!({"name": c.name, "duration": c.duration(), "channels": c.channels.len()})).collect::<Vec<_>>(),
        })
    });
    json!({
        "name": model.name,
        "triangles": model.tri_count(),
        "quads": model.quad_count(),
        "polygons": model.polygon_count(),
        "vertices": model.vertex_count(),
        "bounds": {"min": lo.to_array(), "max": hi.to_array(), "size": (hi - lo).to_array()},
        "pieces": model.pieces.iter().map(|p| json!({
            "name": p.name,
            "triangles": p.tri_count(),
            "material": p.material,
            "skinned": p.skin.is_some(),
            "morph_targets": p.morphs.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "materials": model.materials.iter().map(|m| json!({
            "name": m.name,
            "texture": m.albedo.as_ref().map(|i| [i.width(), i.height()]),
            "base_color": m.base_color,
        })).collect::<Vec<_>>(),
        "rig": rig,
        "recommended_min_tris": if model.is_rigged() { crate::decimate::recommended_min_tris(&model.pieces) } else { 0 },
    })
}

pub fn inspect_model(path: &Path) -> Result<Value> {
    Ok(describe(&Model::load(path)?))
}

pub fn convert_model(o: &ModelOptions, progress: process::Progress) -> Result<Value> {
    let model = Model::load(&o.input)?;
    let pal = o.texture.load_palette()?;
    let preset = o.preset.as_deref().map(crate::presets::find).transpose()?;
    let s = model_settings(o, preset.as_ref().map(|p| p.convert.clone()))?;
    let mut pieces = PieceSettings::defaults_for(&model);
    for name in &o.bake_pieces {
        let Some(i) = model.pieces.iter().position(|p| &p.name == name) else {
            let names: Vec<&str> = model.pieces.iter().map(|p| p.name.as_str()).collect();
            bail!("no piece named '{name}' (pieces: {})", names.join(", "));
        };
        pieces[i].baked = true;
    }
    let c = process::convert(&model, &pieces, &s, pal.as_deref(), progress);
    let d = ModelOptions::default();
    // A preset's export settings apply unless an engine was asked for explicitly.
    let mut es = match preset.as_ref().and_then(|p| p.export.clone()) {
        Some(e) if o.engine == d.engine => e,
        _ => parse_engine(&o.engine)?.settings(),
    };
    if let Some(sc) = o.scale {
        es.scale = sc;
    }
    if let Some(f) = o.fbx {
        es.fbx = f;
    }
    es.preview = o.preview;
    if o.lods > 0 {
        es.lods = o.lods.min(4);
    }
    if o.tessellate > 0 {
        es.tessellate = o.tessellate.min(export::MAX_TESSELLATE);
    }
    let dir = o.output_dir.join(export::sanitize(&model.name));
    let files = export::export_model(&c.model, &dir, &es)?;
    Ok(json!({
        "output_dir": dir.display().to_string(),
        "files": paths(&files),
        "source": describe(&model),
        "result": describe(&c.model),
        "stats": {
            "triangles_before": c.stats.tris_before,
            "triangles_after": c.stats.tris_after,
            "vertices_after": c.stats.verts_after,
            "hidden_removed": c.stats.hidden_removed,
            "subdivided_added": c.stats.subdivided_added,
            "atlas": c.stats.atlas.map(|(n, charts, fill)| json!({
                "size": n,
                "charts": charts,
                "fill_percent": (f64::from(fill) * 1000.0).round() / 10.0,
            })),
            "focus": c.stats.focus,
            "vram": c.stats.vram,
            "recommended_min_tris": c.stats.recommended_min_tris,
            "textures": c.stats.textures.iter().map(|t| json!({"material": t.0, "width": t.1, "height": t.2, "colors": t.3})).collect::<Vec<_>>(),
        },
        "palettes": c.palettes.iter().map(|p| p.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "warnings": c.stats.warnings,
        "preset": preset.map(|p| p.name),
    }))
}

/// Conversion settings for `o`. Without a preset every option applies; with one, the
/// preset is the base and only options that differ from their defaults override it
/// (so `{"preset": "Hero character", "tris": 900}` changes just the budget).
pub fn model_settings(
    o: &ModelOptions,
    preset: Option<ConvertSettings>,
) -> Result<ConvertSettings> {
    let d = ModelOptions::default();
    let all = preset.is_none();
    let mut s = preset.unwrap_or_default();
    let set = |differs: bool| all || differs;
    if set(o.tris != d.tris) {
        s.target_tris = o.tris.max(1);
        s.use_percent = false;
    }
    if let Some(p) = o.percent {
        s.use_percent = true;
        s.percent = p;
    }
    let (t, dt) = (&o.texture, &d.texture);
    let crunch = t.crunch()?;
    if set(t.tex_size != dt.tex_size) {
        s.texture.max_size = crunch.max_size;
    }
    if set(t.colors != dt.colors) {
        s.texture.colors = crunch.colors;
    }
    if set(t.palette != dt.palette || t.palette_mode != dt.palette_mode) {
        s.texture.palette_mode = crunch.palette_mode;
    }
    if set(t.dither != dt.dither) {
        s.texture.dither = crunch.dither;
    }
    if set(t.dither_amount != dt.dither_amount) {
        s.texture.dither_amount = crunch.dither_amount;
    }
    if set(t.brightness != dt.brightness) {
        s.texture.brightness = crunch.brightness;
    }
    if set(t.rgb555 != dt.rgb555) {
        s.texture.rgb555 = crunch.rgb555;
    }
    if let Some(cm) = o.texel_cm {
        s.sizing = TextureSizing::PixelSize;
        s.texel_cm = cm;
    } else if all {
        s.sizing = TextureSizing::Fixed;
    }
    if set(o.units_per_meter != d.units_per_meter) {
        s.units_per_meter = o.units_per_meter;
    }
    if set(o.remove_hidden != d.remove_hidden) {
        s.remove_hidden = o.remove_hidden;
    }
    if set(o.grounded != d.grounded) {
        s.hidden_ignore_below = o.grounded;
    }
    if set(o.ao != d.ao) {
        s.vertex_ao = o.ao;
    }
    if set(o.snap != d.snap) {
        s.geometry_snap = o.snap;
    }
    if set(o.subdivide != d.subdivide) {
        s.subdivide_max_edge = o.subdivide;
    }
    if set(o.uv != d.uv) {
        s.uv_mode = match o.uv.to_ascii_lowercase().as_str() {
            "atlas" => process::UvMode::Atlas,
            "keep" => process::UvMode::Keep,
            other => bail!("unknown uv mode '{other}' (atlas, keep)"),
        };
    }
    if let Some(b) = o
        .bake_distance
        .filter(|_| set(o.bake_distance != d.bake_distance))
    {
        s.bake_distance = b;
    }
    if let Some(a) = o
        .chart_angle
        .filter(|_| set(o.chart_angle != d.chart_angle))
    {
        s.chart_angle = a;
    }
    if set(o.hd_texture != d.hd_texture) {
        s.hd_texture = o.hd_texture;
    }
    if set(o.remesh != d.remesh) {
        s.remesh = o.remesh;
    }
    if set(o.rigid != d.rigid) {
        s.rigid = match o.rigid.to_ascii_lowercase().as_str() {
            "off" | "smooth" => process::RigidMode::Off,
            "one-bone" | "onebone" | "vertex" => process::RigidMode::OneBone,
            "segments" | "rigid" => process::RigidMode::Segments,
            other => bail!("unknown rigid mode '{other}' (off, one-bone, segments)"),
        };
    }
    if set(o.focus_face != d.focus_face) {
        s.focus_face = o.focus_face;
    }
    if let Some(k) = o
        .focus_strength
        .filter(|_| set(o.focus_strength != d.focus_strength))
    {
        s.focus_strength = k;
    }
    Ok(s)
}

/// Built-in and user presets (name, category, description, where it lives).
pub fn list_presets() -> Value {
    let (user, errors) = crate::presets::load_user();
    let row = |p: &crate::presets::Preset| {
        json!({
            "name": p.name,
            "category": p.category,
            "description": p.description,
            "builtin": p.builtin,
            "path": p.path.as_ref().map(|x| x.display().to_string()),
            "tris": if p.convert.use_percent { format!("{}%", p.convert.percent) } else { p.convert.target_tris.to_string() },
            "texture": if p.convert.hd_texture { json!("HD") } else { json!(p.convert.texture.max_size) },
            "colors": if p.convert.hd_texture { json!("full") } else { json!(p.convert.texture.colors) },
            "hd_texture": p.convert.hd_texture,
        })
    };
    json!({
        "builtin": crate::presets::builtins().iter().map(row).collect::<Vec<_>>(),
        "user": user.iter().map(row).collect::<Vec<_>>(),
        "user_dir": crate::presets::user_dir().display().to_string(),
        "errors": errors,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureJob {
    pub inputs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    #[serde(flatten)]
    pub texture: TextureOptions,
    pub indexed: bool,
    /// Treat inputs as PBR material sets: each file (or every set under a folder) is
    /// crunched with its sibling normal/height/AO/metallic/roughness maps.
    pub material: bool,
    #[serde(flatten)]
    pub material_options: crate::material::MaterialSettings,
}

impl Default for TextureJob {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            output_dir: PathBuf::from("psx_out"),
            texture: TextureOptions::default(),
            indexed: true,
            material: false,
            material_options: Default::default(),
        }
    }
}

/// Material mode of [`crunch_textures`]: one output folder per material set.
fn crunch_materials(
    o: &TextureJob,
    s: &texture::CrunchSettings,
    pal: Option<&[Rgb]>,
) -> Result<Value> {
    use crate::material;
    let mut sets: Vec<material::MaterialFiles> = Vec::new();
    for input in &o.inputs {
        let found = if input.is_dir() {
            material::find_all(input)?
        } else {
            vec![material::find_material(input)?.with_context(|| {
                format!(
                    "{}: no material set (needs a colour map such as <name>_basecolor plus at least one other map next to it)",
                    input.display()
                )
            })?]
        };
        for f in found {
            if !sets.iter().any(|s| {
                s.maps.get(&material::MapKind::Color) == f.maps.get(&material::MapKind::Color)
            }) {
                sets.push(f);
            }
        }
    }
    anyhow::ensure!(!sets.is_empty(), "no material sets found in the inputs");
    let mut results = Vec::new();
    for f in &sets {
        let imgs = material::MaterialImages::load(f)?;
        let cm = material::crunch_material(&imgs, s, pal, &o.material_options);
        let dir = o.output_dir.join(&f.name);
        let files = material::export(&dir, &f.name, &cm, o.indexed, o.material_options.godot)?;
        results.push(json!({
            "name": f.name,
            "maps": f.maps.iter().map(|(k, p)| (k.label().to_string(), json!(p.display().to_string()))).collect::<serde_json::Map<_, _>>(),
            "directx_normal": f.directx_normal,
            "outputs": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "width": cm.color.image.width(),
            "height": cm.color.image.height(),
            "colors": texture::count_colors(&cm.color.image),
        }));
    }
    Ok(json!({"materials": results}))
}

pub fn crunch_textures(o: &TextureJob) -> Result<Value> {
    std::fs::create_dir_all(&o.output_dir)?;
    let pal = o.texture.load_palette()?;
    let s = o.texture.crunch()?;
    if o.material {
        return crunch_materials(o, &s, pal.as_deref());
    }
    let mut results = Vec::new();
    for input in &o.inputs {
        let img = image::open(input)
            .with_context(|| format!("opening {}", input.display()))?
            .to_rgba8();
        let c = texture::crunch(&img, &s, &BakeMaps::default(), pal.as_deref());
        let out = o.output_dir.join(format!("{}_psx.png", stem(input)));
        texture::save_png(&out, &c.image, o.indexed)?;
        results.push(json!({
            "input": input.display().to_string(),
            "output": out.display().to_string(),
            "width": c.image.width(),
            "height": c.image.height(),
            "colors": texture::count_colors(&c.image),
            "palette": c.palette.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"results": results}))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SpriteJob {
    pub inputs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    pub strength: f32,
    pub bevel: f32,
    pub bevel_width: f32,
    pub detail: f32,
    /// DirectX-style green channel (Unreal).
    pub flip_y: bool,
    /// Also write a Godot CanvasTexture .tres.
    pub godot: bool,
}

impl Default for SpriteJob {
    fn default() -> Self {
        let d = SpriteSettings::default();
        Self {
            inputs: Vec::new(),
            output_dir: PathBuf::from("psx_out"),
            strength: d.normal_strength,
            bevel: d.bevel,
            bevel_width: d.bevel_width,
            detail: d.detail,
            flip_y: false,
            godot: true,
        }
    }
}

pub fn sprite_maps(o: &SpriteJob) -> Result<Value> {
    std::fs::create_dir_all(&o.output_dir)?;
    let s = SpriteSettings {
        normal_strength: o.strength,
        bevel: o.bevel,
        bevel_width: o.bevel_width,
        detail: o.detail,
        flip_y: o.flip_y,
        ..Default::default()
    };
    let mut results = Vec::new();
    for input in &o.inputs {
        let img = image::open(input)
            .with_context(|| format!("opening {}", input.display()))?
            .to_rgba8();
        let m = sprite::generate(&img, &s);
        let st = stem(input);
        let mut files = Vec::new();
        let mut save = |name: String, img: &image::RgbaImage| -> Result<()> {
            let p = o.output_dir.join(name);
            img.save(&p)?;
            files.push(p);
            Ok(())
        };
        let gray = |g: &image::GrayImage| image::DynamicImage::ImageLuma8(g.clone()).to_rgba8();
        save(format!("{st}_n.png"), &m.normal)?;
        save(format!("{st}_s.png"), &gray(&m.specular))?;
        save(format!("{st}_o.png"), &gray(&m.occlusion))?;
        save(format!("{st}_h.png"), &gray(&m.parallax))?;
        if o.godot {
            let diffuse = format!("{st}.png");
            if !o.output_dir.join(&diffuse).exists() {
                save(diffuse.clone(), &img)?;
            }
            let tres = o.output_dir.join(format!("{st}_canvas.tres"));
            std::fs::write(
                &tres,
                sprite::godot_canvas_texture(
                    &diffuse,
                    &format!("{st}_n.png"),
                    &format!("{st}_s.png"),
                    16.0,
                ),
            )?;
            files.push(tres);
        }
        results.push(json!({"input": input.display().to_string(), "files": paths(&files)}));
    }
    Ok(json!({"results": results}))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScaleJob {
    pub inputs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    /// scale2x, scale3x, scale4x, scale6x, scale9x, eagle, nearest2x/3x/4x
    pub algorithm: String,
    pub tolerance: u8,
    /// Undo an earlier nearest-neighbour upscale by this factor first.
    pub pre_downscale: u32,
}

impl Default for ScaleJob {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            output_dir: PathBuf::from("psx_out"),
            algorithm: "scale3x".into(),
            tolerance: 0,
            pre_downscale: 1,
        }
    }
}

pub fn scale_images(o: &ScaleJob) -> Result<Value> {
    let Some(scaler) = Scaler::from_name(&o.algorithm) else {
        bail!(
            "unknown algorithm '{}' (scale2x, scale3x, scale4x, scale6x, scale9x, eagle, nearest2x/3x/4x)",
            o.algorithm
        )
    };
    std::fs::create_dir_all(&o.output_dir)?;
    let mut results = Vec::new();
    for input in &o.inputs {
        let img = image::open(input)
            .with_context(|| format!("opening {}", input.display()))?
            .to_rgba8();
        let base = if o.pre_downscale > 1 {
            scale::downscale_mode(&img, o.pre_downscale)
        } else {
            img
        };
        let out_img = scaler.apply(&base, o.tolerance);
        let out = o
            .output_dir
            .join(format!("{}_{}x.png", stem(input), scaler.factor()));
        out_img.save(&out)?;
        results.push(json!({"input": input.display().to_string(), "output": out.display().to_string(), "width": out_img.width(), "height": out_img.height()}));
    }
    Ok(json!({"results": results}))
}

/// One block for agent-authored levels: `{"x":0,"y":0,"z":0,"shape":"Wedge","rot":1}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellSpec {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    #[serde(default = "cube")]
    pub shape: Shape,
    #[serde(default)]
    pub rot: u8,
    #[serde(default)]
    pub flip: bool,
    #[serde(default)]
    pub tile: u16,
}

fn cube() -> Shape {
    Shape::Cube
}

/// Fill a box of blocks: `{"from":[0,0,0],"to":[9,0,9],"shape":"Cube","tile":2}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoxSpec {
    pub from: [i32; 3],
    pub to: [i32; 3],
    #[serde(default = "cube")]
    pub shape: Shape,
    #[serde(default)]
    pub rot: u8,
    #[serde(default)]
    pub flip: bool,
    #[serde(default)]
    pub tile: u16,
    /// Carve (remove) instead of fill.
    #[serde(default)]
    pub erase: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BlocksJob {
    /// Existing project to start from (optional).
    pub project: Option<PathBuf>,
    /// Boxes applied in order, then single cells.
    pub boxes: Vec<BoxSpec>,
    pub cells: Vec<CellSpec>,
    pub atlas: Option<PathBuf>,
    pub tile_size: u32,
    pub output_dir: PathBuf,
    pub name: String,
    /// Face grid N x N (1 = off).
    pub subdivide: u32,
    pub godot_collision: bool,
    /// Also save the resulting project JSON here.
    pub save_project: Option<PathBuf>,
    /// Bake AO + sun shadows into the vertex colours (experimental).
    pub light: Option<crate::blocks::LevelLight>,
}

impl Default for BlocksJob {
    fn default() -> Self {
        Self {
            project: None,
            boxes: Vec::new(),
            cells: Vec::new(),
            atlas: None,
            tile_size: 16,
            output_dir: PathBuf::from("psx_out"),
            name: "level".into(),
            subdivide: 1,
            godot_collision: true,
            save_project: None,
            light: None,
        }
    }
}

pub fn build_blocks(o: &BlocksJob) -> Result<Value> {
    let mut w = match &o.project {
        Some(p) => BlockWorld::load(p)?,
        None => BlockWorld {
            tile_size: o.tile_size,
            ..Default::default()
        },
    };
    if o.atlas.is_some() {
        w.atlas_path = o.atlas.as_ref().map(|a| a.display().to_string());
        w.tile_size = o.tile_size;
    }
    for b in &o.boxes {
        let block = (!b.erase).then_some(Block {
            shape: b.shape,
            rot: b.rot % 4,
            flip: b.flip,
            tile: b.tile,
        });
        let n = (0..3)
            .map(|k| (b.from[k] - b.to[k]).unsigned_abs() as u64 + 1)
            .product::<u64>();
        if n > 500_000 {
            bail!("box of {n} blocks is too large (max 500000)");
        }
        w.fill_box(
            glam::IVec3::from_array(b.from),
            glam::IVec3::from_array(b.to),
            block,
        );
    }
    for c in &o.cells {
        w.set(
            glam::IVec3::new(c.x, c.y, c.z),
            Some(Block {
                shape: c.shape,
                rot: c.rot % 4,
                flip: c.flip,
                tile: c.tile,
            }),
        );
    }
    if w.cells.is_empty() {
        bail!("the level has no blocks");
    }
    // Same built-in 16-tile starter atlas as the GUI when none is given.
    let atlas = Some(match &w.atlas_path {
        Some(p) => image::open(p)
            .with_context(|| format!("opening atlas {p}"))?
            .to_rgba8(),
        None => {
            w.tile_size = 16;
            crate::blocks::default_atlas(16)
        }
    });
    let mut model = w.to_model(
        &o.name,
        atlas.as_ref(),
        true,
        o.godot_collision,
        o.subdivide.max(1),
    );
    if let Some(l) = &o.light {
        l.bake(&mut model);
    }
    let s = ExportSettings {
        godot_scene: false,
        indexed_png: false,
        ..Default::default()
    };
    let mut files = export::export_model(&model, &o.output_dir, &s)?;
    if let Some(p) = &o.save_project {
        w.save(p)?;
        files.push(p.clone());
    }
    Ok(json!({"files": paths(&files), "blocks": w.cells.len(), "triangles": model.tri_count()}))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewJob {
    pub input: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Camera orbit angle in degrees around the model.
    pub yaw_degrees: f32,
    /// Animation clip name or index to pose the model in (rigged/morph models).
    pub clip: Option<String>,
    /// Time in seconds within the clip.
    pub time: f32,
    /// "ps1" (default: the PlayStation look) or "pbr" (modern reference shading).
    pub shading: String,
    /// Draw triangle edges on top (judge topology and the triangle budget).
    pub wireframe: bool,
    pub output: Option<PathBuf>,
}

impl Default for PreviewJob {
    fn default() -> Self {
        Self {
            input: PathBuf::new(),
            width: 320,
            height: 240,
            yaw_degrees: 35.0,
            clip: None,
            time: 0.0,
            shading: "ps1".into(),
            wireframe: false,
            output: None,
        }
    }
}

/// PS1-style render of a model (optionally posed). Returns the report and PNG bytes.
pub fn render_preview(o: &PreviewJob) -> Result<(Value, Vec<u8>)> {
    let mut model = Model::load(&o.input)?;
    let mut posed = Value::Null;
    if let (Some(name), true) = (&o.clip, model.rig.is_some()) {
        let Some(ci) = model.clip_index(name) else {
            bail!("clip '{name}' not found")
        };
        model = model.posed(Some(ci), o.time);
        posed =
            json!({"clip": model.rig.as_ref().map(|r| r.clips[ci].name.clone()), "time": o.time});
    }
    let pbr = match o.shading.to_ascii_lowercase().as_str() {
        "ps1" | "psx" => false,
        "pbr" | "reference" => true,
        other => bail!("unknown shading '{other}' (ps1, pbr)"),
    };
    let mut view = export::RenderView::preview(o.yaw_degrees.to_radians(), pbr);
    if o.wireframe {
        view.opts.wireframe = Some([96, 236, 214]);
    }
    let img = export::render_model(
        &model,
        o.width.clamp(16, 1920),
        o.height.clamp(16, 1080),
        &view,
    );
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    if let Some(out) = &o.output {
        std::fs::write(out, &png)?;
    }
    Ok((
        json!({"width": img.width(), "height": img.height(), "output": o.output.as_ref().map(|p| p.display().to_string()), "posed": posed}),
        png,
    ))
}

pub fn extract_palette(path: &Path, colors: usize, save: Option<&Path>) -> Result<Value> {
    let img = image::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .to_rgba8();
    let px: Vec<Rgb> = img
        .pixels()
        .filter(|p| p.0[3] >= 128)
        .map(|p| [p.0[0], p.0[1], p.0[2]])
        .collect();
    let pal: Vec<Rgb> = palette::kmeans_palette(&px, colors.clamp(2, 256), 12)
        .into_iter()
        .map(palette::snap_rgb555)
        .collect();
    let pal = palette::dedup_keep_order(&pal);
    if let Some(s) = save {
        palette::save_palette(s, &pal)?;
    }
    Ok(
        json!({"colors": pal.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>(), "saved": save.map(|s| s.display().to_string())}),
    )
}

/// Experimental: N-direction (x animation frame) billboard sprite sheet of a model.
/// Writes `<name>_sprites.png` and `<name>_sprites.json` (layout) into `out_dir`.
pub fn render_sprites(input: &Path, out_dir: &Path, job: &export::SpriteSheetJob) -> Result<Value> {
    let model = Model::load(input)?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    write_sprite_sheet(&model, stem, out_dir, job)
}

/// Render and write a sprite sheet (PNG, JSON layout, Godot scene + billboard script).
pub fn write_sprite_sheet(
    model: &Model,
    stem: &str,
    out_dir: &Path,
    job: &export::SpriteSheetJob,
) -> Result<Value> {
    let (sheet, info) = export::sprite_sheet(model, job)?;
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let stem = export::sanitize(stem);
    let stem = stem.as_str();
    let png = out_dir.join(format!("{stem}_sprites.png"));
    sheet
        .save(&png)
        .with_context(|| format!("writing {}", png.display()))?;
    let meta = out_dir.join(format!("{stem}_sprites.json"));
    let layout = json!({
        "image": png.file_name().and_then(|n| n.to_str()),
        "layout": info,
        "rows": "directions (row 0 faces the camera, then counter-clockwise seen from above)",
        "columns": "animation frames",
    });
    std::fs::write(&meta, serde_json::to_string_pretty(&layout)?)?;
    // Godot: AnimatedSprite3D scene + the direction-picking script.
    let scene = out_dir.join(format!("{stem}_sprites.tscn"));
    let png_name = png
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sprites.png");
    std::fs::write(&scene, export::godot_sprite_scene(stem, png_name, &info))?;
    let script = out_dir.join(export::GODOT_BILLBOARD_SCRIPT_NAME);
    std::fs::write(&script, export::GODOT_BILLBOARD_SCRIPT)?;
    std::fs::write(
        out_dir.join(format!("{stem}_sprites.png.import")),
        export::godot_texture_import(false),
    )?;
    Ok(json!({
        "files": [png.display().to_string(), meta.display().to_string(), scene.display().to_string(), script.display().to_string()],
        "width": sheet.width(),
        "height": sheet.height(),
        "layout": info,
    }))
}

/// Experimental: lay out textures (PNG files) in PS1 VRAM; optionally save a map image.
pub fn vram_layout(inputs: &[PathBuf], map: Option<&Path>) -> Result<Value> {
    let mut tex = Vec::new();
    for p in inputs {
        let img = image::open(p)
            .with_context(|| format!("opening {}", p.display()))?
            .to_rgba8();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("texture")
            .to_string();
        tex.push((
            name,
            img.width(),
            img.height(),
            crate::texture::count_colors(&img),
        ));
    }
    let est = crate::vram::estimate(&tex);
    let layout = crate::vram::layout(&tex);
    if let Some(m) = map {
        crate::vram::layout_image(&layout)
            .save(m)
            .with_context(|| format!("writing {}", m.display()))?;
    }
    Ok(json!({
        "summary": est.summary(),
        "estimate": est,
        "layout": layout,
        "map": map.map(|m| m.display().to_string()),
    }))
}

fn palette_json(p: &crate::lospec::LospecPalette) -> Value {
    json!({
        "slug": p.slug,
        "name": p.name,
        "author": p.author,
        "colors": p.colors.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>(),
        "use_as": format!("{}{}", crate::lospec::BUILTIN_PREFIX, p.slug),
    })
}

/// The 10 built-in palettes (use as `palette: "builtin:<slug>"`).
pub fn builtin_palettes() -> Value {
    json!({"palettes": crate::lospec::BUILTINS.iter().map(|b| {
        let mut v = palette_json(&b.palette());
        v["note"] = json!(b.note);
        v
    }).collect::<Vec<_>>()})
}

/// Import a Lospec palette by slug or URL. `save`: also write it there (.gpl/.hex/.pal);
/// `library`: add it to the user palette folder (shown in the app's palette library).
pub fn lospec_import(slug_or_url: &str, save: Option<&Path>, library: bool) -> Result<Value> {
    let p = crate::lospec::fetch(slug_or_url)?;
    if let Some(s) = save {
        palette::save_palette(s, &p.colors)?;
    }
    let lib = if library {
        Some(crate::lospec::save_user(&p)?.display().to_string())
    } else {
        None
    };
    let mut v = palette_json(&p);
    // Imported palettes are files, not built-ins.
    v["use_as"] = json!(save.map(|s| s.display().to_string()).or(lib.clone()));
    v["saved"] = json!(save.map(|s| s.display().to_string()));
    v["library"] = json!(lib);
    Ok(v)
}

/// One page of the Lospec palette list.
pub fn lospec_list(q: &crate::lospec::ListQuery) -> Result<Value> {
    let (items, total) = crate::lospec::fetch_list(q)?;
    Ok(json!({
        "total": total,
        "page": q.page,
        "palettes": items.iter().map(|p| {
            let mut v = palette_json(p);
            v["use_as"] = json!(null);
            v
        }).collect::<Vec<_>>(),
    }))
}

pub fn blender_info() -> Value {
    match crate::blender::find_blender() {
        Some(p) => {
            json!({"found": true, "path": p.display().to_string(), "version": crate::blender::version(&p), "import_formats": crate::blender::IMPORT_EXTS})
        }
        None => {
            json!({"found": false, "hint": "Install Blender or set RETROCRUNCH_BLENDER to the blender executable"})
        }
    }
}

/// Convert between any Blender-readable format and GLB/FBX through headless Blender.
pub fn blender_convert(input: &Path, output: &Path) -> Result<Value> {
    crate::blender::to_glb(input, output)?;
    Ok(json!({"output": output.display().to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_is_base_and_only_changed_options_override_it() {
        let hero = crate::presets::find("Hero character").unwrap().convert;
        // Only `tris` differs from the defaults: everything else comes from the preset.
        let o = ModelOptions {
            tris: 900,
            ..Default::default()
        };
        let s = model_settings(&o, Some(hero.clone())).unwrap();
        assert_eq!(s.target_tris, 900);
        assert_eq!(s.texture.max_size, hero.texture.max_size);
        assert_eq!(s.texture.colors, hero.texture.colors);
        // Without a preset every option applies, exactly like before presets existed.
        let s = model_settings(&ModelOptions::default(), None).unwrap();
        assert_eq!(s.target_tris, 600);
        assert_eq!(s.texture.max_size, 128);
        // Photoscan's wider bake search survives when the caller doesn't set it.
        let scan = crate::presets::find("photoscan prop").unwrap().convert;
        assert_eq!(
            model_settings(&ModelOptions::default(), Some(scan))
                .unwrap()
                .bake_distance,
            5.0
        );
    }
}
