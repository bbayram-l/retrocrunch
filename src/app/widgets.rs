//! Shared UI helpers: themed section headers, resettable sliders, zoomable image views,
//! palette swatches and a texture cache.

use egui::{
    Color32, ColorImage, Pos2, Rect, Response, Sense, TextureHandle, TextureOptions, Ui, Vec2,
};
use image::RgbaImage;
use psx_forge::palette::Rgb;

pub const ACCENT: Color32 = Color32::from_rgb(0x7c, 0x5c, 0xff);
pub const ACCENT2: Color32 = Color32::from_rgb(0x2e, 0xd3, 0xb7);
pub const WARN: Color32 = Color32::from_rgb(0xff, 0xc8, 0x57);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x8f, 0xa3);

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = egui::Stroke::new(1.0f32, Color32::WHITE);
    v.hyperlink_color = ACCENT2;
    v.window_rounding = egui::Rounding::same(8.0);
    v.menu_rounding = egui::Rounding::same(6.0);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.rounding = egui::Rounding::same(5.0);
    }
    if dark {
        v.panel_fill = Color32::from_rgb(0x17, 0x18, 0x22);
        v.window_fill = Color32::from_rgb(0x1d, 0x1e, 0x2a);
        v.extreme_bg_color = Color32::from_rgb(0x0f, 0x10, 0x17);
        v.faint_bg_color = Color32::from_rgb(0x21, 0x22, 0x30);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x2a, 0x2b, 0x3b);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x38, 0x36, 0x55);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x4a, 0x3f, 0x8a);
    }
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.slider_width = 150.0;
        s.spacing.interact_size.y = 22.0;
    });
}

/// Collapsible section with an accent title.
pub fn section<R>(
    ui: &mut Ui,
    title: &str,
    open: bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    egui::CollapsingHeader::new(egui::RichText::new(title).strong().color(ACCENT2))
        .default_open(open)
        .show(ui, add)
        .body_returned
}

/// Slider that resets to `default` on double-click. Returns true when changed.
pub fn slider_f32(
    ui: &mut Ui,
    label: &str,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    tip: &str,
) -> bool {
    let r = ui.add(egui::Slider::new(v, range).text(label));
    finish_slider(r, v, default, tip)
}

pub fn slider_usize(
    ui: &mut Ui,
    label: &str,
    v: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    default: usize,
    log: bool,
    tip: &str,
) -> bool {
    let r = ui.add(egui::Slider::new(v, range).logarithmic(log).text(label));
    finish_slider(r, v, default, tip)
}

pub fn slider_u32(
    ui: &mut Ui,
    label: &str,
    v: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    default: u32,
    tip: &str,
) -> bool {
    let r = ui.add(egui::Slider::new(v, range).text(label));
    finish_slider(r, v, default, tip)
}

fn finish_slider<T: Copy>(r: Response, v: &mut T, default: T, tip: &str) -> bool {
    let mut changed = r.changed();
    let r = if tip.is_empty() {
        r
    } else {
        r.on_hover_text(format!("{tip}\n(double-click to reset)"))
    };
    if r.double_clicked() {
        *v = default;
        changed = true;
    }
    changed
}

pub fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Lazily (re)uploaded texture.
#[derive(Default)]
pub struct TexSlot {
    pub handle: Option<TextureHandle>,
}

impl TexSlot {
    pub fn set(&mut self, ctx: &egui::Context, name: &str, img: &RgbaImage) {
        let ci = to_color_image(img);
        match &mut self.handle {
            Some(h) => h.set(ci, TextureOptions::NEAREST),
            None => self.handle = Some(ctx.load_texture(name, ci, TextureOptions::NEAREST)),
        }
    }

    pub fn clear(&mut self) {
        self.handle = None;
    }
}

/// Pan/zoom state for 2D image views.
#[derive(Clone, Copy)]
pub struct ImageView {
    pub zoom: f32,
    pub offset: Vec2,
    pub checker: bool,
}

impl Default for ImageView {
    fn default() -> Self {
        Self {
            zoom: 0.0,
            offset: Vec2::ZERO,
            checker: true,
        }
    }
}

pub fn checkerboard(painter: &egui::Painter, rect: Rect) {
    let s = 10.0;
    painter.rect_filled(rect, 0.0, Color32::from_gray(70));
    let cols = (rect.width() / s).ceil() as i32;
    let rows = (rect.height() / s).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let r = Rect::from_min_size(
                    rect.min + Vec2::new(x as f32 * s, y as f32 * s),
                    Vec2::splat(s),
                )
                .intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(95));
            }
        }
    }
}

impl ImageView {
    /// Draw `tex` (of pixel size `img_size`) into the available space.
    /// Wheel zooms around the cursor, any drag pans, double-click fits.
    /// Returns the response and the on-screen image rect.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        tex: &TextureHandle,
        img_size: Vec2,
        size: Vec2,
    ) -> (Response, Rect) {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let fit = (rect.width() / img_size.x)
            .min(rect.height() / img_size.y)
            .max(0.01);
        if self.zoom <= 0.0 || resp.double_clicked() {
            // Integer zoom keeps pixels crisp when possible.
            self.zoom = if fit >= 1.0 { fit.floor() } else { fit };
            self.offset = Vec2::ZERO;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old = self.zoom;
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.05, 64.0);
                if let Some(p) = resp.hover_pos() {
                    let c = rect.center() + self.offset;
                    self.offset += (p - c) * (1.0 - self.zoom / old);
                }
            }
        }
        if resp.dragged() {
            self.offset += resp.drag_delta();
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        let shown = Rect::from_center_size(rect.center() + self.offset, img_size * self.zoom);
        if self.checker {
            checkerboard(&painter, shown.intersect(rect));
        }
        painter.image(
            tex.id(),
            shown,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        (resp, shown)
    }
}

pub fn swatches(ui: &mut Ui, palette: &[Rgb], size: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(2.0);
        for c in palette {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            ui.painter()
                .rect_filled(r, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
            resp.on_hover_text(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        }
    });
}

pub fn enum_combo<T: PartialEq + Copy>(
    ui: &mut Ui,
    id: &str,
    label: &str,
    v: &mut T,
    all: &[T],
    name: impl Fn(T) -> &'static str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(name(*v))
            .show_ui(ui, |ui| {
                for &o in all {
                    changed |= ui.selectable_value(v, o, name(o)).changed();
                }
            });
        ui.label(label);
    });
    changed
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(DIM));
}

pub fn empty_state(ui: &mut Ui, title: &str, body: &str) -> bool {
    let mut clicked = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(egui::RichText::new(title).heading().color(ACCENT2));
            ui.label(egui::RichText::new(body).color(DIM));
            ui.add_space(8.0);
            clicked = ui.button("Open file...").clicked();
        });
    });
    clicked
}
