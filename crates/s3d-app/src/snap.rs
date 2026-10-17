//! Object snapping: align a moving bounding box with other objects and the plate.

use glam::{Vec2, Vec3};
use s3d_core::Aabb;

pub struct Snap {
    pub correction: Vec3,
    /// World-space guide segments to draw.
    pub guides: Vec<(Vec3, Vec3)>,
}

fn feats(b: &Aabb, a: usize) -> [f32; 3] {
    [b.min[a], b.center()[a], b.max[a]]
}

/// Snap `moving` on the enabled `axes` to min/centre/max of `targets` (and the plate's
/// edges/centre and surface), if within `tol` world units.
pub fn snap_box(moving: &Aabb, targets: &[Aabb], plate_half: Option<Vec2>, axes: [bool; 3], tol: f32) -> Snap {
    let mut correction = Vec3::ZERO;
    let mut guides = vec![];
    for a in 0..3 {
        if !axes[a] {
            continue;
        }
        // (distance, target value, target index or None for plate)
        let mut best: Option<(f32, f32, Option<usize>)> = None;
        let mut consider = |d: f32, tv: f32, ti: Option<usize>| {
            if d.abs() <= tol && best.is_none_or(|b| d.abs() < b.0.abs() - 1e-6) {
                best = Some((d, tv, ti));
            }
        };
        for (ti, t) in targets.iter().enumerate() {
            for tv in feats(t, a) {
                for mv in feats(moving, a) {
                    consider(tv - mv, tv, Some(ti));
                }
            }
        }
        if let Some(h) = plate_half {
            if a < 2 {
                for tv in [-h[a], 0.0, h[a]] {
                    for mv in feats(moving, a) {
                        consider(tv - mv, tv, None);
                    }
                }
            } else {
                consider(-moving.min.z, 0.0, None);
            }
        }
        let Some((d, tv, ti)) = best else { continue };
        correction[a] = d;
        // guide: a line on the aligned plane spanning both objects
        let moved = Aabb { min: moving.min + correction, max: moving.max + correction };
        match ti {
            Some(ti) => {
                let t = &targets[ti];
                let both = moved.union(t);
                let b = if a == 0 { 1 } else { 0 }; // axis the guide runs along
                let pad = 2.0;
                let (mut p1, mut p2) = (Vec3::ZERO, Vec3::ZERO);
                p1[a] = tv;
                p2[a] = tv;
                p1[b] = both.min[b] - pad;
                p2[b] = both.max[b] + pad;
                let c = 3 - a - b; // remaining axis: sit on the higher of the two bottoms (or mid-height for Z)
                let h = if c == 2 { moved.min.z.max(t.min.z) } else { (both.min[c] + both.max[c]) * 0.5 };
                p1[c] = h;
                p2[c] = h;
                guides.push((p1, p2));
            }
            None => {
                if let (Some(h), true) = (plate_half, a < 2) {
                    let b = 1 - a;
                    let (mut q1, mut q2) = (Vec3::ZERO, Vec3::ZERO);
                    q1[a] = tv;
                    q2[a] = tv;
                    q1[b] = -h[b];
                    q2[b] = h[b];
                    guides.push((q1, q2));
                }
            }
        }
    }
    Snap { correction, guides }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min: Vec3::from(min), max: Vec3::from(max) }
    }

    #[test]
    fn snaps_edge_to_edge() {
        let target = bx([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let moving = bx([10.6, 2.0, 0.0], [20.6, 12.0, 10.0]);
        let s = snap_box(&moving, &[target], None, [true, true, false], 1.0);
        assert!((s.correction.x + 0.6).abs() < 1e-5, "{:?}", s.correction);
        assert_eq!(s.correction.y, 0.0); // 2 mm away: outside tolerance
        assert!(!s.guides.is_empty());
    }

    #[test]
    fn respects_tolerance_and_axes() {
        let target = bx([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let moving = bx([13.0, 23.0, 0.0], [17.0, 27.0, 4.0]);
        let s = snap_box(&moving, &[target], None, [true, true, true], 1.0);
        assert_eq!(s.correction, Vec3::ZERO);
        let s = snap_box(&moving, &[target], None, [true, false, false], 3.5);
        assert!((s.correction.x + 3.0).abs() < 1e-5); // min 13 -> max 10
        assert_eq!(s.correction.y, 0.0);
    }

    #[test]
    fn snaps_to_plate_centre_and_surface() {
        let moving = bx([-4.6, -5.0, 0.3], [5.4, 5.0, 10.3]);
        let s = snap_box(&moving, &[], Some(Vec2::splat(100.0)), [true, true, true], 0.5);
        assert!((s.correction.x + 0.4).abs() < 1e-5); // centre 0.4 -> 0
        assert!((s.correction.z + 0.3).abs() < 1e-5); // bottom -> plate
    }
}
