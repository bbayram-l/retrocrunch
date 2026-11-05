//! 2D sprite lighting maps: height (from brightness and/or alpha bevel),
//! normal, specular, parallax and occlusion maps, plus a lit preview.

use glam::Vec3;
use image::{GrayImage, Luma, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SpriteSettings {
    /// Weight of brightness-derived height (emboss detail).
    pub detail: f32,
    pub detail_blur: u32,
    pub invert_detail: bool,
    /// Weight of the alpha-edge bevel (gives sprites a rounded volume).
    pub bevel: f32,
    pub bevel_width: f32,
    pub normal_strength: f32,
    /// Flip green channel: off = OpenGL / Godot (Y+), on = DirectX / Unreal (Y-).
    pub flip_y: bool,
    pub tileable: bool,
    pub specular_contrast: f32,
    pub specular_threshold: f32,
    pub specular_from_height: bool,
    pub occlusion_strength: f32,
    pub occlusion_radius: u32,
    pub parallax_contrast: f32,
}

impl Default for SpriteSettings {
    fn default() -> Self {
        Self {
            detail: 1.0,
            detail_blur: 1,
            invert_detail: false,
            bevel: 0.6,
            bevel_width: 6.0,
            normal_strength: 2.5,
            flip_y: false,
            tileable: false,
            specular_contrast: 1.5,
            specular_threshold: 0.4,
            specular_from_height: false,
            occlusion_strength: 1.0,
            occlusion_radius: 4,
            parallax_contrast: 1.0,
        }
    }
}

pub struct SpriteMaps {
    pub height: Vec<f32>,
    pub normal: RgbaImage,
    pub specular: GrayImage,
    pub occlusion: GrayImage,
    pub parallax: GrayImage,
}

fn luma(p: [u8; 4]) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}

/// Separable box blur on a float field (wraps when `tile`).
pub fn blur(src: &[f32], w: u32, h: u32, r: u32, tile: bool) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let (w, h, r) = (w as i32, h as i32, r as i32);
    let fetch = |x: i32, n: i32| {
        if tile {
            x.rem_euclid(n)
        } else {
            x.clamp(0, n - 1)
        }
    };
    let mut tmp = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in -r..=r {
                s += src[(y * w + fetch(x + k, w)) as usize];
            }
            tmp[(y * w + x) as usize] = s / (2 * r + 1) as f32;
        }
    }
    let mut out = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in -r..=r {
                s += tmp[(fetch(y + k, h) * w + x) as usize];
            }
            out[(y * w + x) as usize] = s / (2 * r + 1) as f32;
        }
    }
    out
}

/// Chamfer distance (in pixels) from each opaque pixel to the nearest transparent one.
fn edge_distance(img: &RgbaImage) -> Vec<f32> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let big = 1e6;
    let mut d: Vec<f32> = img
        .pixels()
        .map(|p| if p.0[3] < 128 { 0.0 } else { big })
        .collect();
    let at = |x: i32, y: i32| (y * w + x) as usize;
    let (o, dg) = (1.0, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let mut v = d[at(x, y)];
            if x > 0 {
                v = v.min(d[at(x - 1, y)] + o);
            } else {
                v = v.min(o);
            }
            if y > 0 {
                v = v.min(d[at(x, y - 1)] + o);
            } else {
                v = v.min(o);
            }
            if x > 0 && y > 0 {
                v = v.min(d[at(x - 1, y - 1)] + dg);
            }
            if x + 1 < w && y > 0 {
                v = v.min(d[at(x + 1, y - 1)] + dg);
            }
            d[at(x, y)] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let mut v = d[at(x, y)];
            if x + 1 < w {
                v = v.min(d[at(x + 1, y)] + o);
            } else {
                v = v.min(o);
            }
            if y + 1 < h {
                v = v.min(d[at(x, y + 1)] + o);
            } else {
                v = v.min(o);
            }
            if x + 1 < w && y + 1 < h {
                v = v.min(d[at(x + 1, y + 1)] + dg);
            }
            if x > 0 && y + 1 < h {
                v = v.min(d[at(x - 1, y + 1)] + dg);
            }
            d[at(x, y)] = v;
        }
    }
    d
}

pub fn height_map(img: &RgbaImage, s: &SpriteSettings) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let lum: Vec<f32> = img
        .pixels()
        .map(|p| {
            let l = luma(p.0);
            if s.invert_detail { 1.0 - l } else { l }
        })
        .collect();
    let lum = blur(&lum, w, h, s.detail_blur, s.tileable);
    let has_alpha = img.pixels().any(|p| p.0[3] < 128);
    let bevel: Vec<f32> = if has_alpha && s.bevel > 0.0 {
        edge_distance(img)
            .into_iter()
            .map(|d| {
                let t = (d / s.bevel_width.max(0.5)).clamp(0.0, 1.0);
                // Rounded profile.
                (1.0 - (1.0 - t) * (1.0 - t)).sqrt()
            })
            .collect()
    } else {
        vec![0.0; lum.len()]
    };
    let total = (s.detail + s.bevel).max(1e-4);
    lum.iter()
        .zip(&bevel)
        .zip(img.pixels())
        .map(|((l, b), p)| {
            if p.0[3] < 8 {
                0.0
            } else {
                (l * s.detail + b * s.bevel) / total
            }
        })
        .collect()
}

pub fn normal_from_height(
    hm: &[f32],
    w: u32,
    h: u32,
    strength: f32,
    flip_y: bool,
    tile: bool,
) -> RgbaImage {
    let (wi, hi) = (w as i32, h as i32);
    let get = |x: i32, y: i32| {
        let (x, y) = if tile {
            (x.rem_euclid(wi), y.rem_euclid(hi))
        } else {
            (x.clamp(0, wi - 1), y.clamp(0, hi - 1))
        };
        hm[(y * wi + x) as usize]
    };
    RgbaImage::from_fn(w, h, |x, y| {
        let (x, y) = (x as i32, y as i32);
        // Sobel.
        let dx = (get(x + 1, y - 1) + 2.0 * get(x + 1, y) + get(x + 1, y + 1))
            - (get(x - 1, y - 1) + 2.0 * get(x - 1, y) + get(x - 1, y + 1));
        let dy = (get(x - 1, y + 1) + 2.0 * get(x, y + 1) + get(x + 1, y + 1))
            - (get(x - 1, y - 1) + 2.0 * get(x, y - 1) + get(x + 1, y - 1));
        // Image y grows downward; OpenGL convention wants +Y pointing up.
        let mut n = Vec3::new(-dx * strength, dy * strength, 1.0).normalize();
        if flip_y {
            n.y = -n.y;
        }
        image::Rgba([
            ((n.x * 0.5 + 0.5) * 255.0) as u8,
            ((n.y * 0.5 + 0.5) * 255.0) as u8,
            ((n.z * 0.5 + 0.5) * 255.0) as u8,
            255,
        ])
    })
}

pub fn generate(img: &RgbaImage, s: &SpriteSettings) -> SpriteMaps {
    let (w, h) = img.dimensions();
    let height = height_map(img, s);
    let normal = normal_from_height(&height, w, h, s.normal_strength, s.flip_y, s.tileable);

    let specular = GrayImage::from_fn(w, h, |x, y| {
        let i = (y * w + x) as usize;
        let p = img.get_pixel(x, y).0;
        let base = if s.specular_from_height {
            height[i]
        } else {
            luma(p)
        };
        let v = ((base - s.specular_threshold) * s.specular_contrast + 0.5).clamp(0.0, 1.0);
        Luma([if p[3] < 8 { 0 } else { (v * 255.0) as u8 }])
    });

    let wide = blur(&height, w, h, s.occlusion_radius.max(1), s.tileable);
    let occlusion = GrayImage::from_fn(w, h, |x, y| {
        let i = (y * w + x) as usize;
        let cavity = (wide[i] - height[i]).max(0.0) * 4.0 * s.occlusion_strength;
        Luma([((1.0 - cavity).clamp(0.0, 1.0) * 255.0) as u8])
    });

    let parallax = GrayImage::from_fn(w, h, |x, y| {
        let v = ((height[(y * w + x) as usize] - 0.5) * s.parallax_contrast + 0.5).clamp(0.0, 1.0);
        Luma([(v * 255.0) as u8])
    });

    SpriteMaps {
        height,
        normal,
        specular,
        occlusion,
        parallax,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LightSettings {
    /// Light position in sprite pixels (x, y) and height above the sprite plane.
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub ambient: [f32; 3],
    pub ambient_intensity: f32,
    pub specular: f32,
    pub shininess: f32,
    pub use_occlusion: bool,
    /// Quantise the lit result to a few bands (cel/pixel-art look).
    pub bands: u32,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self {
            pos: [0.0, 0.0, 40.0],
            color: [1.0, 0.92, 0.8],
            intensity: 1.0,
            ambient: [0.45, 0.5, 0.7],
            ambient_intensity: 0.35,
            specular: 0.6,
            shininess: 16.0,
            use_occlusion: true,
            bands: 0,
        }
    }
}

/// CPU lit preview (point light, Blinn-Phong), the same model Godot's 2D lights use.
pub fn lit_preview(
    diffuse: &RgbaImage,
    maps: &SpriteMaps,
    l: &LightSettings,
    flip_y: bool,
) -> RgbaImage {
    lit_preview_step(diffuse, maps, l, flip_y, 1)
}

/// [`lit_preview`] evaluated on every `step`-th texel (an image `step` times smaller),
/// so large sprites can be relit while the light follows the mouse. Lighting is computed
/// in full-resolution texel coordinates, so it matches the full preview at each sample.
pub fn lit_preview_step(
    diffuse: &RgbaImage,
    maps: &SpriteMaps,
    l: &LightSettings,
    flip_y: bool,
    step: u32,
) -> RgbaImage {
    let step = step.max(1);
    let (w, h) = diffuse.dimensions();
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let sample = |o: u32, max: u32| (o * step + step / 2).min(max - 1);
    let mut out = RgbaImage::new(ow, oh);
    let row_bytes = ow as usize * 4;
    if row_bytes == 0 {
        return out;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows_per_chunk = (oh as usize).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (ci, chunk) in out.chunks_mut(rows_per_chunk * row_bytes).enumerate() {
            scope.spawn(move || {
                for (ri, row) in chunk.chunks_mut(row_bytes).enumerate() {
                    let sy = sample((ci * rows_per_chunk + ri) as u32, h);
                    for (ox, px) in row.chunks_mut(4).enumerate() {
                        let sx = sample(ox as u32, w);
                        px.copy_from_slice(&shade(diffuse, maps, l, flip_y, sx, sy).0);
                    }
                }
            });
        }
    });
    out
}

fn shade(
    diffuse: &RgbaImage,
    maps: &SpriteMaps,
    l: &LightSettings,
    flip_y: bool,
    x: u32,
    y: u32,
) -> image::Rgba<u8> {
    let lp = Vec3::from(l.pos);
    let d = diffuse.get_pixel(x, y).0;
    if d[3] == 0 {
        return image::Rgba([0, 0, 0, 0]);
    }
    let np = maps.normal.get_pixel(x, y).0;
    let mut n = Vec3::new(
        np[0] as f32 / 127.5 - 1.0,
        np[1] as f32 / 127.5 - 1.0,
        np[2] as f32 / 127.5 - 1.0,
    );
    if flip_y {
        n.y = -n.y;
    }
    // Convert to image space (y down).
    let n = Vec3::new(n.x, -n.y, n.z).normalize_or(Vec3::Z);
    let p = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, 0.0);
    let to_l = lp - p;
    let dist = to_l.length().max(1e-3);
    let ld = to_l / dist;
    let atten = 1.0 / (1.0 + (dist / (lp.z.abs() * 4.0 + 64.0)).powi(2));
    let mut diff = n.dot(ld).max(0.0) * atten * l.intensity;
    let half = (ld + Vec3::Z).normalize();
    let spec_mask = maps.specular.get_pixel(x, y).0[0] as f32 / 255.0;
    let mut spec = n.dot(half).max(0.0).powf(l.shininess.max(1.0))
        * l.specular
        * spec_mask
        * atten
        * l.intensity;
    if l.bands > 1 {
        let b = l.bands as f32;
        diff = (diff * b).floor() / b;
        spec = (spec * b).floor() / b;
    }
    let ao = if l.use_occlusion {
        maps.occlusion.get_pixel(x, y).0[0] as f32 / 255.0
    } else {
        1.0
    };
    let base = Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32) / 255.0;
    let lc = Vec3::from(l.color);
    let amb = Vec3::from(l.ambient) * l.ambient_intensity * ao;
    let c = base * (amb + lc * diff) + lc * spec;
    image::Rgba([
        (c.x.clamp(0.0, 1.0) * 255.0) as u8,
        (c.y.clamp(0.0, 1.0) * 255.0) as u8,
        (c.z.clamp(0.0, 1.0) * 255.0) as u8,
        d[3],
    ])
}

/// Godot 4 CanvasTexture resource wiring diffuse + normal + specular for 2D lights.
pub fn godot_canvas_texture(diffuse: &str, normal: &str, specular: &str, shininess: f32) -> String {
    format!(
        "[gd_resource type=\"CanvasTexture\" load_steps=4 format=3]\n\n\
         [ext_resource type=\"Texture2D\" path=\"{diffuse}\" id=\"1_d\"]\n\
         [ext_resource type=\"Texture2D\" path=\"{normal}\" id=\"2_n\"]\n\
         [ext_resource type=\"Texture2D\" path=\"{specular}\" id=\"3_s\"]\n\n\
         [resource]\n\
         diffuse_texture = ExtResource(\"1_d\")\n\
         normal_texture = ExtResource(\"2_n\")\n\
         specular_texture = ExtResource(\"3_s\")\n\
         specular_shininess = {:.3}\n\
         texture_filter = 1\n",
        (shininess / 64.0).clamp(0.0, 1.0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(size: u32) -> RgbaImage {
        let c = size as f32 / 2.0;
        RgbaImage::from_fn(size, size, |x, y| {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            if d < c * 0.8 {
                image::Rgba([180, 120, 90, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        })
    }

    #[test]
    fn bevel_normals_point_outward() {
        let img = disc(32);
        let s = SpriteSettings {
            detail: 0.0,
            bevel: 1.0,
            ..Default::default()
        };
        let m = generate(&img, &s);
        // Left edge of the disc should face left (red < 128), right edge right.
        assert!(m.normal.get_pixel(5, 16).0[0] < 110);
        assert!(m.normal.get_pixel(26, 16).0[0] > 145);
        // Top edge faces up: OpenGL convention => green > 128.
        assert!(m.normal.get_pixel(16, 5).0[1] > 145);
        let flipped = generate(&img, &SpriteSettings { flip_y: true, ..s });
        assert!(flipped.normal.get_pixel(16, 5).0[1] < 110);
    }

    #[test]
    fn flat_image_gives_flat_normals() {
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([100, 100, 100, 255]));
        let m = generate(&img, &SpriteSettings::default());
        let p = m.normal.get_pixel(4, 4).0;
        assert!((p[0] as i32 - 127).abs() <= 1 && (p[1] as i32 - 127).abs() <= 1 && p[2] >= 254);
    }

    #[test]
    fn light_on_left_brightens_left_side() {
        let img = disc(32);
        let s = SpriteSettings {
            detail: 0.0,
            bevel: 1.0,
            ..Default::default()
        };
        let m = generate(&img, &s);
        let l = LightSettings {
            pos: [-20.0, 16.0, 10.0],
            ambient_intensity: 0.0,
            specular: 0.0,
            ..Default::default()
        };
        let out = lit_preview(&img, &m, &l, false);
        assert!(out.get_pixel(6, 16).0[0] > out.get_pixel(25, 16).0[0]);
    }

    #[test]
    fn stepped_preview_matches_full_preview_at_its_samples() {
        // 33 px: the last stepped row/column clamps to the image edge.
        let img = disc(33);
        let m = generate(&img, &SpriteSettings::default());
        let l = LightSettings {
            pos: [5.0, 9.0, 20.0],
            ..Default::default()
        };
        let full = lit_preview(&img, &m, &l, true);
        let small = lit_preview_step(&img, &m, &l, true, 4);
        assert_eq!(small.dimensions(), (9, 9));
        for (x, y, p) in small.enumerate_pixels() {
            let (sx, sy) = ((x * 4 + 2).min(32), (y * 4 + 2).min(32));
            assert_eq!(p, full.get_pixel(sx, sy), "sample ({x},{y})");
        }
    }
}
