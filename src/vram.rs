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
}
