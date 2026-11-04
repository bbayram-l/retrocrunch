//! retrocrunch: a local PS1-style asset pipeline.
//!
//! * `process`  – PS1-style model conversion (decimate, hidden faces, vertex AO, crunch).
//! * `texture` / `palette` – texture crunching, palettes, dithering.
//! * `sprite`   – Normal/specular/occlusion/parallax maps for 2D sprites.
//! * `scale`    – Pixel-art scalers.
//! * `blocks`   – Block level builder.
//! * `raster`   – PS1-accurate software rasterizer used for previews and baking.

pub mod bake;
pub mod blender;
pub mod blocks;
pub mod crt;
pub mod decimate;
pub mod detail;
pub mod export;
pub mod fbx;
pub mod lospec;
pub mod mcp;
pub mod mesh;
pub mod ops;
pub mod palette;
pub mod palette_cycle;
pub mod presets;
pub mod process;
pub mod raster;
pub mod rig;
pub mod scale;
pub mod sprite;
pub mod texture;
pub mod visibility;
pub mod vram;
pub mod weights;

#[cfg(test)]
mod version_tests {
    /// The changelog must have an entry for the crate version (bump both together).
    #[test]
    fn changelog_lists_the_crate_version() {
        let log = include_str!("../CHANGELOG.md");
        let entry = format!("## [{}]", env!("CARGO_PKG_VERSION"));
        assert!(
            log.contains(&entry),
            "CHANGELOG.md has no '{entry}' section"
        );
    }
}
