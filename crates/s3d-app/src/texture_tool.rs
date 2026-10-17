//! Texture tool: choose a texture (built-in pattern or your own picture), choose where
//! (whole object, or a Fill / Brush area like Select faces), and it is pressed into the
//! surface as real geometry, with a live preview.

use crate::app::{App, Tool};
use crate::expr::Kind;
use crate::faces_tool::SelMode;
use crate::preview::PreviewKind;
use crate::ui::num;
use egui::RichText;
use s3d_core::texture::{self, HeightMap, Mapping, Relief, BUILTINS};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TexArea {
    Whole,
    Fill,
    Brush,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TexMapping {
    Triplanar,
    Cylinder,
    Sphere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TexRelief {
    Raise,
    Engrave,
    Both,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TextureSettings {
    /// Index into the built-in list; `None` = your own picture.
    pub builtin: Option<usize>,
    pub area: TexArea,
    pub radius: f32,
    pub angle: f32,
    pub tile: f32,
    pub depth: f32,
    pub rotation: f32,
    pub mapping: TexMapping,
    pub relief: TexRelief,
    pub invert: bool,
    pub smooth: u32,
    pub fade: f32,
    pub detail: f32,
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            builtin: Some(0),
            area: TexArea::Whole,
            radius: 3.0,
            angle: 30.0,
            tile: 5.0,
            depth: 0.6,
            rotation: 0.0,
            mapping: TexMapping::Triplanar,
            relief: TexRelief::Raise,
            invert: false,
            smooth: 0,
            fade: 1.0,
            detail: 0.0,
        }
    }
}

#[derive(Default)]
pub struct TextureRuntime {
    /// Your own picture: (file name, height map).
    pub custom: Option<(String, Arc<HeightMap>)>,
    /// The map in use (after smoothing) and its key.
    pub map: Option<(u64, Arc<HeightMap>)>,
    thumbs: Vec<egui::TextureHandle>,
    custom_thumb: Option<(String, egui::TextureHandle)>,
}

fn thumb_image(m: &HeightMap, size: usize) -> egui::ColorImage {
    let mut px = Vec::with_capacity(size * size);
    for j in 0..size {
        for i in 0..size {
            // two repeats, lit from the top-left, so the relief reads
            let (u, v) = (i as f32 / size as f32 * 2.0, j as f32 / size as f32 * 2.0);
            let e = 1.0 / m.w as f32;
            let h = m.sample(u, v);
            let shade = (m.sample(u - e, v - e) - m.sample(u + e, v + e)) * 6.0;
            let c = ((0.25 + 0.6 * h + shade) * 255.0).clamp(0.0, 255.0) as u8;
            px.push(egui::Color32::from_rgb(c, c, c));
        }
    }
    egui::ColorImage { size: [size, size], pixels: px, source_size: egui::vec2(size as f32, size as f32) }
}

impl App {
    pub fn start_texture_tool(&mut self) {
        self.tool = Tool::Texture;
        if let Some(&id) = self.selection.first() {
            if self.settings.texture.area == TexArea::Whole {
                self.face_select_all_of(id);
            }
        }
    }

    /// Whole-object mode: target `id` with every triangle selected.
    pub fn face_select_all_of(&mut self, id: s3d_core::ObjectId) {
        if self.face_target(id) {
            self.face_select_all(0);
        }
    }

    /// The height map to use (built-in or picture, smoothed), cached.
    pub fn texture_map(&mut self) -> Option<Arc<HeightMap>> {
        let s = &self.settings.texture;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (s.builtin, s.smooth, self.tex_rt.custom.as_ref().map(|c| c.0.clone())).hash(&mut h);
        let key = h.finish();
        if let Some((k, m)) = &self.tex_rt.map {
            if *k == key {
                return Some(m.clone());
            }
        }
        let base = match s.builtin {
            Some(i) => Arc::new(texture::builtin(BUILTINS[i.min(BUILTINS.len() - 1)].0)),
            None => self.tex_rt.custom.as_ref()?.1.clone(),
        };
        let m = if s.smooth > 0 { Arc::new(base.blurred(s.smooth)) } else { base };
        self.tex_rt.map = Some((key, m.clone()));
        Some(m)
    }

    /// Options in the object's own units.
    pub fn texture_options(&self, scale: f32) -> texture::TextureOptions {
        let s = &self.settings.texture;
        let k = scale.max(1e-6);
        texture::TextureOptions {
            depth: s.depth / k,
            tile: s.tile / k,
            rotation_deg: s.rotation,
            mapping: match s.mapping {
                TexMapping::Triplanar => Mapping::Triplanar,
                TexMapping::Cylinder => Mapping::Cylinder,
                TexMapping::Sphere => Mapping::Sphere,
            },
            relief: match s.relief {
                TexRelief::Raise => Relief::Raise,
                TexRelief::Engrave => Relief::Engrave,
                TexRelief::Both => Relief::Both,
            },
            invert: s.invert,
            fade: s.fade / k,
            detail: s.detail / k,
            ..Default::default()
        }
    }

    pub fn load_texture_picture(&mut self) {
        let Some(p) = rfd::FileDialog::new().add_filter("Pictures", &s3d_core::image3d::IMAGE_EXTS).pick_file() else { return };
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| s3d_core::image3d::load_image(&b).map_err(|e| e.to_string())) {
            Ok(g) => {
                let name = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                self.tex_rt.custom = Some((name, Arc::new(HeightMap::from_gray(&g))));
                self.tex_rt.custom_thumb = None;
                self.settings.texture.builtin = None;
            }
            Err(e) => self.error(format!("Can't open that picture: {e}")),
        }
    }
}

/// Right-hand panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Texture");
    // swatches
    if app.tex_rt.thumbs.is_empty() {
        for (b, name) in BUILTINS {
            let img = thumb_image(&texture::builtin(b), 56);
            app.tex_rt.thumbs.push(ui.ctx().load_texture(format!("tex-{name}"), img, egui::TextureOptions::LINEAR));
        }
    }
    if let Some((name, m)) = &app.tex_rt.custom {
        if app.tex_rt.custom_thumb.as_ref().is_none_or(|t| &t.0 != name) {
            let t = ui.ctx().load_texture("tex-custom", thumb_image(m, 56), egui::TextureOptions::LINEAR);
            app.tex_rt.custom_thumb = Some((name.clone(), t));
        }
    }
    let mut load = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (i, (_, name)) in BUILTINS.iter().enumerate() {
            let sel = app.settings.texture.builtin == Some(i);
            let r = ui
                .add(egui::Button::image(egui::Image::new(&app.tex_rt.thumbs[i]).fit_to_exact_size(egui::vec2(44.0, 44.0))).selected(sel));
            if r.on_hover_text(*name).clicked() {
                app.settings.texture.builtin = Some(i);
            }
        }
        if let Some((name, t)) = &app.tex_rt.custom_thumb {
            let sel = app.settings.texture.builtin.is_none();
            if ui
                .add(egui::Button::image(egui::Image::new(t).fit_to_exact_size(egui::vec2(44.0, 44.0))).selected(sel))
                .on_hover_text(name)
                .clicked()
            {
                app.settings.texture.builtin = None;
            }
        }
        if ui.button("Picture…").on_hover_text("Use your own picture as a height map (white = high). Seamless textures work best").clicked()
        {
            load = true;
        }
    });
    if load {
        app.load_texture_picture();
    }
    let label = match app.settings.texture.builtin {
        Some(i) => BUILTINS[i.min(BUILTINS.len() - 1)].1.to_string(),
        None => app.tex_rt.custom.as_ref().map(|c| c.0.clone()).unwrap_or_default(),
    };
    ui.label(RichText::new(label).strong());
    app.texture_map();

    // where
    let s = &mut app.settings.texture;
    let before = s.area;
    ui.horizontal(|ui| {
        ui.label("Where");
        ui.selectable_value(&mut s.area, TexArea::Whole, "Whole object");
        ui.selectable_value(&mut s.area, TexArea::Fill, "Fill").on_hover_text("A face or smooth area, up to sharp edges");
        ui.selectable_value(&mut s.area, TexArea::Brush, "Brush").on_hover_text("Paint the area to texture");
    });
    match s.area {
        TexArea::Brush => {
            ui.horizontal(|ui| {
                ui.label("Brush size");
                ui.add(num(&mut s.radius, Kind::Length).speed(0.05).range(0.1..=200.0).suffix(" mm"));
            });
        }
        TexArea::Fill => {
            ui.horizontal(|ui| {
                ui.label("Stop at edges over");
                ui.add(egui::Slider::new(&mut s.angle, 1.0..=180.0).suffix("°").integer());
            });
        }
        TexArea::Whole => {}
    }
    let area = s.area;
    if area != before {
        app.face_select_all(1);
        if area == TexArea::Whole {
            if let Some(id) = app.face_rt.obj.or(app.selection.first().copied()) {
                app.face_select_all_of(id);
            }
        }
    }
    ui.weak(match area {
        TexArea::Whole => "Click the object to texture.",
        _ => "Click or drag to choose the area; Shift removes.",
    });

    // how
    let s = &mut app.settings.texture;
    egui::Grid::new("texture").num_columns(2).show(ui, |ui| {
        ui.label("Size");
        ui.add(num(&mut s.tile, Kind::Length).speed(0.05).range(0.2..=500.0).suffix(" mm")).on_hover_text("One repeat of the texture");
        ui.end_row();
        ui.label("Depth");
        ui.add(num(&mut s.depth, Kind::Length).speed(0.01).range(0.02..=50.0).suffix(" mm"));
        ui.end_row();
        ui.label("Relief");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.relief, TexRelief::Raise, "Raise").on_hover_text("Added on top: the part grows by the depth");
            ui.selectable_value(&mut s.relief, TexRelief::Engrave, "Engrave").on_hover_text("Cut in: the outside stays where it was");
            ui.selectable_value(&mut s.relief, TexRelief::Both, "Both");
        });
        ui.end_row();
        ui.label("Mapping");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.mapping, TexMapping::Triplanar, "Box")
                .on_hover_text("Projected from all sides — works on any shape");
            ui.selectable_value(&mut s.mapping, TexMapping::Cylinder, "Cylinder")
                .on_hover_text("Wrapped around the vertical axis, no seam");
            ui.selectable_value(&mut s.mapping, TexMapping::Sphere, "Sphere");
        });
        ui.end_row();
        ui.label("Rotation");
        ui.add(num(&mut s.rotation, Kind::Angle).speed(1.0).range(-360.0..=360.0).suffix("°"));
        ui.end_row();
        ui.label("Fade at edges");
        ui.add(num(&mut s.fade, Kind::Length).speed(0.05).range(0.0..=100.0).suffix(" mm"))
            .on_hover_text("Blends the relief into the untextured surface");
        ui.end_row();
        ui.label("Soften");
        ui.add(egui::Slider::new(&mut s.smooth, 0..=6));
        ui.end_row();
        ui.label("Detail");
        ui.add(num(&mut s.detail, Kind::Length).speed(0.01).range(0.0..=10.0).max_decimals(3).suffix(" mm"))
            .on_hover_text("Triangle size of the relief. 0 = automatic (size / 10). Smaller = sharper but heavier");
        ui.end_row();
    });
    ui.checkbox(&mut s.invert, "Invert (swap high and low)");

    // live preview while there is an area
    let has_area = app.face_rt.count() > 0;
    let showing = app.preview.as_ref().is_some_and(|p| p.kind == PreviewKind::Texture);
    if has_area && !showing {
        if let Some(id) = app.face_rt.obj {
            app.selection = vec![id];
        }
        app.start_preview(PreviewKind::Texture);
    } else if !has_area && showing {
        app.cancel_preview();
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(has_area, egui::Button::new(RichText::new("✔ Apply").strong())).on_hover_text("Enter").clicked() {
            app.apply_preview();
        }
        if ui.button("Done").clicked() {
            app.cancel_preview();
            app.tool = Tool::Select;
        }
    });
    ui.weak("The preview updates as you change things. Big areas with fine detail take a few seconds.");
    ui.separator();
}

/// Selection mode for the shared face selection while texturing: None = whole object.
pub fn area_mode(s: &TextureSettings) -> Option<SelMode> {
    match s.area {
        TexArea::Whole => None,
        TexArea::Fill => Some(SelMode::Fill),
        TexArea::Brush => Some(SelMode::Brush),
    }
}
