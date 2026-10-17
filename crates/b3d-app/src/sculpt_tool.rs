//! Sculpt tool: Draw, Inflate, Smooth, Flatten and Grab brushes, X-mirror symmetry, and
//! detail added under the brush as you go.

use crate::app::{App, Tool};
use crate::expr::Kind;
use crate::paint_tool::PaintHover;
use crate::ui::num;
use b3d_core::scene::MeshHandle;
use b3d_core::sculpt::{Brush, Sculpt};
use b3d_core::ObjectId;
use egui::RichText;
use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SculptBrush {
    Draw,
    Inflate,
    Smooth,
    Flatten,
    Grab,
}

impl SculptBrush {
    fn core(self) -> Brush {
        match self {
            SculptBrush::Draw => Brush::Draw,
            SculptBrush::Inflate => Brush::Inflate,
            SculptBrush::Smooth => Brush::Smooth,
            SculptBrush::Flatten => Brush::Flatten,
            SculptBrush::Grab => Brush::Grab,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SculptSettings {
    pub brush: SculptBrush,
    pub radius: f32,
    pub strength: f32,
    pub mirror_x: bool,
    pub detail: bool,
}

impl Default for SculptSettings {
    fn default() -> Self {
        Self { brush: SculptBrush::Draw, radius: 4.0, strength: 0.5, mirror_x: false, detail: true }
    }
}

struct GrabState {
    /// (vertex, weight) and start positions, for the brush and its mirror.
    parts: Vec<(Vec<(usize, f32)>, Vec<Vec3>, bool)>,
    start: Vec3,
    plane_n: Vec3,
}

#[derive(Default)]
pub struct SculptRuntime {
    obj: Option<ObjectId>,
    mesh_id: u64,
    sc: Option<Sculpt>,
    pub hover: Option<PaintHover>,
    last: Option<Vec3>,
    grab: Option<GrabState>,
    invert: bool,
    smooth_override: bool,
    pub stroke: bool,
    /// Big meshes: the scene keeps the mesh from the start of the stroke and the display
    /// is updated in place from these moved vertices; the scene gets the result at the end.
    pending: std::cell::RefCell<Vec<u32>>,
    unpushed: bool,
    /// This stroke's tag and how many display updates it has sent.
    live_tag: u64,
    live_seq: std::cell::Cell<u64>,
}

/// Above this many triangles, strokes update the display in place instead of handing a
/// new copy of the mesh to the scene after every dab.
const BIG_MESH: usize = 250_000;

/// Mean scale of a transform (brush sizes are in mm on screen, the mesh in its own units).
fn mean_scale(m: &Mat4) -> f32 {
    let (s, _, _) = m.to_scale_rotation_translation();
    let s = s.abs();
    ((s.x + s.y + s.z) / 3.0).max(1e-6)
}

impl App {
    pub fn start_sculpt_tool(&mut self) {
        self.tool = Tool::Sculpt;
    }

    /// Make sure the sculpt copy belongs to `id` and is up to date.
    fn sculpt_target(&mut self, id: ObjectId) -> bool {
        let Some(o) = self.scene.get(id) else { return false };
        if o.locked || o.hole {
            return false;
        }
        let rt = &mut self.sculpt_rt;
        if rt.obj != Some(id) || rt.mesh_id != o.mesh.id || rt.sc.is_none() {
            rt.obj = Some(id);
            rt.mesh_id = o.mesh.id;
            rt.sc = Some(Sculpt::with_bvh((*o.mesh.mesh).clone(), o.mesh.bvh()));
            rt.pending.borrow_mut().clear();
            rt.unpushed = false;
        }
        true
    }

    /// The object being sculpted, while the sculpt tool has one.
    pub fn sculpt_target_id(&self) -> Option<ObjectId> {
        let id = self.sculpt_rt.sc.as_ref().and(self.sculpt_rt.obj)?;
        // after an undo the scene has moved on: the sculpt copy no longer applies
        (self.scene.get(id)?.mesh.id == self.sculpt_rt.mesh_id).then_some(id)
    }

    /// Ray hit on the sculpted object, using the up-to-date sculpt copy.
    pub fn sculpt_hit(&self, o: Vec3, d: Vec3, keep: impl Fn(Vec3) -> bool) -> Option<(f32, PaintHover)> {
        let (id, sc) = (self.sculpt_target_id()?, self.sculpt_rt.sc.as_ref()?);
        let ob = self.scene.get(id).filter(|ob| ob.visible)?;
        let m = ob.matrix();
        let inv = m.inverse();
        let (lo, ld) = (inv.transform_point3(o), inv.transform_vector3(d));
        let (t, tri) = sc.bvh.ray_cast_where(&sc.mesh, lo, ld, |lp| keep(m.transform_point3(lp)))?;
        let point = m.transform_point3(lo + ld * t);
        let mut normal = inv.transpose().transform_vector3(sc.mesh.face_normal(tri)).normalize_or_zero();
        if m.determinant() < 0.0 {
            normal = -normal;
        }
        Some(((point - o).length(), PaintHover { id, point, normal, tri }))
    }

    /// Display updates for a big mesh being sculpted (called once per frame).
    pub fn sculpt_live(&self) -> Option<(u64, std::sync::Arc<crate::render::LiveDelta>)> {
        let (id, sc) = (self.sculpt_rt.obj?, self.sculpt_rt.sc.as_ref()?);
        let mut moved = std::mem::take(&mut *self.sculpt_rt.pending.borrow_mut());
        if moved.is_empty() {
            return None;
        }
        let h = &self.scene.get(id)?.mesh;
        if h.mesh.triangles.len() != sc.mesh.triangles.len() {
            return None;
        }
        moved.sort_unstable();
        moved.dedup();
        let smooth = self.settings.smooth_shading;
        let shading = (smooth || h.mesh.is_painted()).then(|| h.shading());
        let normals = match (&shading, smooth) {
            (Some(sh), true) => sh.renormal(&sc.mesh, &moved),
            _ => vec![],
        };
        let positions = moved.iter().map(|&v| (v, sc.mesh.positions[v as usize])).collect();
        let seq = self.sculpt_rt.live_seq.get() + 1;
        self.sculpt_rt.live_seq.set(seq);
        let tag = self.sculpt_rt.live_tag;
        Some((h.id, std::sync::Arc::new(crate::render::LiveDelta { tag, seq, positions, normals, shading })))
    }

    /// After some dabs: show them. Small meshes get a new mesh on the object right away;
    /// big ones are updated in place on screen and handed to the scene when the stroke ends.
    fn sculpt_push(&mut self) {
        let Some(sc) = self.sculpt_rt.sc.as_mut() else { return };
        let topo = std::mem::take(&mut sc.topology_changed);
        if sc.mesh.triangles.len() > BIG_MESH && !topo {
            let moved = std::mem::take(&mut sc.moved);
            self.sculpt_rt.pending.borrow_mut().extend(moved);
            self.sculpt_rt.unpushed = true;
            self.dirty = true;
            return;
        }
        sc.moved.clear();
        self.sculpt_rt.pending.borrow_mut().clear();
        self.sculpt_push_now(false);
    }

    /// Hand the sculpted mesh to the scene. `in_place`: the display already shows it.
    fn sculpt_push_now(&mut self, in_place: bool) {
        let (Some(id), Some(sc)) = (self.sculpt_rt.obj, &self.sculpt_rt.sc) else { return };
        self.sculpt_rt.unpushed = false;
        let h = match self.scene.get(id) {
            Some(o) if in_place => {
                MeshHandle::moved_from(&o.mesh, sc.mesh.clone(), sc.bvh.clone(), self.sculpt_rt.live_tag, self.sculpt_rt.live_seq.get())
            }
            _ => MeshHandle::with_bvh(sc.mesh.clone(), sc.bvh.clone()),
        };
        self.sculpt_rt.mesh_id = h.id;
        if let Some(o) = self.scene.get_mut(id) {
            o.mesh = h;
        }
        self.dirty = true;
        self.edit_gen += 1;
    }

    /// One dab (and its mirror) at a world point.
    fn sculpt_dab(&mut self, id: ObjectId, point: Vec3, normal: Vec3) {
        let Some(o) = self.scene.get(id) else { return };
        let xf = o.matrix();
        let inv = xf.inverse();
        let k = mean_scale(&xf);
        let s = self.settings.sculpt.clone();
        let brush = if self.sculpt_rt.smooth_override { Brush::Smooth } else { s.brush.core() };
        let c = inv.transform_point3(point);
        let n = inv.transpose().transform_vector3(normal).normalize_or(Vec3::Z);
        let r = s.radius / k;
        let invert = self.sculpt_rt.invert;
        let Some(sc) = self.sculpt_rt.sc.as_mut() else { return };
        let mut spots = vec![(c, n)];
        if s.mirror_x {
            spots.push((Vec3::new(-c.x, c.y, c.z), Vec3::new(-n.x, n.y, n.z)));
        }
        for (c, n) in spots {
            if s.detail {
                sc.ensure_detail(c, r, 3_000_000);
            }
            sc.dab(brush, c, n, r, s.strength, invert);
        }
    }

    /// Start a stroke. `view_dir` is the pointer ray's direction (for Grab).
    pub fn sculpt_begin(&mut self, h: PaintHover, view_dir: Vec3, invert: bool, smooth: bool) {
        if !self.sculpt_target(h.id) {
            return;
        }
        self.checkpoint("Sculpt");
        let rt = &mut self.sculpt_rt;
        rt.stroke = true;
        if !rt.unpushed {
            // a new stroke from what the scene (and the display) shows
            rt.live_tag = MeshHandle::next_tag();
            rt.live_seq.set(0);
            rt.pending.borrow_mut().clear();
        }
        rt.invert = invert;
        rt.smooth_override = smooth;
        rt.last = Some(h.point);
        if self.settings.sculpt.brush == SculptBrush::Grab && !smooth {
            let Some(o) = self.scene.get(h.id) else { return };
            let xf = o.matrix();
            let inv = xf.inverse();
            let r = self.settings.sculpt.radius / mean_scale(&xf);
            let c = inv.transform_point3(h.point);
            let mirror = self.settings.sculpt.mirror_x;
            let Some(sc) = self.sculpt_rt.sc.as_mut() else { return };
            if self.settings.sculpt.detail {
                sc.ensure_detail(c, r, 3_000_000);
                if mirror {
                    sc.ensure_detail(Vec3::new(-c.x, c.y, c.z), r, 3_000_000);
                }
            }
            let mut parts = vec![];
            let (hits, base) = sc.grab_begin(c, r);
            parts.push((hits, base, false));
            if mirror {
                let (hits, base) = sc.grab_begin(Vec3::new(-c.x, c.y, c.z), r);
                parts.push((hits, base, true));
            }
            self.sculpt_rt.grab = Some(GrabState { parts, start: h.point, plane_n: view_dir.normalize_or(Vec3::Z) });
            self.sculpt_push();
        } else {
            self.sculpt_dab(h.id, h.point, h.normal);
            self.sculpt_push();
        }
    }

    /// Continue the stroke: the hovered spot, and the pointer ray (for Grab).
    pub fn sculpt_drag(&mut self, h: Option<PaintHover>, ray: Option<(Vec3, Vec3)>) {
        let Some(id) = self.sculpt_rt.obj else { return };
        if !self.sculpt_rt.stroke {
            return;
        }
        if let Some(g) = &self.sculpt_rt.grab {
            let Some((o, d)) = ray else { return };
            let denom = d.dot(g.plane_n);
            if denom.abs() < 1e-6 {
                return;
            }
            let t = (g.start - o).dot(g.plane_n) / denom;
            let p = o + d * t;
            let Some(obj) = self.scene.get(id) else { return };
            let inv = obj.matrix().inverse();
            let delta = inv.transform_vector3(p - g.start);
            let parts: Vec<_> = g.parts.iter().map(|(h, b, m)| (h.clone(), b.clone(), *m)).collect();
            if let Some(sc) = self.sculpt_rt.sc.as_mut() {
                for (hits, base, mirrored) in parts {
                    let d = if mirrored { Vec3::new(-delta.x, delta.y, delta.z) } else { delta };
                    sc.grab_move(&hits, &base, d);
                }
            }
            self.sculpt_push();
            return;
        }
        let Some(h) = h.filter(|h| h.id == id) else { return };
        // dabs along the way so fast strokes stay continuous
        let r = self.settings.sculpt.radius;
        let from = self.sculpt_rt.last.unwrap_or(h.point);
        let dist = from.distance(h.point);
        if dist < r * 0.2 {
            return;
        }
        if dist > r * 6.0 {
            self.sculpt_rt.last = Some(h.point);
            return;
        }
        let steps = (dist / (r * 0.3)).ceil().max(1.0) as usize;
        for k in 1..=steps {
            let p = from.lerp(h.point, k as f32 / steps as f32);
            self.sculpt_dab(id, p, h.normal);
        }
        self.sculpt_rt.last = Some(h.point);
        self.sculpt_push();
    }

    pub fn sculpt_end(&mut self) {
        if self.sculpt_rt.unpushed {
            self.sculpt_push_now(true);
        }
        self.edit_gen += 1;
        let rt = &mut self.sculpt_rt;
        rt.stroke = false;
        rt.grab = None;
        rt.last = None;
        self.report_cache = None;
    }

    /// Forget the sculpt copy when the tool is left (called every frame).
    pub fn tick_sculpt(&mut self) {
        if self.tool != Tool::Sculpt && self.sculpt_rt.sc.is_some() {
            self.sculpt_rt = SculptRuntime::default();
        }
    }
}

/// Right-hand panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Sculpt");
    let s = &mut app.settings.sculpt;
    ui.horizontal_wrapped(|ui| {
        for (b, name, tip) in [
            (SculptBrush::Draw, "Draw", "Push the surface out (Shift: in)"),
            (SculptBrush::Inflate, "Inflate", "Puff it up along its own curvature (Shift: deflate)"),
            (SculptBrush::Smooth, "Smooth", "Even out bumps (or hold Ctrl with any brush)"),
            (SculptBrush::Flatten, "Flatten", "Press the area flat"),
            (SculptBrush::Grab, "Grab", "Pull a patch along with the pointer"),
        ] {
            ui.selectable_value(&mut s.brush, b, name).on_hover_text(tip);
        }
    });
    egui::Grid::new("sculpt").num_columns(2).show(ui, |ui| {
        ui.label("Size");
        ui.add(num(&mut s.radius, Kind::Length).speed(0.05).range(0.2..=500.0).suffix(" mm")).on_hover_text("Brush radius");
        ui.end_row();
        ui.label("Strength");
        ui.add(egui::Slider::new(&mut s.strength, 0.05..=1.0).max_decimals(2));
        ui.end_row();
    });
    ui.checkbox(&mut s.mirror_x, "Mirror left / right (X)").on_hover_text("Every stroke is repeated on the other side of the object");
    ui.checkbox(&mut s.detail, "Add detail under the brush")
        .on_hover_text("Splits big triangles as you sculpt, so even simple shapes deform smoothly");
    ui.weak("Drag on a model to sculpt; drag off it (or Alt+drag) to orbit. Shift reverses Draw / Inflate, Ctrl smooths. Ctrl+Z undoes a stroke.");
    if let Some(o) = app.sculpt_rt.hover.and_then(|h| app.scene.get(h.id)) {
        ui.weak(format!("{}: {} triangles", o.name, o.mesh.mesh.triangles.len()));
    }
    if ui.button(RichText::new("Done")).clicked() {
        app.tool = Tool::Select;
    }
    ui.separator();
}
