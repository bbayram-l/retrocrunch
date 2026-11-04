//! Palette import/export, k-means palette generation and fast nearest-colour lookup.

use anyhow::{Context, Result, bail};
use std::path::Path;

pub type Rgb = [u8; 3];

/// Snap an 8-bit colour to the PS1's 15-bit RGB555 space (5 bits per channel),
/// expanded back to 8 bits with bit replication so white stays 255.
pub fn snap_rgb555(c: Rgb) -> Rgb {
    let s = |v: u8| {
        let five = v >> 3;
        (five << 3) | (five >> 2)
    };
    [s(c[0]), s(c[1]), s(c[2])]
}

// ---------------------------------------------------------------- colour spaces
//
// Palettes are built and matched in Oklab (perceptually uniform: equal distances look
// equally different), and dithered colours mix in linear light, as they do on screen.

/// sRGB-encoded value (0..1) to linear light: the one copy of the transfer curve.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to an sRGB-encoded value (no clamping).
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_table() -> &'static [f32; 256] {
    static T: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// sRGB bytes to linear light (0..1).
#[inline]
pub fn to_linear(c: Rgb) -> [f32; 3] {
    let t = linear_table();
    [t[c[0] as usize], t[c[1] as usize], t[c[2] as usize]]
}

/// Linear light (0..1) to sRGB bytes.
pub fn linear_to_srgb8(l: [f32; 3]) -> Rgb {
    from_linear(l)
}

fn from_linear(l: [f32; 3]) -> Rgb {
    l.map(|v| {
        (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn oklab_from_linear([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn linear_from_oklab([ll, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (ll + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (ll - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (ll - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// sRGB bytes to Oklab.
#[inline]
pub fn oklab(c: Rgb) -> [f32; 3] {
    oklab_from_linear(to_linear(c))
}

/// Oklab back to sRGB bytes.
pub fn from_oklab(lab: [f32; 3]) -> Rgb {
    from_linear(linear_from_oklab(lab))
}

/// Squared perceptual (Oklab) distance.
#[inline]
pub fn color_dist(a: Rgb, b: Rgb) -> f32 {
    lab_dist(&oklab(a), &oklab(b))
}

#[inline]
fn lab_dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let (x, y, z) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    x * x + y * y + z * z
}

/// Nearest and second-nearest palette entries (the same index twice for one entry).
fn nearest_two(lab: &[f32; 3], labs: &[[f32; 3]]) -> (usize, usize) {
    let (mut b1, mut b2) = (0, 0);
    let (mut d1, mut d2) = (f32::MAX, f32::MAX);
    for (i, p) in labs.iter().enumerate() {
        let d = lab_dist(lab, p);
        if d < d1 {
            (b2, d2) = (b1, d1);
            (b1, d1) = (i, d);
        } else if d < d2 {
            (b2, d2) = (i, d);
        }
    }
    if d2 == f32::MAX { (b1, b1) } else { (b1, b2) }
}

/// Mixing proportion (0..1) of `q` into `p` that best reproduces `x`, in linear light
/// with luminance weights (green matters most to the eye).
#[inline]
fn mix_amount(x: &[f32; 3], p: &[f32; 3], q: &[f32; 3]) -> f32 {
    const W: [f32; 3] = [0.2126, 0.7152, 0.0722];
    let (mut num, mut den) = (0.0, 0.0);
    for k in 0..3 {
        let d = q[k] - p[k];
        num += W[k] * (x[k] - p[k]) * d;
        den += W[k] * d * d;
    }
    if den <= 1e-12 {
        0.0
    } else {
        (num / den).clamp(0.0, 1.0)
    }
}

/// The palette entry that dithers best with `nearest` towards the colour `x`.
fn best_partner(
    x: &[f32; 3],
    x_lab: &[f32; 3],
    nearest: usize,
    labs: &[[f32; 3]],
    linear: &[[f32; 3]],
) -> usize {
    let p = &linear[nearest];
    let mut best = (nearest, f32::MAX);
    for (j, q) in linear.iter().enumerate() {
        if j == nearest {
            continue;
        }
        let t = mix_amount(x, p, q);
        if t <= 0.0 {
            continue;
        }
        let mixed = [
            p[0] + (q[0] - p[0]) * t,
            p[1] + (q[1] - p[1]) * t,
            p[2] + (q[2] - p[2]) * t,
        ];
        let d =
            lab_dist(x_lab, &oklab_from_linear(mixed)) + lab_dist(&labs[nearest], &labs[j]) * 0.1;
        if d < best.1 {
            best = (j, d);
        }
    }
    best.0
}

pub fn nearest_index(c: Rgb, palette: &[Rgb]) -> usize {
    let labs: Vec<[f32; 3]> = palette.iter().map(|&p| oklab(p)).collect();
    nearest_two(&oklab(c), &labs).0
}

/// 32x32x32 lookup table mapping RGB555 cells to their nearest palette entry (Oklab)
/// and the entry it dithers best with. Makes per-pixel mapping O(1).
///
/// The dithering partner isn't simply the second-nearest colour, which often lies in
/// the wrong direction: every entry is tried, mixed with the nearest one in the
/// proportion that best reproduces the cell's colour, and scored by how close that mix
/// lands plus a tenth of the distance between the two (far-apart pairs make noisy
/// dither). The same idea as Aseprite's ordered dithering, reimplemented here.
pub struct PaletteLut {
    table: Vec<(u8, u8)>,
    linear: Vec<[f32; 3]>,
}

impl PaletteLut {
    pub fn new(palette: &[Rgb]) -> Self {
        assert!(!palette.is_empty() && palette.len() <= 256);
        let labs: Vec<[f32; 3]> = palette.iter().map(|&p| oklab(p)).collect();
        let linear: Vec<[f32; 3]> = palette.iter().map(|&p| to_linear(p)).collect();
        let mut table = vec![(0u8, 0u8); 32 * 32 * 32];
        // Cells are independent: split the red axis over the cores.
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(32);
        let per = 32usize.div_ceil(threads);
        std::thread::scope(|scope| {
            for (k, chunk) in table.chunks_mut(per * 1024).enumerate() {
                let (labs, linear) = (&labs, &linear);
                scope.spawn(move || {
                    for (n, slot) in chunk.iter_mut().enumerate() {
                        let cell = k * per * 1024 + n;
                        let c = [
                            ((cell >> 10) << 3 | 4) as u8,
                            ((cell >> 5 & 31) << 3 | 4) as u8,
                            ((cell & 31) << 3 | 4) as u8,
                        ];
                        let lab = oklab(c);
                        let i = nearest_two(&lab, labs).0;
                        *slot = (
                            i as u8,
                            best_partner(&to_linear(c), &lab, i, labs, linear) as u8,
                        );
                    }
                });
            }
        });
        Self { table, linear }
    }

    #[inline]
    fn cell(c: Rgb) -> usize {
        ((c[0] as usize >> 3) << 10) | ((c[1] as usize >> 3) << 5) | (c[2] as usize >> 3)
    }

    #[inline]
    pub fn lookup(&self, c: Rgb) -> u8 {
        self.table[Self::cell(c)].0
    }

    /// Palette-aware ordered dithering ("two nearest"): `c` is projected onto the line
    /// between its two nearest palette colours (in linear light, where on-screen mixing
    /// happens), and the threshold `t` (0..1, from a dither matrix) picks between them.
    /// `strength` 0 = plain nearest colour, 1 = full mixing.
    #[inline]
    pub fn lookup_mixed(&self, c: Rgb, t: f32, strength: f32) -> u8 {
        let (i, j) = self.table[Self::cell(c)];
        if i == j || strength <= 0.0 {
            return i;
        }
        let mix = mix_amount(
            &to_linear(c),
            &self.linear[i as usize],
            &self.linear[j as usize],
        );
        if mix > 0.5 + (t - 0.5) * strength.min(1.0) {
            j
        } else {
            i
        }
    }
}

/// Tiny deterministic PRNG so results are reproducible between runs.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// k-means++ palette of at most `k` colours from `pixels`.
pub fn kmeans_palette(pixels: &[Rgb], k: usize, iterations: usize) -> Vec<Rgb> {
    let k = k.clamp(1, 256);
    if pixels.is_empty() {
        return vec![[0, 0, 0]];
    }
    // Subsample large inputs for speed.
    let mut rng = Lcg(0x5eed_1994);
    let max_samples = 30_000;
    // Clustered in Oklab, so colours are spent where the eye sees differences.
    let samples: Vec<[f32; 3]> = if pixels.len() > max_samples {
        (0..max_samples)
            .map(|_| oklab(pixels[rng.below(pixels.len())]))
            .collect()
    } else {
        pixels.iter().map(|&c| oklab(c)).collect()
    };

    // Few unique colours? Return them directly.
    let mut uniq: Vec<Rgb> = pixels.to_vec();
    uniq.sort_unstable();
    uniq.dedup();
    if uniq.len() <= k {
        return uniq;
    }

    let d2 = lab_dist;

    // k-means++ seeding.
    let mut centers: Vec<[f32; 3]> = vec![samples[rng.below(samples.len())]];
    let mut dist: Vec<f32> = samples.iter().map(|s| d2(s, &centers[0])).collect();
    while centers.len() < k {
        let total: f64 = dist.iter().map(|&d| d as f64).sum();
        if total <= 0.0 {
            break;
        }
        let mut target = (rng.next() as f64 / (1u64 << 31) as f64) * total;
        let mut chosen = samples.len() - 1;
        for (i, &d) in dist.iter().enumerate() {
            target -= d as f64;
            if target <= 0.0 {
                chosen = i;
                break;
            }
        }
        let c = samples[chosen];
        centers.push(c);
        for (i, s) in samples.iter().enumerate() {
            dist[i] = dist[i].min(d2(s, &c));
        }
    }

    let mut assign = vec![0usize; samples.len()];
    for _ in 0..iterations {
        for (i, s) in samples.iter().enumerate() {
            let mut best = 0;
            let mut bd = f32::MAX;
            for (j, c) in centers.iter().enumerate() {
                let d = d2(s, c);
                if d < bd {
                    bd = d;
                    best = j;
                }
            }
            assign[i] = best;
        }
        let mut sums = vec![[0f64; 4]; centers.len()];
        for (i, s) in samples.iter().enumerate() {
            let a = &mut sums[assign[i]];
            a[0] += s[0] as f64;
            a[1] += s[1] as f64;
            a[2] += s[2] as f64;
            a[3] += 1.0;
        }
        for (j, c) in centers.iter_mut().enumerate() {
            let a = sums[j];
            if a[3] > 0.0 {
                *c = [
                    (a[0] / a[3]) as f32,
                    (a[1] / a[3]) as f32,
                    (a[2] / a[3]) as f32,
                ];
            } else {
                *c = samples[rng.below(samples.len())];
            }
        }
    }

    dedup_keep_order(&centers.iter().map(|&c| from_oklab(c)).collect::<Vec<_>>())
}

/// Pick the `n` entries of `source` that best represent `pixels`:
/// cluster the image, then map each centroid to its nearest unused source colour.
pub fn best_n_from(pixels: &[Rgb], source: &[Rgb], n: usize) -> Vec<Rgb> {
    if source.len() <= n {
        return source.to_vec();
    }
    let centers = kmeans_palette(pixels, n, 10);
    let mut used = vec![false; source.len()];
    let mut out = Vec::with_capacity(n);
    for c in centers {
        let mut best = None;
        let mut bd = f32::MAX;
        for (i, &p) in source.iter().enumerate() {
            if used[i] {
                continue;
            }
            let d = color_dist(c, p);
            if d < bd {
                bd = d;
                best = Some(i);
            }
        }
        if let Some(i) = best {
            used[i] = true;
            out.push(source[i]);
        }
    }
    out
}

pub fn dedup_keep_order(p: &[Rgb]) -> Vec<Rgb> {
    let mut out: Vec<Rgb> = Vec::with_capacity(p.len());
    for &c in p {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------- import / export

fn parse_hex6(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    let s = if s.len() == 8 { &s[2..] } else { s }; // AARRGGBB (paint.net)
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Load a palette from .gpl, .pal (JASC or RIFF), .act, .hex, paint.net .txt, or a swatch image.
pub fn load_palette(path: &Path) -> Result<Vec<Rgb>> {
    // Built-in Lospec palettes: "builtin:pico-8".
    if let Some(name) = path
        .to_str()
        .and_then(|s| s.strip_prefix(crate::lospec::BUILTIN_PREFIX))
    {
        return crate::lospec::builtin(name)
            .map(|p| p.colors)
            .with_context(|| {
                let names: Vec<&str> = crate::lospec::BUILTINS.iter().map(|b| b.slug).collect();
                format!(
                    "no built-in palette '{name}' (available: {})",
                    names.join(", ")
                )
            });
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let colors = match ext.as_str() {
        "png" | "bmp" | "gif" | "jpg" | "jpeg" | "tga" => {
            let img = image::open(path)
                .context("opening swatch image")?
                .to_rgba8();
            let mut out = Vec::new();
            for p in img.pixels() {
                if p.0[3] < 128 {
                    continue;
                }
                let c = [p.0[0], p.0[1], p.0[2]];
                if !out.contains(&c) {
                    out.push(c);
                    if out.len() > 256 {
                        bail!("swatch image has more than 256 colours");
                    }
                }
            }
            out
        }
        "act" => {
            let bytes = std::fs::read(path)?;
            if bytes.len() < 768 {
                bail!("ACT file too short");
            }
            let count = if bytes.len() >= 772 {
                (u16::from_be_bytes([bytes[768], bytes[769]]) as usize).clamp(1, 256)
            } else {
                256
            };
            (0..count)
                .map(|i| [bytes[i * 3], bytes[i * 3 + 1], bytes[i * 3 + 2]])
                .collect()
        }
        "pal" => {
            let bytes = std::fs::read(path)?;
            if bytes.starts_with(b"RIFF") {
                // RIFF PAL: "RIFF" size "PAL " "data" size ver(2) count(2) entries(rgbx)
                let pos = bytes
                    .windows(4)
                    .position(|w| w == b"data")
                    .context("RIFF PAL without data chunk")?;
                let count = u16::from_le_bytes([bytes[pos + 10], bytes[pos + 11]]) as usize;
                let base = pos + 12;
                (0..count.min(256))
                    .filter(|i| base + i * 4 + 2 < bytes.len())
                    .map(|i| {
                        [
                            bytes[base + i * 4],
                            bytes[base + i * 4 + 1],
                            bytes[base + i * 4 + 2],
                        ]
                    })
                    .collect()
            } else {
                let text = String::from_utf8_lossy(&bytes);
                let mut lines = text.lines();
                let header = lines.next().unwrap_or("");
                if !header.trim().eq_ignore_ascii_case("JASC-PAL") {
                    bail!("unknown .pal format");
                }
                lines.next();
                lines.next();
                lines.filter_map(parse_rgb_triplet).collect()
            }
        }
        "gpl" => {
            let text = std::fs::read_to_string(path)?;
            text.lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty()
                        && !t.starts_with('#')
                        && !t.starts_with("GIMP")
                        && !t.starts_with("Name:")
                        && !t.starts_with("Columns:")
                })
                .filter_map(parse_rgb_triplet)
                .collect()
        }
        _ => {
            // .hex, paint.net .txt and anything else: one hex colour per line, ';' comments.
            let text = std::fs::read_to_string(path)?;
            text.lines()
                .filter(|l| !l.trim_start().starts_with(';'))
                .filter_map(parse_hex6)
                .collect()
        }
    };
    let colors = dedup_keep_order(&colors);
    if colors.is_empty() {
        bail!("palette {} contains no colours", path.display());
    }
    if colors.len() > 256 {
        bail!("palette has {} colours; the maximum is 256", colors.len());
    }
    Ok(colors)
}

fn parse_rgb_triplet(line: &str) -> Option<Rgb> {
    let mut it = line.split_whitespace().map(|t| t.parse::<u16>());
    let r = it.next()?.ok()?;
    let g = it.next()?.ok()?;
    let b = it.next()?.ok()?;
    Some([r.min(255) as u8, g.min(255) as u8, b.min(255) as u8])
}

pub fn save_palette(path: &Path, palette: &[Rgb]) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = match ext.as_str() {
        "gpl" => {
            let mut s = String::from("GIMP Palette\nName: retrocrunch\nColumns: 8\n#\n");
            for c in palette {
                s.push_str(&format!("{:3} {:3} {:3}\tUntitled\n", c[0], c[1], c[2]));
            }
            s
        }
        "pal" => {
            let mut s = format!("JASC-PAL\n0100\n{}\n", palette.len());
            for c in palette {
                s.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
            }
            s
        }
        _ => palette
            .iter()
            .map(|c| format!("{:02x}{:02x}{:02x}\n", c[0], c[1], c[2]))
            .collect(),
    };
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb555_keeps_extremes() {
        assert_eq!(snap_rgb555([255, 255, 255]), [255, 255, 255]);
        assert_eq!(snap_rgb555([0, 0, 0]), [0, 0, 0]);
        assert_eq!(snap_rgb555([9, 9, 9]), [8, 8, 8]);
    }

    #[test]
    fn kmeans_returns_unique_when_few_colours() {
        let px = vec![[255, 0, 0], [0, 255, 0], [255, 0, 0]];
        let p = kmeans_palette(&px, 16, 5);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn kmeans_limits_colour_count() {
        let px: Vec<Rgb> = (0..4096)
            .map(|i| [(i % 256) as u8, (i / 16) as u8, 77])
            .collect();
        let p = kmeans_palette(&px, 8, 8);
        assert!(p.len() <= 8 && p.len() >= 4);
    }

    #[test]
    fn hex_parsing_handles_paintnet_and_hash() {
        assert_eq!(parse_hex6("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex6("FFff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex6("nope"), None);
    }

    #[test]
    fn lut_matches_bruteforce_on_palette_entries() {
        let pal = vec![[0, 0, 0], [255, 255, 255], [200, 30, 30], [30, 200, 30]];
        let lut = PaletteLut::new(&pal);
        for (i, &c) in pal.iter().enumerate() {
            assert_eq!(lut.lookup(c) as usize, i);
        }
    }

    #[test]
    fn palette_roundtrip_gpl_and_hex() {
        let dir = std::env::temp_dir();
        let pal = vec![[1, 2, 3], [250, 128, 64]];
        for ext in ["gpl", "hex", "pal"] {
            let p = dir.join(format!("retrocrunch_test_pal.{ext}"));
            save_palette(&p, &pal).unwrap();
            assert_eq!(load_palette(&p).unwrap(), pal, "{ext}");
            let _ = std::fs::remove_file(p);
        }
    }
}
