//! Paint tool: colour triangles with a brush, fill a face or smooth area, or single
//! triangles. Painted colours print as separate filaments (Bambu AMS / Prusa MMU): when
//! saved as 3MF, every colour gets a filament slot.

use crate::app::{App, JobOutput, Tool};
use crate::expr::Kind;
use crate::ui::num;
use b3d_core::paint;
use b3d_core::{Mesh, ObjectId};
use egui::{Color32, RichText};
use glam::Vec3;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaintMode {
    /// Round brush: drag over the surface.
    Brush,
    /// A face or smooth area, up to sharp edges.
    Fill,
    /// One triangle at a time.
    Triangle,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PaintSettings {
    pub color: [u8; 4],
    pub mode: PaintMode,
    /// Brush radius, mm.
    pub radius: f32,
    /// Fill stops at edges sharper than this, degrees.
    pub angle: f32,
    pub erase: bool,
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self { color: [248, 113, 113, 255], mode: PaintMode::Brush, radius: 2.0, angle: 30.0, erase: false }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaintHover {
    pub id: ObjectId,
    pub point: Vec3,
    pub normal: Vec3,
    pub tri: usize,
}

#[derive(Default)]
pub struct PaintRuntime {
    pub hover: Option<PaintHover>,
    /// The object most recently under the brush (target of Refine / Clear without a selection).
    pub last_obj: Option<ObjectId>,
    /// A stroke is in progress (history checkpoint taken).
    pub stroke: bool,
    last: Option<(ObjectId, Vec3)>,
    /// Filament slots of the scene, keyed by a hash of the objects.
    slots: Option<(u64, Vec<u32>)>,
}

impl App {
    pub fn start_paint_tool(&mut self) {
        self.tool = Tool::Paint;
        if self.scene.objects.is_empty() {
            self.info("Paint: add or open a model first");
        }
    }

    pub fn begin_paint_stroke(&mut self) {
        self.checkpoint("Paint");
        self.paint_rt.stroke = true;
        self.paint_rt.last = None;
    }

    pub fn end_paint_stroke(&mut self) {
        self.paint_rt.stroke = false;
        self.paint_rt.last = None;
    }

    /// Paint at the hovered spot. `first`: start of a click / stroke.
    pub fn paint_at(&mut self, h: PaintHover, first: bool) {
        let s = self.settings.paint.clone();
        let Some(o) = self.scene.get(h.id) else { return };
        if o.locked || o.hole {
            return;
        }
        let m = &o.mesh.mesh;
        if h.tri >= m.triangles.len() {
            return;
        }
        let xf = o.matrix();
        let inv = xf.inverse();
        let mut value = if s.erase { 0 } else { paint::pack(s.color) };
        if value == paint::pack(o.color) {
            value = 0; // the object's own colour is "unpainted"
        }
        let tris: Vec<usize> = match s.mode {
            PaintMode::Triangle => vec![h.tri],
            PaintMode::Fill => {
                if !first {
                    return;
                }
                paint::fill_region(m, h.tri, s.angle)
            }
            PaintMode::Brush => {
                let scale = o.transform.scale.abs();
                let r = s.radius / ((scale.x + scale.y + scale.z) / 3.0).max(1e-6);
                let n = inv.transpose().transform_vector3(h.normal).normalize_or_zero();
                // join this spot to the previous one so fast strokes leave no gaps
                let from = match self.paint_rt.last {
                    Some((id, p)) if id == h.id && p.distance(h.point) < s.radius * 8.0 => p,
                    _ => h.point,
                };
                let mut all = paint::brush_stroke(m, inv.transform_point3(from), inv.transform_point3(h.point), r, n);
                if !all.contains(&h.tri) {
                    all.push(h.tri);
                }
                all
            }
        };
        self.paint_rt.last = Some((h.id, h.point));
        if tris.iter().all(|&i| m.paint_of(i) == value) {
            return;
        }
        if let Some(o) = self.scene.get_mut(h.id) {
            o.mesh.repaint(|p| {
                for &i in &tris {
                    p[i] = value;
                }
            });
        }
        self.dirty = true;
        self.edit_gen += 1;
    }

    /// Ctrl+click: take the colour under the pointer.
    pub fn pick_paint_color(&mut self, h: PaintHover) {
        let Some(o) = self.scene.get(h.id) else { return };
        let c = o.mesh.mesh.paint_of(h.tri);
        self.settings.paint.color = if c == 0 { o.color } else { paint::unpack(c) };
        self.settings.paint.erase = false;
    }

    /// Remove all paint from the selection (or the hovered object).
    pub fn clear_paint_selected(&mut self) {
        let ids: Vec<ObjectId> = self.paint_targets();
        let ids: Vec<ObjectId> = ids.into_iter().filter(|id| self.scene.get(*id).is_some_and(|o| o.mesh.mesh.is_painted())).collect();
        if ids.is_empty() {
            return;
        }
        self.checkpoint("Clear paint");
        for id in ids {
            if let Some(o) = self.scene.get_mut(id) {
                o.mesh.repaint(|p| p.iter_mut().for_each(|c| *c = 0));
            }
        }
        self.info("Paint removed");
    }

    fn paint_targets(&self) -> Vec<ObjectId> {
        if !self.selection.is_empty() {
            self.selection.clone()
        } else {
            self.paint_rt.hover.map(|h| h.id).or(self.paint_rt.last_obj).into_iter().collect()
        }
    }

    /// Split big triangles so the brush can paint finer detail.
    pub fn refine_for_paint(&mut self, ctx: &egui::Context) {
        let edge = (self.settings.paint.radius * 0.75).max(0.2);
        let jobs: Vec<(ObjectId, Mesh, f32)> = self
            .paint_targets()
            .iter()
            .filter_map(|id| self.scene.get(*id))
            .map(|o| {
                let s = o.transform.scale.abs();
                (o.id, (*o.mesh.mesh).clone(), edge / ((s.x + s.y + s.z) / 3.0).max(1e-6))
            })
            .collect();
        if jobs.is_empty() {
            self.error("Paint on the object first, or select it");
            return;
        }
        self.spawn_job(ctx, "Refining for painting…", move |p| {
            let mut out = vec![];
            let (mut before, mut after) = (0, 0);
            for (id, m, e) in jobs {
                if p.is_cancelled() {
                    return JobOutput::Error("cancelled".into());
                }
                let r = paint::refine(&m, e, 2_000_000);
                before += m.triangles.len();
                after += r.triangles.len();
                out.push((id, r));
            }
            JobOutput::ReplaceMeshes {
                label: "Refine for painting".into(),
                meshes: out,
                message: format!("Refined for painting: {before} to {after} triangles"),
            }
        });
    }

    /// Filament slots the current scene would use (cached).
    pub fn paint_slots(&mut self) -> Vec<u32> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for o in &self.scene.objects {
            (o.mesh.id, o.mesh.paint_rev, o.color, o.hole).hash(&mut h);
        }
        let key = h.finish();
        if let Some((k, v)) = &self.paint_rt.slots {
            if *k == key {
                return v.clone();
            }
        }
        let solids: Vec<&b3d_core::Object> = self.scene.objects.iter().filter(|o| !o.hole).collect();
        let v = paint::slots(solids.iter().map(|o| o.color), solids.iter().map(|o| &*o.mesh.mesh));
        self.paint_rt.slots = Some((key, v.clone()));
        v
    }

    /// Carry paint (and, for `colors`, the objects' own colours) from the objects being
    /// replaced over to new meshes. Returns the source mesh (world space), if any.
    pub fn paint_source(&self, ids: &[ObjectId], colors: bool) -> Option<Mesh> {
        let objs: Vec<&b3d_core::Object> = ids.iter().filter_map(|id| self.scene.get(*id)).filter(|o| !o.hole).collect();
        let painted = objs.iter().any(|o| o.mesh.mesh.is_painted());
        let mixed = colors && objs.iter().any(|o| o.color != objs[0].color);
        if !painted && !mixed {
            return None;
        }
        let mut src = Mesh::default();
        for o in objs {
            let mut w = o.world_mesh();
            if mixed {
                let base = paint::pack(o.color);
                let p = w.paint_mut();
                p.iter_mut().filter(|c| **c == 0).for_each(|c| *c = base);
            }
            src.append(&w);
        }
        Some(src)
    }
}

/// Give `dst` the paint of `src` (both in the same space); paint equal to the object's
/// own colour becomes "unpainted".
pub fn carry_paint(src: &Mesh, dst: &mut Mesh, own: [u8; 4], tol: f32) {
    if dst.is_painted() {
        return;
    }
    if paint::transfer(src, dst, tol) {
        let own = paint::pack(own);
        dst.paint.iter_mut().filter(|c| **c == own).for_each(|c| *c = 0);
        dst.tidy_paint();
    }
}

fn swatch(ui: &mut egui::Ui, c: [u8; 4], selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let col = Color32::from_rgb(c[0], c[1], c[2]);
    ui.painter().rect_filled(rect.shrink(2.0), 3.0, col);
    if selected {
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
    }
    resp
}

/// Right-hand panel for the paint tool.
pub fn panel(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.heading("Paint");
    let slots = app.paint_slots();
    let s = &mut app.settings.paint;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut s.mode, PaintMode::Brush, "Brush").on_hover_text("Drag over the surface");
        ui.selectable_value(&mut s.mode, PaintMode::Fill, "Fill").on_hover_text("A face or smooth area, up to sharp edges");
        ui.selectable_value(&mut s.mode, PaintMode::Triangle, "Triangle").on_hover_text("One triangle at a time");
    });
    // colours: what the scene already uses, then a palette, then any colour
    let mut choices: Vec<[u8; 4]> = slots.iter().map(|c| paint::unpack(*c)).collect();
    for o in app.scene.objects.iter().filter(|o| !o.hole) {
        if !choices.contains(&o.color) {
            choices.push(o.color);
        }
    }
    let used = choices.len();
    for c in b3d_core::scene::PALETTE.iter().chain(&[[255, 255, 255, 255], [30, 30, 30, 255]]) {
        if !choices.contains(c) {
            choices.push(*c);
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, c) in choices.iter().enumerate() {
            if i == used && used > 0 {
                ui.add_space(8.0);
            }
            if swatch(ui, *c, !s.erase && s.color == *c).on_hover_text(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])).clicked() {
                s.color = *c;
                s.erase = false;
            }
        }
        let mut c32 = Color32::from_rgb(s.color[0], s.color[1], s.color[2]);
        if ui.color_edit_button_srgba(&mut c32).on_hover_text("Any colour").changed() {
            s.color = [c32.r(), c32.g(), c32.b(), 255];
            s.erase = false;
        }
    });
    ui.checkbox(&mut s.erase, "Eraser (back to the object's colour)");
    egui::Grid::new("paint_params").num_columns(2).show(ui, |ui| match s.mode {
        PaintMode::Brush => {
            ui.label("Brush size");
            ui.add(num(&mut s.radius, Kind::Length).speed(0.05).range(0.1..=200.0).suffix(" mm")).on_hover_text("Radius");
            ui.end_row();
        }
        PaintMode::Fill => {
            ui.label("Stop at edges over");
            ui.add(egui::Slider::new(&mut s.angle, 1.0..=180.0).suffix("°").integer())
                .on_hover_text("Smaller: just one flat face. 180°: the whole connected part.");
            ui.end_row();
        }
        PaintMode::Triangle => {}
    });
    ui.weak("Drag on a model to paint; drag off it (or Alt+drag) to orbit. Ctrl+click picks a colour.");
    ui.horizontal(|ui| {
        if ui
            .button("Refine")
            .on_hover_text("Split big triangles of the selected (or hovered) object so the brush can paint finer detail")
            .clicked()
        {
            app.refine_for_paint(ctx);
        }
        if ui.button("Clear paint").on_hover_text("Remove all paint from the selected (or hovered) object").clicked() {
            app.clear_paint_selected();
        }
    });
    if !slots.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new("Filament slots (3MF)").strong())
            .on_hover_text("Saved as 3MF, each colour prints with its own filament: Bambu AMS / Orca / PrusaSlicer MMU. Load the same colours into these slots.");
        ui.horizontal_wrapped(|ui| {
            for (i, c) in slots.iter().enumerate() {
                let c = paint::unpack(*c);
                ui.label(format!("{}", i + 1));
                swatch(ui, c, false);
                ui.add_space(4.0);
            }
        });
    }
    if ui.button("Done").clicked() {
        app.tool = Tool::Select;
    }
    ui.separator();
}
