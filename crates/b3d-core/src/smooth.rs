//! Smoothing: Loop subdivision (with optional sharp creases), flat midpoint
//! subdivision, and Taubin λ/μ relaxation (shrink-free Laplacian smoothing).

use crate::mesh::Mesh;
use glam::Vec3;
use std::collections::HashMap;

type EdgeKey = (u32, u32);

fn ek(a: u32, b: u32) -> EdgeKey {
    (a.min(b), a.max(b))
}

/// Edge -> opposite vertices of adjacent faces, plus face indices.
fn edge_faces(m: &Mesh) -> HashMap<EdgeKey, Vec<(u32, usize)>> {
    let mut map: HashMap<EdgeKey, Vec<(u32, usize)>> = HashMap::with_capacity(m.triangles.len() * 3 / 2);
    for (fi, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            map.entry(ek(t[k], t[(k + 1) % 3])).or_default().push((t[(k + 2) % 3], fi));
        }
    }
    map
}

/// Edges that are boundaries, non-manifold, or have a dihedral angle above `crease_deg`.
fn sharp_edges(m: &Mesh, ef: &HashMap<EdgeKey, Vec<(u32, usize)>>, crease_deg: Option<f32>) -> HashMap<EdgeKey, bool> {
    let cos_lim = crease_deg.map(|d| d.to_radians().cos());
    ef.iter()
        .map(|(k, f)| {
            let sharp = if f.len() != 2 {
                true
            } else if let Some(c) = cos_lim {
                m.face_normal(f[0].1).dot(m.face_normal(f[1].1)) < c
            } else {
                false
            };
            (*k, sharp)
        })
        .collect()
}

/// Split every triangle into four at edge midpoints, `levels` times. Shape is unchanged.
pub fn subdivide_flat(m: &Mesh, levels: u32) -> Mesh {
    let mut cur = m.clone();
    for _ in 0..levels {
        let mut out = Mesh { positions: cur.positions.clone(), triangles: Vec::with_capacity(cur.triangles.len() * 4) };
        let mut mids: HashMap<EdgeKey, u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, out: &mut Mesh| {
            *mids.entry(ek(a, b)).or_insert_with(|| {
                out.positions.push((cur.positions[a as usize] + cur.positions[b as usize]) * 0.5);
                (out.positions.len() - 1) as u32
            })
        };
        for t in &cur.triangles {
            let (a, b, c) = (t[0], t[1], t[2]);
            let ab = mid(a, b, &mut out);
            let bc = mid(b, c, &mut out);
            let ca = mid(c, a, &mut out);
            out.triangles.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        cur = out;
    }
    cur
}

/// Loop subdivision. With `crease_deg`, edges sharper than that angle stay sharp.
pub fn subdivide_loop(m: &Mesh, levels: u32, crease_deg: Option<f32>) -> Mesh {
    let mut cur = m.clone();
    for _ in 0..levels {
        let ef = edge_faces(&cur);
        let sharp = sharp_edges(&cur, &ef, crease_deg);
        let nv = cur.positions.len();
        // vertex neighbours and their sharp-edge neighbours
        let mut nbrs: Vec<Vec<u32>> = vec![vec![]; nv];
        let mut sharp_nbrs: Vec<Vec<u32>> = vec![vec![]; nv];
        for (&(a, b), &s) in &sharp {
            nbrs[a as usize].push(b);
            nbrs[b as usize].push(a);
            if s {
                sharp_nbrs[a as usize].push(b);
                sharp_nbrs[b as usize].push(a);
            }
        }
        let p = &cur.positions;
        // even (old) vertices
        let mut positions: Vec<Vec3> = (0..nv)
            .map(|v| {
                let s = &sharp_nbrs[v];
                match s.len() {
                    0 => {
                        let n = nbrs[v].len();
                        if n < 3 {
                            return p[v];
                        }
                        let beta = if n == 3 { 3.0 / 16.0 } else { 3.0 / (8.0 * n as f32) };
                        let sum: Vec3 = nbrs[v].iter().map(|&w| p[w as usize]).sum();
                        p[v] * (1.0 - n as f32 * beta) + sum * beta
                    }
                    2 => p[v] * 0.75 + (p[s[0] as usize] + p[s[1] as usize]) * 0.125,
                    _ => p[v], // corner or dart: keep
                }
            })
            .collect();
        // odd (edge) vertices
        let mut mids: HashMap<EdgeKey, u32> = HashMap::with_capacity(ef.len());
        for (&(a, b), opp) in &ef {
            let np = if sharp[&(a, b)] || opp.len() != 2 {
                (p[a as usize] + p[b as usize]) * 0.5
            } else {
                (p[a as usize] + p[b as usize]) * 0.375 + (p[opp[0].0 as usize] + p[opp[1].0 as usize]) * 0.125
            };
            positions.push(np);
            mids.insert((a, b), (positions.len() - 1) as u32);
        }
        let mut tris = Vec::with_capacity(cur.triangles.len() * 4);
        for t in &cur.triangles {
            let (a, b, c) = (t[0], t[1], t[2]);
            let (ab, bc, ca) = (mids[&ek(a, b)], mids[&ek(b, c)], mids[&ek(c, a)]);
            tris.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        cur = Mesh { positions, triangles: tris };
    }
    cur
}

/// Taubin smoothing (λ = 0.5, μ = -0.53). Boundary vertices stay fixed; with `crease_deg`,
/// vertices on sharp edges stay fixed too.
pub fn taubin(m: &mut Mesh, iterations: u32, crease_deg: Option<f32>) {
    let _ = taubin_with(m, iterations, crease_deg, &crate::Progress::new());
}

/// [`taubin`] with progress/cancellation (the mesh may be partially smoothed on cancel).
pub fn taubin_with(m: &mut Mesh, iterations: u32, crease_deg: Option<f32>, prog: &crate::Progress) -> crate::Result<()> {
    let ef = edge_faces(m);
    let sharp = sharp_edges(m, &ef, crease_deg);
    let nv = m.positions.len();
    let mut nbrs: Vec<Vec<u32>> = vec![vec![]; nv];
    let mut fixed = vec![false; nv];
    for (&(a, b), &s) in &sharp {
        nbrs[a as usize].push(b);
        nbrs[b as usize].push(a);
        if s {
            fixed[a as usize] = true;
            fixed[b as usize] = true;
        }
    }
    let step = |pos: &mut Vec<Vec3>, k: f32| {
        let old = pos.clone();
        for v in 0..nv {
            if fixed[v] || nbrs[v].is_empty() {
                continue;
            }
            let avg = nbrs[v].iter().map(|&w| old[w as usize]).sum::<Vec3>() / nbrs[v].len() as f32;
            pos[v] = old[v] + (avg - old[v]) * k;
        }
    };
    for it in 0..iterations {
        prog.check()?;
        prog.set(it as f32 / iterations.max(1) as f32);
        step(&mut m.positions, 0.5);
        step(&mut m.positions, -0.53);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::{cuboid, sphere};

    #[test]
    fn loop_on_cube_rounds_and_stays_closed() {
        let m = subdivide_loop(&cuboid(Vec3::splat(10.0)), 3, None);
        assert_eq!(m.triangles.len(), 12 * 64);
        assert!(analyze(&m).is_watertight());
        assert!(m.volume() < 1000.0 && m.volume() > 300.0, "{}", m.volume());
    }

    #[test]
    fn loop_with_creases_keeps_cube_shape() {
        let m = subdivide_loop(&cuboid(Vec3::splat(10.0)), 2, Some(30.0));
        assert!((m.volume() - 1000.0).abs() < 1e-2, "{}", m.volume());
    }

    #[test]
    fn loop_smooths_coarse_sphere() {
        // largest angle between neighbouring faces must drop sharply
        let max_dihedral = |m: &Mesh| {
            edge_faces(m)
                .values()
                .filter(|f| f.len() == 2)
                .map(|f| m.face_normal(f[0].1).dot(m.face_normal(f[1].1)).clamp(-1.0, 1.0).acos())
                .fold(0.0f32, f32::max)
        };
        let coarse = sphere(10.0, 8, 4);
        let fine = subdivide_loop(&coarse, 3, None);
        assert!(max_dihedral(&fine) < max_dihedral(&coarse) * 0.3, "{} vs {}", max_dihedral(&fine), max_dihedral(&coarse));
        assert!(analyze(&fine).is_watertight());
    }

    #[test]
    fn taubin_reduces_noise_without_shrinking() {
        let mut m = sphere(10.0, 64, 32);
        for (i, p) in m.positions.iter_mut().enumerate() {
            let n = ((i * 7919) % 101) as f32 / 101.0 - 0.5;
            *p *= 1.0 + n * 0.04;
        }
        let noise = |m: &Mesh| m.positions.iter().map(|p| (p.length() - 10.0).abs()).sum::<f32>() / m.positions.len() as f32;
        let before = noise(&m);
        taubin(&mut m, 10, None);
        assert!(noise(&m) < before * 0.5, "{} -> {}", before, noise(&m));
        let mean_r = m.positions.iter().map(|p| p.length()).sum::<f32>() / m.positions.len() as f32;
        assert!((mean_r - 10.0).abs() < 0.2, "shrunk to {mean_r}");
    }
}
