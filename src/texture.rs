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

/// Normalised 8x8 Bayer matrix in [-0.5, 0.5): 64 levels, finer gradients than 4x4.
pub fn bayer8(x: u32, y: u32) -> f32 {
    // Bayer(2n) = [[4M, 4M+2], [4M+3, 4M+1]], unrolled over the three bit levels.
    let (mut v, mut x, mut y) = (0u32, x % 8, y % 8);
    for _ in 0..3 {
        let q = match (x & 1, y & 1) {
            (0, 0) => 0,
            (1, 0) => 2,
            (0, 1) => 3,
            _ => 1,
        };
        v = v * 4 + q;
        x >>= 1;
        y >>= 1;
    }
    // The lowest coordinate bit picks the coarsest level (most significant digit), so
    // neighbouring texels are as far apart as possible.
    v as f32 / 64.0 - 0.5
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
    /// 8x8 ordered matrix: smoother gradients, finer pattern.
    Bayer8,
    Ps1Table,
    /// Error diffusion (serpentine Floyd-Steinberg): the most faithful colours with a
    /// fixed palette, but a noisier pattern that doesn't tile seamlessly.
    FloydSteinberg,
}

impl DitherMode {
    pub const ALL: [DitherMode; 5] = [
        Self::None,
        Self::Bayer4,
        Self::Bayer8,
        Self::Ps1Table,
        Self::FloydSteinberg,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::Bayer4 => "Bayer 4x4",
            Self::Bayer8 => "Bayer 8x8",
            Self::Ps1Table => "PS1 GPU table",
            Self::FloydSteinberg => "Floyd-Steinberg (error diffusion)",
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
#[serde(default)]
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
    /// With a palette: mixing strength between the two nearest entries
    /// (`PALETTE_DITHER_FULL` = full). Without (RGB555 only): offset in 8-bit units.
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
        for (ch, v) in p.0.iter_mut().zip(c) {
            *ch = (v * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

/// Ordered-dither threshold in 0..1 for palette mixing (None = no dithering).
fn dither_threshold(mode: DitherMode, x: u32, y: u32) -> Option<f32> {
    match mode {
        DitherMode::None => None,
        DitherMode::Bayer4 => Some(bayer4(x, y) + 0.5 + 1.0 / 32.0),
        DitherMode::Bayer8 => Some(bayer8(x, y) + 0.5 + 1.0 / 128.0),
        DitherMode::FloydSteinberg => None,
        DitherMode::Ps1Table => {
            Some((PS1_DITHER[(y % 4) as usize][(x % 4) as usize] as f32 + 4.5) / 8.0)
        }
    }
}

/// With a palette, `dither_amount` 16 means full mixing between neighbouring entries.
pub const PALETTE_DITHER_FULL: f32 = 16.0;

fn dither_offset(mode: DitherMode, amount: f32, x: u32, y: u32) -> f32 {
    match mode {
        DitherMode::None => 0.0,
        DitherMode::Bayer4 => bayer4(x, y) * amount,
        DitherMode::Bayer8 => bayer8(x, y) * amount,
        DitherMode::FloydSteinberg => 0.0,
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

/// Whether quantising `img` with `s` leaves cutout (fully transparent) texels, which
/// need their own CLUT entry.
pub fn has_cutout_texels(img: &RgbaImage, s: &CrunchSettings) -> bool {
    s.alpha == AlphaMode::Cutout && img.pixels().any(|p| p.0[3] < s.alpha_threshold)
}

/// Build the palette that `crunch` would use for this (already resized) image:
/// the colours of the opaque `pixels`. With `reserve_transparent`, one CLUT slot is left
/// free for the transparent entry, so a 16-colour cutout texture still fits 4 bits.
pub fn build_palette(
    pixels: &[Rgb],
    s: &CrunchSettings,
    imported: Option<&[Rgb]>,
    reserve_transparent: bool,
) -> Vec<Rgb> {
    let colors = (s.colors - usize::from(reserve_transparent)).clamp(2, 256);
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
        (PaletteMode::ImportedBestN, Some(p)) => palette::best_n_from(pixels, p, colors),
        // Imported mode without a palette falls back to auto.
        _ => snap(palette::kmeans_palette(pixels, colors, 12)),
    }
}

/// Map an image onto a palette with ordered dithering. Shared between textures and sprites.
pub fn quantize(img: &RgbaImage, s: &CrunchSettings, pal: &[Rgb]) -> RgbaImage {
    let lut = if pal.is_empty() {
        None
    } else {
        Some(PaletteLut::new(pal))
    };
    if s.dither == DitherMode::FloydSteinberg {
        return error_diffusion(img, s, pal, lut.as_ref());
    }
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
        let q = match &lut {
            // Palette: mix the two nearest entries with the ordered matrix. Adding a
            // fixed offset instead barely dithers sparse palettes and adds noise to dense ones.
            Some(l) => {
                let c = [p.0[0], p.0[1], p.0[2]];
                let i = match dither_threshold(s.dither, x, y) {
                    Some(t) => l.lookup_mixed(c, t, s.dither_amount / PALETTE_DITHER_FULL),
                    None => l.lookup(c),
                };
                pal[i as usize]
            }
            // No palette (RGB555 only): the PS1 GPU's way, an offset before truncation.
            None => {
                let off = dither_offset(s.dither, s.dither_amount, x, y);
                let c = [
                    (p.0[0] as f32 + off).clamp(0.0, 255.0) as u8,
                    (p.0[1] as f32 + off).clamp(0.0, 255.0) as u8,
                    (p.0[2] as f32 + off).clamp(0.0, 255.0) as u8,
                ];
                if s.rgb555 { palette::snap_rgb555(c) } else { c }
            }
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
    let pal = build_palette(&pixels, s, imported, has_cutout_texels(&img, s));
    let image = quantize(&img, s, &pal);
    let has_transparency = image.pixels().any(|p| p.0[3] < 255);
    Crunched {
        image,
        palette: pal,
        has_transparency,
    }
}

/// Serpentine Floyd-Steinberg: each texel's quantisation error is spread to its
/// unvisited neighbours (7/16 ahead, 3/16 behind below, 5/16 below, 1/16 ahead below),
/// rows alternating direction so the error doesn't drift sideways. The error is kept in
/// linear light, where neighbouring texels actually blend on screen, so areas average
/// out to the right colour (sRGB-space diffusion comes out too dark or too bright).
/// `dither_amount` / `PALETTE_DITHER_FULL` scales the error (1 = full). Without a
/// palette, the target is RGB555 (or 8-bit) like the other modes.
fn error_diffusion(
    img: &RgbaImage,
    s: &CrunchSettings,
    pal: &[Rgb],
    lut: Option<&PaletteLut>,
) -> RgbaImage {
    let (w, h) = img.dimensions();
    let factor = (s.dither_amount / PALETTE_DITHER_FULL).clamp(0.0, 1.0);
    let mut out = img.clone();
    // Error buffers for this row and the next, with a 1-texel margin on each side.
    let mut cur = vec![[0f32; 3]; w as usize + 2];
    let mut next = vec![[0f32; 3]; w as usize + 2];
    for y in 0..h {
        let ltr = y % 2 == 0;
        for i in 0..w {
            let x = if ltr { i } else { w - 1 - i };
            let p = out.get_pixel_mut(x, y);
            match s.alpha {
                AlphaMode::Opaque => p.0[3] = 255,
                AlphaMode::Cutout => {
                    if p.0[3] < s.alpha_threshold {
                        p.0 = [0, 0, 0, 0];
                        continue;
                    }
                    p.0[3] = 255;
                }
                AlphaMode::Keep => {}
            }
            let e = cur[x as usize + 1];
            let src = palette::to_linear([p.0[0], p.0[1], p.0[2]]);
            let want = [0, 1, 2].map(|k| (src[k] + e[k]).clamp(0.0, 1.0));
            let c = palette::linear_to_srgb8(want);
            let q = match lut {
                Some(l) => pal[l.lookup(c) as usize],
                None if s.rgb555 => palette::snap_rgb555(c),
                None => c,
            };
            p.0[0] = q[0];
            p.0[1] = q[1];
            p.0[2] = q[2];
            let got = palette::to_linear(q);
            let err = [0, 1, 2].map(|k| (want[k] - got[k]) * factor);
            // Neighbour offsets in the scan direction.
            let (ahead, behind) = if ltr { (2usize, 0usize) } else { (0, 2) };
            let xi = x as usize;
            for k in 0..3 {
                cur[xi + ahead][k] += err[k] * 7.0 / 16.0;
                next[xi + behind][k] += err[k] * 3.0 / 16.0;
                next[xi + 1][k] += err[k] * 5.0 / 16.0;
                next[xi + ahead][k] += err[k] * 1.0 / 16.0;
            }
        }
        std::mem::swap(&mut cur, &mut next);
        next.iter_mut().for_each(|e| *e = [0.0; 3]);
    }
    out
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
    // Transparent goes first: index 0 is the PS1 convention (and keeps tRNS short).
    let mut pal: Vec<[u8; 4]> = if img.pixels().any(|p| p.0[3] == 0) {
        vec![[0, 0, 0, 0]]
    } else {
        Vec::new()
    };
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
    fn cutout_16_colours_fits_a_4bit_clut_with_transparent_first() {
        let mut img = gradient(64, 64);
        for (x, _, p) in img.enumerate_pixels_mut() {
            if x < 8 {
                p.0[3] = 0;
            }
        }
        let s = CrunchSettings {
            max_size: 64,
            colors: 16,
            alpha: AlphaMode::Cutout,
            dither: DitherMode::None,
            ..Default::default()
        };
        let c = crunch(&img, &s, &BakeMaps::default(), None);
        assert!(c.palette.len() <= 15, "palette {}", c.palette.len());
        let (pal, idx) = to_indexed(&c.image).unwrap();
        assert!(pal.len() <= 16, "CLUT {}", pal.len());
        assert_eq!(pal[0], [0, 0, 0, 0]);
        assert_eq!(idx[0], 0, "transparent texel uses index 0");
    }

    #[test]
    fn palette_dither_mixes_two_nearest_by_position() {
        // A flat colour a quarter of the way from black to white (in linear light).
        let grey = {
            let l = 0.25f32;
            let v = 1.055 * l.powf(1.0 / 2.4) - 0.055;
            (v * 255.0).round() as u8
        };
        let img = RgbaImage::from_pixel(16, 16, image::Rgba([grey, grey, grey, 255]));
        let pal = vec![[0, 0, 0], [255, 255, 255]];
        let count_white = |s: &CrunchSettings| {
            quantize(&img, s, &pal)
                .pixels()
                .filter(|p| p.0[0] == 255)
                .count()
        };
        let mut s = CrunchSettings {
            dither: DitherMode::Bayer4,
            dither_amount: PALETTE_DITHER_FULL,
            alpha: AlphaMode::Opaque,
            ..Default::default()
        };
        let white = count_white(&s);
        assert!((56..=72).contains(&white), "~25% white, got {white}/256");
        s.dither = DitherMode::None;
        assert_eq!(
            count_white(&s),
            256,
            "no dither: plain nearest (perceptually white)"
        );
        s.dither = DitherMode::Ps1Table;
        let ps1 = count_white(&s);
        assert!((32..=96).contains(&ps1), "PS1 table mixes too, got {ps1}");
    }

    #[test]
    fn bayer8_has_every_level_once() {
        let mut seen = [false; 64];
        for y in 0..8 {
            for x in 0..8 {
                let v = ((bayer8(x, y) + 0.5) * 64.0).round() as usize;
                assert!(!seen[v], "duplicate level {v}");
                seen[v] = true;
            }
        }
        // Neighbours differ a lot (dispersed pattern): 0 and 32 sit diagonally.
        assert_eq!(((bayer8(0, 0) + 0.5) * 64.0).round() as i32, 0);
        assert_eq!(((bayer8(1, 1) + 0.5) * 64.0).round() as i32, 16);
    }

    #[test]
    fn dithering_reproduces_colours_between_palette_entries() {
        // Purple between red and blue, with a misleading dark red and a green in the
        // palette. A flat area must average out near the target.
        let target = [128u8, 0, 128];
        let img =
            RgbaImage::from_pixel(32, 32, image::Rgba([target[0], target[1], target[2], 255]));
        let pal = vec![[255, 0, 0], [0, 0, 255], [120, 0, 0], [0, 255, 0]];
        let lin = |c: [u8; 3]| palette::to_linear(c);
        let t = lin(target);
        let err = |img: &RgbaImage| {
            let mut m = [0f32; 3];
            for p in img.pixels() {
                let l = lin([p.0[0], p.0[1], p.0[2]]);
                for k in 0..3 {
                    m[k] += l[k] / (32.0 * 32.0);
                }
            }
            (0..3).map(|k| (m[k] - t[k]).abs()).fold(0.0f32, f32::max)
        };
        let nearest_only = pal
            .iter()
            .map(|&c| {
                (0..3)
                    .map(|k| (lin(c)[k] - t[k]).abs())
                    .fold(0.0f32, f32::max)
            })
            .fold(f32::MAX, f32::min);
        for dither in [DitherMode::Bayer8, DitherMode::FloydSteinberg] {
            let s = CrunchSettings {
                dither,
                dither_amount: PALETTE_DITHER_FULL,
                alpha: AlphaMode::Opaque,
                ..Default::default()
            };
            let out = quantize(&img, &s, &pal);
            assert!(count_colors(&out) >= 2, "{dither:?} mixes colours");
            assert!(
                err(&out) < nearest_only * 0.6,
                "{dither:?}: mean error {} vs best single colour {}",
                err(&out),
                nearest_only
            );
            assert!(
                out.pixels()
                    .all(|p| pal.contains(&[p.0[0], p.0[1], p.0[2]]))
            );
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
