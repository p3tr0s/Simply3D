//! Texture displacement: a tileable height map (built-in pattern or a picture) pressed
//! into or raised out of the surface as real geometry. The selected area is refined to
//! the texture's detail, then every point moves along its normal by the texture height.

use crate::image3d::Gray;
use crate::mesh::Mesh;
use crate::paint::refine_where;
use crate::{Progress, Result};
use glam::{Vec2, Vec3};
use std::collections::BinaryHeap;
use std::f32::consts::{PI, TAU};

/// Tileable height map, values 0 (low) … 1 (high).
#[derive(Debug, Clone)]
pub struct HeightMap {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl HeightMap {
    pub fn from_fn(n: usize, f: impl Fn(f32, f32) -> f32) -> HeightMap {
        let mut v = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                v.push(f(i as f32 / n as f32, j as f32 / n as f32).clamp(0.0, 1.0));
            }
        }
        HeightMap { w: n, h: n, v }
    }

    /// From a picture: brightness = height (white high).
    pub fn from_gray(g: &Gray) -> HeightMap {
        let (lo, hi) = g.v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        let span = (hi - lo).max(1e-6);
        HeightMap { w: g.w, h: g.h, v: g.v.iter().map(|x| (x - lo) / span).collect() }
    }

    /// Bilinear sample, wrapping around (u, v in tile units).
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let x = (u - u.floor()) * self.w as f32 - 0.5;
        let y = (v - v.floor()) * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |i: f32, n: usize| (i as isize).rem_euclid(n as isize) as usize;
        let at = |i: f32, j: f32| self.v[wrap(j, self.h) * self.w + wrap(i, self.w)];
        let a = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
        let b = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
        a * (1.0 - fy) + b * fy
    }

    /// Soften with `passes` box blurs (wrapping).
    pub fn blurred(&self, passes: u32) -> HeightMap {
        let mut cur = self.clone();
        for _ in 0..passes {
            let mut next = cur.v.clone();
            for j in 0..cur.h {
                for i in 0..cur.w {
                    let mut s = 0.0;
                    for dj in [-1isize, 0, 1] {
                        for di in [-1isize, 0, 1] {
                            let x = (i as isize + di).rem_euclid(cur.w as isize) as usize;
                            let y = (j as isize + dj).rem_euclid(cur.h as isize) as usize;
                            s += cur.v[y * cur.w + x];
                        }
                    }
                    next[j * cur.w + i] = s / 9.0;
                }
            }
            cur.v = next;
        }
        cur
    }
}

// ---------- built-in patterns ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Knurl,
    Ribs,
    Bumps,
    Grid,
    Bricks,
    Carbon,
    Wood,
    Leather,
    Waves,
    Sand,
}

pub const BUILTINS: [(Builtin, &str); 10] = [
    (Builtin::Knurl, "Knurl"),
    (Builtin::Ribs, "Ribs"),
    (Builtin::Bumps, "Bumps"),
    (Builtin::Grid, "Grid"),
    (Builtin::Bricks, "Bricks"),
    (Builtin::Carbon, "Carbon fibre"),
    (Builtin::Wood, "Wood grain"),
    (Builtin::Leather, "Leather"),
    (Builtin::Waves, "Waves"),
    (Builtin::Sand, "Sand"),
];

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// Value noise, periodic with `p` cells across the tile.
fn noise(u: f32, v: f32, p: i32, seed: u32) -> f32 {
    let (x, y) = (u * p as f32, v * p as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let at = |i: i32, j: i32| hash(i.rem_euclid(p), j.rem_euclid(p), seed);
    let (i, j) = (x0 as i32, y0 as i32);
    let a = at(i, j) + (at(i + 1, j) - at(i, j)) * s(fx);
    let b = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * s(fx);
    a + (b - a) * s(fy)
}

fn fbm(u: f32, v: f32, base: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut tot, mut p) = (0.0, 1.0, 0.0, base);
    for o in 0..octaves {
        sum += noise(u, v, p, seed + o) * amp;
        tot += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / tot
}

/// Distances to the nearest and second nearest of `n×n` jittered cell points (periodic).
fn worley(u: f32, v: f32, n: i32, seed: u32) -> (f32, f32) {
    let (x, y) = (u * n as f32, v * n as f32);
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let (mut d1, mut d2) = (f32::MAX, f32::MAX);
    for dj in -1..=1 {
        for di in -1..=1 {
            let (i, j) = (cx + di, cy + dj);
            let px = i as f32 + 0.15 + 0.7 * hash(i.rem_euclid(n), j.rem_euclid(n), seed);
            let py = j as f32 + 0.15 + 0.7 * hash(i.rem_euclid(n), j.rem_euclid(n), seed + 7);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < d1 {
                d2 = d1;
                d1 = d;
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn builtin(b: Builtin) -> HeightMap {
    let f = |u: f32, v: f32| -> f32 {
        let fr = |x: f32| x - x.floor();
        match b {
            // square pyramids on the diagonals, like a knurled grip
            Builtin::Knurl => 1.0 - 2.0 * (fr(u + v) - 0.5).abs().max((fr(u - v) - 0.5).abs()),
            Builtin::Ribs => 0.5 + 0.5 * (TAU * u).cos(),
            Builtin::Bumps => {
                let d = |cx: f32, cy: f32| {
                    let dx = (fr(u - cx + 0.5) - 0.5).abs();
                    let dy = (fr(v - cy + 0.5) - 0.5).abs();
                    (dx * dx + dy * dy).sqrt()
                };
                let r = d(0.25, 0.25).min(d(0.75, 0.75));
                (1.0 - (r / 0.3).powi(2)).max(0.0).sqrt()
            }
            Builtin::Grid => {
                let e = (fr(u) - 0.5).abs().max((fr(v) - 0.5).abs());
                1.0 - smoothstep(0.38, 0.46, e)
            }
            Builtin::Bricks => {
                let rows = 4.0;
                let y = v * rows;
                let row = y.floor();
                let x = u * 2.0 + if row as i32 % 2 == 1 { 0.5 } else { 0.0 };
                let ex = (fr(x) - 0.5).abs() * 2.0; // 0 centre … 1 joint
                let ey = (fr(y) - 0.5).abs() * 2.0;
                let edge = (1.0 - ex) * 0.5 / 2.0 * rows; // distance to joint in brick heights
                let edge = edge.min((1.0 - ey) * 0.5);
                smoothstep(0.03, 0.12, edge) * (0.92 + 0.08 * noise(u, v, 16, 3))
            }
            Builtin::Carbon => {
                // 2×2 twill: strands alternate direction cell by cell, offset each row
                let n = 4.0;
                let (x, y) = (u * n, v * n);
                let (i, j) = (x.floor() as i32, y.floor() as i32);
                let horizontal = ((i + j).rem_euclid(4)) < 2;
                let across = if horizontal { fr(y) } else { fr(x) };
                let along = if horizontal { fr(x) } else { fr(y) };
                let strand = (PI * across).sin();
                let dip = 0.85 + 0.15 * (PI * along).sin();
                (0.25 + 0.75 * strand.powf(0.6)) * dip
            }
            Builtin::Wood => {
                let w = fbm(u, v, 4, 4, 11);
                let rings = 0.5 + 0.5 * (TAU * (3.0 * u + 1.4 * w)).sin();
                rings.powf(1.6) * 0.85 + 0.15 * noise(u, v, 32, 5)
            }
            Builtin::Leather => {
                let (d1, d2) = worley(u, v, 9, 21);
                let cell = smoothstep(0.0, 0.18, d2 - d1);
                cell * (0.8 + 0.2 * (1.0 - d1).max(0.0)) * (0.9 + 0.1 * noise(u, v, 32, 9))
            }
            Builtin::Waves => 0.5 + 0.5 * (TAU * (u + 0.15 * (TAU * v).sin())).sin(),
            Builtin::Sand => fbm(u, v, 16, 4, 33),
        }
    };
    HeightMap::from_fn(256, f)
}

// ---------- displacement ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapping {
    /// Projected from the three axes and blended by the surface direction (any shape).
    Triplanar,
    /// Wrapped around the vertical axis (cylinders, bottles, handles).
    Cylinder,
    /// Wrapped around the centre (balls, domes).
    Sphere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relief {
    /// The texture is added on top (the part grows by the depth).
    Raise,
    /// The texture is cut in (the outside stays where it was).
    Engrave,
    /// Half out, half in.
    Both,
}

#[derive(Debug, Clone, Copy)]
pub struct TextureOptions {
    /// Relief depth (mesh units).
    pub depth: f32,
    /// Size of one repeat of the texture (mesh units).
    pub tile: f32,
    pub rotation_deg: f32,
    pub mapping: Mapping,
    pub relief: Relief,
    pub invert: bool,
    /// The relief fades to nothing over this distance from the edge of the area.
    pub fade: f32,
    /// Triangle size for the relief (0 = tile / 10).
    pub detail: f32,
    pub max_triangles: usize,
}

impl Default for TextureOptions {
    fn default() -> Self {
        Self {
            depth: 0.6,
            tile: 5.0,
            rotation_deg: 0.0,
            mapping: Mapping::Triplanar,
            relief: Relief::Raise,
            invert: false,
            fade: 1.0,
            detail: 0.0,
            max_triangles: 3_000_000,
        }
    }
}

fn rot(p: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c)
}

/// Height at a surface point with normal `n` (0…1).
fn height_at(map: &HeightMap, o: &TextureOptions, p: Vec3, n: Vec3, centre: Vec3, radius: f32) -> f32 {
    let a = o.rotation_deg.to_radians();
    let t = o.tile.max(1e-4);
    let planar = |x: f32, y: f32| {
        let q = rot(Vec2::new(x, y), a) / t;
        map.sample(q.x, q.y)
    };
    let tri = || {
        let w = n.abs().powf(4.0);
        let w = w / (w.x + w.y + w.z).max(1e-9);
        w.x * planar(p.y, p.z) + w.y * planar(p.x, p.z) + w.z * planar(p.x, p.y)
    };
    let h = match o.mapping {
        Mapping::Triplanar => tri(),
        Mapping::Cylinder => {
            let d = p - centre;
            let th = d.y.atan2(d.x);
            // a whole number of repeats around, so there is no seam
            let k = (TAU * radius / t).round().max(1.0);
            let q = rot(Vec2::new(th / TAU * k, d.z / (TAU * radius / k)), a);
            let wrap = map.sample(q.x, q.y);
            // caps (facing up / down) use a flat projection
            let cap = smoothstep(0.6, 0.85, n.z.abs());
            wrap * (1.0 - cap) + planar(p.x, p.y) * cap
        }
        Mapping::Sphere => {
            let d = p - centre;
            let th = d.y.atan2(d.x);
            let ph = (d.z / d.length().max(1e-9)).clamp(-1.0, 1.0).asin();
            let k = (TAU * radius / t).round().max(1.0);
            let kl = (PI * radius / t).round().max(1.0);
            let q = rot(Vec2::new(th / TAU * k, (ph / PI + 0.5) * kl), a);
            map.sample(q.x, q.y)
        }
    };
    if o.invert {
        1.0 - h
    } else {
        h
    }
}

#[derive(PartialEq)]
struct Item(f32, u32);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0)
    }
}

/// Press `map` into the selected triangles of `m` (all of them when `sel` is None).
/// Returns the new mesh and whether the triangle budget limited the detail.
pub fn displace(m: &Mesh, sel: Option<&[bool]>, map: &HeightMap, o: &TextureOptions, prog: &Progress) -> Result<(Mesh, bool)> {
    let all = vec![true; m.triangles.len()];
    let sel = sel.unwrap_or(&all);
    let edge = if o.detail > 0.0 { o.detail } else { o.tile / 10.0 }.max(o.tile / 40.0).max(1e-3);
    prog.stage("Adding detail");
    let (mut r, rsel) = refine_where(m, sel, edge, o.max_triangles);
    prog.set(0.5);
    prog.check()?;
    let limited = r.triangles.len() * 100 >= o.max_triangles * 95;
    let nv = r.positions.len();
    // smooth vertex normals from all faces
    let mut vn = vec![Vec3::ZERO; nv];
    let (mut in_sel, mut in_rest) = (vec![false; nv], vec![false; nv]);
    for (i, t) in r.triangles.iter().enumerate() {
        let [a, b, c] = r.tri(i);
        let cr = (b - a).cross(c - a);
        for &v in t {
            vn[v as usize] += cr;
            if rsel[i] {
                in_sel[v as usize] = true;
            } else {
                in_rest[v as usize] = true;
            }
        }
    }
    // distance from the edge of the area (along edges), for the fade
    let mut dist = vec![f32::MAX; nv];
    if o.fade > 0.0 {
        let mut adj: Vec<Vec<u32>> = vec![vec![]; nv];
        for t in &r.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                adj[a as usize].push(b);
                adj[b as usize].push(a);
            }
        }
        let mut heap = BinaryHeap::new();
        for v in 0..nv {
            if in_sel[v] && in_rest[v] {
                dist[v] = 0.0;
                heap.push(Item(0.0, v as u32));
            }
        }
        while let Some(Item(d, v)) = heap.pop() {
            if d > dist[v as usize] || d > o.fade {
                continue;
            }
            for &w in &adj[v as usize] {
                if !in_sel[w as usize] {
                    continue;
                }
                let nd = d + r.positions[v as usize].distance(r.positions[w as usize]);
                if nd < dist[w as usize] {
                    dist[w as usize] = nd;
                    heap.push(Item(nd, w));
                }
            }
        }
    }
    prog.set(0.7);
    prog.check()?;
    let pts: Vec<Vec3> = (0..nv).filter(|&v| in_sel[v]).map(|v| r.positions[v]).collect();
    let b = crate::mesh::Aabb::from_points(pts.iter());
    let centre = b.center();
    let radius = match o.mapping {
        Mapping::Cylinder => pts.iter().map(|p| (*p - centre).truncate().length()).sum::<f32>() / pts.len().max(1) as f32,
        _ => pts.iter().map(|p| (*p - centre).length()).sum::<f32>() / pts.len().max(1) as f32,
    }
    .max(1e-3);
    let mid = match o.relief {
        Relief::Raise => 0.0,
        Relief::Engrave => 1.0,
        Relief::Both => 0.5,
    };
    let old = r.positions.clone();
    for v in 0..nv {
        if !in_sel[v] {
            continue;
        }
        let n = vn[v].normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        let fade = if o.fade > 0.0 {
            smoothstep(0.0, o.fade, dist[v])
        } else if in_rest[v] {
            0.0
        } else {
            1.0
        };
        let h = height_at(map, o, old[v], n, centre, radius);
        r.positions[v] = old[v] + n * (o.depth * (h - mid) * fade);
    }
    prog.set(1.0);
    Ok((r, limited))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn builtins_tile_seamlessly() {
        for (b, name) in BUILTINS {
            let m = builtin(b);
            // left and right edges, top and bottom edges match closely
            let mut worst = 0.0f32;
            for k in 0..64 {
                let t = k as f32 / 64.0;
                worst = worst.max((m.sample(0.0, t) - m.sample(1.0, t)).abs());
                worst = worst.max((m.sample(t, 0.0) - m.sample(t, 1.0)).abs());
            }
            assert!(worst < 1e-4, "{name}");
            let (lo, hi) = m.v.iter().fold((1.0f32, 0.0f32), |(a, b), &x| (a.min(x), b.max(x)));
            assert!(hi - lo > 0.3, "{name} has too little relief");
        }
    }

    #[test]
    fn textured_top_stays_closed() {
        let m = cuboid(Vec3::splat(20.0));
        let sel: Vec<bool> = (0..12).map(|i| m.face_normal(i).z > 0.9).collect();
        let o = TextureOptions { depth: 1.0, tile: 5.0, fade: 0.0, ..Default::default() };
        let (t, limited) = displace(&m, Some(&sel), &builtin(Builtin::Knurl), &o, &Progress::new()).unwrap();
        assert!(!limited);
        assert!(analyze(&t).is_watertight(), "{:?}", analyze(&t).problems());
        let b = t.bounds();
        assert!((b.max.z - 11.0).abs() < 0.05, "{}", b.max.z);
        assert!((b.max.x - 10.0).abs() < 1e-4, "sides untouched");
        assert!(t.volume() > 8000.0 + 50.0);
        // engraving keeps the outside where it was
        let o = TextureOptions { relief: Relief::Engrave, ..o };
        let (e, _) = displace(&m, Some(&sel), &builtin(Builtin::Ribs), &o, &Progress::new()).unwrap();
        assert!(e.bounds().max.z <= 10.0 + 1e-3 && e.volume() < 8000.0);
    }

    #[test]
    fn wraps_a_cylinder() {
        let c = crate::primitives::frustum(10.0, 10.0, 20.0, 64);
        let o = TextureOptions { depth: 0.5, tile: 4.0, mapping: Mapping::Cylinder, ..Default::default() };
        let (t, _) = displace(&c, None, &builtin(Builtin::Ribs), &o, &Progress::new()).unwrap();
        assert!(analyze(&t).is_watertight());
        let r = t.positions.iter().map(|p| p.truncate().length()).fold(0.0, f32::max);
        assert!(r > 10.3 && r < 10.6, "{r}");
    }
}
