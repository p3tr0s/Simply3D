//! Panels, menus, dialogs and keyboard shortcuts.

use crate::app::{App, CutKeep, GizmoKind, PanelTab, PosRef, Tool};
use crate::camera::ViewPreset;
use crate::expr::{self, Kind};
use crate::preview::PreviewKind;
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};
use glam::Vec3;
use s3d_core::csg::BoolOp;
use s3d_core::primitives::Primitive;

pub const ACCENT: Color32 = Color32::from_rgb(0, 120, 212);
const AXIS_COLORS: [Color32; 3] = [Color32::from_rgb(235, 80, 80), Color32::from_rgb(90, 200, 90), Color32::from_rgb(80, 140, 255)];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

/// DragValue that also accepts typed expressions and units (see expr.rs).
pub fn num(v: &mut f32, kind: Kind) -> egui::DragValue<'_> {
    let cur = *v as f64;
    egui::DragValue::new(v).custom_parser(move |s| expr::eval(s, cur, kind)).update_while_editing(false)
}

const NUM_TIP: &str = "Type a value or an expression: 20+5, (10+2)*3, 1in, 2.5cm, *2, /4, 50%";

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    if dark {
        v.panel_fill = Color32::from_rgb(32, 32, 32);
        v.window_fill = Color32::from_rgb(43, 43, 43);
        v.extreme_bg_color = Color32::from_rgb(28, 28, 28);
        v.faint_bg_color = Color32::from_rgb(40, 40, 40);
        v.selection.bg_fill = Color32::from_rgb(0, 95, 184);
    } else {
        // Windows 11 light: soft grey chrome, white cards
        v.panel_fill = Color32::from_rgb(243, 243, 243);
        v.window_fill = Color32::from_rgb(252, 252, 252);
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(236, 236, 236);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(253, 253, 253);
        v.widgets.inactive.bg_fill = Color32::from_rgb(253, 253, 253);
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(214, 214, 214));
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(232, 232, 232);
        v.selection.bg_fill = Color32::from_rgb(153, 201, 239);
        v.selection.stroke.color = Color32::from_rgb(0, 60, 116);
    }
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

pub fn shortcuts(app: &mut App, ctx: &egui::Context) {
    let sc = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
    if sc(Modifiers::COMMAND, Key::K) || sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::P) || sc(Modifiers::NONE, Key::F3) {
        app.palette = if app.palette.is_some() { None } else { Some(crate::commands::Palette::new()) };
    }
    if sc(Modifiers::NONE, Key::F12) {
        app.debug.toggle();
        let msg = match (&app.debug.on, &app.debug.path) {
            (true, Some(p)) => format!("Input diagnostics on — logging to {}", p.display()),
            (true, None) => "Input diagnostics on".to_string(),
            _ => "Input diagnostics off".to_string(),
        };
        app.info(msg);
    }
    if app.palette.is_some() {
        return; // the palette owns the keyboard
    }
    if app.typed_move_input(ctx) {
        return; // typing a distance while dragging
    }
    if sc(Modifiers::COMMAND, Key::O) {
        app.open_dialog(ctx);
    }
    if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
        app.save_as_dialog(ctx, false);
    }
    if sc(Modifiers::COMMAND, Key::S) {
        app.save(ctx);
    }
    // editing shortcuts belong to the text field while one is focused
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) {
        app.redo();
    }
    if sc(Modifiers::COMMAND, Key::Z) {
        app.undo();
    }
    if sc(Modifiers::COMMAND, Key::D) {
        app.duplicate_selected();
    }
    if sc(Modifiers::COMMAND, Key::N) {
        app.request_new_plate();
    }
    if sc(Modifiers::COMMAND, Key::A) {
        app.select_all();
    }
    if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::G) {
        app.ungroup_selected();
    }
    if sc(Modifiers::COMMAND, Key::G) {
        app.group_selected();
    }
    if sc(Modifiers::COMMAND, Key::L) {
        app.toggle_lock_selected();
    }
    if sc(Modifiers::SHIFT, Key::H) {
        app.show_all();
    }
    if sc(Modifiers::SHIFT, Key::S) {
        app.settings.smooth_shading = !app.settings.smooth_shading;
    }
    if sc(Modifiers::SHIFT, Key::W) {
        app.settings.wireframe = !app.settings.wireframe;
    }
    app.arrow_nudge(ctx);
    // single-key shortcuts
    let key = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    if key(Key::Delete) || key(Key::Backspace) {
        app.delete_selected();
    }
    if app.placing.is_some() && key(Key::Enter) {
        app.commit_placing(None);
    }
    if app.preview.is_some() && key(Key::Enter) {
        app.apply_preview();
    }
    if app.placing.is_some() && key(Key::Escape) {
        app.cancel_placing();
    }
    if key(Key::Escape) {
        if app.job.is_some() {
            app.cancel_job();
            return;
        }
        if app.tool != Tool::Select {
            app.tool = Tool::Select;
        } else if app.preview.is_some() {
            app.cancel_preview();
        } else {
            app.selection.clear();
        }
        app.measure.clear();
    }
    if key(Key::W) {
        app.gizmo_kind = GizmoKind::Move;
        app.tool = Tool::Select;
    }
    if key(Key::E) {
        app.gizmo_kind = GizmoKind::Rotate;
        app.tool = Tool::Select;
    }
    if key(Key::R) {
        app.gizmo_kind = GizmoKind::Scale;
        app.tool = Tool::Select;
    }
    if key(Key::Q) {
        app.gizmo_kind = GizmoKind::Off;
    }
    if key(Key::M) {
        app.tool = Tool::Measure;
    }
    if key(Key::T) {
        app.start_text_tool();
    }
    if key(Key::P) {
        app.start_paint_tool();
    }
    if key(Key::S) {
        app.start_faces_tool();
    }
    if key(Key::B) {
        app.start_sculpt_tool();
    }
    if key(Key::A) {
        let g = app.settings.arrange_gap;
        app.auto_arrange(g);
    }
    if key(Key::H) {
        app.hide_selected();
    }
    if key(Key::I) {
        app.toggle_isolate();
    }
    if key(Key::X) {
        app.toggle_section();
    }
    if key(Key::L) {
        app.tool = Tool::LayFlat;
    }
    if key(Key::D) {
        app.drop_selected();
    }
    if key(Key::F) {
        app.frame_selection();
    }
    if key(Key::Home) {
        app.frame_all();
    }
    if key(Key::O) {
        app.settings.camera.ortho = !app.settings.camera.ortho;
    }
    for (k, v) in
        [(Key::Num1, ViewPreset::Front), (Key::Num3, ViewPreset::Right), (Key::Num7, ViewPreset::Top), (Key::Num0, ViewPreset::Iso)]
    {
        if key(k) {
            app.settings.camera.set_preset(v);
        }
    }
}

impl App {
    pub fn frame_selection(&mut self) {
        let b = if self.selection.is_empty() { self.scene.bounds() } else { self.selection_bounds() };
        self.settings.camera.fit(&b, self.viewport_aspect);
    }
    pub fn toggle_section(&mut self) {
        self.section.enabled = !self.section.enabled;
        if self.section.enabled {
            let b = if self.selection.is_empty() { self.scene.bounds() } else { self.selection_bounds() };
            if !b.is_empty() {
                self.section.offset = b.center()[self.section.axis];
            }
        }
    }
    pub fn frame_all(&mut self) {
        let mut b = self.scene.bounds();
        if b.is_empty() {
            let p = self.settings.plate * 0.5;
            b = s3d_core::Aabb { min: Vec3::new(-p.x, -p.y, 0.0), max: Vec3::new(p.x, p.y, 20.0) };
        }
        self.settings.camera.fit(&b, self.viewport_aspect);
    }
}

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    app.issues = app.plate_issues();
    crate::ribbon::top(app, ui);
    status_bar(app, ui);
    if app.settings.sidebar {
        egui::Panel::right("side").resizable(true).default_size(320.0).show(ui, |ui| {
            let open = app.settings.objects_open;
            let r = egui::CollapsingHeader::new(RichText::new(format!("Objects ({})", app.scene.objects.len())).strong())
                .open(Some(open))
                .show(ui, |ui| object_list(app, ui));
            if r.header_response.clicked() {
                app.settings.objects_open = !open;
            }
            ui.separator();
            egui::ScrollArea::vertical().id_salt("props_scroll").auto_shrink([false, false]).show(ui, |ui| properties(app, ui));
        });
    }
    egui::CentralPanel::no_frame().show(ui, |ui| crate::viewport::viewport(app, ui));
    dialogs(app, ui.ctx());
    crate::commands::show(app, ui.ctx());
}

fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(job) = &app.job {
                let f = job.progress.fraction();
                let stage = job.progress.stage_text();
                let secs = job.started.elapsed().as_secs_f32();
                let text = if stage.is_empty() { job.label.clone() } else { format!("{} — {stage}", job.label.trim_end_matches('…')) };
                let bar = egui::ProgressBar::new(f).desired_width(220.0).show_percentage();
                let bar = if f <= 0.0 { bar.animate(true) } else { bar };
                ui.add(bar);
                ui.label(format!("{text}  {secs:.0}s"));
                let mut cancel = false;
                if ui.button("Cancel").on_hover_text("Esc").clicked() {
                    cancel = true;
                }
                if cancel {
                    app.cancel_job();
                }
            } else if let Some(s) = &app.status {
                let t = RichText::new(&s.text);
                ui.label(if s.error { t.color(Color32::from_rgb(255, 110, 100)) } else { t });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tris: usize = app.scene.objects.iter().map(|o| o.mesh.mesh.triangles.len()).sum();
                ui.label(format!("{} objects · {} triangles", app.scene.objects.len(), tris));
                if app.holes.busy {
                    ui.separator();
                    ui.spinner();
                    ui.weak("cutting holes");
                }
                if let Some(p) = app.cursor_plate {
                    ui.separator();
                    ui.monospace(format!("X {:>8.2}  Y {:>8.2}", p.x, p.y));
                }
                ui.separator();
                ui.label(if app.settings.camera.ortho { "Ortho" } else { "Persp" });
            });
        });
    });
}

/// Small clickable icon that is bright when `on`.
fn icon_toggle(ui: &mut egui::Ui, on: bool, icon: &str, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    let col = if resp.hovered() {
        ui.visuals().strong_text_color()
    } else if on {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.55)
    };
    crate::icons::image(icon, 14.0, col).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)));
    resp.on_hover_text(tip).clicked()
}

/// Padlock: closed and amber when locked, open and faint when not.
fn lock_toggle(ui: &mut egui::Ui, locked: bool, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    let col = if locked {
        Color32::from_rgb(214, 140, 30)
    } else if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.55)
    };
    let icon = if locked { "lock" } else { "lock-open" };
    crate::icons::image(icon, 14.0, col).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)));
    resp.on_hover_text(tip).clicked()
}

/// One line of the object list, captured before drawing so the right-click menu can
/// change the scene while the list is being drawn.
enum Row {
    Group { g: s3d_core::scene::GroupId, name: String, ids: Vec<s3d_core::ObjectId>, folded: bool, vis: bool, locked: bool },
    Item { id: s3d_core::ObjectId, indent: f32, name: String, color: [u8; 4], vis: bool, locked: bool },
}

fn list_rows(app: &App) -> Vec<Row> {
    let mut rows = vec![];
    let mut seen = std::collections::HashSet::new();
    let item = |o: &s3d_core::Object, indent: f32| Row::Item {
        id: o.id,
        indent,
        name: o.name.clone(),
        color: o.color,
        vis: o.visible,
        locked: o.locked,
    };
    for o in &app.scene.objects {
        match o.group {
            Some(g) if seen.insert(g) => {
                let members: Vec<&s3d_core::Object> = app.scene.objects.iter().filter(|x| x.group == Some(g)).collect();
                let folded = app.collapsed.contains(&g);
                rows.push(Row::Group {
                    g,
                    name: app.scene.group_name(g).unwrap_or("Group").to_string(),
                    ids: members.iter().map(|m| m.id).collect(),
                    folded,
                    vis: members.iter().all(|m| m.visible),
                    locked: members.iter().all(|m| m.locked),
                });
                if !folded {
                    rows.extend(members.into_iter().map(|m| item(m, 18.0)));
                }
            }
            Some(_) => {}
            None => rows.push(item(o, 0.0)),
        }
    }
    rows
}

fn object_list(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(app.can_ungroup(), egui::Button::new("Ungroup").small()).on_hover_text("Ctrl+Shift+G").clicked() {
                app.ungroup_selected();
            }
            if ui
                .add_enabled(app.selection.len() >= 2, egui::Button::new("Group").small())
                .on_hover_text("Keep the selected objects together (Ctrl+G)")
                .clicked()
            {
                app.group_selected();
            }
        });
    });
    ui.separator();
    if app.scene.objects.is_empty() {
        ui.weak("Nothing here yet: add a shape or open a model.");
        return;
    }
    if app.isolated.is_some() || app.any_hidden() {
        ui.horizontal(|ui| {
            let n = app.scene.objects.iter().filter(|o| !o.visible).count();
            ui.weak(format!("{n} hidden"));
            if ui.small_button("Show all").on_hover_text("Shift+H").clicked() {
                app.show_all();
            }
        });
    }
    let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().id_salt("objects_scroll").max_height(230.0).auto_shrink([false, true]).show(ui, |ui| {
        for row in list_rows(app) {
            match row {
                Row::Group { g, name, ids, folded, vis, locked } => {
                    ui.horizontal(|ui| {
                        if icon_toggle(
                            ui,
                            true,
                            if folded { "chevron-right" } else { "chevron-down" },
                            if folded { "Expand" } else { "Collapse" },
                        ) {
                            if !app.collapsed.remove(&g) {
                                app.collapsed.insert(g);
                            }
                        }
                        if icon_toggle(ui, vis, if vis { "eye" } else { "eye-off" }, "Show / hide the group (H)") {
                            app.set_visible(&ids, !vis);
                        }
                        if lock_toggle(ui, locked, "Lock: can't be clicked or moved in the view") {
                            app.set_locked(&ids, !locked);
                        }
                        let selected = ids.iter().all(|i| app.selection.contains(i));
                        let text = RichText::new(format!("🗀 {name}  ({})", ids.len())).strong();
                        let r =
                            ui.selectable_label(selected, text).on_hover_text("Click selects the whole group · right-click for options");
                        if r.clicked() {
                            app.select_group(g, additive);
                        }
                        if r.secondary_clicked() && !selected {
                            app.select_group(g, false);
                        }
                        r.context_menu(|ui| crate::context::object_menu(app, ui, &ctx));
                    });
                }
                Row::Item { id, indent, name, color, vis, locked } => {
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        if icon_toggle(ui, vis, if vis { "eye" } else { "eye-off" }, "Show / hide (H)") {
                            app.set_visible(&[id], !vis);
                        }
                        if lock_toggle(ui, locked, "Lock: can't be clicked or moved in the view (Ctrl+L)") {
                            app.set_locked(&[id], !locked);
                        }
                        let [r, g, b, _] = color;
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(r, g, b));
                        let selected = app.selection.contains(&id);
                        let mut text = RichText::new(name);
                        if !vis {
                            text = text.weak().italics();
                        }
                        let resp = ui.selectable_label(selected, text);
                        if resp.clicked() {
                            app.click_select(id, additive, true);
                        }
                        if resp.secondary_clicked() && !selected {
                            app.click_select(id, false, true);
                        }
                        resp.context_menu(|ui| crate::context::object_menu(app, ui, &ctx));
                        let off = app.issues.0.contains(&id);
                        let below = app.issues.1.contains(&id);
                        if off || below {
                            let tip = match (off, below) {
                                (true, true) => "Outside the build plate and below it",
                                (true, false) => "Outside the build plate",
                                _ => "Below the build plate",
                            };
                            ui.label(RichText::new("⚠").color(Color32::from_rgb(255, 120, 80))).on_hover_text(tip);
                        }
                    });
                }
            }
        }
    });
}

fn axis_drag(ui: &mut egui::Ui, axis: usize, v: &mut f32, speed: f64, suffix: &str, decimals: usize) -> egui::Response {
    ui.label(RichText::new(AXIS_NAMES[axis]).color(AXIS_COLORS[axis]).strong());
    let kind = if suffix.contains('°') { Kind::Angle } else { Kind::Length };
    ui.add(num(v, kind).speed(speed).suffix(suffix).max_decimals(decimals).min_decimals(decimals.min(2))).on_hover_text(NUM_TIP)
}

fn properties(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.add_space(4.0);
    match app.tool {
        Tool::Measure => crate::measure_tool::panel(app, ui),
        Tool::LayFlat => {
            ui.heading("Lay flat");
            ui.label("Click any face of an object: it is rotated so that face rests on the plate.");
            ui.separator();
        }
        Tool::Cut => cut_panel(app, ui),
        Tool::Text => crate::text_tool::panel(app, ui),
        Tool::Image => crate::image_tool::panel(app, ui, &ctx),
        Tool::Paint => crate::paint_tool::panel(app, ui, &ctx),
        Tool::Faces => crate::faces_tool::panel(app, ui, &ctx),
        Tool::Texture => crate::texture_tool::panel(app, ui),
        Tool::Sculpt => crate::sculpt_tool::panel(app, ui),
        Tool::Select => {}
    }
    let Some(&id) = app.selection.first() else {
        ui.heading("Scene");
        ui.label("Nothing selected. Click an object, or Ctrl+A.");
        ui.add_space(8.0);
        let b = app.scene.bounds();
        if !b.is_empty() {
            let s = b.size();
            ui.label(format!("Scene size: {:.1} × {:.1} × {:.1} mm", s.x, s.y, s.z));
        }
        ui.add_space(12.0);
        ui.weak("Tip: Ctrl+K searches every command. Number fields accept maths and units, e.g. 20+5, 1in, *2, 50%.");
        return;
    };
    let n = app.selection.len();
    let dec = app.settings.decimals;
    if let Some(g) = app.selected_group() {
        let mut name = app.scene.group_name(g).unwrap_or_default().to_string();
        ui.horizontal(|ui| {
            ui.heading("🗀");
            if ui.add(egui::TextEdit::singleline(&mut name).font(egui::TextStyle::Heading).desired_width(150.0)).changed() {
                app.rename_group(g, name);
            }
            ui.weak(format!("{n} objects"));
        });
        ui.weak("Group: click selects all of it, Alt+click one part.");
    } else {
        ui.heading(if n == 1 { "Object".to_string() } else { format!("{n} objects") });
    }
    ui.separator();
    if app.any_locked_selected() {
        ui.horizontal(|ui| {
            lock_toggle(ui, true, "Locked");
            ui.label(RichText::new("Locked").color(Color32::from_rgb(255, 200, 120)));
            if ui.small_button("Unlock").clicked() {
                let ids = app.selection.clone();
                app.set_locked(&ids, false);
            }
        });
    }

    // ----- name & colour -----
    if n == 1 {
        let mut name = app.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name).changed() {
                app.checkpoint_coalesced("Rename");
                if let Some(o) = app.scene.get_mut(id) {
                    o.name = name;
                }
            }
        });
    }
    let mut color = app.scene.get(id).map(|o| o.color).unwrap_or([200, 200, 200, 255]);
    ui.horizontal(|ui| {
        ui.label("Colour");
        if ui.color_edit_button_srgba_unmultiplied(&mut color).changed() {
            app.set_color_selected(color);
        }
        for c in s3d_core::scene::PALETTE {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            if resp.clicked() {
                app.set_color_selected(c);
                app.pending_edit = None;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Type");
        let holes = app.selection.iter().filter(|id| app.scene.get(**id).is_some_and(|o| o.hole)).count();
        if ui.selectable_label(holes == 0, "Solid").clicked() && holes > 0 {
            app.set_hole_selected(false);
        }
        if ui.selectable_label(holes == n, "○ Hole").on_hover_text("Cuts whatever it overlaps; stays movable").clicked() && holes < n {
            app.set_hole_selected(true);
        }
        let any_cut = app.selection.iter().any(|id| app.holes.cuts.contains_key(id)) || holes > 0;
        if any_cut && ui.small_button("Apply holes").on_hover_text("Make the cut permanent (removes the holes that are used up)").clicked()
        {
            let ctx = ui.ctx().clone();
            app.apply_holes_selected(&ctx);
        }
    });
    ui.add_space(6.0);

    // ----- tabs -----
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 3.0 * ui.spacing().item_spacing.x) / 4.0;
        for (t, label) in
            [(PanelTab::Object, "Object"), (PanelTab::Arrange, "Arrange"), (PanelTab::Mesh, "Mesh"), (PanelTab::Repair, "Repair")]
        {
            let r = ui.add_sized([w, 24.0], egui::Button::selectable(app.tab == t, label));
            if r.clicked() {
                app.tab = t;
            }
        }
    });
    ui.separator();
    let tab = app.tab;
    egui::ScrollArea::vertical().id_salt("tab_scroll").auto_shrink([false, false]).show(ui, |ui| {
        // ----- transform (primary object) -----
        if tab == PanelTab::Object {
            section(ui, "Transform");
            let locked = app.any_locked_selected();
            ui.add_enabled_ui(!locked, |ui| {
                if n > 1 {
                    ui.weak("Editing the group's bounding box.");
                }
                ui.horizontal(|ui| {
                    ui.label("Position of");
                    egui::ComboBox::from_id_salt("posref")
                        .selected_text(match app.pos_ref {
                            PosRef::Center => "Center",
                            PosRef::BottomCenter => "Bottom center",
                            PosRef::MinCorner => "Min corner",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut app.pos_ref, PosRef::BottomCenter, "Bottom center");
                            ui.selectable_value(&mut app.pos_ref, PosRef::Center, "Center");
                            ui.selectable_value(&mut app.pos_ref, PosRef::MinCorner, "Min corner");
                        });
                });
                let b = app.selection_bounds();
                let refp = match app.pos_ref {
                    PosRef::Center => b.center(),
                    PosRef::BottomCenter => Vec3::new(b.center().x, b.center().y, b.min.z),
                    PosRef::MinCorner => b.min,
                };
                let mut p = refp;
                let mut changed = false;
                ui.horizontal(|ui| {
                    for a in 0..3 {
                        changed |= axis_drag(ui, a, &mut p[a], 0.1, " mm", dec).changed();
                    }
                });
                if changed {
                    app.checkpoint_coalesced("Move");
                    let d = p - refp;
                    for sid in app.selection.clone() {
                        if let Some(o) = app.scene.get_mut(sid) {
                            o.transform.translation += d;
                        }
                    }
                }

                // size
                let size = b.size();
                let mut s = size;
                let mut changed_axis = None;
                ui.horizontal(|ui| {
                    ui.label("Size");
                    ui.checkbox(&mut app.lock_ratio, "Lock proportions").on_hover_text("Keep proportions");
                });
                ui.horizontal(|ui| {
                    for a in 0..3 {
                        if axis_drag(ui, a, &mut s[a], 0.1, " mm", dec).changed() {
                            changed_axis = Some(a);
                        }
                    }
                });
                if let Some(a) = changed_axis {
                    if size[a] > 1e-6 && s[a] > 1e-4 {
                        let k = s[a] / size[a];
                        let factor = if app.lock_ratio {
                            Vec3::splat(k)
                        } else {
                            let mut f = Vec3::ONE;
                            f[a] = k;
                            f
                        };
                        app.scale_selection(factor, "Resize");
                    }
                }
                // percentage scale
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    let mut pct = 100.0f32;
                    if ui.add(num(&mut pct, Kind::Plain).speed(0.5).suffix(" %").range(1.0..=10000.0)).on_hover_text(NUM_TIP).changed()
                        && (pct - 100.0).abs() > 1e-3
                    {
                        app.scale_selection(Vec3::splat(pct / 100.0), "Scale");
                    }
                    for (t, f) in [("50%", 0.5), ("200%", 2.0), ("in to mm", 25.4), ("mm to in", 1.0 / 25.4)] {
                        if ui.small_button(t).clicked() {
                            app.pending_edit = None;
                            app.scale_selection(Vec3::splat(f), "Scale");
                            app.pending_edit = None;
                        }
                    }
                });

                // rotation (single object)
                if n == 1 {
                    if let Some(o) = app.scene.get(id) {
                        let r0 = o.transform.euler_deg();
                        let mut r = r0;
                        let mut changed = false;
                        ui.label("Rotation");
                        ui.horizontal(|ui| {
                            for a in 0..3 {
                                changed |= axis_drag(ui, a, &mut r[a], 0.5, "°", 2).changed();
                            }
                        });
                        if changed {
                            app.checkpoint_coalesced("Rotate");
                            if let Some(o) = app.scene.get_mut(id) {
                                o.transform.set_euler_deg(r);
                            }
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Rotate 90°");
                    for a in 0..3 {
                        if ui.small_button(RichText::new(format!("{}+", AXIS_NAMES[a])).color(AXIS_COLORS[a])).clicked() {
                            app.rotate_selection(a, 90.0);
                        }
                        if ui.small_button(RichText::new(format!("{}−", AXIS_NAMES[a])).color(AXIS_COLORS[a])).clicked() {
                            app.rotate_selection(a, -90.0);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Drop to plate").on_hover_text("D").clicked() {
                        app.drop_selected();
                    }
                    if ui.button("Center on plate").clicked() {
                        app.center_selected_on_plate();
                    }
                    if ui.button("Reset rot/scale").clicked() {
                        app.reset_transform_selected();
                    }
                    if ui.button("Apply transform").on_hover_text("Bake rotation and scale into the mesh").clicked() {
                        app.bake_selected();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Mirror");
                    for a in 0..3 {
                        if ui.small_button(RichText::new(AXIS_NAMES[a]).color(AXIS_COLORS[a])).clicked() {
                            app.mirror_selected(a);
                        }
                    }
                });
            });
        }

        // ----- arrange -----
        if tab == PanelTab::Arrange {
            section(ui, "Align & distribute");
            ui.scope(|ui| {
                if n < 2 {
                    ui.weak("Select two or more objects to align them.");
                }
                ui.add_enabled_ui(n >= 2, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Align to");
                        ui.selectable_value(&mut app.settings.align_to_first, false, "Selection");
                        ui.selectable_value(&mut app.settings.align_to_first, true, "First selected")
                            .on_hover_text("The first object you selected stays put; the others line up with it");
                    });
                    egui::Grid::new("align").num_columns(4).show(ui, |ui| {
                        for a in 0..3 {
                            ui.label(RichText::new(format!("Align {}", AXIS_NAMES[a])).color(AXIS_COLORS[a]));
                            if ui.small_button("Min").clicked() {
                                app.align(a, 0);
                            }
                            if ui.small_button("Center").clicked() {
                                app.align(a, 1);
                            }
                            if ui.small_button("Max").clicked() {
                                app.align(a, 2);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.add_enabled_ui(n >= 3, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Distribute");
                        for a in 0..3 {
                            if ui.small_button(AXIS_NAMES[a]).clicked() {
                                app.distribute(a);
                            }
                        }
                    });
                });
                section(ui, "Plate");
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Auto-orient")
                        .on_hover_text(
                            "Turn the selected parts (or all) to print with the least overhang and the most contact with the plate",
                        )
                        .clicked()
                    {
                        app.auto_orient_selected();
                    }
                    if ui
                        .button("Auto-arrange")
                        .on_hover_text("Pack the selected parts (or all) on the plate, centred. Groups stay together")
                        .clicked()
                    {
                        let gap = app.settings.arrange_gap;
                        app.auto_arrange(gap);
                    }
                    ui.label("gap");
                    ui.add(num(&mut app.settings.arrange_gap, Kind::Length).speed(0.1).range(0.0..=100.0).suffix(" mm"));
                });
                section(ui, "Array copies");
                ui.horizontal(|ui| {
                    for a in 0..3 {
                        ui.label(RichText::new(AXIS_NAMES[a]).color(AXIS_COLORS[a]));
                        ui.add(egui::DragValue::new(&mut app.array.count[a]).range(1..=100).suffix("×"));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pitch");
                    for a in 0..3 {
                        ui.add(num(&mut app.array.spacing[a], Kind::Length).speed(0.5).suffix(" mm")).on_hover_text(NUM_TIP);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Create array").clicked() {
                        app.array_selected();
                    }
                    if ui.small_button("Fit pitch to size").clicked() {
                        let s = app.selection_bounds().size();
                        app.array.spacing = s + Vec3::splat(5.0);
                    }
                });
            });

            section(ui, "Combine & split");
            ui.scope(|ui| {
                ui.weak("Booleans use the first selected object as the base.");
                ui.horizontal_wrapped(|ui| {
                    let many = n >= 2;
                    if ui.add_enabled(many, egui::Button::new("Union")).clicked() {
                        app.boolean_selected(&ctx, BoolOp::Union);
                    }
                    if ui.add_enabled(many, egui::Button::new("Subtract")).clicked() {
                        app.boolean_selected(&ctx, BoolOp::Subtract);
                    }
                    if ui.add_enabled(many, egui::Button::new("Intersect")).clicked() {
                        app.boolean_selected(&ctx, BoolOp::Intersect);
                    }
                    if ui.add_enabled(many, egui::Button::new("Merge")).on_hover_text("Combine meshes without a boolean").clicked() {
                        app.merge_selected();
                    }
                    if ui.button("Split parts").on_hover_text("Separate disconnected shells into objects").clicked() {
                        app.split_selected(&ctx);
                    }
                    if ui.button("Cut…").clicked() {
                        app.start_cut();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(n >= 2, egui::Button::new("Group")).on_hover_text("Ctrl+G").clicked() {
                        app.group_selected();
                    }
                    if ui.add_enabled(app.can_ungroup(), egui::Button::new("Ungroup")).on_hover_text("Ctrl+Shift+G").clicked() {
                        app.ungroup_selected();
                    }
                    if ui.button("Hide").on_hover_text("H").clicked() {
                        app.hide_selected();
                    }
                    if ui
                        .button(if app.isolated.is_some() { "Show others" } else { "Isolate" })
                        .on_hover_text("Show only the selection (I)")
                        .clicked()
                    {
                        app.toggle_isolate();
                    }
                    if ui.button("Lock").on_hover_text("Ctrl+L").clicked() {
                        app.toggle_lock_selected();
                    }
                });
            });
        }

        // ----- mesh tools -----
        if tab == PanelTab::Mesh {
            mesh_tools(app, ui);
        }

        // ----- analysis & repair -----
        if tab == PanelTab::Repair {
            section(ui, "Analysis");
            ui.scope(|ui| {
                if let Some(r) = app.primary_report() {
                    egui::Grid::new("report").num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("Triangles");
                        ui.label(r.triangles.to_string());
                        ui.end_row();
                        ui.label("Vertices");
                        ui.label(r.vertices.to_string());
                        ui.end_row();
                        ui.label("Shells");
                        ui.label(r.components.to_string());
                        ui.end_row();
                        ui.label("Volume");
                        ui.label(format!("{:.2} cm³", r.volume_mm3 / 1000.0));
                        ui.end_row();
                        ui.label("Surface");
                        ui.label(format!("{:.2} cm²", r.area_mm2 / 100.0));
                        ui.end_row();
                        if let Some(b) = r.bounds {
                            let s = b.size();
                            ui.label("Bounds");
                            ui.label(format!("{:.2} × {:.2} × {:.2} mm", s.x, s.y, s.z));
                            ui.end_row();
                        }
                    });
                    let probs = r.problems();
                    if probs.is_empty() {
                        ui.label(RichText::new("✔ Watertight, ready to print").color(Color32::from_rgb(110, 210, 120)));
                    } else {
                        for p in probs {
                            ui.label(RichText::new(format!("⚠ {p}")).color(Color32::from_rgb(255, 170, 80)));
                        }
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Make solid…").on_hover_text("For badly broken meshes: rebuild as one closed solid (Mesh tab)").clicked()
                    {
                        app.tab = PanelTab::Mesh;
                        app.start_preview(PreviewKind::MakeSolid);
                    }
                    if ui
                        .button("🔧 Repair")
                        .on_hover_text("Weld, remove degenerate/duplicate faces, fix orientation, fill holes")
                        .clicked()
                    {
                        app.repair_selected(&ctx);
                    }
                    ui.label("Weld tol.");
                    ui.add(
                        num(&mut app.settings.weld_tolerance, Kind::Length).speed(0.0001).range(0.0..=1.0).max_decimals(5).suffix(" mm"),
                    )
                    .on_hover_text(NUM_TIP);
                });
            });
        }
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong());
    ui.add_space(2.0);
}

fn mesh_tools(app: &mut App, ui: &mut egui::Ui) {
    use crate::app::SmoothMode;
    use s3d_core::sdf::RoundMode;
    let tris: usize = app.selection.iter().filter_map(|id| app.scene.get(*id)).map(|o| o.mesh.mesh.triangles.len()).sum();
    let active = app.preview.as_ref().map(|p| p.kind);
    let t = &mut app.mesh_tools;

    ui.label(RichText::new("Simplify").strong());
    ui.horizontal(|ui| {
        ui.label("Keep");
        ui.add(egui::Slider::new(&mut t.simplify_pct, 1.0..=99.0).suffix(" %").max_decimals(0));
        ui.weak(format!("≈ {} tris", (tris as f32 * t.simplify_pct / 100.0) as usize));
    });
    ui.horizontal(|ui| {
        ui.label("Max deviation");
        ui.add(num(&mut t.simplify_err, Kind::Length).speed(0.005).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Stop before the surface moves more than this. 0 = no limit");
    });
    let simplify =
        preview_buttons(ui, active, PreviewKind::Simplify, "Quadric edge-collapse decimation; keeps the mesh closed and sharp edges crisp");

    ui.add_space(6.0);
    ui.label(RichText::new("Smooth").strong());
    ui.horizontal(|ui| {
        ui.selectable_value(&mut t.smooth_mode, SmoothMode::Subdivide, "Subdivide")
            .on_hover_text("Loop subdivision: rounder surface, 4× triangles per level");
        ui.selectable_value(&mut t.smooth_mode, SmoothMode::Relax, "Relax")
            .on_hover_text("Taubin smoothing: removes facets/noise, same triangle count, no shrinking");
        ui.selectable_value(&mut t.smooth_mode, SmoothMode::SubdivideFlat, "Refine")
            .on_hover_text("Split triangles without changing the shape");
    });
    ui.horizontal(|ui| match t.smooth_mode {
        SmoothMode::Relax => {
            ui.label("Iterations");
            ui.add(egui::DragValue::new(&mut t.relax_iters).range(1..=200));
        }
        _ => {
            ui.label("Levels");
            ui.add(egui::DragValue::new(&mut t.smooth_levels).range(1..=4));
            ui.weak(format!("gives {} tris", tris << (2 * t.smooth_levels)));
        }
    });
    if t.smooth_mode != SmoothMode::SubdivideFlat {
        ui.horizontal(|ui| {
            ui.checkbox(&mut t.keep_sharp, "Keep edges sharper than");
            ui.add_enabled(t.keep_sharp, num(&mut t.crease_deg, Kind::Angle).range(1.0..=179.0).suffix("°"));
        });
    }
    let smooth = preview_buttons(ui, active, PreviewKind::Smooth, "Preview the smoothed mesh, then Apply");

    ui.add_space(6.0);
    ui.label(RichText::new("Round edges (bevel)").strong());
    ui.horizontal(|ui| {
        ui.label("Radius");
        ui.add(num(&mut t.round_radius, Kind::Length).speed(0.05).range(0.05..=100.0).max_decimals(2).suffix(" mm")).on_hover_text(NUM_TIP);
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut t.round_mode, RoundMode::Convex, "Outside").on_hover_text("Round convex edges and corners");
        ui.selectable_value(&mut t.round_mode, RoundMode::Concave, "Inside").on_hover_text("Fill concave edges with a fillet");
        ui.selectable_value(&mut t.round_mode, RoundMode::Both, "Both");
    });
    ui.horizontal(|ui| {
        ui.label("Detail");
        ui.add(num(&mut t.round_voxel, Kind::Length).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Voxel size. Smaller = more accurate and slower. 0 = automatic");
        if t.round_voxel == 0.0 {
            let b = app.scene.bounds_of(&app.selection);
            if !b.is_empty() {
                ui.weak(format!("auto ≈ {:.2} mm", s3d_core::sdf::auto_voxel(t.round_radius, &b)));
            }
        }
    });
    ui.checkbox(&mut t.round_simplify, "Reduce triangles afterwards");
    let round = preview_buttons(
        ui,
        active,
        PreviewKind::Round,
        "Works on any closed mesh: every edge sharper than the radius becomes a fillet. The surface is rebuilt, so tiny details below the detail size are lost.",
    );

    ui.add_space(6.0);
    ui.label(RichText::new("Extrude down").strong());
    let sel_b = app.scene.bounds_of(&app.selection);
    ui.horizontal(|ui| {
        ui.label("From height");
        let (lo, hi) = if sel_b.is_empty() { (0.0, 100.0) } else { (sel_b.min.z, sel_b.max.z) };
        ui.add(egui::Slider::new(&mut app.mesh_tools.extrude_height, lo..=hi).suffix(" mm").max_decimals(2))
            .on_hover_text("Every underside below this height is extended straight down to the plate");
    });
    let extrude = preview_buttons(
        ui,
        active,
        PreviewKind::ExtrudeDown,
        "Extends the underside of the model straight down to the plate, from the chosen height: a flat base, simple supports under overhangs, floating parts tied to the plate (like 3D Builder).",
    );

    ui.add_space(6.0);
    ui.label(RichText::new("Hollow").strong());
    let t = &mut app.mesh_tools;
    ui.horizontal(|ui| {
        ui.label("Wall");
        ui.add(num(&mut t.hollow_wall, Kind::Length).speed(0.05).range(0.2..=100.0).max_decimals(2).suffix(" mm")).on_hover_text(NUM_TIP);
        ui.label("Detail");
        ui.add(num(&mut t.hollow_voxel, Kind::Length).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Voxel size for the inside surface. 0 = automatic");
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut t.hollow_drain, "Drain hole")
            .on_hover_text("A hole through the bottom wall (for resin, or to let air out), added as a hole object you can move");
        ui.add_enabled(t.hollow_drain, num(&mut t.hollow_drain_d, Kind::Length).speed(0.05).range(0.5..=50.0).suffix(" mm"));
    });
    let hollow = preview_buttons(
        ui,
        active,
        PreviewKind::Hollow,
        "Keeps a shell of the given wall thickness and removes the inside, saving material and weight. The outside stays exactly as it is. Use Section view (X) to see the inside.",
    );

    ui.add_space(6.0);
    ui.label(RichText::new("Make solid / remesh").strong());
    let t = &mut app.mesh_tools;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut t.solid_shell, false, "Solid")
            .on_hover_text("Fill the inside: closes holes, merges overlapping parts, removes inner walls");
        ui.selectable_value(&mut t.solid_shell, true, "Shell")
            .on_hover_text("Thicken the surface itself — for open sheets and scans of thin things");
        if t.solid_shell {
            ui.add(num(&mut t.solid_thickness, Kind::Length).speed(0.05).range(0.1..=50.0).suffix(" mm"));
        }
    });
    ui.horizontal(|ui| {
        ui.label("Detail");
        ui.add(num(&mut t.solid_voxel, Kind::Length).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Size of the rebuilt triangles. 0 = automatic (1/200 of the size)");
        ui.checkbox(&mut t.solid_adaptive, "Fewer triangles on flat areas").on_hover_text("Off = an even remesh");
    });
    let solid = preview_buttons(
        ui,
        active,
        PreviewKind::MakeSolid,
        "Rebuilds any mesh — holes, overlaps, flipped or loose parts — as one clean closed solid (Meshmixer's Make Solid). Details smaller than the detail size are lost.",
    );

    ui.add_space(6.0);
    ui.label(RichText::new("Pattern / lattice").strong());
    let t = &mut app.mesh_tools;
    {
        use s3d_core::sdf::Pattern;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut t.pattern, Pattern::Voronoi, "Voronoi").on_hover_text("The skin cut into organic cells");
            ui.selectable_value(&mut t.pattern, Pattern::Wireframe, "Wireframe")
                .on_hover_text("Struts along the edges of a low-poly version");
            ui.selectable_value(&mut t.pattern, Pattern::Gyroid, "Gyroid").on_hover_text("Filled with a gyroid lattice");
        });
        egui::Grid::new("pattern").num_columns(2).show(ui, |ui| {
            ui.label("Cell size");
            ui.add(num(&mut t.pattern_cell, Kind::Length).speed(0.1).range(1.0..=200.0).suffix(" mm"));
            ui.end_row();
            ui.label("Strut / wall");
            ui.add(num(&mut t.pattern_thickness, Kind::Length).speed(0.05).range(0.2..=50.0).suffix(" mm"));
            ui.end_row();
            match t.pattern {
                Pattern::Voronoi => {
                    ui.label("Depth");
                    ui.add(num(&mut t.pattern_skin, Kind::Length).speed(0.05).range(0.2..=50.0).suffix(" mm"))
                        .on_hover_text("How deep the cell borders go into the part");
                    ui.end_row();
                    ui.label("Variation");
                    ui.add(egui::DragValue::new(&mut t.pattern_seed).range(1..=9999)).on_hover_text("Try another random cell layout");
                    ui.end_row();
                }
                Pattern::Gyroid => {
                    ui.label("Outer skin");
                    ui.add(num(&mut t.pattern_skin, Kind::Length).speed(0.05).range(0.0..=50.0).suffix(" mm"))
                        .on_hover_text("0 = open lattice; more = closed part with lattice inside (light infill)");
                    ui.end_row();
                }
                Pattern::Wireframe => {}
            }
            ui.label("Detail");
            ui.add(num(&mut t.pattern_voxel, Kind::Length).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
                .on_hover_text("0 = automatic");
            ui.end_row();
        });
    }
    let pattern = preview_buttons(
        ui,
        active,
        PreviewKind::Pattern,
        "Turns the part into a decorative or lightweight lattice. Works on closed meshes (use Make solid first if it isn't).",
    );

    for (clicked, kind) in [
        (solid, PreviewKind::MakeSolid),
        (pattern, PreviewKind::Pattern),
        (simplify, PreviewKind::Simplify),
        (smooth, PreviewKind::Smooth),
        (round, PreviewKind::Round),
        (extrude, PreviewKind::ExtrudeDown),
        (hollow, PreviewKind::Hollow),
    ] {
        match clicked {
            Some(PreviewClick::Start) if kind == PreviewKind::ExtrudeDown => app.start_extrude_down(),
            Some(PreviewClick::Start) => app.start_preview(kind),
            Some(PreviewClick::Apply) => app.apply_preview(),
            Some(PreviewClick::Cancel) => app.cancel_preview(),
            None => {}
        }
    }
}

enum PreviewClick {
    Start,
    Apply,
    Cancel,
}

/// "Preview" button, or Apply / Cancel while this operation is being previewed.
fn preview_buttons(ui: &mut egui::Ui, active: Option<PreviewKind>, kind: PreviewKind, tip: &str) -> Option<PreviewClick> {
    let mut out = None;
    ui.horizontal(|ui| {
        if active == Some(kind) {
            if ui.button(RichText::new("✔ Apply").strong()).on_hover_text("Enter").clicked() {
                out = Some(PreviewClick::Apply);
            }
            if ui.button("Cancel").on_hover_text("Esc").clicked() {
                out = Some(PreviewClick::Cancel);
            }
            ui.weak("live preview").on_hover_text("Change the settings above and the preview updates");
        } else if ui
            .button(format!("{}…", kind.name()))
            .on_hover_text(format!("{tip}\n\nShows a live preview first; nothing changes until you Apply."))
            .clicked()
        {
            out = Some(PreviewClick::Start);
        }
    });
    out
}

fn edge_controls(ui: &mut egui::Ui, edge: &mut s3d_core::primitives::Edge) {
    use s3d_core::primitives::EdgeKind;
    ui.label("Edges");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut edge.kind, EdgeKind::Sharp, "Sharp");
        ui.selectable_value(&mut edge.kind, EdgeKind::Fillet, "Fillet");
        ui.selectable_value(&mut edge.kind, EdgeKind::Chamfer, "Chamfer");
    });
    ui.end_row();
    if edge.kind != EdgeKind::Sharp {
        ui.label(if edge.kind == EdgeKind::Fillet { "Radius" } else { "Chamfer" });
        ui.add(num(&mut edge.size, Kind::Length).speed(0.1).range(0.0..=1000.0).suffix(" mm")).on_hover_text(NUM_TIP);
        ui.end_row();
    }
    if edge.kind == EdgeKind::Fillet {
        ui.label("Round segments");
        ui.add(egui::DragValue::new(&mut edge.segments).range(2..=64));
        ui.end_row();
    }
}

impl App {
    pub fn start_cut(&mut self) {
        self.tool = Tool::Cut;
        if !self.selection.is_empty() {
            self.cut.offset = self.selection_bounds().center()[self.cut.axis];
            self.preview = None;
            self.start_preview(PreviewKind::Cut);
        }
    }

    pub fn scale_selection(&mut self, factor: Vec3, label: &str) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint_coalesced(label);
        let b = self.selection_bounds();
        // keep the bottom centre fixed so objects stay on the plate
        let pivot = Vec3::new(b.center().x, b.center().y, b.min.z);
        for id in self.selection.clone() {
            let Some(o) = self.scene.get(id) else { continue };
            if o.transform.rotation.angle_between(glam::Quat::IDENTITY) < 1e-4 {
                // local axes == world axes: just scale the transform
                if let Some(o) = self.scene.get_mut(id) {
                    o.transform.scale *= factor;
                    o.transform.translation = pivot + (o.transform.translation - pivot) * factor;
                }
            } else {
                // rotated: scale along world axes by baking into the mesh
                let mut w = o.world_mesh();
                w.transform(&(glam::Mat4::from_translation(pivot) * glam::Mat4::from_scale(factor) * glam::Mat4::from_translation(-pivot)));
                self.scene.set_world_mesh(id, w);
            }
        }
    }

    pub fn rotate_selection(&mut self, axis: usize, deg: f32) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Rotate 90°");
        let mut ax = Vec3::ZERO;
        ax[axis] = 1.0;
        let q = glam::Quat::from_axis_angle(ax, deg.to_radians());
        let c = self.selection_bounds().center();
        for id in self.selection.clone() {
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.rotation = (q * o.transform.rotation).normalize();
                o.transform.translation = c + q * (o.transform.translation - c);
            }
        }
        for id in self.selection.clone() {
            self.scene.drop_to_plate(id);
        }
    }
}

fn cut_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Cut");
    if app.selection.is_empty() {
        ui.label("Select the object(s) to cut.");
        ui.separator();
        return;
    }
    let b = app.selection_bounds();
    ui.horizontal(|ui| {
        ui.label("Plane normal");
        for a in 0..3 {
            let t = if app.cut.axis == a {
                RichText::new(AXIS_NAMES[a]).strong().color(Color32::WHITE)
            } else {
                RichText::new(AXIS_NAMES[a]).color(AXIS_COLORS[a])
            };
            if ui.selectable_label(app.cut.axis == a, t).clicked() {
                app.cut.axis = a;
                app.cut.offset = b.center()[a];
            }
        }
    });
    let a = app.cut.axis;
    ui.horizontal(|ui| {
        ui.label("Position");
        ui.add(egui::Slider::new(&mut app.cut.offset, b.min[a]..=b.max[a]).suffix(" mm").max_decimals(2));
    });
    ui.horizontal(|ui| {
        ui.label("Keep");
        ui.selectable_value(&mut app.cut.keep, CutKeep::Both, "Both");
        ui.selectable_value(&mut app.cut.keep, CutKeep::Above, "Upper");
        ui.selectable_value(&mut app.cut.keep, CutKeep::Below, "Lower");
    });
    {
        use s3d_core::connectors::ConnectorKind;
        let c = &mut app.cut.conn;
        ui.horizontal(|ui| {
            ui.label("Connectors");
            ui.selectable_value(&mut c.kind, ConnectorKind::None, "None");
            ui.selectable_value(&mut c.kind, ConnectorKind::Pegs, "Pegs")
                .on_hover_text("Pegs on the lower part, matching holes in the upper part");
            ui.selectable_value(&mut c.kind, ConnectorKind::Dowels, "Dowels").on_hover_text("Holes in both parts and separate dowel pins");
        });
        if c.kind != ConnectorKind::None {
            egui::Grid::new("conn").num_columns(2).show(ui, |ui| {
                ui.label("Diameter");
                ui.add(num(&mut c.diameter, Kind::Length).speed(0.1).range(1.0..=50.0).suffix(" mm"));
                ui.end_row();
                ui.label("Depth");
                ui.add(num(&mut c.depth, Kind::Length).speed(0.1).range(1.0..=100.0).suffix(" mm")).on_hover_text("How far into each part");
                ui.end_row();
                ui.label("Clearance");
                ui.add(num(&mut c.clearance, Kind::Length).speed(0.01).range(0.0..=1.0).suffix(" mm"))
                    .on_hover_text("Play in the holes so printed parts fit (0.15–0.3 mm)");
                ui.end_row();
                ui.label("How many");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut c.count, 0, "Auto");
                    for k in 1..=4u32 {
                        ui.selectable_value(&mut c.count, k, k.to_string());
                    }
                });
                ui.end_row();
            });
        }
    }
    if app.preview.as_ref().is_none_or(|p| p.kind != PreviewKind::Cut) {
        app.start_preview(PreviewKind::Cut);
    }
    ui.weak("The preview pulls the pieces apart so you can see the cut.");
    ui.horizontal(|ui| {
        if ui.button(RichText::new("✂ Cut").strong()).on_hover_text("Enter").clicked() {
            app.apply_preview();
        }
        if ui.button("Cancel").on_hover_text("Esc").clicked() {
            app.tool = Tool::Select;
        }
    });
    ui.separator();
}

fn dialogs(app: &mut App, ctx: &egui::Context) {
    if app.recovery.is_some() {
        let mut choice = None;
        let r = app.recovery.as_ref().unwrap();
        egui::Modal::new(egui::Id::new("recovery")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Recover unsaved work?");
            ui.add_space(4.0);
            let age = if r.age_minutes < 1 { "less than a minute".to_string() } else { format!("{} min", r.age_minutes) };
            let what = r
                .original
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "an untitled project".into());
            ui.label(format!("Simply3D didn't close normally last time. An autosave of {what} from {age} ago is available."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Recover").strong()).clicked() {
                    choice = Some(true);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => app.recover(ctx),
            Some(false) => {
                crate::autosave::clear();
                app.recovery = None;
            }
            None => {}
        }
    }

    if app.close_prompt {
        let mut choice = None;
        let name = app
            .current_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        egui::Modal::new(egui::Id::new("close_prompt")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Save changes?");
            ui.label(format!("{name} has unsaved changes."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Save").strong()).clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                app.close_prompt = false;
                app.close_after_save = true;
                app.save(ctx);
                if app.job.is_none() {
                    app.close_after_save = false; // save dialog cancelled
                }
            }
            Some(1) => {
                app.close_prompt = false;
                crate::autosave::clear();
                app.allow_close = true;
            }
            Some(2) => app.close_prompt = false,
            _ => {}
        }
    }

    if app.new_prompt {
        let mut choice = None;
        let name = app
            .current_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        egui::Modal::new(egui::Id::new("new_prompt")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Save before starting a new plate?");
            ui.label(format!("{name} has unsaved changes."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Save").strong()).clicked() {
                    choice = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(2);
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                choice = Some(2);
            }
        });
        match choice {
            Some(0) => {
                app.new_prompt = false;
                app.new_after_save = true;
                app.save(ctx);
                if app.job.is_none() {
                    app.new_after_save = false; // save dialog cancelled: stay on this plate
                }
            }
            Some(1) => {
                app.new_prompt = false;
                app.new_scene();
            }
            Some(2) => app.new_prompt = false,
            _ => {}
        }
    }

    if let Some(mut p) = app.add_dialog {
        let mut open = true;
        let mut add = false;
        egui::Window::new(format!("Add {}", p.name()))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .show(ctx, |ui| {
                egui::Grid::new("prim").num_columns(2).show(ui, |ui| {
                    let mm = |ui: &mut egui::Ui, label: &str, v: &mut f32| {
                        ui.label(label);
                        ui.add(num(v, Kind::Length).speed(0.2).range(0.01..=10000.0).suffix(" mm")).on_hover_text(NUM_TIP);
                        ui.end_row();
                    };
                    let segs = |ui: &mut egui::Ui, label: &str, v: &mut u32| {
                        ui.label(label);
                        ui.add(egui::DragValue::new(v).range(3..=512));
                        ui.end_row();
                    };
                    match &mut p {
                        Primitive::Box { size, edge } => {
                            mm(ui, "Width (X)", &mut size.x);
                            mm(ui, "Depth (Y)", &mut size.y);
                            mm(ui, "Height (Z)", &mut size.z);
                            edge_controls(ui, edge);
                        }
                        Primitive::Wedge { size } => {
                            mm(ui, "Width (X)", &mut size.x);
                            mm(ui, "Depth (Y)", &mut size.y);
                            mm(ui, "Height (Z)", &mut size.z);
                        }
                        Primitive::Cylinder { radius, height, segments, edge } => {
                            mm(ui, "Radius", radius);
                            mm(ui, "Height", height);
                            segs(ui, "Segments", segments);
                            edge_controls(ui, edge);
                        }
                        Primitive::Cone { bottom_radius, top_radius, height, segments } => {
                            mm(ui, "Bottom radius", bottom_radius);
                            ui.label("Top radius");
                            ui.add(num(top_radius, Kind::Length).speed(0.2).range(0.0..=10000.0).suffix(" mm"));
                            ui.end_row();
                            mm(ui, "Height", height);
                            segs(ui, "Segments", segments);
                        }
                        Primitive::Sphere { radius, segments, rings } => {
                            mm(ui, "Radius", radius);
                            segs(ui, "Segments", segments);
                            segs(ui, "Rings", rings);
                        }
                        Primitive::Torus { major_radius, minor_radius, major_segments, minor_segments } => {
                            mm(ui, "Ring radius", major_radius);
                            mm(ui, "Tube radius", minor_radius);
                            segs(ui, "Ring segments", major_segments);
                            segs(ui, "Tube segments", minor_segments);
                        }
                        Primitive::Tube { outer_radius, inner_radius, height, segments } => {
                            mm(ui, "Outer radius", outer_radius);
                            mm(ui, "Inner radius", inner_radius);
                            mm(ui, "Height", height);
                            segs(ui, "Segments", segments);
                        }
                        Primitive::Pyramid { base, height } => {
                            mm(ui, "Base", base);
                            mm(ui, "Height", height);
                        }
                        Primitive::Fastener(f) => {
                            use s3d_core::threads::{FastenerKind, Head, SIZES};
                            ui.label("Type");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut f.kind, FastenerKind::Bolt, "Bolt");
                                ui.selectable_value(&mut f.kind, FastenerKind::Nut, "Nut");
                                ui.selectable_value(&mut f.kind, FastenerKind::Rod, "Rod");
                                ui.selectable_value(&mut f.kind, FastenerKind::ThreadedHole, "Hole").on_hover_text(
                                    "A threaded hole: added as a hole object that cuts the thread into whatever it overlaps",
                                );
                            });
                            ui.end_row();
                            ui.label("Size");
                            egui::ComboBox::from_id_salt("thread_size").selected_text(f.spec().name).show_ui(ui, |ui| {
                                for (i, s) in SIZES.iter().enumerate() {
                                    ui.selectable_value(&mut f.size, i, format!("{}  (pitch {} mm)", s.name, s.pitch));
                                }
                            });
                            ui.end_row();
                            if f.kind != FastenerKind::Nut {
                                ui.label(if f.kind == FastenerKind::ThreadedHole { "Depth" } else { "Thread length" });
                                ui.add(num(&mut f.length, Kind::Length).speed(0.5).range(1.0..=500.0).suffix(" mm")).on_hover_text(NUM_TIP);
                                ui.end_row();
                            }
                            if f.kind == FastenerKind::Bolt {
                                ui.label("Head");
                                ui.horizontal(|ui| {
                                    ui.selectable_value(&mut f.head, Head::Hex, "Hex");
                                    ui.selectable_value(&mut f.head, Head::Socket, "Socket");
                                    ui.selectable_value(&mut f.head, Head::None, "None");
                                });
                                ui.end_row();
                            }
                            if matches!(f.kind, FastenerKind::Nut | FastenerKind::ThreadedHole) {
                                ui.label("Clearance");
                                ui.add(num(&mut f.clearance, Kind::Length).speed(0.01).range(0.0..=1.0).suffix(" mm"))
                                    .on_hover_text("Extra room on the internal thread so printed parts fit (0.15–0.3 mm is typical)");
                                ui.end_row();
                            }
                        }
                        Primitive::Gear(g) => {
                            ui.label("Teeth");
                            ui.add(egui::DragValue::new(&mut g.teeth).range(6..=300));
                            ui.end_row();
                            ui.label("Module");
                            ui.add(num(&mut g.module, Kind::Length).speed(0.05).range(0.2..=20.0).suffix(" mm"))
                                .on_hover_text("Tooth size. Gears that mesh need the same module. Pitch diameter = module × teeth");
                            ui.end_row();
                            mm(ui, "Thickness", &mut g.thickness);
                            mm(ui, "Bore (hole)", &mut g.bore);
                            ui.label("Pressure angle");
                            ui.add(num(&mut g.pressure_deg, Kind::Angle).range(14.5..=30.0).suffix("°"));
                            ui.end_row();
                            ui.label("Backlash");
                            ui.add(num(&mut g.backlash, Kind::Length).speed(0.01).range(0.0..=1.0).suffix(" mm"))
                                .on_hover_text("Play between meshing teeth so printed gears turn freely");
                            ui.end_row();
                            ui.label("");
                            ui.weak(format!(
                                "Pitch Ø {:.2} mm. Two gears mesh when their centres are (Ø1 + Ø2) / 2 apart",
                                g.module * g.teeth as f32
                            ));
                            ui.end_row();
                        }
                        Primitive::Spring(s) => {
                            mm(ui, "Outside diameter", &mut s.diameter);
                            mm(ui, "Wire thickness", &mut s.wire);
                            mm(ui, "Free length", &mut s.length);
                            ui.label("Coils");
                            ui.add(egui::DragValue::new(&mut s.coils).speed(0.1).range(1.0..=100.0));
                            ui.end_row();
                            ui.label("Ends");
                            ui.checkbox(&mut s.flat_ends, "Closed and flat (stands up)");
                            ui.end_row();
                        }
                        Primitive::Hinge(h) => {
                            mm(ui, "Length (along the pin)", &mut h.length);
                            mm(ui, "Leaf width", &mut h.leaf);
                            mm(ui, "Thickness", &mut h.thickness);
                            ui.label("Knuckles");
                            ui.add(egui::DragValue::new(&mut h.knuckles).range(3..=15))
                                .on_hover_text("Odd number, so the pin is held at both ends");
                            ui.end_row();
                            mm(ui, "Pin diameter", &mut h.pin);
                            ui.label("Clearance");
                            ui.add(num(&mut h.clearance, Kind::Length).speed(0.01).range(0.1..=1.0).suffix(" mm"))
                                .on_hover_text("Gap between the moving parts: 0.3–0.4 mm prints in place on most printers");
                            ui.end_row();
                        }
                        Primitive::SnapClip(c) => {
                            mm(ui, "Beam length", &mut c.length);
                            mm(ui, "Width", &mut c.width);
                            mm(ui, "Beam thickness", &mut c.thickness);
                            mm(ui, "Hook", &mut c.hook);
                            ui.label("Base");
                            ui.add(num(&mut c.base, Kind::Length).speed(0.1).range(0.0..=100.0).suffix(" mm"))
                                .on_hover_text("Block under the beam; 0 to attach the clip to your own part (Union)");
                            ui.end_row();
                            ui.label("");
                            ui.weak("Tip: duplicate it, make the copy a Hole and scale it up a little to cut the matching slot.");
                            ui.end_row();
                        }
                    }
                });
                ui.add_space(6.0);
                if ui.button(RichText::new("Add to scene").strong()).on_hover_text("Enter").clicked()
                    || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                {
                    add = true;
                }
            });
        if add {
            app.add_primitive(p);
            app.add_dialog = None;
        } else if open {
            app.add_dialog = Some(p);
        } else {
            app.add_dialog = None;
        }
    }

    if app.show_settings {
        let mut open = true;
        egui::Window::new("Settings").open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                let s = &mut app.settings;
                ui.label("Build plate X");
                ui.add(num(&mut s.plate.x, Kind::Length).range(10.0..=2000.0).suffix(" mm"));
                ui.end_row();
                ui.label("Build plate Y");
                ui.add(num(&mut s.plate.y, Kind::Length).range(10.0..=2000.0).suffix(" mm"));
                ui.end_row();
                ui.label("Grid spacing");
                ui.add(num(&mut s.grid_step, Kind::Length).range(0.5..=100.0).suffix(" mm"));
                ui.end_row();
                ui.label("New shapes");
                ui.checkbox(&mut s.place_by_click, "Click to place them (on the plate or on a face)")
                    .on_hover_text("Off: new shapes go straight onto the plate beside the others");
                ui.end_row();
                ui.label("Snap: move");
                ui.add(num(&mut s.snap_move, Kind::Length).range(0.01..=100.0).suffix(" mm"));
                ui.end_row();
                ui.label("Snap: rotate");
                ui.add(num(&mut s.snap_angle_deg, Kind::Angle).range(0.1..=90.0).suffix("°"));
                ui.end_row();
                ui.label("Snap: scale");
                ui.add(num(&mut s.snap_scale, Kind::Plain).range(0.01..=1.0));
                ui.end_row();
                ui.label("Autosave every");
                ui.add(num(&mut s.autosave_minutes, Kind::Plain).range(0.0..=60.0).suffix(" min"))
                    .on_hover_text("0 = off. Recovery files are kept until you save or close normally.");
                ui.end_row();
                ui.label("Decimals shown");
                ui.add(egui::DragValue::new(&mut s.decimals).range(0..=4));
                ui.end_row();
                ui.label("Field of view");
                let mut fov = s.camera.fov_y.to_degrees();
                if ui.add(egui::Slider::new(&mut fov, 10.0..=90.0).suffix("°")).changed() {
                    s.camera.fov_y = fov.to_radians();
                }
                ui.end_row();
            });
            ui.separator();
            ui.label(RichText::new("Mouse").strong());
            ui.horizontal(|ui| {
                ui.label("Left-drag on empty space");
                ui.selectable_value(&mut app.settings.left_drag, crate::app::LeftDrag::Orbit, "Orbit")
                    .on_hover_text("3D Builder style: Shift+drag box-selects, right-drag pans");
                ui.selectable_value(&mut app.settings.left_drag, crate::app::LeftDrag::BoxSelect, "Box select")
                    .on_hover_text("CAD style: right-drag orbits, middle-drag pans");
            });
            ui.checkbox(&mut app.settings.drag_moves_objects, "Left-drag on an object moves it");
            ui.separator();
            ui.label(RichText::new("Holes in saved 3MF files").strong());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.settings.holes_cut, false, "Negative parts")
                    .on_hover_text("Bambu Studio and OrcaSlicer keep them as editable negative parts");
                ui.selectable_value(&mut app.settings.holes_cut, true, "Cut")
                    .on_hover_text("Cut into the model: works in every slicer (PrusaSlicer, Cura, …)");
            });
            ui.weak("Either way Simply3D reopens the file with the holes still editable.");
            ui.checkbox(&mut app.settings.snap_objects, "Snap to other objects while moving");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Plate presets");
                for (n, x, y) in
                    [("Bambu X1/P1", 256.0, 256.0), ("Prusa MK4", 250.0, 210.0), ("Ender 3", 220.0, 220.0), ("A1 mini", 180.0, 180.0)]
                {
                    if ui.small_button(n).clicked() {
                        app.settings.plate = glam::Vec2::new(x, y);
                    }
                }
            });
        });
        app.show_settings = open;
    }

    if app.show_shortcuts {
        let mut open = true;
        egui::Window::new("Keyboard & mouse").open(&mut open).resizable(false).show(ctx, |ui| {
            egui::Grid::new("keys").num_columns(2).striped(true).show(ui, |ui| {
                for (k, d) in [
                    ("Left-drag on an object", "Move it (rests on objects under the cursor)"),
                    ("  + Alt / + Shift (while moving)", "Move vertically / lock to X or Y"),
                    ("  + type 25 Enter (while moving)", "Move exactly 25 mm that way (10,5 = X and Y; Esc cancels)"),
                    ("Arrow keys / Page Up, Down", "Nudge the selection one grid step (Shift ×10, Alt ×0.1)"),
                    ("Left-drag on empty space", "Orbit (or box select in CAD mouse mode)"),
                    ("Shift + left-drag", "Box select, adds to selection (drag right: inside, drag left: touching)"),
                    ("Alt + left-drag", "Always orbit"),
                    ("Right / middle drag", "Pan"),
                    ("Wheel", "Zoom to cursor"),
                    ("Click / Shift+click", "Select / add to selection"),
                    ("Double-click", "Focus on point"),
                    ("W / E / R / Q", "Move / rotate / scale / hide gizmo"),
                    ("Ctrl while dragging gizmo", "Toggle snapping"),
                    ("M / L / T", "Measure / lay flat / text tools"),
                    ("Alt+click", "Select one part of a group"),
                    ("Ctrl+G / Ctrl+Shift+G", "Group / ungroup"),
                    ("H / Shift+H", "Hide selected / show all"),
                    ("I", "Isolate selection (again: show the rest)"),
                    ("Ctrl+L", "Lock / unlock (locked objects can't be clicked or moved)"),
                    ("X", "Section view"),
                    ("Enter / Esc", "Apply / cancel a preview"),
                    ("D", "Drop to plate"),
                    ("F / Home", "Frame selection / all"),
                    ("1 / 3 / 7 / 0", "Front / right / top / iso view"),
                    ("O", "Toggle orthographic"),
                    ("Shift+S / Shift+W", "Smooth shading / wireframe"),
                    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                    ("Ctrl+D, Del", "Duplicate, delete"),
                    ("Ctrl+K / F3", "Command palette: search every action"),
                    ("Esc", "Cancel running operation / leave tool / clear selection"),
                    ("In number fields", "20+5, (10+2)*3, 1in, 2.5cm, 45°, *2, /4, 50%"),
                ] {
                    ui.label(RichText::new(k).monospace());
                    ui.label(d);
                    ui.end_row();
                }
            });
        });
        app.show_shortcuts = open;
    }

    if app.show_about {
        let mut open = true;
        egui::Window::new("About Simply3D").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("Simply3D").heading().strong());
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("A fast, open 3D Builder-style mesh editor written in Rust.");
            ui.label("Booleans: boolmesh · UI: egui + wgpu");
            if !app.viewport_ok {
                ui.colored_label(Color32::RED, "GPU renderer unavailable");
            }
        });
        app.show_about = open;
    }
}
