//! Auto-orient and auto-arrange, working on "units": a group moves as one, and a loose
//! hole travels with the solid it cuts.

use crate::app::App;
use b3d_core::arrange::{best_orientation, pack};
use b3d_core::{Aabb, Mesh, ObjectId};
use glam::{Quat, Vec2, Vec3};

impl App {
    /// Objects that move together: groups, single objects, and loose holes attached to
    /// the first solid they overlap.
    pub fn units_of(&self, ids: &[ObjectId]) -> Vec<Vec<ObjectId>> {
        let mut units: Vec<Vec<ObjectId>> = vec![];
        let mut seen: Vec<ObjectId> = vec![];
        let mut loose_holes = vec![];
        for &id in ids {
            if seen.contains(&id) {
                continue;
            }
            let Some(o) = self.scene.get(id) else { continue };
            if o.hole && o.group.is_none() {
                loose_holes.push(id);
                continue;
            }
            let members = self.scene.with_group(id);
            seen.extend(&members);
            units.push(members);
        }
        for h in loose_holes {
            let hb = self.scene.bounds_of(&[h]);
            match units.iter_mut().find(|u| b3d_core::io::overlaps(&self.scene.bounds_of(u), &hb)) {
                Some(u) => u.push(h),
                None => units.push(vec![h]),
            }
        }
        units
    }

    /// Rotate each selected object (or group) so it prints with the least overhang and the
    /// most contact with the plate.
    pub fn auto_orient_selected(&mut self) {
        let ids = if self.selection.is_empty() { self.visible_ids() } else { self.selection.clone() };
        let units = self.units_of(&ids);
        if units.is_empty() {
            return;
        }
        self.checkpoint("Auto-orient");
        let mut changed = 0;
        let mut saved = 0.0f32;
        for u in units {
            let solids: Vec<ObjectId> = u.iter().copied().filter(|id| self.scene.get(*id).is_some_and(|o| !o.hole)).collect();
            let mut all = Mesh::default();
            for id in &solids {
                if let Some(o) = self.scene.get(*id) {
                    all.append(&o.world_mesh());
                }
            }
            if all.is_empty() {
                continue;
            }
            let (q, before, after) = best_orientation(&all);
            if q.angle_between(Quat::IDENTITY) < 1e-3 {
                continue;
            }
            let c = all.bounds().center();
            for id in &u {
                if let Some(o) = self.scene.get_mut(*id) {
                    o.transform.translation = c + q * (o.transform.translation - c);
                    o.transform.rotation = (q * o.transform.rotation).normalize();
                }
            }
            // back onto the plate as one piece
            let dz = self.scene.bounds_of(&solids).min.z;
            for id in &u {
                if let Some(o) = self.scene.get_mut(*id) {
                    o.transform.translation.z -= dz;
                }
            }
            changed += 1;
            saved += (before.overhang - after.overhang).max(0.0);
        }
        self.info(if changed == 0 {
            "Already in the best orientation".to_string()
        } else {
            format!("Re-oriented {changed} part{} — about {:.0} mm² less overhang", if changed == 1 { "" } else { "s" }, saved)
        });
    }

    fn visible_ids(&self) -> Vec<ObjectId> {
        self.scene.objects.iter().filter(|o| o.visible && !o.locked).map(|o| o.id).collect()
    }

    /// Pack the selection (or everything) on the plate, centred, `gap` mm apart.
    pub fn auto_arrange(&mut self, gap: f32) {
        let ids = if self.selection.len() >= 2 { self.selection.clone() } else { self.visible_ids() };
        let units = self.units_of(&ids);
        if units.is_empty() {
            return;
        }
        let boxes: Vec<Aabb> = units
            .iter()
            .map(|u| {
                let solids: Vec<ObjectId> = u.iter().copied().filter(|id| self.scene.get(*id).is_some_and(|o| !o.hole)).collect();
                self.scene.bounds_of(if solids.is_empty() { u } else { &solids })
            })
            .collect();
        let sizes: Vec<Vec2> = boxes.iter().map(|b| b.size().truncate()).collect();
        let centres = pack(&sizes, gap, self.settings.plate.x);
        self.checkpoint("Arrange");
        for ((u, b), c) in units.iter().zip(&boxes).zip(centres) {
            let d = Vec3::new(c.x - b.center().x, c.y - b.center().y, -b.min.z);
            for id in u {
                if let Some(o) = self.scene.get_mut(*id) {
                    o.transform.translation += d;
                }
            }
        }
        let total = self.scene.bounds_of(&ids);
        let fits = total.size().x <= self.settings.plate.x + 0.01 && total.size().y <= self.settings.plate.y + 0.01;
        self.info(if fits {
            format!("Arranged {} part{} on the plate", units.len(), if units.len() == 1 { "" } else { "s" })
        } else {
            "Arranged — but it doesn't all fit on one plate".to_string()
        });
    }
}
