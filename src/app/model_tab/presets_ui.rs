//! Model tab: the preset bar (templates and user presets) and the save-preset window.

use super::*;

impl ModelTab {
    pub(super) fn all_presets(&mut self, sh: &mut Shared) -> &Vec<retrocrunch::presets::Preset> {
        self.presets.get_or_insert_with(|| {
            let (user, errors) = retrocrunch::presets::load_user();
            if !errors.is_empty() {
                sh.error = Some(format!(
                    "Some presets could not be read:\n{}",
                    errors.join("\n")
                ));
            }
            retrocrunch::presets::builtins()
                .into_iter()
                .chain(user)
                .collect()
        })
    }

    pub(super) fn apply_preset(&mut self, p: &retrocrunch::presets::Preset, sh: &mut Shared) {
        self.settings = p.convert.clone();
        if let Some(e) = &p.export {
            self.export = e.clone();
        }
        self.active_preset = Some(p.name.clone());
        sh.status = format!("Preset '{}' applied.", p.name);
    }

    pub(super) fn preset_bar(&mut self, ui: &mut Ui, sh: &mut Shared) {
        let presets = self.all_presets(sh).clone();
        let active = self.active_preset.clone();
        let current = active
            .as_ref()
            .and_then(|n| presets.iter().find(|p| &p.name == n));
        let modified = current.is_some_and(|p| {
            serde_json::to_string(&p.convert).ok() != serde_json::to_string(&self.settings).ok()
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preset").color(ACCENT2));
            let label = match (&active, modified) {
                (Some(n), true) => format!("{n} (modified)"),
                (Some(n), false) => n.clone(),
                (None, _) => "Custom".to_string(),
            };
            let mut picked = None;
            egui::ComboBox::from_id_salt("preset")
                .selected_text(label)
                .width(190.0)
                .show_ui(ui, |ui| {
                    let mut last = String::new();
                    for p in &presets {
                        let cat = if p.builtin {
                            p.category.clone()
                        } else {
                            "My presets".to_string()
                        };
                        if cat != last {
                            ui.label(RichText::new(&cat).small().color(DIM));
                            last = cat;
                        }
                        let resp = ui
                            .selectable_label(active.as_deref() == Some(p.name.as_str()), &p.name);
                        let resp = if p.description.is_empty() {
                            resp
                        } else {
                            resp.on_hover_text(&p.description)
                        };
                        if resp.clicked() {
                            picked = Some(p.clone());
                        }
                    }
                });
            if let Some(p) = picked {
                self.apply_preset(&p, sh);
            }
            ui.menu_button("...", |ui| {
                if ui.button("Save as preset...").clicked() {
                    ui.close_menu();
                    self.save_preset = Some(SavePresetDialog {
                        name: current
                            .filter(|p| !p.builtin)
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                        category: "My presets".into(),
                        ..Default::default()
                    });
                }
                if ui.button("Import preset file...").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Preset", &["json"])
                        .pick_file()
                    {
                        match retrocrunch::presets::read(&path)
                            .and_then(|p| retrocrunch::presets::save_user(&p).map(|_| p))
                        {
                            Ok(p) => {
                                self.presets = None;
                                self.apply_preset(&p, sh);
                            }
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                if ui.button("Export current settings...").clicked() {
                    ui.close_menu();
                    let name = active.clone().unwrap_or_else(|| "My preset".into());
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Preset", &["json"])
                        .set_file_name(format!("{}.json", export::sanitize(&name)))
                        .save_file()
                    {
                        let p = retrocrunch::presets::Preset {
                            name,
                            category: "My presets".into(),
                            description: String::new(),
                            convert: self.settings.clone(),
                            export: Some(self.export.clone()),
                            builtin: false,
                            path: None,
                        };
                        match retrocrunch::presets::write(&path, &p) {
                            Ok(()) => sh.status = format!("Preset written to {}", path.display()),
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                let deletable = current.filter(|p| !p.builtin).cloned();
                if ui
                    .add_enabled(deletable.is_some(), egui::Button::new("Delete this preset"))
                    .clicked()
                {
                    ui.close_menu();
                    if let Some(p) = deletable {
                        match retrocrunch::presets::delete_user(&p) {
                            Ok(()) => {
                                self.presets = None;
                                self.active_preset = None;
                                sh.status = format!("Deleted preset '{}'.", p.name);
                            }
                            Err(e) => sh.error = Some(format!("{e:#}")),
                        }
                    }
                }
                if ui.button("Open presets folder").clicked() {
                    ui.close_menu();
                    widgets::open_folder(&retrocrunch::presets::user_dir());
                }
                if ui.button("Reload presets").clicked() {
                    ui.close_menu();
                    self.presets = None;
                }
            });
        });
        if let Some(desc) = current
            .map(|p| p.description.clone())
            .filter(|d| !d.is_empty())
        {
            hint(ui, &desc);
        }
    }

    /// "Save preset" window; call once per frame.
    pub fn preset_window(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        let Some(d) = &mut self.save_preset else {
            return;
        };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Save preset")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.name);
                });
                ui.horizontal(|ui| {
                    ui.label("Category");
                    ui.text_edit_singleline(&mut d.category);
                });
                ui.label("Description");
                ui.text_edit_multiline(&mut d.description);
                ui.checkbox(
                    &mut d.include_export,
                    "Include export settings (engine, formats, scale)",
                );
                hint(
                    ui,
                    &format!("Saved to {}", retrocrunch::presets::user_dir().display()),
                );
                save = ui
                    .add_enabled(
                        !d.name.trim().is_empty(),
                        egui::Button::new(RichText::new("Save").strong()),
                    )
                    .clicked();
            });
        if save {
            let p = retrocrunch::presets::Preset {
                name: d.name.trim().to_string(),
                category: d.category.trim().to_string(),
                description: d.description.trim().to_string(),
                convert: self.settings.clone(),
                export: d.include_export.then(|| self.export.clone()),
                builtin: false,
                path: None,
            };
            match retrocrunch::presets::save_user(&p) {
                Ok(path) => {
                    sh.status = format!("Saved preset '{}' to {}", p.name, path.display());
                    self.active_preset = Some(p.name);
                    self.presets = None;
                    self.save_preset = None;
                }
                Err(e) => sh.error = Some(format!("{e:#}")),
            }
        } else if !open {
            self.save_preset = None;
        }
    }
}
