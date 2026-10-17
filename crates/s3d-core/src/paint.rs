//! Per-triangle colour painting: brush and fill selection, carrying paint over to a
//! rebuilt mesh, refining big triangles, and the Bambu Studio / PrusaSlicer encoding
//! (`paint_color` / `slic3rpe:mmu_segmentation`, a serialized TriangleSelector tree).

use crate::mesh::Mesh;
use glam::Vec3;
use std::collections::HashMap;

/// Pack an sRGBA colour (alpha forced to 255 so a colour is never 0 = "unpainted").
pub fn pack(c: [u8; 4]) -> u32 {
    u32::from_le_bytes([c[0], c[1], c[2], 255])
}

pub fn unpack(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

// ---------- Bambu / Prusa paint strings ----------

/// Encode an unsplit triangle painted with filament `state` (1-based extruder; 0 = none).
/// Bit stream: 2 bits "split sides" (0), then the state: 2 bits if < 3, else `11` and
/// 4 bits of state − 3. Nibbles are written LSB-first, the hex string reversed.
pub fn bambu_encode(state: u32) -> String {
    match state {
        0 => String::new(),
        1 | 2 => format!("{:X}", state << 2),
        _ => {
            let n = (state.min(16) - 3) & 0xF;
            format!("{n:X}C")
        }
    }
}

struct Bits {
    nibbles: Vec<u8>,
    pos: usize,
}

impl Bits {
    fn new(s: &str) -> Bits {
        // the string is written last nibble first
        let nibbles = s.chars().rev().filter_map(|c| c.to_digit(16)).map(|d| d as u8).collect();
        Bits { nibbles, pos: 0 }
    }
    fn bit(&mut self) -> Option<u32> {
        let n = *self.nibbles.get(self.pos / 4)?;
        let b = (n >> (self.pos % 4)) & 1;
        self.pos += 1;
        Some(b as u32)
    }
    fn bits(&mut self, k: usize) -> Option<u32> {
        let mut v = 0;
        for i in 0..k {
            v |= self.bit()? << i;
        }
        Some(v)
    }
}

/// Walk a (possibly subdivided) triangle, adding each leaf's share of the area to its state.
fn decode_node(b: &mut Bits, weight: f32, acc: &mut HashMap<u32, f32>, depth: u32) -> Option<()> {
    if depth > 24 {
        return None;
    }
    let split = b.bits(2)?;
    if split > 0 {
        b.bits(2)?; // special side
        let children = split + 1;
        for _ in 0..children {
            decode_node(b, weight / children as f32, acc, depth + 1)?;
        }
    } else {
        let mut n = b.bits(2)?;
        if n == 3 {
            let mut m = b.bits(4)?;
            n = 3 + m;
            // longer form: every all-ones group continues
            while m == 15 {
                m = b.bits(4)?;
                n += m;
            }
        }
        *acc.entry(n).or_default() += weight;
    }
    Some(())
}

/// Decode a triangle's paint string to the filament that covers most of it (0 = none).
pub fn bambu_decode(s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    let mut acc = HashMap::new();
    let mut b = Bits::new(s);
    let _ = decode_node(&mut b, 1.0, &mut acc, 0);
    acc.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k).unwrap_or(0)
}

// ---------- geometry helpers ----------

/// Closest point on triangle abc to p (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
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

/// Triangles within `radius` of `center` whose face points the same way as `normal`
/// (so the brush doesn't paint through a thin wall).
pub fn brush(m: &Mesh, center: Vec3, radius: f32, normal: Vec3) -> Vec<usize> {
    brush_stroke(m, None, center, center, radius, normal)
}

/// Like [`brush`], along the segment `from`–`to` (one step of a stroke).
/// `bvh` (built for `m`) makes it only look at nearby triangles.
pub fn brush_stroke(m: &Mesh, bvh: Option<&crate::bvh::Bvh>, from: Vec3, to: Vec3, radius: f32, normal: Vec3) -> Vec<usize> {
    let r2 = radius * radius;
    let (blo, bhi) = (from.min(to) - Vec3::splat(radius), from.max(to) + Vec3::splat(radius));
    let steps = ((from.distance(to) / (radius * 0.5).max(1e-6)).ceil() as usize).clamp(1, 64);
    let mut out = vec![];
    let candidates: Vec<usize> = match bvh.filter(|b| b.len() == m.triangles.len()) {
        Some(b) => {
            let mut c = vec![];
            b.tris_in_box(blo, bhi, &mut c);
            c.sort_unstable();
            c
        }
        None => (0..m.triangles.len()).collect(),
    };
    for i in candidates {
        let [a, b, c] = m.tri(i);
        if a.min(b).min(c).cmpgt(bhi).any() || a.max(b).max(c).cmplt(blo).any() {
            continue;
        }
        if (b - a).cross(c - a).dot(normal) <= 0.0 {
            continue;
        }
        let near = (0..=steps).any(|k| {
            let p = from.lerp(to, k as f32 / steps as f32);
            closest_on_triangle(p, a, b, c).distance_squared(p) <= r2
        });
        if near {
            out.push(i);
        }
    }
    out
}

fn edge_map(m: &Mesh) -> HashMap<(u32, u32), Vec<usize>> {
    let mut e: HashMap<(u32, u32), Vec<usize>> = HashMap::with_capacity(m.triangles.len() * 2);
    for (i, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            e.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    e
}

/// Flood fill from `start` across edges where the surface bends less than `max_angle_deg`
/// (a flat face, a smooth curved area). 180° = the whole connected part.
pub fn fill_region(m: &Mesh, start: usize, max_angle_deg: f32) -> Vec<usize> {
    if start >= m.triangles.len() {
        return vec![];
    }
    let edges = edge_map(m);
    let cos = max_angle_deg.to_radians().cos();
    let normals: Vec<Vec3> = (0..m.triangles.len()).map(|i| m.face_normal(i)).collect();
    let mut seen = vec![false; m.triangles.len()];
    let mut stack = vec![start];
    seen[start] = true;
    let mut out = vec![];
    while let Some(i) = stack.pop() {
        out.push(i);
        let t = m.triangles[i];
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            for &j in &edges[&(a.min(b), a.max(b))] {
                if !seen[j] && (max_angle_deg >= 180.0 || normals[i].dot(normals[j]) >= cos) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    out
}

/// Split triangles until no edge is longer than `max_edge` (or the triangle budget is
/// reached), so a small brush can paint finer detail. Watertight: an edge is split for
/// both of its triangles at the same midpoint. Paint is inherited.
pub fn refine(m: &Mesh, max_edge: f32, max_triangles: usize) -> Mesh {
    let painted = m.is_painted();
    let tags: Vec<u64> = (0..m.triangles.len()).map(|i| m.paint_of(i) as u64).collect();
    let (mut out, tags) = refine_impl(m, tags, |_| true, max_edge, max_triangles);
    out.paint = if painted { tags.iter().map(|t| *t as u32).collect() } else { vec![] };
    out
}

/// Like [`refine`], but only the selected triangles ask to be split (neighbours split a
/// shared edge too, to stay watertight). Returns the mesh and the new selection.
pub fn refine_where(m: &Mesh, sel: &[bool], max_edge: f32, max_triangles: usize) -> (Mesh, Vec<bool>) {
    let painted = m.is_painted();
    let tags: Vec<u64> = (0..m.triangles.len()).map(|i| m.paint_of(i) as u64 | ((sel[i] as u64) << 32)).collect();
    let (mut out, tags) = refine_impl(m, tags, |t| t >> 32 == 1, max_edge, max_triangles);
    out.paint = if painted { tags.iter().map(|t| *t as u32).collect() } else { vec![] };
    (out, tags.iter().map(|t| t >> 32 == 1).collect())
}

fn refine_impl(m: &Mesh, mut tags: Vec<u64>, wants: impl Fn(u64) -> bool, max_edge: f32, max_triangles: usize) -> (Mesh, Vec<u64>) {
    let mut cur = m.clone();
    cur.paint.clear();
    for _ in 0..40 {
        let l2 = max_edge * max_edge;
        // longest-edge bisection: each triangle asks for its longest edge to be split
        // (its neighbour then splits that edge too), which keeps slivers from multiplying
        let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
        for (i, t) in cur.triangles.iter().enumerate() {
            if !wants(tags[i]) {
                continue;
            }
            let len = |k: usize| cur.positions[t[k] as usize].distance_squared(cur.positions[t[(k + 1) % 3] as usize]);
            let k = (0..3).max_by(|a, b| len(*a).total_cmp(&len(*b))).unwrap();
            if len(k) > l2 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                mids.entry((a.min(b), a.max(b))).or_insert(0);
            }
        }
        if mids.is_empty() || cur.triangles.len() + mids.len() * 2 > max_triangles {
            break;
        }
        let mut keys: Vec<(u32, u32)> = mids.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            let p = (cur.positions[k.0 as usize] + cur.positions[k.1 as usize]) * 0.5;
            cur.positions.push(p);
            mids.insert(k, (cur.positions.len() - 1) as u32);
        }
        let mut tris = Vec::with_capacity(cur.triangles.len() * 2);
        let mut new_tags = Vec::with_capacity(cur.triangles.len() * 2);
        for (i, t) in cur.triangles.iter().enumerate() {
            let mid = |k: usize| {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                mids.get(&(a.min(b), a.max(b))).copied()
            };
            let ms = [mid(0), mid(1), mid(2)];
            let n = ms.iter().flatten().count();
            let before = tris.len();
            match n {
                0 => tris.push(*t),
                3 => {
                    let (m0, m1, m2) = (ms[0].unwrap(), ms[1].unwrap(), ms[2].unwrap());
                    tris.extend([[t[0], m0, m2], [m0, t[1], m1], [m2, m1, t[2]], [m0, m1, m2]]);
                }
                1 => {
                    // rotate so the split edge is t[r]–t[r+1]
                    let r = ms.iter().position(|x| x.is_some()).unwrap();
                    let (a, b, c, m) = (t[r], t[(r + 1) % 3], t[(r + 2) % 3], ms[r].unwrap());
                    tris.extend([[a, m, c], [m, b, c]]);
                }
                _ => {
                    // two split edges: the unsplit one is t[r]–t[r+1]
                    let r = ms.iter().position(|x| x.is_none()).unwrap();
                    let (a, b, c) = (t[r], t[(r + 1) % 3], t[(r + 2) % 3]);
                    let (mbc, mca) = (ms[(r + 1) % 3].unwrap(), ms[(r + 2) % 3].unwrap());
                    tris.extend([[a, b, mbc], [a, mbc, mca], [mca, mbc, c]]);
                }
            }
            new_tags.extend(std::iter::repeat_n(tags[i], tris.len() - before));
        }
        cur.triangles = tris;
        tags = new_tags;
        for _ in 0..4 {
            if flip_pass(&mut cur, &tags) == 0 {
                break;
            }
        }
    }
    (cur, tags)
}

/// One sweep of Delaunay edge flips between coplanar triangles with the same tag (keeps
/// the shape exactly, makes splitting produce well-shaped triangles instead of needles).
fn flip_pass(m: &mut Mesh, tags: &[u64]) -> usize {
    let mut edges: HashMap<(u32, u32), (usize, usize, u8)> = HashMap::with_capacity(m.triangles.len() * 2);
    for (i, t) in m.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let e = edges.entry((a.min(b), a.max(b))).or_insert((i, usize::MAX, 0));
            if e.2 == 1 {
                e.1 = i;
            }
            e.2 += 1;
        }
    }
    let mut touched = vec![false; m.triangles.len()];
    let mut flips = 0;
    let keys: Vec<(u32, u32)> = edges.iter().filter(|(_, e)| e.2 == 2).map(|(k, _)| *k).collect();
    for key in keys {
        let (t1, t2, _) = edges[&key];
        if touched[t1] || touched[t2] || tags[t1] != tags[t2] {
            continue;
        }
        // t1 = (a, b, c) with a→b along the edge; t2 has b→a and apex d
        let tr1 = m.triangles[t1];
        let Some(k) = (0..3).find(|&k| (tr1[k].min(tr1[(k + 1) % 3]), tr1[k].max(tr1[(k + 1) % 3])) == key) else { continue };
        let (a, b, c) = (tr1[k], tr1[(k + 1) % 3], tr1[(k + 2) % 3]);
        let tr2 = m.triangles[t2];
        let Some(&d) = tr2.iter().find(|&&v| v != a && v != b) else { continue };
        if c == d || edges.contains_key(&(c.min(d), c.max(d))) {
            continue;
        }
        let p = |v: u32| m.positions[v as usize];
        let (pa, pb, pc, pd) = (p(a), p(b), p(c), p(d));
        let n1 = (pb - pa).cross(pc - pa);
        let n2 = (pa - pb).cross(pd - pb);
        if n1.normalize_or_zero().dot(n2.normalize_or_zero()) < 0.99995 {
            continue;
        }
        let angle = |o: Vec3, x: Vec3, y: Vec3| (x - o).angle_between(y - o);
        if angle(pc, pa, pb) + angle(pd, pa, pb) <= std::f32::consts::PI + 1e-3 {
            continue;
        }
        let (f1, f2) = ([a, d, c], [d, b, c]);
        // both new triangles must face the same way as before
        let nf = |t: [u32; 3]| (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        if nf(f1).dot(n1) <= 0.0 || nf(f2).dot(n1) <= 0.0 {
            continue;
        }
        m.triangles[t1] = f1;
        m.triangles[t2] = f2;
        touched[t1] = true;
        touched[t2] = true;
        flips += 1;
    }
    flips
}

/// Carry paint from `from` over to `to` (the same object after an edit that rebuilt the
/// triangles): each new triangle takes the paint of the old triangle nearest its centre,
/// if that one lies within `tol` and faces the same way. Returns whether anything was painted.
pub fn transfer(from: &Mesh, to: &mut Mesh, tol: f32) -> bool {
    if !from.is_painted() || to.triangles.is_empty() {
        return false;
    }
    let tris: Vec<usize> = (0..from.triangles.len()).filter(|&i| from.paint[i] != 0).collect();
    // a uniform grid over the painted triangles' boxes
    let b = from.bounds();
    let size = b.size().max(Vec3::splat(1e-3));
    let cells_per_axis = ((tris.len() as f32).cbrt() * 2.0).clamp(1.0, 128.0);
    let cell = (size.max_element() / cells_per_axis).max(tol * 2.0).max(1e-3);
    let key = |p: Vec3| -> [i32; 3] {
        let q = ((p - b.min) / cell).floor();
        [q.x as i32, q.y as i32, q.z as i32]
    };
    let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for &i in &tris {
        let [p, q, r] = from.tri(i);
        let lo = key(p.min(q).min(r) - Vec3::splat(tol));
        let hi = key(p.max(q).max(r) + Vec3::splat(tol));
        let span = (0..3).map(|k| (hi[k] - lo[k] + 1) as i64).product::<i64>();
        if span > 4096 {
            // a huge triangle: put it in its corner cells only (rare; keeps memory bounded)
            grid.entry(lo).or_default().push(i);
            grid.entry(hi).or_default().push(i);
            continue;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    grid.entry([x, y, z]).or_default().push(i);
                }
            }
        }
    }
    let normals: HashMap<usize, Vec3> = tris.iter().map(|&i| (i, from.face_normal(i))).collect();
    let mut paint = vec![0u32; to.triangles.len()];
    let mut any = false;
    for (j, slot) in paint.iter_mut().enumerate() {
        let [p, q, r] = to.tri(j);
        let c = (p + q + r) / 3.0;
        let n = (q - p).cross(r - p).normalize_or_zero();
        let Some(cands) = grid.get(&key(c)) else { continue };
        let mut best = (tol * tol, 0u32);
        for &i in cands {
            if normals[&i].dot(n) < 0.7 {
                continue;
            }
            let [a, b2, c2] = from.tri(i);
            let d = closest_on_triangle(c, a, b2, c2).distance_squared(c);
            if d <= best.0 {
                best = (d, from.paint[i]);
            }
        }
        if best.1 != 0 {
            *slot = best.1;
            any = true;
        }
    }
    if any {
        to.paint = paint;
    }
    any
}

/// The distinct paint colours of a mesh, most used first.
pub fn colors(m: &Mesh) -> Vec<u32> {
    if !m.is_painted() {
        return vec![];
    }
    let mut count: HashMap<u32, usize> = HashMap::new();
    for &c in m.paint.iter().filter(|&&c| c != 0) {
        *count.entry(c).or_default() += 1;
    }
    let mut v: Vec<(u32, usize)> = count.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|x| x.0).collect()
}

/// Filament slots for a multi-colour print: the objects' own colours first, then the
/// painted colours; at most 16. Empty when nothing is painted.
pub fn slots<'a>(bases: impl IntoIterator<Item = [u8; 4]>, meshes: impl IntoIterator<Item = &'a Mesh> + Clone) -> Vec<u32> {
    if !meshes.clone().into_iter().any(|m| m.is_painted()) {
        return vec![];
    }
    let mut v: Vec<u32> = vec![];
    for c in bases.into_iter().map(pack).chain(meshes.into_iter().flat_map(colors)) {
        if !v.contains(&c) && v.len() < 16 {
            v.push(c);
        }
    }
    v
}

/// 1-based slot for a colour: exact, else the nearest.
pub fn slot_of(slots: &[u32], c: u32) -> u32 {
    let d = |a: u32| {
        let (x, y) = (unpack(a), unpack(c));
        (0..3).map(|k| (x[k] as i32 - y[k] as i32).pow(2)).sum::<i32>()
    };
    slots.iter().enumerate().min_by_key(|(_, s)| d(**s)).map(|(i, _)| i as u32 + 1).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::cuboid;

    #[test]
    fn bambu_strings() {
        assert_eq!(bambu_encode(1), "4");
        assert_eq!(bambu_encode(2), "8");
        assert_eq!(bambu_encode(3), "0C");
        assert_eq!(bambu_encode(4), "1C");
        assert_eq!(bambu_encode(16), "DC");
        for s in 1..=16 {
            assert_eq!(bambu_decode(&bambu_encode(s)), s, "{s}");
        }
        assert_eq!(bambu_decode(""), 0);
        // split in 4 (split sides = 3): three children painted 2, one painted 1
        // bits: 11 (split 3) 00 (special) then 4 leaves: 00 01 | 00 10 | 00 10 | 00 10
        let bits = [1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
        let mut nib = vec![];
        for ch in bits.chunks(4) {
            nib.push(ch.iter().enumerate().fold(0u8, |v, (i, b)| v | ((*b as u8) << i)));
        }
        let s: String = nib.iter().rev().map(|n| format!("{n:X}")).collect();
        assert_eq!(bambu_decode(&s), 2);
    }

    #[test]
    fn fill_and_brush() {
        let m = cuboid(Vec3::splat(10.0));
        // a cube face is 2 triangles, bounded by 90° edges
        assert_eq!(fill_region(&m, 0, 30.0).len(), 2);
        assert_eq!(fill_region(&m, 0, 180.0).len(), 12);
        let top = Vec3::new(0.0, 0.0, 5.0);
        let b = brush(&m, top, 1.0, Vec3::Z);
        assert!(!b.is_empty() && b.iter().all(|&i| m.face_normal(i).z > 0.9));
    }

    #[test]
    fn refine_keeps_closed_and_paint() {
        let mut m = cuboid(Vec3::splat(10.0));
        m.paint = vec![0; 12];
        m.paint[0] = pack([255, 0, 0, 255]);
        let r = refine(&m, 2.0, 1_000_000);
        assert!(analyze(&r).is_watertight());
        assert!(r.triangles.len() > 12 * 16);
        assert_eq!(r.paint.len(), r.triangles.len());
        assert!((r.volume() - 1000.0).abs() < 1e-3);
        let painted_area: f32 = (0..r.triangles.len())
            .filter(|&i| r.paint[i] != 0)
            .map(|i| {
                let [a, b, c] = r.tri(i);
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        assert!((painted_area - 50.0).abs() < 1e-3, "{painted_area}");
    }

    #[test]
    fn transfer_follows_the_surface() {
        let mut m = cuboid(Vec3::splat(10.0));
        let top: Vec<usize> = (0..12).filter(|&i| m.face_normal(i).z > 0.9).collect();
        let red = pack([255, 0, 0, 255]);
        let p = m.paint_mut();
        for i in top {
            p[i] = red;
        }
        let mut finer = refine(&cuboid(Vec3::splat(10.0)), 3.0, 100_000);
        assert!(transfer(&m, &mut finer, 0.1));
        for i in 0..finer.triangles.len() {
            assert_eq!(finer.paint[i] == red, finer.face_normal(i).z > 0.9);
        }
    }
}
