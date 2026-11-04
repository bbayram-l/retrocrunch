# Changelog

All notable changes to psx-forge. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) (see "Versioning" in the README).

## [Unreleased]

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
