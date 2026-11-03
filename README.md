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
- **Palettes** are built and matched in Oklab, so colours go where the eye sees differences. With a palette, dithering mixes each texel's two nearest palette colours with the ordered matrix (Bayer or the PS1 table), positioned in linear light; `dither amount` 16 is full mixing. Without a palette (RGB555 only) dithering is the PS1 GPU's offset before truncation.
- **HD texture** (Texture > Quality > HD, `--hd-texture`, MCP `hd_texture`): low-poly PS1 geometry wearing the source's full-resolution, full-colour texture. The new atlas is sized to keep the source's texel density (up to 4096 px); with Keep original UVs the source textures pass through untouched. No palette, dither or 15-bit snap; texels stay pixel-sharp (nearest filtering, mipmapped in the Godot shader so big textures don't shimmer). The PS1 screen effects (jitter, affine, dither) still apply in the viewport and the Godot shader.
- **Presets**: the Preset picker applies a built-in template or one of your saved presets. Its `...` menu has Save as preset, Import/Export preset file, Delete, Open presets folder and Reload. "(modified)" shows when you've changed settings since applying. User presets are JSON files in `%APPDATA%\psx-forge\presets` (or `~/.config/psx-forge/presets`; override with `PSXFORGE_PRESETS`).
- **Revert to defaults** resets the tab's settings; **Clear** unloads the current asset and keeps the settings. Both are on every tab (Blocks has Defaults; New clears the level).
- **GPU viewport**: on by default (the `GPU` toggle). OpenGL renders the same PS1 pipeline as the CPU rasterizer (jitter, affine weight, dither table, 15-bit, fog) at the retro resolution and scales it up with nearest filtering, so Native resolution and big models stay smooth. The CPU rasterizer takes over when Z-buffer is off (the PS1 ordering table needs painter's sorting), when OpenGL is unavailable, or if the shaders fail (the toggle's tooltip shows why). Previews written by the CLI/MCP always use the CPU.
- **Original view** is a modern PBR reference: GGX with metal/roughness maps, normal maps, emission, AO and a studio environment, rendered on all cores. The PS1 toggles (jitter, affine, dither, 15-bit, fog) only affect the PSX view.
- **Pieces list**: enable/delete, merge (same material), per-piece budget weight, **Bake** (remove the piece from the geometry but paint it into the texture underneath; **Restore** brings it back), undo/redo (Ctrl+Z / Ctrl+Y).
- **UV layout**: **New atlas** (default) unwraps the low-poly mesh into one tightly packed atlas and rebakes every material from the full-resolution source, so low budgets don't smear textures across seams and the model uses a single texture page. **Keep original** keeps the source UVs (best for tiling textures).
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
psx-forge model jacket.glb --tris 400 --bake-piece pocket --bake-piece strap   # paint loose pieces into the texture
psx-forge model tiles.obj --uv keep                               # keep original UVs (tiling textures)
psx-forge model scene.blend -o out                                # .blend/USD/Alembic via Blender
psx-forge inspect hero.fbx --json                                 # triangles, rig, clips, morphs
psx-forge presets                                                 # built-in templates + your presets
psx-forge model zombie.fbx --preset "Survival horror character" --tris 700   # preset, budget overridden
psx-forge preview helmet.glb --shading pbr -o reference.png       # modern reference render
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

The Godot preset writes `name.glb`, `name.png` (indexed, nearest) with a `name.png.import`, `name.tscn`, the PSX shaders and the screen pass.
Copy the folder into your project and instance the `.tscn`: every mesh already has the PSX material.

**Materials.** Each material gets the shader variant it needs, chosen from its alpha and glTF `doubleSided`:

| shader | used for |
|---|---|
| `psx_opaque.gdshader` | solid surfaces (no `discard`, keeps the early depth test) |
| `psx_cutout.gdshader` | 1-bit alpha (foliage, fences, hair cards): alpha scissor, stays in the opaque pass |
| `psx_blend.gdshader` | partial alpha or base-colour alpha < 1: alpha blending |
| `*_double.gdshader` | the same, with both faces drawn |

They share `psx_common.gdshaderinc`: vertex snapping to `snap_lines` (the width follows the viewport's aspect), affine UVs, colour and fog (`affine_weight` 0-1 blends from perspective-correct to full PS1 warping), vertex-colour lighting, 15-bit colour with the PS1 dither table, and optional fog. The colour maths match psx-forge's preview: texture x vertex colour x tint in 8-bit sRGB values, truncated like the hardware. In Godot 4.7 a render of an exported model matches the preview for 95.8% of pixels exactly (99.5% within 8 levels); keep the Environment tonemap on Linear. The viewport's "warp" value uses the same maths. Use the shaders on any other mesh too.

**Screen pass.** `psx_screen.gd` + `psx_post.gdshader`: add a Control with the script to your main scene and put the 3D level (camera included) under it. It renders at 240 lines (width from the window's aspect), upscales with nearest filtering and applies the PS1 dither and 15-bit colour once per virtual pixel, like the real GPU, switching those off in the psx materials under it. Mouse and keyboard input are forwarded into the low-resolution viewport; Controls under it stay at full resolution on top.

**Import settings.** The `.png.import` files keep textures lossless without VRAM compression (Godot's "Detect 3D" would otherwise compress them and smear the palette) and keep transparent texels exact. HD textures (over 512 px) get mipmaps, sampled with nearest-mipmap filtering.

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

## Presets

Built-in templates:

| Category | Presets |
|---|---|
| Characters | Hero character, NPC / enemy, Survival horror character, Boss / showcase |
| Environment | Architecture (tiling), Environment prop (2 cm texels), Distant LOD / background |
| Props & items | Item / pickup, Weapon (first person), Photoscan prop |
| Vehicles | Vehicle |
| Style | Strict PS1 hardware, Concept art match (uses your loaded palette), Clean low-poly (no dither) |
| High quality | HQ character (6000 tris, 1024 px), HQ prop (2500, 512 px), HQ environment (8000, tiling UVs), HQ vehicle (5000, 1024 px), PSX model, HD texture (1500 tris, source texture), Late-90s PC (high-res textures), Max quality (archive: 50% tris, 2048 px) |

The High quality templates go past PS1 limits (256 colours, no 15-bit snap, little or no dither) for PS2/Dreamcast-era looks, remasters, or a clean master to crunch further later.

A preset stores the full conversion settings, plus export settings if you choose to include them. Built-ins leave your engine choice alone. On the CLI and over MCP, `preset` is the base and any option that differs from its default overrides it.

## UV atlas and rebake

- **Charts:** faces are grouped while their normals stay within the chart angle (default 60 degrees) of the chart's largest face, so a chart's projection never folds. Slivers left by decimation merge into a neighbouring chart. Each chart is projected onto its average plane at one uniform texel density (Pixel size mode stays exact) and rotated to its tightest bounding box.
- **Packing:** charts are shelf-packed into a square atlas, with the largest density that fits found by binary search. Gutters are 2 texels (1 at 128 px and below) and filled by dilation, so nearest sampling never shows seams.
- **Rebake:** each atlas texel averages up to 6x6 samples, enough to cover the source texels it spans. Each sample is a ray from just above the low-poly surface back along its normal (the search distance defaults to 3% of the model size). The outermost front-facing source hit wins, with the closest point as a fallback. The source colour is texture x base colour x vertex colour, with AO, normal relief and emission already applied.
- **Bake into texture:** pieces marked **Bake** are part of the bake source but not of the output geometry. Because the outermost hit wins, a strap, pocket or decal sitting on the surface is painted onto the surface underneath. Baking pieces always uses the atlas.

## Not supported yet

- OBJ has no skeleton or animation in the format, so rigged models export to OBJ in their bind pose. Use GLB or FBX for animation.
