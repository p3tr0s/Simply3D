//! Signed-distance-field tools: round (fillet) edges of any closed mesh, and offset.
//!
//! Pipeline: exact narrow-band distances to the triangles → closest-point
//! propagation to the whole grid → morphological opening/closing with a ball of
//! radius r (done by re-distancing iso-surfaces) → Surface Nets → QEM simplify.

use crate::analysis::analyze;
use crate::mesh::{Aabb, Mesh};
use crate::simplify::{simplify, SimplifyOptions};
use crate::{Error, Result};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundMode {
    /// Round outside (convex) edges — morphological opening.
    Convex,
    /// Fill inside (concave) edges with a fillet — morphological closing.
    Concave,
    Both,
}

#[derive(Debug, Clone, Copy)]
pub struct RoundOptions {
    pub radius: f32,
    pub mode: RoundMode,
    /// Voxel size in mm; 0 = automatic.
    pub voxel: f32,
    /// Upper bound on grid size (memory ≈ 20 bytes per voxel).
    pub max_voxels: usize,
    /// Reduce the triangle count afterwards (keeps error below voxel/20).
    pub simplify: bool,
}

impl Default for RoundOptions {
    fn default() -> Self {
        Self { radius: 2.0, mode: RoundMode::Convex, voxel: 0.0, max_voxels: 8_000_000, simplify: true }
    }
}

struct Grid {
    origin: Vec3,
    h: f32,
    n: [usize; 3],
}

impl Grid {
    fn new(b: &Aabb, pad: f32, h: f32) -> Grid {
        // fractional offset so axis-aligned faces never sit exactly on grid planes
        let origin = b.min - Vec3::splat(pad) - Vec3::new(0.3713, 0.2917, 0.4371) * h;
        let size = b.size() + Vec3::splat(2.0 * pad + h);
        let n = [(size.x / h).ceil() as usize + 1, (size.y / h).ceil() as usize + 1, (size.z / h).ceil() as usize + 1];
        Grid { origin, h, n }
    }
    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }
    #[inline]
    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.n[0] * (j + self.n[1] * k)
    }
    #[inline]
    fn pos(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.h
    }
}

/// A distance field: `d` is the signed distance to some surface (negative inside),
/// `cp` the closest surface point of every voxel, `shift` an offset applied on read.
struct Field {
    d: Vec<f32>,
    cp: Vec<Vec3>,
    shift: f32,
}

impl Field {
    #[inline]
    fn val(&self, id: usize) -> f32 {
        self.d[id] + self.shift
    }
    /// Unit gradient (outward normal of the iso-surfaces) at a voxel.
    #[inline]
    fn grad(&self, id: usize, p: Vec3) -> Vec3 {
        let v = (p - self.cp[id]) * self.d[id].signum();
        v.normalize_or_zero()
    }
}

/// Closest point on triangle (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_on_tri(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Spread closest points from seeded voxels to the whole grid (two raster sweeps, twice).
fn propagate(g: &Grid, cp: &mut [Vec3], d2: &mut [f32]) {
    let [nx, ny, nz] = g.n;
    let mut offs: Vec<(isize, isize, isize)> = vec![];
    for dz in -1..=1isize {
        for dy in -1..=1isize {
            for dx in -1..=1isize {
                if dz < 0 || (dz == 0 && dy < 0) || (dz == 0 && dy == 0 && dx < 0) {
                    offs.push((dx, dy, dz));
                }
            }
        }
    }
    let back: Vec<(isize, isize, isize)> = offs.iter().map(|&(a, b, c)| (-a, -b, -c)).collect();
    let mut sweep = |offs: &[(isize, isize, isize)], forward: bool| {
        for kk in 0..nz {
            let k = if forward { kk } else { nz - 1 - kk };
            for jj in 0..ny {
                let j = if forward { jj } else { ny - 1 - jj };
                for ii in 0..nx {
                    let i = if forward { ii } else { nx - 1 - ii };
                    let id = g.idx(i, j, k);
                    let p = g.pos(i, j, k);
                    let mut best = d2[id];
                    let mut bcp = cp[id];
                    for &(dx, dy, dz) in offs {
                        let (x, y, z) = (i as isize + dx, j as isize + dy, k as isize + dz);
                        if x < 0 || y < 0 || z < 0 || x >= nx as isize || y >= ny as isize || z >= nz as isize {
                            continue;
                        }
                        let nid = g.idx(x as usize, y as usize, z as usize);
                        if d2[nid].is_finite() {
                            let c = cp[nid];
                            let dd = c.distance_squared(p);
                            if dd < best {
                                best = dd;
                                bcp = c;
                            }
                        }
                    }
                    d2[id] = best;
                    cp[id] = bcp;
                }
            }
        }
    };
    for _ in 0..2 {
        sweep(&offs, true);
        sweep(&back, false);
    }
}

/// Inside/outside by ray parity along +X for every (y, z) row.
fn inside_mask(m: &Mesh, g: &Grid) -> Vec<bool> {
    let [nx, ny, nz] = g.n;
    let jy = g.h * 0.000_731_4;
    let jz = g.h * 0.000_517_9;
    let mut rows: Vec<Vec<f32>> = vec![vec![]; ny * nz];
    for t in 0..m.triangles.len() {
        let [a, b, c] = m.tri(t);
        let lo = a.min(b).min(c);
        let hi = a.max(b).max(c);
        let j0 = (((lo.y - g.origin.y - jy) / g.h).ceil().max(0.0)) as usize;
        let j1 = (((hi.y - g.origin.y - jy) / g.h).floor().min(ny as f32 - 1.0)) as isize;
        let k0 = (((lo.z - g.origin.z - jz) / g.h).ceil().max(0.0)) as usize;
        let k1 = (((hi.z - g.origin.z - jz) / g.h).floor().min(nz as f32 - 1.0)) as isize;
        if j1 < j0 as isize || k1 < k0 as isize {
            continue;
        }
        let n = (b - a).cross(c - a);
        if n.x.abs() < 1e-20 {
            continue; // parallel to the ray
        }
        for k in k0..=k1 as usize {
            for j in j0..=j1 as usize {
                let y = g.origin.y + j as f32 * g.h + jy;
                let z = g.origin.z + k as f32 * g.h + jz;
                // 2D point-in-triangle in YZ
                let e = |p: Vec3, q: Vec3| (q.y - p.y) * (z - p.z) - (q.z - p.z) * (y - p.y);
                let (w0, w1, w2) = (e(a, b), e(b, c), e(c, a));
                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    // plane: n·(p - a) = 0  ->  x
                    let x = a.x - (n.y * (y - a.y) + n.z * (z - a.z)) / n.x;
                    rows[j + ny * k].push(x);
                }
            }
        }
    }
    let mut inside = vec![false; g.len()];
    for k in 0..nz {
        for j in 0..ny {
            let r = &mut rows[j + ny * k];
            if r.is_empty() {
                continue;
            }
            r.sort_by(|a, b| a.total_cmp(b));
            let mut c = 0;
            for i in 0..nx {
                let x = g.origin.x + i as f32 * g.h;
                while c < r.len() && r[c] < x {
                    c += 1;
                }
                inside[g.idx(i, j, k)] = c % 2 == 1;
            }
        }
    }
    inside
}

/// Exact signed distance to the mesh (negative inside).
fn mesh_sdf(m: &Mesh, g: &Grid) -> Field {
    let n = g.len();
    let mut cp = vec![Vec3::ZERO; n];
    let mut d2 = vec![f32::INFINITY; n];
    let band = g.h * 1.5;
    for t in 0..m.triangles.len() {
        let [a, b, c] = m.tri(t);
        let lo = ((a.min(b).min(c) - Vec3::splat(band) - g.origin) / g.h).floor().max(Vec3::ZERO);
        let hi = ((a.max(b).max(c) + Vec3::splat(band) - g.origin) / g.h).ceil();
        let hi = hi.min(Vec3::new(g.n[0] as f32 - 1.0, g.n[1] as f32 - 1.0, g.n[2] as f32 - 1.0));
        for k in lo.z as usize..=hi.z as usize {
            for j in lo.y as usize..=hi.y as usize {
                for i in lo.x as usize..=hi.x as usize {
                    let p = g.pos(i, j, k);
                    let q = closest_on_tri(p, a, b, c);
                    let dd = q.distance_squared(p);
                    let id = g.idx(i, j, k);
                    if dd < d2[id] {
                        d2[id] = dd;
                        cp[id] = q;
                    }
                }
            }
        }
    }
    propagate(g, &mut cp, &mut d2);
    let inside = inside_mask(m, g);
    let d = d2.iter().zip(inside).map(|(d, ins)| if ins { -d.sqrt() } else { d.sqrt() }).collect();
    Field { d, cp, shift: 0.0 }
}

const CORNERS: [(usize, usize, usize); 8] = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)];
const EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

/// Dual-contouring vertex of the iso-surface `f.val == level` inside cell (i,j,k):
/// the least-squares intersection of the tangent planes at the edge crossings
/// (recovers sharp edges and corners), falling back to the crossings' mean.
/// Returns None if the cell has no sign change.
fn cell_vertex(g: &Grid, f: &Field, level: f32, i: usize, j: usize, k: usize) -> Option<Vec3> {
    let ids: [usize; 8] = CORNERS.map(|(a, b, c)| g.idx(i + a, j + b, k + c));
    let v: [f32; 8] = ids.map(|id| f.val(id) - level);
    let neg = v.iter().filter(|x| **x < 0.0).count();
    if neg == 0 || neg == 8 {
        return None;
    }
    let base = g.pos(i, j, k);
    let corner = |c: usize| base + Vec3::new(CORNERS[c].0 as f32, CORNERS[c].1 as f32, CORNERS[c].2 as f32) * g.h;
    let mut mass = Vec3::ZERO;
    let mut cnt = 0.0;
    let mut ata = glam::Mat3::ZERO;
    let mut atb = Vec3::ZERO;
    for (a, b) in EDGES {
        if (v[a] < 0.0) == (v[b] < 0.0) {
            continue;
        }
        let t = v[a] / (v[a] - v[b]);
        let (pa, pb) = (corner(a), corner(b));
        let x = pa + (pb - pa) * t;
        mass += x;
        cnt += 1.0;
        // take the normal from whichever endpoint's closest feature is nearer to the
        // crossing: blending across a medial plane would round off sharp edges
        let n = if x.distance_squared(f.cp[ids[a]]) <= x.distance_squared(f.cp[ids[b]]) { f.grad(ids[a], pa) } else { f.grad(ids[b], pb) };
        ata += glam::Mat3::from_cols(n * n.x, n * n.y, n * n.z);
        atb += n * n.dot(x);
    }
    let mass = mass / cnt;
    // regularise toward the mass point so flat/curved cells stay well-conditioned
    let lambda = 0.05;
    let a = ata + glam::Mat3::from_diagonal(Vec3::splat(lambda));
    let b = atb + mass * lambda;
    if a.determinant().abs() > 1e-8 {
        let x = a.inverse() * b;
        let lo = base - Vec3::splat(g.h * 0.25);
        let hi = base + Vec3::splat(g.h * 1.25);
        if x.cmpge(lo).all() && x.cmple(hi).all() {
            return Some(x);
        }
    }
    Some(mass)
}

/// Distance field to the iso-surface `f.val == level` (negative where f.val < level).
/// Seeds: edge crossings plus dual-contouring vertices (exact at sharp features).
fn redistance(g: &Grid, f: &Field, level: f32) -> Field {
    let n = g.len();
    let mut cp = vec![Vec3::ZERO; n];
    let mut d2 = vec![f32::INFINITY; n];
    let [nx, ny, nz] = g.n;
    let seed = |v: usize, p: Vec3, q: Vec3, cp: &mut Vec<Vec3>, d2: &mut Vec<f32>| {
        let dd = q.distance_squared(p);
        if dd < d2[v] {
            d2[v] = dd;
            cp[v] = q;
        }
    };
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let id = g.idx(i, j, k);
                let f0 = f.val(id) - level;
                for (a, (di, dj, dk)) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)].into_iter().enumerate() {
                    let (i2, j2, k2) = (i + di, j + dj, k + dk);
                    if i2 >= nx || j2 >= ny || k2 >= nz {
                        continue;
                    }
                    let id2 = g.idx(i2, j2, k2);
                    let f1 = f.val(id2) - level;
                    if (f0 < 0.0) == (f1 < 0.0) {
                        continue;
                    }
                    let t = f0 / (f0 - f1);
                    let mut q = g.pos(i, j, k);
                    q[a] += t * g.h;
                    seed(id, g.pos(i, j, k), q, &mut cp, &mut d2);
                    seed(id2, g.pos(i2, j2, k2), q, &mut cp, &mut d2);
                }
                if i + 1 < nx && j + 1 < ny && k + 1 < nz {
                    if let Some(q) = cell_vertex(g, f, level, i, j, k) {
                        for (a, b, c) in CORNERS {
                            seed(g.idx(i + a, j + b, k + c), g.pos(i + a, j + b, k + c), q, &mut cp, &mut d2);
                        }
                    }
                }
            }
        }
    }
    propagate(g, &mut cp, &mut d2);
    let d = (0..n).map(|id| if f.val(id) < level { -d2[id].sqrt() } else { d2[id].sqrt() }).collect();
    Field { d, cp, shift: 0.0 }
}

/// Extract the zero iso-surface with marching tetrahedra (Kuhn 6-tet split of each
/// cell). Unlike Surface Nets / classic Marching Cubes this has no ambiguous cases,
/// so the output is always a closed 2-manifold. Outward-oriented.
fn extract_surface(g: &Grid, fld: &Field) -> Mesh {
    use std::collections::HashMap;
    let [nx, ny, nz] = g.n;
    // nudge exact zeros so every crossing lies strictly inside an edge
    let val = |id: usize| {
        let v = fld.val(id);
        if v.abs() < 1e-7 {
            1e-7
        } else {
            v
        }
    };
    const TETS: [[usize; 4]; 6] = [[0, 1, 3, 7], [0, 1, 5, 7], [0, 2, 3, 7], [0, 2, 6, 7], [0, 4, 5, 7], [0, 4, 6, 7]];
    let mut m = Mesh::default();
    let mut verts: HashMap<u64, u32> = HashMap::new();
    for k in 0..nz - 1 {
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let ids: [usize; 8] = CORNERS.map(|(a, b, c)| g.idx(i + a, j + b, k + c));
                let v: [f32; 8] = ids.map(val);
                let neg = v.iter().filter(|x| **x < 0.0).count();
                if neg == 0 || neg == 8 {
                    continue;
                }
                let base = g.pos(i, j, k);
                let pos = |c: usize| base + Vec3::new(CORNERS[c].0 as f32, CORNERS[c].1 as f32, CORNERS[c].2 as f32) * g.h;
                for tet in TETS {
                    let inside: Vec<usize> = tet.iter().copied().filter(|&c| v[c] < 0.0).collect();
                    let outside: Vec<usize> = tet.iter().copied().filter(|&c| v[c] >= 0.0).collect();
                    if inside.is_empty() || outside.is_empty() {
                        continue;
                    }
                    let mut edge_vert = |a: usize, b: usize, m: &mut Mesh| -> u32 {
                        let (ga, gb) = (ids[a] as u64, ids[b] as u64);
                        let key = ga.min(gb) << 32 | ga.max(gb);
                        *verts.entry(key).or_insert_with(|| {
                            let t = v[a] / (v[a] - v[b]);
                            m.positions.push(pos(a) + (pos(b) - pos(a)) * t);
                            (m.positions.len() - 1) as u32
                        })
                    };
                    // orientation from the tet corners (robust even if the emitted triangle is tiny)
                    let ci: Vec3 = inside.iter().map(|&c| pos(c)).sum::<Vec3>() / inside.len() as f32;
                    let co: Vec3 = outside.iter().map(|&c| pos(c)).sum::<Vec3>() / outside.len() as f32;
                    let out_dir = co - ci;
                    let mid = |a: usize, b: usize| (pos(a) + pos(b)) * 0.5;
                    match (inside.len(), outside.len()) {
                        (1, 3) | (3, 1) => {
                            let (lone, o) = if inside.len() == 1 { (inside[0], outside.clone()) } else { (outside[0], inside.clone()) };
                            let (p0, p1, p2) = (mid(lone, o[0]), mid(lone, o[1]), mid(lone, o[2]));
                            let flip = (p1 - p0).cross(p2 - p0).dot(out_dir) < 0.0;
                            let mut t = [edge_vert(lone, o[0], &mut m), edge_vert(lone, o[1], &mut m), edge_vert(lone, o[2], &mut m)];
                            if flip {
                                t.swap(1, 2);
                            }
                            m.triangles.push(t);
                        }
                        _ => {
                            let (a, b) = (inside[0], inside[1]);
                            let (c, d) = (outside[0], outside[1]);
                            // quad ac-ad-bd-bc is a proper cycle around the tet
                            let (q0, q1, q2) = (mid(a, c), mid(a, d), mid(b, d));
                            let flip = (q1 - q0).cross(q2 - q0).dot(out_dir) < 0.0;
                            let (ac, ad, bd, bc) =
                                (edge_vert(a, c, &mut m), edge_vert(a, d, &mut m), edge_vert(b, d, &mut m), edge_vert(b, c, &mut m));
                            let (t1, t2) = ([ac, ad, bd], [ac, bd, bc]);
                            if flip {
                                m.triangles.push([t1[0], t1[2], t1[1]]);
                                m.triangles.push([t2[0], t2[2], t2[1]]);
                            } else {
                                m.triangles.push(t1);
                                m.triangles.push(t2);
                            }
                        }
                    }
                }
            }
        }
    }
    m
}

/// Default detail: ~5 voxels across the radius, but not finer than size/350.
pub fn auto_voxel(radius: f32, b: &Aabb) -> f32 {
    (radius / 5.0).max(b.size().max_element() / 350.0).max(0.02)
}

fn pick_voxel(b: &Aabb, pad: f32, wanted: f32, max_voxels: usize) -> f32 {
    let size = b.size() + Vec3::splat(2.0 * pad);
    let mut h = wanted.max(1e-3);
    while (size.x / h + 2.0) * (size.y / h + 2.0) * (size.z / h + 2.0) > max_voxels as f32 {
        h *= 1.1;
    }
    h
}

fn finish(mut out: Mesh, h: f32, simplify_after: bool) -> Result<Mesh> {
    if out.is_empty() {
        return Err(Error::Parse("result is empty — the radius is too large for this part".into()));
    }
    if simplify_after {
        simplify(&mut out, SimplifyOptions { target_triangles: 0, max_error: (h * 0.05) as f64 });
    }
    out.compact();
    if out.volume() < 0.0 {
        out.flip();
    }
    Ok(out)
}

fn check_closed(m: &Mesh) -> Result<()> {
    let r = analyze(m);
    if r.boundary_edges > 0 {
        return Err(Error::Parse(format!("mesh has {} open edges — run Repair first", r.boundary_edges)));
    }
    Ok(())
}

/// Round the edges of a closed mesh with radius `opts.radius` (mm). Returns (mesh, voxel size used).
pub fn round_edges(m: &Mesh, opts: RoundOptions) -> Result<(Mesh, f32)> {
    check_closed(m)?;
    let r = opts.radius.max(0.0);
    let b = m.bounds();
    let wanted = if opts.voxel > 0.0 { opts.voxel } else { auto_voxel(r, &b) };
    let pad = r + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, opts.max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    let mut f = mesh_sdf(m, &g);
    if matches!(opts.mode, RoundMode::Convex | RoundMode::Both) {
        // opening: erode by r, dilate by r
        f = redistance(&g, &f, -r);
        f.shift = -r;
    }
    if matches!(opts.mode, RoundMode::Concave | RoundMode::Both) {
        // closing: dilate by r, erode by r
        f = redistance(&g, &f, r);
        f.shift = r;
    }
    Ok((finish(extract_surface(&g, &f), h, opts.simplify)?, h))
}

/// Grow (positive) or shrink (negative) a closed mesh by `distance` mm.
pub fn offset(m: &Mesh, distance: f32, voxel: f32, max_voxels: usize) -> Result<(Mesh, f32)> {
    check_closed(m)?;
    let b = m.bounds();
    let wanted = if voxel > 0.0 { voxel } else { auto_voxel(distance.abs(), &b) };
    let pad = distance.max(0.0) + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    let mut f = mesh_sdf(m, &g);
    f.shift = -distance;
    Ok((finish(extract_surface(&g, &f), h, true)?, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{cuboid, sphere};

    #[test]
    fn sdf_of_cube_is_exact() {
        let m = cuboid(Vec3::splat(10.0));
        let g = Grid::new(&m.bounds(), 3.0, 0.5);
        let f = mesh_sdf(&m, &g);
        let [nx, ny, nz] = g.n;
        for k in (0..nz).step_by(3) {
            for j in (0..ny).step_by(3) {
                for i in (0..nx).step_by(3) {
                    let p = g.pos(i, j, k);
                    let q = p.abs() - Vec3::splat(5.0);
                    let exact = q.max(Vec3::ZERO).length() + q.max_element().min(0.0);
                    assert!((f.val(g.idx(i, j, k)) - exact).abs() < 0.05, "{p}: {} vs {exact}", f.val(g.idx(i, j, k)));
                }
            }
        }
    }

    #[test]
    fn remesh_sphere_is_closed_and_accurate() {
        let m = sphere(10.0, 64, 32);
        let (o, _) = offset(&m, 0.0, 0.4, 10_000_000).unwrap();
        let r = analyze(&o);
        assert!(r.is_watertight(), "{r:?}");
        assert!(o.volume() > 0.0);
        assert!((o.volume() - m.volume()).abs() / m.volume() < 0.02, "{} vs {}", o.volume(), m.volume());
    }

    #[test]
    fn rounded_cube_volume_matches_analytic() {
        let s = 20.0f32;
        let r = 3.0f32;
        let (o, h) = round_edges(&cuboid(Vec3::splat(s)), RoundOptions { radius: r, voxel: 0.25, ..Default::default() }).unwrap();
        let (s, r) = (s as f64, r as f64);
        let pi = std::f64::consts::PI;
        let inner = s - 2.0 * r;
        let exact = inner.powi(3) + 6.0 * inner * inner * r + 3.0 * pi * r * r * inner + 4.0 / 3.0 * pi * r.powi(3);
        assert!(analyze(&o).is_watertight());
        assert!((o.volume() - exact).abs() / exact < 0.01, "{} vs {exact} (h={h})", o.volume());
        // flat faces stay flat and get decimated
        assert!(o.triangles.len() < 30_000, "{}", o.triangles.len());
        let max = o.bounds().max;
        assert!((max.x - 10.0).abs() < 0.05, "{max}");
    }

    #[test]
    fn concave_fillet_adds_material_in_corner() {
        // L-shape: the inner corner gets filled
        let mut a = cuboid(Vec3::new(20.0, 10.0, 10.0));
        let mut b = cuboid(Vec3::new(10.0, 10.0, 20.0));
        a.translate(Vec3::new(10.0, 5.0, 5.0));
        b.translate(Vec3::new(5.0, 5.0, 10.0));
        let l = crate::csg::boolean(&a, &b, crate::csg::BoolOp::Union).unwrap();
        let v0 = l.volume();
        let (o, _) = round_edges(&l, RoundOptions { radius: 3.0, mode: RoundMode::Concave, voxel: 0.25, ..Default::default() }).unwrap();
        let fillet = (9.0 - std::f64::consts::PI * 9.0 / 4.0) * 10.0; // (r² - πr²/4) × length
        assert!((o.volume() - (v0 + fillet)).abs() < 3.0, "{} vs {}", o.volume(), v0 + fillet);
    }

    #[test]
    fn rejects_open_mesh() {
        let mut m = cuboid(Vec3::ONE);
        m.triangles.pop();
        assert!(round_edges(&m, RoundOptions::default()).is_err());
    }
}
