//! Mechanical part generators: involute spur gears, compression springs, print-in-place
//! hinges and cantilever snap-fit clips.

use crate::csg::{boolean, boolean_many, cut_by_plane, BoolOp};
use crate::mesh::Mesh;
use crate::primitives::{cuboid, frustum};
use crate::text::extrude_contours;
use glam::{Mat4, Vec2, Vec3};
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gear {
    pub teeth: u32,
    /// Module (tooth size), mm: pitch diameter = module × teeth.
    pub module: f32,
    pub thickness: f32,
    pub pressure_deg: f32,
    /// Centre hole diameter (0 = none).
    pub bore: f32,
    /// Play between meshing teeth, mm (thins each tooth by half of it).
    pub backlash: f32,
}

impl Default for Gear {
    fn default() -> Self {
        Gear { teeth: 20, module: 1.5, thickness: 6.0, pressure_deg: 20.0, bore: 5.0, backlash: 0.15 }
    }
}

fn inv(a: f32) -> f32 {
    a.tan() - a
}

fn polar(r: f32, a: f32) -> Vec2 {
    Vec2::new(r * a.cos(), r * a.sin())
}

/// The gear's outline (one closed polygon, counter-clockwise).
pub fn gear_outline(g: &Gear) -> Vec<Vec2> {
    let z = g.teeth.max(6) as f32;
    let m = g.module.max(0.1);
    let alpha = g.pressure_deg.clamp(10.0, 35.0).to_radians();
    let rp = m * z * 0.5;
    let rb = rp * alpha.cos();
    let ra = rp + m;
    let rd = (rp - 1.25 * m).max(0.5);
    let half = PI / (2.0 * z) - g.backlash / (4.0 * rp);
    // angle of the tooth's flank (from the tooth centre) at radius r
    let flank = |r: f32| -> f32 {
        let r = r.max(rb);
        (half + inv(alpha) - inv((rb / r).clamp(-1.0, 1.0).acos())).max(0.002)
    };
    let steps = 8;
    let mut pts = vec![];
    for k in 0..g.teeth.max(6) {
        let c = TAU * k as f32 / z;
        let r0 = rb.max(rd);
        // root up to the base circle, then the involute out to the tip
        if rd < rb {
            pts.push(polar(rd, c - flank(rb)));
        }
        for i in 0..=steps {
            let r = r0 + (ra - r0) * i as f32 / steps as f32;
            pts.push(polar(r, c - flank(r)));
        }
        // tip
        let (a0, a1) = (c - flank(ra), c + flank(ra));
        for i in 1..4 {
            pts.push(polar(ra, a0 + (a1 - a0) * i as f32 / 4.0));
        }
        for i in (0..=steps).rev() {
            let r = r0 + (ra - r0) * i as f32 / steps as f32;
            pts.push(polar(r, c + flank(r)));
        }
        if rd < rb {
            pts.push(polar(rd, c + flank(rb)));
        }
        // root arc to the next tooth
        let next = c + TAU / z - flank(rb);
        let from = c + flank(rb);
        for i in 1..4 {
            pts.push(polar(rd, from + (next - from) * i as f32 / 4.0));
        }
    }
    pts
}

fn circle(r: f32, n: usize) -> Vec<Vec2> {
    (0..n).map(|i| polar(r, TAU * i as f32 / n as f32)).collect()
}

pub fn gear_mesh(g: &Gear) -> Mesh {
    let mut contours = vec![gear_outline(g)];
    let rd = g.module * g.teeth as f32 * 0.5 - 1.25 * g.module;
    if g.bore > 0.0 && g.bore * 0.5 < rd - 0.8 {
        contours.push(circle(g.bore * 0.5, 64));
    }
    extrude_contours(contours, g.thickness.max(0.2)).unwrap_or_default()
}

// ---------- spring ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// Outside diameter of the coil.
    pub diameter: f32,
    pub wire: f32,
    /// Free length.
    pub length: f32,
    pub coils: f32,
    /// Closed and flattened ends, so it stands flat.
    pub flat_ends: bool,
}

impl Default for Spring {
    fn default() -> Self {
        Spring { diameter: 16.0, wire: 1.6, length: 30.0, coils: 6.0, flat_ends: true }
    }
}

/// A round wire swept along `path` (closed at both ends).
fn sweep(path: &[Vec3], r: f32, sides: usize) -> Mesh {
    let mut m = Mesh::default();
    let n = path.len();
    let mut prev_normal: Option<Vec3> = None;
    for i in 0..n {
        let t = if i + 1 < n { path[i + 1] - path[i] } else { path[i] - path[i - 1] }.normalize_or(Vec3::Z);
        // parallel transport of the frame, so the wire doesn't twist
        let nrm = match prev_normal {
            None => t.any_orthonormal_vector(),
            Some(p) => (p - t * p.dot(t)).normalize_or(t.any_orthonormal_vector()),
        };
        prev_normal = Some(nrm);
        let b = t.cross(nrm);
        for k in 0..sides {
            let a = TAU * k as f32 / sides as f32;
            m.positions.push(path[i] + (nrm * a.cos() + b * a.sin()) * r);
        }
    }
    let idx = |i: usize, k: usize| (i * sides + k % sides) as u32;
    for i in 0..n - 1 {
        for k in 0..sides {
            m.triangles.push([idx(i, k), idx(i + 1, k + 1), idx(i + 1, k)]);
            m.triangles.push([idx(i, k), idx(i, k + 1), idx(i + 1, k + 1)]);
        }
    }
    let (c0, c1) = (m.positions.len() as u32, m.positions.len() as u32 + 1);
    m.positions.push(path[0]);
    m.positions.push(path[n - 1]);
    for k in 0..sides {
        m.triangles.push([c0, idx(0, k + 1), idx(0, k)]);
        m.triangles.push([c1, idx(n - 1, k), idx(n - 1, k + 1)]);
    }
    if m.volume() < 0.0 {
        m.flip();
    }
    m
}

pub fn spring_mesh(s: &Spring) -> Mesh {
    let w = s.wire.max(0.2);
    let rc = ((s.diameter - w) * 0.5).max(w);
    let coils = s.coils.max(1.0);
    // closed ends: the last ¾ turn at each end lies against its neighbour
    let end = if s.flat_ends { 0.75f32.min(coils * 0.25) } else { 0.0 };
    let closed_pitch = w * 1.04;
    let active = (coils - 2.0 * end).max(0.1);
    let free = (s.length - w).max(w * coils);
    let pitch = ((free - 2.0 * end * closed_pitch) / active).max(closed_pitch);
    let per_turn = 48usize;
    let total = (coils * per_turn as f32).ceil() as usize;
    let mut path = Vec::with_capacity(total + 1);
    let mut z = w * 0.5;
    for i in 0..=total {
        let turn = i as f32 / per_turn as f32;
        let a = TAU * turn;
        path.push(Vec3::new(rc * a.cos(), rc * a.sin(), z));
        let p = if turn < end || turn > coils - end { closed_pitch } else { pitch };
        z += p / per_turn as f32;
    }
    let mut m = sweep(&path, w * 0.5, 16);
    if s.flat_ends {
        // grind the ends flat
        let b = m.bounds();
        if let Ok((above, _)) = cut_by_plane(&m, Vec3::new(0.0, 0.0, b.min.z + w * 0.35), Vec3::Z) {
            if !above.is_empty() {
                m = above;
            }
        }
        let b = m.bounds();
        if let Ok((_, below)) = cut_by_plane(&m, Vec3::new(0.0, 0.0, b.max.z - w * 0.35), Vec3::Z) {
            if !below.is_empty() {
                m = below;
            }
        }
    }
    m
}

// ---------- print-in-place hinge ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hinge {
    /// Along the pin.
    pub length: f32,
    /// Each leaf, from the pin outwards.
    pub leaf: f32,
    pub thickness: f32,
    /// Knuckles (odd, ≥ 3).
    pub knuckles: u32,
    pub pin: f32,
    pub clearance: f32,
}

impl Default for Hinge {
    fn default() -> Self {
        Hinge { length: 40.0, leaf: 15.0, thickness: 3.0, knuckles: 5, pin: 3.0, clearance: 0.35 }
    }
}

fn box_at(min: Vec3, max: Vec3) -> Mesh {
    let mut b = cuboid(max - min);
    b.translate((min + max) * 0.5);
    b
}

/// Cylinder along X from x0 to x1, axis at (y, z), radius r.
fn rod_x(x0: f32, x1: f32, r: f32, y: f32, z: f32) -> Mesh {
    let mut c = frustum(r, r, x1 - x0, 64);
    c.transform(&(Mat4::from_translation(Vec3::new((x0 + x1) * 0.5, y, z)) * Mat4::from_rotation_y(PI * 0.5)));
    c
}

/// Two leaves joined by interleaved knuckles around a captive pin — prints flat, in one
/// piece, and moves straight off the plate.
pub fn hinge_mesh(h: &Hinge) -> crate::Result<Mesh> {
    let n = (h.knuckles.max(3) | 1) as usize;
    let c = h.clearance.max(0.1);
    let t = h.thickness.max(1.0);
    let pin_r = (h.pin * 0.5).max(0.6);
    let r = (t * 0.5).max(pin_r + c + 0.8);
    let seg = ((h.length - (n - 1) as f32 * c) / n as f32).max(1.0);
    let x_of = |i: usize| i as f32 * (seg + c);
    let leaf = h.leaf.max(r + c + 2.0);
    let mut a_parts = vec![];
    let mut b_parts = vec![];
    // leaves, clear of the other side's knuckles
    a_parts.push(box_at(Vec3::new(0.0, r + c, 0.0), Vec3::new(h.length, leaf, t)));
    b_parts.push(box_at(Vec3::new(0.0, -leaf, 0.0), Vec3::new(h.length, -(r + c), t)));
    for i in 0..n {
        let (x0, x1) = (x_of(i), x_of(i) + seg);
        let knuckle = rod_x(x0, x1, r, 0.0, r);
        if i % 2 == 0 {
            a_parts.push(knuckle);
            a_parts.push(box_at(Vec3::new(x0, 0.0, 0.0), Vec3::new(x1, r + c + 0.5, t.min(2.0 * r))));
        } else {
            b_parts.push(knuckle);
            b_parts.push(box_at(Vec3::new(x0, -(r + c + 0.5), 0.0), Vec3::new(x1, 0.0, t.min(2.0 * r))));
        }
    }
    // the pin belongs to leaf A and runs through B's knuckles
    a_parts.push(rod_x(0.0, h.length, pin_r, 0.0, r));
    let first = a_parts.remove(0);
    let a = boolean_many(&first, &a_parts, BoolOp::Union)?;
    let first = b_parts.remove(0);
    let b = boolean_many(&first, &b_parts, BoolOp::Union)?;
    let b = boolean(&b, &rod_x(-1.0, h.length + 1.0, pin_r + c, 0.0, r), BoolOp::Subtract)?;
    let mut out = a;
    out.append(&b);
    Ok(out)
}

// ---------- snap-fit clip ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapClip {
    /// Beam length (to the tip).
    pub length: f32,
    pub width: f32,
    pub thickness: f32,
    /// How far the hook sticks out.
    pub hook: f32,
    /// Base block under the beam (0 = none, to attach it to your own part).
    pub base: f32,
}

impl Default for SnapClip {
    fn default() -> Self {
        SnapClip { length: 14.0, width: 8.0, thickness: 1.8, hook: 1.2, base: 3.0 }
    }
}

/// A cantilever snap-fit hook: a flexible beam with a ramp to slide in and a flat catch
/// that holds. Profile in X/Z, extruded along Y.
pub fn snap_clip_mesh(s: &SnapClip) -> Mesh {
    let t = s.thickness.max(0.4);
    let h = s.hook.max(0.2);
    let l = s.length.max(h * 3.0 + 1.0);
    let hl = h * 2.5; // ramp length
    let catch_z = l - hl;
    let mut p = vec![];
    if s.base > 0.0 {
        let b = s.base;
        p.extend([Vec2::new(-t, -b), Vec2::new(2.0 * t + h, -b), Vec2::new(2.0 * t + h, 0.0)]);
    } else {
        p.push(Vec2::new(0.0, 0.0));
    }
    p.extend([Vec2::new(t, 0.0), Vec2::new(t, catch_z), Vec2::new(t + h, catch_z), Vec2::new(t, l), Vec2::new(0.0, l)]);
    if s.base > 0.0 {
        p.extend([Vec2::new(0.0, 0.0), Vec2::new(-t, 0.0)]);
    }
    let mut m = extrude_contours(vec![p], s.width.max(0.5)).unwrap_or_default();
    // profile plane (x, z): rotate the extrusion direction onto Y
    m.transform(&Mat4::from_rotation_x(PI * 0.5));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    #[test]
    fn gear_size_and_closed() {
        let g = Gear::default();
        let m = gear_mesh(&g);
        assert!(analyze(&m).is_watertight(), "{:?}", analyze(&m).problems());
        let d = m.bounds().size().x;
        let tip = g.module * (g.teeth as f32 + 2.0);
        assert!((d - tip).abs() < 0.2, "{d} vs {tip}");
        assert!((m.bounds().size().z - g.thickness).abs() < 1e-4);
    }

    #[test]
    fn spring_closed_and_long_enough() {
        let s = Spring::default();
        let m = spring_mesh(&s);
        assert!(analyze(&m).is_watertight(), "{:?}", analyze(&m).problems());
        let b = m.bounds();
        assert!((b.size().x - s.diameter).abs() < 0.2, "{}", b.size().x);
        assert!(b.size().z > s.length * 0.85 && b.size().z <= s.length + 0.1, "{}", b.size().z);
    }

    #[test]
    fn hinge_is_two_pieces() {
        let m = hinge_mesh(&Hinge::default()).unwrap();
        assert!(analyze(&m).is_watertight());
        assert_eq!(m.connected_components().len(), 2, "two leaves that move freely");
    }

    #[test]
    fn snap_clip_profile() {
        let s = SnapClip::default();
        let m = snap_clip_mesh(&s);
        assert!(analyze(&m).is_watertight());
        let b = m.bounds();
        assert!((b.size().y - s.width).abs() < 1e-3);
        assert!((b.size().z - (s.length + s.base)).abs() < 1e-3);
    }
}
