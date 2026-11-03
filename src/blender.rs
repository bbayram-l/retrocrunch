//! Blender bridge: find a Blender install and run it headless to
//! (a) import formats psx-forge doesn't read natively (.blend, USD, Alembic, STL, PLY, ...)
//! by converting them to GLB, and (b) turn exported GLBs into FBX for Unity / Mixamo.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Extensions that are routed through Blender on import.
pub const IMPORT_EXTS: &[&str] = &[
    "blend", "usd", "usda", "usdc", "usdz", "abc", "stl", "ply", "dae", "x3d", "wrl",
];

const SCRIPT: &str = r#"
import bpy, sys, os
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst, mode = argv[0], argv[1], argv[2]

def call(op, **kw):
    # Exporter options differ between Blender versions: retry with fewer options.
    try:
        return op(**kw)
    except TypeError:
        return op(filepath=kw["filepath"])

if mode != "blend":
    bpy.ops.wm.read_factory_settings(use_empty=True)
    ext = os.path.splitext(src)[1].lower()
    importers = {
        ".fbx": lambda: bpy.ops.import_scene.fbx(filepath=src),
        ".glb": lambda: bpy.ops.import_scene.gltf(filepath=src),
        ".gltf": lambda: bpy.ops.import_scene.gltf(filepath=src),
        ".obj": lambda: bpy.ops.wm.obj_import(filepath=src),
        ".stl": lambda: bpy.ops.wm.stl_import(filepath=src),
        ".ply": lambda: bpy.ops.wm.ply_import(filepath=src),
        ".usd": lambda: bpy.ops.wm.usd_import(filepath=src),
        ".usda": lambda: bpy.ops.wm.usd_import(filepath=src),
        ".usdc": lambda: bpy.ops.wm.usd_import(filepath=src),
        ".usdz": lambda: bpy.ops.wm.usd_import(filepath=src),
        ".abc": lambda: bpy.ops.wm.alembic_import(filepath=src),
        ".dae": lambda: bpy.ops.wm.collada_import(filepath=src),
        ".x3d": lambda: bpy.ops.import_scene.x3d(filepath=src),
        ".wrl": lambda: bpy.ops.import_scene.x3d(filepath=src),
    }
    if ext not in importers:
        raise RuntimeError("no Blender importer for " + ext)
    importers[ext]()

# Helper objects (e.g. the glTF importer's bone-shape icosphere) must not be exported.
for coll in [c for c in bpy.data.collections if c.name.startswith("glTF_not_exported")]:
    for ob in list(coll.objects):
        bpy.data.objects.remove(ob, do_unlink=True)
    bpy.data.collections.remove(coll)

if os.path.splitext(dst)[1].lower() == ".fbx":
    # Blender's FBX exporter only writes an animation stack for objects that have their
    # own action: shape-key-only animation (e.g. imported from glTF) would be dropped.
    # A constant location key on the object keeps the motion unchanged.
    scn = bpy.context.scene
    for ob in bpy.data.objects:
        key = ob.data.shape_keys if ob.type == "MESH" and ob.data.shape_keys else None
        ad = key.animation_data if key else None
        if ad is None:
            continue
        if ad.action is None:
            strips = [s for t in ad.nla_tracks for s in t.strips]
            if strips:
                ad.action = strips[0].action
        if ad.action and not (ob.animation_data and ob.animation_data.action):
            start, end = (int(f) for f in ad.action.frame_range)
            scn.frame_start, scn.frame_end = min(scn.frame_start, start), max(scn.frame_end, end)
            ob.keyframe_insert("location", frame=start)
    # The FBX exporter only finds textures wired straight into a Principled BSDF's Base
    # Color. glTF imports multiply the image by vertex colour (and unlit materials use an
    # emission graph), so wire the image directly; vertex colours still export as a layer.
    for mat in bpy.data.materials:
        nt = mat.node_tree if mat.use_nodes else None
        img = next((n for n in nt.nodes if n.type == "TEX_IMAGE" and n.image), None) if nt else None
        if img is None:
            continue
        bsdf = next((n for n in nt.nodes if n.type == "BSDF_PRINCIPLED"), None) or nt.nodes.new("ShaderNodeBsdfPrincipled")
        out = next((n for n in nt.nodes if n.type == "OUTPUT_MATERIAL"), None) or nt.nodes.new("ShaderNodeOutputMaterial")
        nt.links.new(img.outputs["Color"], bsdf.inputs["Base Color"])
        nt.links.new(bsdf.outputs[0], out.inputs["Surface"])
    call(bpy.ops.export_scene.fbx, filepath=dst, path_mode="COPY", embed_textures=True,
         add_leaf_bones=False, bake_anim=True, use_mesh_modifiers=True, apply_scale_options="FBX_SCALE_ALL")
else:
    call(bpy.ops.export_scene.gltf, filepath=dst, export_format="GLB", export_apply=True,
         export_animations=True, export_skins=True, export_morph=True)
print("PSXFORGE_OK")
"#;

/// Blender executable: `PSXFORGE_BLENDER`, then PATH, then the usual install folders
/// (newest version first).
pub fn find_blender() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PSXFORGE_BLENDER")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    let exe = if cfg!(windows) {
        "blender.exe"
    } else {
        "blender"
    };
    if let Some(paths) = std::env::var_os("PATH")
        && let Some(p) = std::env::split_paths(&paths)
            .map(|d| d.join(exe))
            .find(|p| p.is_file())
    {
        return Some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for base in [
            "C:\\Program Files\\Blender Foundation",
            "C:\\Program Files (x86)\\Blender Foundation",
        ] {
            if let Ok(dirs) = std::fs::read_dir(base) {
                candidates.extend(dirs.flatten().map(|d| d.path().join(exe)));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local).join("Programs\\Blender Foundation\\Blender\\blender.exe"),
            );
        }
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/Blender.app/Contents/MacOS/Blender",
        ));
    } else {
        candidates.extend(
            [
                "/usr/bin/blender",
                "/usr/local/bin/blender",
                "/snap/bin/blender",
            ]
            .map(PathBuf::from),
        );
    }
    // "Blender 5.2" sorts after "Blender 4.5": take the newest.
    candidates.sort();
    candidates.into_iter().rev().find(|p| p.is_file())
}

/// First line of `blender --version`, e.g. "Blender 5.2.0".
pub fn version(exe: &Path) -> Option<String> {
    let out = Command::new(exe).arg("--version").output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
}

fn require() -> Result<PathBuf> {
    find_blender().context(
        "Blender not found: install Blender (blender.org) or set PSXFORGE_BLENDER to blender.exe",
    )
}

fn run(exe: &Path, input: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut cmd = Command::new(exe);
    // Untrusted files: no user preferences/add-ons and no auto-run of Python scripts
    // embedded in .blend files (drivers, registered text blocks).
    cmd.args(["-b", "--factory-startup", "--disable-autoexec"]);
    if ext == "blend" {
        cmd.arg(input);
    }
    cmd.args(["--python-exit-code", "1", "--python-expr", SCRIPT, "--"])
        .arg(input)
        .arg(output)
        .arg(if ext == "blend" { "blend" } else { "import" });
    let out = cmd
        .output()
        .with_context(|| format!("running {}", exe.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !stdout.contains("PSXFORGE_OK") || !output.is_file() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stdout
            .lines()
            .chain(stderr.lines())
            .filter(|l| !l.trim().is_empty())
            .collect();
        bail!(
            "Blender conversion of {} failed:\n{}",
            input.display(),
            tail[tail.len().saturating_sub(12)..].join("\n")
        );
    }
    Ok(())
}

/// Convert anything Blender can open into a GLB (animations, skins and shape keys kept).
pub fn to_glb(input: &Path, output: &Path) -> Result<()> {
    run(&require()?, input, output)
}

/// Convert a GLB (or other importable file) into a binary FBX with embedded textures.
pub fn to_fbx(input: &Path, output: &Path) -> Result<()> {
    run(&require()?, input, output)
}

/// Temporary GLB path for a converted source file.
pub fn temp_glb(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    std::env::temp_dir()
        .join("psx-forge-blender")
        .join(format!("{stem}_{}.glb", std::process::id()))
}
