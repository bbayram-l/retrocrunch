# psx-forge for coding agents

psx-forge turns models, textures and sprites into PS1-style game assets. Agents use it
through the MCP server (`psx-forge mcp`) or the CLI with `--json`. Both call the same
operations (`src/ops.rs`) and return the same JSON reports.

## Setup

```powershell
cargo build --release
claude mcp add psx-forge -- C:\path\to\psx-forge\target\release\psx-forge.exe mcp
```

Blender is optional. It enables `.blend`/USD/Alembic/STL/PLY import and FBX export.
Check it with the `blender_info` tool or `psx-forge blender status`.

## Recommended loop for a model

1. `inspect_model {path}` reports triangles, pieces, materials, bounds, rig (joints,
   clips) and morph targets. For rigs, `recommended_min_tris` is a lower bound for
   clean joint bending.
2. `convert_model {input, output_dir, tris, colors, tex_size, engine, ...}` returns the
   written files, before/after stats, palettes and warnings. Read `warnings`: a budget
   below the rig minimum is flagged there.
3. `render_preview {input: <the .glb from step 2>, yaw_degrees, clip?, time?}` returns
   a PS1-style render as an image. **Look at it** before reporting success. Pose rigged
   or morph models with `clip` and `time` to check deformation.
4. Adjust and repeat. Typical knobs:
   - Silhouette too blocky: raise `tris`.
   - Texture muddy: raise `colors` or `tex_size`, or lower `dither_amount`.
   - Floors and walls warp: set `subdivide` to about 1/4 of the model size.
   - Holes in the mesh: set `remove_hidden: false`.

PS1 budgets for reference: props 100-500 triangles, characters 300-1500, 64-256 px
textures, 16-colour (4-bit) or 256-colour (8-bit) palettes.

## Shared palette across a project

Use `extract_palette {path: <concept art or key asset>, colors: 16, save: shared.gpl}`.
Then pass `palette: "shared.gpl", palette_mode: "all"` to every `convert_model` /
`crunch_texture` call, so all assets share one CLUT.

## Engines

| engine | writes |
|---|---|
| `godot` | `.glb`, indexed `.png`, `psx_lit.gdshader`, `.tscn` with the PSX material already assigned (rigged scenes use `psx_apply_materials.gd`) |
| `unity` | `.fbx` via Blender (OBJ if Blender is missing), `.png` |
| `unreal` | `.glb` at 100x scale |
| `blender`, `generic` | `.glb` (+ `.obj` for generic) |

Every export also writes `IMPORT_NOTES.txt` with engine-specific import steps, plus a
320x240 `_preview.png`.

## Levels

`build_blocks` takes boxes and cells. Shapes are `Cube`, `Slab`, `Wedge` (ramp, rises
toward -Z at rot 0), `Corner`, `InnerCorner` and `Pillar`. `rot` is quarter turns
around +Y; `flip` puts the shape upside down. Boxes are applied before cells;
`erase: true` carves.

```json
{"name": "arena", "output_dir": "out", "subdivide": 2, "godot_collision": true,
 "boxes": [{"from": [-8, 0, -8], "to": [8, 0, 8], "tile": 1},
           {"from": [-8, 1, -8], "to": [8, 3, -8], "tile": 0}],
 "cells": [{"x": 0, "y": 1, "z": 0, "shape": "Wedge", "rot": 2}],
 "save_project": "out/arena.json"}
```

The saved project opens in the GUI's Blocks tab for hand editing.

## 2D

- `sprite_maps {inputs, godot: true}`: normal, specular, occlusion and height maps, plus a
  Godot `CanvasTexture`.
- `scale_pixel_art {inputs, algorithm: "scale3x"}`: smooth upscales with no new colours.
- `crunch_texture {inputs, tex_size, colors}`: PS1 texture specs.

## Conventions

- Paths are local file paths; outputs go to `output_dir` (default `psx_out`). Every path
  must be inside the server's workspace (its working directory, or `--root`). Paths
  outside it fail with an `isError` result.
- Tool errors return `isError: true` with a readable message. Fix the arguments and
  retry instead of giving up.
- With `--json`, stdout holds only the JSON report; progress goes to stderr.
