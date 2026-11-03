//! Pixel Scale tab: Scale2x/3x/4x/6x/9x, Eagle and nearest, plus mode downscale.

use super::Shared;
use super::texture_tab::{IMAGE_EXTS, batch_images};
use super::widgets::{self, DIM, ImageView, TexSlot, hint, section};
use egui::{RichText, Ui};
use image::RgbaImage;
use psx_forge::scale::{self, Scaler};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct ScaleTab {
    pub scaler: Scaler,
    pub tolerance: u8,
    /// Shrink by this factor first (undo a previous nearest upscale).
    pub pre_downscale: u32,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    source: Option<RgbaImage>,
    #[serde(skip)]
    result: Option<RgbaImage>,
    #[serde(skip)]
    tex_src: TexSlot,
    #[serde(skip)]
    tex_out: TexSlot,
    #[serde(skip)]
    view_src: ImageView,
    #[serde(skip)]
    view_out: ImageView,
    #[serde(skip)]
    last_key: String,
}

impl Default for ScaleTab {
    fn default() -> Self {
        Self {
            scaler: Scaler::Scale3x,
            tolerance: 0,
            pre_downscale: 1,
            path: None,
            source: None,
            result: None,
            tex_src: TexSlot::default(),
            tex_out: TexSlot::default(),
            view_src: ImageView::default(),
            view_out: ImageView::default(),
            last_key: String::new(),
        }
    }
}

const MAX_PIXELS: u32 = 1024 * 1024;

impl ScaleTab {
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
                if img.width() * img.height() > MAX_PIXELS {
                    sh.status = "Large image: pixel-art scalers are meant for small sprites and may be slow.".into();
                }
                self.source = Some(img);
                self.path = Some(p);
                self.last_key.clear();
                self.tex_src.clear();
                self.view_src = ImageView::default();
                self.view_out = ImageView::default();
            }
            Err(e) => sh.error = Some(format!("Could not open image: {e}")),
        }
    }

    fn apply(&self, img: &RgbaImage) -> RgbaImage {
        let base = if self.pre_downscale > 1 {
            scale::downscale_mode(img, self.pre_downscale)
        } else {
            img.clone()
        };
        self.scaler.apply(&base, self.tolerance)
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let Some(src) = &self.source else { return };
        if self.tex_src.handle.is_none() {
            self.tex_src.set(ctx, "scale_src", src);
        }
        let key = format!(
            "{:?}|{}|{}",
            self.scaler, self.tolerance, self.pre_downscale
        );
        if key != self.last_key {
            self.last_key = key;
            let out = self.apply(src);
            self.tex_out.set(ctx, "scale_out", &out);
            self.result = Some(out);
            self.view_out.zoom = 0.0;
        }
    }

    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open image...").clicked() {
                self.open_dialog(sh);
            }
            if ui.button("Batch...").clicked() {
                self.batch(sh);
            }
        });
        if let Some(p) = &self.path {
            hint(ui, &p.display().to_string());
        }
        ui.separator();
        section(ui, "Algorithm", true, |ui| {
            for s in Scaler::ALL {
                ui.radio_value(&mut self.scaler, s, s.label());
            }
            hint(
                ui,
                "Scale2x/3x keep the palette: no new colours, no blur. Meant for small, non-antialiased pixel art.",
            );
        });
        section(ui, "Options", true, |ui| {
            ui.add(egui::Slider::new(&mut self.tolerance, 0..=64).text("colour tolerance"))
                .on_hover_text("Treat nearly-equal colours as equal (helps with JPEG noise)");
            ui.add(egui::Slider::new(&mut self.pre_downscale, 1..=8).text("pre-downscale"))
                .on_hover_text("Undo an earlier nearest-neighbour upscale before smoothing");
        });
        section(ui, "Save", true, |ui| {
            if ui
                .add_enabled(self.result.is_some(), egui::Button::new("Save PNG..."))
                .clicked()
            {
                self.save(sh);
            }
        });
    }

    fn save(&self, sh: &mut Shared) {
        let Some(img) = &self.result else { return };
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("image");
        let name = format!("{stem}_{}x.png", self.scaler.factor());
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(name)
            .save_file()
        {
            match img.save(&p) {
                Ok(()) => sh.status = format!("Saved {}", p.display()),
                Err(e) => sh.error = Some(e.to_string()),
            }
        }
    }

    fn batch(&self, sh: &mut Shared) {
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
        let suffix = format!("_{}x", self.scaler.factor());
        let (ok, errors) = batch_images(&files, &dir, |img| self.apply(img), &suffix, false);
        sh.status = format!("Batch: {ok} images scaled into {}", dir.display());
        if !errors.is_empty() {
            sh.error = Some(errors.join("\n"));
        }
    }

    pub fn central(&mut self, ui: &mut Ui, sh: &mut Shared) {
        self.refresh(ui.ctx());
        let (Some(src), Some(hs), Some(ho), Some(out)) = (
            &self.source,
            &self.tex_src.handle,
            &self.tex_out.handle,
            &self.result,
        ) else {
            if widgets::empty_state(
                ui,
                "Drop pixel art here",
                "Scale2x / Scale3x / Eagle: smooth upscales without new colours",
            ) {
                self.open_dialog(sh);
            }
            return;
        };
        let avail = ui.available_size();
        let src_w = (avail.x * 0.35).max(120.0);
        let h = avail.y - 30.0;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!("Source {}x{}", src.width(), src.height())).color(DIM),
                );
                self.view_src.show(
                    ui,
                    hs,
                    egui::vec2(src.width() as f32, src.height() as f32),
                    egui::vec2(src_w, h),
                );
            });
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  {}x{}",
                        self.scaler.label(),
                        out.width(),
                        out.height()
                    ))
                    .color(DIM),
                );
                self.view_out.show(
                    ui,
                    ho,
                    egui::vec2(out.width() as f32, out.height() as f32),
                    egui::vec2(avail.x - src_w - 12.0, h),
                );
            });
        });
    }
}
