//! Blender bridge: find a Blender install and run it headless to
//! (a) import formats retrocrunch doesn't read natively (.blend, USD, Alembic, STL, PLY, ...)
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
print("RETROCRUNCH_OK")
"#;

/// Blender executable: `RETROCRUNCH_BLENDER`, then PATH, then the usual install folders
/// (newest version first).
pub fn find_blender() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RETROCRUNCH_BLENDER")
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
        "Blender not found: install Blender (blender.org) or set RETROCRUNCH_BLENDER to blender.exe",
    )
}

const REMESH_SCRIPT: &str = r#"
import bpy, sys
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst, target, voxel = argv[0], argv[1], int(argv[2]), float(argv[3])
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.wm.obj_import(filepath=src)
meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
if not meshes:
    raise RuntimeError("nothing to remesh")
bpy.ops.object.select_all(action="DESELECT")
for o in meshes:
    o.select_set(True)
bpy.context.view_layer.objects.active = meshes[0]
if len(meshes) > 1:
    bpy.ops.object.join()
ob = bpy.context.view_layer.objects.active

def weld():
    # Joins UV-seam duplicates, and QuadriFlow rejects near-zero edges.
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.mesh.remove_doubles(threshold=voxel * 0.003)
    bpy.ops.object.mode_set(mode="OBJECT")

def quadriflow():
    # Even, curvature-aligned quads.
    return bpy.ops.object.quadriflow_remesh(mode="FACES", target_faces=target, seed=0)

# Clean meshes (open ones included) remesh directly. Anything QuadriFlow refuses
# (non-manifold, flipped faces, triangle soup) is first rebuilt as one closed surface
# by a voxel remesh; that would turn open surfaces into double-walled shells, so it's
# only the fallback. If QuadriFlow still gives up, the voxel result is kept.
weld()
r = quadriflow()
if r != {"FINISHED"}:
    ob.data.remesh_voxel_size = voxel
    bpy.ops.object.voxel_remesh()
    weld()
    r = quadriflow()
    print("RETROCRUNCH_VOXEL", r)
print("RETROCRUNCH_QUADRIFLOW", r, len(ob.data.polygons))
bpy.ops.wm.obj_export(filepath=dst, export_materials=False, export_uv=False,
                      export_normals=True, export_triangulated_mesh=True)
print("RETROCRUNCH_OK")
"#;

/// QuadriFlow can stall on awkward input; never hang a conversion on it.
const REMESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Run `cmd`, killing it after `limit`. Output goes through files (no pipe deadlock).
/// Returns (success, stdout, stderr).
fn output_with_timeout(
    mut cmd: Command,
    limit: std::time::Duration,
    log: &Path,
) -> Result<(bool, String, String)> {
    let (out_path, err_path) = (log.with_extension("out"), log.with_extension("err"));
    cmd.stdout(std::fs::File::create(&out_path)?)
        .stderr(std::fs::File::create(&err_path)?);
    let mut child = cmd.spawn().context("starting Blender")?;
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let (stdout, stderr) = (read(&out_path), read(&err_path));
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    match status {
        Some(s) => Ok((s.success(), stdout, stderr)),
        None => bail!(
            "Blender took longer than {}s and was stopped",
            limit.as_secs()
        ),
    }
}

/// Remesh results of recent inputs, so conversions that only change textures, colours
/// or lighting don't run Blender again. Keyed by the input geometry and the target size.
static REMESH_CACHE: std::sync::Mutex<Vec<(u64, Vec<crate::mesh::Piece>)>> =
    std::sync::Mutex::new(Vec::new());
/// How many remesh results to keep (a few budgets/models to switch between).
const REMESH_CACHE_SIZE: usize = 4;
/// Number of times Blender actually ran a remesh (for tests and diagnostics).
pub static REMESH_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn remesh_key(pieces: &[crate::mesh::Piece], target_quads: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    target_quads.hash(&mut h);
    for p in pieces {
        p.indices.hash(&mut h);
        for v in &p.vertices {
            v.pos.to_array().map(f32::to_bits).hash(&mut h);
        }
    }
    h.finish()
}

/// `remesh`, reusing the result when the geometry and target haven't changed.
/// Returns the pieces and whether they came from the cache.
pub fn remesh_cached(
    pieces: &[crate::mesh::Piece],
    target_quads: usize,
) -> Result<(Vec<crate::mesh::Piece>, bool)> {
    let key = remesh_key(pieces, target_quads);
    if let Ok(cache) = REMESH_CACHE.lock()
        && let Some((_, hit)) = cache.iter().find(|(k, _)| *k == key)
    {
        return Ok((hit.clone(), true));
    }
    let out = remesh(pieces, target_quads)?;
    if let Ok(mut cache) = REMESH_CACHE.lock() {
        cache.retain(|(k, _)| *k != key);
        cache.insert(0, (key, out.clone()));
        cache.truncate(REMESH_CACHE_SIZE);
    }
    Ok((out, false))
}

/// Rebuild `pieces` as one even, closed surface: voxel remesh, then QuadriFlow to about
/// `target_quads` quads, triangulated. Positions only: UVs, colours, materials, skins and
/// morphs are gone, so callers rebake textures from the original pieces.
pub fn remesh(
    pieces: &[crate::mesh::Piece],
    target_quads: usize,
) -> Result<Vec<crate::mesh::Piece>> {
    use std::fmt::Write as _;
    let exe = require()?;
    REMESH_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join("retrocrunch-blender");
    std::fs::create_dir_all(&dir)?;
    let tag = format!("{}_{}", std::process::id(), target_quads);
    let (src, dst) = (
        dir.join(format!("remesh_in_{tag}.obj")),
        dir.join(format!("remesh_out_{tag}.obj")),
    );
    let mut obj = String::new();
    let mut base = 1usize;
    let mut area = 0.0f32;
    for p in pieces {
        for v in &p.vertices {
            writeln!(obj, "v {} {} {}", v.pos.x, v.pos.y, v.pos.z)?;
        }
        for t in p.indices.chunks_exact(3) {
            writeln!(
                obj,
                "f {} {} {}",
                t[0] as usize + base,
                t[1] as usize + base,
                t[2] as usize + base
            )?;
        }
        base += p.vertices.len();
        area += p.surface_area();
    }
    std::fs::write(&src, obj)?;
    // About 4 voxel faces per target quad: dense enough for QuadriFlow, small enough to be fast.
    let target_quads = target_quads.clamp(16, 200_000);
    let voxel = (area / (target_quads as f32 * 4.0)).sqrt().max(1e-5);
    let mut cmd = Command::new(&exe);
    cmd.args([
        "-b",
        "--factory-startup",
        "--disable-autoexec",
        "--python-exit-code",
        "1",
    ])
    .args(["--python-expr", REMESH_SCRIPT, "--"])
    .arg(&src)
    .arg(&dst)
    .arg(target_quads.to_string())
    .arg(voxel.to_string());
    let out = output_with_timeout(cmd, REMESH_TIMEOUT, &dir.join(format!("remesh_log_{tag}")));
    let _ = std::fs::remove_file(&src);
    let (ok, stdout, stderr) = out?;
    if !ok || !stdout.contains("RETROCRUNCH_OK") || !dst.is_file() {
        let tail: Vec<&str> = stdout
            .lines()
            .chain(stderr.lines())
            .filter(|l| !l.trim().is_empty())
            .collect();
        bail!(
            "Blender remesh failed:\n{}",
            tail[tail.len().saturating_sub(12)..].join("\n")
        );
    }
    let model = crate::mesh::Model::load(&dst);
    let _ = std::fs::remove_file(&dst);
    let mut out_pieces = model?.pieces;
    for p in &mut out_pieces {
        p.name = "remeshed".into();
        p.material = None;
        for v in &mut p.vertices {
            v.color = [1.0; 4];
        }
    }
    Ok(out_pieces)
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
    if !out.status.success() || !stdout.contains("RETROCRUNCH_OK") || !output.is_file() {
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
        .join("retrocrunch-blender")
        .join(format!("{stem}_{}.glb", std::process::id()))
}
