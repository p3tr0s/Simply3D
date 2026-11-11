//! Support fins: thin, break-away walls standing under overhangs — the ceilings of holes
//! and slots through a wall — so they print cleanly without slicer supports.
//!
//! Each overhang area gets a row of thin vertical fins, running across its short side
//! (so they slide out of a slot), standing on whatever is below and stopping a small
//! gap under the ceiling so they snap off. Overhangs inside a closed cavity get none:
//! fins there could never be removed.

use crate::bvh::Bvh;
use crate::mesh::Mesh;
use crate::progress::Progress;
use crate::{Error, Result};
use glam::{Vec2, Vec3};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FinDir {
    /// Across each overhang's short side.
    Auto,
    X,
    Y,
}

#[derive(Debug, Clone)]
pub struct FinOptions {
    /// Faces steeper than this from vertical, in degrees, get fins (45 = slicer default).
    pub angle: f32,
    /// Fin thickness, mm (two lines of a 0.4 nozzle = 0.8).
    pub thickness: f32,
    /// Distance between fins, mm.
    pub spacing: f32,
    /// Gap under the ceiling and at the ends, mm (about one layer).
    pub gap: f32,
    pub dir: FinDir,
    /// Only where the part itself is below (holes and slots); off = also down to the plate.
    pub openings_only: bool,
    /// Only these triangles (a face selection), if given.
    pub faces: Option<Vec<bool>>,
}

impl Default for FinOptions {
    fn default() -> Self {
        Self { angle: 45.0, thickness: 0.8, spacing: 4.0, gap: 0.2, dir: FinDir::Auto, openings_only: true, faces: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FinReport {
    pub fins: usize,
    /// Overhang areas that got fins.
    pub areas: usize,
    /// Overhang areas skipped because they are inside a closed cavity.
    pub enclosed: usize,
}

/// Openings lower than this need no fins, mm.
const MIN_HEIGHT: f32 = 1.0;
/// Overhang areas smaller than this are ignored, mm².
const MIN_AREA: f32 = 0.5;
/// How far fins reach into the surface they stand on, so they fuse with it, mm.
const SINK: f32 = 0.1;
const MAX_FINS: usize = 5000;

/// Downward-facing triangles grouped into connected overhang areas.
fn overhang_areas(m: &Mesh, o: &FinOptions) -> Vec<Vec<usize>> {
    let limit = -o.angle.clamp(1.0, 89.0).to_radians().sin();
    let plate = m.bounds().min.z.max(0.0) + 0.05;
    let down: Vec<bool> = (0..m.triangles.len())
        .map(|i| {
            if o.faces.as_ref().is_some_and(|f| !f.get(i).copied().unwrap_or(false)) {
                return false;
            }
            let [a, b, c] = m.tri(i);
            let n = (b - a).cross(c - a);
            let len = n.length();
            len > 1e-9 && n.z / len < limit && a.z.max(b.z).max(c.z) > plate
        })
        .collect();
    // join triangles that share an edge (by position, so unwelded meshes work too)
    let key = |p: Vec3| ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64, (p.z * 1e4).round() as i64);
    let mut parent: Vec<usize> = (0..m.triangles.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    type Key = (i64, i64, i64);
    let mut edges: HashMap<(Key, Key), usize> = HashMap::new();
    for i in (0..m.triangles.len()).filter(|i| down[*i]) {
        let v = m.tri(i).map(key);
        for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            match edges.get(&e) {
                Some(&j) => {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    parent[ri] = rj;
                }
                None => {
                    edges.insert(e, i);
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in (0..m.triangles.len()).filter(|i| down[*i]) {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|g| g[0]);
    out
}

fn tri_area(m: &Mesh, i: usize) -> f32 {
    let [a, b, c] = m.tri(i);
    (b - a).cross(c - a).length() * 0.5
}

/// Direction the fins run in for an area: across its short side.
fn auto_dir(m: &Mesh, tris: &[usize]) -> Vec2 {
    let mut w = 0.0;
    let mut mean = Vec2::ZERO;
    for &i in tris {
        let [a, b, c] = m.tri(i);
        let ar = tri_area(m, i);
        mean += ((a + b + c) / 3.0).truncate() * ar;
        w += ar;
    }
    mean /= w.max(1e-12);
    // second moments of the area about its centre (exact per triangle)
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for &i in tris {
        let v = m.tri(i).map(|p| p.truncate() - mean);
        let ar = tri_area(m, i) / 12.0;
        let (sx, sy) = (v[0].x + v[1].x + v[2].x, v[0].y + v[1].y + v[2].y);
        sxx += ar * (v.iter().map(|p| p.x * p.x).sum::<f32>() + sx * sx);
        syy += ar * (v.iter().map(|p| p.y * p.y).sum::<f32>() + sy * sy);
        sxy += ar * (v.iter().map(|p| p.x * p.y).sum::<f32>() + sx * sy);
    }
    // long axis of the area; fins run across it
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let long = Vec2::from_angle(angle);
    let spread = |d: Vec2| sxx * d.x * d.x + 2.0 * sxy * d.x * d.y + syy * d.y * d.y;
    if spread(long) < spread(long.perp()) * 1.3 {
        // about as long as wide: line them up with the axes
        return if sxx >= syy { Vec2::Y } else { Vec2::X };
    }
    let d = long.perp();
    // nearly along an axis: exactly along it
    for axis in [Vec2::X, Vec2::Y] {
        if d.dot(axis).abs() > 10f32.to_radians().cos() {
            return axis;
        }
    }
    d
}

/// The surface below `p`: its height, or None when there's nothing (the plate) or the
/// hit faces down (inside the part).
fn floor_below(m: &Mesh, bvh: &Bvh, p: Vec3) -> Option<Option<f32>> {
    match bvh.ray_cast_where(m, p, Vec3::NEG_Z, |_| true) {
        None => Some(None),
        Some((t, tri)) => {
            let [a, b, c] = m.tri(tri);
            let n = (b - a).cross(c - a);
            (n.z > 0.0).then_some(Some(p.z - t))
        }
    }
}

/// Whether the space at `p` reaches the outside sideways.
fn escapes(m: &Mesh, bvh: &Bvh, p: Vec3) -> bool {
    (0..64).any(|k| {
        let d = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 64.0).extend(0.0);
        bvh.ray_cast_where(m, p, d, |_| true).is_none()
    })
}

/// Fins for every overhang of `m` (world space), as one mesh of separate thin walls.
pub fn support_fins(m: &Mesh, o: &FinOptions, progress: &Progress) -> Result<(Mesh, FinReport)> {
    let areas = overhang_areas(m, o);
    let bvh = Bvh::build(m);
    let mut out = Mesh::default();
    let mut report = FinReport::default();
    let t = o.thickness.max(0.2);
    let spacing = o.spacing.max(t + 0.2);
    let gap = o.gap.max(0.0);
    let n_areas = areas.len().max(1) as f32;
    for (ai, tris) in areas.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(Error::Cancelled);
        }
        progress.set(ai as f32 / n_areas);
        let area: f32 = tris.iter().map(|&i| tri_area(m, i)).sum();
        if area < MIN_AREA {
            continue;
        }
        // a spot with room under it (big areas can start right on the plate, like the
        // first turn of a spring): the biggest of a spread of triangles that has room
        let mut cands: Vec<usize> = tris.clone();
        cands.sort_by(|a, b| tri_area(m, *b).total_cmp(&tri_area(m, *a)));
        let stride = (cands.len() / 32).max(1);
        let spot = cands.iter().step_by(stride).take(32).find_map(|&i| {
            let [a, b, c] = m.tri(i);
            let centre = (a + b + c) / 3.0;
            let floor = floor_below(m, &bvh, centre - Vec3::Z * 0.01)?;
            if floor.is_none() && o.openings_only {
                return None;
            }
            let floor_z = floor.unwrap_or(0.0);
            (centre.z - floor_z >= MIN_HEIGHT).then_some((centre, floor_z, floor.is_none()))
        });
        let Some((centre, floor_z, on_plate)) = spot else { continue };
        // open sideways, or down onto the plate (like the inside of a brick): fins there
        // can be taken out
        if !on_plate && !escapes(m, &bvh, Vec3::new(centre.x, centre.y, (centre.z + floor_z) * 0.5)) {
            report.enclosed += 1;
            continue;
        }

        // the ceiling on its own, to find its height anywhere
        let mut ceiling = Mesh::default();
        for &i in tris {
            let base = ceiling.positions.len() as u32;
            ceiling.positions.extend(m.tri(i));
            ceiling.triangles.push([base, base + 1, base + 2]);
        }
        let cbvh = Bvh::build(&ceiling);
        let low = ceiling.bounds().min.z - 1.0;

        let d = match o.dir {
            FinDir::Auto => auto_dir(m, tris),
            FinDir::X => Vec2::X,
            FinDir::Y => Vec2::Y,
        }
        .normalize();
        let d3 = d.extend(0.0);
        let p3 = d3.cross(Vec3::Z); // across the fins
        let p = p3.truncate();
        let (mut umin, mut umax, mut wmin, mut wmax) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for q in &ceiling.positions {
            let (u, w) = (q.truncate().dot(d), q.truncate().dot(p));
            umin = umin.min(u);
            umax = umax.max(u);
            wmin = wmin.min(w);
            wmax = wmax.max(w);
        }
        // fins evenly across, clear of the sides
        let margin = t * 0.5 + gap;
        let avail = wmax - wmin - 2.0 * margin;
        let rows: Vec<f32> = if avail <= 0.0 {
            if wmax - wmin < t {
                continue;
            }
            vec![(wmin + wmax) * 0.5]
        } else {
            let n = (avail / spacing).round() as usize + 1;
            if n == 1 {
                vec![(wmin + wmax) * 0.5]
            } else {
                (0..n).map(|k| wmin + margin + avail * k as f32 / (n - 1) as f32).collect()
            }
        };
        let step = ((umax - umin) / 400.0).clamp(0.1, 0.5);
        let samples = ((umax - umin) / step).ceil() as usize + 1;
        let mut made = 0;
        for w in rows {
            // (u, top, bottom) along the fin; None where there's nothing to support
            let col: Vec<Option<(f32, f32, f32)>> = (0..samples)
                .map(|k| {
                    let u = (umin + k as f32 * step).min(umax);
                    let xy = d * u + p * w;
                    let (tt, _) = cbvh.ray_cast_where(&ceiling, Vec3::new(xy.x, xy.y, low), Vec3::Z, |_| true)?;
                    let top = low + tt;
                    let bottom = match floor_below(m, &bvh, Vec3::new(xy.x, xy.y, top - 0.01))? {
                        Some(z) => z - SINK,
                        None if o.openings_only => return None,
                        None => 0.0,
                    };
                    (top - gap - bottom > MIN_HEIGHT * 0.5).then_some((u, top - gap, bottom))
                })
                .collect();
            // runs of neighbouring samples without big jumps
            let mut runs: Vec<Vec<(f32, f32, f32)>> = vec![];
            let mut cur: Vec<(f32, f32, f32)> = vec![];
            for s in col {
                match s {
                    Some(s) => {
                        if let Some(l) = cur.last() {
                            if (s.1 - l.1).abs() > 1.0 || (s.2 - l.2).abs() > 1.0 {
                                runs.push(std::mem::take(&mut cur));
                            }
                        }
                        cur.push(s);
                    }
                    None => runs.push(std::mem::take(&mut cur)),
                }
            }
            runs.push(cur);
            for run in runs {
                // clear of the ends
                let run: Vec<(f32, f32, f32)> = {
                    let (a, b) = match (run.first(), run.last()) {
                        (Some(a), Some(b)) => (a.0 + gap, b.0 - gap),
                        _ => continue,
                    };
                    if b - a < t {
                        continue;
                    }
                    run.into_iter().filter(|s| s.0 >= a - 1e-4 && s.0 <= b + 1e-4).collect()
                };
                if run.len() < 2 {
                    continue;
                }
                let mut poly: Vec<Vec2> = run.iter().map(|s| Vec2::new(s.0, s.1)).collect();
                poly.extend(run.iter().rev().map(|s| Vec2::new(s.0, s.2)));
                let fin = match crate::text::extrude_contours(vec![poly], t) {
                    Ok(f) if !f.is_empty() => f,
                    _ => continue,
                };
                // (u, z, across) → world
                let mut placed = fin;
                for v in &mut placed.positions {
                    let (u, z, k) = (v.x, v.y, v.z);
                    *v = d3 * u + Vec3::Z * z + p3 * (w + k - t * 0.5);
                }
                out.append(&placed);
                made += 1;
                report.fins += 1;
                if report.fins >= MAX_FINS {
                    break;
                }
            }
        }
        if made > 0 {
            report.areas += 1;
        }
        if report.fins >= MAX_FINS {
            break;
        }
    }
    if out.is_empty() {
        return Err(Error::Parse(if report.enclosed > 0 {
            "the only overhangs are inside closed spaces, where fins couldn't be taken out".into()
        } else if o.openings_only {
            "no holes or slots with an overhang (Down to the plate also supports overhangs over the plate)".into()
        } else {
            "no overhangs that need support".into()
        }));
    }
    Ok((out, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::csg::{boolean, BoolOp};
    use crate::primitives;

    fn block(lo: Vec3, hi: Vec3) -> Mesh {
        let mut c = primitives::cuboid(hi - lo);
        let b = c.bounds().min;
        c.translate(lo - b);
        c
    }

    /// A 40 × 10 × 30 wall with a 20 × 10 × 4 slot through it (along Y), 10 mm up.
    fn slotted_wall() -> Mesh {
        let wall = block(Vec3::ZERO, Vec3::new(40.0, 10.0, 30.0));
        let slot = block(Vec3::new(10.0, -1.0, 10.0), Vec3::new(30.0, 11.0, 14.0));
        boolean(&wall, &slot, BoolOp::Subtract).unwrap()
    }

    #[test]
    fn slot_gets_fins_along_its_depth() {
        let m = slotted_wall();
        let (fins, r) = support_fins(&m, &FinOptions::default(), &Progress::new()).unwrap();
        assert_eq!(r.areas, 1);
        // 20 mm wide slot, fins every 4 mm: about 5 of them
        assert!((4..=6).contains(&r.fins), "{r:?}");
        let b = fins.bounds();
        // standing on the slot floor (fused a little), a gap under the ceiling
        assert!((b.min.z - (10.0 - SINK)).abs() < 0.02, "{b:?}");
        assert!((b.max.z - (14.0 - 0.2)).abs() < 0.02, "{b:?}");
        // inside the slot, clear of its sides, running through it along Y
        assert!(b.min.x > 10.0 && b.max.x < 30.0, "{b:?}");
        assert!(b.size().y > 9.0 && b.min.y >= 0.0 && b.max.y <= 10.0, "{b:?}");
        // each fin is a closed solid
        for f in fins.connected_components() {
            assert!(analyze(&f).is_watertight());
            assert!(f.volume() > 0.0);
            let s = f.bounds().size();
            assert!((s.x - 0.8).abs() < 0.01, "fins are thin across X: {s:?}");
        }
    }

    #[test]
    fn directions_and_the_plate() {
        let m = slotted_wall();
        // fins across the slot instead (along X)
        let o = FinOptions { dir: FinDir::X, ..Default::default() };
        let (fins, _) = support_fins(&m, &o, &Progress::new()).unwrap();
        for f in fins.connected_components() {
            assert!((f.bounds().size().y - 0.8).abs() < 0.01);
        }
        // a T: the arm hangs over the plate — only with "down to the plate"
        let t = boolean(
            &block(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 20.0)),
            &block(Vec3::new(0.0, 0.0, 15.0), Vec3::new(30.0, 10.0, 20.0)),
            BoolOp::Union,
        )
        .unwrap();
        assert!(support_fins(&t, &FinOptions::default(), &Progress::new()).is_err());
        let o = FinOptions { openings_only: false, ..Default::default() };
        let (fins, r) = support_fins(&t, &o, &Progress::new()).unwrap();
        assert!(r.fins >= 2, "{r:?}");
        let b = fins.bounds();
        assert!(b.min.z.abs() < 1e-4 && (b.max.z - 14.8).abs() < 0.02 && b.min.x > 10.0, "{b:?}");
    }

    #[test]
    fn closed_cavities_get_none() {
        // a hollow box: its inside ceiling can't have fins
        let outer = block(Vec3::ZERO, Vec3::splat(20.0));
        let inner = block(Vec3::splat(4.0), Vec3::splat(16.0));
        let mut m = outer.clone();
        let mut i = inner.clone();
        i.flip();
        m.append(&i);
        let e = support_fins(&m, &FinOptions::default(), &Progress::new()).unwrap_err();
        assert!(e.to_string().contains("closed"), "{e}");
    }

    #[test]
    fn round_hole_through_a_wall() {
        // a 12 mm round hole along Y through a 10 mm wall
        let wall = block(Vec3::ZERO, Vec3::new(30.0, 10.0, 30.0));
        let mut hole = primitives::frustum(6.0, 6.0, 14.0, 48);
        let b = hole.bounds();
        hole.translate(-b.center());
        // lie it along Y
        hole = hole.transformed(&glam::Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2));
        hole.translate(Vec3::new(15.0, 5.0, 15.0));
        let m = boolean(&wall, &hole, BoolOp::Subtract).unwrap();
        let (fins, r) = support_fins(&m, &FinOptions::default(), &Progress::new()).unwrap();
        assert_eq!(r.areas, 1, "{r:?}");
        assert!(r.fins >= 2, "{r:?}");
        let b = fins.bounds();
        // inside the hole, under its top
        assert!(b.min.z > 9.0 - 0.2 && b.max.z < 21.0 - 0.15, "{b:?}");
        assert!(b.min.x > 9.0 && b.max.x < 21.0, "{b:?}");
        for f in fins.connected_components() {
            assert!(analyze(&f).is_watertight());
        }
    }

    #[test]
    fn only_selected_faces() {
        let m = slotted_wall();
        let none = vec![false; m.triangles.len()];
        let o = FinOptions { faces: Some(none), ..Default::default() };
        assert!(support_fins(&m, &o, &Progress::new()).is_err());
    }
}
