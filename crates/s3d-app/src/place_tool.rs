//! Click-to-place: a new shape follows the pointer over the plate and over faces of other
//! objects (standing on the face, along its normal) until you click (Tinkercad's
//! workplane). Enter puts it beside the others, Esc cancels.

use crate::app::App;
use glam::{Quat, Vec3};
use s3d_core::scene::MeshHandle;
use s3d_core::{Mesh, Transform};

pub struct Placing {
    pub ghost: MeshHandle,
    pub name: String,
    pub hole: bool,
    /// Shown after placing.
    pub note: Option<String>,
    /// Generator settings, kept on the object so it can be edited later.
    pub params: Option<s3d_core::primitives::Primitive>,
}

impl App {
    /// Start placing a new shape (its mesh centred on its bounding box).
    pub fn begin_placing(&mut self, mesh: Mesh, name: String, hole: bool, note: Option<String>, params: Option<s3d_core::primitives::Primitive>) {
        if !self.settings.place_by_click {
            let t = self.placement(None, &mesh);
            self.add_placed(mesh, name, hole, note, t, params);
            return;
        }
        self.placing = Some(Placing { ghost: MeshHandle::new(mesh), name, hole, note, params });
        self.place_hover = None;
    }

    /// Where the shape goes: standing on the hovered face / plate spot, or beside the others.
    pub fn placement(&self, at: Option<(Vec3, Vec3)>, mesh: &Mesh) -> Transform {
        let b = mesh.bounds();
        match at {
            Some((p, n)) => {
                let n = n.normalize_or(Vec3::Z);
                let rotation = Quat::from_rotation_arc(Vec3::Z, n);
                // the shape's bottom sits on the surface
                Transform { translation: p - n * b.min.z, rotation, scale: Vec3::ONE }
            }
            None => {
                let existing = self.scene.bounds();
                let size = b.size();
                let mut pos = Vec3::new(0.0, 0.0, -b.min.z);
                if !existing.is_empty() {
                    pos.x = existing.max.x + 10.0 + size.x * 0.5;
                    pos.y = existing.center().y;
                }
                Transform { translation: pos, ..Default::default() }
            }
        }
    }

    fn add_placed(&mut self, mesh: Mesh, name: String, hole: bool, note: Option<String>, t: Transform, params: Option<s3d_core::primitives::Primitive>) {
        self.checkpoint(&format!("Add {name}"));
        let name = self.scene.unique_name(&name);
        let id = self.scene.add_object(name, mesh, t, None);
        if let Some(o) = self.scene.get_mut(id) {
            o.hole = hole;
            if let Some(p) = params {
                o.set_params(p);
            }
        }
        self.selection = vec![id];
        if let Some(n) = note {
            self.info(n);
        }
    }

    /// Drop the shape at `at` (None = beside the others).
    pub fn commit_placing(&mut self, at: Option<(Vec3, Vec3)>) {
        let Some(p) = self.placing.take() else { return };
        let mesh = (*p.ghost.mesh).clone();
        let t = self.placement(at, &mesh);
        self.add_placed(mesh, p.name, p.hole, p.note, t, p.params);
    }

    pub fn cancel_placing(&mut self) {
        if self.placing.take().is_some() {
            self.info("Cancelled");
        }
    }
}
