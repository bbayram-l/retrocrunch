//! The model conversion pipeline: pieces -> budget -> decimate ->
//! hidden-face removal -> vertex lighting -> texture crunch (optionally shared palette) -> snap.

use crate::bake::{self, BakeSource, SourceMaps};
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
    /// "Bake into the texture": the piece leaves the output geometry but its colour is
    /// painted onto the surface underneath (needs the UV atlas).
    #[serde(default)]
    pub baked: bool,
}

impl PieceSettings {
    pub fn defaults_for(model: &Model) -> Vec<PieceSettings> {
        (0..model.pieces.len())
            .map(|i| PieceSettings {
                enabled: true,
                weight: 1.0,
                group: i,
                baked: false,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UvMode {
    /// Keep the source UVs (texture is resized per material).
    Keep,
    /// Unwrap a fresh atlas for the low-poly mesh and rebake everything into one texture.
    Atlas,
}

fn default_uv_mode() -> UvMode {
    UvMode::Atlas
}
fn default_chart_angle() -> f32 {
    60.0
}
fn default_bake_distance() -> f32 {
    3.0
}
fn default_atlas_padding() -> u32 {
    2
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureSizing {
    Fixed,
    /// One physical texel size for the whole game.
    PixelSize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
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
    /// UV layout of the output.
    #[serde(default = "default_uv_mode")]
    pub uv_mode: UvMode,
    /// Atlas charts group faces whose normals stay within this many degrees.
    #[serde(default = "default_chart_angle")]
    pub chart_angle: f32,
    /// Rebake search distance above/below the low-poly surface, % of the model size.
    #[serde(default = "default_bake_distance")]
    pub bake_distance: f32,
    /// Gutter between atlas charts, in texels.
    #[serde(default = "default_atlas_padding")]
    pub atlas_padding: u32,
    /// Rebuild static models as an even, closed surface before decimation (Blender voxel
    /// remesh + QuadriFlow): fewer long slivers, so less affine warping and smoother
    /// vertex lighting; repairs scans. Textures are rebaked into a fresh atlas.
    pub remesh: bool,
    /// Remeshed triangles per final triangle (the decimator then reduces to the budget).
    pub remesh_density: f32,
    /// HD texture: keep the source's texel density and full colour (no palette, dither
    /// or 15-bit snap) on the low-poly model. Still pixel-sharp with nearest filtering.
    pub hd_texture: bool,
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
            hd_texture: false,
            remesh: false,
            remesh_density: 3.0,
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
            uv_mode: UvMode::Atlas,
            chart_angle: 60.0,
            bake_distance: 3.0,
            atlas_padding: 2,
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
    /// Fresh atlas, if one was made: (size in texels, number of charts).
    pub atlas: Option<(u32, usize)>,
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
            baked: false,
            enabled: true,
            weight: 1.0,
            group: i,
        });
        if !s.enabled || s.baked {
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

/// Everything the atlas is baked from: all enabled pieces at full resolution, including
/// pieces marked "baked" (which are left out of the output geometry).
pub fn bake_source_pieces(model: &Model, ps: &[PieceSettings]) -> Vec<Piece> {
    model
        .pieces
        .iter()
        .enumerate()
        .filter(|(i, _)| ps.get(*i).is_none_or(|s| s.enabled || s.baked))
        .map(|(_, p)| p.clone())
        .collect()
}

/// Square atlas size: the fixed texture size, or (Pixel size mode) enough texels for the
/// whole surface at the chosen texel size, assuming ~65% packing efficiency.
pub fn atlas_size_for(pieces: &[Piece], s: &ConvertSettings) -> u32 {
    let n = match s.sizing {
        TextureSizing::Fixed => s.texture.max_size,
        TextureSizing::PixelSize => {
            let world: f32 = pieces.iter().map(Piece::surface_area).sum();
            let texel = (s.texel_cm / 100.0 * s.units_per_meter).max(1e-6);
            ((world / 0.65).sqrt() / texel).round() as u32
        }
    };
    let n = n.clamp(8, 2048);
    if s.texture.power_of_two {
        let p = n.next_power_of_two();
        if p > n && p / 2 >= 8 && (s.sizing == TextureSizing::Fixed) {
            p / 2
        } else {
            p.min(2048)
        }
    } else {
        n
    }
}

/// Atlas size that keeps the source textures' texel density (HD texture mode): the
/// texels the source UVs cover, plus packing slack, rounded to the nearest power of two.
pub fn hd_atlas_size(source: &[Piece], materials: &[Material]) -> u32 {
    let texels: f64 = source
        .iter()
        .filter_map(|p| {
            let img = p.material.and_then(|m| materials.get(m))?.albedo.as_ref()?;
            Some(p.uv_area() as f64 * img.width() as f64 * img.height() as f64)
        })
        .sum();
    let largest = materials
        .iter()
        .filter_map(|m| m.albedo.as_ref())
        .map(|i| i.width().max(i.height()))
        .max()
        .unwrap_or(1024);
    // Untextured or degenerate UVs: fall back to the largest source texture.
    let n = if texels > 1.0 {
        (texels / 0.65).sqrt() as u32
    } else {
        largest
    };
    let p = n.clamp(128, 4096).next_power_of_two();
    let n = if (n as f32) < p as f32 * 0.75 {
        p / 2
    } else {
        p
    };
    n.clamp(128, 4096)
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

/// Replace `src` by a Blender remesh (static models only). Returns whether it happened.
fn remesh_pieces(
    src: &mut Vec<Piece>,
    model: &Model,
    budget: usize,
    s: &ConvertSettings,
    stats: &mut Stats,
    progress: Progress,
) -> bool {
    if model.is_rigged() || src.iter().any(Piece::has_morphs) {
        stats.warnings.push(
            "Remesh skipped: it would lose skin weights and morph targets (static models only)."
                .into(),
        );
        return false;
    }
    progress("Remeshing (Blender)", 0.03);
    let tris = (budget.max(1) as f32 * s.remesh_density.max(1.0)) as usize;
    match crate::blender::remesh(src, tris.div_ceil(2)) {
        Ok(p) if !p.is_empty() => {
            *src = p;
            true
        }
        Ok(_) => {
            stats
                .warnings
                .push("Remesh produced no geometry: kept the original.".into());
            false
        }
        Err(e) => {
            stats
                .warnings
                .push(format!("Remesh failed, kept the original mesh: {e:#}"));
            false
        }
    }
}

struct PreparedTexture {
    image: RgbaImage,
    pixels: Vec<Rgb>,
    cutout: bool,
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
    let mut src = gather_pieces(model, ps);
    let budget_hint = if s.use_percent {
        (model.tri_count() as f32 * s.percent / 100.0).round() as usize
    } else {
        s.target_tris
    };
    let remeshed = s.remesh && remesh_pieces(&mut src, model, budget_hint, s, &mut stats, progress);

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
    let mut shade: Vec<Vec<f32>> = if s.vertex_ao || s.sun {
        progress("Lighting vertices", 0.65);
        visibility::vertex_shading(&pieces, &shade_cfg)
    } else {
        pieces.iter().map(|p| vec![1.0; p.vertices.len()]).collect()
    };

    // 3b. Fresh UV atlas: unwrap the low-poly mesh and rebake the full-resolution source
    //     (plus pieces marked "baked") into one texture.
    let any_baked = ps.iter().any(|p| p.baked && p.enabled);
    let any_textured = model.materials.iter().any(|m| m.albedo.is_some());
    let mut atlas_materials: Option<Vec<Material>> = None;
    if (s.uv_mode == UvMode::Atlas && any_textured) || any_baked || remeshed {
        if s.uv_mode == UvMode::Keep && !remeshed {
            stats.warnings.push(
                "Baking pieces into the texture needs a fresh UV atlas: used Atlas mode.".into(),
            );
        }
        progress("Unwrapping UV atlas", 0.7);
        let src_pieces = bake_source_pieces(model, ps);
        let n = if s.hd_texture {
            hd_atlas_size(&src_pieces, &model.materials)
        } else {
            atlas_size_for(&pieces, s)
        };
        // Gutters cost a lot of space in tiny atlases: 1 texel is enough at <= 128 px.
        let pad = if n <= 128 {
            s.atlas_padding.min(1)
        } else {
            s.atlas_padding
        };
        match bake::unwrap_atlas(&mut pieces, n, pad, s.chart_angle) {
            Some(u) => {
                for (sh, r) in shade.iter_mut().zip(&u.remap) {
                    if !r.is_empty() {
                        *sh = r
                            .iter()
                            .map(|&o| sh.get(o as usize).copied().unwrap_or(1.0))
                            .collect();
                    }
                }
                progress("Baking textures", 0.75);
                let source = BakeSource::new(
                    &src_pieces,
                    &model.materials,
                    &SourceMaps {
                        ao: s.bake_ao_map,
                        ao_strength: s.ao_map_strength,
                        normal: s.bake_normal_map,
                        normal_strength: s.normal_map_strength,
                        emissive: s.bake_emissive,
                    },
                );
                let cage = source.diagonal * s.bake_distance.max(0.01) / 100.0;
                let img = bake::bake_atlas(&pieces, n, &source, cage, source.supersample_for(n));
                // Source colours (texture, base colour, vertex colour) now live in the atlas.
                for p in &mut pieces {
                    p.material = Some(0);
                    for v in &mut p.vertices {
                        v.color = [1.0, 1.0, 1.0, 1.0];
                    }
                }
                // One atlas material: double-sided if any source surface was.
                let double_sided = src_pieces.iter().any(|p| {
                    p.material
                        .and_then(|m| model.materials.get(m))
                        .is_some_and(|m| m.double_sided)
                });
                atlas_materials = Some(vec![Material {
                    name: format!("{}_atlas", model.name),
                    double_sided,
                    albedo: Some(img),
                    ..Default::default()
                }]);
                stats.atlas = Some((n, u.charts));
            }
            None => stats.warnings.push(format!(
                "The UV atlas did not fit in {n}x{n}; kept the original UVs."
            )),
        }
    }
    let atlas = atlas_materials.is_some();
    let src_materials: &[Material] = atlas_materials.as_deref().unwrap_or(&model.materials);

    // 4. Textures.
    let mut materials: Vec<Material> = src_materials.to_vec();
    let mut prepared: Vec<Option<PreparedTexture>> = Vec::with_capacity(materials.len());
    let mut baked_shading = vec![false; materials.len()];
    for (mi, m) in src_materials.iter().enumerate() {
        progress(
            &format!("Texture {}", m.name),
            0.75 + 0.15 * mi as f32 / materials.len().max(1) as f32,
        );
        let used = pieces.iter().any(|p| p.material == Some(mi));
        let Some(albedo) = m.albedo.as_ref().filter(|_| used) else {
            prepared.push(None);
            continue;
        };
        let (tw, th) = if atlas || s.hd_texture {
            albedo.dimensions()
        } else {
            texture::target_dims(
                albedo.width(),
                albedo.height(),
                texture_size_for(&pieces, mi, s),
                s.texture.power_of_two,
            )
        };
        // The atlas is already baked at its final size, with the maps applied.
        let mut img = if atlas || s.hd_texture {
            albedo.clone()
        } else {
            texture::resize(albedo, tw, th, s.texture.filter)
        };
        // glTF base colour factor tints the texture.
        let f = m.base_color;
        if !atlas && f[..3] != [1.0, 1.0, 1.0] {
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
        if !atlas {
            texture::bake_maps(&mut img, &maps);
        }
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
        let cutout = texture::has_cutout_texels(&img, &s.texture);
        prepared.push(Some(PreparedTexture {
            image: img,
            pixels,
            cutout,
        }));
    }

    progress("Quantising palettes", 0.9);
    let shared =
        if s.shared_palette && !s.hd_texture && prepared.iter().filter(|p| p.is_some()).count() > 1
        {
            let all: Vec<Rgb> = prepared
                .iter()
                .flatten()
                .flat_map(|p| p.pixels.iter().copied())
                .collect();
            let cutout = prepared.iter().flatten().any(|p| p.cutout);
            Some(texture::build_palette(
                &all,
                &s.texture,
                imported_palette,
                cutout,
            ))
        } else {
            None
        };
    let mut palettes = vec![Vec::new(); materials.len()];
    for (mi, prep) in prepared.into_iter().enumerate() {
        let Some(prep) = prep else { continue };
        let (pal, out) = if s.hd_texture {
            // Full colour: only the alpha mode applies.
            let full = texture::CrunchSettings {
                dither: texture::DitherMode::None,
                rgb555: false,
                ..s.texture.clone()
            };
            (Vec::new(), texture::quantize(&prep.image, &full, &[]))
        } else {
            let pal = shared.clone().unwrap_or_else(|| {
                texture::build_palette(&prep.pixels, &s.texture, imported_palette, prep.cutout)
            });
            let out = texture::quantize(&prep.image, &s.texture, &pal);
            (pal, out)
        };
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
        materials[mi].base_color = [1.0, 1.0, 1.0, m_alpha(&src_materials[mi])];
        palettes[mi] = pal;
    }

    // 5. Vertex colours: lighting x original colour x (untextured) material colour.
    for (pi, p) in pieces.iter_mut().enumerate() {
        let mat = p.material.and_then(|m| src_materials.get(m));
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
    fn atlas_mode_makes_one_square_texture_with_valid_uvs() {
        let m = test_model();
        let ps = PieceSettings::defaults_for(&m);
        let c = convert(
            &m,
            &ps,
            &ConvertSettings {
                target_tris: 300,
                ..Default::default()
            },
            None,
            &|_, _| {},
        );
        assert_eq!(c.model.materials.len(), 1);
        let tex = c.model.materials[0].albedo.as_ref().unwrap();
        assert_eq!(tex.dimensions(), (128, 128));
        assert!(
            c.stats
                .atlas
                .is_some_and(|(n, charts)| n == 128 && charts > 1)
        );
        for p in &c.model.pieces {
            assert_eq!(p.material, Some(0));
            assert!(
                p.vertices
                    .iter()
                    .all(|v| (0.0..=1.0).contains(&v.uv.x) && (0.0..=1.0).contains(&v.uv.y))
            );
        }
        // Keep mode leaves the original material layout alone.
        let keep = convert(
            &m,
            &ps,
            &ConvertSettings {
                target_tris: 300,
                uv_mode: UvMode::Keep,
                ..Default::default()
            },
            None,
            &|_, _| {},
        );
        assert!(keep.stats.atlas.is_none());
        assert_eq!(keep.model.materials.len(), m.materials.len());
    }

    #[test]
    fn baked_piece_leaves_geometry_but_paints_the_texture() {
        // Grey floor + small red patch floating just above it, untextured.
        let quad = |size: f32, y: f32, mat: usize| {
            let v = |x: f32, z: f32| crate::mesh::Vertex {
                pos: Vec3::new(x, y, z),
                normal: Vec3::Y,
                uv: Vec2::new(x, z),
                color: [1.0; 4],
                ..Default::default()
            };
            Piece {
                name: format!("q{mat}"),
                vertices: vec![
                    v(-size, -size),
                    v(size, -size),
                    v(size, size),
                    v(-size, size),
                ],
                indices: vec![0, 2, 1, 0, 3, 2],
                material: Some(mat),
                ..Default::default()
            }
        };
        let m = Model {
            name: "floor".into(),
            pieces: vec![quad(1.0, 0.0, 0), quad(0.3, 0.01, 1)],
            materials: vec![
                Material {
                    name: "grey".into(),
                    base_color: [0.5, 0.5, 0.5, 1.0],
                    ..Default::default()
                },
                Material {
                    name: "red".into(),
                    base_color: [1.0, 0.0, 0.0, 1.0],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut ps = PieceSettings::defaults_for(&m);
        ps[1].baked = true;
        let s = ConvertSettings {
            remove_hidden: false,
            vertex_ao: false,
            uv_mode: UvMode::Keep,
            ..Default::default()
        };
        let c = convert(&m, &ps, &s, None, &|_, _| {});
        assert_eq!(
            c.model.pieces.len(),
            1,
            "the baked patch is gone from the geometry"
        );
        assert_eq!(c.model.tri_count(), 2);
        assert!(
            c.stats.warnings.iter().any(|w| w.contains("Atlas")),
            "Keep mode is upgraded to an atlas"
        );
        let tex = c.model.materials[0].albedo.as_ref().unwrap();
        let p = &c.model.pieces[0];
        let center = p.vertices.iter().map(|v| v.uv).sum::<Vec2>() / p.vertices.len() as f32;
        let px = tex
            .get_pixel(
                (center.x * tex.width() as f32) as u32,
                (center.y * tex.height() as f32) as u32,
            )
            .0;
        assert!(
            px[0] > 180 && px[1] < 80,
            "red patch painted onto the floor, got {px:?}"
        );
    }

    #[test]
    fn hd_texture_keeps_source_density_and_full_colour() {
        let m = test_model();
        let ps = PieceSettings::defaults_for(&m);
        let hd = ConvertSettings {
            target_tris: 300,
            hd_texture: true,
            ..Default::default()
        };
        // Keep mode: the source texture comes through untouched (256 px, every colour).
        let keep = convert(
            &m,
            &ps,
            &ConvertSettings {
                uv_mode: UvMode::Keep,
                ..hd.clone()
            },
            None,
            &|_, _| {},
        );
        let tex = keep.model.materials[0].albedo.as_ref().unwrap();
        assert_eq!(tex.dimensions(), (256, 256));
        assert!(texture::count_colors(tex) > 256, "full colour, no CLUT");
        // Atlas mode: sized from the source texel density, not the 128 px default.
        let atlas = convert(&m, &ps, &hd, None, &|_, _| {});
        let (n, _) = atlas.stats.atlas.unwrap();
        assert!((256..=512).contains(&n), "atlas {n}");
        let tex = atlas.model.materials[0].albedo.as_ref().unwrap();
        assert!(texture::count_colors(tex) > 256);
        assert_eq!(hd_atlas_size(&[], &[]), 1024, "untextured fallback");
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
