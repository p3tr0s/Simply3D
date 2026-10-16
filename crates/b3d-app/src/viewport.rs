//! 3D viewport: camera input, picking, gizmo, GPU frame and 2D overlays.

use crate::app::{App, DragMode, GizmoKind, LeftDrag, Tool};
use crate::camera::ViewPreset;
use crate::preview::PreviewKind;
use crate::render::{CameraUniform, DrawItem, LineVertex, ViewportFrame};
use crate::text_tool::{TextHover, TextMode};
use crate::ui::ACCENT;
use b3d_core::scene::PickHit;
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
    if moved && !response.dragged() && total_tris < 1_500_000 {
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
                        bend: b3d_core::wrap::Bend::FLAT,
                    })
                    .or_else(|| plate_hit(o, d).map(TextHover::on_plate)),
                _ => pick(app, o, d, false).map(|h| {
                    let frame = app.surface_frame(h.point, h.normal, view.cam.right_up().1);
                    let bend = match app.text_base() {
                        Some(b) if app.settings.text.wrap => app.fit_bend(h.id, frame, App::shape_half(&b.mesh)),
                        _ => b3d_core::wrap::Bend::FLAT,
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
                        bend: b3d_core::wrap::Bend::FLAT,
                    })
                    .or_else(|| plate_hit(o, d).map(TextHover::on_plate)),
                _ => pick(app, o, d, false).map(|h| {
                    let frame = app.image_surface_frame(h.point, h.normal, view.cam.right_up().1);
                    let bend = match &app.img_rt.unit {
                        Some(u) if app.settings.image.wrap => app.fit_bend(h.id, frame, App::shape_half(&u.mesh)),
                        _ => b3d_core::wrap::Bend::FLAT,
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
            app.sculpt_rt.hover = ray.and_then(|(o, d)| pick(app, o, d, false)).map(|h| crate::paint_tool::PaintHover {
                id: h.id,
                point: h.point,
                normal: h.normal,
                tri: h.tri,
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
                if let Some(p) = pointer.or(ui.input(|i| i.pointer.latest_pos())) {
                    update_move(app, &view, p, shift, ctrl);
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
        app.commit_placing(at);
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
                            }
                        }
                        (_, Some(h)) => app.apply_image_on_surface(&ctx, h),
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
                        (TextMode::Object, h) => app.add_text_object(h),
                        (_, Some(h)) => app.apply_text_on_surface(&ctx, h),
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

fn build_frame(app: &App, view: &View) -> ViewportFrame {
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
        |o: &b3d_core::Object| DrawItem { mesh: o.mesh.clone(), model: o.matrix(), color: [0.85, 0.88, 0.95, 0.22], params: [0.0; 4] };
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
        let t = app.placement(app.place_hover, &pl.ghost.mesh);
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
    ViewportFrame { camera, items, ghosts, tris, lines, overlay }
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
        Tool::Measure => Some(match app.measure.mode {
            crate::app::MeasureMode::Points => "Measure: click two points",
            crate::app::MeasureMode::Angle => "Angle: click three points",
            crate::app::MeasureMode::Faces => "Faces: click two flat faces",
            crate::app::MeasureMode::Circle => "Hole / pin: click a round surface",
        }),
        Tool::LayFlat => Some("Lay flat: click the face that should touch the plate"),
        Tool::Cut => Some("Cut: set the plane in the right panel, Enter to cut"),
        Tool::Text => Some(match app.settings.text.mode {
            TextMode::Object => "Text: click the plate or a face to place it",
            TextMode::Emboss => "Emboss: click a face",
            TextMode::Engrave => "Engrave: click a face",
        }),
        Tool::Image => Some(match app.settings.image.place {
            TextMode::Object => "Picture: click the plate or a face to place it",
            TextMode::Emboss => "Emboss picture: click a face",
            TextMode::Engrave => "Engrave picture: click a face",
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
        let t = format!("Click the plate or a face to place {} · Enter: beside the others · Esc: cancel", pl.name);
        label(p, rect.center_top() + egui::vec2(0.0, 18.0), t, Color32::WHITE);
    } else if let Some(h) = hint {
        label(p, rect.center_top() + egui::vec2(0.0, 18.0), h.to_string(), Color32::WHITE);
    }
    axis_triad(app, ui, p, view);
    plate_warnings(app, ui, view);
    bottom_bars(app, ui, view);
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

/// Floating notice in the viewport's top-right corner with one-click fixes.
fn plate_warnings(app: &mut App, ui: &mut egui::Ui, view: &View) {
    let (off, below) = app.issues.clone();
    if off.is_empty() && below.is_empty() {
        return;
    }
    let n = |v: &Vec<b3d_core::ObjectId>| if v.len() == 1 { "1 object".to_string() } else { format!("{} objects", v.len()) };
    egui::Area::new(egui::Id::new("plate_warn"))
        .fixed_pos(view.rect.right_top() + egui::vec2(-12.0, 12.0))
        .pivot(egui::Align2::RIGHT_TOP)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(Color32::from_rgba_unmultiplied(70, 28, 24, 235)).show(ui, |ui| {
                let warn = Color32::from_rgb(255, 170, 140);
                if !off.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("⚠ {} outside the build plate", n(&off))).color(warn));
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
                        ui.label(egui::RichText::new(format!("⚠ {} below the plate", n(&below))).color(warn));
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
    let dec = app.settings.decimals;
    app.drag_label = Some(if vertical {
        format!("ΔZ {:+.*} mm", dec, delta.z)
    } else if delta.z.abs() > 1e-4 {
        format!("ΔX {:+.*}  ΔY {:+.*}  on top (Z {:.*})", dec, delta.x, dec, delta.y, dec, bounds.min.z + delta.z)
    } else {
        format!("ΔX {:+.*}  ΔY {:+.*} mm", dec, delta.x, dec, delta.y)
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
    if !app.section.enabled && app.preview.is_none() {
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
            if app.preview.is_some() {
                egui::Frame::popup(ui.style()).show(ui, |ui| preview_bar(app, ui));
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

fn preview_bar(app: &mut App, ui: &mut egui::Ui) {
    let Some(p) = &app.preview else { return };
    let kind = p.kind;
    let fresh = p.result.as_ref().is_some_and(|r| r.key == p.key);
    let mut apply = false;
    let mut cancel = false;
    let mut new_height = None;
    // status in a fixed-width box: the bar is centred, so if its width changed while a
    // slider is being dragged, the slider would slide away under the pointer
    let status = |ui: &mut egui::Ui, w: f32| {
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
                ui.add(egui::Label::new(&r.message).truncate()).on_hover_text(&r.message);
            }
        });
    };
    let extrude = kind == PreviewKind::ExtrudeDown;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} preview", kind.name())).strong());
            if extrude {
                // the height plane right here, so the side panel isn't needed
                let b = app.scene.bounds_of(&app.selection);
                if !b.is_empty() {
                    ui.spacing_mut().slider_width = 180.0;
                    let mut h = app.mesh_tools.extrude_height;
                    let mut changed = ui.add(egui::Slider::new(&mut h, b.min.z..=b.max.z).show_value(false)).changed();
                    // fixed-width number box, so the bar doesn't change width as the value changes
                    let field = egui::DragValue::new(&mut h).range(b.min.z..=b.max.z).speed(0.1).suffix(" mm").max_decimals(1);
                    changed |= ui.add_sized([78.0, ui.spacing().interact_size.y], field).changed();
                    if changed {
                        new_height = Some(h);
                    }
                }
            } else {
                status(ui, 340.0);
            }
            let label = if kind == PreviewKind::Cut { "✂ Cut" } else { "✔ Apply" };
            if ui.add_enabled(p.error.is_none(), egui::Button::new(egui::RichText::new(label).strong())).on_hover_text("Enter").clicked() {
                apply = true;
            }
            if ui.button("Cancel").on_hover_text("Esc").clicked() {
                cancel = true;
            }
        });
        if extrude {
            status(ui, 420.0);
        }
    });
    if let Some(h) = new_height {
        app.mesh_tools.extrude_height = h;
    }
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
