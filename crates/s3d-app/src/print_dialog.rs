//! Print: the last step. A checklist of what could go wrong on the printer (with one-click
//! fixes), then save a file or open the plate straight in a slicer.

use crate::app::{App, JobOutput};
use crate::slicers::{self, Slicer};
use egui::{Color32, RichText};
use s3d_core::io;
use s3d_core::ObjectId;

#[derive(Default)]
pub struct PrintState {
    pub open: bool,
    /// Slicers found on this computer (looked up when the window first opens).
    pub slicers: Option<Vec<Slicer>>,
}

/// Formats offered by "Save file…": (label, extension, tip).
const FORMATS: [(&str, &str, &str); 4] = [
    ("3MF", "3mf", "Recommended: keeps separate parts, colours and painted areas"),
    ("STL", "stl", "The classic: one shape per file, no colours"),
    ("OBJ", "obj", "Widely supported, no painted areas"),
    ("PLY", "ply", "Mesh with vertex data"),
];

enum Fix {
    OntoPlate(Vec<ObjectId>),
    Drop(Vec<ObjectId>),
    Repair(Vec<ObjectId>),
    Split(Vec<ObjectId>),
    Select(Vec<ObjectId>),
    Orient(Vec<ObjectId>),
    PrintCheck,
}

impl App {
    pub fn open_print(&mut self) {
        if self.scene.objects.iter().all(|o| !o.visible || o.hole) {
            self.error("Nothing to print yet: add a shape or open a model");
            return;
        }
        self.print.open = true;
        if self.print.slicers.is_none() {
            self.print.slicers = Some(slicers::detect());
        }
    }

    /// What Print sends: visible solids (and holes, which cut them), all or the selection.
    fn print_ids(&self) -> Vec<ObjectId> {
        let sel = self.settings.print_selection && !self.selection.is_empty();
        self.scene.objects.iter().filter(|o| o.visible && !o.hole && (!sel || self.selection.contains(&o.id))).map(|o| o.id).collect()
    }

    /// Save the plate in a folder the slicer can read and open it there.
    pub fn send_to_slicer(&mut self, ctx: &egui::Context, slicer: Slicer) {
        let only_selected = self.settings.print_selection && !self.selection.is_empty();
        let objs = self.export_objects(only_selected, false);
        if objs.iter().all(|o| o.hole) {
            self.error("Nothing to print");
            return;
        }
        let name = self
            .current_file
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Simply3D plate".into());
        let name: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
        let path = slicers::handoff_dir().join(format!("{name}.3mf"));
        // holes are cut in: every slicer understands that
        let opts = io::SaveOptions { cut_holes: true, thumbnail: true };
        self.settings.slicer = slicer.name.clone();
        self.spawn_job(ctx, format!("Preparing for {}…", slicer.name), move |_p| match io::save_with(&path, &objs, opts) {
            Ok(()) => JobOutput::Exported { path, slicer: Some(slicer) },
            Err(e) => JobOutput::Error(format!("Couldn't prepare the file: {e}")),
        });
    }

    /// "Save file…" with a chosen format.
    pub fn export_dialog(&mut self, ctx: &egui::Context, ext: &str) {
        let only_selected = self.settings.print_selection && !self.selection.is_empty();
        let default = self.current_file.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
        let (label, _, _) = FORMATS.iter().find(|f| f.1 == ext).copied().unwrap_or(FORMATS[0]);
        let Some(path) = rfd::FileDialog::new().add_filter(label, &[ext]).set_file_name(format!("{default}.{ext}")).save_file() else {
            return;
        };
        let path = if path.extension().is_none() { path.with_extension(ext) } else { path };
        self.save_to(ctx, path, only_selected);
    }
}

/// The Print window.
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.print.open {
        return;
    }
    let mut open = true;
    let mut fix = None;
    let mut send = None;
    let mut save_as = None;
    egui::Window::new(RichText::new("Ready to print").strong())
        .id(egui::Id::new("print_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(470.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
        .show(ctx, |ui| {
            ui.set_width(470.0);
            // ----- printer -----
            ui.horizontal(|ui| {
                ui.label(RichText::new("Printer").strong());
                crate::ui::printer_combo(app, ui, "print_printer");
            });
            ui.add_space(6.0);

            // ----- what -----
            let has_sel = !app.selection.is_empty();
            if !has_sel {
                app.settings.print_selection = false;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Print").strong());
                ui.selectable_value(&mut app.settings.print_selection, false, "Everything on the plate");
                ui.add_enabled_ui(has_sel, |ui| {
                    ui.selectable_value(&mut app.settings.print_selection, true, "Only the selection")
                        .on_disabled_hover_text("Select something first");
                });
            });
            ui.separator();

            // ----- checks -----
            let ids = app.print_ids();
            let (off, below) = app.issues.clone();
            let off: Vec<_> = off.into_iter().filter(|i| ids.contains(i)).collect();
            let below: Vec<_> = below.into_iter().filter(|i| ids.contains(i)).collect();
            let mut floating = vec![];
            let mut broken = vec![];
            let mut checking = false;
            let mut oversize = vec![];
            let mut volume = 0.0f64;
            let mut so = app.split_opts;
            so.volume = app.build_volume();
            for &id in &ids {
                let Some(b) = app.object_bounds(id) else { continue };
                if b.min.z > 0.05 {
                    floating.push(id);
                }
                let Some(o) = app.scene.get(id) else { continue };
                match o.mesh.needs_repair() {
                    Some(true) => broken.push(id),
                    None => checking = true,
                    _ => {}
                }
                if !s3d_core::split::fits(b.size(), &so) {
                    oversize.push(id);
                }
                let sc = o.transform.scale;
                volume += o.mesh.mesh.volume().abs() * (sc.x * sc.y * sc.z).abs() as f64;
            }
            let n = |v: usize| if v == 1 { "1 part".to_string() } else { format!("{v} parts") };
            let ok_col = if app.settings.dark { Color32::from_rgb(110, 210, 130) } else { Color32::from_rgb(30, 140, 60) };
            let warn_col = if app.settings.dark { Color32::from_rgb(255, 190, 90) } else { Color32::from_rgb(170, 100, 0) };
            let row = |ui: &mut egui::Ui, good: Option<bool>, text: String, buttons: &mut dyn FnMut(&mut egui::Ui)| {
                ui.horizontal(|ui| {
                    match good {
                        Some(true) => ui.add(crate::icons::image("circle-check", 16.0, ok_col)),
                        Some(false) => ui.add(crate::icons::image("triangle-alert", 16.0, warn_col)),
                        None => ui.add(crate::icons::image("info", 16.0, ui.visuals().weak_text_color())),
                    };
                    let t = RichText::new(text);
                    ui.label(if good == Some(false) { t.color(warn_col) } else { t });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| buttons(ui));
                });
            };
            row(ui, None, format!("{} · {:.1} cm³ of plastic if printed solid", n(ids.len()), volume / 1000.0), &mut |_| {});
            if off.is_empty() && below.is_empty() {
                row(ui, Some(true), "Everything is on the plate".into(), &mut |_| {});
            }
            if !off.is_empty() {
                row(ui, Some(false), format!("{} outside the plate", n(off.len())), &mut |ui| {
                    if ui.small_button("Move onto plate").clicked() {
                        fix = Some(Fix::OntoPlate(off.clone()));
                    }
                });
            }
            if !below.is_empty() {
                row(ui, Some(false), format!("{} below the plate", n(below.len())), &mut |ui| {
                    if ui.small_button("Drop to plate").clicked() {
                        fix = Some(Fix::Drop(below.clone()));
                    }
                });
            }
            if !floating.is_empty() {
                row(ui, None, format!("{} not touching the plate (fine if resting on another part)", n(floating.len())), &mut |ui| {
                    if ui.small_button("Drop").clicked() {
                        fix = Some(Fix::Drop(floating.clone()));
                    }
                    if ui.small_button("Select").clicked() {
                        fix = Some(Fix::Select(floating.clone()));
                    }
                });
            }
            if oversize.is_empty() {
                let v = app.build_volume();
                row(ui, Some(true), format!("Fits the printer ({:.0} × {:.0} × {:.0} mm)", v.x, v.y, v.z), &mut |_| {});
            } else {
                row(ui, Some(false), format!("{} too big for the printer", n(oversize.len())), &mut |ui| {
                    if ui.small_button("Split to fit…").on_hover_text("Cut into pieces that fit, with pegs to join them").clicked() {
                        fix = Some(Fix::Split(oversize.clone()));
                    }
                });
            }
            if !broken.is_empty() {
                row(ui, Some(false), format!("{} not closed (holes or broken faces)", n(broken.len())), &mut |ui| {
                    if ui.small_button("Repair").clicked() {
                        fix = Some(Fix::Repair(broken.clone()));
                    }
                });
            } else if checking {
                row(ui, None, "Checking the meshes…".into(), &mut |ui| {
                    ui.spinner();
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            } else {
                row(ui, Some(true), "All parts are closed solids".into(), &mut |_| {});
            }
            // overhangs and thin walls: from the print check, when it's on
            if app.settings.print_check {
                let mut total = s3d_core::printcheck::CheckStats::default();
                let mut done = 0;
                for id in &ids {
                    if let Some((_, st)) = app.check_cache.get(id).and_then(|e| e.result.as_ref()) {
                        total.overhang_area += st.overhang_area;
                        total.thin_area += st.thin_area;
                        total.islands += st.islands;
                        done += 1;
                    }
                }
                if done < ids.len() {
                    row(ui, None, "Looking for overhangs and thin walls…".into(), &mut |ui| {
                        ui.spinner();
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                } else {
                    let supports = total.overhang_area > 25.0 || total.islands > 0;
                    let text = if supports {
                        format!(
                            "Needs supports: {:.1} cm² overhang{}",
                            total.overhang_area / 100.0,
                            if total.islands > 0 { format!(", {} floating start{}", total.islands, if total.islands == 1 { "" } else { "s" }) } else { String::new() }
                        )
                    } else {
                        "No overhangs that need supports".into()
                    };
                    row(ui, Some(!supports), text, &mut |ui| {
                        if supports && ui.small_button("Orient").on_hover_text("Turn parts to print with the least overhang").clicked() {
                            fix = Some(Fix::Orient(ids.clone()));
                        }
                    });
                    if total.thin_area > 1.0 {
                        let w = app.settings.check.min_wall;
                        row(ui, Some(false), format!("Walls thinner than {w:.1} mm on {:.1} cm²", total.thin_area / 100.0), &mut |_| {});
                    }
                }
            } else {
                row(ui, None, "Overhangs and thin walls: not checked".into(), &mut |ui| {
                    if ui.small_button("Check").on_hover_text("Turn on Print check: colours the problem areas").clicked() {
                        fix = Some(Fix::PrintCheck);
                    }
                });
            }
            if app.scene.objects.iter().any(|o| o.visible && o.hole) {
                row(ui, None, "Hole objects are cut into the parts".into(), &mut |_| {});
            }
            ui.separator();

            // ----- send -----
            ui.label(RichText::new("Open in a slicer").strong());
            let list = app.print.slicers.clone().unwrap_or_default();
            let mut pick = list.iter().position(|s| s.name == app.settings.slicer).unwrap_or(0);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("print_slicer")
                    .selected_text(list.get(pick).map(|s| s.name.as_str()).unwrap_or("Default app"))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (i, s) in list.iter().enumerate() {
                            let tip = match &s.launch {
                                slicers::Launch::Exe(p) => p.display().to_string(),
                                slicers::Launch::Flatpak(id) => format!("Flatpak {id}"),
                                slicers::Launch::Default => "Whatever opens 3MF files on this computer".into(),
                            };
                            if ui.selectable_label(pick == i, &s.name).on_hover_text(tip).clicked() {
                                pick = i;
                                app.settings.slicer = s.name.clone();
                            }
                        }
                    });
                if let Some(s) = list.get(pick) {
                    let label = if s.launch == slicers::Launch::Default { "Open".to_string() } else { format!("Open in {}", s.name) };
                    let b = egui::Button::image_and_text(crate::icons::image("external-link", 15.0, Color32::WHITE), RichText::new(label).strong().color(Color32::WHITE))
                        .fill(crate::ui::ACCENT);
                    if ui.add(b).on_hover_text("Saves the plate as 3MF and opens it there, ready to slice").clicked() {
                        send = Some(s.clone());
                    }
                }
            });
            if list.len() <= 1 {
                ui.weak("No slicer found in the usual places: Open uses the app your computer opens 3MF files with.");
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Or save a file").strong());
            ui.horizontal(|ui| {
                for (label, ext, tip) in FORMATS {
                    if ui.button(format!("{label}…")).on_hover_text(tip).clicked() {
                        save_as = Some(ext);
                    }
                }
            });
        });
    app.print.open = open;
    if let Some(s) = send {
        app.print.open = false;
        app.send_to_slicer(ctx, s);
    }
    if let Some(ext) = save_as {
        app.export_dialog(ctx, ext);
    }
    match fix {
        Some(Fix::OntoPlate(ids)) => app.fit_onto_plate(&ids),
        Some(Fix::Drop(ids)) => app.drop_ids(&ids),
        Some(Fix::Repair(ids)) => {
            app.selection = ids;
            app.repair_selected(ctx);
        }
        Some(Fix::Split(ids)) => {
            app.selection = ids;
            app.print.open = false;
            app.split_dialog = true;
        }
        Some(Fix::Orient(ids)) => {
            app.selection = ids;
            app.auto_orient_selected();
        }
        Some(Fix::Select(ids)) => app.selection = ids,
        Some(Fix::PrintCheck) => app.settings.print_check = true,
        None => {}
    }
}
