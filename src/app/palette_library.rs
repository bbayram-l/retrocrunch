//! Palette library window: 10 built-in palettes, the Lospec palette list (browse by
//! tag, colour count and sort order, or import by name/URL) and the user's imported
//! palettes. Network calls run on a background thread.

use super::widgets::{ACCENT2, DIM, WARN, hint, open_folder, swatches};
use egui::{RichText, Ui};
use psx_forge::lospec::{self, CountFilter, ListQuery, LospecPalette, Sorting};
use psx_forge::palette::Rgb;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Builtin,
    Lospec,
    Mine,
}

enum Reply {
    List(anyhow::Result<(Vec<LospecPalette>, usize)>),
    Import(anyhow::Result<LospecPalette>),
}

/// What the window asks the app to do.
pub enum Action {
    /// Make this the active palette; `persist` is what to remember across sessions
    /// (a file path or "builtin:<slug>").
    Use {
        name: String,
        colors: Vec<Rgb>,
        persist: PathBuf,
    },
}

pub struct PaletteLibrary {
    pub open: bool,
    section: Section,
    query: ListQuery,
    results: Option<(Vec<LospecPalette>, usize)>,
    import: String,
    job: Option<Receiver<Reply>>,
    error: Option<String>,
    status: String,
    mine: Option<Vec<(String, PathBuf, Vec<Rgb>)>>,
}

impl Default for PaletteLibrary {
    fn default() -> Self {
        Self {
            open: false,
            section: Section::Builtin,
            query: ListQuery::default(),
            results: None,
            import: String::new(),
            job: None,
            error: None,
            status: String::new(),
            mine: None,
        }
    }
}

impl PaletteLibrary {
    fn search(&mut self, ctx: &egui::Context) {
        let q = self.query.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Reply::List(lospec::fetch_list(&q)));
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.error = None;
        self.status = "Loading the Lospec palette list...".into();
    }

    fn import(&mut self, ctx: &egui::Context, slug: String) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Reply::Import(lospec::fetch(&slug)));
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.error = None;
        self.status = "Downloading from Lospec...".into();
    }

    /// Save to the user palette folder and use it.
    fn keep(&mut self, p: &LospecPalette) -> Option<Action> {
        match lospec::save_user(p) {
            Ok(path) => {
                self.mine = None;
                self.status = format!("Saved {} to {}", p.name, path.display());
                Some(Action::Use {
                    name: p.name.clone(),
                    colors: p.colors.clone(),
                    persist: path,
                })
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                None
            }
        }
    }

    pub fn window(&mut self, ctx: &egui::Context) -> Option<Action> {
        if !self.open {
            return None;
        }
        let mut action = None;
        if let Some(rx) = &self.job
            && let Ok(reply) = rx.try_recv()
        {
            self.job = None;
            match reply {
                Reply::List(Ok(r)) => {
                    self.status = format!("{} palettes match", r.1);
                    self.results = Some(r);
                }
                Reply::Import(Ok(p)) => action = self.keep(&p),
                Reply::List(Err(e)) | Reply::Import(Err(e)) => {
                    self.status.clear();
                    self.error = Some(format!("{e:#}"));
                }
            }
        }
        let mut open = self.open;
        egui::Window::new("Palette library")
            .open(&mut open)
            .default_size([520.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.section, Section::Builtin, "Built-in (10)");
                    ui.selectable_value(&mut self.section, Section::Lospec, "Lospec");
                    ui.selectable_value(&mut self.section, Section::Mine, "My palettes");
                });
                ui.separator();
                let picked = match self.section {
                    Section::Builtin => self.builtin_ui(ui),
                    Section::Lospec => self.lospec_ui(ui, ctx),
                    Section::Mine => self.mine_ui(ui),
                };
                action = action.take().or(picked);
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(WARN).small());
                } else if !self.status.is_empty() {
                    hint(ui, &self.status);
                }
            });
        self.open = open;
        action
    }

    fn row(ui: &mut Ui, name: &str, sub: &str, colors: &[Rgb], button: &str) -> bool {
        let mut clicked = false;
        ui.horizontal(|ui| {
            clicked = ui.button(button).clicked();
            ui.label(RichText::new(name).strong());
            ui.label(
                RichText::new(format!("{} colours", colors.len()))
                    .color(DIM)
                    .small(),
            );
            if !sub.is_empty() {
                ui.label(RichText::new(sub).color(DIM).small());
            }
        });
        swatches(ui, colors, 11.0);
        ui.add_space(4.0);
        clicked
    }

    fn builtin_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        hint(
            ui,
            "Ready to use, no download. Also available to the CLI and agents as builtin:<name>.",
        );
        let mut out = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for b in lospec::BUILTINS {
                let sub = format!("{} · {}", b.note, b.author);
                if Self::row(ui, b.name, &sub, &b.palette().colors, "Use") {
                    out = Some(Action::Use {
                        name: b.name.to_string(),
                        colors: b.palette().colors,
                        persist: PathBuf::from(format!("{}{}", lospec::BUILTIN_PREFIX, b.slug)),
                    });
                }
            }
        });
        out
    }

    fn lospec_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) -> Option<Action> {
        let busy = self.job.is_some();
        ui.horizontal(|ui| {
            ui.label("Import");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.import)
                    .hint_text("name or lospec.com URL, e.g. nyx8")
                    .desired_width(260.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui
                .add_enabled(
                    !busy && !self.import.trim().is_empty(),
                    egui::Button::new("Import"),
                )
                .clicked()
                || enter)
                && !busy
            {
                match lospec::parse_slug(&self.import) {
                    Ok(slug) => self.import(ctx, slug),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
        });
        ui.separator();
        let mut go = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Tag");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.query.tag)
                    .hint_text("horror, gameboy, nes...")
                    .desired_width(120.0),
            );
            go |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            egui::ComboBox::from_id_salt("lospec_count")
                .selected_text(match self.query.count {
                    CountFilter::Any => "Any size",
                    CountFilter::Max => "At most",
                    CountFilter::Min => "At least",
                    CountFilter::Exact => "Exactly",
                })
                .show_ui(ui, |ui| {
                    for (c, l) in [
                        (CountFilter::Any, "Any size"),
                        (CountFilter::Max, "At most"),
                        (CountFilter::Min, "At least"),
                        (CountFilter::Exact, "Exactly"),
                    ] {
                        ui.selectable_value(&mut self.query.count, c, l);
                    }
                });
            if self.query.count != CountFilter::Any {
                ui.add(
                    egui::DragValue::new(&mut self.query.colors)
                        .range(2..=256)
                        .suffix(" colours"),
                );
            }
            egui::ComboBox::from_id_salt("lospec_sort")
                .selected_text(self.query.sorting.label())
                .show_ui(ui, |ui| {
                    for s in Sorting::ALL {
                        ui.selectable_value(&mut self.query.sorting, s, s.label());
                    }
                });
            if ui.add_enabled(!busy, egui::Button::new("Search")).clicked() {
                go = true;
            }
        });
        if go && !busy {
            self.query.page = 0;
            self.search(ctx);
        }
        if busy {
            ui.spinner();
        }
        let mut out = None;
        let mut page_to = None;
        if let Some((items, total)) = &self.results {
            let pages = total.div_ceil(items.len().max(1)).max(1);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.query.page > 0 && !busy, egui::Button::new("◀"))
                    .clicked()
                {
                    page_to = Some(self.query.page - 1);
                }
                ui.label(format!("page {} of {pages}", self.query.page + 1));
                if ui
                    .add_enabled(self.query.page + 1 < pages && !busy, egui::Button::new("▶"))
                    .clicked()
                {
                    page_to = Some(self.query.page + 1);
                }
            });
            let mut keep = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in items {
                    let sub = if p.author.is_empty() {
                        p.slug.clone()
                    } else {
                        format!("{} · {}", p.slug, p.author)
                    };
                    if Self::row(ui, &p.name, &sub, &p.colors, "Use") {
                        keep = Some(p.clone());
                    }
                }
            });
            if let Some(p) = keep {
                out = self.keep(&p);
            }
        } else if !busy {
            hint(ui, "Search the Lospec palette list, or import one by name.");
        }
        if let Some(p) = page_to {
            self.query.page = p;
            self.search(ctx);
        }
        ui.label(
            RichText::new(
                "Palettes by their authors on lospec.com; using one saves a copy to My palettes.",
            )
            .color(ACCENT2)
            .small(),
        );
        out
    }

    fn mine_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let mine = self.mine.get_or_insert_with(lospec::load_user);
        let mut out = None;
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                *mine = lospec::load_user();
            }
            if ui.button("Open folder").clicked() {
                open_folder(&lospec::user_dir());
            }
            hint(ui, &lospec::user_dir().display().to_string());
        });
        if mine.is_empty() {
            hint(
                ui,
                "Imported Lospec palettes and .gpl/.hex/.pal files in this folder show up here.",
            );
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (name, path, colors) in mine.iter() {
                if Self::row(ui, name, "", colors, "Use") {
                    out = Some(Action::Use {
                        name: name.clone(),
                        colors: colors.clone(),
                        persist: path.clone(),
                    });
                }
            }
        });
        out
    }
}
