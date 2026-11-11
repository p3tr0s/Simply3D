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
    /// Stand the fins on small pegs instead of their whole bottom edge, so they come
    /// away with a snip or a file.
    pub pegs: Option<Pegs>,
    /// Width of the fin's top, mm: the fin flares out from its thickness to this just
    /// under the ceiling (at 45°, so it prints), holding the ceiling up across a wide
    /// strip instead of a thin edge. Its thickness or less: straight fins.
    pub top: f32,
    /// Openings whose ceiling spans no more than this between the walls holding it get
    /// no fins: the printer bridges them. 0 = support every opening.
    pub max_bridge: f32,
}

/// Fins held up on pegs: a gap under each fin, bridged by tapered pegs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pegs {
    /// Gap between the fin and the surface below, mm.
    pub lift: f32,
    /// Distance between pegs along a fin, mm.
    pub spacing: f32,
}

impl Default for Pegs {
    fn default() -> Self {
        Self { lift: 1.0, spacing: 5.0 }
    }
}

impl Default for FinOptions {
    fn default() -> Self {
        Self { angle: 45.0, thickness: 0.8, spacing: 4.0, gap: 0.2, dir: FinDir::Auto, openings_only: true, faces: None, pegs: Some(Pegs::default()), top: 3.0, max_bridge: 0.0 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FinReport {
    pub fins: usize,
    /// Overhang areas that got fins.
    pub areas: usize,
    /// Overhang areas skipped because they are inside a closed cavity.
    pub enclosed: usize,
    /// Overhang areas skipped because they are short enough to bridge.
    pub bridged: usize,
}

/// Openings lower than this need no fins, mm.
const MIN_HEIGHT: f32 = 1.0;
/// Overhang areas smaller than this are ignored, mm².
const MIN_AREA: f32 = 0.5;
/// How far fins reach into the surface they stand on, so they fuse with it, mm.
const SINK: f32 = 0.1;
const MAX_FINS: usize = 5000;
/// Peg width where it meets the fin, and where it touches the surface below, mm.
const PEG_TOP: f32 = 1.6;
const PEG_TIP: f32 = 0.6;

/// Height of the flat band at the top of a flared fin, mm.
const BAND: f32 = 0.4;

/// The flared top of a fin along a run: from the fin's thickness `t` out to `2 × half`
/// just under the ceiling, in fin space (x along, y up, z across from 0 to `t`).
fn flare(run: &[(f32, f32, f32)], t: f32, half: f32) -> Mesh {
    let mut m = Mesh::default();
    let rise = half - t * 0.5;
    for s in run {
        let (top, z1) = (s.1, s.1 - BAND);
        let z0 = z1 - rise;
        // the cross-section, counter-clockwise across (z) and up (y)
        for (w, y) in [(-t * 0.5, z0), (t * 0.5, z0), (half, z1), (half, top), (-half, top), (-half, z1)] {
            m.positions.push(Vec3::new(s.0, y, t * 0.5 + w));
        }
    }
    let n = run.len() as u32;
    let v = |i: u32, j: u32| i * 6 + (j % 6);
    for i in 0..n - 1 {
        for j in 0..6 {
            m.triangles.push([v(i, j), v(i + 1, j + 1), v(i, j + 1)]);
            m.triangles.push([v(i, j), v(i + 1, j), v(i + 1, j + 1)]);
        }
    }
    for j in 1..5 {
        m.triangles.push([v(0, 0), v(0, j), v(0, j + 1)]);
        m.triangles.push([v(n - 1, 0), v(n - 1, j + 1), v(n - 1, j)]);
    }
    if m.volume() < 0.0 {
        m.flip();
    }
    m
}

/// Value at `u` along a run of (u, top, bottom) samples (`f` picks top or bottom).
fn along(run: &[(f32, f32, f32)], u: f32, f: impl Fn(&(f32, f32, f32)) -> f32) -> f32 {
    let i = run.partition_point(|s| s.0 < u).clamp(1, run.len() - 1);
    let (a, b) = (&run[i - 1], &run[i]);
    let t = if b.0 > a.0 { ((u - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 0.0 };
    f(a) + (f(b) - f(a)) * t
}

/// The side outline of one fin in (u, z): its top under the ceiling and its bottom on
/// the surface — whole, or lifted and resting on tapered pegs.
fn fin_outline(run: &[(f32, f32, f32)], pegs: Option<Pegs>) -> Vec<Vec2> {
    let mut poly: Vec<Vec2> = run.iter().map(|s| Vec2::new(s.0, s.1)).collect();
    let solid = |mut poly: Vec<Vec2>| {
        poly.extend(run.iter().rev().map(|s| Vec2::new(s.0, s.2)));
        poly
    };
    let Some(pg) = pegs else { return solid(poly) };
    let lift = pg.lift.max(0.1);
    let (ua, ub) = (run[0].0, run[run.len() - 1].0);
    let len = ub - ua;
    // too low, or too short, for pegs: stand on the whole edge
    let room = run.iter().map(|s| s.1 - (s.2 + SINK + lift)).fold(f32::INFINITY, f32::min);
    if room < 0.8 || len < PEG_TOP {
        return solid(poly);
    }
    let base = |u: f32| along(run, u, |s| s.2) + SINK + lift;
    let n = if len < 2.0 * PEG_TOP { 1 } else { ((len - PEG_TOP) / pg.spacing.max(PEG_TOP)).ceil() as usize + 1 };
    let pegs: Vec<f32> = if n == 1 {
        vec![(ua + ub) * 0.5]
    } else {
        (0..n).map(|k| ua + PEG_TOP * 0.5 + (len - PEG_TOP) * k as f32 / (n - 1) as f32).collect()
    };
    let inside = |u: f32| pegs.iter().any(|c| (u - c).abs() < PEG_TOP * 0.5 + 1e-4);
    // the underside, right to left: the lifted edge, dipping down at each peg
    let mut bottom: Vec<Vec2> = run.iter().filter(|s| !inside(s.0)).map(|s| Vec2::new(s.0, base(s.0))).collect();
    for &c in &pegs {
        let foot = along(run, c, |s| s.2);
        bottom.push(Vec2::new(c + PEG_TOP * 0.5, base(c + PEG_TOP * 0.5)));
        bottom.push(Vec2::new(c + PEG_TIP * 0.5, foot));
        bottom.push(Vec2::new(c - PEG_TIP * 0.5, foot));
        bottom.push(Vec2::new(c - PEG_TOP * 0.5, base(c - PEG_TOP * 0.5)));
    }
    bottom.sort_by(|a, b| b.x.total_cmp(&a.x));
    // the ends are always at a peg's outer edge or a sample; keep the outline closed there
    poly.extend(bottom);
    poly
}

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
        // the ceiling bridges across the fins' rows (between the walls at either side):
        // short enough, and the printer manages without them
        if o.max_bridge > 0.0 && wmax - wmin <= o.max_bridge {
            report.bridged += 1;
            continue;
        }
        // fins evenly across, their tops clear of the sides
        let margin = o.top.max(t) * 0.5 + gap;
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
        // how wide the tops can flare: not into the next fin, nor the sides
        let row_step = if rows.len() > 1 { rows[1] - rows[0] } else { f32::INFINITY };
        let step = ((umax - umin) / 400.0).clamp(0.1, 0.5);
        let samples = ((umax - umin) / step).ceil() as usize + 1;
        let mut made = 0;
        for w in rows {
            let half = (o.top * 0.5).min((row_step - gap.max(0.3)) * 0.5).min((w - wmin).min(wmax - w) - gap);
            let half = if half >= t * 0.5 + 0.2 { half } else { t * 0.5 };
            let ceil_at = |xy: Vec2| cbvh.ray_cast_where(&ceiling, Vec3::new(xy.x, xy.y, low), Vec3::Z, |_| true).map(|h| low + h.0);
            // (u, top, bottom) along the fin; None where there's nothing to support
            let col: Vec<Option<(f32, f32, f32)>> = (0..samples)
                .map(|k| {
                    let u = (umin + k as f32 * step).min(umax);
                    let xy = d * u + p * w;
                    let mut top = ceil_at(xy)?;
                    // a wide top stays clear of the ceiling across its whole width
                    if half > t * 0.5 {
                        for side in [-half, half] {
                            if let Some(z) = ceil_at(xy + p * side) {
                                top = top.min(z);
                            }
                        }
                    }
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
                // the flare, as wide as the room under the ceiling allows here
                let floor_gap = o.pegs.map_or(0.3, |pg| SINK + pg.lift.max(0.1) + 0.3);
                let room = run.iter().map(|s| s.1 - BAND - (s.2 + floor_gap)).fold(f32::INFINITY, f32::min);
                let half_run = half.min(t * 0.5 + room);
                let cap = (half_run >= t * 0.5 + 0.2).then(|| flare(&run, t, half_run));
                // under a flare the fin itself stops inside it
                let body: Vec<(f32, f32, f32)> = match cap {
                    Some(_) => run.iter().map(|s| (s.0, s.1 - BAND * 0.5, s.2)).collect(),
                    None => run.clone(),
                };
                let poly = fin_outline(&body, o.pegs);
                let fin = match crate::text::extrude_contours(vec![poly], t) {
                    Ok(f) if !f.is_empty() => f,
                    _ => continue,
                };
                let fin = match cap {
                    Some(cap) => crate::csg::boolean(&fin, &cap, crate::csg::BoolOp::Union).unwrap_or_else(|_| {
                        let mut f = fin.clone();
                        f.append(&cap);
                        f
                    }),
                    None => fin,
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
        return Err(Error::Parse(if report.bridged > 0 {
            format!("every opening spans {} mm or less, short enough to bridge", o.max_bridge)
        } else if report.enclosed > 0 {
            "the only overhangs are inside closed spaces, where fins couldn't be taken out".to_string()
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
        let o = FinOptions { top: 0.0, ..Default::default() };
        let (fins, r) = support_fins(&m, &o, &Progress::new()).unwrap();
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
    fn fins_on_pegs() {
        let m = slotted_wall();
        let solid = FinOptions { pegs: None, top: 0.0, ..Default::default() };
        let (whole, _) = support_fins(&m, &solid, &Progress::new()).unwrap();
        let o = FinOptions { top: 0.0, ..Default::default() };
        let (pegged, r) = support_fins(&m, &o, &Progress::new()).unwrap();
        let parts = pegged.connected_components();
        // still one closed piece per fin, reaching the floor only at its pegs
        assert_eq!(parts.len(), r.fins);
        for f in &parts {
            assert!(analyze(f).is_watertight());
            assert!((f.bounds().min.z - (10.0 - SINK)).abs() < 0.02);
        }
        assert!(pegged.volume() < whole.volume() * 0.9, "{} vs {}", pegged.volume(), whole.volume());
        // what touches the floor: a sliver just above it holds only the peg tips
        let touching: f32 = (0..pegged.triangles.len())
            .filter_map(|i| {
                let [a, b, c] = pegged.tri(i);
                let n = (b - a).cross(c - a);
                (n.z < 0.0 && a.z.max(b.z).max(c.z) < 10.0).then(|| n.length() * 0.5)
            })
            .sum();
        // 10 mm long fins with pegs every 5 mm: 3 tips of 0.6 × 0.8 each
        assert!(touching < r.fins as f32 * 3.0 * 0.6 * 0.8 * 1.1, "{touching}");
    }

    #[test]
    fn flared_tops() {
        let m = slotted_wall();
        let (fins, r) = support_fins(&m, &FinOptions::default(), &Progress::new()).unwrap();
        let parts = fins.connected_components();
        assert_eq!(parts.len(), r.fins);
        for f in &parts {
            assert!(analyze(f).is_watertight(), "flared fin isn't closed");
            let b = f.bounds();
            // 3 mm wide at the top, under the 14 mm ceiling, still on pegs at the floor
            assert!((b.size().x - 3.0).abs() < 0.05, "{b:?}");
            assert!((b.max.z - 13.8).abs() < 0.02 && (b.min.z - (10.0 - SINK)).abs() < 0.02, "{b:?}");
            // thin lower down: a slice through the middle is the fin's thickness
            let mid: Vec<f32> = f.positions.iter().filter(|p| (p.z - 11.5).abs() < 0.6).map(|p| p.x).collect();
            if let (Some(lo), Some(hi)) = (mid.iter().cloned().reduce(f32::min), mid.iter().cloned().reduce(f32::max)) {
                assert!(hi - lo < 0.8 + 0.01, "{}", hi - lo);
            }
        }
        // the tops of neighbouring fins don't touch
        let mut xs: Vec<(f32, f32)> = parts.iter().map(|f| (f.bounds().min.x, f.bounds().max.x)).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in xs.windows(2) {
            assert!(w[1].0 - w[0].1 >= 0.2, "{xs:?}");
        }
    }

    #[test]
    fn short_spans_bridge() {
        // the slot's ceiling spans 20 mm between its side walls
        let m = slotted_wall();
        let o = FinOptions { max_bridge: 25.0, ..Default::default() };
        let e = support_fins(&m, &o, &Progress::new()).unwrap_err();
        assert!(e.to_string().contains("bridge"), "{e}");
        let o = FinOptions { max_bridge: 15.0, ..Default::default() };
        let (_, r) = support_fins(&m, &o, &Progress::new()).unwrap();
        assert!(r.fins > 0 && r.bridged == 0, "{r:?}");
    }

    #[test]
    fn directions_and_the_plate() {
        let m = slotted_wall();
        // fins across the slot instead (along X)
        let o = FinOptions { dir: FinDir::X, top: 0.0, ..Default::default() };
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
