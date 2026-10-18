//! Every action in one registry, searchable from the command palette (Ctrl+K).

use crate::app::{App, GizmoKind, PanelTab, Tool};
use crate::camera::ViewPreset;
use crate::preview::PreviewKind;
use crate::text_tool::TextMode;
use egui::{Color32, Key, Modifiers, RichText};
use s3d_core::csg::BoolOp;
use s3d_core::primitives::{Edge, EdgeKind, Primitive};

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub keys: &'static str,
    pub enabled: fn(&App) -> bool,
    pub run: fn(&mut App, &egui::Context),
}

fn always(_: &App) -> bool {
    true
}
fn has_sel(a: &App) -> bool {
    !a.selection.is_empty()
}
fn two_sel(a: &App) -> bool {
    a.selection.len() >= 2
}
fn three_sel(a: &App) -> bool {
    a.selection.len() >= 3
}
fn has_objects(a: &App) -> bool {
    !a.scene.objects.is_empty()
}

macro_rules! cmd {
    ($group:expr, $name:expr, $keys:expr, $en:expr, $run:expr) => {
        Command { group: $group, name: $name, keys: $keys, enabled: $en, run: $run }
    };
}

pub fn all() -> Vec<Command> {
    let mut v = vec![
        // file
        cmd!("File", "New plate", "Ctrl+N", always, |a, _| a.request_new_plate()),
        cmd!("File", "Open / import…", "Ctrl+O", always, |a, c| a.open_dialog(c)),
        cmd!("File", "Save", "Ctrl+S", has_objects, |a, c| a.save(c)),
        cmd!("File", "Save as…", "Ctrl+Shift+S", has_objects, |a, c| a.save_as_dialog(c, false)),
        cmd!("File", "Export selection…", "", has_sel, |a, c| a.save_as_dialog(c, true)),
        cmd!("File", "Settings…", "", always, |a, _| a.show_settings = true),
        // edit
        cmd!("Edit", "Undo", "Ctrl+Z", |a| a.history.can_undo(), |a, _| a.undo()),
        cmd!("Edit", "Redo", "Ctrl+Y", |a| a.history.can_redo(), |a, _| a.redo()),
        cmd!("Edit", "Select all", "Ctrl+A", has_objects, |a, _| a.select_all()),
        cmd!("Edit", "Deselect", "Esc", has_sel, |a, _| a.selection.clear()),
        cmd!("Edit", "Duplicate", "Ctrl+D", has_sel, |a, _| a.duplicate_selected()),
        cmd!("Edit", "Delete", "Del", has_sel, |a, _| a.delete_selected()),
        // transform
        cmd!("Transform", "Move tool", "W", always, |a, _| {
            a.tool = Tool::Select;
            a.gizmo_kind = GizmoKind::Move
        }),
        cmd!("Transform", "Rotate tool", "E", always, |a, _| {
            a.tool = Tool::Select;
            a.gizmo_kind = GizmoKind::Rotate
        }),
        cmd!("Transform", "Scale tool", "R", always, |a, _| {
            a.tool = Tool::Select;
            a.gizmo_kind = GizmoKind::Scale
        }),
        cmd!("Transform", "Hide gizmo", "Q", always, |a, _| a.gizmo_kind = GizmoKind::Off),
        cmd!("Transform", "Toggle grid snapping", "", always, |a, _| a.settings.snap_enabled = !a.settings.snap_enabled),
        cmd!("Transform", "Toggle object snapping", "", always, |a, _| a.settings.snap_objects = !a.settings.snap_objects),
        cmd!("View", "Mouse: 3D Builder style (left-drag orbits)", "", always, |a, _| a.settings.left_drag = crate::app::LeftDrag::Orbit),
        cmd!("View", "Mouse: CAD style (left-drag box-selects)", "", always, |a, _| a.settings.left_drag = crate::app::LeftDrag::BoxSelect),
        cmd!("Transform", "Drop to plate", "D", has_sel, |a, _| a.drop_selected()),
        cmd!("Transform", "Center on plate", "", has_sel, |a, _| a.center_selected_on_plate()),
        cmd!("Transform", "Move back onto the plate", "", has_sel, |a, _| {
            let ids = a.selection.clone();
            a.fit_onto_plate(&ids)
        }),
        cmd!("Transform", "Lay flat on face", "L", always, |a, _| a.tool = Tool::LayFlat),
        cmd!("Transform", "Reset rotation & scale", "", has_sel, |a, _| a.reset_transform_selected()),
        cmd!("Transform", "Apply transform", "", has_sel, |a, _| a.bake_selected()),
        cmd!("Transform", "Rotate 90° around X", "", has_sel, |a, _| a.rotate_selection(0, 90.0)),
        cmd!("Transform", "Rotate 90° around Y", "", has_sel, |a, _| a.rotate_selection(1, 90.0)),
        cmd!("Transform", "Rotate 90° around Z", "", has_sel, |a, _| a.rotate_selection(2, 90.0)),
        cmd!("Transform", "Mirror X", "", has_sel, |a, _| a.mirror_selected(0)),
        cmd!("Transform", "Mirror Y", "", has_sel, |a, _| a.mirror_selected(1)),
        cmd!("Transform", "Mirror Z", "", has_sel, |a, _| a.mirror_selected(2)),
        // arrange
        cmd!("Arrange", "Align left (X min)", "", two_sel, |a, _| a.align(0, 0)),
        cmd!("Arrange", "Align centres X", "", two_sel, |a, _| a.align(0, 1)),
        cmd!("Arrange", "Align right (X max)", "", two_sel, |a, _| a.align(0, 2)),
        cmd!("Arrange", "Align front (Y min)", "", two_sel, |a, _| a.align(1, 0)),
        cmd!("Arrange", "Align centres Y", "", two_sel, |a, _| a.align(1, 1)),
        cmd!("Arrange", "Align back (Y max)", "", two_sel, |a, _| a.align(1, 2)),
        cmd!("Arrange", "Align bottoms (Z min)", "", two_sel, |a, _| a.align(2, 0)),
        cmd!("Arrange", "Align centres Z", "", two_sel, |a, _| a.align(2, 1)),
        cmd!("Arrange", "Align tops (Z max)", "", two_sel, |a, _| a.align(2, 2)),
        cmd!("Arrange", "Auto-orient for printing (least overhang)", "", has_objects, |a, _| a.auto_orient_selected()),
        cmd!("Arrange", "Auto-arrange on the plate", "A", has_objects, |a, _| {
            let g = a.settings.arrange_gap;
            a.auto_arrange(g)
        }),
        cmd!("Arrange", "Distribute along X", "", three_sel, |a, _| a.distribute(0)),
        cmd!("Arrange", "Distribute along Y", "", three_sel, |a, _| a.distribute(1)),
        cmd!("Arrange", "Distribute along Z", "", three_sel, |a, _| a.distribute(2)),
        cmd!("Arrange", "Array copies…", "", has_sel, |a, _| a.tab = PanelTab::Arrange),
        // combine
        cmd!("Combine", "Union", "", two_sel, |a, c| a.boolean_selected(c, BoolOp::Union)),
        cmd!("Combine", "Subtract", "", two_sel, |a, c| a.boolean_selected(c, BoolOp::Subtract)),
        cmd!("Combine", "Intersect", "", two_sel, |a, c| a.boolean_selected(c, BoolOp::Intersect)),
        cmd!("Combine", "Merge (no boolean)", "", two_sel, |a, _| a.merge_selected()),
        cmd!("Combine", "Split into parts", "", has_sel, |a, c| a.split_selected(c)),
        cmd!("Combine", "Cut with plane…", "", has_sel, |a, _| a.start_cut()),
        // mesh
        cmd!("Mesh", "Simplify (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::Simplify)
        }),
        cmd!("Mesh", "Smooth (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::Smooth)
        }),
        cmd!("Mesh", "Round edges / bevel (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::Round)
        }),
        cmd!("Mesh", "Make solid: rebuild broken / open meshes as one closed solid (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::MakeSolid)
        }),
        cmd!("Mesh", "Remesh: even triangles (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.mesh_tools.solid_adaptive = false;
            a.start_preview(PreviewKind::MakeSolid)
        }),
        cmd!("Mesh", "Pattern / lattice: wireframe, Voronoi, gyroid (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::Pattern)
        }),
        cmd!("Mesh", "Hollow: shell with wall thickness (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_preview(PreviewKind::Hollow)
        }),
        cmd!("Mesh", "Extrude down to the plate (preview)", "", has_sel, |a, _| {
            a.tab = PanelTab::Mesh;
            a.start_extrude_down()
        }),
        cmd!("Mesh", "Apply preview", "Enter", |a| a.preview.is_some(), |a, _| a.apply_preview()),
        cmd!("Mesh", "Cancel preview", "Esc", |a| a.preview.is_some(), |a, _| a.cancel_preview()),
        cmd!("Mesh", "Repair", "", has_sel, |a, c| {
            a.tab = PanelTab::Repair;
            a.repair_selected(c)
        }),
        cmd!("Mesh", "Analyse mesh", "", has_sel, |a, _| a.tab = PanelTab::Repair),
        cmd!("Mesh", "Mesh tools…", "", has_sel, |a, _| a.tab = PanelTab::Mesh),
        // view
        cmd!("View", "Front view", "1", always, |a, _| a.settings.camera.set_preset(ViewPreset::Front)),
        cmd!("View", "Back view", "", always, |a, _| a.settings.camera.set_preset(ViewPreset::Back)),
        cmd!("View", "Left view", "", always, |a, _| a.settings.camera.set_preset(ViewPreset::Left)),
        cmd!("View", "Right view", "3", always, |a, _| a.settings.camera.set_preset(ViewPreset::Right)),
        cmd!("View", "Top view", "7", always, |a, _| a.settings.camera.set_preset(ViewPreset::Top)),
        cmd!("View", "Bottom view", "", always, |a, _| a.settings.camera.set_preset(ViewPreset::Bottom)),
        cmd!("View", "Isometric view", "0", always, |a, _| a.settings.camera.set_preset(ViewPreset::Iso)),
        cmd!("View", "Toggle orthographic", "O", always, |a, _| a.settings.camera.ortho = !a.settings.camera.ortho),
        cmd!("View", "Smooth shading on / off", "Shift+S", always, |a, _| a.settings.smooth_shading = !a.settings.smooth_shading),
        cmd!("View", "Wireframe on / off", "Shift+W", always, |a, _| a.settings.wireframe = !a.settings.wireframe),
        cmd!("View", "Frame selection", "F", always, |a, _| a.frame_selection()),
        cmd!("View", "Frame all", "Home", always, |a, _| a.frame_all()),
        cmd!("View", "Toggle see-through for overlapping objects", "", always, |a, _| a.settings.see_through = !a.settings.see_through),
        cmd!("View", "Toggle build plate", "", always, |a, _| a.settings.show_plate = !a.settings.show_plate),
        cmd!("View", "Toggle dimensions", "", always, |a, _| a.settings.show_dimensions = !a.settings.show_dimensions),
        cmd!("View", "Toggle dark theme", "", always, |a, c| {
            a.settings.dark = !a.settings.dark;
            crate::ui::apply_theme(c, a.settings.dark)
        }),
        cmd!("Tools", "Measure", "M", always, |a, _| a.tool = Tool::Measure),
        cmd!("Tools", "Text: add 3D text", "T", always, |a, _| {
            a.settings.text.mode = TextMode::Object;
            a.start_text_tool()
        }),
        cmd!("Tools", "Text: emboss onto a face", "", has_objects, |a, _| {
            a.settings.text.mode = TextMode::Emboss;
            a.start_text_tool()
        }),
        cmd!("Tools", "Text: engrave into a face", "", has_objects, |a, _| {
            a.settings.text.mode = TextMode::Engrave;
            a.start_text_tool()
        }),
        cmd!("Tools", "Picture to 3D (contour, height map, stamp, lithophane)…", "", always, |a, c| a.image_dialog(c)),
        cmd!("Tools", "Paint colours (multi-colour / AMS)", "P", has_objects, |a, _| a.start_paint_tool()),
        cmd!("Tools", "Select faces: extrude, push in, delete, copy as solid", "S", has_objects, |a, _| a.start_faces_tool()),
        cmd!("Tools", "Sculpt: draw, inflate, smooth, flatten, grab", "B", has_objects, |a, _| a.start_sculpt_tool()),
        cmd!("Tools", "Texture: knurl, carbon fibre, wood, leather, picture… pressed into the surface", "", has_objects, |a, _| a
            .start_texture_tool()),
        cmd!("Tools", "Paint: clear paint from selection", "", has_sel, |a, _| a.clear_paint_selected()),
        cmd!("Tools", "Paint: refine selection for finer painting", "", has_sel, |a, c| a.refine_for_paint(c)),
        cmd!("View", "Section view", "X", has_objects, |a, _| a.toggle_section()),
        // organise
        cmd!("Organise", "Group", "Ctrl+G", two_sel, |a, _| a.group_selected()),
        cmd!("Combine", "Make hole (negative object)", "", has_sel, |a, _| a.set_hole_selected(true)),
        cmd!("Combine", "Make solid", "", has_sel, |a, _| a.set_hole_selected(false)),
        cmd!("Combine", "Apply holes (make the cut permanent)", "", |a| a.scene.objects.iter().any(|o| o.hole), |a, c| a
            .apply_holes_selected(c)),
        cmd!("Organise", "Ungroup", "Ctrl+Shift+G", |a| a.can_ungroup(), |a, _| a.ungroup_selected()),
        cmd!("Organise", "Hide selected", "H", has_sel, |a, _| a.hide_selected()),
        cmd!("Organise", "Show all", "Shift+H", |a| a.any_hidden(), |a, _| a.show_all()),
        cmd!("Organise", "Isolate selection (show only it)", "I", |a| has_sel(a) || a.isolated.is_some(), |a, _| a.toggle_isolate()),
        cmd!("Organise", "Lock / unlock selected", "Ctrl+L", has_sel, |a, _| a.toggle_lock_selected()),
        cmd!("Organise", "Unlock everything", "", |a| a.scene.objects.iter().any(|o| o.locked), |a, _| {
            let ids: Vec<_> = a.scene.objects.iter().filter(|o| o.locked).map(|o| o.id).collect();
            a.set_locked(&ids, false)
        }),
        // help
        cmd!("Help", "Keyboard & mouse", "", always, |a, _| a.show_shortcuts = true),
        cmd!("Help", "About", "", always, |a, _| a.show_about = true),
        cmd!("Help", "Input diagnostics (debug)", "F12", always, |a, _| a.debug.toggle()),
    ];
    // add-shape entries
    for (name, label) in [
        ("Box", "Add box"),
        ("Cylinder", "Add cylinder"),
        ("Cone", "Add cone"),
        ("Sphere", "Add sphere"),
        ("Torus", "Add torus"),
        ("Tube", "Add tube"),
        ("Pyramid", "Add pyramid"),
        ("Wedge", "Add wedge"),
    ] {
        v.push(Command { group: "Add", name: label, keys: "", enabled: always, run: add_fn(name) });
    }
    v.push(cmd!("Add", "Add bolt (metric thread)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Bolt"))));
    v.push(cmd!("Add", "Add gear (involute spur gear)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Gear"))));
    v.push(cmd!("Add", "Add box with lid (enclosure, dividers)", "", always, |a, _| a.add_dialog =
        Some(Primitive::default_for("Box with lid"))));
    v.push(cmd!("Add", "Add Gridfinity bin", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Gridfinity bin"))));
    v.push(cmd!("Add", "Add Gridfinity baseplate", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Gridfinity baseplate"))));
    v.push(cmd!("Add", "Add brick (LEGO-compatible)", "", always, |a, _| a.add_dialog =
        Some(Primitive::default_for("Brick (LEGO-compatible)"))));
    v.push(cmd!("Add", "Add plate (LEGO-compatible)", "", always, |a, _| a.add_dialog =
        Some(Primitive::default_for("Plate (LEGO-compatible)"))));
    v.push(
        cmd!("Add", "Add tile (LEGO-compatible)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Tile (LEGO-compatible)")))
    );
    v.push(cmd!("Add", "Add brick (DUPLO-compatible)", "", always, |a, _| a.add_dialog =
        Some(Primitive::default_for("Brick (DUPLO-compatible)"))));
    v.push(cmd!("Add", "Add spring (compression)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Spring"))));
    v.push(cmd!("Add", "Add hinge (print-in-place)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Hinge"))));
    v.push(cmd!("Add", "Add snap-fit clip", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Snap-fit clip"))));
    v.push(cmd!("Add", "Add nut", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Nut"))));
    v.push(cmd!("Add", "Add threaded rod", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Threaded rod"))));
    v.push(cmd!("Add", "Add threaded hole (tapped hole)", "", always, |a, _| a.add_dialog = Some(Primitive::default_for("Threaded hole"))));
    v.push(cmd!("Add", "Add rounded box", "", always, |a, _| {
        a.add_dialog = Some(Primitive::Box { size: glam::Vec3::splat(20.0), edge: Edge { kind: EdgeKind::Fillet, ..Default::default() } })
    }));
    v.push(cmd!("Add", "Add chamfered box", "", always, |a, _| {
        a.add_dialog = Some(Primitive::Box { size: glam::Vec3::splat(20.0), edge: Edge { kind: EdgeKind::Chamfer, ..Default::default() } })
    }));
    v
}

fn add_fn(name: &str) -> fn(&mut App, &egui::Context) {
    match name {
        "Cylinder" => |a, _| a.add_dialog = Some(Primitive::default_for("Cylinder")),
        "Cone" => |a, _| a.add_dialog = Some(Primitive::default_for("Cone")),
        "Sphere" => |a, _| a.add_dialog = Some(Primitive::default_for("Sphere")),
        "Torus" => |a, _| a.add_dialog = Some(Primitive::default_for("Torus")),
        "Tube" => |a, _| a.add_dialog = Some(Primitive::default_for("Tube")),
        "Pyramid" => |a, _| a.add_dialog = Some(Primitive::default_for("Pyramid")),
        "Wedge" => |a, _| a.add_dialog = Some(Primitive::default_for("Wedge")),
        _ => |a, _| a.add_dialog = Some(Primitive::default_for("Box")),
    }
}

/// Fuzzy subsequence score; None = no match. Rewards word starts, runs and substrings.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev = usize::MAX;
    let mut first = usize::MAX;
    for qc in q {
        loop {
            if ti >= t.len() {
                return None;
            }
            if t[ti] == qc {
                score += 1;
                if prev != usize::MAX && ti == prev + 1 {
                    score += 5;
                }
                if ti == 0 || !t[ti - 1].is_alphanumeric() {
                    score += 8;
                }
                if first == usize::MAX {
                    first = ti;
                }
                prev = ti;
                ti += 1;
                break;
            }
            ti += 1;
        }
    }
    let ql = query.to_lowercase();
    if text.to_lowercase().contains(ql.trim()) {
        score += 20;
    }
    // compact matches beat letters scattered across the name
    let span = (prev - first) as i32;
    Some(score - span - t.len() as i32 / 8)
}

pub struct Palette {
    pub query: String,
    pub selected: usize,
    pub focus: bool,
}

impl Palette {
    pub fn new() -> Self {
        Palette { query: String::new(), selected: 0, focus: true }
    }
}

/// Draw the palette (if open). Returns after running at most one command.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut pal) = app.palette.take() else { return };
    let cmds = all();
    // rank
    let mut ranked: Vec<(i32, usize)> = cmds
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let s = score(&pal.query, c.name).or_else(|| score(&pal.query, &format!("{} {}", c.group, c.name)).map(|s| s - 10))?;
            let recent_bonus = app.settings.recent_commands.iter().position(|r| r == c.name).map(|p| 30 - p as i32 * 3).unwrap_or(0);
            let enabled_bonus = if (c.enabled)(app) { 15 } else { 0 };
            Some((s + recent_bonus + enabled_bonus, i))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.truncate(14);
    if pal.selected >= ranked.len() {
        pal.selected = ranked.len().saturating_sub(1);
    }

    // keyboard (consumed before the text field sees it)
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        return; // closed
    }
    if down && !ranked.is_empty() {
        pal.selected = (pal.selected + 1) % ranked.len();
    }
    if up && !ranked.is_empty() {
        pal.selected = (pal.selected + ranked.len() - 1) % ranked.len();
    }
    let mut run: Option<usize> = if enter { ranked.get(pal.selected).map(|r| r.1) } else { None };
    let mut close = false;

    egui::Window::new("Command palette")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_size([480.0, 0.0])
        .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
        .show(ctx, |ui| {
            let te = ui.add(
                egui::TextEdit::singleline(&mut pal.query)
                    .hint_text("Type a command…  (↑↓ to pick, Enter to run, Esc to close)")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Heading),
            );
            if pal.focus {
                te.request_focus();
                pal.focus = false;
            }
            if te.changed() {
                pal.selected = 0;
            }
            ui.separator();
            if ranked.is_empty() {
                ui.weak("No matching command");
            }
            for (row, (_, i)) in ranked.iter().enumerate() {
                let c = &cmds[*i];
                let enabled = (c.enabled)(app);
                let selected = row == pal.selected;
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_width(ui.available_width());
                        let mut name = RichText::new(c.name);
                        if !enabled {
                            name = name.weak();
                        }
                        let r = ui.add(egui::Button::selectable(selected, name).frame_when_inactive(false));
                        ui.weak(c.group);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !c.keys.is_empty() {
                                ui.label(RichText::new(c.keys).monospace().color(Color32::GRAY));
                            }
                        });
                        r
                    })
                    .inner;
                if resp.clicked() {
                    run = Some(*i);
                }
                if resp.hovered() {
                    pal.selected = row;
                }
            }
            if ranked.iter().any(|(_, i)| !(cmds[*i].enabled)(app)) {
                ui.add_space(2.0);
                ui.weak("Greyed-out commands need a (larger) selection.");
            }
        });
    // click outside closes
    if ctx.input(|i| i.pointer.any_pressed()) && !ctx.is_pointer_over_egui() {
        close = true;
    }
    if let Some(i) = run {
        let c = &cmds[i];
        if (c.enabled)(app) {
            app.settings.recent_commands.retain(|r| r != c.name);
            app.settings.recent_commands.insert(0, c.name.to_string());
            app.settings.recent_commands.truncate(10);
            (c.run)(app, ctx);
            return; // closed after running
        } else {
            app.error(format!("\"{}\" needs a selection", c.name));
        }
    }
    if !close {
        app.palette = Some(pal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy() {
        assert!(score("rnd", "Round edges / bevel").is_some());
        assert!(score("xyz", "Round edges").is_none());
        assert!(score("sub", "Subtract").unwrap() > score("sub", "Rounded tube").unwrap_or(-100));
        assert!(score("cyl", "Add cylinder").is_some());
        assert!(score("rnd", "Round edges / bevel").unwrap() > score("rnd", "Rotate 90° around X").unwrap());
        assert!(score("mir", "Mirror X").unwrap() > score("mir", "Move tool").unwrap_or(-100));
    }

    #[test]
    fn names_unique() {
        let c = all();
        let mut names: Vec<_> = c.iter().map(|c| c.name).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }
}
