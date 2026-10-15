//! 3D viewport: camera input, picking, gizmo, GPU frame and 2D overlays.

use crate::app::{App, GizmoKind, Tool};
use crate::camera::ViewPreset;
use crate::render::{CameraUniform, DrawItem, LineVertex, ViewportFrame};
use crate::ui::ACCENT;
use b3d_core::csg::BoolOp;
use b3d_core::Aabb;
use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Shape, Stroke};
use glam::{DQuat, DVec3, Vec2, Vec3};
use transform_gizmo_egui::prelude::*;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn lin(c: [u8; 4]) -> [f32; 4] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), c[3] as f32 / 255.0]
}
fn lin_rgba(r: f32, g: f32, b: f32, a: f32) -> [f32; 4] {
    [r, g, b, a]
}

struct View {
    rect: Rect,
    aspect: f32,
    cam: crate::camera::OrbitCamera,
}

impl View {
    fn ndc(&self, p: Pos2) -> Vec2 {
        Vec2::new((p.x - self.rect.left()) / self.rect.width() * 2.0 - 1.0, 1.0 - (p.y - self.rect.top()) / self.rect.height() * 2.0)
    }
    fn ray(&self, p: Pos2) -> (Vec3, Vec3) {
        self.cam.ray(self.ndc(p), self.aspect)
    }
    fn to_screen(&self, w: Vec3) -> Option<Pos2> {
        let n = self.cam.project(w, self.aspect)?;
        Some(Pos2::new(self.rect.left() + (n.x + 1.0) * 0.5 * self.rect.width(), self.rect.top() + (1.0 - n.y) * 0.5 * self.rect.height()))
    }
}

fn mint_v3(v: DVec3) -> mint::Vector3<f64> {
    mint::Vector3 { x: v.x, y: v.y, z: v.z }
}
fn mint_mat(m: glam::DMat4) -> mint::RowMatrix4<f64> {
    let r = |i| {
        let v = m.row(i);
        mint::Vector4 { x: v.x, y: v.y, z: v.z, w: v.w }
    };
    mint::RowMatrix4 { x: r(0), y: r(1), z: r(2), w: r(3) }
}

fn plate_hit(o: Vec3, d: Vec3) -> Option<Vec3> {
    if d.z.abs() < 1e-6 {
        return None;
    }
    let t = -o.z / d.z;
    (t > 0.0).then(|| o + d * t)
}

pub fn viewport(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    app.viewport_aspect = rect.width() / rect.height().max(1.0);
    if app.fit_on_next_frame {
        app.frame_all();
        app.fit_on_next_frame = false;
    }
    let painter = ui.painter_at(rect);
    background(&painter, rect, app.settings.dark);
    // reserve the 3D slot so the gizmo and overlays are painted on top of it
    let slot = painter.add(Shape::Noop);

    let view = View { rect, aspect: app.viewport_aspect, cam: app.settings.camera };
    let pointer = response.hover_pos();
    let ray = pointer.map(|p| view.ray(p));
    app.cursor_plate = ray.and_then(|(o, d)| plate_hit(o, d));

    // ---------- gizmo ----------
    let gizmo_focused = run_gizmo(app, ui, &view);
    let gizmo_busy = gizmo_focused || app.gizmo_dragging;

    // ---------- hover ----------
    let moved = pointer != app.last_pointer;
    app.last_pointer = pointer;
    let total_tris: usize = app.scene.objects.iter().map(|o| o.mesh.mesh.triangles.len()).sum();
    if moved && !response.dragged() && total_tris < 1_500_000 {
        app.hovered = ray.and_then(|(o, d)| app.scene.pick(o, d)).map(|h| h.0);
    }
    if pointer.is_none() {
        app.hovered = None;
    }
    if matches!(app.tool, Tool::Measure | Tool::LayFlat) && response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }

    // ---------- camera ----------
    let shift = ui.input(|i| i.modifiers.shift);
    if response.dragged_by(egui::PointerButton::Primary) && !gizmo_busy {
        let d = response.drag_delta();
        if shift {
            app.settings.camera.pan(Vec2::new(d.x, d.y), rect.height());
        } else {
            app.settings.camera.orbit(Vec2::new(d.x, d.y));
        }
    }
    if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
        let d = response.drag_delta();
        app.settings.camera.pan(Vec2::new(d.x, d.y), rect.height());
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta().y);
        if scroll.abs() > 0.0 {
            let toward = ray.and_then(|(o, d)| app.scene.pick(o, d).map(|h| h.2).or_else(|| plate_hit(o, d)));
            app.settings.camera.zoom(scroll, toward);
        }
    }

    // ---------- clicks ----------
    let cmd = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if response.clicked() && !gizmo_busy {
        if let Some((o, d)) = ray {
            match app.tool {
                Tool::Select | Tool::Cut => match app.scene.pick(o, d) {
                    Some((id, _, _)) => {
                        if cmd {
                            app.toggle_select(id)
                        } else {
                            app.select_only(id)
                        }
                    }
                    None if !cmd => app.selection.clear(),
                    None => {}
                },
                Tool::Measure => {
                    if let Some(hit) = app.scene.pick_detail(o, d) {
                        let mut p = hit.point;
                        if app.measure.snap_vertices {
                            if let Some(c) = hit.corners.iter().min_by(|a, b| a.distance(p).total_cmp(&b.distance(p))) {
                                if let (Some(sc), Some(sp)) = (view.to_screen(*c), view.to_screen(p)) {
                                    if sc.distance(sp) < 12.0 {
                                        p = *c;
                                    }
                                }
                            }
                        }
                        if app.measure.a.is_none() || app.measure.b.is_some() {
                            app.measure.a = Some(p);
                            app.measure.b = None;
                        } else {
                            app.measure.b = Some(p);
                        }
                    }
                }
                Tool::LayFlat => {
                    if let Some(hit) = app.scene.pick_detail(o, d) {
                        app.lay_flat(hit.id, hit.normal);
                    }
                }
            }
        }
    }
    if response.double_clicked() {
        if let Some((o, d)) = ray {
            if let Some((_, _, p)) = app.scene.pick(o, d) {
                app.settings.camera.target = p;
            }
        }
    }
    if response.secondary_clicked() {
        if let Some((o, d)) = ray {
            if let Some((id, _, _)) = app.scene.pick(o, d) {
                if !app.is_selected(id) {
                    app.select_only(id);
                }
            }
        }
    }
    let ctx = ui.ctx().clone();
    response.context_menu(|ui| context_menu(app, ui, &ctx));

    // ---------- GPU frame ----------
    let view = View { rect, aspect: app.viewport_aspect, cam: app.settings.camera };
    if app.viewport_ok {
        let frame = build_frame(app, &view);
        painter.set(slot, egui_wgpu::Callback::new_paint_callback(rect, frame));
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "GPU renderer unavailable",
            egui::FontId::proportional(18.0),
            Color32::RED,
        );
    }

    // ---------- overlays ----------
    overlays(app, ui, &painter, &view);
}

fn background(p: &egui::Painter, r: Rect, dark: bool) {
    let (top, bot) = if dark {
        (Color32::from_rgb(52, 58, 68), Color32::from_rgb(24, 26, 30))
    } else {
        (Color32::from_rgb(236, 240, 246), Color32::from_rgb(196, 204, 216))
    };
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bot);
    m.colored_vertex(r.left_bottom(), bot);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

fn run_gizmo(app: &mut App, ui: &mut egui::Ui, view: &View) -> bool {
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    if !pointer_down {
        app.gizmo_dragging = false;
    }
    if app.tool != Tool::Select || app.gizmo_kind == GizmoKind::Off || app.selection.is_empty() {
        return false;
    }
    let modes = match app.gizmo_kind {
        GizmoKind::Move => GizmoMode::all_translate(),
        GizmoKind::Rotate => GizmoMode::all_rotate(),
        GizmoKind::Scale => GizmoMode::all_scale(),
        GizmoKind::Off => return false,
    };
    let ctrl = ui.input(|i| i.modifiers.command);
    let cam = &view.cam;
    app.gizmo.update_config(GizmoConfig {
        view_matrix: mint_mat(cam.view().as_dmat4()),
        projection_matrix: mint_mat(cam.proj(view.aspect).as_dmat4()),
        viewport: view.rect,
        modes,
        orientation: if app.gizmo_kind == GizmoKind::Scale { GizmoOrientation::Local } else { GizmoOrientation::Global },
        snapping: app.settings.snap_enabled ^ ctrl,
        snap_distance: app.settings.snap_move,
        snap_angle: app.settings.snap_angle_deg.to_radians(),
        snap_scale: app.settings.snap_scale,
        pixels_per_point: ui.ctx().pixels_per_point(),
        ..Default::default()
    });
    let targets: Vec<transform_gizmo_egui::math::Transform> = app
        .selection
        .iter()
        .filter_map(|id| app.scene.get(*id))
        .map(|o| {
            let q = o.transform.rotation.as_dquat();
            transform_gizmo_egui::math::Transform::from_scale_rotation_translation(
                mint_v3(o.transform.scale.as_dvec3()),
                mint::Quaternion { v: mint::Vector3 { x: q.x, y: q.y, z: q.z }, s: q.w },
                mint_v3(o.transform.translation.as_dvec3()),
            )
        })
        .collect();
    if let Some((result, new)) = app.gizmo.interact(ui, &targets) {
        if !app.gizmo_dragging {
            let label = match result {
                GizmoResult::Translation { .. } => "Move",
                GizmoResult::Scale { .. } => "Scale",
                _ => "Rotate",
            };
            app.checkpoint(label);
            app.gizmo_dragging = true;
        }
        let ids: Vec<_> = app.selection.clone();
        for (id, t) in ids.into_iter().zip(new) {
            if let Some(o) = app.scene.get_mut(id) {
                let (tr, ro, sc) = (t.translation, t.rotation, t.scale);
                o.transform.translation = DVec3::new(tr.x, tr.y, tr.z).as_vec3();
                o.transform.rotation = DQuat::from_xyzw(ro.v.x, ro.v.y, ro.v.z, ro.s).as_quat().normalize();
                o.transform.scale = DVec3::new(sc.x, sc.y, sc.z).as_vec3().max(Vec3::splat(1e-4));
            }
        }
        app.report_cache = None;
    }
    app.gizmo.is_focused() || app.gizmo_dragging
}

fn build_frame(app: &App, view: &View) -> ViewportFrame {
    let cam = &view.cam;
    let (r, u) = cam.right_up();
    let light = (-cam.forward() + u * 0.8 + r * 0.35).normalize();
    let eye = cam.eye();
    let camera = CameraUniform {
        view_proj: cam.view_proj(view.aspect).to_cols_array_2d(),
        eye: [eye.x, eye.y, eye.z, 1.0],
        light: [light.x, light.y, light.z, if app.encode_srgb { 1.0 } else { 0.0 }],
    };
    let items = app
        .scene
        .objects
        .iter()
        .filter(|o| o.visible)
        .map(|o| {
            let m = o.matrix();
            DrawItem {
                mesh: o.mesh.clone(),
                model: m,
                color: lin(o.color),
                params: [
                    app.is_selected(o.id) as u8 as f32,
                    (app.hovered == Some(o.id)) as u8 as f32,
                    (m.determinant() < 0.0) as u8 as f32,
                    app.settings.show_problems as u8 as f32,
                ],
            }
        })
        .collect();

    let dark = app.settings.dark;
    let mut tris = vec![];
    let mut lines = vec![];
    let mut overlay = vec![];
    if app.settings.show_plate {
        let h = app.settings.plate * 0.5;
        let pc = if dark { lin_rgba(0.35, 0.42, 0.55, 0.22) } else { lin_rgba(0.3, 0.38, 0.5, 0.12) };
        let c = [Vec3::new(-h.x, -h.y, 0.0), Vec3::new(h.x, -h.y, 0.0), Vec3::new(h.x, h.y, 0.0), Vec3::new(-h.x, h.y, 0.0)];
        for i in [0, 1, 2, 0, 2, 3] {
            tris.push(LineVertex::new(c[i], pc));
        }
        let step = app.settings.grid_step.max(0.5);
        let minor = if dark { lin_rgba(0.6, 0.65, 0.75, 0.10) } else { lin_rgba(0.1, 0.12, 0.2, 0.10) };
        let major = if dark { lin_rgba(0.7, 0.75, 0.85, 0.28) } else { lin_rgba(0.1, 0.12, 0.2, 0.25) };
        let mut grid = |axis_x: bool, half_len: f32, half_other: f32| {
            let n = (half_other / step).floor() as i32;
            for i in -n..=n {
                let v = i as f32 * step;
                let col = if i % 5 == 0 { major } else { minor };
                let (a, b) = if axis_x {
                    (Vec3::new(v, -half_len, 0.0), Vec3::new(v, half_len, 0.0))
                } else {
                    (Vec3::new(-half_len, v, 0.0), Vec3::new(half_len, v, 0.0))
                };
                lines.push(LineVertex::new(a, col));
                lines.push(LineVertex::new(b, col));
            }
        };
        grid(true, h.y, h.x);
        grid(false, h.x, h.y);
        let border = if dark { lin_rgba(0.8, 0.85, 0.95, 0.6) } else { lin_rgba(0.2, 0.25, 0.35, 0.6) };
        for i in 0..4 {
            lines.push(LineVertex::new(c[i], border));
            lines.push(LineVertex::new(c[(i + 1) % 4], border));
        }
        let ax = h.x.min(h.y) * 0.25;
        lines.push(LineVertex::new(Vec3::ZERO, lin_rgba(0.9, 0.2, 0.2, 0.9)));
        lines.push(LineVertex::new(Vec3::X * ax, lin_rgba(0.9, 0.2, 0.2, 0.9)));
        lines.push(LineVertex::new(Vec3::ZERO, lin_rgba(0.2, 0.8, 0.2, 0.9)));
        lines.push(LineVertex::new(Vec3::Y * ax, lin_rgba(0.2, 0.8, 0.2, 0.9)));
    }

    // selection bounding box
    if !app.selection.is_empty() && app.settings.show_dimensions {
        let b = app.selection_bounds();
        push_box(&mut overlay, &b, lin_rgba(0.22, 0.52, 1.0, 0.55));
    }

    // cut plane preview
    if app.tool == crate::app::Tool::Cut && !app.selection.is_empty() {
        let b = app.selection_bounds();
        let pad = b.size().max_element() * 0.15 + 5.0;
        let a = app.cut.axis;
        let (i, j) = ((a + 1) % 3, (a + 2) % 3);
        let corner = |si: f32, sj: f32| {
            let mut p = Vec3::ZERO;
            p[a] = app.cut.offset;
            p[i] = if si < 0.0 { b.min[i] - pad } else { b.max[i] + pad };
            p[j] = if sj < 0.0 { b.min[j] - pad } else { b.max[j] + pad };
            p
        };
        let q = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        let fill = lin_rgba(1.0, 0.35, 0.2, 0.28);
        for k in [0, 1, 2, 0, 2, 3] {
            tris.push(LineVertex::new(q[k], fill));
        }
        for k in 0..4 {
            overlay.push(LineVertex::new(q[k], lin_rgba(1.0, 0.45, 0.25, 1.0)));
            overlay.push(LineVertex::new(q[(k + 1) % 4], lin_rgba(1.0, 0.45, 0.25, 1.0)));
        }
    }

    // measurement
    if let (Some(a), Some(b)) = (app.measure.a, app.measure.b) {
        let c = lin_rgba(1.0, 0.85, 0.1, 1.0);
        overlay.push(LineVertex::new(a, c));
        overlay.push(LineVertex::new(b, c));
        // axis components
        let p1 = Vec3::new(b.x, a.y, a.z);
        let p2 = Vec3::new(b.x, b.y, a.z);
        for (s, e, col) in
            [(a, p1, lin_rgba(0.9, 0.3, 0.3, 0.7)), (p1, p2, lin_rgba(0.3, 0.8, 0.3, 0.7)), (p2, b, lin_rgba(0.3, 0.5, 1.0, 0.7))]
        {
            overlay.push(LineVertex::new(s, col));
            overlay.push(LineVertex::new(e, col));
        }
    }
    ViewportFrame { camera, items, tris, lines, overlay }
}

fn push_box(out: &mut Vec<LineVertex>, b: &Aabb, c: [f32; 4]) {
    let k = b.corners();
    for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
        out.push(LineVertex::new(k[i], c));
        out.push(LineVertex::new(k[j], c));
    }
}

/// Bottom corner index adjacent to `i` along `axis` (corners 0..4 are the bottom face).
fn near_of(k: &[Vec3; 8], i: usize, axis: usize) -> usize {
    (0..4).find(|&j| j != i && (0..3).all(|a| (a == axis) != (k[j][a] == k[i][a]))).unwrap_or(i)
}

fn label(p: &egui::Painter, at: Pos2, text: String, color: Color32) {
    let galley = p.layout_no_wrap(text, egui::FontId::proportional(13.0), color);
    let r = Rect::from_center_size(at, galley.size()).expand2(egui::vec2(5.0, 2.0));
    p.rect_filled(r, 4.0, Color32::from_black_alpha(170));
    p.galley(r.min + egui::vec2(5.0, 2.0), galley, color);
}

fn overlays(app: &mut App, ui: &mut egui::Ui, p: &egui::Painter, view: &View) {
    let rect = view.rect;
    // dimension labels
    if app.settings.show_dimensions && !app.selection.is_empty() && !app.gizmo_dragging {
        let b = app.selection_bounds();
        let s = b.size();
        let dec = app.settings.decimals;
        let mid = |a: Vec3, c: Vec3| (a + c) * 0.5;
        let k = b.corners();
        let cols = [Color32::from_rgb(255, 120, 120), Color32::from_rgb(130, 230, 130), Color32::from_rgb(130, 170, 255)];
        let center = view.to_screen(b.center());
        let screen_pts: Vec<Pos2> = k.iter().filter_map(|c| view.to_screen(*c)).collect();
        let screen_box = Rect::from_points(&screen_pts);
        if screen_box.width().max(screen_box.height()) < 260.0 || screen_pts.len() < 8 {
            // small on screen: one combined label above the box
            let at = Pos2::new(screen_box.center().x, screen_box.top() - 14.0);
            if rect.contains(at) {
                let t = format!("{:.*} × {:.*} × {:.*} mm", dec, s.x, dec, s.y, dec, s.z);
                label(p, at, t, Color32::WHITE);
            }
        } else {
            // pick the edges facing the camera: nearest bottom corner
            let eye = view.cam.eye();
            let near = (0..4).min_by(|i, j| k[*i].distance(eye).total_cmp(&k[*j].distance(eye))).unwrap();
            let x_nb = near_of(&k, near, 0);
            let y_nb = near_of(&k, near, 1);
            for (axis, (a, c)) in [(0, (k[near], k[x_nb])), (1, (k[near], k[y_nb])), (2, (k[near], k[near + 4]))] {
                if let (Some(sp), Some(cp)) = (view.to_screen(mid(a, c)), center) {
                    // push the label outward from the box centre so it doesn't cover the object
                    let at = sp + (sp - cp).normalized() * 22.0;
                    if rect.contains(at) {
                        label(p, at, format!("{:.*} mm", dec, s[axis]), cols[axis]);
                    }
                }
            }
        }
    }
    // measurement
    if let Some(a) = app.measure.a {
        if let Some(sa) = view.to_screen(a) {
            p.circle_filled(sa, 4.5, Color32::from_rgb(255, 215, 30));
        }
        if let Some(b) = app.measure.b {
            if let Some(sb) = view.to_screen(b) {
                p.circle_filled(sb, 4.5, Color32::from_rgb(255, 215, 30));
            }
            if let Some(sm) = view.to_screen((a + b) * 0.5) {
                label(p, sm + egui::vec2(0.0, -14.0), format!("{:.3} mm", a.distance(b)), Color32::from_rgb(255, 225, 90));
            }
        }
    }
    // empty-scene hint
    if app.scene.objects.is_empty() {
        let c = if app.settings.dark { Color32::from_white_alpha(140) } else { Color32::from_black_alpha(140) };
        p.text(
            rect.center_top() + egui::vec2(0.0, 40.0),
            egui::Align2::CENTER_CENTER,
            "Drop STL / OBJ / PLY / 3MF files here  ·  Ctrl+O to open  ·  Add shape from the toolbar",
            egui::FontId::proportional(15.0),
            c,
        );
    }
    // tool hint
    let hint = match app.tool {
        Tool::Measure => Some("Measure: click two points"),
        Tool::LayFlat => Some("Lay flat: click the face that should touch the plate"),
        Tool::Cut => Some("Cut: set the plane in the right panel"),
        Tool::Select => None,
    };
    if let Some(h) = hint {
        label(p, rect.center_top() + egui::vec2(0.0, 18.0), h.to_string(), Color32::WHITE);
    }
    axis_triad(app, ui, p, view);
}

fn axis_triad(app: &mut App, ui: &mut egui::Ui, p: &egui::Painter, view: &View) {
    let center = Pos2::new(view.rect.left() + 55.0, view.rect.bottom() - 55.0);
    let v = view.cam.view();
    let mut axes: Vec<(usize, Vec3)> = (0..3).map(|a| (a, v.transform_vector3(Vec3::AXES[a]))).collect();
    axes.sort_by(|a, b| a.1.z.total_cmp(&b.1.z)); // draw far axes first
    p.circle_filled(center, 42.0, Color32::from_black_alpha(60));
    let cols = [Color32::from_rgb(235, 80, 80), Color32::from_rgb(90, 200, 90), Color32::from_rgb(80, 140, 255)];
    for (a, d) in axes {
        let end = center + egui::vec2(d.x, -d.y) * 34.0;
        p.line_segment([center, end], Stroke::new(2.5, cols[a]));
        let r = Rect::from_center_size(end, egui::vec2(18.0, 18.0));
        let resp = ui.interact(r, ui.id().with(("triad", a)), Sense::click());
        p.circle_filled(end, if resp.hovered() { 10.0 } else { 8.5 }, cols[a]);
        p.text(end, egui::Align2::CENTER_CENTER, ["X", "Y", "Z"][a], egui::FontId::proportional(11.0), Color32::BLACK);
        if resp.clicked() {
            app.settings.camera.set_preset([ViewPreset::Right, ViewPreset::Back, ViewPreset::Top][a]);
        }
    }
    let _ = ACCENT;
}

fn context_menu(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let has = !app.selection.is_empty();
    let many = app.selection.len() >= 2;
    if ui.add_enabled(has, egui::Button::new("Duplicate")).clicked() {
        app.duplicate_selected();
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new("Delete")).clicked() {
        app.delete_selected();
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new("Drop to plate")).clicked() {
        app.drop_selected();
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new("Frame")).clicked() {
        app.frame_selection();
        ui.close();
    }
    ui.separator();
    for (n, op) in [("Union", BoolOp::Union), ("Subtract", BoolOp::Subtract), ("Intersect", BoolOp::Intersect)] {
        if ui.add_enabled(many, egui::Button::new(n)).clicked() {
            app.boolean_selected(ctx, op);
            ui.close();
        }
    }
    if ui.add_enabled(has, egui::Button::new("Repair")).clicked() {
        app.repair_selected(ctx);
        ui.close();
    }
    ui.separator();
    if ui.button("Select all").clicked() {
        app.select_all();
        ui.close();
    }
}
