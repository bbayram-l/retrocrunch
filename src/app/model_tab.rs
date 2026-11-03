//! Model tab: PS1-style conversion with live PS1 preview.

use super::Shared;
use super::viewport::{Controls, RenderScene, Viewport, axis_gizmo};
use super::widgets::{
    self, ACCENT2, DIM, TexSlot, WARN, enum_combo, hint, section, slider_f32, slider_usize,
};
use egui::{RichText, Ui};
use psx_forge::export::{self, EnginePreset, ExportSettings};
use psx_forge::mesh::Model;
use psx_forge::process::{self, ConvertSettings, Converted, PieceSettings, TextureSizing, UvMode};
use psx_forge::texture::{AlphaMode, DitherMode, PaletteMode, ResizeFilter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

enum JobResult {
    Loaded(anyhow::Result<Model>),
    Converted(Converted),
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
    presets: Option<Vec<psx_forge::presets::Preset>>,
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
    /// Pose currently baked into the displayed scene: (scene is original?, clip, time).
    #[serde(skip)]
    posed: Option<(bool, Option<usize>, f32)>,
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
            .add_filter("Via Blender", psx_forge::blender::IMPORT_EXTS)
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
            let _ = tx.send(JobResult::Converted(c));
        });
        self.job = Some(Job { rx, progress });
    }

    fn settings_key(&self, sh: &Shared) -> String {
        format!(
            "{}|{}|{}",
            serde_json::to_string(&self.settings).unwrap_or_default(),
            serde_json::to_string(&self.pieces).unwrap_or_default(),
            sh.palette
                .as_ref()
                .map_or(String::new(), |p| p.name.clone())
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
                            psx_forge::decimate::recommended_min_tris(&m.pieces)
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
                        self.converted = Some(Arc::new(c));
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

    fn all_presets(&mut self, sh: &mut Shared) -> &Vec<psx_forge::presets::Preset> {
        self.presets.get_or_insert_with(|| {
            let (user, errors) = psx_forge::presets::load_user();
            if !errors.is_empty() {
                sh.error = Some(format!(
                    "Some presets could not be read:\n{}",
                    errors.join("\n")
                ));
            }
            psx_forge::presets::builtins()
                .into_iter()
                .chain(user)
                .collect()
        })
    }

    fn apply_preset(&mut self, p: &psx_forge::presets::Preset, sh: &mut Shared) {
        self.settings = p.convert.clone();
        if let Some(e) = &p.export {
            self.export = e.clone();
        }
        self.active_preset = Some(p.name.clone());
        sh.status = format!("Preset '{}' applied.", p.name);
    }

    fn preset_bar(&mut self, ui: &mut Ui, sh: &mut Shared) {
        let presets = self.all_presets(sh).clone();
        let active = self.active_preset.clone();
        let current = active
            .as_ref()
            .and_then(|n| presets.iter().find(|p| &p.name == n));
        let modified = current.is_some_and(|p| {
            serde_json::to_string(&p.convert).ok() != serde_json::to_string(&self.settings).ok()
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preset").color(ACCENT2));
            let label = match (&active, modified) {
                (Some(n), true) => format!("{n} (modified)"),
                (Some(n), false) => n.clone(),
                (None, _) => "Custom".to_string(),
            };
            let mut picked = None;
            egui::ComboBox::from_id_salt("preset")
                .selected_text(label)
                .width(190.0)
                .show_ui(ui, |ui| {
                    let mut last = String::new();
                    for p in &presets {
                        let cat = if p.builtin {
                            p.category.clone()
                        } else {
                            "My presets".to_string()
                        };
                        if cat != last {
                            ui.label(RichText::new(&cat).small().color(DIM));
                            last = cat;
                        }
                        let resp = ui
                            .selectable_label(active.as_deref() == Some(p.name.as_str()), &p.name);
                        let resp = if p.description.is_empty() {
                            resp
                        } else {
                            resp.on_hover_text(&p.description)
                        };
                        if resp.clicked() {
                            picked = Some(p.clone());
                        }
                    }
                });
            if let Some(p) = picked {
                self.apply_preset(&p, sh);
            }
            ui.menu_button("...", |ui| {
                if ui.button("Save as preset...").clicked() {
                    ui.close_menu();
                    self.save_preset = Some(SavePresetDialog {
                        name: current
                            .filter(|p| !p.builtin)
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                        category: "My presets".into(),
                        ..Default::default()
                    });
                }
                if ui.button("Import preset file...").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Preset", &["json"])
                        .pick_file()
                    {
                        match psx_forge::presets::read(&path)
                            .and_then(|p| psx_forge::presets::save_user(&p).map(|_| p))
                        {
                            Ok(p) => {
                                self.presets = None;
                                self.apply_preset(&p, sh);
                            }
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                if ui.button("Export current settings...").clicked() {
                    ui.close_menu();
                    let name = active.clone().unwrap_or_else(|| "My preset".into());
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Preset", &["json"])
                        .set_file_name(format!("{}.json", export::sanitize(&name)))
                        .save_file()
                    {
                        let p = psx_forge::presets::Preset {
                            name,
                            category: "My presets".into(),
                            description: String::new(),
                            convert: self.settings.clone(),
                            export: Some(self.export.clone()),
                            builtin: false,
                            path: None,
                        };
                        match psx_forge::presets::write(&path, &p) {
                            Ok(()) => sh.status = format!("Preset written to {}", path.display()),
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                let deletable = current.filter(|p| !p.builtin).cloned();
                if ui
                    .add_enabled(deletable.is_some(), egui::Button::new("Delete this preset"))
                    .clicked()
                {
                    ui.close_menu();
                    if let Some(p) = deletable {
                        match psx_forge::presets::delete_user(&p) {
                            Ok(()) => {
                                self.presets = None;
                                self.active_preset = None;
                                sh.status = format!("Deleted preset '{}'.", p.name);
                            }
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                if ui.button("Open presets folder").clicked() {
                    ui.close_menu();
                    let dir = psx_forge::presets::user_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let opener = if cfg!(windows) {
                        "explorer"
                    } else if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    };
                    let _ = std::process::Command::new(opener).arg(&dir).spawn();
                }
                if ui.button("Reload presets").clicked() {
                    ui.close_menu();
                    self.presets = None;
                }
            });
        });
        if let Some(desc) = current
            .map(|p| p.description.clone())
            .filter(|d| !d.is_empty())
        {
            hint(ui, &desc);
        }
    }

    /// "Save preset" window; call once per frame.
    pub fn preset_window(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        let Some(d) = &mut self.save_preset else {
            return;
        };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Save preset")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Category");
                    ui.text_edit_singleline(&mut d.category);
                });
                ui.label("Description");
                ui.text_edit_multiline(&mut d.description);
                ui.checkbox(
                    &mut d.include_export,
                    "Include export settings (engine, formats, scale)",
                );
                hint(
                    ui,
                    &format!("Saved to {}", psx_forge::presets::user_dir().display()),
                );
                save = ui
                    .add_enabled(
                        !d.name.trim().is_empty(),
                        egui::Button::new(RichText::new("Save").strong()),
                    )
                    .clicked();
            });
        if save {
            let p = psx_forge::presets::Preset {
                name: d.name.trim().to_string(),
                category: d.category.trim().to_string(),
                description: d.description.trim().to_string(),
                convert: self.settings.clone(),
                export: d.include_export.then(|| self.export.clone()),
                builtin: false,
                path: None,
            };
            match psx_forge::presets::save_user(&p) {
                Ok(path) => {
                    sh.status = format!("Saved preset '{}' to {}", p.name, path.display());
                    self.active_preset = Some(p.name);
                    self.presets = None;
                    self.save_preset = None;
                }
                Err(e) => sh.error = Some(format!("{e:#}")),
            }
        } else if !open {
            self.save_preset = None;
        }
    }

    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open model...").clicked() {
                self.open_dialog(sh);
            }
            if ui
                .add_enabled(
                    self.source.is_some() && self.job.is_none(),
                    egui::Button::new("Clear"),
                )
                .on_hover_text("Unload the model (settings are kept)")
                .clicked()
            {
                self.clear_asset(sh);
            }
            if self.converted.is_some()
                && ui
                    .button("Quick Export")
                    .on_hover_text("Ctrl+Shift+E")
                    .clicked()
            {
                self.quick_export_open = true;
                self.quick_step = 0;
            }
        });
        if let Some(m) = &self.source {
            ui.label(RichText::new(&m.name).strong());
            hint(
                ui,
                &format!(
                    "{} tris · {} verts · {} pieces · {} materials",
                    m.tri_count(),
                    m.vertex_count(),
                    m.pieces.len(),
                    m.materials.len()
                ),
            );
        } else {
            hint(
                ui,
                "OBJ, GLB/glTF, FBX or .blend. Drag a file onto the window.",
            );
        }
        ui.separator();
        self.preset_bar(ui, sh);
        if ui
            .button("Revert to defaults")
            .on_hover_text("Reset every conversion and export setting to the built-in defaults")
            .clicked()
        {
            self.revert_to_defaults(sh);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.pieces_section(ui);
                self.geometry_section(ui);
                self.lighting_section(ui);
                self.texture_section(ui, sh);
                self.export_section(ui, sh);
            });
    }

    fn pieces_section(&mut self, ui: &mut Ui) {
        let Some(model) = self.source.clone() else {
            return;
        };
        section(
            ui,
            &format!("Pieces ({})", model.pieces.len()),
            true,
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.redo();
                    }
                    let sel = self.selected.len();
                    if ui
                        .add_enabled(sel >= 2, egui::Button::new("Merge"))
                        .on_hover_text("Merge selected pieces that share a material")
                        .clicked()
                    {
                        self.push_undo();
                        let target = *self.selected.iter().next().unwrap_or(&0);
                        let g = self.pieces[target].group;
                        let mat = model.pieces[target].material;
                        for &i in &self.selected {
                            if model.pieces[i].material == mat {
                                self.pieces[i].group = g;
                            }
                        }
                    }
                    if ui
                        .add_enabled(sel >= 1, egui::Button::new("Delete"))
                        .on_hover_text(
                            "Exclude selected pieces from the conversion (source is untouched)",
                        )
                        .clicked()
                    {
                        self.push_undo();
                        for &i in &self.selected {
                            self.pieces[i].enabled = false;
                        }
                    }
                    if ui
                        .add_enabled(sel >= 1, egui::Button::new("Bake"))
                        .on_hover_text("Bake into base texture: remove the selected pieces from the geometry but paint them into the texture underneath (straps, pockets, decals). Uses the UV atlas.")
                        .clicked()
                    {
                        self.push_undo();
                        for &i in &self.selected {
                            self.pieces[i].baked = true;
                            self.pieces[i].enabled = true;
                        }
                    }
                    if ui.button("Reset").clicked() {
                        self.push_undo();
                        self.pieces = PieceSettings::defaults_for(&model);
                    }
                });
                hint(
                    ui,
                    "Click to select, Ctrl+click for several. Weight = share of the triangle budget.",
                );
                let ctrl = ui.input(|i| i.modifiers.command);
                egui::ScrollArea::vertical()
                    .id_salt("pieces")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (i, p) in model.pieces.iter().enumerate() {
                            let mut snapshot = None;
                            ui.horizontal(|ui| {
                                let s = &mut self.pieces[i];
                                let before = s.clone();
                                ui.checkbox(&mut s.enabled, "");
                                let merged =
                                    model.pieces.iter().enumerate().any(|(j, _)| {
                                        j != i && self.pieces_group(j) == before.group
                                    });
                                let name = if merged {
                                    format!("{} ⛓", p.name)
                                } else {
                                    p.name.clone()
                                };
                                let sel = self.selected.contains(&i);
                                let text = RichText::new(name).color(if before.baked {
                                    ACCENT2
                                } else if before.enabled {
                                    ui.visuals().text_color()
                                } else {
                                    DIM
                                });
                                if ui
                                    .selectable_label(sel, text)
                                    .on_hover_text(format!("{} triangles", p.tri_count()))
                                    .clicked()
                                {
                                    if !ctrl {
                                        self.selected.clear();
                                    }
                                    if sel && ctrl {
                                        self.selected.remove(&i);
                                    } else {
                                        self.selected.insert(i);
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let s = &mut self.pieces[i];
                                        if s.baked {
                                            if ui
                                                .small_button("Restore")
                                                .on_hover_text("Bring the piece back as geometry")
                                                .clicked()
                                            {
                                                s.baked = false;
                                            }
                                            ui.label(RichText::new("baked").small().color(ACCENT2));
                                            return;
                                        }
                                        ui.add(
                                            egui::DragValue::new(&mut s.weight)
                                                .range(0.0..=10.0)
                                                .speed(0.02)
                                                .prefix("w "),
                                        );
                                        ui.label(
                                            RichText::new(format!("{}", p.tri_count()))
                                                .small()
                                                .color(DIM),
                                        );
                                    },
                                );
                                if self.pieces[i] != before {
                                    snapshot = Some(before);
                                }
                            });
                            if let Some(before) = snapshot {
                                let mut prev = self.pieces.clone();
                                prev[i] = before;
                                self.undo.push(prev);
                                self.redo.clear();
                            }
                        }
                    });
            },
        );
    }

    fn pieces_group(&self, i: usize) -> usize {
        self.pieces.get(i).map_or(usize::MAX, |p| p.group)
    }

    fn geometry_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        section(ui, "Geometry", true, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.use_percent, false, "Triangles");
                ui.selectable_value(&mut s.use_percent, true, "Percent");
            });
            if s.use_percent {
                slider_f32(
                    ui,
                    "% of source",
                    &mut s.percent,
                    0.1..=100.0,
                    10.0,
                    "Target as a percentage of the source triangle count",
                );
            } else {
                slider_usize(
                    ui,
                    "triangles",
                    &mut s.target_tris,
                    12..=20000,
                    600,
                    true,
                    "Total triangle budget, shared between pieces by area x weight",
                );
            }
            hint(ui, "PS1 reference: props 100-500, characters 300-1500.");
            ui.checkbox(&mut s.lock_borders, "Lock open borders")
                .on_hover_text(
                    "Keep mesh boundaries (silhouettes of open meshes) intact; reduces less",
                );
            slider_f32(
                ui,
                "UV seam care",
                &mut s.uv_weight,
                0.0..=4.0,
                1.0,
                "How much UV distortion counts against a collapse",
            );
            ui.checkbox(&mut s.remove_hidden, "Remove faces nobody will see")
                .on_hover_text(
                    "Renders the model from 96 directions and deletes faces that never show up",
                );
            if s.remove_hidden {
                ui.indent("hb", |ui| {
                    ui.checkbox(
                        &mut s.hidden_ignore_below,
                        "Model sits on the ground (ignore views from below)",
                    );
                });
            }
            ui.horizontal(|ui| {
                ui.label("Vertex grid");
                ui.add(
                    egui::DragValue::new(&mut s.geometry_snap)
                        .range(0.0..=1.0)
                        .speed(0.0005)
                        .max_decimals(4),
                );
                hint(ui, "world units, 0 = off");
            });
            ui.horizontal(|ui| {
                ui.label("Split polygons over");
                ui.add(
                    egui::DragValue::new(&mut s.subdivide_max_edge)
                        .range(0.0..=1000.0)
                        .speed(0.005)
                        .max_decimals(3),
                )
                .on_hover_text("Edges longer than this (world units) are split, crack-free, so affine texture warping stays small. PS1 games did this for floors and walls. 0 = off.");
                if let Some(m) = &self.source {
                    let (lo, hi) = m.bounds();
                    hint(ui, &format!("model is {:.2} across", (hi - lo).length()));
                }
            });
            if self.rig_min_tris > 0 {
                let budget = if s.use_percent {
                    self.source
                        .as_ref()
                        .map_or(0, |m| (m.tri_count() as f32 * s.percent / 100.0) as usize)
                } else {
                    s.target_tris
                };
                ui.horizontal(|ui| {
                    let text = format!("Rig minimum: ~{} triangles", self.rig_min_tris);
                    if budget < self.rig_min_tris {
                        ui.label(RichText::new(format!("⚠ {text}")).color(WARN))
                            .on_hover_text("Below this, joints may not have enough edge loops to bend cleanly. You can go lower.");
                    } else {
                        hint(ui, &text);
                    }
                    if ui.small_button("Use").clicked() {
                        s.use_percent = false;
                        s.target_tris = self.rig_min_tris;
                    }
                });
            }
        });
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
        let original = self.show_original || self.scene_out.is_none();
        let key = (original, self.anim_clip, self.anim_time);
        if self.posed == Some(key) {
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
            return;
        };
        let (clip, t) = (self.anim_clip, self.anim_time);
        let mats = rig.skin_matrices(&rig.world(&rig.pose(clip, t)));
        for (rp, p) in scene.pieces.iter_mut().zip(&model.pieces) {
            let skin = p.skin.and_then(|s| mats.get(s)).filter(|_| clip.is_some());
            if (skin.is_none() && !p.has_morphs()) || rp.positions.len() != p.vertices.len() {
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
                    Some(m) => psx_forge::rig::skin_point(b, v.joints, v.weights, m),
                    None => b,
                };
            }
        }
        self.generation += 1;
        scene.generation = self.generation;
        self.posed = Some(key);
    }

    fn lighting_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        section(ui, "Lighting (vertex colours)", false, |ui| {
            ui.checkbox(&mut s.vertex_ao, "Ray-traced ambient occlusion");
            if s.vertex_ao {
                slider_f32(ui, "AO strength", &mut s.ao_strength, 0.0..=1.0, 0.7, "");
            }
            ui.checkbox(&mut s.sun, "Directional light with shadows");
            if s.sun {
                slider_f32(
                    ui,
                    "sun yaw",
                    &mut s.sun_yaw,
                    -std::f32::consts::PI..=std::f32::consts::PI,
                    0.8,
                    "",
                );
                slider_f32(ui, "sun pitch", &mut s.sun_pitch, 0.0..=1.55, 0.9, "");
                slider_f32(ui, "sun strength", &mut s.sun_strength, 0.0..=1.5, 0.6, "");
                slider_f32(ui, "ambient", &mut s.ambient, 0.0..=1.0, 0.55, "");
            }
            ui.checkbox(&mut s.bake_shading_into_texture, "Bake lighting into the texture").on_hover_text("Paint the shading into the albedo instead of vertex colours (needs non-overlapping UVs)");
        });
    }

    fn texture_section(&mut self, ui: &mut Ui, sh: &mut Shared) {
        let s = &mut self.settings;
        section(ui, "Texture", true, |ui| {
            ui.horizontal(|ui| {
                ui.label("Quality");
                ui.selectable_value(&mut s.hd_texture, false, "PSX")
                    .on_hover_text("Crunch to PS1 specs: small texture, CLUT palette, dither, 15-bit colour.");
                ui.selectable_value(&mut s.hd_texture, true, "HD")
                    .on_hover_text("Keep the source's texel density and full colour on the low-poly model. Still pixel-sharp (nearest filtering), just high resolution. Size, palette, dither and 15-bit are skipped.");
            });
            if s.hd_texture {
                hint(ui, "HD: source resolution, full colour, nearest filtering.");
                if let Some(c) = &self.converted {
                    for (name, w, h, _) in &c.stats.textures {
                        hint(ui, &format!("{name}: {w}x{h}"));
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.sizing, TextureSizing::Fixed, "Fixed size");
                    ui.selectable_value(&mut s.sizing, TextureSizing::PixelSize, "Pixel size");
                });
                match s.sizing {
                    TextureSizing::Fixed => {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("texsize")
                                .selected_text(format!("{} px", s.texture.max_size))
                                .show_ui(ui, |ui| {
                                    let mut sizes = SIZES;
                                    sizes.sort();
                                    for v in sizes {
                                        ui.selectable_value(
                                            &mut s.texture.max_size,
                                            v,
                                            format!("{v} px"),
                                        );
                                    }
                                });
                            ui.label("longest side");
                        });
                    }
                    TextureSizing::PixelSize => {
                        slider_f32(
                            ui,
                            "cm per texel",
                            &mut s.texel_cm,
                            0.25..=20.0,
                            2.0,
                            "Same physical pixel density for every asset in your game",
                        );
                        ui.horizontal(|ui| {
                            ui.label("Model units per metre");
                            ui.add(
                                egui::DragValue::new(&mut s.units_per_meter)
                                    .range(0.001..=1000.0)
                                    .speed(0.1),
                            );
                        });
                        if let (Some(c), true) = (&self.converted, true) {
                            for (name, w, h, _) in &c.stats.textures {
                                hint(ui, &format!("{name}: {w}x{h}"));
                            }
                        }
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label("UV layout");
                ui.selectable_value(&mut s.uv_mode, UvMode::Atlas, "New atlas")
                    .on_hover_text("Unwrap the low-poly mesh into one tight atlas and rebake every material from the full-resolution source. No seam smearing at low budgets; one texture page.");
                ui.selectable_value(&mut s.uv_mode, UvMode::Keep, "Keep original")
                    .on_hover_text("Keep the source UVs and resize each material's texture (best for tiling textures).");
            });
            if s.uv_mode == UvMode::Atlas || self.pieces.iter().any(|p| p.baked) {
                ui.indent("atlas", |ui| {
                    slider_f32(ui, "bake distance %", &mut s.bake_distance, 0.2..=15.0, 3.0, "How far above/below the low-poly surface to look for source detail (% of model size). Raise it if parts come out with the wrong colour, lower it if detail from neighbouring surfaces bleeds in.");
                    slider_f32(ui, "chart angle", &mut s.chart_angle, 20.0..=85.0, 60.0, "Faces within this angle of a chart's main direction share one UV island: higher = fewer, larger islands.");
                    if let Some((n, charts)) = self.converted.as_ref().and_then(|c| c.stats.atlas) {
                        hint(ui, &format!("{n}x{n} atlas, {charts} charts"));
                    }
                });
            }
            if !s.hd_texture {
                ui.checkbox(&mut s.texture.power_of_two, "Power-of-two sizes");
                enum_combo(
                    ui,
                    "filter",
                    "downscale",
                    &mut s.texture.filter,
                    &ResizeFilter::ALL,
                    ResizeFilter::label,
                );
            }
            slider_f32(
                ui,
                "brightness",
                &mut s.texture.brightness,
                -0.5..=0.5,
                0.0,
                "Applied before the palette, so imported colours stay exact",
            );
            slider_f32(ui, "contrast", &mut s.texture.contrast, -0.5..=1.0, 0.0, "");
            slider_f32(
                ui,
                "saturation",
                &mut s.texture.saturation,
                -1.0..=1.0,
                0.0,
                "",
            );
            if !s.hd_texture {
                ui.separator();
                enum_combo(
                    ui,
                    "palmode",
                    "palette",
                    &mut s.texture.palette_mode,
                    &PaletteMode::ALL,
                    PaletteMode::label,
                );
                let imported = matches!(
                    s.texture.palette_mode,
                    PaletteMode::ImportedAll | PaletteMode::ImportedBestN
                );
                if imported && sh.palette.is_none() {
                    ui.label(
                        RichText::new(
                            "No palette loaded: using auto. Load one in the Palette panel.",
                        )
                        .color(WARN)
                        .small(),
                    );
                }
                if s.texture.palette_mode != PaletteMode::ImportedAll
                    && s.texture.palette_mode != PaletteMode::Rgb555Only
                {
                    slider_usize(
                        ui,
                        "colours",
                        &mut s.texture.colors,
                        2..=256,
                        16,
                        true,
                        "Colours per texture (CLUT size: 16 = 4-bit, 256 = 8-bit)",
                    );
                }
                ui.checkbox(&mut s.shared_palette, "One palette for all materials");
                ui.checkbox(&mut s.texture.rgb555, "Snap to 15-bit RGB555");
                enum_combo(
                    ui,
                    "dither",
                    "dither",
                    &mut s.texture.dither,
                    &DitherMode::ALL,
                    DitherMode::label,
                );
                if s.texture.dither != DitherMode::None {
                    slider_f32(
                        ui,
                        "dither amount",
                        &mut s.texture.dither_amount,
                        0.0..=64.0,
                        12.0,
                        "With a palette: how much the two nearest palette colours mix (16 = full, 0 = nearest only). RGB555 only: offset in 8-bit units, like the PS1 GPU (its table is +-4).",
                    );
                }
            }
            ui.horizontal(|ui| {
                ui.label("Alpha");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Opaque, "Opaque");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Cutout, "Cutout");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Keep, "RGBA");
            });
            ui.separator();
            ui.label(RichText::new("Bake into albedo").color(DIM));
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.bake_ao_map, "AO map");
                ui.checkbox(&mut s.bake_normal_map, "Normal relief");
                ui.checkbox(&mut s.bake_emissive, "Emission");
            });
            if s.bake_normal_map {
                slider_f32(ui, "relief", &mut s.normal_map_strength, 0.0..=2.0, 0.8, "");
            }
        });
    }

    fn export_section(&mut self, ui: &mut Ui, sh: &mut Shared) {
        section(ui, "Export", true, |ui| {
            ui.horizontal(|ui| {
                let mut p = self.export.preset;
                egui::ComboBox::from_id_salt("preset")
                    .selected_text(p.label())
                    .show_ui(ui, |ui| {
                        for e in EnginePreset::ALL {
                            ui.selectable_value(&mut p, e, e.label());
                        }
                    });
                if p != self.export.preset {
                    self.export = p.settings();
                }
                ui.label("engine");
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.export.glb, "GLB");
                ui.checkbox(&mut self.export.obj, "OBJ");
                ui.checkbox(&mut self.export.godot_scene, "Godot scene + shader");
                ui.checkbox(&mut self.export.fbx, "FBX")
                    .on_hover_text("Binary FBX through headless Blender: keeps skeletons and clips (Unity Humanoid / Mixamo). Needs Blender.");
            });
            if self.export.fbx && psx_forge::blender::find_blender().is_none() {
                ui.label(
                    RichText::new(
                        "Blender not found: FBX falls back to OBJ. Set PSXFORGE_BLENDER.",
                    )
                    .color(WARN)
                    .small(),
                );
            }
            ui.checkbox(&mut self.export.indexed_png, "Indexed (CLUT) PNG");
            ui.checkbox(&mut self.export.unlit, "Unlit material");
            ui.horizontal(|ui| {
                ui.label("Scale");
                ui.add(
                    egui::DragValue::new(&mut self.export.scale)
                        .range(0.001..=1000.0)
                        .speed(0.01),
                );
            });
            ui.add_space(4.0);
            let ready = self.converted.is_some() && self.job.is_none();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new("Export...").strong())
                        .min_size([ui.available_width(), 28.0].into()),
                )
                .clicked()
            {
                self.export_dialog(sh);
            }
        });
    }

    fn export_dialog(&mut self, sh: &mut Shared) {
        let mut d = rfd::FileDialog::new().set_title("Choose export folder");
        if let Some(p) = &self.last_export_dir {
            d = d.set_directory(p);
        }
        if let Some(dir) = d.pick_folder() {
            self.export_to(dir, sh);
        }
    }

    fn export_to(&mut self, dir: PathBuf, sh: &mut Shared) {
        let Some(c) = &self.converted else { return };
        let target = dir.join(export::sanitize(&c.model.name));
        match export::export_model(&c.model, &target, &self.export) {
            Ok(files) => {
                for (i, pal) in c.palettes.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
                    let p = target.join(format!(
                        "{}_palette{}.hex",
                        export::sanitize(&c.model.name),
                        if i == 0 {
                            String::new()
                        } else {
                            format!("_{i}")
                        }
                    ));
                    let _ = psx_forge::palette::save_palette(&p, pal);
                }
                sh.status = format!("Exported {} files to {}", files.len(), target.display());
                self.last_export_dir = Some(dir);
            }
            Err(e) => sh.error = Some(format!("Export failed: {e:#}")),
        }
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
        let reference = self.show_original || self.scene_out.is_none();
        let scene = if reference {
            self.scene_src.as_ref()
        } else {
            self.scene_out.as_ref()
        };
        let info = ui
            .allocate_ui(egui::vec2(avail.x, (avail.y - tex_h).max(100.0)), |ui| {
                self.viewport
                    .show(ui, scene, Controls::Viewer, &[], reference)
            })
            .inner;
        if reference {
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
            for (k, w) in c.stats.warnings.iter().enumerate() {
                painter.text(
                    info.rect.left_top() + egui::vec2(8.0, 44.0 + k as f32 * 16.0),
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

    pub fn quick_export_window(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        if !self.quick_export_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Quick Export")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if self.converted.is_none() {
                    ui.label("Convert a model first.");
                    return;
                }
                match self.quick_step {
                    0 => {
                        ui.label("Which engine is this for?");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            for e in EnginePreset::ALL {
                                if ui
                                    .add_sized(
                                        [96.0, 56.0],
                                        egui::Button::new(RichText::new(e.label()).strong()),
                                    )
                                    .clicked()
                                {
                                    self.export = e.settings();
                                    self.quick_step = 1;
                                }
                            }
                        });
                    }
                    _ => {
                        ui.label(
                            RichText::new(self.export.preset.label())
                                .heading()
                                .color(ACCENT2),
                        );
                        let formats: Vec<&str> = [
                            (self.export.glb, "GLB"),
                            (self.export.obj, "OBJ"),
                            (self.export.godot_scene, "Godot .tscn + shader"),
                        ]
                        .into_iter()
                        .filter(|f| f.0)
                        .map(|f| f.1)
                        .collect();
                        ui.label(format!(
                            "Formats: {}  ·  scale {}  ·  {}",
                            formats.join(", "),
                            self.export.scale,
                            if self.export.indexed_png {
                                "indexed PNG"
                            } else {
                                "RGBA PNG"
                            }
                        ));
                        ui.add_space(4.0);
                        ui.label(RichText::new(self.export.preset.import_notes()).small());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Back").clicked() {
                                self.quick_step = 0;
                            }
                            if ui
                                .button(RichText::new("Choose folder & export").strong())
                                .clicked()
                            {
                                self.export_dialog(sh);
                                self.quick_export_open = false;
                            }
                        });
                    }
                }
            });
        if !open {
            self.quick_export_open = false;
        }
    }
}
