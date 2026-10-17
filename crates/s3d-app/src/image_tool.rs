//! Picture → 3D tool (3D Builder's *Load image*): Contour, Height map, Edge, Stamp and
//! Lithophane, placed as a new object or embossed / engraved onto a face.

use crate::app::{App, Tool};
use crate::expr::Kind;
use crate::preview::BgTask;
use crate::text_tool::{surface_frame, TextHover, TextMode};
use crate::ui::num;
use egui::RichText;
use glam::{Mat4, Vec3};
use s3d_core::image3d::{self, Gray, ImageMode, ImageOptions};
use s3d_core::scene::MeshHandle;
use s3d_core::Mesh;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    /// 0 contour, 1 height map, 2 edge, 3 stamp, 4 lithophane
    pub mode: u8,
    pub width: f32,
    pub resolution: u32,
    pub smooth: u32,
    pub levels: u32,
    pub threshold: f32,
    pub invert: bool,
    pub height: f32,
    pub base: f32,
    pub frame: f32,
    pub use_alpha: bool,
    pub reduce: bool,
    pub place: TextMode,
    pub angle: f32,
    /// Bend the picture around curved surfaces.
    pub wrap: bool,
}

impl Default for ImageSettings {
    fn default() -> Self {
        let o = ImageOptions::default();
        Self {
            mode: 0,
            width: o.width,
            resolution: o.resolution,
            smooth: o.smooth,
            levels: o.levels,
            threshold: o.threshold,
            invert: o.invert,
            height: o.height,
            base: o.base,
            frame: o.frame,
            use_alpha: o.use_alpha,
            reduce: true,
            place: TextMode::Object,
            angle: 0.0,
            wrap: true,
        }
    }
}

const MODES: [(ImageMode, &str, &str); 5] = [
    (ImageMode::Contour, "Contour", "Outline of the dark areas, extruded — logos, icons, silhouettes"),
    (ImageMode::HeightMap, "Height map", "Brightness becomes height (white = high)"),
    (ImageMode::Edge, "Edge", "Edges in the picture become ridges"),
    (ImageMode::Stamp, "Stamp", "Mirrored raised contour on a base plate, for stamping"),
    (ImageMode::Lithophane, "Lithophane", "Thin panel that shows the picture when lit from behind (dark = thick)"),
];

impl ImageSettings {
    pub fn mode(&self) -> ImageMode {
        MODES.get(self.mode as usize).map(|m| m.0).unwrap_or(ImageMode::Contour)
    }
    pub fn options(&self, final_build: bool) -> ImageOptions {
        ImageOptions {
            mode: self.mode(),
            width: self.width,
            resolution: self.resolution,
            smooth: self.smooth,
            levels: self.levels,
            threshold: self.threshold,
            invert: self.invert,
            height: self.height,
            base: self.base,
            frame: self.frame,
            use_alpha: self.use_alpha,
            reduce: final_build && self.reduce,
        }
    }
}

pub struct Source {
    pub name: String,
    pub gray: Arc<Gray>,
    pub id: u64,
}

type Built = Result<(Mesh, egui::ColorImage), String>;

#[derive(Default)]
pub struct ImageRuntime {
    pub source: Option<Source>,
    loading: Option<BgTask<Result<(String, Gray), String>>>,
    task: Option<BgTask<Built>>,
    /// Preview mesh (not reduced) and its key.
    pub mesh: Option<(u64, MeshHandle)>,
    /// The same mesh squashed to z ∈ [0, 1] for embossing.
    pub unit: Option<MeshHandle>,
    pub thumb: Option<egui::TextureHandle>,
    pub error: Option<String>,
    error_key: u64,
    pub hover: Option<TextHover>,
}

impl App {
    /// Load a picture (in the background) and open the picture tool.
    pub fn open_image(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.tool = Tool::Image;
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        self.img_rt.error = None;
        self.img_rt.loading = Some(BgTask::spawn(ctx, 0, move |_p| {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            image3d::load_image(&bytes).map(|g| (name, g)).map_err(|e| e.to_string())
        }));
    }

    pub fn image_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Pictures", &image3d::IMAGE_EXTS).pick_file() {
            self.open_image(p, ctx);
        }
    }

    fn image_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.img_rt.source.as_ref().map(|s| s.id).hash(&mut h);
        format!("{:?}", self.settings.image.options(false)).hash(&mut h);
        h.finish()
    }

    pub fn tick_image(&mut self, ctx: &egui::Context) {
        let rt = &mut self.img_rt;
        if let Some(r) = rt.loading.as_ref().and_then(|t| t.poll()) {
            rt.loading = None;
            match r {
                Some(Ok((name, g))) => {
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    rt.source = Some(Source { name, gray: Arc::new(g), id });
                    rt.mesh = None;
                    rt.unit = None;
                }
                Some(Err(e)) => rt.error = Some(e),
                None => rt.error = Some("could not read this picture".into()),
            }
        }
        if self.tool != Tool::Image {
            self.img_rt.hover = None;
            return;
        }
        if let Some(r) = self.img_rt.task.as_ref().and_then(|t| t.poll()) {
            let t = self.img_rt.task.take().unwrap();
            match r {
                Some(Ok((m, thumb))) => {
                    let zmax = m.bounds().max.z.max(1e-3);
                    let unit = m.transformed(&Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0 / zmax)));
                    self.img_rt.mesh = Some((t.key, MeshHandle::new(m)));
                    self.img_rt.unit = Some(MeshHandle::new(unit));
                    self.img_rt.thumb = Some(ctx.load_texture("image-thumb", thumb, egui::TextureOptions::LINEAR));
                    self.img_rt.error = None;
                }
                Some(Err(e)) => {
                    self.img_rt.error = Some(e);
                    self.img_rt.error_key = t.key;
                }
                None => {
                    self.img_rt.error = Some("this picture could not be converted".into());
                    self.img_rt.error_key = t.key;
                }
            }
        }
        let Some(src) = &self.img_rt.source else { return };
        let key = self.image_key();
        let current =
            self.img_rt.mesh.as_ref().is_some_and(|m| m.0 == key) || (self.img_rt.error.is_some() && self.img_rt.error_key == key);
        let running = self.img_rt.task.as_ref().is_some_and(|t| t.key == key);
        if !current && !running {
            self.img_rt.error = None;
            let gray = src.gray.clone();
            let o = self.settings.image.options(false);
            self.img_rt.task = Some(BgTask::spawn(ctx, key, move |p| {
                let mesh = image3d::image_mesh(&gray, &o, p).map_err(|e| e.to_string())?;
                Ok((mesh, thumbnail(&gray, &o)))
            }));
        }
        if self.img_rt.task.is_some() || self.img_rt.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub fn image_preview_mesh(&self) -> Option<&MeshHandle> {
        self.img_rt.mesh.as_ref().map(|m| &m.1)
    }

    /// World matrix of a new picture object centred on `at` on the plate.
    pub fn image_object_matrix(&self, at: Vec3) -> Mat4 {
        Mat4::from_translation(at) * Mat4::from_rotation_z(self.settings.image.angle.to_radians())
    }

    /// Matrix for a new picture object: on a face (`h.frame`), on the plate, or beside the others.
    pub fn image_object_matrix_h(&self, h: Option<TextHover>) -> Mat4 {
        match h {
            Some(h) if h.frame != Mat4::IDENTITY => h.frame,
            Some(h) => self.image_object_matrix(h.point),
            None => self.image_object_matrix(self.image_default_spot()),
        }
    }

    pub fn image_default_spot(&self) -> Vec3 {
        let existing = self.scene.bounds();
        if existing.is_empty() {
            return Vec3::ZERO;
        }
        let w = self.image_preview_mesh().map(|m| m.mesh.bounds().size().x).unwrap_or(self.settings.image.width);
        Vec3::new(existing.max.x + 10.0 + w * 0.5, existing.center().y, 0.0)
    }

    pub fn image_surface_frame(&self, point: Vec3, normal: Vec3, cam_up: Vec3) -> Mat4 {
        surface_frame(point, normal, cam_up, self.settings.image.angle)
    }

    /// Add the picture as a new object (final build in the background, reduced if asked).
    pub fn add_image_object(&mut self, ctx: &egui::Context, at: Option<TextHover>) {
        let Some(src) = &self.img_rt.source else {
            self.error("Open a picture first");
            return;
        };
        let m = self.image_object_matrix_h(at);
        let gray = src.gray.clone();
        let o = self.settings.image.options(true);
        let name = std::path::Path::new(&src.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or("Picture".into());
        let label = MODES.iter().find(|x| x.0 == o.mode).map(|x| x.1).unwrap_or("Picture");
        let color = s3d_core::scene::PALETTE[self.scene.objects.len() % s3d_core::scene::PALETTE.len()];
        self.spawn_job(ctx, format!("Building {label}…"), move |p| match image3d::image_mesh(&gray, &o, p) {
            Ok(mesh) => {
                let n = mesh.triangles.len();
                crate::app::JobOutput::Replace {
                    label: format!("Add {label}"),
                    remove: vec![],
                    add: vec![(format!("{name} ({label})"), mesh.transformed(&m), color)],
                    message: format!("Added {label} from {name}: {n} triangles"),
                }
            }
            Err(s3d_core::Error::Cancelled) => crate::app::JobOutput::Error("cancelled".into()),
            Err(e) => crate::app::JobOutput::Error(format!("{label}: {e}")),
        });
    }

    pub fn apply_image_on_surface(&mut self, ctx: &egui::Context, hover: TextHover) {
        let Some(unit) = self.img_rt.unit.clone() else {
            self.error("Picture: still preparing");
            return;
        };
        let s = &self.settings.image;
        let engrave = s.place == TextMode::Engrave;
        let label = if engrave { "Engrave picture" } else { "Emboss picture" };
        let reach = s.width * 0.3;
        let depth = s.height;
        self.apply_on_surface(ctx, hover, &unit, depth, engrave, reach, label);
    }
}

/// What the mode "sees": black/white for contour & stamp, grey levels for height modes.
fn thumbnail(g: &Gray, o: &ImageOptions) -> egui::ColorImage {
    let mut o2 = *o;
    o2.resolution = o.resolution.min(320);
    let f = image3d::prepare(g, &o2);
    let mut px = Vec::with_capacity(f.w * f.h);
    for i in 0..f.w * f.h {
        let c = if !f.mask[i] {
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0)
        } else {
            let v = match o.mode {
                ImageMode::Contour | ImageMode::Stamp => {
                    if f.v[i] < o.threshold {
                        0.15
                    } else {
                        0.95
                    }
                }
                _ => f.v[i],
            };
            let b = (v.clamp(0.0, 1.0) * 255.0) as u8;
            egui::Color32::from_rgb(b, b, b)
        };
        px.push(c);
    }
    egui::ColorImage::new([f.w, f.h], px)
}

/// Right-hand panel for the picture tool.
pub fn panel(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.heading("Picture to 3D");
    ui.horizontal(|ui| {
        match &app.img_rt.source {
            Some(s) => ui.label(RichText::new(&s.name).strong()),
            None => ui.weak("No picture yet"),
        };
        if app.img_rt.loading.is_some() {
            ui.spinner();
        }
        if ui.button("Open picture…").on_hover_text("PNG, JPG, BMP, TGA, GIF, WebP — or drop one on the window").clicked() {
            app.image_dialog(ctx);
        }
    });
    if let Some(t) = &app.img_rt.thumb {
        let size = t.size_vec2();
        let w = ui.available_width().min(280.0);
        let h = (w * size.y / size.x.max(1.0)).min(200.0);
        let w = h * size.x / size.y.max(1.0);
        egui::Frame::new().fill(egui::Color32::from_rgb(120, 120, 128)).show(ui, |ui| {
            ui.add(egui::Image::new((t.id(), egui::vec2(w, h))));
        });
    }
    let s = &mut app.settings.image;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (i, (mode, name, tip)) in MODES.iter().enumerate() {
            if ui.selectable_label(s.mode as usize == i, *name).on_hover_text(*tip).clicked() && s.mode as usize != i {
                s.mode = i as u8;
                let (height, base) = ImageOptions::for_mode(*mode);
                s.height = height;
                s.base = base;
                s.reduce = *mode != ImageMode::Lithophane;
                if *mode == ImageMode::Lithophane && s.width < 80.0 {
                    s.width = 100.0;
                }
            }
        }
    });
    let mode = s.mode();
    let flat = matches!(mode, ImageMode::Contour | ImageMode::Stamp);
    ui.horizontal(|ui| {
        ui.label("Place");
        ui.selectable_value(&mut s.place, TextMode::Object, "New object");
        ui.selectable_value(&mut s.place, TextMode::Emboss, "Emboss").on_hover_text("Join it onto a face of an object");
        ui.selectable_value(&mut s.place, TextMode::Engrave, "Engrave").on_hover_text("Cut it into a face of an object");
    });
    egui::Grid::new("image_params").num_columns(2).show(ui, |ui| {
        ui.label("Width");
        ui.add(num(&mut s.width, Kind::Length).speed(0.5).range(2.0..=1000.0).suffix(" mm")).on_hover_text("Width of the result");
        ui.end_row();
        if flat {
            ui.label("Threshold");
            ui.add(egui::Slider::new(&mut s.threshold, 0.02..=0.98).max_decimals(2))
                .on_hover_text("Areas darker than this become solid (Invert to flip)");
            ui.end_row();
        } else {
            ui.label("Levels");
            ui.add(egui::Slider::new(&mut s.levels, 2..=256)).on_hover_text("Number of height steps (256 = smooth)");
            ui.end_row();
        }
        ui.label("Smooth");
        ui.add(egui::Slider::new(&mut s.smooth, 0..=8)).on_hover_text("Blur the picture first (removes noise and jagged edges)");
        ui.end_row();
        ui.label(match mode {
            ImageMode::Lithophane => "Extra thickness",
            ImageMode::Stamp => "Raised by",
            ImageMode::Contour => "Height",
            _ => "Relief",
        });
        ui.add(num(&mut s.height, Kind::Length).speed(0.05).range(0.1..=200.0).suffix(" mm"));
        ui.end_row();
        ui.label(if mode == ImageMode::Lithophane { "Thinnest" } else { "Base" });
        ui.add(num(&mut s.base, Kind::Length).speed(0.05).range(0.0..=100.0).suffix(" mm"));
        ui.end_row();
        if mode == ImageMode::Lithophane {
            ui.label("Frame");
            ui.add(num(&mut s.frame, Kind::Length).speed(0.1).range(0.0..=50.0).suffix(" mm"));
            ui.end_row();
        }
        ui.label("Detail");
        ui.add(egui::Slider::new(&mut s.resolution, 50..=1000).suffix(" px"))
            .on_hover_text("Pixels along the longest side. Higher = finer and slower.");
        ui.end_row();
        ui.label("Angle");
        ui.add(num(&mut s.angle, Kind::Angle).speed(1.0).range(-360.0..=360.0).suffix("°"));
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut s.invert, "Invert");
        if app.img_rt.source.as_ref().is_some_and(|x| x.gray.has_alpha()) {
            ui.checkbox(&mut s.use_alpha, "Use transparency as outline");
        }
        if !flat {
            ui.checkbox(&mut s.reduce, "Reduce triangles").on_hover_text("Merge flat areas when adding (slower to build)");
        }
    });
    let place = s.place;
    // status
    if let Some(e) = &app.img_rt.error {
        ui.colored_label(egui::Color32::from_rgb(255, 120, 100), format!("⚠ {e}"));
    } else if let Some(m) = app.image_preview_mesh() {
        let b = m.mesh.bounds().size();
        let busy = if app.img_rt.task.is_some() { "  ·  updating…" } else { "" };
        ui.weak(format!("{:.1} × {:.1} × {:.1} mm{busy}", b.x, b.y, b.z));
    } else if app.img_rt.source.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("converting…");
        });
    }
    ui.add_space(4.0);
    match place {
        TextMode::Object => {
            if ui.add_enabled(app.img_rt.mesh.is_some(), egui::Button::new(RichText::new("Add to plate").strong())).clicked() {
                app.add_image_object(ctx, None);
            }
            ui.weak("…or click the plate or any face to stand it there.");
        }
        _ => {
            ui.label("Hover over a face to preview, click to apply.");
            ui.checkbox(&mut app.settings.image.wrap, "Follow curved surfaces")
                .on_hover_text("Bend the picture around cylinders and spheres");
        }
    }
    if ui.button("Done").clicked() {
        app.tool = Tool::Select;
    }
    ui.separator();
}
