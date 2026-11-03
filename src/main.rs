//! psx-forge: GUI by default, a scriptable CLI (`--json` for machine output), or an MCP
//! server for coding agents (`psx-forge mcp`).

mod app;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use psx_forge::ops::{
    self, BlocksJob, ModelOptions, PreviewJob, ScaleJob, SpriteJob, TextureJob, TextureOptions,
};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "psx-forge",
    version,
    about = "Local PS1-style asset pipeline. Run without arguments for the GUI."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// File to open in the GUI.
    file: Option<PathBuf>,
    /// Print a machine-readable JSON report on stdout (progress goes to stderr).
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert a 3D model (OBJ, GLB/glTF, FBX, or .blend/USD/... via Blender) into a PS1-style asset.
    Model(ModelArgs),
    /// Report triangles, materials, rig, clips and morph targets of a model.
    Inspect { path: PathBuf },
    /// Render a PS1-style preview PNG of a model (optionally posed at a clip/time).
    Preview(PreviewArgs),
    /// Crunch images (resize, palette, RGB555, dither).
    Texture(TextureArgs),
    /// Generate normal/specular/occlusion/height maps for 2D sprites.
    Normals(NormalArgs),
    /// Upscale pixel art with Scale2x/Scale3x/Eagle.
    Scale(ScaleArgs),
    /// Export a block level project (.json) to GLB/OBJ.
    Blocks(BlocksArgs),
    /// Extract a shared N-colour palette from an image.
    Palette {
        image: PathBuf,
        #[arg(long, default_value_t = 16)]
        colors: usize,
        /// Save as .hex, .gpl or .pal.
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Blender bridge: status, or convert any Blender-readable file to GLB/FBX.
    Blender {
        #[command(subcommand)]
        action: BlenderCmd,
    },
    /// Run as an MCP server on stdio for coding agents.
    Mcp {
        /// Workspace folder tool paths must stay inside (default: current directory).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Allow tool paths anywhere on disk.
        #[arg(long, conflicts_with = "root")]
        allow_any_path: bool,
    },
}

#[derive(Subcommand)]
enum BlenderCmd {
    /// Show the Blender executable psx-forge will use.
    Status,
    /// Convert through headless Blender; the output extension (.glb/.fbx) decides the format.
    Convert { input: PathBuf, output: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum Engine {
    Godot,
    Unity,
    Unreal,
    Blender,
    Generic,
}

impl Engine {
    fn name(self) -> &'static str {
        match self {
            Self::Godot => "godot",
            Self::Unity => "unity",
            Self::Unreal => "unreal",
            Self::Blender => "blender",
            Self::Generic => "generic",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PalMode {
    All,
    Best,
}

#[derive(Clone, Copy, ValueEnum)]
enum Dither {
    Off,
    Bayer,
    Ps1,
}

#[derive(Args, Clone)]
struct TexOpts {
    /// Longest texture side in pixels.
    #[arg(long, default_value_t = 128)]
    tex_size: u32,
    /// Palette size (2-256).
    #[arg(long, default_value_t = 16)]
    colors: usize,
    /// Palette file (.gpl .pal .act .hex .txt or swatch image).
    #[arg(long)]
    palette: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = PalMode::Best)]
    palette_mode: PalMode,
    #[arg(long, value_enum, default_value_t = Dither::Bayer)]
    dither: Dither,
    #[arg(long, default_value_t = 12.0)]
    dither_amount: f32,
    #[arg(long, default_value_t = 0.0)]
    brightness: f32,
    /// Disable RGB555 snapping.
    #[arg(long)]
    no_rgb555: bool,
}

impl From<&TexOpts> for TextureOptions {
    fn from(t: &TexOpts) -> Self {
        TextureOptions {
            tex_size: t.tex_size,
            colors: t.colors,
            palette: t.palette.clone(),
            palette_mode: match t.palette_mode {
                PalMode::All => "all",
                PalMode::Best => "best",
            }
            .into(),
            dither: match t.dither {
                Dither::Off => "off",
                Dither::Bayer => "bayer",
                Dither::Ps1 => "ps1",
            }
            .into(),
            dither_amount: t.dither_amount,
            brightness: t.brightness,
            rgb555: !t.no_rgb555,
        }
    }
}

#[derive(Args)]
struct ModelArgs {
    input: PathBuf,
    /// Output folder (a sub-folder named after the model is created).
    #[arg(short, long, default_value = "psx_out")]
    out: PathBuf,
    /// Target triangle count.
    #[arg(short, long, default_value_t = 600)]
    tris: usize,
    /// Target as percentage of the source instead of --tris.
    #[arg(long)]
    percent: Option<f32>,
    #[command(flatten)]
    tex: TexOpts,
    /// Physical texel size in cm (enables Pixel size mode).
    #[arg(long)]
    texel_cm: Option<f32>,
    #[arg(long, default_value_t = 1.0)]
    units_per_meter: f32,
    #[arg(long, value_enum, default_value_t = Engine::Godot)]
    engine: Engine,
    /// Keep faces that are never visible.
    #[arg(long)]
    keep_hidden: bool,
    /// Treat the model as sitting on the ground.
    #[arg(long)]
    grounded: bool,
    /// Disable vertex ambient occlusion.
    #[arg(long)]
    no_ao: bool,
    /// World-space vertex grid (0 = off).
    #[arg(long, default_value_t = 0.0)]
    snap: f32,
    /// Split polygons with edges longer than this (world units) to limit affine warping.
    #[arg(long, default_value_t = 0.0)]
    subdivide: f32,
    /// Override the engine preset's export scale.
    #[arg(long)]
    scale: Option<f32>,
    /// Also export FBX through Blender (default for --engine unity).
    #[arg(long)]
    fbx: bool,
    /// Don't write the 320x240 preview PNG.
    #[arg(long)]
    no_preview: bool,
}

#[derive(Args)]
struct PreviewArgs {
    input: PathBuf,
    #[arg(short, long)]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 320)]
    width: u32,
    #[arg(long, default_value_t = 240)]
    height: u32,
    #[arg(long, default_value_t = 35.0)]
    yaw: f32,
    /// Clip name or index to pose the model in.
    #[arg(long)]
    clip: Option<String>,
    #[arg(long, default_value_t = 0.0)]
    time: f32,
}

#[derive(Args)]
struct TextureArgs {
    /// Image files.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    #[arg(short, long, default_value = "psx_out")]
    out: PathBuf,
    #[command(flatten)]
    tex: TexOpts,
}

#[derive(Args)]
struct NormalArgs {
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    #[arg(short, long, default_value = "psx_out")]
    out: PathBuf,
    #[arg(long, default_value_t = 2.5)]
    strength: f32,
    #[arg(long, default_value_t = 0.6)]
    bevel: f32,
    #[arg(long, default_value_t = 6.0)]
    bevel_width: f32,
    #[arg(long, default_value_t = 1.0)]
    detail: f32,
    /// DirectX-style green channel (Unreal).
    #[arg(long)]
    flip_y: bool,
    /// Also write a Godot CanvasTexture .tres.
    #[arg(long)]
    godot: bool,
}

#[derive(Args)]
struct ScaleArgs {
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    #[arg(short, long, default_value = "psx_out")]
    out: PathBuf,
    /// scale2x, scale3x, scale4x, scale6x, scale9x, eagle, nearest2x/3x/4x
    #[arg(short, long, default_value = "scale3x")]
    algo: String,
    #[arg(long, default_value_t = 0)]
    tolerance: u8,
}

#[derive(Args)]
struct BlocksArgs {
    project: PathBuf,
    #[arg(short, long, default_value = "psx_out")]
    out: PathBuf,
    /// Add Godot's -col suffix for automatic collision.
    #[arg(long)]
    godot_collision: bool,
    /// Split every block face into an N x N grid (1 = off) to limit affine warping.
    #[arg(long, default_value_t = 1)]
    subdivide: u32,
}

/// Human output: list written files on stdout, everything else on stderr.
fn print_report(v: &Value, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        return;
    }
    let mut files: Vec<&str> = Vec::new();
    if let Some(f) = v.get("files").and_then(Value::as_array) {
        files.extend(f.iter().filter_map(Value::as_str));
    }
    if let Some(r) = v.get("results").and_then(Value::as_array) {
        for item in r {
            if let Some(o) = item.get("output").and_then(Value::as_str) {
                files.push(o);
            }
            if let Some(f) = item.get("files").and_then(Value::as_array) {
                files.extend(f.iter().filter_map(Value::as_str));
            }
        }
    }
    if let Some(st) = v.get("stats") {
        eprintln!(
            "{} -> {} triangles ({} hidden removed, {} added by subdivision)",
            st["triangles_before"],
            st["triangles_after"],
            st["hidden_removed"],
            st["subdivided_added"]
        );
        if st["recommended_min_tris"].as_u64().unwrap_or(0) > 0 {
            eprintln!(
                "Recommended minimum for this rig: {} triangles",
                st["recommended_min_tris"]
            );
        }
    }
    if let Some(rig) = v.pointer("/source/rig").filter(|r| !r.is_null()) {
        let clips: Vec<&str> = rig["clips"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| c["name"].as_str()).collect())
            .unwrap_or_default();
        eprintln!(
            "Rig: {} joints, clips: [{}]",
            rig["joints"],
            clips.join(", ")
        );
    }
    for w in v
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        eprintln!("warning: {}", w.as_str().unwrap_or_default());
    }
    if files.is_empty() {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
    for f in files {
        println!("{f}");
    }
}

#[cfg(all(windows, not(debug_assertions)))]
unsafe extern "system" {
    fn FreeConsole() -> i32;
}

fn run_gui(file: Option<PathBuf>) -> Result<()> {
    // Console subsystem keeps CLI output working; detach it for the GUI in release builds.
    #[cfg(all(windows, not(debug_assertions)))]
    // SAFETY: FreeConsole takes no arguments and only detaches this process from its console.
    unsafe {
        FreeConsole();
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("psx-forge")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "psx-forge",
        options,
        Box::new(move |cc| Ok(Box::new(app::PsxForgeApp::new(cc, file)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;
    let progress = move |m: &str, f: f32| {
        if !json {
            eprintln!("  [{:>3.0}%] {m}", f * 100.0);
        }
    };
    let report = match cli.cmd {
        None => return run_gui(cli.file),
        Some(Cmd::Mcp {
            root,
            allow_any_path,
        }) => {
            let root = if allow_any_path {
                None
            } else {
                Some(root.map_or_else(std::env::current_dir, Ok)?)
            };
            return psx_forge::mcp::serve(root);
        }
        Some(Cmd::Model(a)) => {
            let engine = a.engine.name();
            let o = ModelOptions {
                input: a.input,
                output_dir: a.out,
                tris: a.tris,
                percent: a.percent,
                texture: (&a.tex).into(),
                texel_cm: a.texel_cm,
                units_per_meter: a.units_per_meter,
                engine: engine.into(),
                remove_hidden: !a.keep_hidden,
                grounded: a.grounded,
                ao: !a.no_ao,
                snap: a.snap,
                subdivide: a.subdivide,
                scale: a.scale,
                fbx: a.fbx.then_some(true),
                preview: !a.no_preview,
            };
            ops::convert_model(&o, &progress)?
        }
        Some(Cmd::Inspect { path }) => ops::inspect_model(&path)?,
        Some(Cmd::Preview(a)) => {
            let out = a
                .out
                .clone()
                .unwrap_or_else(|| a.input.with_extension("preview.png"));
            let job = PreviewJob {
                input: a.input,
                width: a.width,
                height: a.height,
                yaw_degrees: a.yaw,
                clip: a.clip,
                time: a.time,
                output: Some(out.clone()),
            };
            let (mut v, _) = ops::render_preview(&job)?;
            v["files"] = serde_json::json!([out.display().to_string()]);
            v
        }
        Some(Cmd::Texture(a)) => ops::crunch_textures(&TextureJob {
            inputs: a.inputs,
            output_dir: a.out,
            texture: (&a.tex).into(),
            indexed: true,
        })?,
        Some(Cmd::Normals(a)) => ops::sprite_maps(&SpriteJob {
            inputs: a.inputs,
            output_dir: a.out,
            strength: a.strength,
            bevel: a.bevel,
            bevel_width: a.bevel_width,
            detail: a.detail,
            flip_y: a.flip_y,
            godot: a.godot,
        })?,
        Some(Cmd::Scale(a)) => ops::scale_images(&ScaleJob {
            inputs: a.inputs,
            output_dir: a.out,
            algorithm: a.algo,
            tolerance: a.tolerance,
            pre_downscale: 1,
        })?,
        Some(Cmd::Blocks(a)) => {
            let name = a
                .project
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("level")
                .to_string();
            ops::build_blocks(&BlocksJob {
                project: Some(a.project),
                output_dir: a.out,
                name,
                subdivide: a.subdivide,
                godot_collision: a.godot_collision,
                ..Default::default()
            })?
        }
        Some(Cmd::Palette {
            image,
            colors,
            save,
        }) => ops::extract_palette(&image, colors, save.as_deref())?,
        Some(Cmd::Blender {
            action: BlenderCmd::Status,
        }) => ops::blender_info(),
        Some(Cmd::Blender {
            action: BlenderCmd::Convert { input, output },
        }) => {
            let mut v = ops::blender_convert(&input, &output)?;
            v["files"] = serde_json::json!([output.display().to_string()]);
            v
        }
    };
    print_report(&report, json);
    Ok(())
}
