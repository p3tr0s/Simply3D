//! Extrude down (as in 3D Builder): every downward-facing part of the surface below a
//! height threshold is extended straight down to the build plate and joined to the model.
//! Gives a model a flat base, props up overhangs and ties floating parts to the plate.

use crate::csg::{boolean, BoolOp};
use crate::mesh::Mesh;
use crate::{Error, Progress, Result};
use glam::Vec3;
use std::collections::HashMap;

/// Faces steeper than this (|normal.z| below it) count as walls, not undersides.
const DOWN: f32 = -0.02;

fn key(p: Vec3) -> [u32; 3] {
    [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
}

/// Point where segment p–q crosses z = h. Endpoints are ordered first so both triangles
/// sharing an edge get bit-identical points (needed for exact welding).
fn cross_z(p: Vec3, q: Vec3, h: f32) -> Vec3 {
    let (p, q) = if key(p) <= key(q) { (p, q) } else { (q, p) };
    let t = (h - p.z) / (q.z - p.z);
    let mut r = p + (q - p) * t;
    r.z = h;
    r
}

/// Clip a polygon to `keep(z)` against the plane z = h (Sutherland–Hodgman).
fn clip(poly: &[Vec3], h: f32, below: bool) -> Vec<Vec3> {
    let inside = |p: &Vec3| if below { p.z <= h } else { p.z >= h };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        match (inside(&a), inside(&b)) {
            (true, true) => out.push(b),
            (true, false) => out.push(cross_z(a, b, h)),
            (false, true) => {
                out.push(cross_z(a, b, h));
                out.push(b);
            }
            (false, false) => {}
        }
    }
    out
}

/// Solid columns under the downward-facing surface of `m` between `floor` and `height`,
/// one closed mesh per connected underside patch.
pub fn underside_prisms(m: &Mesh, height: f32, floor: f32) -> Vec<Mesh> {
    // weld by position so patches connect even if the input has split vertices
    let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
    let vid: Vec<u32> = m
        .positions
        .iter()
        .map(|p| {
            let n = ids.len() as u32;
            *ids.entry(key(*p)).or_insert(n)
        })
        .collect();
    let down: Vec<usize> = (0..m.triangles.len())
        .filter(|&i| {
            let [a, b, c] = m.tri(i);
            m.face_normal(i).z < DOWN && a.z.min(b.z).min(c.z) < height && a.z.max(b.z).max(c.z) > floor + 1e-5
        })
        .collect();
    if down.is_empty() {
        return vec![];
    }
    // connected patches (union-find over shared edges)
    let mut parent: Vec<usize> = (0..down.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut edge_owner: HashMap<(u32, u32), usize> = HashMap::new();
    for (k, &ti) in down.iter().enumerate() {
        let t = m.triangles[ti];
        for e in 0..3 {
            let (a, b) = (vid[t[e] as usize], vid[t[(e + 1) % 3] as usize]);
            let ek = (a.min(b), a.max(b));
            match edge_owner.get(&ek) {
                Some(&o) => {
                    let (ra, rb) = (find(&mut parent, k), find(&mut parent, o));
                    parent[ra] = rb;
                }
                None => {
                    edge_owner.insert(ek, k);
                }
            }
        }
    }
    let mut patches: HashMap<usize, Vec<usize>> = HashMap::new();
    for k in 0..down.len() {
        let r = find(&mut parent, k);
        patches.entry(r).or_default().push(down[k]);
    }

    let mut out = vec![];
    for tris in patches.values() {
        // clip the underside to floor <= z <= height and triangulate
        let mut top: Vec<[Vec3; 3]> = vec![];
        for &ti in tris {
            let poly = clip(&clip(&m.tri(ti), height, true), floor, false);
            if poly.len() < 3 || poly.iter().all(|p| p.z <= floor + 1e-5) {
                continue;
            }
            for i in 1..poly.len() - 1 {
                top.push([poly[0], poly[i], poly[i + 1]]);
            }
        }
        if top.is_empty() {
            continue;
        }
        let patch = Mesh::from_soup(&top);
        let floor_of = |p: Vec3| Vec3::new(p.x, p.y, floor);
        let mut soup: Vec<[Vec3; 3]> = vec![];
        let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &patch.triangles {
            for e in 0..3 {
                *directed.entry((t[e], t[(e + 1) % 3])).or_default() += 1;
            }
        }
        for (i, t) in patch.triangles.iter().enumerate() {
            let [a, b, c] = patch.tri(i);
            soup.push([c, b, a]); // top of the column faces up (into the model)
            soup.push([floor_of(a), floor_of(b), floor_of(c)]); // bottom faces down
            for e in 0..3 {
                let (ia, ib) = (t[e], t[(e + 1) % 3]);
                if !directed.contains_key(&(ib, ia)) {
                    let (pa, pb) = (patch.positions[ia as usize], patch.positions[ib as usize]);
                    soup.push([pa, pb, floor_of(pb)]);
                    soup.push([pa, floor_of(pb), floor_of(pa)]);
                }
            }
        }
        let mut prism = Mesh::from_soup(&soup);
        prism.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
        if prism.volume() > 1e-9 {
            out.push(prism);
        }
    }
    out
}

/// Extrude the underside of `m` below `height` straight down to `floor` (the plate).
pub fn extrude_down(m: &Mesh, height: f32, floor: f32, progress: &Progress) -> Result<Mesh> {
    let prisms = underside_prisms(m, height, floor);
    if prisms.is_empty() {
        return Err(Error::Parse("nothing faces down below that height".into()));
    }
    // columns whose footprints don't overlap can be joined without a boolean
    let mut bins: Vec<(Mesh, Vec<crate::Aabb>)> = vec![];
    for p in prisms {
        let b = p.bounds();
        let overlaps = |o: &crate::Aabb| b.min.x < o.max.x && o.min.x < b.max.x && b.min.y < o.max.y && o.min.y < b.max.y;
        match bins.iter_mut().find(|(_, bs)| !bs.iter().any(overlaps)) {
            Some((mesh, bs)) => {
                mesh.append(&p);
                bs.push(b);
            }
            None => bins.push((p, vec![b])),
        }
    }
    let n = bins.len() as f32;
    let mut acc = m.clone();
    for (i, (bin, _)) in bins.into_iter().enumerate() {
        progress.check()?;
        progress.set(i as f32 / n);
        acc = boolean(&acc, &bin, BoolOp::Union)?;
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::Primitive;

    fn sphere(r: f32, z: f32) -> Mesh {
        let mut m = Primitive::Sphere { radius: r, segments: 48, rings: 24 }.build();
        m.translate(Vec3::new(0.0, 0.0, z));
        m
    }

    #[test]
    fn floating_sphere_gets_a_column_to_the_plate() {
        // sphere floating 5 mm above the plate; threshold at its equator
        let m = sphere(10.0, 15.0);
        let out = extrude_down(&m, 15.0, 0.0, &Progress::new()).unwrap();
        let r = analyze(&out);
        assert!(r.is_watertight(), "{:?}", r.problems());
        let b = out.bounds();
        assert!(b.min.z.abs() < 1e-4, "reaches the plate: {}", b.min.z);
        assert!((b.max.z - 25.0).abs() < 1e-3);
        // ≈ upper hemisphere + cylinder r=10 from 0 to 15
        let expect = 2.0 / 3.0 * std::f64::consts::PI * 1000.0 + std::f64::consts::PI * 100.0 * 15.0;
        assert!((out.volume() - expect).abs() / expect < 0.02, "{} vs {expect}", out.volume());
    }

    #[test]
    fn low_threshold_only_touches_the_bottom() {
        let m = sphere(10.0, 15.0);
        let out = extrude_down(&m, 8.0, 0.0, &Progress::new()).unwrap();
        assert!(analyze(&out).is_watertight());
        // the column is narrower than the sphere: it is the cap's footprint at z = 8
        let b = out.bounds();
        assert!(b.min.z.abs() < 1e-4 && (b.size().x - 20.0).abs() < 0.2);
        assert!(out.volume() > m.volume() && out.volume() < m.volume() + std::f64::consts::PI * 100.0 * 8.0);
    }

    #[test]
    fn overhang_gets_supported() {
        // a T: stem on the plate, a bar sticking out on both sides at the top
        let mut stem = Primitive::Box { size: Vec3::new(4.0, 4.0, 20.0), edge: Default::default() }.build();
        stem.translate(Vec3::new(0.0, 0.0, 10.0));
        let mut bar = Primitive::Box { size: Vec3::new(30.0, 4.0, 4.0), edge: Default::default() }.build();
        bar.translate(Vec3::new(0.0, 0.0, 22.0));
        let t = boolean(&stem, &bar, BoolOp::Union).unwrap();
        let out = extrude_down(&t, 25.0, 0.0, &Progress::new()).unwrap();
        assert!(analyze(&out).is_watertight());
        // becomes a solid block 30 × 4 × 24
        assert!((out.volume() - 30.0 * 4.0 * 24.0).abs() < 1.0, "{}", out.volume());
        // threshold below the bar: unchanged
        assert!(extrude_down(&t, 15.0, 0.0, &Progress::new()).is_err());
    }
}

#[cfg(test)]
mod heavy {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::Primitive;

    #[test]
    fn torus_and_dense_sphere() {
        let mut t = Primitive::Torus { major_radius: 20.0, minor_radius: 5.0, major_segments: 96, minor_segments: 48 }.build();
        t.translate(Vec3::new(0.0, 0.0, 12.0));
        let t0 = std::time::Instant::now();
        let out = extrude_down(&t, 12.0, 0.0, &Progress::new()).unwrap();
        let r = analyze(&out);
        assert!(r.is_watertight(), "{:?}", r.problems());
        assert_eq!(out.connected_components().len(), 1);
        eprintln!("torus {} tris in {:?}", out.triangles.len(), t0.elapsed());

        let mut s = Primitive::Sphere { radius: 30.0, segments: 400, rings: 200 }.build();
        s.translate(Vec3::new(0.0, 0.0, 40.0));
        let t0 = std::time::Instant::now();
        let out = extrude_down(&s, 30.0, 0.0, &Progress::new()).unwrap();
        assert!(analyze(&out).is_watertight());
        eprintln!("sphere {} → {} tris in {:?}", s.triangles.len(), out.triangles.len(), t0.elapsed());
    }
}
