//! Bounding-volume hierarchy over a mesh's triangles, so picking and brushes only look at
//! the few triangles near the pointer instead of all of them (millions on a scan).

use crate::mesh::{Aabb, Mesh};
use glam::Vec3;

const LEAF: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first index into `tris`. Inner: index of the left child (right = left + 1).
    first: u32,
    /// Leaf: number of triangles (> 0). Inner: 0.
    count: u32,
}

#[derive(Debug, Clone)]
pub struct Bvh {
    nodes: Vec<Node>,
    tris: Vec<u32>,
    /// Leaf node holding each triangle.
    leaf_of: Vec<u32>,
    parent: Vec<u32>,
}

fn tri_box(m: &Mesh, t: u32) -> (Vec3, Vec3) {
    let [a, b, c] = m.tri(t as usize);
    (a.min(b).min(c), a.max(b).max(c))
}

/// Squared distance from `p` to a box (0 inside).
fn box_dist2(min: Vec3, max: Vec3, p: Vec3) -> f32 {
    let d = (min - p).max(p - max).max(Vec3::ZERO);
    d.length_squared()
}

/// Ray / box slab test: entry distance, if the ray hits before `limit`.
fn ray_box(min: Vec3, max: Vec3, o: Vec3, inv: Vec3, limit: f32) -> Option<f32> {
    let t1 = (min - o) * inv;
    let t2 = (max - o) * inv;
    let tmin = t1.min(t2).max_element().max(0.0);
    let tmax = t1.max(t2).min_element();
    (tmax >= tmin && tmin <= limit).then_some(tmin)
}

/// Möller–Trumbore: distance along the ray, if it hits the triangle.
#[inline]
pub fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-6).then_some(t)
}

impl Bvh {
    pub fn build(m: &Mesh) -> Bvh {
        let n = m.triangles.len();
        let mut tris: Vec<u32> = (0..n as u32).collect();
        let cent: Vec<Vec3> = (0..n as u32)
            .map(|t| {
                let (lo, hi) = tri_box(m, t);
                (lo + hi) * 0.5
            })
            .collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(n / 2 + 1);
        let mut parent: Vec<u32> = Vec::with_capacity(n / 2 + 1);
        nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: n as u32 });
        parent.push(u32::MAX);
        // (node, start, end)
        let mut stack = vec![(0usize, 0usize, n)];
        while let Some((ni, s, e)) = stack.pop() {
            let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
            let (mut clo, mut chi) = (lo, hi);
            for &t in &tris[s..e] {
                let (a, b) = tri_box(m, t);
                lo = lo.min(a);
                hi = hi.max(b);
                clo = clo.min(cent[t as usize]);
                chi = chi.max(cent[t as usize]);
            }
            nodes[ni].min = lo;
            nodes[ni].max = hi;
            if e - s <= LEAF {
                nodes[ni].first = s as u32;
                nodes[ni].count = (e - s) as u32;
                continue;
            }
            let ext = chi - clo;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let mid = (s + e) / 2;
            tris[s..e].select_nth_unstable_by(mid - s, |a, b| cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis]));
            let left = nodes.len();
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            parent.push(ni as u32);
            parent.push(ni as u32);
            nodes[ni].first = left as u32;
            nodes[ni].count = 0;
            stack.push((left + 1, mid, e));
            stack.push((left, s, mid));
        }
        let mut leaf_of = vec![0u32; n];
        for (i, nd) in nodes.iter().enumerate() {
            if nd.count > 0 {
                for &t in &tris[nd.first as usize..(nd.first + nd.count) as usize] {
                    leaf_of[t as usize] = i as u32;
                }
            }
        }
        if n == 0 {
            nodes[0].min = Vec3::splat(f32::INFINITY);
            nodes[0].max = Vec3::splat(f32::NEG_INFINITY);
        }
        Bvh { nodes, tris, leaf_of, parent }
    }

    /// Number of triangles this was built for (it must match the mesh it's used with).
    pub fn len(&self) -> usize {
        self.leaf_of.len()
    }
    pub fn is_empty(&self) -> bool {
        self.leaf_of.is_empty()
    }

    pub fn bounds(&self) -> Aabb {
        Aabb { min: self.nodes[0].min, max: self.nodes[0].max }
    }

    /// Nearest hit whose point passes `accept`: (distance, triangle).
    pub fn ray_cast_where(&self, m: &Mesh, o: Vec3, d: Vec3, accept: impl Fn(Vec3) -> bool) -> Option<(f32, usize)> {
        if self.is_empty() {
            return None;
        }
        let inv = d.recip();
        let mut best: Option<(f32, usize)> = None;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(ni) = stack.pop() {
            let nd = self.nodes[ni as usize];
            let limit = best.map_or(f32::INFINITY, |b| b.0);
            if ray_box(nd.min, nd.max, o, inv, limit).is_none() {
                continue;
            }
            if nd.count > 0 {
                for &t in &self.tris[nd.first as usize..(nd.first + nd.count) as usize] {
                    let [a, b, c] = m.tri(t as usize);
                    if let Some(dist) = ray_triangle(o, d, a, b, c) {
                        if best.is_none_or(|b| dist < b.0) && accept(o + d * dist) {
                            best = Some((dist, t as usize));
                        }
                    }
                }
            } else {
                // visit the nearer child first
                let (l, r) = (nd.first, nd.first + 1);
                let dl = ray_box(self.nodes[l as usize].min, self.nodes[l as usize].max, o, inv, limit);
                let dr = ray_box(self.nodes[r as usize].min, self.nodes[r as usize].max, o, inv, limit);
                match (dl, dr) {
                    (Some(a), Some(b)) if a <= b => {
                        stack.push(r);
                        stack.push(l);
                    }
                    (Some(_), Some(_)) => {
                        stack.push(l);
                        stack.push(r);
                    }
                    (Some(_), None) => stack.push(l),
                    (None, Some(_)) => stack.push(r),
                    (None, None) => {}
                }
            }
        }
        best
    }

    /// Triangles whose bounding box comes within `r` of `c`.
    pub fn tris_near(&self, c: Vec3, r: f32, out: &mut Vec<usize>) {
        let r2 = r * r;
        self.visit(|min, max| box_dist2(min, max, c) <= r2, |t| out.push(t));
    }

    /// Triangles whose bounding box overlaps the box `lo`–`hi`.
    pub fn tris_in_box(&self, lo: Vec3, hi: Vec3, out: &mut Vec<usize>) {
        self.visit(|min, max| !(min.cmpgt(hi).any() || max.cmplt(lo).any()), |t| out.push(t));
    }

    fn visit(&self, keep: impl Fn(Vec3, Vec3) -> bool, mut f: impl FnMut(usize)) {
        if self.is_empty() {
            return;
        }
        let mut stack: Vec<u32> = vec![0];
        while let Some(ni) = stack.pop() {
            let nd = self.nodes[ni as usize];
            if !keep(nd.min, nd.max) {
                continue;
            }
            if nd.count > 0 {
                for &t in &self.tris[nd.first as usize..(nd.first + nd.count) as usize] {
                    f(t as usize);
                }
            } else {
                stack.push(nd.first);
                stack.push(nd.first + 1);
            }
        }
    }

    /// Update the boxes after the given triangles moved (same triangles, new positions).
    pub fn refit(&mut self, m: &Mesh, moved: &[usize]) {
        let mut dirty: Vec<u32> = Vec::with_capacity(moved.len() * 2);
        let mut seen = std::collections::HashSet::with_capacity(moved.len() * 2);
        for &t in moved {
            let Some(&leaf) = self.leaf_of.get(t) else { continue };
            let mut n = leaf;
            while n != u32::MAX && seen.insert(n) {
                dirty.push(n);
                n = self.parent[n as usize];
            }
        }
        // children always come after their parent, so refit from the highest index down
        dirty.sort_unstable_by(|a, b| b.cmp(a));
        for n in dirty {
            self.refit_node(m, n as usize);
        }
    }

    /// Update every box (all positions may have moved).
    pub fn refit_all(&mut self, m: &Mesh) {
        for n in (0..self.nodes.len()).rev() {
            self.refit_node(m, n);
        }
    }

    fn refit_node(&mut self, m: &Mesh, n: usize) {
        let nd = self.nodes[n];
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        if nd.count > 0 {
            for &t in &self.tris[nd.first as usize..(nd.first + nd.count) as usize] {
                let (a, b) = tri_box(m, t);
                lo = lo.min(a);
                hi = hi.max(b);
            }
        } else {
            for c in [nd.first, nd.first + 1] {
                lo = lo.min(self.nodes[c as usize].min);
                hi = hi.max(self.nodes[c as usize].max);
            }
        }
        self.nodes[n].min = lo;
        self.nodes[n].max = hi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sphere;

    #[test]
    fn same_hits_as_brute_force() {
        let mut m = sphere(20.0, 64, 32);
        let mut b = crate::primitives::cuboid(Vec3::splat(8.0));
        b.translate(Vec3::new(30.0, 0.0, 0.0));
        m.append(&b);
        let bvh = Bvh::build(&m);
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let mut hits = 0;
        for _ in 0..3000 {
            let o = Vec3::new(rnd(), rnd(), rnd()) * 80.0;
            let target = Vec3::new(rnd() * 40.0, rnd() * 25.0, rnd() * 25.0);
            let d = (target - o).normalize();
            let a = m.ray_cast(o, d);
            let b = bvh.ray_cast_where(&m, o, d, |_| true);
            match (a, b) {
                (Some(a), Some(b)) => {
                    assert!((a.0 - b.0).abs() < 1e-3, "{a:?} {b:?}");
                    hits += 1;
                }
                (None, None) => {}
                _ => panic!("{a:?} vs {b:?}"),
            }
        }
        assert!(hits > 500);
        // radius query finds everything brute force finds
        let c = Vec3::new(20.0, 0.0, 0.0);
        let mut near = vec![];
        bvh.tris_near(c, 3.0, &mut near);
        for i in 0..m.triangles.len() {
            let [a, b, cc] = m.tri(i);
            if box_dist2(a.min(b).min(cc), a.max(b).max(cc), c) <= 9.0 {
                assert!(near.contains(&i));
            }
        }
    }

    #[test]
    fn refit_follows_moved_vertices() {
        let mut m = sphere(10.0, 32, 16);
        let mut bvh = Bvh::build(&m);
        let top = m.positions.iter().enumerate().max_by(|a, b| a.1.z.total_cmp(&b.1.z)).unwrap().0;
        m.positions[top].z += 15.0;
        let moved: Vec<usize> = (0..m.triangles.len()).filter(|&i| m.triangles[i].contains(&(top as u32))).collect();
        bvh.refit(&m, &moved);
        let (t, _) = bvh.ray_cast_where(&m, Vec3::new(0.0, 0.0, 100.0), -Vec3::Z, |_| true).unwrap();
        let (tb, _) = m.ray_cast(Vec3::new(0.0, 0.0, 100.0), -Vec3::Z).unwrap();
        assert!((t - tb).abs() < 1e-3);
        assert!(bvh.bounds().max.z > 24.0);
    }
}
