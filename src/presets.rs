//! Conversion presets: built-in templates for common PS1-era asset types and art styles,
//! plus user presets stored as JSON files (one per preset) in the config folder.

use crate::export::ExportSettings;
use crate::process::{ConvertSettings, TextureSizing, UvMode};
use crate::texture::{DitherMode, PaletteMode, ResizeFilter};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub convert: ConvertSettings,
    /// Export settings are optional: built-ins leave your engine choice alone.
    #[serde(default)]
    pub export: Option<ExportSettings>,
    #[serde(skip)]
    pub builtin: bool,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

fn template(
    category: &str,
    name: &str,
    description: &str,
    f: impl FnOnce(&mut ConvertSettings),
) -> Preset {
    let mut convert = ConvertSettings::default();
    f(&mut convert);
    Preset {
        name: name.into(),
        category: category.into(),
        description: description.into(),
        convert,
        export: None,
        builtin: true,
        path: None,
    }
}

/// Built-in templates, grouped by category.
pub fn builtins() -> Vec<Preset> {
    vec![
        template(
            "Characters",
            "Hero character",
            "Main playable character: ~1500 triangles, 256 px atlas, 64 colours. Late-era PS1 hero quality.",
            |s| {
                s.target_tris = 1500;
                s.texture.max_size = 256;
                s.texture.colors = 64;
                s.texture.dither_amount = 10.0;
                s.ao_strength = 0.6;
            },
        ),
        template(
            "Characters",
            "NPC / enemy",
            "Crowd and enemy characters: ~600 triangles, 128 px, 16 colours (4-bit CLUT).",
            |s| {
                s.target_tris = 600;
                s.texture.max_size = 128;
                s.texture.colors = 16;
                s.ao_strength = 0.6;
            },
        ),
        template(
            "Characters",
            "Survival horror character",
            "Dark, desaturated, heavy AO and the PS1 dither table: ~900 triangles, 128 px, 32 colours.",
            |s| {
                s.target_tris = 900;
                s.texture.max_size = 128;
                s.texture.colors = 32;
                s.texture.brightness = -0.05;
                s.texture.contrast = 0.2;
                s.texture.saturation = -0.3;
                s.texture.dither = DitherMode::Ps1Table;
                s.texture.dither_amount = 16.0;
                s.ao_strength = 0.85;
            },
        ),
        template(
            "Characters",
            "Boss / showcase",
            "Hero-shot or boss model: ~2500 triangles, 256 px, full 256-colour CLUT, light dither.",
            |s| {
                s.target_tris = 2500;
                s.texture.max_size = 256;
                s.texture.colors = 256;
                s.texture.dither_amount = 6.0;
            },
        ),
        template(
            "Environment",
            "Architecture (tiling)",
            "Rooms, walls and floors: keeps tiling UVs, keeps inside faces, splits polygons over 1 unit against affine warping, strong AO.",
            |s| {
                s.target_tris = 2000;
                s.uv_mode = UvMode::Keep;
                s.remove_hidden = false;
                s.subdivide_max_edge = 1.0;
                s.texture.max_size = 128;
                s.texture.colors = 32;
                s.ao_strength = 0.8;
            },
        ),
        template(
            "Environment",
            "Environment prop",
            "Furniture, crates, debris: ~300 triangles, 2 cm texels so every prop shares one texel density, sits on the ground.",
            |s| {
                s.target_tris = 300;
                s.sizing = TextureSizing::PixelSize;
                s.texel_cm = 2.0;
                s.texture.colors = 16;
                s.hidden_ignore_below = true;
                s.ao_strength = 0.8;
            },
        ),
        template(
            "Environment",
            "Distant LOD / background",
            "Far-away scenery: 3% of the source, 32 px, 8 colours, no dither.",
            |s| {
                s.use_percent = true;
                s.percent = 3.0;
                s.texture.max_size = 32;
                s.texture.colors = 8;
                s.texture.dither = DitherMode::None;
                s.vertex_ao = false;
            },
        ),
        template(
            "Props & items",
            "Item / pickup",
            "Keys, ammo, health: ~150 triangles, 64 px, 16 colours, soft AO.",
            |s| {
                s.target_tris = 150;
                s.texture.max_size = 64;
                s.texture.colors = 16;
                s.ao_strength = 0.4;
            },
        ),
        template(
            "Props & items",
            "Weapon (first person)",
            "Viewmodel weapon seen up close: ~800 triangles, 256 px, 64 colours.",
            |s| {
                s.target_tris = 800;
                s.texture.max_size = 256;
                s.texture.colors = 64;
                s.ao_strength = 0.5;
            },
        ),
        template(
            "Props & items",
            "Photoscan prop",
            "Phone/drone scans: ~500 triangles, everything rebaked into a fresh atlas, stronger normal relief, wider bake search for noisy scans.",
            |s| {
                s.target_tris = 500;
                s.texture.colors = 32;
                s.uv_mode = UvMode::Atlas;
                s.bake_distance = 5.0;
                s.normal_map_strength = 1.2;
                s.hidden_ignore_below = true;
            },
        ),
        template(
            "Vehicles",
            "Vehicle",
            "Cars and bikes: ~600 triangles, 128 px, 32 colours, baked sun with shadows, punchier colour.",
            |s| {
                s.target_tris = 600;
                s.texture.max_size = 128;
                s.texture.colors = 32;
                s.texture.saturation = 0.15;
                s.texture.contrast = 0.1;
                s.hidden_ignore_below = true;
                s.sun = true;
            },
        ),
        template(
            "Style",
            "Strict PS1 hardware",
            "Hardware-faithful: 128 px texture page, 4-bit CLUT, 15-bit colour, the GPU dither table, cutout alpha.",
            |s| {
                s.texture.max_size = 128;
                s.texture.colors = 16;
                s.texture.rgb555 = true;
                s.texture.dither = DitherMode::Ps1Table;
                s.texture.dither_amount = 8.0;
                s.texture.alpha = crate::texture::AlphaMode::Cutout;
            },
        ),
        template(
            "Style",
            "Concept art match",
            "Match a concept painting: load its palette first (Palette > Load, or extract one from the concept image); picks the 16 best entries, punchy contrast.",
            |s| {
                s.texture.palette_mode = PaletteMode::ImportedBestN;
                s.texture.colors = 16;
                s.texture.contrast = 0.15;
                s.texture.saturation = 0.1;
                s.texture.dither_amount = 16.0;
            },
        ),
        template(
            "Style",
            "Clean low-poly (no dither)",
            "Smooth flat-shaded look: no dither, no 15-bit snap, 32 colours, gentle AO.",
            |s| {
                s.texture.dither = DitherMode::None;
                s.texture.rgb555 = false;
                s.texture.colors = 32;
                s.ao_strength = 0.5;
            },
        ),
        // Beyond PS1 limits: for PS2/Dreamcast-era looks, remasters, or a clean
        // high-detail master to crunch further later.
        template(
            "High quality",
            "HQ character",
            "PS2/Dreamcast-era character: ~6000 triangles, 1024 px atlas, 256 colours, no dither or 15-bit snap, soft AO.",
            |s| {
                s.target_tris = 6000;
                s.texture.max_size = 1024;
                s.texture.colors = 256;
                s.texture.dither = DitherMode::None;
                s.texture.rgb555 = false;
                s.texture.filter = ResizeFilter::Sharp;
                s.ao_strength = 0.5;
            },
        ),
        template(
            "High quality",
            "HQ prop",
            "Detailed prop: ~2500 triangles, 512 px, 256 colours, very light dither, no 15-bit snap.",
            |s| {
                s.target_tris = 2500;
                s.texture.max_size = 512;
                s.texture.colors = 256;
                s.texture.dither_amount = 4.0;
                s.texture.rgb555 = false;
                s.texture.filter = ResizeFilter::Sharp;
                s.ao_strength = 0.5;
            },
        ),
        template(
            "High quality",
            "HQ environment",
            "Detailed rooms and set pieces: ~8000 triangles, keeps tiling UVs and inside faces, 512 px, 256 colours, no dither.",
            |s| {
                s.target_tris = 8000;
                s.uv_mode = UvMode::Keep;
                s.remove_hidden = false;
                s.texture.max_size = 512;
                s.texture.colors = 256;
                s.texture.dither = DitherMode::None;
                s.texture.rgb555 = false;
                s.ao_strength = 0.6;
            },
        ),
        template(
            "High quality",
            "HQ vehicle",
            "Hero vehicle: ~5000 triangles, 1024 px, 256 colours, baked sun, light dither.",
            |s| {
                s.target_tris = 5000;
                s.texture.max_size = 1024;
                s.texture.colors = 256;
                s.texture.dither_amount = 4.0;
                s.texture.rgb555 = false;
                s.texture.filter = ResizeFilter::Sharp;
                s.hidden_ignore_below = true;
                s.sun = true;
            },
        ),
        template(
            "High quality",
            "PSX model, HD texture",
            "Low-poly PS1 geometry (~1500 triangles) wearing the source's full-resolution, full-colour texture: pixel-sharp but HD.",
            |s| {
                s.target_tris = 1500;
                s.hd_texture = true;
                s.ao_strength = 0.5;
            },
        ),
        template(
            "High quality",
            "Late-90s PC (high-res textures)",
            "Early 3D-accelerator look: ~3000 triangles, 512 px, 256 colours, 15-bit colour with ordered (Bayer) dither.",
            |s| {
                s.target_tris = 3000;
                s.texture.max_size = 512;
                s.texture.colors = 256;
                s.texture.rgb555 = true;
                s.texture.dither = DitherMode::Bayer4;
                s.texture.dither_amount = 8.0;
            },
        ),
        template(
            "High quality",
            "Max quality (archive)",
            "Near-lossless master: keeps 50% of the triangles, 2048 px atlas, 256 colours, no dither or 15-bit snap. Crunch it further later.",
            |s| {
                s.use_percent = true;
                s.percent = 50.0;
                s.texture.max_size = 2048;
                s.texture.colors = 256;
                s.texture.dither = DitherMode::None;
                s.texture.rgb555 = false;
                s.texture.filter = ResizeFilter::Sharp;
                s.ao_strength = 0.4;
            },
        ),
    ]
}

/// `%APPDATA%\psx-forge\presets` on Windows, `$XDG_CONFIG_HOME` or `~/.config/psx-forge/presets` elsewhere.
pub fn user_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PSXFORGE_PRESETS").map(PathBuf::from) {
        return d;
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("psx-forge")
        .join("presets")
}

fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "preset".into() } else { s }
}

/// User presets from `dir`; unreadable files are reported, not fatal.
pub fn load_user_from(dir: &Path) -> (Vec<Preset>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (out, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for p in paths {
        match read(&p) {
            Ok(mut preset) => {
                preset.path = Some(p);
                out.push(preset);
            }
            Err(e) => errors.push(format!("{}: {e:#}", p.display())),
        }
    }
    (out, errors)
}

pub fn load_user() -> (Vec<Preset>, Vec<String>) {
    load_user_from(&user_dir())
}

pub fn read(path: &Path) -> Result<Preset> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut p: Preset = serde_json::from_str(&text)
        .with_context(|| format!("parsing preset {}", path.display()))?;
    if p.name.trim().is_empty() {
        p.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("preset")
            .to_string();
    }
    Ok(p)
}

/// Write a preset as JSON (for sharing or the user folder).
pub fn write(path: &Path, p: &Preset) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(p)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// Save into `dir` (overwrites a user preset with the same name). Built-in names are reserved.
pub fn save_user_in(dir: &Path, p: &Preset) -> Result<PathBuf> {
    if p.name.trim().is_empty() {
        bail!("give the preset a name");
    }
    if builtins()
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(p.name.trim()))
    {
        bail!(
            "'{}' is a built-in preset name; pick another",
            p.name.trim()
        );
    }
    let path = dir.join(format!("{}.json", slug(&p.name)));
    write(
        &path,
        &Preset {
            builtin: false,
            path: None,
            ..p.clone()
        },
    )?;
    Ok(path)
}

pub fn save_user(p: &Preset) -> Result<PathBuf> {
    save_user_in(&user_dir(), p)
}

pub fn delete_user(p: &Preset) -> Result<()> {
    match &p.path {
        Some(path) if !p.builtin => {
            std::fs::remove_file(path).with_context(|| format!("deleting {}", path.display()))
        }
        _ => bail!("built-in presets can't be deleted"),
    }
}

/// A preset by file path, user preset name, or built-in name (case-insensitive).
/// Whether `find` treats `name_or_path` as a file path (anything that looks like a path
/// or has a .json extension). MCP uses the same test to sandbox it.
pub fn is_path_like(name_or_path: &str) -> bool {
    let p = Path::new(name_or_path);
    p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        || name_or_path.contains(['/', '\\'])
        || p.is_absolute()
}

pub fn find(name_or_path: &str) -> Result<Preset> {
    let p = Path::new(name_or_path);
    if is_path_like(name_or_path) {
        return read(p);
    }
    let (user, _) = load_user();
    user.into_iter()
        .chain(builtins())
        .find(|x| x.name.eq_ignore_ascii_case(name_or_path.trim()))
        .with_context(|| {
            let names: Vec<String> = builtins()
                .into_iter()
                .map(|b| b.name)
                .chain(load_user().0.into_iter().map(|u| u.name))
                .collect();
            format!(
                "no preset '{name_or_path}' (available: {})",
                names.join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_distinct_and_serialisable() {
        let b = builtins();
        assert!(b.len() >= 12);
        let mut names: Vec<&str> = b.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), b.len(), "unique names");
        for p in &b {
            let json = serde_json::to_string(p).unwrap();
            let back: Preset = serde_json::from_str(&json).unwrap();
            assert_eq!(
                serde_json::to_string(&back.convert).unwrap(),
                serde_json::to_string(&p.convert).unwrap()
            );
        }
        let horror = b
            .iter()
            .find(|p| p.name == "Survival horror character")
            .unwrap();
        assert_eq!(horror.convert.texture.dither, DitherMode::Ps1Table);
        let hq: Vec<&Preset> = b
            .iter()
            .filter(|p| p.category == "High quality" && !p.convert.hd_texture)
            .collect();
        assert!(hq.len() >= 5, "high-quality templates present");
        assert!(hq.iter().all(|p| p.convert.texture.colors == 256));
        assert!(hq.iter().any(|p| p.convert.texture.max_size >= 1024));
    }

    #[test]
    fn user_presets_save_load_delete_and_reserve_builtin_names() {
        let dir = std::env::temp_dir().join("psxforge_presets_test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = builtins().remove(0);
        p.name = "My Characters!".into();
        p.builtin = false;
        p.convert.target_tris = 777;
        let path = save_user_in(&dir, &p).unwrap();
        assert!(path.ends_with("my_characters.json"));
        let (loaded, errors) = load_user_from(&dir);
        assert!(errors.is_empty());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].convert.target_tris, 777);
        assert!(!loaded[0].builtin);
        // Older/partial files still load: missing fields take defaults.
        std::fs::write(
            dir.join("partial.json"),
            r#"{"name":"Partial","convert":{"target_tris":42}}"#,
        )
        .unwrap();
        let (loaded, _) = load_user_from(&dir);
        let partial = loaded.iter().find(|p| p.name == "Partial").unwrap();
        assert_eq!(partial.convert.target_tris, 42);
        assert_eq!(
            partial.convert.texture.max_size,
            ConvertSettings::default().texture.max_size
        );
        delete_user(&loaded[0]).unwrap();
        assert!(
            save_user_in(
                &dir,
                &Preset {
                    name: "hero character".into(),
                    ..p
                }
            )
            .is_err(),
            "built-in names are reserved"
        );
        assert!(delete_user(&builtins()[0]).is_err());
    }
}
