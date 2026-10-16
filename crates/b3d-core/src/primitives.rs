//! Parametric primitives. All are closed, outward-oriented and centred on the
//! origin (bounding-box centre), Z up.

use crate::mesh::Mesh;
use glam::Vec3;
use std::f32::consts::TAU;

/// Edge treatment for boxes and cylinders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeKind {
    #[default]
    Sharp,
    Fillet,
    Chamfer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// Fillet radius / chamfer distance in mm.
    pub size: f32,
    /// Segments per quarter-round (fillet only).
    pub segments: u32,
}

impl Default for Edge {
    fn default() -> Self {
        Edge { kind: EdgeKind::Sharp, size: 2.0, segments: 8 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Primitive {
    Box {
        size: Vec3,
        edge: Edge,
    },
    Cylinder {
        radius: f32,
        height: f32,
        segments: u32,
        edge: Edge,
    },
    Cone {
        bottom_radius: f32,
        top_radius: f32,
        height: f32,
        segments: u32,
    },
    Sphere {
        radius: f32,
        segments: u32,
        rings: u32,
    },
    Torus {
        major_radius: f32,
        minor_radius: f32,
        major_segments: u32,
        minor_segments: u32,
    },
    Tube {
        outer_radius: f32,
        inner_radius: f32,
        height: f32,
        segments: u32,
    },
    Pyramid {
        base: f32,
        height: f32,
    },
    Wedge {
        size: Vec3,
    },
    /// Bolt, nut, threaded rod or threaded-hole cutter.
    Fastener(crate::threads::Fastener),
    Gear(crate::parts::Gear),
    Spring(crate::parts::Spring),
    Hinge(crate::parts::Hinge),
    SnapClip(crate::parts::SnapClip),
}

impl Primitive {
    pub const ALL_NAMES: [&'static str; 8] = ["Box", "Cylinder", "Cone", "Sphere", "Torus", "Tube", "Pyramid", "Wedge"];
    pub const FASTENER_NAMES: [&'static str; 4] = ["Bolt", "Nut", "Threaded rod", "Threaded hole"];
    pub const PART_NAMES: [&'static str; 4] = ["Gear", "Spring", "Hinge", "Snap-fit clip"];

    pub fn default_for(name: &str) -> Primitive {
        match name {
            "Cylinder" => Primitive::Cylinder { radius: 10.0, height: 20.0, segments: 64, edge: Edge::default() },
            "Cone" => Primitive::Cone { bottom_radius: 10.0, top_radius: 0.0, height: 20.0, segments: 64 },
            "Sphere" => Primitive::Sphere { radius: 10.0, segments: 64, rings: 32 },
            "Torus" => Primitive::Torus { major_radius: 15.0, minor_radius: 5.0, major_segments: 64, minor_segments: 32 },
            "Tube" => Primitive::Tube { outer_radius: 10.0, inner_radius: 7.0, height: 20.0, segments: 64 },
            "Pyramid" => Primitive::Pyramid { base: 20.0, height: 20.0 },
            "Wedge" => Primitive::Wedge { size: Vec3::new(20.0, 20.0, 20.0) },
            "Bolt" | "Nut" | "Threaded rod" | "Threaded hole" => {
                use crate::threads::{Fastener, FastenerKind};
                let kind = match name {
                    "Nut" => FastenerKind::Nut,
                    "Threaded rod" => FastenerKind::Rod,
                    "Threaded hole" => FastenerKind::ThreadedHole,
                    _ => FastenerKind::Bolt,
                };
                Primitive::Fastener(Fastener { kind, ..Default::default() })
            }
            "Gear" => Primitive::Gear(Default::default()),
            "Spring" => Primitive::Spring(Default::default()),
            "Hinge" => Primitive::Hinge(Default::default()),
            "Snap-fit clip" => Primitive::SnapClip(Default::default()),
            _ => Primitive::Box { size: Vec3::splat(20.0), edge: Edge::default() },
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "Box",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Cone { .. } => "Cone",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Torus { .. } => "Torus",
            Primitive::Tube { .. } => "Tube",
            Primitive::Pyramid { .. } => "Pyramid",
            Primitive::Wedge { .. } => "Wedge",
            Primitive::Fastener(f) => match f.kind {
                crate::threads::FastenerKind::Bolt => "Bolt",
                crate::threads::FastenerKind::Nut => "Nut",
                crate::threads::FastenerKind::Rod => "Threaded rod",
                crate::threads::FastenerKind::ThreadedHole => "Threaded hole",
            },
            Primitive::Gear(_) => "Gear",
            Primitive::Spring(_) => "Spring",
            Primitive::Hinge(_) => "Hinge",
            Primitive::SnapClip(_) => "Snap-fit clip",
        }
    }

    pub fn build(&self) -> Mesh {
        match *self {
            Primitive::Box { size, edge } => match edge.kind {
                EdgeKind::Sharp => cuboid(size),
                k => beveled_box(size, edge.size, if k == EdgeKind::Chamfer { 1 } else { edge.segments }, k == EdgeKind::Chamfer),
            },
            Primitive::Cylinder { radius, height, segments, edge } => match edge.kind {
                EdgeKind::Sharp => frustum(radius, radius, height, segments),
                k => beveled_cylinder(radius, height, segments, edge.size, if k == EdgeKind::Chamfer { 1 } else { edge.segments }),
            },
            Primitive::Cone { bottom_radius, top_radius, height, segments } => frustum(bottom_radius, top_radius, height, segments),
            Primitive::Sphere { radius, segments, rings } => sphere(radius, segments, rings),
            Primitive::Torus { major_radius, minor_radius, major_segments, minor_segments } => {
                torus(major_radius, minor_radius, major_segments, minor_segments)
            }
            Primitive::Tube { outer_radius, inner_radius, height, segments } => tube(outer_radius, inner_radius, height, segments),
            Primitive::Pyramid { base, height } => pyramid(base, height),
            Primitive::Wedge { size } => wedge(size),
            Primitive::Fastener(f) => {
                let mut m = crate::threads::fastener_mesh(&f).unwrap_or_default();
                let c = m.bounds().center();
                m.translate(-c);
                m
            }
            Primitive::Gear(_) | Primitive::Spring(_) | Primitive::Hinge(_) | Primitive::SnapClip(_) => {
                let mut m = match *self {
                    Primitive::Gear(g) => crate::parts::gear_mesh(&g),
                    Primitive::Spring(s) => crate::parts::spring_mesh(&s),
                    Primitive::Hinge(h) => crate::parts::hinge_mesh(&h).unwrap_or_default(),
                    Primitive::SnapClip(s) => crate::parts::snap_clip_mesh(&s),
                    _ => unreachable!(),
                };
                let c = m.bounds().center();
                m.translate(-c);
                m
            }
        }
    }
}

/// Box with filleted (round) or chamfered edges and corners.
///
/// Built as a grid on the box faces whose lines sit at the start of each edge zone,
/// then every vertex is pushed out from the shrunken "inner box": along the L2
/// direction for fillets (giving exact quarter-circles / sphere octants with even
/// angular spacing) or the L1 direction for chamfers (45° flats, triangular corners).
pub fn beveled_box(size: Vec3, radius: f32, segments: u32, chamfer: bool) -> Mesh {
    let h = size * 0.5;
    let r = radius.clamp(0.0, h.min_element());
    if r <= 1e-6 {
        return cuboid(size);
    }
    let k = segments.max(1);
    let inner = h - Vec3::splat(r);
    // per-axis grid coordinates: edge zone, flat zone, edge zone
    let coords = |a: usize| -> Vec<f32> {
        let mut c = vec![];
        let offs: Vec<f32> = (0..=k)
            .map(|j| if chamfer { r * j as f32 / k as f32 } else { r * (std::f32::consts::FRAC_PI_4 * j as f32 / k as f32).tan() })
            .collect();
        for o in offs.iter().rev() {
            c.push(-inner[a] - o);
        }
        for o in offs.iter() {
            c.push(inner[a] + o);
        }
        c.dedup_by(|x, y| (*x - *y).abs() < 1e-7);
        c
    };
    let cs = [coords(0), coords(1), coords(2)];
    let mut soup: Vec<[Vec3; 3]> = vec![];
    for a in 0..3 {
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        for sign in [-1.0f32, 1.0] {
            for i in 0..cs[b].len() - 1 {
                for j in 0..cs[c].len() - 1 {
                    let pt = |ib: usize, jc: usize| {
                        let mut p = Vec3::ZERO;
                        p[a] = sign * h[a];
                        p[b] = cs[b][ib];
                        p[c] = cs[c][jc];
                        p
                    };
                    let (p00, p10, p11, p01) = (pt(i, j), pt(i + 1, j), pt(i + 1, j + 1), pt(i, j + 1));
                    if sign > 0.0 {
                        soup.push([p00, p10, p11]);
                        soup.push([p00, p11, p01]);
                    } else {
                        soup.push([p00, p11, p10]);
                        soup.push([p00, p01, p11]);
                    }
                }
            }
        }
    }
    let mut m = Mesh::from_soup(&soup);
    for p in &mut m.positions {
        let q = p.clamp(-inner, inner);
        let d = *p - q;
        let len = if chamfer { d.x.abs() + d.y.abs() + d.z.abs() } else { d.length() };
        if len > 1e-9 {
            *p = q + d / len * r;
        }
    }
    // zero-area triangles appear where grid cells collapse (r == half-size); drop them
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    crate::repair::weld(&mut m, r * 1e-5);
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    m.compact();
    m
}

/// Surface of revolution around Z from a profile of (radius, z) points running from
/// the bottom axis to the top axis. Points with radius 0 become poles.
pub fn revolve(profile: &[(f32, f32)], segments: u32) -> Mesh {
    let n = segments.max(3);
    let mut m = Mesh::default();
    let rings: Vec<Vec<u32>> = profile
        .iter()
        .map(|&(r, z)| {
            let start = m.positions.len() as u32;
            if r <= 1e-7 {
                m.positions.push(Vec3::new(0.0, 0.0, z));
                vec![start; n as usize]
            } else {
                m.positions.extend(ring(r, z, n));
                (start..start + n).collect()
            }
        })
        .collect();
    for w in rings.windows(2) {
        let (lo, hi) = (&w[0], &w[1]);
        for s in 0..n as usize {
            let t = (s + 1) % n as usize;
            let tri1 = [lo[s], lo[t], hi[t]];
            let tri2 = [lo[s], hi[t], hi[s]];
            for tri in [tri1, tri2] {
                if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                    m.triangles.push(tri);
                }
            }
        }
    }
    m
}

/// Cylinder with filleted (segments > 1) or chamfered (segments == 1) top and bottom edges.
pub fn beveled_cylinder(radius: f32, height: f32, segments: u32, edge: f32, edge_segments: u32) -> Mesh {
    let e = edge.clamp(0.0, radius.min(height * 0.5));
    let hz = height * 0.5;
    if e <= 1e-6 {
        return frustum(radius, radius, height, segments);
    }
    let k = edge_segments.max(1);
    let mut prof = vec![(0.0, -hz)];
    for j in 0..=k {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_2 * j as f32 / k as f32;
        prof.push((radius - e + e * a.cos(), -hz + e + e * a.sin()));
    }
    for j in 0..=k {
        let a = std::f32::consts::FRAC_PI_2 * j as f32 / k as f32;
        prof.push((radius - e + e * a.cos(), hz - e + e * a.sin()));
    }
    prof.push((0.0, hz));
    prof.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-7 && (a.1 - b.1).abs() < 1e-7);
    // fully rounded ends start on the axis: drop the duplicate pole points
    if prof.len() > 1 && prof[1].0 <= 1e-6 {
        prof.remove(0);
    }
    let l = prof.len();
    if l > 1 && prof[l - 2].0 <= 1e-6 {
        prof.pop();
    }
    revolve(&prof, segments)
}

pub fn cuboid(size: Vec3) -> Mesh {
    let h = size * 0.5;
    let p = vec![
        Vec3::new(-h.x, -h.y, -h.z),
        Vec3::new(h.x, -h.y, -h.z),
        Vec3::new(h.x, h.y, -h.z),
        Vec3::new(-h.x, h.y, -h.z),
        Vec3::new(-h.x, -h.y, h.z),
        Vec3::new(h.x, -h.y, h.z),
        Vec3::new(h.x, h.y, h.z),
        Vec3::new(-h.x, h.y, h.z),
    ];
    let t = vec![
        [0, 2, 1],
        [0, 3, 2], // bottom
        [4, 5, 6],
        [4, 6, 7], // top
        [0, 1, 5],
        [0, 5, 4], // -y
        [1, 2, 6],
        [1, 6, 5], // +x
        [2, 3, 7],
        [2, 7, 6], // +y
        [3, 0, 4],
        [3, 4, 7], // -x
    ];
    Mesh::new(p, t)
}

fn ring(r: f32, z: f32, n: u32) -> impl Iterator<Item = Vec3> {
    (0..n).map(move |i| {
        let a = TAU * i as f32 / n as f32;
        Vec3::new(r * a.cos(), r * a.sin(), z)
    })
}

/// Cylinder / cone / frustum. A radius of 0 collapses that end to an apex.
pub fn frustum(r_bottom: f32, r_top: f32, height: f32, segments: u32) -> Mesh {
    let n = segments.max(3);
    let hz = height * 0.5;
    let mut m = Mesh::default();
    let ring_or_apex = |m: &mut Mesh, r: f32, z: f32| -> Vec<u32> {
        let start = m.positions.len() as u32;
        if r <= 0.0 {
            m.positions.push(Vec3::new(0.0, 0.0, z));
            vec![start; n as usize]
        } else {
            m.positions.extend(ring(r, z, n));
            (start..start + n).collect()
        }
    };
    let b = ring_or_apex(&mut m, r_bottom, -hz);
    let t = ring_or_apex(&mut m, r_top, hz);
    for i in 0..n as usize {
        let j = (i + 1) % n as usize;
        if r_bottom > 0.0 {
            m.triangles.push([b[i], b[j], t[j]]);
        }
        if r_top > 0.0 {
            m.triangles.push([b[i], t[j], t[i]]);
        }
    }
    if r_bottom > 0.0 {
        let c = m.positions.len() as u32;
        m.positions.push(Vec3::new(0.0, 0.0, -hz));
        for i in 0..n as usize {
            m.triangles.push([c, b[(i + 1) % n as usize], b[i]]);
        }
    }
    if r_top > 0.0 {
        let c = m.positions.len() as u32;
        m.positions.push(Vec3::new(0.0, 0.0, hz));
        for i in 0..n as usize {
            m.triangles.push([c, t[i], t[(i + 1) % n as usize]]);
        }
    }
    m
}

pub fn sphere(radius: f32, segments: u32, rings: u32) -> Mesh {
    let n = segments.max(3);
    let rings = rings.max(2);
    let mut m = Mesh::default();
    m.positions.push(Vec3::new(0.0, 0.0, radius)); // north = 0
                                                   // ring k (1..rings) from top to bottom
    for k in 1..rings {
        let phi = std::f32::consts::PI * k as f32 / rings as f32;
        m.positions.extend(ring(radius * phi.sin(), radius * phi.cos(), n));
    }
    let south = m.positions.len() as u32;
    m.positions.push(Vec3::new(0.0, 0.0, -radius));
    let idx = |k: u32, i: u32| 1 + (k - 1) * n + (i % n);
    for i in 0..n {
        // top cap: ring 1 is "lower" relative to north
        m.triangles.push([idx(1, i), idx(1, i + 1), 0]);
        // bottom cap: last ring is "upper" relative to south
        m.triangles.push([south, idx(rings - 1, i + 1), idx(rings - 1, i)]);
    }
    for k in 1..rings - 1 {
        for i in 0..n {
            let (u0, u1, l0, l1) = (idx(k, i), idx(k, i + 1), idx(k + 1, i), idx(k + 1, i + 1));
            m.triangles.push([l0, l1, u1]);
            m.triangles.push([l0, u1, u0]);
        }
    }
    m
}

pub fn torus(major: f32, minor: f32, nmaj: u32, nmin: u32) -> Mesh {
    let (nmaj, nmin) = (nmaj.max(3), nmin.max(3));
    let mut m = Mesh::default();
    for i in 0..nmaj {
        let th = TAU * i as f32 / nmaj as f32;
        for j in 0..nmin {
            let ph = TAU * j as f32 / nmin as f32;
            let r = major + minor * ph.cos();
            m.positions.push(Vec3::new(r * th.cos(), r * th.sin(), minor * ph.sin()));
        }
    }
    let id = |i: u32, j: u32| (i % nmaj) * nmin + (j % nmin);
    for i in 0..nmaj {
        for j in 0..nmin {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            m.triangles.push([a, b, c]);
            m.triangles.push([a, c, d]);
        }
    }
    m
}

pub fn tube(r_out: f32, r_in: f32, height: f32, segments: u32) -> Mesh {
    let n = segments.max(3);
    let r_in = r_in.clamp(0.0, r_out * 0.999);
    if r_in <= 0.0 {
        return frustum(r_out, r_out, height, n);
    }
    let hz = height * 0.5;
    let mut m = Mesh::default();
    m.positions.extend(ring(r_out, -hz, n)); // ob 0..n
    m.positions.extend(ring(r_out, hz, n)); // ot n..2n
    m.positions.extend(ring(r_in, -hz, n)); // ib 2n..3n
    m.positions.extend(ring(r_in, hz, n)); // it 3n..4n
    for i in 0..n {
        let j = (i + 1) % n;
        let (ob, ot, ib, it) = (0, n, 2 * n, 3 * n);
        // outer wall (outward)
        m.triangles.push([ob + i, ob + j, ot + j]);
        m.triangles.push([ob + i, ot + j, ot + i]);
        // inner wall (facing the axis)
        m.triangles.push([ib + i, it + j, ib + j]);
        m.triangles.push([ib + i, it + i, it + j]);
        // top annulus (+Z)
        m.triangles.push([it + i, ot + i, ot + j]);
        m.triangles.push([it + i, ot + j, it + j]);
        // bottom annulus (-Z)
        m.triangles.push([ib + i, ob + j, ob + i]);
        m.triangles.push([ib + i, ib + j, ob + j]);
    }
    m
}

pub fn pyramid(base: f32, height: f32) -> Mesh {
    let mut m = frustum(base * std::f32::consts::FRAC_1_SQRT_2, 0.0, height, 4);
    // rotate 45° so the base edges align with X/Y
    m.transform(&glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4));
    m
}

/// Right-angle wedge: full footprint on the bottom, slope rising toward -X.
pub fn wedge(size: Vec3) -> Mesh {
    let h = size * 0.5;
    let p = vec![
        Vec3::new(-h.x, -h.y, -h.z), // 0
        Vec3::new(h.x, -h.y, -h.z),  // 1
        Vec3::new(h.x, h.y, -h.z),   // 2
        Vec3::new(-h.x, h.y, -h.z),  // 3
        Vec3::new(-h.x, -h.y, h.z),  // 4
        Vec3::new(-h.x, h.y, h.z),   // 5
    ];
    let t = vec![
        [0, 2, 1],
        [0, 3, 2], // bottom
        [0, 1, 4], // -y side
        [3, 5, 2], // +y side
        [3, 0, 4],
        [3, 4, 5], // -x back
        [1, 2, 5],
        [1, 5, 4], // slope
    ];
    Mesh::new(p, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    #[test]
    fn all_primitives_are_closed_and_outward() {
        for name in Primitive::ALL_NAMES {
            let m = Primitive::default_for(name).build();
            let a = analyze(&m);
            assert!(a.is_watertight(), "{name} not watertight: {a:?}");
            assert!(m.volume() > 0.0, "{name} inside out: {}", m.volume());
        }
    }

    #[test]
    fn beveled_box_volumes() {
        use std::f64::consts::PI;
        let (s, r) = (20.0f32, 3.0f32);
        let a = (s - 2.0 * r) as f64;
        let rr = r as f64;
        let fillet = beveled_box(Vec3::splat(s), r, 16, false);
        let exact = a.powi(3) + 6.0 * a * a * rr + 3.0 * PI * rr * rr * a + 4.0 / 3.0 * PI * rr.powi(3);
        assert!(analyze(&fillet).is_watertight(), "{:?}", analyze(&fillet));
        assert!((fillet.volume() - exact).abs() / exact < 0.003, "{} vs {exact}", fillet.volume());
        let ch = beveled_box(Vec3::splat(s), r, 1, true);
        // cube minus 12 edge prisms (r²/2 × a) minus 8 corner pieces (r³·(1/2 - 1/6)... ) — check bounds & closure
        assert!(analyze(&ch).is_watertight());
        let edge_cut = 12.0 * (rr * rr / 2.0) * a;
        let corner_cut = 8.0 * (rr.powi(3) - rr.powi(3) / 6.0);
        let expect = (s as f64).powi(3) - edge_cut - corner_cut;
        assert!((ch.volume() - expect).abs() < 1e-2, "{} vs {expect}", ch.volume());
        // fully rounded box (r = half size) is a sphere
        let ball = beveled_box(Vec3::splat(20.0), 10.0, 16, false);
        assert!(analyze(&ball).is_watertight(), "{:?}", analyze(&ball));
        assert!((ball.volume() - 4.0 / 3.0 * PI * 1000.0).abs() / 4188.0 < 0.01);
    }

    #[test]
    fn beveled_cylinder_ok() {
        for (e, k) in [(2.0, 8), (2.0, 1), (10.0, 8)] {
            let m = beveled_cylinder(10.0, 20.0, 64, e, k);
            assert!(analyze(&m).is_watertight(), "e={e} k={k}: {:?}", analyze(&m));
            assert!(m.volume() > 0.0);
            let b = m.bounds();
            assert!((b.max.z - 10.0).abs() < 1e-5 && (b.max.x - 10.0).abs() < 1e-3);
        }
    }

    #[test]
    fn volumes_close_to_analytic() {
        use std::f64::consts::PI;
        let s = sphere(10.0, 128, 64).volume();
        assert!((s - 4.0 / 3.0 * PI * 1000.0).abs() / s < 0.01);
        let c = frustum(10.0, 10.0, 20.0, 256).volume();
        assert!((c - PI * 100.0 * 20.0).abs() / c < 0.01);
        let t = torus(15.0, 5.0, 256, 128).volume();
        assert!((t - 2.0 * PI * PI * 15.0 * 25.0).abs() / t < 0.01);
        let w = wedge(Vec3::splat(10.0)).volume();
        assert!((w - 500.0).abs() < 1e-3);
    }
}
