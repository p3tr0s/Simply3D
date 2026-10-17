//! Groups, hide / isolate / show, lock — and group-aware selection.

use crate::app::App;
use s3d_core::scene::GroupId;
use s3d_core::ObjectId;

impl App {
    /// Select from a click on an object. Clicking a grouped object selects the whole group
    /// unless `single` (Alt+click in the viewport, or clicking a member row in the list).
    pub fn click_select(&mut self, id: ObjectId, additive: bool, single: bool) {
        let ids = if single { vec![id] } else { self.scene.with_group(id) };
        if additive {
            if ids.iter().all(|i| self.selection.contains(i)) {
                self.selection.retain(|s| !ids.contains(s));
            } else {
                for i in ids {
                    if !self.selection.contains(&i) {
                        self.selection.push(i);
                    }
                }
            }
        } else {
            // keep the clicked object first: it is the base for booleans
            let mut v = vec![id];
            v.extend(ids.into_iter().filter(|i| *i != id));
            self.selection = v;
        }
    }

    pub fn select_group(&mut self, g: GroupId, additive: bool) {
        let ids = self.scene.members(g);
        if let Some(&first) = ids.first() {
            self.click_select(first, additive, false);
        }
    }

    /// The group whose members are exactly the current selection.
    pub fn selected_group(&self) -> Option<GroupId> {
        let g = self.scene.group_of(*self.selection.first()?)?;
        let m = self.scene.members(g);
        (m.len() == self.selection.len() && m.iter().all(|i| self.selection.contains(i))).then_some(g)
    }

    pub fn can_ungroup(&self) -> bool {
        self.selection.iter().any(|id| self.scene.group_of(*id).is_some())
    }

    pub fn group_selected(&mut self) {
        if self.selection.len() < 2 {
            self.error("Select two or more objects to group them");
            return;
        }
        self.checkpoint("Group");
        let n = (1..).find(|i| !self.scene.groups.iter().any(|g| g.name == format!("Group {i}"))).unwrap();
        self.scene.group(&self.selection.clone(), format!("Group {n}"));
        self.info(format!("Grouped {} objects — click selects them together, Alt+click picks one", self.selection.len()));
    }

    pub fn ungroup_selected(&mut self) {
        if !self.can_ungroup() {
            return;
        }
        self.checkpoint("Ungroup");
        // ungroup whole groups touched by the selection
        let mut ids = self.selection.clone();
        for id in self.selection.clone() {
            for m in self.scene.with_group(id) {
                if !ids.contains(&m) {
                    ids.push(m);
                }
            }
        }
        self.scene.ungroup(&ids);
        self.info("Ungrouped");
    }

    pub fn rename_group(&mut self, g: GroupId, name: String) {
        self.checkpoint_coalesced("Rename group");
        if let Some(gr) = self.scene.groups.iter_mut().find(|x| x.id == g) {
            gr.name = name;
        }
    }

    // ---------- visibility ----------

    pub fn set_visible(&mut self, ids: &[ObjectId], visible: bool) {
        self.checkpoint(if visible { "Show" } else { "Hide" });
        for o in self.scene.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
            o.visible = visible;
        }
        if !visible {
            self.selection.retain(|s| !ids.contains(s));
        }
    }

    pub fn hide_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        let n = ids.len();
        self.set_visible(&ids, false);
        self.info(format!("Hid {n} object{} — Shift+H shows everything again", if n == 1 { "" } else { "s" }));
    }

    pub fn any_hidden(&self) -> bool {
        self.scene.objects.iter().any(|o| !o.visible)
    }

    pub fn show_all(&mut self) {
        self.isolated = None;
        if !self.any_hidden() {
            return;
        }
        let ids: Vec<_> = self.scene.objects.iter().filter(|o| !o.visible).map(|o| o.id).collect();
        self.set_visible(&ids, true);
    }

    /// Show only the selection (I). Pressing I again brings the others back.
    pub fn toggle_isolate(&mut self) {
        if let Some(hidden) = self.isolated.take() {
            let ids: Vec<_> = hidden.into_iter().filter(|id| self.scene.get(*id).is_some_and(|o| !o.visible)).collect();
            if !ids.is_empty() {
                self.set_visible(&ids, true);
            }
            return;
        }
        if self.selection.is_empty() {
            self.error("Select what you want to isolate");
            return;
        }
        let others: Vec<_> = self.scene.objects.iter().filter(|o| o.visible && !self.selection.contains(&o.id)).map(|o| o.id).collect();
        if others.is_empty() {
            return;
        }
        self.set_visible(&others, false);
        self.isolated = Some(others);
        self.fit_on_next_frame = false;
        self.frame_selection();
    }

    // ---------- lock ----------

    pub fn any_locked_selected(&self) -> bool {
        self.selection.iter().any(|id| self.scene.get(*id).is_some_and(|o| o.locked))
    }

    /// Lock the selection (or unlock it if it is all locked already).
    pub fn toggle_lock_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let lock = !self.selection.iter().all(|id| self.scene.get(*id).is_some_and(|o| o.locked));
        let ids = self.selection.clone();
        self.set_locked(&ids, lock);
    }

    pub fn set_locked(&mut self, ids: &[ObjectId], lock: bool) {
        self.checkpoint(if lock { "Lock" } else { "Unlock" });
        for o in self.scene.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
            o.locked = lock;
        }
        if lock {
            self.selection.retain(|s| !ids.contains(s));
            self.info("Locked: it can't be clicked or moved in the view. Unlock it from the object list.");
        }
    }
}
