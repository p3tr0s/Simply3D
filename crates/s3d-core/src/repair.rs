//! Mesh repair: welding, cleanup, consistent orientation and hole filling.

use crate::analysis::{boundary_loops, edge_map};
use crate::mesh::Mesh;
use glam::{Vec2, Vec3};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Default)]
pub struct RepairLog {
    pub welded_vertices: usize,
    pub removed_degenerate: usize,
    pub removed_duplicates: usize,
    pub removed_inner_walls: usize,
    pub flipped_faces: usize,
    pub filled_holes: usize,
    pub added_triangles: usize,
    pub inverted_components: usize,
}

impl RepairLog {
    pub fn summary(&self) -> String {
        let mut parts = vec![];
        let mut add = |n: usize, s: &str| {
            if n > 0 {
                // "1 holes filled" -> "1 hole filled"
                let s = if n == 1 { s.replacen("ices ", "ex ", 1).replacen("s ", " ", 1) } else { s.to_string() };
                parts.push(format!("{n} {s}"))
            }
        };
        add(self.welded_vertices, "vertices welded");
        add(self.removed_degenerate, "degenerate faces removed");
        add(self.removed_duplicates, "duplicate faces removed");
        add(self.removed_inner_walls, "faces of hidden inner walls removed");
        add(self.flipped_faces, "faces re-oriented");
        add(self.filled_holes, "holes filled");
        add(self.inverted_components, "inside-out parts fixed");
        if parts.is_empty() {
            "No problems found".into()
        } else {
            parts.join(", ")
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RepairOptions {
    pub weld_tolerance: f32,
    pub fill_holes: bool,
    pub max_hole_edges: usize,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self { weld_tolerance: 1e-4, fill_holes: true, max_hole_edges: 10_000 }
    }
}

/// Run the full repair pipeline in place.
pub fn repair(m: &mut Mesh, opts: RepairOptions) -> RepairLog {
    let mut log = RepairLog::default();
    log.welded_vertices = weld(m, opts.weld_tolerance);
    log.removed_degenerate = remove_degenerate(m);
    log.removed_inner_walls = remove_internal_walls(m);
    log.removed_duplicates = remove_duplicate_faces(m);
    log.flipped_faces = orient_consistently(m);
    if opts.fill_holes {
        let (h, t) = fill_holes(m, opts.max_hole_edges);
        log.filled_holes = h;
        log.added_triangles = t;
        if h > 0 {
            log.flipped_faces += orient_consistently(m);
        }
    }
    log.inverted_components = orient_outward(m);
    m.compact();
    log
}

/// Merge vertices closer than `tol`. Returns number of vertices removed.
pub fn weld(m: &mut Mesh, tol: f32) -> usize {
    let tol = tol.max(1e-9);
    let inv = 1.0 / tol;
    let key = |p: Vec3| ((p.x * inv).floor() as i64, (p.y * inv).floor() as i64, (p.z * inv).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut remap = vec![0u32; m.positions.len()];
    let mut out: Vec<Vec3> = Vec::with_capacity(m.positions.len());
    let tol2 = tol * tol;
    for (i, &p) in m.positions.iter().enumerate() {
        let k = key(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(c) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        for &j in c {
                            if out[j as usize].distance_squared(p) <= tol2 {
                                found = Some(j);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        remap[i] = found.unwrap_or_else(|| {
            let j = out.len() as u32;
            out.push(p);
            grid.entry(k).or_default().push(j);
            j
        });
    }
    let removed = m.positions.len() - out.len();
    for t in &mut m.triangles {
        for v in t.iter_mut() {
            *v = remap[*v as usize];
        }
    }
    m.positions = out;
    removed
}

pub fn remove_degenerate(m: &mut Mesh) -> usize {
    let before = m.triangles.len();
    let pos = &m.positions;
    m.triangles.retain(|t| {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            return false;
        }
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        (b - a).cross(c - a).length_squared() > 1e-18
    });
    before - m.triangles.len()
}

/// Remove zero-thickness walls inside the model: faces lying flat against a face that
/// points the opposite way (left by joining touching pieces, or by extruding faces whose
/// corners weren't joined). They hide the outside surface from booleans and slicers.
/// Returns the number of faces removed.
pub fn remove_internal_walls(m: &mut Mesh) -> usize {
    let nt = m.triangles.len();
    if nt == 0 {
        return 0;
    }
    let size = m.bounds().size().max_element().max(1e-3);
    let tol = size * 1e-5;
    // group faces by plane, ignoring which way they face
    let mut planes: HashMap<(i64, i64, i64, i64), Vec<(usize, bool)>> = HashMap::new();
    let mut normal = vec![Vec3::ZERO; nt];
    for i in 0..nt {
        let [a, b, c] = m.tri(i);
        let n = (b - a).cross(c - a);
        if n.length_squared() <= 1e-24 {
            continue;
        }
        let n = n.normalize();
        normal[i] = n;
        // the same way round for both sides
        let first = if n.x.abs() > 1e-6 { n.x } else if n.y.abs() > 1e-6 { n.y } else { n.z };
        let (cn, side) = if first > 0.0 { (n, true) } else { (-n, false) };
        let d = cn.dot(a);
        let k = ((cn.x * 1e3).round() as i64, (cn.y * 1e3).round() as i64, (cn.z * 1e3).round() as i64, (d / (tol * 20.0)).round() as i64);
        planes.entry(k).or_default().push((i, side));
    }
    let mut drop = vec![false; nt];
    let inside = |p: Vec3, t: usize| -> bool {
        let [a, b, c] = m.tri(t);
        let n = normal[t];
        if (p - a).dot(n).abs() > tol * 20.0 {
            return false;
        }
        let e = |u: Vec3, v: Vec3| (v - u).cross(p - u).dot(n);
        let lim = tol * size;
        e(a, b) > lim && e(b, c) > lim && e(c, a) > lim
    };
    for faces in planes.values() {
        let (pos, neg): (Vec<_>, Vec<_>) = faces.iter().partition(|f| f.1);
        if pos.is_empty() || neg.is_empty() {
            continue;
        }
        for (mine, theirs) in [(&pos, &neg), (&neg, &pos)] {
            for &(i, _) in mine.iter() {
                let [a, b, c] = m.tri(i);
                // a few points inside it (the centre alone can sit right on the other
                // side's diagonal)
                let pts = [(a + b + c) / 3.0, a * 0.6 + b * 0.2 + c * 0.2, a * 0.2 + b * 0.6 + c * 0.2, a * 0.2 + b * 0.2 + c * 0.6];
                if pts.iter().any(|&p| theirs.iter().any(|&(j, _)| inside(p, j))) {
                    drop[i] = true;
                }
            }
        }
    }
    let removed = drop.iter().filter(|&&d| d).count();
    if removed > 0 {
        if m.is_painted() {
            let mut k = 0;
            m.paint.retain(|_| {
                k += 1;
                !drop[k - 1]
            });
        }
        let mut k = 0;
        m.triangles.retain(|_| {
            k += 1;
            !drop[k - 1]
        });
    }
    removed
}

pub fn remove_duplicate_faces(m: &mut Mesh) -> usize {
    let before = m.triangles.len();
    let mut seen = HashSet::new();
    m.triangles.retain(|t| {
        let mut k = *t;
        k.sort_unstable();
        seen.insert(k)
    });
    before - m.triangles.len()
}

/// Make neighbouring faces agree on winding (BFS across manifold edges).
/// Returns number of faces flipped.
pub fn orient_consistently(m: &mut Mesh) -> usize {
    let em = edge_map(m);
    let nt = m.triangles.len();
    let mut adj: Vec<Vec<(usize, (u32, u32))>> = vec![vec![]; nt];
    for (key, uses) in &em {
        if uses.len() == 2 {
            adj[uses[0].0].push((uses[1].0, *key));
            adj[uses[1].0].push((uses[0].0, *key));
        }
    }
    let dir_of = |t: &[u32; 3], key: (u32, u32)| -> bool {
        // true if triangle traverses key.0 -> key.1
        (0..3).any(|k| t[k] == key.0 && t[(k + 1) % 3] == key.1)
    };
    let mut visited = vec![false; nt];
    let mut flipped = 0;
    for seed in 0..nt {
        if visited[seed] {
            continue;
        }
        visited[seed] = true;
        let mut q = VecDeque::from([seed]);
        while let Some(f) = q.pop_front() {
            for &(g, key) in &adj[f] {
                if visited[g] {
                    continue;
                }
                visited[g] = true;
                if dir_of(&m.triangles[f], key) == dir_of(&m.triangles[g], key) {
                    m.triangles[g].swap(1, 2);
                    flipped += 1;
                }
                q.push_back(g);
            }
        }
    }
    flipped
}

/// Flip any connected component with negative signed volume.
pub fn orient_outward(m: &mut Mesh) -> usize {
    let comps = m.connected_components();
    if comps.len() <= 1 {
        if m.volume() < 0.0 {
            m.flip();
            return 1;
        }
        return 0;
    }
    let mut out = Mesh::default();
    let mut n = 0;
    for mut c in comps {
        if c.volume() < 0.0 {
            c.flip();
            n += 1;
        }
        out.append(&c);
    }
    if n > 0 {
        *m = out;
    }
    n
}

/// Fill boundary loops. Returns (holes filled, triangles added).
pub fn fill_holes(m: &mut Mesh, max_edges: usize) -> (usize, usize) {
    let loops = boundary_loops(m);
    let mut edges: HashSet<(u32, u32)> = edge_map(m).into_keys().collect();
    let mut holes = 0;
    let mut added = 0;
    for lp in loops {
        if lp.len() < 3 || lp.len() > max_edges {
            continue;
        }
        // Boundary edges run in the direction of their existing triangle; the patch must run the other way.
        let poly: Vec<u32> = lp.iter().rev().copied().collect();
        let tris = ear_clip(&m.positions, &poly, &mut edges).unwrap_or_else(|| {
            // fallback: fan around the centroid
            let c = poly.iter().map(|&i| m.positions[i as usize]).sum::<Vec3>() / poly.len() as f32;
            let ci = m.positions.len() as u32;
            m.positions.push(c);
            (0..poly.len()).map(|i| [poly[i], poly[(i + 1) % poly.len()], ci]).collect()
        });
        added += tris.len();
        m.triangles.extend(tris);
        holes += 1;
    }
    (holes, added)
}

fn ear_clip(pos: &[Vec3], poly: &[u32], edges: &mut HashSet<(u32, u32)>) -> Option<Vec<[u32; 3]>> {
    let ekey = |a: u32, b: u32| (a.min(b), a.max(b));
    let n = poly.len();
    if n == 3 {
        return Some(vec![[poly[0], poly[1], poly[2]]]);
    }
    if n > 2000 {
        return None;
    }
    // Newell normal
    let mut nrm = Vec3::ZERO;
    for i in 0..n {
        let a = pos[poly[i] as usize];
        let b = pos[poly[(i + 1) % n] as usize];
        nrm += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    let nrm = nrm.try_normalize()?;
    let u = nrm.any_orthonormal_vector();
    let v = nrm.cross(u);
    let p2: Vec<Vec2> = poly
        .iter()
        .map(|&i| {
            let p = pos[i as usize];
            Vec2::new(p.dot(u), p.dot(v))
        })
        .collect();
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while idx.len() > 3 {
        guard += 1;
        if guard > n * n {
            return None;
        }
        let k = idx.len();
        let mut clipped = false;
        for i in 0..k {
            let (ia, ib, ic) = (idx[(i + k - 1) % k], idx[i], idx[(i + 1) % k]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue; // reflex or degenerate
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = p2[j];
                cross(a, b, p) > 0.0 && cross(b, c, p) > 0.0 && cross(c, a, p) > 0.0
            });
            if inside {
                continue;
            }
            // the new diagonal must not duplicate an existing edge (would make it non-manifold)
            let diag = ekey(poly[ia], poly[ic]);
            if idx.len() > 3 && edges.contains(&diag) {
                continue;
            }
            edges.insert(diag);
            out.push([poly[ia], poly[ib], poly[ic]]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            return None;
        }
    }
    out.push([poly[idx[0]], poly[idx[1]], poly[idx[2]]]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::{cuboid, sphere};

    #[test]
    fn hidden_inner_walls_are_removed() {
        // a box with a zero-thickness double wall through it, its two sides split along
        // different diagonals
        let mut m = cuboid(Vec3::splat(10.0));
        let n = m.positions.len() as u32;
        m.positions.extend([Vec3::new(0.0, -5.0, -5.0), Vec3::new(0.0, 5.0, -5.0), Vec3::new(0.0, 5.0, 5.0), Vec3::new(0.0, -5.0, 5.0)]);
        let (a, b, c, d) = (n, n + 1, n + 2, n + 3);
        m.triangles.extend([[a, b, c], [a, c, d], [b, a, d], [b, d, c]]);
        let log = repair(&mut m, RepairOptions::default());
        assert_eq!(log.removed_inner_walls, 4, "{log:?}");
        assert!(analyze(&m).is_watertight());
        assert_eq!(m.triangles.len(), 12);
        assert!((m.volume() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn repairs_open_flipped_soup() {
        // Build an STL-like soup (unwelded), punch a hole and flip a face
        let src = sphere(10.0, 32, 16);
        let mut soup: Vec<[Vec3; 3]> = (0..src.triangles.len()).map(|i| src.tri(i)).collect();
        soup.truncate(soup.len() - 5);
        soup[3].swap(1, 2);
        let mut m =
            Mesh::new(soup.iter().flatten().copied().collect(), (0..soup.len() as u32).map(|i| [3 * i, 3 * i + 1, 3 * i + 2]).collect());
        assert!(!analyze(&m).is_watertight());
        let log = repair(&mut m, RepairOptions::default());
        let r = analyze(&m);
        assert!(r.is_watertight(), "{r:?} {log:?}");
        assert!(r.volume_mm3 > 0.0);
        assert!(log.filled_holes >= 1);
    }

    #[test]
    fn fixes_inside_out() {
        let mut m = cuboid(Vec3::ONE);
        m.flip();
        let log = repair(&mut m, RepairOptions::default());
        assert_eq!(log.inverted_components, 1);
        assert!(m.volume() > 0.0);
    }
}
