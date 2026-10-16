//! Select faces: brush or fill-select an area of one object, then extrude / push it,
//! delete it, or copy it out as a solid (Meshmixer-style select & edit).

use crate::app::{App, Tool};
use crate::expr::Kind;
use crate::paint_tool::PaintHover;
use crate::ui::num;
use b3d_core::scene::MeshHandle;
use b3d_core::{faces, paint, ObjectId, Transform};
use egui::RichText;
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SelMode {
    Brush,
    Fill,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FaceSettings {
    pub mode: SelMode,
    pub radius: f32,
    pub angle: f32,
    /// Extrude distance, mm (negative pushes in).
    pub dist: f32,
    pub along_normals: bool,
    /// Copy as solid: thickness, mm.
    pub thickness: f32,
}

impl Default for FaceSettings {
    fn default() -> Self {
        Self { mode: SelMode::Fill, radius: 3.0, angle: 20.0, dist: 2.0, along_normals: false, thickness: 2.0 }
    }
}

/// Selection colour (shown as paint on a display copy).
const HIGHLIGHT: [u8; 4] = [255, 196, 40, 255];

#[derive(Default)]
pub struct FaceRuntime {
    pub obj: Option<ObjectId>,
    /// Geometry the selection belongs to (mesh id).
    mesh_id: u64,
    pub sel: Vec<bool>,
    pub hover: Option<PaintHover>,
    last: Option<Vec3>,
    /// Display copy of the object with the selection painted in.
    display: Option<MeshHandle>,
    dirty: bool,
}

impl FaceRuntime {
    pub fn count(&self) -> usize {
        self.sel.iter().filter(|s| **s).count()
    }
}

impl App {
    pub fn start_faces_tool(&mut self) {
        self.tool = Tool::Faces;
        if let Some(&id) = self.selection.first() {
            self.face_target(id);
        }
    }

    /// Make `id` the object being selected on (clears a selection on another object or
    /// an outdated one).
    fn face_target(&mut self, id: ObjectId) -> bool {
        let Some(o) = self.scene.get(id) else { return false };
        if o.locked || o.hole {
            return false;
        }
        let rt = &mut self.face_rt;
        if rt.obj != Some(id) || rt.mesh_id != o.mesh.id || rt.sel.len() != o.mesh.mesh.triangles.len() {
            rt.obj = Some(id);
            rt.mesh_id = o.mesh.id;
            rt.sel = vec![false; o.mesh.mesh.triangles.len()];
            rt.display = None;
            rt.dirty = true;
        }
        true
    }

    /// Keep the selection valid (called every frame).
    pub fn tick_faces(&mut self) {
        if self.tool != Tool::Faces {
            if self.face_rt.obj.is_some() {
                self.face_rt = FaceRuntime::default();
            }
            return;
        }
        if let Some(id) = self.face_rt.obj {
            match self.scene.get(id) {
                Some(o) if o.mesh.id == self.face_rt.mesh_id => {}
                Some(_) => {
                    // the mesh changed (undo, another edit): start over on the new one
                    self.face_rt.mesh_id = 0;
                    self.face_target(id);
                }
                None => self.face_rt = FaceRuntime::default(),
            }
        }
        if self.face_rt.dirty {
            self.face_rt.dirty = false;
            let Some(o) = self.face_rt.obj.and_then(|id| self.scene.get(id)) else { return };
            let base = o.mesh.mesh.clone();
            let sel = self.face_rt.sel.clone();
            let disp = self.face_rt.display.get_or_insert_with(|| MeshHandle::new((*base).clone()));
            let hi = paint::pack(HIGHLIGHT);
            disp.repaint(|p| {
                for (i, c) in p.iter_mut().enumerate() {
                    *c = if sel.get(i).copied().unwrap_or(false) { hi } else { base.paint_of(i) };
                }
            });
        }
    }

    /// Mesh to draw for `id` while it has a face selection.
    pub fn face_display(&self, id: ObjectId) -> Option<MeshHandle> {
        if self.tool == Tool::Faces && self.face_rt.obj == Some(id) {
            self.face_rt.display.clone()
        } else {
            None
        }
    }

    /// Select (or with `remove`, deselect) at the hovered spot.
    pub fn face_select_at(&mut self, h: PaintHover, first: bool, remove: bool) {
        if !self.face_target(h.id) {
            return;
        }
        if first {
            self.face_rt.last = None;
        }
        let s = self.settings.faces.clone();
        let Some(o) = self.scene.get(h.id) else { return };
        let m = &o.mesh.mesh;
        if h.tri >= m.triangles.len() {
            return;
        }
        let inv = o.matrix().inverse();
        let tris = match s.mode {
            SelMode::Fill => {
                if !first {
                    return;
                }
                paint::fill_region(m, h.tri, s.angle)
            }
            SelMode::Brush => {
                let sc = o.transform.scale.abs();
                let r = s.radius / ((sc.x + sc.y + sc.z) / 3.0).max(1e-6);
                let n = inv.transpose().transform_vector3(h.normal).normalize_or_zero();
                let from = self.face_rt.last.filter(|p| p.distance(h.point) < s.radius * 8.0).unwrap_or(h.point);
                let mut t = paint::brush_stroke(m, inv.transform_point3(from), inv.transform_point3(h.point), r, n);
                t.push(h.tri);
                t
            }
        };
        self.face_rt.last = Some(h.point);
        let sel = &mut self.face_rt.sel;
        let mut changed = false;
        for i in tris {
            if sel[i] == remove {
                sel[i] = !remove;
                changed = true;
            }
        }
        self.face_rt.dirty |= changed;
    }

    pub fn face_select_all(&mut self, how: u8) {
        let Some(id) = self.face_rt.obj else { return };
        let Some(o) = self.scene.get(id) else { return };
        let m = o.mesh.mesh.clone();
        let sel = &mut self.face_rt.sel;
        match how {
            0 => sel.iter_mut().for_each(|s| *s = true),
            1 => sel.iter_mut().for_each(|s| *s = false),
            2 => sel.iter_mut().for_each(|s| *s = !*s),
            3 => faces::grow(&m, sel),
            _ => faces::shrink(&m, sel),
        }
        self.face_rt.dirty = true;
    }

    fn face_scale(&self, id: ObjectId) -> f32 {
        self.scene
            .get(id)
            .map(|o| {
                let s = o.transform.scale.abs();
                ((s.x + s.y + s.z) / 3.0).max(1e-6)
            })
            .unwrap_or(1.0)
    }

    /// Apply an edit to the selected faces. `op`: 0 extrude, 1 delete, 2 copy as solid.
    pub fn face_edit(&mut self, op: u8) {
        let Some(id) = self.face_rt.obj else { return };
        let n = self.face_rt.count();
        if n == 0 {
            self.error("Select some faces first");
            return;
        }
        let Some(o) = self.scene.get(id).cloned() else { return };
        let s = self.settings.faces.clone();
        let k = self.face_scale(id);
        let sel = self.face_rt.sel.clone();
        let m = &o.mesh.mesh;
        match op {
            0 => {
                self.checkpoint("Extrude faces");
                let out = faces::extrude(m, &sel, s.dist / k, s.along_normals);
                let walls = out.triangles.len() - m.triangles.len();
                let h = MeshHandle::new(out);
                let mut new_sel = sel;
                new_sel.resize(new_sel.len() + walls, false);
                if let Some(ob) = self.scene.get_mut(id) {
                    ob.mesh = h.clone();
                }
                // keep the same faces selected, so you can extrude again
                self.face_rt = FaceRuntime { obj: Some(id), mesh_id: h.id, sel: new_sel, dirty: true, ..Default::default() };
                self.report_cache = None;
                self.info(format!("Moved {n} triangles {:+.2} mm", s.dist));
            }
            1 => {
                self.checkpoint("Delete faces");
                let out = faces::delete(m, &sel);
                if let Some(ob) = self.scene.get_mut(id) {
                    ob.mesh = MeshHandle::new(out);
                }
                self.report_cache = None;
                self.info(format!("Deleted {n} triangles — the model now has an opening (Repair closes it)"));
            }
            _ => {
                self.checkpoint("Copy faces as solid");
                let out = faces::copy_as_solid(m, &sel, s.thickness / k);
                let name = self.scene.unique_name(&format!("{} piece", o.name));
                let nid = self.scene.add_object(name, out, Transform { ..o.transform }, Some(o.color));
                self.selection = vec![nid];
                self.info(format!("Copied the selection as a solid {:.2} mm thick", s.thickness));
            }
        }
    }
}

/// Right-hand panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.heading("Select faces");
    let n = app.face_rt.count();
    let s = &mut app.settings.faces;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut s.mode, SelMode::Fill, "Fill").on_hover_text("A face or smooth area, up to sharp edges");
        ui.selectable_value(&mut s.mode, SelMode::Brush, "Brush").on_hover_text("Drag over the surface");
    });
    egui::Grid::new("facesel").num_columns(2).show(ui, |ui| match s.mode {
        SelMode::Brush => {
            ui.label("Brush size");
            ui.add(num(&mut s.radius, Kind::Length).speed(0.05).range(0.1..=200.0).suffix(" mm"));
            ui.end_row();
        }
        SelMode::Fill => {
            ui.label("Stop at edges over");
            ui.add(egui::Slider::new(&mut s.angle, 1.0..=180.0).suffix("°").integer());
            ui.end_row();
        }
    });
    ui.weak("Click or drag on a model to select; Shift removes. Drag off the model (or Alt+drag) to orbit.");
    ui.horizontal_wrapped(|ui| {
        for (label, how) in [("All", 0u8), ("None", 1), ("Invert", 2), ("Grow", 3), ("Shrink", 4)] {
            if ui.small_button(label).clicked() {
                app.face_select_all(how);
            }
        }
        if ui.small_button("Refine").on_hover_text("Split big triangles so you can select finer areas").clicked() {
            if let Some(id) = app.face_rt.obj {
                app.selection = vec![id];
                app.refine_for_paint(ctx);
            }
        }
    });
    ui.label(if n == 0 { "Nothing selected".to_string() } else { format!("{n} triangles selected") });
    ui.add_space(4.0);
    let s = &mut app.settings.faces;
    let mut op = None;
    ui.add_enabled_ui(n > 0, |ui| {
        ui.label(RichText::new("Extrude / push").strong());
        ui.horizontal(|ui| {
            ui.add(num(&mut s.dist, Kind::Length).speed(0.05).range(-500.0..=500.0).suffix(" mm"))
                .on_hover_text("Positive pulls the area out, negative pushes it in");
            if ui.button("Extrude").clicked() {
                op = Some(0);
            }
        });
        ui.horizontal(|ui| {
            ui.radio_value(&mut s.along_normals, false, "Straight")
                .on_hover_text("Everything moves along the average direction (keeps the shape)");
            ui.radio_value(&mut s.along_normals, true, "Along the surface")
                .on_hover_text("Every point moves along its own normal (an offset; good on curved areas)");
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Copy as solid").on_hover_text("A new object: the selected surface, thickened inwards").clicked() {
                op = Some(2);
            }
            ui.add(num(&mut s.thickness, Kind::Length).speed(0.05).range(0.1..=100.0).suffix(" mm"));
        });
        if ui.button("Delete faces").on_hover_text("Leaves an opening (Repair fills it) — handy for cleaning scans").clicked() {
            op = Some(1);
        }
    });
    if let Some(op) = op {
        app.face_edit(op);
    }
    if ui.button("Done").clicked() {
        app.tool = Tool::Select;
    }
    ui.separator();
}
