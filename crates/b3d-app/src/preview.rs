//! Live previews: simplify / smooth / round edges / cut run in the background as you
//! change their settings, are shown in place of the originals, and only change the scene
//! when you press Apply.

use crate::app::{App, CutKeep, JobOutput, SmoothMode, Tool};
use b3d_core::csg;
use b3d_core::scene::MeshHandle;
use b3d_core::sdf::{self, RoundOptions};
use b3d_core::simplify::{self, SimplifyOptions};
use b3d_core::smooth;
use b3d_core::{Mesh, ObjectId, Progress};
use glam::{Mat4, Vec3};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

pub type JobFn = Box<dyn FnOnce(&Progress) -> JobOutput + Send>;

/// A cancellable background computation. Dropping it cancels the work.
pub struct BgTask<T> {
    pub key: u64,
    pub progress: Progress,
    rx: Receiver<Option<T>>,
    pub started: Instant,
}

impl<T: Send + 'static> BgTask<T> {
    pub fn spawn(ctx: &egui::Context, key: u64, f: impl FnOnce(&Progress) -> T + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let progress = Progress::new();
        let p = progress.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&p))).ok();
            let _ = tx.send(out);
            ctx.request_repaint();
        });
        BgTask { key, progress, rx, started: Instant::now() }
    }
    /// `Some(None)` = the task panicked.
    pub fn poll(&self) -> Option<Option<T>> {
        self.rx.try_recv().ok()
    }
}

impl<T> Drop for BgTask<T> {
    fn drop(&mut self) {
        self.progress.cancel();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewKind {
    Simplify,
    Smooth,
    Round,
    Cut,
    ExtrudeDown,
}

impl PreviewKind {
    pub fn name(self) -> &'static str {
        match self {
            PreviewKind::Simplify => "Simplify",
            PreviewKind::Smooth => "Smooth",
            PreviewKind::Round => "Round edges",
            PreviewKind::Cut => "Cut",
            PreviewKind::ExtrudeDown => "Extrude down",
        }
    }
}

/// What the preview shows and what Apply does.
pub struct PreviewResult {
    pub key: u64,
    pub targets: Vec<ObjectId>,
    /// Objects whose local mesh is replaced (transform kept).
    pub swap: Vec<(ObjectId, MeshHandle)>,
    /// Objects removed and replaced by new world-space meshes.
    pub hide: Vec<ObjectId>,
    /// (name, world mesh, colour, display offset) for new objects.
    pub add: Vec<(String, MeshHandle, [u8; 4], Vec3)>,
    pub message: String,
    pub secs: f32,
}

pub struct Preview {
    pub kind: PreviewKind,
    pub key: u64,
    pub changed: Instant,
    pub task: Option<BgTask<JobOutput>>,
    pub result: Option<PreviewResult>,
    pub error: Option<String>,
    pub apply_when_ready: bool,
}

const DEBOUNCE: Duration = Duration::from_millis(250);

impl App {
    /// Hash of everything the result of `kind` depends on.
    fn preview_key(&self, kind: PreviewKind) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        kind.hash(&mut h);
        let t = &self.mesh_tools;
        let params = match kind {
            PreviewKind::Simplify => format!("{} {}", t.simplify_pct, t.simplify_err),
            PreviewKind::Smooth => format!("{:?} {} {} {} {}", t.smooth_mode, t.smooth_levels, t.relax_iters, t.keep_sharp, t.crease_deg),
            PreviewKind::Round => format!("{} {:?} {} {}", t.round_radius, t.round_mode, t.round_voxel, t.round_simplify),
            PreviewKind::Cut => format!("{:?}", self.cut),
            PreviewKind::ExtrudeDown => format!("{}", t.extrude_height),
        };
        params.hash(&mut h);
        let world = matches!(kind, PreviewKind::Round | PreviewKind::Cut | PreviewKind::ExtrudeDown);
        for o in self.selection.iter().filter_map(|id| self.scene.get(*id)) {
            o.id.hash(&mut h);
            o.mesh.id.hash(&mut h);
            if world {
                format!("{:?}", o.transform).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Open (or switch) the live preview for an operation on the selection.
    pub fn start_preview(&mut self, kind: PreviewKind) {
        if self.selection.is_empty() {
            self.error(format!("{}: select an object first", kind.name()));
            return;
        }
        if self.preview.as_ref().is_some_and(|p| p.kind == kind) {
            return;
        }
        self.preview = Some(Preview {
            kind,
            key: 0,
            changed: Instant::now() - DEBOUNCE,
            task: None,
            result: None,
            error: None,
            apply_when_ready: false,
        });
    }

    /// Extrude down with the plane at a sensible starting height (a quarter of the way up).
    pub fn start_extrude_down(&mut self) {
        let b = self.selection_bounds();
        if !b.is_empty() && (self.mesh_tools.extrude_height <= b.min.z || self.mesh_tools.extrude_height > b.max.z) {
            self.mesh_tools.extrude_height = b.min.z + b.size().z * 0.25;
        }
        self.start_preview(PreviewKind::ExtrudeDown);
    }

    pub fn cancel_preview(&mut self) {
        if self.preview.take().is_some() {
            self.info("Preview cancelled — nothing changed");
        }
    }

    /// Apply now, or as soon as the current settings have been computed.
    pub fn apply_preview(&mut self) {
        let Some(p) = &mut self.preview else { return };
        let ready = p.result.as_ref().is_some_and(|r| r.key == p.key) && p.task.is_none();
        if !ready {
            if p.error.is_some() {
                return;
            }
            p.apply_when_ready = true;
            return;
        }
        let p = self.preview.take().unwrap();
        let kind = p.kind;
        let r = p.result.unwrap();
        if !r.swap.is_empty() {
            self.checkpoint(kind.name());
            for (id, h) in r.swap {
                if let Some(o) = self.scene.get_mut(id) {
                    o.mesh = h;
                }
            }
            self.report_cache = None;
            self.info(r.message);
        } else {
            let add =
                r.add.into_iter().map(|(n, h, c, _)| (n, std::sync::Arc::try_unwrap(h.mesh).unwrap_or_else(|a| (*a).clone()), c)).collect();
            self.apply_output(JobOutput::Replace { label: kind.name().into(), remove: r.hide, add, message: r.message }, r.secs);
        }
        if kind == PreviewKind::Cut {
            self.tool = Tool::Select;
        }
    }

    /// Called every frame: restart the computation when the inputs change, collect results.
    pub fn tick_preview(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.preview.as_ref().map(|p| p.kind) else { return };
        if self.selection.is_empty() || (kind == PreviewKind::Cut && self.tool != Tool::Cut) {
            self.preview = None;
            return;
        }
        let key = self.preview_key(kind);
        let p = self.preview.as_mut().unwrap();
        if key != p.key {
            p.key = key;
            p.changed = Instant::now();
            p.task = None; // cancels
            p.error = None;
        }
        // collect
        let polled = match p.task.as_ref().and_then(|t| t.poll()) {
            Some(out) => {
                let t = p.task.take().unwrap();
                Some((out, t.key, t.started.elapsed().as_secs_f32()))
            }
            None => None,
        };
        if let Some((out, tkey, secs)) = polled {
            match out {
                None => p.error = Some("the operation crashed".into()),
                Some(JobOutput::Error(e)) if e == "cancelled" => {}
                Some(JobOutput::Error(e)) => {
                    p.error = Some(e);
                    p.apply_when_ready = false;
                }
                Some(out) => {
                    let r = build_result(tkey, self.selection.clone(), out, secs, kind, self.cut_plane());
                    let p = self.preview.as_mut().unwrap();
                    p.result = Some(r);
                    if p.apply_when_ready {
                        self.apply_preview();
                        return;
                    }
                }
            }
        }
        let p = self.preview.as_mut().unwrap();
        let fresh = p.result.as_ref().is_some_and(|r| r.key == p.key);
        if !fresh && p.task.is_none() && p.error.is_none() {
            if p.changed.elapsed() >= DEBOUNCE {
                match self.build_op(kind) {
                    Ok((_, f)) => {
                        let task = BgTask::spawn(ctx, key, f);
                        self.preview.as_mut().unwrap().task = Some(task);
                    }
                    Err(e) => self.preview.as_mut().unwrap().error = Some(e),
                }
            } else {
                ctx.request_repaint_after(DEBOUNCE);
            }
        }
        if self.preview.as_ref().is_some_and(|p| p.task.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The preview result to draw, if it belongs to the current selection.
    pub fn shown_preview(&self) -> Option<&PreviewResult> {
        let r = self.preview.as_ref()?.result.as_ref()?;
        (r.targets == self.selection).then_some(r)
    }

    fn cut_plane(&self) -> (Vec3, Vec3) {
        let mut normal = Vec3::ZERO;
        normal[self.cut.axis] = 1.0;
        let mut point = Vec3::ZERO;
        point[self.cut.axis] = self.cut.offset;
        (point, normal)
    }

    /// Build the background work for an operation on the current selection.
    pub fn build_op(&self, kind: PreviewKind) -> Result<(String, JobFn), String> {
        if self.selection.is_empty() {
            return Err("nothing selected".into());
        }
        let t = self.mesh_tools.clone();
        let local: Vec<(ObjectId, Mesh, f32)> = self
            .selection
            .iter()
            .filter_map(|id| self.scene.get(*id))
            .map(|o| (o.id, (*o.mesh.mesh).clone(), o.transform.scale.abs().max_element().max(1e-6)))
            .collect();
        let world = || -> Vec<(ObjectId, String, [u8; 4], Mesh)> {
            self.selection.iter().filter_map(|id| self.scene.get(*id)).map(|o| (o.id, o.name.clone(), o.color, o.world_mesh())).collect()
        };
        Ok(match kind {
            PreviewKind::Simplify => {
                let (pct, err) = (t.simplify_pct, t.simplify_err);
                (
                    "Simplifying…".into(),
                    Box::new(move |p: &Progress| {
                        let (mut before, mut after) = (0, 0);
                        let n = local.len() as f32;
                        let mut meshes = vec![];
                        for (i, (id, mut m, scale)) in local.into_iter().enumerate() {
                            before += m.triangles.len();
                            let target = ((m.triangles.len() as f32) * pct / 100.0) as usize;
                            let opts = SimplifyOptions { target_triangles: target, max_error: (err / scale) as f64 };
                            if let Err(e) = simplify::simplify_with(&mut m, opts, &p.range(i as f32 / n, (i + 1) as f32 / n)) {
                                return JobOutput::Error(e.to_string());
                            }
                            after += m.triangles.len();
                            meshes.push((id, m));
                        }
                        JobOutput::ReplaceMeshes {
                            label: "Simplify".into(),
                            meshes,
                            message: format!("Simplified {before} → {after} triangles"),
                        }
                    }),
                )
            }
            PreviewKind::Smooth => {
                let crease = t.keep_sharp.then_some(t.crease_deg);
                if t.smooth_mode != SmoothMode::Relax {
                    let est: usize = local.iter().map(|o| o.1.triangles.len() << (2 * t.smooth_levels)).sum();
                    if est > 8_000_000 {
                        return Err(format!("that would create {est} triangles — simplify first or use fewer levels"));
                    }
                }
                (
                    "Smoothing…".into(),
                    Box::new(move |p: &Progress| {
                        let mut total = 0;
                        let n = local.len() as f32;
                        let mut meshes = vec![];
                        for (i, (id, m, _)) in local.into_iter().enumerate() {
                            if p.is_cancelled() {
                                return JobOutput::Error("cancelled".into());
                            }
                            p.set(i as f32 / n);
                            let out = match t.smooth_mode {
                                SmoothMode::Subdivide => smooth::subdivide_loop(&m, t.smooth_levels, crease),
                                SmoothMode::SubdivideFlat => smooth::subdivide_flat(&m, t.smooth_levels),
                                SmoothMode::Relax => {
                                    let mut m = m;
                                    let r = p.range(i as f32 / n, (i + 1) as f32 / n);
                                    if let Err(e) = smooth::taubin_with(&mut m, t.relax_iters, crease, &r) {
                                        return JobOutput::Error(e.to_string());
                                    }
                                    m
                                }
                            };
                            total += out.triangles.len();
                            meshes.push((id, out));
                        }
                        JobOutput::ReplaceMeshes { label: "Smooth".into(), meshes, message: format!("Smoothed: {total} triangles") }
                    }),
                )
            }
            PreviewKind::Round => {
                let objs = world();
                let opts = RoundOptions {
                    radius: t.round_radius,
                    mode: t.round_mode,
                    voxel: t.round_voxel,
                    simplify: t.round_simplify,
                    ..Default::default()
                };
                (
                    "Rounding edges…".into(),
                    Box::new(move |p: &Progress| {
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
                        JobOutput::Replace {
                            label: "Round edges".into(),
                            remove,
                            add,
                            message: format!("Rounded edges — {}", info.join("; ")),
                        }
                    }),
                )
            }
            PreviewKind::ExtrudeDown => {
                let objs = world();
                let h = t.extrude_height;
                (
                    "Extruding down…".into(),
                    Box::new(move |p: &Progress| {
                        let mut add = vec![];
                        let mut remove = vec![];
                        let n = objs.len() as f32;
                        for (i, (id, name, color, m)) in objs.into_iter().enumerate() {
                            match b3d_core::extrude::extrude_down(&m, h, 0.0, &p.range(i as f32 / n, (i + 1) as f32 / n)) {
                                Ok(out) => {
                                    remove.push(id);
                                    add.push((name, out, color));
                                }
                                Err(b3d_core::Error::Cancelled) => return JobOutput::Error("cancelled".into()),
                                Err(b3d_core::Error::Parse(_)) => {} // nothing below the height on this one
                                Err(e) => return JobOutput::Error(format!("Extrude down failed on {name}: {e}")),
                            }
                        }
                        if remove.is_empty() {
                            return JobOutput::Error("Nothing faces down below that height — raise the plane".into());
                        }
                        let n = remove.len();
                        JobOutput::Replace {
                            label: "Extrude down".into(),
                            remove,
                            add,
                            message: format!("Extruded {n} object{} down to the plate", if n == 1 { "" } else { "s" }),
                        }
                    }),
                )
            }
            PreviewKind::Cut => {
                let objs = world();
                let (point, normal) = self.cut_plane();
                let keep = self.cut.keep;
                (
                    "Cutting…".into(),
                    Box::new(move |p: &Progress| {
                        let mut add = vec![];
                        let mut remove = vec![];
                        let n = objs.len() as f32;
                        for (i, (id, name, color, m)) in objs.into_iter().enumerate() {
                            if p.is_cancelled() {
                                return JobOutput::Error("cancelled".into());
                            }
                            p.set(i as f32 / n);
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
                            return JobOutput::Error("The cut plane does not touch the selection".into());
                        }
                        let n = add.len();
                        JobOutput::Replace { label: "Cut".into(), remove, add, message: format!("Cut into {n} pieces") }
                    }),
                )
            }
        })
    }
}

fn build_result(key: u64, targets: Vec<ObjectId>, out: JobOutput, secs: f32, kind: PreviewKind, plane: (Vec3, Vec3)) -> PreviewResult {
    let mut r = PreviewResult { key, targets, swap: vec![], hide: vec![], add: vec![], message: String::new(), secs };
    match out {
        JobOutput::ReplaceMeshes { meshes, message, .. } => {
            r.swap = meshes.into_iter().map(|(id, m)| (id, MeshHandle::new(m))).collect();
            r.message = message;
        }
        JobOutput::Replace { remove, add, message, .. } => {
            r.hide = remove;
            // cut: pull the pieces apart so the cut is visible
            let gap = add.iter().map(|a| a.1.bounds().size().max_element()).fold(0.0, f32::max) * 0.08 + 2.0;
            r.add = add
                .into_iter()
                .map(|(n, m, c)| {
                    let off = if kind == PreviewKind::Cut {
                        let side = (m.bounds().center() - plane.0).dot(plane.1).signum();
                        plane.1 * side * gap * 0.5
                    } else {
                        Vec3::ZERO
                    };
                    (n, MeshHandle::new(m), c, off)
                })
                .collect();
            r.message = message;
        }
        _ => {}
    }
    r
}

/// Model matrix for a preview piece.
pub fn offset_matrix(off: Vec3) -> Mat4 {
    Mat4::from_translation(off)
}
