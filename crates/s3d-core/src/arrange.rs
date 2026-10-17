//! Auto-orient (least overhang, most bed contact) and auto-arrange (pack footprints on
//! the plate).

use crate::mesh::Mesh;
use glam::{Quat, Vec2, Vec3};
use std::collections::HashMap;

/// How printable a part is when `down` points at the bed (lower = better).
#[derive(Debug, Clone, Copy)]
pub struct OrientScore {
    pub overhang: f32,
    pub contact: f32,
    pub height: f32,
    pub score: f32,
}

/// Faces steeper than this (from horizontal, facing down) need support.
const OVERHANG_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;

pub fn score(m: &Mesh, down: Vec3, total_area: f32) -> OrientScore {
    let d = down.normalize_or(-Vec3::Z);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in &m.positions {
        let h = p.dot(d);
        lo = lo.min(h);
        hi = hi.max(h);
    }
    let bed = hi; // the extreme along `down` touches the bed
    let tol = ((hi - lo) * 1e-3).max(0.02);
    let (mut overhang, mut contact) = (0.0, 0.0);
    for i in 0..m.triangles.len() {
        let [a, b, c] = m.tri(i);
        let cr = (b - a).cross(c - a);
        let area = cr.length() * 0.5;
        if area <= 0.0 {
            continue;
        }
        let nd = cr.dot(d) / (area * 2.0);
        if nd <= OVERHANG_COS {
            continue;
        }
        let on_bed = [a, b, c].iter().all(|p| bed - p.dot(d) < tol);
        if on_bed {
            contact += area;
        } else {
            overhang += area * nd;
        }
    }
    let height = hi - lo;
    let score = overhang - 0.3 * contact + 0.02 * height * total_area.sqrt();
    OrientScore { overhang, contact, height, score }
}

/// The best rotation for printing: tries the directions of the largest flat areas (and
/// the six axes) as "down". Returns the rotation to apply, and the scores before/after.
pub fn best_orientation(m: &Mesh) -> (Quat, OrientScore, OrientScore) {
    let mut buckets: HashMap<[i32; 3], (Vec3, f32)> = HashMap::new();
    let mut total = 0.0;
    for i in 0..m.triangles.len() {
        let [a, b, c] = m.tri(i);
        let cr = (b - a).cross(c - a);
        let area = cr.length() * 0.5;
        if area <= 0.0 {
            continue;
        }
        total += area;
        let n = cr / (area * 2.0);
        let k = (n * 6.0).round();
        let e = buckets.entry([k.x as i32, k.y as i32, k.z as i32]).or_insert((Vec3::ZERO, 0.0));
        e.0 += n * area;
        e.1 += area;
    }
    let mut cands: Vec<(Vec3, f32)> = buckets.into_values().map(|(n, a)| (n.normalize_or_zero(), a)).collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    cands.truncate(24);
    let mut dirs: Vec<Vec3> = cands.into_iter().map(|c| c.0).filter(|d| *d != Vec3::ZERO).collect();
    dirs.extend([Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z]);
    let now = score(m, -Vec3::Z, total);
    let mut best = (-Vec3::Z, now);
    for d in dirs {
        let s = score(m, d, total);
        if s.score < best.1.score - 1e-3 * total.max(1.0) {
            best = (d, s);
        }
    }
    (Quat::from_rotation_arc(best.0, -Vec3::Z), now, best.1)
}

/// Pack rectangles (footprint sizes) in rows, as square as possible and no wider than
/// `max_width`. Returns the centre of each, with the whole layout centred on the origin.
pub fn pack(sizes: &[Vec2], gap: f32, max_width: f32) -> Vec<Vec2> {
    if sizes.is_empty() {
        return vec![];
    }
    let area: f32 = sizes.iter().map(|s| (s.x + gap) * (s.y + gap)).sum();
    let widest = sizes.iter().map(|s| s.x).fold(0.0, f32::max);
    let row_w = (area.sqrt() * 1.15).max(widest).min(max_width.max(widest));
    // tallest first, rows left to right
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].y.total_cmp(&sizes[a].y).then(sizes[b].x.total_cmp(&sizes[a].x)));
    let mut out = vec![Vec2::ZERO; sizes.len()];
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    let mut used_w = 0.0f32;
    for i in order {
        let s = sizes[i];
        if x > 0.0 && x + s.x > row_w {
            y += row_h + gap;
            x = 0.0;
            row_h = 0.0;
        }
        out[i] = Vec2::new(x + s.x * 0.5, y + s.y * 0.5);
        x += s.x + gap;
        used_w = used_w.max(x - gap);
        row_h = row_h.max(s.y);
    }
    let centre = Vec2::new(used_w * 0.5, (y + row_h) * 0.5);
    // the first row at the back looks odd; put it at the front (−Y is the front)
    out.iter().map(|p| *p - centre).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{cuboid, frustum};

    #[test]
    fn orients_a_cone_onto_its_base() {
        // a cone standing on its tip prints badly; the best is its flat base down
        let mut c = frustum(10.0, 0.5, 20.0, 48);
        c.transform(&glam::Mat4::from_rotation_x(std::f32::consts::PI));
        let (q, before, after) = best_orientation(&c);
        let r = c.transformed(&glam::Mat4::from_quat(q));
        assert!(after.score < before.score);
        // after rotating, the wide end is at the bottom
        let b = r.bounds();
        let low: Vec<&Vec3> = r.positions.iter().filter(|p| p.z < b.min.z + 0.1).collect();
        assert!(low.iter().any(|p| p.truncate().length() > 9.0));
        // a cube is already fine
        let (q, ..) = best_orientation(&cuboid(Vec3::splat(10.0)));
        assert!(q.angle_between(Quat::IDENTITY) < 1e-3 || (q * Vec3::Z).z.abs() > 0.999);
    }

    #[test]
    fn packs_without_overlap() {
        let sizes = vec![Vec2::new(30.0, 20.0), Vec2::new(10.0, 10.0), Vec2::new(25.0, 40.0), Vec2::new(5.0, 50.0)];
        let c = pack(&sizes, 5.0, 250.0);
        for i in 0..sizes.len() {
            for j in i + 1..sizes.len() {
                let d = (c[i] - c[j]).abs();
                let sep = (sizes[i] + sizes[j]) * 0.5;
                assert!(d.x >= sep.x + 4.9 || d.y >= sep.y + 4.9, "{i} {j}");
            }
        }
    }
}
