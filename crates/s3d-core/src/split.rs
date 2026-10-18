//! Split a model that's too big for the printer into pieces that fit, cut with straight
//! planes and joined with pegs or dowels, laid out ready to print.

use crate::connectors::{cut_with_connectors, ConnectorKind, ConnectorOptions};
use crate::mesh::Mesh;
use crate::{Progress, Result};
use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitOptions {
    /// Usable build volume of the printer, mm.
    pub volume: Vec3,
    /// Kept free around each piece (brim, skirt, a little slack).
    pub margin: f32,
    pub connectors: ConnectorOptions,
}

impl Default for SplitOptions {
    fn default() -> Self {
        SplitOptions {
            volume: Vec3::new(256.0, 256.0, 256.0),
            margin: 5.0,
            connectors: ConnectorOptions { kind: ConnectorKind::Pegs, ..Default::default() },
        }
    }
}

pub struct SplitResult {
    /// The pieces (and any dowel pins), world space, laid out on the plate.
    pub pieces: Vec<Mesh>,
    pub dowels: usize,
    /// Pieces along X, Y and Z.
    pub grid: [u32; 3],
    /// Whether the model was turned 90° to need fewer pieces.
    pub turned: bool,
}

/// How many pieces along each axis a box of `size` needs for a printer `room`.
pub fn pieces_needed(size: Vec3, room: Vec3) -> [u32; 3] {
    let n = |s: f32, r: f32| ((s / r.max(1.0)) - 1e-4).ceil().max(1.0) as u32;
    [n(size.x, room.x), n(size.y, room.y), n(size.z, room.z)]
}

/// Whether a model of this size needs splitting at all (either way round on the plate).
pub fn fits(size: Vec3, o: &SplitOptions) -> bool {
    let room = o.volume - Vec3::splat(o.margin * 2.0).with_z(0.0);
    let a = pieces_needed(size, room);
    let b = pieces_needed(Vec3::new(size.y, size.x, size.z), room);
    a == [1, 1, 1] || b == [1, 1, 1]
}

/// Split a world-space mesh. Pieces come out standing on the plate in rows starting at
/// the model's own corner.
pub fn split_to_fit(m: &Mesh, o: &SplitOptions, progress: &Progress) -> Result<SplitResult> {
    let room = o.volume - Vec3::new(o.margin * 2.0, o.margin * 2.0, 0.0);
    let b = m.bounds();
    let size = b.size();
    let straight = pieces_needed(size, room);
    let turned_n = pieces_needed(Vec3::new(size.y, size.x, size.z), room);
    let count = |g: [u32; 3]| g[0] * g[1] * g[2];
    let turned = count(turned_n) < count(straight);
    let mut mesh = m.clone();
    if turned {
        let c = b.center();
        mesh.transform(&(Mat4::from_translation(c) * Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2) * Mat4::from_translation(-c)));
    }
    let grid = if turned { turned_n } else { straight };
    let b = mesh.bounds();
    let mut pieces = vec![mesh];
    let mut dowels = vec![];
    let total_cuts = (grid[0] - 1 + grid[1] - 1 + grid[2] - 1).max(1) as f32;
    let mut done = 0.0;
    for (axis, n) in grid.iter().enumerate() {
        let normal = Vec3::AXES[axis];
        for k in 1..*n {
            let at = b.min[axis] + b.size()[axis] * k as f32 / *n as f32;
            let point = b.center() + normal * (at - b.center()[axis]);
            let sub = progress.range(done / total_cuts, (done + 1.0) / total_cuts);
            done += 1.0;
            let mut next = vec![];
            for p in pieces {
                progress.check()?;
                let pb = p.bounds();
                if pb.max[axis] <= at + 1e-4 || pb.min[axis] >= at - 1e-4 {
                    next.push(p);
                    continue;
                }
                let r = cut_with_connectors(&p, point, normal, &o.connectors, &sub)?;
                for half in [r.below, r.above] {
                    if !half.is_empty() {
                        next.push(half);
                    }
                }
                dowels.extend(r.dowels);
            }
            pieces = next;
        }
    }
    // separate shells a cut may have made stay together with their piece; lay out
    let n_dowels = dowels.len();
    pieces.extend(dowels);
    layout(&mut pieces, b.min, o);
    Ok(SplitResult { pieces, dowels: n_dowels, grid, turned })
}

/// Stand every piece on the plate and set them out in rows (shelf packing) within the
/// printer's width, starting at `origin`.
fn layout(pieces: &mut [Mesh], origin: Vec3, o: &SplitOptions) {
    let gap = o.margin.max(3.0) * 2.0;
    let width = o.volume.x.max(50.0);
    // biggest first gives tidier rows
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by(|&a, &b| pieces[b].bounds().size().y.total_cmp(&pieces[a].bounds().size().y));
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    for i in order {
        let pb = pieces[i].bounds();
        let s = pb.size();
        if x > 0.0 && x + s.x > width {
            x = 0.0;
            y += row_h + gap;
            row_h = 0.0;
        }
        let target = Vec3::new(origin.x + x, origin.y + y, 0.0);
        pieces[i].translate(target - Vec3::new(pb.min.x, pb.min.y, pb.min.z));
        x += s.x + gap;
        row_h = row_h.max(s.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn splits_into_pieces_that_fit() {
        let mut big = cuboid(Vec3::new(300.0, 120.0, 40.0));
        big.translate(Vec3::new(0.0, 0.0, 20.0));
        let o = SplitOptions { volume: Vec3::new(180.0, 180.0, 180.0), ..Default::default() };
        assert!(!fits(big.bounds().size(), &o));
        let r = split_to_fit(&big, &o, &Progress::new()).unwrap();
        assert_eq!(r.grid, [2, 1, 1]);
        assert!(!r.turned);
        assert_eq!(r.pieces.len(), 2);
        let vol: f64 = r.pieces.iter().map(|p| p.volume()).sum();
        // pegs add a little, holes take a little
        assert!((vol - 300.0 * 120.0 * 40.0).abs() < 2000.0, "{vol}");
        for p in &r.pieces {
            assert!(analyze(p).is_watertight());
            let s = p.bounds().size();
            assert!(s.x <= 170.0 + 7.0 && s.y <= 170.0 && p.bounds().min.z.abs() < 1e-3, "{s}");
        }
        // turning saves pieces: 120 × 300 on a 300 × 130 plate
        let o = SplitOptions { volume: Vec3::new(310.0, 140.0, 200.0), ..Default::default() };
        let mut tall = cuboid(Vec3::new(120.0, 300.0, 30.0));
        tall.translate(Vec3::new(0.0, 0.0, 15.0));
        let r = split_to_fit(&tall, &o, &Progress::new()).unwrap();
        assert!(r.turned && r.pieces.len() == 1, "{:?}", r.grid);
        // tall things split in Z too
        let mut tower = cuboid(Vec3::new(40.0, 40.0, 400.0));
        tower.translate(Vec3::new(0.0, 0.0, 200.0));
        let r = split_to_fit(&tower, &SplitOptions::default(), &Progress::new()).unwrap();
        assert_eq!(r.grid, [1, 1, 2]);
        assert!(r.pieces.iter().all(|p| p.bounds().size().z <= 256.0));
    }
}
