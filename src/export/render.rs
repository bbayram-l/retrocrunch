//! CPU renders of models: PS1-style previews and sprite sheets for 2.5D billboards.

use crate::mesh::Model;
use serde::{Deserialize, Serialize};

/// Render the model like a PlayStation would (jitter, affine, dither, 15-bit) at `w`x`h`.
pub fn preview_image(model: &Model, w: u32, h: u32, yaw: f32) -> image::RgbaImage {
    preview_image_shaded(model, w, h, yaw, false)
}

/// `pbr`: modern reference render (metal/rough, normal maps, emission, environment)
/// instead of the PS1 look; used to compare a conversion against its source.
pub fn preview_image_shaded(
    model: &Model,
    w: u32,
    h: u32,
    yaw: f32,
    pbr: bool,
) -> image::RgbaImage {
    render_model(model, w, h, &RenderView::preview(yaw, pbr))
}

/// Camera and look for `render_model`.
#[derive(Clone, Debug)]
pub struct RenderView {
    pub yaw: f32,
    pub pitch: f32,
    pub pbr: bool,
    pub opts: crate::raster::RasterOptions,
    /// Vertical gradient (top, bottom); None = transparent.
    pub background: Option<([u8; 3], [u8; 3])>,
    /// Frame these bounds instead of the model's own (keeps the scale fixed across
    /// animation frames and directions).
    pub bounds: Option<(glam::Vec3, glam::Vec3)>,
}

impl RenderView {
    /// The standard preview: orbit at `yaw`, 0.3 rad above, PS1 look (or PBR).
    pub fn preview(yaw: f32, pbr: bool) -> Self {
        Self {
            yaw,
            pitch: 0.3,
            pbr,
            opts: if pbr {
                crate::raster::RasterOptions::reference()
            } else {
                crate::raster::RasterOptions::default()
            },
            background: Some(([0x20, 0x1c, 0x3a], [0x06, 0x06, 0x0c])),
            bounds: None,
        }
    }
}

/// Render `model` with the CPU rasterizer.
pub fn render_model(model: &Model, w: u32, h: u32, v: &RenderView) -> image::RgbaImage {
    use crate::raster::{self, DrawCall, Frame, OrbitCamera, PbrMaterial};
    let (lo, hi) = v.bounds.unwrap_or_else(|| model.bounds());
    let mut cam = OrbitCamera {
        yaw: v.yaw,
        pitch: v.pitch,
        ..Default::default()
    };
    cam.frame_bounds(lo, hi);
    let pbr = v.pbr;
    let normals: Vec<Vec<glam::Vec3>> = model
        .pieces
        .iter()
        .map(|p| p.vertices.iter().map(|v| v.normal).collect())
        .collect();
    type Streams = (Vec<glam::Vec3>, Vec<glam::Vec2>, Vec<[f32; 4]>);
    let data: Vec<Streams> = model
        .pieces
        .iter()
        .map(|p| {
            let mat = p.material.and_then(|m| model.materials.get(m));
            // PBR shading applies the material's base colour itself.
            let tint = if pbr || mat.is_some_and(|m| m.albedo.is_some()) {
                [1.0; 4]
            } else {
                mat.map_or([1.0; 4], |m| m.base_color)
            };
            (
                p.vertices.iter().map(|v| v.pos).collect(),
                p.vertices.iter().map(|v| v.uv).collect(),
                p.vertices
                    .iter()
                    .map(|v| {
                        [
                            v.color[0] * tint[0],
                            v.color[1] * tint[1],
                            v.color[2] * tint[2],
                            1.0,
                        ]
                    })
                    .collect(),
            )
        })
        .collect();
    let calls: Vec<DrawCall> = model
        .pieces
        .iter()
        .zip(&data)
        .zip(&normals)
        .map(|((p, d), nrm)| DrawCall {
            positions: &d.0,
            uvs: &d.1,
            colors: &d.2,
            indices: &p.indices,
            texture: p
                .material
                .and_then(|m| model.materials.get(m))
                .and_then(|m| m.albedo.as_ref()),
            id_base: 0,
            double_sided: p
                .material
                .and_then(|m| model.materials.get(m))
                .is_some_and(|m| m.double_sided),
            normals: nrm,
            pbr: p
                .material
                .and_then(|m| model.materials.get(m))
                .map(PbrMaterial::from_material),
        })
        .collect();
    let mut frame = Frame::new(w, h);
    match v.background {
        Some((top, bottom)) => frame.clear_gradient(top, bottom),
        None => frame.clear([0, 0, 0, 0]),
    }
    raster::render(
        &mut frame,
        cam.view(),
        cam.proj(w as f32 / h as f32),
        &calls,
        &v.opts,
    );
    frame.to_rgba()
}

/// Settings for `sprite_sheet` (experimental).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SpriteSheetJob {
    /// Viewing directions around the model (8 = Doom-style).
    pub directions: u32,
    /// Cell size in pixels (square).
    pub size: u32,
    /// Camera elevation in degrees.
    pub pitch_degrees: f32,
    /// Animation clip (name or index); None = one still frame.
    pub clip: Option<String>,
    /// Frames sampled evenly over the clip.
    pub frames: u32,
    /// PS1 look (jitter, dither, 15-bit) instead of clean rendering.
    pub ps1: bool,
}

impl Default for SpriteSheetJob {
    fn default() -> Self {
        Self {
            directions: 8,
            size: 64,
            pitch_degrees: 10.0,
            clip: None,
            frames: 1,
            ps1: true,
        }
    }
}

/// Layout of a sprite sheet: rows are directions (front first, then turning
/// counter-clockwise as seen from above), columns are animation frames.
#[derive(Clone, Debug, Serialize)]
pub struct SpriteSheetInfo {
    pub cell: u32,
    pub directions: u32,
    pub frames: u32,
    pub clip: Option<String>,
    pub frame_times: Vec<f32>,
    pub yaw_degrees: Vec<f32>,
    /// World units per sprite pixel (Godot `pixel_size`) and the model point at the
    /// centre of every cell, so a billboard matches the 3D model's size and position.
    pub units_per_pixel: f32,
    pub center: [f32; 3],
    /// Frames per second that play the clip at its real speed.
    pub fps: f32,
}

/// Render the model from `directions` angles x `frames` animation frames into one
/// transparent sheet: 2.5D billboard sprites (Doom-style) for PS1-era games.
pub fn sprite_sheet(
    model: &Model,
    job: &SpriteSheetJob,
) -> anyhow::Result<(image::RgbaImage, SpriteSheetInfo)> {
    let dirs = job.directions.clamp(1, 32);
    let cell = job.size.clamp(8, 512);
    let clip = match &job.clip {
        Some(name) => Some(
            model
                .clip_index(name)
                .ok_or_else(|| anyhow::anyhow!("clip '{name}' not found"))?,
        ),
        None => None,
    };
    let duration = clip
        .and_then(|c| model.rig.as_ref().map(|r| r.clips[c].duration()))
        .unwrap_or(0.0);
    let frames = if clip.is_some() {
        job.frames.clamp(1, 64)
    } else {
        1
    };
    let times: Vec<f32> = (0..frames)
        .map(|f| duration * f as f32 / frames as f32)
        .collect();
    let poses: Vec<Model> = times.iter().map(|&t| model.posed(clip, t)).collect();
    // One framing for every cell, so the character doesn't scale between frames.
    let bounds = poses.iter().fold(
        (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)),
        |(a, b), m| {
            let (lo, hi) = m.bounds();
            (a.min(lo), b.max(hi))
        },
    );
    let mut sheet = image::RgbaImage::new(cell * frames, cell * dirs);
    let mut yaws = Vec::new();
    for d in 0..dirs {
        let yaw = std::f32::consts::TAU * d as f32 / dirs as f32;
        yaws.push(yaw.to_degrees());
        for (f, m) in poses.iter().enumerate() {
            let mut opts = if job.ps1 {
                crate::raster::RasterOptions::default()
            } else {
                crate::raster::RasterOptions::clean()
            };
            opts.fog = None;
            let view = RenderView {
                yaw,
                pitch: job.pitch_degrees.to_radians(),
                pbr: false,
                opts,
                background: None,
                bounds: Some(bounds),
            };
            let img = render_model(m, cell, cell, &view);
            image::imageops::overlay(
                &mut sheet,
                &img,
                (f as u32 * cell) as i64,
                (d * cell) as i64,
            );
        }
    }
    Ok((
        sheet,
        SpriteSheetInfo {
            cell,
            directions: dirs,
            frames,
            clip: clip.and_then(|c| model.rig.as_ref().map(|r| r.clips[c].name.clone())),
            fps: if duration > 0.0 {
                frames as f32 / duration
            } else {
                1.0
            },
            frame_times: times,
            yaw_degrees: yaws,
            units_per_pixel: {
                // OrbitCamera::frame_bounds: distance = r / sin(fov/2) * 1.1; the visible
                // height at that distance is 2 * d * tan(fov/2).
                let fov = crate::raster::OrbitCamera::default().fov_y;
                let r = ((bounds.1 - bounds.0).length() * 0.5).max(1e-3);
                let d = r / (fov * 0.5).sin() * 1.1;
                2.0 * d * (fov * 0.5).tan() / cell as f32
            },
            center: ((bounds.0 + bounds.1) * 0.5).to_array(),
        },
    ))
}
