//! Text tool: add 3D text as an object, or emboss / engrave it onto a face.

use crate::app::{App, JobOutput, Tool};
use crate::expr::Kind;
use crate::preview::BgTask;
use crate::ui::num;
use egui::RichText;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use s3d_core::csg::{self, BoolOp};
use s3d_core::fonts::FontLibrary;
use s3d_core::scene::MeshHandle;
use s3d_core::text::{self, Align, TextOptions};
use s3d_core::wrap::Bend;
use s3d_core::{Mesh, ObjectId};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TextMode {
    /// A separate text object on the plate.
    Object,
    /// Raised letters joined onto a face.
    Emboss,
    /// Letters cut into a face.
    Engrave,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TextSettings {
    pub text: String,
    pub font: String,
    pub size: f32,
    pub depth: f32,
    pub spacing: f32,
    pub line_spacing: f32,
    /// 0 left, 1 centre, 2 right
    pub align: u8,
    pub mode: TextMode,
    pub standing: bool,
    pub angle: f32,
    /// Bend the letters around curved surfaces (cylinders, spheres).
    pub wrap: bool,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            text: "Hello".into(),
            font: String::new(),
            size: 10.0,
            depth: 2.0,
            spacing: 0.0,
            line_spacing: 1.0,
            align: 1,
            mode: TextMode::Object,
            standing: false,
            angle: 0.0,
            wrap: true,
        }
    }
}

/// Where the text would go if you clicked now.
#[derive(Debug, Clone, Copy)]
pub struct TextHover {
    pub target: Option<ObjectId>,
    pub point: Vec3,
    /// Placement frame: X = reading direction, Y = up, Z = surface normal, origin on the surface.
    pub frame: Mat4,
    /// How the surface curves under the shape (flat when not following curves).
    pub bend: Bend,
}

impl TextHover {
    pub fn on_plate(point: Vec3) -> Self {
        TextHover { target: None, point, frame: Mat4::IDENTITY, bend: Bend::FLAT }
    }
}

/// Shapes sliced for bending, and the bent hover preview.
#[derive(Default)]
pub struct WrapCache {
    prep: Option<(u64, Arc<Mesh>)>,
    pub ghost: Option<(u64, MeshHandle)>,
}

fn hash_of(parts: &[u64]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

fn ghost_key(unit: &MeshHandle, bend: Bend, z0: f32, z1: f32) -> u64 {
    hash_of(&[unit.id, bend.rx.to_bits() as u64, bend.ry.to_bits() as u64, z0.to_bits() as u64, z1.to_bits() as u64])
}

/// Heights of the hover preview: raised, or sunk in for engraving.
fn preview_span(depth: f32, engrave: bool) -> (f32, f32) {
    let d = depth.max(0.05);
    if engrave {
        (-d, 0.0)
    } else {
        (0.0, d)
    }
}

#[derive(Default)]
pub struct TextRuntime {
    pub fonts: Option<Arc<FontLibrary>>,
    fonts_rx: Option<Receiver<FontLibrary>>,
    font_data: Option<(String, Arc<Vec<u8>>, u32)>,
    /// Text mesh with depth 1 (scaled in Z for the real depth) and its key.
    pub base: Option<(u64, MeshHandle)>,
    task: Option<BgTask<Result<Mesh, String>>>,
    pub error: Option<String>,
    error_key: u64,
    pub hover: Option<TextHover>,
    pub filter: String,
}

const BUILTIN_FONT: &str = "Ubuntu Light (built-in)";

impl TextRuntime {
    pub fn load_fonts(&mut self) {
        if self.fonts.is_some() || self.fonts_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let lib = FontLibrary::system(Some((BUILTIN_FONT, epaint_default_fonts::UBUNTU_LIGHT)));
            let _ = tx.send(lib);
        });
        self.fonts_rx = Some(rx);
    }
}

fn align_of(a: u8) -> Align {
    match a {
        0 => Align::Left,
        2 => Align::Right,
        _ => Align::Center,
    }
}

impl App {
    pub fn start_text_tool(&mut self) {
        self.tool = Tool::Text;
        self.text_rt.load_fonts();
    }

    fn text_key(&self) -> u64 {
        let t = &self.settings.text;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{}|{}|{}|{}|{}|{}", t.text, t.font, t.size, t.spacing, t.line_spacing, t.align).hash(&mut h);
        h.finish()
    }

    /// Keep the font list and the text mesh up to date (called every frame).
    pub fn tick_text(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.text_rt.fonts_rx {
            if let Ok(lib) = rx.try_recv() {
                if lib.find(&self.settings.text.font).is_none() {
                    if let Some(i) = lib.default_index() {
                        self.settings.text.font = lib.entries[i].label.clone();
                    }
                }
                self.text_rt.fonts = Some(Arc::new(lib));
                self.text_rt.fonts_rx = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if self.tool != Tool::Text {
            self.text_rt.hover = None;
            return;
        }
        let Some(lib) = self.text_rt.fonts.clone() else { return };
        // finished?
        if let Some(out) = self.text_rt.task.as_ref().and_then(|t| t.poll()) {
            let t = self.text_rt.task.take().unwrap();
            match out {
                Some(Ok(m)) => {
                    self.text_rt.base = Some((t.key, MeshHandle::new(m)));
                    self.text_rt.error = None;
                }
                Some(Err(e)) => {
                    self.text_rt.base = None;
                    self.text_rt.error = Some(e);
                    self.text_rt.error_key = t.key;
                }
                None => {
                    self.text_rt.error = Some("this font could not be used".into());
                    self.text_rt.error_key = t.key;
                }
            }
        }
        let key = self.text_key();
        let current =
            self.text_rt.base.as_ref().is_some_and(|b| b.0 == key) || (self.text_rt.error.is_some() && self.text_rt.error_key == key);
        let running = self.text_rt.task.as_ref().is_some_and(|t| t.key == key);
        if !running && !current {
            self.text_rt.error = None;
            let t = self.settings.text.clone();
            if t.text.trim().is_empty() {
                self.text_rt.base = None;
                return;
            }
            let Some(i) = lib.find(&t.font).or(lib.default_index()) else { return };
            let label = lib.entries[i].label.clone();
            let data = match &self.text_rt.font_data {
                Some((l, d, idx)) if *l == label => Some((d.clone(), *idx)),
                _ => lib.data(i),
            };
            let Some((data, idx)) = data else {
                self.text_rt.error = Some("can't read this font".into());
                self.text_rt.error_key = key;
                return;
            };
            self.text_rt.font_data = Some((label, data.clone(), idx));
            let opts = TextOptions {
                text: t.text.clone(),
                size: t.size.max(0.1),
                depth: 1.0,
                spacing: t.spacing,
                line_spacing: t.line_spacing,
                align: align_of(t.align),
            };
            self.text_rt.task = Some(BgTask::spawn(ctx, key, move |_p| text::text_mesh(&data, idx, &opts).map_err(|e| e.to_string())));
        }
        if self.text_rt.task.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The current text mesh (depth 1), if it matches the settings.
    pub fn text_base(&self) -> Option<&MeshHandle> {
        self.text_rt.base.as_ref().map(|b| &b.1)
    }

    /// World matrix of a new text object whose footprint is centred on `at` (on the plate).
    pub fn text_object_matrix(&self, at: Vec3) -> Option<Mat4> {
        let base = self.text_base()?;
        let b = base.mesh.bounds();
        let t = &self.settings.text;
        let depth = t.depth.max(0.05);
        let spin = Mat4::from_rotation_z(t.angle.to_radians());
        Some(if t.standing {
            // reading from the front: up = +Z, extruded toward the viewer (−Y)
            let stand = Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2);
            Mat4::from_translation(at + Vec3::new(0.0, 0.0, -b.min.y))
                * spin
                * Mat4::from_translation(Vec3::new(0.0, depth * 0.5, 0.0))
                * stand
                * Mat4::from_scale(Vec3::new(1.0, 1.0, depth))
        } else {
            Mat4::from_translation(at) * spin * Mat4::from_scale(Vec3::new(1.0, 1.0, depth))
        })
    }

    /// Matrix for a new text object at the hovered spot: lying on a face of another
    /// object (`h.frame`), on the plate, or (None) beside the others.
    pub fn text_object_matrix_h(&self, h: Option<TextHover>) -> Option<Mat4> {
        match h {
            Some(h) if h.frame != Mat4::IDENTITY => {
                let depth = self.settings.text.depth.max(0.05);
                self.text_base()?;
                Some(h.frame * Mat4::from_scale(Vec3::new(1.0, 1.0, depth)))
            }
            Some(h) => self.text_object_matrix(h.point),
            None => self.text_object_matrix(self.text_default_spot()),
        }
    }

    /// Where "Add" puts the text: beside what is already on the plate.
    pub fn text_default_spot(&self) -> Vec3 {
        let existing = self.scene.bounds();
        if existing.is_empty() {
            return Vec3::ZERO;
        }
        let w = self.text_base().map(|b| b.mesh.bounds().size().x).unwrap_or(40.0);
        Vec3::new(existing.max.x + 10.0 + w * 0.5, existing.center().y, 0.0)
    }

    pub fn add_text_object(&mut self, at: Option<TextHover>) {
        let (Some(base), Some(m)) = (self.text_base().cloned(), self.text_object_matrix_h(at)) else {
            self.error("Text: nothing to add yet");
            return;
        };
        self.checkpoint("Add text");
        let name: String = self.settings.text.text.lines().next().unwrap_or("Text").chars().take(24).collect();
        let name = self.scene.unique_name(&format!("Text \"{}\"", name.trim()));
        let id = self.scene.add_world_mesh(name, base.mesh.transformed(&m), None);
        self.selection = vec![id];
        self.info("Text added — drag it anywhere. Shift+click places more while the text tool is open");
    }

    /// How the target's surface curves under a shape of half-size `half` placed at
    /// `frame`: heights sampled symmetrically along the frame's X and Y.
    pub fn fit_bend(&self, target: ObjectId, frame: Mat4, half: Vec2) -> Bend {
        let Some(obj) = self.scene.get(target) else { return Bend::FLAT };
        let n = frame.z_axis.truncate();
        let far = obj.world_bounds().size().length() + 10.0;
        let height = |l: Vec3| -> Option<f32> {
            let q = frame.transform_point3(l);
            let h = self.scene.pick_filtered(q + n * far, -n, |o| o.id == target, |_| true)?;
            (h.normal.dot(n) > 0.2).then(|| (h.point - q).dot(n))
        };
        let along = |axis: Vec3, a: f32| -> f32 {
            // try smaller spans if the surface turns away before the shape's edge
            let mut a = a;
            for _ in 0..4 {
                if a < 0.5 {
                    break;
                }
                if let (Some(h1), Some(h2)) = (height(axis * a), height(-axis * a)) {
                    // a step or an edge rather than a smooth curve: leave it flat
                    if (h1 - h2).abs() > 0.25 * h1.abs().max(h2.abs()) + 0.05 {
                        return f32::INFINITY;
                    }
                    return Bend::fit(&[(a, h1), (a, h2)]);
                }
                a *= 0.5;
            }
            f32::INFINITY
        };
        let mut b = Bend { rx: along(Vec3::X, half.x), ry: along(Vec3::Y, half.y) };
        // never wrap round more than a full turn
        for (r, h) in [(&mut b.rx, half.x), (&mut b.ry, half.y)] {
            if r.is_finite() && r.abs() < h / std::f32::consts::PI {
                *r = r.signum() * h / std::f32::consts::PI;
            }
        }
        b
    }

    /// Half-size of a unit shape around its placement point.
    pub fn shape_half(unit: &Mesh) -> Vec2 {
        let b = unit.bounds();
        Vec2::new(b.min.x.abs().max(b.max.x.abs()), b.min.y.abs().max(b.max.y.abs()))
    }

    /// `unit` (z ∈ [0, 1]) sliced and bent, its height spanning z0..z1, in frame space.
    pub fn bent_shape(&mut self, unit: &MeshHandle, bend: Bend, z0: f32, z1: f32) -> Mesh {
        let bend = bend.safe(z0, z1);
        if bend.is_flat() {
            return unit
                .mesh
                .transformed(&(Mat4::from_translation(Vec3::new(0.0, 0.0, z0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, z1 - z0))));
        }
        let s = unit.mesh.bounds().size();
        let steps = bend.steps(s.x, s.y);
        let key = hash_of(&[unit.id, steps.0.map_or(0, |v| v.to_bits() as u64 + 1), steps.1.map_or(0, |v| v.to_bits() as u64 + 1)]);
        let prep = match &self.wrap.prep {
            Some((k, m)) if *k == key => m.clone(),
            _ => {
                let m = Arc::new(s3d_core::wrap::prepare_steps(&unit.mesh, steps));
                self.wrap.prep = Some((key, m.clone()));
                m
            }
        };
        bend.map(&prep, z0, z1)
    }

    /// The surface placement the active tool is previewing: (unit shape, hover, depth, engrave).
    fn surface_placement(&self) -> Option<(MeshHandle, TextHover, f32, bool)> {
        match self.tool {
            Tool::Text if self.settings.text.mode != TextMode::Object => {
                let h = self.text_rt.hover.filter(|h| h.target.is_some())?;
                Some((self.text_base()?.clone(), h, self.settings.text.depth, self.settings.text.mode == TextMode::Engrave))
            }
            Tool::Image if self.settings.image.place != TextMode::Object => {
                let h = self.img_rt.hover.filter(|h| h.target.is_some())?;
                Some((self.img_rt.unit.clone()?, h, self.settings.image.height, self.settings.image.place == TextMode::Engrave))
            }
            _ => None,
        }
    }

    /// Rebuild the bent hover preview when needed (called by the viewport every frame).
    pub fn update_wrap_ghost(&mut self) {
        let Some((unit, h, depth, engrave)) = self.surface_placement() else { return };
        if h.bend.is_flat() {
            return;
        }
        let (z0, z1) = preview_span(depth, engrave);
        let key = ghost_key(&unit, h.bend, z0, z1);
        if self.wrap.ghost.as_ref().is_some_and(|g| g.0 == key) {
            return;
        }
        let m = self.bent_shape(&unit, h.bend, z0, z1);
        self.wrap.ghost = Some((key, MeshHandle::new(m)));
    }

    /// Hover preview for a shape on a surface: (mesh, model matrix).
    pub fn surface_ghost(&self, unit: &MeshHandle, h: &TextHover, depth: f32, engrave: bool) -> (MeshHandle, Mat4) {
        if !h.bend.is_flat() {
            let (z0, z1) = preview_span(depth, engrave);
            if let Some(g) = self.wrap.ghost.as_ref().filter(|g| g.0 == ghost_key(unit, h.bend, z0, z1)) {
                return (g.1.clone(), h.frame);
            }
        }
        (unit.clone(), surface_preview(h.frame, depth, engrave))
    }

    pub fn surface_frame(&self, point: Vec3, normal: Vec3, cam_up: Vec3) -> Mat4 {
        surface_frame(point, normal, cam_up, self.settings.text.angle)
    }

    /// Emboss / engrave at the hovered spot (background boolean).
    pub fn apply_text_on_surface(&mut self, ctx: &egui::Context, hover: TextHover) {
        let Some(base) = self.text_base().cloned() else {
            self.error("Text: still preparing the letters");
            return;
        };
        let t = &self.settings.text;
        let (depth, engrave, reach) = (t.depth, t.mode == TextMode::Engrave, t.size * 3.0);
        let label = if engrave { "Engrave text" } else { "Emboss text" };
        self.apply_on_surface(ctx, hover, &base, depth, engrave, reach, label);
    }

    /// Join (emboss) or cut (engrave) a flat shape onto the hovered face. `unit` lies on
    /// XY with z in [0, 1]; it is scaled to `depth`. Curved surfaces are measured under the
    /// shape so it always overlaps the object (emboss) or clears the surface (engrave).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_on_surface(
        &mut self,
        ctx: &egui::Context,
        hover: TextHover,
        unit: &MeshHandle,
        depth: f32,
        engrave: bool,
        reach: f32,
        label: &'static str,
    ) {
        let Some(target) = hover.target else { return };
        let Some(obj) = self.scene.get(target).cloned() else { return };
        if obj.locked {
            return;
        }
        let d = depth.max(0.05);
        let (frame, bend) = (hover.frame, hover.bend);
        let fb = unit.mesh.bounds();
        let far = obj.world_bounds().size().length() + 10.0;
        let mut lo: f32 = 0.0;
        let mut hi: f32 = 0.0;
        let (nx, ny) = if bend.is_flat() { (5, 3) } else { (9, 3) };
        for i in 0..nx {
            for j in 0..ny {
                let x = fb.min.x + (fb.max.x - fb.min.x) * i as f32 / (nx - 1) as f32;
                let y = fb.min.y + (fb.max.y - fb.min.y) * j as f32 / (ny - 1) as f32;
                // measured along the (bent) surface normal under this point of the shape
                let q = frame.transform_point3(bend.apply(Vec3::new(x, y, 0.0)));
                let n = frame.transform_vector3(bend.normal(x, y)).normalize_or(frame.z_axis.truncate());
                if let Some(h) = self.scene.pick_filtered(q + n * far, -n, |o| o.id == target, |_| true) {
                    let off = (h.point - q).dot(n);
                    if off.abs() < reach + d {
                        lo = lo.min(off);
                        hi = hi.max(off);
                    }
                }
            }
        }
        let (z0, z1) = if engrave { (-d, hi + 0.3) } else { (lo - 0.3, d) };
        let shape = self.bent_shape(unit, bend, z0, z1).transformed(&frame);
        let body = obj.world_mesh();
        let op = if engrave { BoolOp::Subtract } else { BoolOp::Union };
        let (name, color) = (obj.name.clone(), obj.color);
        self.spawn_job(ctx, format!("{label}…"), move |_p| match csg::boolean_many(&body, &[shape], op) {
            Ok(m) if m.is_empty() => JobOutput::Error(format!("{label}: the result is empty")),
            Ok(m) => JobOutput::Replace {
                label: label.into(),
                remove: vec![target],
                add: vec![(name, m, color)],
                message: format!("{label}: done"),
            },
            Err(e) => JobOutput::Error(format!("{label} failed: {e}")),
        });
    }
}

/// Frame on a surface: Z = normal, Y = "up" (world Z projected, or the camera's up on
/// top/bottom faces), X = reading direction; then spun by `angle_deg`.
pub fn surface_frame(point: Vec3, normal: Vec3, cam_up: Vec3, angle_deg: f32) -> Mat4 {
    let n = normal.normalize_or(Vec3::Z);
    let pick_up = if n.z.abs() < 0.9 { Vec3::Z } else { cam_up };
    let mut up = (pick_up - n * pick_up.dot(n)).normalize_or_zero();
    if up == Vec3::ZERO {
        up = n.any_orthonormal_vector();
    }
    let right = up.cross(n);
    let spin = Quat::from_axis_angle(n, angle_deg.to_radians());
    let (right, up) = (spin * right, spin * up);
    Mat4::from_cols(right.extend(0.0), up.extend(0.0), n.extend(0.0), Vec4::new(point.x, point.y, point.z, 1.0))
}

/// Hover preview of a unit-depth shape on a surface: raised, or sunk in for engraving.
fn surface_preview(frame: Mat4, depth: f32, engrave: bool) -> Mat4 {
    let d = depth.max(0.05);
    if engrave {
        frame * Mat4::from_translation(Vec3::new(0.0, 0.0, -d)) * Mat4::from_scale(Vec3::new(1.0, 1.0, d))
    } else {
        frame * Mat4::from_scale(Vec3::new(1.0, 1.0, d))
    }
}

/// Right-hand panel for the text tool.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Text");
    let t = &mut app.settings.text;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut t.mode, TextMode::Object, "New object").on_hover_text("Separate text object on the plate");
        ui.selectable_value(&mut t.mode, TextMode::Emboss, "Emboss").on_hover_text("Raised letters joined onto a face");
        ui.selectable_value(&mut t.mode, TextMode::Engrave, "Engrave").on_hover_text("Letters cut into a face");
    });
    ui.add(egui::TextEdit::multiline(&mut t.text).desired_rows(2).desired_width(f32::INFINITY).hint_text("Type your text"));

    // font picker with a search box
    ui.horizontal(|ui| {
        ui.label("Font");
        match app.text_rt.fonts.clone() {
            None => {
                ui.spinner();
                ui.weak("finding fonts…");
            }
            Some(lib) => {
                let filter = &mut app.text_rt.filter;
                egui::ComboBox::from_id_salt("font")
                    .width(ui.available_width() - 8.0)
                    .height(360.0)
                    .selected_text(t.font.as_str())
                    .show_ui(ui, |ui| {
                        let r = ui.add(egui::TextEdit::singleline(filter).hint_text("Search fonts…"));
                        r.request_focus();
                        let f = filter.to_lowercase();
                        for e in lib.entries.iter().filter(|e| f.is_empty() || e.label.to_lowercase().contains(&f)) {
                            if ui.selectable_label(t.font == e.label, &e.label).clicked() {
                                t.font = e.label.clone();
                                ui.close();
                            }
                        }
                    });
            }
        }
    });
    egui::Grid::new("text_params").num_columns(2).show(ui, |ui| {
        ui.label("Letter height");
        ui.add(num(&mut t.size, Kind::Length).speed(0.1).range(0.5..=1000.0).suffix(" mm")).on_hover_text("Height of capital letters");
        ui.end_row();
        ui.label(match t.mode {
            TextMode::Engrave => "Engrave depth",
            TextMode::Emboss => "Raise by",
            TextMode::Object => "Thickness",
        });
        ui.add(num(&mut t.depth, Kind::Length).speed(0.05).range(0.1..=500.0).suffix(" mm"));
        ui.end_row();
        ui.label("Letter spacing");
        ui.add(num(&mut t.spacing, Kind::Length).speed(0.05).range(-50.0..=200.0).suffix(" mm"));
        ui.end_row();
        ui.label("Angle");
        ui.add(num(&mut t.angle, Kind::Angle).speed(1.0).range(-360.0..=360.0).suffix("°"));
        ui.end_row();
        if t.text.contains('\n') {
            ui.label("Line spacing");
            ui.add(egui::DragValue::new(&mut t.line_spacing).speed(0.01).range(0.5..=3.0).suffix("×"));
            ui.end_row();
            ui.label("Align");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut t.align, 0, "Left");
                ui.selectable_value(&mut t.align, 1, "Centre");
                ui.selectable_value(&mut t.align, 2, "Right");
            });
            ui.end_row();
        }
    });
    let mode = t.mode;
    if mode == TextMode::Object {
        ui.checkbox(&mut t.standing, "Standing up (reads from the front)");
    }
    // status
    if let Some(e) = &app.text_rt.error {
        ui.colored_label(egui::Color32::from_rgb(255, 120, 100), format!("⚠ {e}"));
    } else if let Some(b) = app.text_base() {
        let s = b.mesh.bounds().size();
        ui.weak(format!("{:.1} × {:.1} mm, {} triangles", s.x, s.y, b.mesh.triangles.len()));
    } else if !app.settings.text.text.trim().is_empty() && app.text_rt.fonts.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("shaping letters…");
        });
    }
    ui.add_space(4.0);
    match mode {
        TextMode::Object => {
            if ui.add_enabled(app.text_base().is_some(), egui::Button::new(RichText::new("Add to plate").strong())).clicked() {
                app.add_text_object(None);
                app.tool = crate::app::Tool::Select;
            }
            ui.weak("…or click the plate or any face to stand it there (Shift+click to place more).");
        }
        _ => {
            ui.label(if mode == TextMode::Emboss {
                "Hover over a face to preview, click to emboss."
            } else {
                "Hover over a face to preview, click to engrave."
            });
            ui.checkbox(&mut app.settings.text.wrap, "Follow curved surfaces")
                .on_hover_text("Bend the letters around cylinders and spheres");
            ui.weak("Text reads upright on side faces; on top faces it follows the view.");
        }
    }
    if ui.button("Done").clicked() {
        app.tool = Tool::Select;
    }
    ui.separator();
}
