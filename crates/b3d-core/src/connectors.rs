//! Cut with connectors: after a plane cut, add pegs (one half) with matching holes (other
//! half), or holes in both halves plus separate dowel pins, so printed halves line up.
//! Connectors are placed automatically inside the cut section, away from its edges.

use crate::csg::{boolean_many, cut_by_plane, BoolOp};
use crate::mesh::Mesh;
use crate::primitives::frustum;
use crate::{Progress, Result};
use glam::{Mat4, Quat, Vec2, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    None,
    /// Pegs on the lower half, holes in the upper half.
    Pegs,
    /// Holes in both halves and separate dowel pins.
    Dowels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectorOptions {
    pub kind: ConnectorKind,
    pub diameter: f32,
    /// How far a peg / dowel goes into each half.
    pub depth: f32,
    /// Radial play in the holes.
    pub clearance: f32,
    /// 0 = as many as fit (up to 4).
    pub count: u32,
}

impl Default for ConnectorOptions {
    fn default() -> Self {
        ConnectorOptions { kind: ConnectorKind::None, diameter: 5.0, depth: 6.0, clearance: 0.2, count: 0 }
    }
}

pub struct CutResult {
    /// Part on the +normal side.
    pub above: Mesh,
    pub below: Mesh,
    /// Separate dowel pins (standing on z = 0, centred on the origin in X/Y).
    pub dowels: Vec<Mesh>,
    /// How many connectors were placed (may be fewer than asked if they don't fit).
    pub placed: usize,
}

/// Orthonormal basis (u, v) of the plane with normal n.
fn basis(n: Vec3) -> (Vec3, Vec3) {
    let u = n.any_orthonormal_vector();
    (u, n.cross(u))
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn in_tri(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = (b - a).perp_dot(p - a);
    let d2 = (c - b).perp_dot(p - b);
    let d3 = (a - c).perp_dot(p - c);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Spots for connectors inside the section: as far from the section's edges and from each
/// other as possible. `need` = minimum distance to an edge.
fn place(below: &Mesh, point: Vec3, n: Vec3, need: f32, spacing: f32, count: u32) -> Vec<Vec3> {
    let (u, v) = basis(n);
    let to2 = |p: Vec3| Vec2::new((p - point).dot(u), (p - point).dot(v));
    // the cap of the lower half: triangles on the plane facing +n
    let mut tris: Vec<[Vec2; 3]> = vec![];
    let mut edges: std::collections::HashMap<(u32, u32), (Vec2, Vec2, i32)> = Default::default();
    for (i, t) in below.triangles.iter().enumerate() {
        let [a, b, c] = below.tri(i);
        let on = |p: Vec3| (p - point).dot(n).abs() < 1e-3;
        if on(a) && on(b) && on(c) && below.face_normal(i).dot(n) > 0.99 {
            tris.push([to2(a), to2(b), to2(c)]);
            for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let k = (x.min(y), x.max(y));
                let e = edges.entry(k).or_insert((to2(below.positions[x as usize]), to2(below.positions[y as usize]), 0));
                e.2 += 1;
            }
        }
    }
    let boundary: Vec<(Vec2, Vec2)> = edges.values().filter(|e| e.2 == 1).map(|e| (e.0, e.1)).collect();
    if tris.is_empty() || boundary.is_empty() {
        return vec![];
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for t in &tris {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    // candidate grid inside the section with each sample's distance to the edge
    let steps = 64;
    let mut cands: Vec<(Vec2, f32)> = vec![];
    for j in 0..=steps {
        for i in 0..=steps {
            let p = lo + (hi - lo) * Vec2::new(i as f32 / steps as f32, j as f32 / steps as f32);
            if !tris.iter().any(|t| in_tri(p, t[0], t[1], t[2])) {
                continue;
            }
            let d = boundary.iter().map(|(a, b)| seg_dist(p, *a, *b)).fold(f32::MAX, f32::min);
            if d >= need {
                cands.push((p, d));
            }
        }
    }
    let max = if count == 0 { 4 } else { count as usize };
    let mut chosen: Vec<Vec2> = vec![];
    // first: the roomiest spot; then the spot farthest from those chosen
    while chosen.len() < max {
        let best = cands
            .iter()
            .filter(|(p, _)| chosen.iter().all(|c| c.distance(*p) >= spacing))
            .max_by(|a, b| {
                let score = |(p, d): &(Vec2, f32)| {
                    if chosen.is_empty() {
                        *d
                    } else {
                        chosen.iter().map(|c| c.distance(*p)).fold(f32::MAX, f32::min) + d * 0.5
                    }
                };
                score(a).total_cmp(&score(b))
            })
            .map(|c| c.0);
        match best {
            Some(p) => chosen.push(p),
            None => break,
        }
        // auto mode: stop at one connector for small sections
        if count == 0 && chosen.len() == 1 && (hi - lo).max_element() < spacing * 2.5 {
            break;
        }
    }
    chosen.into_iter().map(|p| point + u * p.x + v * p.y).collect()
}

/// Cylinder of radius r along `n`, from `a` (distance along n from p) to `b`.
fn rod(p: Vec3, n: Vec3, r: f32, a: f32, b: f32) -> Mesh {
    let mut m = frustum(r, r, b - a, 64);
    m.transform(&(Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Z, n), p + n * (a + b) * 0.5)));
    m
}

/// Plane cut with optional connectors.
pub fn cut_with_connectors(m: &Mesh, point: Vec3, normal: Vec3, o: &ConnectorOptions, progress: &Progress) -> Result<CutResult> {
    let n = normal.normalize_or(Vec3::Z);
    let (above, below) = cut_by_plane(m, point, n)?;
    let mut r = CutResult { above, below, dowels: vec![], placed: 0 };
    if o.kind == ConnectorKind::None || r.above.is_empty() || r.below.is_empty() {
        return Ok(r);
    }
    progress.check()?;
    let rad = o.diameter.max(0.5) * 0.5;
    let need = rad + o.clearance + 1.2; // keep a wall around the hole
    let spots = place(&r.below, point, n, need, rad * 4.0 + 2.0, o.count);
    r.placed = spots.len();
    if spots.is_empty() {
        return Ok(r);
    }
    let hole_r = rad + o.clearance;
    let hole_d = o.depth + o.clearance * 2.0;
    match o.kind {
        ConnectorKind::Pegs => {
            // pegs reach a little into the lower half so the union is solid
            let pegs: Vec<Mesh> = spots.iter().map(|p| rod(*p, n, rad, -0.5, o.depth)).collect();
            let holes: Vec<Mesh> = spots.iter().map(|p| rod(*p, n, hole_r, -0.01, hole_d)).collect();
            r.below = boolean_many(&r.below, &pegs, BoolOp::Union)?;
            progress.set(0.6);
            r.above = boolean_many(&r.above, &holes, BoolOp::Subtract)?;
        }
        ConnectorKind::Dowels => {
            let up: Vec<Mesh> = spots.iter().map(|p| rod(*p, n, hole_r, -0.01, hole_d)).collect();
            let down: Vec<Mesh> = spots.iter().map(|p| rod(*p, n, hole_r, -hole_d, 0.01)).collect();
            r.above = boolean_many(&r.above, &up, BoolOp::Subtract)?;
            progress.set(0.6);
            r.below = boolean_many(&r.below, &down, BoolOp::Subtract)?;
            // the pins: a bit shorter than the two holes, with chamfered ends
            let len = (2.0 * o.depth - 0.4).max(1.0);
            for _ in &spots {
                let cham = (rad * 0.25).min(0.6);
                let mut pin = crate::primitives::beveled_cylinder(rad, len, 64, cham, 1);
                let b = pin.bounds();
                pin.translate(Vec3::new(0.0, 0.0, -b.min.z));
                r.dowels.push(pin);
            }
        }
        ConnectorKind::None => {}
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    fn block() -> Mesh {
        let mut m = cuboid(Vec3::new(40.0, 30.0, 20.0));
        m.translate(Vec3::new(0.0, 0.0, 10.0));
        m
    }

    #[test]
    fn pegs_match_holes() {
        let o = ConnectorOptions { kind: ConnectorKind::Pegs, diameter: 5.0, depth: 6.0, clearance: 0.2, count: 2 };
        let r = cut_with_connectors(&block(), Vec3::new(0.0, 0.0, 10.0), Vec3::Z, &o, &Progress::new()).unwrap();
        assert_eq!(r.placed, 2);
        assert!(analyze(&r.above).is_watertight() && analyze(&r.below).is_watertight());
        let peg = std::f64::consts::PI * 2.5f64.powi(2) * 6.0;
        let hole = std::f64::consts::PI * 2.7f64.powi(2) * 6.4;
        let half = 40.0 * 30.0 * 10.0;
        assert!((r.below.volume() - (half + 2.0 * peg)).abs() < 2.0, "{}", r.below.volume());
        assert!((r.above.volume() - (half - 2.0 * hole)).abs() < 2.0, "{}", r.above.volume());
        // pegs stick up 6 mm above the cut
        assert!((r.below.bounds().max.z - 16.0).abs() < 1e-3);
    }

    #[test]
    fn dowels_and_auto_count() {
        let o = ConnectorOptions { kind: ConnectorKind::Dowels, ..Default::default() };
        let r = cut_with_connectors(&block(), Vec3::new(0.0, 0.0, 10.0), Vec3::Z, &o, &Progress::new()).unwrap();
        assert!(r.placed >= 2 && r.placed <= 4, "{}", r.placed);
        assert_eq!(r.dowels.len(), r.placed);
        for d in &r.dowels {
            assert!(analyze(d).is_watertight());
            assert!((d.bounds().size().z - 11.6).abs() < 1e-3);
        }
        assert!(analyze(&r.above).is_watertight() && analyze(&r.below).is_watertight());
    }

    #[test]
    fn too_thin_for_connectors() {
        let mut m = cuboid(Vec3::new(40.0, 4.0, 20.0));
        m.translate(Vec3::new(0.0, 0.0, 10.0));
        let o = ConnectorOptions { kind: ConnectorKind::Pegs, ..Default::default() };
        let r = cut_with_connectors(&m, Vec3::new(0.0, 0.0, 10.0), Vec3::Z, &o, &Progress::new()).unwrap();
        assert_eq!(r.placed, 0);
        assert!(!r.above.is_empty() && !r.below.is_empty());
    }
}
