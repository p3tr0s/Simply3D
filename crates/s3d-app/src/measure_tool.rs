//! Measure tool: point distances, angles, flat faces (distance between parallel faces,
//! angle between faces), circles (hole / pin diameter), "scale so this measures …", and
//! a tube or channel between two measured points.

use crate::app::{App, MeasureMode, MeasuredCircle, MeasuredFace};
use crate::expr::Kind;
use crate::render::LineVertex;
use crate::ui::num;
use egui::{Color32, RichText};
use glam::{Quat, Vec3};
use s3d_core::measure::{angle_deg, circle_at, plane_at};
use s3d_core::scene::PickHit;
use s3d_core::{ObjectId, Transform};

const AXIS: [&str; 3] = ["X", "Y", "Z"];

impl App {
    /// A click with the measure tool. `p` is the (possibly vertex-snapped) point.
    pub fn measure_click(&mut self, hit: &PickHit, p: Vec3) {
        let m = &mut self.measure;
        match m.mode {
            MeasureMode::Points => {
                if m.a.is_none() || m.b.is_some() {
                    m.clear();
                    m.a = Some(p);
                } else {
                    m.b = Some(p);
                }
                m.objs.push(hit.id);
            }
            MeasureMode::Angle => {
                if m.pts.len() >= 3 {
                    m.clear();
                }
                m.pts.push(p);
                m.objs.push(hit.id);
            }
            MeasureMode::Faces => {
                let Some(o) = self.scene.get(hit.id) else { return };
                let mesh = &o.mesh.mesh;
                if hit.tri >= mesh.triangles.len() {
                    return;
                }
                let pl = plane_at(mesh, hit.tri);
                let xf = o.matrix();
                let mut normal = xf.inverse().transpose().transform_vector3(pl.normal).normalize_or_zero();
                if xf.determinant() < 0.0 {
                    normal = -normal;
                }
                let tris = pl.tris.iter().take(20_000).map(|&i| mesh.tri(i).map(|q| xf.transform_point3(q))).collect();
                let f = MeasuredFace { obj: hit.id, normal, point: xf.transform_point3(pl.point), tris };
                let m = &mut self.measure;
                if m.faces.len() >= 2 {
                    m.clear();
                }
                m.faces.push(f);
            }
            MeasureMode::Circle => {
                let Some(o) = self.scene.get(hit.id) else { return };
                let mesh = &o.mesh.mesh;
                match (hit.tri < mesh.triangles.len()).then(|| circle_at(mesh, hit.tri)).flatten() {
                    Some(c) => {
                        let xf = o.matrix();
                        let center = xf.transform_point3(c.center);
                        let u = c.axis.any_orthonormal_vector();
                        let radius = (xf.transform_point3(c.center + u * c.radius) - center).length();
                        let axis = xf.transform_vector3(c.axis).normalize_or(Vec3::Z);
                        self.measure.clear();
                        self.measure.circle = Some(MeasuredCircle { obj: hit.id, center, axis, radius });
                    }
                    None => self.error("No round surface there — click the inside of a hole or the side of a pin"),
                }
            }
        }
    }

    /// The current measurement, if it is taken on one object (so the object can be
    /// scaled to make it a given size): (object, value in mm, what it is).
    pub fn measured_value(&self) -> Option<(ObjectId, f32, &'static str)> {
        let m = &self.measure;
        let one = |ids: &[ObjectId]| ids.first().copied().filter(|f| ids.iter().all(|x| x == f));
        match m.mode {
            MeasureMode::Points => {
                let (a, b) = (m.a?, m.b?);
                Some((one(&m.objs)?, a.distance(b), "distance"))
            }
            MeasureMode::Faces => {
                let [f1, f2] = [m.faces.first()?, m.faces.get(1)?];
                (f1.obj == f2.obj && angle_deg(f1.normal, -f2.normal) < 0.5)
                    .then(|| (f1.obj, (f2.point - f1.point).dot(f1.normal).abs(), "distance"))
            }
            MeasureMode::Circle => m.circle.map(|c| (c.obj, c.radius * 2.0, "diameter")),
            MeasureMode::Angle => None,
        }
    }

    /// Scale the measured object uniformly so the measurement becomes `target`.
    pub fn scale_to_measure(&mut self) {
        let Some((id, v, what)) = self.measured_value() else { return };
        let target = self.measure.target;
        if v <= 1e-6 || target <= 0.0 {
            return;
        }
        let f = target / v;
        self.checkpoint("Scale to measurement");
        let on_plate = self.scene.get(id).is_some_and(|o| o.world_bounds().min.z.abs() < 0.01);
        if let Some(o) = self.scene.get_mut(id) {
            o.transform.scale *= f;
        }
        if on_plate {
            self.scene.drop_to_plate(id);
        }
        self.measure.clear();
        self.info(format!("Scaled ×{f:.4} so the {what} is {target:.2} mm"));
    }

    /// A tube (solid) or channel (hole object) between the two measured points.
    pub fn tube_between(&mut self, channel: bool) {
        let (Some(a), Some(b)) = (self.measure.a, self.measure.b) else { return };
        let len = a.distance(b);
        if len < 1e-3 {
            return;
        }
        let d = self.measure.tube_d.max(0.1);
        // a little longer than the points, so a channel opens cleanly through the surface
        let extra = if channel { d * 0.25 + 0.5 } else { 0.0 };
        let mesh = s3d_core::primitives::frustum(d * 0.5, d * 0.5, len + 2.0 * extra, 48);
        let dir = (b - a) / len;
        let t = Transform { translation: (a + b) * 0.5, rotation: Quat::from_rotation_arc(Vec3::Z, dir), scale: Vec3::ONE };
        self.checkpoint(if channel { "Add channel" } else { "Add tube" });
        let name = self.scene.unique_name(if channel { "Channel" } else { "Tube" });
        let id = self.scene.add_object(name, mesh, t, None);
        if channel {
            if let Some(o) = self.scene.get_mut(id) {
                o.hole = true;
            }
        }
        self.selection = vec![id];
        self.info(if channel {
            "Channel added as a hole: it cuts through whatever it passes"
        } else {
            "Tube added: Union it with the model to join them"
        });
    }

    /// 3D lines / translucent triangles for the measurement.
    pub fn measure_overlay(&self, tris: &mut Vec<LineVertex>, overlay: &mut Vec<LineVertex>) {
        let m = &self.measure;
        let gold = [1.0, 0.72, 0.03, 1.0];
        let line = |o: &mut Vec<LineVertex>, a: Vec3, b: Vec3, c: [f32; 4]| {
            o.push(LineVertex::new(a, c));
            o.push(LineVertex::new(b, c));
        };
        match m.mode {
            MeasureMode::Angle => {
                if m.pts.len() >= 2 {
                    line(overlay, m.pts[0], m.pts[1], gold);
                }
                if m.pts.len() >= 3 {
                    line(overlay, m.pts[1], m.pts[2], gold);
                }
            }
            MeasureMode::Faces => {
                for (k, f) in m.faces.iter().enumerate() {
                    let c = if k == 0 { [1.0, 0.6, 0.05, 0.45] } else { [0.15, 0.55, 1.0, 0.45] };
                    for t in &f.tris {
                        for p in t {
                            tris.push(LineVertex::new(*p + f.normal * 0.03, c));
                        }
                    }
                }
                if let [f1, f2] = &m.faces[..] {
                    if angle_deg(f1.normal, -f2.normal) < 0.5 {
                        // the gap, measured straight across from the first face
                        let q = f2.point - f1.normal * (f2.point - f1.point).dot(f1.normal);
                        line(overlay, q, f2.point, gold);
                    }
                }
            }
            MeasureMode::Circle => {
                if let Some(c) = m.circle {
                    let u = c.axis.any_orthonormal_vector();
                    let v = c.axis.cross(u);
                    let at = |k: usize| {
                        let a = k as f32 / 64.0 * std::f32::consts::TAU;
                        c.center + (u * a.cos() + v * a.sin()) * c.radius
                    };
                    for k in 0..64 {
                        line(overlay, at(k), at(k + 1), gold);
                    }
                    line(overlay, at(0), at(32), gold);
                    let s = c.radius * 0.15;
                    line(overlay, c.center - c.axis * c.radius * 0.6, c.center + c.axis * c.radius * 0.6, [1.0, 1.0, 1.0, 0.8]);
                    line(overlay, c.center - v * s, c.center + v * s, [1.0, 1.0, 1.0, 0.8]);
                }
            }
            MeasureMode::Points => {}
        }
    }

    /// Text labels for the measurement (screen space).
    pub fn measure_labels(&self) -> Vec<(Vec3, String)> {
        let m = &self.measure;
        let mut out = vec![];
        match m.mode {
            MeasureMode::Angle if m.pts.len() == 3 => {
                let a = angle_deg(m.pts[0] - m.pts[1], m.pts[2] - m.pts[1]);
                out.push((m.pts[1], format!("{a:.2}°")));
            }
            MeasureMode::Faces => {
                if let [f1, f2] = &m.faces[..] {
                    let mid = (f1.point + f2.point) * 0.5;
                    let t = if angle_deg(f1.normal, -f2.normal) < 0.5 {
                        format!("{:.3} mm", (f2.point - f1.point).dot(f1.normal).abs())
                    } else {
                        format!("{:.2}°", 180.0 - angle_deg(f1.normal, f2.normal))
                    };
                    out.push((mid, t));
                }
            }
            MeasureMode::Circle => {
                if let Some(c) = m.circle {
                    out.push((c.center, format!("Ø {:.3} mm", c.radius * 2.0)));
                }
            }
            _ => {}
        }
        out
    }
}

/// Right-hand panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Measure");
    let before = app.measure.mode;
    ui.horizontal(|ui| {
        let m = &mut app.measure.mode;
        ui.selectable_value(m, MeasureMode::Points, "Distance").on_hover_text("Two points");
        ui.selectable_value(m, MeasureMode::Angle, "Angle").on_hover_text("Three points: the angle is at the middle one");
        ui.selectable_value(m, MeasureMode::Faces, "Faces")
            .on_hover_text("Two flat faces: the distance between parallel faces, or the angle between them");
        ui.selectable_value(m, MeasureMode::Circle, "Hole / pin")
            .on_hover_text("Click the inside of a hole or the side of a pin: diameter and centre");
    });
    if app.measure.mode != before {
        app.measure.clear();
    }
    let m = &app.measure;
    match m.mode {
        MeasureMode::Points | MeasureMode::Angle => {
            ui.checkbox(&mut app.measure.snap_vertices, "Snap to corners");
        }
        _ => {}
    }
    let m = &app.measure;
    let strong = |ui: &mut egui::Ui, s: String| ui.label(RichText::new(s).strong());
    match m.mode {
        MeasureMode::Points => match (m.a, m.b) {
            (Some(a), Some(b)) => {
                let d = b - a;
                egui::Grid::new("meas").num_columns(2).show(ui, |ui| {
                    ui.label("Distance");
                    strong(ui, format!("{:.3} mm", d.length()));
                    ui.end_row();
                    for k in 0..3 {
                        ui.label(format!("Δ{}", AXIS[k]));
                        ui.label(format!("{:.3} mm", d[k].abs()));
                        ui.end_row();
                    }
                });
            }
            (Some(_), None) => {
                ui.label("Click the second point.");
            }
            _ => {
                ui.label("Click two points on the model.");
            }
        },
        MeasureMode::Angle => {
            if m.pts.len() == 3 {
                let a = angle_deg(m.pts[0] - m.pts[1], m.pts[2] - m.pts[1]);
                ui.horizontal(|ui| {
                    ui.label("Angle");
                    strong(ui, format!("{a:.2}°"));
                });
            } else {
                ui.label(format!("Click three points; the angle is at the second ({} of 3).", m.pts.len()));
            }
        }
        MeasureMode::Faces => match &m.faces[..] {
            [f1, f2] => {
                let between = angle_deg(f1.normal, f2.normal);
                if angle_deg(f1.normal, -f2.normal) < 0.5 {
                    ui.horizontal(|ui| {
                        ui.label("Parallel faces, distance");
                        strong(ui, format!("{:.3} mm", (f2.point - f1.point).dot(f1.normal).abs()));
                    });
                } else if between < 0.5 {
                    ui.horizontal(|ui| {
                        ui.label("Faces point the same way, offset");
                        strong(ui, format!("{:.3} mm", (f2.point - f1.point).dot(f1.normal).abs()));
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label("Angle between the faces");
                        strong(ui, format!("{:.2}°", 180.0 - between));
                    });
                    ui.weak(format!("(their directions differ by {between:.2}°)"));
                }
            }
            [_] => {
                ui.label("Click the second face.");
            }
            _ => {
                ui.label("Click two flat faces.");
            }
        },
        MeasureMode::Circle => match m.circle {
            Some(c) => {
                egui::Grid::new("circ").num_columns(2).show(ui, |ui| {
                    ui.label("Diameter");
                    strong(ui, format!("{:.3} mm", c.radius * 2.0));
                    ui.end_row();
                    ui.label("Radius");
                    ui.label(format!("{:.3} mm", c.radius));
                    ui.end_row();
                    ui.label("Centre");
                    ui.label(format!("{:.2}, {:.2}, {:.2}", c.center.x, c.center.y, c.center.z));
                    ui.end_row();
                    ui.label("Axis");
                    ui.label(format!("{:.3}, {:.3}, {:.3}", c.axis.x, c.axis.y, c.axis.z));
                    ui.end_row();
                });
            }
            None => {
                ui.label("Click the inside of a hole or the side of a pin.");
            }
        },
    }
    // scale the object so the measurement is a given size
    if let Some((_, v, what)) = app.measured_value() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(format!("Make the {what}"));
            ui.add(num(&mut app.measure.target, Kind::Length).speed(0.05).range(0.001..=100_000.0).suffix(" mm"));
            if ui
                .button("Scale")
                .on_hover_text(format!("Scales the object ×{:.4}, evenly in all directions", app.measure.target / v))
                .clicked()
            {
                app.scale_to_measure();
            }
        });
    } else if matches!(app.measure.mode, MeasureMode::Points) && app.measure.b.is_some() {
        ui.weak("Measure on one object to scale it to a size.");
    }
    // tube / channel between two points
    if app.measure.mode == MeasureMode::Points && app.measure.b.is_some() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Between the points:");
            ui.add(num(&mut app.measure.tube_d, Kind::Length).speed(0.05).range(0.1..=500.0).prefix("Ø ").suffix(" mm"));
        });
        ui.horizontal(|ui| {
            if ui.button("Add tube").on_hover_text("A solid rod from point to point").clicked() {
                app.tube_between(false);
            }
            if ui
                .button("Add channel")
                .on_hover_text("A hole object from point to point: cuts a channel (wiring, light pipes, drain paths)")
                .clicked()
            {
                app.tube_between(true);
            }
        });
    }
    if app.measure.a.is_some() || !app.measure.pts.is_empty() || !app.measure.faces.is_empty() || app.measure.circle.is_some() {
        if ui.button("Clear").clicked() {
            app.measure.clear();
        }
    }
    ui.weak("Esc leaves the tool.");
    ui.separator();
}

pub fn label_color() -> Color32 {
    Color32::from_rgb(255, 225, 90)
}
