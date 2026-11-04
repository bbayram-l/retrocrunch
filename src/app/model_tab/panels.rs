//! Model tab: the settings side panel (pieces, geometry, lighting, texture).

use super::*;

impl ModelTab {
    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open model...").clicked() {
                self.open_dialog(sh);
            }
            if ui
                .add_enabled(
                    self.source.is_some() && self.job.is_none(),
                    egui::Button::new("Clear"),
                )
                .on_hover_text("Unload the model (settings are kept)")
                .clicked()
            {
                self.clear_asset(sh);
            }
            if self.converted.is_some()
                && ui
                    .button("Quick Export")
                    .on_hover_text("Ctrl+Shift+E")
                    .clicked()
            {
                self.quick_export_open = true;
                self.quick_step = 0;
            }
        });
        if let Some(m) = &self.source {
            ui.label(RichText::new(&m.name).strong());
            hint(
                ui,
                &format!(
                    "{} tris · {} verts · {} pieces · {} materials",
                    m.tri_count(),
                    m.vertex_count(),
                    m.pieces.len(),
                    m.materials.len()
                ),
            );
        } else {
            hint(
                ui,
                "OBJ, GLB/glTF, FBX or .blend. Drag a file onto the window.",
            );
        }
        ui.separator();
        self.preset_bar(ui, sh);
        if ui
            .button("Revert to defaults")
            .on_hover_text("Reset every conversion and export setting to the built-in defaults")
            .clicked()
        {
            self.revert_to_defaults(sh);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.pieces_section(ui);
                if let Some(src) = self.source.as_mut() {
                    self.rig.section(ui, src);
                }
                self.geometry_section(ui);
                self.detail
                    .section(ui, &mut self.settings, self.source.as_mut());
                self.lighting_section(ui);
                self.texture_section(ui, sh);
                self.export_section(ui, sh);
            });
    }

    pub(super) fn pieces_section(&mut self, ui: &mut Ui) {
        let Some(model) = self.source.clone() else {
            return;
        };
        section(
            ui,
            &format!("Pieces ({})", model.pieces.len()),
            true,
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.redo();
                    }
                    let sel = self.selected.len();
                    if ui
                        .add_enabled(sel >= 2, egui::Button::new("Merge"))
                        .on_hover_text("Merge selected pieces that share a material")
                        .clicked()
                    {
                        self.push_undo();
                        let target = *self.selected.iter().next().unwrap_or(&0);
                        let g = self.pieces[target].group;
                        let mat = model.pieces[target].material;
                        for &i in &self.selected {
                            if model.pieces[i].material == mat {
                                self.pieces[i].group = g;
                            }
                        }
                    }
                    if ui
                        .add_enabled(sel >= 1, egui::Button::new("Delete"))
                        .on_hover_text(
                            "Exclude selected pieces from the conversion (source is untouched)",
                        )
                        .clicked()
                    {
                        self.push_undo();
                        for &i in &self.selected {
                            self.pieces[i].enabled = false;
                        }
                    }
                    if ui
                        .add_enabled(sel >= 1, egui::Button::new("Bake"))
                        .on_hover_text("Bake into base texture: remove the selected pieces from the geometry but paint them into the texture underneath (straps, pockets, decals). Uses the UV atlas.")
                        .clicked()
                    {
                        self.push_undo();
                        for &i in &self.selected {
                            self.pieces[i].baked = true;
                            self.pieces[i].enabled = true;
                        }
                    }
                    if ui.button("Reset").clicked() {
                        self.push_undo();
                        self.pieces = PieceSettings::defaults_for(&model);
                    }
                });
                hint(
                    ui,
                    "Click to select, Ctrl+click for several. Weight = share of the triangle budget.",
                );
                let ctrl = ui.input(|i| i.modifiers.command);
                egui::ScrollArea::vertical()
                    .id_salt("pieces")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (i, p) in model.pieces.iter().enumerate() {
                            let mut snapshot = None;
                            ui.horizontal(|ui| {
                                let s = &mut self.pieces[i];
                                let before = s.clone();
                                ui.checkbox(&mut s.enabled, "");
                                let merged =
                                    model.pieces.iter().enumerate().any(|(j, _)| {
                                        j != i && self.pieces_group(j) == before.group
                                    });
                                let name = if merged {
                                    format!("{} ⛓", p.name)
                                } else {
                                    p.name.clone()
                                };
                                let sel = self.selected.contains(&i);
                                let text = RichText::new(name).color(if before.baked {
                                    ACCENT2
                                } else if before.enabled {
                                    ui.visuals().text_color()
                                } else {
                                    DIM
                                });
                                if ui
                                    .selectable_label(sel, text)
                                    .on_hover_text(format!("{} triangles", p.tri_count()))
                                    .clicked()
                                {
                                    if !ctrl {
                                        self.selected.clear();
                                    }
                                    if sel && ctrl {
                                        self.selected.remove(&i);
                                    } else {
                                        self.selected.insert(i);
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let s = &mut self.pieces[i];
                                        if s.baked {
                                            if ui
                                                .small_button("Restore")
                                                .on_hover_text("Bring the piece back as geometry")
                                                .clicked()
                                            {
                                                s.baked = false;
                                            }
                                            ui.label(RichText::new("baked").small().color(ACCENT2));
                                            return;
                                        }
                                        ui.add(
                                            egui::DragValue::new(&mut s.weight)
                                                .range(0.0..=10.0)
                                                .speed(0.02)
                                                .prefix("w "),
                                        );
                                        ui.label(
                                            RichText::new(format!("{}", p.tri_count()))
                                                .small()
                                                .color(DIM),
                                        );
                                    },
                                );
                                if self.pieces[i] != before {
                                    snapshot = Some(before);
                                }
                            });
                            if let Some(before) = snapshot {
                                let mut prev = self.pieces.clone();
                                prev[i] = before;
                                self.undo.push(prev);
                                self.redo.clear();
                            }
                        }
                    });
            },
        );
    }

    pub(super) fn pieces_group(&self, i: usize) -> usize {
        self.pieces.get(i).map_or(usize::MAX, |p| p.group)
    }

    pub(super) fn geometry_section(&mut self, ui: &mut Ui) {
        let animated = self
            .source
            .as_ref()
            .is_some_and(|m| m.is_rigged() || m.pieces.iter().any(|p| p.has_morphs()));
        let s = &mut self.settings;
        section(ui, "Geometry", true, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.use_percent, false, "Triangles");
                ui.selectable_value(&mut s.use_percent, true, "Percent");
            });
            if s.use_percent {
                slider_f32(
                    ui,
                    "% of source",
                    &mut s.percent,
                    0.1..=100.0,
                    10.0,
                    "Target as a percentage of the source triangle count",
                );
            } else {
                slider_usize(
                    ui,
                    "triangles",
                    &mut s.target_tris,
                    12..=20000,
                    600,
                    true,
                    "Total triangle budget, shared between pieces by area x weight",
                );
            }
            hint(ui, "PS1 reference: props 100-500, characters 300-1500.");
            if animated {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("rigid")
                        .selected_text(s.rigid.label())
                        .show_ui(ui, |ui| {
                            for m in process::RigidMode::ALL {
                                ui.selectable_value(&mut s.rigid, m, m.label());
                            }
                        });
                    ui.label("skinning");
                })
                .response
                .on_hover_text("Experimental, PS1-style stiff skinning. One bone per vertex: stiff joints, closed mesh (later PS1 games). Rigid segments: body parts move as solid blocks with seams at the joints (early PS1 games, whose parts were modelled to overlap; continuous meshes show cracks).");
            }
            ui.add_enabled_ui(!animated, |ui| {
                ui.checkbox(&mut s.remesh, "Remesh first (Blender)").on_hover_text(
                    "Rebuild the model as one even, closed surface (voxel remesh + QuadriFlow)                      before reducing: far fewer long slivers, so less affine texture warping and                      smoother vertex lighting; repairs messy scans. Textures are rebaked into a                      new atlas. Needs Blender; static models only.",
                );
                if s.remesh {
                    slider_f32(
                        ui,
                        "remesh density",
                        &mut s.remesh_density,
                        1.0..=8.0,
                        3.0,
                        "Remeshed triangles per final triangle; the reducer then brings it down to the budget",
                    );
                }
            })
            .response
            .on_disabled_hover_text("Remesh would lose skin weights and morph targets.");
            ui.checkbox(&mut s.lock_borders, "Lock open borders")
                .on_hover_text(
                    "Keep mesh boundaries (silhouettes of open meshes) intact; reduces less",
                );
            slider_f32(
                ui,
                "UV seam care",
                &mut s.uv_weight,
                0.0..=4.0,
                1.0,
                "How much UV distortion counts against a collapse",
            );
            ui.checkbox(&mut s.remove_hidden, "Remove faces nobody will see")
                .on_hover_text(
                    "Renders the model from 96 directions and deletes faces that never show up",
                );
            if s.remove_hidden {
                ui.indent("hb", |ui| {
                    ui.checkbox(
                        &mut s.hidden_ignore_below,
                        "Model sits on the ground (ignore views from below)",
                    );
                });
            }
            ui.horizontal(|ui| {
                ui.label("Vertex grid");
                ui.add(
                    egui::DragValue::new(&mut s.geometry_snap)
                        .range(0.0..=1.0)
                        .speed(0.0005)
                        .max_decimals(4),
                );
                hint(ui, "world units, 0 = off");
            });
            ui.horizontal(|ui| {
                ui.label("Split polygons over");
                ui.add(
                    egui::DragValue::new(&mut s.subdivide_max_edge)
                        .range(0.0..=1000.0)
                        .speed(0.005)
                        .max_decimals(3),
                )
                .on_hover_text("Edges longer than this (world units) are split, crack-free, so affine texture warping stays small. PS1 games did this for floors and walls. 0 = off.");
                if let Some(m) = &self.source {
                    let (lo, hi) = m.bounds();
                    hint(ui, &format!("model is {:.2} across", (hi - lo).length()));
                }
            });
            if self.rig_min_tris > 0 {
                let budget = if s.use_percent {
                    self.source
                        .as_ref()
                        .map_or(0, |m| (m.tri_count() as f32 * s.percent / 100.0) as usize)
                } else {
                    s.target_tris
                };
                ui.horizontal(|ui| {
                    let text = format!("Rig minimum: ~{} triangles", self.rig_min_tris);
                    if budget < self.rig_min_tris {
                        ui.label(RichText::new(format!("⚠ {text}")).color(WARN))
                            .on_hover_text("Below this, joints may not have enough edge loops to bend cleanly. You can go lower.");
                    } else {
                        hint(ui, &text);
                    }
                    if ui.small_button("Use").clicked() {
                        s.use_percent = false;
                        s.target_tris = self.rig_min_tris;
                    }
                });
            }
        });
    }

    pub(super) fn lighting_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        section(ui, "Lighting (vertex colours)", false, |ui| {
            ui.checkbox(&mut s.vertex_ao, "Ray-traced ambient occlusion");
            if s.vertex_ao {
                slider_f32(ui, "AO strength", &mut s.ao_strength, 0.0..=1.0, 0.7, "");
            }
            ui.checkbox(&mut s.sun, "Directional light with shadows");
            if s.sun {
                slider_f32(
                    ui,
                    "sun yaw",
                    &mut s.sun_yaw,
                    -std::f32::consts::PI..=std::f32::consts::PI,
                    0.8,
                    "",
                );
                slider_f32(ui, "sun pitch", &mut s.sun_pitch, 0.0..=1.55, 0.9, "");
                slider_f32(ui, "sun strength", &mut s.sun_strength, 0.0..=1.5, 0.6, "");
                slider_f32(ui, "ambient", &mut s.ambient, 0.0..=1.0, 0.55, "");
            }
            ui.checkbox(&mut s.bake_shading_into_texture, "Bake lighting into the texture").on_hover_text("Paint the shading into the albedo instead of vertex colours (needs non-overlapping UVs)");
        });
    }

    pub(super) fn texture_section(&mut self, ui: &mut Ui, sh: &mut Shared) {
        let s = &mut self.settings;
        section(ui, "Texture", true, |ui| {
            ui.horizontal(|ui| {
                ui.label("Quality");
                ui.selectable_value(&mut s.hd_texture, false, "PSX")
                    .on_hover_text("Crunch to PS1 specs: small texture, CLUT palette, dither, 15-bit colour.");
                ui.selectable_value(&mut s.hd_texture, true, "HD")
                    .on_hover_text("Keep the source's texel density and full colour on the low-poly model. Still pixel-sharp (nearest filtering), just high resolution. Size, palette, dither and 15-bit are skipped.");
            });
            if s.hd_texture {
                hint(ui, "HD: source resolution, full colour, nearest filtering.");
                if let Some(c) = &self.converted {
                    for (name, w, h, _) in &c.stats.textures {
                        hint(ui, &format!("{name}: {w}x{h}"));
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.sizing, TextureSizing::Fixed, "Fixed size");
                    ui.selectable_value(&mut s.sizing, TextureSizing::PixelSize, "Pixel size");
                });
                match s.sizing {
                    TextureSizing::Fixed => {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("texsize")
                                .selected_text(format!("{} px", s.texture.max_size))
                                .show_ui(ui, |ui| {
                                    let mut sizes = SIZES;
                                    sizes.sort();
                                    for v in sizes {
                                        ui.selectable_value(
                                            &mut s.texture.max_size,
                                            v,
                                            format!("{v} px"),
                                        );
                                    }
                                });
                            ui.label("longest side");
                        });
                    }
                    TextureSizing::PixelSize => {
                        slider_f32(
                            ui,
                            "cm per texel",
                            &mut s.texel_cm,
                            0.25..=20.0,
                            2.0,
                            "Same physical pixel density for every asset in your game",
                        );
                        ui.horizontal(|ui| {
                            ui.label("Model units per metre");
                            ui.add(
                                egui::DragValue::new(&mut s.units_per_meter)
                                    .range(0.001..=1000.0)
                                    .speed(0.1),
                            );
                        });
                        if let (Some(c), true) = (&self.converted, true) {
                            for (name, w, h, _) in &c.stats.textures {
                                hint(ui, &format!("{name}: {w}x{h}"));
                            }
                        }
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label("UV layout");
                ui.selectable_value(&mut s.uv_mode, UvMode::Atlas, "New atlas")
                    .on_hover_text("Unwrap the low-poly mesh into one tight atlas and rebake every material from the full-resolution source. No seam smearing at low budgets; one texture page.");
                ui.selectable_value(&mut s.uv_mode, UvMode::Keep, "Keep original")
                    .on_hover_text("Keep the source UVs and resize each material's texture (best for tiling textures).");
            });
            if s.uv_mode == UvMode::Atlas || self.pieces.iter().any(|p| p.baked) {
                ui.indent("atlas", |ui| {
                    slider_f32(ui, "bake distance %", &mut s.bake_distance, 0.2..=15.0, 3.0, "How far above/below the low-poly surface to look for source detail (% of model size). Raise it if parts come out with the wrong colour, lower it if detail from neighbouring surfaces bleeds in.");
                    slider_f32(ui, "chart angle", &mut s.chart_angle, 20.0..=85.0, 60.0, "Faces within this angle of a chart's main direction share one UV island: higher = fewer, larger islands.");
                    if let Some((n, charts)) = self.converted.as_ref().and_then(|c| c.stats.atlas) {
                        hint(ui, &format!("{n}x{n} atlas, {charts} charts"));
                    }
                });
            }
            if !s.hd_texture {
                ui.checkbox(&mut s.texture.power_of_two, "Power-of-two sizes");
                enum_combo(
                    ui,
                    "filter",
                    "downscale",
                    &mut s.texture.filter,
                    &ResizeFilter::ALL,
                    ResizeFilter::label,
                );
            }
            slider_f32(
                ui,
                "brightness",
                &mut s.texture.brightness,
                -0.5..=0.5,
                0.0,
                "Applied before the palette, so imported colours stay exact",
            );
            slider_f32(ui, "contrast", &mut s.texture.contrast, -0.5..=1.0, 0.0, "");
            slider_f32(
                ui,
                "saturation",
                &mut s.texture.saturation,
                -1.0..=1.0,
                0.0,
                "",
            );
            if !s.hd_texture {
                ui.separator();
                enum_combo(
                    ui,
                    "palmode",
                    "palette",
                    &mut s.texture.palette_mode,
                    &PaletteMode::ALL,
                    PaletteMode::label,
                );
                let imported = matches!(
                    s.texture.palette_mode,
                    PaletteMode::ImportedAll | PaletteMode::ImportedBestN
                );
                if imported && sh.palette.is_none() {
                    ui.label(
                        RichText::new(
                            "No palette loaded: using auto. Load one in the Palette panel.",
                        )
                        .color(WARN)
                        .small(),
                    );
                }
                if s.texture.palette_mode != PaletteMode::ImportedAll
                    && s.texture.palette_mode != PaletteMode::Rgb555Only
                {
                    slider_usize(
                        ui,
                        "colours",
                        &mut s.texture.colors,
                        2..=256,
                        16,
                        true,
                        "Colours per texture (CLUT size: 16 = 4-bit, 256 = 8-bit)",
                    );
                }
                ui.checkbox(&mut s.shared_palette, "One palette for all materials");
                ui.checkbox(&mut s.texture.rgb555, "Snap to 15-bit RGB555");
                enum_combo(
                    ui,
                    "dither",
                    "dither",
                    &mut s.texture.dither,
                    &DitherMode::ALL,
                    DitherMode::label,
                );
                if s.texture.dither != DitherMode::None {
                    slider_f32(
                        ui,
                        "dither amount",
                        &mut s.texture.dither_amount,
                        0.0..=64.0,
                        12.0,
                        "With a palette: how much the two nearest palette colours mix (16 = full, 0 = nearest only). RGB555 only: offset in 8-bit units, like the PS1 GPU (its table is +-4).",
                    );
                }
            }
            ui.horizontal(|ui| {
                ui.label("Alpha");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Opaque, "Opaque");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Cutout, "Cutout");
                ui.selectable_value(&mut s.texture.alpha, AlphaMode::Keep, "RGBA");
            });
            ui.separator();
            ui.label(RichText::new("Bake into albedo").color(DIM));
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.bake_ao_map, "AO map");
                ui.checkbox(&mut s.bake_normal_map, "Normal relief");
                ui.checkbox(&mut s.bake_emissive, "Emission");
            });
            if s.bake_normal_map {
                slider_f32(ui, "relief", &mut s.normal_map_strength, 0.0..=2.0, 0.8, "");
            }
        });
    }
}
