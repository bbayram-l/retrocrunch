//! Blocks tab: build levels from shaped 1x1x1 blocks with an atlas,
//! then export GLB/OBJ (optionally with a Godot collision hint).

use super::Shared;
use super::texture_tab::IMAGE_EXTS;
use super::viewport::{Controls, RenderPiece, RenderScene, Viewport, axis_gizmo};
use super::widgets::{ACCENT, ACCENT2, DIM, TexSlot, hint, section};
use egui::{Color32, Key, RichText, Stroke, Ui};
use glam::{IVec3, Vec2, Vec3};
use image::RgbaImage;
use retrocrunch::blocks::{self, Atlas, Block, BlockWorld, Shape};
use retrocrunch::export::{self, ExportSettings};
use retrocrunch::raster::RasterOptions;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Tool {
    Place,
    Erase,
    Paint,
    Box,
    BoxErase,
    Pick,
}

impl Tool {
    const ALL: [(Tool, &'static str, &'static str); 6] = [
        (Tool::Place, "Place", "Q"),
        (Tool::Erase, "Erase", "E"),
        (Tool::Paint, "Paint", "P"),
        (Tool::Box, "Box fill", "B"),
        (Tool::BoxErase, "Box carve", "C"),
        (Tool::Pick, "Pick", "I"),
    ];
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct BlocksTab {
    pub tool: Tool,
    pub brush: Block,
    pub layer: i32,
    pub viewport: Viewport,
    pub godot_collision: bool,
    pub face_shading: bool,
    /// Bake AO + sun shadows into the vertex colours on export (experimental).
    pub bake_light: bool,
    pub light: retrocrunch::blocks::LevelLight,
    pub glb: bool,
    pub obj: bool,
    /// Split each block face into an N x N grid (1 = off) to limit affine warping.
    pub subdivisions: u32,
    pub tile_size: u32,
    pub atlas_path: Option<PathBuf>,
    #[serde(skip)]
    world: BlockWorld,
    #[serde(skip)]
    atlas: Option<RgbaImage>,
    #[serde(skip)]
    atlas_tex: TexSlot,
    #[serde(skip)]
    undo: Vec<HashMap<IVec3, Block>>,
    #[serde(skip)]
    redo: Vec<HashMap<IVec3, Block>>,
    #[serde(skip)]
    box_start: Option<IVec3>,
    #[serde(skip)]
    project: Option<PathBuf>,
    #[serde(skip)]
    scene: Option<RenderScene>,
    #[serde(skip)]
    grid: Vec<RenderPiece>,
    #[serde(skip)]
    grid_layer: i32,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    last_drag_cell: Option<IVec3>,
}

impl Default for BlocksTab {
    fn default() -> Self {
        let mut viewport = Viewport::default();
        viewport.opts = RasterOptions {
            affine: false,
            vertex_snap: false,
            dither: false,
            color15: false,
            ..Default::default()
        };
        viewport.cam.target = Vec3::new(0.0, 0.0, 0.0);
        viewport.cam.distance = 14.0;
        viewport.cam.pitch = 0.7;
        Self {
            tool: Tool::Place,
            brush: Block::default(),
            layer: 0,
            viewport,
            godot_collision: true,
            face_shading: true,
            bake_light: false,
            light: Default::default(),
            glb: true,
            obj: false,
            subdivisions: 2,
            tile_size: 16,
            atlas_path: None,
            world: BlockWorld {
                tile_size: 16,
                ..Default::default()
            },
            atlas: None,
            atlas_tex: TexSlot::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            box_start: None,
            project: None,
            scene: None,
            grid: Vec::new(),
            grid_layer: i32::MIN,
            generation: 1,
            dirty: true,
            last_drag_cell: None,
        }
    }
}

fn grid_texture() -> RgbaImage {
    RgbaImage::from_fn(16, 16, |x, y| {
        if x == 0 || y == 0 {
            image::Rgba([110, 110, 170, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    })
}

fn grid_pieces(layer: i32) -> Vec<RenderPiece> {
    let (r, step) = (12i32, 2i32);
    let y = layer as f32 + 0.002;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for z in (-r..r).step_by(step as usize) {
        for x in (-r..r).step_by(step as usize) {
            let b = positions.len() as u32;
            for (dx, dz) in [(0, 0), (step, 0), (step, step), (0, step)] {
                positions.push(Vec3::new((x + dx) as f32, y, (z + dz) as f32));
                uvs.push(Vec2::new((x + dx) as f32, (z + dz) as f32));
            }
            indices.extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]);
        }
    }
    let colors = vec![[1.0; 4]; positions.len()];
    vec![RenderPiece {
        positions,
        uvs,
        colors,
        indices,
        texture: Some(1),
        double_sided: true,
        normals: Vec::new(),
        vertex_colors: Vec::new(),
        material: None,
    }]
}

impl BlocksTab {
    fn atlas_info(&self) -> Atlas {
        self.atlas
            .as_ref()
            .map(|a| Atlas::from_image(a, self.tile_size))
            .unwrap_or_default()
    }

    fn ensure_atlas(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        if self.atlas.is_some() {
            return;
        }
        let loaded = self.atlas_path.as_ref().and_then(|p| match image::open(p) {
            Ok(i) => Some(i.to_rgba8()),
            Err(e) => {
                sh.error = Some(format!("Could not load atlas {}: {e}", p.display()));
                None
            }
        });
        if loaded.is_none() {
            self.atlas_path = None;
            self.tile_size = 16;
        }
        let img = loaded.unwrap_or_else(|| blocks::default_atlas(16));
        self.atlas_tex.set(ctx, "atlas", &img);
        self.atlas = Some(img);
        self.dirty = true;
    }

    fn rebuild(&mut self) {
        if self.grid_layer != self.layer {
            self.grid = grid_pieces(self.layer);
            self.grid_layer = self.layer;
            self.generation += 1;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.generation += 1;
        let mut piece = self.world.build_piece(&self.atlas_info(), true);
        // Preview what gets exported, so affine warping can be judged in the viewport.
        piece.subdivide(
            blocks::face_grid_edge(self.subdivisions),
            blocks::MAX_SUBDIVIDED_TRIS,
        );
        let rp = RenderPiece {
            positions: piece.vertices.iter().map(|v| v.pos).collect(),
            uvs: piece.vertices.iter().map(|v| v.uv).collect(),
            colors: piece.vertices.iter().map(|v| v.color).collect(),
            indices: piece.indices,
            texture: Some(0),
            double_sided: false,
            normals: Vec::new(),
            vertex_colors: Vec::new(),
            material: None,
        };
        let (lo, hi) = self
            .world
            .bounds()
            .map(|(a, b)| (a.as_vec3(), b.as_vec3()))
            .unwrap_or((Vec3::splat(-4.0), Vec3::splat(4.0)));
        self.scene = Some(RenderScene {
            pieces: vec![rp],
            textures: vec![self.atlas.clone(), Some(grid_texture())],
            materials: Vec::new(),
            lo,
            hi,
            generation: self.generation,
        });
    }

    fn snapshot(&mut self) {
        self.undo.push(self.world.cells.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        if let Some(c) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.world.cells, c));
            self.dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if let Some(c) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.world.cells, c));
            self.dirty = true;
        }
    }

    pub fn side_panel(&mut self, ui: &mut Ui, sh: &mut Shared) {
        self.ensure_atlas(ui.ctx(), sh);
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.snapshot();
                self.world.cells.clear();
                self.project = None;
                self.dirty = true;
            }
            if ui.button("Open...").clicked() {
                self.open_project(sh);
            }
            if ui
                .button("Defaults")
                .on_hover_text("Reset tool, brush and export settings (the level is kept)")
                .clicked()
            {
                let d = Self::default();
                self.tool = d.tool;
                self.brush = d.brush;
                self.layer = d.layer;
                self.godot_collision = d.godot_collision;
                self.face_shading = d.face_shading;
                self.glb = d.glb;
                self.obj = d.obj;
                self.subdivisions = d.subdivisions;
                self.dirty = true;
                sh.status = "Blocks settings reverted to defaults.".into();
            }
            if ui.button("Save...").clicked() {
                self.save_project(sh);
            }
        });
        if let Some(p) = &self.project {
            hint(ui, &p.display().to_string());
        }
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Tool", true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (t, name, key) in Tool::ALL {
                        if ui.selectable_label(self.tool == t, name).on_hover_text(format!("Shortcut: {key}")).clicked() {
                            self.tool = t;
                            self.box_start = None;
                        }
                    }
                });
                hint(ui, "LMB: use tool · RMB: orbit · MMB / Shift+RMB: pan · wheel: zoom");
                ui.horizontal(|ui| {
                    ui.label("Build layer");
                    if ui.add(egui::DragValue::new(&mut self.layer).range(-64..=64)).changed() {
                        self.box_start = None;
                    }
                    hint(ui, "PgUp / PgDn");
                });
            });
            section(ui, "Shape", true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, s) in Shape::ALL.iter().enumerate() {
                        ui.selectable_value(&mut self.brush.shape, *s, s.label()).on_hover_text(format!("Shortcut: {}", i + 1));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("⟲ Rotate (R)").clicked() {
                        self.brush.rot = (self.brush.rot + 1) % 4;
                    }
                    ui.checkbox(&mut self.brush.flip, "Upside down (F)");
                    ui.label(RichText::new(format!("{}°", self.brush.rot as u32 * 90)).color(DIM));
                });
            });
            section(ui, "Tiles", true, |ui| self.tile_picker(ui, sh));
            section(ui, "Export", true, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.glb, "GLB");
                    ui.checkbox(&mut self.obj, "OBJ");
                });
                ui.checkbox(&mut self.godot_collision, "Godot collision (-col node)").on_hover_text("Godot generates a StaticBody3D + trimesh collider for nodes ending in -col");
                ui.checkbox(&mut self.face_shading, "Bake face shading into vertex colours");
                ui.checkbox(&mut self.bake_light, "Bake lighting on export (experimental)")
                    .on_hover_text("Ray-traced ambient occlusion and sun shadows in the vertex colours, like pre-lit PS1 levels");
                if self.bake_light {
                    let l = &mut self.light;
                    ui.checkbox(&mut l.ao, "Ambient occlusion");
                    if l.ao {
                        ui.add(egui::Slider::new(&mut l.ao_strength, 0.0..=1.0).text("AO strength"));
                    }
                    ui.checkbox(&mut l.sun, "Sun with shadows");
                    if l.sun {
                        ui.add(egui::Slider::new(&mut l.sun_yaw, -180.0..=180.0).text("sun yaw"));
                        ui.add(egui::Slider::new(&mut l.sun_pitch, 5.0..=90.0).text("sun height"));
                        ui.add(egui::Slider::new(&mut l.sun_strength, 0.0..=1.5).text("sun strength"));
                        ui.add(egui::Slider::new(&mut l.ambient, 0.0..=1.0).text("shadow light"));
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Face grid");
                    for n in [1u32, 2, 3, 4] {
                        let label = if n == 1 { "off".to_string() } else { format!("{n}x{n}") };
                        if ui.selectable_value(&mut self.subdivisions, n, label).changed() {
                            self.dirty = true;
                        }
                    }
                })
                .response
                .on_hover_text("Split every face into a grid so affine texture warping stays small, like PS1 games did with big floors and walls. Turn on Affine in the viewport to compare.");
                let tris = self.scene.as_ref().map_or(0, |s| s.pieces.first().map_or(0, |p| p.indices.len() / 3));
                hint(ui, &format!("{} blocks · {} triangles after culling", self.world.cells.len(), tris));
                if ui.add_enabled(!self.world.cells.is_empty(), egui::Button::new(RichText::new("Export level...").strong())).clicked() {
                    self.export(sh);
                }
            });
        });
    }

    fn tile_picker(&mut self, ui: &mut Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Load atlas...").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("Images", &IMAGE_EXTS)
                    .pick_file()
            {
                self.atlas_path = Some(p);
                self.atlas = None;
                self.ensure_atlas(ui.ctx(), sh);
            }
            if self.atlas_path.is_some() && ui.button("Default").clicked() {
                self.atlas_path = None;
                self.atlas = None;
                self.ensure_atlas(ui.ctx(), sh);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Tile size");
            if ui
                .add(egui::DragValue::new(&mut self.tile_size).range(4..=512))
                .changed()
            {
                self.world.tile_size = self.tile_size;
                self.dirty = true;
            }
        });
        let info = self.atlas_info();
        let Some(h) = &self.atlas_tex.handle else {
            return;
        };
        let id = h.id();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            for t in 0..info.tile_count().min(256) {
                let (cx, cy) = ((t % info.cols) as f32, (t / info.cols) as f32);
                let uv = egui::Rect::from_min_max(
                    egui::pos2(cx / info.cols as f32, cy / info.rows as f32),
                    egui::pos2((cx + 1.0) / info.cols as f32, (cy + 1.0) / info.rows as f32),
                );
                let img = egui::Image::new((id, egui::vec2(30.0, 30.0))).uv(uv);
                let sel = self.brush.tile as u32 == t;
                if ui
                    .add(egui::ImageButton::new(img).selected(sel))
                    .on_hover_text(format!("Tile {t}"))
                    .clicked()
                {
                    self.brush.tile = t as u16;
                }
            }
        });
    }

    fn open_project(&mut self, sh: &mut Shared) {
        let Some(p) = rfd::FileDialog::new()
            .add_filter("Block project", &["json"])
            .pick_file()
        else {
            return;
        };
        match BlockWorld::load(&p) {
            Ok(w) => {
                self.snapshot();
                self.tile_size = w.tile_size.max(1);
                if let Some(a) = &w.atlas_path {
                    self.atlas_path = Some(PathBuf::from(a));
                    self.atlas = None;
                }
                self.world = w;
                self.project = Some(p);
                self.dirty = true;
                if let Some(s) = self.scene.as_ref() {
                    self.viewport.frame(s);
                }
            }
            Err(e) => sh.error = Some(format!("{e:#}")),
        }
    }

    fn save_project(&mut self, sh: &mut Shared) {
        let mut d = rfd::FileDialog::new()
            .add_filter("Block project", &["json"])
            .set_file_name("level.json");
        if let Some(p) = &self.project {
            d = d.set_file_name(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("level.json"),
            );
        }
        let Some(p) = d.save_file() else { return };
        self.world.tile_size = self.tile_size;
        self.world.atlas_path = self.atlas_path.as_ref().map(|a| a.display().to_string());
        match self.world.save(&p) {
            Ok(()) => {
                sh.status = format!("Saved {}", p.display());
                self.project = Some(p);
            }
            Err(e) => sh.error = Some(format!("{e:#}")),
        }
    }

    fn export(&mut self, sh: &mut Shared) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Export folder")
            .pick_folder()
        else {
            return;
        };
        let name = self
            .project
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("level")
            .to_string();
        self.world.tile_size = self.tile_size;
        let mut model = self.world.to_model(
            &name,
            self.atlas.as_ref(),
            self.face_shading,
            self.godot_collision,
            self.subdivisions,
        );
        if self.bake_light {
            self.light.bake(&mut model);
        }
        let s = ExportSettings {
            glb: self.glb,
            obj: self.obj,
            godot_scene: false,
            indexed_png: false,
            unlit: true,
            ..Default::default()
        };
        match export::export_model(&model, &dir, &s) {
            Ok(files) => sh.status = format!("Exported {} files to {}", files.len(), dir.display()),
            Err(e) => sh.error = Some(format!("Export failed: {e:#}")),
        }
    }

    fn shortcuts(&mut self, ui: &Ui) {
        ui.input(|i| {
            for (t, _, k) in Tool::ALL {
                let key = match k {
                    "Q" => Key::Q,
                    "E" => Key::E,
                    "P" => Key::P,
                    "B" => Key::B,
                    "C" => Key::C,
                    _ => Key::I,
                };
                if i.key_pressed(key) && !i.modifiers.command {
                    self.tool = t;
                    self.box_start = None;
                }
            }
            if i.key_pressed(Key::R) {
                self.brush.rot = (self.brush.rot + if i.modifiers.shift { 3 } else { 1 }) % 4;
            }
            if i.key_pressed(Key::F) {
                self.brush.flip = !self.brush.flip;
            }
            if i.key_pressed(Key::PageUp) {
                self.layer += 1;
            }
            if i.key_pressed(Key::PageDown) {
                self.layer -= 1;
            }
            if i.key_pressed(Key::Escape) {
                self.box_start = None;
            }
            for (n, key) in [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
            ]
            .into_iter()
            .enumerate()
            {
                if i.key_pressed(key) {
                    self.brush.shape = Shape::ALL[n];
                }
            }
        });
    }

    pub fn central(&mut self, ui: &mut Ui, sh: &mut Shared) {
        self.ensure_atlas(ui.ctx(), sh);
        self.rebuild();
        ui.horizontal(|ui| {
            if ui.button("Frame").clicked()
                && let Some(s) = &self.scene
            {
                self.viewport.frame(s);
            }
            ui.label(
                RichText::new(format!(
                    "Tool: {:?} · {} {}° {}",
                    self.tool,
                    self.brush.shape.label(),
                    self.brush.rot as u32 * 90,
                    if self.brush.flip { "(flipped)" } else { "" }
                ))
                .color(ACCENT2),
            );
        });
        self.viewport.toolbar(ui);
        let info = self
            .viewport
            .show(ui, self.scene.as_ref(), Controls::Editor, &self.grid, false);
        axis_gizmo(ui, &info);
        if info.response.hovered() {
            self.shortcuts(ui);
        }

        let Some(mouse) = info.response.hover_pos() else {
            return;
        };
        let (origin, dir) = self.viewport.cam.ray(info.ndc(mouse), info.aspect);
        let hit = blocks::raycast(&self.world, origin, dir, 1000.0, Some(self.layer));
        let Some(hit) = hit else { return };
        let on_block = self.world.get(hit.cell).is_some();
        let target = match self.tool {
            Tool::Place | Tool::Box => Some(hit.cell + hit.normal),
            Tool::BoxErase => Some(if on_block {
                hit.cell
            } else {
                hit.cell + hit.normal
            }),
            Tool::Erase | Tool::Paint | Tool::Pick => on_block.then_some(hit.cell),
        };
        let Some(target) = target else { return };

        // Ghost / selection preview.
        let (lo, hi) = match (self.box_start, self.tool) {
            (Some(a), Tool::Box | Tool::BoxErase) => (a.min(target), a.max(target)),
            _ => (target, target),
        };
        let color = match self.tool {
            Tool::Erase | Tool::BoxErase => Color32::from_rgb(255, 90, 90),
            Tool::Pick => ACCENT2,
            _ => ACCENT,
        };
        draw_box(ui, &info, lo.as_vec3(), (hi + IVec3::ONE).as_vec3(), color);

        let r = &info.response;
        if r.clicked_by(egui::PointerButton::Primary) {
            self.apply_tool(target, sh);
        } else if r.dragged_by(egui::PointerButton::Primary)
            && matches!(self.tool, Tool::Erase | Tool::Paint)
            && self.last_drag_cell != Some(target)
        {
            if r.drag_started() {
                self.snapshot();
            }
            self.last_drag_cell = Some(target);
            if self.tool == Tool::Erase {
                self.world.set(target, None);
            } else if let Some(b) = self.world.cells.get_mut(&target) {
                b.tile = self.brush.tile;
            }
            self.dirty = true;
        }
        if r.drag_stopped() {
            self.last_drag_cell = None;
        }
    }

    fn apply_tool(&mut self, target: IVec3, sh: &mut Shared) {
        match self.tool {
            Tool::Place => {
                self.snapshot();
                self.world.set(target, Some(self.brush));
            }
            Tool::Erase => {
                self.snapshot();
                self.world.set(target, None);
            }
            Tool::Paint => {
                self.snapshot();
                if let Some(b) = self.world.cells.get_mut(&target) {
                    b.tile = self.brush.tile;
                }
            }
            Tool::Pick => {
                if let Some(b) = self.world.get(target) {
                    self.brush = *b;
                    self.tool = Tool::Place;
                    sh.status = format!("Picked {} tile {}", b.shape.label(), b.tile);
                }
            }
            Tool::Box | Tool::BoxErase => match self.box_start.take() {
                None => self.box_start = Some(target),
                Some(a) => {
                    let n = ((a - target).abs() + IVec3::ONE).element_product();
                    if n > 200_000 {
                        sh.error = Some(format!("Box of {n} blocks is too large"));
                        return;
                    }
                    self.snapshot();
                    let fill = if self.tool == Tool::Box {
                        Some(self.brush)
                    } else {
                        None
                    };
                    self.world.fill_box(a, target, fill);
                }
            },
        }
        self.dirty = true;
    }
}

fn draw_box(ui: &Ui, info: &super::viewport::ViewInfo, lo: Vec3, hi: Vec3, color: Color32) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { hi.x } else { lo.x },
            if y { hi.y } else { lo.y },
            if z { hi.z } else { lo.z },
        )
    };
    let corners: Vec<Vec3> = (0..8)
        .map(|i| c(i & 1 != 0, i & 2 != 0, i & 4 != 0))
        .collect();
    let painter = ui.painter_at(info.rect);
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        if let (Some(p), Some(q)) = (info.project(corners[a]), info.project(corners[b])) {
            painter.line_segment([p, q], Stroke::new(2.0f32, color));
        }
    }
}
