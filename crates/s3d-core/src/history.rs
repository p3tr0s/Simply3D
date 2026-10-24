//! Snapshot-based undo/redo. Scenes are cheap to clone because meshes are `Arc`-shared.

use crate::scene::Scene;

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<(String, Scene)>,
    redo: Vec<(String, Scene)>,
    pub limit: usize,
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self { limit, ..Default::default() }
    }

    /// Call *before* mutating `current`.
    pub fn push(&mut self, label: impl Into<String>, current: &Scene) {
        self.undo.push((label.into(), current.clone()));
        if self.limit > 0 && self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// What each step that can be undone did, the most recent first.
    pub fn undo_labels(&self) -> Vec<&str> {
        self.undo.iter().rev().map(|(l, _)| l.as_str()).collect()
    }

    /// Number of steps that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn undo(&mut self, current: &mut Scene) -> Option<String> {
        let (label, prev) = self.undo.pop()?;
        let now = std::mem::replace(current, prev);
        self.redo.push((label.clone(), now));
        Some(label)
    }

    pub fn redo(&mut self, current: &mut Scene) -> Option<String> {
        let (label, next) = self.redo.pop()?;
        let now = std::mem::replace(current, next);
        self.undo.push((label.clone(), now));
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cuboid;
    use glam::Vec3;

    #[test]
    fn undo_redo_roundtrip() {
        let mut s = Scene::default();
        let mut h = History::new(50);
        h.push("add", &s);
        s.add_world_mesh("a", cuboid(Vec3::ONE), None);
        assert_eq!(s.objects.len(), 1);
        assert_eq!(h.undo(&mut s).as_deref(), Some("add"));
        assert_eq!(s.objects.len(), 0);
        h.redo(&mut s);
        assert_eq!(s.objects.len(), 1);
    }
}
