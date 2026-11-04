//! Palette cycling (experimental): a texture stored as palette indices, where a range of
//! entries rotates over time, the classic 8/16-bit way to animate water, lava, lights and
//! conveyor belts without drawing new frames. Exported as an index image + palette strip
//! for the Godot shaders (`psx_palette_cycle[_3d].gdshader`) and as an animated GIF.

use crate::palette::Rgb;
use anyhow::{Context, Result};
use image::{GrayImage, RgbaImage};
use std::path::{Path, PathBuf};

/// Texture as palette indices plus the palette (RGBA; a transparent entry is appended
/// when the texture has transparent texels).
pub struct Indexed {
    pub index: GrayImage,
    pub palette: Vec<[u8; 4]>,
}

/// Index `img` against `pal` (exact matches; anything else maps to the nearest entry).
pub fn index_image(img: &RgbaImage, pal: &[Rgb]) -> Indexed {
    let mut palette: Vec<[u8; 4]> = pal.iter().map(|c| [c[0], c[1], c[2], 255]).collect();
    let clear = img.pixels().any(|p| p.0[3] < 128);
    let clear_index = if clear {
        palette.push([0, 0, 0, 0]);
        palette.len() - 1
    } else {
        0
    };
    let index = GrayImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        if p[3] < 128 && clear {
            return image::Luma([clear_index as u8]);
        }
        let c = [p[0], p[1], p[2]];
        let i = pal
            .iter()
            .position(|&q| q == c)
            .unwrap_or_else(|| crate::palette::nearest_index(c, pal));
        image::Luma([i.min(255) as u8])
    });
    Indexed { index, palette }
}

impl Indexed {
    /// The texture with entries `start..=end` rotated by `step` places.
    pub fn frame(&self, start: usize, end: usize, step: i64) -> RgbaImage {
        let n = end.saturating_sub(start) + 1;
        RgbaImage::from_fn(self.index.width(), self.index.height(), |x, y| {
            let mut i = self.index.get_pixel(x, y).0[0] as usize;
            if n > 1 && (start..=end).contains(&i) {
                i = start + (i - start + step.rem_euclid(n as i64) as usize) % n;
            }
            image::Rgba(self.palette.get(i).copied().unwrap_or([0, 0, 0, 0]))
        })
    }

    /// Palette as an N x 1 strip.
    pub fn palette_image(&self) -> RgbaImage {
        RgbaImage::from_fn(self.palette.len().max(1) as u32, 1, |x, _| {
            image::Rgba(
                self.palette
                    .get(x as usize)
                    .copied()
                    .unwrap_or([0, 0, 0, 0]),
            )
        })
    }
}

/// Write `<name>_index.png`, `<name>_palette.png` (with Godot import settings), the
/// Godot shaders and `<name>_cycle.gif` (one full loop at `speed` steps per second).
pub fn export(
    dir: &Path,
    name: &str,
    ix: &Indexed,
    start: usize,
    end: usize,
    speed: f32,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = crate::export::sanitize(name);
    let mut files = Vec::new();
    let index = dir.join(format!("{name}_index.png"));
    ix.index
        .save(&index)
        .with_context(|| format!("writing {}", index.display()))?;
    let pal = dir.join(format!("{name}_palette.png"));
    ix.palette_image()
        .save(&pal)
        .with_context(|| format!("writing {}", pal.display()))?;
    for p in [&index, &pal] {
        let imp = PathBuf::from(format!("{}.import", p.display()));
        std::fs::write(&imp, crate::export::godot_texture_import(false))?;
        files.push(p.clone());
        files.push(imp);
    }
    for (file, text) in [
        (
            "psx_palette_cycle.gdshader",
            include_str!("../godot/psx_palette_cycle.gdshader"),
        ),
        (
            "psx_palette_cycle_3d.gdshader",
            include_str!("../godot/psx_palette_cycle_3d.gdshader"),
        ),
    ] {
        let p = dir.join(file);
        std::fs::write(&p, text)?;
        files.push(p);
    }
    // Animated GIF of one loop.
    let gif = dir.join(format!("{name}_cycle.gif"));
    let n = end.saturating_sub(start) + 1;
    let ms = (1000.0 / speed.abs().max(0.1)).round() as u32;
    let file = std::fs::File::create(&gif).with_context(|| format!("writing {}", gif.display()))?;
    let mut enc = image::codecs::gif::GifEncoder::new(std::io::BufWriter::new(file));
    enc.set_repeat(image::codecs::gif::Repeat::Infinite)?;
    for k in 0..n.max(1) {
        let step = if speed < 0.0 { -(k as i64) } else { k as i64 };
        let frame = image::Frame::from_parts(
            ix.frame(start, end, step),
            0,
            0,
            image::Delay::from_numer_denom_ms(ms, 1),
        );
        enc.encode_frame(frame)?;
    }
    files.push(gif);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_rotates_only_the_chosen_range() {
        // Four stripes using palette entries 0..3, plus a transparent corner.
        let pal: Vec<Rgb> = vec![[10, 0, 0], [20, 0, 0], [30, 0, 0], [40, 0, 0]];
        let mut img = RgbaImage::from_fn(4, 2, |x, _| {
            let c = pal[x as usize];
            image::Rgba([c[0], c[1], c[2], 255])
        });
        img.put_pixel(0, 1, image::Rgba([0, 0, 0, 0]));
        let ix = index_image(&img, &pal);
        assert_eq!(ix.palette.len(), 5, "transparent entry appended");
        assert_eq!(ix.index.get_pixel(0, 1).0[0], 4);
        // Rotate entries 1..=3 by one: 1->2, 2->3, 3->1; entry 0 stays.
        let f = ix.frame(1, 3, 1);
        assert_eq!(f.get_pixel(0, 0).0, [10, 0, 0, 255]);
        assert_eq!(f.get_pixel(1, 0).0, [30, 0, 0, 255]);
        assert_eq!(f.get_pixel(2, 0).0, [40, 0, 0, 255]);
        assert_eq!(f.get_pixel(3, 0).0, [20, 0, 0, 255]);
        assert_eq!(f.get_pixel(0, 1).0[3], 0, "transparency kept");
        // A full loop comes back; negative steps go the other way.
        assert_eq!(ix.frame(1, 3, 3), ix.frame(1, 3, 0));
        assert_eq!(ix.frame(1, 3, -1), ix.frame(1, 3, 2));
        let dir = std::env::temp_dir().join("retrocrunch_cycle_test");
        let _ = std::fs::remove_dir_all(&dir);
        let files = export(&dir, "water", &ix, 1, 3, 6.0).unwrap();
        assert!(files.iter().any(|f| f.ends_with("water_cycle.gif")));
        let back = image::open(dir.join("water_index.png")).unwrap().to_luma8();
        assert_eq!(back.get_pixel(3, 0).0[0], 3);
        assert_eq!(
            image::open(dir.join("water_palette.png")).unwrap().width(),
            5
        );
    }
}
