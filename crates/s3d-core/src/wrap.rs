//! Bending flat shapes (embossed text, pictures) onto curved surfaces: slice the shape into
//! thin strips (watertight), then map each point around a cylinder or sphere-like surface.

use crate::mesh::Mesh;
use glam::Vec3;

fn key(p: Vec3) -> [u32; 3] {
    [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
}

/// Where segment p–q crosses `axis = h`; endpoints ordered so neighbours agree bit-exactly.
fn cross(p: Vec3, q: Vec3, axis: usize, h: f32) -> Vec3 {
    let (p, q) = if key(p) <= key(q) { (p, q) } else { (q, p) };
    let t = (h - p[axis]) / (q[axis] - p[axis]);
    let mut r = p + (q - p) * t;
    r[axis] = h;
    r
}

/// Polygon vertex plus the original edge the side *arriving* at it lies on (None = a cut
/// along a slicing plane). Crossings are always computed on the original edge, so the
/// strips on both sides of a plane get bit-identical points.
type Pv = (Vec3, Option<(Vec3, Vec3)>);

fn clip(poly: &[Pv], axis: usize, h: f32, keep_below: bool) -> Vec<Pv> {
    let inside = |p: &Vec3| if keep_below { p[axis] <= h } else { p[axis] >= h };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = poly[i].0;
        let (b, origin) = poly[(i + 1) % poly.len()];
        let at = |o: Option<(Vec3, Vec3)>| match o {
            Some((p, q)) => cross(p, q, axis, h),
            None => cross(a, b, axis, h),
        };
        match (inside(&a), inside(&b)) {
            (true, true) => out.push((b, origin)),
            (true, false) => out.push((at(origin), origin)),
            (false, true) => {
                out.push((at(origin), None));
                out.push((b, origin));
            }
            (false, false) => {}
        }
    }
    out
}

/// Split every triangle along planes `axis = k·step`. The result is still watertight.
pub fn slice(m: &Mesh, axis: usize, step: f32) -> Mesh {
    if step <= 0.0 {
        return m.clone();
    }
    let mut soup: Vec<[Vec3; 3]> = Vec::with_capacity(m.triangles.len() * 2);
    for i in 0..m.triangles.len() {
        let t = m.tri(i);
        let lo = t.iter().map(|p| p[axis]).fold(f32::MAX, f32::min);
        let hi = t.iter().map(|p| p[axis]).fold(f32::MIN, f32::max);
        let (k0, k1) = ((lo / step).floor() as i64, (hi / step).ceil() as i64);
        if k1 - k0 <= 1 {
            soup.push(t);
            continue;
        }
        for k in k0..k1 {
            let (a, b) = (k as f32 * step, (k + 1) as f32 * step);
            let tri: Vec<Pv> = (0..3).map(|j| (t[j], Some((t[(j + 2) % 3], t[j])))).collect();
            let piece = clip(&clip(&tri, axis, b, true), axis, a, false);
            if piece.len() >= 3 {
                for j in 1..piece.len() - 1 {
                    soup.push([piece[0].0, piece[j].0, piece[j + 1].0]);
                }
            }
        }
    }
    let mut out = Mesh::from_soup(&soup);
    out.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    out
}

/// A bend: the shape's XY plane follows a surface curved with radius `rx` around the Y
/// axis (across the reading direction) and `ry` around the X axis. Infinite = flat;
/// negative = concave. Z is the height above the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bend {
    pub rx: f32,
    pub ry: f32,
}

impl Bend {
    pub const FLAT: Bend = Bend { rx: f32::INFINITY, ry: f32::INFINITY };

    pub fn is_flat(&self) -> bool {
        !self.rx.is_finite() && !self.ry.is_finite()
    }

    /// Map a point of the flat shape onto the bent surface (same frame, surface point at
    /// the origin, normal +Z).
    pub fn apply(&self, p: Vec3) -> Vec3 {
        let (mut x, mut y, mut z) = (p.x, p.y, p.z);
        if self.rx.is_finite() {
            let th = x / self.rx;
            let r = self.rx + z;
            x = th.sin() * r;
            z = th.cos() * r - self.rx;
        }
        if self.ry.is_finite() {
            let ph = y / self.ry;
            let r = self.ry + z;
            y = ph.sin() * r;
            z = ph.cos() * r - self.ry;
        }
        Vec3::new(x, y, z)
    }

    /// Outward surface direction at shape point (x, y).
    pub fn normal(&self, x: f32, y: f32) -> Vec3 {
        (self.apply(Vec3::new(x, y, 1.0)) - self.apply(Vec3::new(x, y, 0.0))).normalize_or(Vec3::Z)
    }

    /// Radius from surface heights `h` (along the normal) measured at distances `a` from
    /// the centre: the circle through (0, 0) and (a, h), averaged as curvature. Infinite
    /// when (almost) flat. Rounded to ~2 % so small hover jitter doesn't rebuild meshes.
    pub fn fit(samples: &[(f32, f32)]) -> f32 {
        let ks: Vec<f32> = samples
            .iter()
            .filter(|(a, _)| *a > 0.0)
            .map(|&(a, h)| if h.abs() < 0.01 + a * 0.002 { 0.0 } else { -2.0 * h / (a * a + h * h) })
            .collect();
        if ks.is_empty() {
            return f32::INFINITY;
        }
        let k = ks.iter().sum::<f32>() / ks.len() as f32;
        if k.abs() < 1e-4 {
            return f32::INFINITY;
        }
        let r = 1.0 / k;
        r.signum() * ((r.abs().ln() * 50.0).round() / 50.0).exp()
    }

    /// Drop the bend around an axis whose radius is too tight for heights z0..z1 (the
    /// shape would fold through the axis).
    pub fn safe(self, z0: f32, z1: f32) -> Bend {
        // the radius at every height must keep the sign of r (and not shrink to nothing)
        let ok = |r: f32| !r.is_finite() || ((r + z0) / r >= 0.05 && (r + z1) / r >= 0.05);
        Bend { rx: if ok(self.rx) { self.rx } else { f32::INFINITY }, ry: if ok(self.ry) { self.ry } else { f32::INFINITY } }
    }

    /// Strip widths for slicing (rounded so they change rarely).
    pub fn steps(&self, size_x: f32, size_y: f32) -> (Option<f32>, Option<f32>) {
        let q = |s: f32| ((s.log2() * 2.0).floor() / 2.0).exp2();
        let st = |r: f32, size: f32| r.is_finite().then(|| q((r.abs() * 0.06).clamp(0.2, size / 8.0 + 0.2)));
        (st(self.rx, size_x), st(self.ry, size_y))
    }

    /// Prepare a shape for bending: thin strips across the bending directions.
    pub fn prepare(&self, m: &Mesh) -> Mesh {
        let s = m.bounds().size();
        prepare_steps(m, self.steps(s.x, s.y))
    }

    /// Map a prepared shape with z ∈ [0, 1] onto the bent surface, its height spanning
    /// z0..z1 (relative to the surface).
    pub fn map(&self, prepared: &Mesh, z0: f32, z1: f32) -> Mesh {
        let mut out = prepared.clone();
        for p in &mut out.positions {
            *p = self.apply(Vec3::new(p.x, p.y, z0 + p.z * (z1 - z0)));
        }
        out
    }
}

/// Slice with explicit strip widths (see [`Bend::steps`]).
pub fn prepare_steps(m: &Mesh, steps: (Option<f32>, Option<f32>)) -> Mesh {
    let mut out = m.clone();
    if let Some(s) = steps.0 {
        out = slice(&out, 0, s);
    }
    if let Some(s) = steps.1 {
        out = slice(&out, 1, s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn slicing_keeps_it_closed() {
        let mut m = cuboid(Vec3::new(30.0, 6.0, 2.0));
        m.translate(Vec3::new(0.0, 0.0, 1.0));
        let s = slice(&slice(&m, 0, 1.0), 1, 1.0);
        let r = analyze(&s);
        assert!(r.is_watertight(), "{:?}", r.problems());
        assert!((s.volume() - m.volume()).abs() < 1e-3);
        assert!(s.triangles.len() > m.triangles.len() * 10);
    }

    #[test]
    fn wraps_onto_a_cylinder() {
        // a strip 31.4 mm long wrapped on a radius-10 cylinder covers half a turn
        let bend = Bend { rx: 10.0, ry: f32::INFINITY };
        let p = bend.apply(Vec3::new(std::f32::consts::PI * 10.0 / 2.0, 0.0, 0.0));
        assert!((p - Vec3::new(10.0, 0.0, -10.0)).length() < 1e-4, "{p}");
        // height stays radial
        let q = bend.apply(Vec3::new(std::f32::consts::PI * 10.0 / 2.0, 0.0, 2.0));
        assert!((q - Vec3::new(12.0, 0.0, -10.0)).length() < 1e-4);
        // and the fit recovers the radius from sampled heights
        let a = 6.0;
        let h = -(10.0 - (100.0f32 - a * a).sqrt());
        assert!((Bend::fit(&[(a, h), (a, h)]) - 10.0).abs() < 0.15);
        assert!(!Bend::fit(&[(a, 0.0), (a, 0.001)]).is_finite());
        // concave
        assert!((Bend::fit(&[(a, -h)]) + 10.0).abs() < 0.15);
        // too tight for an 11 mm deep engraving: dropped
        assert!(!Bend { rx: 10.0, ry: f32::INFINITY }.safe(-11.0, 0.5).rx.is_finite());
        assert!(Bend { rx: 10.0, ry: f32::INFINITY }.safe(-2.0, 0.5).rx.is_finite());
        assert!(!Bend { rx: -10.0, ry: f32::INFINITY }.safe(-1.0, 12.0).rx.is_finite());
    }

    #[test]
    fn bent_text_stays_closed() {
        let mut m = cuboid(Vec3::new(30.0, 6.0, 1.0));
        m.translate(Vec3::new(0.0, 0.0, 0.5));
        let bend = Bend { rx: 12.0, ry: 40.0 };
        let b = bend.map(&bend.prepare(&m), -0.5, 2.0);
        assert!(analyze(&b).is_watertight());
        assert!(b.volume() > 0.0);
    }

    #[test]
    fn sphere_points_stay_on_the_sphere() {
        let bend = Bend { rx: 20.0, ry: 20.0 };
        for (x, y) in [(5.0, 3.0), (-8.0, 6.0), (10.0, -10.0)] {
            let p = bend.apply(Vec3::new(x, y, 0.0)) + Vec3::new(0.0, 0.0, 20.0);
            assert!((p.length() - 20.0).abs() < 1e-3, "{p}");
        }
    }
}
