//! 3D viewport: renders a scene with the PS1 software rasterizer at a retro resolution,
//! scales it up with nearest filtering and handles orbit/pan/zoom.

use super::widgets::TexSlot;
use egui::{Color32, Pos2, Rect, Response, Sense, Ui, Vec2};
use glam::{Mat4, Vec2 as GVec2, Vec3};
use image::RgbaImage;
use psx_forge::mesh::Model;
use psx_forge::raster::{self, DrawCall, FogSettings, Frame, OrbitCamera, RasterOptions};
use serde::{Deserialize, Serialize};

pub struct RenderPiece {
    pub positions: Vec<Vec3>,
    pub uvs: Vec<GVec2>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub texture: Option<usize>,
    pub double_sided: bool,
}

#[derive(Default)]
pub struct RenderScene {
    pub pieces: Vec<RenderPiece>,
    pub textures: Vec<Option<RgbaImage>>,
    pub lo: Vec3,
    pub hi: Vec3,
    /// Bumped whenever content changes so the viewport knows to re-render.
    pub generation: u64,
}

impl RenderScene {
    pub fn from_model(m: &Model, generation: u64) -> Self {
        let pieces = m
            .pieces
            .iter()
            .map(|p| {
                let mat = p.material.and_then(|i| m.materials.get(i));
                let textured = mat.is_some_and(|m| m.albedo.is_some());
                let tint = if textured {
                    [1.0; 4]
                } else {
                    mat.map_or([1.0; 4], |m| m.base_color)
                };
                RenderPiece {
                    positions: p.vertices.iter().map(|v| v.pos).collect(),
                    uvs: p.vertices.iter().map(|v| v.uv).collect(),
                    colors: p
                        .vertices
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
                    indices: p.indices.clone(),
                    texture: if textured { p.material } else { None },
                    double_sided: false,
                }
            })
            .collect();
        let (lo, hi) = m.bounds();
        Self {
            pieces,
            textures: m.materials.iter().map(|m| m.albedo.clone()).collect(),
            lo,
            hi,
            generation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResMode {
    Lines224,
    Lines240,
    Lines480,
    Native,
}

impl ResMode {
    pub const ALL: [ResMode; 4] = [Self::Lines240, Self::Lines224, Self::Lines480, Self::Native];
    pub fn label(self) -> &'static str {
        match self {
            Self::Lines224 => "224p (NTSC safe)",
            Self::Lines240 => "240p (PS1)",
            Self::Lines480 => "480i (hi-res)",
            Self::Native => "Native",
        }
    }
    fn lines(self) -> Option<u32> {
        match self {
            Self::Lines224 => Some(224),
            Self::Lines240 => Some(240),
            Self::Lines480 => Some(480),
            Self::Native => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Controls {
    /// LMB orbit, RMB/MMB or Shift+LMB pan.
    Viewer,
    /// RMB orbit, MMB or Shift+RMB pan, LMB left to the tool.
    Editor,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Viewport {
    pub cam: OrbitCamera,
    pub opts: RasterOptions,
    pub res: ResMode,
    pub turntable: bool,
    pub fog: bool,
    pub bg_top: [u8; 3],
    pub bg_bottom: [u8; 3],
    #[serde(skip)]
    tex: TexSlot,
    #[serde(skip)]
    last_key: String,
    #[serde(skip)]
    frame_size: (u32, u32),
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            cam: OrbitCamera::default(),
            opts: RasterOptions::default(),
            res: ResMode::Lines240,
            turntable: false,
            fog: false,
            bg_top: [0x20, 0x1c, 0x3a],
            bg_bottom: [0x06, 0x06, 0x0c],
            tex: TexSlot::default(),
            last_key: String::new(),
            frame_size: (1, 1),
        }
    }
}

/// Everything callers need to map between world space and the on-screen image.
pub struct ViewInfo {
    pub response: Response,
    pub rect: Rect,
    pub view: Mat4,
    pub proj: Mat4,
    pub aspect: f32,
}

impl ViewInfo {
    pub fn project(&self, p: Vec3) -> Option<Pos2> {
        let c = self.proj * self.view * p.extend(1.0);
        if c.w <= 1e-4 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(Pos2::new(
            self.rect.min.x + (n.x * 0.5 + 0.5) * self.rect.width(),
            self.rect.min.y + (0.5 - n.y * 0.5) * self.rect.height(),
        ))
    }

    pub fn ndc(&self, p: Pos2) -> GVec2 {
        GVec2::new(
            (p.x - self.rect.min.x) / self.rect.width() * 2.0 - 1.0,
            1.0 - (p.y - self.rect.min.y) / self.rect.height() * 2.0,
        )
    }
}

impl Viewport {
    pub fn frame(&mut self, scene: &RenderScene) {
        self.cam.frame_bounds(scene.lo, scene.hi);
    }

    /// Toolbar with the PS1 effect toggles.
    pub fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt(ui.id().with("res"))
                .selected_text(self.res.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for r in ResMode::ALL {
                        ui.selectable_value(&mut self.res, r, r.label());
                    }
                });
            ui.separator();
            ui.toggle_value(&mut self.opts.vertex_snap, "Jitter")
                .on_hover_text("Integer vertex snapping (wobbly polygons)");
            ui.toggle_value(&mut self.opts.affine, "Affine")
                .on_hover_text("Affine texture mapping (no perspective correction)");
            if self.opts.affine {
                ui.add(
                    egui::DragValue::new(&mut self.opts.affine_weight)
                        .range(0.0..=1.0)
                        .speed(0.01)
                        .max_decimals(2)
                        .prefix("warp "),
                )
                .on_hover_text("Same as affine_weight in psx_lit.gdshader: 0 = perspective-correct, 1 = full PS1 warping");
            }
            ui.toggle_value(&mut self.opts.dither, "Dither")
                .on_hover_text("PS1 GPU 4x4 dither table");
            ui.toggle_value(&mut self.opts.color15, "15-bit")
                .on_hover_text("RGB555 framebuffer");
            ui.toggle_value(&mut self.opts.zbuffer, "Z-buffer")
                .on_hover_text(
                    "Off = PS1 ordering table (painter's sort) with its classic sorting glitches",
                );
            ui.toggle_value(&mut self.opts.textured, "Textures");
            ui.toggle_value(&mut self.fog, "Fog");
            ui.toggle_value(&mut self.turntable, "Turntable");
            if self.opts.vertex_snap {
                ui.add(
                    egui::DragValue::new(&mut self.opts.snap_size)
                        .range(1.0..=8.0)
                        .speed(0.05)
                        .prefix("snap ")
                        .suffix("px"),
                );
            }
        });
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        scene: Option<&RenderScene>,
        controls: Controls,
        extra: &[RenderPiece],
    ) -> ViewInfo {
        let avail = ui.available_size().max(Vec2::splat(64.0));
        let (rect, response) = ui.allocate_exact_size(avail, Sense::click_and_drag());
        self.handle_input(ui, &response, controls, rect);
        if self.turntable {
            self.cam.yaw += ui.input(|i| i.stable_dt).min(0.1) * 0.6;
            ui.ctx().request_repaint();
        }

        let ppp = ui.ctx().pixels_per_point();
        let (fw, fh) = match self.res.lines() {
            Some(lines) => (
                ((lines as f32) * rect.width() / rect.height())
                    .round()
                    .max(1.0) as u32,
                lines,
            ),
            None => (
                ((rect.width() * ppp) as u32).max(1),
                ((rect.height() * ppp) as u32).max(1),
            ),
        };
        let aspect = fw as f32 / fh as f32;
        let view = self.cam.view();
        let proj = self.cam.proj(aspect);

        let key = format!(
            "{:?}|{}|{fw}x{fh}|{}|{}|{}",
            (
                self.cam.target,
                self.cam.yaw,
                self.cam.pitch,
                self.cam.distance
            ),
            serde_json::to_string(&self.opts).unwrap_or_default(),
            scene.map_or(0, |s| s.generation),
            self.fog,
            extra.len()
        );
        if key != self.last_key || self.frame_size != (fw, fh) || self.tex.handle.is_none() {
            self.last_key = key;
            self.frame_size = (fw, fh);
            let mut frame = Frame::new(fw, fh);
            frame.clear_gradient(self.bg_top, self.bg_bottom);
            let mut opts = self.opts.clone();
            if self.fog {
                let d = self.cam.distance;
                opts.fog = Some(FogSettings {
                    color: self.bg_bottom,
                    start: d * 0.8,
                    end: d * 2.5,
                });
            }
            let mut calls: Vec<DrawCall> = Vec::new();
            if let Some(s) = scene {
                for p in s.pieces.iter().chain(extra) {
                    calls.push(draw_call(p, &s.textures));
                }
            }
            raster::render(&mut frame, view, proj, &calls, &opts);
            self.tex.set(ui.ctx(), "viewport", &frame.to_rgba());
        }
        if let Some(h) = &self.tex.handle {
            ui.painter_at(rect).image(
                h.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        ViewInfo {
            response,
            rect,
            view,
            proj,
            aspect,
        }
    }

    fn handle_input(&mut self, ui: &Ui, r: &Response, controls: Controls, rect: Rect) {
        use egui::PointerButton::*;
        let shift = ui.input(|i| i.modifiers.shift);
        let d = r.drag_delta();
        let (orbit, pan) = match controls {
            Controls::Viewer => (
                r.dragged_by(Primary) && !shift,
                r.dragged_by(Secondary) || r.dragged_by(Middle) || (r.dragged_by(Primary) && shift),
            ),
            Controls::Editor => (
                r.dragged_by(Secondary) && !shift,
                r.dragged_by(Middle) || (r.dragged_by(Secondary) && shift),
            ),
        };
        if orbit {
            self.cam.orbit(d.x, d.y);
            self.turntable = false;
        } else if pan {
            self.cam.pan(d.x, d.y, rect.height());
        }
        if r.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.cam.zoom(scroll);
            }
        }
    }
}

fn draw_call<'a>(p: &'a RenderPiece, textures: &'a [Option<RgbaImage>]) -> DrawCall<'a> {
    DrawCall {
        positions: &p.positions,
        uvs: &p.uvs,
        colors: &p.colors,
        indices: &p.indices,
        texture: p
            .texture
            .and_then(|t| textures.get(t))
            .and_then(|t| t.as_ref()),
        id_base: 0,
        double_sided: p.double_sided,
    }
}

/// Small XYZ gizmo in the corner of a viewport.
pub fn axis_gizmo(ui: &Ui, info: &ViewInfo) {
    let painter = ui.painter_at(info.rect);
    let origin = info.rect.left_bottom() + Vec2::new(36.0, -36.0);
    let rot = Mat4::from_cols(
        info.view.x_axis,
        info.view.y_axis,
        info.view.z_axis,
        glam::Vec4::W,
    );
    for (axis, color, name) in [
        (Vec3::X, Color32::from_rgb(235, 80, 90), "X"),
        (Vec3::Y, Color32::from_rgb(120, 220, 90), "Y"),
        (Vec3::Z, Color32::from_rgb(80, 140, 255), "Z"),
    ] {
        let v = rot.transform_vector3(axis);
        let end = origin + Vec2::new(v.x, -v.y) * 24.0;
        painter.line_segment([origin, end], egui::Stroke::new(2.0f32, color));
        painter.text(
            end,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::monospace(11.0),
            color,
        );
    }
}
