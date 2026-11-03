//! psx-forge: a local PS1-style asset pipeline.
//!
//! * `process`  – PS1-style model conversion (decimate, hidden faces, vertex AO, crunch).
//! * `texture` / `palette` – texture crunching, palettes, dithering.
//! * `sprite`   – Normal/specular/occlusion/parallax maps for 2D sprites.
//! * `scale`    – Pixel-art scalers.
//! * `blocks`   – Block level builder.
//! * `raster`   – PS1-accurate software rasterizer used for previews and baking.

pub mod blender;
pub mod blocks;
pub mod decimate;
pub mod export;
pub mod fbx;
pub mod mcp;
pub mod mesh;
pub mod ops;
pub mod palette;
pub mod process;
pub mod raster;
pub mod rig;
pub mod scale;
pub mod sprite;
pub mod texture;
pub mod visibility;
