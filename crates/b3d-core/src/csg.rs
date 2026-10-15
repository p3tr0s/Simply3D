//! Robust mesh booleans backed by `boolmesh` (a pure-Rust Manifold port).

use crate::mesh::Mesh;
use crate::primitives::cuboid;
use crate::{Error, Result};
use boolmesh::prelude::{compute_boolean, Manifold, OpType};
use glam::{Mat4, Quat, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
}

fn to_manifold(m: &Mesh) -> Result<Manifold> {
    if m.is_empty() {
        return Err(Error::Boolean("empty mesh".into()));
    }
    let pos: Vec<f64> = m.positions.iter().flat_map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect();
    let idx: Vec<usize> = m.triangles.iter().flat_map(|t| t.map(|i| i as usize)).collect();
    Manifold::new(&pos, &idx).map_err(|e| Error::Boolean(format!("input is not a closed manifold ({e}). Try Repair first.")))
}

fn from_manifold(r: &Manifold) -> Mesh {
    let positions = r.ps.iter().map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32)).collect();
    let triangles = r.hs.chunks_exact(3).map(|h| [h[0].tail as u32, h[1].tail as u32, h[2].tail as u32]).collect();
    let mut m = Mesh::new(positions, triangles);
    m.compact();
    m
}

/// Boolean between two world-space meshes.
pub fn boolean(a: &Mesh, b: &Mesh, op: BoolOp) -> Result<Mesh> {
    let ma = to_manifold(a)?;
    let mb = to_manifold(b)?;
    let op = match op {
        BoolOp::Union => OpType::Add,
        BoolOp::Subtract => OpType::Subtract,
        BoolOp::Intersect => OpType::Intersect,
    };
    let r = compute_boolean(&ma, &mb, op).map_err(Error::Boolean)?;
    Ok(from_manifold(&r))
}

/// Apply `op` with `base` as the left operand and each of `others` in turn.
pub fn boolean_many(base: &Mesh, others: &[Mesh], op: BoolOp) -> Result<Mesh> {
    let mut acc = base.clone();
    for o in others {
        acc = boolean(&acc, o, op)?;
    }
    Ok(acc)
}

/// Split a mesh by a plane through `point` with `normal`.
/// Returns (part on the normal side, part on the opposite side); either may be empty.
pub fn cut_by_plane(m: &Mesh, point: Vec3, normal: Vec3) -> Result<(Mesh, Mesh)> {
    let n = normal.try_normalize().ok_or_else(|| Error::Boolean("invalid plane normal".into()))?;
    let b = m.bounds();
    let big = (b.size().length() + (b.center() - point).length()) * 4.0 + 10.0;
    // half-space slab: a big box whose bottom face lies on the plane
    let mut slab = cuboid(Vec3::splat(big));
    let xf = Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Z, n), point) * Mat4::from_translation(Vec3::Z * big * 0.5);
    slab.transform(&xf);
    let above = boolean(m, &slab, BoolOp::Intersect)?;
    let below = boolean(m, &slab, BoolOp::Subtract)?;
    Ok((above, below))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::{cuboid, frustum, sphere};

    #[test]
    fn union_of_overlapping_cubes() {
        let a = cuboid(Vec3::splat(10.0));
        let mut b = cuboid(Vec3::splat(10.0));
        b.translate(Vec3::new(5.0, 0.0, 0.0));
        let u = boolean(&a, &b, BoolOp::Union).unwrap();
        assert!((u.volume() - 1500.0).abs() < 1e-3, "{}", u.volume());
        assert!(analyze(&u).is_watertight());
    }

    #[test]
    fn subtract_hole() {
        let a = cuboid(Vec3::splat(20.0));
        let c = frustum(5.0, 5.0, 40.0, 64);
        let r = boolean(&a, &c, BoolOp::Subtract).unwrap();
        let expected = 8000.0 - c.volume() / 2.0;
        assert!((r.volume() - expected).abs() < 1.0, "{} vs {}", r.volume(), expected);
        assert!(analyze(&r).is_watertight());
    }

    #[test]
    fn intersect_sphere_cube() {
        let a = cuboid(Vec3::splat(10.0));
        let s = sphere(100.0, 32, 16);
        let r = boolean(&a, &s, BoolOp::Intersect).unwrap();
        assert!((r.volume() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn cut_cube_in_half() {
        let a = cuboid(Vec3::splat(10.0));
        let (up, down) = cut_by_plane(&a, Vec3::new(0.0, 0.0, 2.0), Vec3::Z).unwrap();
        assert!((up.volume() - 300.0).abs() < 1e-2, "{}", up.volume());
        assert!((down.volume() - 700.0).abs() < 1e-2, "{}", down.volume());
    }

    #[test]
    fn rejects_open_mesh() {
        let mut a = cuboid(Vec3::ONE);
        a.triangles.pop();
        assert!(boolean(&a, &cuboid(Vec3::ONE), BoolOp::Union).is_err());
    }
}
