//! Application state, actions and background jobs.

use crate::camera::OrbitCamera;
use crate::render::ViewportRenderer;
use b3d_core::analysis::{analyze, MeshReport};
use b3d_core::csg::{self, BoolOp};
use b3d_core::history::History;
use b3d_core::io::{self, Format, IoObject};
use b3d_core::primitives::Primitive;
use b3d_core::repair::{self, RepairOptions};
use b3d_core::scene::{MeshHandle, ObjectId, Scene, Transform};
use b3d_core::{Aabb, Mesh};
use glam::{Mat4, Quat, Vec2, Vec3};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;
use transform_gizmo_egui::Gizmo;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub plate: Vec2,
    pub grid_step: f32,
    pub snap_enabled: bool,
    pub snap_move: f32,
    pub snap_angle_deg: f32,
    pub snap_scale: f32,
    pub decimals: usize,
    pub show_plate: bool,
    pub show_problems: bool,
    pub show_dimensions: bool,
    pub dark: bool,
    pub camera: OrbitCamera,
    pub weld_tolerance: f32,
    pub recent: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            plate: Vec2::new(256.0, 256.0),
            grid_step: 10.0,
            snap_enabled: false,
            snap_move: 1.0,
            snap_angle_deg: 15.0,
            snap_scale: 0.1,
            decimals: 2,
            show_plate: true,
            show_problems: true,
            show_dimensions: true,
            dark: true,
            camera: OrbitCamera::default(),
            weld_tolerance: 1e-4,
            recent: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Measure,
    LayFlat,
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoKind {
    Move,
    Rotate,
    Scale,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosRef {
    Center,
    BottomCenter,
    MinCorner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutKeep {
    Both,
    Above,
    Below,
}

#[derive(Debug, Clone)]
pub struct CutState {
    pub axis: usize,
    pub offset: f32,
    pub keep: CutKeep,
}

#[derive(Debug, Clone, Default)]
pub struct MeasureState {
    pub a: Option<Vec3>,
    pub b: Option<Vec3>,
    pub snap_vertices: bool,
}

#[derive(Debug, Clone)]
pub struct ArrayParams {
    pub count: [u32; 3],
    pub spacing: Vec3,
}

pub enum JobOutput {
    Imported {
        groups: Vec<(PathBuf, Vec<IoObject>)>,
    },
    /// Remove objects and add new world-space meshes.
    Replace {
        label: String,
        remove: Vec<ObjectId>,
        add: Vec<(String, Mesh, [u8; 4])>,
        message: String,
    },
    /// Replace local meshes in place (transform kept).
    ReplaceMeshes {
        label: String,
        meshes: Vec<(ObjectId, Mesh)>,
        message: String,
    },
    Saved {
        path: PathBuf,
    },
    Error(String),
}

pub struct Job {
    pub label: String,
    pub rx: Receiver<JobOutput>,
    pub started: Instant,
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

pub struct App {
    pub scene: Scene,
    pub history: History,
    pub selection: Vec<ObjectId>,
    pub hovered: Option<ObjectId>,
    pub settings: Settings,
    pub gizmo: Gizmo,
    pub gizmo_kind: GizmoKind,
    pub gizmo_dragging: bool,
    pub tool: Tool,
    pub measure: MeasureState,
    pub cut: CutState,
    pub array: ArrayParams,
    pub add_dialog: Option<Primitive>,
    pub show_settings: bool,
    pub show_about: bool,
    pub show_shortcuts: bool,
    pub pos_ref: PosRef,
    pub lock_ratio: bool,
    pub pending_edit: Option<String>,
    pub report_cache: Option<(u64, MeshReport)>,
    pub status: Option<Status>,
    pub job: Option<Job>,
    pub current_file: Option<PathBuf>,
    pub dirty: bool,
    pub cursor_plate: Option<Vec3>,
    pub viewport_aspect: f32,
    pub viewport_ok: bool,
    pub last_pointer: Option<egui::Pos2>,
    pub fit_on_next_frame: bool,
    pub last_title: String,
    pub encode_srgb: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, "settings")).unwrap_or_default();
        let mut encode_srgb = true;
        let viewport_ok = if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let r = ViewportRenderer::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(r);
            encode_srgb = !rs.target_format.is_srgb();
            log::info!("render target {:?}, adapter {:?}", rs.target_format, rs.adapter.get_info().name);
            true
        } else {
            false
        };
        crate::ui::apply_theme(&cc.egui_ctx, settings.dark);
        let mut app = Self {
            scene: Scene::default(),
            history: History::new(100),
            selection: vec![],
            hovered: None,
            settings,
            gizmo: Gizmo::default(),
            gizmo_kind: GizmoKind::Move,
            gizmo_dragging: false,
            tool: Tool::Select,
            measure: MeasureState { snap_vertices: true, ..Default::default() },
            cut: CutState { axis: 2, offset: 0.0, keep: CutKeep::Both },
            array: ArrayParams { count: [3, 1, 1], spacing: Vec3::new(25.0, 25.0, 25.0) },
            add_dialog: None,
            show_settings: false,
            show_about: false,
            show_shortcuts: false,
            pos_ref: PosRef::BottomCenter,
            lock_ratio: true,
            pending_edit: None,
            report_cache: None,
            status: None,
            job: None,
            current_file: None,
            dirty: false,
            cursor_plate: None,
            viewport_aspect: 1.5,
            viewport_ok,
            last_pointer: None,
            fit_on_next_frame: false,
            last_title: String::new(),
            encode_srgb,
        };
        // command-line files
        let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).filter(|p| Format::from_path(p).is_some()).collect();
        if !files.is_empty() {
            app.import_files(files, &cc.egui_ctx);
        }
        app
    }

    // ---------- status / history helpers ----------

    pub fn info(&mut self, s: impl Into<String>) {
        self.status = Some(Status { text: s.into(), error: false });
    }
    pub fn error(&mut self, s: impl Into<String>) {
        let s = s.into();
        log::error!("{s}");
        self.status = Some(Status { text: s, error: true });
    }

    /// Snapshot for undo before a discrete change.
    pub fn checkpoint(&mut self, label: &str) {
        self.history.push(label, &self.scene);
        self.dirty = true;
    }

    /// Snapshot for continuous edits (drag values): only once per interaction.
    pub fn checkpoint_coalesced(&mut self, label: &str) {
        if self.pending_edit.as_deref() != Some(label) {
            self.checkpoint(label);
            self.pending_edit = Some(label.to_string());
        }
    }

    pub fn undo(&mut self) {
        if let Some(l) = self.history.undo(&mut self.scene) {
            self.info(format!("Undid: {l}"));
            self.prune_selection();
            self.dirty = true;
        }
    }
    pub fn redo(&mut self) {
        if let Some(l) = self.history.redo(&mut self.scene) {
            self.info(format!("Redid: {l}"));
            self.prune_selection();
            self.dirty = true;
        }
    }
    fn prune_selection(&mut self) {
        let scene = &self.scene;
        self.selection.retain(|id| scene.get(*id).is_some());
    }

    pub fn is_selected(&self, id: ObjectId) -> bool {
        self.selection.contains(&id)
    }
    pub fn select_only(&mut self, id: ObjectId) {
        self.selection = vec![id];
    }
    pub fn toggle_select(&mut self, id: ObjectId) {
        if let Some(i) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(i);
        } else {
            self.selection.push(id);
        }
    }
    pub fn selection_bounds(&self) -> Aabb {
        self.scene.bounds_of(&self.selection)
    }

    // ---------- background jobs ----------

    pub fn spawn_job(&mut self, ctx: &egui::Context, label: impl Into<String>, f: impl FnOnce() -> JobOutput + Send + 'static) {
        if self.job.is_some() {
            self.error("Another operation is still running");
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let out =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| JobOutput::Error("operation crashed".into()));
            let _ = tx.send(out);
            ctx.request_repaint();
        });
        self.job = Some(Job { label: label.into(), rx, started: Instant::now() });
    }

    pub fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(out) = job.rx.try_recv() else { return };
        let elapsed = job.started.elapsed().as_secs_f32();
        self.job = None;
        match out {
            JobOutput::Imported { groups } => {
                self.checkpoint("Import");
                let mut sel = vec![];
                for (path, objects) in groups {
                    sel.extend(self.finish_import(objects, &path));
                }
                self.selection = sel;
            }
            JobOutput::Replace { label, remove, add, message } => {
                self.checkpoint(&label);
                let mut new_ids = vec![];
                // results take the list position of the first removed object
                let insert_at = remove.iter().filter_map(|id| self.scene.objects.iter().position(|o| o.id == *id)).min();
                for id in &remove {
                    self.scene.remove(*id);
                }
                for (name, mesh, color) in add {
                    if mesh.is_empty() {
                        continue;
                    }
                    new_ids.push(self.scene.add_world_mesh(name, mesh, Some(color)));
                }
                if let Some(at) = insert_at {
                    let n = new_ids.len();
                    let len = self.scene.objects.len();
                    let tail: Vec<_> = self.scene.objects.drain(len - n..).collect();
                    for (k, o) in tail.into_iter().enumerate() {
                        self.scene.objects.insert((at + k).min(self.scene.objects.len()), o);
                    }
                }
                self.selection = new_ids;
                self.info(format!("{message} ({elapsed:.1}s)"));
            }
            JobOutput::ReplaceMeshes { label, meshes, message } => {
                self.checkpoint(&label);
                for (id, mesh) in meshes {
                    if let Some(o) = self.scene.get_mut(id) {
                        o.mesh = MeshHandle::new(mesh);
                    }
                }
                self.report_cache = None;
                self.info(format!("{message} ({elapsed:.1}s)"));
            }
            JobOutput::Saved { path } => {
                self.info(format!("Saved {}", path.display()));
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")) {
                    self.current_file = Some(path.clone());
                    self.dirty = false;
                }
                self.push_recent(path);
            }
            JobOutput::Error(e) => self.error(e),
        }
    }

    fn push_recent(&mut self, p: PathBuf) {
        self.settings.recent.retain(|r| r != &p);
        self.settings.recent.insert(0, p);
        self.settings.recent.truncate(8);
    }

    // ---------- files ----------

    pub fn open_dialog(&mut self, ctx: &egui::Context) {
        let files = rfd::FileDialog::new()
            .add_filter("3D models", &Format::IMPORT_EXTS)
            .add_filter("3MF", &["3mf"])
            .add_filter("STL", &["stl"])
            .add_filter("OBJ", &["obj"])
            .add_filter("PLY", &["ply"])
            .pick_files();
        if let Some(files) = files {
            self.import_files(files, ctx);
        }
    }

    pub fn import_files(&mut self, files: Vec<PathBuf>, ctx: &egui::Context) {
        let label =
            format!("Loading {}", files.iter().map(|f| f.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(", "));
        self.spawn_job(ctx, label, move || {
            let mut groups = vec![];
            for f in &files {
                match io::load(f) {
                    Ok(o) => groups.push((f.clone(), o)),
                    Err(e) => return JobOutput::Error(format!("{}: {e}", f.display())),
                }
            }
            JobOutput::Imported { groups }
        });
    }

    fn finish_import(&mut self, objects: Vec<IoObject>, path: &Path) -> Vec<ObjectId> {
        // place the imported group: centred in XY, resting on the plate, beside existing objects
        let group = objects.iter().fold(Aabb::EMPTY, |b, o| b.union(&o.mesh.bounds()));
        let existing = self.scene.bounds();
        let mut offset = Vec3::new(-group.center().x, -group.center().y, -group.min.z);
        if !existing.is_empty() {
            offset.x = existing.max.x + 10.0 - group.min.x;
        }
        let n = objects.len();
        let tris: usize = objects.iter().map(|o| o.mesh.triangles.len()).sum();
        let mut ids = vec![];
        for mut o in objects {
            o.mesh.translate(offset);
            let name = self.scene.unique_name(&o.name);
            ids.push(self.scene.add_world_mesh(name, o.mesh, o.color));
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")) && self.scene.objects.len() == n {
            self.current_file = Some(path.to_path_buf());
            self.dirty = false;
        }
        self.push_recent(path.to_path_buf());
        self.fit_on_next_frame = true;
        self.info(format!("Imported {n} object(s), {tris} triangles from {}", path.display()));
        ids
    }

    pub fn export_objects(&self, only_selected: bool) -> Vec<IoObject> {
        self.scene
            .objects
            .iter()
            .filter(|o| o.visible && (!only_selected || self.selection.contains(&o.id)))
            .map(|o| IoObject { name: o.name.clone(), mesh: o.world_mesh(), color: Some(o.color) })
            .collect()
    }

    pub fn save_as_dialog(&mut self, ctx: &egui::Context, only_selected: bool) {
        let default = self
            .current_file
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "model".into());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("3MF (keeps objects & colours)", &["3mf"])
            .add_filter("STL (binary)", &["stl"])
            .add_filter("OBJ", &["obj"])
            .add_filter("PLY (binary)", &["ply"])
            .set_file_name(format!("{default}.3mf"))
            .save_file()
        else {
            return;
        };
        let path = if Format::from_path(&path).is_none() { path.with_extension("3mf") } else { path };
        self.save_to(ctx, path, only_selected);
    }

    pub fn save(&mut self, ctx: &egui::Context) {
        match self.current_file.clone() {
            Some(p) => self.save_to(ctx, p, false),
            None => self.save_as_dialog(ctx, false),
        }
    }

    fn save_to(&mut self, ctx: &egui::Context, path: PathBuf, only_selected: bool) {
        let objs = self.export_objects(only_selected);
        if objs.is_empty() {
            self.error("Nothing to save");
            return;
        }
        self.spawn_job(ctx, format!("Saving {}", path.display()), move || match io::save(&path, &objs) {
            Ok(()) => JobOutput::Saved { path },
            Err(e) => JobOutput::Error(format!("Save failed: {e}")),
        });
    }

    pub fn new_scene(&mut self) {
        self.checkpoint("New scene");
        self.scene = Scene::default();
        self.selection.clear();
        self.current_file = None;
        self.dirty = false;
    }

    // ---------- object actions ----------

    pub fn add_primitive(&mut self, p: Primitive) {
        self.checkpoint(&format!("Add {}", p.name()));
        let mesh = p.build();
        let name = self.scene.unique_name(p.name());
        let existing = self.scene.bounds();
        let size = mesh.bounds().size();
        let mut pos = Vec3::new(0.0, 0.0, size.z * 0.5);
        if !existing.is_empty() {
            pos.x = existing.max.x + 10.0 + size.x * 0.5;
            pos.y = existing.center().y;
        }
        let id = self.scene.add_object(name, mesh, Transform { translation: pos, ..Default::default() }, None);
        self.selection = vec![id];
    }

    pub fn delete_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Delete");
        for id in std::mem::take(&mut self.selection) {
            self.scene.remove(id);
        }
    }

    pub fn duplicate_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Duplicate");
        let shift = self.selection_bounds().size().x + 5.0;
        let mut new = vec![];
        for id in self.selection.clone() {
            let Some(o) = self.scene.get(id).cloned() else { continue };
            let mut t = o.transform;
            t.translation.x += shift;
            let name = self.scene.unique_name(&o.name);
            let nid = self.scene.add_object(name, (*o.mesh.mesh).clone(), t, Some(o.color));
            new.push(nid);
        }
        self.selection = new;
    }

    pub fn array_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Array");
        let p = self.array.clone();
        let mut new = vec![];
        for id in self.selection.clone() {
            let Some(o) = self.scene.get(id).cloned() else { continue };
            for ix in 0..p.count[0].max(1) {
                for iy in 0..p.count[1].max(1) {
                    for iz in 0..p.count[2].max(1) {
                        if ix == 0 && iy == 0 && iz == 0 {
                            continue;
                        }
                        let mut t = o.transform;
                        t.translation += Vec3::new(ix as f32, iy as f32, iz as f32) * p.spacing;
                        let name = self.scene.unique_name(&o.name);
                        new.push(self.scene.add_object(name, (*o.mesh.mesh).clone(), t, Some(o.color)));
                    }
                }
            }
        }
        self.info(format!("Created {} copies", new.len()));
        self.selection.extend(new);
    }

    pub fn select_all(&mut self) {
        self.selection = self.scene.objects.iter().filter(|o| o.visible).map(|o| o.id).collect();
    }

    pub fn drop_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Drop to plate");
        for id in self.selection.clone() {
            self.scene.drop_to_plate(id);
        }
    }

    pub fn center_selected_on_plate(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Center on plate");
        let b = self.selection_bounds();
        let d = Vec3::new(-b.center().x, -b.center().y, 0.0);
        for id in self.selection.clone() {
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.translation += d;
            }
        }
    }

    /// Align selected objects to the selection bounds. `mode`: 0 = min, 1 = center, 2 = max.
    pub fn align(&mut self, axis: usize, mode: u8) {
        if self.selection.len() < 2 {
            return;
        }
        self.checkpoint("Align");
        let all = self.selection_bounds();
        let target = match mode {
            0 => all.min[axis],
            1 => all.center()[axis],
            _ => all.max[axis],
        };
        for id in self.selection.clone() {
            let Some(o) = self.scene.get(id) else { continue };
            let b = o.world_bounds();
            let cur = match mode {
                0 => b.min[axis],
                1 => b.center()[axis],
                _ => b.max[axis],
            };
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.translation[axis] += target - cur;
            }
        }
    }

    /// Spread selected objects so their gaps along `axis` are equal.
    pub fn distribute(&mut self, axis: usize) {
        if self.selection.len() < 3 {
            return;
        }
        self.checkpoint("Distribute");
        let mut items: Vec<(ObjectId, Aabb)> =
            self.selection.iter().filter_map(|id| self.scene.get(*id).map(|o| (*id, o.world_bounds()))).collect();
        items.sort_by(|a, b| a.1.center()[axis].total_cmp(&b.1.center()[axis]));
        let total: f32 = items.iter().map(|(_, b)| b.size()[axis]).sum();
        let span = items.last().unwrap().1.max[axis] - items[0].1.min[axis];
        let gap = (span - total) / (items.len() - 1) as f32;
        let mut cursor = items[0].1.min[axis];
        for (id, b) in items {
            let d = cursor - b.min[axis];
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.translation[axis] += d;
            }
            cursor += b.size()[axis] + gap;
        }
    }

    pub fn mirror_selected(&mut self, axis: usize) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Mirror");
        for id in self.selection.clone() {
            let Some(o) = self.scene.get(id) else { continue };
            let mut w = o.world_mesh();
            let c = w.bounds().center();
            let mut s = Vec3::ONE;
            s[axis] = -1.0;
            w.transform(&(Mat4::from_translation(c) * Mat4::from_scale(s) * Mat4::from_translation(-c)));
            self.scene.set_world_mesh(id, w);
        }
    }

    pub fn reset_transform_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Reset rotation/scale");
        for id in self.selection.clone() {
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.rotation = Quat::IDENTITY;
                o.transform.scale = Vec3::ONE;
            }
        }
    }

    pub fn bake_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint("Apply transform");
        for id in self.selection.clone() {
            self.scene.bake_transform(id);
        }
        self.info("Rotation and scale applied to the mesh");
    }

    /// Combine selected objects into one without a boolean (fast "group").
    pub fn merge_selected(&mut self) {
        if self.selection.len() < 2 {
            return;
        }
        self.checkpoint("Merge");
        let first = self.scene.get(self.selection[0]).cloned().unwrap();
        let mut all = Mesh::default();
        for id in &self.selection {
            if let Some(o) = self.scene.get(*id) {
                all.append(&o.world_mesh());
            }
        }
        for id in std::mem::take(&mut self.selection) {
            self.scene.remove(id);
        }
        let id = self.scene.add_world_mesh(first.name, all, Some(first.color));
        self.selection = vec![id];
    }

    pub fn lay_flat(&mut self, id: ObjectId, world_normal: Vec3) {
        self.checkpoint("Lay on face");
        if let Some(o) = self.scene.get_mut(id) {
            let q = Quat::from_rotation_arc(world_normal.normalize(), -Vec3::Z);
            o.transform.rotation = (q * o.transform.rotation).normalize();
        }
        self.scene.drop_to_plate(id);
        self.selection = vec![id];
        self.info("Face placed on the plate");
    }

    pub fn set_color_selected(&mut self, c: [u8; 4]) {
        self.checkpoint_coalesced("Color");
        for id in self.selection.clone() {
            if let Some(o) = self.scene.get_mut(id) {
                o.color = c;
            }
        }
    }

    // ---------- heavy geometry (background) ----------

    pub fn boolean_selected(&mut self, ctx: &egui::Context, op: BoolOp) {
        if self.selection.len() < 2 {
            self.error("Select at least two objects (the first one selected is the base)");
            return;
        }
        let base = self.scene.get(self.selection[0]).cloned().unwrap();
        let others: Vec<Mesh> = self.selection[1..].iter().filter_map(|id| self.scene.get(*id)).map(|o| o.world_mesh()).collect();
        let remove = self.selection.clone();
        let base_mesh = base.world_mesh();
        let name = format!("{op:?}");
        self.spawn_job(ctx, format!("{name}…"), move || match csg::boolean_many(&base_mesh, &others, op) {
            Ok(m) if m.is_empty() => JobOutput::Error(format!("{name}: result is empty")),
            Ok(m) => JobOutput::Replace {
                label: name.clone(),
                remove,
                message: format!("{name} done: {} triangles", m.triangles.len()),
                add: vec![(base.name, m, base.color)],
            },
            Err(e) => JobOutput::Error(format!("{name} failed: {e}")),
        });
    }

    pub fn cut_selected(&mut self, ctx: &egui::Context) {
        if self.selection.is_empty() {
            return;
        }
        let mut normal = Vec3::ZERO;
        normal[self.cut.axis] = 1.0;
        let mut point = Vec3::ZERO;
        point[self.cut.axis] = self.cut.offset;
        let keep = self.cut.keep;
        let objs: Vec<_> =
            self.selection.iter().filter_map(|id| self.scene.get(*id)).map(|o| (o.id, o.name.clone(), o.color, o.world_mesh())).collect();
        self.spawn_job(ctx, "Cutting…", move || {
            let mut add = vec![];
            let mut remove = vec![];
            for (id, name, color, m) in objs {
                match csg::cut_by_plane(&m, point, normal) {
                    Ok((above, below)) => {
                        if above.is_empty() || below.is_empty() {
                            continue; // plane misses this object
                        }
                        remove.push(id);
                        if keep != CutKeep::Below {
                            add.push((format!("{name} (top)"), above, color));
                        }
                        if keep != CutKeep::Above {
                            add.push((format!("{name} (bottom)"), below, color));
                        }
                    }
                    Err(e) => return JobOutput::Error(format!("Cut failed on {name}: {e}")),
                }
            }
            if remove.is_empty() {
                return JobOutput::Error("The cut plane does not intersect the selection".into());
            }
            JobOutput::Replace { label: "Cut".into(), remove, add, message: "Cut done".into() }
        });
    }

    pub fn split_selected(&mut self, ctx: &egui::Context) {
        let objs: Vec<_> =
            self.selection.iter().filter_map(|id| self.scene.get(*id)).map(|o| (o.id, o.name.clone(), o.color, o.world_mesh())).collect();
        if objs.is_empty() {
            return;
        }
        self.spawn_job(ctx, "Splitting…", move || {
            let mut add = vec![];
            let mut remove = vec![];
            for (id, name, color, m) in objs {
                let parts = m.connected_components();
                if parts.len() < 2 {
                    continue;
                }
                remove.push(id);
                for (i, p) in parts.into_iter().enumerate() {
                    add.push((format!("{name} part {}", i + 1), p, color));
                }
            }
            if remove.is_empty() {
                return JobOutput::Error("Selection is already a single connected part".into());
            }
            let n = add.len();
            JobOutput::Replace { label: "Split".into(), remove, add, message: format!("Split into {n} parts") }
        });
    }

    pub fn repair_selected(&mut self, ctx: &egui::Context) {
        let opts = RepairOptions { weld_tolerance: self.settings.weld_tolerance, ..Default::default() };
        let objs: Vec<_> = self.selection.iter().filter_map(|id| self.scene.get(*id)).map(|o| (o.id, (*o.mesh.mesh).clone())).collect();
        if objs.is_empty() {
            return;
        }
        self.spawn_job(ctx, "Repairing…", move || {
            let mut out = vec![];
            let mut logs = vec![];
            for (id, mut m) in objs {
                let log = repair::repair(&mut m, opts);
                logs.push(log.summary());
                out.push((id, m));
            }
            JobOutput::ReplaceMeshes { label: "Repair".into(), meshes: out, message: format!("Repair: {}", logs.join("; ")) }
        });
    }

    /// Cached analysis of the primary selection's local mesh.
    pub fn primary_report(&mut self) -> Option<MeshReport> {
        let id = *self.selection.first()?;
        let o = self.scene.get(id)?;
        let mid = o.mesh.id;
        if self.report_cache.as_ref().map(|c| c.0) != Some(mid) {
            let mut r = analyze(&o.mesh.mesh);
            // volume/area/bounds in world units
            let w = o.world_mesh();
            r.volume_mm3 = w.volume();
            r.area_mm2 = w.surface_area();
            r.bounds = Some(w.bounds());
            self.report_cache = Some((mid, r));
        }
        self.report_cache.as_ref().map(|c| c.1.clone())
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_job();
        // finish coalesced edits once nothing is being dragged or typed into
        if !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input() {
            self.pending_edit = None;
        }
        // drag & drop
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        let dropped: Vec<PathBuf> = dropped.into_iter().filter(|p| Format::from_path(p).is_some()).collect();
        if !dropped.is_empty() {
            self.import_files(dropped, ctx);
        }
        crate::ui::shortcuts(self, ctx);
        let title = format!(
            "{}{} — Builder3D",
            if self.dirty { "• " } else { "" },
            self.current_file
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".into())
        );
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::ui::draw(self, ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }
}
