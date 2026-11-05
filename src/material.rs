//! PBR material sets: a colour map plus normal / height / AO / metallic / roughness /
//! emission maps that share a name (`Bricks_basecolor.jpg`, `Bricks_normalogl.jpg`, ...).
//! Finds the sibling maps, shrinks them all onto the colour texture's pixel grid, packs
//! AO, roughness and metallic into one ORM texture and writes a Godot material.

use crate::palette::Rgb;
use crate::texture::{self, BakeMaps, CrunchSettings, Crunched, ResizeFilter};
use anyhow::{Context, Result};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
    Color,
    Normal,
    Height,
    Ao,
    Metallic,
    Roughness,
    Emission,
}

impl MapKind {
    pub const ALL: [MapKind; 7] = [
        Self::Color,
        Self::Normal,
        Self::Height,
        Self::Ao,
        Self::Metallic,
        Self::Roughness,
        Self::Emission,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Color => "colour",
            Self::Normal => "normal",
            Self::Height => "height",
            Self::Ao => "AO",
            Self::Metallic => "metallic",
            Self::Roughness => "roughness",
            Self::Emission => "emission",
        }
    }

    /// File-name suffixes, most preferred first (`_normalogl` wins over `_normal`).
    fn suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Color => &[
                "basecolor",
                "base_color",
                "albedo",
                "diffuse",
                "diff",
                "color",
                "col",
            ],
            Self::Normal => &[
                "normalogl",
                "normal_ogl",
                "normal_gl",
                "normalgl",
                "nor_gl",
                "normal",
                "nrm",
                "nor",
                "normaldx",
                "normal_dx",
                "nor_dx",
            ],
            Self::Height => &["height", "displacement", "disp", "bump"],
            Self::Ao => &["ambientocclusion", "ambient_occlusion", "occlusion", "ao"],
            Self::Metallic => &["metallic", "metalness", "metal"],
            Self::Roughness => &["roughness", "rough"],
            Self::Emission => &["emissive", "emission"],
        }
    }
}

/// DirectX normal maps (green = down); their green channel is flipped on load.
const DIRECTX_NORMALS: [&str; 3] = ["normaldx", "normal_dx", "nor_dx"];
/// Lossless formats first: a 16-bit `_height.png` beats a `_height.jpg` of the same map.
const EXTS: [&str; 5] = ["png", "tga", "bmp", "jpg", "jpeg"];

/// Split `Bricks_01_basecolor` into (`Bricks_01`, Color, suffix rank). The longest
/// matching suffix wins, so `X_base_color` is `X` + `base_color`, not `X_base` + `color`.
fn split_name(stem: &str) -> Option<(&str, MapKind, usize)> {
    let lower = stem.to_ascii_lowercase();
    let mut best: Option<(&str, MapKind, usize, usize)> = None;
    for kind in MapKind::ALL {
        for (rank, suf) in kind.suffixes().iter().enumerate() {
            let Some(base_len) = lower.len().checked_sub(suf.len() + 1) else {
                continue;
            };
            let sep = lower.as_bytes()[base_len];
            if base_len == 0 || !lower.ends_with(suf) || !matches!(sep, b'_' | b'-' | b' ' | b'.') {
                continue;
            }
            if best.is_none_or(|b| suf.len() > b.3) {
                best = Some((&stem[..base_len], kind, rank, suf.len()));
            }
        }
    }
    best.map(|(base, kind, rank, _)| (base, kind, rank))
}

/// The files of one material set.
#[derive(Clone, Debug, Serialize)]
pub struct MaterialFiles {
    /// Shared name, e.g. `BricksWall_01`.
    pub name: String,
    pub maps: BTreeMap<MapKind, PathBuf>,
    /// The normal map is DirectX-style (green flipped on load).
    pub directx_normal: bool,
}

impl MaterialFiles {
    /// "colour, normal, height, AO" for status lines.
    pub fn summary(&self) -> String {
        self.maps
            .keys()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn image_ext_rank(p: &Path) -> Option<usize> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    EXTS.iter().position(|e| *e == ext)
}

/// Regular files in `dir` (symlinks are skipped so a scan can't leave the folder).
fn files_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let e = e?;
        if e.file_type()?.is_file() {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

/// The material set `path` belongs to: any of its maps works. `None` when the name has
/// no map suffix, or no colour map plus at least one other map is found next to it.
pub fn find_material(path: &Path) -> Result<Option<MaterialFiles>> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let Some((base, _, _)) = split_name(stem) else {
        return Ok(None);
    };
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(collect_set(base, &files_in(dir)?))
}

fn collect_set(base: &str, files: &[PathBuf]) -> Option<MaterialFiles> {
    // Best (suffix rank, extension rank) per kind.
    let mut best: BTreeMap<MapKind, ((usize, usize), PathBuf, bool)> = BTreeMap::new();
    for f in files {
        let (Some(ext_rank), Some(stem)) =
            (image_ext_rank(f), f.file_stem().and_then(|s| s.to_str()))
        else {
            continue;
        };
        let Some((b, kind, rank)) = split_name(stem) else {
            continue;
        };
        if !b.eq_ignore_ascii_case(base) {
            continue;
        }
        let suffix = stem[b.len() + 1..].to_ascii_lowercase();
        let dx = kind == MapKind::Normal && DIRECTX_NORMALS.contains(&suffix.as_str());
        let key = (rank, ext_rank);
        if best.get(&kind).is_none_or(|(k, _, _)| key < *k) {
            best.insert(kind, (key, f.clone(), dx));
        }
    }
    if !best.contains_key(&MapKind::Color) || best.len() < 2 {
        return None;
    }
    let directx_normal = best.get(&MapKind::Normal).is_some_and(|(_, _, dx)| *dx);
    Some(MaterialFiles {
        name: base.to_string(),
        maps: best.into_iter().map(|(k, (_, p, _))| (k, p)).collect(),
        directx_normal,
    })
}

/// Every material set under `dir` (recursively), one per colour map.
pub fn find_all(dir: &Path) -> Result<Vec<MaterialFiles>> {
    let mut sets = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut subdirs = Vec::new();
        for e in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let e = e?;
            if e.file_type()?.is_dir() {
                subdirs.push(e.path());
            }
        }
        let files = files_in(&d)?;
        let mut seen = Vec::new();
        for f in &files {
            let Some((base, MapKind::Color, _)) =
                f.file_stem().and_then(|s| s.to_str()).and_then(split_name)
            else {
                continue;
            };
            if image_ext_rank(f).is_none()
                || seen.iter().any(|s: &String| s.eq_ignore_ascii_case(base))
            {
                continue;
            }
            seen.push(base.to_string());
            sets.extend(collect_set(base, &files));
        }
        subdirs.sort();
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(sets)
}

/// The decoded maps of a set (normal map converted to OpenGL / Godot convention).
pub struct MaterialImages {
    pub color: RgbaImage,
    pub normal: Option<RgbaImage>,
    pub height: Option<GrayImage>,
    pub ao: Option<GrayImage>,
    pub metallic: Option<GrayImage>,
    pub roughness: Option<GrayImage>,
    pub emission: Option<RgbaImage>,
}

impl MaterialImages {
    pub fn load(f: &MaterialFiles) -> Result<Self> {
        use image::DynamicImage;
        // Decode every map on its own thread: six 4K images take about a second in a row.
        let decoded: Vec<(MapKind, DynamicImage)> = std::thread::scope(|sc| {
            let jobs: Vec<_> = f
                .maps
                .iter()
                .map(|(&k, p)| {
                    let job = sc.spawn(move || -> Result<DynamicImage> {
                        let i =
                            image::open(p).with_context(|| format!("opening {}", p.display()))?;
                        Ok(match k {
                            MapKind::Color | MapKind::Normal | MapKind::Emission => {
                                DynamicImage::ImageRgba8(i.into_rgba8())
                            }
                            _ => DynamicImage::ImageLuma8(i.into_luma8()),
                        })
                    });
                    (k, job)
                })
                .collect();
            jobs.into_iter()
                .map(|(k, job)| Ok((k, join(job)?)))
                .collect::<Result<_>>()
        })?;
        let mut maps: BTreeMap<MapKind, DynamicImage> = decoded.into_iter().collect();
        let mut rgba = |k| maps.remove(&k).map(DynamicImage::into_rgba8);
        let color = rgba(MapKind::Color).context("material set has no colour map")?;
        let mut normal = rgba(MapKind::Normal);
        let emission = rgba(MapKind::Emission);
        if f.directx_normal
            && let Some(n) = &mut normal
        {
            n.pixels_mut().for_each(|p| p.0[1] = 255 - p.0[1]);
        }
        let mut gray = |k| maps.remove(&k).map(DynamicImage::into_luma8);
        Ok(Self {
            color,
            normal,
            height: gray(MapKind::Height),
            ao: gray(MapKind::Ao),
            metallic: gray(MapKind::Metallic),
            roughness: gray(MapKind::Roughness),
            emission,
        })
    }
}

/// Join a scoped thread, re-raising its panic.
fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialSettings {
    /// Darken the colour map with the AO map (for engines that don't use the ORM).
    pub bake_ao: bool,
    pub ao_strength: f32,
    /// Paint the normal map's relief into the colour map as fixed top-left light
    /// (the PS1 way: no normal maps on the console).
    pub bake_normal: bool,
    pub normal_strength: f32,
    /// Also write a Godot ORMMaterial3D `.tres` and `.png.import` files.
    pub godot: bool,
}

impl Default for MaterialSettings {
    fn default() -> Self {
        Self {
            bake_ao: false,
            ao_strength: 1.0,
            bake_normal: false,
            normal_strength: 1.0,
            godot: true,
        }
    }
}

/// Every map at the colour texture's final size.
pub struct CrunchedMaterial {
    pub color: Crunched,
    pub normal: Option<RgbaImage>,
    pub height: Option<GrayImage>,
    /// R = ambient occlusion, G = roughness, B = metallic (glTF / Godot / Unreal).
    pub orm: Option<RgbaImage>,
    pub emission: Option<RgbaImage>,
}

fn shrink_gray(g: &GrayImage, w: u32, h: u32, filter: ResizeFilter) -> GrayImage {
    let rgba = RgbaImage::from_fn(g.width(), g.height(), |x, y| {
        let v = g.get_pixel(x, y).0[0];
        image::Rgba([v, v, v, 255])
    });
    let small = texture::resize(&rgba, w, h, filter);
    GrayImage::from_fn(w, h, |x, y| image::Luma([small.get_pixel(x, y).0[0]]))
}

/// Averaging normals shortens them: renormalise after the resize.
fn shrink_normal(n: &RgbaImage, w: u32, h: u32, filter: ResizeFilter) -> RgbaImage {
    let mut small = texture::resize(n, w, h, filter);
    for p in small.pixels_mut() {
        let v = glam::Vec3::new(
            p.0[0] as f32 / 127.5 - 1.0,
            p.0[1] as f32 / 127.5 - 1.0,
            p.0[2] as f32 / 127.5 - 1.0,
        )
        .normalize_or(glam::Vec3::Z);
        for (c, f) in p.0.iter_mut().zip([v.x, v.y, v.z]) {
            *c = ((f + 1.0) * 127.5).round().clamp(0.0, 255.0) as u8;
        }
        p.0[3] = 255;
    }
    small
}

/// The non-colour maps shrunk onto the colour texture's final pixel grid. They only
/// depend on the size and filter, so the GUI keeps them while colour settings change.
pub struct ShrunkMaps {
    pub size: (u32, u32),
    pub filter: ResizeFilter,
    pub normal: Option<RgbaImage>,
    pub height: Option<GrayImage>,
    pub ao: Option<GrayImage>,
    pub roughness: Option<GrayImage>,
    pub metallic: Option<GrayImage>,
    pub emission: Option<RgbaImage>,
}

/// Shrink every non-colour map to the size [`texture::crunch`] gives the colour map.
pub fn shrink_maps(m: &MaterialImages, s: &CrunchSettings) -> ShrunkMaps {
    let (w, h) = texture::target_dims(
        m.color.width(),
        m.color.height(),
        s.max_size,
        s.power_of_two,
    );
    let f = s.filter;
    // One thread per map: each is a full pass over a (often 4K) image.
    std::thread::scope(|sc| {
        let [height, ao, roughness, metallic] =
            [&m.height, &m.ao, &m.roughness, &m.metallic].map(|g| {
                g.as_ref()
                    .map(|g| sc.spawn(move || shrink_gray(g, w, h, f)))
            });
        let normal = m
            .normal
            .as_ref()
            .map(|n| sc.spawn(move || shrink_normal(n, w, h, f)));
        let emission = m
            .emission
            .as_ref()
            .map(|e| sc.spawn(move || texture::resize(e, w, h, f)));
        ShrunkMaps {
            size: (w, h),
            filter: f,
            normal: normal.map(join),
            height: height.map(join),
            ao: ao.map(join),
            roughness: roughness.map(join),
            metallic: metallic.map(join),
            emission: emission.map(join),
        }
    })
}

/// Crunch the colour map PS1-style (palette, dither, RGB555) and pack the shrunk maps,
/// so each texel of every map lines up.
pub fn crunch_with_maps(
    color: &RgbaImage,
    sm: &ShrunkMaps,
    s: &CrunchSettings,
    palette: Option<&[Rgb]>,
    ms: &MaterialSettings,
) -> CrunchedMaterial {
    // Baked from the shrunk maps: they are already on the colour texture's grid.
    let bake = BakeMaps {
        ao: sm.ao.as_ref().filter(|_| ms.bake_ao),
        ao_strength: ms.ao_strength,
        normal: sm.normal.as_ref().filter(|_| ms.bake_normal),
        normal_strength: ms.normal_strength,
        emissive: None,
    };
    let color = texture::crunch(color, s, &bake, palette);
    let (w, h) = sm.size;
    let (ao, rough, metal) = (&sm.ao, &sm.roughness, &sm.metallic);
    let orm = (ao.is_some() || rough.is_some() || metal.is_some()).then(|| {
        let at = |g: &Option<GrayImage>, x, y, missing| {
            g.as_ref().map_or(missing, |g| g.get_pixel(x, y).0[0])
        };
        // Missing maps: no occlusion, fully rough, not metallic.
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                at(ao, x, y, 255),
                at(rough, x, y, 255),
                at(metal, x, y, 0),
                255,
            ])
        })
    });
    CrunchedMaterial {
        normal: sm.normal.clone(),
        height: sm.height.clone(),
        orm,
        emission: sm.emission.clone(),
        color,
    }
}

/// [`shrink_maps`] + [`crunch_with_maps`].
pub fn crunch_material(
    m: &MaterialImages,
    s: &CrunchSettings,
    palette: Option<&[Rgb]>,
    ms: &MaterialSettings,
) -> CrunchedMaterial {
    crunch_with_maps(&m.color, &shrink_maps(m, s), s, palette, ms)
}

/// Godot 4 ORMMaterial3D using the written maps, with nearest filtering.
pub fn godot_material(name: &str, cm: &CrunchedMaterial) -> String {
    let mut ext = Vec::new();
    let mut props = Vec::new();
    let mut add = |file: String, id: &str, lines: &[&str]| {
        ext.push(format!(
            "[ext_resource type=\"Texture2D\" path=\"{file}\" id=\"{id}\"]"
        ));
        props.extend(lines.iter().map(|l| l.replace("{id}", id)));
    };
    add(
        format!("{name}_albedo.png"),
        "1_albedo",
        &["albedo_texture = ExtResource(\"{id}\")"],
    );
    if cm.orm.is_some() {
        add(
            format!("{name}_orm.png"),
            "2_orm",
            &["orm_texture = ExtResource(\"{id}\")"],
        );
    }
    if cm.normal.is_some() {
        add(
            format!("{name}_normal.png"),
            "3_normal",
            &[
                "normal_enabled = true",
                "normal_texture = ExtResource(\"{id}\")",
            ],
        );
    }
    if cm.height.is_some() {
        add(
            format!("{name}_height.png"),
            "4_height",
            &[
                "heightmap_enabled = true",
                "heightmap_texture = ExtResource(\"{id}\")",
            ],
        );
    }
    if cm.emission.is_some() {
        add(
            format!("{name}_emission.png"),
            "5_emission",
            &[
                "emission_enabled = true",
                "emission_texture = ExtResource(\"{id}\")",
            ],
        );
    }
    format!(
        "[gd_resource type=\"ORMMaterial3D\" load_steps={} format=3]\n\n{}\n\n[resource]\n{}\ntexture_filter = 0\n",
        ext.len() + 1,
        ext.join("\n"),
        props.join("\n")
    )
}

/// Write `<name>_albedo/_normal/_height/_orm/_emission.png` (and the Godot material)
/// into `dir`. Returns the written files.
pub fn export(
    dir: &Path,
    name: &str,
    cm: &CrunchedMaterial,
    indexed: bool,
    godot: bool,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut written = Vec::new();
    let mut save = |suffix: &str, img: RgbaImage, indexed: bool| -> Result<()> {
        let p = dir.join(format!("{name}_{suffix}.png"));
        texture::save_png(&p, &img, indexed)?;
        if godot {
            let imp = p.with_extension("png.import");
            std::fs::write(&imp, crate::export::godot_texture_import(false))?;
        }
        written.push(p);
        Ok(())
    };
    let gray_rgba = |g: &GrayImage| image::DynamicImage::ImageLuma8(g.clone()).to_rgba8();
    save("albedo", cm.color.image.clone(), indexed)?;
    if let Some(n) = &cm.normal {
        save("normal", n.clone(), false)?;
    }
    if let Some(h) = &cm.height {
        save("height", gray_rgba(h), false)?;
    }
    if let Some(o) = &cm.orm {
        save("orm", o.clone(), false)?;
    }
    if let Some(e) = &cm.emission {
        save("emission", e.clone(), false)?;
    }
    if godot {
        let p = dir.join(format!("{name}.tres"));
        std::fs::write(&p, godot_material(name, cm))
            .with_context(|| format!("writing {}", p.display()))?;
        written.push(p);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_names_by_longest_suffix() {
        assert_eq!(
            split_name("BricksWall_01_basecolor"),
            Some(("BricksWall_01", MapKind::Color, 0))
        );
        assert_eq!(
            split_name("BricksWall_01_ambientocclusion").map(|s| s.1),
            Some(MapKind::Ao)
        );
        assert_eq!(split_name("rock_base_color").map(|s| s.0), Some("rock"));
        assert_eq!(
            split_name("rock_normal_gl").map(|s| s.1),
            Some(MapKind::Normal)
        );
        // A bare suffix or no separator isn't a map name.
        assert_eq!(split_name("normal"), None);
        assert_eq!(split_name("abnormal"), None);
    }

    fn write_set(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for n in names {
            let px = if n.contains("normal") {
                [128, 128, 255, 255]
            } else {
                [200; 4]
            };
            image::RgbImage::from_pixel(64, 32, image::Rgb([px[0], px[1], px[2]]))
                .save(dir.join(n))
                .unwrap();
        }
    }

    #[test]
    fn finds_siblings_and_prefers_opengl_normals_and_png() {
        let dir = std::env::temp_dir().join("retrocrunch_material_find");
        let _ = std::fs::remove_dir_all(&dir);
        write_set(
            &dir,
            &[
                "Wall_basecolor.png",
                "Wall_normal.png",
                "Wall_normalogl.png",
                "Wall_height.jpg",
                "Wall_height.png",
                "Wall_roughness.png",
                "Other_basecolor.png",
            ],
        );
        // Opening any map of the set finds the whole set.
        let m = find_material(&dir.join("Wall_roughness.png"))
            .unwrap()
            .unwrap();
        assert_eq!(m.name, "Wall");
        assert!(m.maps[&MapKind::Normal].ends_with("Wall_normalogl.png"));
        assert!(m.maps[&MapKind::Height].ends_with("Wall_height.png"));
        assert!(!m.directx_normal);
        assert_eq!(m.maps.len(), 4);
        // A lone colour map is not a material set.
        assert!(
            find_material(&dir.join("Other_basecolor.png"))
                .unwrap()
                .is_none()
        );
        let all = find_all(&dir).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn every_map_lands_on_the_colour_grid_and_orm_is_packed() {
        let dir = std::env::temp_dir().join("retrocrunch_material_crunch");
        let _ = std::fs::remove_dir_all(&dir);
        write_set(
            &dir,
            &[
                "M_albedo.png",
                "M_normaldx.png",
                "M_ao.png",
                "M_metallic.png",
                "M_height.png",
            ],
        );
        let f = find_material(&dir.join("M_albedo.png")).unwrap().unwrap();
        assert!(f.directx_normal);
        let imgs = MaterialImages::load(&f).unwrap();
        let s = CrunchSettings {
            max_size: 16,
            ..Default::default()
        };
        let cm = crunch_material(&imgs, &s, None, &MaterialSettings::default());
        let dims = cm.color.image.dimensions();
        assert_eq!(dims, (16, 8));
        assert_eq!(cm.normal.as_ref().unwrap().dimensions(), dims);
        assert_eq!(cm.height.as_ref().unwrap().dimensions(), dims);
        let orm = cm.orm.as_ref().unwrap();
        assert_eq!(orm.dimensions(), dims);
        // AO 200 in R, missing roughness = fully rough in G, metallic 200 in B.
        assert_eq!(orm.get_pixel(3, 3).0, [200, 255, 200, 255]);
        // DirectX green (128) flipped to OpenGL, still unit length.
        let n = cm.normal.as_ref().unwrap().get_pixel(0, 0).0;
        let v = glam::Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) / 127.5 - 1.0;
        assert!((v.length() - 1.0).abs() < 0.02, "{v:?}");

        let out = dir.join("out");
        let files = export(&out, "M", &cm, true, true).unwrap();
        for f in [
            "M_albedo.png",
            "M_normal.png",
            "M_height.png",
            "M_orm.png",
            "M.tres",
        ] {
            assert!(files.iter().any(|p| p.ends_with(f)), "{f} in {files:?}");
        }
        let tres = std::fs::read_to_string(out.join("M.tres")).unwrap();
        assert!(tres.contains("orm_texture = ExtResource(\"2_orm\")"));
        assert!(tres.contains("normal_enabled = true"));
        assert!(tres.contains("texture_filter = 0"));
        assert!(out.join("M_orm.png.import").exists());
    }

    #[test]
    fn baking_ao_darkens_the_colour_map() {
        let images = MaterialImages {
            color: RgbaImage::from_pixel(32, 32, image::Rgba([200, 200, 200, 255])),
            normal: None,
            height: None,
            ao: Some(GrayImage::from_pixel(32, 32, image::Luma([64]))),
            metallic: None,
            roughness: None,
            emission: None,
        };
        let s = CrunchSettings {
            max_size: 16,
            dither: texture::DitherMode::None,
            ..Default::default()
        };
        let luma = |ms: &MaterialSettings| {
            let cm = crunch_material(&images, &s, None, ms);
            cm.color.image.get_pixel(8, 8).0[0]
        };
        let plain = luma(&MaterialSettings::default());
        let baked = luma(&MaterialSettings {
            bake_ao: true,
            ..Default::default()
        });
        assert!(baked < plain / 2, "baked {baked} vs plain {plain}");
    }
}
