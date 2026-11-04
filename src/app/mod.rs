//! retrocrunch desktop app shell: tabs, menus, palette library, preferences, drag & drop.

mod blocks_tab;
mod detail_panel;
mod gpu;
mod model_tab;
mod palette_library;
mod rig_panel;
mod scale_tab;
mod sprite_tab;
mod texture_tab;
mod viewport;
mod widgets;

use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use retrocrunch::palette::{self, Rgb};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use widgets::{ACCENT, ACCENT2, DIM, WARN};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamedPalette {
    pub name: String,
    pub path: Option<PathBuf>,
    pub colors: Vec<Rgb>,
}

/// State shared by all tabs.
#[derive(Default)]
pub struct Shared {
    pub status: String,
    pub error: Option<String>,
    pub palette: Option<NamedPalette>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Tab {
    Model,
    Texture,
    Sprite,
    Scale,
    Blocks,
}

impl Tab {
    const ALL: [(Tab, &'static str, &'static str); 5] = [
        (Tab::Model, "🗿 Model", "PS1-style 3D conversion"),
        (Tab::Texture, "🖼 Texture", "Crunch textures to PS1 specs"),
        (
            Tab::Sprite,
            "💡 Sprite Lights",
            "Normal / specular / occlusion maps for 2D lights",
        ),
        (Tab::Scale, "🔍 Pixel Scale", "Scale2x / Scale3x / Eagle"),
        (Tab::Blocks, "📦 Blocks", "Block-based level builder"),
    ];
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub ui_scale: f32,
    pub dark: bool,
    pub show_palette_panel: bool,
    pub palette_path: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            dark: true,
            show_palette_panel: true,
            palette_path: None,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Persisted {
    tab: Option<Tab>,
    prefs: Prefs,
    model: model_tab::ModelTab,
    texture: texture_tab::TextureTab,
    sprite: sprite_tab::SpriteTab,
    scale: scale_tab::ScaleTab,
    blocks: blocks_tab::BlocksTab,
}

pub struct RetrocrunchApp {
    tab: Tab,
    prefs: Prefs,
    shared: Shared,
    model: model_tab::ModelTab,
    texture: texture_tab::TextureTab,
    sprite: sprite_tab::SpriteTab,
    scale: scale_tab::ScaleTab,
    blocks: blocks_tab::BlocksTab,
    show_prefs: bool,
    show_about: bool,
    applied_theme: Option<(bool, f32)>,
    /// Detected Blender executable (path lookup only; no process is started).
    blender: Option<String>,
    palette_lib: palette_library::PaletteLibrary,
}

const STORAGE_KEY: &str = "retrocrunch-state";

impl RetrocrunchApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let p: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        let mut app = Self {
            tab: p.tab.unwrap_or(Tab::Model),
            prefs: p.prefs,
            shared: Shared {
                status: "Ready. Drop a model, texture or sprite onto the window.".into(),
                ..Default::default()
            },
            model: p.model,
            texture: p.texture,
            sprite: p.sprite,
            scale: p.scale,
            blocks: p.blocks,
            show_prefs: false,
            show_about: false,
            applied_theme: None,
            blender: retrocrunch::blender::find_blender().map(|p| p.display().to_string()),
            palette_lib: Default::default(),
        };
        if let Some(pp) = app.prefs.palette_path.clone() {
            app.load_palette(&pp);
        }
        if let Some(path) = open {
            app.open_path(path);
        }
        app
    }

    fn load_palette(&mut self, p: &Path) {
        match palette::load_palette(p) {
            Ok(colors) => {
                let builtin = p
                    .to_str()
                    .and_then(|s| s.strip_prefix(retrocrunch::lospec::BUILTIN_PREFIX))
                    .and_then(retrocrunch::lospec::builtin);
                let name = match builtin {
                    Some(b) => b.name,
                    None => p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("palette")
                        .to_string(),
                };
                self.shared.status = format!("Palette {name}: {} colours", colors.len());
                self.shared.palette = Some(NamedPalette {
                    name,
                    path: Some(p.to_path_buf()),
                    colors,
                });
                self.prefs.palette_path = Some(p.to_path_buf());
            }
            Err(e) => {
                self.shared.error = Some(format!("Could not load palette: {e:#}"));
                self.prefs.palette_path = None;
            }
        }
    }

    /// Route a file to the right tab by extension (and current tab for images).
    fn open_path(&mut self, p: PathBuf) {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let sh = &mut self.shared;
        match ext.as_str() {
            e if ["obj", "glb", "gltf", "fbx"].contains(&e)
                || retrocrunch::blender::IMPORT_EXTS.contains(&e) =>
            {
                self.tab = Tab::Model;
                self.model.load(p, sh);
            }
            "gpl" | "pal" | "act" | "hex" | "txt" => {
                self.load_palette(&p);
                self.use_loaded_palette();
            }
            "png" | "jpg" | "jpeg" | "bmp" | "tga" | "gif" => match self.tab {
                Tab::Sprite => self.sprite.load(p, sh),
                Tab::Scale => self.scale.load(p, sh),
                _ => {
                    self.tab = Tab::Texture;
                    self.texture.load(p, sh);
                }
            },
            _ => sh.error = Some(format!("Don't know how to open '.{ext}' files")),
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open model...").clicked() {
                    ui.close_menu();
                    self.tab = Tab::Model;
                    self.model.open_dialog(&mut self.shared);
                }
                if ui.button("Open image...").clicked() {
                    ui.close_menu();
                    match self.tab {
                        Tab::Sprite => self.sprite.open_dialog(&mut self.shared),
                        Tab::Scale => self.scale.open_dialog(&mut self.shared),
                        _ => {
                            self.tab = Tab::Texture;
                            self.texture.open_dialog(&mut self.shared);
                        }
                    }
                }
                if ui.button("Palette library...").clicked() {
                    ui.close_menu();
                    self.palette_lib.open = true;
                }
                if ui.button("Load palette...").clicked() {
                    ui.close_menu();
                    self.palette_dialog();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.model.has_model(),
                        egui::Button::new("Quick Export...").shortcut_text("Ctrl+Shift+E"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.tab = Tab::Model;
                    self.model.quick_export_open = true;
                }
                ui.separator();
                if ui.button("Preferences...").clicked() {
                    ui.close_menu();
                    self.show_prefs = true;
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add(egui::Button::new("Undo").shortcut_text("Ctrl+Z"))
                    .clicked()
                {
                    ui.close_menu();
                    self.undo();
                }
                if ui
                    .add(egui::Button::new("Redo").shortcut_text("Ctrl+Y"))
                    .clicked()
                {
                    ui.close_menu();
                    self.redo();
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.prefs.show_palette_panel, "Palette panel");
                ui.checkbox(&mut self.prefs.dark, "Dark theme");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Retrocrunch").clicked() {
                    ui.close_menu();
                    self.show_about = true;
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("RETROCRUNCH")
                        .strong()
                        .color(ACCENT)
                        .monospace(),
                );
            });
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (t, name, tip) in Tab::ALL {
                let sel = self.tab == t;
                let text = RichText::new(name).size(15.0);
                let text = if sel {
                    text.strong().color(egui::Color32::WHITE)
                } else {
                    text
                };
                let b = egui::Button::new(text)
                    .fill(if sel {
                        ACCENT
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .min_size(egui::vec2(0.0, 28.0));
                if ui.add(b).on_hover_text(tip).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    fn palette_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(
                "Palettes",
                &["gpl", "pal", "act", "hex", "txt", "png", "bmp", "gif"],
            )
            .pick_file()
        {
            self.load_palette(&p);
            self.use_loaded_palette();
        }
    }

    /// A palette the user just chose should change the colours: switch Model and Texture
    /// to "imported: all colours" (the auto modes build their own palette).
    fn use_loaded_palette(&mut self) {
        use retrocrunch::texture::PaletteMode;
        let Some(p) = &self.shared.palette else {
            return;
        };
        for mode in [
            &mut self.model.settings.texture.palette_mode,
            &mut self.texture.settings.palette_mode,
        ] {
            if !matches!(*mode, PaletteMode::ImportedAll | PaletteMode::ImportedBestN) {
                *mode = PaletteMode::ImportedAll;
            }
        }
        self.shared.status = format!(
            "Palette {} ({} colours) now drives Model and Texture.",
            p.name,
            p.colors.len()
        );
    }

    fn palette_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Palette").strong().color(ACCENT2));
            match &self.shared.palette {
                Some(p) => {
                    ui.label(format!("{} · {} colours", p.name, p.colors.len()));
                }
                None => {
                    ui.label(RichText::new("none (auto k-means)").color(DIM));
                }
            }
            if ui
                .button("Library...")
                .on_hover_text(
                    "10 built-in palettes, the Lospec palette list and your imported palettes",
                )
                .clicked()
            {
                self.palette_lib.open = true;
            }
            if ui
                .button("Load...")
                .on_hover_text(".gpl .pal .act .hex paint.net .txt or a swatch image")
                .clicked()
            {
                self.palette_dialog();
            }
            if self.shared.palette.is_some() {
                if ui.button("Save as...").clicked()
                    && let (Some(p), Some(dest)) = (
                        &self.shared.palette,
                        rfd::FileDialog::new()
                            .add_filter("Palette", &["hex", "gpl", "pal"])
                            .set_file_name("palette.gpl")
                            .save_file(),
                    )
                    && let Err(e) = palette::save_palette(&dest, &p.colors)
                {
                    self.shared.error = Some(e.to_string());
                }
                if ui.button("Clear").clicked() {
                    self.shared.palette = None;
                    self.prefs.palette_path = None;
                }
                let unused = !matches!(
                    self.model.settings.texture.palette_mode,
                    retrocrunch::texture::PaletteMode::ImportedAll
                        | retrocrunch::texture::PaletteMode::ImportedBestN
                );
                if unused
                    && ui
                        .button("Use in Model + Texture")
                        .on_hover_text("The current palette mode ignores this palette: switch both tabs to 'imported: all colours'")
                        .clicked()
                {
                    self.use_loaded_palette();
                }
            }
            widgets::hint(ui, "Used by 'Imported' palette modes in Model and Texture.");
        });
        if let Some(p) = &self.shared.palette {
            widgets::swatches(ui, &p.colors, 12.0);
        }
    }

    fn undo(&mut self) {
        match self.tab {
            Tab::Model => self.model.undo(),
            Tab::Blocks => self.blocks.undo(),
            _ => {}
        }
    }

    fn redo(&mut self) {
        match self.tab {
            Tab::Model => self.model.redo(),
            Tab::Blocks => self.blocks.redo(),
            _ => {}
        }
    }

    fn global_shortcuts(&mut self, ctx: &egui::Context) {
        let qe = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::E);
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
        let redo2 = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let (q, u, r, o) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&qe),
                i.consume_shortcut(&undo),
                i.consume_shortcut(&redo) || i.consume_shortcut(&redo2),
                i.consume_shortcut(&open),
            )
        });
        if q && self.model.has_model() {
            self.tab = Tab::Model;
            self.model.quick_export_open = true;
        }
        if u {
            self.undo();
        }
        if r {
            self.redo();
        }
        if o {
            match self.tab {
                Tab::Model => self.model.open_dialog(&mut self.shared),
                Tab::Texture => self.texture.open_dialog(&mut self.shared),
                Tab::Sprite => self.sprite.open_dialog(&mut self.shared),
                Tab::Scale => self.scale.open_dialog(&mut self.shared),
                Tab::Blocks => {}
            }
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for p in dropped {
            self.open_path(p);
        }
    }

    fn windows(&mut self, ctx: &egui::Context) {
        if self.show_prefs {
            let mut open = true;
            egui::Window::new("Preferences")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.prefs.ui_scale, 0.75..=2.0)
                            .text("UI scale")
                            .step_by(0.05),
                    );
                    ui.checkbox(&mut self.prefs.dark, "Dark theme");
                    ui.checkbox(&mut self.prefs.show_palette_panel, "Show palette panel");
                    ui.separator();
                    match &self.blender {
                        Some(path) => {
                            ui.label("Blender found (for .blend/USD/Alembic import and FBX export)");
                            widgets::hint(ui, path);
                        }
                        None => {
                            ui.label(RichText::new("Blender not found").color(WARN));
                            widgets::hint(ui, "Needed for .blend/USD/Alembic import and FBX export. Install Blender or set RETROCRUNCH_BLENDER.");
                        }
                    }
                    if ui.button("Reset all tab settings").clicked() {
                        self.model.settings = Default::default();
                        self.texture.settings = Default::default();
                        self.sprite.settings = Default::default();
                        self.sprite.light = Default::default();
                    }
                });
            self.show_prefs = open;
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(RichText::new("Retrocrunch").heading().color(ACCENT));
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label("A local, free PS1-style asset pipeline:");
                    for (_, name, tip) in Tab::ALL {
                        ui.label(format!("  {name}: {tip}"));
                    }
                    ui.label(
                        RichText::new("Also usable from the command line: retrocrunch --help")
                            .color(DIM),
                    );
                    ui.separator();
                    ui.label(
                        "Free to use. If it saves you time, you can pay what you want on itch.io.",
                    );
                    ui.horizontal(|ui| {
                        ui.hyperlink_to(
                            "Download and updates",
                            "https://laegh.itch.io/retrocrunch",
                        );
                        ui.label(RichText::new("·").color(DIM));
                        ui.hyperlink_to("Source code", "https://github.com/bbayram-l/retrocrunch");
                    });
                    ui.separator();
                    ui.label("Everything Retrocrunch creates is yours, commercial games included.");
                    ui.label(
                        RichText::new(
                            "Retrocrunch is source-available under PolyForm Shield 1.0.0: \
                             don't resell or redistribute it. Open-source libraries: \
                             THIRD_PARTY_LICENSES.html.",
                        )
                        .color(DIM),
                    );
                    ui.label(RichText::new("© 2026 Burak Bayram").color(DIM));
                });
            self.show_about = open;
        }
        if let Some(err) = self.shared.error.clone() {
            let mut open = true;
            egui::Window::new(RichText::new("Something went wrong").color(WARN))
                .open(&mut open)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            ui.label(&err);
                        });
                    if ui.button("OK").clicked() {
                        self.shared.error = None;
                    }
                });
            if !open {
                self.shared.error = None;
            }
        }
        self.model.quick_export_window(ctx, &mut self.shared);
        self.model.preset_window(ctx, &mut self.shared);
        self.model.vram_window(ctx);
        if let Some(palette_library::Action::Use {
            name,
            colors,
            persist,
        }) = self.palette_lib.window(ctx)
        {
            self.shared.status = format!("Palette {name}: {} colours", colors.len());
            let is_file = persist.is_file();
            self.shared.palette = Some(NamedPalette {
                name,
                path: is_file.then(|| persist.clone()),
                colors,
            });
            self.prefs.palette_path = Some(persist);
            self.use_loaded_palette();
        }
    }
}

impl eframe::App for RetrocrunchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        viewport::GPU_AVAILABLE.store(frame.gl().is_some(), std::sync::atomic::Ordering::Relaxed);
        let theme = (self.prefs.dark, self.prefs.ui_scale);
        if self.applied_theme != Some(theme) {
            widgets::apply_theme(ctx, self.prefs.dark);
            ctx.set_zoom_factor(self.prefs.ui_scale);
            self.applied_theme = Some(theme);
        }
        self.global_shortcuts(ctx);
        self.model.update(ctx, &mut self.shared);

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            self.menu_bar(ctx, ui);
            ui.add_space(2.0);
            self.tab_bar(ui);
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.model.is_busy() {
                    ui.spinner();
                }
                ui.label(RichText::new(&self.shared.status).color(DIM));
            });
        });
        if self.prefs.show_palette_panel && matches!(self.tab, Tab::Model | Tab::Texture) {
            egui::TopBottomPanel::bottom("palette")
                .resizable(false)
                .show(ctx, |ui| self.palette_panel(ui));
        }
        egui::SidePanel::left("settings")
            .resizable(true)
            .default_width(330.0)
            .width_range(260.0..=520.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                let sh = &mut self.shared;
                match self.tab {
                    Tab::Model => self.model.side_panel(ui, sh),
                    Tab::Texture => self.texture.side_panel(ui, sh),
                    Tab::Sprite => self.sprite.side_panel(ui, sh),
                    Tab::Scale => self.scale.side_panel(ui, sh),
                    Tab::Blocks => self.blocks.side_panel(ui, sh),
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            let sh = &mut self.shared;
            match self.tab {
                Tab::Model => self.model.central(ui, sh),
                Tab::Texture => self.texture.central(ui, sh),
                Tab::Sprite => self.sprite.central(ui, sh),
                Tab::Scale => self.scale.central(ui, sh),
                Tab::Blocks => self.blocks.central(ui, sh),
            }
        });
        self.windows(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let p = Persisted {
            tab: Some(self.tab),
            prefs: std::mem::take(&mut self.prefs),
            model: std::mem::take(&mut self.model),
            texture: std::mem::take(&mut self.texture),
            sprite: std::mem::take(&mut self.sprite),
            scale: std::mem::take(&mut self.scale),
            blocks: std::mem::take(&mut self.blocks),
        };
        eframe::set_value(storage, STORAGE_KEY, &p);
        self.prefs = p.prefs;
        self.model = p.model;
        self.texture = p.texture;
        self.sprite = p.sprite;
        self.scale = p.scale;
        self.blocks = p.blocks;
    }
}
