//! Detail focus for the Model tab: the automatic face focus, a heat map of where the
//! converter keeps extra triangles and texels, and a brush to paint or erase detail.

use super::rig_panel::{BrushStroke, brush_hits};
use super::viewport::{RenderPiece, RenderScene, ViewInfo};
use super::widgets::{hint, section, slider_f32};
use egui::{Color32, Stroke, Ui};
use psx_forge::detail::DetailField;
use psx_forge::mesh::Model;
use psx_forge::process::ConvertSettings;
use psx_forge::weights;
use std::sync::Arc;

pub struct DetailEditor {
    /// Heat map of the detail field on the source model.
    pub view: bool,
    /// Brush: LMB paints (or erases), RMB orbits.
    pub paint: bool,
    pub erase: bool,
    pub radius: f32,
    pub strength: f32,
    /// Bumped after committed edits (conversion key).
    pub revision: u64,
    view_revision: u64,
    undo: Vec<Vec<Vec<f32>>>,
    stroke: Option<BrushStroke>,
    heat: Option<(u64, u64, bool, RenderScene)>,
}

impl Default for DetailEditor {
    fn default() -> Self {
        Self {
            view: false,
            paint: false,
            erase: false,
            radius: 30.0,
            strength: 0.6,
            revision: 0,
            view_revision: 0,
            undo: Vec::new(),
            stroke: None,
            heat: None,
        }
    }
}

fn painted_of(m: &Model) -> Vec<Vec<f32>> {
    m.pieces.iter().map(|p| p.detail.clone()).collect()
}

impl DetailEditor {
    pub fn reset(&mut self) {
        *self = Self {
            radius: self.radius,
            strength: self.strength,
            ..Default::default()
        };
    }

    pub fn wants_source(&self) -> bool {
        self.view || self.paint
    }

    pub fn section(
        &mut self,
        ui: &mut Ui,
        s: &mut ConvertSettings,
        source: Option<&mut Arc<Model>>,
    ) {
        let mut clear = false;
        let mut undo = false;
        section(ui, "Detail focus", true, |ui| {
            ui.checkbox(&mut s.focus_face, "Keep the face detailed")
                .on_hover_text(
                    "Find the head (head bones on rigs, else the shape of an upright character) \
                 and give it a guaranteed share of the triangles and extra texel density, so \
                 big armour or spikes can't flatten the face.",
                );
            slider_f32(
                ui,
                "strength",
                &mut s.focus_strength,
                1.0..=8.0,
                4.0,
                "The focus area's surface counts this many times for the triangle budget; its texel density goes up to this many times. 1 = off.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut self.view, "Show")
                    .on_hover_text("Heat map of the focus area on the source model (red = most detail)");
                if ui
                    .toggle_value(&mut self.paint, "Paint")
                    .on_hover_text("Paint extra detail anywhere (hands, emblems, a second face). LMB paints, RMB orbits.")
                    .changed()
                    && self.paint
                {
                    self.view = true;
                }
                if self.paint {
                    ui.toggle_value(&mut self.erase, "Erase");
                }
            });
            if self.paint {
                slider_f32(
                    ui,
                    "radius px",
                    &mut self.radius,
                    4.0..=200.0,
                    30.0,
                    "Brush radius on screen",
                );
                slider_f32(
                    ui,
                    "strength ",
                    &mut self.strength,
                    0.05..=1.0,
                    0.6,
                    "Paint amount per second",
                );
            }
            let painted = source
                .as_ref()
                .is_some_and(|m| m.pieces.iter().any(|p| !p.detail.is_empty()));
            ui.horizontal(|ui| {
                clear = ui
                    .add_enabled(painted, egui::Button::new("Clear painted"))
                    .clicked();
                undo = ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                    .clicked();
            });
            hint(
                ui,
                "Painted detail is used on top of the automatic face focus.",
            );
        });
        if let Some(src) = source
            && (clear || undo)
        {
            let m = Arc::make_mut(src);
            if clear {
                self.undo.push(painted_of(m));
                m.pieces.iter_mut().for_each(|p| p.detail.clear());
            } else if let Some(prev) = self.undo.pop() {
                for (p, d) in m.pieces.iter_mut().zip(prev) {
                    p.detail = d;
                }
            }
            self.revision += 1;
            self.view_revision += 1;
        }
    }

    /// Heat-map copy of the (posed) source scene showing the detail field.
    pub fn heat_scene(&mut self, model: &Model, src: &RenderScene, auto: bool) -> &RenderScene {
        let key = (src.generation, self.view_revision, auto);
        if self.heat.as_ref().map(|h| (h.0, h.1, h.2)) != Some(key) {
            let field = DetailField::for_model(model, auto);
            let pieces = src
                .pieces
                .iter()
                .zip(&model.pieces)
                .map(|(rp, p)| {
                    // Sample at bind-space positions (what the converter sees).
                    let colors: Vec<[f32; 4]> = p
                        .vertices
                        .iter()
                        .map(|v| weights::heat(field.at(v.pos)))
                        .collect();
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
                generation: src.generation.wrapping_mul(7_000_003)
                    ^ self.view_revision
                    ^ (u64::from(auto) << 40)
                    ^ (1 << 61),
            };
            self.heat = Some((key.0, key.1, key.2, scene));
        }
        &self.heat.as_ref().expect("just built").3
    }

    /// Brush input on the detail view (`scene` = displayed source mesh).
    pub fn paint_input(
        &mut self,
        ui: &Ui,
        info: &ViewInfo,
        scene: &RenderScene,
        source: &mut Arc<Model>,
    ) -> bool {
        if !self.paint {
            return false;
        }
        let r = &info.response;
        if let Some(h) = r.hover_pos() {
            ui.painter_at(info.rect).circle_stroke(
                h,
                self.radius,
                Stroke::new(
                    1.5_f32,
                    if self.erase {
                        Color32::from_rgb(120, 180, 255)
                    } else {
                        Color32::from_rgb(255, 140, 90)
                    },
                ),
            );
        }
        use egui::PointerButton::Primary;
        if r.drag_started_by(Primary) {
            self.undo.push(painted_of(source));
            self.stroke = Some(BrushStroke::new(info, scene));
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
        let hits = brush_hits(st, scene, |p| info.project(p), at, self.radius);
        if hits.iter().all(Vec::is_empty) {
            return false;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
        let amount = (self.strength * dt * 3.0).min(1.0) * if self.erase { -1.0 } else { 1.0 };
        let m = Arc::make_mut(source);
        for (p, h) in m.pieces.iter_mut().zip(&hits) {
            if h.is_empty() {
                continue;
            }
            if p.detail.len() != p.vertices.len() {
                p.detail = vec![0.0; p.vertices.len()];
            }
            for &(v, f) in h {
                p.detail[v] = (p.detail[v] + amount * f).clamp(0.0, 1.0);
            }
        }
        self.view_revision += 1;
        true
    }
}
