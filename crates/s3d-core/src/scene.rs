use crate::bvh::Bvh;
use crate::mesh::{Aabb, Mesh};
use crate::normals::Shading;
use glam::{EulerRot, Mat4, Quat, Vec3};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub type ObjectId = u64;

#[derive(Debug, Clone, Copy)]
pub struct PickHit {
    pub id: ObjectId,
    pub point: Vec3,
    pub normal: Vec3,
    pub corners: [Vec3; 3],
    pub distance: f32,
    /// Triangle index in the object's mesh.
    pub tri: usize,
}

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);

/// Meshes up to this size get their picking index built right away; bigger ones in the
/// background (picking falls back to testing every triangle until it's ready).
const BVH_SYNC_LIMIT: usize = 150_000;

/// Things worked out once per mesh: bounds and the picking index.
#[derive(Default)]
pub struct MeshCache {
    bounds: OnceLock<Aabb>,
    bvh: Mutex<Option<Arc<Bvh>>>,
    building: AtomicBool,
    /// Bounds of the rotated + scaled vertices, for the last rotation and scale asked for.
    world: Mutex<Option<(Quat, Vec3, Aabb)>>,
    shading: OnceLock<Arc<Shading>>,
    /// Grouping from an earlier mesh with the same triangles (normals still to update).
    inherited: Mutex<Option<Arc<Shading>>>,
    /// Whether the mesh has holes, flipped or non-manifold edges (worked out once).
    broken: Arc<OnceLock<bool>>,
    checking: AtomicBool,
}

/// Which older mesh's display shows this mesh: valid only if that display has received
/// exactly the in-place updates `tag`/`seq` (a stroke and how many updates it sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuAlias {
    pub id: u64,
    pub tag: u64,
    pub seq: u64,
}

/// Triangles repainted between two paint revisions, newest first (lets the display
/// update just those instead of the whole mesh).
#[derive(Debug)]
pub struct PaintDelta {
    pub from: u64,
    pub to: u64,
    pub tris: Vec<u32>,
    pub prev: Option<Arc<PaintDelta>>,
    depth: u32,
}

impl std::fmt::Debug for MeshCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeshCache")
    }
}

/// Immutable, shareable mesh with a unique id (used as the GPU cache key).
#[derive(Debug, Clone)]
pub struct MeshHandle {
    pub id: u64,
    pub mesh: Arc<Mesh>,
    /// Changes whenever the paint changes while the geometry (and `id`) stays the same.
    pub paint_rev: u64,
    /// The latest repaints, if they were small (see [`PaintDelta`]).
    pub paint_delta: Option<Arc<PaintDelta>>,
    /// The display of an older mesh already shows this one (it was updated in place while
    /// sculpting), so it can be reused instead of uploaded again.
    pub gpu_alias: Option<GpuAlias>,
    cache: Arc<MeshCache>,
}

impl MeshHandle {
    pub fn new(mesh: Mesh) -> Self {
        let id = NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed);
        Self { id, mesh: Arc::new(mesh), paint_rev: id, paint_delta: None, gpu_alias: None, cache: Arc::default() }
    }

    /// The same triangles as `prev` with some vertices moved: shares its smooth-shading
    /// layout, reuses its display, and comes with a ready picking index.
    pub fn moved_from(prev: &MeshHandle, mesh: Mesh, bvh: Arc<Bvh>, tag: u64, seq: u64) -> Self {
        let mut h = Self::with_bvh(mesh, bvh);
        h.gpu_alias = Some(GpuAlias { id: prev.id, tag, seq });
        if let Some(sh) = prev.cache.shading.get() {
            if h.mesh.triangles.len() == prev.mesh.triangles.len() {
                *h.cache.inherited.lock().unwrap() = Some(sh.clone());
            }
        }
        h
    }

    /// A new mesh whose picking index is already known (e.g. refitted while sculpting).
    pub fn with_bvh(mesh: Mesh, bvh: Arc<Bvh>) -> Self {
        let h = Self::new(mesh);
        if bvh.len() == h.mesh.triangles.len() {
            *h.cache.bvh.lock().unwrap() = Some(bvh);
        }
        h
    }

    /// Same geometry, new paint (keeps the id so geometry caches stay valid).
    pub fn repaint(&mut self, f: impl FnOnce(&mut Vec<u32>)) {
        let m = Arc::make_mut(&mut self.mesh);
        f(m.paint_mut());
        m.tidy_paint();
        self.paint_rev = NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed);
        self.paint_delta = None;
    }

    /// Set the paint of a few triangles (the display updates only those).
    pub fn repaint_tris(&mut self, tris: &[usize], value: impl Fn(usize) -> u32) {
        let was_painted = self.mesh.is_painted();
        let m = Arc::make_mut(&mut self.mesh);
        let p = m.paint_mut();
        for &t in tris {
            p[t] = value(t);
        }
        m.tidy_paint();
        let from = self.paint_rev;
        self.paint_rev = NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed);
        // painting the first triangle or clearing the last changes the display layout
        if was_painted != self.mesh.is_painted() {
            self.paint_delta = None;
            return;
        }
        let prev = self.paint_delta.take().filter(|d| d.to == from && d.depth < 64);
        let depth = prev.as_ref().map_or(0, |d| d.depth + 1);
        self.paint_delta =
            Some(Arc::new(PaintDelta { from, to: self.paint_rev, tris: tris.iter().map(|&t| t as u32).collect(), prev, depth }));
    }

    /// Whether the mesh needs repair (holes, flipped or non-manifold edges). Big meshes are
    /// checked in the background: `None` until the answer is ready.
    pub fn needs_repair(&self) -> Option<bool> {
        if let Some(b) = self.cache.broken.get() {
            return Some(*b);
        }
        if self.mesh.triangles.len() <= BVH_SYNC_LIMIT {
            let b = !crate::analysis::analyze(&self.mesh).is_watertight();
            let _ = self.cache.broken.set(b);
            return Some(b);
        }
        if !self.cache.checking.swap(true, Ordering::AcqRel) {
            let (mesh, broken) = (self.mesh.clone(), self.cache.broken.clone());
            std::thread::spawn(move || {
                let _ = broken.set(!crate::analysis::analyze(&mesh).is_watertight());
            });
        }
        None
    }

    /// A fresh unique number (for tagging in-place display updates).
    pub fn next_tag() -> u64 {
        NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Smooth-shading layout (worked out once).
    pub fn shading(&self) -> Arc<Shading> {
        self.cache
            .shading
            .get_or_init(|| match self.cache.inherited.lock().unwrap().take() {
                Some(sh) => Arc::new(sh.with_positions(&self.mesh)),
                None => Arc::new(crate::normals::shading(&self.mesh, crate::normals::CREASE_DEG)),
            })
            .clone()
    }

    /// Bounding box of the mesh (worked out once).
    pub fn bounds(&self) -> Aabb {
        *self.cache.bounds.get_or_init(|| self.mesh.bounds())
    }

    /// The picking index, if it's ready. Small meshes build it now; big ones start it in
    /// the background and return `None` until it's done.
    pub fn bvh(&self) -> Option<Arc<Bvh>> {
        if let Some(b) = self.cache.bvh.lock().unwrap().as_ref() {
            return Some(b.clone());
        }
        if self.mesh.triangles.len() <= BVH_SYNC_LIMIT {
            let b = Arc::new(Bvh::build(&self.mesh));
            *self.cache.bvh.lock().unwrap() = Some(b.clone());
            return Some(b);
        }
        if !self.cache.building.swap(true, Ordering::AcqRel) {
            let (mesh, cache) = (self.mesh.clone(), self.cache.clone());
            std::thread::spawn(move || {
                let b = Arc::new(Bvh::build(&mesh));
                *cache.bvh.lock().unwrap() = Some(b);
            });
        }
        None
    }

    /// Nearest ray hit in mesh space: (distance, triangle).
    pub fn ray_cast_where(&self, origin: Vec3, dir: Vec3, accept: impl Fn(Vec3) -> bool) -> Option<(f32, usize)> {
        self.bounds().ray_hit(origin, dir)?;
        match self.bvh() {
            Some(b) => b.ray_cast_where(&self.mesh, origin, dir, accept),
            None => self.mesh.ray_cast_where(origin, dir, accept),
        }
    }

    /// Exact bounds of the mesh after `t` (cached for the last rotation and scale, so
    /// moving an object doesn't walk all its vertices again).
    pub fn world_bounds(&self, t: &Transform) -> Aabb {
        let mut w = self.cache.world.lock().unwrap();
        if let Some((r, s, b)) = *w {
            if r == t.rotation && s == t.scale {
                return Aabb { min: b.min + t.translation, max: b.max + t.translation };
            }
        }
        let m = Mat4::from_scale_rotation_translation(t.scale, t.rotation, Vec3::ZERO);
        let mut b = Aabb::EMPTY;
        for p in &self.mesh.positions {
            b.grow(m.transform_point3(*p));
        }
        *w = Some((t.rotation, t.scale, b));
        if b.is_empty() {
            return b;
        }
        Aabb { min: b.min + t.translation, max: b.max + t.translation }
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
    /// Locked objects can't be picked or moved in the viewport.
    pub locked: bool,
    pub group: Option<GroupId>,
    /// Negative object: cuts the solids it overlaps, never printed.
    pub hole: bool,
    /// The generator settings this mesh was made from (see [`Object::parametric`]).
    pub params: Option<Params>,
}

/// Settings of a generated shape, valid while the object still has the mesh they built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub prim: crate::primitives::Primitive,
    pub mesh_id: u64,
}

pub type GroupId = u64;

#[derive(Debug, Clone)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
}

impl Object {
    /// The generator settings, if the mesh hasn't been edited since it was generated.
    pub fn parametric(&self) -> Option<crate::primitives::Primitive> {
        self.params.filter(|p| p.mesh_id == self.mesh.id).map(|p| p.prim)
    }
    /// Remember that the current mesh was generated from `prim`.
    pub fn set_params(&mut self, prim: crate::primitives::Primitive) {
        self.params = Some(Params { prim, mesh_id: self.mesh.id });
    }
    pub fn matrix(&self) -> Mat4 {
        self.transform.matrix()
    }
    pub fn world_mesh(&self) -> Mesh {
        self.mesh.mesh.transformed(&self.matrix())
    }
    pub fn world_bounds(&self) -> Aabb {
        // exact (not box-of-box) bounds of the transformed vertices
        self.mesh.world_bounds(&self.transform)
    }
    /// Size of the local mesh times scale (rotation-independent dimensions).
    pub fn local_size(&self) -> Vec3 {
        self.mesh.bounds().size() * self.transform.scale.abs()
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
    pub groups: Vec<Group>,
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
        self.objects.push(Object {
            id,
            name: name.into(),
            mesh: MeshHandle::new(mesh),
            transform,
            color,
            visible: true,
            locked: false,
            group: None,
            hole: false,
            params: None,
        });
        id
    }

    // ---------- groups ----------

    /// Put the objects in a new group (removing them from any previous group).
    pub fn group(&mut self, ids: &[ObjectId], name: impl Into<String>) -> GroupId {
        self.next_id += 1;
        let gid = self.next_id;
        self.groups.push(Group { id: gid, name: name.into() });
        for o in self.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
            o.group = Some(gid);
        }
        self.prune_groups();
        gid
    }

    pub fn ungroup(&mut self, ids: &[ObjectId]) {
        for o in self.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
            o.group = None;
        }
        self.prune_groups();
    }

    /// Drop groups with fewer than two members.
    pub fn prune_groups(&mut self) {
        let objects = &mut self.objects;
        self.groups.retain(|g| objects.iter().filter(|o| o.group == Some(g.id)).count() >= 2);
        let live: Vec<GroupId> = self.groups.iter().map(|g| g.id).collect();
        for o in objects.iter_mut() {
            if o.group.is_some_and(|g| !live.contains(&g)) {
                o.group = None;
            }
        }
    }

    pub fn group_of(&self, id: ObjectId) -> Option<GroupId> {
        self.get(id).and_then(|o| o.group)
    }

    pub fn group_name(&self, gid: GroupId) -> Option<&str> {
        self.groups.iter().find(|g| g.id == gid).map(|g| g.name.as_str())
    }

    pub fn members(&self, gid: GroupId) -> Vec<ObjectId> {
        self.objects.iter().filter(|o| o.group == Some(gid)).map(|o| o.id).collect()
    }

    /// The object plus the rest of its group.
    pub fn with_group(&self, id: ObjectId) -> Vec<ObjectId> {
        match self.group_of(id) {
            Some(g) => self.members(g),
            None => vec![id],
        }
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let i = self.objects.iter().position(|o| o.id == id)?;
        let o = self.objects.remove(i);
        if o.group.is_some() {
            self.prune_groups();
        }
        Some(o)
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
        self.pick_filtered(origin, dir, |_| true, |_| true).map(|h| (h.id, h.distance, h.point))
    }

    /// Detailed pick: hit point, world face normal and world triangle corners.
    pub fn pick_detail(&self, origin: Vec3, dir: Vec3) -> Option<PickHit> {
        self.pick_filtered(origin, dir, |_| true, |_| true)
    }

    /// Like [`pick_detail`](Self::pick_detail) but only considers objects for which `keep` is true.
    pub fn pick_detail_where(&self, origin: Vec3, dir: Vec3, keep: impl Fn(&Object) -> bool) -> Option<PickHit> {
        self.pick_filtered(origin, dir, keep, |_| true)
    }

    /// Most general pick: filter objects, and filter hit points in world space
    /// (used to see through the part removed by a section plane).
    pub fn pick_filtered(
        &self,
        origin: Vec3,
        dir: Vec3,
        keep: impl Fn(&Object) -> bool,
        accept_point: impl Fn(Vec3) -> bool,
    ) -> Option<PickHit> {
        let mut best: Option<(f32, PickHit)> = None;
        for o in self.objects.iter().filter(|o| o.visible && keep(o)) {
            let m = o.matrix();
            let inv = m.inverse();
            let lo = inv.transform_point3(origin);
            let ld = inv.transform_vector3(dir);
            if let Some((t, tri)) = o.mesh.ray_cast_where(lo, ld, |lp| accept_point(m.transform_point3(lp))) {
                let point = m.transform_point3(lo + ld * t);
                let dist = (point - origin).length();
                if best.as_ref().is_none_or(|b| dist < b.0) {
                    let mut normal = inv.transpose().transform_vector3(o.mesh.mesh.face_normal(tri)).normalize_or_zero();
                    if m.determinant() < 0.0 {
                        normal = -normal;
                    }
                    let corners = o.mesh.mesh.tri(tri).map(|p| m.transform_point3(p));
                    best = Some((dist, PickHit { id: o.id, point, normal, corners, distance: dist, tri }));
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
    fn groups() {
        let mut s = Scene::default();
        let a = s.add_world_mesh("a", cuboid(Vec3::ONE), None);
        let b = s.add_world_mesh("b", cuboid(Vec3::ONE), None);
        let c = s.add_world_mesh("c", cuboid(Vec3::ONE), None);
        let g = s.group(&[a, b], "pair");
        assert_eq!(s.with_group(a), vec![a, b]);
        assert_eq!(s.with_group(c), vec![c]);
        s.remove(b); // a group of one dissolves
        assert!(s.group_of(a).is_none() && s.group_name(g).is_none());
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
