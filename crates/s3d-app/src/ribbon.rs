//! 3D Builder–style chrome: a row of tabs (Insert, Object, Edit, Surface, View, Help) with a
//! ribbon of big icon buttons under it, the object box that floats at the bottom of the
//! view while something is selected, the start card on an empty plate, and the notices in
//! the bottom-right corner.

use crate::app::{App, GizmoKind, Tool};
use crate::camera::ViewPreset;
use crate::icons;
use crate::preview::PreviewKind;
use crate::ui::ACCENT;
use egui::{Align, Align2, Color32, FontId, Layout, Rect, Response, RichText, Sense, Ui, Vec2};
use s3d_core::csg::BoolOp;
use s3d_core::primitives::Primitive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RibbonTab {
    #[default]
    Insert,
    Object,
    Edit,
    /// Paint, faces, texture and sculpt (was "Paint").
    #[serde(alias = "Paint")]
    Surface,
    View,
    Help,
}

impl RibbonTab {
    const ALL: [RibbonTab; 6] = [RibbonTab::Insert, RibbonTab::Object, RibbonTab::Edit, RibbonTab::Surface, RibbonTab::View, RibbonTab::Help];
    fn name(self) -> &'static str {
        match self {
            RibbonTab::Insert => "Insert",
            RibbonTab::Object => "Object",
            RibbonTab::Edit => "Edit",
            RibbonTab::Surface => "Surface",
            RibbonTab::View => "View",
            RibbonTab::Help => "Help",
        }
    }
}

fn text_color(ui: &Ui) -> Color32 {
    ui.visuals().text_color()
}

fn weak(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// A big ribbon button: icon above a short label. `menu` adds a small arrow (it opens a list).
fn big(ui: &mut Ui, icon: &str, label: &str, enabled: bool, active: bool, menu: bool) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, Color32::PLACEHOLDER);
    let w = (galley.size().x + if menu { 24.0 } else { 14.0 }).max(50.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 56.0), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = enabled && resp.hovered();
        if active {
            p.rect_filled(rect, 6.0, ACCENT.gamma_multiply(0.16));
            p.rect_stroke(rect, 6.0, egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Inside);
        } else if hovered {
            p.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let col = if !enabled {
            weak(ui).gamma_multiply(0.45)
        } else if active {
            ACCENT
        } else {
            text_color(ui)
        };
        let icon_rect = Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 18.0), Vec2::splat(24.0));
        icons::image(icon, 24.0, col).paint_at(ui, icon_rect);
        let tw = galley.size().x + if menu { 10.0 } else { 0.0 };
        let tp = egui::pos2(rect.center().x - tw * 0.5, rect.top() + 35.0);
        let th = galley.size().y;
        ui.painter().galley(tp, galley, col);
        if menu {
            let c = egui::pos2(tp.x + tw - 3.5, tp.y + th * 0.5 + 1.0);
            ui.painter().add(egui::Shape::convex_polygon(
                vec![c + Vec2::new(-3.5, -2.0), c + Vec2::new(3.5, -2.0), c + Vec2::new(0.0, 2.0)],
                col,
                egui::Stroke::NONE,
            ));
        }
    }
    resp
}

/// A group of ribbon buttons with its caption underneath, then a divider.
fn group(ui: &mut Ui, caption: &str, add: impl FnOnce(&mut Ui)) {
    let r = ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui);
        });
        ui.add_space(-2.0);
        let w = ui.min_rect().width();
        ui.allocate_ui_with_layout(Vec2::new(w, 14.0), Layout::top_down(Align::Center), |ui| {
            ui.label(RichText::new(caption).size(10.5).color(weak(ui)));
        });
    });
    let x = r.response.rect.right() + 7.0;
    let y = r.response.rect.y_range();
    ui.add_space(14.0);
    ui.painter().vline(x, (y.min + 6.0)..=(y.max - 4.0), egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color));
}

/// Small square icon button (tab row, object box).
fn icon_button(ui: &mut Ui, icon: &str, tip: &str, enabled: bool, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if active {
            p.rect_filled(rect, 6.0, ACCENT.gamma_multiply(0.18));
        } else if enabled && resp.hovered() {
            p.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let col = if !enabled {
            weak(ui).gamma_multiply(0.45)
        } else if active {
            ACCENT
        } else {
            text_color(ui)
        };
        icons::image(icon, 18.0, col).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(18.0)));
    }
    resp.on_hover_text(tip)
}

fn add_menu(ui: &mut Ui, app: &mut App, names: &[&str]) {
    for name in names {
        if ui.button(*name).clicked() {
            app.add_dialog = Some(Primitive::default_for(name));
        }
    }
}

/// The tab row and the ribbon under it.
pub fn top(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    // a tool started from the keyboard, a menu or the command search brings up its tab
    if app.tool != app.ribbon_tool {
        app.ribbon_tool = app.tool;
        app.ribbon = match app.tool {
            Tool::Select => app.ribbon,
            Tool::Text | Tool::Image => RibbonTab::Insert,
            Tool::Measure | Tool::LayFlat => RibbonTab::Object,
            Tool::Cut => RibbonTab::Edit,
            Tool::Paint | Tool::Faces | Tool::Texture | Tool::Sculpt => RibbonTab::Surface,
        };
    }
    egui::Panel::top("tabs").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 3))).show(ui, |ui| {
        ui.horizontal(|ui| {
            let m = icon_button(ui, "menu", "File: new, open, save, settings", true, false);
            egui::Popup::menu(&m).show(|ui| app_menu(app, ui, &ctx));
            ui.add(icons::logo(18.0));
            ui.label(RichText::new("Simply3D").strong());
            ui.add_space(14.0);
            for t in RibbonTab::ALL {
                let on = app.ribbon == t;
                let txt = RichText::new(t.name()).size(13.5);
                let txt = if on { txt.color(ACCENT).strong() } else { txt };
                let r = ui.add(egui::Label::new(txt).selectable(false).sense(Sense::click()));
                if on {
                    let y = r.rect.bottom() + 3.0;
                    ui.painter().hline(r.rect.x_range(), y, egui::Stroke::new(2.5, ACCENT));
                }
                if r.clicked() {
                    app.ribbon = t;
                }
                ui.add_space(10.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, "panel-right", "Show / hide the side panel", true, app.settings.sidebar).clicked() {
                    app.settings.sidebar = !app.settings.sidebar;
                    // your choice now: don't close it by itself when the tool ends
                    app.panel_for_tool = false;
                }
                // the last step: check it and send it to the printer
                let has_parts = app.scene.objects.iter().any(|o| o.visible && !o.hole);
                let print = egui::Button::image_and_text(
                    icons::image("printer", 16.0, if has_parts { Color32::WHITE } else { weak(ui) }),
                    RichText::new("Print").strong().color(if has_parts { Color32::WHITE } else { weak(ui) }),
                )
                .fill(if has_parts { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill })
                .min_size(Vec2::new(0.0, 26.0))
                .corner_radius(13.0);
                if ui
                    .add_enabled(has_parts, print)
                    .on_hover_text("Check the plate and open it in your slicer, or save a file (Ctrl+P)")
                    .on_disabled_hover_text("Add a shape or open a model first")
                    .clicked()
                {
                    app.open_print();
                }
                if icon_button(ui, "save", "Save (Ctrl+S)", true, false).clicked() {
                    app.save(&ctx);
                }
                let search = ui
                    .add(
                        egui::Button::new(RichText::new("Search commands…   Ctrl+K").color(weak(ui)))
                            .min_size(Vec2::new(200.0, 26.0))
                            .corner_radius(13.0),
                    )
                    .on_hover_text("Find any action by name");
                if search.clicked() {
                    app.palette = Some(crate::commands::Palette::new());
                }
                let rl = app.history.redo_label().map(|l| format!("Redo {l} (Ctrl+Y)")).unwrap_or_else(|| "Redo (Ctrl+Y)".into());
                if icon_button(ui, "redo-2", &rl, app.history.can_redo(), false).clicked() {
                    app.redo();
                }
                // the last steps: jump back several at once
                let steps: Vec<String> = app.history.undo_labels().into_iter().take(20).map(str::to_string).collect();
                let (rect, hist) = ui.allocate_exact_size(Vec2::new(14.0, 30.0), if steps.is_empty() { Sense::hover() } else { Sense::click() });
                if ui.is_rect_visible(rect) {
                    let col = if steps.is_empty() { weak(ui).gamma_multiply(0.45) } else if hist.hovered() { text_color(ui) } else { weak(ui) };
                    icons::image("chevron-down", 12.0, col).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(12.0)));
                }
                let hist = hist.on_hover_text("Undo history");
                egui::Popup::menu(&hist).show(|ui| {
                    ui.set_min_width(200.0);
                    ui.label(RichText::new("Undo back to before…").color(weak(ui)).small());
                    for (k, l) in steps.iter().enumerate() {
                        let text = if k == 0 { format!("{l}   (last step)") } else { l.clone() };
                        if ui.button(text).clicked() {
                            for _ in 0..=k {
                                app.undo();
                            }
                        }
                    }
                    if app.history.undo_len() > steps.len() {
                        ui.label(RichText::new(format!("… {} older", app.history.undo_len() - steps.len())).color(weak(ui)).small());
                    }
                });
                let ul = app.history.undo_label().map(|l| format!("Undo {l} (Ctrl+Z)")).unwrap_or_else(|| "Undo (Ctrl+Z)".into());
                if icon_button(ui, "undo-2", &ul, app.history.can_undo(), false).clicked() {
                    app.undo();
                }
            });
        });
    });
    egui::Panel::top("ribbon").frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 4))).show(ui, |ui| {
        egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
            ui.horizontal(|ui| match app.ribbon {
                RibbonTab::Insert => insert_tab(app, ui, &ctx),
                RibbonTab::Object => object_tab(app, ui, &ctx),
                RibbonTab::Edit => edit_tab(app, ui, &ctx),
                RibbonTab::Surface => surface_tab(app, ui, &ctx),
                RibbonTab::View => view_tab(app, ui),
                RibbonTab::Help => help_tab(app, ui),
            });
        });
    });
}

fn app_menu(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    ui.set_min_width(220.0);
    if ui.add(egui::Button::new("New plate").shortcut_text("Ctrl+N")).clicked() {
        app.request_new_plate();
    }
    if ui.add(egui::Button::new("Open…").shortcut_text("Ctrl+O")).clicked() {
        app.open_dialog(ctx);
    }
    ui.menu_button("Recent files", |ui| {
        if app.settings.recent.is_empty() {
            ui.weak("(none yet)");
        }
        for p in app.settings.recent.clone() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if ui.button(name).on_hover_text(p.display().to_string()).clicked() {
                app.import_files(vec![p], ctx);
            }
        }
    });
    ui.separator();
    if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
        app.save(ctx);
    }
    if ui.add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S")).clicked() {
        app.save_as_dialog(ctx, false);
    }
    if ui.add_enabled(!app.selection.is_empty(), egui::Button::new("Export selection…")).clicked() {
        app.save_as_dialog(ctx, true);
    }
    if ui.add(egui::Button::new("Print / open in slicer…").shortcut_text("Ctrl+P")).clicked() {
        app.open_print();
    }
    ui.separator();
    if ui.button("Settings…").clicked() {
        app.show_settings = true;
    }
    if ui.button("About Simply3D").clicked() {
        app.show_about = true;
    }
    if ui.button("Quit").clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn insert_tab(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    group(ui, "Shapes", |ui| {
        for (icon, name, label) in [("box", "Cube", "Cube"), ("cylinder", "Cylinder", "Cylinder"), ("globe", "Sphere", "Sphere")] {
            if big(ui, icon, label, true, false, false).on_hover_text(format!("Add a {}", label.to_lowercase())).clicked() {
                app.add_dialog = Some(Primitive::default_for(name));
            }
        }
        let r = big(ui, "shapes", "More", true, false, true).on_hover_text("Cone, torus, tube, pyramid, wedge…");
        egui::Popup::menu(&r).show(|ui| add_menu(ui, app, &Primitive::ALL_NAMES));
    });
    group(ui, "Parts", |ui| {
        let r = big(ui, "bolt", "Threads", true, false, true).on_hover_text("Bolts, nuts, threaded rods and holes");
        egui::Popup::menu(&r).show(|ui| add_menu(ui, app, &Primitive::FASTENER_NAMES));
        let r = big(ui, "cog", "Mechanical", true, false, true).on_hover_text("Gears, springs, hinges, snap-fit clips");
        egui::Popup::menu(&r).show(|ui| add_menu(ui, app, &Primitive::PART_NAMES));
    });
    group(ui, "Storage", |ui| {
        if big(ui, "package", "Box & lid", true, false, false)
            .on_hover_text("A box with a slide, snap or hinged lid, rounded corners and dividers")
            .clicked()
        {
            app.add_dialog = Some(Primitive::default_for("Box with lid"));
        }
        let r = big(ui, "boxes", "Gridfinity", true, false, true).on_hover_text("Gridfinity bins and baseplates (42 mm grid)");
        egui::Popup::menu(&r).show(|ui| add_menu(ui, app, &Primitive::STORAGE_NAMES[1..]));
    });
    group(ui, "Toys", |ui| {
        let r = big(ui, "blocks", "Bricks", true, false, true).on_hover_text("Bricks, plates and tiles that fit LEGO® and DUPLO® bricks");
        egui::Popup::menu(&r).show(|ui| add_menu(ui, app, &Primitive::BRICK_NAMES));
    });
    group(ui, "Text & pictures", |ui| {
        if big(ui, "type", "Text", true, app.tool == Tool::Text, false)
            .on_hover_text("3D text, or emboss / engrave it onto a face (T). Click again to close it")
            .clicked()
        {
            if app.tool == Tool::Text {
                app.tool = Tool::Select;
            } else {
                app.start_text_tool();
            }
        }
        if big(ui, "image", "Picture", true, app.tool == Tool::Image, false)
            .on_hover_text("Turn a picture into 3D: contour, height map, edge, stamp, lithophane")
            .clicked()
        {
            if app.tool == Tool::Image {
                app.tool = Tool::Select;
            } else {
                // the tool's bar shows "Open picture…" even if the file dialog is cancelled
                app.tool = Tool::Image;
                if app.img_rt.source.is_none() {
                    app.image_dialog(ctx);
                }
            }
        }
    });
    group(ui, "File", |ui| {
        if big(ui, "file-input", "Add model", true, false, false)
            .on_hover_text("Add STL, OBJ, PLY or 3MF files to the plate (Ctrl+O)")
            .clicked()
        {
            app.open_dialog(ctx);
        }
    });
}

fn object_tab(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    let has = !app.selection.is_empty();
    let many = app.selection.len() >= 2;
    let sel = app.tool == Tool::Select;
    group(ui, "Transform", |ui| {
        for (icon, label, kind, tip) in [
            ("move", "Move", GizmoKind::Move, "Move handles (W) · Q hides the handles"),
            ("rotate-3d", "Rotate", GizmoKind::Rotate, "Rotation rings (E)"),
            ("scale-3d", "Scale", GizmoKind::Scale, "Scale handles (R)"),
        ] {
            if big(ui, icon, label, true, sel && app.gizmo_kind == kind, false).on_hover_text(tip).clicked() {
                app.tool = Tool::Select;
                app.gizmo_kind = kind;
            }
        }
        let r = big(ui, "flip-horizontal-2", "Mirror", has, false, true).on_hover_text("Mirror across X, Y or Z");
        egui::Popup::menu(&r).show(|ui| {
            for (a, n) in ["X", "Y", "Z"].iter().enumerate() {
                if ui.button(format!("Mirror {n}")).clicked() {
                    app.mirror_selected(a);
                }
            }
        });
        if big(ui, "check", "Bake", app.bakeable(), false, false)
            .on_hover_text("Build the rotation and scale into the shape itself, so it reads 0° and 100% again. Nothing changes on screen")
            .on_disabled_hover_text("Nothing to bake: the selection isn't rotated or scaled")
            .clicked()
        {
            app.bake_selected();
        }
    });
    group(ui, "Place", |ui| {
        if big(ui, "arrow-down-to-line", "Lay flat", true, app.tool == Tool::LayFlat, false)
            .on_hover_text("Click a face to put it down on the plate (L). Shift+click to lay several")
            .clicked()
        {
            app.tool = if app.tool == Tool::LayFlat { Tool::Select } else { Tool::LayFlat };
        }
        if big(ui, "arrow-down-to-dot", "Drop", has, false, false).on_hover_text("Drop to plate (D)").clicked() {
            app.drop_selected();
        }
        if big(ui, "locate-fixed", "Centre", has, false, false).on_hover_text("Centre on plate").clicked() {
            app.center_selected_on_plate();
        }
        if big(ui, "layout-grid", "Arrange", !app.scene.objects.is_empty(), false, false)
            .on_hover_text("Pack the parts on the plate (A)")
            .clicked()
        {
            let g = app.settings.arrange_gap;
            app.auto_arrange(g);
        }
        if big(ui, "compass", "Orient", !app.scene.objects.is_empty(), false, false)
            .on_hover_text("Turn parts to print with the least overhang")
            .clicked()
        {
            app.auto_orient_selected();
        }
        let r = big(ui, "align-center-horizontal", "Align", many, false, true).on_hover_text("Line up the selected objects");
        egui::Popup::menu(&r).show(|ui| {
            ui.checkbox(&mut app.settings.align_to_first, "To the first selected");
            ui.separator();
            for (a, n) in ["X", "Y", "Z"].iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(*n);
                    for (m, t) in ["Min", "Centre", "Max"].iter().enumerate() {
                        if ui.small_button(*t).clicked() {
                            app.align(a, m as u8);
                        }
                    }
                    if ui.small_button("Distribute").clicked() {
                        app.distribute(a);
                    }
                });
            }
        });
    });
    group(ui, "Organise", |ui| {
        if big(ui, "copy", "Duplicate", has, false, false).on_hover_text("Make a copy beside it (Ctrl+D)").clicked() {
            app.duplicate_selected();
        }
        if big(ui, "copy-plus", "Array", has, app.array_open, false)
            .on_hover_text("Copies in a grid or around a circle, set up in the bar under the view")
            .clicked()
        {
            app.array_open = !app.array_open;
            if app.array_open {
                app.tool = Tool::Select;
                if app.preview.is_some() {
                    app.cancel_preview();
                }
            }
        }
        if big(ui, "trash-2", "Delete", has, false, false).on_hover_text("Remove the selection (Del)").clicked() {
            app.delete_selected();
        }
        if big(ui, "group", "Group", many, false, false).on_hover_text("Keep the selected objects together (Ctrl+G)").clicked() {
            app.group_selected();
        }
        if big(ui, "ungroup", "Ungroup", app.can_ungroup(), false, false).on_hover_text("Undo the grouping (Ctrl+Shift+G)").clicked() {
            app.ungroup_selected();
        }
        let all_holes = has && app.selection.iter().all(|id| app.scene.get(*id).is_some_and(|o| o.hole));
        if big(ui, "circle-dashed", "Hole", has, all_holes, false)
            .on_hover_text("Make it a hole: it cuts whatever it overlaps and stays movable. Click again for a solid.")
            .clicked()
        {
            app.toggle_hole_selected();
        }
    });
    group(ui, "Show", |ui| {
        if big(ui, "eye-off", "Hide", has, false, false).on_hover_text("Hide the selection (H) · show everything again (Shift+H)").clicked() {
            app.hide_selected();
        }
        if big(ui, "focus", "Isolate", has || app.isolated.is_some(), app.isolated.is_some(), false)
            .on_hover_text("Show only the selection (I)")
            .clicked()
        {
            app.toggle_isolate();
        }
        if big(ui, "lock", "Lock", has, app.any_locked_selected(), false)
            .on_hover_text("Locked objects can't be clicked or moved (Ctrl+L)")
            .clicked()
        {
            app.toggle_lock_selected();
        }
    });
    group(ui, "Tools", |ui| {
        if big(ui, "ruler", "Measure", true, app.tool == Tool::Measure, false)
            .on_hover_text("Distances, angles, holes and faces (M)")
            .clicked()
        {
            app.tool = if app.tool == Tool::Measure { Tool::Select } else { Tool::Measure };
        }
        if big(ui, "magnet", "Grid snap", true, app.settings.snap_enabled, false)
            .on_hover_text("Moves and rotations snap to the grid steps (hold Ctrl while dragging to invert)")
            .clicked()
        {
            app.settings.snap_enabled = !app.settings.snap_enabled;
        }
        if big(ui, "mouse-pointer-2", "Object snap", true, app.settings.snap_objects, false)
            .on_hover_text("Line up edges and centres with other objects while moving")
            .clicked()
        {
            app.settings.snap_objects = !app.settings.snap_objects;
        }
    });
    let _ = ctx;
}

fn mesh_op(app: &mut App, kind: PreviewKind) {
    app.tool = Tool::Select;
    app.array_open = false;
    // the settings are in the bar under the view
    if kind == PreviewKind::ExtrudeDown {
        app.start_extrude_down();
    } else {
        app.start_preview(kind);
    }
}

fn edit_tab(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    let has = !app.selection.is_empty();
    let many = app.selection.len() >= 2;
    let active = |app: &App, k: PreviewKind| app.preview.as_ref().is_some_and(|p| p.kind == k);
    group(ui, "Combine", |ui| {
        if big(ui, "union", "Merge", many, false, false).on_hover_text("Join the selected objects into one solid").clicked() {
            app.boolean_selected(ctx, BoolOp::Union);
        }
        if big(ui, "subtract", "Subtract", many, false, false)
            .on_hover_text("Cut the other selected objects out of the first one")
            .clicked()
        {
            app.boolean_selected(ctx, BoolOp::Subtract);
        }
        if big(ui, "intersect", "Intersect", many, false, false).on_hover_text("Keep only where they overlap").clicked() {
            app.boolean_selected(ctx, BoolOp::Intersect);
        }
        if big(ui, "combine", "Combine", many, false, false).on_hover_text("Put them in one object without a boolean (fast)").clicked() {
            app.merge_selected();
        }
    });
    group(ui, "Cut", |ui| {
        if big(ui, "scissors", "Cut", has, app.tool == Tool::Cut, false).on_hover_text("Cut with a plane, with optional pegs").clicked() {
            app.start_cut();
        }
        if big(ui, "split", "Separate", has, false, false).on_hover_text("Separate into its loose parts").clicked() {
            app.split_selected(ctx);
        }
        if big(ui, "maximize", "Fit plate", has, app.split_dialog, false)
            .on_hover_text("Cut a model that's too big for your printer into pieces that fit, with pegs")
            .clicked()
        {
            app.split_dialog = !app.split_dialog;
        }
    });
    group(ui, "Shape", |ui| {
        for (icon, label, kind, tip) in [
            ("triangle", "Simplify", PreviewKind::Simplify, "Fewer triangles, same shape"),
            ("waves", "Smooth", PreviewKind::Smooth, "Smooth out facets and bumps"),
            ("squircle", "Round edges", PreviewKind::Round, "Fillets on every sharp edge"),
            ("arrow-down-from-line", "Extrude down", PreviewKind::ExtrudeDown, "Extrude the underside down to the plate"),
            ("container", "Hollow", PreviewKind::Hollow, "Hollow it out with a wall thickness"),
            ("hexagon", "Pattern", PreviewKind::Pattern, "Voronoi, wireframe or gyroid lattice"),
        ] {
            if big(ui, icon, label, has, active(app, kind), false).on_hover_text(tip).clicked() {
                mesh_op(app, kind);
            }
        }
    });
    group(ui, "Fix", |ui| {
        if big(ui, "wrench", "Repair", has, false, false).on_hover_text("Close holes, fix flipped and broken faces").clicked() {
            app.repair_selected(ctx);
        }
        if big(ui, "shield-check", "Make solid", has, active(app, PreviewKind::MakeSolid), false)
            .on_hover_text("Rebuild any mesh as one clean closed solid")
            .clicked()
        {
            mesh_op(app, PreviewKind::MakeSolid);
        }
        let on = app.settings.print_check;
        if big(ui, "triangle-alert", "Print check", !app.scene.objects.is_empty(), on, false)
            .on_hover_text("Colour overhangs, thin walls and islands that may print badly")
            .clicked()
        {
            app.settings.print_check = !on;
        }
    });
}

fn surface_tab(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    let has = !app.selection.is_empty();
    group(ui, "Paint", |ui| {
        if big(ui, "paintbrush", "Paint", true, app.tool == Tool::Paint, false)
            .on_hover_text("Paint colours onto the model for multi-colour printing (P)")
            .clicked()
        {
            app.start_paint_tool();
        }
        if big(ui, "eraser", "Clear", has, false, false).on_hover_text("Remove the paint from the selection").clicked() {
            app.clear_paint_selected();
        }
        if big(ui, "grid-3x3", "Refine", has, false, false).on_hover_text("Split big triangles so you can paint finer details").clicked() {
            app.refine_for_paint(ctx);
        }
    });
    group(ui, "Object colour", |ui| {
        ui.vertical(|ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for c in s3d_core::scene::PALETTE {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), if has { Sense::click() } else { Sense::hover() });
                    let col = Color32::from_rgb(c[0], c[1], c[2]);
                    ui.painter().rect_filled(rect, 4.0, if has { col } else { col.gamma_multiply(0.35) });
                    if has && resp.hovered() {
                        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(2.0, text_color(ui)), egui::StrokeKind::Outside);
                    }
                    if resp.clicked() {
                        app.set_color_selected(c);
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut color = app.selection.first().and_then(|id| app.scene.get(*id)).map(|o| o.color).unwrap_or([200, 200, 200, 255]);
                ui.add_enabled_ui(has, |ui| {
                    if ui.color_edit_button_srgba_unmultiplied(&mut color).changed() {
                        app.set_color_selected(color);
                    }
                });
                ui.weak("Any colour");
            });
        });
    });
    group(ui, "Reshape", |ui| {
        if big(ui, "square-dashed-mouse-pointer", "Faces", true, app.tool == Tool::Faces, false)
            .on_hover_text("Select faces, then extrude, smooth, flatten, paint or separate them (S)")
            .clicked()
        {
            app.start_faces_tool();
        }
        if big(ui, "brick-wall", "Texture", true, app.tool == Tool::Texture, false)
            .on_hover_text("Press a texture into the surface: knurl, carbon, wood, your own picture…")
            .clicked()
        {
            app.start_texture_tool();
        }
        if big(ui, "hand", "Sculpt", true, app.tool == Tool::Sculpt, false)
            .on_hover_text("Draw, inflate, smooth, flatten, grab (B)")
            .clicked()
        {
            app.start_sculpt_tool();
        }
    });
}

fn view_tab(app: &mut App, ui: &mut Ui) {
    group(ui, "Camera", |ui| {
        if big(ui, "house", "Fit all", true, false, false).on_hover_text("Show everything (Home)").clicked() {
            app.frame_all();
        }
        if big(ui, "scan", "Fit selection", !app.selection.is_empty(), false, false).on_hover_text("Zoom to the selection (F)").clicked() {
            app.frame_selection();
        }
        let r = big(ui, "axis-3d", "Views", true, false, true).on_hover_text("Front (1), right (3), top (7), isometric (0)");
        egui::Popup::menu(&r).show(|ui| {
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
        });
        let ortho = app.settings.camera.ortho;
        if big(ui, "box", "Ortho", true, ortho, false).on_hover_text("Orthographic: no perspective (O)").clicked() {
            app.settings.camera.ortho = !ortho;
        }
    });
    group(ui, "Display", |ui| {
        let s = &mut app.settings;
        for (icon, label, on, tip) in [
            ("sparkles", "Smooth shading", &mut s.smooth_shading, "Smooth shading; sharp edges stay sharp (Shift+S)"),
            ("grid-2x2", "Wireframe", &mut s.wireframe, "Show the triangles (Shift+W)"),
            ("layout-grid", "Plate", &mut s.show_plate, "Build plate and grid"),
            ("ratio", "Sizes", &mut s.show_dimensions, "Dimensions of the selection"),
            ("triangle-alert", "Problems", &mut s.show_problems, "Show holes and inside-out faces in red"),
            ("layers", "X-ray", &mut s.see_through, "See through objects overlapping the selection"),
        ] {
            if big(ui, icon, label, true, *on, false).on_hover_text(tip).clicked() {
                *on = !*on;
            }
        }
    });
    group(ui, "Section", |ui| {
        if big(ui, "square-split-horizontal", "Section", !app.scene.objects.is_empty(), app.section.enabled, false)
            .on_hover_text("Slice the view with a plane to look inside (X)")
            .clicked()
        {
            app.toggle_section();
        }
    });
    group(ui, "Theme", |ui| {
        let dark = app.settings.dark;
        if big(ui, "sun-moon", if dark { "Light" } else { "Dark" }, true, false, false)
            .on_hover_text("Switch between light and dark")
            .clicked()
        {
            app.settings.dark = !dark;
            crate::ui::apply_theme(ui.ctx(), app.settings.dark);
        }
    });
}

fn help_tab(app: &mut App, ui: &mut Ui) {
    group(ui, "Help", |ui| {
        if big(ui, "search", "Commands", true, false, false).on_hover_text("Search every action (Ctrl+K)").clicked() {
            app.palette = Some(crate::commands::Palette::new());
        }
        if big(ui, "keyboard", "Shortcuts", true, false, false).on_hover_text("Every key and mouse action (?)").clicked() {
            app.show_shortcuts = true;
        }
        if big(ui, "info", "About", true, false, false).clicked() {
            app.show_about = true;
        }
    });
    group(ui, "Settings", |ui| {
        if big(ui, "settings", "Settings", true, false, false).on_hover_text("Plate size, snapping, mouse, autosave…").clicked() {
            app.show_settings = true;
        }
    });
}

/// The bar that floats at the bottom of the view while objects are selected.
pub fn object_box(app: &mut App, ui: &mut Ui, view: Rect) {
    // a bar at the bottom (preview settings, array) takes this spot
    if app.selection.is_empty() || app.tool != Tool::Select || app.placing.is_some() || app.preview.is_some() || app.array_open {
        return;
    }
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::new("object_box"))
        .fixed_pos(view.center_bottom() + Vec2::new(0.0, -14.0))
        .pivot(Align2::CENTER_BOTTOM)
        .order(egui::Order::Middle)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for (icon, kind, tip) in [
                        ("move", GizmoKind::Move, "Move (W)"),
                        ("rotate-3d", GizmoKind::Rotate, "Rotate (E)"),
                        ("scale-3d", GizmoKind::Scale, "Scale (R)"),
                    ] {
                        if icon_button(ui, icon, tip, true, app.gizmo_kind == kind).clicked() {
                            app.gizmo_kind = if app.gizmo_kind == kind { GizmoKind::Off } else { kind };
                        }
                    }
                    ui.separator();
                    if icon_button(ui, "arrow-down-to-line", "Lay flat: click a face (L)", true, app.tool == Tool::LayFlat).clicked() {
                        app.tool = if app.tool == Tool::LayFlat { Tool::Select } else { Tool::LayFlat };
                    }
                    if icon_button(ui, "arrow-down-to-dot", "Drop to plate (D)", true, false).clicked() {
                        app.drop_selected();
                    }
                    if icon_button(ui, "locate-fixed", "Centre on plate", true, false).clicked() {
                        app.center_selected_on_plate();
                    }
                    ui.separator();
                    if icon_button(ui, "copy", "Duplicate (Ctrl+D)", true, false).clicked() {
                        app.duplicate_selected();
                    }
                    if icon_button(ui, "trash-2", "Delete (Del)", true, false).clicked() {
                        app.delete_selected();
                    }
                    let n = app.selection.len();
                    ui.separator();
                    let b = app.selection_bounds();
                    let s = b.size();
                    let dec = app.settings.decimals.min(1);
                    ui.label(
                        RichText::new(format!(
                            "{}  {:.*} × {:.*} × {:.*} mm",
                            if n == 1 { "1 object".to_string() } else { format!("{n} objects") },
                            dec,
                            s.x,
                            dec,
                            s.y,
                            dec,
                            s.z
                        ))
                        .color(weak(ui)),
                    );
                });
            });
        });
}

/// On an empty plate: what you can do to get started.
pub fn start_card(app: &mut App, ui: &mut Ui, view: Rect) {
    if app.start_closed || !app.scene.objects.is_empty() || app.placing.is_some() || app.tool != Tool::Select || app.add_dialog.is_some() {
        return;
    }
    let ctx = ui.ctx().clone();
    egui::Area::new(egui::Id::new("start_card")).fixed_pos(view.center()).pivot(Align2::CENTER_CENTER).order(egui::Order::Middle).show(
        &ctx,
        |ui| {
            egui::Frame::popup(ui.style()).corner_radius(14.0).inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                ui.set_width(440.0);
                ui.horizontal(|ui| {
                    ui.add(icons::logo(40.0));
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Simply3D").size(22.0).strong());
                        ui.label(RichText::new("Make it, fix it, print it.").color(weak(ui)));
                    });
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let w = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).floor();
                    let tile = |ui: &mut Ui, icon: &str, label: &str| {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 86.0), Sense::click());
                        let p = ui.painter();
                        let fill = if resp.hovered() { ACCENT.gamma_multiply(0.18) } else { ui.visuals().faint_bg_color };
                        p.rect_filled(rect, 10.0, fill);
                        p.rect_stroke(
                            rect,
                            10.0,
                            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                            egui::StrokeKind::Inside,
                        );
                        icons::image(icon, 30.0, ACCENT)
                            .paint_at(ui, Rect::from_center_size(rect.center() - Vec2::new(0.0, 12.0), Vec2::splat(30.0)));
                        ui.painter().text(
                            rect.center() + Vec2::new(0.0, 22.0),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::proportional(13.0),
                            text_color(ui),
                        );
                        resp
                    };
                    if tile(ui, "folder-open", "Open a model").clicked() {
                        app.open_dialog(&ctx);
                    }
                    if tile(ui, "grid-3x3", "Empty plate")
                        .on_hover_text("Start with an empty plate: shapes, parts and storage are on the Insert tab")
                        .clicked()
                    {
                        app.start_closed = true;
                        app.ribbon = RibbonTab::Insert;
                    }
                });
                let recent: Vec<_> = app.settings.recent.iter().filter(|p| p.exists()).take(5).cloned().collect();
                if !recent.is_empty() {
                    ui.add_space(14.0);
                    ui.label(RichText::new("Recent").strong());
                    for p in recent {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let folder = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
                        ui.horizontal(|ui| {
                            if ui.link(name).on_hover_text(p.display().to_string()).clicked() {
                                app.import_files(vec![p.clone()], &ctx);
                            }
                            ui.label(RichText::new(folder).small().color(weak(ui)));
                        });
                    }
                }
                ui.add_space(10.0);
                ui.label(RichText::new("Tip: drop STL, OBJ, PLY, 3MF or picture files anywhere on the window. Ctrl+K finds any command, ? shows every shortcut.").small().color(weak(ui)));
            });
        },
    );
}
