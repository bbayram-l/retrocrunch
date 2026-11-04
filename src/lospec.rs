//! Palettes from lospec.com: 10 built in, plus browsing and importing the Lospec
//! palette list. Network replies are untrusted: slugs and tags are validated before
//! they go into a URL, replies are size-capped, and colours must be strict hex.

use crate::palette::{self, Rgb};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

pub struct Builtin {
    pub slug: &'static str,
    pub name: &'static str,
    pub author: &'static str,
    pub note: &'static str,
    pub colors: &'static [u32],
}

/// A palette from Lospec (or a built-in one).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LospecPalette {
    pub slug: String,
    pub name: String,
    pub author: String,
    pub colors: Vec<Rgb>,
}

impl Builtin {
    pub fn palette(&self) -> LospecPalette {
        LospecPalette {
            slug: self.slug.into(),
            name: self.name.into(),
            author: self.author.into(),
            colors: self
                .colors
                .iter()
                .map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
                .collect(),
        }
    }
}

/// Prefix for built-in palettes wherever a palette path is accepted.
pub const BUILTIN_PREFIX: &str = "builtin:";

/// Built-in palette by slug or name (case-insensitive).
pub fn builtin(name: &str) -> Option<LospecPalette> {
    let n = name.trim().to_lowercase();
    BUILTINS
        .iter()
        .find(|b| b.slug == n || b.name.to_lowercase() == n)
        .map(Builtin::palette)
}

const HOST: &str = "https://lospec.com/palette-list";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Replies bigger than this are refused (a list page is ~30 KB).
const MAX_REPLY: u64 = 4 << 20;

/// A palette slug from a slug or a lospec.com URL: lower-case letters, digits and '-'.
pub fn parse_slug(input: &str) -> Result<String> {
    let s = input.trim().trim_end_matches('/');
    let s = s
        .rsplit_once("/palette-list/")
        .map_or(s, |(_, rest)| rest)
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".json")
        .to_lowercase();
    if s.is_empty()
        || s.len() > 80
        || !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!(
            "'{input}' is not a Lospec palette name (e.g. pico-8 or https://lospec.com/palette-list/pico-8)"
        );
    }
    Ok(s)
}

fn get(url: &str) -> Result<String> {
    let reply = ureq::get(url)
        .timeout(TIMEOUT)
        .set(
            "User-Agent",
            concat!("psx-forge/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let mut body = String::new();
    reply
        .into_reader()
        .take(MAX_REPLY + 1)
        .read_to_string(&mut body)
        .context("reading the reply")?;
    if body.len() as u64 > MAX_REPLY {
        bail!("reply from {url} is too large");
    }
    Ok(body)
}

fn hex_colors(v: &serde_json::Value) -> Result<Vec<Rgb>> {
    let list = v.as_array().context("no colour list")?;
    if list.is_empty() || list.len() > 256 {
        bail!("palette has {} colours (1-256 supported)", list.len());
    }
    list.iter()
        .map(|c| {
            let s = c
                .as_str()
                .context("colour is not text")?
                .trim_start_matches('#');
            if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("bad colour '{s}'");
            }
            let n = u32::from_str_radix(s, 16)?;
            Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8])
        })
        .collect()
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect()
}

/// Parse `palette-list/<slug>.json`: {"name", "author", "colors": ["rrggbb", ...]}.
pub fn parse_palette(slug: &str, json: &str) -> Result<LospecPalette> {
    let v: serde_json::Value = serde_json::from_str(json).context("Lospec reply is not JSON")?;
    Ok(LospecPalette {
        slug: slug.to_string(),
        name: Some(text(&v, "name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| slug.to_string()),
        author: text(&v, "author"),
        colors: hex_colors(&v["colors"])?,
    })
}

/// Download one palette by slug or URL.
pub fn fetch(slug_or_url: &str) -> Result<LospecPalette> {
    let slug = parse_slug(slug_or_url)?;
    let body = get(&format!("{HOST}/{slug}.json"))?;
    parse_palette(&slug, &body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CountFilter {
    Any,
    Max,
    Min,
    Exact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Sorting {
    Default,
    Alphabetical,
    Downloads,
    Newest,
}

impl Sorting {
    pub const ALL: [Sorting; 4] = [
        Self::Default,
        Self::Downloads,
        Self::Newest,
        Self::Alphabetical,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Featured",
            Self::Alphabetical => "A-Z",
            Self::Downloads => "Most downloaded",
            Self::Newest => "Newest",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Alphabetical => "alphabetical",
            Self::Downloads => "downloads",
            Self::Newest => "newest",
        }
    }
}

/// One page of the Lospec palette list.
#[derive(Clone, Debug)]
pub struct ListQuery {
    /// Tag such as "horror", "nes", "gameboy" (empty = any).
    pub tag: String,
    pub count: CountFilter,
    pub colors: usize,
    pub sorting: Sorting,
    pub page: usize,
}

impl Default for ListQuery {
    fn default() -> Self {
        Self {
            tag: String::new(),
            count: CountFilter::Any,
            colors: 16,
            sorting: Sorting::Default,
            page: 0,
        }
    }
}

impl ListQuery {
    pub fn url(&self) -> Result<String> {
        let tag = self.tag.trim().to_lowercase().replace(' ', "-");
        if !tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || tag.len() > 40
        {
            bail!("tags are letters, digits and '-' (e.g. horror, gameboy, nes)");
        }
        let count = match self.count {
            CountFilter::Any => "any",
            CountFilter::Max => "max",
            CountFilter::Min => "min",
            CountFilter::Exact => "exact",
        };
        Ok(format!(
            "{HOST}/load?colorNumberFilterType={count}&colorNumber={}&page={}&tag={tag}&sortingType={}",
            self.colors.clamp(2, 256),
            self.page.min(10_000),
            self.sorting.key()
        ))
    }
}

/// Parse a list page: {"palettes": [{"title", "slug", "colorsArray"|"colors", "user": {"name"}}], "totalCount"}.
pub fn parse_list(json: &str) -> Result<(Vec<LospecPalette>, usize)> {
    let v: serde_json::Value = serde_json::from_str(json).context("Lospec reply is not JSON")?;
    let total = v["totalCount"].as_u64().unwrap_or(0) as usize;
    let items = v["palettes"]
        .as_array()
        .context("no palette list in the reply")?;
    let list = items
        .iter()
        .filter_map(|p| {
            let slug = parse_slug(p["slug"].as_str()?).ok()?;
            let colors = hex_colors(
                p.get("colorsArray")
                    .filter(|c| c.is_array())
                    .unwrap_or(&p["colors"]),
            )
            .ok()?;
            let author = p["user"]["name"].as_str().map(|_| text(&p["user"], "name"));
            Some(LospecPalette {
                name: Some(text(p, "title"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| slug.clone()),
                slug,
                author: author.unwrap_or_default(),
                colors,
            })
        })
        .collect();
    Ok((list, total))
}

/// Fetch one page of the Lospec palette list: (palettes, total matching).
pub fn fetch_list(q: &ListQuery) -> Result<(Vec<LospecPalette>, usize)> {
    parse_list(&get(&q.url()?)?)
}

/// Where imported palettes are kept: `PSXFORGE_PALETTES`, else
/// `%APPDATA%\psx-forge\palettes` / `~/.config/psx-forge/palettes`.
pub fn user_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PSXFORGE_PALETTES") {
        return PathBuf::from(d);
    }
    crate::presets::user_dir()
        .parent()
        .map_or_else(|| PathBuf::from("palettes"), |p| p.join("palettes"))
}

/// Save to the user palette folder as `<slug>.gpl` (name and author in the header).
pub fn save_user(p: &LospecPalette) -> Result<PathBuf> {
    let dir = user_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let slug = parse_slug(&p.slug)?;
    let path = dir.join(format!("{slug}.gpl"));
    let mut s = format!(
        "GIMP Palette\nName: {}\nColumns: 8\n",
        p.name.replace('\n', " ")
    );
    if !p.author.is_empty() {
        s.push_str(&format!("# Author: {}\n", p.author.replace('\n', " ")));
    }
    s.push_str(&format!("# https://lospec.com/palette-list/{slug}\n"));
    for c in &p.colors {
        s.push_str(&format!(
            "{:3} {:3} {:3}\t{:02x}{:02x}{:02x}\n",
            c[0], c[1], c[2], c[0], c[1], c[2]
        ));
    }
    std::fs::write(&path, s).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Palettes in the user folder: (name, path, colours).
pub fn load_user() -> Vec<(String, PathBuf, Vec<Rgb>)> {
    let Ok(rd) = std::fs::read_dir(user_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf, Vec<Rgb>)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["gpl", "hex", "pal", "txt"].contains(&e.to_ascii_lowercase().as_str())
            })
        })
        .filter_map(|p| {
            let colors = palette::load_palette(&p).ok()?;
            let name = p.file_stem()?.to_str()?.to_string();
            Some((name, p, colors))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Built-in palettes (colours from lospec.com; authors credited).
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        slug: "pico-8",
        name: "PICO-8",
        author: "Lexaloffle Games",
        note: "Fantasy console",
        colors: &[
            0x000000, 0x1d2b53, 0x7e2553, 0x008751, 0xab5236, 0x5f574f, 0xc2c3c7, 0xfff1e8,
            0xff004d, 0xffa300, 0xffec27, 0x00e436, 0x29adff, 0x83769c, 0xff77a8, 0xffccaa,
        ],
    },
    Builtin {
        slug: "sweetie-16",
        name: "Sweetie 16",
        author: "GrafxKid",
        note: "Soft, friendly 16",
        colors: &[
            0x1a1c2c, 0x5d275d, 0xb13e53, 0xef7d57, 0xffcd75, 0xa7f070, 0x38b764, 0x257179,
            0x29366f, 0x3b5dc9, 0x41a6f6, 0x73eff7, 0xf4f4f4, 0x94b0c2, 0x566c86, 0x333c57,
        ],
    },
    Builtin {
        slug: "dawnbringer-16",
        name: "DawnBringer 16",
        author: "DawnBringer",
        note: "Classic all-rounder",
        colors: &[
            0x140c1c, 0x442434, 0x30346d, 0x4e4a4e, 0x854c30, 0x346524, 0xd04648, 0x757161,
            0x597dce, 0xd27d2c, 0x8595a1, 0x6daa2c, 0xd2aa99, 0x6dc2ca, 0xdad45e, 0xdeeed6,
        ],
    },
    Builtin {
        slug: "dawnbringer-32",
        name: "DawnBringer 32",
        author: "DawnBringer",
        note: "Classic all-rounder, 32",
        colors: &[
            0x000000, 0x222034, 0x45283c, 0x663931, 0x8f563b, 0xdf7126, 0xd9a066, 0xeec39a,
            0xfbf236, 0x99e550, 0x6abe30, 0x37946e, 0x4b692f, 0x524b24, 0x323c39, 0x3f3f74,
            0x306082, 0x5b6ee1, 0x639bff, 0x5fcde4, 0xcbdbfc, 0xffffff, 0x9badb7, 0x847e87,
            0x696a6a, 0x595652, 0x76428a, 0xac3232, 0xd95763, 0xd77bba, 0x8f974a, 0x8a6f30,
        ],
    },
    Builtin {
        slug: "endesga-32",
        name: "Endesga 32",
        author: "ENDESGA",
        note: "Vivid, game-ready",
        colors: &[
            0xbe4a2f, 0xd77643, 0xead4aa, 0xe4a672, 0xb86f50, 0x733e39, 0x3e2731, 0xa22633,
            0xe43b44, 0xf77622, 0xfeae34, 0xfee761, 0x63c74d, 0x3e8948, 0x265c42, 0x193c3e,
            0x124e89, 0x0099db, 0x2ce8f5, 0xffffff, 0xc0cbdc, 0x8b9bb4, 0x5a6988, 0x3a4466,
            0x262b44, 0x181425, 0xff0044, 0x68386c, 0xb55088, 0xf6757a, 0xe8b796, 0xc28569,
        ],
    },
    Builtin {
        slug: "aap-64",
        name: "AAP-64",
        author: "Adigun A. Polack",
        note: "Wide ramps",
        colors: &[
            0x060608, 0x141013, 0x3b1725, 0x73172d, 0xb4202a, 0xdf3e23, 0xfa6a0a, 0xf9a31b,
            0xffd541, 0xfffc40, 0xd6f264, 0x9cdb43, 0x59c135, 0x14a02e, 0x1a7a3e, 0x24523b,
            0x122020, 0x143464, 0x285cc4, 0x249fde, 0x20d6c7, 0xa6fcdb, 0xffffff, 0xfef3c0,
            0xfad6b8, 0xf5a097, 0xe86a73, 0xbc4a9b, 0x793a80, 0x403353, 0x242234, 0x221c1a,
            0x322b28, 0x71413b, 0xbb7547, 0xdba463, 0xf4d29c, 0xdae0ea, 0xb3b9d1, 0x8b93af,
            0x6d758d, 0x4a5462, 0x333941, 0x422433, 0x5b3138, 0x8e5252, 0xba756a, 0xe9b5a3,
            0xe3e6ff, 0xb9bffb, 0x849be4, 0x588dbe, 0x477d85, 0x23674e, 0x328464, 0x5daf8d,
            0x92dcba, 0xcdf7e2, 0xe4d2aa, 0xc7b08b, 0xa08662, 0x796755, 0x5a4e44, 0x423934,
        ],
    },
    Builtin {
        slug: "resurrect-64",
        name: "Resurrect 64",
        author: "Kerrie Lake",
        note: "Rich, painterly",
        colors: &[
            0x2e222f, 0x3e3546, 0x625565, 0x966c6c, 0xab947a, 0x694f62, 0x7f708a, 0x9babb2,
            0xc7dcd0, 0xffffff, 0x6e2727, 0xb33831, 0xea4f36, 0xf57d4a, 0xae2334, 0xe83b3b,
            0xfb6b1d, 0xf79617, 0xf9c22b, 0x7a3045, 0x9e4539, 0xcd683d, 0xe6904e, 0xfbb954,
            0x4c3e24, 0x676633, 0xa2a947, 0xd5e04b, 0xfbff86, 0x165a4c, 0x239063, 0x1ebc73,
            0x91db69, 0xcddf6c, 0x313638, 0x374e4a, 0x547e64, 0x92a984, 0xb2ba90, 0x0b5e65,
            0x0b8a8f, 0x0eaf9b, 0x30e1b9, 0x8ff8e2, 0x323353, 0x484a77, 0x4d65b4, 0x4d9be6,
            0x8fd3ff, 0x45293f, 0x6b3e75, 0x905ea9, 0xa884f3, 0xeaaded, 0x753c54, 0xa24b6f,
            0xcf657f, 0xed8099, 0x831c5d, 0xc32454, 0xf04f78, 0xf68181, 0xfca790, 0xfdcbb0,
        ],
    },
    Builtin {
        slug: "apollo",
        name: "Apollo",
        author: "AdamCYounis",
        note: "Natural ramps",
        colors: &[
            0x172038, 0x253a5e, 0x3c5e8b, 0x4f8fba, 0x73bed3, 0xa4dddb, 0x19332d, 0x25562e,
            0x468232, 0x75a743, 0xa8ca58, 0xd0da91, 0x4d2b32, 0x7a4841, 0xad7757, 0xc09473,
            0xd7b594, 0xe7d5b3, 0x341c27, 0x602c2c, 0x884b2b, 0xbe772b, 0xde9e41, 0xe8c170,
            0x241527, 0x411d31, 0x752438, 0xa53030, 0xcf573c, 0xda863e, 0x1e1d39, 0x402751,
            0x7a367b, 0xa23e8c, 0xc65197, 0xdf84a5, 0x090a14, 0x10141f, 0x151d28, 0x202e37,
            0x394a50, 0x577277, 0x819796, 0xa8b5b2, 0xc7cfcc, 0xebede9,
        ],
    },
    Builtin {
        slug: "steam-lords",
        name: "Steam Lords",
        author: "Slynyrd",
        note: "Dark, earthy",
        colors: &[
            0x213b25, 0x3a604a, 0x4f7754, 0xa19f7c, 0x77744f, 0x775c4f, 0x603b3a, 0x3b2137,
            0x170e19, 0x2f213b, 0x433a60, 0x4f5277, 0x65738c, 0x7c94a1, 0xa0b9ba, 0xc0d1cc,
        ],
    },
    Builtin {
        slug: "darkseed-16",
        name: "Darkseed 16",
        author: "Ricardo Juchem",
        note: "Horror, desaturated",
        colors: &[
            0x000000, 0x001418, 0x002024, 0x002c38, 0x143444, 0x443444, 0x583c48, 0x6c4c44,
            0x806058, 0x6c706c, 0x888078, 0xa49484, 0xc4ac9c, 0xd8b0a8, 0xecd4d0, 0xfcfcfc,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_builtins_with_valid_colours() {
        assert_eq!(BUILTINS.len(), 10);
        let mut slugs: Vec<&str> = BUILTINS.iter().map(|b| b.slug).collect();
        slugs.sort();
        slugs.dedup();
        assert_eq!(slugs.len(), 10);
        let pico = builtin("PICO-8").unwrap();
        assert_eq!(pico.colors.len(), 16);
        assert_eq!(pico.colors[0], [0, 0, 0]);
        assert_eq!(pico.colors[8], [0xff, 0x00, 0x4d]);
        assert!(
            BUILTINS
                .iter()
                .all(|b| !b.author.is_empty() && !b.colors.is_empty())
        );
        assert!(builtin("no-such").is_none());
    }

    #[test]
    fn slugs_from_names_and_urls_only() {
        assert_eq!(parse_slug("pico-8").unwrap(), "pico-8");
        assert_eq!(
            parse_slug("https://lospec.com/palette-list/Sweetie-16/").unwrap(),
            "sweetie-16"
        );
        assert_eq!(
            parse_slug("https://lospec.com/palette-list/apollo.json?x=1").unwrap(),
            "apollo"
        );
        for bad in [
            "",
            "../etc",
            "a b",
            "x&tag=y",
            "pico-8%2f..",
            "https://evil/palette-list/a/b",
        ] {
            assert!(parse_slug(bad).is_err(), "{bad}");
        }
        let q = ListQuery {
            tag: "horror&x=1".into(),
            ..Default::default()
        };
        assert!(q.url().is_err());
        let q = ListQuery {
            tag: "Game Boy".into(),
            page: 2,
            ..Default::default()
        };
        assert!(
            q.url()
                .unwrap()
                .ends_with("page=2&tag=game-boy&sortingType=default")
        );
    }

    #[test]
    fn parses_palette_and_list_replies_strictly() {
        let p = parse_palette(
            "x",
            r##"{"name":"X","author":"A","colors":["000000","#FFffFF"]}"##,
        )
        .unwrap();
        assert_eq!(p.colors, vec![[0, 0, 0], [255, 255, 255]]);
        assert!(parse_palette("x", r#"{"name":"X","colors":["zzzzzz"]}"#).is_err());
        assert!(parse_palette("x", r#"{"name":"X","colors":[]}"#).is_err());
        let list = r#"{"palettes":[
            {"title":"Good","slug":"good","colorsArray":["112233","445566"],"user":{"name":"Kas"}},
            {"title":"Bad slug","slug":"../x","colorsArray":["112233"]},
            {"title":"Bad colour","slug":"bad","colorsArray":["12"]}
        ],"totalCount":3}"#;
        let (items, total) = parse_list(list).unwrap();
        assert_eq!(total, 3);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].author, "Kas");
        assert_eq!(items[0].colors[1], [0x44, 0x55, 0x66]);
    }

    #[test]
    fn imported_palettes_round_trip_through_the_user_folder() {
        let dir = std::env::temp_dir().join("psxforge_palettes_test");
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY (test only): no other test reads this variable.
        unsafe { std::env::set_var("PSXFORGE_PALETTES", &dir) };
        let p = builtin("steam-lords").unwrap();
        let path = save_user(&p).unwrap();
        assert!(path.ends_with("steam-lords.gpl"));
        let users = load_user();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].2, p.colors);
        assert_eq!(
            crate::palette::load_palette(std::path::Path::new("builtin:pico-8"))
                .unwrap()
                .len(),
            16
        );
    }
}
