//! Model tab: export settings and dialogs, sprite sheets, the VRAM map, quick export.

use super::*;

impl ModelTab {
    pub(super) fn export_section(&mut self, ui: &mut Ui, sh: &mut Shared) {
        section(ui, "Export", true, |ui| {
            ui.horizontal(|ui| {
                let mut p = self.export.preset;
                egui::ComboBox::from_id_salt("preset")
                    .selected_text(p.label())
                    .show_ui(ui, |ui| {
                        for e in EnginePreset::ALL {
                            ui.selectable_value(&mut p, e, e.label());
                        }
                    });
                if p != self.export.preset {
                    self.export = p.settings();
                }
                ui.label("engine");
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.export.glb, "GLB");
                ui.checkbox(&mut self.export.obj, "OBJ");
                ui.checkbox(&mut self.export.godot_scene, "Godot scene + shader");
                ui.checkbox(&mut self.export.fbx, "FBX")
                    .on_hover_text("Binary FBX through headless Blender: keeps skeletons and clips (Unity Humanoid / Mixamo). Needs Blender.");
            });
            if self.export.fbx && retrocrunch::blender::find_blender().is_none() {
                ui.label(
                    RichText::new(
                        "Blender not found: FBX falls back to OBJ. Set RETROCRUNCH_BLENDER.",
                    )
                    .color(WARN)
                    .small(),
                );
            }
            ui.checkbox(&mut self.export.indexed_png, "Indexed (CLUT) PNG");
            ui.checkbox(&mut self.export.unlit, "Unlit material");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.export.lods).range(0..=4).prefix("LODs "))
                    .on_hover_text("Experimental: extra levels of detail, each with half the triangles. Godot scenes of static models switch between them by distance.");
                ui.add(egui::DragValue::new(&mut self.export.tessellate).range(0..=3).prefix("Near detail "))
                    .on_hover_text("Experimental: versions with long edges split, which Godot scenes of static models show up close (like PS1 games did against affine texture warping).");
            });
            ui.horizontal(|ui| {
                ui.label("Scale");
                ui.add(
                    egui::DragValue::new(&mut self.export.scale)
                        .range(0.001..=1000.0)
                        .speed(0.01),
                );
            });
            ui.add_space(4.0);
            let ready = self.converted.is_some() && self.job.is_none();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new("Export...").strong())
                        .min_size([ui.available_width(), 28.0].into()),
                )
                .clicked()
            {
                self.export_dialog(sh);
            }
            ui.add_space(6.0);
            if ui
                .add_enabled(ready, egui::Button::new("VRAM map (experimental)..."))
                .on_hover_text(
                    "Lay the textures out in PS1 VRAM: texture pages, CLUTs, framebuffers",
                )
                .clicked()
                && let Some(c) = &self.converted
            {
                let l = retrocrunch::vram::layout(&c.stats.textures);
                let mut slot = TexSlot::default();
                slot.set(ui.ctx(), "vram_map", &retrocrunch::vram::layout_image(&l));
                self.vram_map = Some((slot, l));
            }
            ui.collapsing("Sprite sheet (experimental)", |ui| {
                hint(
                    ui,
                    "Render the converted model from 8 directions (Doom-style 2.5D billboards), with the selected clip's frames as columns. Transparent PNG + JSON layout.",
                );
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.sprites.directions).range(1..=32).suffix(" dirs"));
                    ui.add(egui::DragValue::new(&mut self.sprites.size).range(8..=512).suffix(" px"));
                    ui.add(egui::DragValue::new(&mut self.sprites.frames).range(1..=64).suffix(" frames"));
                    ui.checkbox(&mut self.sprites.ps1, "PS1 look");
                });
                if ui
                    .add_enabled(ready, egui::Button::new("Render sprite sheet..."))
                    .clicked()
                {
                    self.sprite_dialog(sh);
                }
            });
        });
    }

    pub(super) fn sprite_dialog(&mut self, sh: &mut Shared) {
        let Some(c) = self.converted.clone() else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Folder for the sprite sheet")
            .pick_folder()
        else {
            return;
        };
        let mut job = self.sprites.clone();
        job.clip = self
            .anim_clip
            .and_then(|i| c.model.rig.as_ref().and_then(|r| r.clips.get(i)))
            .map(|clip| clip.name.clone());
        match retrocrunch::ops::write_sprite_sheet(&c.model, &c.model.name, &dir, &job) {
            Ok(v) => {
                sh.status = format!(
                    "Sprite sheet: {} directions x {} frames (+ Godot scene) in {}",
                    v["layout"]["directions"],
                    v["layout"]["frames"],
                    dir.display()
                )
            }
            Err(e) => sh.error = Some(format!("{e:#}")),
        }
    }

    pub(super) fn export_dialog(&mut self, sh: &mut Shared) {
        let mut d = rfd::FileDialog::new().set_title("Choose export folder");
        if let Some(p) = &self.last_export_dir {
            d = d.set_directory(p);
        }
        if let Some(dir) = d.pick_folder() {
            self.export_to(dir, sh);
        }
    }

    pub(super) fn export_to(&mut self, dir: PathBuf, sh: &mut Shared) {
        let Some(c) = &self.converted else { return };
        let target = dir.join(export::sanitize(&c.model.name));
        match export::export_model(&c.model, &target, &self.export) {
            Ok(files) => {
                for (i, pal) in c.palettes.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
                    let p = target.join(format!(
                        "{}_palette{}.hex",
                        export::sanitize(&c.model.name),
                        if i == 0 {
                            String::new()
                        } else {
                            format!("_{i}")
                        }
                    ));
                    let _ = retrocrunch::palette::save_palette(&p, pal);
                }
                sh.status = format!("Exported {} files to {}", files.len(), target.display());
                self.last_export_dir = Some(dir);
            }
            Err(e) => sh.error = Some(format!("Export failed: {e:#}")),
        }
    }

    pub fn vram_window(&mut self, ctx: &egui::Context) {
        let Some((slot, l)) = &self.vram_map else {
            return;
        };
        let mut open = true;
        egui::Window::new("PS1 VRAM map (experimental)")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                if let Some(h) = &slot.handle {
                    ui.add(egui::Image::new((h.id(), egui::vec2(768.0, 384.0))).texture_options(egui::TextureOptions::NEAREST));
                }
                hint(ui, "1024 x 512 words. Grey: framebuffers. Coloured: textures (texture-page grid every 64 words, 256 lines). Yellow: CLUTs.");
                for p in l.placements.iter().filter(|p| p.kind != "framebuffer") {
                    ui.label(
                        RichText::new(format!(
                            "{:<8} {}  at ({}, {})  {}x{} words{}",
                            p.kind,
                            p.name,
                            p.x,
                            p.y,
                            p.w,
                            p.h,
                            p.page.map_or(String::new(), |(x, y)| format!("  page ({x}, {y}) {}-bit", p.bits))
                        ))
                        .monospace()
                        .small(),
                    );
                }
                for u in &l.unplaced {
                    ui.label(RichText::new(format!("does not fit: {u}")).color(WARN));
                }
            });
        if !open {
            self.vram_map = None;
        }
    }

    pub fn quick_export_window(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        if !self.quick_export_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Quick Export")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if self.converted.is_none() {
                    ui.label("Convert a model first.");
                    return;
                }
                match self.quick_step {
                    0 => {
                        ui.label("Which engine is this for?");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            for e in EnginePreset::ALL {
                                if ui
                                    .add_sized(
                                        [96.0, 56.0],
                                        egui::Button::new(RichText::new(e.label()).strong()),
                                    )
                                    .clicked()
                                {
                                    self.export = e.settings();
                                    self.quick_step = 1;
                                }
                            }
                        });
                    }
                    _ => {
                        ui.label(
                            RichText::new(self.export.preset.label())
                                .heading()
                                .color(ACCENT2),
                        );
                        let formats: Vec<&str> = [
                            (self.export.glb, "GLB"),
                            (self.export.obj, "OBJ"),
                            (self.export.godot_scene, "Godot .tscn + shader"),
                        ]
                        .into_iter()
                        .filter(|f| f.0)
                        .map(|f| f.1)
                        .collect();
                        ui.label(format!(
                            "Formats: {}  ·  scale {}  ·  {}",
                            formats.join(", "),
                            self.export.scale,
                            if self.export.indexed_png {
                                "indexed PNG"
                            } else {
                                "RGBA PNG"
                            }
                        ));
                        ui.add_space(4.0);
                        ui.label(RichText::new(self.export.preset.import_notes()).small());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Back").clicked() {
                                self.quick_step = 0;
                            }
                            if ui
                                .button(RichText::new("Choose folder & export").strong())
                                .clicked()
                            {
                                self.export_dialog(sh);
                                self.quick_export_open = false;
                            }
                        });
                    }
                }
            });
        if !open {
            self.quick_export_open = false;
        }
    }
}
