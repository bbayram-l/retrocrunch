//! PS1 VRAM budget (experimental): how much of the console's 1 MB of video memory
//! (1024x512 16-bit words) a model's textures and colour tables would take, and
//! whether they fit the GPU's 256x256 texture pages.

use serde::Serialize;

/// VRAM size in 16-bit words.
pub const VRAM_WORDS: u32 = 1024 * 512;
/// Two 320x240 15-bit framebuffers (double buffering): the usual fixed cost.
pub const FRAMEBUFFER_WORDS: u32 = 2 * 320 * 240;
/// Largest texture one texture page can address.
pub const PAGE_TEXELS: u32 = 256;

#[derive(Clone, Debug, Serialize)]
pub struct TextureUse {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub colors: usize,
    /// 4 or 8 (CLUT) or 16 (direct colour).
    pub bits: u8,
    /// Texture data in 16-bit words (4-bit texels pack 4 per word, 8-bit 2).
    pub words: u32,
    /// Colour table (CLUT) words: 16 or 256, 0 for direct colour.
    pub clut_words: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VramReport {
    pub textures: Vec<TextureUse>,
    /// Texture + CLUT words.
    pub total_words: u32,
    pub total_bytes: u32,
    /// Share of the VRAM left after two 320x240 framebuffers.
    pub percent_of_free: f32,
    pub warnings: Vec<String>,
}

/// Estimate for textures given as (name, width, height, colours used).
pub fn estimate(textures: &[(String, u32, u32, usize)]) -> VramReport {
    let mut r = VramReport::default();
    for (name, w, h, colors) in textures {
        let (bits, clut) = match colors {
            0..=16 => (4u8, 16u32),
            17..=256 => (8, 256),
            _ => (16, 0),
        };
        let words = (w * bits as u32).div_ceil(16) * h;
        if *w > PAGE_TEXELS || *h > PAGE_TEXELS {
            r.warnings.push(format!(
                "{name}: {w}x{h} is larger than a 256x256 texture page; the PS1 would need it split"
            ));
        }
        if bits == 16 {
            r.warnings.push(format!(
                "{name}: {colors} colours means 16-bit direct colour, 4x the VRAM of a 4-bit texture"
            ));
        }
        r.total_words += words + clut;
        r.textures.push(TextureUse {
            name: name.clone(),
            width: *w,
            height: *h,
            colors: *colors,
            bits,
            words,
            clut_words: clut,
        });
    }
    r.total_bytes = r.total_words * 2;
    let free = VRAM_WORDS - FRAMEBUFFER_WORDS;
    r.percent_of_free = r.total_words as f32 / free as f32 * 100.0;
    if r.total_words > free {
        r.warnings
            .push("Doesn't fit in VRAM next to two 320x240 framebuffers".into());
    }
    r
}

impl VramReport {
    /// One-line summary, e.g. "VRAM 4.1 KB (1.1% of free) · 4-bit 128x128".
    pub fn summary(&self) -> String {
        let kinds: Vec<String> = self
            .textures
            .iter()
            .map(|t| format!("{}-bit {}x{}", t.bits, t.width, t.height))
            .collect();
        format!(
            "VRAM {:.1} KB ({:.1}% of free){}{}",
            self.total_bytes as f32 / 1024.0,
            self.percent_of_free,
            if kinds.is_empty() { "" } else { " · " },
            kinds.join(", ")
        )
    }
}

// ------------------------------------------------------------------ layout (packer)

/// One rectangle in VRAM, in 16-bit words (x, width) and lines (y, height).
#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub name: String,
    /// "framebuffer", "texture" or "clut".
    pub kind: &'static str,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Texture page base (x, y) the texture is addressed from (textures only).
    pub page: Option<(u32, u32)>,
    pub bits: u8,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VramLayout {
    pub placements: Vec<Placement>,
    /// Textures or CLUTs that found no room.
    pub unplaced: Vec<String>,
}

/// Width in words a texture page addresses for a bit depth (256 texels).
fn page_span(bits: u8) -> u32 {
    match bits {
        4 => 64,
        8 => 128,
        _ => 256,
    }
}

/// Place textures (name, width, height, colours) like a PS1 game would: two 320x240
/// framebuffers on the left, textures in the rest without crossing a texture page,
/// CLUTs in the free rows below the framebuffers (16-word aligned).
pub fn layout(textures: &[(String, u32, u32, usize)]) -> VramLayout {
    let est = estimate(textures);
    let mut out = VramLayout::default();
    for (i, name) in ["framebuffer A", "framebuffer B"].iter().enumerate() {
        out.placements.push(Placement {
            name: name.to_string(),
            kind: "framebuffer",
            x: 0,
            y: i as u32 * 240,
            w: 320,
            h: 240,
            page: None,
            bits: 16,
        });
    }
    // Free rectangles for textures (guillotine): right of the framebuffers.
    let mut free: Vec<(u32, u32, u32, u32)> = vec![(320, 0, 1024 - 320, 512)];
    let mut order: Vec<&TextureUse> = est.textures.iter().collect();
    order.sort_by_key(|t| std::cmp::Reverse((t.height, t.words)));
    for t in &order {
        let w = t.words / t.height.max(1);
        let h = t.height;
        let span = page_span(t.bits);
        // First free rectangle (top-left first) where the texture fits inside one
        // texture page window, trying page-aligned x positions inside it.
        let mut spot = None;
        let mut cands: Vec<usize> = (0..free.len()).collect();
        cands.sort_by_key(|&i| (free[i].1, free[i].0));
        'search: for i in cands {
            let (fx, fy, fw, fh) = free[i];
            if w > fw || h > fh || w > span || h > 256 {
                continue;
            }
            let mut x = fx;
            while x + w <= fx + fw {
                let px = x / 64 * 64;
                let py = fy / 256 * 256;
                if x + w <= px + span && fy + h <= py + 256 {
                    spot = Some((i, x, fy, (px, py)));
                    break 'search;
                }
                // Next page column.
                x = (x / 64 + 1) * 64;
            }
        }
        match spot {
            Some((i, x, y, page)) => {
                let (fx, fy, fw, fh) = free.remove(i);
                // Split the rest of the free rectangle: left gap, right part, below.
                if x > fx {
                    free.push((fx, fy, x - fx, h));
                }
                if fx + fw > x + w {
                    free.push((x + w, fy, fx + fw - (x + w), h));
                }
                if fh > h {
                    free.push((fx, fy + h, fw, fh - h));
                }
                out.placements.push(Placement {
                    name: t.name.clone(),
                    kind: "texture",
                    x,
                    y,
                    w,
                    h,
                    page: Some(page),
                    bits: t.bits,
                });
            }
            None => out.unplaced.push(t.name.clone()),
        }
    }
    // CLUTs: rows 480..512 under the framebuffers, 16-word aligned, one row each.
    let (mut cx, mut cy) = (0u32, 480u32);
    for t in est.textures.iter().filter(|t| t.clut_words > 0) {
        let w = t.clut_words;
        if cx + w > 320 {
            cx = 0;
            cy += 1;
        }
        if cy >= 512 {
            out.unplaced.push(format!("{} CLUT", t.name));
            continue;
        }
        out.placements.push(Placement {
            name: format!("{} CLUT", t.name),
            kind: "clut",
            x: cx,
            y: cy,
            w,
            h: 1,
            page: None,
            bits: 16,
        });
        cx += w.div_ceil(16) * 16;
    }
    out
}

/// A 1024x512 picture of the layout: framebuffers grey, textures coloured with a white
/// outline, CLUTs bright yellow, texture-page grid faint.
pub fn layout_image(l: &VramLayout) -> image::RgbaImage {
    let mut img = image::RgbaImage::from_pixel(1024, 512, image::Rgba([16, 16, 22, 255]));
    for y in 0..512 {
        for x in 0..1024 {
            if x % 64 == 0 || y % 256 == 0 {
                img.put_pixel(x, y, image::Rgba([40, 40, 52, 255]));
            }
        }
    }
    let mut fill = |p: &Placement, c: [u8; 3], border: [u8; 3]| {
        for y in p.y..(p.y + p.h).min(512) {
            for x in p.x..(p.x + p.w).min(1024) {
                let edge = y == p.y || y + 1 == p.y + p.h || x == p.x || x + 1 == p.x + p.w;
                let c = if edge && p.h > 2 { border } else { c };
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    };
    let hues: [[u8; 3]; 6] = [
        [200, 90, 70],
        [80, 170, 210],
        [120, 200, 90],
        [210, 170, 60],
        [170, 110, 210],
        [70, 200, 170],
    ];
    let mut k = 0;
    for p in &l.placements {
        match p.kind {
            "framebuffer" => fill(p, [70, 70, 80], [110, 110, 125]),
            "clut" => fill(p, [250, 240, 120], [250, 240, 120]),
            _ => {
                let c = hues[k % hues.len()];
                k += 1;
                fill(p, c, [255, 255, 255]);
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_texels_by_bit_depth_and_warns_past_hardware_limits() {
        let r = estimate(&[
            ("a".into(), 128, 128, 16),
            ("b".into(), 64, 64, 200),
            ("c".into(), 512, 256, 3000),
        ]);
        assert_eq!(r.textures[0].bits, 4);
        assert_eq!(r.textures[0].words, 32 * 128);
        assert_eq!(r.textures[0].clut_words, 16);
        assert_eq!(r.textures[1].bits, 8);
        assert_eq!(r.textures[1].words, 32 * 64);
        assert_eq!(r.textures[2].bits, 16);
        assert_eq!(r.total_words, 32 * 128 + 16 + 32 * 64 + 256 + 512 * 256);
        assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
        assert!(r.summary().starts_with("VRAM "));
        let small = estimate(&[("a".into(), 128, 128, 16)]);
        assert!(small.warnings.is_empty());
        assert!(small.percent_of_free < 2.0);
    }

    #[test]
    fn layout_keeps_textures_inside_pages_and_off_the_framebuffers() {
        let tex: Vec<(String, u32, u32, usize)> = (0..12)
            .map(|i| {
                (
                    format!("t{i}"),
                    128 + (i % 2) * 128,
                    128,
                    if i % 3 == 0 { 200 } else { 16 },
                )
            })
            .collect();
        let l = layout(&tex);
        assert!(l.unplaced.is_empty(), "{:?}", l.unplaced);
        let texs: Vec<&Placement> = l
            .placements
            .iter()
            .filter(|p| p.kind == "texture")
            .collect();
        assert_eq!(texs.len(), 12);
        for p in &texs {
            assert!(p.x >= 320, "{} overlaps the framebuffers", p.name);
            let (px, py) = p.page.unwrap();
            assert!(
                p.x + p.w <= px + page_span(p.bits) && p.y + p.h <= py + 256,
                "{} crosses its page",
                p.name
            );
        }
        for (i, a) in l.placements.iter().enumerate() {
            for b in &l.placements[i + 1..] {
                let apart =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(apart, "{} overlaps {}", a.name, b.name);
            }
        }
        assert_eq!(l.placements.iter().filter(|p| p.kind == "clut").count(), 12);
        assert_eq!(layout_image(&l).dimensions(), (1024, 512));
        // Too much: twenty full-colour 256x256 textures can't all fit.
        let huge: Vec<(String, u32, u32, usize)> =
            (0..20).map(|i| (format!("h{i}"), 256, 256, 5000)).collect();
        assert!(!layout(&huge).unplaced.is_empty());
    }
}
