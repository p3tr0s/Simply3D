//! Signed-distance-field tools: round (fillet) edges of any closed mesh, and offset.
//!
//! Pipeline: exact narrow-band distances to the triangles → closest-point
//! propagation to the whole grid → morphological opening/closing with a ball of
//! radius r (done by re-distancing iso-surfaces) → Surface Nets → QEM simplify.

use crate::analysis::analyze;
use crate::mesh::{Aabb, Mesh};
use crate::simplify::{simplify_with, SimplifyOptions};
use crate::{Error, Progress, Result};
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
fn propagate(g: &Grid, cp: &mut [Vec3], d2: &mut [f32], prog: &Progress) -> Result<()> {
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
    let mut sweep = |offs: &[(isize, isize, isize)], forward: bool, pass: usize| -> Result<()> {
        for kk in 0..nz {
            if kk % 4 == 0 {
                prog.check()?;
                prog.set((pass * nz + kk) as f32 / (4 * nz) as f32);
            }
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
        Ok(())
    };
    for round in 0..2 {
        sweep(&offs, true, round * 2)?;
        sweep(&back, false, round * 2 + 1)?;
    }
    Ok(())
}

/// Inside/outside by ray parity along +X for every (y, z) row.
fn inside_mask(m: &Mesh, g: &Grid) -> Vec<bool> {
    parity_mask(m, g, 0)
}

/// Inside test by counting crossings along rays parallel to `axis` (exact for closed meshes).
fn parity_mask(m: &Mesh, g: &Grid, axis: usize) -> Vec<bool> {
    let (ax, bx, cx) = (axis, (axis + 1) % 3, (axis + 2) % 3);
    let (na, nb, nc) = (g.n[ax], g.n[bx], g.n[cx]);
    // tiny jitter so rays never pass exactly through vertices or along edges
    let jb = g.h * 0.000_731_4;
    let jc = g.h * 0.000_517_9;
    let mut rows: Vec<Vec<f32>> = vec![vec![]; nb * nc];
    for t in 0..m.triangles.len() {
        let [a, b, c] = m.tri(t);
        let lo = a.min(b).min(c);
        let hi = a.max(b).max(c);
        let j0 = (((lo[bx] - g.origin[bx] - jb) / g.h).ceil().max(0.0)) as usize;
        let j1 = (((hi[bx] - g.origin[bx] - jb) / g.h).floor().min(nb as f32 - 1.0)) as isize;
        let k0 = (((lo[cx] - g.origin[cx] - jc) / g.h).ceil().max(0.0)) as usize;
        let k1 = (((hi[cx] - g.origin[cx] - jc) / g.h).floor().min(nc as f32 - 1.0)) as isize;
        if j1 < j0 as isize || k1 < k0 as isize {
            continue;
        }
        let n = (b - a).cross(c - a);
        if n[ax].abs() < 1e-20 {
            continue; // parallel to the ray
        }
        for k in k0..=k1 as usize {
            for j in j0..=j1 as usize {
                let y = g.origin[bx] + j as f32 * g.h + jb;
                let z = g.origin[cx] + k as f32 * g.h + jc;
                let e = |p: Vec3, q: Vec3| (q[bx] - p[bx]) * (z - p[cx]) - (q[cx] - p[cx]) * (y - p[bx]);
                let (w0, w1, w2) = (e(a, b), e(b, c), e(c, a));
                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    let x = a[ax] - (n[bx] * (y - a[bx]) + n[cx] * (z - a[cx])) / n[ax];
                    rows[j + nb * k].push(x);
                }
            }
        }
    }
    let mut inside = vec![false; g.len()];
    for k in 0..nc {
        for j in 0..nb {
            let r = &mut rows[j + nb * k];
            if r.is_empty() {
                continue;
            }
            r.sort_by(|a, b| a.total_cmp(b));
            let mut c = 0;
            for i in 0..na {
                let x = g.origin[ax] + i as f32 * g.h;
                while c < r.len() && r[c] < x {
                    c += 1;
                }
                let mut ijk = [0usize; 3];
                ijk[ax] = i;
                ijk[bx] = j;
                ijk[cx] = k;
                inside[g.idx(ijk[0], ijk[1], ijk[2])] = c % 2 == 1;
            }
        }
    }
    inside
}

/// Inside test for broken meshes (holes, overlaps): majority vote of the X, Y and Z rays.
fn vote_mask(m: &Mesh, g: &Grid) -> Vec<bool> {
    let (x, y, z) = (parity_mask(m, g, 0), parity_mask(m, g, 1), parity_mask(m, g, 2));
    (0..g.len()).map(|i| (x[i] as u8 + y[i] as u8 + z[i] as u8) >= 2).collect()
}

/// Unsigned distances (and closest points) to the triangles, everywhere in the grid.
fn distances(m: &Mesh, g: &Grid, prog: &Progress) -> Result<(Vec<Vec3>, Vec<f32>)> {
    let n = g.len();
    let mut cp = vec![Vec3::ZERO; n];
    let mut d2 = vec![f32::INFINITY; n];
    let band = g.h * 1.5;
    let nt = m.triangles.len().max(1);
    for t in 0..m.triangles.len() {
        if t % 512 == 0 {
            prog.check()?;
            prog.set(0.3 * t as f32 / nt as f32);
        }
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
    propagate(g, &mut cp, &mut d2, &prog.range(0.3, 0.9))?;
    Ok((cp, d2))
}

/// Exact signed distance to the mesh (negative inside).
fn mesh_sdf(m: &Mesh, g: &Grid, prog: &Progress) -> Result<Field> {
    let (cp, d2) = distances(m, g, prog)?;
    let inside = inside_mask(m, g);
    prog.set(1.0);
    let d = d2.iter().zip(inside).map(|(d, ins)| if ins { -d.sqrt() } else { d.sqrt() }).collect();
    Ok(Field { d, cp, shift: 0.0 })
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
fn redistance(g: &Grid, f: &Field, level: f32, prog: &Progress) -> Result<Field> {
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
        prog.check()?;
        prog.set(0.25 * k as f32 / nz as f32);
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
    propagate(g, &mut cp, &mut d2, &prog.range(0.25, 1.0))?;
    let d = (0..n).map(|id| if f.val(id) < level { -d2[id].sqrt() } else { d2[id].sqrt() }).collect();
    Ok(Field { d, cp, shift: 0.0 })
}

/// Extract the zero iso-surface with marching tetrahedra (Kuhn 6-tet split of each
/// cell). Unlike Surface Nets / classic Marching Cubes this has no ambiguous cases,
/// so the output is always a closed 2-manifold. Outward-oriented.
fn extract_surface(g: &Grid, fld: &Field, prog: &Progress) -> Result<Mesh> {
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
        prog.check()?;
        prog.set(k as f32 / nz as f32);
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
    Ok(m)
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

fn finish(mut out: Mesh, h: f32, simplify_after: bool, prog: &Progress) -> Result<Mesh> {
    if out.is_empty() {
        return Err(Error::Parse("result is empty — the radius is too large for this part".into()));
    }
    if simplify_after {
        prog.stage("Reducing triangles");
        simplify_with(&mut out, SimplifyOptions { target_triangles: 0, max_error: (h * 0.05) as f64 }, prog)?;
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
    round_edges_with(m, opts, &Progress::new())
}

/// [`round_edges`] with progress reporting and cancellation.
pub fn round_edges_with(m: &Mesh, opts: RoundOptions, prog: &Progress) -> Result<(Mesh, f32)> {
    check_closed(m)?;
    let r = opts.radius.max(0.0);
    let b = m.bounds();
    let wanted = if opts.voxel > 0.0 { opts.voxel } else { auto_voxel(r, &b) };
    let pad = r + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, opts.max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    let both = opts.mode == RoundMode::Both;
    // progress budget: sdf 0-.3, morphology .3-.75, meshing .75-.85, simplify .85-1
    prog.stage("Distance field");
    let mut f = mesh_sdf(m, &g, &prog.range(0.0, 0.3))?;
    let (mid, end) = (if both { 0.525 } else { 0.75 }, 0.75);
    if matches!(opts.mode, RoundMode::Convex | RoundMode::Both) {
        // opening: erode by r, dilate by r
        prog.stage("Rounding outside edges");
        f = redistance(&g, &f, -r, &prog.range(0.3, mid))?;
        f.shift = -r;
    }
    if matches!(opts.mode, RoundMode::Concave | RoundMode::Both) {
        // closing: dilate by r, erode by r
        prog.stage("Filling inside edges");
        f = redistance(&g, &f, r, &prog.range(if both { mid } else { 0.3 }, end))?;
        f.shift = r;
    }
    prog.stage("Building surface");
    let out = extract_surface(&g, &f, &prog.range(0.75, 0.85))?;
    drop(f);
    Ok((finish(out, h, opts.simplify, &prog.range(0.85, 1.0))?, h))
}

/// Grow (positive) or shrink (negative) a closed mesh by `distance` mm.
pub fn offset(m: &Mesh, distance: f32, voxel: f32, max_voxels: usize) -> Result<(Mesh, f32)> {
    offset_with(m, distance, voxel, max_voxels, &Progress::new())
}

pub fn offset_with(m: &Mesh, distance: f32, voxel: f32, max_voxels: usize, prog: &Progress) -> Result<(Mesh, f32)> {
    check_closed(m)?;
    let b = m.bounds();
    let wanted = if voxel > 0.0 { voxel } else { auto_voxel(distance.abs(), &b) };
    let pad = distance.max(0.0) + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    prog.stage("Distance field");
    let mut f = mesh_sdf(m, &g, &prog.range(0.0, 0.6))?;
    f.shift = -distance;
    prog.stage("Building surface");
    let out = extract_surface(&g, &f, &prog.range(0.6, 0.75))?;
    Ok((finish(out, h, true, &prog.range(0.75, 1.0))?, h))
}

// ---------- make solid / remesh ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SolidMode {
    /// Fill the inside (closes holes, removes overlaps and inner walls).
    Solid,
    /// Thicken the surface itself into a shell this thick (for open sheets, scans).
    Shell(f32),
}

/// Rebuild any mesh — open, self-intersecting, many overlapping parts — as one clean,
/// closed solid (Meshmixer's *Make Solid*). With `adaptive`, flat areas get fewer
/// triangles; otherwise triangles stay even in size (a remesh).
pub fn make_solid(m: &Mesh, voxel: f32, mode: SolidMode, adaptive: bool, max_voxels: usize, prog: &Progress) -> Result<(Mesh, f32)> {
    if m.triangles.is_empty() {
        return Err(Error::Parse("nothing to rebuild".into()));
    }
    let b = m.bounds();
    let wanted = if voxel > 0.0 { voxel } else { (b.size().max_element() / 200.0).max(0.05) };
    let extra = match mode {
        SolidMode::Shell(t) => t.max(0.0),
        SolidMode::Solid => 0.0,
    };
    let pad = extra + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    prog.stage("Distance field");
    let (cp, d2) = distances(m, &g, &prog.range(0.0, 0.55))?;
    prog.stage("Inside / outside");
    let d: Vec<f32> = match mode {
        SolidMode::Solid => {
            let inside = vote_mask(m, &g);
            d2.iter().zip(inside).map(|(d, ins)| if ins { -d.sqrt() } else { d.sqrt() }).collect()
        }
        SolidMode::Shell(t) => d2.iter().map(|d| d.sqrt() - (t * 0.5).max(h * 0.6)).collect(),
    };
    prog.check()?;
    let f = Field { d, cp, shift: 0.0 };
    prog.stage("Building surface");
    let out = extract_surface(&g, &f, &prog.range(0.6, 0.75))?;
    drop(f);
    Ok((finish(out, h, adaptive, &prog.range(0.75, 1.0))?, h))
}

// ---------- patterns ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pattern {
    /// Struts along the edges of the (simplified) surface mesh, balls at the corners.
    Wireframe,
    /// The outer skin cut into Voronoi cells: only the cell borders remain.
    Voronoi,
    /// Filled with a gyroid lattice (optionally inside a closed skin).
    Gyroid,
}

#[derive(Debug, Clone, Copy)]
pub struct PatternOptions {
    pub pattern: Pattern,
    /// Cell size / strut spacing, mm.
    pub cell: f32,
    /// Strut / wall thickness, mm.
    pub thickness: f32,
    /// Voronoi: depth of the shell. Gyroid: outer skin (0 = none).
    pub skin: f32,
    pub voxel: f32,
    pub max_voxels: usize,
    pub seed: u64,
}

impl Default for PatternOptions {
    fn default() -> Self {
        Self { pattern: Pattern::Voronoi, cell: 8.0, thickness: 1.6, skin: 1.6, voxel: 0.0, max_voxels: 12_000_000, seed: 1 }
    }
}

fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Small deterministic random generator (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Points spread evenly over the surface, about `spacing` apart.
fn surface_samples(m: &Mesh, spacing: f32, seed: u64) -> Vec<Vec3> {
    let area = m.surface_area() as f32;
    let want = ((area / (spacing * spacing * 0.8)) as usize).clamp(4, 20_000);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    // area-weighted candidates, then keep those far enough from the ones already kept
    let mut cum = Vec::with_capacity(m.triangles.len());
    let mut acc = 0.0f32;
    for i in 0..m.triangles.len() {
        let [a, b, c] = m.tri(i);
        acc += (b - a).cross(c - a).length() * 0.5;
        cum.push(acc);
    }
    let cell = spacing * 0.75;
    let key = |p: Vec3| ((p / cell).floor().as_ivec3()).to_array();
    let mut grid: std::collections::HashMap<[i32; 3], Vec<Vec3>> = Default::default();
    let mut out = vec![];
    for _ in 0..want * 30 {
        if out.len() >= want {
            break;
        }
        let r = rng.next() * acc;
        let i = cum.partition_point(|&c| c < r).min(m.triangles.len() - 1);
        let [a, b, c] = m.tri(i);
        let (mut u, mut v) = (rng.next(), rng.next());
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let p = a + (b - a) * u + (c - a) * v;
        let k = key(p);
        let mut ok = true;
        'n: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(v) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        if v.iter().any(|q| q.distance(p) < spacing * 0.75) {
                            ok = false;
                            break 'n;
                        }
                    }
                }
            }
        }
        if ok {
            grid.entry(k).or_default().push(p);
            out.push(p);
        }
    }
    out
}

/// Turn a solid into a decorative / lightweight pattern: wireframe struts, a Voronoi
/// shell, or a gyroid lattice.
pub fn pattern(m: &Mesh, o: PatternOptions, prog: &Progress) -> Result<(Mesh, f32)> {
    check_closed(m)?;
    let b = m.bounds();
    let t = o.thickness.max(0.2);
    let wanted = if o.voxel > 0.0 { o.voxel } else { (t / 3.0).max(b.size().max_element() / 300.0).max(0.05) };
    let pad = t + wanted * 4.0;
    let h = pick_voxel(&b, pad, wanted, o.max_voxels);
    let g = Grid::new(&b, pad + 2.0 * h, h);
    let n = g.len();
    let big = 1e6f32;
    let d: Vec<f32> = match o.pattern {
        Pattern::Wireframe => {
            // edges of a simplified copy, about `cell` long
            prog.stage("Simplifying");
            let mut s = m.clone();
            let target = ((m.surface_area() as f32 / (o.cell * o.cell * 0.43)) as usize).max(8);
            simplify_with(&mut s, SimplifyOptions { target_triangles: target, max_error: 0.0 }, &prog.range(0.0, 0.2))?;
            let mut edges: Vec<(u32, u32)> =
                s.triangles.iter().flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]).map(|(a, b)| (a.min(b), a.max(b))).collect();
            edges.sort_unstable();
            edges.dedup();
            prog.stage("Struts");
            let r = t * 0.5;
            let node_r = r * 1.4;
            let mut d = vec![big; n];
            let band = node_r + 2.0 * h;
            let stamp = |a: Vec3, bb: Vec3, rad: f32, d: &mut Vec<f32>| {
                let lo = ((a.min(bb) - Vec3::splat(band) - g.origin) / g.h).floor().max(Vec3::ZERO);
                let hi = ((a.max(bb) + Vec3::splat(band) - g.origin) / g.h).ceil().min(Vec3::new(
                    g.n[0] as f32 - 1.0,
                    g.n[1] as f32 - 1.0,
                    g.n[2] as f32 - 1.0,
                ));
                for k in lo.z as usize..=hi.z as usize {
                    for j in lo.y as usize..=hi.y as usize {
                        for i in lo.x as usize..=hi.x as usize {
                            let p = g.pos(i, j, k);
                            let v = seg_dist(p, a, bb) - rad;
                            let id = g.idx(i, j, k);
                            if v < d[id] {
                                d[id] = v;
                            }
                        }
                    }
                }
            };
            let ne = edges.len().max(1);
            for (k, (a, bb)) in edges.iter().enumerate() {
                if k % 256 == 0 {
                    prog.check()?;
                    prog.set(0.2 + 0.5 * k as f32 / ne as f32);
                }
                stamp(s.positions[*a as usize], s.positions[*bb as usize], r, &mut d);
            }
            for p in &s.positions {
                stamp(*p, *p, node_r, &mut d);
            }
            d
        }
        Pattern::Voronoi | Pattern::Gyroid => {
            prog.stage("Distance field");
            let f = mesh_sdf(m, &g, &prog.range(0.0, 0.5))?;
            let sd = f.d;
            if o.pattern == Pattern::Voronoi {
                prog.stage("Voronoi cells");
                let seeds = surface_samples(m, o.cell, o.seed);
                let cs = o.cell;
                let key = |p: Vec3| ((p / cs).floor().as_ivec3()).to_array();
                let mut sg: std::collections::HashMap<[i32; 3], Vec<Vec3>> = Default::default();
                for s in &seeds {
                    sg.entry(key(*s)).or_default().push(*s);
                }
                let depth = o.skin.max(t);
                let mut d = vec![big; n];
                for kk in 0..g.n[2] {
                    prog.check()?;
                    prog.set(0.5 + 0.4 * kk as f32 / g.n[2] as f32);
                    for j in 0..g.n[1] {
                        for i in 0..g.n[0] {
                            let id = g.idx(i, j, kk);
                            let s = sd[id];
                            // the shell: −depth < s < 0
                            let shell = (s + depth * 0.5).abs() - depth * 0.5;
                            if shell > 2.0 * h {
                                d[id] = shell;
                                continue;
                            }
                            let p = g.pos(i, j, kk);
                            let k0 = key(p);
                            let (mut d1, mut d2) = (f32::MAX, f32::MAX);
                            for dz in -1..=1 {
                                for dy in -1..=1 {
                                    for dx in -1..=1 {
                                        if let Some(v) = sg.get(&[k0[0] + dx, k0[1] + dy, k0[2] + dz]) {
                                            for q in v {
                                                let dd = q.distance(p);
                                                if dd < d1 {
                                                    d2 = d1;
                                                    d1 = dd;
                                                } else if dd < d2 {
                                                    d2 = dd;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            let border = if d2 == f32::MAX { big } else { (d2 - d1) * 0.5 };
                            d[id] = shell.max(border - t * 0.5);
                        }
                    }
                }
                d
            } else {
                prog.stage("Gyroid");
                let w = std::f32::consts::TAU / o.cell.max(0.5);
                // gyroid value / gradient ≈ distance; |∇| ≈ w·1.2 on average
                let scale = 1.0 / (w * 1.2);
                let mut d = vec![big; n];
                for kk in 0..g.n[2] {
                    prog.check()?;
                    prog.set(0.5 + 0.4 * kk as f32 / g.n[2] as f32);
                    for j in 0..g.n[1] {
                        for i in 0..g.n[0] {
                            let id = g.idx(i, j, kk);
                            let p = g.pos(i, j, kk) * w;
                            let gy = (p.x.sin() * p.y.cos() + p.y.sin() * p.z.cos() + p.z.sin() * p.x.cos()).abs() * scale - t * 0.5;
                            let s = sd[id];
                            let mut v = s.max(gy);
                            if o.skin > 0.0 {
                                v = v.min(s.max(-s - o.skin));
                            }
                            d[id] = v;
                        }
                    }
                }
                d
            }
        }
    };
    let f = Field { d, cp: vec![], shift: 0.0 };
    prog.stage("Building surface");
    let out = extract_surface(&g, &f, &prog.range(0.9, 0.95))?;
    drop(f);
    let mut out = finish(out, h, true, &prog.range(0.95, 1.0))?;
    // drop crumbs: pieces much smaller than a strut
    let parts = out.connected_components();
    if parts.len() > 1 {
        let min_v = (t * t * t) as f64;
        let mut keep = Mesh::default();
        for p in parts.into_iter().filter(|p| p.volume() > min_v) {
            keep.append(&p);
        }
        out = keep;
    }
    Ok((out, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{cuboid, sphere};

    #[test]
    fn sdf_of_cube_is_exact() {
        let m = cuboid(Vec3::splat(10.0));
        let g = Grid::new(&m.bounds(), 3.0, 0.5);
        let f = mesh_sdf(&m, &g, &Progress::new()).unwrap();
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
    fn cancel_stops_early() {
        let p = Progress::new();
        p.cancel();
        let r = round_edges_with(&cuboid(Vec3::splat(20.0)), RoundOptions::default(), &p);
        assert!(matches!(r, Err(Error::Cancelled)));
    }

    #[test]
    fn make_solid_closes_a_broken_box() {
        let mut m = cuboid(Vec3::splat(20.0));
        m.triangles.pop(); // a hole
        let (o, _) = make_solid(&m, 0.5, SolidMode::Solid, true, 10_000_000, &Progress::new()).unwrap();
        assert!(analyze(&o).is_watertight());
        assert!((o.volume() - 8000.0).abs() / 8000.0 < 0.03, "{}", o.volume());
        // an open sheet thickened into a shell
        let mut sheet = cuboid(Vec3::new(20.0, 20.0, 0.0001));
        sheet.triangles.truncate(2);
        let (s, _) = make_solid(&sheet, 0.25, SolidMode::Shell(2.0), true, 10_000_000, &Progress::new()).unwrap();
        assert!(analyze(&s).is_watertight());
        assert!(s.volume() > 600.0, "{}", s.volume());
    }

    #[test]
    fn patterns_are_closed_and_lighter() {
        let m = sphere(15.0, 48, 24);
        let full = m.volume();
        for pat in [Pattern::Wireframe, Pattern::Voronoi, Pattern::Gyroid] {
            let o = PatternOptions { pattern: pat, cell: 8.0, thickness: 1.6, skin: 0.0, voxel: 0.4, ..Default::default() };
            let (p, _) = pattern(&m, o, &Progress::new()).unwrap();
            assert!(analyze(&p).is_watertight(), "{pat:?}");
            assert!(p.volume() > 0.0 && p.volume() < full * 0.7, "{pat:?}: {} of {full}", p.volume());
        }
    }

    #[test]
    fn rejects_open_mesh() {
        let mut m = cuboid(Vec3::ONE);
        m.triangles.pop();
        assert!(round_edges(&m, RoundOptions::default()).is_err());
    }
}
