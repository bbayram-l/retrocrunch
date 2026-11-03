//! Sprite Lights tab: generate normal/specular/occlusion/parallax maps
//! and preview them under a movable point light.

use super::Shared;
use super::texture_tab::IMAGE_EXTS;
use super::widgets::{self, DIM, ImageView, TexSlot, hint, section, slider_f32, slider_u32};
use egui::{RichText, Ui};
use image::{DynamicImage, RgbaImage};
use psx_forge::sprite::{self, LightSettings, SpriteMaps, SpriteSettings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MapView {
    Lit,
    Diffuse,
    Normal,
    Specular,
    Occlusion,
    Parallax,
}

impl MapView {
    const ALL: [MapView; 6] = [
        Self::Lit,
        Self::Diffuse,
        Self::Normal,
        Self::Specular,
        Self::Occlusion,
        Self::Parallax,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit preview",
            Self::Diffuse => "Diffuse",
            Self::Normal => "Normal",
            Self::Specular => "Specular",
            Self::Occlusion => "Occlusion",
            Self::Parallax => "Parallax",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SpriteTab {
    pub settings: SpriteSettings,
    pub light: LightSettings,
    pub view_mode: MapView,
    pub export_godot: bool,
    pub light_follows_mouse: bool,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    source: Option<RgbaImage>,
    #[serde(skip)]
    maps: Option<SpriteMaps>,
    #[serde(skip)]
    tex: TexSlot,
    #[serde(skip)]
    view: ImageView,
    #[serde(skip)]
    last_key: String,
    #[serde(skip)]
    shown_key: String,
}

impl Default for SpriteTab {
    fn default() -> Self {
        Self {
            settings: SpriteSettings::default(),
            light: LightSettings::default(),
            view_mode: MapView::Lit,
            export_godot: true,
            light_follows_mouse: true,
            path: None,
            source: None,
            maps: None,
            tex: TexSlot::default(),
            view: ImageView {
                checker: true,
                ..Default::default()
            },
            last_key: String::new(),
            shown_key: String::new(),
        }
    }
}

fn gray_to_rgba(g: &image::GrayImage) -> RgbaImage {
    DynamicImage::ImageLuma8(g.clone()).to_rgba8()
}

/// Write `<stem>_n/_s/_o/_h.png` (+ optional Godot CanvasTexture) next to each other.
pub fn export_maps(
    dir: &Path,
    stem: &str,
    diffuse: &RgbaImage,
    m: &SpriteMaps,
    godot: bool,
    shininess: f32,
    copy_diffuse: bool,
) -> anyhow::Result<usize> {
    let mut n = 0;
    let names = [
        (format!("{stem}_n.png"), m.normal.clone()),
        (format!("{stem}_s.png"), gray_to_rgba(&m.specular)),
        (format!("{stem}_o.png"), gray_to_rgba(&m.occlusion)),
        (format!("{stem}_h.png"), gray_to_rgba(&m.parallax)),
    ];
    for (name, img) in &names {
        img.save(dir.join(name))?;
        n += 1;
    }
    let diffuse_name = format!("{stem}.png");
    if copy_diffuse && !dir.join(&diffuse_name).exists() {
        diffuse.save(dir.join(&diffuse_name))?;
        n += 1;
    }
    if godot {
        let tres = sprite::godot_canvas_texture(&diffuse_name, &names[0].0, &names[1].0, shininess);
        std::fs::write(dir.join(format!("{stem}_canvas.tres")), tres)?;
        n += 1;
    }
    Ok(n)
}

impl SpriteTab {
    pub fn open_dialog(&mut self, sh: &mut Shared) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Images", &IMAGE_EXTS)
            .pick_file()
        {
            self.load(p, sh);
        }
    }

    pub fn load(&mut self, p: PathBuf, sh: &mut Shared) {
        match image::open(&p) {
            Ok(i) => {
                let img = i.to_rgba8();
                self.light.pos = [
                    img.width() as f32 * 0.25,
                    img.height() as f32 * 0.25,
                    self.light.pos[2],
                ];
                self.source = Some(img);
                self.path = Some(p);
                self.last_key.clear();
                self.view = ImageView::default();
            }
            Err(e) => sh.error = Some(format!("Could not open image: {e}")),
        }
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let Some(src) = &self.source else { return };
        let key = serde_json::to_string(&self.settings).unwrap_or_default();
        if key != self.last_key {
            self.last_key = key;
            self.maps = Some(sprite::generate(src, &self.settings));
            self.shown_key.clear();
        }
        let Some(m) = &self.maps else { return };
        let shown = format!(
            "{:?}|{}",
            self.view_mode as u8,
            if self.view_mode == MapView::Lit {
                serde_json::to_string(&self.light).unwrap_or_default()
            } else {
                String::new()
            }
        );
        if shown != self.shown_key {
            self.shown_key = shown;
            let img = match self.view_mode {
                MapView::Lit => sprite::lit_preview(src, m, &self.light, self.settings.flip_y),
                MapView::Diffuse => src.clone(),
                MapView::Normal => m.normal.clone(),
                MapView::Specular => gray_to_rgba(&m.specular),
                MapView::Occlusion => gray_to_rgba(&m.occlusion),
                MapView::Parallax => gray_to_rgba(&m.parallax),
            };
            self.tex.set(ctx, "sprite", &img);
        }
    }

    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open sprite...").clicked() {
                self.open_dialog(sh);
            }
            if ui
                .button("Batch...")
                .on_hover_text("Generate maps for many sprites")
                .clicked()
            {
                self.batch(sh);
            }
        });
        if let Some(p) = &self.path {
            hint(ui, &p.display().to_string());
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let s = &mut self.settings;
                section(ui, "Height", true, |ui| {
                    slider_f32(
                        ui,
                        "detail",
                        &mut s.detail,
                        0.0..=2.0,
                        1.0,
                        "Emboss from brightness (surface texture)",
                    );
                    slider_u32(
                        ui,
                        "detail blur",
                        &mut s.detail_blur,
                        0..=8,
                        1,
                        "Soften brightness noise before embossing",
                    );
                    ui.checkbox(&mut s.invert_detail, "Invert detail (dark = high)");
                    slider_f32(
                        ui,
                        "bevel",
                        &mut s.bevel,
                        0.0..=2.0,
                        0.6,
                        "Rounded volume from the sprite's alpha edges",
                    );
                    slider_f32(
                        ui,
                        "bevel width",
                        &mut s.bevel_width,
                        0.5..=48.0,
                        6.0,
                        "Distance in pixels from the edge to the plateau",
                    );
                    ui.checkbox(&mut s.tileable, "Tileable (wrap edges)");
                });
                section(ui, "Normal map", true, |ui| {
                    slider_f32(ui, "strength", &mut s.normal_strength, 0.0..=10.0, 2.5, "");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut s.flip_y, false, "OpenGL / Godot (Y+)");
                        ui.selectable_value(&mut s.flip_y, true, "DirectX / Unreal (Y-)");
                    });
                });
                section(ui, "Specular / Occlusion / Parallax", false, |ui| {
                    ui.checkbox(
                        &mut s.specular_from_height,
                        "Specular from height (else brightness)",
                    );
                    slider_f32(
                        ui,
                        "spec contrast",
                        &mut s.specular_contrast,
                        0.0..=5.0,
                        1.5,
                        "",
                    );
                    slider_f32(
                        ui,
                        "spec threshold",
                        &mut s.specular_threshold,
                        0.0..=1.0,
                        0.4,
                        "",
                    );
                    slider_f32(
                        ui,
                        "occlusion",
                        &mut s.occlusion_strength,
                        0.0..=4.0,
                        1.0,
                        "",
                    );
                    slider_u32(
                        ui,
                        "occlusion radius",
                        &mut s.occlusion_radius,
                        1..=24,
                        4,
                        "",
                    );
                    slider_f32(
                        ui,
                        "parallax contrast",
                        &mut s.parallax_contrast,
                        0.0..=3.0,
                        1.0,
                        "",
                    );
                });
                let l = &mut self.light;
                section(ui, "Preview light", true, |ui| {
                    ui.checkbox(
                        &mut self.light_follows_mouse,
                        "Light follows the mouse (click to pin)",
                    );
                    slider_f32(
                        ui,
                        "height",
                        &mut l.pos[2],
                        1.0..=300.0,
                        40.0,
                        "Light distance above the sprite",
                    );
                    slider_f32(ui, "intensity", &mut l.intensity, 0.0..=4.0, 1.0, "");
                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut l.color);
                        ui.label("light");
                        ui.color_edit_button_rgb(&mut l.ambient);
                        ui.label("ambient");
                    });
                    slider_f32(ui, "ambient", &mut l.ambient_intensity, 0.0..=1.5, 0.35, "");
                    slider_f32(ui, "specular", &mut l.specular, 0.0..=3.0, 0.6, "");
                    slider_f32(ui, "shininess", &mut l.shininess, 1.0..=128.0, 16.0, "");
                    slider_u32(
                        ui,
                        "toon bands",
                        &mut l.bands,
                        0..=8,
                        0,
                        "Quantise lighting into bands (0 = smooth)",
                    );
                    ui.checkbox(&mut l.use_occlusion, "Use occlusion map");
                });
                section(ui, "Export", true, |ui| {
                    ui.checkbox(&mut self.export_godot, "Godot CanvasTexture (.tres)");
                    hint(
                        ui,
                        "Writes _n (normal), _s (specular), _o (occlusion), _h (height/parallax).",
                    );
                    if ui
                        .add_enabled(self.maps.is_some(), egui::Button::new("Export maps..."))
                        .clicked()
                    {
                        self.export(sh);
                    }
                });
            });
    }

    fn export(&mut self, sh: &mut Shared) {
        let (Some(src), Some(m), Some(path)) = (&self.source, &self.maps, &self.path) else {
            return;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sprite")
            .to_string();
        let mut d = rfd::FileDialog::new().set_title("Export folder");
        if let Some(parent) = path.parent() {
            d = d.set_directory(parent);
        }
        let Some(dir) = d.pick_folder() else { return };
        let copy = path.parent() != Some(dir.as_path());
        match export_maps(
            &dir,
            &stem,
            src,
            m,
            self.export_godot,
            self.light.shininess,
            copy,
        ) {
            Ok(n) => sh.status = format!("Wrote {n} files to {}", dir.display()),
            Err(e) => sh.error = Some(format!("Export failed: {e:#}")),
        }
    }

    fn batch(&mut self, sh: &mut Shared) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Images", &IMAGE_EXTS)
            .pick_files()
        else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Output folder")
            .pick_folder()
        else {
            return;
        };
        let mut ok = 0;
        let mut errs = Vec::new();
        for f in &files {
            let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("sprite");
            let res = image::open(f).map_err(anyhow::Error::from).and_then(|i| {
                let img = i.to_rgba8();
                let m = sprite::generate(&img, &self.settings);
                export_maps(
                    &dir,
                    stem,
                    &img,
                    &m,
                    self.export_godot,
                    self.light.shininess,
                    f.parent() != Some(dir.as_path()),
                )
            });
            match res {
                Ok(_) => ok += 1,
                Err(e) => errs.push(format!("{}: {e:#}", f.display())),
            }
        }
        sh.status = format!("Batch: {ok} sprites processed into {}", dir.display());
        if !errs.is_empty() {
            sh.error = Some(errs.join("\n"));
        }
    }

    pub fn central(&mut self, ui: &mut Ui, sh: &mut Shared) {
        self.refresh(ui.ctx());
        let (Some(src), Some(h)) = (&self.source, self.tex.handle.clone()) else {
            if widgets::empty_state(
                ui,
                "Drop a sprite here",
                "Normal, specular, occlusion and parallax maps for 2D lighting",
            ) {
                self.open_dialog(sh);
            }
            return;
        };
        ui.horizontal(|ui| {
            for v in MapView::ALL {
                ui.selectable_value(&mut self.view_mode, v, v.label());
            }
            ui.separator();
            ui.checkbox(&mut self.view.checker, "Checker");
        });
        let size = egui::vec2(src.width() as f32, src.height() as f32);
        let avail = ui.available_size();
        let (resp, shown) = self.view.show(ui, &h, size, avail - egui::vec2(0.0, 22.0));
        if self.view_mode == MapView::Lit {
            let pinned = resp.clicked();
            if pinned {
                self.light_follows_mouse = !self.light_follows_mouse;
            }
            if (self.light_follows_mouse || resp.dragged_by(egui::PointerButton::Secondary))
                && let Some(p) = resp.hover_pos()
            {
                let t = (p - shown.min) / shown.size();
                self.light.pos[0] = t.x * size.x;
                self.light.pos[1] = t.y * size.y;
            }
            // Light marker.
            let lp =
                shown.min + egui::vec2(self.light.pos[0], self.light.pos[1]) / size * shown.size();
            let c = egui::Color32::from_rgb(
                (self.light.color[0] * 255.0) as u8,
                (self.light.color[1] * 255.0) as u8,
                (self.light.color[2] * 255.0) as u8,
            );
            ui.painter()
                .circle(lp, 6.0, c, egui::Stroke::new(1.5f32, egui::Color32::BLACK));
        }
        ui.label(
            RichText::new(format!(
                "{}x{} · wheel to zoom, drag to pan, double-click to fit",
                src.width(),
                src.height()
            ))
            .small()
            .color(DIM),
        );
    }
}
