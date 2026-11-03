# psx-forge

A free, local PS1-style asset pipeline written in Rust. One desktop app (plus a CLI) with five tools:

| Tab | What it does |
|---|---|
| **Model** | Decimate OBJ, GLB/glTF, FBX (and .blend/USD/Alembic/STL/PLY via Blender) to a triangle budget, remove faces nobody sees, bake AO/sun into vertex colours, crunch textures to a tiny CLUT, split big polygons, preview like a PlayStation, export for Godot/Unity/Unreal/Blender. Skeletons, skin weights, morph targets (blend shapes) and animation clips are kept |
| **Texture** | Resize, brightness/contrast/saturation, k-means or imported palette, RGB555 snap, Bayer / PS1 dither, cutout alpha, indexed PNG, batch |
| **Sprite Lights** | Normal, specular, occlusion and parallax maps from 2D sprites (brightness emboss + alpha bevel), live point-light preview, Godot `CanvasTexture` export, batch |
| **Pixel Scale** | Scale2x/3x/4x/6x/9x, Eagle, nearest; colour tolerance; mode-downscale; batch |
| **Blocks** | Block level builder: cube / slab / ramp / outer & inner corner / pillar, rotate & flip, atlas tiles, box fill/carve, paint, pick, undo; optional 2x2-4x4 face grid against affine warping; exports a culled GLB/OBJ with Godot `-col` collision |

## Build

```powershell
cargo build --release
.\target\release\psx-forge.exe            # GUI
.\target\release\psx-forge.exe model.glb  # GUI with a file open
```

Requires the Rust MSVC toolchain plus the Visual Studio C++ build tools (meshoptimizer is C++).
`.cargo/config.toml` overrides a global `CC=gcc` (MSYS2) so the cc crate uses MSVC's `cl.exe`.

## GUI

- Drag and drop files onto the window: models go to **Model**, palettes (`.gpl .pal .act .hex .txt`) go to the palette library, and images go to the current image tab.
- **Model viewport**: LMB orbit, RMB/MMB or Shift+LMB pan, wheel zoom. Header toggles cover jitter, affine UVs, dither, 15-bit colour, Z-buffer (off = PS1 ordering-table sorting), textures, fog, turntable and resolution (224p / 240p / 480i / native).
- **Pieces list**: enable/delete, merge (same material), per-piece budget weight, undo/redo (Ctrl+Z / Ctrl+Y).
- **Split polygons over**: edges longer than this (world units) are split without cracks, so affine warping stays small on big floors and walls. 0 turns it off.
- **Rigged models**: an Animation bar plays and scrubs every clip in the PS1 viewport (CPU skinning). The panel suggests a minimum triangle count for the rig, about 20 triangles per deforming joint; you can go lower, it only warns.
- **Pixel size mode**: one texel size (cm) for the whole game, so every asset gets the atlas resolution that matches.
- **Quick Export** (Ctrl+Shift+E): pick an engine, then a folder.
- **Blocks**: LMB applies the tool, RMB orbits, MMB pans. Keys: Q/E/P/B/C/I select tools, 1-6 select shapes, R rotates, F flips, PgUp/PgDn change the build layer. Projects save as JSON.
- Settings are remembered between sessions. You can change UI scale and theme in File → Preferences.

## CLI

```powershell
psx-forge model helmet.glb -o out --tris 800 --tex-size 128 --colors 32 --engine godot
psx-forge model car.obj --texel-cm 4 --palette shared.gpl --palette-mode all --grounded
psx-forge texture *.png -o out --tex-size 64 --colors 16 --dither ps1
psx-forge normals hero.png enemy.png -o out --strength 3 --godot
psx-forge scale sprite.png -o out -a scale3x
psx-forge model fox.glb -o out --tris 500 --subdivide 0.5        # rigged: skeleton + clips kept
psx-forge blocks level.json -o out --godot-collision --subdivide 4
psx-forge model hero.fbx --engine unity --tris 900                # FBX in, FBX out (via Blender)
psx-forge model scene.blend -o out                                # .blend/USD/Alembic via Blender
psx-forge inspect hero.fbx --json                                 # triangles, rig, clips, morphs
psx-forge preview out/hero/hero.glb --clip Run --time 0.4 -o run.png
psx-forge palette concept.png --colors 16 --save shared.gpl
psx-forge blender status
psx-forge blender convert scene.blend scene.glb
psx-forge mcp                                                     # MCP server for coding agents
```

`psx-forge <command> --help` lists every option. Add `--json` to any command for a machine-readable report on stdout; progress goes to stderr.

## Blender pipeline

psx-forge finds Blender through `PSXFORGE_BLENDER`, `PATH`, or the standard install folders (newest version first). Check with `psx-forge blender status`.

- **Import:** `.blend`, `.usd/.usda/.usdc/.usdz`, `.abc`, `.stl`, `.ply`, `.dae` and `.x3d` open in every tab and command. Headless Blender converts them to a temporary GLB, keeping armatures, shape keys and actions. FBX is read natively (below) and only falls back to Blender if ufbx rejects a file.
- **FBX export:** the FBX checkbox, `--fbx` or the Unity preset writes a binary FBX with embedded textures through Blender, with skeleton and clips kept, ready for Unity Humanoid or Mixamo. Two Blender quirks are worked around: shape-key-only animation gets a constant object key so the exporter writes the stack, and textures are wired straight into the Principled BSDF so the exporter finds them. Without Blender, an OBJ is written instead and the import notes say so.
- **Add-on (Blender → psx-forge):** install `blender/psx_forge_addon.py` with Edit > Preferences > Add-ons > Install from Disk, then point it at `psx-forge.exe` in its preferences (or set `PSXFORGE_EXE`). It adds a 3D Viewport sidebar tab, **PSX Forge**:
  - **PSX-ify Selected** exports the selection with its armature, shape keys and actions, converts it, and imports the PS1 version into a `PSX_<name>` collection next to the original.
  - **Export for Engine** writes engine-ready files to a folder.
  - **Open in psx-forge** opens the selection in the app for interactive tuning.

## FBX import

FBX is read natively with [ufbx](https://github.com/ufbx/ufbx): meshes and material parts, embedded or external textures, skin clusters, blend shapes, and every animation stack (baked to linear keys, with blend-shape weights included). Scenes are converted to metres, Y-up and right-handed. Static meshes under animated nodes become rigid skinned parts, the same as glTF.

## Agents (MCP)

`psx-forge mcp` is a Model Context Protocol server on stdio. Register it with your agent:

```powershell
claude mcp add psx-forge -- D:\path\to\psx-forge.exe mcp          # Claude Code
```

```toml
# Codex: ~/.codex/config.toml
[mcp_servers.psx-forge]
command = "D:\\path\\to\\psx-forge.exe"
args = ["mcp"]
```

Tools: `inspect_model`, `convert_model`, `render_preview` (returns the PS1-style render as an image, so the agent can look at its result), `crunch_texture`, `sprite_maps`, `scale_pixel_art`, `build_blocks` (author levels from JSON boxes and cells), `extract_palette`, `blender_info` and `blender_convert`. Failures come back as `isError` results the agent can read and act on. See `AGENTS.md` for the recommended loop.

All tool paths must stay inside a workspace folder: the server's working directory by default, or `--root <dir>`. Use `--allow-any-path` to lift the restriction.

### Untrusted files

Model files are treated as untrusted. Textures, MTL files and glTF buffers referenced from inside a model are only looked up in the model's own folder (absolute, UNC and `..` references are reduced to the file name). glTF files pointing outside their folder are refused. Blender always runs with `--factory-startup --disable-autoexec`, so Python embedded in a `.blend` never runs.

## Godot 4 workflow

The Godot preset writes `name.glb`, `name.png` (indexed, nearest), `psx_lit.gdshader` and `name.tscn`.
Copy the folder into your project and instance the `.tscn`: every mesh already has the PSX material.
The material does vertex snapping to `snap_resolution`, affine UVs (`affine_weight` 0-1 blends from perspective-correct to full PS1 warping), vertex-colour lighting, 15-bit colour with the PS1 dither table, and optional fog. The viewport's "warp" value uses the same maths.
Use the shader on any other mesh too.

Rigged models import with a `Skeleton3D`, a skinned mesh and an `AnimationPlayer` holding every clip. Godot moves skinned meshes under the skeleton, so their `.tscn` uses `psx_apply_materials.gd` (a `@tool` script on the root) to assign the PSX materials by material name.

For sprites, assign `name_canvas.tres` (a `CanvasTexture` with diffuse + normal + specular) as the Sprite2D texture and add a `PointLight2D`.

For block levels, the `-col` node suffix makes Godot generate a `StaticBody3D` trimesh collider on import.

## How it works

- `src/process.rs`: pipeline (gather pieces, distribute budget, decimate with meshoptimizer, remove hidden faces, light vertices, crunch textures, shared palette, vertex grid).
- `src/raster.rs`: PS1-accurate CPU rasterizer (integer vertex snap, affine mapping, ordering table, dither, RGB555). It drives the viewports and the 96-view ID/depth renders used for hidden-face removal and ambient occlusion.
- `src/texture.rs`, `src/palette.rs`: crunching, k-means++, palette I/O (GIMP, JASC/RIFF PAL, ACT, HEX, paint.net, swatch images), indexed PNG.
- `src/sprite.rs`, `src/scale.rs`, `src/blocks.rs`: the 2D and level-building tools.
- `src/export.rs`: OBJ/MTL, GLB (skins, animations, KHR_materials_unlit, nearest samplers), Godot scene and shader.
- `src/rig.rs`: skeleton, skins and clips (transform and morph-weight channels); keyframe sampling (linear, step, cubic spline), pose evaluation, linear blend skinning, rigid skins for node-animated meshes.
- `src/fbx.rs`: native FBX import through ufbx.
- `src/blender.rs`: headless Blender bridge (import formats, FBX export); `blender/psx_forge_addon.py` is the in-Blender side.
- `src/ops.rs`: typed operations shared by the CLI (`--json`) and the MCP server (`src/mcp.rs`).

`cargo test` runs 59 unit tests plus a Blender + FBX round-trip integration test (skipped when Blender isn't installed). Together they cover palettes, crunching, decimation, visibility, the rasterizer, rigs, morphs and skinning, subdivision, exporters, the MCP protocol, sprite maps, scalers and blocks.

## How rigs are handled

- Skinned meshes stay in bind space; node transforms are ignored for them, as glTF specifies.
- Up to 4 influences per vertex are kept. JOINTS_1/WEIGHTS_1 are merged in, keeping the strongest 4, renormalised.
- Decimation only collapses onto existing vertices, so surviving vertices keep their weights. Vertices shared by several joints (elbows, knees) get extra priority, and the "sloppy" fallback is never used on rigs.
- Hidden-face removal never deletes faces from skinned pieces, because faces hidden in the bind pose (armpits, inner thighs) show up when the character moves. They still occlude other pieces.
- Plain meshes parented to a joint (helmets, swords) become rigid attachments with weight 1 on that joint.
- Plain meshes under an animated node (doors, fans, the inner box of a toy) join a synthetic `psx_rigid` skin, so node animation survives every exporter and engine and plays in the preview.
- Morph targets keep their names (from `targetNames`, accessor names or FBX channel names) and default weights. Their deltas stay aligned with vertices through decimation, compaction and subdivision. Morphed vertices get decimation priority, and morph pieces are protected from hidden-face removal. Weight animation (glTF `weights`, FBX `DeformPercent`) is exported and previewed.
- The export scale also scales node translations, inverse bind matrices and translation keys, so a rig exported at 100x for Unreal still animates correctly.

## Not supported yet

- OBJ has no skeleton or animation in the format, so rigged models export to OBJ in their bind pose. Use GLB or FBX for animation.
- Fresh UV atlas unwrap and re-bake. The original UVs are kept, so very low budgets on models with many UV charts can smear textures at seams. Raise "UV seam care" or the triangle budget.
- "Bake loose pieces into the texture". Use Delete on the piece instead.
