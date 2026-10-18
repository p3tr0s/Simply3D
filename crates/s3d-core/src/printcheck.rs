//! Print check: the parts of a model that are likely to print badly, per triangle.
//!
//! - **Overhangs**: downward-facing surfaces leaning further than an angle from vertical
//!   (they need supports). The bottom the model stands on doesn't count.
//! - **Islands**: spots where a layer starts in mid-air, with nothing below it (a lowest
//!   point or lowest flat area that isn't on the bottom).
//! - **Thin walls**: places where the model is thinner than a minimum, measured straight
//!   through the surface.

use crate::bvh::Bvh;
use crate::mesh::Mesh;
use glam::{Mat4, Vec3};

pub const OVERHANG: u8 = 1;
pub const THIN: u8 = 2;
pub const ISLAND: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CheckOptions {
    /// Steepest printable lean from vertical, degrees (45 is typical).
    pub overhang_deg: f32,
    /// Thinnest printable wall, mm (about two nozzle widths). 0 = don't check.
    pub min_wall: f32,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions { overhang_deg: 45.0, min_wall: 0.8 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CheckStats {
    /// Areas in mm².
    pub overhang_area: f32,
    pub thin_area: f32,
    /// Separate places where printing starts in mid-air.
    pub islands: usize,
}

/// Flags per triangle for the mesh placed by `xf` (rotation and scale matter; the check
/// is relative to the model's own bottom, so moving it doesn't change anything).
/// `bvh` (for `m` as given) speeds up the wall check.
pub fn check(m: &Mesh, xf: &Mat4, o: &CheckOptions, bvh: Option<&Bvh>) -> (Vec<u8>, CheckStats) {
    let nt = m.triangles.len();
    let mut flags = vec![0u8; nt];
    let mut stats = CheckStats::default();
    if nt == 0 {
        return (flags, stats);
    }
    let w: Vec<Vec3> = m.positions.iter().map(|p| xf.transform_point3(*p)).collect();
    let flip = xf.determinant() < 0.0;
    let bottom = w.iter().map(|p| p.z).fold(f32::INFINITY, f32::min);
    let size = w.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let eps = ((size.1 - size.0).max_element() * 1e-5).max(1e-4);
    let on_bed = 0.05f32.max(eps * 10.0);
    let limit = -(o.overhang_deg.clamp(0.0, 89.0).to_radians().sin());
    // the first couple of layers get squashed onto the plate: bumps and textures there are fine
    let first_layers = 0.5f32.max(on_bed);

    // world normals and areas
    let mut normal = Vec::with_capacity(nt);
    let mut area = Vec::with_capacity(nt);
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| w[i as usize]);
        let cr = (b - a).cross(c - a);
        let len = cr.length();
        area.push(len * 0.5);
        let n = if len > 0.0 { cr / len } else { Vec3::ZERO };
        normal.push(if flip { -n } else { n });
    }

    // overhangs
    for (i, t) in m.triangles.iter().enumerate() {
        let n = normal[i];
        let lowest_on_bed = t.iter().all(|&v| w[v as usize].z <= bottom + first_layers);
        if n.z < limit && !lowest_on_bed {
            flags[i] |= OVERHANG;
            stats.overhang_area += area[i];
        }
    }

    // ray casting against the model: the mesh as given when it isn't scaled (rays are
    // turned into its own space), otherwise the placed mesh
    let unscaled = {
        let (sc, _, _) = xf.to_scale_rotation_translation();
        (sc.abs() - Vec3::ONE).abs().max_element() < 1e-4
    };
    let built;
    let world_mesh;
    let (ray_mesh, ray_bvh, to_mesh): (&Mesh, &Bvh, Mat4) = match bvh {
        Some(b) if unscaled => (m, b, xf.inverse()),
        _ => {
            world_mesh = Mesh { positions: w.clone(), triangles: m.triangles.clone(), paint: vec![] };
            built = Bvh::build(&world_mesh);
            (&world_mesh, &built, Mat4::IDENTITY)
        }
    };
    // first hit from a world point along a world direction: (distance, triangle)
    let cast = |p: Vec3, d: Vec3| -> Option<(f32, usize)> {
        let (o, dir) = (to_mesh.transform_point3(p), to_mesh.transform_vector3(d).normalize_or_zero());
        ray_bvh.ray_cast_where(ray_mesh, o, dir, |_| true)
    };

    // islands: a lowest spot under the model (only faces pointing down around it) with
    // nothing of the model just below it
    let nv = m.positions.len();
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
    let mut vtris = vec![0u32; start[nv] as usize];
    for (f, t) in m.triangles.iter().enumerate() {
        for &v in t {
            vtris[fill[v as usize] as usize] = f as u32;
            fill[v as usize] += 1;
        }
    }
    let faces_of = |v: usize| &vtris[start[v] as usize..start[v + 1] as usize];
    let lower_neighbour = |v: usize| {
        faces_of(v).iter().any(|&f| m.triangles[f as usize].iter().any(|&u| w[u as usize].z < w[v].z - eps))
    };
    let mut seen = vec![false; nv];
    let mut island_vert = vec![false; nv];
    let mut stack = vec![];
    let mut comp = vec![];
    let gap = (o.min_wall * 2.0).max(1.0);
    // nearby starts (one rough spot) count once
    let mut cells: std::collections::HashSet<[i32; 3]> = Default::default();
    for s in 0..nv {
        if seen[s] || faces_of(s).is_empty() || w[s].z <= bottom + first_layers || lower_neighbour(s) {
            continue;
        }
        // the flat area at this height; an island only if no part of it reaches lower
        // (a bridge's ceiling reaches walls that go down) and it faces down
        let z = w[s].z;
        let mut island = true;
        let mut faces_down = false;
        comp.clear();
        stack.push(s);
        seen[s] = true;
        while let Some(v) = stack.pop() {
            comp.push(v);
            for &f in faces_of(v) {
                // a dip in a top surface isn't an island
                let nz = normal[f as usize].z;
                if nz > 0.3 {
                    island = false;
                }
                faces_down |= nz < -0.3;
                for &u in &m.triangles[f as usize] {
                    let u = u as usize;
                    if seen[u] || (w[u].z - z).abs() > eps {
                        continue;
                    }
                    seen[u] = true;
                    if lower_neighbour(u) {
                        island = false;
                    } else {
                        stack.push(u);
                    }
                }
            }
        }
        island &= faces_down;
        // something of the model right under it holds it up (textures, small dips)
        if island && cast(w[s] - Vec3::Z * (eps * 4.0), -Vec3::Z).is_some_and(|(d, _)| d < gap) {
            island = false;
        }
        // ... or material right beside it, a little lower: it grows out of the layer below
        // (a bump on a sloping surface is an overhang, not an island)
        if island {
            let out: Vec3 = faces_of(s).iter().map(|&f| normal[f as usize] * area[f as usize]).sum();
            let side = Vec3::new(-out.x, -out.y, 0.0).normalize_or_zero();
            if side != Vec3::ZERO {
                let q = w[s] + side * gap - Vec3::Z * (eps * 8.0).max(0.05);
                let inside = cast(q, Vec3::Z).is_some_and(|(_, j)| normal[j].z > 0.0);
                if inside {
                    island = false;
                }
            }
        }
        if island {
            let c = (w[s] / 3.0).floor().as_ivec3().to_array();
            let near = (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz| cells.contains(&[c[0] + dx, c[1] + dy, c[2] + dz]))));
            if !near {
                stats.islands += 1;
            }
            cells.insert(c);
            for &v in &comp {
                island_vert[v] = true;
            }
        }
    }
    for (i, t) in m.triangles.iter().enumerate() {
        if t.iter().any(|&v| island_vert[v as usize]) && normal[i].z < 0.0 {
            flags[i] |= ISLAND;
        }
    }

    // thin walls: shoot inwards from each triangle's centre
    if o.min_wall > 0.0 {
        for (i, t) in m.triangles.iter().enumerate() {
            let n = normal[i];
            if area[i] <= 0.0 || n == Vec3::ZERO {
                continue;
            }
            let centre = (w[t[0] as usize] + w[t[1] as usize] + w[t[2] as usize]) / 3.0;
            if let Some((d, j)) = cast(centre - n * (eps * 4.0), -n) {
                if j != i && d + eps * 4.0 < o.min_wall {
                    flags[i] |= THIN;
                    stats.thin_area += area[i];
                }
            }
        }
    }
    (flags, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cuboid;

    #[test]
    fn finds_overhangs_islands_and_thin_walls() {
        // a T shape: the arms overhang, nothing is an island, nothing is thin
        let mut post = cuboid(Vec3::new(10.0, 10.0, 20.0));
        post.translate(Vec3::new(0.0, 0.0, 10.0));
        let mut arms = cuboid(Vec3::new(40.0, 10.0, 4.0));
        arms.translate(Vec3::new(0.0, 0.0, 22.0));
        let t = crate::csg::boolean(&post, &arms, crate::csg::BoolOp::Union).unwrap();
        let (f, s) = check(&t, &Mat4::IDENTITY, &CheckOptions::default(), None);
        assert!((s.overhang_area - 300.0).abs() < 1.0, "{s:?}"); // 2 × 15 × 10 underneath the arms
        assert_eq!(s.islands, 0);
        assert_eq!(s.thin_area, 0.0);
        assert!(f.iter().any(|&x| x & OVERHANG != 0));

        // standing upside down on its arms: nothing overhangs
        let flipped = Mat4::from_rotation_x(std::f32::consts::PI);
        let (_, s) = check(&t, &flipped, &CheckOptions::default(), None);
        assert!(s.overhang_area < 1.0, "{s:?}");

        // a cube floating above another: its bottom is an island (and an overhang)
        let mut a = cuboid(Vec3::splat(10.0));
        a.translate(Vec3::new(0.0, 0.0, 5.0));
        let mut b = cuboid(Vec3::splat(4.0));
        b.translate(Vec3::new(20.0, 0.0, 12.0));
        a.append(&b);
        let (_, s) = check(&a, &Mat4::IDENTITY, &CheckOptions::default(), None);
        assert_eq!(s.islands, 1, "{s:?}");

        // a 0.5 mm sheet is thin
        let sheet = cuboid(Vec3::new(20.0, 20.0, 0.5));
        let (_, s) = check(&sheet, &Mat4::IDENTITY, &CheckOptions::default(), None);
        assert!(s.thin_area > 700.0, "{s:?}");
        // ... but not when scaled up 4×
        let (_, s) = check(&sheet, &Mat4::from_scale(Vec3::splat(4.0)), &CheckOptions::default(), None);
        assert!(s.thin_area < 1.0, "{s:?}");
    }
}
