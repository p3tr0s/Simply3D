//! Sculpting: brushes that push, pull, smooth and flatten the surface, with detail added
//! under the brush on the fly so even low-poly models can be sculpted.

use crate::mesh::Mesh;
use crate::paint::refine_where;
use glam::Vec3;

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
}

/// Smooth falloff: 1 at the centre, 0 at the rim.
fn falloff(d: f32, r: f32) -> f32 {
    let t = (1.0 - (d / r).powi(2)).max(0.0);
    t * t
}

impl Sculpt {
    pub fn new(mesh: Mesh) -> Sculpt {
        let mut s = Sculpt { mesh, nbrs: vec![], vfaces: vec![] };
        s.rebuild();
        s
    }

    fn rebuild(&mut self) {
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

    fn within(&self, c: Vec3, r: f32) -> Vec<(usize, f32)> {
        let r2 = r * r;
        self.mesh
            .positions
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let d2 = p.distance_squared(c);
                (d2 < r2).then(|| (i, falloff(d2.sqrt(), r)))
            })
            .collect()
    }

    /// Split triangles near `c` that are too big for a brush of radius `r`. Returns true
    /// if the mesh changed shape (topology).
    pub fn ensure_detail(&mut self, c: Vec3, r: f32, budget: usize) -> bool {
        let max_edge = r * 0.3;
        let reach = r * 1.5;
        let mut any = false;
        let sel: Vec<bool> = (0..self.mesh.triangles.len())
            .map(|i| {
                let [a, b, cc] = self.mesh.tri(i);
                let (lo, hi) = (a.min(b).min(cc) - Vec3::splat(reach), a.max(b).max(cc) + Vec3::splat(reach));
                if c.cmplt(lo).any() || c.cmpgt(hi).any() {
                    return false;
                }
                let near = crate::paint::closest_on_triangle(c, a, b, cc).distance(c) < reach;
                let big = near && (a.distance(b) > max_edge || b.distance(cc) > max_edge || cc.distance(a) > max_edge);
                any |= big;
                near
            })
            .collect();
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
        let old = self.mesh.positions.clone();
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
            self.mesh.positions[v] = np;
        }
    }

    /// Start a grab: the vertices within `r` of `c` and how much each follows.
    pub fn grab_begin(&self, c: Vec3, r: f32) -> (Vec<(usize, f32)>, Vec<Vec3>) {
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
    }
}
