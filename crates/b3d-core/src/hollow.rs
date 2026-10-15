//! Hollow a solid: keep a shell of a given wall thickness. The inside surface comes from a
//! distance field (offset inwards), the outside stays exactly as it was.

use crate::csg::{boolean, BoolOp};
use crate::mesh::Mesh;
use crate::sdf;
use crate::{Error, Progress, Result};
use glam::Vec3;

/// Detail used for the inside surface when `voxel` is 0: fine enough for the wall, but
/// bounded so big parts stay quick.
pub fn auto_voxel(wall: f32, m: &Mesh) -> f32 {
    (wall / 3.0).max(m.bounds().size().max_element() / 250.0).max(0.05)
}

/// Hollow `m` (closed, world space) leaving walls `wall` mm thick. Returns the shell and
/// the voxel size used for the inside.
pub fn hollow(m: &Mesh, wall: f32, voxel: f32, prog: &Progress) -> Result<(Mesh, f32)> {
    let wall = wall.max(0.1);
    let v = if voxel > 0.0 { voxel } else { auto_voxel(wall, m) };
    prog.stage("Inside surface");
    let (inner, h) = match sdf::offset_with(m, -wall, v, 20_000_000, &prog.range(0.0, 0.7)) {
        Ok(r) => r,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(Error::Parse(s)) if s.contains("empty") => {
            return Err(Error::Parse(format!("nothing is thicker than two {wall} mm walls — use thinner walls")))
        }
        Err(e) => return Err(e),
    };
    if inner.is_empty() || inner.volume() <= 0.0 {
        return Err(Error::Parse(format!("nothing is thicker than two {wall} mm walls — use thinner walls")));
    }
    prog.check()?;
    prog.stage("Removing the inside");
    let out = boolean(m, &inner, BoolOp::Subtract)?;
    prog.set(1.0);
    Ok((out, h))
}

/// Where a drain hole goes: the middle of the lowest downward-facing area near the
/// centre of the part. Returns the point on the bottom surface.
pub fn drain_spot(m: &Mesh, wall: f32) -> Option<Vec3> {
    let b = m.bounds();
    let c = b.center();
    (0..m.triangles.len())
        .filter(|&i| m.face_normal(i).z < -0.7)
        .map(|i| {
            let [p, q, r] = m.tri(i);
            ((p + q + r) / 3.0, i)
        })
        .filter(|(p, _)| p.z < b.min.z + wall.max(1.0))
        .min_by(|a, b2| {
            let d = |p: Vec3| (p.x - c.x).powi(2) + (p.y - c.y).powi(2);
            d(a.0).total_cmp(&d(b2.0))
        })
        .map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn hollow_cube_keeps_a_shell() {
        let m = cuboid(Vec3::splat(20.0));
        let (s, _) = hollow(&m, 2.0, 0.25, &Progress::new()).unwrap();
        assert!(analyze(&s).is_watertight());
        // 20³ − 16³ (the voxel inside has slightly rounded corners: a little more material)
        let v = s.volume();
        assert!(v > 8000.0 - 4096.0 - 1.0 && v < 8000.0 - 4096.0 + 150.0, "{v}");
        assert_eq!(s.connected_components().len(), 2, "outer and inner skins");
        assert!(drain_spot(&s, 2.0).unwrap().z < -9.9);
    }

    #[test]
    fn too_thin_to_hollow() {
        let m = cuboid(Vec3::new(20.0, 20.0, 3.0));
        assert!(hollow(&m, 2.0, 0.25, &Progress::new()).is_err());
    }
}
