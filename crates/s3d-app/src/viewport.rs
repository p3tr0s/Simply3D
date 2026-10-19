//! 3D viewport: camera input, picking, gizmo, GPU frame and 2D overlays.

use crate::app::{App, DragMode, GizmoKind, LeftDrag, Tool};
use crate::camera::ViewPreset;
use crate::preview::PreviewKind;
use crate::render::{CameraUniform, DrawItem, LineVertex, ViewportFrame};
use crate::text_tool::{TextHover, TextMode};
use crate::ui::ACCENT;
use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Shape, Stroke};
use glam::{DQuat, DVec3, Vec2, Vec3};
use s3d_core::scene::PickHit;
use s3d_core::Aabb;
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
    /// World units per screen point at world position `p`.
    fn world_per_px(&self, p: Vec3) -> f32 {
        let c = &self.cam;
        let depth = if c.ortho { c.distance } else { (p - c.eye()).dot(c.forward()).max(0.01) };
        2.0 * depth * (c.fov_y * 0.5).tan() / self.rect.height().max(1.0)
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

/// Pick what is visible: respects the section plane; locked objects only if asked.
/// Clicks go through see-through objects to a selected object inside them.
fn pick(app: &App, o: Vec3, d: Vec3, include_locked: bool) -> Option<PickHit> {
    let sec = app.section;
    // holes are drawn see-through and usually sit inside solids: they win when hit
    let hole = app.scene.pick_filtered(o, d, |ob| ob.hole && (include_locked || !ob.locked), |p| sec.keeps(p));
    if hole.is_some() {
        return hole;
    }
    let hit = app.scene.pick_filtered(o, d, |ob| include_locked || !ob.locked, |p| sec.keeps(p));
    match hit {
        Some(h) if app.see_through.contains(&h.id) => {
            app.scene.pick_filtered(o, d, |ob| app.selection.contains(&ob.id) && (include_locked || !ob.locked), |p| sec.keeps(p)).or(hit)
        }
        _ => hit,
    }
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
    app.update_see_through();
    let pointer = response.hover_pos();
    let ray = pointer.map(|p| view.ray(p));
    app.cursor_plate = ray.and_then(|(o, d)| plate_hit(o, d));

    // ---------- gizmo ----------
    app.snap_guides.clear();
    if !matches!(app.drag, DragMode::Move { .. }) {
        app.drag_label = None;
    }
    let gizmo_focused = run_gizmo(app, ui, &view, response.hovered());
    let gizmo_busy = gizmo_focused || app.gizmo_dragging;

    // ---------- hover ----------
    let moved = pointer != app.last_pointer;
    app.last_pointer = pointer;
    let total_tris: usize = app.scene.objects.iter().map(|o| o.mesh.mesh.triangles.len()).sum();
    if moved && !response.dragged() {
        app.hovered = ray.and_then(|(o, d)| pick(app, o, d, false)).map(|h| h.id);
    }
    if pointer.is_none() {
        app.hovered = None;
    }
    // ---------- text tool: where would the text go? ----------
    if app.tool == Tool::Text {
        if moved || app.text_rt.hover.is_none() {
            app.text_rt.hover = ray.and_then(|(o, d)| match app.settings.text.mode {
                // a new object: lying on a face of another object, or on the plate
                TextMode::Object => pick(app, o, d, false)
                    .map(|h| TextHover {
                        target: None,
                        point: h.point,
                        frame: app.surface_frame(h.point, h.normal, view.cam.right_up().1),
                        bend: s3d_core::wrap::Bend::FLAT,
                    })
                    .or_else(|| plate_hit(o, d).map(TextHover::on_plate)),
                _ => pick(app, o, d, false).map(|h| {
                    let frame = app.surface_frame(h.point, h.normal, view.cam.right_up().1);
                    let bend = match app.text_base() {
                        Some(b) if app.settings.text.wrap => app.fit_bend(h.id, frame, App::shape_half(&b.mesh)),
                        _ => s3d_core::wrap::Bend::FLAT,
                    };
                    TextHover { target: Some(h.id), point: h.point, frame, bend }
                }),
            });
        }
        if pointer.is_none() {
            app.text_rt.hover = None;
        }
    }
    crate::debug::frame(app, ui, crate::debug::Probe { response: &response, gizmo_focused: gizmo_busy, moved, total_tris });
    // ---------- picture tool: same placement as text ----------
    if app.tool == Tool::Image {
        if moved || app.img_rt.hover.is_none() {
            app.img_rt.hover = ray.and_then(|(o, d)| match app.settings.image.place {
                TextMode::Object => pick(app, o, d, false)
                    .map(|h| TextHover {
                        target: None,
                        point: h.point,
                        frame: app.image_surface_frame(h.point, h.normal, view.cam.right_up().1),
                        bend: s3d_core::wrap::Bend::FLAT,
                    })
                    .or_else(|| plate_hit(o, d).map(TextHover::on_plate)),
                _ => pick(app, o, d, false).map(|h| {
                    let frame = app.image_surface_frame(h.point, h.normal, view.cam.right_up().1);
                    let bend = match &app.img_rt.unit {
                        Some(u) if app.settings.image.wrap => app.fit_bend(h.id, frame, App::shape_half(&u.mesh)),
                        _ => s3d_core::wrap::Bend::FLAT,
                    };
                    TextHover { target: Some(h.id), point: h.point, frame, bend }
                }),
            });
        }
        if pointer.is_none() {
            app.img_rt.hover = None;
        }
    }
    app.update_wrap_ghost();
    // ---------- placing a new shape: on a face or the plate ----------
    if app.placing.is_some() {
        if moved || app.place_hover.is_none() {
            app.place_hover = ray
                .and_then(|(o, d)| pick(app, o, d, false).map(|h| (h.point, h.normal)).or_else(|| plate_hit(o, d).map(|p| (p, Vec3::Z))));
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
    }
    // ---------- paint tool: what's under the brush ----------
    if app.tool == Tool::Paint {
        if moved || app.paint_rt.hover.is_none() {
            app.paint_rt.hover = ray.and_then(|(o, d)| pick(app, o, d, false)).map(|h| crate::paint_tool::PaintHover {
                id: h.id,
                point: h.point,
                normal: h.normal,
                tri: h.tri,
            });
        }
        if let Some(h) = app.paint_rt.hover {
            app.paint_rt.last_obj = Some(h.id);
        }
        if pointer.is_none() {
            app.paint_rt.hover = None;
        }
    }
    // ---------- sculpt: what's under the brush ----------
    if app.tool == Tool::Sculpt {
        if moved || app.sculpt_rt.hover.is_none() {
            // the object being sculpted is picked on its live copy (the scene may still
            // hold the mesh from before the current stroke)
            let sec = app.section;
            app.sculpt_rt.hover = ray.and_then(|(o, d)| {
                let target = app.sculpt_target_id();
                let other = pick(app, o, d, false)
                    .filter(|h| Some(h.id) != target)
                    .map(|h| (h.distance, crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri }));
                let live = app.sculpt_hit(o, d, |p| sec.keeps(p));
                match (other, live) {
                    (Some(a), Some(b)) => Some(if a.0 < b.0 { a.1 } else { b.1 }),
                    (a, b) => a.or(b).map(|x| x.1),
                }
            });
        }
        if pointer.is_none() {
            app.sculpt_rt.hover = None;
        }
    }
    // ---------- select faces: what's under the brush ----------
    if matches!(app.tool, Tool::Faces | Tool::Texture) {
        if moved || app.face_rt.hover.is_none() {
            app.face_rt.hover = ray.and_then(|(o, d)| pick(app, o, d, false)).map(|h| crate::paint_tool::PaintHover {
                id: h.id,
                point: h.point,
                normal: h.normal,
                tri: h.tri,
            });
        }
        if pointer.is_none() {
            app.face_rt.hover = None;
        }
    }
    if matches!(
        app.tool,
        Tool::Measure | Tool::LayFlat | Tool::Text | Tool::Image | Tool::Paint | Tool::Faces | Tool::Texture | Tool::Sculpt
    ) && response.hovered()
    {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    } else if matches!(app.drag, DragMode::Move { .. }) {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if gizmo_busy && response.hovered() {
        // over a gizmo handle: dragging moves/rotates/scales with the gizmo
        ui.ctx().set_cursor_icon(CursorIcon::Move);
    } else if app.tool == Tool::Select && app.settings.drag_moves_objects && app.hovered.is_some() && response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }

    // ---------- drags: orbit / pan / move objects / box select ----------
    let (shift, alt, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.modifiers.command));
    let cad = app.settings.left_drag == LeftDrag::BoxSelect;
    if response.drag_started_by(egui::PointerButton::Primary) && !gizmo_busy {
        let origin = ui.input(|i| i.pointer.press_origin()).or(pointer).unwrap_or(rect.center());
        app.drag = start_primary_drag(app, &view, origin, shift, alt, ctrl, cad);
    }
    if response.drag_started_by(egui::PointerButton::Secondary) {
        app.drag = if cad { DragMode::Orbit } else { DragMode::Pan };
    }
    if response.drag_started_by(egui::PointerButton::Middle) {
        app.drag = DragMode::Pan;
    }
    if response.dragged() {
        let d = response.drag_delta();
        match &app.drag {
            DragMode::Orbit => app.settings.camera.orbit(Vec2::new(d.x, d.y)),
            DragMode::Pan => app.settings.camera.pan(Vec2::new(d.x, d.y), rect.height()),
            DragMode::Move { .. } => {
                // once a distance is typed, the mouse no longer moves the objects
                if app.move_typed.is_empty() {
                    if let Some(p) = pointer.or(ui.input(|i| i.pointer.latest_pos())) {
                        update_move(app, &view, p, shift, ctrl);
                    }
                }
            }
            DragMode::Paint => {
                if let (true, Some(h)) = (moved, app.paint_rt.hover) {
                    app.paint_at(h, false);
                }
            }
            DragMode::Sculpt => {
                if moved {
                    let h = app.sculpt_rt.hover;
                    app.sculpt_drag(h, ray);
                }
            }
            DragMode::FaceSel { remove } => {
                if let (true, Some(h)) = (moved, app.face_rt.hover) {
                    let remove = *remove;
                    app.face_select_at(h, false, remove);
                }
            }
            _ => {}
        }
    }
    if response.drag_stopped() {
        if matches!(app.drag, DragMode::Sculpt) {
            app.sculpt_end();
        }
        if matches!(app.drag, DragMode::FaceSel { .. }) {
            app.face_rt.hover = None;
        }
        if matches!(app.drag, DragMode::Paint) {
            app.end_paint_stroke();
        }
        if let DragMode::Box { start, additive } = app.drag.clone() {
            if let Some(end) = ui.input(|i| i.pointer.latest_pos()) {
                finish_box_select(app, &view, start, end, additive);
            }
        }
        if matches!(app.drag, DragMode::Move { .. }) {
            if let Some(l) = app.drag_label.take() {
                app.info(format!("Moved {l}"));
            }
        }
        app.drag = DragMode::None;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta().y);
        if scroll.abs() > 0.0 {
            let toward = ray.and_then(|(o, d)| pick(app, o, d, true).map(|h| h.point).or_else(|| plate_hit(o, d)));
            app.settings.camera.zoom(scroll, toward);
        }
    }

    // ---------- clicks ----------
    let cmd = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if response.clicked() && !app.gizmo_used && app.placing.is_some() {
        let at = app.place_hover;
        app.commit_placing(match at {
            Some((p, n)) => crate::app::PlaceAt::Surface(p, n),
            None => crate::app::PlaceAt::Beside,
        });
    } else if response.clicked() && !app.gizmo_used {
        if let Some((o, d)) = ray {
            match app.tool {
                Tool::Select | Tool::Cut => match pick(app, o, d, false) {
                    Some(h) => app.click_select(h.id, cmd, alt),
                    None if !cmd => app.selection.clear(),
                    None => {}
                },
                Tool::Image => {
                    let ctx = ui.ctx().clone();
                    match (app.settings.image.place, app.img_rt.hover) {
                        (TextMode::Object, h) => {
                            if app.img_rt.mesh.is_some() {
                                app.add_image_object(&ctx, h);
                                placed_one(app, shift);
                            }
                        }
                        (_, Some(h)) => {
                            app.apply_image_on_surface(&ctx, h);
                            placed_one(app, shift);
                        }
                        _ => {}
                    }
                }
                Tool::Sculpt => {
                    if let Some(h) = pick(app, o, d, false) {
                        let h = crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri };
                        app.sculpt_begin(h, d, shift, ctrl);
                        app.sculpt_end();
                    }
                }
                Tool::Faces | Tool::Texture => {
                    if let Some(h) = pick(app, o, d, false) {
                        let h = crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri };
                        app.face_select_at(h, true, shift);
                    }
                }
                Tool::Paint => {
                    if let Some(h) = pick(app, o, d, false) {
                        let h = crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri };
                        if ctrl {
                            app.pick_paint_color(h);
                        } else {
                            app.begin_paint_stroke();
                            app.paint_at(h, true);
                            app.end_paint_stroke();
                        }
                    }
                }
                Tool::Text => {
                    let ctx = ui.ctx().clone();
                    match (app.settings.text.mode, app.text_rt.hover) {
                        (TextMode::Object, h) => {
                            app.add_text_object(h);
                            placed_one(app, shift);
                        }
                        (_, Some(h)) => {
                            app.apply_text_on_surface(&ctx, h);
                            placed_one(app, shift);
                        }
                        _ => {}
                    }
                }
                Tool::Measure => {
                    if let Some(hit) = pick(app, o, d, true) {
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
                        app.measure_click(&hit, p);
                    }
                }
                Tool::LayFlat => {
                    if let Some(hit) = pick(app, o, d, false) {
                        app.lay_flat(hit.id, hit.normal);
                        // done after one, like 3D Builder; Shift+click keeps it on for more
                        if !response.ctx.input(|i| i.modifiers.shift) {
                            app.tool = Tool::Select;
                        }
                    }
                }
            }
        }
    }
    if response.double_clicked() {
        if let Some((o, d)) = ray {
            if let Some(h) = pick(app, o, d, true) {
                app.settings.camera.target = h.point;
            }
        }
    }
    if response.secondary_clicked() {
        if let Some((o, d)) = ray {
            if let Some(h) = pick(app, o, d, false) {
                if !app.is_selected(h.id) {
                    app.click_select(h.id, false, alt);
                }
            }
        }
    }
    let ctx = ui.ctx().clone();
    response.context_menu(|ui| crate::context::object_menu(app, ui, &ctx));

    // ---------- GPU frame ----------
    let view = View { rect, aspect: app.viewport_aspect, cam: app.settings.camera };
    if app.viewport_ok {
        let checked = update_print_checks(app, ui.ctx());
        let frame = build_frame(app, &view, &checked);
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

fn run_gizmo(app: &mut App, ui: &mut egui::Ui, view: &View, viewport_hovered: bool) -> bool {
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    if !pointer_down {
        app.gizmo_dragging = false;
    }
    if ui.input(|i| i.pointer.primary_pressed()) {
        app.gizmo_used = false;
    }
    if app.tool != Tool::Select || app.gizmo_kind == GizmoKind::Off || app.selection.is_empty() || app.any_locked_selected() {
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
    if let Some((result, new)) = gizmo_interact(&mut app.gizmo, ui, view.rect, &targets, viewport_hovered || app.gizmo_dragging) {
        // a press on a handle that hasn't moved anything yet is not an edit (no undo step,
        // and the click still selects what's under the cursor)
        let changed = app.selection.iter().zip(&new).any(|(id, t)| {
            app.scene.get(*id).is_some_and(|o| {
                let (tr, ro, sc) = (t.translation, t.rotation, t.scale);
                (DVec3::new(tr.x, tr.y, tr.z).as_vec3() - o.transform.translation).length() > 1e-5
                    || DQuat::from_xyzw(ro.v.x, ro.v.y, ro.v.z, ro.s).as_quat().angle_between(o.transform.rotation) > 1e-5
                    || (DVec3::new(sc.x, sc.y, sc.z).as_vec3() - o.transform.scale).length() > 1e-5
            })
        });
        if !app.gizmo_dragging && !changed {
            return true;
        }
        app.gizmo_used = true;
        if !app.gizmo_dragging {
            app.gizmo_start = app.selection.iter().filter_map(|id| app.scene.get(*id)).map(|o| (o.id, o.transform)).collect();
            app.gizmo_start_bounds = app.selection_bounds();
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
        // translations: rebuild from start + total (the gizmo works incrementally, which would
        // accumulate snap corrections), then snap the group to other objects
        if let GizmoResult::Translation { total, .. } = result {
            let mut total = DVec3::new(total.x, total.y, total.z).as_vec3();
            let ids: Vec<_> = app.gizmo_start.iter().map(|s| s.0).collect();
            if app.settings.snap_objects {
                let b0 = app.gizmo_start_bounds;
                let moved = Aabb { min: b0.min + total, max: b0.max + total };
                let axes = [total.x.abs() > 1e-6, total.y.abs() > 1e-6, total.z.abs() > 1e-6];
                let targets = app.other_bounds(&ids);
                let tol = view.world_per_px(moved.center()) * 8.0;
                let plate = app.settings.show_plate.then_some(app.settings.plate * 0.5);
                let s = crate::snap::snap_box(&moved, &targets, plate, axes, tol);
                total += s.correction;
                app.snap_guides = s.guides;
            }
            for (id, t0) in app.gizmo_start.clone() {
                if let Some(o) = app.scene.get_mut(id) {
                    o.transform.translation = t0.translation + total;
                }
            }
            let dec = app.settings.decimals;
            app.drag_label = Some(format!("ΔX {:+.*}  ΔY {:+.*}  ΔZ {:+.*} mm", dec, total.x, dec, total.y, dec, total.z));
        }
        app.report_cache = None;
    }
    app.gizmo.is_focused() || app.gizmo_dragging
}

/// Drive and draw the gizmo with the viewport's own hover state. (The library's
/// `interact` registers a 1×1 widget under the cursor that steals hover and clicks
/// from the viewport whenever the mouse is still or moving slowly.)
fn gizmo_interact(
    gizmo: &mut Gizmo,
    ui: &egui::Ui,
    rect: Rect,
    targets: &[transform_gizmo_egui::math::Transform],
    hovered: bool,
) -> Option<(GizmoResult, Vec<transform_gizmo_egui::math::Transform>)> {
    let cursor = ui.input(|i| i.pointer.hover_pos()).unwrap_or_default();
    let result = gizmo.update(
        GizmoInteraction {
            cursor_pos: (cursor.x, cursor.y),
            hovered,
            drag_started: ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)),
            dragging: ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary)),
        },
        targets,
    );
    let draw = gizmo.draw();
    let mesh = egui::Mesh {
        indices: draw.indices,
        vertices: draw
            .vertices
            .into_iter()
            .zip(draw.colors)
            .map(|(pos, [r, g, b, a])| egui::epaint::Vertex {
                pos: pos.into(),
                uv: Pos2::default(),
                color: egui::Rgba::from_rgba_premultiplied(r, g, b, a).into(),
            })
            .collect(),
        ..Default::default()
    };
    egui::Painter::new(ui.ctx().clone(), ui.layer_id(), rect).add(mesh);
    result
}

fn build_frame(app: &App, view: &View, checked: &std::collections::HashMap<s3d_core::ObjectId, s3d_core::scene::MeshHandle>) -> ViewportFrame {
    let cam = &view.cam;
    let (r, u) = cam.right_up();
    let light = (-cam.forward() + u * 0.8 + r * 0.35).normalize();
    let eye = cam.eye();
    let camera = CameraUniform {
        view_proj: cam.view_proj(view.aspect).to_cols_array_2d(),
        eye: [eye.x, eye.y, eye.z, 1.0],
        light: [light.x, light.y, light.z, if app.encode_srgb { 1.0 } else { 0.0 }],
        clip: match app.section.plane() {
            Some((n, d)) => [n.x, n.y, n.z, d],
            None => [0.0; 4],
        },
    };
    let pv = app.shown_preview();
    let pv_kind = app.preview.as_ref().map(|p| p.kind);
    let mut ghosts = vec![];
    let original_ghost =
        |o: &s3d_core::Object| DrawItem { mesh: o.mesh.clone(), model: o.matrix(), color: [0.85, 0.88, 0.95, 0.22], params: [0.0; 4] };
    let print_check = app.settings.print_check;
    let mut items: Vec<DrawItem> = app
        .scene
        .objects
        .iter()
        .filter(|o| o.visible)
        .filter_map(|o| {
            let m = o.matrix();
            if o.hole {
                // glass-like, visible through solids
                let sel = app.is_selected(o.id) || app.hovered == Some(o.id);
                let color = if sel { [0.45, 0.65, 1.0, 0.45] } else { [0.62, 0.65, 0.72, 0.32] };
                ghosts.push(DrawItem { mesh: o.mesh.clone(), model: m, color, params: [0.55, 0.0, 0.0, 0.0] });
                return None;
            }
            // live cut by the holes it overlaps
            let mesh_override = app.display_mesh(o.id);
            if let Some(None) = mesh_override {
                return None; // fully cut away
            }
            if app.see_through.contains(&o.id) {
                let c = lin(o.color);
                let mesh = mesh_override.clone().flatten().unwrap_or_else(|| o.mesh.clone());
                ghosts.push(DrawItem { mesh, model: m, color: [c[0], c[1], c[2], 0.3], params: [0.0; 4] });
                return None;
            }
            let mut mesh = mesh_override.flatten().unwrap_or_else(|| o.mesh.clone());
            let check = checked.get(&o.id).cloned();
            if let Some(h) = &check {
                mesh = h.clone();
            }
            if let Some(h) = app.face_display(o.id) {
                mesh = h;
            }
            if let Some(r) = pv {
                if r.hide.contains(&o.id) {
                    if pv_kind == Some(PreviewKind::Round) {
                        ghosts.push(original_ghost(o));
                    }
                    return None;
                }
                if let Some((_, h)) = r.swap.iter().find(|s| s.0 == o.id) {
                    mesh = h.clone();
                }
            }
            Some(DrawItem {
                mesh,
                model: m,
                color: {
                    let c = lin(o.color);
                    if app.issues.0.contains(&o.id) || app.issues.1.contains(&o.id) {
                        // off / below the plate: push toward warning red
                        [c[0] * 0.35 + 0.65 * 0.9, c[1] * 0.35 + 0.65 * 0.08, c[2] * 0.35 + 0.65 * 0.05, c[3]]
                    } else if print_check {
                        // neutral, so the check colours stand out
                        lin([178, 182, 190, 255])
                    } else {
                        c
                    }
                },
                params: [
                    app.is_selected(o.id) as u8 as f32,
                    (app.hovered == Some(o.id)) as u8 as f32,
                    (m.determinant() < 0.0) as u8 as f32,
                    app.settings.show_problems as u8 as f32,
                ],
            })
        })
        .collect();
    if let Some(r) = pv {
        for (_, h, c, off) in &r.add {
            items.push(DrawItem {
                mesh: h.clone(),
                model: crate::preview::offset_matrix(*off),
                color: lin(*c),
                params: [1.0, 0.0, 0.0, app.settings.show_problems as u8 as f32],
            });
        }
    }
    // the shape being placed
    if let Some(pl) = &app.placing {
        let t = app.placement(
            match app.place_hover {
                Some((p, n)) => crate::app::PlaceAt::Surface(p, n),
                None => crate::app::PlaceAt::Beside,
            },
            &pl.ghost.mesh,
        );
        ghosts.push(DrawItem { mesh: pl.ghost.clone(), model: t.matrix(), color: [0.3, 0.75, 1.0, 0.6], params: [0.45, 0.0, 0.0, 0.0] });
    }
    // picture tool ghost
    if app.tool == Tool::Image {
        let s = &app.settings.image;
        let (mesh, model) = match (s.place, app.img_rt.hover) {
            (TextMode::Object, h) => (app.image_preview_mesh().cloned(), Some(app.image_object_matrix_h(h))),
            (_, Some(h)) if h.target.is_some() => match &app.img_rt.unit {
                Some(u) => {
                    let (m, model) = app.surface_ghost(u, &h, s.height, s.place == TextMode::Engrave);
                    (Some(m), Some(model))
                }
                None => (None, None),
            },
            _ => (None, None),
        };
        if let (Some(mesh), Some(model)) = (mesh, model) {
            let color = match s.place {
                TextMode::Engrave => [1.0, 0.35, 0.25, 0.55],
                TextMode::Emboss => [1.0, 0.75, 0.2, 0.6],
                TextMode::Object => [0.75, 0.78, 0.85, 0.7],
            };
            ghosts.push(DrawItem { mesh, model, color, params: [0.4, 0.0, 0.0, 0.0] });
        }
    }
    // text tool ghost
    if app.tool == Tool::Text {
        if let Some(base) = app.text_base() {
            let t = &app.settings.text;
            let ghost = match (t.mode, app.text_rt.hover) {
                (TextMode::Object, h) => app.text_object_matrix_h(h).map(|m| (base.clone(), m)),
                (_, Some(h)) if h.target.is_some() => Some(app.surface_ghost(base, &h, t.depth, t.mode == TextMode::Engrave)),
                _ => None,
            };
            if let Some((mesh, model)) = ghost {
                let color = match t.mode {
                    TextMode::Engrave => [1.0, 0.35, 0.25, 0.55],
                    TextMode::Emboss => [1.0, 0.75, 0.2, 0.6],
                    TextMode::Object => [0.3, 0.6, 1.0, 0.55],
                };
                ghosts.push(DrawItem { mesh, model, color, params: [0.45, 0.0, 0.0, 0.0] });
            }
        }
    }

    let dark = app.settings.dark;
    let mut tris = vec![];
    let mut lines = vec![];
    let mut overlay = vec![];
    // circular array: the circle the copies will sit on, and where each one goes
    if app.tab == crate::app::PanelTab::Arrange && app.array.circular && !app.selection.is_empty() && app.tool == Tool::Select {
        let a = &app.array;
        let c = app.circular_centre();
        let p0 = app.selection_bounds().center();
        let r = p0 - c;
        let n = a.circ_count.max(2);
        let sweep = a.circ_angle.clamp(-360.0, 360.0);
        let full = (sweep.abs() - 360.0).abs() < 1e-3;
        let step = if full { sweep / n as f32 } else { sweep / (n - 1) as f32 };
        let col = lin_rgba(0.35, 0.75, 1.0, 1.0);
        let rot = |deg: f32| glam::Quat::from_rotation_z(deg.to_radians());
        let segs = 96;
        let arc = if full { 360.0 } else { sweep };
        for k in 0..segs {
            let (t0, t1) = (arc * k as f32 / segs as f32, arc * (k + 1) as f32 / segs as f32);
            overlay.push(LineVertex::new(c + rot(t0) * r, col));
            overlay.push(LineVertex::new(c + rot(t1) * r, col));
        }
        let mark = r.length().max(1.0) * 0.06;
        for k in 0..n {
            let p = c + rot(step * k as f32) * r;
            for d in [Vec3::X, Vec3::Y] {
                overlay.push(LineVertex::new(p - d * mark, col));
                overlay.push(LineVertex::new(p + d * mark, col));
            }
        }
        // the centre
        for d in [Vec3::X, Vec3::Y] {
            overlay.push(LineVertex::new(c - d * mark * 1.5, col));
            overlay.push(LineVertex::new(c + d * mark * 1.5, col));
        }
    }
    // paint / face-select brush cursor
    let brush = match app.tool {
        Tool::Paint => app.paint_rt.hover.map(|h| {
            let s = &app.settings.paint;
            let c = if s.erase { lin_rgba(1.0, 1.0, 1.0, 0.9) } else { lin([s.color[0], s.color[1], s.color[2], 255]) };
            (h, c, s.mode == crate::paint_tool::PaintMode::Brush, s.radius)
        }),
        Tool::Sculpt => app.sculpt_rt.hover.map(|h| (h, lin_rgba(0.75, 0.9, 1.0, 1.0), true, app.settings.sculpt.radius)),
        Tool::Faces | Tool::Texture => app.face_rt.hover.map(|h| {
            let (mode, radius, _) = app.sel_params();
            (h, lin_rgba(1.0, 0.77, 0.16, 1.0), mode == Some(crate::faces_tool::SelMode::Brush), radius)
        }),
        _ => None,
    };
    if let Some((h, c, round, radius)) = brush {
        let n = h.normal.normalize_or(Vec3::Z);
        match round {
            true => {
                let (u, v) = (n.any_orthonormal_vector(), n.cross(n.any_orthonormal_vector()));
                let at = |k: usize| {
                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                    h.point + n * 0.05 + (u * a.cos() + v * a.sin()) * radius
                };
                for k in 0..48 {
                    overlay.push(LineVertex::new(at(k), c));
                    overlay.push(LineVertex::new(at(k + 1), c));
                }
            }
            false => {
                if let Some(o) = app.scene.get(h.id).filter(|o| h.tri < o.mesh.mesh.triangles.len()) {
                    let q = o.mesh.mesh.tri(h.tri).map(|p| o.matrix().transform_point3(p) + n * 0.05);
                    for k in 0..3 {
                        overlay.push(LineVertex::new(q[k], c));
                        overlay.push(LineVertex::new(q[(k + 1) % 3], c));
                    }
                }
            }
        }
    }
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

    // extrude-down height plane
    if app.preview.as_ref().is_some_and(|p| p.kind == PreviewKind::ExtrudeDown) && !app.selection.is_empty() {
        let b = app.selection_bounds();
        let pad = b.size().max_element() * 0.15 + 5.0;
        let z = app.mesh_tools.extrude_height;
        let q = [
            Vec3::new(b.min.x - pad, b.min.y - pad, z),
            Vec3::new(b.max.x + pad, b.min.y - pad, z),
            Vec3::new(b.max.x + pad, b.max.y + pad, z),
            Vec3::new(b.min.x - pad, b.max.y + pad, z),
        ];
        let fill = lin_rgba(0.3, 0.9, 0.5, 0.22);
        for k in [0, 1, 2, 0, 2, 3] {
            tris.push(LineVertex::new(q[k], fill));
        }
        for k in 0..4 {
            overlay.push(LineVertex::new(q[k], lin_rgba(0.35, 1.0, 0.55, 1.0)));
            overlay.push(LineVertex::new(q[(k + 1) % 4], lin_rgba(0.35, 1.0, 0.55, 1.0)));
        }
    }

    // section plane outline
    if let Some((n, _)) = app.section.plane() {
        let b = app.scene.bounds();
        if !b.is_empty() {
            let pad = b.size().max_element() * 0.05 + 2.0;
            let a = app.section.axis;
            let (i, j) = ((a + 1) % 3, (a + 2) % 3);
            let corner = |si: bool, sj: bool| {
                let mut p = Vec3::ZERO;
                p[a] = app.section.offset;
                p[i] = if si { b.max[i] + pad } else { b.min[i] - pad };
                p[j] = if sj { b.max[j] + pad } else { b.min[j] - pad };
                p
            };
            let q = [corner(false, false), corner(true, false), corner(true, true), corner(false, true)];
            let c = lin_rgba(0.2, 0.85, 1.0, 0.9);
            for k in 0..4 {
                lines.push(LineVertex::new(q[k], c));
                lines.push(LineVertex::new(q[(k + 1) % 4], c));
            }
            let _ = n;
        }
    }

    // snap guides
    for (a, b) in &app.snap_guides {
        let c = lin_rgba(1.0, 0.2, 0.85, 1.0);
        overlay.push(LineVertex::new(*a, c));
        overlay.push(LineVertex::new(*b, c));
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
    if app.tool == Tool::Measure {
        app.measure_overlay(&mut tris, &mut overlay);
    }
    let style = crate::render::MeshStyle { smooth: app.settings.smooth_shading, wireframe: app.settings.wireframe };
    let live = app.sculpt_live().into_iter().collect();
    ViewportFrame { camera, items, ghosts, tris, lines, overlay, style, live }
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
    for (at, text) in app.measure_labels() {
        if let Some(s) = view.to_screen(at) {
            label(p, s + egui::vec2(0.0, -16.0), text, crate::measure_tool::label_color());
        }
    }
    if app.measure.mode == crate::app::MeasureMode::Angle {
        for q in &app.measure.pts {
            if let Some(s) = view.to_screen(*q) {
                p.circle_filled(s, 4.5, Color32::from_rgb(255, 215, 30));
            }
        }
    }
    // tool hint
    let hint = match app.tool {
        Tool::Measure => Some(match app.measure.mode {
            crate::app::MeasureMode::Points => "Measure: click two points",
            crate::app::MeasureMode::Angle => "Angle: click three points",
            crate::app::MeasureMode::Faces => "Faces: click two flat faces",
            crate::app::MeasureMode::Circle => "Hole / pin: click a round surface",
        }),
        Tool::LayFlat => Some("Lay flat: click the face that should touch the plate (Shift+click to lay more) · L or Esc to stop"),
        Tool::Cut => Some("Cut: set the plane in the bar below, Enter to cut"),
        Tool::Text => Some(match app.settings.text.mode {
            TextMode::Object => "Text: click the plate or a face to place it · Shift+click to place more",
            TextMode::Emboss => "Emboss: click a face · Shift+click to do more",
            TextMode::Engrave => "Engrave: click a face · Shift+click to do more",
        }),
        Tool::Image => Some(match app.settings.image.place {
            TextMode::Object => "Picture: click the plate or a face to place it · Shift+click to place more",
            TextMode::Emboss => "Emboss picture: click a face · Shift+click to do more",
            TextMode::Engrave => "Engrave picture: click a face · Shift+click to do more",
        }),
        Tool::Paint => Some(match app.settings.paint.mode {
            crate::paint_tool::PaintMode::Brush => "Paint: drag on the model · drag off it to orbit · Ctrl+click picks a colour",
            crate::paint_tool::PaintMode::Fill => "Fill: click a face · Ctrl+click picks a colour",
            crate::paint_tool::PaintMode::Triangle => "Paint triangles: click or drag · Ctrl+click picks a colour",
        }),
        Tool::Faces => Some(match app.settings.faces.mode {
            crate::faces_tool::SelMode::Fill => "Select faces: click an area · Shift+click removes",
            crate::faces_tool::SelMode::Brush => "Select faces: drag on the model · Shift removes · drag off it to orbit",
        }),
        Tool::Texture => Some(match app.settings.texture.area {
            crate::texture_tool::TexArea::Whole => "Texture: click the object",
            crate::texture_tool::TexArea::Fill => "Texture: click an area · Shift+click removes",
            crate::texture_tool::TexArea::Brush => "Texture: drag over the area · Shift removes",
        }),
        Tool::Sculpt => Some("Sculpt: drag on the model · Shift reverses · Ctrl smooths · drag off it to orbit"),
        Tool::Select => None,
    };
    if let Some(pl) = &app.placing {
        let t = format!("Click the plate or a face to place {} · C: plate centre · Enter: beside the others · Esc: cancel", pl.name);
        label(p, rect.center_top() + egui::vec2(0.0, 18.0), t, Color32::WHITE);
        // the same as buttons, for touch and mouse-only use
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::new("placing_buttons"))
            .fixed_pos(rect.center_top() + egui::vec2(0.0, 40.0))
            .pivot(egui::Align2::CENTER_TOP)
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Plate centre").clicked() {
                        app.commit_placing(crate::app::PlaceAt::Centre);
                    }
                    if ui.button("Beside the others").clicked() {
                        app.commit_placing(crate::app::PlaceAt::Beside);
                    }
                    if ui.button("Cancel").clicked() {
                        app.cancel_placing();
                    }
                });
            });
    } else if let Some(h) = hint {
        label(p, rect.center_top() + egui::vec2(0.0, 18.0), h.to_string(), Color32::WHITE);
    }
    axis_triad(app, ui, p, view);
    plate_warnings(app, ui, view);
    print_check_legend(app, ui, view);
    bottom_bars(app, ui, view);
    if !app.section.enabled && app.preview.is_none() {
        crate::ribbon::object_box(app, ui, rect);
    }
    crate::ribbon::start_card(app, ui, rect);
    if let Some(hidden) = &app.isolated {
        let n = hidden.len();
        egui::Area::new(egui::Id::new("isolate_banner"))
            .fixed_pos(rect.center_top() + egui::vec2(0.0, 44.0))
            .pivot(egui::Align2::CENTER_TOP)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("Isolated — {n} other object{} hidden", if n == 1 { "" } else { "s" }));
                        if ui.small_button("Show them (I)").clicked() {
                            app.toggle_isolate();
                        }
                    });
                });
            });
    }
    drag_overlays(app, ui, p);
}

fn axis_triad(app: &mut App, ui: &mut egui::Ui, p: &egui::Painter, view: &View) {
    let center = Pos2::new(view.rect.left() + 55.0, view.rect.bottom() - 55.0);
    let v = view.cam.view();
    let mut axes: Vec<(usize, Vec3)> = (0..3).map(|a| (a, v.transform_vector3(Vec3::AXES[a]))).collect();
    axes.sort_by(|a, b| a.1.z.total_cmp(&b.1.z)); // draw far axes first
    p.circle_filled(center, 42.0, if app.settings.dark { Color32::from_black_alpha(60) } else { Color32::from_white_alpha(150) });
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

/// Notices in the bottom-right corner (like 3D Builder's repair prompt), with one-click
/// fixes: objects off or below the plate, and meshes that need repair.
fn plate_warnings(app: &mut App, ui: &mut egui::Ui, view: &View) {
    let (off, below) = app.issues.clone();
    let broken: Vec<s3d_core::ObjectId> =
        app.scene.objects.iter().filter(|o| o.visible && !o.hole && o.mesh.needs_repair() == Some(true)).map(|o| o.id).collect();
    if off.is_empty() && below.is_empty() && broken.is_empty() {
        return;
    }
    let n = |v: &Vec<s3d_core::ObjectId>| if v.len() == 1 { "1 object".to_string() } else { format!("{} objects", v.len()) };
    let dark = app.settings.dark;
    let (fill, warn) = if dark {
        (Color32::from_rgba_unmultiplied(70, 52, 20, 240), Color32::from_rgb(255, 200, 120))
    } else {
        (Color32::from_rgb(255, 244, 206), Color32::from_rgb(110, 70, 0))
    };
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::new("plate_warn"))
        .fixed_pos(view.rect.right_bottom() + egui::vec2(-12.0, -12.0))
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .order(egui::Order::Foreground)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(fill).corner_radius(8.0).show(ui, |ui| {
                let line = |ui: &mut egui::Ui, text: String| {
                    ui.add(crate::icons::image("triangle-alert", 16.0, warn));
                    ui.label(egui::RichText::new(text).color(warn));
                };
                if !broken.is_empty() {
                    ui.horizontal(|ui| {
                        line(ui, format!("{} need repair", n(&broken)));
                        if ui.small_button("Repair").on_hover_text("Close holes, fix flipped and broken faces").clicked() {
                            app.selection = broken.clone();
                            app.repair_selected(&ctx);
                        }
                        if ui.small_button("Select").clicked() {
                            app.selection = broken.clone();
                        }
                    });
                }
                if !off.is_empty() {
                    ui.horizontal(|ui| {
                        line(ui, format!("{} outside the build plate", n(&off)));
                        if ui.small_button("Move onto plate").clicked() {
                            app.fit_onto_plate(&off);
                        }
                        if ui.small_button("Select").clicked() {
                            app.selection = off.clone();
                        }
                    });
                }
                if !below.is_empty() {
                    ui.horizontal(|ui| {
                        line(ui, format!("{} below the plate", n(&below)));
                        if ui.small_button("Drop to plate").clicked() {
                            app.drop_ids(&below);
                        }
                        if ui.small_button("Select").clicked() {
                            app.selection = below.clone();
                        }
                    });
                }
            });
        });
}

/// After placing text or a picture the tool closes (like adding a shape); Shift keeps it
/// open for another one.
fn placed_one(app: &mut App, shift: bool) {
    if !shift {
        app.tool = Tool::Select;
    }
}

/// Decide what a left-drag does, based on what is under the press point and modifiers.
fn start_primary_drag(app: &mut App, view: &View, origin: Pos2, shift: bool, alt: bool, ctrl: bool, cad: bool) -> DragMode {
    if app.placing.is_some() {
        return DragMode::Orbit;
    }
    if app.tool == Tool::Sculpt && !alt {
        let (o, d) = view.ray(origin);
        if let Some(h) = pick(app, o, d, false).filter(|h| app.scene.get(h.id).is_some_and(|o| !o.hole && !o.locked)) {
            app.sculpt_begin(crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri }, d, shift, ctrl);
            return DragMode::Sculpt;
        }
        return DragMode::Orbit;
    }
    if matches!(app.tool, Tool::Faces | Tool::Texture) && !alt {
        let (o, d) = view.ray(origin);
        if let Some(h) = pick(app, o, d, false).filter(|h| app.scene.get(h.id).is_some_and(|o| !o.hole && !o.locked)) {
            app.face_select_at(crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri }, true, shift);
            return DragMode::FaceSel { remove: shift };
        }
        return DragMode::Orbit;
    }
    if app.tool == Tool::Paint && !alt && !ctrl {
        let (o, d) = view.ray(origin);
        if let Some(h) = pick(app, o, d, false).filter(|h| app.scene.get(h.id).is_some_and(|o| !o.hole)) {
            app.begin_paint_stroke();
            app.paint_at(crate::paint_tool::PaintHover { id: h.id, point: h.point, normal: h.normal, tri: h.tri }, true);
            return DragMode::Paint;
        }
        return DragMode::Orbit;
    }
    if !matches!(app.tool, Tool::Select | Tool::Cut) {
        return DragMode::Orbit;
    }
    let (o, d) = view.ray(origin);
    // Shift at the start of a drag always box-selects (even over objects)
    if shift && !alt {
        return DragMode::Box { start: origin, additive: true };
    }
    if app.settings.drag_moves_objects && app.tool == Tool::Select {
        if let Some(hit) = pick(app, o, d, false) {
            if !app.is_selected(hit.id) {
                app.click_select(hit.id, shift || ctrl, alt);
            }
            app.checkpoint("Move");
            let start: Vec<_> =
                app.selection.iter().filter_map(|id| app.scene.get(*id)).filter(|o| !o.locked).map(|o| (o.id, o.transform)).collect();
            let ids: Vec<_> = start.iter().map(|s| s.0).collect();
            let bounds = app.scene.bounds_of(&ids);
            return DragMode::Move { grab: hit.point, start, bounds, vertical: alt };
        }
    }
    if alt {
        DragMode::Orbit
    } else if cad || shift {
        DragMode::Box { start: origin, additive: shift || ctrl }
    } else {
        DragMode::Orbit
    }
}

/// Move the dragged objects to follow the cursor: slide on a horizontal plane through the
/// grab point (Alt: vertical), rest on top of objects under the cursor, snap to grid/objects.
fn update_move(app: &mut App, view: &View, p: Pos2, shift: bool, ctrl: bool) {
    let DragMode::Move { grab, start, bounds, vertical } = app.drag.clone() else { return };
    let (o, d) = view.ray(p);
    let mut delta = Vec3::ZERO;
    if vertical {
        let f = view.cam.forward();
        let n = Vec3::new(f.x, f.y, 0.0).normalize_or_zero();
        let den = n.dot(d);
        if n == Vec3::ZERO || den.abs() < 1e-6 {
            return;
        }
        let hit = o + d * (n.dot(grab - o) / den);
        delta.z = hit.z - grab.z;
    } else {
        if d.z.abs() < 1e-5 {
            return;
        }
        let t = (grab.z - o.z) / d.z;
        if t <= 0.0 {
            return;
        }
        let hit = o + d * t;
        delta = Vec3::new(hit.x - grab.x, hit.y - grab.y, 0.0);
        // keep the move reasonable when the plane is nearly edge-on
        let max = view.cam.distance * 4.0;
        if delta.length() > max {
            delta = delta.normalize() * max;
        }
        if shift {
            if delta.x.abs() > delta.y.abs() {
                delta.y = 0.0;
            } else {
                delta.x = 0.0;
            }
        }
    }
    // grid snapping of the offset
    if app.settings.snap_enabled ^ ctrl {
        let step = app.settings.snap_move.max(1e-3);
        delta = (delta / step).round() * step;
    }
    let ids: Vec<_> = start.iter().map(|s| s.0).collect();
    // rest on top of whatever upward-facing surface is under the cursor
    if !vertical {
        let hit = app.scene.pick_detail_where(o, d, |ob| !ids.contains(&ob.id));
        match hit {
            Some(h) if h.normal.z > 0.7 => delta.z = h.point.z - bounds.min.z,
            _ => delta.z = 0.0,
        }
    }
    // object snapping
    if app.settings.snap_objects {
        let moved = Aabb { min: bounds.min + delta, max: bounds.max + delta };
        let targets = app.other_bounds(&ids);
        let tol = view.world_per_px(moved.center()) * 8.0;
        let axes = if vertical { [false, false, true] } else { [true, true, false] };
        let plate = app.settings.show_plate.then_some(app.settings.plate * 0.5);
        let s = crate::snap::snap_box(&moved, &targets, plate, axes, tol);
        delta += s.correction;
        app.snap_guides = s.guides;
    }
    for (id, t0) in &start {
        if let Some(ob) = app.scene.get_mut(*id) {
            ob.transform.translation = t0.translation + delta;
        }
    }
    if delta.x.abs() + delta.y.abs() > 1e-4 {
        app.move_dir = Vec3::new(delta.x, delta.y, 0.0);
    }
    let dec = app.settings.decimals;
    app.drag_label = Some(if vertical {
        format!("ΔZ {:+.*} mm", dec, delta.z)
    } else if delta.z.abs() > 1e-4 {
        format!("ΔX {:+.*}  ΔY {:+.*}  on top (Z {:.*})", dec, delta.x, dec, delta.y, dec, bounds.min.z + delta.z)
    } else {
        format!("ΔX {:+.*}  ΔY {:+.*} mm  ·  type a distance for an exact move", dec, delta.x, dec, delta.y)
    });
}

/// Window (drag left→right): objects fully inside. Crossing (right→left): objects touched.
fn finish_box_select(app: &mut App, view: &View, start: Pos2, end: Pos2, additive: bool) {
    let r = Rect::from_two_pos(start, end);
    if r.width() < 3.0 && r.height() < 3.0 {
        return;
    }
    let window = end.x >= start.x;
    let center_ray = view.ray(r.center());
    let center_hit = pick(app, center_ray.0, center_ray.1, false).map(|h| h.id);
    let mut picked = vec![];
    for o in app.scene.objects.iter().filter(|o| o.visible && !o.locked) {
        // test the real silhouette via (sampled) vertices, not the bounding box
        let m = o.matrix();
        let verts = &o.mesh.mesh.positions;
        let stride = (verts.len() / 4000).max(1);
        let mut any_in = false;
        let mut all_in = true;
        for v in verts.iter().step_by(stride) {
            match view.to_screen(m.transform_point3(*v)) {
                Some(sp) if r.contains(sp) => any_in = true,
                _ => all_in = false,
            }
        }
        let hit = if window { all_in } else { any_in || center_hit == Some(o.id) };
        if hit {
            picked.push(o.id);
        }
    }
    if !additive {
        app.selection.clear();
    }
    // groups come along as a whole
    let picked: Vec<_> = picked.into_iter().flat_map(|id| app.scene.with_group(id)).collect();
    for id in picked {
        if !app.selection.contains(&id) {
            app.selection.push(id);
        }
    }
    let n = app.selection.len();
    app.info(format!("{n} object{} selected", if n == 1 { "" } else { "s" }));
}

/// Box-select rectangle, snap guides label and the move readout.
fn drag_overlays(app: &App, ui: &egui::Ui, p: &egui::Painter) {
    if let DragMode::Box { start, .. } = app.drag {
        if let Some(end) = ui.input(|i| i.pointer.latest_pos()) {
            let r = Rect::from_two_pos(start, end);
            let window = end.x >= start.x;
            let (fill, stroke) = if window {
                (Color32::from_rgba_unmultiplied(56, 132, 255, 30), Color32::from_rgb(90, 160, 255))
            } else {
                (Color32::from_rgba_unmultiplied(80, 220, 120, 30), Color32::from_rgb(110, 230, 140))
            };
            p.rect_filled(r, 0.0, fill);
            if window {
                p.rect_stroke(r, 0.0, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
            } else {
                let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
                p.extend(Shape::dashed_line(&c, Stroke::new(1.0, stroke), 6.0, 4.0));
            }
            let hint = if window { "inside" } else { "touching" };
            p.text(end + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, hint, egui::FontId::proportional(12.0), stroke);
        }
    }
    if let (Some(l), Some(pos)) = (&app.drag_label, ui.input(|i| i.pointer.latest_pos())) {
        label(p, pos + egui::vec2(0.0, 28.0), l.clone(), Color32::WHITE);
    }
}

/// Floating bars at the bottom of the viewport: section-plane controls and the
/// Apply / Cancel bar of a running preview.
fn bottom_bars(app: &mut App, ui: &mut egui::Ui, view: &View) {
    let picture = app.tool == Tool::Image && app.preview.is_none();
    if !app.section.enabled && app.preview.is_none() && !picture {
        return;
    }
    let ctx = ui.ctx().clone();
    // keep clear of the axis triad in the bottom-left corner
    let mut room = view.rect;
    room.min.x += 112.0;
    egui::Area::new(egui::Id::new("bottom_bars"))
        .fixed_pos(view.rect.center_bottom() + egui::vec2(0.0, -14.0))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .constrain_to(room)
        .order(egui::Order::Foreground)
        .show(&ctx, |ui| {
            if app.section.enabled {
                egui::Frame::popup(ui.style()).show(ui, |ui| section_bar(app, ui));
                ui.add_space(6.0);
            }
            if picture {
                let w = (room.width() - 40.0).clamp(380.0, 760.0);
                let ctx = ui.ctx().clone();
                egui::Frame::popup(ui.style()).show(ui, |ui| crate::image_tool::bar(app, ui, &ctx, w));
            }
            if app.preview.is_some() {
                // fixed width: the bar is centred, so a bar that changed width as values
                // change would slide a dragged slider away under the pointer
                let w = (room.width() - 40.0).clamp(380.0, 760.0);
                egui::Frame::popup(ui.style()).show(ui, |ui| preview_bar(app, ui, w));
            }
        });
}

fn section_bar(app: &mut App, ui: &mut egui::Ui) {
    let b = app.scene.bounds();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("▣ Section").strong());
        for (a, n) in ["X", "Y", "Z"].iter().enumerate() {
            if ui.selectable_label(app.section.axis == a, *n).clicked() && app.section.axis != a {
                app.section.axis = a;
                if !b.is_empty() {
                    app.section.offset = b.center()[a];
                }
            }
        }
        let a = app.section.axis;
        let (lo, hi) = if b.is_empty() { (-100.0, 100.0) } else { (b.min[a] - 1.0, b.max[a] + 1.0) };
        ui.spacing_mut().slider_width = 220.0;
        ui.add(egui::Slider::new(&mut app.section.offset, lo..=hi).suffix(" mm").max_decimals(2));
        ui.toggle_value(&mut app.section.flip, "Flip side").on_hover_text("Look at the other half");
        if ui.small_button("✖").on_hover_text("Close section view (X)").clicked() {
            app.section.enabled = false;
        }
    });
}

fn preview_bar(app: &mut App, ui: &mut egui::Ui, width: f32) {
    let Some(kind) = app.preview.as_ref().map(|p| p.kind) else { return };
    let mut apply = false;
    let mut cancel = false;
    ui.set_width(width);
    ui.vertical(|ui| {
        // the operation's settings, so the side panel isn't needed
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(kind.name()).strong());
            ui.add_space(6.0);
            crate::ui::op_settings(app, ui, kind);
        });
        ui.add_space(2.0);
        let Some(p) = &app.preview else { return };
        let fresh = p.result.as_ref().is_some_and(|r| r.key == p.key);
        ui.horizontal(|ui| {
            let buttons = 150.0;
            let w = (width - buttons).max(120.0);
            ui.allocate_ui_with_layout(egui::vec2(w, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                if let Some(e) = &p.error {
                    ui.add(egui::Label::new(egui::RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(255, 130, 110))).truncate())
                        .on_hover_text(e);
                } else if let Some(t) = &p.task {
                    let f = t.progress.fraction();
                    let bar = egui::ProgressBar::new(f).desired_width(140.0).show_percentage();
                    ui.add(if f <= 0.0 { bar.animate(true) } else { bar });
                    if p.apply_when_ready {
                        ui.weak("applies when done");
                    }
                } else if !fresh {
                    ui.spinner();
                } else if let Some(r) = &p.result {
                    ui.add(egui::Label::new(egui::RichText::new(&r.message).weak()).truncate()).on_hover_text(&r.message);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").on_hover_text("Esc").clicked() {
                    cancel = true;
                }
                let label = if kind == PreviewKind::Cut { "✂ Cut" } else { "✔ Apply" };
                if ui.add_enabled(p.error.is_none(), egui::Button::new(egui::RichText::new(label).strong())).on_hover_text("Enter").clicked() {
                    apply = true;
                }
            });
        });
    });
    if apply {
        app.apply_preview();
    }
    if cancel {
        if kind == PreviewKind::Cut {
            app.tool = Tool::Select;
        }
        app.cancel_preview();
    }
}

/// Print check: coloured copies of the solids (computed in the background).
fn update_print_checks(app: &mut App, ctx: &egui::Context) -> std::collections::HashMap<s3d_core::ObjectId, s3d_core::scene::MeshHandle> {
    let mut checked: std::collections::HashMap<s3d_core::ObjectId, s3d_core::scene::MeshHandle> = Default::default();
    if app.settings.print_check {
        let todo: Vec<(s3d_core::ObjectId, s3d_core::scene::MeshHandle, glam::Mat4)> = app
            .scene
            .objects
            .iter()
            .filter(|o| o.visible && !o.hole)
            .filter_map(|o| match app.display_mesh(o.id) {
                Some(None) => None,
                Some(Some(h)) => Some((o.id, h, o.matrix())),
                None => Some((o.id, o.mesh.clone(), o.matrix())),
            })
            .collect();
        for (id, h, m) in todo {
            if let Some(c) = app.check_display(id, &h, &m) {
                checked.insert(id, c);
            }
        }
        let live: std::collections::HashSet<_> = app.scene.objects.iter().map(|o| o.id).collect();
        app.check_cache.retain(|id, _| live.contains(id));
        if app.checks_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    checked
}

/// What the print check found, with its settings (top left of the view).
fn print_check_legend(app: &mut App, ui: &mut egui::Ui, view: &View) {
    if !app.settings.print_check {
        return;
    }
    let mut total = s3d_core::printcheck::CheckStats::default();
    for o in app.scene.objects.iter().filter(|o| o.visible && !o.hole) {
        if let Some((_, s)) = app.check_cache.get(&o.id).and_then(|e| e.result.as_ref()) {
            total.overhang_area += s.overhang_area;
            total.thin_area += s.thin_area;
            total.islands += s.islands;
        }
    }
    let pending = app.checks_pending();
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::new("print_check_legend"))
        .fixed_pos(view.rect.left_top() + egui::vec2(12.0, 12.0))
        .order(egui::Order::Middle)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(8.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Print check").strong());
                    if pending {
                        ui.spinner();
                    }
                    if ui.add(egui::Button::image(crate::icons::image("x", 12.0, ui.visuals().text_color())).frame(false)).on_hover_text("Turn off").clicked() {
                        app.settings.print_check = false;
                    }
                });
                let row = |ui: &mut egui::Ui, c: [u8; 3], label: &str, value: String| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
                        ui.label(label);
                        ui.weak(value);
                    });
                };
                let area = |a: f32| if a < 1.0 { "none".to_string() } else if a < 100.0 { format!("{a:.0} mm²") } else { format!("{:.1} cm²", a / 100.0) };
                for (flag, c, name) in crate::app::CHECK_COLORS {
                    let v = match flag {
                        s3d_core::printcheck::OVERHANG => area(total.overhang_area),
                        s3d_core::printcheck::THIN => area(total.thin_area),
                        _ if total.islands == 0 => "none".to_string(),
                        _ => format!("{}", total.islands),
                    };
                    row(ui, c, name, v);
                }
                ui.separator();
                let s = &mut app.settings.check;
                ui.horizontal(|ui| {
                    ui.label("Overhang over");
                    ui.add(egui::DragValue::new(&mut s.overhang_deg).range(10.0..=80.0).speed(0.5).suffix("°"))
                        .on_hover_text("Surfaces leaning more than this from vertical need support (45° is typical)");
                });
                ui.horizontal(|ui| {
                    ui.label("Walls under");
                    ui.add(egui::DragValue::new(&mut s.min_wall).range(0.0..=10.0).speed(0.02).suffix(" mm"))
                        .on_hover_text("About two nozzle widths: 0.8 mm for a 0.4 mm nozzle (0 = don't check)");
                });
            });
        });
}
