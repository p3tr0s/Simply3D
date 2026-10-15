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
use b3d_core::sdf::{self, RoundOptions};
use b3d_core::simplify::{self, SimplifyOptions};
use b3d_core::smooth;
use b3d_core::Progress;
use b3d_core::{Aabb, Mesh};
use glam::{Mat4, Quat, Vec2, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
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
    /// Minutes between autosaves (0 = off).
    pub autosave_minutes: f32,
    pub recent_commands: Vec<String>,
    /// What a left-drag on empty space does.
    pub left_drag: LeftDrag,
    /// Left-drag on an object moves it (3D Builder style).
    pub drag_moves_objects: bool,
    /// Snap moved objects to other objects' edges/centres and the plate.
    pub snap_objects: bool,
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
            autosave_minutes: 2.0,
            recent_commands: vec![],
            left_drag: LeftDrag::Orbit,
            drag_moves_objects: true,
            snap_objects: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LeftDrag {
    /// 3D Builder style: left-drag orbits, Shift+left-drag box-selects, right-drag pans.
    Orbit,
    /// CAD style: left-drag box-selects, right-drag orbits, middle-drag pans.
    BoxSelect,
}

/// The current mouse drag in the viewport.
#[derive(Debug, Clone, Default)]
pub enum DragMode {
    #[default]
    None,
    Orbit,
    Pan,
    Box {
        start: egui::Pos2,
        additive: bool,
    },
    Move {
        grab: Vec3,
        start: Vec<(ObjectId, Transform)>,
        bounds: Aabb,
        vertical: bool,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmoothMode {
    /// Loop subdivision: smooth and 4× triangles per level.
    Subdivide,
    /// Midpoint subdivision: same shape, 4× triangles per level.
    SubdivideFlat,
    /// Taubin relaxation: same triangle count, removes noise/facets.
    Relax,
}

#[derive(Debug, Clone)]
pub struct MeshTools {
    pub simplify_pct: f32,
    pub simplify_err: f32,
    pub smooth_mode: SmoothMode,
    pub smooth_levels: u32,
    pub relax_iters: u32,
    pub keep_sharp: bool,
    pub crease_deg: f32,
    pub round_radius: f32,
    pub round_mode: b3d_core::sdf::RoundMode,
    pub round_voxel: f32,
    pub round_simplify: bool,
}

impl Default for MeshTools {
    fn default() -> Self {
        Self {
            simplify_pct: 50.0,
            simplify_err: 0.0,
            smooth_mode: SmoothMode::Subdivide,
            smooth_levels: 1,
            relax_iters: 10,
            keep_sharp: true,
            crease_deg: 40.0,
            round_radius: 2.0,
            round_mode: b3d_core::sdf::RoundMode::Convex,
            round_voxel: 0.0,
            round_simplify: true,
        }
    }
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
    pub progress: Progress,
    pub label: String,
    pub rx: Receiver<JobOutput>,
    pub started: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Object,
    Arrange,
    Mesh,
    Repair,
}

pub struct RecoveryInfo {
    pub file: PathBuf,
    pub original: Option<PathBuf>,
    pub age_minutes: u64,
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
    pub mesh_tools: MeshTools,
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
    pub tab: PanelTab,
    pub palette: Option<crate::commands::Palette>,
    /// object id -> (mesh id, transform, world bounds)
    pub bounds_cache: HashMap<ObjectId, (u64, Transform, Aabb)>,
    pub edit_gen: u64,
    pub autosaved_gen: u64,
    pub last_autosave: Instant,
    pub autosave_busy: Arc<AtomicBool>,
    pub recovery: Option<RecoveryInfo>,
    pub close_prompt: bool,
    pub close_after_save: bool,
    pub allow_close: bool,
    pub pending_recovery_target: Option<Option<PathBuf>>,
    /// (off the plate, below the plate), refreshed every frame
    pub issues: (Vec<ObjectId>, Vec<ObjectId>),
    pub drag: DragMode,
    /// Snap guide segments for this frame.
    pub snap_guides: Vec<(Vec3, Vec3)>,
    /// Text shown next to the cursor while moving (e.g. the offset).
    pub drag_label: Option<String>,
    /// Transforms + group bounds when a gizmo move started.
    pub gizmo_start: Vec<(ObjectId, Transform)>,
    pub gizmo_start_bounds: Aabb,
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
            mesh_tools: MeshTools::default(),
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
            tab: PanelTab::Object,
            palette: None,
            bounds_cache: HashMap::new(),
            edit_gen: 0,
            autosaved_gen: 0,
            last_autosave: Instant::now(),
            autosave_busy: Arc::new(AtomicBool::new(false)),
            recovery: None,
            close_prompt: false,
            close_after_save: false,
            allow_close: false,
            pending_recovery_target: None,
            issues: (vec![], vec![]),
            drag: DragMode::None,
            snap_guides: vec![],
            drag_label: None,
            gizmo_start: vec![],
            gizmo_start_bounds: Aabb::EMPTY,
            encode_srgb,
        };
        app.recovery = crate::autosave::find_recovery();
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
        self.edit_gen += 1;
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
            self.edit_gen += 1;
        }
    }
    pub fn redo(&mut self) {
        if let Some(l) = self.history.redo(&mut self.scene) {
            self.info(format!("Redid: {l}"));
            self.prune_selection();
            self.dirty = true;
            self.edit_gen += 1;
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

    pub fn spawn_job(&mut self, ctx: &egui::Context, label: impl Into<String>, f: impl FnOnce(&Progress) -> JobOutput + Send + 'static) {
        if self.job.is_some() {
            self.error("Another operation is still running");
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let progress = Progress::new();
        let p = progress.clone();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&p)))
                .unwrap_or_else(|_| JobOutput::Error("operation crashed".into()));
            let _ = tx.send(out);
            ctx.request_repaint();
        });
        self.job = Some(Job { progress, label: label.into(), rx, started: Instant::now() });
    }

    /// Stop the running operation. Work that can't be interrupted (booleans) finishes
    /// in the background and its result is discarded.
    pub fn cancel_job(&mut self) {
        if let Some(j) = self.job.take() {
            j.progress.cancel();
            self.close_after_save = false;
            self.info(format!("Cancelled: {}", j.label.trim_end_matches('…')));
        }
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
                if let Some(orig) = self.pending_recovery_target.take() {
                    self.current_file = orig;
                    self.dirty = true;
                    self.info("Recovered unsaved work — save it to keep it");
                }
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
                if !self.dirty {
                    crate::autosave::clear();
                    self.autosaved_gen = self.edit_gen;
                }
                if self.close_after_save {
                    self.allow_close = true;
                    self.close_after_save = false;
                }
            }
            JobOutput::Error(e) => {
                self.close_after_save = false;
                if e != "cancelled" {
                    self.error(e)
                }
            }
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
        self.spawn_job(ctx, label, move |p| {
            let mut groups = vec![];
            for f in &files {
                if p.is_cancelled() {
                    return JobOutput::Error("cancelled".into());
                }
                match io::load(f) {
                    Ok(o) => groups.push((f.clone(), o)),
                    Err(e) => return JobOutput::Error(format!("{}: {e}", f.display())),
                }
            }
            JobOutput::Imported { groups }
        });
    }

    fn finish_import(&mut self, objects: Vec<IoObject>, path: &Path) -> Vec<ObjectId> {
        let existing = self.scene.bounds();
        let recovered = crate::autosave::is_recovery_file(path);
        // our own projects opened into an empty scene keep their exact layout;
        // anything else is centred on the plate (or placed beside what's there)
        let keep = recovered || (existing.is_empty() && !objects.is_empty() && objects.iter().all(|o| o.keep_position));
        let group = objects.iter().fold(Aabb::EMPTY, |b, o| b.union(&o.world_mesh().bounds()));
        let mut offset = Vec3::new(-group.center().x, -group.center().y, -group.min.z);
        if !existing.is_empty() {
            offset.x = existing.max.x + 10.0 - group.min.x;
        }
        if keep {
            offset = Vec3::ZERO;
        }
        let n = objects.len();
        let tris: usize = objects.iter().map(|o| o.mesh.triangles.len()).sum();
        let mut ids = vec![];
        let mut groups: Vec<(String, Vec<ObjectId>)> = vec![];
        for o in objects {
            let name = self.scene.unique_name(&o.name);
            let id = match o.transform.and_then(|xf| {
                // keep rotation/scale as a transform when it is a clean TRS matrix
                let (sc, rot, tr) = xf.to_scale_rotation_translation();
                let back = Mat4::from_scale_rotation_translation(sc, rot, tr);
                let tol = 1e-4 * xf.abs().to_cols_array().iter().cloned().fold(1.0, f32::max);
                back.abs_diff_eq(xf, tol).then_some((xf, sc, rot))
            }) {
                Some((xf, sc, rot)) => {
                    let mut mesh = o.mesh;
                    let c = mesh.bounds().center();
                    mesh.translate(-c);
                    let t = Transform { translation: xf.transform_point3(c) + offset, rotation: rot, scale: sc };
                    self.scene.add_object(name, mesh, t, o.color)
                }
                None => {
                    let mut mesh = o.world_mesh();
                    mesh.translate(offset);
                    self.scene.add_world_mesh(name, mesh, o.color)
                }
            };
            if let Some(ob) = self.scene.get_mut(id) {
                ob.visible = !o.hidden;
                ob.locked = o.locked;
            }
            if let Some(g) = o.group {
                match groups.iter_mut().find(|(n, _)| *n == g) {
                    Some((_, v)) => v.push(id),
                    None => groups.push((g, vec![id])),
                }
            }
            ids.push(id);
        }
        for (name, members) in groups {
            if members.len() >= 2 {
                self.scene.group(&members, name);
            }
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")) && self.scene.objects.len() == n {
            self.current_file = Some(path.to_path_buf());
            self.dirty = false;
        }
        if !recovered {
            self.push_recent(path.to_path_buf());
        }
        self.fit_on_next_frame = true;
        self.info(format!("Imported {n} object(s), {tris} triangles from {}", path.display()));
        ids.retain(|id| self.scene.get(*id).is_some_and(|o| o.visible));
        ids
    }

    /// Objects to write. `project`: everything (hidden too) with placements, groups and
    /// flags, for 3MF projects. Otherwise: visible objects as world meshes.
    pub fn export_objects(&self, only_selected: bool, project: bool) -> Vec<IoObject> {
        self.scene
            .objects
            .iter()
            .filter(|o| if project { true } else { o.visible && (!only_selected || self.selection.contains(&o.id)) })
            .map(|o| {
                if project {
                    IoObject {
                        name: o.name.clone(),
                        mesh: (*o.mesh.mesh).clone(),
                        color: Some(o.color),
                        transform: Some(o.matrix()),
                        group: o.group.and_then(|g| self.scene.group_name(g)).map(str::to_string),
                        hidden: !o.visible,
                        locked: o.locked,
                        keep_position: true,
                    }
                } else {
                    IoObject::new(o.name.clone(), o.world_mesh(), Some(o.color))
                }
            })
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
        let project = !only_selected && Format::from_path(&path) == Some(Format::ThreeMf);
        let objs = self.export_objects(only_selected, project);
        if objs.is_empty() {
            self.error("Nothing to save");
            return;
        }
        self.spawn_job(ctx, format!("Saving {}", path.display()), move |_p| match io::save(&path, &objs) {
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
        crate::autosave::clear();
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
        self.spawn_job(ctx, format!("{name}…"), move |_p| match csg::boolean_many(&base_mesh, &others, op) {
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
        self.spawn_job(ctx, "Cutting…", move |_p| {
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
        self.spawn_job(ctx, "Splitting…", move |_p| {
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
        self.spawn_job(ctx, "Repairing…", move |_p| {
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

    /// Selected objects' local meshes (transform kept) with the largest scale factor.
    fn selected_local(&self) -> Vec<(ObjectId, Mesh, f32)> {
        self.selection
            .iter()
            .filter_map(|id| self.scene.get(*id))
            .map(|o| (o.id, (*o.mesh.mesh).clone(), o.transform.scale.abs().max_element().max(1e-6)))
            .collect()
    }

    pub fn simplify_selected(&mut self, ctx: &egui::Context) {
        let objs = self.selected_local();
        if objs.is_empty() {
            return;
        }
        let (pct, err) = (self.mesh_tools.simplify_pct, self.mesh_tools.simplify_err);
        self.spawn_job(ctx, "Simplifying…", move |p| {
            let (mut before, mut after) = (0, 0);
            let n = objs.len() as f32;
            let mut meshes = vec![];
            for (i, (id, mut m, scale)) in objs.into_iter().enumerate() {
                before += m.triangles.len();
                let target = ((m.triangles.len() as f32) * pct / 100.0) as usize;
                let opts = SimplifyOptions { target_triangles: target, max_error: (err / scale) as f64 };
                if let Err(e) = simplify::simplify_with(&mut m, opts, &p.range(i as f32 / n, (i + 1) as f32 / n)) {
                    return JobOutput::Error(e.to_string());
                }
                after += m.triangles.len();
                meshes.push((id, m));
            }
            JobOutput::ReplaceMeshes { label: "Simplify".into(), meshes, message: format!("Simplified {before} to {after} triangles") }
        });
    }

    pub fn smooth_selected(&mut self, ctx: &egui::Context) {
        let objs = self.selected_local();
        if objs.is_empty() {
            return;
        }
        let t = self.mesh_tools.clone();
        let crease = t.keep_sharp.then_some(t.crease_deg);
        if t.smooth_mode != SmoothMode::Relax {
            let est: usize = objs.iter().map(|o| o.1.triangles.len() << (2 * t.smooth_levels)).sum();
            if est > 8_000_000 {
                self.error(format!("That would create {est} triangles — simplify first or use fewer levels"));
                return;
            }
        }
        self.spawn_job(ctx, "Smoothing…", move |p| {
            let mut total = 0;
            let n = objs.len() as f32;
            let mut meshes = vec![];
            for (i, (id, m, _)) in objs.into_iter().enumerate() {
                if p.is_cancelled() {
                    return JobOutput::Error("cancelled".into());
                }
                p.set(i as f32 / n);
                let out = match t.smooth_mode {
                    SmoothMode::Subdivide => smooth::subdivide_loop(&m, t.smooth_levels, crease),
                    SmoothMode::SubdivideFlat => smooth::subdivide_flat(&m, t.smooth_levels),
                    SmoothMode::Relax => {
                        let mut m = m;
                        if let Err(e) = smooth::taubin_with(&mut m, t.relax_iters, crease, &p.range(i as f32 / n, (i + 1) as f32 / n)) {
                            return JobOutput::Error(e.to_string());
                        }
                        m
                    }
                };
                total += out.triangles.len();
                meshes.push((id, out));
            }
            JobOutput::ReplaceMeshes { label: "Smooth".into(), meshes, message: format!("Smoothed: {total} triangles") }
        });
    }

    pub fn round_selected(&mut self, ctx: &egui::Context) {
        let objs: Vec<_> =
            self.selection.iter().filter_map(|id| self.scene.get(*id)).map(|o| (o.id, o.name.clone(), o.color, o.world_mesh())).collect();
        if objs.is_empty() {
            return;
        }
        let t = self.mesh_tools.clone();
        let opts = RoundOptions {
            radius: t.round_radius,
            mode: t.round_mode,
            voxel: t.round_voxel,
            simplify: t.round_simplify,
            ..Default::default()
        };
        self.spawn_job(ctx, "Rounding edges…", move |p| {
            let mut add = vec![];
            let mut remove = vec![];
            let mut info = vec![];
            let n = objs.len() as f32;
            for (i, (id, name, color, m)) in objs.into_iter().enumerate() {
                match sdf::round_edges_with(&m, opts, &p.range(i as f32 / n, (i + 1) as f32 / n)) {
                    Ok((out, h)) => {
                        let lost = m.connected_components().len() > out.connected_components().len();
                        info.push(format!(
                            "{name}: {} triangles (detail {h:.2} mm){}",
                            out.triangles.len(),
                            if lost { " — some thin parts disappeared" } else { "" }
                        ));
                        remove.push(id);
                        add.push((name, out, color));
                    }
                    Err(b3d_core::Error::Cancelled) => return JobOutput::Error("cancelled".into()),
                    Err(e) => return JobOutput::Error(format!("Round edges failed on {name}: {e}")),
                }
            }
            JobOutput::Replace { label: "Round edges".into(), remove, add, message: format!("Rounded edges — {}", info.join("; ")) }
        });
    }

    /// World bounds of an object, cached until its mesh or transform changes.
    pub fn object_bounds(&mut self, id: ObjectId) -> Option<Aabb> {
        let o = self.scene.get(id)?;
        if let Some((mid, t, b)) = self.bounds_cache.get(&id) {
            if *mid == o.mesh.id && *t == o.transform {
                return Some(*b);
            }
        }
        let b = o.world_bounds();
        self.bounds_cache.insert(id, (o.mesh.id, o.transform, b));
        Some(b)
    }

    /// Bounds of every visible object except the given ones (snap targets).
    pub fn other_bounds(&mut self, exclude: &[ObjectId]) -> Vec<Aabb> {
        let ids: Vec<ObjectId> = self.scene.objects.iter().filter(|o| o.visible && !exclude.contains(&o.id)).map(|o| o.id).collect();
        ids.into_iter().filter_map(|id| self.object_bounds(id)).collect()
    }

    /// (objects sticking out of the plate area, objects below the plate)
    pub fn plate_issues(&mut self) -> (Vec<ObjectId>, Vec<ObjectId>) {
        let half = self.settings.plate * 0.5;
        let tol = 0.01;
        let ids: Vec<ObjectId> = self.scene.objects.iter().filter(|o| o.visible).map(|o| o.id).collect();
        self.bounds_cache.retain(|k, _| ids.contains(k));
        let (mut off, mut below) = (vec![], vec![]);
        for id in ids {
            let Some(b) = self.object_bounds(id) else { continue };
            if b.min.x < -half.x - tol || b.max.x > half.x + tol || b.min.y < -half.y - tol || b.max.y > half.y + tol {
                off.push(id);
            }
            if b.min.z < -tol {
                below.push(id);
            }
        }
        (off, below)
    }

    /// Move the given objects (as a group) back inside the plate, keeping their layout.
    pub fn fit_onto_plate(&mut self, ids: &[ObjectId]) {
        if ids.is_empty() {
            return;
        }
        self.checkpoint("Move onto plate");
        let half = self.settings.plate * 0.5;
        for &id in ids {
            let Some(b) = self.object_bounds(id) else { continue };
            let mut d = Vec3::ZERO;
            for a in 0..2 {
                if b.size()[a] > half[a] * 2.0 {
                    d[a] = -b.center()[a]; // too big: centre it
                } else if b.min[a] < -half[a] {
                    d[a] = -half[a] - b.min[a];
                } else if b.max[a] > half[a] {
                    d[a] = half[a] - b.max[a];
                }
            }
            if let Some(o) = self.scene.get_mut(id) {
                o.transform.translation += d;
            }
        }
    }

    pub fn drop_ids(&mut self, ids: &[ObjectId]) {
        if ids.is_empty() {
            return;
        }
        self.checkpoint("Drop to plate");
        for &id in ids {
            self.scene.drop_to_plate(id);
        }
    }

    // ---------- autosave & closing ----------

    fn tick_autosave(&mut self) {
        let mins = self.settings.autosave_minutes;
        if mins <= 0.0 || self.edit_gen == self.autosaved_gen || self.autosave_busy.load(Ordering::Relaxed) {
            return;
        }
        if self.last_autosave.elapsed().as_secs_f32() < mins * 60.0 || self.scene.objects.is_empty() {
            return;
        }
        self.last_autosave = Instant::now();
        self.autosaved_gen = self.edit_gen;
        let objs = self.export_objects(false, true);
        crate::autosave::write_async(objs, self.current_file.clone(), self.autosave_busy.clone());
    }

    pub fn recover(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.recovery.take() {
            let original = r.original.clone();
            self.import_files(vec![r.file], ctx);
            // after loading, treat as unsaved work belonging to the original file
            self.pending_recovery_target = Some(original);
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.allow_close && !self.scene.objects.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_prompt = true;
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
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
        self.tick_autosave();
        self.handle_close(ctx);
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
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

    fn on_exit(&mut self) {
        // a clean exit leaves nothing to recover
        if !self.dirty || self.allow_close {
            crate::autosave::clear();
        }
    }
}
