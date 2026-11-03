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
    let s = ConvertSettings {
        use_percent: o.percent.is_some(),
        percent: o.percent.unwrap_or(10.0),
        target_tris: o.tris.max(1),
        remove_hidden: o.remove_hidden,
        hidden_ignore_below: o.grounded,
        vertex_ao: o.ao,
        texture: o.texture.crunch()?,
        sizing: if o.texel_cm.is_some() {
            TextureSizing::PixelSize
        } else {
            TextureSizing::Fixed
        },
        texel_cm: o.texel_cm.unwrap_or(2.0),
        units_per_meter: o.units_per_meter,
        geometry_snap: o.snap,
        subdivide_max_edge: o.subdivide,
        ..Default::default()
    };
    let c = process::convert(
        &model,
        &PieceSettings::defaults_for(&model),
        &s,
        pal.as_deref(),
        progress,
    );
    let mut es = parse_engine(&o.engine)?.settings();
    if let Some(sc) = o.scale {
        es.scale = sc;
    }
    if let Some(f) = o.fbx {
        es.fbx = f;
    }
    es.preview = o.preview;
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
            "recommended_min_tris": c.stats.recommended_min_tris,
            "textures": c.stats.textures.iter().map(|t| json!({"material": t.0, "width": t.1, "height": t.2, "colors": t.3})).collect::<Vec<_>>(),
        },
        "palettes": c.palettes.iter().map(|p| p.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "warnings": c.stats.warnings,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureJob {
    pub inputs: Vec<PathBuf>,
    pub output_dir: PathBuf,
    #[serde(flatten)]
    pub texture: TextureOptions,
    pub indexed: bool,
}

impl Default for TextureJob {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            output_dir: PathBuf::from("psx_out"),
            texture: TextureOptions::default(),
            indexed: true,
        }
    }
}

pub fn crunch_textures(o: &TextureJob) -> Result<Value> {
    std::fs::create_dir_all(&o.output_dir)?;
    let pal = o.texture.load_palette()?;
    let s = o.texture.crunch()?;
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
    let model = w.to_model(
        &o.name,
        atlas.as_ref(),
        true,
        o.godot_collision,
        o.subdivide.max(1),
    );
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
            output: None,
        }
    }
}

/// PS1-style render of a model (optionally posed). Returns the report and PNG bytes.
pub fn render_preview(o: &PreviewJob) -> Result<(Value, Vec<u8>)> {
    let mut model = Model::load(&o.input)?;
    let mut posed = Value::Null;
    if let (Some(name), Some(rig)) = (&o.clip, model.rig.clone()) {
        let clip = rig
            .clips
            .iter()
            .position(|c| &c.name == name)
            .or_else(|| name.parse::<usize>().ok().filter(|&i| i < rig.clips.len()));
        let Some(ci) = clip else {
            bail!("clip '{name}' not found")
        };
        let mats = rig.skin_matrices(&rig.world(&rig.pose(Some(ci), o.time)));
        for p in &mut model.pieces {
            let weights = p
                .morph_group
                .and_then(|g| rig.morph_weights(Some(ci), o.time, g))
                .unwrap_or_else(|| p.morph_weights.clone());
            let base = p.morphed_positions(&weights);
            let skin = p.skin.and_then(|s| mats.get(s));
            for (v, b) in p.vertices.iter_mut().zip(base) {
                v.pos = match skin {
                    Some(m) => crate::rig::skin_point(b, v.joints, v.weights, m),
                    None => b,
                };
            }
        }
        posed = json!({"clip": rig.clips[ci].name, "time": o.time});
    }
    let img = export::preview_image(
        &model,
        o.width.clamp(16, 1920),
        o.height.clamp(16, 1080),
        o.yaw_degrees.to_radians(),
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

pub fn blender_info() -> Value {
    match crate::blender::find_blender() {
        Some(p) => {
            json!({"found": true, "path": p.display().to_string(), "version": crate::blender::version(&p), "import_formats": crate::blender::IMPORT_EXTS})
        }
        None => {
            json!({"found": false, "hint": "Install Blender or set PSXFORGE_BLENDER to the blender executable"})
        }
    }
}

/// Convert between any Blender-readable format and GLB/FBX through headless Blender.
pub fn blender_convert(input: &Path, output: &Path) -> Result<Value> {
    crate::blender::to_glb(input, output)?;
    Ok(json!({"output": output.display().to_string()}))
}
