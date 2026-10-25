//! Sketches: flat shapes drawn on a plane (rectangles, circles, polygons and outlines
//! made of straight and curved segments), turned into solids by extruding or revolving
//! them. Shapes marked as holes are cut out of the others; overlapping shapes merge.
//!
//! Sketch space is the XY plane of the sketch; extrusions rise along +Z.

use crate::csg::{boolean_many, BoolOp};
use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::{Vec2, Vec3};
use std::f32::consts::TAU;

/// Largest gap between a curve and the straight segments that stand in for it, mm.
const TOL: f32 = 0.02;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Shape {
    /// Opposite corners (any order); the corners are rounded by `radius`.
    Rect { a: Vec2, b: Vec2, radius: f32 },
    Circle { c: Vec2, r: f32 },
    /// Regular polygon: centre, radius to the corners, number of sides and the
    /// direction of the first corner (radians).
    Polygon { c: Vec2, r: f32, sides: u32, angle: f32 },
    /// A closed outline through `pts`. Segment i runs from `pts[i]` to the next point
    /// (the last one back to the first): straight, or an arc through `bulge[i]`.
    Outline { pts: Vec<Vec2>, bulge: Vec<Option<Vec2>> },
}

/// A shape of a sketch: solid, or a hole cut out of the solid shapes.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub shape: Shape,
    pub hole: bool,
}

/// Number of straight segments for an arc of radius `r` turning by `sweep` radians.
fn arc_segments(r: f32, sweep: f32) -> usize {
    let r = r.abs().max(1e-3);
    let step = (2.0 * (1.0 - (TOL / r).min(1.0)).acos()).max(TAU / 256.0);
    let sweep = sweep.abs();
    let n = (sweep / step).ceil().max((sweep / (TAU / 24.0)).ceil());
    (n as usize).clamp(1, 1024)
}

fn signed_area(c: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..c.len() {
        let (p, q) = (c[i], c[(i + 1) % c.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// The circle through three points: (centre, radius, start angle, signed sweep from `a`
/// to `b` passing `m`). None when the points are (nearly) in a line.
pub fn arc_through(a: Vec2, m: Vec2, b: Vec2) -> Option<(Vec2, f32, f32, f32)> {
    let d = 2.0 * (a.x * (m.y - b.y) + m.x * (b.y - a.y) + b.x * (a.y - m.y));
    let span = a.distance(b).max(a.distance(m)).max(m.distance(b));
    if d.abs() < 1e-6 * span * span || span < 1e-6 {
        return None;
    }
    let (a2, m2, b2) = (a.length_squared(), m.length_squared(), b.length_squared());
    let c = Vec2::new(
        (a2 * (m.y - b.y) + m2 * (b.y - a.y) + b2 * (a.y - m.y)) / d,
        (a2 * (b.x - m.x) + m2 * (a.x - b.x) + b2 * (m.x - a.x)) / d,
    );
    let r = c.distance(a);
    // a near-straight "arc" with a huge radius: treat it as a line
    if r > span * 1e4 {
        return None;
    }
    let ang = |p: Vec2| (p.y - c.y).atan2(p.x - c.x);
    let (ta, tm, tb) = (ang(a), ang(m), ang(b));
    let ccw = (tb - ta).rem_euclid(TAU);
    let mid = (tm - ta).rem_euclid(TAU);
    let sweep = if mid <= ccw { ccw } else { ccw - TAU };
    Some((c, r, ta, sweep))
}

/// Points along the arc from `a` through `m` to `b`, without `a`, ending with `b`.
pub fn arc_points(a: Vec2, m: Vec2, b: Vec2) -> Vec<Vec2> {
    let Some((c, r, t0, sweep)) = arc_through(a, m, b) else { return vec![b] };
    let n = arc_segments(r, sweep).max(2);
    let mut out: Vec<Vec2> = (1..n).map(|k| c + Vec2::from_angle(t0 + sweep * k as f32 / n as f32) * r).collect();
    out.push(b);
    out
}

/// Drop repeated points (also between the last and the first).
fn dedup(mut c: Vec<Vec2>) -> Vec<Vec2> {
    let eps2 = 1e-10_f32.max(c.iter().fold(0.0_f32, |m, p| m.max(p.abs().max_element())) * 1e-6).powi(2);
    c.dedup_by(|a, b| a.distance_squared(*b) <= eps2);
    while c.len() > 1 && c[0].distance_squared(*c.last().unwrap()) <= eps2 {
        c.pop();
    }
    c
}

fn segments_cross(p1: Vec2, p2: Vec2, q1: Vec2, q2: Vec2) -> bool {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p - o).perp_dot(q - o);
    let (d1, d2) = (cross(q1, q2, p1), cross(q1, q2, p2));
    let (d3, d4) = (cross(p1, p2, q1), cross(p1, p2, q2));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0
}

/// Whether a closed polygon crosses itself.
pub fn self_intersects(c: &[Vec2]) -> bool {
    let n = c.len();
    if n < 4 {
        return false;
    }
    for i in 0..n {
        let (p1, p2) = (c[i], c[(i + 1) % n]);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // neighbours through the closing segment
            }
            if segments_cross(p1, p2, c[j], c[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

impl Shape {
    pub fn name(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "Rectangle",
            Shape::Circle { .. } => "Circle",
            Shape::Polygon { .. } => "Polygon",
            Shape::Outline { .. } => "Outline",
        }
    }

    /// The closed outline, counter-clockwise, curves as short straight segments.
    pub fn outline(&self) -> Vec<Vec2> {
        let mut c = match self {
            Shape::Rect { a, b, radius } => {
                let (lo, hi) = (a.min(*b), a.max(*b));
                let (w, h) = (hi.x - lo.x, hi.y - lo.y);
                let r = radius.clamp(0.0, (w.min(h) * 0.5).max(0.0));
                if r < 1e-4 {
                    vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)]
                } else {
                    let n = arc_segments(r, TAU / 4.0).max(2);
                    let corners = [
                        (Vec2::new(hi.x - r, lo.y + r), -TAU / 4.0),
                        (Vec2::new(hi.x - r, hi.y - r), 0.0),
                        (Vec2::new(lo.x + r, hi.y - r), TAU / 4.0),
                        (Vec2::new(lo.x + r, lo.y + r), TAU / 2.0),
                    ];
                    let mut pts = vec![];
                    for (c, t0) in corners {
                        for k in 0..=n {
                            pts.push(c + Vec2::from_angle(t0 + TAU / 4.0 * k as f32 / n as f32) * r);
                        }
                    }
                    pts
                }
            }
            Shape::Circle { c, r } => {
                let n = arc_segments(*r, TAU).max(8);
                (0..n).map(|k| *c + Vec2::from_angle(TAU * k as f32 / n as f32) * r.abs()).collect()
            }
            Shape::Polygon { c, r, sides, angle } => {
                let s = (*sides).clamp(3, 64);
                (0..s).map(|k| *c + Vec2::from_angle(angle + TAU * k as f32 / s as f32) * r.abs()).collect()
            }
            Shape::Outline { pts, bulge } => {
                let n = pts.len();
                let mut out = vec![];
                for i in 0..n {
                    out.push(pts[i]);
                    if let Some(Some(m)) = bulge.get(i) {
                        let mut arc = arc_points(pts[i], *m, pts[(i + 1) % n]);
                        arc.pop(); // the next point comes next
                        out.extend(arc);
                    }
                }
                out
            }
        };
        c = dedup(c);
        if signed_area(&c) < 0.0 {
            c.reverse();
        }
        c
    }

    /// Why this shape can't be made into a solid, if it can't.
    pub fn problem(&self) -> Option<&'static str> {
        let o = self.outline();
        if o.len() < 3 || signed_area(&o).abs() < 1e-4 {
            return Some("it has no area");
        }
        if matches!(self, Shape::Outline { .. }) && self_intersects(&o) {
            return Some("the outline crosses itself");
        }
        None
    }

    pub fn translate(&mut self, d: Vec2) {
        match self {
            Shape::Rect { a, b, .. } => {
                *a += d;
                *b += d;
            }
            Shape::Circle { c, .. } | Shape::Polygon { c, .. } => *c += d,
            Shape::Outline { pts, bulge } => {
                for p in pts.iter_mut() {
                    *p += d;
                }
                for m in bulge.iter_mut().flatten() {
                    *m += d;
                }
            }
        }
    }

    /// (min, max) corners of the outline.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        let o = self.outline();
        let lo = o.iter().fold(Vec2::splat(f32::INFINITY), |a, p| a.min(*p));
        let hi = o.iter().fold(Vec2::splat(f32::NEG_INFINITY), |a, p| a.max(*p));
        (lo, hi)
    }

    pub fn center(&self) -> Vec2 {
        match self {
            Shape::Circle { c, .. } | Shape::Polygon { c, .. } => *c,
            _ => {
                let (lo, hi) = self.bounds();
                (lo + hi) * 0.5
            }
        }
    }

    /// Points other shapes snap to: corners, centres, quadrants, midpoints.
    pub fn snap_points(&self) -> Vec<Vec2> {
        let mut v = vec![self.center()];
        match self {
            Shape::Rect { a, b, .. } => {
                let (lo, hi) = (a.min(*b), a.max(*b));
                let m = (lo + hi) * 0.5;
                v.extend([lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)]);
                v.extend([Vec2::new(m.x, lo.y), Vec2::new(hi.x, m.y), Vec2::new(m.x, hi.y), Vec2::new(lo.x, m.y)]);
            }
            Shape::Circle { c, r } => v.extend([Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y].map(|d| *c + d * *r)),
            Shape::Polygon { .. } => v.extend(self.outline()),
            Shape::Outline { pts, .. } => v.extend(pts.iter().copied()),
        }
        v
    }

    /// Distance from `p` to the outline.
    pub fn distance_to(&self, p: Vec2) -> f32 {
        let o = self.outline();
        let n = o.len();
        (0..n)
            .map(|i| {
                let (a, b) = (o[i], o[(i + 1) % n]);
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                p.distance(a + ab * t)
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// Whether `p` is inside the outline.
    pub fn contains(&self, p: Vec2) -> bool {
        let poly = self.outline();
        let mut inside = false;
        let mut j = poly.len().saturating_sub(1);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[j]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// Triangles filling a closed outline (indices into it), for drawing.
pub fn triangulate(outline: &[Vec2]) -> Vec<[usize; 3]> {
    let flat: Vec<f64> = outline.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
    earcutr::earcut(&flat, &[], 2).map(|t| t.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()).unwrap_or_default()
}

/// Every shape's outline must make a solid, and at least one shape must be solid.
fn check(items: &[Item]) -> Result<()> {
    if items.is_empty() {
        return Err(Error::Parse("draw a shape first".into()));
    }
    for it in items {
        if let Some(p) = it.shape.problem() {
            return Err(Error::Parse(format!("{}: {p}", it.shape.name().to_lowercase())));
        }
    }
    if items.iter().all(|i| i.hole) {
        return Err(Error::Parse("every shape is a hole: draw a solid shape too".into()));
    }
    Ok(())
}

/// Solids merged, then the holes cut out.
fn combine(mut solids: Vec<Mesh>, holes: Vec<Mesh>) -> Result<Mesh> {
    if solids.is_empty() {
        return Err(Error::Parse("every shape is a hole: draw a solid shape too".into()));
    }
    let first = solids.remove(0);
    let mut m = if solids.is_empty() { first } else { boolean_many(&first, &solids, BoolOp::Union)? };
    if !holes.is_empty() {
        m = boolean_many(&m, &holes, BoolOp::Subtract)?;
    }
    if m.is_empty() {
        return Err(Error::Parse("nothing is left: the holes cover the whole sketch".into()));
    }
    Ok(m)
}

/// The sketch pushed up into a solid from z = 0 to z = `depth`.
pub fn extrude(items: &[Item], depth: f32) -> Result<Mesh> {
    check(items)?;
    let d = depth.abs().max(0.01);
    let (mut solids, mut holes) = (vec![], vec![]);
    for it in items {
        let o = it.shape.outline();
        if it.hole {
            // a little longer, so no faces of the cut lie exactly on the solid's
            let mut m = crate::text::extrude_contours(vec![o], d + 2.0)?;
            m.translate(Vec3::Z * -1.0);
            holes.push(m);
        } else {
            solids.push(crate::text::extrude_contours(vec![o], d)?);
        }
    }
    combine(solids, holes)
}

/// Keep the part of a polygon with x ≥ 0 (points on the line count as inside).
fn clip_right(poly: &[Vec2]) -> Vec<Vec2> {
    let n = poly.len();
    let mut out = vec![];
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let (pin, qin) = (p.x >= 0.0, q.x >= 0.0);
        if pin {
            out.push(p);
        }
        if pin != qin {
            let t = p.x / (p.x - q.x);
            out.push(Vec2::new(0.0, p.y + (q.y - p.y) * t));
        }
    }
    dedup(out)
}

/// One closed (r, z) outline (r ≥ 0) turned around the Z axis from `start` by `sweep`
/// radians; a sweep short of a full turn gets flat ends.
fn revolve_outline(c: &[Vec2], start: f32, sweep: f32) -> Result<Mesh> {
    let mut c = c.to_vec();
    if signed_area(&c) < 0.0 {
        c.reverse();
    }
    let full = sweep >= TAU - 1e-4;
    let rmax = c.iter().fold(0.0_f32, |m, p| m.max(p.x));
    let steps = arc_segments(rmax, sweep).max(if full { 8 } else { 2 });
    let rings = if full { steps } else { steps + 1 };
    let eps = 1e-5 * rmax.max(1.0);
    let mut pos: Vec<Vec3> = vec![];
    let mut idx: Vec<Vec<u32>> = vec![];
    for p in &c {
        if p.x <= eps {
            // on the axis: one point (a pole)
            let v = pos.len() as u32;
            pos.push(Vec3::new(0.0, 0.0, p.y));
            idx.push(vec![v; rings]);
        } else {
            let base = pos.len() as u32;
            for s in 0..rings {
                let t = start + sweep * s as f32 / steps as f32;
                pos.push(Vec3::new(p.x * t.cos(), p.x * t.sin(), p.y));
            }
            idx.push((base..base + rings as u32).collect());
        }
    }
    let n = c.len();
    let mut tris: Vec<[u32; 3]> = vec![];
    let mut push = |t: [u32; 3]| {
        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
            tris.push(t);
        }
    };
    for i in 0..n {
        let j = (i + 1) % n;
        for s in 0..steps {
            let t = if full { (s + 1) % steps } else { s + 1 };
            let (a, b, cc, d) = (idx[i][s], idx[j][s], idx[j][t], idx[i][t]);
            push([a, cc, b]);
            push([a, d, cc]);
        }
    }
    if !full {
        // flat ends: the outline itself at the start and end angles
        let flat: Vec<f64> = c.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
        let caps = earcutr::earcut(&flat, &[], 2).map_err(|e| Error::Parse(format!("revolve: {e:?}")))?;
        for t in caps.chunks_exact(3) {
            let (i, mut j, mut k) = (t[0], t[1], t[2]);
            if (c[j] - c[i]).perp_dot(c[k] - c[i]) < 0.0 {
                std::mem::swap(&mut j, &mut k);
            }
            push([idx[i][0], idx[j][0], idx[k][0]]);
            push([idx[i][steps], idx[k][steps], idx[j][steps]]);
        }
    }
    let mut m = Mesh::new(pos, tris);
    if m.volume() < 0.0 {
        m.flip();
    }
    Ok(m)
}

/// Which side of the line x = `axis_x` holds most of the solid shapes: true = right.
fn right_side(items: &[Item], axis_x: f32) -> bool {
    let mut right = 0.0;
    let mut left = 0.0;
    for it in items.iter().filter(|i| !i.hole) {
        let o: Vec<Vec2> = it.shape.outline().iter().map(|p| Vec2::new(p.x - axis_x, p.y)).collect();
        right += signed_area(&clip_right(&o)).abs();
        let m: Vec<Vec2> = o.iter().map(|p| Vec2::new(-p.x, p.y)).collect();
        left += signed_area(&clip_right(&m)).abs();
    }
    right >= left
}

/// The sketch turned around the line x = `axis_x` (along the sketch's Y) by `angle_deg`
/// degrees. The result stands upright: the axis is Z through the origin, sketch Y
/// becomes height. Only the side of the axis holding most of the sketch is used.
pub fn revolve(items: &[Item], axis_x: f32, angle_deg: f32) -> Result<Mesh> {
    check(items)?;
    let sweep = angle_deg.abs().clamp(1.0, 360.0).to_radians();
    let right = right_side(items, axis_x);
    let to_rz = |o: Vec<Vec2>| -> Vec<Vec2> {
        let o: Vec<Vec2> = o.iter().map(|p| Vec2::new(if right { p.x - axis_x } else { axis_x - p.x }, p.y)).collect();
        clip_right(&o)
    };
    let (mut solids, mut holes) = (vec![], vec![]);
    for it in items {
        let rz = to_rz(it.shape.outline());
        if rz.len() < 3 || signed_area(&rz).abs() < 1e-6 {
            continue;
        }
        if it.hole {
            // a touch wider than the solids, so the cut's ends don't lie on theirs
            let extra = if sweep >= TAU - 1e-4 { 0.0 } else { 0.5_f32.to_radians() };
            holes.push(revolve_outline(&rz, -extra, (sweep + 2.0 * extra).min(TAU))?);
        } else {
            solids.push(revolve_outline(&rz, 0.0, sweep)?);
        }
    }
    if solids.is_empty() {
        return Err(Error::Parse("nothing to turn on this side of the axis".into()));
    }
    combine(solids, holes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    fn item(shape: Shape) -> Item {
        Item { shape, hole: false }
    }
    fn hole(shape: Shape) -> Item {
        Item { shape, hole: true }
    }
    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= b.abs() * rel
    }

    #[test]
    fn arcs_go_through_the_middle_point() {
        // a half circle above the X axis, and one below
        let (c, r, _, sweep) = arc_through(Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)).unwrap();
        assert!(c.length() < 1e-5 && (r - 1.0).abs() < 1e-5);
        assert!((sweep + std::f32::consts::PI).abs() < 1e-4, "clockwise over the top: {sweep}");
        let pts = arc_points(Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0));
        assert!(pts.iter().all(|p| p.y <= 1e-5));
        assert_eq!(*pts.last().unwrap(), Vec2::new(1.0, 0.0));
        assert!(arc_through(Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)).is_none());
    }

    #[test]
    fn rectangle_extrudes_to_a_box() {
        let m = extrude(&[item(Shape::Rect { a: Vec2::new(0.0, 0.0), b: Vec2::new(20.0, 10.0), radius: 0.0 })], 5.0).unwrap();
        assert!(analyze(&m).is_watertight());
        assert!(close(m.volume(), 1000.0, 1e-4));
        let b = m.bounds();
        assert!((b.max.z - 5.0).abs() < 1e-5 && b.min.z.abs() < 1e-5);
    }

    #[test]
    fn rounded_rectangle_and_slot() {
        let r = extrude(&[item(Shape::Rect { a: Vec2::ZERO, b: Vec2::new(20.0, 10.0), radius: 2.0 })], 1.0).unwrap();
        let want = 200.0 - (4.0 - std::f64::consts::PI) * 4.0;
        assert!(analyze(&r).is_watertight() && close(r.volume(), want, 5e-3), "{}", r.volume());
        // radius = half the height: a slot
        let s = extrude(&[item(Shape::Rect { a: Vec2::ZERO, b: Vec2::new(20.0, 10.0), radius: 5.0 })], 1.0).unwrap();
        let want = 100.0 + std::f64::consts::PI * 25.0;
        assert!(analyze(&s).is_watertight() && close(s.volume(), want, 5e-3), "{}", s.volume());
    }

    #[test]
    fn circle_with_a_hole_and_an_overlap() {
        let items = [
            item(Shape::Circle { c: Vec2::ZERO, r: 10.0 }),
            hole(Shape::Circle { c: Vec2::ZERO, r: 4.0 }),
            // a tab sticking out of the ring, merged with it
            item(Shape::Rect { a: Vec2::new(8.0, -2.0), b: Vec2::new(15.0, 2.0), radius: 0.0 }),
        ];
        let m = extrude(&items, 3.0).unwrap();
        assert!(analyze(&m).is_watertight());
        assert_eq!(m.connected_components().len(), 1);
        let ring = std::f64::consts::PI * (100.0 - 16.0) * 3.0;
        assert!(m.volume() > ring + 7.0 * 4.0 * 3.0 * 0.6 && m.volume() < ring + 7.0 * 4.0 * 3.0, "{}", m.volume());
    }

    #[test]
    fn outline_with_an_arc() {
        // a D shape: flat side on the left, round side on the right
        let pts = vec![Vec2::new(0.0, -10.0), Vec2::new(0.0, 10.0)];
        let shape = Shape::Outline { pts, bulge: vec![None, Some(Vec2::new(10.0, 0.0))] };
        assert!(shape.problem().is_none());
        let m = extrude(&[item(shape)], 2.0).unwrap();
        assert!(analyze(&m).is_watertight());
        assert!(close(m.volume(), std::f64::consts::PI * 50.0 * 2.0, 5e-3), "{}", m.volume());
    }

    #[test]
    fn crossing_outline_is_refused() {
        let pts = vec![Vec2::new(0.0, 0.0), Vec2::new(20.0, 10.0), Vec2::new(20.0, 0.0), Vec2::new(0.0, 4.0)];
        let shape = Shape::Outline { pts, bulge: vec![None; 4] };
        assert_eq!(shape.problem(), Some("the outline crosses itself"));
        assert!(extrude(&[item(shape)], 1.0).is_err());
        assert!(extrude(&[hole(Shape::Circle { c: Vec2::ZERO, r: 1.0 })], 1.0).is_err());
    }

    #[test]
    fn revolve_makes_a_ring_and_a_cylinder() {
        // a rectangle 5..8 from the axis: a tube 10 tall
        let tube = revolve(&[item(Shape::Rect { a: Vec2::new(5.0, 0.0), b: Vec2::new(8.0, 10.0), radius: 0.0 })], 0.0, 360.0).unwrap();
        assert!(analyze(&tube).is_watertight());
        let want = std::f64::consts::PI * (64.0 - 25.0) * 10.0;
        assert!(close(tube.volume(), want, 5e-3), "{} vs {want}", tube.volume());
        let b = tube.bounds();
        assert!((b.max.z - 10.0).abs() < 1e-4 && (b.max.x - 8.0).abs() < 1e-3);
        // touching the axis: a solid cylinder (poles, no hole in the middle)
        let cyl = revolve(&[item(Shape::Rect { a: Vec2::new(2.0, 0.0), b: Vec2::new(6.0, 4.0), radius: 0.0 })], 2.0, 360.0).unwrap();
        assert!(analyze(&cyl).is_watertight());
        assert!(close(cyl.volume(), std::f64::consts::PI * 16.0 * 4.0, 1e-2), "{}", cyl.volume());
        // a shape crossing the axis uses the bigger side: here the right
        let x = revolve(&[item(Shape::Rect { a: Vec2::new(-1.0, 0.0), b: Vec2::new(4.0, 2.0), radius: 0.0 })], 0.0, 360.0).unwrap();
        assert!(analyze(&x).is_watertight());
        assert!(close(x.volume(), std::f64::consts::PI * 16.0 * 2.0, 1e-2), "{}", x.volume());
    }

    #[test]
    fn partial_revolve_has_ends() {
        let m = revolve(&[item(Shape::Rect { a: Vec2::new(5.0, 0.0), b: Vec2::new(8.0, 10.0), radius: 0.0 })], 0.0, 90.0).unwrap();
        assert!(analyze(&m).is_watertight());
        let want = std::f64::consts::PI * (64.0 - 25.0) * 10.0 / 4.0;
        assert!(close(m.volume(), want, 5e-3), "{} vs {want}", m.volume());
        // with a round hole through the wall
        let items = [
            item(Shape::Rect { a: Vec2::new(5.0, 0.0), b: Vec2::new(10.0, 10.0), radius: 0.0 }),
            hole(Shape::Circle { c: Vec2::new(7.5, 5.0), r: 1.5 }),
        ];
        let h = revolve(&items, 0.0, 180.0).unwrap();
        assert!(analyze(&h).is_watertight());
        let solid = std::f64::consts::PI * (100.0 - 25.0) * 10.0 / 2.0;
        assert!(h.volume() < solid * 0.95 && h.volume() > solid * 0.8, "{}", h.volume());
    }
}
