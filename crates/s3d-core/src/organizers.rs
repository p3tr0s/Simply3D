//! Storage generators: boxes with lids (slide, snap or hinged) and dividers, and
//! Gridfinity bins and baseplates.

use crate::csg::{boolean, boolean_many, BoolOp};
use crate::mesh::Mesh;
use crate::primitives::{cuboid, frustum};
use glam::{Mat4, Vec2, Vec3};
use std::f32::consts::{FRAC_PI_2, PI};

/// Corner segments used for rounded rectangles.
const SEGS: usize = 8;

/// A rounded rectangle centred on the origin, counter-clockwise.
fn rrect(hx: f32, hy: f32, r: f32) -> Vec<Vec2> {
    let r = r.clamp(0.02, hx.min(hy) - 0.01);
    let (cx, cy) = (hx - r, hy - r);
    let mut pts = Vec::with_capacity(4 * (SEGS + 1));
    for (k, (sx, sy)) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)].into_iter().enumerate() {
        for i in 0..=SEGS {
            let a = (k as f32 + i as f32 / SEGS as f32) * FRAC_PI_2;
            pts.push(Vec2::new(sx * cx + r * a.cos(), sy * cy + r * a.sin()));
        }
    }
    pts
}

/// A closed solid through rounded-rectangle sections: `(z, half x, half y, corner radius)`,
/// bottom to top. Good for tapers, chamfers and straight prisms alike.
pub fn loft(levels: &[(f32, f32, f32, f32)]) -> Mesh {
    let mut m = Mesh::default();
    let n = 4 * (SEGS + 1);
    for &(z, hx, hy, r) in levels {
        m.positions.extend(rrect(hx, hy, r).into_iter().map(|p| p.extend(z)));
    }
    for l in 0..levels.len().saturating_sub(1) {
        let (lo, hi) = ((l * n) as u32, ((l + 1) * n) as u32);
        for j in 0..n as u32 {
            let j1 = (j + 1) % n as u32;
            m.triangles.push([lo + j, lo + j1, hi + j1]);
            m.triangles.push([lo + j, hi + j1, hi + j]);
        }
    }
    let (z0, z1) = (levels[0].0, levels[levels.len() - 1].0);
    let c0 = m.positions.len() as u32;
    m.positions.push(Vec3::new(0.0, 0.0, z0));
    let c1 = c0 + 1;
    m.positions.push(Vec3::new(0.0, 0.0, z1));
    let top = ((levels.len() - 1) * n) as u32;
    for j in 0..n as u32 {
        let j1 = (j + 1) % n as u32;
        m.triangles.push([c0, j1, j]);
        m.triangles.push([c1, top + j, top + j1]);
    }
    m
}

fn prism(z0: f32, z1: f32, hx: f32, hy: f32, r: f32) -> Mesh {
    loft(&[(z0, hx, hy, r), (z1, hx, hy, r)])
}

fn box_at(min: Vec3, max: Vec3) -> Mesh {
    let mut b = cuboid(max - min);
    b.translate((min + max) * 0.5);
    b
}

/// Cylinder along X from x0 to x1 with its axis at (y, z).
fn rod_x(x0: f32, x1: f32, r: f32, y: f32, z: f32) -> Mesh {
    let mut c = frustum(r, r, x1 - x0, 48);
    c.transform(&(Mat4::from_translation(Vec3::new((x0 + x1) * 0.5, y, z)) * Mat4::from_rotation_y(PI * 0.5)));
    c
}

fn moved(mut m: Mesh, d: Vec3) -> Mesh {
    m.translate(d);
    m
}

// ---------- boxes ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LidKind {
    None,
    /// A plate sliding in grooves near the top of the long walls.
    Slide,
    /// A lid with a lip that clicks into a groove inside the walls.
    Snap,
    /// A lid on a pin hinge along the back edge.
    Hinged,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Enclosure {
    /// Outside width, depth and height of the box (without the lid).
    pub size: Vec3,
    pub wall: f32,
    pub floor: f32,
    /// Radius of the vertical corners.
    pub radius: f32,
    pub lid: LidKind,
    pub lid_thickness: f32,
    /// Gap between parts that fit together.
    pub clearance: f32,
    /// Divider walls across the width (X) and the depth (Y).
    pub dividers_x: u32,
    pub dividers_y: u32,
}

impl Default for Enclosure {
    fn default() -> Self {
        Enclosure {
            size: Vec3::new(80.0, 50.0, 35.0),
            wall: 2.0,
            floor: 1.6,
            radius: 4.0,
            lid: LidKind::Snap,
            lid_thickness: 2.0,
            clearance: 0.2,
            dividers_x: 0,
            dividers_y: 0,
        }
    }
}

/// Depth of the snap lid's lip.
const SNAP_LIP: f32 = 6.0;

/// The box, with its lid (and hinge pin) laid out next to it, ready to print.
pub fn enclosure_mesh(e: &Enclosure) -> crate::Result<Mesh> {
    let w = e.wall.max(0.8);
    let (hx, hy, h) = (e.size.x.max(4.0 * w + 4.0) * 0.5, e.size.y.max(4.0 * w + 4.0) * 0.5, e.size.z.max(e.floor + 4.0));
    let r = e.radius.clamp(0.1, hx.min(hy) - 0.5);
    let c = e.clearance.max(0.05);
    let lt = e.lid_thickness.max(0.8);
    let floor = e.floor.clamp(0.4, h - 2.0);
    let (ix, iy, ir) = (hx - w, hy - w, (r - w).max(0.5));

    // body: the outside minus the inside (with a snap groove if there's a snap lid)
    let outer = prism(0.0, h, hx, hy, r);
    let cavity = match e.lid {
        LidKind::Snap if w >= 1.4 => {
            let g = (w * 0.3).min(0.6);
            let b = h - SNAP_LIP;
            loft(&[
                (floor, ix, iy, ir),
                (b + 0.5, ix, iy, ir),
                (b + 0.5 + g, ix + g, iy + g, ir + g),
                (b + 1.7, ix + g, iy + g, ir + g),
                (b + 1.7 + g, ix, iy, ir),
                (h + 1.0, ix, iy, ir),
            ])
        }
        _ => prism(floor, h + 1.0, ix, iy, ir),
    };
    let mut body = boolean(&outer, &cavity, BoolOp::Subtract)?;

    // dividers stop below whatever the lid needs at the top
    let top_gap = match e.lid {
        LidKind::None | LidKind::Hinged => 0.0,
        LidKind::Slide => 1.2 + lt + c + 0.5,
        LidKind::Snap => SNAP_LIP + 0.5,
    };
    let dz = (h - top_gap).max(floor + 1.0);
    let mut divs = vec![];
    for i in 1..=e.dividers_x {
        let x = -ix + 2.0 * ix * i as f32 / (e.dividers_x + 1) as f32;
        divs.push(box_at(Vec3::new(x - w * 0.5, -(hy - w * 0.5), floor - 0.2), Vec3::new(x + w * 0.5, hy - w * 0.5, dz)));
    }
    for i in 1..=e.dividers_y {
        let y = -iy + 2.0 * iy * i as f32 / (e.dividers_y + 1) as f32;
        divs.push(box_at(Vec3::new(-(hx - w * 0.5), y - w * 0.5, floor - 0.2), Vec3::new(hx - w * 0.5, y + w * 0.5, dz)));
    }
    if !divs.is_empty() {
        body = boolean_many(&body, &divs, BoolOp::Union)?;
    }

    let beside = Vec3::new(2.0 * hx + 8.0, 0.0, 0.0);
    let mut out = match e.lid {
        LidKind::None => body,
        LidKind::Snap => {
            // printed upside down: the plate on the bed, the lip pointing up
            let a = ix - c;
            let lw = w.max(1.2);
            let tip = lt + SNAP_LIP - 0.5;
            let bead = (w * 0.3).min(0.6) * 0.8;
            let plate = prism(0.0, lt, hx, hy, r);
            let lip = loft(&[
                (lt - 0.2, a, iy - c, (ir - c).max(0.3)),
                (tip - 1.6, a, iy - c, (ir - c).max(0.3)),
                (tip - 1.2, a + bead, iy - c + bead, (ir - c + bead).max(0.3)),
                (tip - 0.6, a + bead, iy - c + bead, (ir - c + bead).max(0.3)),
                (tip - 0.2, a, iy - c, (ir - c).max(0.3)),
                (tip, a, iy - c, (ir - c).max(0.3)),
            ]);
            let hollow = prism(lt, tip + 1.0, a - lw, iy - c - lw, (ir - c - lw).max(0.3));
            let lid = boolean(&boolean(&plate, &lip, BoolOp::Union)?, &hollow, BoolOp::Subtract)?;
            let mut out = body;
            out.append(&moved(lid, beside));
            out
        }
        LidKind::Slide => {
            // grooves in the long (Y) walls, open at the +X end
            let land = 1.2;
            let (gz0, gz1) = (h - land - lt - c, h - land);
            let gd = w * 0.5;
            let grooves = [
                box_at(Vec3::new(-ix + 0.01 - gd, iy - 0.01, gz0), Vec3::new(hx + 1.0, iy + gd, gz1)),
                box_at(Vec3::new(-ix + 0.01 - gd, -iy - gd, gz0), Vec3::new(hx + 1.0, -iy + 0.01, gz1)),
                // the front wall comes down to the grooves so the lid can slide in
                box_at(Vec3::new(ix - 0.01, -iy - gd, gz0), Vec3::new(hx + 1.0, iy + gd, h + 1.0)),
            ];
            let body = boolean_many(&body, &grooves, BoolOp::Subtract)?;
            let (lx0, lx1) = (-ix - gd + c, hx);
            let ly = iy + gd - c;
            let plate = box_at(Vec3::new(lx0, -ly, 0.0), Vec3::new(lx1, ly, lt));
            // a ridge to push it with
            let grip = box_at(Vec3::new(lx1 - 6.0, -ly * 0.5, lt - 0.1), Vec3::new(lx1 - 4.0, ly * 0.5, lt + 1.0));
            let lid = boolean(&plate, &grip, BoolOp::Union)?;
            let mut out = body;
            out.append(&moved(lid, beside - Vec3::new((lx0 + lx1) * 0.5, 0.0, 0.0)));
            out
        }
        LidKind::Hinged => {
            let pin_r = 0.9; // fits a piece of 1.75 mm filament, or the printed pin
            let kr = (pin_r + c + 1.4).max(2.4);
            let ya = -hy - kr - c;
            let za = h - kr - c;
            let span = 2.0 * (hx - r).max(hx * 0.5);
            let n = 5usize;
            let seg = (span - (n - 1) as f32 * c) / n as f32;
            let x_of = |i: usize| -span * 0.5 + i as f32 * (seg + c);
            let mut body_parts = vec![];
            let mut lid_parts = vec![];
            for i in 0..n {
                let (x0, x1) = (x_of(i), x_of(i) + seg);
                if i % 2 == 0 {
                    body_parts.push(rod_x(x0, x1, kr, ya, za));
                    body_parts.push(box_at(Vec3::new(x0, ya, za - kr), Vec3::new(x1, -hy + w * 0.5, za)));
                } else {
                    lid_parts.push(rod_x(x0, x1, kr, ya, za));
                    lid_parts.push(box_at(Vec3::new(x0, ya - kr, za), Vec3::new(x1, ya, h + 0.2)));
                }
            }
            let body = boolean_many(&body, &body_parts, BoolOp::Union)?;
            // the pin is snug in the body's knuckles and turns in the lid's
            let body = boolean(&body, &rod_x(-hx - 1.0, hx + 1.0, pin_r + 0.05, ya, za), BoolOp::Subtract)?;
            // the lid, closed, reaching back over the hinge
            let plate = loft(&[(h, hx, hy, r), (h + lt, hx, hy, r)]);
            let back = box_at(Vec3::new(-hx + r, ya - kr, h), Vec3::new(hx - r, -hy + r, h + lt));
            let mut lid = boolean_many(&plate, &[back], BoolOp::Union)?;
            lid = boolean_many(&lid, &lid_parts, BoolOp::Union)?;
            lid = boolean(&lid, &rod_x(-hx - 1.0, hx + 1.0, pin_r + c, ya, za), BoolOp::Subtract)?;
            // flip it to print top-down, next to the body
            lid.transform(&Mat4::from_rotation_x(PI));
            let lb = lid.bounds();
            lid.translate(Vec3::new(beside.x, -lb.center().y, -lb.min.z));
            let mut pin = rod_x(-span * 0.5 - 1.0, span * 0.5 + 1.0, pin_r, 0.0, 0.0);
            pin.translate(Vec3::new(0.0, -hy - kr - 6.0, pin_r));
            let mut out = body;
            out.append(&lid);
            out.append(&pin);
            out
        }
    };
    out.tidy_paint();
    Ok(out)
}

// ---------- Gridfinity ----------

/// Gridfinity grid pitch and height unit, mm.
pub const GRID: f32 = 42.0;
pub const HEIGHT_UNIT: f32 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GridBin {
    pub units_x: u32,
    pub units_y: u32,
    /// Height in 7 mm units (not counting the stacking lip).
    pub height_units: u32,
    pub compartments_x: u32,
    pub compartments_y: u32,
    /// 6 × 2 mm magnet pockets under each grid square.
    pub magnets: bool,
    /// The stacking lip on top, so bins stack on each other.
    pub lip: bool,
    pub wall: f32,
}

impl Default for GridBin {
    fn default() -> Self {
        GridBin { units_x: 2, units_y: 1, height_units: 3, compartments_x: 1, compartments_y: 1, magnets: false, lip: true, wall: 1.2 }
    }
}

/// The base profile under each grid square (0.8 chamfer, 1.8 straight, 2.15 chamfer),
/// up to 4.75 mm, plus a little overlap into the body above.
fn bin_foot(cx: f32, cy: f32) -> Mesh {
    let mut f =
        loft(&[(0.0, 17.8, 17.8, 0.8), (0.8, 18.6, 18.6, 1.6), (2.6, 18.6, 18.6, 1.6), (4.75, 20.75, 20.75, 3.75), (5.2, 20.5, 20.5, 3.5)]);
    f.translate(Vec3::new(cx, cy, 0.0));
    f
}

fn cell_centres(nx: u32, ny: u32) -> Vec<(f32, f32)> {
    let mut v = vec![];
    for i in 0..nx {
        for j in 0..ny {
            v.push(((i as f32 - (nx as f32 - 1.0) * 0.5) * GRID, (j as f32 - (ny as f32 - 1.0) * 0.5) * GRID));
        }
    }
    v
}

pub fn gridfinity_bin(b: &GridBin) -> crate::Result<Mesh> {
    let (nx, ny) = (b.units_x.clamp(1, 8), b.units_y.clamp(1, 8));
    let hu = b.height_units.clamp(2, 20);
    let w = b.wall.clamp(0.8, 3.0);
    let (hx, hy) = (nx as f32 * GRID * 0.5 - 0.25, ny as f32 * GRID * 0.5 - 0.25);
    let r = 3.75;
    let h = hu as f32 * HEIGHT_UNIT;
    let lip = if b.lip { 4.4 } else { 0.0 };
    let top = h + lip;
    let floor_top = 4.75 + 1.0;

    let mut parts: Vec<Mesh> = cell_centres(nx, ny).into_iter().map(|(x, y)| bin_foot(x, y)).collect();
    let first = parts.remove(0);
    let feet = if parts.is_empty() { first } else { boolean_many(&first, &parts, BoolOp::Union)? };
    let shell = prism(4.75, top, hx, hy, r);
    let mut bin = boolean(&feet, &shell, BoolOp::Union)?;

    // the inside: straight walls, then (with a lip) a ledge and the stacking profile
    let (ix, iy, ir) = (hx - w, hy - w, (r - w).max(0.6));
    let cavity = if b.lip {
        // the lip seats the next bin's feet: 45° shelf, 1.8 straight, 45° out to the top
        let (lx, ly) = (hx - 2.6, hy - 2.6);
        let shelf = (w - 2.6).abs();
        loft(&[
            (floor_top, ix, iy, ir),
            (h - shelf - 0.01, ix, iy, ir),
            (h, lx, ly, 1.15),
            (h + 0.7, lx + 0.7, ly + 0.7, 1.85),
            (h + 2.5, lx + 0.7, ly + 0.7, 1.85),
            (top, hx - 0.4, hy - 0.4, 3.35),
            (top + 1.0, hx - 0.4, hy - 0.4, 3.35),
        ])
    } else {
        prism(floor_top, top + 1.0, ix, iy, ir)
    };
    bin = boolean(&bin, &cavity, BoolOp::Subtract)?;

    // compartments
    let cz = if b.lip { h - 2.0 } else { top - 1.0 };
    let mut divs = vec![];
    for i in 1..b.compartments_x.clamp(1, 12) {
        let x = -ix + 2.0 * ix * i as f32 / b.compartments_x as f32;
        divs.push(box_at(Vec3::new(x - w * 0.5, -(hy - w * 0.5), floor_top - 0.2), Vec3::new(x + w * 0.5, hy - w * 0.5, cz)));
    }
    for i in 1..b.compartments_y.clamp(1, 12) {
        let y = -iy + 2.0 * iy * i as f32 / b.compartments_y as f32;
        divs.push(box_at(Vec3::new(-(hx - w * 0.5), y - w * 0.5, floor_top - 0.2), Vec3::new(hx - w * 0.5, y + w * 0.5, cz)));
    }
    if !divs.is_empty() {
        bin = boolean_many(&bin, &divs, BoolOp::Union)?;
    }

    if b.magnets {
        let mut holes = vec![];
        for (cx, cy) in cell_centres(nx, ny) {
            for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                let mut m = frustum(3.25, 3.25, 2.4, 32);
                m.translate(Vec3::new(cx + sx * 13.0, cy + sy * 13.0, 1.2 - 0.01));
                holes.push(m);
            }
        }
        bin = boolean_many(&bin, &holes, BoolOp::Subtract)?;
    }
    Ok(bin)
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GridPlate {
    pub units_x: u32,
    pub units_y: u32,
}

impl Default for GridPlate {
    fn default() -> Self {
        GridPlate { units_x: 3, units_y: 3 }
    }
}

/// A lightweight Gridfinity baseplate: a frame with one open socket per grid square.
pub fn gridfinity_baseplate(p: &GridPlate) -> crate::Result<Mesh> {
    let (nx, ny) = (p.units_x.clamp(1, 12), p.units_y.clamp(1, 12));
    let h = 4.65;
    // a quarter millimetre of rim so the outer edge isn't knife-thin
    let plate = prism(0.0, h, nx as f32 * GRID * 0.5 + 0.25, ny as f32 * GRID * 0.5 + 0.25, 4.0);
    let sockets: Vec<Mesh> = cell_centres(nx, ny)
        .into_iter()
        .map(|(x, y)| {
            let mut s = loft(&[
                (-0.5, 18.15, 18.15, 0.8),
                (0.0, 18.15, 18.15, 0.8),
                (0.7, 18.85, 18.85, 1.5),
                (2.5, 18.85, 18.85, 1.5),
                (h, 21.0, 21.0, 4.0),
                (h + 0.5, 21.0, 21.0, 4.0),
            ]);
            s.translate(Vec3::new(x, y, 0.0));
            s
        })
        .collect();
    boolean_many(&plate, &sockets, BoolOp::Subtract)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    fn check(m: &Mesh, parts: usize) {
        assert!(!m.is_empty());
        let r = analyze(m);
        assert!(r.is_watertight(), "{:?}", r.problems());
        assert_eq!(m.connected_components().len(), parts);
        assert!(m.volume() > 0.0);
    }

    #[test]
    fn loft_is_closed() {
        let m = loft(&[(0.0, 10.0, 5.0, 1.0), (3.0, 12.0, 7.0, 2.0), (8.0, 12.0, 7.0, 2.0)]);
        check(&m, 1);
    }

    #[test]
    fn boxes_with_every_lid() {
        for (lid, parts) in [(LidKind::None, 1), (LidKind::Snap, 2), (LidKind::Slide, 2), (LidKind::Hinged, 3)] {
            let e = Enclosure { lid, dividers_x: 1, dividers_y: 1, ..Default::default() };
            let m = enclosure_mesh(&e).unwrap();
            check(&m, parts);
            let b = m.bounds();
            assert!((b.max.z - b.min.z - e.size.z).abs() < 0.5, "{lid:?} {:?}", b);
        }
    }

    #[test]
    fn gridfinity_sizes() {
        let bin = gridfinity_bin(&GridBin { magnets: true, compartments_x: 2, ..Default::default() }).unwrap();
        check(&bin, 1);
        let s = bin.bounds().size();
        assert!((s.x - 83.5).abs() < 0.05 && (s.y - 41.5).abs() < 0.05, "{s}");
        assert!((s.z - (21.0 + 4.4)).abs() < 0.05, "{s}");
        let plate = gridfinity_baseplate(&GridPlate::default()).unwrap();
        check(&plate, 1);
        let s = plate.bounds().size();
        assert!((s.x - 126.5).abs() < 0.05 && (s.z - 4.65).abs() < 0.05, "{s}");
    }
}
