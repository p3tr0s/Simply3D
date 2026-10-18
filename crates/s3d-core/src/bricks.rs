//! Toy building bricks that fit LEGO® (8 mm system) and DUPLO® (16 mm system) bricks,
//! with fit adjustments for 3D printing.
//!
//! The underside tubes and pins are sized from the stud size and pitch, so they always
//! touch the studs they clamp; the fit settings then make the studs and tubes a little
//! fatter or thinner to suit a printer.

use crate::csg::{boolean, boolean_many, BoolOp};
use crate::mesh::Mesh;
use crate::primitives::{cuboid, frustum};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BrickSystem {
    /// 8 mm pitch, 9.6 mm bricks, 3.2 mm plates.
    Lego,
    /// Twice the size: 16 mm pitch, 19.2 mm bricks.
    Duplo,
}

/// Nominal sizes of a system, mm.
#[derive(Debug, Clone, Copy)]
pub struct System {
    pub pitch: f32,
    /// Height step: a LEGO plate (3.2) or half a DUPLO brick (9.6).
    pub unit: f32,
    /// Units in a full brick.
    pub brick_units: u32,
    /// Taken off each outside face so neighbouring bricks don't rub.
    pub gap: f32,
    pub stud_d: f32,
    pub stud_h: f32,
    /// Hole in hollow studs.
    pub stud_hole: f32,
    pub wall: f32,
    pub top: f32,
    /// Wall thickness of the round tubes underneath.
    pub tube_wall: f32,
}

impl BrickSystem {
    pub fn dims(self) -> System {
        match self {
            BrickSystem::Lego => System {
                pitch: 8.0,
                unit: 3.2,
                brick_units: 3,
                gap: 0.1,
                stud_d: 4.8,
                stud_h: 1.7,
                stud_hole: 3.0,
                wall: 1.5,
                top: 1.0,
                tube_wall: 0.85,
            },
            BrickSystem::Duplo => System {
                pitch: 16.0,
                unit: 9.6,
                brick_units: 2,
                gap: 0.15,
                stud_d: 9.4,
                stud_h: 4.5,
                stud_hole: 6.5,
                wall: 1.55,
                top: 1.6,
                tube_wall: 1.3,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Brick {
    pub system: BrickSystem,
    pub studs_x: u32,
    pub studs_y: u32,
    /// Height in units: LEGO plates (3 = brick), or DUPLO half bricks (2 = brick).
    pub height: u32,
    /// Studs on top (off: a smooth tile).
    pub studs: bool,
    pub hollow_studs: bool,
    /// Make the studs fatter (+) or thinner (−), mm on the diameter.
    pub stud_fit: f32,
    /// Make the tubes and pins underneath fatter (+, tighter grip) or thinner (−), mm.
    pub tube_fit: f32,
}

impl Brick {
    pub fn lego(studs_x: u32, studs_y: u32, height: u32, studs: bool) -> Brick {
        Brick { system: BrickSystem::Lego, studs_x, studs_y, height, studs, hollow_studs: false, stud_fit: 0.0, tube_fit: 0.0 }
    }
    pub fn duplo(studs_x: u32, studs_y: u32) -> Brick {
        Brick { system: BrickSystem::Duplo, studs_x, studs_y, height: 2, studs: true, hollow_studs: true, stud_fit: 0.0, tube_fit: 0.0 }
    }
}

impl Default for Brick {
    fn default() -> Self {
        Brick::lego(4, 2, 3, true)
    }
}

fn box_at(min: Vec3, max: Vec3) -> Mesh {
    let mut b = cuboid(max - min);
    b.translate((min + max) * 0.5);
    b
}

/// Upright cylinder from z0 to z1.
fn cyl(x: f32, y: f32, z0: f32, z1: f32, d: f32, seg: u32) -> Mesh {
    let mut c = frustum(d * 0.5, d * 0.5, z1 - z0, seg);
    c.translate(Vec3::new(x, y, (z0 + z1) * 0.5));
    c
}

fn tube(x: f32, y: f32, z0: f32, z1: f32, od: f32, id: f32) -> crate::Result<Mesh> {
    boolean(&cyl(x, y, z0, z1, od, 48), &cyl(x, y, z0 - 1.0, z1 + 1.0, id, 48), BoolOp::Subtract)
}

/// A brick, plate or tile standing on the plate, centred on X/Y.
pub fn brick_mesh(b: &Brick) -> crate::Result<Mesh> {
    let s = b.system.dims();
    let (nx, ny) = (b.studs_x.clamp(1, 48), b.studs_y.clamp(1, 48));
    let p = s.pitch;
    let h = b.height.clamp(1, 36) as f32 * s.unit;
    let (hx, hy) = (nx as f32 * p * 0.5 - s.gap, ny as f32 * p * 0.5 - s.gap);
    let top = s.top.min(h * 0.5);
    let w = s.wall;
    let stud_d = (s.stud_d + b.stud_fit).max(0.5);
    let pos = |i: u32, n: u32| (i as f32 - (n as f32 - 1.0) * 0.5) * p;

    // shell: open underneath
    let outer = box_at(Vec3::new(-hx, -hy, 0.0), Vec3::new(hx, hy, h));
    let inner = box_at(Vec3::new(-hx + w, -hy + w, -1.0), Vec3::new(hx - w, hy - w, h - top));
    let mut brick = boolean(&outer, &inner, BoolOp::Subtract)?;

    let mut add: Vec<Mesh> = vec![];
    // what holds the studs of the brick below: tubes between four studs, pins between two
    let below = h - top + 0.01;
    let fit = b.tube_fit;
    if nx >= 2 && ny >= 2 {
        let od = 2.0 * (p * std::f32::consts::FRAC_1_SQRT_2 - s.stud_d * 0.5) + fit;
        let id = (od - 2.0 * s.tube_wall).max(0.5);
        for i in 0..nx - 1 {
            for j in 0..ny - 1 {
                let (x, y) = (pos(i, nx) + p * 0.5, pos(j, ny) + p * 0.5);
                add.push(tube(x, y, 0.0, below, od, id)?);
            }
        }
    } else if nx.max(ny) >= 2 {
        let d = 2.0 * (p * 0.5 - s.stud_d * 0.5) + fit;
        let n = nx.max(ny);
        for i in 0..n - 1 {
            let c = pos(i, n) + p * 0.5;
            let (x, y) = if nx >= ny { (c, 0.0) } else { (0.0, c) };
            add.push(cyl(x, y, 0.0, below, d, 32));
        }
    }
    // where the walls don't reach the studs (DUPLO), ribs on the walls grip them
    let reach = p * 0.5 - s.gap - w - s.stud_d * 0.5;
    if reach > 0.05 {
        let rib = 1.2;
        let depth = reach - fit * 0.5;
        for i in 0..nx {
            let x = pos(i, nx);
            add.push(box_at(Vec3::new(x - rib * 0.5, -hy + w - 0.01, 0.0), Vec3::new(x + rib * 0.5, -hy + w + depth, below)));
            add.push(box_at(Vec3::new(x - rib * 0.5, hy - w - depth, 0.0), Vec3::new(x + rib * 0.5, hy - w + 0.01, below)));
        }
        for j in 0..ny {
            let y = pos(j, ny);
            add.push(box_at(Vec3::new(-hx + w - 0.01, y - rib * 0.5, 0.0), Vec3::new(-hx + w + depth, y + rib * 0.5, below)));
            add.push(box_at(Vec3::new(hx - w - depth, y - rib * 0.5, 0.0), Vec3::new(hx - w + 0.01, y + rib * 0.5, below)));
        }
    }
    // studs on top
    if b.studs {
        for i in 0..nx {
            for j in 0..ny {
                let (x, y) = (pos(i, nx), pos(j, ny));
                let stud = cyl(x, y, h - 0.01, h + s.stud_h, stud_d, 48);
                if b.hollow_studs {
                    add.push(boolean(&stud, &cyl(x, y, h + 0.3, h + s.stud_h + 1.0, s.stud_hole.min(stud_d - 1.0), 32), BoolOp::Subtract)?);
                } else {
                    add.push(stud);
                }
            }
        }
    }
    if !add.is_empty() {
        brick = boolean_many(&brick, &add, BoolOp::Union)?;
    }
    Ok(brick)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    #[test]
    fn bricks_fit_their_system() {
        for (b, size) in [
            (Brick::lego(4, 2, 3, true), Vec3::new(31.8, 15.8, 9.6 + 1.7)),
            (Brick::lego(1, 4, 1, true), Vec3::new(7.8, 31.8, 3.2 + 1.7)),
            (Brick::lego(2, 2, 1, false), Vec3::new(15.8, 15.8, 3.2)),
            (Brick::lego(1, 1, 3, true), Vec3::new(7.8, 7.8, 9.6 + 1.7)),
            (Brick::duplo(2, 4), Vec3::new(31.7, 63.7, 19.2 + 4.5)),
            (Brick { height: 1, ..Brick::duplo(4, 2) }, Vec3::new(63.7, 31.7, 9.6 + 4.5)),
            (Brick { height: 1, studs: false, ..Brick::duplo(2, 2) }, Vec3::new(31.7, 31.7, 9.6)),
            (Brick { height: 1, ..Brick::duplo(1, 2) }, Vec3::new(15.7, 31.7, 9.6 + 4.5)),
        ] {
            let m = brick_mesh(&b).unwrap();
            assert!(analyze(&m).is_watertight(), "{b:?}");
            assert_eq!(m.connected_components().len(), 1);
            let got = m.bounds().size();
            assert!((got - size).abs().max_element() < 0.02, "{b:?}: {got} vs {size}");
        }
        // the standard tube for 8 mm bricks comes out at 6.51 mm
        let s = BrickSystem::Lego.dims();
        let od = 2.0 * (s.pitch * std::f32::consts::FRAC_1_SQRT_2 - s.stud_d * 0.5);
        assert!((od - 6.51).abs() < 0.01, "{od}");
    }
}
