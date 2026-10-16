//! Editing a selected area of a mesh (a set of triangles): extrude / push it, delete it,
//! copy it out as a solid, grow / shrink the selection.

use crate::mesh::Mesh;
use glam::Vec3;
use std::collections::HashMap;

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// Triangles per edge.
fn edges(m: &Mesh) -> HashMap<(u32, u32), Vec<usize>> {
    let mut e: HashMap<(u32, u32), Vec<usize>> = HashMap::with_capacity(m.triangles.len() * 2);
    for (i, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            e.entry(edge_key(t[k], t[(k + 1) % 3])).or_default().push(i);
        }
    }
    e
}

/// Add every triangle that shares a vertex with the selection.
pub fn grow(m: &Mesh, sel: &mut [bool]) {
    let mut v = vec![false; m.positions.len()];
    for (i, t) in m.triangles.iter().enumerate() {
        if sel[i] {
            t.iter().for_each(|&x| v[x as usize] = true);
        }
    }
    for (i, t) in m.triangles.iter().enumerate() {
        if t.iter().any(|&x| v[x as usize]) {
            sel[i] = true;
        }
    }
}

/// Drop every selected triangle that touches an unselected one.
pub fn shrink(m: &Mesh, sel: &mut [bool]) {
    let mut v = vec![false; m.positions.len()];
    for (i, t) in m.triangles.iter().enumerate() {
        if !sel[i] {
            t.iter().for_each(|&x| v[x as usize] = true);
        }
    }
    for (i, t) in m.triangles.iter().enumerate() {
        if t.iter().any(|&x| v[x as usize]) {
            sel[i] = false;
        }
    }
}

/// Area-weighted normal of the selection (unit), or None if nothing is selected.
pub fn average_normal(m: &Mesh, sel: &[bool]) -> Option<Vec3> {
    let mut n = Vec3::ZERO;
    for (i, _) in m.triangles.iter().enumerate().filter(|(i, _)| sel[*i]) {
        let [a, b, c] = m.tri(i);
        n += (b - a).cross(c - a);
    }
    (n.length_squared() > 0.0).then(|| n.normalize())
}

/// Per-vertex normals from the selected faces only.
fn region_normals(m: &Mesh, sel: &[bool]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; m.positions.len()];
    for (i, t) in m.triangles.iter().enumerate() {
        if sel[i] {
            let [a, b, c] = m.tri(i);
            let cr = (b - a).cross(c - a);
            for &v in t {
                n[v as usize] += cr;
            }
        }
    }
    n.into_iter().map(|v| v.normalize_or_zero()).collect()
}

/// Move the selected area out (positive) or in (negative) by `dist`, with walls joining
/// it to the rest, so a closed mesh stays closed. `along_normals`: every point moves
/// along its own normal (an offset); otherwise all move along the average normal.
pub fn extrude(m: &Mesh, sel: &[bool], dist: f32, along_normals: bool) -> Mesh {
    let mut out = m.clone();
    let painted = m.is_painted();
    let Some(avg) = average_normal(m, sel) else { return out };
    let vn = region_normals(m, sel);
    let e = edges(m);
    // vertices used by selected and by unselected triangles, or on an open edge, keep
    // their original copy (for the walls); the others just move
    let mut in_sel = vec![false; m.positions.len()];
    let mut in_rest = vec![false; m.positions.len()];
    for (i, t) in m.triangles.iter().enumerate() {
        for &v in t {
            if sel[i] {
                in_sel[v as usize] = true;
            } else {
                in_rest[v as usize] = true;
            }
        }
    }
    let mut boundary: Vec<(u32, u32, usize)> = vec![]; // directed as in the selected triangle
    for (i, t) in m.triangles.iter().enumerate() {
        if !sel[i] {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let others = &e[&edge_key(a, b)];
            if others.iter().all(|&j| j == i || !sel[j]) {
                boundary.push((a, b, i));
                in_rest[a as usize] |= others.len() == 1;
                in_rest[b as usize] |= others.len() == 1;
            }
        }
    }
    let mut moved: HashMap<u32, u32> = HashMap::new();
    for v in 0..m.positions.len() {
        if !in_sel[v] {
            continue;
        }
        let dir = if along_normals { vn[v] } else { avg };
        let p = m.positions[v] + dir * dist;
        if in_rest[v] {
            out.positions.push(p);
            moved.insert(v as u32, (out.positions.len() - 1) as u32);
        } else {
            out.positions[v] = p;
            moved.insert(v as u32, v as u32);
        }
    }
    for (i, t) in out.triangles.iter_mut().enumerate() {
        if sel[i] {
            *t = t.map(|v| moved[&v]);
        }
    }
    for (a, b, i) in boundary {
        let (a2, b2) = (moved[&a], moved[&b]);
        if a2 == a || b2 == b {
            continue;
        }
        out.triangles.push([a, b, b2]);
        out.triangles.push([a, b2, a2]);
        if painted {
            let c = m.paint[i];
            out.paint.extend([c, c]);
        }
    }
    out
}

/// Remove the selected triangles (leaves an opening; Repair can close it).
pub fn delete(m: &Mesh, sel: &[bool]) -> Mesh {
    let keep: Vec<usize> = (0..m.triangles.len()).filter(|&i| !sel[i]).collect();
    subset(m, &keep)
}

/// The triangles `tris` as a mesh of their own (paint kept).
pub fn subset(m: &Mesh, tris: &[usize]) -> Mesh {
    let mut out = Mesh { positions: m.positions.clone(), triangles: tris.iter().map(|&i| m.triangles[i]).collect(), paint: vec![] };
    if m.is_painted() {
        out.paint = tris.iter().map(|&i| m.paint[i]).collect();
    }
    out.compact();
    out.tidy_paint();
    out
}

/// The selected surface as a closed solid `thickness` thick (grown inwards).
pub fn copy_as_solid(m: &Mesh, sel: &[bool], thickness: f32) -> Mesh {
    let tris: Vec<usize> = (0..m.triangles.len()).filter(|&i| sel[i]).collect();
    let s = subset(m, &tris);
    let all = vec![true; s.triangles.len()];
    let vn = region_normals(&s, &all);
    let n = s.positions.len() as u32;
    let mut out = s.clone();
    out.positions.extend(s.positions.iter().zip(&vn).map(|(p, d)| *p - *d * thickness));
    for t in &s.triangles {
        out.triangles.push([t[0] + n, t[2] + n, t[1] + n]);
    }
    let e = edges(&s);
    for t in &s.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if e[&edge_key(a, b)].len() == 1 {
                out.triangles.push([b, a, a + n]);
                out.triangles.push([b, a + n, b + n]);
            }
        }
    }
    if s.is_painted() {
        let p = s.paint.clone();
        out.paint = p.iter().chain(p.iter()).copied().collect();
        out.paint.resize(out.triangles.len(), 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    fn top_of(m: &Mesh) -> Vec<bool> {
        (0..m.triangles.len()).map(|i| m.face_normal(i).z > 0.9).collect()
    }

    #[test]
    fn extrude_top_face() {
        let m = cuboid(Vec3::splat(10.0));
        let up = extrude(&m, &top_of(&m), 5.0, false);
        assert!(analyze(&up).is_watertight());
        assert!((up.volume() - 1500.0).abs() < 1e-3, "{}", up.volume());
        let down = extrude(&m, &top_of(&m), -4.0, false);
        assert!(analyze(&down).is_watertight());
        assert!((down.volume() - 600.0).abs() < 1e-3, "{}", down.volume());
        // a refined face pushed out along its normals keeps the rest in place
        let r = crate::paint::refine(&m, 3.0, 100_000);
        let sel = top_of(&r);
        let o = extrude(&r, &sel, 2.0, true);
        assert!(analyze(&o).is_watertight());
        assert!((o.bounds().max.z - 7.0).abs() < 1e-4);
    }

    #[test]
    fn delete_and_copy() {
        let m = cuboid(Vec3::splat(10.0));
        let d = delete(&m, &top_of(&m));
        assert_eq!(d.triangles.len(), 10);
        assert!(!analyze(&d).is_watertight());
        let c = copy_as_solid(&m, &top_of(&m), 2.0);
        assert!(analyze(&c).is_watertight(), "{:?}", analyze(&c).problems());
        assert!((c.volume() - 200.0).abs() < 1e-3, "{}", c.volume());
    }

    #[test]
    fn grow_and_shrink() {
        let m = crate::paint::refine(&cuboid(Vec3::splat(10.0)), 2.0, 100_000);
        let mut s = vec![false; m.triangles.len()];
        s[0] = true;
        grow(&m, &mut s);
        let g = s.iter().filter(|x| **x).count();
        assert!(g > 3);
        shrink(&m, &mut s);
        assert!(s.iter().filter(|x| **x).count() < g);
    }
}
