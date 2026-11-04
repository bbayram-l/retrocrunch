//! 3D viewport: renders a scene with the PS1 software rasterizer at a retro resolution,
//! scales it up with nearest filtering and handles orbit/pan/zoom.

use super::gpu::{DrawParams, MaterialUpload, MeshUpload, SharedGpu, TextureUpload};
use super::widgets::TexSlot;
use egui::{Color32, Pos2, Rect, Response, Sense, Ui, Vec2};
use glam::{Mat4, Vec2 as GVec2, Vec3};
use image::RgbaImage;
use psx_forge::mesh::Model;
use psx_forge::raster::{self, DrawCall, FogSettings, Frame, OrbitCamera, RasterOptions};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once per frame by the app when eframe runs on the glow (OpenGL) backend.
pub static GPU_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub struct RenderPiece {
    pub positions: Vec<Vec3>,
    pub uvs: Vec<GVec2>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub texture: Option<usize>,
    pub double_sided: bool,
    /// For the PBR reference view: normals, raw vertex colours and the material index.
    pub normals: Vec<Vec3>,
    pub vertex_colors: Vec<[f32; 4]>,
    pub material: Option<usize>,
}

#[derive(Default)]
pub struct RenderScene {
    pub pieces: Vec<RenderPiece>,
    pub textures: Vec<Option<RgbaImage>>,
    /// Full materials (metal/rough, normal, emission) for the PBR reference view.
    pub materials: Vec<psx_forge::mesh::Material>,
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
                    double_sided: mat.is_some_and(|m| m.double_sided),
                    normals: p.vertices.iter().map(|v| v.normal).collect(),
                    vertex_colors: p.vertices.iter().map(|v| v.color).collect(),
                    material: p.material,
                }
            })
            .collect();
        let (lo, hi) = m.bounds();
        Self {
            pieces,
            textures: m.materials.iter().map(|m| m.albedo.clone()).collect(),
            materials: m.materials.clone(),
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

/// What the viewport colours surfaces with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Shaded,
    /// Texture (or base colour) without vertex lighting.
    Unlit,
    /// Vertex colours only: baked lighting and AO, no texture.
    Lighting,
    /// Normals as colours (flipped or broken normals stand out).
    Normals,
    /// A checker through the UVs: texel density and stretching.
    UvChecker,
}

impl ViewMode {
    pub const ALL: [ViewMode; 5] = [
        Self::Shaded,
        Self::Unlit,
        Self::Lighting,
        Self::Normals,
        Self::UvChecker,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Shaded => "Shaded",
            Self::Unlit => "Texture only",
            Self::Lighting => "Vertex lighting",
            Self::Normals => "Normals",
            Self::UvChecker => "UV checker",
        }
    }
    fn tooltip(self) -> &'static str {
        match self {
            Self::Shaded => "Texture x vertex lighting, as exported",
            Self::Unlit => "Texture or base colour without the baked vertex lighting",
            Self::Lighting => "Only the vertex colours: baked AO, sun and shadows",
            Self::Normals => "Vertex normals as RGB (X red, Y green, Z blue)",
            Self::UvChecker => {
                "Checker through the UVs: uneven squares mean uneven texel density, skewed ones mean stretching"
            }
        }
    }
}

/// Edge colour of the wireframe overlay.
const WIRE: [u8; 3] = [96, 236, 214];

/// A copy of `s` coloured for `mode` (flat shading, no materials).
fn view_variant(s: &RenderScene, mode: ViewMode) -> RenderScene {
    let checker = (mode == ViewMode::UvChecker).then(|| {
        // 32x32 cells: fine enough to compare texel density between atlas charts.
        RgbaImage::from_fn(256, 256, |x, y| {
            let (cx, cy) = (x / 8, y / 8);
            let v = if (cx + cy) % 2 == 0 { 210.0 } else { 80.0 };
            // Red grows along U, blue along V, so orientation and mirroring show.
            let r = v * (0.55 + 0.45 * cx as f32 / 31.0);
            let b = v * (0.55 + 0.45 * cy as f32 / 31.0);
            image::Rgba([r as u8, (v * 0.8) as u8, b as u8, 255])
        })
    });
    let pieces = s
        .pieces
        .iter()
        .map(|p| {
            let n = p.positions.len();
            let base = p
                .material
                .and_then(|m| s.materials.get(m))
                .map_or([1.0; 4], |m| m.base_color);
            let (colors, texture) = match mode {
                ViewMode::Shaded => (p.colors.clone(), p.texture),
                ViewMode::Unlit => (
                    vec![if p.texture.is_some() { [1.0; 4] } else { base }; n],
                    p.texture,
                ),
                ViewMode::Lighting => (
                    if p.vertex_colors.len() == n {
                        p.vertex_colors.clone()
                    } else {
                        p.colors.clone()
                    },
                    None,
                ),
                ViewMode::Normals => (
                    (0..n)
                        .map(|i| {
                            let v = p
                                .normals
                                .get(i)
                                .copied()
                                .unwrap_or(Vec3::Y)
                                .normalize_or(Vec3::Y);
                            [v.x * 0.5 + 0.5, v.y * 0.5 + 0.5, v.z * 0.5 + 0.5, 1.0]
                        })
                        .collect(),
                    None,
                ),
                ViewMode::UvChecker => (vec![[1.0; 4]; n], (p.uvs.len() == n).then_some(0)),
            };
            RenderPiece {
                positions: p.positions.clone(),
                uvs: p.uvs.clone(),
                vertex_colors: colors.clone(),
                colors,
                indices: p.indices.clone(),
                texture,
                double_sided: p.double_sided,
                normals: p.normals.clone(),
                material: None,
            }
        })
        .collect();
    RenderScene {
        pieces,
        textures: match (mode, checker) {
            (_, Some(c)) => vec![Some(c)],
            (ViewMode::Unlit, _) => s.textures.clone(),
            _ => Vec::new(),
        },
        materials: Vec::new(),
        lo: s.lo,
        hi: s.hi,
        generation: s.generation.wrapping_mul(31) ^ ((mode as u64 + 1) << 56),
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
    /// Render with OpenGL when available (falls back to the CPU rasterizer).
    pub gpu: bool,
    /// Draw triangle edges over the model.
    pub wireframe: bool,
    pub view: ViewMode,
    #[serde(skip)]
    variant: Option<((u64, ViewMode), RenderScene)>,
    #[serde(skip)]
    gpu_state: SharedGpu,
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
            gpu: true,
            wireframe: false,
            view: ViewMode::Shaded,
            variant: None,
            gpu_state: SharedGpu::default(),
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
                .on_hover_text("Same as affine_weight in the Godot psx_*.gdshader materials: 0 = perspective-correct, 1 = full PS1 warping");
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
            ui.separator();
            ui.toggle_value(&mut self.wireframe, "Wire")
                .on_hover_text("Show triangle edges (only visible ones; they jitter with the mesh)");
            egui::ComboBox::from_id_salt(ui.id().with("view"))
                .selected_text(self.view.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for m in ViewMode::ALL {
                        ui.selectable_value(&mut self.view, m, m.label())
                            .on_hover_text(m.tooltip());
                    }
                });
            if GPU_AVAILABLE.load(Ordering::Relaxed) {
                let err = self.gpu_error();
                let r = ui.toggle_value(&mut self.gpu, "GPU");
                match err {
                    Some(e) => r.on_hover_text(format!("GPU renderer failed, using the CPU: {e}")),
                    None => r.on_hover_text(
                        "Render with OpenGL (fast at native resolution). Off = CPU rasterizer. \
                         Ordering-table mode (Z-buffer off) always uses the CPU.",
                    ),
                };
            }
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
        reference: bool,
    ) -> ViewInfo {
        let Some(s) = scene.filter(|_| self.view != ViewMode::Shaded) else {
            return self.show_scene(ui, scene, controls, extra, reference);
        };
        let key = (s.generation, self.view);
        let variant = match self.variant.take() {
            Some((k, v)) if k == key => v,
            _ => view_variant(s, self.view),
        };
        let info = self.show_scene(ui, Some(&variant), controls, extra, reference);
        self.variant = Some((key, variant));
        info
    }

    fn show_scene(
        &mut self,
        ui: &mut Ui,
        scene: Option<&RenderScene>,
        controls: Controls,
        extra: &[RenderPiece],
        reference: bool,
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

        if self.use_gpu() {
            self.paint_gpu(ui, rect, scene, extra, reference, (fw, fh), view, proj);
            return ViewInfo {
                response,
                rect,
                view,
                proj,
                aspect,
            };
        }
        let key = format!(
            "{:?}|{}|{fw}x{fh}|{}|{}|{:?}",
            (
                self.cam.target,
                self.cam.yaw,
                self.cam.pitch,
                self.cam.distance
            ),
            serde_json::to_string(&self.opts).unwrap_or_default(),
            scene.map_or(0, |s| s.generation),
            self.fog,
            (extra.len(), reference, self.wireframe)
        );
        if key != self.last_key || self.frame_size != (fw, fh) || self.tex.handle.is_none() {
            self.last_key = key;
            self.frame_size = (fw, fh);
            let mut frame = Frame::new(fw, fh);
            frame.clear_gradient(self.bg_top, self.bg_bottom);
            // Reference view: modern PBR shading, none of the PS1 effects.
            let opts = self.effective_opts(reference);
            let mut calls: Vec<DrawCall> = Vec::new();
            if let Some(s) = scene {
                for p in s.pieces.iter().chain(extra) {
                    calls.push(draw_call(p, s, reference));
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

    fn effective_opts(&self, reference: bool) -> RasterOptions {
        let mut opts = if reference {
            RasterOptions {
                textured: self.opts.textured,
                ..RasterOptions::reference()
            }
        } else {
            self.opts.clone()
        };
        opts.wireframe = self.wireframe.then_some(WIRE);
        if self.fog && !reference {
            let d = self.cam.distance;
            opts.fog = Some(FogSettings {
                color: self.bg_bottom,
                start: d * 0.8,
                end: d * 2.5,
            });
        }
        opts
    }

    fn gpu_error(&self) -> Option<String> {
        self.gpu_state.lock().ok().and_then(|g| g.error.clone())
    }

    /// GPU unless it's off, unavailable, failed, or the ordering table is needed.
    fn use_gpu(&self) -> bool {
        self.gpu
            && self.opts.zbuffer
            && GPU_AVAILABLE.load(Ordering::Relaxed)
            && self.gpu_error().is_none()
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_gpu(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        scene: Option<&RenderScene>,
        extra: &[RenderPiece],
        reference: bool,
        res: (u32, u32),
        view: Mat4,
        proj: Mat4,
    ) {
        let key = (scene.map_or(0, |s| s.generation), extra.len());
        if let Ok(mut g) = self.gpu_state.lock()
            && g.key != Some(key)
        {
            g.key = Some(key);
            let (meshes, textures) = gpu_uploads(scene, extra);
            g.queue(meshes, textures);
        }
        // The CPU frame goes stale while the GPU draws; re-render it if we switch back.
        self.tex.clear();
        let o = self.effective_opts(reference);
        let f = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);
        let params = DrawParams {
            view,
            proj,
            res,
            reference,
            snap: o.vertex_snap.then_some(o.snap_size),
            affine: if o.affine {
                o.affine_weight.clamp(0.0, 1.0)
            } else {
                0.0
            },
            dither: o.dither,
            color15: o.color15,
            textured: o.textured,
            cull_back: o.cull_back,
            fog: o.fog.map(|f| (f.color.map(|v| v as f32), f.start, f.end)),
            bg_top: f(self.bg_top),
            bg_bottom: f(self.bg_bottom),
            wireframe: o.wireframe.map(|c| c.map(|v| v as f32 / 255.0)),
        };
        let state = self.gpu_state.clone();
        let ctx = ui.ctx().clone();
        let cb = eframe::egui_glow::CallbackFn::new(move |info, painter| {
            if let Ok(mut g) = state.lock() {
                g.paint(painter.gl(), &info, painter.intermediate_fbo(), &params);
                if g.error.is_some() {
                    // Failures only show up while painting: repaint so the CPU takes over.
                    ctx.request_repaint();
                }
            }
        });
        ui.painter_at(rect).add(egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(cb),
        });
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

fn draw_call<'a>(p: &'a RenderPiece, s: &'a RenderScene, reference: bool) -> DrawCall<'a> {
    let pbr = p
        .material
        .filter(|_| reference)
        .and_then(|m| s.materials.get(m))
        .map(psx_forge::raster::PbrMaterial::from_material);
    DrawCall {
        positions: &p.positions,
        uvs: &p.uvs,
        // PBR applies the material's base colour itself: use the raw vertex colours.
        colors: if pbr.is_some() {
            &p.vertex_colors
        } else {
            &p.colors
        },
        indices: &p.indices,
        texture: p
            .texture
            .and_then(|t| s.textures.get(t))
            .and_then(|t| t.as_ref()),
        id_base: 0,
        double_sided: p.double_sided,
        normals: &p.normals,
        pbr,
    }
}

fn gpu_uploads(
    scene: Option<&RenderScene>,
    extra: &[RenderPiece],
) -> (Vec<MeshUpload>, TextureUpload) {
    let Some(s) = scene else {
        return (
            Vec::new(),
            TextureUpload {
                flat: Vec::new(),
                materials: Vec::new(),
            },
        );
    };
    let meshes = s
        .pieces
        .iter()
        .chain(extra)
        .map(|p| {
            let n = p.positions.len();
            let mut v = Vec::with_capacity(n * 16);
            for i in 0..n {
                let pos = p.positions[i];
                let uv = p.uvs.get(i).copied().unwrap_or_default();
                let col = p.colors.get(i).copied().unwrap_or([1.0; 4]);
                let nrm = p.normals.get(i).copied().unwrap_or_default();
                let raw = p.vertex_colors.get(i).copied().unwrap_or(col);
                v.extend_from_slice(&[pos.x, pos.y, pos.z, uv.x, uv.y]);
                v.extend_from_slice(&col);
                v.extend_from_slice(&[nrm.x, nrm.y, nrm.z]);
                v.extend_from_slice(&raw);
            }
            MeshUpload {
                vertices: v,
                // Never let the GPU read past the vertex buffer.
                indices: p
                    .indices
                    .chunks_exact(3)
                    .filter(|t| t.iter().all(|&i| (i as usize) < n))
                    .flatten()
                    .copied()
                    .collect(),
                texture: p.texture,
                material: p.material,
                double_sided: p.double_sided,
            }
        })
        .collect();
    let textures = TextureUpload {
        flat: s.textures.clone(),
        materials: s
            .materials
            .iter()
            .map(|m| MaterialUpload {
                base: m.base_color,
                metallic: m.metallic,
                roughness: m.roughness,
                emissive_factor: m.emissive_factor,
                albedo: m.albedo.clone(),
                metal_rough: m.metal_rough.clone(),
                normal: m.normal.clone(),
                emissive: m.emissive.clone(),
                ao: m
                    .ao
                    .as_ref()
                    .map(|g| image::DynamicImage::ImageLuma8(g.clone()).to_rgba8()),
            })
            .collect(),
    };
    (meshes, textures)
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
