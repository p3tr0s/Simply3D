//! Measuring helpers: flat faces (plane regions) and circles (holes, bosses, round edges)
//! recognised from the triangles around a clicked point, even on low-poly meshes.

use crate::mesh::Mesh;
use crate::paint::fill_region;
use glam::{Mat3, Vec3};

/// A flat face: the triangles, a unit normal and a point on it (centroid), all in the
/// mesh's own space.
#[derive(Debug, Clone)]
pub struct Plane {
    pub tris: Vec<usize>,
    pub normal: Vec3,
    pub point: Vec3,
    pub area: f32,
}

/// A circle: centre, axis (unit) and radius, in the mesh's own space.
#[derive(Debug, Clone, Copy)]
pub struct Circle {
    pub center: Vec3,
    pub axis: Vec3,
    pub radius: f32,
}

/// The flat face around triangle `tri` (neighbours within 1°).
pub fn plane_at(m: &Mesh, tri: usize) -> Plane {
    let tris = fill_region(m, tri, 1.0);
    let (mut n, mut c, mut area) = (Vec3::ZERO, Vec3::ZERO, 0.0);
    for &i in &tris {
        let [a, b, cc] = m.tri(i);
        let cr = (b - a).cross(cc - a);
        let ar = cr.length() * 0.5;
        n += cr;
        c += (a + b + cc) / 3.0 * ar;
        area += ar;
    }
    let normal = n.normalize_or(m.face_normal(tri));
    let point = if area > 0.0 { c / area } else { m.tri(tri)[0] };
    Plane { tris, normal, point, area }
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3×3 matrix (Jacobi).
fn smallest_eigvec(m: Mat3) -> Vec3 {
    let mut a = m.to_cols_array_2d();
    let mut v = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        // largest off-diagonal
        let (mut p, mut q, mut best) = (0, 1, a[0][1].abs());
        for (i, j) in [(0, 2), (1, 2)] {
            if a[i][j].abs() > best {
                best = a[i][j].abs();
                p = i;
                q = j;
            }
        }
        if best < 1e-12 {
            break;
        }
        let th = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = th.sin_cos();
        for k in 0..3 {
            let (akp, akq) = (a[k][p], a[k][q]);
            a[k][p] = c * akp - s * akq;
            a[k][q] = s * akp + c * akq;
        }
        for k in 0..3 {
            let (apk, aqk) = (a[p][k], a[q][k]);
            a[p][k] = c * apk - s * aqk;
            a[q][k] = s * apk + c * aqk;
        }
        for row in v.iter_mut() {
            let (vp, vq) = (row[p], row[q]);
            row[p] = c * vp - s * vq;
            row[q] = s * vp + c * vq;
        }
    }
    let k = (0..3).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap();
    Vec3::new(v[0][k], v[1][k], v[2][k]).normalize_or(Vec3::Z)
}

/// Least-squares circle through 2D points (Kåsa fit): (centre, radius).
fn fit_circle_2d(pts: &[(f32, f32)]) -> Option<((f32, f32), f32)> {
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0f64, 0.0f64), |s, p| (s.0 + p.0 as f64, s.1 + p.1 as f64));
    let (mx, my) = (mx / n, my / n);
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (u, v) = (p.0 as f64 - mx, p.1 as f64 - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-12 {
        return None;
    }
    let (r1, r2) = (0.5 * (suuu + suvv), 0.5 * (svvv + svuu));
    let uc = (r1 * svv - r2 * suv) / det;
    let vc = (r2 * suu - r1 * suv) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    Some((((uc + mx) as f32, (vc + my) as f32), r as f32))
}

/// The circle of the curved surface (a hole, a boss, a rounded edge) around triangle
/// `tri`: the smooth region is collected, its axis is the direction all its normals are
/// perpendicular to, and a circle is fitted to its points seen along that axis.
pub fn circle_at(m: &Mesh, tri: usize) -> Option<Circle> {
    let tris = fill_region(m, tri, 30.0);
    if tris.len() < 3 {
        return None;
    }
    let mut cov = Mat3::ZERO;
    let mut verts: Vec<u32> = vec![];
    for &i in &tris {
        let [a, b, c] = m.tri(i);
        let cr = (b - a).cross(c - a);
        let w = cr.length();
        let n = cr.normalize_or_zero();
        cov += Mat3::from_cols(n * n.x, n * n.y, n * n.z) * w;
        verts.extend_from_slice(&m.triangles[i]);
    }
    let axis = smallest_eigvec(cov);
    verts.sort_unstable();
    verts.dedup();
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    let pts: Vec<(f32, f32)> = verts.iter().map(|&i| m.positions[i as usize]).map(|p| (p.dot(u), p.dot(v))).collect();
    let ((cu, cv), r) = fit_circle_2d(&pts)?;
    // reject poor fits (a flat face, a random blob)
    let err = pts.iter().map(|&(x, y)| (((x - cu).powi(2) + (y - cv).powi(2)).sqrt() - r).abs()).fold(0.0f32, f32::max);
    if !r.is_finite() || err > r * 0.08 + 0.02 {
        return None;
    }
    // the normals must cover a real arc (not a nearly flat patch)
    let ns: Vec<Vec3> = tris.iter().map(|&i| m.face_normal(i)).collect();
    let spread = ns.iter().map(|a| ns.iter().map(|b| a.dot(*b)).fold(1.0f32, f32::min)).fold(1.0f32, f32::min);
    if spread > 0.98 {
        return None;
    }
    let along = verts.iter().map(|&i| m.positions[i as usize].dot(axis)).sum::<f32>() / verts.len() as f32;
    Some(Circle { center: u * cu + v * cv + axis * along, axis, radius: r })
}

/// Angle between two directions in degrees (0–180).
pub fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero().dot(b.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{cuboid, frustum};

    #[test]
    fn finds_faces_and_circles() {
        let m = cuboid(Vec3::new(10.0, 20.0, 30.0));
        let top = (0..12).find(|&i| m.face_normal(i).z > 0.9).unwrap();
        let p = plane_at(&m, top);
        assert_eq!(p.tris.len(), 2);
        assert!((p.point.z - 15.0).abs() < 1e-4 && (p.area - 200.0).abs() < 1e-3);
        assert!(circle_at(&m, top).is_none(), "a flat face is not a circle");

        // a 24-sided cylinder of radius 6: the fit sees the polygon's radius
        let c = frustum(6.0, 6.0, 10.0, 24);
        let side = (0..c.triangles.len()).find(|&i| c.face_normal(i).z.abs() < 0.1).unwrap();
        let circ = circle_at(&c, side).unwrap();
        assert!(circ.axis.z.abs() > 0.999, "{:?}", circ.axis);
        assert!((circ.radius - 6.0).abs() < 0.1, "{}", circ.radius);
        assert!(circ.center.truncate().length() < 0.05);
    }

    #[test]
    fn hole_in_a_block() {
        let block = cuboid(Vec3::new(30.0, 30.0, 10.0));
        let pin = frustum(4.0, 4.0, 20.0, 48);
        let m = crate::csg::boolean(&block, &pin, crate::csg::BoolOp::Subtract).unwrap();
        let wall = (0..m.triangles.len())
            .find(|&i| {
                let [a, ..] = m.tri(i);
                m.face_normal(i).z.abs() < 0.1 && a.truncate().length() < 4.5
            })
            .unwrap();
        let c = circle_at(&m, wall).unwrap();
        assert!((c.radius * 2.0 - 8.0).abs() < 0.1, "{}", c.radius * 2.0);
    }
}
