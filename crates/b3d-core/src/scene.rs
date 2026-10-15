use crate::mesh::{Aabb, Mesh};
use glam::{EulerRot, Mat4, Quat, Vec3};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub type ObjectId = u64;

#[derive(Debug, Clone, Copy)]
pub struct PickHit {
    pub id: ObjectId,
    pub point: Vec3,
    pub normal: Vec3,
    pub corners: [Vec3; 3],
    pub distance: f32,
}

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);

/// Immutable, shareable mesh with a unique id (used as the GPU cache key).
#[derive(Debug, Clone)]
pub struct MeshHandle {
    pub id: u64,
    pub mesh: Arc<Mesh>,
}

impl MeshHandle {
    pub fn new(mesh: Mesh) -> Self {
        Self { id: NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed), mesh: Arc::new(mesh) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
    /// XYZ Euler angles in degrees (what the UI shows).
    pub fn euler_deg(&self) -> Vec3 {
        let (x, y, z) = self.rotation.to_euler(EulerRot::XYZ);
        // round away float noise and negative zero for display
        (Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)).map(|v| {
            let r = (v * 1e4).round() / 1e4;
            if r == 0.0 {
                0.0
            } else {
                r
            }
        })
    }
    pub fn set_euler_deg(&mut self, d: Vec3) {
        let r = d * (std::f32::consts::PI / 180.0);
        self.rotation = Quat::from_euler(EulerRot::XYZ, r.x, r.y, r.z);
    }
}

#[derive(Debug, Clone)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    /// Mesh in local space, centred on its bounding-box centre.
    pub mesh: MeshHandle,
    pub transform: Transform,
    /// sRGB colour, 0-255.
    pub color: [u8; 4],
    pub visible: bool,
}

impl Object {
    pub fn matrix(&self) -> Mat4 {
        self.transform.matrix()
    }
    pub fn world_mesh(&self) -> Mesh {
        self.mesh.mesh.transformed(&self.matrix())
    }
    pub fn world_bounds(&self) -> Aabb {
        // exact (not box-of-box) bounds: transform all vertices
        let m = self.matrix();
        Aabb::from_points(self.mesh.mesh.positions.iter().map(|p| m.transform_point3(*p)).collect::<Vec<_>>().iter())
    }
    /// Size of the local mesh times scale (rotation-independent dimensions).
    pub fn local_size(&self) -> Vec3 {
        self.mesh.mesh.bounds().size() * self.transform.scale.abs()
    }
}

pub const PALETTE: [[u8; 4]; 8] = [
    [96, 165, 250, 255],
    [251, 146, 60, 255],
    [74, 222, 128, 255],
    [244, 114, 182, 255],
    [250, 204, 21, 255],
    [167, 139, 250, 255],
    [45, 212, 191, 255],
    [248, 113, 113, 255],
];

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub objects: Vec<Object>,
    next_id: ObjectId,
}

impl Scene {
    /// Add a mesh given in world coordinates. It is re-centred so its pivot is the bbox centre.
    pub fn add_world_mesh(&mut self, name: impl Into<String>, mut mesh: Mesh, color: Option<[u8; 4]>) -> ObjectId {
        let c = mesh.bounds().center();
        mesh.translate(-c);
        self.add_object(name, mesh, Transform { translation: c, ..Default::default() }, color)
    }

    pub fn add_object(&mut self, name: impl Into<String>, mesh: Mesh, transform: Transform, color: Option<[u8; 4]>) -> ObjectId {
        self.next_id += 1;
        let id = self.next_id;
        let color = color.unwrap_or(PALETTE[(id as usize - 1) % PALETTE.len()]);
        self.objects.push(Object { id, name: name.into(), mesh: MeshHandle::new(mesh), transform, color, visible: true });
        id
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let i = self.objects.iter().position(|o| o.id == id)?;
        Some(self.objects.remove(i))
    }

    pub fn bounds(&self) -> Aabb {
        self.objects.iter().filter(|o| o.visible).fold(Aabb::EMPTY, |b, o| b.union(&o.world_bounds()))
    }
    pub fn bounds_of(&self, ids: &[ObjectId]) -> Aabb {
        ids.iter().filter_map(|id| self.get(*id)).fold(Aabb::EMPTY, |b, o| b.union(&o.world_bounds()))
    }

    /// Replace an object's mesh with a new world-space mesh, keeping its rotation/scale reset.
    pub fn set_world_mesh(&mut self, id: ObjectId, mut mesh: Mesh) {
        if let Some(o) = self.get_mut(id) {
            let c = mesh.bounds().center();
            mesh.translate(-c);
            o.mesh = MeshHandle::new(mesh);
            o.transform = Transform { translation: c, ..Default::default() };
        }
    }

    /// Bake rotation + scale into the vertices (keeps world geometry identical).
    pub fn bake_transform(&mut self, id: ObjectId) {
        if let Some(o) = self.get(id) {
            let w = o.world_mesh();
            self.set_world_mesh(id, w);
        }
    }

    pub fn drop_to_plate(&mut self, id: ObjectId) {
        let Some(o) = self.get(id) else { return };
        let dz = o.world_bounds().min.z;
        if let Some(o) = self.get_mut(id) {
            o.transform.translation.z -= dz;
        }
    }

    /// Ray pick: returns (object id, distance, world hit point).
    pub fn pick(&self, origin: Vec3, dir: Vec3) -> Option<(ObjectId, f32, Vec3)> {
        let mut best: Option<(ObjectId, f32, Vec3)> = None;
        for o in self.objects.iter().filter(|o| o.visible) {
            let m = o.matrix();
            let inv = m.inverse();
            let lo = inv.transform_point3(origin);
            let ld = inv.transform_vector3(dir);
            if let Some((t, _)) = o.mesh.mesh.ray_cast(lo, ld) {
                let hit = m.transform_point3(lo + ld * t);
                let dist = (hit - origin).length();
                if best.is_none_or(|b| dist < b.1) {
                    best = Some((o.id, dist, hit));
                }
            }
        }
        best
    }

    /// Detailed pick: hit point, world face normal and world triangle corners.
    pub fn pick_detail(&self, origin: Vec3, dir: Vec3) -> Option<PickHit> {
        self.pick_detail_where(origin, dir, |_| true)
    }

    /// Like [`pick_detail`](Self::pick_detail) but only considers objects for which `keep` is true.
    pub fn pick_detail_where(&self, origin: Vec3, dir: Vec3, keep: impl Fn(&Object) -> bool) -> Option<PickHit> {
        let mut best: Option<(f32, PickHit)> = None;
        for o in self.objects.iter().filter(|o| o.visible && keep(o)) {
            let m = o.matrix();
            let inv = m.inverse();
            let lo = inv.transform_point3(origin);
            let ld = inv.transform_vector3(dir);
            if let Some((t, tri)) = o.mesh.mesh.ray_cast(lo, ld) {
                let point = m.transform_point3(lo + ld * t);
                let dist = (point - origin).length();
                if best.as_ref().is_none_or(|b| dist < b.0) {
                    let mut normal = inv.transpose().transform_vector3(o.mesh.mesh.face_normal(tri)).normalize_or_zero();
                    if m.determinant() < 0.0 {
                        normal = -normal;
                    }
                    let corners = o.mesh.mesh.tri(tri).map(|p| m.transform_point3(p));
                    best = Some((dist, PickHit { id: o.id, point, normal, corners, distance: dist }));
                }
            }
        }
        best.map(|b| b.1)
    }

    pub fn unique_name(&self, base: &str) -> String {
        if !self.objects.iter().any(|o| o.name == base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !self.objects.iter().any(|o| &o.name == n)).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cuboid;

    #[test]
    fn add_recentres_and_picks() {
        let mut s = Scene::default();
        let mut m = cuboid(Vec3::splat(2.0));
        m.translate(Vec3::new(10.0, 0.0, 1.0));
        let id = s.add_world_mesh("cube", m, None);
        let o = s.get(id).unwrap();
        assert!((o.transform.translation - Vec3::new(10.0, 0.0, 1.0)).length() < 1e-6);
        let (hit, d, p) = s.pick(Vec3::new(10.0, 0.0, 50.0), -Vec3::Z).unwrap();
        assert_eq!(hit, id);
        assert!((d - 48.0).abs() < 1e-4);
        assert!((p.z - 2.0).abs() < 1e-4);
    }

    #[test]
    fn drop_to_plate_works_with_rotation() {
        let mut s = Scene::default();
        let id = s.add_world_mesh("c", cuboid(Vec3::new(10.0, 2.0, 2.0)), None);
        s.get_mut(id).unwrap().transform.set_euler_deg(Vec3::new(0.0, 90.0, 0.0));
        s.drop_to_plate(id);
        assert!(s.get(id).unwrap().world_bounds().min.z.abs() < 1e-4);
    }
}
