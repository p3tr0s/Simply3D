//! Mesh diagnostics: watertightness, holes, non-manifold edges, orientation.

use crate::mesh::{Aabb, Mesh};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct MeshReport {
    pub vertices: usize,
    pub triangles: usize,
    /// Edges used by exactly one triangle.
    pub boundary_edges: usize,
    /// Edges used by more than two triangles.
    pub non_manifold_edges: usize,
    /// Edges shared by two triangles that traverse it in the same direction
    /// (i.e. neighbouring faces with inconsistent winding).
    pub flipped_edges: usize,
    /// Closed loops of boundary edges.
    pub holes: usize,
    pub degenerate_triangles: usize,
    pub components: usize,
    pub volume_mm3: f64,
    pub area_mm2: f64,
    pub bounds: Option<Aabb>,
}

impl MeshReport {
    pub fn is_watertight(&self) -> bool {
        self.boundary_edges == 0 && self.non_manifold_edges == 0 && self.flipped_edges == 0
    }
    pub fn is_printable(&self) -> bool {
        self.is_watertight() && self.degenerate_triangles == 0 && self.volume_mm3 > 0.0
    }
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.holes > 0 {
            v.push(format!("{} hole(s) ({} open edges)", self.holes, self.boundary_edges));
        } else if self.boundary_edges > 0 {
            v.push(format!("{} open edges", self.boundary_edges));
        }
        if self.non_manifold_edges > 0 {
            v.push(format!("{} non-manifold edges", self.non_manifold_edges));
        }
        if self.flipped_edges > 0 {
            v.push(format!("{} edges with inconsistent winding", self.flipped_edges));
        }
        if self.degenerate_triangles > 0 {
            v.push(format!("{} degenerate triangles", self.degenerate_triangles));
        }
        if self.is_watertight() && self.volume_mm3 < 0.0 {
            v.push("mesh is inside-out".into());
        }
        v
    }
}

/// Undirected edge key (lo, hi) -> list of (triangle, directed lo->hi?)
pub(crate) fn edge_map(m: &Mesh) -> HashMap<(u32, u32), Vec<(usize, bool)>> {
    let mut map: HashMap<(u32, u32), Vec<(usize, bool)>> = HashMap::with_capacity(m.triangles.len() * 3 / 2);
    for (ti, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if a == b {
                continue;
            }
            let key = (a.min(b), a.max(b));
            map.entry(key).or_default().push((ti, a < b));
        }
    }
    map
}

/// Directed boundary edges (a -> b as they appear in their single triangle).
pub(crate) fn boundary_edges(m: &Mesh) -> Vec<(u32, u32)> {
    edge_map(m).into_iter().filter(|(_, v)| v.len() == 1).map(|((lo, hi), v)| if v[0].1 { (lo, hi) } else { (hi, lo) }).collect()
}

/// Group directed boundary edges into closed loops.
pub fn boundary_loops(m: &Mesh) -> Vec<Vec<u32>> {
    let edges = boundary_edges(m);
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for (a, b) in &edges {
        next.entry(*a).or_default().push(*b);
    }
    let mut loops = Vec::new();
    let mut starts: Vec<u32> = edges.iter().map(|e| e.0).collect();
    starts.sort_unstable();
    for s in starts {
        while next.get(&s).is_some_and(|v| !v.is_empty()) {
            let mut lp = vec![s];
            let mut cur = s;
            loop {
                let Some(n) = next.get_mut(&cur).and_then(|v| v.pop()) else { break };
                if n == s {
                    break;
                }
                lp.push(n);
                cur = n;
                if lp.len() > edges.len() {
                    break;
                }
            }
            loops.push(lp);
        }
    }
    loops
}

pub fn analyze(m: &Mesh) -> MeshReport {
    let em = edge_map(m);
    let mut r = MeshReport { vertices: m.positions.len(), triangles: m.triangles.len(), ..Default::default() };
    for uses in em.values() {
        match uses.len() {
            1 => r.boundary_edges += 1,
            2 => {
                if uses[0].1 == uses[1].1 {
                    r.flipped_edges += 1
                }
            }
            _ => r.non_manifold_edges += 1,
        }
    }
    r.holes = if r.boundary_edges > 0 { boundary_loops(m).len() } else { 0 };
    r.degenerate_triangles = (0..m.triangles.len())
        .filter(|&i| {
            let t = m.triangles[i];
            if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                return true;
            }
            let [a, b, c] = m.tri(i);
            (b - a).cross(c - a).length_squared() < 1e-14
        })
        .count();
    r.components = if m.is_empty() { 0 } else { m.connected_components().len() };
    r.volume_mm3 = m.volume();
    r.area_mm2 = m.surface_area();
    r.bounds = (!m.positions.is_empty()).then(|| m.bounds());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cuboid;
    use glam::Vec3;

    #[test]
    fn detects_hole() {
        let mut m = cuboid(Vec3::ONE);
        m.triangles.truncate(10); // remove one side (2 tris)
        let r = analyze(&m);
        assert_eq!(r.holes, 1);
        assert_eq!(r.boundary_edges, 4);
        assert!(!r.is_watertight());
    }

    #[test]
    fn detects_flip() {
        let mut m = cuboid(Vec3::ONE);
        m.triangles[0].swap(1, 2);
        let r = analyze(&m);
        assert!(r.flipped_edges > 0);
    }
}
