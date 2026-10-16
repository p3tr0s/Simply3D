//! The right-click menu, shared by the viewport and the object list.

use crate::app::{App, PanelTab, Tool};
use crate::preview::PreviewKind;
use crate::text_tool::TextMode;
use b3d_core::csg::BoolOp;
use b3d_core::primitives::Primitive;
use egui::{Color32, RichText};

/// Menu entry that closes the menu when clicked.
fn item(ui: &mut egui::Ui, enabled: bool, label: &str, keys: &str) -> bool {
    let clicked = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(keys)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

pub fn object_menu(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.set_min_width(235.0);
    let n = app.selection.len();
    if n == 0 {
        scene_menu(app, ui);
        return;
    }
    let many = n >= 2;
    // what this menu is about
    let title = match (app.selected_group(), n) {
        (Some(g), _) => format!("🗀 {}", app.scene.group_name(g).unwrap_or("Group")),
        (None, 1) => app.scene.get(app.selection[0]).map(|o| o.name.clone()).unwrap_or_default(),
        _ => format!("{n} objects"),
    };
    ui.label(RichText::new(title).strong());
    ui.separator();

    if item(ui, true, "Duplicate", "Ctrl+D") {
        app.duplicate_selected();
    }
    if item(ui, true, "Delete", "Del") {
        app.delete_selected();
    }
    ui.separator();
    if item(ui, many, "Group", "Ctrl+G") {
        app.group_selected();
    }
    if item(ui, app.can_ungroup(), "Ungroup", "Ctrl+Shift+G") {
        app.ungroup_selected();
    }
    if item(ui, true, "Hide", "H") {
        app.hide_selected();
    }
    if item(ui, true, if app.isolated.is_some() { "Show the others" } else { "Isolate" }, "I") {
        app.toggle_isolate();
    }
    let all_holes = app.selection.iter().all(|id| app.scene.get(*id).is_some_and(|o| o.hole));
    if item(ui, true, if all_holes { "Make solid" } else { "Make hole" }, "") {
        app.set_hole_selected(!all_holes);
    }
    let has_cuts = app.any_hole_selected() || app.selection.iter().any(|id| app.holes.cuts.contains_key(id));
    if item(ui, has_cuts, "Apply holes (make permanent)", "") {
        app.apply_holes_selected(ctx);
    }
    let locked = app.any_locked_selected();
    if item(ui, true, if locked { "Unlock" } else { "Lock" }, "Ctrl+L") {
        if locked {
            let ids = app.selection.clone();
            app.set_locked(&ids, false);
        } else {
            app.toggle_lock_selected();
        }
    }
    ui.separator();

    ui.menu_button("Colour", |ui| {
        ui.horizontal(|ui| {
            for c in b3d_core::scene::PALETTE {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                if resp.clicked() {
                    app.set_color_selected(c);
                    app.pending_edit = None;
                    ui.close();
                }
            }
        });
    });
    ui.menu_button("Position & rotation", |ui| {
        if item(ui, true, "Drop to plate", "D") {
            app.drop_selected();
        }
        if item(ui, true, "Center on plate", "") {
            app.center_selected_on_plate();
        }
        if item(ui, true, "Lay flat on a face…", "L") {
            app.tool = Tool::LayFlat;
        }
        ui.separator();
        for (a, name) in ["X", "Y", "Z"].iter().enumerate() {
            if item(ui, true, &format!("Rotate 90° around {name}"), "") {
                app.rotate_selection(a, 90.0);
            }
        }
        ui.separator();
        for (a, name) in ["X", "Y", "Z"].iter().enumerate() {
            if item(ui, true, &format!("Mirror {name}"), "") {
                app.mirror_selected(a);
            }
        }
        ui.separator();
        if item(ui, true, "Reset rotation & scale", "") {
            app.reset_transform_selected();
        }
        if item(ui, true, "Apply transform", "") {
            app.bake_selected();
        }
    });
    ui.menu_button("Combine", |ui| {
        if item(ui, many, "Union", "") {
            app.boolean_selected(ctx, BoolOp::Union);
        }
        if item(ui, many, "Subtract", "") {
            app.boolean_selected(ctx, BoolOp::Subtract);
        }
        if item(ui, many, "Intersect", "") {
            app.boolean_selected(ctx, BoolOp::Intersect);
        }
        if item(ui, many, "Merge (no boolean)", "") {
            app.merge_selected();
        }
        ui.separator();
        if item(ui, true, "Split into parts", "") {
            app.split_selected(ctx);
        }
        if item(ui, true, "Cut with plane…", "") {
            app.start_cut();
        }
    });
    ui.menu_button("Mesh", |ui| {
        for (label, kind) in [
            ("Simplify…", PreviewKind::Simplify),
            ("Smooth…", PreviewKind::Smooth),
            ("Round edges…", PreviewKind::Round),
            ("Extrude down…", PreviewKind::ExtrudeDown),
            ("Hollow…", PreviewKind::Hollow),
            ("Make solid / remesh…", PreviewKind::MakeSolid),
            ("Pattern / lattice…", PreviewKind::Pattern),
        ] {
            if item(ui, true, label, "") {
                app.tab = PanelTab::Mesh;
                if kind == PreviewKind::ExtrudeDown {
                    app.start_extrude_down();
                } else {
                    app.start_preview(kind);
                }
            }
        }
        ui.separator();
        if item(ui, true, "Repair", "") {
            app.tab = PanelTab::Repair;
            app.repair_selected(ctx);
        }
    });
    if item(ui, true, "Emboss / engrave text…", "") {
        app.settings.text.mode = TextMode::Emboss;
        app.start_text_tool();
    }
    if item(ui, true, "Paint colours…", "P") {
        app.start_paint_tool();
    }
    if item(ui, true, "Select faces…", "S") {
        app.start_faces_tool();
    }
    if item(ui, true, "Texture…", "") {
        app.start_texture_tool();
    }
    ui.separator();
    if item(ui, true, "Frame", "F") {
        app.frame_selection();
    }
    if item(ui, true, "Export selection…", "") {
        app.save_as_dialog(ctx, true);
    }
}

/// Menu for empty space (nothing selected).
fn scene_menu(app: &mut App, ui: &mut egui::Ui) {
    let has_objects = !app.scene.objects.is_empty();
    if item(ui, has_objects, "Select all", "Ctrl+A") {
        app.select_all();
    }
    if item(ui, app.any_hidden(), "Show all", "Shift+H") {
        app.show_all();
    }
    ui.separator();
    ui.menu_button("Add shape", |ui| {
        for name in Primitive::ALL_NAMES {
            if item(ui, true, name, "") {
                app.add_dialog = Some(Primitive::default_for(name));
            }
        }
        ui.separator();
        for name in Primitive::FASTENER_NAMES {
            if item(ui, true, name, "") {
                app.add_dialog = Some(Primitive::default_for(name));
            }
        }
    });
    if item(ui, true, "Add text…", "T") {
        app.settings.text.mode = TextMode::Object;
        app.start_text_tool();
    }
    if item(ui, true, "Picture to 3D…", "") {
        app.image_dialog(ui.ctx());
    }
    ui.separator();
    if item(ui, true, "Frame all", "Home") {
        app.frame_all();
    }
    if item(ui, has_objects, "Section view", "X") {
        app.toggle_section();
    }
}
