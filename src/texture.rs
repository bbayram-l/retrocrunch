//! PS1-style texture crunching: resize, bake maps into albedo, colour adjust,
//! palette reduction (k-means or imported), RGB555 snap and ordered dithering.

use crate::palette::{self, PaletteLut, Rgb};
use anyhow::{Context, Result};
use image::{GrayImage, RgbaImage, imageops::FilterType};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The PlayStation GPU's 4x4 dither table (values in 8-bit units).
pub const PS1_DITHER: [[i32; 4]; 4] = [
    [-4, 0, -3, 1],
    [2, -2, 3, -1],
    [-3, 1, -4, 0],
    [3, -1, 2, -2],
];

/// Normalised 4x4 Bayer matrix in [-0.5, 0.5).
pub fn bayer4(x: u32, y: u32) -> f32 {
    const M: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    M[(y % 4) as usize][(x % 4) as usize] as f32 / 16.0 - 0.5
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResizeFilter {
    Nearest,
    Box,
    Smooth,
    Sharp,
}

impl ResizeFilter {
    pub const ALL: [ResizeFilter; 4] = [Self::Nearest, Self::Box, Self::Smooth, Self::Sharp];
    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest",
            Self::Box => "Box (average)",
            Self::Smooth => "Smooth (triangle)",
            Self::Sharp => "Sharp (Lanczos)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaletteMode {
    /// Generate a palette with k-means.
    Auto,
    /// Use every entry of the imported palette (shared CLUT).
    ImportedAll,
    /// Use the best N entries of the imported palette.
    ImportedBestN,
    /// No palette: just snap to RGB555 (32k colours).
    Rgb555Only,
}

impl PaletteMode {
    pub const ALL: [PaletteMode; 4] = [
        Self::Auto,
        Self::ImportedAll,
        Self::ImportedBestN,
        Self::Rgb555Only,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (k-means)",
            Self::ImportedAll => "Imported: all colours",
            Self::ImportedBestN => "Imported: best N",
            Self::Rgb555Only => "15-bit only (no CLUT)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DitherMode {
    None,
    Bayer4,
    Ps1Table,
}

impl DitherMode {
    pub const ALL: [DitherMode; 3] = [Self::None, Self::Bayer4, Self::Ps1Table];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Bayer4 => "Bayer 4x4",
            Self::Ps1Table => "PS1 GPU table",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    Opaque,
    /// Binary transparency (PS1 STP-style): alpha below threshold becomes fully clear.
    Cutout,
    /// Keep full alpha (exported as RGBA).
    Keep,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrunchSettings {
    /// Longest side in pixels (4..=2048). Non-square textures keep aspect.
    pub max_size: u32,
    pub power_of_two: bool,
    pub filter: ResizeFilter,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub colors: usize,
    pub palette_mode: PaletteMode,
    pub rgb555: bool,
    pub dither: DitherMode,
    /// Dither amplitude in 8-bit units (PS1 table is +-4; palettes need more).
    pub dither_amount: f32,
    pub alpha: AlphaMode,
    pub alpha_threshold: u8,
}

impl Default for CrunchSettings {
    fn default() -> Self {
        Self {
            max_size: 128,
            power_of_two: true,
            filter: ResizeFilter::Box,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            colors: 16,
            palette_mode: PaletteMode::Auto,
            rgb555: true,
            dither: DitherMode::Bayer4,
            dither_amount: 12.0,
            alpha: AlphaMode::Cutout,
            alpha_threshold: 128,
        }
    }
}

/// Optional maps baked into the albedo before quantisation.
#[derive(Default)]
pub struct BakeMaps<'a> {
    pub ao: Option<&'a GrayImage>,
    pub ao_strength: f32,
    /// Tangent-space normal map; its relief is baked as fixed top-left lighting.
    pub normal: Option<&'a RgbaImage>,
    pub normal_strength: f32,
    pub emissive: Option<&'a RgbaImage>,
}

pub struct Crunched {
    pub image: RgbaImage,
    /// Palette used (empty when `Rgb555Only`). Index 0 is NOT reserved; transparency
    /// is tracked separately via `has_transparency`.
    pub palette: Vec<Rgb>,
    pub has_transparency: bool,
}

pub fn target_dims(w: u32, h: u32, max_size: u32, pot: bool) -> (u32, u32) {
    let max_size = max_size.clamp(4, 2048);
    let scale = max_size as f32 / w.max(h).max(1) as f32;
    let mut tw = ((w as f32 * scale).round() as u32).max(1);
    let mut th = ((h as f32 * scale).round() as u32).max(1);
    if pot {
        tw = tw.next_power_of_two().min(max_size.next_power_of_two());
        th = th.next_power_of_two().min(max_size.next_power_of_two());
        if tw > max_size {
            tw /= 2;
        }
        if th > max_size {
            th /= 2;
        }
    }
    (tw.max(1), th.max(1))
}

pub fn resize(img: &RgbaImage, w: u32, h: u32, filter: ResizeFilter) -> RgbaImage {
    if img.width() == w && img.height() == h {
        return img.clone();
    }
    match filter {
        ResizeFilter::Nearest => image::imageops::resize(img, w, h, FilterType::Nearest),
        ResizeFilter::Smooth => image::imageops::resize(img, w, h, FilterType::Triangle),
        ResizeFilter::Sharp => image::imageops::resize(img, w, h, FilterType::Lanczos3),
        ResizeFilter::Box => box_downscale(img, w, h),
    }
}

/// Area-average downscale, alpha-weighted so transparent texels don't darken edges.
fn box_downscale(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = img.dimensions();
    if w > sw || h > sh {
        return image::imageops::resize(img, w, h, FilterType::Nearest);
    }
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        let y0 = y * sh / h;
        let y1 = ((y + 1) * sh / h).max(y0 + 1);
        for x in 0..w {
            let x0 = x * sw / w;
            let x1 = ((x + 1) * sw / w).max(x0 + 1);
            let mut acc = [0f32; 4];
            let mut n = 0f32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = img.get_pixel(sx, sy).0;
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a;
                    acc[1] += p[1] as f32 * a;
                    acc[2] += p[2] as f32 * a;
                    acc[3] += a;
                    n += 1.0;
                }
            }
            let p = if acc[3] > 0.0 {
                [
                    (acc[0] / acc[3]) as u8,
                    (acc[1] / acc[3]) as u8,
                    (acc[2] / acc[3]) as u8,
                    (acc[3] / n * 255.0).round() as u8,
                ]
            } else {
                [0, 0, 0, 0]
            };
            out.put_pixel(x, y, image::Rgba(p));
        }
    }
    out
}

/// Multiply AO, add normal relief and emission at the target resolution.
pub fn bake_maps(img: &mut RgbaImage, maps: &BakeMaps) {
    let (w, h) = img.dimensions();
    let ao = maps
        .ao
        .map(|a| image::imageops::resize(a, w, h, FilterType::Triangle));
    let nrm = maps
        .normal
        .map(|n| image::imageops::resize(n, w, h, FilterType::Triangle));
    let emi = maps
        .emissive
        .map(|e| image::imageops::resize(e, w, h, FilterType::Triangle));
    // Light from top-left-front, matching the classic "baked" look.
    let l = glam::Vec3::new(-0.4, 0.5, 0.77).normalize();
    for (x, y, p) in img.enumerate_pixels_mut() {
        let mut c = [p.0[0] as f32, p.0[1] as f32, p.0[2] as f32];
        if let Some(ao) = &ao {
            let a = ao.get_pixel(x, y).0[0] as f32 / 255.0;
            let f = 1.0 - maps.ao_strength * (1.0 - a);
            c.iter_mut().for_each(|v| *v *= f);
        }
        if let Some(n) = &nrm {
            let q = n.get_pixel(x, y).0;
            let nv = glam::Vec3::new(
                q[0] as f32 / 127.5 - 1.0,
                q[1] as f32 / 127.5 - 1.0,
                q[2] as f32 / 127.5 - 1.0,
            )
            .normalize_or_zero();
            // Relative shading: flat normal -> factor 1.
            let flat = l.z;
            let f = 1.0 + maps.normal_strength * (nv.dot(l) - flat);
            c.iter_mut().for_each(|v| *v *= f.max(0.0));
        }
        if let Some(e) = &emi {
            let q = e.get_pixel(x, y).0;
            c[0] += q[0] as f32;
            c[1] += q[1] as f32;
            c[2] += q[2] as f32;
        }
        p.0[0] = c[0].clamp(0.0, 255.0) as u8;
        p.0[1] = c[1].clamp(0.0, 255.0) as u8;
        p.0[2] = c[2].clamp(0.0, 255.0) as u8;
    }
}

pub fn adjust(img: &mut RgbaImage, brightness: f32, contrast: f32, saturation: f32) {
    if brightness == 0.0 && contrast == 0.0 && saturation == 0.0 {
        return;
    }
    let cf = 1.0 + contrast;
    let sf = 1.0 + saturation;
    for p in img.pixels_mut() {
        let mut c = [
            p.0[0] as f32 / 255.0,
            p.0[1] as f32 / 255.0,
            p.0[2] as f32 / 255.0,
        ];
        let luma = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        for v in &mut c {
            *v = luma + (*v - luma) * sf;
            *v = (*v - 0.5) * cf + 0.5 + brightness;
        }
        for i in 0..3 {
            p.0[i] = (c[i] * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

fn dither_offset(mode: DitherMode, amount: f32, x: u32, y: u32) -> f32 {
    match mode {
        DitherMode::None => 0.0,
        DitherMode::Bayer4 => bayer4(x, y) * amount,
        DitherMode::Ps1Table => {
            PS1_DITHER[(y % 4) as usize][(x % 4) as usize] as f32 * amount / 8.0
        }
    }
}

fn opaque_pixels(img: &RgbaImage, threshold: u8) -> Vec<Rgb> {
    img.pixels()
        .filter(|p| p.0[3] >= threshold)
        .map(|p| [p.0[0], p.0[1], p.0[2]])
        .collect()
}

/// Build the palette that `crunch` would use for this (already resized) image.
pub fn build_palette(pixels: &[Rgb], s: &CrunchSettings, imported: Option<&[Rgb]>) -> Vec<Rgb> {
    let snap = |p: Vec<Rgb>| -> Vec<Rgb> {
        if s.rgb555 {
            palette::dedup_keep_order(&p.into_iter().map(palette::snap_rgb555).collect::<Vec<_>>())
        } else {
            p
        }
    };
    match (s.palette_mode, imported) {
        (PaletteMode::Rgb555Only, _) => Vec::new(),
        (PaletteMode::ImportedAll, Some(p)) => p.to_vec(),
        (PaletteMode::ImportedBestN, Some(p)) => palette::best_n_from(pixels, p, s.colors),
        // Imported mode without a palette falls back to auto.
        _ => snap(palette::kmeans_palette(pixels, s.colors.clamp(2, 256), 12)),
    }
}

/// Map an image onto a palette with ordered dithering. Shared between textures and sprites.
pub fn quantize(img: &RgbaImage, s: &CrunchSettings, pal: &[Rgb]) -> RgbaImage {
    let lut = if pal.is_empty() {
        None
    } else {
        Some(PaletteLut::new(pal))
    };
    let mut out = img.clone();
    for (x, y, p) in out.enumerate_pixels_mut() {
        let a = p.0[3];
        match s.alpha {
            AlphaMode::Opaque => p.0[3] = 255,
            AlphaMode::Cutout => {
                if a < s.alpha_threshold {
                    p.0 = [0, 0, 0, 0];
                    continue;
                }
                p.0[3] = 255;
            }
            AlphaMode::Keep => {}
        }
        let off = dither_offset(s.dither, s.dither_amount, x, y);
        let c = [
            (p.0[0] as f32 + off).clamp(0.0, 255.0) as u8,
            (p.0[1] as f32 + off).clamp(0.0, 255.0) as u8,
            (p.0[2] as f32 + off).clamp(0.0, 255.0) as u8,
        ];
        let q = match &lut {
            Some(l) => pal[l.lookup(c) as usize],
            None if s.rgb555 => palette::snap_rgb555(c),
            None => c,
        };
        p.0[0] = q[0];
        p.0[1] = q[1];
        p.0[2] = q[2];
    }
    out
}

/// Full pipeline: resize -> bake -> adjust -> palette -> dither/map.
pub fn crunch(
    src: &RgbaImage,
    s: &CrunchSettings,
    maps: &BakeMaps,
    imported: Option<&[Rgb]>,
) -> Crunched {
    let (tw, th) = target_dims(src.width(), src.height(), s.max_size, s.power_of_two);
    let mut img = resize(src, tw, th, s.filter);
    bake_maps(&mut img, maps);
    adjust(&mut img, s.brightness, s.contrast, s.saturation);
    let thr = if s.alpha == AlphaMode::Cutout {
        s.alpha_threshold
    } else {
        1
    };
    let pixels = opaque_pixels(&img, thr);
    let pal = build_palette(&pixels, s, imported);
    let image = quantize(&img, s, &pal);
    let has_transparency = image.pixels().any(|p| p.0[3] < 255);
    Crunched {
        image,
        palette: pal,
        has_transparency,
    }
}

/// Number of distinct RGBA colours in an image (for stats).
pub fn count_colors(img: &RgbaImage) -> usize {
    let mut v: Vec<u32> = img.pixels().map(|p| u32::from_le_bytes(p.0)).collect();
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// Write an indexed (CLUT) PNG when the image has <= 256 colours, otherwise RGBA.
/// Fully transparent texels share one palette slot marked transparent via tRNS.
pub fn save_png(path: &Path, img: &RgbaImage, indexed: bool) -> Result<()> {
    if indexed && let Some((pal, idx)) = to_indexed(img) {
        let file =
            std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width(), img.height());
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(
            pal.iter()
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect::<Vec<u8>>(),
        );
        if pal.iter().any(|c| c[3] < 255) {
            enc.set_trns(pal.iter().map(|c| c[3]).collect::<Vec<u8>>());
        }
        let mut w = enc.write_header()?;
        w.write_image_data(&idx)?;
        return Ok(());
    }
    img.save(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn to_indexed(img: &RgbaImage) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let mut pal: Vec<[u8; 4]> = Vec::new();
    let mut idx = Vec::with_capacity((img.width() * img.height()) as usize);
    for p in img.pixels() {
        let c = if p.0[3] == 0 { [0, 0, 0, 0] } else { p.0 };
        let i = match pal.iter().position(|&q| q == c) {
            Some(i) => i,
            None => {
                if pal.len() == 256 {
                    return None;
                }
                pal.push(c);
                pal.len() - 1
            }
        };
        idx.push(i as u8);
    }
    // tRNS entries must come first for minimal size; not required for correctness.
    Some((pal, idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255])
        })
    }

    #[test]
    fn crunch_respects_color_budget_and_size() {
        let img = gradient(512, 256);
        let s = CrunchSettings {
            colors: 8,
            ..Default::default()
        };
        let out = crunch(&img, &s, &BakeMaps::default(), None);
        assert_eq!(out.image.dimensions(), (128, 64));
        assert!(count_colors(&out.image) <= 8);
    }

    #[test]
    fn cutout_alpha_is_binary() {
        let mut img = gradient(32, 32);
        for (x, _, p) in img.enumerate_pixels_mut() {
            p.0[3] = (x * 8) as u8;
        }
        let s = CrunchSettings {
            max_size: 32,
            alpha: AlphaMode::Cutout,
            ..Default::default()
        };
        let out = crunch(&img, &s, &BakeMaps::default(), None);
        assert!(out.image.pixels().all(|p| p.0[3] == 0 || p.0[3] == 255));
        assert!(out.has_transparency);
    }

    #[test]
    fn imported_all_uses_only_palette_colours() {
        let img = gradient(64, 64);
        let pal = vec![[0, 0, 0], [255, 255, 255], [255, 0, 0]];
        let s = CrunchSettings {
            max_size: 64,
            palette_mode: PaletteMode::ImportedAll,
            ..Default::default()
        };
        let out = crunch(&img, &s, &BakeMaps::default(), Some(&pal));
        for p in out.image.pixels() {
            assert!(pal.contains(&[p.0[0], p.0[1], p.0[2]]));
        }
    }

    #[test]
    fn target_dims_power_of_two() {
        assert_eq!(target_dims(1000, 500, 128, true), (128, 64));
        assert_eq!(target_dims(300, 300, 100, true), (64, 64));
        assert_eq!(target_dims(300, 300, 100, false), (100, 100));
    }

    #[test]
    fn indexed_png_roundtrip() {
        let img = gradient(16, 16);
        let s = CrunchSettings {
            max_size: 16,
            colors: 4,
            ..Default::default()
        };
        let out = crunch(&img, &s, &BakeMaps::default(), None);
        let p = std::env::temp_dir().join("psxforge_idx.png");
        save_png(&p, &out.image, true).unwrap();
        let back = image::open(&p).unwrap().to_rgba8();
        assert_eq!(back, out.image);
        let _ = std::fs::remove_file(p);
    }
}
