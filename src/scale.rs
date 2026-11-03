//! Pixel-art scalers. None of them introduce new colours.

use image::RgbaImage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scaler {
    Nearest2x,
    Nearest3x,
    Nearest4x,
    Scale2x,
    Scale3x,
    Scale4x,
    Eagle2x,
    Scale6x,
    Scale9x,
}

impl Scaler {
    pub const ALL: [Scaler; 9] = [
        Self::Scale2x,
        Self::Scale3x,
        Self::Scale4x,
        Self::Scale6x,
        Self::Scale9x,
        Self::Eagle2x,
        Self::Nearest2x,
        Self::Nearest3x,
        Self::Nearest4x,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest2x => "Nearest 2x",
            Self::Nearest3x => "Nearest 3x",
            Self::Nearest4x => "Nearest 4x",
            Self::Scale2x => "Scale2x (EPX)",
            Self::Scale3x => "Scale3x",
            Self::Scale4x => "Scale4x (2x2x)",
            Self::Scale6x => "Scale6x (2x3x)",
            Self::Scale9x => "Scale9x (3x3x)",
            Self::Eagle2x => "Eagle 2x",
        }
    }

    pub fn factor(self) -> u32 {
        match self {
            Self::Nearest2x | Self::Scale2x | Self::Eagle2x => 2,
            Self::Nearest3x | Self::Scale3x => 3,
            Self::Nearest4x | Self::Scale4x => 4,
            Self::Scale6x => 6,
            Self::Scale9x => 9,
        }
    }

    pub fn from_name(s: &str) -> Option<Scaler> {
        let s = s.to_ascii_lowercase().replace(['-', '_', ' '], "");
        Some(match s.as_str() {
            "nearest2x" | "nearest2" => Self::Nearest2x,
            "nearest3x" | "nearest3" => Self::Nearest3x,
            "nearest4x" | "nearest4" => Self::Nearest4x,
            "scale2x" | "epx" => Self::Scale2x,
            "scale3x" => Self::Scale3x,
            "scale4x" => Self::Scale4x,
            "scale6x" => Self::Scale6x,
            "scale9x" => Self::Scale9x,
            "eagle" | "eagle2x" => Self::Eagle2x,
            _ => return None,
        })
    }

    pub fn apply(self, img: &RgbaImage, tolerance: u8) -> RgbaImage {
        match self {
            Self::Nearest2x => nearest(img, 2),
            Self::Nearest3x => nearest(img, 3),
            Self::Nearest4x => nearest(img, 4),
            Self::Scale2x => scale2x(img, tolerance),
            Self::Scale3x => scale3x(img, tolerance),
            Self::Scale4x => scale2x(&scale2x(img, tolerance), tolerance),
            Self::Scale6x => scale3x(&scale2x(img, tolerance), tolerance),
            Self::Scale9x => scale3x(&scale3x(img, tolerance), tolerance),
            Self::Eagle2x => eagle2x(img, tolerance),
        }
    }
}

type Px = [u8; 4];

#[inline]
fn eq(a: Px, b: Px, tol: u8) -> bool {
    if tol == 0 {
        return a == b;
    }
    a.iter().zip(b.iter()).all(|(x, y)| x.abs_diff(*y) <= tol)
}

struct Grid<'a> {
    img: &'a RgbaImage,
    w: i32,
    h: i32,
}

impl Grid<'_> {
    #[inline]
    fn at(&self, x: i32, y: i32) -> Px {
        self.img
            .get_pixel(x.clamp(0, self.w - 1) as u32, y.clamp(0, self.h - 1) as u32)
            .0
    }
}

pub fn nearest(img: &RgbaImage, f: u32) -> RgbaImage {
    RgbaImage::from_fn(img.width() * f, img.height() * f, |x, y| {
        *img.get_pixel(x / f, y / f)
    })
}

/// AdvMAME2x / Scale2x (EPX).
pub fn scale2x(img: &RgbaImage, tol: u8) -> RgbaImage {
    let g = Grid {
        img,
        w: img.width() as i32,
        h: img.height() as i32,
    };
    let mut out = RgbaImage::new(img.width() * 2, img.height() * 2);
    for y in 0..g.h {
        for x in 0..g.w {
            let (a, b, c, d, p) = (
                g.at(x, y - 1),
                g.at(x + 1, y),
                g.at(x - 1, y),
                g.at(x, y + 1),
                g.at(x, y),
            );
            let (mut e0, mut e1, mut e2, mut e3) = (p, p, p, p);
            if !eq(a, d, tol) && !eq(c, b, tol) {
                if eq(c, a, tol) {
                    e0 = c;
                }
                if eq(a, b, tol) {
                    e1 = b;
                }
                if eq(d, c, tol) {
                    e2 = c;
                }
                if eq(b, d, tol) {
                    e3 = d;
                }
            }
            let (ox, oy) = (x as u32 * 2, y as u32 * 2);
            out.put_pixel(ox, oy, image::Rgba(e0));
            out.put_pixel(ox + 1, oy, image::Rgba(e1));
            out.put_pixel(ox, oy + 1, image::Rgba(e2));
            out.put_pixel(ox + 1, oy + 1, image::Rgba(e3));
        }
    }
    out
}

/// AdvMAME3x / Scale3x.
pub fn scale3x(img: &RgbaImage, tol: u8) -> RgbaImage {
    let g = Grid {
        img,
        w: img.width() as i32,
        h: img.height() as i32,
    };
    let mut out = RgbaImage::new(img.width() * 3, img.height() * 3);
    for y in 0..g.h {
        for x in 0..g.w {
            // A B C / D E F / G H I
            let (a, b, c) = (g.at(x - 1, y - 1), g.at(x, y - 1), g.at(x + 1, y - 1));
            let (d, e, f) = (g.at(x - 1, y), g.at(x, y), g.at(x + 1, y));
            let (gg, h, i) = (g.at(x - 1, y + 1), g.at(x, y + 1), g.at(x + 1, y + 1));
            let mut o = [e; 9];
            if !eq(b, h, tol) && !eq(d, f, tol) {
                o[0] = if eq(d, b, tol) { d } else { e };
                o[1] = if (eq(d, b, tol) && !eq(e, c, tol)) || (eq(b, f, tol) && !eq(e, a, tol)) {
                    b
                } else {
                    e
                };
                o[2] = if eq(b, f, tol) { f } else { e };
                o[3] = if (eq(d, b, tol) && !eq(e, gg, tol)) || (eq(d, h, tol) && !eq(e, a, tol)) {
                    d
                } else {
                    e
                };
                o[4] = e;
                o[5] = if (eq(b, f, tol) && !eq(e, i, tol)) || (eq(h, f, tol) && !eq(e, c, tol)) {
                    f
                } else {
                    e
                };
                o[6] = if eq(d, h, tol) { d } else { e };
                o[7] = if (eq(d, h, tol) && !eq(e, i, tol)) || (eq(h, f, tol) && !eq(e, gg, tol)) {
                    h
                } else {
                    e
                };
                o[8] = if eq(h, f, tol) { f } else { e };
            }
            let (ox, oy) = (x as u32 * 3, y as u32 * 3);
            for k in 0..9u32 {
                out.put_pixel(ox + k % 3, oy + k / 3, image::Rgba(o[k as usize]));
            }
        }
    }
    out
}

/// Eagle: each output corner takes the neighbour colour when its three neighbours agree.
pub fn eagle2x(img: &RgbaImage, tol: u8) -> RgbaImage {
    let g = Grid {
        img,
        w: img.width() as i32,
        h: img.height() as i32,
    };
    let mut out = RgbaImage::new(img.width() * 2, img.height() * 2);
    for y in 0..g.h {
        for x in 0..g.w {
            let c = g.at(x, y);
            let (tl, t, tr) = (g.at(x - 1, y - 1), g.at(x, y - 1), g.at(x + 1, y - 1));
            let (l, r) = (g.at(x - 1, y), g.at(x + 1, y));
            let (bl, b, br) = (g.at(x - 1, y + 1), g.at(x, y + 1), g.at(x + 1, y + 1));
            let pick = |p: Px, q: Px, s: Px| if eq(p, q, tol) && eq(q, s, tol) { p } else { c };
            let (ox, oy) = (x as u32 * 2, y as u32 * 2);
            out.put_pixel(ox, oy, image::Rgba(pick(l, tl, t)));
            out.put_pixel(ox + 1, oy, image::Rgba(pick(t, tr, r)));
            out.put_pixel(ox, oy + 1, image::Rgba(pick(l, bl, b)));
            out.put_pixel(ox + 1, oy + 1, image::Rgba(pick(b, br, r)));
        }
    }
    out
}

/// Pixelate down: mode (most frequent colour) of each block, so no new colours appear.
pub fn downscale_mode(img: &RgbaImage, f: u32) -> RgbaImage {
    let f = f.max(1);
    let (w, h) = ((img.width() / f).max(1), (img.height() / f).max(1));
    RgbaImage::from_fn(w, h, |x, y| {
        let mut counts: Vec<(Px, u32)> = Vec::new();
        for dy in 0..f {
            for dx in 0..f {
                let p = img
                    .get_pixel(
                        (x * f + dx).min(img.width() - 1),
                        (y * f + dy).min(img.height() - 1),
                    )
                    .0;
                match counts.iter_mut().find(|(c, _)| *c == p) {
                    Some(e) => e.1 += 1,
                    None => counts.push((p, 1)),
                }
            }
        }
        image::Rgba(
            counts
                .into_iter()
                .max_by_key(|e| e.1)
                .map(|e| e.0)
                .unwrap_or([0; 4]),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::count_colors;

    fn diagonal() -> RgbaImage {
        RgbaImage::from_fn(8, 8, |x, y| {
            if x >= y {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        })
    }

    #[test]
    fn scalers_produce_right_size_without_new_colours() {
        let img = diagonal();
        for s in Scaler::ALL {
            let out = s.apply(&img, 0);
            assert_eq!(out.width(), 8 * s.factor(), "{s:?}");
            assert!(count_colors(&out) <= 2, "{s:?}");
        }
    }

    #[test]
    fn scale2x_smooths_diagonal_staircase() {
        let img = diagonal();
        let n = nearest(&img, 2);
        let s = scale2x(&img, 0);
        assert_ne!(n, s, "scale2x must differ from nearest on diagonals");
    }

    #[test]
    fn flat_image_is_unchanged_by_scale3x() {
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]));
        assert_eq!(scale3x(&img, 0), nearest(&img, 3));
    }

    #[test]
    fn mode_downscale_inverts_nearest() {
        let img = diagonal();
        assert_eq!(downscale_mode(&nearest(&img, 3), 3), img);
    }
}
