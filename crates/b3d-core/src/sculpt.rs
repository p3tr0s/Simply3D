//! Sculpting: brushes that push, pull, smooth and flatten the surface, with detail added
//! under the brush on the fly so even low-poly models can be sculpted.

use crate::bvh::Bvh;
use crate::mesh::Mesh;
use crate::paint::refine_where;
use glam::Vec3;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brush {
    /// Push the surface out (in, inverted) as one piece.
    Draw,
    /// Puff the surface out along its own normals (deflate, inverted).
    Inflate,
    /// Even out bumps.
    Smooth,
    /// Press the area flat.
    Flatten,
    /// Pull a patch along with the pointer.
    Grab,
}

/// A mesh being sculpted, with the adjacency the brushes need.
pub struct Sculpt {
    pub mesh: Mesh,
    nbrs: Vec<Vec<u32>>,
    vfaces: Vec<Vec<u32>>,
    /// Picking index, kept in step with `mesh` (shared with the scene's copy).
    pub bvh: Arc<Bvh>,
    /// Scratch: last query each vertex was seen in.
    stamp: Vec<u32>,
    query: u32,
    /// Vertices moved since this was last emptied (for updating the display in place).
    pub moved: Vec<u32>,
    /// Set when detail was added (the triangles changed, not just the positions).
    pub topology_changed: bool,
}

/// Smooth falloff: 1 at the centre, 0 at the rim.
fn falloff(d: f32, r: f32) -> f32 {
    let t = (1.0 - (d / r).powi(2)).max(0.0);
    t * t
}

impl Sculpt {
    pub fn new(mesh: Mesh) -> Sculpt {
        Self::with_bvh(mesh, None)
    }

    /// Start from a mesh whose picking index is already built.
    pub fn with_bvh(mesh: Mesh, bvh: Option<Arc<Bvh>>) -> Sculpt {
        let bvh = bvh.filter(|b| b.len() == mesh.triangles.len()).unwrap_or_else(|| Arc::new(Bvh::build(&mesh)));
        let mut s = Sculpt { mesh, nbrs: vec![], vfaces: vec![], bvh, stamp: vec![], query: 0, moved: vec![], topology_changed: false };
        s.rebuild_adjacency();
        s
    }

    fn rebuild(&mut self) {
        self.topology_changed = true;
        self.moved.clear();
        self.bvh = Arc::new(Bvh::build(&self.mesh));
        self.rebuild_adjacency();
    }

    fn rebuild_adjacency(&mut self) {
        self.stamp = vec![0; self.mesh.positions.len()];
        self.query = 0;
        let n = self.mesh.positions.len();
        self.nbrs = vec![vec![]; n];
        self.vfaces = vec![vec![]; n];
        for (i, t) in self.mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                self.vfaces[a as usize].push(i as u32);
                if !self.nbrs[a as usize].contains(&b) {
                    self.nbrs[a as usize].push(b);
                }
                if !self.nbrs[b as usize].contains(&a) {
                    self.nbrs[b as usize].push(a);
                }
            }
        }
    }

    fn vnormal(&self, v: usize) -> Vec3 {
        let mut n = Vec3::ZERO;
        for &f in &self.vfaces[v] {
            let [a, b, c] = self.mesh.tri(f as usize);
            n += (b - a).cross(c - a);
        }
        n.normalize_or_zero()
    }

    fn within(&mut self, c: Vec3, r: f32) -> Vec<(usize, f32)> {
        let r2 = r * r;
        let mut tris = vec![];
        self.bvh.tris_near(c, r, &mut tris);
        self.query = self.query.wrapping_add(1);
        if self.query == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.query = 1;
        }
        let mut out = vec![];
        for t in tris {
            for &v in &self.mesh.triangles[t] {
                let v = v as usize;
                if self.stamp[v] == self.query {
                    continue;
                }
                self.stamp[v] = self.query;
                let d2 = self.mesh.positions[v].distance_squared(c);
                if d2 < r2 {
                    out.push((v, falloff(d2.sqrt(), r)));
                }
            }
        }
        out
    }

    /// Bring the picking index up to date after these vertices moved.
    fn refit(&mut self, moved: impl Iterator<Item = usize>) {
        let mut tris: Vec<usize> = vec![];
        for v in moved {
            self.moved.push(v as u32);
            tris.extend(self.vfaces[v].iter().map(|&f| f as usize));
        }
        tris.sort_unstable();
        tris.dedup();
        Arc::make_mut(&mut self.bvh).refit(&self.mesh, &tris);
    }

    /// Split triangles near `c` that are too big for a brush of radius `r`. Returns true
    /// if the mesh changed shape (topology).
    pub fn ensure_detail(&mut self, c: Vec3, r: f32, budget: usize) -> bool {
        let max_edge = r * 0.3;
        let reach = r * 1.5;
        let mut near_tris = vec![];
        self.bvh.tris_near(c, reach, &mut near_tris);
        let mut any = false;
        let mut sel = vec![false; self.mesh.triangles.len()];
        for i in near_tris {
            let [a, b, cc] = self.mesh.tri(i);
            let near = crate::paint::closest_on_triangle(c, a, b, cc).distance(c) < reach;
            if near {
                sel[i] = true;
                any |= a.distance(b) > max_edge || b.distance(cc) > max_edge || cc.distance(a) > max_edge;
            }
        }
        if !any || self.mesh.triangles.len() >= budget {
            return false;
        }
        let (m, _) = refine_where(&self.mesh, &sel, max_edge, budget);
        self.mesh = m;
        self.rebuild();
        true
    }

    /// One dab of `brush` at `c` (on the surface, normal `n`), radius `r`, strength 0…1.
    pub fn dab(&mut self, brush: Brush, c: Vec3, n: Vec3, r: f32, strength: f32, invert: bool) {
        let hits = self.within(c, r);
        if hits.is_empty() {
            return;
        }
        let sign = if invert { -1.0 } else { 1.0 };
        let step = r * 0.12 * strength;
        // area normal and centre of what's under the brush
        let mut an = Vec3::ZERO;
        let mut centre = Vec3::ZERO;
        let mut wsum = 0.0;
        for &(v, w) in &hits {
            an += self.vnormal(v) * w;
            centre += self.mesh.positions[v] * w;
            wsum += w;
        }
        let an = an.normalize_or(n);
        let centre = centre / wsum.max(1e-9);
        // work out every new position from the current ones, then apply them
        let old = &self.mesh.positions;
        let mut moved: Vec<(usize, Vec3)> = Vec::with_capacity(hits.len());
        for &(v, w) in &hits {
            let p = old[v];
            let np = match brush {
                Brush::Draw => p + an * (step * w * sign),
                Brush::Inflate => p + self.vnormal(v) * (step * w * sign),
                Brush::Smooth => {
                    let nb = &self.nbrs[v];
                    if nb.is_empty() {
                        p
                    } else {
                        let avg = nb.iter().map(|&q| old[q as usize]).sum::<Vec3>() / nb.len() as f32;
                        p.lerp(avg, (0.5 * strength * w).min(1.0))
                    }
                }
                Brush::Flatten => {
                    let off = (p - centre).dot(an);
                    p - an * (off * (0.4 * strength * w).min(1.0))
                }
                Brush::Grab => p,
            };
            moved.push((v, np));
        }
        for &(v, np) in &moved {
            self.mesh.positions[v] = np;
        }
        self.refit(hits.iter().map(|h| h.0));
    }

    /// Start a grab: the vertices within `r` of `c` and how much each follows.
    pub fn grab_begin(&mut self, c: Vec3, r: f32) -> (Vec<(usize, f32)>, Vec<Vec3>) {
        let hits = self.within(c, r);
        let base = hits.iter().map(|&(v, _)| self.mesh.positions[v]).collect();
        (hits, base)
    }

    /// Move the grabbed vertices by `delta` (weighted) from where they started.
    pub fn grab_move(&mut self, hits: &[(usize, f32)], base: &[Vec3], delta: Vec3) {
        for (&(v, w), b) in hits.iter().zip(base) {
            if v < self.mesh.positions.len() {
                self.mesh.positions[v] = *b + delta * w;
            }
        }
        let n = self.vfaces.len();
        self.refit(hits.iter().map(|h| h.0).filter(move |&v| v < n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn brushes_keep_the_mesh_closed() {
        let top = glam::Vec3::new(0.0, 0.0, 10.0);
        let mut s = Sculpt::new(cuboid(Vec3::splat(20.0)));
        assert!(s.ensure_detail(top, 4.0, 1_000_000));
        let before = s.mesh.volume();
        for _ in 0..10 {
            s.dab(Brush::Draw, top, Vec3::Z, 4.0, 1.0, false);
        }
        assert!(s.mesh.volume() > before + 1.0);
        assert!(s.mesh.bounds().max.z > 10.5);
        let peak = s.mesh.bounds().max.z;
        for _ in 0..20 {
            s.dab(Brush::Smooth, top, Vec3::Z, 4.0, 1.0, false);
        }
        assert!(s.mesh.bounds().max.z < peak);
        for _ in 0..30 {
            s.dab(Brush::Flatten, top, Vec3::Z, 6.0, 1.0, false);
        }
        let (hits, base) = s.grab_begin(top, 4.0);
        s.grab_move(&hits, &base, Vec3::Z * 3.0);
        assert!(analyze(&s.mesh).is_watertight());
        assert!(s.mesh.bounds().max.z > 12.0);
        // the picking index followed every change
        let o = Vec3::new(0.3, 0.2, 100.0);
        let a = s.mesh.ray_cast(o, -Vec3::Z).unwrap();
        let b = s.bvh.ray_cast_where(&s.mesh, o, -Vec3::Z, |_| true).unwrap();
        assert!((a.0 - b.0).abs() < 1e-4);
    }
}
