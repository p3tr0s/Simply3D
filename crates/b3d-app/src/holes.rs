//! Hole (negative) objects: they cut every solid they overlap, live and non-destructively.
//! The cut is computed in the background and shown in place of the solid; the scene keeps
//! the original solids and holes so everything stays editable.

use crate::app::{App, JobOutput};
use crate::preview::BgTask;
use b3d_core::csg::{self, BoolOp};
use b3d_core::scene::MeshHandle;
use b3d_core::{Aabb, Mesh, ObjectId};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

type CutResult = Vec<(ObjectId, u64, Option<Mesh>)>;

#[derive(Default)]
pub struct HoleCache {
    /// solid -> (inputs key, cut mesh in the solid's local space; None = nothing left)
    pub cuts: HashMap<ObjectId, (u64, Option<MeshHandle>)>,
    task: Option<BgTask<CutResult>>,
    pub busy: bool,
}

fn overlap(a: &Aabb, b: &Aabb) -> bool {
    b3d_core::io::overlaps(a, b)
}

impl App {
    pub fn any_hole_selected(&self) -> bool {
        self.selection.iter().any(|id| self.scene.get(*id).is_some_and(|o| o.hole))
    }

    /// Turn the selection into holes (or back into solids).
    pub fn set_hole_selected(&mut self, hole: bool) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint(if hole { "Make hole" } else { "Make solid" });
        for id in self.selection.clone() {
            if let Some(o) = self.scene.get_mut(id) {
                o.hole = hole;
            }
        }
        if hole {
            self.info("Hole: it cuts whatever it overlaps. Move it around freely; Apply holes makes the cut permanent.");
        }
    }

    pub fn toggle_hole_selected(&mut self) {
        let make = !self.selection.iter().all(|id| self.scene.get(*id).is_some_and(|o| o.hole));
        self.set_hole_selected(make);
    }

    /// Visible solids overlapping each visible hole: (solid, holes) pairs.
    fn hole_pairs(&mut self) -> Vec<(ObjectId, Vec<ObjectId>)> {
        let holes: Vec<ObjectId> = self.scene.objects.iter().filter(|o| o.visible && o.hole).map(|o| o.id).collect();
        if holes.is_empty() {
            return vec![];
        }
        let hb: Vec<(ObjectId, Aabb)> = holes.into_iter().filter_map(|h| self.object_bounds(h).map(|b| (h, b))).collect();
        let solids: Vec<ObjectId> = self.scene.objects.iter().filter(|o| o.visible && !o.hole).map(|o| o.id).collect();
        let mut out = vec![];
        for s in solids {
            let Some(sb) = self.object_bounds(s) else { continue };
            let hs: Vec<ObjectId> = hb.iter().filter(|(_, b)| overlap(&sb, b)).map(|(h, _)| *h).collect();
            if !hs.is_empty() {
                out.push((s, hs));
            }
        }
        out
    }

    fn cut_key(&self, solid: ObjectId, holes: &[ObjectId]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for id in std::iter::once(&solid).chain(holes) {
            if let Some(o) = self.scene.get(*id) {
                o.mesh.id.hash(&mut h);
                o.mesh.paint_rev.hash(&mut h);
                format!("{:?}", o.transform).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Keep the displayed cuts up to date (called every frame).
    pub fn tick_holes(&mut self, ctx: &egui::Context) {
        if let Some(out) = self.holes.task.as_ref().and_then(|t| t.poll()) {
            self.holes.task = None;
            for (id, key, m) in out.unwrap_or_default() {
                self.holes.cuts.insert(id, (key, m.map(MeshHandle::new)));
            }
        }
        let pairs = self.hole_pairs();
        let live: Vec<ObjectId> = pairs.iter().map(|p| p.0).collect();
        self.holes.cuts.retain(|id, _| live.contains(id));
        let stale: Vec<(ObjectId, u64, Vec<ObjectId>)> = pairs
            .into_iter()
            .map(|(s, hs)| (s, self.cut_key(s, &hs), hs))
            .filter(|(s, k, _)| self.holes.cuts.get(s).is_none_or(|c| c.0 != *k))
            .collect();
        self.holes.busy = !stale.is_empty();
        if stale.is_empty() || self.holes.task.is_some() {
            if self.holes.task.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        // inputs: each solid's local mesh, and its holes moved into that local space
        let jobs: Vec<(ObjectId, u64, Mesh, Vec<Mesh>)> = stale
            .into_iter()
            .filter_map(|(s, key, hs)| {
                let so = self.scene.get(s)?;
                let inv = so.matrix().inverse();
                let cutters = hs.iter().filter_map(|h| self.scene.get(*h)).map(|h| h.mesh.mesh.transformed(&(inv * h.matrix()))).collect();
                Some((s, key, (*so.mesh.mesh).clone(), cutters))
            })
            .collect();
        self.holes.task = Some(BgTask::spawn(ctx, 0, move |p| {
            let mut out = vec![];
            for (s, key, m, cutters) in jobs {
                if p.is_cancelled() {
                    break;
                }
                let cut = csg::boolean_many(&m, &cutters, BoolOp::Subtract).ok();
                // a failed boolean shows the solid uncut rather than nothing
                let cut = match cut {
                    Some(c) if c.is_empty() => None,
                    Some(mut c) => {
                        let tol = (c.bounds().size().length() * 0.002).clamp(0.05, 0.5);
                        b3d_core::paint::transfer(&m, &mut c, tol);
                        Some(c)
                    }
                    None => Some(m),
                };
                out.push((s, key, cut));
            }
            out
        }));
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Make the cut permanent. Selected holes cut what they overlap; with only solids
    /// selected, the holes overlapping them are applied to them. Holes that no longer
    /// cut anything else are removed.
    pub fn apply_holes_selected(&mut self, ctx: &egui::Context) {
        let pairs = self.hole_pairs();
        let sel_holes: Vec<ObjectId> = self.selection.iter().copied().filter(|id| self.scene.get(*id).is_some_and(|o| o.hole)).collect();
        let sel_solids: Vec<ObjectId> = self.selection.iter().copied().filter(|id| self.scene.get(*id).is_some_and(|o| !o.hole)).collect();
        let all = self.selection.is_empty();
        let mut work: Vec<(ObjectId, Vec<ObjectId>)> = vec![];
        for (s, hs) in &pairs {
            let hs: Vec<ObjectId> = hs.iter().copied().filter(|h| all || sel_holes.contains(h) || sel_holes.is_empty()).collect();
            let wanted = all || (!sel_holes.is_empty() && !hs.is_empty()) || sel_solids.contains(s);
            if wanted && !hs.is_empty() {
                work.push((*s, hs));
            }
        }
        if work.is_empty() {
            self.error("No hole overlaps a solid here");
            return;
        }
        // holes are removed only if every solid they overlap gets cut now
        let used: Vec<ObjectId> = work.iter().flat_map(|w| w.1.clone()).collect();
        let mut remove_holes: Vec<ObjectId> = vec![];
        for h in used {
            if remove_holes.contains(&h) {
                continue;
            }
            let everywhere =
                pairs.iter().filter(|(_, hs)| hs.contains(&h)).all(|(s, _)| work.iter().any(|w| w.0 == *s && w.1.contains(&h)));
            if everywhere {
                remove_holes.push(h);
            }
        }
        let jobs: Vec<(ObjectId, String, [u8; 4], Mesh, Vec<Mesh>)> = work
            .iter()
            .filter_map(|(s, hs)| {
                let so = self.scene.get(*s)?;
                let cutters = hs.iter().filter_map(|h| self.scene.get(*h)).map(|h| h.world_mesh()).collect();
                Some((*s, so.name.clone(), so.color, so.world_mesh(), cutters))
            })
            .collect();
        self.spawn_job(ctx, "Applying holes…", move |p| {
            let n = jobs.len() as f32;
            let mut remove = vec![];
            let mut add = vec![];
            for (i, (s, name, color, m, cutters)) in jobs.into_iter().enumerate() {
                if p.is_cancelled() {
                    return JobOutput::Error("cancelled".into());
                }
                p.set(i as f32 / n);
                match csg::boolean_many(&m, &cutters, BoolOp::Subtract) {
                    Ok(c) => {
                        remove.push(s);
                        add.push((name, c, color));
                    }
                    Err(e) => return JobOutput::Error(format!("Apply holes failed on {name}: {e}")),
                }
            }
            let k = add.len();
            remove.extend(remove_holes);
            JobOutput::Replace {
                label: "Apply holes".into(),
                remove,
                add,
                message: format!("Holes applied to {k} object{}", if k == 1 { "" } else { "s" }),
            }
        });
    }

    /// The mesh to draw for a solid: its live cut if it has holes.
    pub fn display_mesh(&self, id: ObjectId) -> Option<Option<MeshHandle>> {
        self.holes.cuts.get(&id).map(|c| c.1.clone())
    }
}
