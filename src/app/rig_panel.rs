//! Rig viewer and skin-weight editor for the Model tab: bone list and skeleton overlay,
//! test posing, a weight heat map and brush painting on the source model.

use super::viewport::{RenderPiece, RenderScene, ViewInfo};
use super::widgets::{ACCENT, ACCENT2, DIM, WARN, hint, section, slider_f32};
use egui::{Color32, Rect, Stroke, Ui};
use glam::{EulerRot, Mat4, Quat, Vec3};
use retrocrunch::mesh::Model;
use retrocrunch::raster::{self, DrawCall, Frame, RasterOptions};
use retrocrunch::rig::Rig;
use retrocrunch::weights::{self, BrushMode, Neighbours, WeightSnapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Visibility of the posed source mesh, rendered when a brush stroke starts so the
/// brush only reaches surfaces the user can see.
pub(super) struct BrushStroke {
    ids: Vec<u32>,
    w: u32,
    h: u32,
    rect: Rect,
    /// First triangle id of every piece.
    tri_base: Vec<u32>,
    neighbours: Vec<Option<Neighbours>>,
}

impl BrushStroke {
    /// Visibility of `scene` as shown in `info` (no smoothing neighbourhoods).
    pub(super) fn new(info: &ViewInfo, scene: &RenderScene) -> Self {
        let (ids, w, h, tri_base) = render_visibility(info.view, info.proj, info.rect, scene);
        Self {
            ids,
            w,
            h,
            rect: info.rect,
            tri_base,
            neighbours: Vec::new(),
        }
    }
}

enum Edit {
    SmoothBone(usize),
    Normalize,
    Undo,
    Redo,
}

pub struct RigEditor {
    pub show_bones: bool,
    /// Show the selected bone's weights as a heat map (on the source model).
    pub weight_view: bool,
    /// Brush painting: LMB paints, RMB orbits.
    pub paint: bool,
    pub selected: Option<usize>,
    pub mode: BrushMode,
    pub radius: f32,
    pub strength: f32,
    /// Extra rotation per rig node (degrees, XYZ) on top of the clip / bind pose.
    pub pose: BTreeMap<usize, [f32; 3]>,
    /// Bumped after every committed weight edit (stroke end, smooth, normalise, undo).
    pub revision: u64,
    /// Bumped on every visible weight change (each brush dab too).
    pub view_revision: u64,
    filter: String,
    undo: Vec<WeightSnapshot>,
    redo: Vec<WeightSnapshot>,
    stroke: Option<BrushStroke>,
    heat: Option<(u64, u64, Option<usize>, RenderScene)>,
}

impl Default for RigEditor {
    fn default() -> Self {
        Self {
            show_bones: true,
            weight_view: false,
            paint: false,
            selected: None,
            mode: BrushMode::Add,
            radius: 40.0,
            strength: 0.5,
            pose: BTreeMap::new(),
            revision: 0,
            view_revision: 0,
            filter: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            stroke: None,
            heat: None,
        }
    }
}

/// Rig nodes that are joints of any skin.
fn joint_nodes(rig: &Rig) -> BTreeSet<usize> {
    rig.skins
        .iter()
        .flat_map(|s| s.joints.iter().copied())
        .collect()
}

/// Depth-first order of rig nodes with their depth (roots first).
fn tree_order(rig: &Rig) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(rig.nodes.len());
    let mut roots: Vec<usize> = (0..rig.nodes.len())
        .filter(|&i| rig.nodes[i].parent.is_none())
        .collect();
    roots.reverse();
    let mut stack: Vec<(usize, usize)> = roots.into_iter().map(|r| (r, 0)).collect();
    let mut seen = vec![false; rig.nodes.len()];
    while let Some((n, d)) = stack.pop() {
        if std::mem::replace(&mut seen[n], true) {
            continue;
        }
        out.push((n, d));
        for &c in rig.nodes[n].children.iter().rev() {
            if c < rig.nodes.len() {
                stack.push((c, d + 1));
            }
        }
    }
    out
}

impl RigEditor {
    /// Clear per-model state (new model loaded).
    pub fn reset(&mut self) {
        *self = Self {
            show_bones: self.show_bones,
            mode: self.mode,
            radius: self.radius,
            strength: self.strength,
            ..Default::default()
        };
    }

    /// Weight view / painting work on the source model.
    pub fn wants_source(&self) -> bool {
        self.weight_view || self.paint
    }

    /// Key for the posed scene: manual pose + committed weights.
    pub fn pose_key(&self) -> String {
        format!("{:?}|{}", self.pose, self.view_revision)
    }

    /// Clip/bind pose with the manual bone rotations applied.
    pub fn apply_manual(&self, pose: &mut [(Vec3, Quat, Vec3)]) {
        for (&n, e) in &self.pose {
            if let Some(slot) = pose.get_mut(n) {
                let q = Quat::from_euler(
                    EulerRot::XYZ,
                    e[0].to_radians(),
                    e[1].to_radians(),
                    e[2].to_radians(),
                );
                slot.1 = (slot.1 * q).normalize();
            }
        }
    }

    /// Side-panel section. Edits the source model through `source` (copy-on-write).
    pub fn section(&mut self, ui: &mut Ui, source: &mut Arc<Model>) {
        let Some(rig) = source.rig.as_ref().filter(|_| source.is_rigged()) else {
            return;
        };
        let joints = joint_nodes(rig);
        let order = tree_order(rig);
        let mut edit: Option<Edit> = None;
        section(ui, &format!("Rig ({} bones)", joints.len()), true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut self.show_bones, "Bones")
                    .on_hover_text("Draw the skeleton over the model; click a joint to select it");
                ui.toggle_value(&mut self.weight_view, "Weights")
                    .on_hover_text("Heat map of the selected bone's skin weights (blue 0 -> red 1)");
                if ui
                    .toggle_value(&mut self.paint, "Paint")
                    .on_hover_text("Weight brush: LMB paints the selected bone, RMB orbits, Ctrl+click selects a joint")
                    .changed()
                    && self.paint
                {
                    self.weight_view = true;
                }
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter bones")
                    .desired_width(f32::INFINITY),
            );
            let filter = self.filter.to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt("bones")
                .max_height(200.0)
                .show(ui, |ui| {
                    for &(n, depth) in &order {
                        let name = &rig.nodes[n].name;
                        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                            continue;
                        }
                        let is_joint = joints.contains(&n);
                        let posed = self.pose.contains_key(&n);
                        let label = format!(
                            "{}{}{}",
                            "  ".repeat(depth.min(12)),
                            if name.is_empty() { "(unnamed)" } else { name },
                            if posed { "  *" } else { "" }
                        );
                        let text = egui::RichText::new(label).color(if is_joint {
                            ui.visuals().text_color()
                        } else {
                            DIM
                        });
                        if ui
                            .selectable_label(self.selected == Some(n), text)
                            .on_hover_text(if is_joint {
                                "Joint"
                            } else {
                                "Node (not a skin joint)"
                            })
                            .clicked()
                        {
                            self.selected = Some(n);
                        }
                    }
                });
            let Some(sel) = self.selected.filter(|&n| n < rig.nodes.len()) else {
                hint(ui, "Select a bone to pose it or edit its weights.");
                return;
            };
            ui.label(
                egui::RichText::new(&rig.nodes[sel].name)
                    .color(ACCENT2)
                    .strong(),
            );
            // Test pose.
            ui.horizontal(|ui| {
                ui.label("Rotate");
                let e = self.pose.entry(sel).or_insert([0.0; 3]);
                for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    ui.add(
                        egui::DragValue::new(&mut e[k])
                            .range(-180.0..=180.0)
                            .speed(1.0)
                            .prefix(format!("{axis} "))
                            .suffix("°"),
                    );
                }
                if *e == [0.0; 3] {
                    self.pose.remove(&sel);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Reset bone").clicked() {
                    self.pose.remove(&sel);
                }
                if ui
                    .add_enabled(!self.pose.is_empty(), egui::Button::new("Reset pose"))
                    .on_hover_text("Back to the clip / bind pose")
                    .clicked()
                {
                    self.pose.clear();
                }
            });
            hint(ui, "Posing only previews deformation; it isn't exported.");
            ui.separator();
            // Weight tools.
            let influenced: usize = source
                .pieces
                .iter()
                .filter_map(|p| weights::joint_slot(rig, p, sel).map(|j| (p, j)))
                .map(|(p, j)| {
                    p.vertices
                        .iter()
                        .filter(|v| weights::weight_of(v, j) > 0.0)
                        .count()
                })
                .sum();
            if !joints.contains(&sel) {
                ui.label(
                    egui::RichText::new("Not a skin joint: no weights.")
                        .color(WARN)
                        .small(),
                );
            } else {
                hint(ui, &format!("{influenced} vertices influenced"));
            }
            ui.horizontal(|ui| {
                ui.label("Brush");
                for m in BrushMode::ALL {
                    ui.selectable_value(&mut self.mode, m, m.label());
                }
            });
            slider_f32(
                ui,
                "radius px",
                &mut self.radius,
                4.0..=200.0,
                40.0,
                "Brush radius on screen",
            );
            slider_f32(
                ui,
                "strength",
                &mut self.strength,
                0.0..=1.0,
                0.5,
                "Add/Subtract: amount per second. Set: the target weight. Smooth: blend speed.",
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(joints.contains(&sel), egui::Button::new("Smooth bone"))
                    .on_hover_text("Blur this bone's weights over the whole mesh")
                    .clicked()
                {
                    edit = Some(Edit::SmoothBone(sel));
                }
                if ui
                    .button("Normalize")
                    .on_hover_text("Re-normalise every vertex (weights sum to 1, max 4 bones)")
                    .clicked()
                {
                    edit = Some(Edit::Normalize);
                }
                if ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                    .clicked()
                {
                    edit = Some(Edit::Undo);
                }
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("Redo"))
                    .clicked()
                {
                    edit = Some(Edit::Redo);
                }
            });
            hint(
                ui,
                "Edits change the source model: conversion and export use them.",
            );
        });
        if let Some(e) = edit {
            self.apply(Arc::make_mut(source), e);
        }
    }

    fn apply(&mut self, m: &mut Model, e: Edit) {
        match e {
            Edit::SmoothBone(node) => {
                self.undo.push(weights::snapshot(&m.pieces));
                self.redo.clear();
                let rig = m.rig.clone().unwrap_or_default();
                for p in &mut m.pieces {
                    if let Some(j) = weights::joint_slot(&rig, p, node) {
                        weights::smooth_all(p, j, 2);
                    }
                }
            }
            Edit::Normalize => {
                self.undo.push(weights::snapshot(&m.pieces));
                self.redo.clear();
                m.pieces.iter_mut().for_each(weights::normalize_all);
            }
            Edit::Undo => {
                let Some(snap) = self.undo.pop() else { return };
                self.redo.push(weights::snapshot(&m.pieces));
                weights::restore(&mut m.pieces, &snap);
            }
            Edit::Redo => {
                let Some(snap) = self.redo.pop() else { return };
                self.undo.push(weights::snapshot(&m.pieces));
                weights::restore(&mut m.pieces, &snap);
            }
        }
        self.revision += 1;
        self.view_revision += 1;
    }

    /// Heat-map copy of the (posed) source scene for the selected bone.
    pub fn weight_scene(&mut self, model: &Model, src: &RenderScene) -> &RenderScene {
        let key = (src.generation, self.view_revision, self.selected);
        if self.heat.as_ref().map(|h| (h.0, h.1, h.2)) != Some(key) {
            let rig = model.rig.clone().unwrap_or_default();
            let pieces = src
                .pieces
                .iter()
                .zip(&model.pieces)
                .map(|(rp, p)| {
                    let slot = self.selected.and_then(|n| weights::joint_slot(&rig, p, n));
                    let colors = p
                        .vertices
                        .iter()
                        .map(|v| match (p.skin, slot) {
                            (None, _) => [0.35, 0.35, 0.38, 1.0],
                            (Some(_), Some(j)) => weights::heat(weights::weight_of(v, j)),
                            (Some(_), None) => weights::heat(0.0),
                        })
                        .collect::<Vec<_>>();
                    RenderPiece {
                        positions: rp.positions.clone(),
                        uvs: rp.uvs.clone(),
                        vertex_colors: colors.clone(),
                        colors,
                        indices: rp.indices.clone(),
                        texture: None,
                        double_sided: rp.double_sided,
                        normals: rp.normals.clone(),
                        material: None,
                    }
                })
                .collect();
            let scene = RenderScene {
                pieces,
                textures: Vec::new(),
                materials: Vec::new(),
                lo: src.lo,
                hi: src.hi,
                // Distinct from every model scene generation.
                generation: src.generation.wrapping_mul(1_000_003) ^ self.view_revision ^ (1 << 62),
            };
            self.heat = Some((key.0, key.1, key.2, scene));
        }
        &self.heat.as_ref().expect("just built").3
    }

    /// Skeleton overlay. Clicking a joint selects it (Ctrl+click while painting).
    pub fn overlay(&mut self, ui: &Ui, info: &ViewInfo, model: &Model, world: &[Mat4]) {
        let Some(rig) = model.rig.as_ref().filter(|_| model.is_rigged()) else {
            return;
        };
        if !self.show_bones || world.len() != rig.nodes.len() {
            return;
        }
        let joints = joint_nodes(rig);
        let painter = ui.painter_at(info.rect);
        let pos = |n: usize| info.project(world[n].w_axis.truncate());
        let selected = self.selected;
        let is_sel = |n: usize| Some(n) == selected;
        for &n in &joints {
            let Some(p) = pos(n) else { continue };
            // Bone: from the nearest ancestor joint (or direct parent) to this joint.
            let mut parent = rig.nodes[n].parent;
            let mut guard = 0;
            while let Some(q) = parent.filter(|q| !joints.contains(q)) {
                parent = rig.nodes.get(q).and_then(|x| x.parent);
                guard += 1;
                if guard > 64 {
                    parent = None;
                }
            }
            if let Some(q) = parent
                && let Some(pp) = pos(q)
            {
                let hot = is_sel(q) || is_sel(n);
                painter.line_segment(
                    [pp, p],
                    Stroke::new(4.0_f32, Color32::from_black_alpha(160)),
                );
                painter.line_segment(
                    [pp, p],
                    Stroke::new(2.0_f32, if hot { ACCENT } else { Color32::from_gray(225) }),
                );
            }
        }
        for &n in &joints {
            let Some(p) = pos(n) else { continue };
            let r = if is_sel(n) { 5.0 } else { 3.5 };
            painter.circle(
                p,
                r,
                if is_sel(n) { ACCENT } else { ACCENT2 },
                Stroke::new(1.0_f32, Color32::BLACK),
            );
        }
        if let Some(n) = selected
            && let Some(p) = pos(n)
        {
            painter.text(
                p + egui::vec2(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                &rig.nodes[n].name,
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        // Picking.
        let r = &info.response;
        let ctrl = ui.input(|i| i.modifiers.command);
        if r.clicked()
            && (!self.paint || ctrl)
            && let Some(click) = r.interact_pointer_pos()
        {
            let best = joints
                .iter()
                .filter_map(|&n| pos(n).map(|p| (n, p.distance(click))))
                .filter(|(_, d)| *d < 12.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((n, _)) = best {
                self.selected = Some(n);
            }
        }
    }

    /// Brush input on the weight view. `scene` holds the displayed (posed) source mesh.
    /// Returns whether the weights changed this frame.
    pub fn paint_input(
        &mut self,
        ui: &Ui,
        info: &ViewInfo,
        scene: &RenderScene,
        source: &mut Arc<Model>,
    ) -> bool {
        let r = &info.response;
        let Some(node) = self.selected else {
            return false;
        };
        if !self.paint || !source.is_rigged() || ui.input(|i| i.modifiers.command) {
            return false;
        }
        // Brush outline.
        if let Some(h) = r.hover_pos() {
            ui.painter_at(info.rect).circle_stroke(
                h,
                self.radius,
                Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 180)),
            );
        }
        use egui::PointerButton::Primary;
        if r.drag_started_by(Primary) {
            self.undo.push(weights::snapshot(&source.pieces));
            self.redo.clear();
            self.stroke = Some(self.visibility(info, scene, source));
        }
        if r.drag_stopped() && self.stroke.take().is_some() {
            self.revision += 1;
        }
        if !r.dragged_by(Primary) {
            return false;
        }
        let (Some(st), Some(at)) = (self.stroke.as_ref(), r.interact_pointer_pos()) else {
            return false;
        };
        let falloff = brush_hits(st, scene, |p| info.project(p), at, self.radius);
        if falloff.iter().all(Vec::is_empty) {
            return false;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
        let amount = match self.mode {
            BrushMode::Set => self.strength,
            _ => (self.strength * dt * 3.0).min(1.0),
        };
        // Set pulls towards its value over time, like the other modes.
        let set_rate = (dt * 8.0).min(1.0);
        let hits: Vec<Vec<(usize, f32)>> = falloff
            .into_iter()
            .map(|h| {
                h.into_iter()
                    .map(|(v, f)| {
                        (
                            v,
                            if self.mode == BrushMode::Set {
                                f * set_rate
                            } else {
                                f
                            },
                        )
                    })
                    .collect()
            })
            .collect();
        let rig = source.rig.clone().unwrap_or_default();
        let m = Arc::make_mut(source);
        let mut changed = false;
        for (pi, h) in hits.iter().enumerate() {
            let Some(p) = m.pieces.get_mut(pi) else {
                continue;
            };
            let Some(j) = weights::joint_slot(&rig, p, node) else {
                continue;
            };
            let nb = st.neighbours.get(pi).and_then(|n| n.as_ref());
            changed |= weights::paint(p, j, h, self.mode, amount, nb);
        }
        if changed {
            self.view_revision += 1;
        }
        changed
    }

    fn visibility(&self, info: &ViewInfo, scene: &RenderScene, model: &Model) -> BrushStroke {
        let (ids, w, h, tri_base) = render_visibility(info.view, info.proj, info.rect, scene);
        let rect = info.rect;
        let neighbours = model
            .pieces
            .iter()
            .map(|p| {
                (self.mode == BrushMode::Smooth && p.skin.is_some()).then(|| Neighbours::new(p))
            })
            .collect();
        BrushStroke {
            ids,
            w,
            h,
            rect,
            tri_base,
            neighbours,
        }
    }
}

/// Triangle-id buffer of `scene` as seen through `view`/`proj` in `rect` (3/4 resolution):
/// (ids, width, height, first triangle id of every piece).
pub(super) fn render_visibility(
    view: Mat4,
    proj: Mat4,
    rect: Rect,
    scene: &RenderScene,
) -> (Vec<u32>, u32, u32, Vec<u32>) {
    let (w, h) = (
        (rect.width() * 0.75).max(16.0) as u32,
        (rect.height() * 0.75).max(16.0) as u32,
    );
    let mut tri_base = Vec::with_capacity(scene.pieces.len());
    let mut base = 0u32;
    let calls: Vec<DrawCall> = scene
        .pieces
        .iter()
        .map(|p| {
            tri_base.push(base);
            let c = DrawCall {
                positions: &p.positions,
                uvs: &[],
                colors: &[],
                indices: &p.indices,
                texture: None,
                id_base: base,
                double_sided: p.double_sided,
                normals: &[],
                pbr: None,
            };
            base += (p.indices.len() / 3) as u32;
            c
        })
        .collect();
    let mut frame = Frame::new(w, h);
    let opts = RasterOptions {
        vertex_snap: false,
        affine: false,
        dither: false,
        color15: false,
        textured: false,
        zbuffer: true,
        cull_back: true,
        write_ids: true,
        fog: None,
        ..RasterOptions::default()
    };
    raster::render(&mut frame, view, proj, &calls, &opts);
    (frame.ids, w, h, tri_base)
}

/// Vertices under the brush at `at` (radius in points) that belong to visible
/// triangles, with a smooth falloff: per piece, (vertex, 0..1).
pub(super) fn brush_hits(
    st: &BrushStroke,
    scene: &RenderScene,
    project: impl Fn(Vec3) -> Option<egui::Pos2>,
    at: egui::Pos2,
    radius: f32,
) -> Vec<Vec<(usize, f32)>> {
    let mut tris: BTreeSet<u32> = BTreeSet::new();
    let (sx, sy) = (
        st.w as f32 / st.rect.width(),
        st.h as f32 / st.rect.height(),
    );
    let c = ((at.x - st.rect.min.x) * sx, (at.y - st.rect.min.y) * sy);
    let rr = radius * sx;
    let (x0, x1) = (
        (c.0 - rr).max(0.0) as i32,
        (c.0 + rr).min(st.w as f32 - 1.0) as i32,
    );
    let (y0, y1) = (
        (c.1 - rr).max(0.0) as i32,
        (c.1 + rr).min(st.h as f32 - 1.0) as i32,
    );
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 - c.0, y as f32 - c.1);
            if dx * dx + dy * dy <= rr * rr {
                let id = st.ids[(y as u32 * st.w + x as u32) as usize];
                if id > 0 {
                    tris.insert(id - 1);
                }
            }
        }
    }
    let mut hits: Vec<Vec<(usize, f32)>> = vec![Vec::new(); scene.pieces.len()];
    for t in tris {
        let pi = st.tri_base.partition_point(|&b| b <= t).saturating_sub(1);
        let local = (t - st.tri_base[pi]) as usize;
        let Some(rp) = scene.pieces.get(pi) else {
            continue;
        };
        for k in 0..3 {
            let Some(&vi) = rp.indices.get(local * 3 + k) else {
                continue;
            };
            let vi = vi as usize;
            if hits[pi].iter().any(|h| h.0 == vi) {
                continue;
            }
            let Some(sp) = rp.positions.get(vi).and_then(|&p| project(p)) else {
                continue;
            };
            let d = sp.distance(at) / radius.max(1e-3);
            if d < 1.0 {
                hits[pi].push((vi, (1.0 - d * d).powi(2)));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A front quad at z = 0.5 and a back quad at z = -0.5, both facing +Z (the camera).
    fn two_quads() -> RenderScene {
        let quad = |z: f32| RenderPiece {
            positions: vec![
                Vec3::new(-1.0, -1.0, z),
                Vec3::new(1.0, -1.0, z),
                Vec3::new(1.0, 1.0, z),
                Vec3::new(-1.0, 1.0, z),
            ],
            uvs: Vec::new(),
            colors: Vec::new(),
            indices: vec![0, 1, 2, 0, 2, 3],
            texture: None,
            double_sided: false,
            normals: Vec::new(),
            vertex_colors: Vec::new(),
            material: None,
        };
        RenderScene {
            pieces: vec![quad(0.5), quad(-0.5)],
            ..Default::default()
        }
    }

    #[test]
    fn brush_reaches_visible_surface_only_with_falloff() {
        let scene = two_quads();
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(50f32.to_radians(), 1.0, 0.1, 100.0);
        let project = |p: Vec3| {
            let c = proj * view * p.extend(1.0);
            let n = c.truncate() / c.w;
            Some(egui::pos2(
                rect.min.x + (n.x * 0.5 + 0.5) * rect.width(),
                rect.min.y + (0.5 - n.y * 0.5) * rect.height(),
            ))
        };
        let (ids, w, h, tri_base) = render_visibility(view, proj, rect, &scene);
        let st = BrushStroke {
            ids,
            w,
            h,
            rect,
            tri_base,
            neighbours: Vec::new(),
        };
        // A big brush at the top-right corner of the front quad.
        let corner = project(Vec3::new(1.0, 1.0, 0.5)).unwrap();
        let hits = brush_hits(&st, &scene, project, corner, 60.0);
        assert!(
            hits[1].is_empty(),
            "hidden back quad must not be painted: {:?}",
            hits[1]
        );
        let at_corner = hits[0]
            .iter()
            .find(|h| h.0 == 2)
            .expect("corner vertex hit");
        assert!(at_corner.1 > 0.99, "full strength at the centre");
        assert!(
            hits[0].iter().all(|h| h.0 == 2 || h.1 < 0.99),
            "others fall off or are out of reach: {:?}",
            hits[0]
        );
        // Missing the mesh hits nothing.
        let miss = brush_hits(&st, &scene, project, egui::pos2(5.0, 5.0), 3.0);
        assert!(miss.iter().all(Vec::is_empty));
    }
}
