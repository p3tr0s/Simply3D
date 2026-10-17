//! Smooth shading normals: each corner gets the average normal of the faces around its
//! vertex that meet at less than a crease angle, so curved surfaces look smooth while
//! sharp edges stay sharp.

use crate::mesh::Mesh;
use glam::Vec3;
use std::sync::Arc;

/// Smooth-shading data for a mesh.
pub struct Shading {
    /// Normal of each vertex group (a vertex split along sharp edges).
    pub normals: Vec<Vec3>,
    /// The original vertex each group belongs to.
    pub vertex: Arc<Vec<u32>>,
    /// Group of every triangle corner (3 per triangle).
    pub corner: Arc<Vec<u32>>,
    /// Groups of vertex `v` are `first[v]..first[v + 1]`.
    pub first: Arc<Vec<u32>>,
    /// Corners of vertex `v` are `vcorners[vstart[v]..vstart[v + 1]]`.
    pub vstart: Arc<Vec<u32>>,
    pub vcorners: Arc<Vec<u32>>,
}

/// Crease angle used for display.
pub const CREASE_DEG: f32 = 35.0;

impl Shading {
    /// The same grouping on a mesh with the same triangles but moved vertices.
    pub fn with_positions(&self, m: &Mesh) -> Shading {
        let mut sums = vec![Vec3::ZERO; self.normals.len()];
        for (f, t) in m.triangles.iter().enumerate() {
            let [a, b, c] = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
            let n = (b - a).cross(c - a);
            for k in 0..3 {
                sums[self.corner[f * 3 + k] as usize] += n;
            }
        }
        Shading {
            normals: sums.iter().zip(&self.normals).map(|(s, o)| s.normalize_or(*o)).collect(),
            vertex: self.vertex.clone(),
            corner: self.corner.clone(),
            first: self.first.clone(),
            vstart: self.vstart.clone(),
            vcorners: self.vcorners.clone(),
        }
    }

    /// Fresh normals for every group touching the given vertices' faces, after those
    /// vertices moved (same triangles). Returns (group, normal).
    pub fn renormal(&self, m: &Mesh, moved: &[u32]) -> Vec<(u32, Vec3)> {
        // vertices whose groups see a changed face: the moved ones and their neighbours
        let mut verts: Vec<u32> = Vec::with_capacity(moved.len() * 7);
        for &v in moved {
            for &c in &self.vcorners[self.vstart[v as usize] as usize..self.vstart[v as usize + 1] as usize] {
                verts.extend_from_slice(&m.triangles[(c / 3) as usize]);
            }
        }
        verts.sort_unstable();
        verts.dedup();
        let mut out = Vec::with_capacity(verts.len());
        let mut sums: Vec<(u32, Vec3)> = Vec::with_capacity(8);
        for v in verts {
            sums.clear();
            for &c in &self.vcorners[self.vstart[v as usize] as usize..self.vstart[v as usize + 1] as usize] {
                let [a, b, cc] = m.tri((c / 3) as usize);
                let n = (b - a).cross(cc - a);
                let g = self.corner[c as usize];
                match sums.iter_mut().find(|s| s.0 == g) {
                    Some(s) => s.1 += n,
                    None => sums.push((g, n)),
                }
            }
            for &(g, n) in &sums {
                out.push((g, n.normalize_or(self.normals[g as usize])));
            }
        }
        out
    }
}

pub fn shading(m: &Mesh, crease_deg: f32) -> Shading {
    let nv = m.positions.len();
    let nt = m.triangles.len();
    let cos = crease_deg.to_radians().cos();
    // area-weighted face normals, and unit ones for the crease test
    let fw: Vec<Vec3> = (0..nt)
        .map(|i| {
            let [a, b, c] = m.tri(i);
            (b - a).cross(c - a)
        })
        .collect();
    let fu: Vec<Vec3> = fw.iter().map(|n| n.normalize_or_zero()).collect();
    // vertex -> corners (CSR)
    let mut start = vec![0u32; nv + 1];
    for t in &m.triangles {
        for &v in t {
            start[v as usize + 1] += 1;
        }
    }
    for i in 0..nv {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut corners = vec![0u32; nt * 3];
    for (f, t) in m.triangles.iter().enumerate() {
        for (k, &v) in t.iter().enumerate() {
            corners[fill[v as usize] as usize] = (f * 3 + k) as u32;
            fill[v as usize] += 1;
        }
    }
    let mut normals: Vec<Vec3> = Vec::with_capacity(nv);
    let mut vertex: Vec<u32> = Vec::with_capacity(nv);
    let mut corner_group = vec![0u32; nt * 3];
    let mut first = vec![0u32; nv + 1];
    // (seed normal, summed normal, group id) for the current vertex
    let mut groups: Vec<(Vec3, Vec3, u32)> = Vec::with_capacity(8);
    for v in 0..nv {
        first[v] = normals.len() as u32;
        groups.clear();
        for &c in &corners[start[v] as usize..start[v + 1] as usize] {
            let f = (c / 3) as usize;
            let n = fu[f];
            let g = match groups.iter_mut().find(|g| g.0.dot(n) >= cos) {
                Some(g) => {
                    g.1 += fw[f];
                    g.2
                }
                None => {
                    let id = normals.len() as u32;
                    normals.push(Vec3::ZERO);
                    vertex.push(v as u32);
                    groups.push((n, fw[f], id));
                    id
                }
            };
            corner_group[c as usize] = g;
        }
        for g in &groups {
            normals[g.2 as usize] = g.1.normalize_or(g.0);
        }
    }
    first[nv] = normals.len() as u32;
    Shading {
        normals,
        vertex: Arc::new(vertex),
        corner: Arc::new(corner_group),
        first: Arc::new(first),
        vstart: Arc::new(start),
        vcorners: Arc::new(corners),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{cuboid, sphere};

    #[test]
    fn cube_keeps_sharp_edges_sphere_is_smooth() {
        let c = cuboid(Vec3::splat(10.0));
        let s = shading(&c, 35.0);
        // every corner of a cube splits into 3 groups (one per side)
        assert_eq!(s.normals.len(), c.positions.len() * 3);
        for (i, n) in s.normals.iter().enumerate() {
            let axis = n.abs().max_element();
            assert!((axis - 1.0).abs() < 1e-5, "group {i}: {n}");
        }
        // moving a vertex and renormalling matches a fresh computation
        let mut sp = sphere(10.0, 64, 32);
        let base = shading(&sp, 35.0);
        let v = 300u32;
        sp.positions[v as usize] *= 1.01;
        let fresh = shading(&sp, 35.0);
        assert_eq!(base.normals.len(), fresh.normals.len());
        let moved = base.with_positions(&sp);
        for (a, b) in moved.normals.iter().zip(&fresh.normals) {
            assert!(a.distance(*b) < 1e-4);
        }
        for (g, n) in base.renormal(&sp, &[v]) {
            assert!(n.distance(fresh.normals[g as usize]) < 1e-4, "{g} {n} {} {}", fresh.normals[g as usize], base.normals[g as usize]);
        }
        let sp = sphere(10.0, 32, 16);
        let s = shading(&sp, 35.0);
        assert_eq!(s.normals.len(), sp.positions.len());
        for (g, n) in s.normals.iter().enumerate() {
            let p = sp.positions[s.vertex[g] as usize].normalize();
            assert!(n.dot(p) > 0.97);
        }
    }
}
