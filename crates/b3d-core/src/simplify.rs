//! Mesh decimation by quadric-error edge collapse (Garland & Heckbert '97).
//!
//! * keeps the mesh 2-manifold (link condition) and never flips faces,
//! * preserves open boundaries with perpendicular constraint planes,
//! * stops at a target triangle count and/or a maximum geometric error (mm).

use crate::mesh::Mesh;
use glam::DVec3;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Debug, Clone, Copy)]
pub struct SimplifyOptions {
    /// Stop when this many triangles remain.
    pub target_triangles: usize,
    /// Never collapse an edge whose error exceeds this distance (mm). 0 = unlimited.
    pub max_error: f64,
}

#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]); // a² ab ac ad b² bc bd c² cd d²

impl Quadric {
    fn plane(n: DVec3, d: f64, w: f64) -> Self {
        let (a, b, c) = (n.x, n.y, n.z);
        Quadric([a * a * w, a * b * w, a * c * w, a * d * w, b * b * w, b * c * w, b * d * w, c * c * w, c * d * w, d * d * w])
    }
    fn add(&self, o: &Quadric) -> Quadric {
        let mut r = *self;
        for i in 0..10 {
            r.0[i] += o.0[i];
        }
        r
    }
    fn eval(&self, p: DVec3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p.x, p.y, p.z);
        (q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9])
            .max(0.0)
    }
    /// Minimiser of the quadric, if the 3×3 system is well conditioned.
    fn optimal(&self) -> Option<DVec3> {
        let q = &self.0;
        let m = glam::DMat3::from_cols(DVec3::new(q[0], q[1], q[2]), DVec3::new(q[1], q[4], q[5]), DVec3::new(q[2], q[5], q[7]));
        let det = m.determinant();
        let scale = (q[0] + q[4] + q[7]).powi(3).max(1e-30);
        if det.abs() < 1e-9 * scale {
            return None;
        }
        Some(m.inverse() * -DVec3::new(q[3], q[6], q[8]))
    }
}

#[derive(PartialEq)]
struct Candidate {
    cost: f64,
    u: u32,
    v: u32,
    ver_u: u32,
    ver_v: u32,
    pos: DVec3,
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost) // min-heap
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

struct State {
    pos: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    tri_alive: Vec<bool>,
    vtris: Vec<Vec<u32>>,
    alive: Vec<bool>,
    version: Vec<u32>,
    boundary: Vec<bool>,
    q: Vec<Quadric>,
}

impl State {
    fn tri_normal(&self, t: [u32; 3], replace: Option<(u32, u32, DVec3)>) -> DVec3 {
        let p = |i: u32| match replace {
            Some((a, b, np)) if i == a || i == b => np,
            _ => self.pos[i as usize],
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        (b - a).cross(c - a)
    }

    fn neighbors(&self, v: u32) -> Vec<u32> {
        let mut n: Vec<u32> = self.vtris[v as usize]
            .iter()
            .filter(|&&t| self.tri_alive[t as usize])
            .flat_map(|&t| self.tris[t as usize])
            .filter(|&w| w != v)
            .collect();
        n.sort_unstable();
        n.dedup();
        n
    }

    fn candidate(&self, u: u32, v: u32) -> Option<Candidate> {
        let (bu, bv) = (self.boundary[u as usize], self.boundary[v as usize]);
        let q = self.q[u as usize].add(&self.q[v as usize]);
        let (pu, pv) = (self.pos[u as usize], self.pos[v as usize]);
        let mid = (pu + pv) * 0.5;
        let mut opts = vec![pu, pv, mid];
        // keep boundary vertices on the boundary
        if bu && !bv {
            opts = vec![pu];
        } else if bv && !bu {
            opts = vec![pv];
        } else if let Some(o) = q.optimal() {
            // reject far-away optima (ill-conditioned flat regions)
            if o.distance(mid) <= pu.distance(pv) * 2.0 {
                opts.push(o);
            }
        }
        let (pos, cost) = opts.into_iter().map(|p| (p, q.eval(p))).min_by(|a, b| a.1.total_cmp(&b.1))?;
        Some(Candidate { cost, u, v, ver_u: self.version[u as usize], ver_v: self.version[v as usize], pos })
    }

    /// Attempt to collapse v into u at `p`. Returns removed triangle count, or None if rejected.
    fn collapse(&mut self, u: u32, v: u32, p: DVec3) -> Option<usize> {
        let shared: Vec<u32> =
            self.vtris[u as usize].iter().copied().filter(|&t| self.tri_alive[t as usize] && self.tris[t as usize].contains(&v)).collect();
        if shared.is_empty() {
            return None;
        }
        // an interior vertex may not be pulled onto a non-boundary edge between two boundary vertices
        if self.boundary[u as usize] && self.boundary[v as usize] && shared.len() != 1 {
            return None;
        }
        // link condition: common neighbours must be exactly the opposite vertices of shared faces
        let nu = self.neighbors(u);
        let nv = self.neighbors(v);
        let common = nu.iter().filter(|w| nv.binary_search(w).is_ok()).count();
        if common != shared.len() {
            return None;
        }
        // don't shrink below a tetrahedron-ish neighbourhood
        if nu.len() + nv.len() < 5 {
            return None;
        }
        // flip / degeneracy test on the faces that survive
        for &vert in &[u, v] {
            for &t in &self.vtris[vert as usize] {
                if !self.tri_alive[t as usize] || shared.contains(&t) {
                    continue;
                }
                let tri = self.tris[t as usize];
                let before = self.tri_normal(tri, None);
                let after = self.tri_normal(tri, Some((u, v, p)));
                let (lb, la) = (before.length(), after.length());
                if lb > 1e-14 && (la < 1e-14 || before.dot(after) < 0.2 * lb * la) {
                    return None;
                }
            }
        }
        // apply
        for &t in &shared {
            self.tri_alive[t as usize] = false;
        }
        let vt = std::mem::take(&mut self.vtris[v as usize]);
        for t in vt {
            if !self.tri_alive[t as usize] {
                continue;
            }
            for i in self.tris[t as usize].iter_mut() {
                if *i == v {
                    *i = u;
                }
            }
            self.vtris[u as usize].push(t);
        }
        let alive = &self.tri_alive;
        self.vtris[u as usize].retain(|&t| alive[t as usize]);
        self.pos[u as usize] = p;
        self.q[u as usize] = self.q[u as usize].add(&self.q[v as usize]);
        self.boundary[u as usize] |= self.boundary[v as usize];
        self.alive[v as usize] = false;
        self.version[u as usize] += 1;
        self.version[v as usize] += 1;
        Some(shared.len())
    }
}

/// Decimate in place. Returns the number of triangles removed.
pub fn simplify(m: &mut Mesh, opts: SimplifyOptions) -> usize {
    simplify_with(m, opts, &crate::Progress::new()).unwrap_or(0)
}

/// Like [`simplify`], reporting progress; on cancellation the mesh is left unchanged.
pub fn simplify_with(m: &mut Mesh, opts: SimplifyOptions, progress: &crate::Progress) -> crate::Result<usize> {
    let nv = m.positions.len();
    let nt = m.triangles.len();
    if nt <= opts.target_triangles || nt < 8 {
        return Ok(0);
    }
    let mut st = State {
        pos: m.positions.iter().map(|p| p.as_dvec3()).collect(),
        tris: m.triangles.clone(),
        tri_alive: vec![true; nt],
        vtris: vec![vec![]; nv],
        alive: vec![true; nv],
        version: vec![0; nv],
        boundary: vec![false; nv],
        q: vec![Quadric::default(); nv],
    };
    for (i, t) in st.tris.iter().enumerate() {
        for &v in t {
            st.vtris[v as usize].push(i as u32);
        }
    }
    // face quadrics + edge bookkeeping
    let mut edges: HashMap<(u32, u32), (u32, u8)> = HashMap::with_capacity(nt * 3 / 2);
    for (i, t) in st.tris.iter().enumerate() {
        let n = st.tri_normal(*t, None);
        if let Some(nn) = n.try_normalize() {
            let d = -nn.dot(st.pos[t[0] as usize]);
            let q = Quadric::plane(nn, d, 1.0);
            for &v in t {
                st.q[v as usize] = st.q[v as usize].add(&q);
            }
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let e = edges.entry((a.min(b), a.max(b))).or_insert((i as u32, 0));
            e.1 += 1;
        }
    }
    // boundary constraint planes
    for (&(a, b), &(t, count)) in &edges {
        if count != 1 {
            continue;
        }
        st.boundary[a as usize] = true;
        st.boundary[b as usize] = true;
        let fnrm = st.tri_normal(st.tris[t as usize], None).normalize_or_zero();
        let (pa, pb) = (st.pos[a as usize], st.pos[b as usize]);
        if let Some(n) = (pb - pa).cross(fnrm).try_normalize() {
            let q = Quadric::plane(n, -n.dot(pa), 10.0);
            st.q[a as usize] = st.q[a as usize].add(&q);
            st.q[b as usize] = st.q[b as usize].add(&q);
        }
    }
    let mut heap = BinaryHeap::with_capacity(edges.len());
    for &(a, b) in edges.keys() {
        if let Some(c) = st.candidate(a, b) {
            heap.push(c);
        }
    }
    drop(edges);

    let max_cost = if opts.max_error > 0.0 { opts.max_error * opts.max_error } else { f64::INFINITY };
    let mut remaining = nt;
    let goal = (nt - opts.target_triangles).max(1) as f32;
    let mut iter = 0u32;
    while remaining > opts.target_triangles {
        iter = iter.wrapping_add(1);
        if iter % 4096 == 0 {
            progress.check()?;
            progress.set((nt - remaining) as f32 / goal);
        }
        let Some(c) = heap.pop() else { break };
        if c.cost > max_cost {
            break;
        }
        let (u, v) = (c.u as usize, c.v as usize);
        if !st.alive[u] || !st.alive[v] || st.version[u] != c.ver_u || st.version[v] != c.ver_v {
            continue;
        }
        if let Some(removed) = st.collapse(c.u, c.v, c.pos) {
            remaining -= removed;
            for w in st.neighbors(c.u) {
                if let Some(nc) = st.candidate(c.u, w) {
                    heap.push(nc);
                }
            }
        }
    }

    m.positions = st.pos.iter().map(|p| p.as_vec3()).collect();
    m.triangles = st.tris.iter().zip(&st.tri_alive).filter(|(_, a)| **a).map(|(t, _)| *t).collect();
    m.compact();
    progress.set(1.0);
    Ok(nt - m.triangles.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::{cuboid, sphere};
    use glam::Vec3;

    #[test]
    fn sphere_halves_and_stays_closed() {
        let mut m = sphere(10.0, 128, 64);
        let n0 = m.triangles.len();
        let v0 = m.volume();
        simplify(&mut m, SimplifyOptions { target_triangles: n0 / 10, max_error: 0.0 });
        let r = analyze(&m);
        assert!(m.triangles.len() <= n0 / 10 + 2, "{}", m.triangles.len());
        assert!(r.is_watertight(), "{r:?}");
        assert!((m.volume() - v0).abs() / v0 < 0.02, "{} vs {v0}", m.volume());
    }

    #[test]
    fn flat_tessellation_collapses_to_few_triangles() {
        // a cube with heavily subdivided faces must collapse back to ~12 triangles without changing shape
        let mut m = crate::smooth::subdivide_flat(&cuboid(Vec3::splat(10.0)), 3);
        assert!(m.triangles.len() > 700);
        simplify(&mut m, SimplifyOptions { target_triangles: 0, max_error: 1e-4 });
        assert!(m.triangles.len() <= 16, "{}", m.triangles.len());
        assert!((m.volume() - 1000.0).abs() < 1e-3);
        assert!(analyze(&m).is_watertight());
    }

    #[test]
    fn error_bound_is_respected() {
        let mut m = sphere(10.0, 64, 32);
        let n0 = m.triangles.len();
        simplify(&mut m, SimplifyOptions { target_triangles: 0, max_error: 0.05 });
        assert!(m.triangles.len() < n0 && m.triangles.len() > 100, "{}", m.triangles.len());
        for p in &m.positions {
            assert!((p.length() - 10.0).abs() < 0.2);
        }
    }

    #[test]
    fn open_mesh_boundary_kept() {
        let mut m = sphere(10.0, 64, 32);
        m.triangles.retain(|t| (0..3).all(|k| m.positions[t[k] as usize].z > -0.01)); // hemisphere
        m.compact();
        let before = analyze(&m).boundary_edges;
        assert!(before > 0);
        simplify(&mut m, SimplifyOptions { target_triangles: 200, max_error: 0.0 });
        let r = analyze(&m);
        assert_eq!(r.non_manifold_edges, 0);
        assert_eq!(r.holes, 1);
        for p in &m.positions {
            assert!(p.z > -0.05, "boundary drifted: {p}");
        }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn bench_1m() {
        let mut m = crate::primitives::sphere(10.0, 1024, 512);
        let t = std::time::Instant::now();
        let n0 = m.triangles.len();
        super::simplify(&mut m, super::SimplifyOptions { target_triangles: n0 / 20, max_error: 0.0 });
        eprintln!("BENCH {} -> {} tris in {:?}", n0, m.triangles.len(), t.elapsed());
    }
}
