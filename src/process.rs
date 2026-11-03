//! The model conversion pipeline: pieces -> budget -> decimate ->
//! hidden-face removal -> vertex lighting -> texture crunch (optionally shared palette) -> snap.

use crate::decimate::{self, DecimateOptions};
use crate::mesh::{Material, Model, Piece};
use crate::palette::Rgb;
use crate::texture::{self, BakeMaps, CrunchSettings};
use crate::visibility::{self, ShadeSettings};
use glam::{Vec2, Vec3};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PieceSettings {
    pub enabled: bool,
    /// Relative importance when distributing the triangle budget.
    pub weight: f32,
    /// Pieces with the same group id (and material) are merged before converting.
    pub group: usize,
}

impl PieceSettings {
    pub fn defaults_for(model: &Model) -> Vec<PieceSettings> {
        (0..model.pieces.len())
            .map(|i| PieceSettings {
                enabled: true,
                weight: 1.0,
                group: i,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureSizing {
    Fixed,
    /// One physical texel size for the whole game.
    PixelSize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConvertSettings {
    pub use_percent: bool,
    pub target_tris: usize,
    pub percent: f32,
    pub lock_borders: bool,
    pub uv_weight: f32,

    pub remove_hidden: bool,
    pub hidden_ignore_below: bool,

    pub vertex_ao: bool,
    pub ao_strength: f32,
    pub sun: bool,
    pub sun_yaw: f32,
    pub sun_pitch: f32,
    pub sun_strength: f32,
    pub ambient: f32,
    /// Paint vertex lighting into the texture instead of keeping vertex colours.
    pub bake_shading_into_texture: bool,

    pub texture: CrunchSettings,
    pub sizing: TextureSizing,
    /// Texel size in centimetres (Pixel size mode).
    pub texel_cm: f32,
    /// Model units per metre (1 for Blender/Godot, 100 for centimetre FBX exports).
    pub units_per_meter: f32,
    pub bake_ao_map: bool,
    pub ao_map_strength: f32,
    pub bake_normal_map: bool,
    pub normal_map_strength: f32,
    pub bake_emissive: bool,
    pub shared_palette: bool,

    /// World-space grid for vertex positions (fixed-point look). 0 = off.
    pub geometry_snap: f32,
    /// Split polygons with edges longer than this (world units) so affine texture
    /// warping stays small, like PS1 games did. 0 = off.
    #[serde(default)]
    pub subdivide_max_edge: f32,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            use_percent: false,
            target_tris: 600,
            percent: 10.0,
            lock_borders: false,
            uv_weight: 1.0,
            remove_hidden: true,
            hidden_ignore_below: false,
            vertex_ao: true,
            ao_strength: 0.7,
            sun: false,
            sun_yaw: 0.8,
            sun_pitch: 0.9,
            sun_strength: 0.6,
            ambient: 0.55,
            bake_shading_into_texture: false,
            texture: CrunchSettings::default(),
            sizing: TextureSizing::Fixed,
            texel_cm: 2.0,
            units_per_meter: 1.0,
            bake_ao_map: true,
            ao_map_strength: 1.0,
            bake_normal_map: true,
            normal_map_strength: 0.8,
            bake_emissive: true,
            shared_palette: true,
            geometry_snap: 0.0,
            subdivide_max_edge: 0.0,
        }
    }
}

impl ConvertSettings {
    pub fn sun_dir(&self) -> Vec3 {
        Vec3::new(
            self.sun_pitch.cos() * self.sun_yaw.sin(),
            self.sun_pitch.sin(),
            self.sun_pitch.cos() * self.sun_yaw.cos(),
        )
        .normalize()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub tris_before: usize,
    pub tris_after: usize,
    pub verts_after: usize,
    pub hidden_removed: usize,
    /// Triangles added by subdivision (already included in `tris_after`).
    pub subdivided_added: usize,
    /// Suggested lower bound for rigged models (0 when not rigged).
    pub recommended_min_tris: usize,
    pub textures: Vec<(String, u32, u32, usize)>,
    pub warnings: Vec<String>,
}

pub struct Converted {
    pub model: Model,
    /// Palette per material (empty for untextured / 15-bit only).
    pub palettes: Vec<Vec<Rgb>>,
    pub stats: Stats,
}

/// Safety cap so a tiny subdivision edge can't explode the mesh.
pub const MAX_SUBDIVIDED_TRIS: usize = 200_000;

pub type Progress<'a> = &'a (dyn Fn(&str, f32) + Sync);

/// Apply enable/merge settings and return the working piece list.
pub fn gather_pieces(model: &Model, ps: &[PieceSettings]) -> Vec<Piece> {
    // Pieces only merge with the same material and the same skin.
    // Same material, same skin and same morph group (identical morph target sets).
    type MergeKey = (Option<usize>, Option<usize>, Option<usize>);
    let mut groups: Vec<(usize, MergeKey, Piece)> = Vec::new();
    for (i, p) in model.pieces.iter().enumerate() {
        let s = ps.get(i).cloned().unwrap_or(PieceSettings {
            enabled: true,
            weight: 1.0,
            group: i,
        });
        if !s.enabled {
            continue;
        }
        if let Some((_, _, g)) = groups
            .iter_mut()
            .find(|(gid, key, _)| *gid == s.group && *key == (p.material, p.skin, p.morph_group))
        {
            let base = g.vertices.len() as u32;
            g.vertices.extend_from_slice(&p.vertices);
            g.indices.extend(p.indices.iter().map(|i| i + base));
            // Same morph group => same target list; keep deltas aligned with vertices.
            for (gm, pm) in g.morphs.iter_mut().zip(&p.morphs) {
                gm.positions.extend_from_slice(&pm.positions);
                gm.normals.extend_from_slice(&pm.normals);
            }
        } else {
            groups.push((s.group, (p.material, p.skin, p.morph_group), p.clone()));
        }
    }
    groups.into_iter().map(|g| g.2).collect()
}

fn piece_weight(model: &Model, ps: &[PieceSettings], piece: &Piece) -> f32 {
    // A merged piece keeps the weight of the first original piece with its name.
    model
        .pieces
        .iter()
        .position(|p| p.name == piece.name)
        .and_then(|i| ps.get(i))
        .map_or(1.0, |s| s.weight)
}

pub fn texture_size_for(pieces: &[Piece], material: usize, s: &ConvertSettings) -> u32 {
    if s.sizing == TextureSizing::Fixed {
        return s.texture.max_size;
    }
    let (mut world, mut uv) = (0.0f32, 0.0f32);
    for p in pieces.iter().filter(|p| p.material == Some(material)) {
        world += p.surface_area();
        uv += p.uv_area();
    }
    if uv <= 1e-8 || world <= 0.0 {
        return s.texture.max_size;
    }
    let texel_units = s.texel_cm / 100.0 * s.units_per_meter;
    let size = (world / uv).sqrt() / texel_units.max(1e-6);
    (size.round() as u32)
        .clamp(4, 2048)
        .next_power_of_two()
        .min(2048)
}

/// Rasterise per-vertex shading into UV space at the texture's resolution.
/// Returns a per-texel factor (None = not covered by any triangle).
fn shading_in_uv(
    pieces: &[Piece],
    material: usize,
    shade: &[Vec<f32>],
    w: u32,
    h: u32,
) -> Vec<Option<f32>> {
    let mut buf: Vec<Option<f32>> = vec![None; (w * h) as usize];
    for (pi, p) in pieces.iter().enumerate() {
        if p.material != Some(material) {
            continue;
        }
        for t in p.indices.chunks_exact(3) {
            let uv: [Vec2; 3] = std::array::from_fn(|k| {
                p.vertices[t[k] as usize].uv * Vec2::new(w as f32, h as f32)
            });
            let f: [f32; 3] = std::array::from_fn(|k| shade[pi][t[k] as usize]);
            let off = uv[0].min(uv[1]).min(uv[2]);
            let shift = Vec2::new(
                (off.x / w as f32).floor() * w as f32,
                (off.y / h as f32).floor() * h as f32,
            );
            let uv = uv.map(|u| u - shift);
            let area = (uv[1] - uv[0]).perp_dot(uv[2] - uv[0]);
            if area.abs() < 1e-9 {
                continue;
            }
            let lo = uv[0].min(uv[1]).min(uv[2]).floor();
            let hi = uv[0].max(uv[1]).max(uv[2]).ceil();
            for y in lo.y as i32..=hi.y as i32 {
                for x in lo.x as i32..=hi.x as i32 {
                    let pt = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = (uv[1] - pt).perp_dot(uv[2] - pt) / area;
                    let w1 = (uv[2] - pt).perp_dot(uv[0] - pt) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {
                        continue;
                    }
                    let tx = x.rem_euclid(w as i32) as u32;
                    let ty = y.rem_euclid(h as i32) as u32;
                    let cell = &mut buf[(ty * w + tx) as usize];
                    if cell.is_none() {
                        *cell = Some(w0 * f[0] + w1 * f[1] + w2 * f[2]);
                    }
                }
            }
        }
    }
    // Dilate a few texels so seams don't show unshaded gaps.
    for _ in 0..2 {
        let src = buf.clone();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if src[(y as u32 * w + x as u32) as usize].is_some() {
                    continue;
                }
                let mut acc = (0.0, 0);
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0
                        && ny >= 0
                        && nx < w as i32
                        && ny < h as i32
                        && let Some(v) = src[(ny as u32 * w + nx as u32) as usize]
                    {
                        acc.0 += v;
                        acc.1 += 1;
                    }
                }
                if acc.1 > 0 {
                    buf[(y as u32 * w + x as u32) as usize] = Some(acc.0 / acc.1 as f32);
                }
            }
        }
    }
    buf
}

struct PreparedTexture {
    image: RgbaImage,
    pixels: Vec<Rgb>,
}

pub fn convert(
    model: &Model,
    ps: &[PieceSettings],
    s: &ConvertSettings,
    imported_palette: Option<&[Rgb]>,
    progress: Progress,
) -> Converted {
    let mut stats = Stats {
        tris_before: model.tri_count(),
        ..Default::default()
    };
    progress("Gathering pieces", 0.02);
    let src = gather_pieces(model, ps);

    // 1. Budget + decimation.
    let total: usize = src.iter().map(Piece::tri_count).sum();
    let budget = if s.use_percent {
        (total as f32 * s.percent / 100.0).round() as usize
    } else {
        s.target_tris
    };
    let info: Vec<(usize, f32, f32)> = src
        .iter()
        .map(|p| (p.tri_count(), p.surface_area(), piece_weight(model, ps, p)))
        .collect();
    let targets = decimate::distribute_budget(budget.max(1), &info);
    if model.is_rigged() {
        stats.recommended_min_tris = decimate::recommended_min_tris(&src);
        if budget < stats.recommended_min_tris {
            stats.warnings.push(format!(
                "Budget {budget} is below the recommended {} triangles for this rig: joints may not bend cleanly.",
                stats.recommended_min_tris
            ));
        }
    }
    let dopt = DecimateOptions {
        lock_borders: s.lock_borders,
        uv_weight: s.uv_weight,
        ..Default::default()
    };
    let mut pieces: Vec<Piece> = Vec::with_capacity(src.len());
    for (i, (p, &t)) in src.iter().zip(&targets).enumerate() {
        progress(
            &format!("Decimating {}", p.name),
            0.05 + 0.45 * i as f32 / src.len().max(1) as f32,
        );
        let mut d = decimate::decimate(p, t, &dopt);
        if d.vertices.iter().all(|v| v.normal == Vec3::ZERO) {
            d.recompute_normals();
        }
        pieces.push(d);
    }

    // 2. Hidden faces.
    if s.remove_hidden {
        progress("Removing hidden faces", 0.55);
        // Skinned pieces only occlude: faces hidden in the bind pose (armpits, inner
        // thighs) can show up once the character moves.
        let protect: Vec<bool> = pieces
            .iter()
            .map(|p| p.skin.is_some() || p.has_morphs())
            .collect();
        stats.hidden_removed =
            visibility::remove_hidden(&mut pieces, &protect, 96, 384, s.hidden_ignore_below);
        pieces.retain(|p| !p.indices.is_empty());
    }

    // 3. Vertex lighting.
    let shade_cfg = ShadeSettings {
        ao: s.vertex_ao,
        ao_strength: s.ao_strength,
        ao_views: 48,
        sun: s.sun.then(|| (s.sun_dir(), s.sun_strength)),
        ambient: if s.sun { s.ambient } else { 1.0 },
    };
    let shade: Vec<Vec<f32>> = if s.vertex_ao || s.sun {
        progress("Lighting vertices", 0.65);
        visibility::vertex_shading(&pieces, &shade_cfg)
    } else {
        pieces.iter().map(|p| vec![1.0; p.vertices.len()]).collect()
    };

    // 4. Textures.
    let mut materials: Vec<Material> = model.materials.clone();
    let mut prepared: Vec<Option<PreparedTexture>> = Vec::with_capacity(materials.len());
    let mut baked_shading = vec![false; materials.len()];
    for (mi, m) in model.materials.iter().enumerate() {
        progress(
            &format!("Texture {}", m.name),
            0.75 + 0.15 * mi as f32 / materials.len().max(1) as f32,
        );
        let used = pieces.iter().any(|p| p.material == Some(mi));
        let Some(albedo) = m.albedo.as_ref().filter(|_| used) else {
            prepared.push(None);
            continue;
        };
        let size = texture_size_for(&pieces, mi, s);
        let (tw, th) = texture::target_dims(
            albedo.width(),
            albedo.height(),
            size,
            s.texture.power_of_two,
        );
        let mut img = texture::resize(albedo, tw, th, s.texture.filter);
        // glTF base colour factor tints the texture.
        let f = m.base_color;
        if f[..3] != [1.0, 1.0, 1.0] {
            for p in img.pixels_mut() {
                for c in 0..3 {
                    p.0[c] = (p.0[c] as f32 * f[c]).clamp(0.0, 255.0) as u8;
                }
            }
        }
        let maps = BakeMaps {
            ao: m.ao.as_ref().filter(|_| s.bake_ao_map),
            ao_strength: s.ao_map_strength,
            normal: m.normal.as_ref().filter(|_| s.bake_normal_map),
            normal_strength: s.normal_map_strength,
            emissive: m.emissive.as_ref().filter(|_| s.bake_emissive),
        };
        texture::bake_maps(&mut img, &maps);
        if s.bake_shading_into_texture && (s.vertex_ao || s.sun) {
            let fac = shading_in_uv(&pieces, mi, &shade, tw, th);
            for (i, p) in img.pixels_mut().enumerate() {
                if let Some(k) = fac[i] {
                    for c in 0..3 {
                        p.0[c] = (p.0[c] as f32 * k).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            baked_shading[mi] = true;
        }
        texture::adjust(
            &mut img,
            s.texture.brightness,
            s.texture.contrast,
            s.texture.saturation,
        );
        let thr = if s.texture.alpha == texture::AlphaMode::Cutout {
            s.texture.alpha_threshold
        } else {
            1
        };
        let pixels: Vec<Rgb> = img
            .pixels()
            .filter(|p| p.0[3] >= thr)
            .map(|p| [p.0[0], p.0[1], p.0[2]])
            .collect();
        prepared.push(Some(PreparedTexture { image: img, pixels }));
    }

    progress("Quantising palettes", 0.9);
    let shared = if s.shared_palette && prepared.iter().filter(|p| p.is_some()).count() > 1 {
        let all: Vec<Rgb> = prepared
            .iter()
            .flatten()
            .flat_map(|p| p.pixels.iter().copied())
            .collect();
        Some(texture::build_palette(&all, &s.texture, imported_palette))
    } else {
        None
    };
    let mut palettes = vec![Vec::new(); materials.len()];
    for (mi, prep) in prepared.into_iter().enumerate() {
        let Some(prep) = prep else { continue };
        let pal = shared
            .clone()
            .unwrap_or_else(|| texture::build_palette(&prep.pixels, &s.texture, imported_palette));
        let out = texture::quantize(&prep.image, &s.texture, &pal);
        stats.textures.push((
            materials[mi].name.clone(),
            out.width(),
            out.height(),
            texture::count_colors(&out),
        ));
        materials[mi].albedo = Some(out);
        materials[mi].ao = None;
        materials[mi].normal = None;
        materials[mi].emissive = None;
        materials[mi].base_color = [1.0, 1.0, 1.0, m_alpha(&model.materials[mi])];
        palettes[mi] = pal;
    }

    // 5. Vertex colours: lighting x original colour x (untextured) material colour.
    for (pi, p) in pieces.iter_mut().enumerate() {
        let mat = p.material.and_then(|m| model.materials.get(m));
        let textured = mat.is_some_and(|m| m.albedo.is_some());
        let tint = if textured {
            [1.0; 4]
        } else {
            mat.map_or([1.0; 4], |m| m.base_color)
        };
        let baked = p
            .material
            .is_some_and(|m| baked_shading.get(m).copied().unwrap_or(false));
        for (vi, v) in p.vertices.iter_mut().enumerate() {
            let l = if baked { 1.0 } else { shade[pi][vi] };
            for c in 0..3 {
                v.color[c] = (v.color[c] * tint[c] * l).clamp(0.0, 1.0);
            }
            if !textured && s.texture.rgb555 {
                for c in 0..3 {
                    v.color[c] = ((v.color[c] * 31.0).round() / 31.0).clamp(0.0, 1.0);
                }
            }
        }
        if s.subdivide_max_edge > 0.0 {
            let before = p.tri_count();
            p.subdivide(s.subdivide_max_edge, MAX_SUBDIVIDED_TRIS);
            stats.subdivided_added += p.tri_count() - before;
        }
        if s.geometry_snap > 0.0 {
            for v in &mut p.vertices {
                v.pos = (v.pos / s.geometry_snap).round() * s.geometry_snap;
            }
        }
    }

    if s.texture.alpha == texture::AlphaMode::Keep {
        stats.warnings.push("Semi-transparent texels are exported as RGBA; PS1 hardware only had 1-bit transparency.".into());
    }
    let out = Model {
        name: model.name.clone(),
        source: model.source.clone(),
        pieces,
        materials,
        rig: model.rig.clone(),
    };
    stats.tris_after = out.tri_count();
    stats.verts_after = out.vertex_count();
    progress("Done", 1.0);
    Converted {
        model: out,
        palettes,
        stats,
    }
}

fn m_alpha(m: &Material) -> f32 {
    m.base_color[3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::uv_sphere;

    fn test_model() -> Model {
        let mut a = uv_sphere(48);
        a.material = Some(0);
        let mut b = uv_sphere(16);
        b.name = "small".into();
        for v in &mut b.vertices {
            v.pos = v.pos * 0.3 + Vec3::new(2.0, 0.0, 0.0);
        }
        b.material = Some(0);
        let albedo = RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 200, 255]));
        Model {
            name: "t".into(),
            pieces: vec![a, b],
            materials: vec![Material {
                albedo: Some(albedo),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn convert_reduces_and_crunches() {
        let m = test_model();
        let ps = PieceSettings::defaults_for(&m);
        let s = ConvertSettings {
            target_tris: 400,
            ..Default::default()
        };
        let c = convert(&m, &ps, &s, None, &|_, _| {});
        assert!(c.stats.tris_after <= 520, "tris {}", c.stats.tris_after);
        let tex = c.model.materials[0].albedo.as_ref().unwrap();
        assert_eq!(tex.dimensions(), (128, 128));
        assert!(texture::count_colors(tex) <= 16);
    }

    #[test]
    fn disabled_pieces_are_dropped_and_merge_groups_join() {
        let m = test_model();
        let mut ps = PieceSettings::defaults_for(&m);
        ps[1].enabled = false;
        assert_eq!(gather_pieces(&m, &ps).len(), 1);
        let mut ps = PieceSettings::defaults_for(&m);
        ps[1].group = 0;
        let g = gather_pieces(&m, &ps);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].tri_count(), m.tri_count());
    }

    #[test]
    fn rigged_conversion_keeps_rig_skin_and_hidden_faces() {
        let mut m = test_model();
        // Inner piece is fully enclosed; skin it so hidden-face removal must keep it.
        m.pieces[1].skin = Some(0);
        for v in &mut m.pieces[1].vertices {
            v.pos -= glam::Vec3::new(2.0, 0.0, 0.0); // move inside the big sphere
            v.joints = [1, 0, 0, 0];
            v.weights = [1.0, 0.0, 0.0, 0.0];
        }
        m.rig = Some(crate::rig::tests::two_bone_rig());
        let before_inner = m.pieces[1].tri_count();
        let ps = PieceSettings::defaults_for(&m);
        let s = ConvertSettings {
            target_tris: 100_000,
            subdivide_max_edge: 0.05,
            ..Default::default()
        };
        let c = convert(&m, &ps, &s, None, &|_, _| {});
        assert!(c.model.rig.is_some());
        let inner = c
            .model
            .pieces
            .iter()
            .find(|p| p.skin == Some(0))
            .expect("skinned piece survives");
        assert!(
            inner.tri_count() >= before_inner,
            "skinned faces are never culled"
        );
        assert!(
            inner
                .vertices
                .iter()
                .all(|v| v.joints[0] == 1 && (v.weights[0] - 1.0).abs() < 1e-5)
        );
        assert!(c.stats.subdivided_added > 0);
        assert!(c.stats.recommended_min_tris > 0);
    }

    #[test]
    fn pixel_size_mode_scales_with_texel() {
        let m = test_model();
        let mut s = ConvertSettings {
            sizing: TextureSizing::PixelSize,
            texel_cm: 1.0,
            ..Default::default()
        };
        let fine = texture_size_for(&m.pieces, 0, &s);
        s.texel_cm = 4.0;
        let coarse = texture_size_for(&m.pieces, 0, &s);
        assert!(fine > coarse, "{fine} vs {coarse}");
    }
}
