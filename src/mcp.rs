//! Model Context Protocol server (`psx-forge mcp`): JSON-RPC 2.0 over stdio, one message
//! per line. Exposes the `ops` operations as tools so coding agents (Claude Code, Codex,
//! Cursor, ...) can convert assets, inspect results and *see* PS1-style previews.
//!
//! stdout carries protocol messages only; diagnostics go to stderr.

use crate::ops;
use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

fn texture_props() -> Value {
    json!({
        "tex_size": {"type": "integer", "description": "Longest texture side in pixels (4-2048). Default 128."},
        "colors": {"type": "integer", "description": "Palette size 2-256 (16 = 4-bit CLUT). Default 16."},
        "palette": {"type": "string", "description": "Palette file (.gpl .pal .act .hex .txt or swatch image) shared across assets."},
        "palette_mode": {"type": "string", "enum": ["best", "all"], "description": "best: pick the N best palette entries; all: use every entry."},
        "dither": {"type": "string", "enum": ["bayer", "ps1", "off"]},
        "dither_amount": {"type": "number", "description": "Dither amplitude in 8-bit units. Default 12."},
        "brightness": {"type": "number", "description": "-0.5..0.5, applied before the palette."},
        "rgb555": {"type": "boolean", "description": "Snap colours to 15-bit like the PS1. Default true."}
    })
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
    a
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
    })
}

pub fn tools() -> Vec<Value> {
    vec![
        tool(
            "inspect_model",
            "Load a 3D model (OBJ, GLB/glTF, FBX, or via Blender: .blend/USD/Alembic/STL/PLY) and report triangles, pieces, materials, bounds, rig joints, animation clips, morph targets and the recommended minimum triangle count for rigs. Use before convert_model to pick a budget.",
            json!({"path": {"type": "string", "description": "Model file path"}}),
            &["path"],
        ),
        tool(
            "convert_model",
            "Convert a model into a PS1-style asset: decimate to a triangle budget, remove hidden faces, bake ambient occlusion into vertex colours, crunch textures to a small palette with dithering, optionally split large polygons, and export for an engine (Godot scene + PSX shader, Unity FBX, Unreal, Blender, generic). Skeletons, skin weights, morph targets and animation clips are kept. Returns written files, stats and warnings; call render_preview on the result to look at it.",
            merge(
                json!({
                    "input": {"type": "string", "description": "Source model path"},
                    "output_dir": {"type": "string", "description": "Folder; a sub-folder named after the model is created. Default psx_out"},
                    "tris": {"type": "integer", "description": "Triangle budget. PS1 props 100-500, characters 300-1500. Default 600"},
                    "percent": {"type": "number", "description": "Budget as % of the source instead of tris"},
                    "texel_cm": {"type": "number", "description": "Pixel-size mode: one physical texel size (cm) for every asset"},
                    "units_per_meter": {"type": "number"},
                    "engine": {"type": "string", "enum": ["godot", "unity", "unreal", "blender", "generic"]},
                    "remove_hidden": {"type": "boolean"},
                    "grounded": {"type": "boolean", "description": "Model sits on the ground: ignore views from below"},
                    "ao": {"type": "boolean"},
                    "snap": {"type": "number", "description": "World-space vertex grid, 0 = off"},
                    "subdivide": {"type": "number", "description": "Split polygons with edges longer than this (world units) to limit affine warping, 0 = off"},
                    "scale": {"type": "number", "description": "Export scale override (Unreal preset uses 100)"},
                    "fbx": {"type": "boolean", "description": "Also write FBX via Blender"},
                    "preview": {"type": "boolean", "description": "Write a 320x240 preview PNG. Default true"},
                    "uv": {"type": "string", "enum": ["atlas", "keep"], "description": "atlas (default): fresh UV atlas, everything rebaked into one texture - no seam smearing at low budgets. keep: original UVs"},
                    "bake_pieces": {"type": "array", "items": {"type": "string"}, "description": "Piece names (see inspect_model) to remove from the geometry and paint into the texture underneath, e.g. pockets, straps, decals"},
                    "bake_distance": {"type": "number", "description": "Rebake search distance, % of model size. Default 3; raise if baked detail is missing"},
                    "chart_angle": {"type": "number", "description": "Atlas chart crease angle in degrees. Default 60"},
                    "focus_face": {"type": "boolean", "description": "Default true: find the head (head bones, or the shape of an upright character) and keep more triangles and texels on the face"},
                    "focus_strength": {"type": "number", "description": "Extra detail for the face: collapse cost up to this squared, texel density up to this many times. Default 4, 1 = off"},
                    "remesh": {"type": "boolean", "description": "Static models only, needs Blender: voxel remesh + QuadriFlow before decimation for even triangles (less affine warping) and repaired scans. Textures are rebaked into a fresh atlas"},
                    "hd_texture": {"type": "boolean", "description": "HD texture: keep the source's texel density and full colour (no palette/dither/15-bit) on the low-poly model - pixel-sharp but high resolution. tex_size and colors are ignored"},
                    "preset": {"type": "string", "description": "Start from a preset (see list_presets), e.g. 'Hero character', 'Environment prop', 'Survival horror character'. Other arguments you pass override it"}
                }),
                texture_props(),
            ),
            &["input"],
        ),
        tool(
            "render_preview",
            "Render a model the way a PlayStation would (240p, vertex jitter, affine textures, dither, 15-bit colour) and return the image so you can look at it. Optionally pose a rigged/morph model at a clip and time.",
            json!({
                "input": {"type": "string", "description": "Model path (e.g. the GLB written by convert_model)"},
                "width": {"type": "integer"}, "height": {"type": "integer"},
                "yaw_degrees": {"type": "number", "description": "Orbit angle around the model"},
                "clip": {"type": "string", "description": "Animation clip name or index"},
                "time": {"type": "number", "description": "Seconds into the clip"},
                "shading": {"type": "string", "enum": ["ps1", "pbr"], "description": "ps1 (default): PlayStation look. pbr: modern reference render of the source (metal/rough, normal maps, emission) to compare against"},
                "output": {"type": "string", "description": "Also save the PNG here"}
            }),
            &["input"],
        ),
        tool(
            "crunch_texture",
            "Crunch images to PS1 texture specs (resize, palette via k-means or an imported palette, RGB555, Bayer or PS1 dither) and write indexed PNGs.",
            merge(
                json!({"inputs": {"type": "array", "items": {"type": "string"}}, "output_dir": {"type": "string"}, "indexed": {"type": "boolean"}}),
                texture_props(),
            ),
            &["inputs"],
        ),
        tool(
            "sprite_maps",
            "Generate 2D lighting maps for sprites (normal, specular, occlusion, height) and optionally a Godot CanvasTexture .tres.",
            json!({
                "inputs": {"type": "array", "items": {"type": "string"}}, "output_dir": {"type": "string"},
                "strength": {"type": "number"}, "bevel": {"type": "number"}, "bevel_width": {"type": "number"},
                "detail": {"type": "number"}, "flip_y": {"type": "boolean", "description": "DirectX green channel (Unreal)"},
                "godot": {"type": "boolean"}
            }),
            &["inputs"],
        ),
        tool(
            "scale_pixel_art",
            "Upscale pixel art without adding colours (Scale2x/3x/4x/6x/9x, Eagle, nearest).",
            json!({
                "inputs": {"type": "array", "items": {"type": "string"}}, "output_dir": {"type": "string"},
                "algorithm": {"type": "string", "enum": ["scale2x", "scale3x", "scale4x", "scale6x", "scale9x", "eagle", "nearest2x", "nearest3x", "nearest4x"]},
                "tolerance": {"type": "integer"}, "pre_downscale": {"type": "integer"}
            }),
            &["inputs"],
        ),
        tool(
            "build_blocks",
            "Build a block level (1x1x1 bloxels: Cube, Slab, Wedge (ramp), Corner, InnerCorner, Pillar; rot = quarter turns around Y) from boxes and cells, with hidden faces culled, and export GLB (Godot -col collision optional). Boxes are applied before cells; set erase to carve.",
            json!({
                "project": {"type": "string", "description": "Existing project JSON to start from"},
                "boxes": {"type": "array", "items": {"type": "object", "properties": {
                    "from": {"type": "array", "items": {"type": "integer"}}, "to": {"type": "array", "items": {"type": "integer"}},
                    "shape": {"type": "string"}, "rot": {"type": "integer"}, "flip": {"type": "boolean"}, "tile": {"type": "integer"}, "erase": {"type": "boolean"}}}},
                "cells": {"type": "array", "items": {"type": "object", "properties": {
                    "x": {"type": "integer"}, "y": {"type": "integer"}, "z": {"type": "integer"},
                    "shape": {"type": "string"}, "rot": {"type": "integer"}, "flip": {"type": "boolean"}, "tile": {"type": "integer"}}}},
                "atlas": {"type": "string"}, "tile_size": {"type": "integer"},
                "output_dir": {"type": "string"}, "name": {"type": "string"},
                "subdivide": {"type": "integer", "description": "Face grid N x N (1 = off)"},
                "godot_collision": {"type": "boolean"},
                "save_project": {"type": "string"}
            }),
            &[],
        ),
        tool(
            "extract_palette",
            "Extract an N-colour, 15-bit-snapped palette from an image (hex list), optionally saving .hex/.gpl/.pal so every asset can share it.",
            json!({"path": {"type": "string"}, "colors": {"type": "integer"}, "save": {"type": "string"}}),
            &["path"],
        ),
        tool(
            "list_presets",
            "List conversion presets: built-in templates for asset types and art styles (characters, environment, props, vehicles, strict PS1, concept-art match, ...) and the user's saved presets. Pass a name as convert_model's `preset`.",
            json!({}),
            &[],
        ),
        tool(
            "blender_info",
            "Report whether Blender is available for .blend/USD/Alembic import and FBX export.",
            json!({}),
            &[],
        ),
        tool(
            "blender_convert",
            "Convert any Blender-readable file to GLB or FBX (output extension decides) through headless Blender, keeping animations, skins and shape keys.",
            json!({"input": {"type": "string"}, "output": {"type": "string"}}),
            &["input", "output"],
        ),
    ]
}

fn parse<T: DeserializeOwned>(args: &Value) -> Result<T> {
    serde_json::from_value(args.clone()).map_err(|e| anyhow!("invalid arguments: {e}"))
}

fn str_arg(args: &Value, key: &str) -> Result<std::path::PathBuf> {
    args.get(key)
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .with_context(|| format!("missing '{key}'"))
}

fn text(v: &Value) -> Value {
    json!({"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()})
}

/// Run one tool; returns MCP content blocks.
pub fn call_tool(name: &str, args: &Value) -> Result<Vec<Value>> {
    let quiet = |_: &str, _: f32| {};
    let report = match name {
        "inspect_model" => ops::inspect_model(&str_arg(args, "path")?)?,
        "convert_model" => ops::convert_model(&parse(args)?, &quiet)?,
        "render_preview" => {
            let (report, png) = ops::render_preview(&parse(args)?)?;
            return Ok(vec![
                text(&report),
                json!({"type": "image", "mimeType": "image/png", "data": base64(&png)}),
            ]);
        }
        "crunch_texture" => ops::crunch_textures(&parse(args)?)?,
        "sprite_maps" => ops::sprite_maps(&parse(args)?)?,
        "scale_pixel_art" => ops::scale_images(&parse(args)?)?,
        "build_blocks" => ops::build_blocks(&parse(args)?)?,
        "extract_palette" => {
            let colors = args.get("colors").and_then(Value::as_u64).unwrap_or(16) as usize;
            let save = args
                .get("save")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from);
            ops::extract_palette(&str_arg(args, "path")?, colors, save.as_deref())?
        }
        "blender_info" => ops::blender_info(),
        "list_presets" => ops::list_presets(),
        "blender_convert" => {
            ops::blender_convert(&str_arg(args, "input")?, &str_arg(args, "output")?)?
        }
        other => anyhow::bail!("unknown tool '{other}'"),
    };
    Ok(vec![text(&report)])
}

/// Handle one JSON-RPC message; `None` for notifications.
/// Argument keys that carry file paths (strings or arrays of strings).
const PATH_KEYS: &[&str] = &[
    "input",
    "inputs",
    "output",
    "output_dir",
    "path",
    "save",
    "palette",
    "project",
    "atlas",
    "save_project",
];

/// Resolve `p` (relative to the current directory) without requiring it to exist:
/// canonicalise the longest existing ancestor and re-append the rest.
fn resolve(p: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut existing = abs.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(f), Some(parent)) => {
                rest.push(f.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for part in rest.iter().rev() {
        out.push(part);
    }
    if out.components().any(|c| c == Component::ParentDir) {
        anyhow::bail!(
            "path '{}' uses '..' outside an existing folder",
            p.display()
        );
    }
    Ok(out)
}

/// Every path argument must stay inside the workspace `root`, so a confused or
/// prompt-injected agent can't read or overwrite files elsewhere through this server.
pub fn check_paths(args: &Value, root: &Path) -> Result<()> {
    let shown = root.display().to_string();
    let root = root
        .canonicalize()
        .with_context(|| format!("workspace root {shown}"))?;
    let mut values: Vec<&str> = Vec::new();
    for key in PATH_KEYS {
        match args.get(*key) {
            Some(Value::String(s)) => values.push(s),
            Some(Value::Array(a)) => values.extend(a.iter().filter_map(Value::as_str)),
            _ => {}
        }
    }
    // `preset` is a name unless it looks like a file path, which must be inside the root.
    if let Some(Value::String(s)) = args.get("preset")
        && crate::presets::is_path_like(s)
    {
        values.push(s);
    }
    for v in values {
        let r = resolve(Path::new(v))?;
        if !r.starts_with(&root) {
            anyhow::bail!(
                "path '{v}' is outside the workspace {} (start the server with --root <dir> or --allow-any-path)",
                shown
            );
        }
    }
    Ok(())
}

/// `root`: confine tool paths to this folder (None = unrestricted).
pub fn handle(msg: &Value, root: Option<&Path>) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications (no id) never get a response
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let err = |code: i64, message: String| {
        Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
    };
    match method {
        "initialize" => {
            let requested = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            let version = if SUPPORTED.contains(&requested) {
                requested
            } else {
                PROTOCOL
            };
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "psx-forge", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "PS1-style asset pipeline. Typical flow: inspect_model -> convert_model -> render_preview (look at the image) -> adjust tris/colors/subdivide and repeat. Paths are local file paths.",
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(json!({}));
            // Tool failures are results with isError, so the agent can read and recover.
            let result = match root {
                Some(r) => check_paths(&args, r).and_then(|()| call_tool(name, &args)),
                None => call_tool(name, &args),
            };
            match result {
                Ok(content) => ok(json!({"content": content, "isError": false})),
                Err(e) => ok(
                    json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
                ),
            }
        }
        "resources/list" => ok(json!({"resources": []})),
        "prompts/list" => ok(json!({"prompts": []})),
        _ => err(-32601, format!("method not found: {method}")),
    }
}

/// Blocking stdio loop.
/// Blocking stdio loop. Relative paths resolve against `root` (the working directory).
pub fn serve(root: Option<PathBuf>) -> Result<()> {
    if let Some(r) = &root {
        std::env::set_current_dir(r)
            .with_context(|| format!("entering workspace {}", r.display()))?;
        eprintln!("psx-forge mcp: workspace {}", r.display());
    } else {
        eprintln!("psx-forge mcp: unrestricted paths (--allow-any-path)");
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, root.as_deref()),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
            ),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{}", serde_json::to_string(&r)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Standard base64 (RFC 4648) for image content.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle_any(msg: &Value) -> Option<Value> {
        handle(msg, None)
    }

    #[test]
    fn workspace_root_rejects_outside_paths() {
        let root = std::env::temp_dir().join("psxforge_root_test");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let inside = root.join("assets").join("new").join("out.glb");
        assert!(check_paths(&json!({"output": inside.display().to_string()}), &root).is_ok());
        let outside = std::env::temp_dir().join("elsewhere.glb");
        assert!(
            check_paths(
                &json!({"inputs": [inside.display().to_string(), outside.display().to_string()]}),
                &root
            )
            .is_err()
        );
        let outside_preset = std::env::temp_dir().join("evil_preset.json");
        assert!(
            check_paths(
                &json!({"preset": outside_preset.display().to_string()}),
                &root
            )
            .is_err(),
            "preset files outside the workspace are rejected"
        );
        for sneaky in ["evil.JSON", "evil.json/.", "../evil", "sub\\evil"] {
            let p = std::env::temp_dir().join(sneaky);
            assert!(
                check_paths(&json!({"preset": p.display().to_string()}), &root).is_err(),
                "{sneaky}"
            );
        }
        assert!(check_paths(&json!({"preset": "Hero character"}), &root).is_ok());
        let sneaky = root.join("assets").join("..").join("..").join("x.glb");
        assert!(check_paths(&json!({"output_dir": sneaky.display().to_string()}), &root).is_err());
        let msg = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "inspect_model", "arguments": {"path": outside.display().to_string()}}});
        let r = handle(&msg, Some(&root)).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("outside the workspace")
        );
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn initialize_list_and_errors() {
        let init = handle_any(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert!(
            handle_any(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none()
        );
        let list = handle_any(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"convert_model") && names.contains(&"render_preview"));
        let bad = handle_any(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "inspect_model", "arguments": {"path": "nope.glb"}}})).unwrap();
        assert_eq!(bad["result"]["isError"], true);
        let unknown = handle_any(&json!({"jsonrpc": "2.0", "id": 4, "method": "nope"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn build_blocks_and_preview_through_tools() {
        let dir = std::env::temp_dir().join("psxforge_mcp_test");
        let _ = std::fs::remove_dir_all(&dir);
        let args = json!({
            "boxes": [{"from": [0, 0, 0], "to": [3, 0, 3]}],
            "cells": [{"x": 1, "y": 1, "z": 1, "shape": "Wedge", "rot": 1}],
            "output_dir": dir.display().to_string(), "name": "arena", "subdivide": 2
        });
        let out = call_tool("build_blocks", &args).unwrap();
        let report: Value = serde_json::from_str(out[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["blocks"], 17);
        let glb = dir.join("arena.glb");
        assert!(glb.is_file());
        let prev = call_tool(
            "render_preview",
            &json!({"input": glb.display().to_string(), "width": 64, "height": 48}),
        )
        .unwrap();
        assert_eq!(prev[1]["type"], "image");
        assert!(
            prev[1]["data"].as_str().unwrap().starts_with("iVBOR"),
            "PNG signature in base64"
        );
        let info = call_tool("inspect_model", &json!({"path": glb.display().to_string()})).unwrap();
        assert!(info[0]["text"].as_str().unwrap().contains("\"triangles\""));
    }
}
