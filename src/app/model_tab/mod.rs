//! Model tab: PS1-style conversion with live PS1 preview.
//!
//! State, loading, conversion and the viewport live here; `panels` holds the settings
//! side panel, `presets_ui` the preset bar and `export_ui` the export side.

mod export_ui;
mod panels;
mod presets_ui;

use super::Shared;
use super::detail_panel::DetailEditor;
use super::rig_panel::RigEditor;
use super::viewport::{Controls, RenderScene, Viewport, axis_gizmo};
use super::widgets::{
    self, ACCENT2, DIM, TexSlot, WARN, enum_combo, hint, section, slider_f32, slider_usize,
};
use egui::{RichText, Ui};
use retrocrunch::export::{self, EnginePreset, ExportSettings};
use retrocrunch::mesh::Model;
use retrocrunch::process::{
    self, ConvertSettings, Converted, PieceSettings, TextureSizing, UvMode,
};
use retrocrunch::texture::{AlphaMode, DitherMode, PaletteMode, ResizeFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

enum JobResult {
    Loaded(anyhow::Result<Model>),
    Converted(Box<Converted>),
}

struct Job {
    rx: Receiver<JobResult>,
    progress: Arc<Mutex<(String, f32)>>,
}

/// State of the "Save preset" window.
#[derive(Default)]
struct SavePresetDialog {
    name: String,
    category: String,
    description: String,
    include_export: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct ModelTab {
    pub settings: ConvertSettings,
    pub export: ExportSettings,
    pub viewport: Viewport,
    pub auto_convert: bool,
    pub last_export_dir: Option<PathBuf>,
    pub last_model: Option<PathBuf>,
    /// Name of the preset last applied (shown with "modified" when settings drift).
    pub active_preset: Option<String>,
    #[serde(skip)]
    presets: Option<Vec<retrocrunch::presets::Preset>>,
    #[serde(skip)]
    save_preset: Option<SavePresetDialog>,
    #[serde(skip)]
    show_original: bool,
    pub anim_speed: f32,
    #[serde(skip)]
    anim_clip: Option<usize>,
    #[serde(skip)]
    anim_time: f32,
    #[serde(skip)]
    anim_playing: bool,
    /// Pose currently baked into the displayed scene (see `apply_pose`).
    #[serde(skip)]
    posed: Option<String>,
    /// World matrices of the displayed pose (skeleton overlay).
    #[serde(skip)]
    pose_world: Vec<glam::Mat4>,
    /// Bone list, test posing and the weight brush.
    #[serde(skip)]
    rig: RigEditor,
    /// Face focus heat map and the detail brush.
    #[serde(skip)]
    detail: DetailEditor,
    /// Sprite sheet settings (experimental).
    pub sprites: retrocrunch::export::SpriteSheetJob,
    #[serde(skip)]
    vram_map: Option<(TexSlot, retrocrunch::vram::VramLayout)>,
    #[serde(skip)]
    rig_min_tris: usize,
    #[serde(skip)]
    source: Option<Arc<Model>>,
    #[serde(skip)]
    pieces: Vec<PieceSettings>,
    #[serde(skip)]
    undo: Vec<Vec<PieceSettings>>,
    #[serde(skip)]
    redo: Vec<Vec<PieceSettings>>,
    #[serde(skip)]
    selected: BTreeSet<usize>,
    #[serde(skip)]
    converted: Option<Arc<Converted>>,
    #[serde(skip)]
    scene_src: Option<RenderScene>,
    #[serde(skip)]
    scene_out: Option<RenderScene>,
    #[serde(skip)]
    job: Option<Job>,
    #[serde(skip)]
    last_key: String,
    #[serde(skip)]
    pending_since: Option<f64>,
    #[serde(skip)]
    tex_preview: TexSlot,
    #[serde(skip)]
    preview_material: usize,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    pub quick_export_open: bool,
    #[serde(skip)]
    quick_step: usize,
}

impl Default for ModelTab {
    fn default() -> Self {
        Self {
            settings: ConvertSettings::default(),
            export: EnginePreset::Godot.settings(),
            viewport: Viewport::default(),
            auto_convert: true,
            last_export_dir: None,
            last_model: None,
            show_original: false,
            source: None,
            pieces: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            selected: BTreeSet::new(),
            converted: None,
            scene_src: None,
            scene_out: None,
            job: None,
            last_key: String::new(),
            pending_since: None,
            tex_preview: TexSlot::default(),
            preview_material: 0,
            generation: 1,
            quick_export_open: false,
            active_preset: None,
            presets: None,
            save_preset: None,
            quick_step: 0,
            anim_speed: 1.0,
            anim_clip: None,
            anim_time: 0.0,
            anim_playing: false,
            posed: None,
            pose_world: Vec::new(),
            rig: RigEditor::default(),
            detail: DetailEditor::default(),
            sprites: Default::default(),
            vram_map: None,
            rig_min_tris: 0,
        }
    }
}

/// Formats read natively (FBX through ufbx); Blender formats are added separately.
const MODEL_EXTS: [&str; 4] = ["obj", "glb", "gltf", "fbx"];

const SIZES: [u32; 10] = [8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4];

impl ModelTab {
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn has_model(&self) -> bool {
        self.source.is_some()
    }

    pub fn open_dialog(&mut self, sh: &mut Shared) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("3D models", &MODEL_EXTS)
            .add_filter("Via Blender", retrocrunch::blender::IMPORT_EXTS)
            .pick_file()
        {
            self.load(p, sh);
        }
    }

    pub fn load(&mut self, path: PathBuf, sh: &mut Shared) {
        sh.status = format!("Loading {}...", path.display());
        self.last_model = Some(path.clone());
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(("Loading model".to_string(), 0.1)));
        std::thread::spawn(move || {
            let _ = tx.send(JobResult::Loaded(Model::load(&path)));
        });
        self.job = Some(Job { rx, progress });
    }

    fn start_convert(&mut self, sh: &Shared) {
        let Some(src) = self.source.clone() else {
            return;
        };
        let pieces = self.pieces.clone();
        let settings = self.settings.clone();
        let palette = sh.palette.as_ref().map(|p| p.colors.clone());
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(("Starting".to_string(), 0.0)));
        let prog = progress.clone();
        std::thread::spawn(move || {
            let report = move |m: &str, f: f32| {
                if let Ok(mut p) = prog.lock() {
                    *p = (m.to_string(), f);
                }
            };
            let c = process::convert(&src, &pieces, &settings, palette.as_deref(), &report);
            let _ = tx.send(JobResult::Converted(Box::new(c)));
        });
        self.job = Some(Job { rx, progress });
    }

    fn settings_key(&self, sh: &Shared) -> String {
        format!(
            "{}|{}|{}|w{}|d{}",
            serde_json::to_string(&self.settings).unwrap_or_default(),
            serde_json::to_string(&self.pieces).unwrap_or_default(),
            sh.palette
                .as_ref()
                .map_or(String::new(), |p| p.name.clone()),
            self.rig.revision,
            self.detail.revision
        )
    }

    /// Poll background jobs and schedule auto conversion. Call once per frame.
    pub fn update(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        if let Some(job) = &self.job {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            if let Ok(r) = job.rx.try_recv() {
                self.job = None;
                match r {
                    JobResult::Loaded(Ok(m)) => {
                        sh.status = format!(
                            "Loaded {}: {} triangles, {} pieces, {} materials",
                            m.name,
                            m.tri_count(),
                            m.pieces.len(),
                            m.materials.len()
                        );
                        self.pieces = PieceSettings::defaults_for(&m);
                        self.rig_min_tris = if m.is_rigged() {
                            retrocrunch::decimate::recommended_min_tris(&m.pieces)
                        } else {
                            0
                        };
                        // Start a rigged model on its first clip, playing.
                        self.anim_clip = m
                            .rig
                            .as_ref()
                            .filter(|r| !r.clips.is_empty() && m.has_animation())
                            .map(|_| 0);
                        self.anim_playing = self.anim_clip.is_some();
                        self.anim_time = 0.0;
                        self.posed = None;
                        self.rig.reset();
                        self.detail.reset();
                        self.undo.clear();
                        self.redo.clear();
                        self.selected.clear();
                        self.generation += 1;
                        let scene = RenderScene::from_model(&m, self.generation);
                        self.viewport.frame(&scene);
                        self.scene_src = Some(scene);
                        self.source = Some(Arc::new(m));
                        self.converted = None;
                        self.scene_out = None;
                        self.last_key.clear();
                        self.pending_since = Some(0.0);
                    }
                    JobResult::Loaded(Err(e)) => {
                        sh.error = Some(format!("Could not load model: {e:#}"))
                    }
                    JobResult::Converted(c) => {
                        self.generation += 1;
                        sh.status = format!(
                            "Converted: {} -> {} triangles ({} hidden removed), {} vertices",
                            c.stats.tris_before,
                            c.stats.tris_after,
                            c.stats.hidden_removed,
                            c.stats.verts_after
                        );
                        self.scene_out = Some(RenderScene::from_model(&c.model, self.generation));
                        self.preview_material = c
                            .model
                            .materials
                            .iter()
                            .position(|m| m.albedo.is_some())
                            .unwrap_or(0);
                        if let Some(img) = c
                            .model
                            .materials
                            .get(self.preview_material)
                            .and_then(|m| m.albedo.as_ref())
                        {
                            self.tex_preview.set(ctx, "model_tex", img);
                        } else {
                            self.tex_preview.clear();
                        }
                        self.converted = Some(Arc::new(*c));
                        self.posed = None;
                    }
                }
            }
            return;
        }
        if self.source.is_none() {
            return;
        }
        let key = self.settings_key(sh);
        let now = ctx.input(|i| i.time);
        if key != self.last_key {
            self.last_key = key;
            self.pending_since = Some(now);
        }
        if let Some(t) = self.pending_since
            && (self.auto_convert || self.converted.is_none())
        {
            if now - t > 0.35 {
                self.pending_since = None;
                self.start_convert(sh);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.pieces.clone());
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.pieces, p));
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.pieces, p));
        }
    }

    /// Unload the current asset (settings and presets stay).
    pub fn clear_asset(&mut self, sh: &mut Shared) {
        let keep = (
            std::mem::take(&mut self.settings),
            std::mem::take(&mut self.export),
            std::mem::take(&mut self.viewport),
            self.auto_convert,
            self.active_preset.take(),
            self.last_export_dir.take(),
            self.anim_speed,
        );
        *self = ModelTab::default();
        (
            self.settings,
            self.export,
            self.viewport,
            self.auto_convert,
            self.active_preset,
            self.last_export_dir,
            self.anim_speed,
        ) = keep;
        sh.status = "Cleared the model.".into();
    }

    /// Back to the built-in default settings (the loaded model stays).
    pub fn revert_to_defaults(&mut self, sh: &mut Shared) {
        self.settings = ConvertSettings::default();
        self.export = EnginePreset::Godot.settings();
        self.active_preset = None;
        sh.status = "Settings reverted to defaults.".into();
    }

    /// Clip picker, play/pause, scrubber and speed for rigged models.
    fn animation_bar(&mut self, ui: &mut Ui) {
        let Some(rig) = self
            .source
            .as_ref()
            .filter(|m| m.has_animation())
            .and_then(|m| m.rig.clone())
        else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Animation").color(ACCENT2));
            let name = |c: Option<usize>| {
                c.and_then(|c| rig.clips.get(c))
                    .map_or("Bind pose".to_string(), |c| c.name.clone())
            };
            egui::ComboBox::from_id_salt("clip")
                .selected_text(name(self.anim_clip))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.anim_clip, None, "Bind pose")
                        .changed()
                    {
                        self.anim_playing = false;
                    }
                    for i in 0..rig.clips.len() {
                        if ui
                            .selectable_value(&mut self.anim_clip, Some(i), name(Some(i)))
                            .changed()
                        {
                            self.anim_time = 0.0;
                            self.anim_playing = true;
                        }
                    }
                });
            let Some(clip) = self.anim_clip.and_then(|c| rig.clips.get(c)) else {
                hint(
                    ui,
                    &format!("{} joints · {} clips", rig.joint_count(), rig.clips.len()),
                );
                return;
            };
            let dur = clip.duration().max(1e-3);
            if ui
                .button(if self.anim_playing { "⏸" } else { "▶" })
                .clicked()
            {
                self.anim_playing = !self.anim_playing;
            }
            let r = ui.add(
                egui::Slider::new(&mut self.anim_time, 0.0..=dur)
                    .suffix(" s")
                    .max_decimals(2),
            );
            if r.dragged() {
                self.anim_playing = false;
            }
            ui.add(
                egui::DragValue::new(&mut self.anim_speed)
                    .range(0.05..=4.0)
                    .speed(0.01)
                    .suffix("x"),
            );
            hint(ui, &format!("{} joints", rig.joint_count()));
        });
        if self.anim_playing
            && let Some(clip) = self.anim_clip.and_then(|c| rig.clips.get(c))
        {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.anim_time = (self.anim_time + dt * self.anim_speed) % clip.duration().max(1e-3);
            ui.ctx().request_repaint();
        }
    }

    /// Skin the displayed scene into the current pose (CPU linear blend skinning).
    fn apply_pose(&mut self) {
        let original = self.show_original
            || self.scene_out.is_none()
            || self.rig.wants_source()
            || self.detail.wants_source();
        let key = format!(
            "{original}|{:?}|{}|{}",
            self.anim_clip,
            self.anim_time,
            self.rig.pose_key()
        );
        if self.posed.as_deref() == Some(key.as_str()) {
            return;
        }
        let (model, scene) = if original {
            (self.source.as_deref(), self.scene_src.as_mut())
        } else {
            (
                self.converted.as_ref().map(|c| &c.model),
                self.scene_out.as_mut(),
            )
        };
        let (Some(model), Some(scene)) = (model, scene) else {
            return;
        };
        let Some(rig) = model.rig.as_ref().filter(|_| model.has_animation()) else {
            self.pose_world.clear();
            return;
        };
        let (clip, t) = (self.anim_clip, self.anim_time);
        let mut pose = rig.pose(clip, t);
        self.rig.apply_manual(&mut pose);
        let world = rig.world(&pose);
        let posed = clip.is_some() || !self.rig.pose.is_empty();
        let mats = rig.skin_matrices(&world);
        for (rp, p) in scene.pieces.iter_mut().zip(&model.pieces) {
            let skin = p.skin.and_then(|s| mats.get(s)).filter(|_| posed);
            // Skinned pieces are rewritten even in bind pose, so leaving a pose resets them.
            if (p.skin.is_none() && !p.has_morphs()) || rp.positions.len() != p.vertices.len() {
                continue;
            }
            // Morphs first (in bind space), then skinning, like a GPU would.
            let weights = p
                .morph_group
                .and_then(|g| rig.morph_weights(clip, t, g))
                .unwrap_or_else(|| p.morph_weights.clone());
            let base = p.morphed_positions(&weights);
            for ((out, v), b) in rp.positions.iter_mut().zip(&p.vertices).zip(base) {
                *out = match skin {
                    Some(m) => retrocrunch::rig::skin_point(b, v.joints, v.weights, m),
                    None => b,
                };
            }
        }
        self.generation += 1;
        scene.generation = self.generation;
        self.pose_world = world;
        self.posed = Some(key);
    }

    pub fn central(&mut self, ui: &mut Ui, sh: &mut Shared) {
        if self.source.is_none() && self.job.is_none() {
            if widgets::empty_state(
                ui,
                "Drop a 3D model here",
                "OBJ / GLB / glTF · high-poly assets and photoscans welcome",
            ) {
                self.open_dialog(sh);
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.show_original, true, "Original");
            ui.selectable_value(&mut self.show_original, false, "PSX");
            ui.separator();
            if ui
                .button("Frame")
                .on_hover_text("Fit the model in view")
                .clicked()
                && let Some(s) = &self.scene_src
            {
                self.viewport.frame(s);
            }
            ui.checkbox(&mut self.auto_convert, "Auto convert");
            if !self.auto_convert
                && ui
                    .add_enabled(self.job.is_none(), egui::Button::new("Convert"))
                    .clicked()
            {
                self.start_convert(sh);
            }
        });
        self.viewport.toolbar(ui);
        self.animation_bar(ui);
        self.apply_pose();
        if let Some(job) = &self.job {
            let (m, f) = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
            ui.add(egui::ProgressBar::new(f).text(m).animate(true));
        }
        let tex_h = if self.tex_preview.handle.is_some() {
            150.0
        } else {
            0.0
        };
        let avail = ui.available_size();
        // The detail heat map wins over the weight heat map when both are on.
        let detail_on = self.detail.wants_source() && self.source.is_some();
        let weights_on = !detail_on
            && self.rig.wants_source()
            && self.source.as_ref().is_some_and(|m| m.is_rigged());
        let reference = self.show_original || self.scene_out.is_none() || weights_on || detail_on;
        let controls = if (weights_on && self.rig.paint) || (detail_on && self.detail.paint) {
            Controls::Editor
        } else {
            Controls::Viewer
        };
        let focus_face = self.settings.focus_face;
        let scene = match (&self.source, &self.scene_src) {
            (Some(m), Some(s)) if detail_on => Some(self.detail.heat_scene(m, s, focus_face)),
            (Some(m), Some(s)) if weights_on => Some(self.rig.weight_scene(m, s)),
            _ if reference => self.scene_src.as_ref(),
            _ => self.scene_out.as_ref(),
        };
        let info = ui
            .allocate_ui(egui::vec2(avail.x, (avail.y - tex_h).max(100.0)), |ui| {
                self.viewport.show(ui, scene, controls, &[], reference)
            })
            .inner;
        // Skeleton overlay on whichever model is displayed, then the weight brush.
        let shown = if reference {
            self.source.as_deref()
        } else {
            self.converted.as_ref().map(|c| &c.model)
        };
        if let Some(m) = shown {
            self.rig.overlay(ui, &info, m, &self.pose_world);
        }
        if weights_on
            && let (Some(src), Some(scene)) = (self.source.as_mut(), self.scene_src.as_ref())
        {
            self.rig.paint_input(ui, &info, scene, src);
        }
        if detail_on
            && let (Some(src), Some(scene)) = (self.source.as_mut(), self.scene_src.as_ref())
        {
            self.detail.paint_input(ui, &info, scene, src);
        }
        if detail_on {
            ui.painter_at(info.rect).text(
                info.rect.right_top() + egui::vec2(-8.0, 26.0),
                egui::Align2::RIGHT_TOP,
                if self.detail.paint {
                    "Detail paint: LMB paint · RMB orbit · MMB pan"
                } else {
                    "Detail focus: red keeps the most triangles and texels"
                },
                egui::FontId::proportional(12.0),
                ACCENT2,
            );
        } else if weights_on {
            ui.painter_at(info.rect).text(
                info.rect.right_top() + egui::vec2(-8.0, 26.0),
                egui::Align2::RIGHT_TOP,
                if self.rig.paint {
                    "Weight paint: LMB paint · RMB orbit · MMB pan · Ctrl+click joint"
                } else {
                    "Weights of the selected bone (blue 0 -> red 1)"
                },
                egui::FontId::proportional(12.0),
                ACCENT2,
            );
        } else if reference {
            ui.painter_at(info.rect).text(
                info.rect.right_top() + egui::vec2(-8.0, 8.0),
                egui::Align2::RIGHT_TOP,
                "Original: PBR reference (PS1 effects apply to the PSX view)",
                egui::FontId::proportional(12.0),
                DIM,
            );
        }
        axis_gizmo(ui, &info);
        if let Some(c) = &self.converted {
            let painter = ui.painter_at(info.rect);
            let lines = [
                format!("{} tris  {} verts", c.stats.tris_after, c.stats.verts_after),
                c.stats
                    .textures
                    .first()
                    .map(|t| format!("{}x{}  {} colours", t.1, t.2, t.3))
                    .unwrap_or_default(),
                // Experimental: what the textures would cost on a real PS1.
                if c.stats.vram.textures.is_empty() {
                    String::new()
                } else {
                    c.stats.vram.summary()
                },
            ];
            for (k, l) in lines.iter().enumerate() {
                painter.text(
                    info.rect.left_top() + egui::vec2(8.0, 8.0 + k as f32 * 16.0),
                    egui::Align2::LEFT_TOP,
                    l,
                    egui::FontId::monospace(13.0),
                    ACCENT2,
                );
            }
            for (k, w) in c
                .stats
                .warnings
                .iter()
                .chain(&c.stats.vram.warnings)
                .enumerate()
            {
                painter.text(
                    info.rect.left_top() + egui::vec2(8.0, 60.0 + k as f32 * 16.0),
                    egui::Align2::LEFT_TOP,
                    w,
                    egui::FontId::proportional(12.0),
                    WARN,
                );
            }
        }
        if let (Some(h), Some(c)) = (&self.tex_preview.handle, &self.converted) {
            ui.horizontal(|ui| {
                let size = h.size_vec2();
                let scale = (tex_h - 10.0) / size.y.max(1.0);
                ui.add(egui::Image::new((h.id(), size * scale)));
                ui.vertical(|ui| {
                    if let Some(pal) = c.palettes.get(self.preview_material) {
                        ui.label(
                            RichText::new(format!("Palette: {} colours", pal.len())).color(DIM),
                        );
                        widgets::swatches(ui, pal, 14.0);
                    }
                });
            });
        }
    }
}
