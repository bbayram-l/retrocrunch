//! Texture tab: crunch any image PS1-style (resize, palette, RGB555, dither), single or batch.

use super::Shared;
use super::widgets::{
    self, DIM, ImageView, TexSlot, enum_combo, hint, section, slider_f32, slider_usize,
};
use egui::{RichText, Ui};
use image::RgbaImage;
use retrocrunch::palette::Rgb;
use retrocrunch::texture::{
    self, AlphaMode, BakeMaps, CrunchSettings, DitherMode, PaletteMode, ResizeFilter,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TextureTab {
    pub settings: CrunchSettings,
    pub indexed_png: bool,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    source: Option<RgbaImage>,
    #[serde(skip)]
    result: Option<(RgbaImage, Vec<Rgb>)>,
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
    /// Colour counts of the source and the result, cached: counting a 4K image
    /// every frame made the tab lag.
    #[serde(skip)]
    color_counts: (usize, usize),
    /// Palette cycling preview (experimental).
    #[serde(skip)]
    cycle: Cycle,
}

/// Palette cycling state: entries `start..=end` of the result palette rotate.
struct Cycle {
    on: bool,
    start: usize,
    end: usize,
    speed: f32,
    indexed: Option<retrocrunch::palette_cycle::Indexed>,
    tex: TexSlot,
    step: Option<i64>,
}

impl Default for Cycle {
    fn default() -> Self {
        Self {
            on: false,
            start: 0,
            end: 3,
            speed: 8.0,
            indexed: None,
            tex: TexSlot::default(),
            step: None,
        }
    }
}

pub const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "tga", "gif"];

pub fn crunch_with(
    img: &RgbaImage,
    s: &CrunchSettings,
    palette: Option<&[Rgb]>,
) -> (RgbaImage, Vec<Rgb>) {
    let c = texture::crunch(img, s, &BakeMaps::default(), palette);
    (c.image, c.palette)
}

impl TextureTab {
    pub fn clear_asset(&mut self) {
        let (settings, indexed) = (std::mem::take(&mut self.settings), self.indexed_png);
        *self = Self {
            settings,
            indexed_png: indexed,
            ..Default::default()
        };
    }

    pub fn revert_to_defaults(&mut self) {
        self.settings = CrunchSettings::default();
        self.indexed_png = true;
        self.last_key.clear();
    }

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
                self.color_counts = (texture::count_colors(&img), 0);
                self.source = Some(img);
                self.path = Some(p);
                self.last_key.clear();
                self.view_src = ImageView::default();
                self.view_out = ImageView::default();
                self.tex_src.clear();
            }
            Err(e) => sh.error = Some(format!("Could not open image: {e}")),
        }
    }

    fn update(&mut self, ctx: &egui::Context, sh: &Shared) {
        let Some(src) = &self.source else { return };
        let key = format!(
            "{}|{}",
            serde_json::to_string(&self.settings).unwrap_or_default(),
            sh.palette.as_ref().map_or("", |p| &p.name)
        );
        if self.tex_src.handle.is_none() {
            self.tex_src.set(ctx, "tex_src", src);
        }
        if key != self.last_key {
            self.last_key = key;
            let r = crunch_with(
                src,
                &self.settings,
                sh.palette.as_ref().map(|p| p.colors.as_slice()),
            );
            self.tex_out.set(ctx, "tex_out", &r.0);
            self.color_counts.1 = texture::count_colors(&r.0);
            self.result = Some(r);
            // The palette changed: re-index for cycling.
            self.cycle.indexed = None;
            self.cycle.step = None;
        }
    }

    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open image...").clicked() {
                self.open_dialog(sh);
            }
            if ui
                .button("Batch...")
                .on_hover_text("Crunch many images with these settings")
                .clicked()
            {
                self.batch(sh);
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.source.is_some(), egui::Button::new("Clear"))
                .on_hover_text("Unload the image (settings are kept)")
                .clicked()
            {
                self.clear_asset();
                sh.status = "Cleared.".into();
            }
            if ui
                .button("Revert to defaults")
                .on_hover_text("Reset this tab's settings")
                .clicked()
            {
                self.revert_to_defaults();
                sh.status = "Settings reverted to defaults.".into();
            }
        });
        if let Some(p) = &self.path {
            hint(ui, &p.display().to_string());
        }
        ui.separator();
        let s = &mut self.settings;
        let mut do_save = false;
        let mut do_cycle_export = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                section(ui, "Size", true, |ui| {
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("tsize")
                            .selected_text(format!("{} px", s.max_size))
                            .show_ui(ui, |ui| {
                                for v in [4u32, 8, 16, 32, 64, 128, 256, 512, 1024, 2048] {
                                    ui.selectable_value(&mut s.max_size, v, format!("{v} px"));
                                }
                            });
                        ui.label("longest side");
                    });
                    ui.checkbox(&mut s.power_of_two, "Power-of-two");
                    enum_combo(
                        ui,
                        "tfilter",
                        "downscale",
                        &mut s.filter,
                        &ResizeFilter::ALL,
                        ResizeFilter::label,
                    );
                });
                section(ui, "Colour", true, |ui| {
                    slider_f32(ui, "brightness", &mut s.brightness, -0.5..=0.5, 0.0, "");
                    slider_f32(ui, "contrast", &mut s.contrast, -0.5..=1.0, 0.0, "");
                    slider_f32(ui, "saturation", &mut s.saturation, -1.0..=1.0, 0.0, "");
                    enum_combo(
                        ui,
                        "tpal",
                        "palette",
                        &mut s.palette_mode,
                        &PaletteMode::ALL,
                        PaletteMode::label,
                    );
                    if s.palette_mode != PaletteMode::ImportedAll
                        && s.palette_mode != PaletteMode::Rgb555Only
                    {
                        slider_usize(ui, "colours", &mut s.colors, 2..=256, 16, true, "");
                    }
                    ui.checkbox(&mut s.rgb555, "Snap to 15-bit RGB555");
                    enum_combo(
                        ui,
                        "tdither",
                        "dither",
                        &mut s.dither,
                        &DitherMode::ALL,
                        DitherMode::label,
                    );
                    if s.dither != DitherMode::None {
                        slider_f32(
                            ui,
                            "dither amount",
                            &mut s.dither_amount,
                            0.0..=64.0,
                            12.0,
                            "With a palette: how much the two nearest palette colours mix (16 = full, 0 = nearest only). RGB555 only: offset in 8-bit units, like the PS1 GPU (its table is +-4).",
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label("Alpha");
                        ui.selectable_value(&mut s.alpha, AlphaMode::Opaque, "Opaque");
                        ui.selectable_value(&mut s.alpha, AlphaMode::Cutout, "Cutout");
                        ui.selectable_value(&mut s.alpha, AlphaMode::Keep, "RGBA");
                    });
                });
                if let Some((_, pal)) = &self.result
                    && !pal.is_empty()
                {
                    let c = &mut self.cycle;
                    let last = pal.len() - 1;
                    section(ui, "Palette cycling (experimental)", false, |ui| {
                        hint(ui, "Rotate a range of palette entries over time (water, lava, lights). Entries are numbered as in the swatches below the image, from 0.");
                        if ui.checkbox(&mut c.on, "Animate").changed() {
                            c.step = None;
                        }
                        ui.horizontal(|ui| {
                            ui.label("entries");
                            ui.add(egui::DragValue::new(&mut c.start).range(0..=last));
                            ui.label("to");
                            ui.add(egui::DragValue::new(&mut c.end).range(0..=last));
                        });
                        c.end = c.end.max(c.start).min(last);
                        slider_f32(ui, "steps/s", &mut c.speed, -30.0..=30.0, 8.0, "Palette steps per second; negative runs the other way");
                        if ui
                            .button("Export cycle...")
                            .on_hover_text("Index PNG + palette strip + Godot shaders (2D and 3D) + animated GIF")
                            .clicked()
                        {
                            do_cycle_export = true;
                        }
                    });
                }
                section(ui, "Save", true, |ui| {
                    ui.checkbox(&mut self.indexed_png, "Indexed (CLUT) PNG");
                    if ui
                        .add_enabled(self.result.is_some(), egui::Button::new("Save PNG..."))
                        .clicked()
                    {
                        do_save = true;
                    }
                    if let Some((_, pal)) = &self.result
                        && !pal.is_empty()
                        && ui.button("Save palette...").clicked()
                        && let Some(p) = rfd::FileDialog::new()
                            .add_filter("Palette", &["hex", "gpl", "pal"])
                            .set_file_name("palette.hex")
                            .save_file()
                    {
                        match retrocrunch::palette::save_palette(&p, pal) {
                            Ok(()) => sh.status = format!("Saved {}", p.display()),
                            Err(e) => sh.error = Some(e.to_string()),
                        }
                    }
                });
            });
        if do_save {
            self.save(sh);
        }
        if do_cycle_export {
            self.export_cycle(sh);
        }
    }

    fn ensure_indexed(&mut self) {
        if self.cycle.indexed.is_none()
            && let Some((img, pal)) = &self.result
        {
            self.cycle.indexed = Some(retrocrunch::palette_cycle::index_image(img, pal));
        }
    }

    fn export_cycle(&mut self, sh: &mut Shared) {
        self.ensure_indexed();
        let Some(ix) = &self.cycle.indexed else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Folder for the palette cycle")
            .pick_folder()
        else {
            return;
        };
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("texture")
            .to_string();
        let c = &self.cycle;
        match retrocrunch::palette_cycle::export(&dir, &name, ix, c.start, c.end, c.speed) {
            Ok(files) => {
                sh.status = format!("Palette cycle: {} files in {}", files.len(), dir.display())
            }
            Err(e) => sh.error = Some(format!("{e:#}")),
        }
    }

    fn save(&mut self, sh: &mut Shared) {
        let Some((img, _)) = &self.result else { return };
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("texture");
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(format!("{name}_psx.png"))
            .save_file()
        {
            match texture::save_png(&p, img, self.indexed_png) {
                Ok(()) => sh.status = format!("Saved {}", p.display()),
                Err(e) => sh.error = Some(format!("{e:#}")),
            }
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
        let pal = sh.palette.as_ref().map(|p| p.colors.clone());
        let (ok, errors) = batch_images(
            &files,
            &dir,
            |img| crunch_with(img, &self.settings, pal.as_deref()).0,
            "_psx",
            self.indexed_png,
        );
        sh.status = format!("Batch: {ok} images written to {}", dir.display());
        if !errors.is_empty() {
            sh.error = Some(errors.join("\n"));
        }
    }

    pub fn central(&mut self, ui: &mut Ui, sh: &mut Shared) {
        self.update(ui.ctx(), sh);
        let (Some(src), Some(hs), Some(ho)) =
            (&self.source, &self.tex_src.handle, &self.tex_out.handle)
        else {
            if widgets::empty_state(
                ui,
                "Drop an image here",
                "Crunch textures to PS1 specs: tiny size, CLUT palette, 15-bit colour, dither",
            ) {
                self.open_dialog(sh);
            }
            return;
        };
        let (src_size, out) = (
            egui::vec2(src.width() as f32, src.height() as f32),
            self.result.as_ref(),
        );
        let out_size = out.map_or(src_size, |(o, _)| {
            egui::vec2(o.width() as f32, o.height() as f32)
        });
        let avail = ui.available_size();
        let half = egui::vec2((avail.x - 12.0) / 2.0, avail.y - 70.0);
        // Palette cycling: show the current step of the animation instead of the result.
        let mut shown = ho.clone();
        if self.cycle.on && self.result.is_some() {
            let ctx = ui.ctx().clone();
            let step = (ctx.input(|i| i.time) * self.cycle.speed as f64).floor() as i64;
            if self.cycle.step != Some(step) {
                self.cycle.step = Some(step);
                if self.cycle.indexed.is_none()
                    && let Some((img, pal)) = &self.result
                {
                    self.cycle.indexed = Some(retrocrunch::palette_cycle::index_image(img, pal));
                }
                if let Some(ix) = &self.cycle.indexed {
                    let f = ix.frame(self.cycle.start, self.cycle.end, step);
                    self.cycle.tex.set(&ctx, "tex_cycle", &f);
                }
            }
            if let Some(h) = &self.cycle.tex.handle {
                shown = h.clone();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Original  {}x{}  {} colours",
                        src.width(),
                        src.height(),
                        self.color_counts.0
                    ))
                    .color(DIM),
                );
                self.view_src.show(ui, hs, src_size, half);
            });
            ui.vertical(|ui| {
                if let Some((o, _)) = out {
                    ui.label(
                        RichText::new(format!(
                            "PSX  {}x{}  {} colours",
                            o.width(),
                            o.height(),
                            self.color_counts.1
                        ))
                        .color(DIM),
                    );
                }
                self.view_out.show(ui, &shown, out_size, half);
            });
        });
        if let Some((_, pal)) = out {
            widgets::swatches(ui, pal, 14.0);
        }
    }
}

/// Apply `f` to every image and write `<stem><suffix>.png` into `dir`.
pub fn batch_images(
    files: &[PathBuf],
    dir: &Path,
    f: impl Fn(&RgbaImage) -> RgbaImage,
    suffix: &str,
    indexed: bool,
) -> (usize, Vec<String>) {
    let mut ok = 0;
    let mut errors = Vec::new();
    for file in files {
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let out = dir.join(format!("{stem}{suffix}.png"));
        match image::open(file)
            .map_err(anyhow::Error::from)
            .and_then(|i| texture::save_png(&out, &f(&i.to_rgba8()), indexed))
        {
            Ok(()) => ok += 1,
            Err(e) => errors.push(format!("{}: {e:#}", file.display())),
        }
    }
    (ok, errors)
}
