# Changelog

All notable changes to Retrocrunch (called psx-forge up to 0.8.0). The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) (see "Versioning" in the README).

## [Unreleased]

## [0.12.0] - 2026-11-06

PS1 limits for whole Godot scenes, and PS1 rendering details for Godot: moving
per-vertex lights, sorting without a z-buffer and the PS1's blend modes. Plus pixelised
PBR material sets.

### Added
- PBR material sets in the Texture tab. Opening any map of a set (`<name>_basecolor`,
  `_normal`/`_normalogl`, `_height`, `_ambientocclusion`, `_metallic`, `_roughness`,
  `_emissive`...) loads the whole set. Every map is pixelised onto the colour texture's
  grid, AO/roughness/metallic are packed into one `_orm.png` (R/G/B), and "Save
  material..." also writes a Godot `ORMMaterial3D` with nearest filtering. AO and normal
  relief can be baked into the colour instead. "Batch materials..." does every set in a
  folder and its subfolders. DirectX normal maps are converted to OpenGL. Also as
  `crunch_texture {material: true}` and `retrocrunch texture --material`.
- Godot: `psx_draw_distance.gd` (written with every Godot export) finds how far the
  camera can see before the objects in view pass a polygon budget, and cuts everything
  farther (like Silent Hill without its fog) or tints it red. A debug tool for finding
  how far each scene can draw.
- The Godot PS1 Budget dock's limits are settings, saved per project: polygon budget,
  VRAM size (1, 2, 4 or 8 MB), framebuffers kept in VRAM (0, 1 or 2) and their
  resolution. Free texture memory follows from them (724 KB for the PS1 default).
- Near-camera tessellation (`tessellate`, `--tessellate`, Export > Near detail; 0-3):
  versions with long edges split (`<name>_near1.glb`...) that the Godot scene of a
  static model shows up close, like PS1 games did against affine texture warping.
  Each version's pieces carry their triangle ratio as metadata.
- Godot: `psx_detail_heatmap.gd` colours near-detail and LOD versions by their triangles
  against the converted model (yellow 2x to red 8x, blue for LODs).
- Godot: `psx_vram_overlay.gd` draws a picture of PS1 video memory over the game: the
  textures on screen packed next to the framebuffers and colour tables, as a PS1 game
  would, with totals and what doesn't fit. Same packing as the `vram` command.
- Godot: `psx_lights.gd` lets ordinary DirectionalLight3D, OmniLight3D and SpotLight3D
  nodes light the PS1 materials, per vertex like the PS1's GTE (up to 8, nearest first,
  up to 2x brightness), before the 15-bit colour and dither.
- Godot PS1 materials: `flat_depth` gives each triangle one depth, so big polygons sort
  as a whole like on a PS1 with no z-buffer; `depth_bias` nudges a material.
- PS1 semi-transparency: materials named `..._avg`, `_add`, `_add25` or `_sub` get the
  PS1 blend rules (B/2+F/2, B+F, B+F/4, B-F), computed on 8-bit values like the
  hardware (`psx_average/add/add_quarter/subtract.gdshader`).
- Godot PS1 materials: `uv_scale` repeats a texture across plain meshes.
- `examples/godot_demo`: a Godot project with all of the above on a night street that's
  far over budget: PS1 materials, lamps and a flashlight, the speed limit, draw
  distance, near-detail road with the heatmap, the VRAM overlay and the budget dock,
  with a fly camera and a key for every mode.

### Changed
- Godot PS1 materials always write depth (for `flat_depth`), which turns off the GPU's
  early depth test for them; at PS1 scene sizes the cost is small.
- The draw distance node and the budget dock skip LOD and near-detail versions outside
  their distance band, as Godot does; they counted every version before.
- The budget dock's "To fit" line now gives the distance to draw up to and names only
  the farthest few objects to clip; a big street listed over a hundred.

### Fixed
- The Texture tab lagged with large (4K) images: it counted the original's colours on
  every frame.
- The Sprite Lights preview lagged with large images: the lit preview is now computed on
  all cores and at most 1024 px wide, and the maps regenerate when a slider is released.

## [0.11.1] - 2026-11-06

The first macOS build, and file-reference checks that work the same on every system.

### Added
- macOS builds for Apple Silicon: `Retrocrunch.app` with a Fox icon, ad-hoc signed, in
  a zip with the guide and the Blender and Godot folders. Every tagged version is now
  published on GitHub Releases for Windows and macOS.

### Fixed
- File references inside models (textures, MTL files, glTF buffers) are checked the same
  way on every system. On macOS, Windows-style references such as `C:\Users\...` or
  `\\host\share\...` weren't recognised as pointing outside the model's folder.
- glTF files whose buffer or image URIs contain a backslash are now refused on every
  system. URIs must use `/`; on macOS a name with a backslash could be a different file
  than the one checked.

## [0.11.0] - 2026-11-05

PS1 budgets for whole scenes: polygons counted the PS1 way, and two Godot tools that
measure and simulate a scene's cost.

### Fixed
- Reduced characters no longer lose their arms and legs when one part (a dense, seamed
  head) resists reduction: at 600 triangles CesiumMan kept 587 on the head and almost
  nothing elsewhere. The reduction now lets a few UV seams move when that keeps far more
  of the shape, so every limb survives (the head drops to 85).
- Godot exports now include a `.glb.import` that turns off Godot's automatic mesh LODs.
  Godot drew distant models with a fraction of their triangles (a 600-triangle
  character at 150), so they looked smoother than the budget and every polygon count
  was off.

### Added
- Model reports (`inspect_model`, and `source`/`result` in `convert_model`) count
  `quads` and `polygons` next to `triangles`, for budgets in the PS1 sense where a flat
  quad is one polygon. Two triangles make a quad when they share an edge (same corner
  positions and UVs), are within 10 degrees of flat and form a convex shape.
- The convert report's `atlas` shows `fill_percent`, the share of the atlas the model
  actually covers. Everything else is padding and gaps that still cost VRAM. The GUI
  shows it under the atlas settings.
- Godot: `psx_speed_limit.gd` (written with every Godot export) caps the frame rate to
  what the PS1 GPU could draw, from the triangles Godot drew each frame: 180 textured
  polygons per ms, so 60 fps up to 3000, 30 up to 6000, 20 up to 9000. Flat shading,
  PAL and a smooth mode are options.
- Godot: a scene budget editor add-on (`godot/addons/retrocrunch_budget`). Its dock
  measures the open scene: polygons in view against 3000/6000, texture VRAM in PS1
  formats against 724 KB (shared textures counted once), the most detailed objects, and
  which far objects to clip to fit, with a suggested `visibility_range_end`.

## [0.10.0] - 2026-11-05

Cleaner, sharper rebaked textures.

### Fixed
- Rebaked textures no longer take colour from a neighbouring surface the low-poly model
  keeps separately: a neck inside a collar picked up the collar's white and maroon, the
  collar picked up skin and hair, an ear picked up hair. The bake still paints detail that
  decimation removed (pockets, straps, baked pieces) onto the surface beneath it.

### Changed
- UV atlases pack about 15-50% more texels onto the model (charts lie flat and fill the
  gaps between taller ones), so the same texture size looks sharper.

## [0.9.0] - 2026-11-04

First release as Retrocrunch, and the first ready-built release on itch.io.

### Added
- Licence: PolyForm Shield 1.0.0 (source-available; use Retrocrunch for anything,
  including commercial games, but don't sell or redistribute it). Converted output is
  yours; `godot/` and `blender/` are MIT No Attribution. Contribution terms in
  `CONTRIBUTING.md`, third-party notices in `THIRD_PARTY_LICENSES.html`.

### Changed
- **Renamed psx-forge to Retrocrunch** (PSX is a Sony trademark). The program is now
  `retrocrunch` (`retrocrunch.exe`), the crate `retrocrunch`, the MCP server `retrocrunch`,
  the Blender add-on `retrocrunch_addon.py`, and the environment variables
  `RETROCRUNCH_BLENDER`, `RETROCRUNCH_PRESETS` and `RETROCRUNCH_PALETTES`. The settings
  folder `%APPDATA%\psx-forge` (presets, palettes, window state) is moved to
  `%APPDATA%\retrocrunch` on first start. Godot shader and script names (`psx_*`) are
  unchanged.
- New README with art rendered by Retrocrunch itself (`tools/readme-art/` rebuilds it);
  the full reference moved to `docs/GUIDE.md`.
- Help > About links the itch.io download page and the source code, and shows the
  licence.
- Internal clean-up, no behaviour change (41 reference outputs byte-identical):
  `export`, `mesh` and the Model tab are split into focused modules, one copy of the
  sRGB transfer curve is shared by the rasterizer, palettes and CRT pass, and the
  remaining clippy warnings are fixed.

### Fixed
- Detail focus could destroy a model when the rest of the mesh resisted reduction (UV
  seams everywhere, as on photoscans): the focus area itself was collapsed to meet the
  budget. A photoscanned gas mask at 600 triangles came out in scattered fragments; it
  now keeps the mask, hose and canister. Models that never hit this convert exactly as before.

## [0.8.0] - 2026-11-04

Eight experimental features.

### Added
- **CRT view**: viewport toggle (OpenGL pass and CPU) and `crt` on the Godot screen
  pass: composite blur with colour bleed, scanlines, aperture grille, vignette.
- **Fixed-point (GTE) wobble**: viewport toggle and `gte_unit` in the Godot shaders.
  View-space positions round to a fixed grid before projection, so vertices shift
  against each other as the camera moves, like the PS1 geometry chip.
- **Automatic LODs**: `lods` (GUI Export, `--lods`, MCP) writes `<name>_lod1.glb`... with
  half the triangles each; Godot scenes of static models switch by distance
  (visibility ranges).
- **PS1-style stiff skinning** for rigged models: one bone per vertex (closed mesh), or
  rigid per-bone segments (`--rigid one-bone|segments`, MCP `rigid`).
- **Sprite sheets to Godot**: `<name>_sprites.tscn` (AnimatedSprite3D, one animation per
  direction, sized like the 3D model) and `psx_billboard.gd`, which plays the row that
  faces the camera.
- **VRAM layout packer**: textures placed in PS1 VRAM inside texture pages next to the
  framebuffers, CLUTs below; a 1024x512 map (GUI Export > VRAM map, `psx-forge vram`,
  MCP `vram_layout`).
- **Baked lighting for block levels**: ambient occlusion and sun shadows in the vertex
  colours (Blocks tab, MCP `build_blocks` `light`).
- **Palette cycling**: rotate a range of palette entries over time (Texture tab preview);
  exports an index PNG, a palette strip, Godot shaders (2D and 3D) and an animated GIF.

### Fixed
- Built-in presets with '/' in their name ("NPC / enemy", "Item / pickup",
  "Boss / showcase", "Distant LOD / background") could not be used from the CLI or MCP
  since 0.2.1: they were taken for file paths. Names are now matched first.

## [0.7.0] - 2026-11-04

### Added
- **Wireframe** toggle in the viewport toolbar ("Wire"): visible edges only, jittered
  with the mesh, on the OpenGL and CPU renderers. `render_preview` / `psx-forge preview`
  take `wireframe` / `--wireframe`.
- **View modes**: Shaded, Texture only (no baked lighting), Vertex lighting (AO, sun and
  shadows only), Normals, and UV checker (texel density and stretching).
- Experimental: **PS1 VRAM meter**. Every conversion reports how much of the 1 MB VRAM
  its textures and CLUTs would take (4/8/16-bit packing, % of what's free next to two
  320x240 framebuffers), with warnings past the 256x256 texture page or the VRAM size.
  Shown in the viewport overlay and as `vram` in the JSON report.
- Experimental: **sprite sheets**. Render a model from 8 (or N) directions, optionally
  with a clip's frames, into a transparent sheet for Doom-style 2.5D billboards.
  Export > Sprite sheet in the GUI, `psx-forge sprites`, MCP `render_sprites`.

### Changed
- `Model::posed` and `export::render_model` are shared by previews and sprite sheets.

## [0.6.0] - 2026-11-04

### Added
- **Palette library** (palette panel > Library..., Edit menu): 10 built-in palettes
  from lospec.com with author credits (PICO-8, Sweetie 16, DawnBringer 16/32,
  Endesga 32, AAP-64, Resurrect 64, Apollo, Steam Lords, Darkseed 16); browse the
  Lospec palette list by tag, colour count and sort order; import by name or URL;
  "My palettes" folder (`%APPDATA%\psx-forge\palettes`, `PSXFORGE_PALETTES`).
- `builtin:<slug>` works anywhere a palette file does (CLI `--palette`, MCP `palette`).
- CLI `psx-forge palettes` (list built-ins, import, browse) and MCP tool `palettes`.
- Dither modes **Bayer 8x8** and **Floyd-Steinberg** (serpentine error diffusion in
  linear light) for textures and models; CLI `--dither bayer8|floyd`.
- `CHANGELOG.md` and a versioning policy.

### Changed
- Ordered dithering with a palette picks each colour's mixing partner from the whole
  palette (best reproduction of the colour, penalising far-apart pairs), not just the
  second-nearest entry. Palette error on test textures: 7-18% lower; Floyd-Steinberg
  21-43% lower than before on three of four tests. The approach follows Aseprite's
  ordered dithering, reimplemented independently.
- Tooltips explain what the dither amount means with and without a palette.

### Fixed
- "Remesh first" no longer reruns Blender on every change: the result is cached by input
  geometry and target size, so texture, colour and lighting edits reuse it. Changing
  the pieces, the triangle budget or the remesh density still remeshes.
- Choosing a palette (library, Load..., drag and drop) now changes the colours: Model
  and Texture switch to "Imported: all colours". The default auto (k-means) mode ignored
  the loaded palette.

### Security
- Lospec names and tags are validated before they go into a URL; replies are
  size-capped and colours must be strict hex. Over MCP, imports only write to the
  requested (workspace-checked) `save` path.

## [0.5.0] - 2026-11-04

### Added
- **Detail focus** (on by default): the head is found from head bones or the shape of an
  upright character; it gets a guaranteed share of the triangle budget and up to
  `focus_strength` x texel density. Heat map and paint/erase brush in the GUI;
  `--no-face-focus`, `--focus-strength`, MCP `focus_face`/`focus_strength`.

### Fixed
- Faces collapsing first next to large armour, spikes or capes.

## [0.4.0] - 2026-11-04

### Added
- **Remesh first** for static models (Blender QuadriFlow, voxel remesh as fallback),
  then decimation and an atlas rebake; `--remesh`, MCP `remesh`.
- **Rig viewer and weight editor**: bone tree, skeleton overlay, test posing, weight
  heat map and a visibility-aware brush (add/subtract/set/smooth), normalise, undo.

### Fixed
- Leaving a pose (back to bind pose) left skinned meshes in the last pose.

## [0.3.0] - 2026-11-04

### Added
- Godot shader variants `psx_{opaque,cutout,blend}[_double]` with a shared include,
  chosen per material; double-sided materials (glTF `doubleSided`).
- Optional 240-line Godot screen pass (`psx_screen.gd`, `psx_post.gdshader`).
- `.png.import` files: lossless, no VRAM compression, mipmaps for HD textures.
- Oklab palettes and palette-aware ("two nearest") ordered dithering.

### Changed
- Godot colour maths match the psx-forge preview (95.8% of pixels identical in
  Godot 4.7). `psx_lit.gdshader` is replaced by the variants; `snap_resolution` is now
  `snap_lines`.

## [0.2.1] - 2026-11-04

### Security
- Triangles with out-of-range indices are dropped on load (GPU out-of-bounds reads).
- MCP `preset` paths are sandboxed to the workspace with the same test the loader uses.

## [0.2.0] - 2026-11-04

### Added
- Fresh UV atlas and rebake from the full-resolution source; bake loose pieces into the
  texture.
- Presets: built-in templates (including a "High quality" group), user presets, and
  `--preset`.
- HD texture mode (low-poly geometry, source-resolution full-colour texture).
- PBR reference view for the original model.
- OpenGL viewport mirroring the CPU PS1 rasterizer (CPU fallback).
- Clear / Revert-to-defaults buttons on every tab.

### Fixed
- 16-colour cutout textures now fit a 4-bit CLUT (transparency reserves index 0).

## [0.1.1] - 2026-11-03

### Security
- Model files and MCP agents are treated as untrusted input: texture/buffer paths stay in
  the model's folder, MCP paths stay in the workspace, Blender runs with auto-exec off.
- Crafted glTF URIs no longer crash the loader.

## [0.1.0] - 2026-11-03

### Added
- First release: PS1-style model conversion (decimation, hidden-face removal, vertex
  lighting, texture crunching to CLUT/RGB555 with dithering), rigs, morphs and
  animation passthrough, FBX import, Blender bridge, Godot/Unity/Unreal export,
  2D sprite maps, pixel-art scaling, a block level editor, CLI with `--json` and an MCP
  server.

Each version is tagged in git as `v<version>` (for example `git show v0.5.0`).
