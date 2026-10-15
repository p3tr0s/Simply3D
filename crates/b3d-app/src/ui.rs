//! Panels, menus, dialogs and keyboard shortcuts.

use crate::app::{App, CutKeep, GizmoKind, PosRef, Tool};
use crate::camera::ViewPreset;
use b3d_core::csg::BoolOp;
use b3d_core::primitives::Primitive;
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};
use glam::Vec3;

pub const ACCENT: Color32 = Color32::from_rgb(56, 132, 255);
const AXIS_COLORS: [Color32; 3] = [Color32::from_rgb(235, 80, 80), Color32::from_rgb(90, 200, 90), Color32::from_rgb(80, 140, 255)];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    if dark {
        v.panel_fill = Color32::from_rgb(30, 32, 36);
        v.window_fill = Color32::from_rgb(36, 38, 43);
        v.extreme_bg_color = Color32::from_rgb(22, 23, 26);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 5.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

pub fn shortcuts(app: &mut App, ctx: &egui::Context) {
    let sc = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
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
    if sc(Modifiers::COMMAND, Key::A) {
        app.select_all();
    }
    // single-key shortcuts
    let key = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
    if key(Key::Delete) || key(Key::Backspace) {
        app.delete_selected();
    }
    if key(Key::Escape) {
        if app.tool != Tool::Select {
            app.tool = Tool::Select;
        } else {
            app.selection.clear();
        }
        app.measure.a = None;
        app.measure.b = None;
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
    pub fn frame_all(&mut self) {
        let mut b = self.scene.bounds();
        if b.is_empty() {
            let p = self.settings.plate * 0.5;
            b = b3d_core::Aabb { min: Vec3::new(-p.x, -p.y, 0.0), max: Vec3::new(p.x, p.y, 20.0) };
        }
        self.settings.camera.fit(&b, self.viewport_aspect);
    }
}

pub fn draw(app: &mut App, ui: &mut egui::Ui) {
    menu_bar(app, ui);
    toolbar(app, ui);
    status_bar(app, ui);
    egui::Panel::left("objects").resizable(true).default_size(210.0).show(ui, |ui| object_list(app, ui));
    egui::Panel::right("props").resizable(true).default_size(300.0).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| properties(app, ui));
    });
    egui::CentralPanel::no_frame().show(ui, |ui| crate::viewport::viewport(app, ui));
    dialogs(app, ui.ctx());
}

fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            let ctx = ui.ctx().clone();
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    app.new_scene();
                }
                if ui.add(egui::Button::new("Open / Import…").shortcut_text("Ctrl+O")).clicked() {
                    app.open_dialog(&ctx);
                }
                ui.menu_button("Recent", |ui| {
                    if app.settings.recent.is_empty() {
                        ui.label("(none)");
                    }
                    for p in app.settings.recent.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            app.import_files(vec![p], &ctx);
                        }
                    }
                });
                ui.separator();
                if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                    app.save(&ctx);
                }
                if ui.add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S")).clicked() {
                    app.save_as_dialog(&ctx, false);
                }
                if ui.add_enabled(!app.selection.is_empty(), egui::Button::new("Export selection…")).clicked() {
                    app.save_as_dialog(&ctx, true);
                }
                ui.separator();
                if ui.button("Settings…").clicked() {
                    app.show_settings = true;
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let ul = app.history.undo_label().map(|l| format!("Undo {l}")).unwrap_or_else(|| "Undo".into());
                if ui.add_enabled(app.history.can_undo(), egui::Button::new(ul).shortcut_text("Ctrl+Z")).clicked() {
                    app.undo();
                }
                let rl = app.history.redo_label().map(|l| format!("Redo {l}")).unwrap_or_else(|| "Redo".into());
                if ui.add_enabled(app.history.can_redo(), egui::Button::new(rl).shortcut_text("Ctrl+Y")).clicked() {
                    app.redo();
                }
                ui.separator();
                if ui.add(egui::Button::new("Select all").shortcut_text("Ctrl+A")).clicked() {
                    app.select_all();
                }
                if ui.add(egui::Button::new("Duplicate").shortcut_text("Ctrl+D")).clicked() {
                    app.duplicate_selected();
                }
                if ui.add(egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                    app.delete_selected();
                }
            });
            ui.menu_button("Add", |ui| {
                for name in Primitive::ALL_NAMES {
                    if ui.button(name).clicked() {
                        app.add_dialog = Some(Primitive::default_for(name));
                    }
                }
            });
            ui.menu_button("Object", |ui| {
                let has = !app.selection.is_empty();
                let many = app.selection.len() >= 2;
                if ui.add_enabled(many, egui::Button::new("Union")).clicked() {
                    app.boolean_selected(&ctx, BoolOp::Union);
                }
                if ui.add_enabled(many, egui::Button::new("Subtract")).clicked() {
                    app.boolean_selected(&ctx, BoolOp::Subtract);
                }
                if ui.add_enabled(many, egui::Button::new("Intersect")).clicked() {
                    app.boolean_selected(&ctx, BoolOp::Intersect);
                }
                ui.separator();
                if ui.add_enabled(many, egui::Button::new("Merge (no boolean)")).clicked() {
                    app.merge_selected();
                }
                if ui.add_enabled(has, egui::Button::new("Split into parts")).clicked() {
                    app.split_selected(&ctx);
                }
                if ui.add_enabled(has, egui::Button::new("Cut with plane…")).clicked() {
                    app.start_cut();
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Repair")).clicked() {
                    app.repair_selected(&ctx);
                }
                if ui.add_enabled(has, egui::Button::new("Apply transform")).clicked() {
                    app.bake_selected();
                }
            });
            ui.menu_button("View", |ui| {
                for (n, v) in [
                    ("Front (1)", ViewPreset::Front),
                    ("Back", ViewPreset::Back),
                    ("Left", ViewPreset::Left),
                    ("Right (3)", ViewPreset::Right),
                    ("Top (7)", ViewPreset::Top),
                    ("Bottom", ViewPreset::Bottom),
                    ("Isometric (0)", ViewPreset::Iso),
                ] {
                    if ui.button(n).clicked() {
                        app.settings.camera.set_preset(v);
                    }
                }
                ui.separator();
                ui.checkbox(&mut app.settings.camera.ortho, "Orthographic (O)");
                ui.checkbox(&mut app.settings.show_plate, "Build plate");
                ui.checkbox(&mut app.settings.show_dimensions, "Dimensions");
                ui.checkbox(&mut app.settings.show_problems, "Highlight mesh problems");
                ui.separator();
                if ui.button("Frame selection (F)").clicked() {
                    app.frame_selection();
                }
                if ui.button("Frame all (Home)").clicked() {
                    app.frame_all();
                }
                if ui.checkbox(&mut app.settings.dark, "Dark theme").changed() {
                    apply_theme(ui.ctx(), app.settings.dark);
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard & mouse").clicked() {
                    app.show_shortcuts = true;
                }
                if ui.button("About").clicked() {
                    app.show_about = true;
                }
            });
        });
    });
}

fn tool_button(ui: &mut egui::Ui, active: bool, text: &str, tip: &str) -> egui::Response {
    ui.add(egui::Button::selectable(active, text)).on_hover_text(tip)
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let ctx = ui.ctx().clone();
            if ui.button("📂 Open").on_hover_text("Open / import STL, OBJ, PLY, 3MF (Ctrl+O)").clicked() {
                app.open_dialog(&ctx);
            }
            if ui.button("💾 Save").on_hover_text("Save project as 3MF (Ctrl+S)").clicked() {
                app.save(&ctx);
            }
            ui.separator();
            if ui.add_enabled(app.history.can_undo(), egui::Button::new("⟲ Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                app.undo();
            }
            if ui.add_enabled(app.history.can_redo(), egui::Button::new("⟳ Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
                app.redo();
            }
            ui.separator();
            let sel = app.tool == Tool::Select;
            if tool_button(ui, sel && app.gizmo_kind == GizmoKind::Move, "Move", "Move (W)").clicked() {
                app.tool = Tool::Select;
                app.gizmo_kind = GizmoKind::Move;
            }
            if tool_button(ui, sel && app.gizmo_kind == GizmoKind::Rotate, "⟳ Rotate", "Rotate (E)").clicked() {
                app.tool = Tool::Select;
                app.gizmo_kind = GizmoKind::Rotate;
            }
            if tool_button(ui, sel && app.gizmo_kind == GizmoKind::Scale, "Scale", "Scale (R)").clicked() {
                app.tool = Tool::Select;
                app.gizmo_kind = GizmoKind::Scale;
            }
            if tool_button(ui, app.tool == Tool::Measure, "📏 Measure", "Click two points to measure (M)").clicked() {
                app.tool = Tool::Measure;
            }
            if tool_button(ui, app.tool == Tool::LayFlat, "⬇ Lay flat", "Click a face to put it on the plate (L)").clicked() {
                app.tool = Tool::LayFlat;
            }
            if tool_button(ui, app.tool == Tool::Cut, "✂ Cut", "Cut selection with a plane").clicked() {
                app.start_cut();
            }
            ui.separator();
            let snap_tip = "Snap moves/rotations to the grid. Hold Ctrl while dragging to invert.";
            ui.toggle_value(&mut app.settings.snap_enabled, "Snap").on_hover_text(snap_tip);
            ui.separator();
            let many = app.selection.len() >= 2;
            if ui.add_enabled(many, egui::Button::new("Union")).on_hover_text("Join selected objects into one solid").clicked() {
                app.boolean_selected(&ctx, BoolOp::Union);
            }
            if ui
                .add_enabled(many, egui::Button::new("Subtract"))
                .on_hover_text("Subtract the other selected objects from the first selected")
                .clicked()
            {
                app.boolean_selected(&ctx, BoolOp::Subtract);
            }
            if ui.add_enabled(many, egui::Button::new("Intersect")).on_hover_text("Keep only the overlapping volume").clicked() {
                app.boolean_selected(&ctx, BoolOp::Intersect);
            }
            ui.separator();
            ui.menu_button("➕ Add shape", |ui| {
                for name in Primitive::ALL_NAMES {
                    if ui.button(name).clicked() {
                        app.add_dialog = Some(Primitive::default_for(name));
                    }
                }
            });
        });
    });
}

fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(job) = &app.job {
                ui.spinner();
                ui.label(format!("{} {:.1}s", job.label, job.started.elapsed().as_secs_f32()));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            } else if let Some(s) = &app.status {
                let t = RichText::new(&s.text);
                ui.label(if s.error { t.color(Color32::from_rgb(255, 110, 100)) } else { t });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tris: usize = app.scene.objects.iter().map(|o| o.mesh.mesh.triangles.len()).sum();
                ui.label(format!("{} objects · {} triangles", app.scene.objects.len(), tris));
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

fn object_list(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.heading("Objects");
    ui.separator();
    if app.scene.objects.is_empty() {
        ui.weak("Drop STL / OBJ / PLY / 3MF files here, or use Add shape.");
        return;
    }
    let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut clicked = None;
        let mut toggle_vis = None;
        for o in &app.scene.objects {
            ui.horizontal(|ui| {
                let mut vis = o.visible;
                if ui.checkbox(&mut vis, "").on_hover_text("Visible").changed() {
                    toggle_vis = Some(o.id);
                }
                let [r, g, b, _] = o.color;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(r, g, b));
                let selected = app.selection.contains(&o.id);
                let mut text = RichText::new(&o.name);
                if !o.visible {
                    text = text.weak();
                }
                if ui.selectable_label(selected, text).clicked() {
                    clicked = Some(o.id);
                }
            });
        }
        if let Some(id) = clicked {
            if shift {
                app.toggle_select(id);
            } else {
                app.select_only(id);
            }
        }
        if let Some(id) = toggle_vis {
            app.checkpoint("Visibility");
            if let Some(o) = app.scene.get_mut(id) {
                o.visible = !o.visible;
            }
            app.selection.retain(|s| *s != id);
        }
    });
}

fn axis_drag(ui: &mut egui::Ui, axis: usize, v: &mut f32, speed: f64, suffix: &str, decimals: usize) -> egui::Response {
    ui.label(RichText::new(AXIS_NAMES[axis]).color(AXIS_COLORS[axis]).strong());
    ui.add(egui::DragValue::new(v).speed(speed).suffix(suffix).max_decimals(decimals).min_decimals(decimals.min(2)))
}

fn properties(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.add_space(4.0);
    match app.tool {
        Tool::Measure => measure_panel(app, ui),
        Tool::LayFlat => {
            ui.heading("Lay flat");
            ui.label("Click any face of an object: it is rotated so that face rests on the plate.");
            ui.separator();
        }
        Tool::Cut => cut_panel(app, ui, &ctx),
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
        return;
    };
    let n = app.selection.len();
    let dec = app.settings.decimals;
    ui.heading(if n == 1 { "Object".to_string() } else { format!("{n} objects") });
    ui.separator();

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
        for c in b3d_core::scene::PALETTE {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
            if resp.clicked() {
                app.set_color_selected(c);
                app.pending_edit = None;
            }
        }
    });
    ui.add_space(6.0);

    // ----- transform (primary object) -----
    egui::CollapsingHeader::new("Transform").default_open(true).show(ui, |ui| {
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
            if ui.add(egui::DragValue::new(&mut pct).speed(0.5).suffix(" %").range(1.0..=10000.0)).changed() && (pct - 100.0).abs() > 1e-3 {
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

    // ----- arrange -----
    egui::CollapsingHeader::new("Arrange").default_open(n > 1).show(ui, |ui| {
        ui.add_enabled_ui(n >= 2, |ui| {
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
        ui.separator();
        ui.label("Array copies");
        ui.horizontal(|ui| {
            for a in 0..3 {
                ui.label(RichText::new(AXIS_NAMES[a]).color(AXIS_COLORS[a]));
                ui.add(egui::DragValue::new(&mut app.array.count[a]).range(1..=100).suffix("×"));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Pitch");
            for a in 0..3 {
                ui.add(egui::DragValue::new(&mut app.array.spacing[a]).speed(0.5).suffix(" mm"));
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

    // ----- combine -----
    egui::CollapsingHeader::new("Combine & split").default_open(true).show(ui, |ui| {
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
    });

    // ----- mesh tools -----
    egui::CollapsingHeader::new("Mesh tools").default_open(true).show(ui, |ui| mesh_tools(app, ui, &ctx));

    // ----- analysis & repair -----
    egui::CollapsingHeader::new("Analysis & repair").default_open(true).show(ui, |ui| {
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
            if ui.button("🔧 Repair").on_hover_text("Weld, remove degenerate/duplicate faces, fix orientation, fill holes").clicked() {
                app.repair_selected(&ctx);
            }
            ui.label("Weld tol.");
            ui.add(egui::DragValue::new(&mut app.settings.weld_tolerance).speed(0.0001).range(0.0..=1.0).max_decimals(5).suffix(" mm"));
        });
    });
}

fn mesh_tools(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    use crate::app::SmoothMode;
    use b3d_core::sdf::RoundMode;
    let tris: usize = app.selection.iter().filter_map(|id| app.scene.get(*id)).map(|o| o.mesh.mesh.triangles.len()).sum();
    let t = &mut app.mesh_tools;

    ui.label(RichText::new("Simplify").strong());
    ui.horizontal(|ui| {
        ui.label("Keep");
        ui.add(egui::Slider::new(&mut t.simplify_pct, 1.0..=99.0).suffix(" %").max_decimals(0));
        ui.weak(format!("≈ {} tris", (tris as f32 * t.simplify_pct / 100.0) as usize));
    });
    ui.horizontal(|ui| {
        ui.label("Max deviation");
        ui.add(egui::DragValue::new(&mut t.simplify_err).speed(0.005).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Stop before the surface moves more than this. 0 = no limit");
    });
    let simplify =
        ui.button("Simplify").on_hover_text("Quadric edge-collapse decimation; keeps the mesh closed and sharp edges crisp").clicked();

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
            ui.add_enabled(t.keep_sharp, egui::DragValue::new(&mut t.crease_deg).range(1.0..=179.0).suffix("°"));
        });
    }
    let smooth = ui.button("Smooth").clicked();

    ui.add_space(6.0);
    ui.label(RichText::new("Round edges (bevel)").strong());
    ui.horizontal(|ui| {
        ui.label("Radius");
        ui.add(egui::DragValue::new(&mut t.round_radius).speed(0.05).range(0.05..=100.0).max_decimals(2).suffix(" mm"));
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut t.round_mode, RoundMode::Convex, "Outside").on_hover_text("Round convex edges and corners");
        ui.selectable_value(&mut t.round_mode, RoundMode::Concave, "Inside").on_hover_text("Fill concave edges with a fillet");
        ui.selectable_value(&mut t.round_mode, RoundMode::Both, "Both");
    });
    ui.horizontal(|ui| {
        ui.label("Detail");
        ui.add(egui::DragValue::new(&mut t.round_voxel).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Voxel size. Smaller = more accurate and slower. 0 = automatic");
        if t.round_voxel == 0.0 {
            let b = app.scene.bounds_of(&app.selection);
            if !b.is_empty() {
                ui.weak(format!("auto ≈ {:.2} mm", b3d_core::sdf::auto_voxel(t.round_radius, &b)));
            }
        }
    });
    ui.checkbox(&mut t.round_simplify, "Reduce triangles afterwards");
    let round = ui
        .button("Round edges")
        .on_hover_text("Works on any closed mesh: every edge sharper than the radius becomes a fillet. The surface is rebuilt, so tiny details below the detail size are lost.")
        .clicked();

    if simplify {
        app.simplify_selected(ctx);
    }
    if smooth {
        app.smooth_selected(ctx);
    }
    if round {
        app.round_selected(ctx);
    }
}

fn edge_controls(ui: &mut egui::Ui, edge: &mut b3d_core::primitives::Edge) {
    use b3d_core::primitives::EdgeKind;
    ui.label("Edges");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut edge.kind, EdgeKind::Sharp, "Sharp");
        ui.selectable_value(&mut edge.kind, EdgeKind::Fillet, "Fillet");
        ui.selectable_value(&mut edge.kind, EdgeKind::Chamfer, "Chamfer");
    });
    ui.end_row();
    if edge.kind != EdgeKind::Sharp {
        ui.label(if edge.kind == EdgeKind::Fillet { "Radius" } else { "Chamfer" });
        ui.add(egui::DragValue::new(&mut edge.size).speed(0.1).range(0.0..=1000.0).suffix(" mm"));
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

fn measure_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Measure");
    ui.label("Click two points on objects. Esc to leave.");
    ui.checkbox(&mut app.measure.snap_vertices, "Snap to vertices");
    if let (Some(a), Some(b)) = (app.measure.a, app.measure.b) {
        let d = b - a;
        egui::Grid::new("meas").num_columns(2).show(ui, |ui| {
            ui.label("Distance");
            ui.label(RichText::new(format!("{:.3} mm", d.length())).strong());
            ui.end_row();
            for k in 0..3 {
                ui.label(RichText::new(format!("Δ{}", AXIS_NAMES[k])).color(AXIS_COLORS[k]));
                ui.label(format!("{:.3} mm", d[k].abs()));
                ui.end_row();
            }
        });
        if ui.button("Clear").clicked() {
            app.measure.a = None;
            app.measure.b = None;
        }
    } else if let Some(a) = app.measure.a {
        ui.label(format!("First point: {:.2}, {:.2}, {:.2}", a.x, a.y, a.z));
    }
    ui.separator();
}

fn cut_panel(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
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
    ui.horizontal(|ui| {
        if ui.button(RichText::new("✂ Cut").strong()).clicked() {
            app.cut_selected(ctx);
            app.tool = Tool::Select;
        }
        if ui.button("Cancel").clicked() {
            app.tool = Tool::Select;
        }
    });
    ui.separator();
}

fn dialogs(app: &mut App, ctx: &egui::Context) {
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
                        ui.add(egui::DragValue::new(v).speed(0.2).range(0.01..=10000.0).suffix(" mm"));
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
                            ui.add(egui::DragValue::new(top_radius).speed(0.2).range(0.0..=10000.0).suffix(" mm"));
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
                    }
                });
                ui.add_space(6.0);
                if ui.button(RichText::new("Add to scene").strong()).clicked() {
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
                ui.add(egui::DragValue::new(&mut s.plate.x).range(10.0..=2000.0).suffix(" mm"));
                ui.end_row();
                ui.label("Build plate Y");
                ui.add(egui::DragValue::new(&mut s.plate.y).range(10.0..=2000.0).suffix(" mm"));
                ui.end_row();
                ui.label("Grid spacing");
                ui.add(egui::DragValue::new(&mut s.grid_step).range(0.5..=100.0).suffix(" mm"));
                ui.end_row();
                ui.label("Snap: move");
                ui.add(egui::DragValue::new(&mut s.snap_move).range(0.01..=100.0).suffix(" mm"));
                ui.end_row();
                ui.label("Snap: rotate");
                ui.add(egui::DragValue::new(&mut s.snap_angle_deg).range(0.1..=90.0).suffix("°"));
                ui.end_row();
                ui.label("Snap: scale");
                ui.add(egui::DragValue::new(&mut s.snap_scale).range(0.01..=1.0));
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
                    ("Left drag", "Orbit (on gizmo: transform)"),
                    ("Right / middle drag, Shift+left", "Pan"),
                    ("Wheel", "Zoom to cursor"),
                    ("Click / Shift+click", "Select / add to selection"),
                    ("Double-click", "Focus on point"),
                    ("W / E / R / Q", "Move / rotate / scale / hide gizmo"),
                    ("Ctrl while dragging gizmo", "Toggle snapping"),
                    ("M / L", "Measure / lay flat tools"),
                    ("D", "Drop to plate"),
                    ("F / Home", "Frame selection / all"),
                    ("1 / 3 / 7 / 0", "Front / right / top / iso view"),
                    ("O", "Toggle orthographic"),
                    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                    ("Ctrl+D, Del", "Duplicate, delete"),
                    ("Esc", "Leave tool / clear selection"),
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
        egui::Window::new("About Builder3D").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("Builder3D").heading().strong());
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
