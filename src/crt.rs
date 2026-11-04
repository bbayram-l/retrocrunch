//! CRT / composite TV look (experimental): how a 240-line PS1 picture looked on a
//! consumer television. Upscales the low-resolution frame and adds the three things
//! that defined it: composite signal blur with a little colour bleed, dark gaps between
//! scanlines, and the RGB aperture grille. The OpenGL viewport (app/gpu.rs) and the
//! Godot screen pass (psx_post.gdshader) use the same recipe.

use image::RgbaImage;

/// Scanline darkness between lines (0 = flat, 1 = black gaps).
pub const SCANLINE: f32 = 0.45;
/// Aperture grille: the two "other" phosphors of each column are dimmed to this.
pub const MASK: f32 = 0.78;
/// Brightness compensation for scanlines and mask.
pub const GAIN: f32 = 1.3;

fn srgb_to_lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lin_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Upscale `src` by `scale` (2..=6) with the CRT look. Alpha is kept as is (nearest).
pub fn crt(src: &RgbaImage, scale: u32) -> RgbaImage {
    let scale = scale.clamp(2, 6);
    let (w, h) = src.dimensions();
    let lin: Vec<[f32; 3]> = src
        .pixels()
        .map(|p| {
            [
                srgb_to_lin(p.0[0]),
                srgb_to_lin(p.0[1]),
                srgb_to_lin(p.0[2]),
            ]
        })
        .collect();
    let at = |x: f32, y: u32, c: usize| {
        let xi = (x.floor() as i64).clamp(0, w as i64 - 1) as u32;
        lin[(y * w + xi) as usize][c]
    };
    let mut out = RgbaImage::new(w * scale, h * scale);
    for (ox, oy, px) in out.enumerate_pixels_mut() {
        let sx = (ox as f32 + 0.5) / scale as f32;
        let sy = oy / scale;
        let fy = ((oy % scale) as f32 + 0.5) / scale as f32;
        // Composite blur along the line, red a third of a pixel late, blue early.
        let ch = |c: usize, shift: f32| {
            0.25 * at(sx + shift - 0.6, sy, c)
                + 0.5 * at(sx + shift, sy, c)
                + 0.25 * at(sx + shift + 0.6, sy, c)
        };
        let mut rgb = [ch(0, -0.33), ch(1, 0.0), ch(2, 0.33)];
        // Beam profile: brightest mid-line, dark gaps between lines.
        let beam = 1.0 - SCANLINE * (1.0 - (std::f32::consts::PI * fy).sin());
        // Aperture grille: one phosphor colour per output column.
        let phase = (ox % 3) as usize;
        for (c, v) in rgb.iter_mut().enumerate() {
            *v *= beam * GAIN * if c == phase { 1.0 } else { MASK };
        }
        // Slight vignette towards the corners of the tube.
        let (u, v) = (
            ox as f32 / (w * scale) as f32 - 0.5,
            oy as f32 / (h * scale) as f32 - 0.5,
        );
        let vig = 1.0 - 0.35 * (u * u + v * v);
        let a = src.get_pixel((sx as u32).min(w - 1), sy).0[3];
        px.0 = [
            lin_to_srgb(rgb[0] * vig),
            lin_to_srgb(rgb[1] * vig),
            lin_to_srgb(rgb[2] * vig),
            a,
        ];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscales_with_scanlines_and_a_phosphor_mask() {
        let src = RgbaImage::from_pixel(16, 8, image::Rgba([180, 180, 180, 255]));
        let out = crt(&src, 4);
        assert_eq!(out.dimensions(), (64, 32));
        // Mid-line is brighter than the gap at the top of the line (same column).
        let mid = out.get_pixel(30, 4 * 3 + 2).0[1];
        let gap = out.get_pixel(30, 4 * 3).0[1];
        assert!(mid > gap, "scanline: {mid} vs {gap}");
        // Neighbouring columns light different phosphors.
        let (a, b) = (out.get_pixel(30, 14).0, out.get_pixel(31, 14).0);
        assert_ne!(a, b);
        // A sharp edge bleeds a little into its neighbour.
        let mut edge = RgbaImage::from_pixel(8, 2, image::Rgba([0, 0, 0, 255]));
        for y in 0..2 {
            for x in 4..8 {
                edge.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let e = crt(&edge, 3);
        assert!(e.get_pixel(3 * 4 - 1, 1).0[1] > 0, "blur crosses the edge");
    }
}
