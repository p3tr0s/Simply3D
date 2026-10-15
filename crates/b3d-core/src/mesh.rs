use glam::{Mat4, Vec3};

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    pub fn from_points<'a>(pts: impl IntoIterator<Item = &'a Vec3>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in pts {
            b.grow(*p);
        }
        b
    }
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            self.max - self.min
        }
    }
    pub fn center(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            (self.min + self.max) * 0.5
        }
    }
    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(b.x, b.y, b.z),
            Vec3::new(a.x, b.y, b.z),
        ]
    }
    pub fn transformed(&self, m: &Mat4) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        Aabb::from_points(self.corners().iter().map(|c| m.transform_point3(*c)).collect::<Vec<_>>().iter())
    }
    /// Slab test. Returns entry distance along the ray.
    pub fn ray_hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let inv = dir.recip();
        let t1 = (self.min - origin) * inv;
        let t2 = (self.max - origin) * inv;
        let tmin = t1.min(t2).max_element();
        let tmax = t1.max(t2).min_element();
        (tmax >= tmin.max(0.0)).then_some(tmin.max(0.0))
    }
}

/// Indexed triangle mesh. Counter-clockwise winding = outward facing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn new(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Self {
        Self { positions, triangles }
    }

    /// Build from a triangle soup (3 positions per triangle). Vertices are welded exactly.
    pub fn from_soup(tris: &[[Vec3; 3]]) -> Self {
        let mut m = Mesh::default();
        let mut map: std::collections::HashMap<[u32; 3], u32> = std::collections::HashMap::new();
        for t in tris {
            let mut idx = [0u32; 3];
            for (k, p) in t.iter().enumerate() {
                // canonicalise -0.0 so it welds with 0.0
                let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
                idx[k] = *map.entry(key).or_insert_with(|| {
                    m.positions.push(*p);
                    (m.positions.len() - 1) as u32
                });
            }
            m.triangles.push(idx);
        }
        m
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
    pub fn tri(&self, i: usize) -> [Vec3; 3] {
        let t = self.triangles[i];
        [self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]]
    }
    pub fn face_normal(&self, i: usize) -> Vec3 {
        let [a, b, c] = self.tri(i);
        (b - a).cross(c - a).normalize_or_zero()
    }
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.positions.iter())
    }
    /// Signed volume (positive for closed, outward-oriented meshes).
    pub fn volume(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let a = self.positions[t[0] as usize].as_dvec3();
                let b = self.positions[t[1] as usize].as_dvec3();
                let c = self.positions[t[2] as usize].as_dvec3();
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }
    pub fn surface_area(&self) -> f64 {
        (0..self.triangles.len())
            .map(|i| {
                let [a, b, c] = self.tri(i);
                ((b - a).as_dvec3().cross((c - a).as_dvec3())).length() * 0.5
            })
            .sum()
    }
    pub fn transformed(&self, m: &Mat4) -> Mesh {
        let mut out = self.clone();
        out.transform(m);
        out
    }
    /// Apply a matrix to all vertices. Flips winding if the matrix mirrors.
    pub fn transform(&mut self, m: &Mat4) {
        for p in &mut self.positions {
            *p = m.transform_point3(*p);
        }
        if m.determinant() < 0.0 {
            self.flip();
        }
    }
    pub fn translate(&mut self, d: Vec3) {
        for p in &mut self.positions {
            *p += d;
        }
    }
    pub fn flip(&mut self) {
        for t in &mut self.triangles {
            t.swap(1, 2);
        }
    }
    pub fn append(&mut self, other: &Mesh) {
        let off = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.triangles.extend(other.triangles.iter().map(|t| [t[0] + off, t[1] + off, t[2] + off]));
    }
    /// Drop vertices not referenced by any triangle.
    pub fn compact(&mut self) {
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut pos = Vec::with_capacity(self.positions.len());
        for t in &mut self.triangles {
            for v in t.iter_mut() {
                let r = &mut remap[*v as usize];
                if *r == u32::MAX {
                    *r = pos.len() as u32;
                    pos.push(self.positions[*v as usize]);
                }
                *v = *r;
            }
        }
        self.positions = pos;
    }

    /// Möller–Trumbore ray cast. Returns (distance, triangle index).
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3) -> Option<(f32, usize)> {
        self.ray_cast_where(origin, dir, |_| true)
    }

    /// Nearest hit whose (local) hit point passes `accept` — e.g. to skip a clipped region.
    pub fn ray_cast_where(&self, origin: Vec3, dir: Vec3, accept: impl Fn(Vec3) -> bool) -> Option<(f32, usize)> {
        if self.bounds().ray_hit(origin, dir).is_none() {
            return None;
        }
        let mut best: Option<(f32, usize)> = None;
        for (i, _) in self.triangles.iter().enumerate() {
            let [a, b, c] = self.tri(i);
            let e1 = b - a;
            let e2 = c - a;
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let s = origin - a;
            let u = s.dot(p) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = dir.dot(q) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let t = e2.dot(q) * inv;
            if t > 1e-6 && best.map_or(true, |(bt, _)| t < bt) && accept(origin + dir * t) {
                best = Some((t, i));
            }
        }
        best
    }

    /// Split into connected components (by shared vertex indices).
    pub fn connected_components(&self) -> Vec<Mesh> {
        let n = self.positions.len();
        let mut parent: Vec<u32> = (0..n as u32).collect();
        fn find(p: &mut [u32], mut x: u32) -> u32 {
            while p[x as usize] != x {
                p[x as usize] = p[p[x as usize] as usize];
                x = p[x as usize];
            }
            x
        }
        for t in &self.triangles {
            let a = find(&mut parent, t[0]);
            for &v in &t[1..] {
                let b = find(&mut parent, v);
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
        let mut groups: std::collections::BTreeMap<u32, Vec<[u32; 3]>> = Default::default();
        for t in &self.triangles {
            let r = find(&mut parent, t[0]);
            groups.entry(r).or_default().push(*t);
        }
        groups
            .into_values()
            .map(|tris| {
                let mut m = Mesh { positions: self.positions.clone(), triangles: tris };
                m.compact();
                m
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives;

    #[test]
    fn cube_volume_and_area() {
        let m = primitives::cuboid(Vec3::new(10.0, 20.0, 30.0));
        assert!((m.volume() - 6000.0).abs() < 1e-6);
        assert!((m.surface_area() - 2200.0).abs() < 1e-6);
    }

    #[test]
    fn ray_cast_hits_cube() {
        let m = primitives::cuboid(Vec3::splat(2.0));
        let (t, _) = m.ray_cast(Vec3::new(0.0, 0.0, 10.0), -Vec3::Z).unwrap();
        assert!((t - 9.0).abs() < 1e-5);
    }

    #[test]
    fn mirror_keeps_volume_positive() {
        let mut m = primitives::cuboid(Vec3::splat(2.0));
        m.transform(&Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert!(m.volume() > 0.0);
    }

    #[test]
    fn components() {
        let mut m = primitives::cuboid(Vec3::splat(1.0));
        let mut b = primitives::cuboid(Vec3::splat(1.0));
        b.translate(Vec3::X * 5.0);
        m.append(&b);
        assert_eq!(m.connected_components().len(), 2);
    }
}
