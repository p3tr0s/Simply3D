//! Text → closed solid. Glyph outlines (TrueType/OpenType via ttf-parser) are
//! flattened, sorted into outer contours and holes by nesting depth, triangulated
//! with earcut and extruded. Overlapping contours (variable fonts) are unioned.

use crate::csg::{boolean_many, BoolOp};
use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::{Vec2, Vec3};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextOptions {
    pub text: String,
    /// Height of capital letters in mm.
    pub size: f32,
    /// Extrusion depth in mm.
    pub depth: f32,
    /// Extra space between letters in mm.
    pub spacing: f32,
    /// Line height as a multiple of the font's natural line height.
    pub line_spacing: f32,
    pub align: Align,
}

impl Default for TextOptions {
    fn default() -> Self {
        TextOptions { text: "Text".into(), size: 10.0, depth: 2.0, spacing: 0.0, line_spacing: 1.0, align: Align::Center }
    }
}

/// Collects flattened contours in font units.
struct Contours {
    all: Vec<Vec<Vec2>>,
    cur: Vec<Vec2>,
    tol: f32,
}

impl Contours {
    fn finish(&mut self) {
        let mut c = std::mem::take(&mut self.cur);
        c.dedup_by(|a, b| a.distance_squared(*b) < 1e-6);
        while c.len() > 1 && c[0].distance_squared(*c.last().unwrap()) < 1e-6 {
            c.pop();
        }
        remove_collinear(&mut c);
        if c.len() >= 3 && signed_area(&c).abs() > 1e-3 {
            self.all.push(c);
        }
    }
    fn segments(&self, approx_len: f32) -> usize {
        ((approx_len / self.tol).sqrt().ceil() as usize).clamp(2, 24)
    }
}

impl OutlineBuilder for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.cur.push(Vec2::new(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cur.push(Vec2::new(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = *self.cur.last().unwrap_or(&Vec2::ZERO);
        let (p1, p2) = (Vec2::new(x1, y1), Vec2::new(x, y));
        let n = self.segments(p0.distance(p1) + p1.distance(p2));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.cur.push(p0 * u * u + p1 * 2.0 * u * t + p2 * t * t);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = *self.cur.last().unwrap_or(&Vec2::ZERO);
        let (p1, p2, p3) = (Vec2::new(x1, y1), Vec2::new(x2, y2), Vec2::new(x, y));
        let n = self.segments(p0.distance(p1) + p1.distance(p2) + p2.distance(p3));
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.cur.push(p0 * u * u * u + p1 * 3.0 * u * u * t + p2 * 3.0 * u * t * t + p3 * t * t * t);
        }
    }
    fn close(&mut self) {
        self.finish();
    }
}

/// Drop points exactly on the line through their neighbours. earcut removes these
/// internally, which would leave the walls with vertices the caps don't have.
fn remove_collinear(c: &mut Vec<Vec2>) {
    let mut changed = true;
    while changed && c.len() > 3 {
        changed = false;
        let n = c.len();
        for i in 0..n {
            let (p, q, r) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
            let (px, py, qx, qy, rx, ry) = (p.x as f64, p.y as f64, q.x as f64, q.y as f64, r.x as f64, r.y as f64);
            let area = (qy - py) * (rx - qx) - (qx - px) * (ry - qy);
            if area == 0.0 || q == p {
                c.remove(i);
                changed = true;
                break;
            }
        }
    }
}

fn signed_area(c: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..c.len() {
        let (p, q) = (c[i], c[(i + 1) % c.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn segments_intersect(a: &[Vec2], b: &[Vec2]) -> bool {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p - o).perp_dot(q - o);
    for i in 0..a.len() {
        let (p1, p2) = (a[i], a[(i + 1) % a.len()]);
        for j in 0..b.len() {
            let (q1, q2) = (b[j], b[(j + 1) % b.len()]);
            let d1 = cross(q1, q2, p1);
            let d2 = cross(q1, q2, p2);
            let d3 = cross(p1, p2, q1);
            let d4 = cross(p1, p2, q2);
            if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0 {
                return true;
            }
        }
    }
    false
}

/// Points shared by several contour corners (pixel fonts, pinched outlines) would make
/// non-manifold walls: move each occurrence a hair into its own corner's interior.
fn separate_touching(mut contours: Vec<Vec<Vec2>>) -> Vec<Vec<Vec2>> {
    use std::collections::HashMap;
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    for c in &contours {
        for p in c {
            *count.entry((p.x.to_bits(), p.y.to_bits())).or_default() += 1;
        }
    }
    if count.values().all(|&n| n == 1) {
        return contours;
    }
    let eps = 1e-3;
    for c in contours.iter_mut() {
        let s = signed_area(c).signum();
        let orig = c.clone();
        let n = orig.len();
        for i in 0..n {
            let p = orig[i];
            if count[&(p.x.to_bits(), p.y.to_bits())] < 2 {
                continue;
            }
            let (a, b) = (orig[(i + n - 1) % n], orig[(i + 1) % n]);
            let left = |e: Vec2| Vec2::new(-e.y, e.x).normalize_or_zero();
            let (n1, n2) = (left(p - a), left(b - p));
            let mut d = (n1 + n2).normalize_or_zero();
            if d == Vec2::ZERO {
                d = n1;
            }
            c[i] = p + d * s * eps;
        }
    }
    contours
}

/// A filled region: outer contour (CCW) with holes (CW), in mm.
struct Region {
    outer: Vec<Vec2>,
    holes: Vec<Vec<Vec2>>,
}

/// Sort contours into regions by nesting depth (even = outer, odd = hole).
fn regions(contours: Vec<Vec<Vec2>>) -> Vec<Region> {
    let n = contours.len();
    let areas: Vec<f32> = contours.iter().map(|c| signed_area(c).abs()).collect();
    let contains = |i: usize, j: usize| -> bool {
        // does contour i contain contour j? (test a few points of j)
        i != j && areas[i] > areas[j] && contours[j].iter().take(3).all(|p| point_in_polygon(*p, &contours[i]))
    };
    let depth: Vec<usize> = (0..n).map(|j| (0..n).filter(|&i| contains(i, j)).count()).collect();
    let mut out: Vec<(usize, Region)> = vec![];
    for i in 0..n {
        if depth[i] % 2 == 0 {
            let mut c = contours[i].clone();
            if signed_area(&c) < 0.0 {
                c.reverse();
            }
            out.push((i, Region { outer: c, holes: vec![] }));
        }
    }
    for j in 0..n {
        if depth[j] % 2 == 1 {
            // parent = smallest containing contour one level up
            let parent = (0..n).filter(|&i| depth[i] + 1 == depth[j] && contains(i, j)).min_by(|&a, &b| areas[a].total_cmp(&areas[b]));
            if let Some(pi) = parent {
                if let Some((_, r)) = out.iter_mut().find(|(i, _)| *i == pi) {
                    let mut h = contours[j].clone();
                    if signed_area(&h) > 0.0 {
                        h.reverse();
                    }
                    r.holes.push(h);
                }
            }
        }
    }
    out.into_iter().map(|(_, r)| r).collect()
}

/// Extrude one region from z=0 to z=depth into a closed solid.
fn extrude(r: &Region, depth: f32, soup: &mut Vec<[Vec3; 3]>) -> Result<()> {
    let mut flat: Vec<f64> = vec![];
    let mut holes_idx = vec![];
    let mut pts: Vec<Vec2> = vec![];
    for p in &r.outer {
        flat.extend([p.x as f64, p.y as f64]);
        pts.push(*p);
    }
    for h in &r.holes {
        holes_idx.push(pts.len());
        for p in h {
            flat.extend([p.x as f64, p.y as f64]);
            pts.push(*p);
        }
    }
    let tris = earcutr::earcut(&flat, &holes_idx, 2).map_err(|e| Error::Parse(format!("text triangulation failed: {e:?}")))?;
    let mut edges: std::collections::HashMap<(usize, usize), (usize, usize, u32)> = Default::default();
    for t in tris.chunks_exact(3) {
        let (i, mut j, mut k) = (t[0], t[1], t[2]);
        if (pts[j] - pts[i]).perp_dot(pts[k] - pts[i]) < 0.0 {
            std::mem::swap(&mut j, &mut k);
        }
        let (a, b, c) = (pts[i], pts[j], pts[k]);
        soup.push([a.extend(depth), b.extend(depth), c.extend(depth)]); // top, +Z
        soup.push([a.extend(0.0), c.extend(0.0), b.extend(0.0)]); // bottom, -Z
        for (p, q) in [(i, j), (j, k), (k, i)] {
            let e = edges.entry((p.min(q), p.max(q))).or_insert((p, q, 0));
            e.2 += 1;
        }
    }
    // walls along the caps' boundary edges (interior on the left), so caps and walls
    // always share exactly the same vertices
    for (p, q, n) in edges.into_values() {
        if n == 1 {
            let (p, q) = (pts[p], pts[q]);
            soup.push([p.extend(0.0), q.extend(0.0), q.extend(depth)]);
            soup.push([p.extend(0.0), q.extend(depth), p.extend(depth)]);
        }
    }
    Ok(())
}

/// Closed solid from closed 2D outlines (any orientation; nesting decides holes),
/// extruded from z = 0 to z = depth. Used for image contours.
pub fn extrude_contours(contours: Vec<Vec<Vec2>>, depth: f32) -> Result<Mesh> {
    let contours: Vec<Vec<Vec2>> = contours
        .into_iter()
        .map(|mut c| {
            remove_collinear(&mut c);
            c
        })
        .filter(|c| c.len() >= 3 && signed_area(c).abs() > 1e-9)
        .collect();
    let rs = regions(separate_touching(contours));
    let mut soup = vec![];
    for r in &rs {
        extrude(r, depth, &mut soup)?;
    }
    let m = Mesh::from_soup(&soup);
    if crate::analysis::analyze(&m).is_watertight() {
        return Ok(m);
    }
    let mut solids = vec![];
    for r in &rs {
        let mut soup = vec![];
        extrude(r, depth, &mut soup)?;
        solids.push(Mesh::from_soup(&soup));
    }
    if solids.is_empty() {
        return Ok(Mesh::default());
    }
    let first = solids.remove(0);
    boolean_many(&first, &solids, BoolOp::Union)
}

/// Build the text as a closed mesh lying on the XY plane (reading along +X, up = +Y),
/// extruded from z = 0 to z = depth and centred on the origin in X/Y.
pub fn text_mesh(font: &[u8], face_index: u32, opts: &TextOptions) -> Result<Mesh> {
    let face = Face::parse(font, face_index).map_err(|e| Error::Parse(format!("font: {e}")))?;
    let upem = face.units_per_em() as f32;
    // capital height: measure a real 'H' (the OS/2 value is often off), else the table value
    let cap = face
        .glyph_index('H')
        .and_then(|g| face.glyph_bounding_box(g))
        .map(|b| b.y_max as f32)
        .filter(|h| *h > 0.0)
        .or_else(|| face.capital_height().filter(|c| *c > 0).map(|c| c as f32))
        .unwrap_or(upem * 0.7);
    let scale = opts.size / cap;
    let line_h = (face.ascender() as f32 - face.descender() as f32 + face.line_gap() as f32) * scale * opts.line_spacing.max(0.1);
    let kern = |a: GlyphId, b: GlyphId| -> f32 {
        face.tables()
            .kern
            .and_then(|k| k.subtables.into_iter().filter(|s| s.horizontal && !s.variable).find_map(|s| s.glyphs_kerning(a, b)))
            .unwrap_or(0) as f32
    };

    let lines: Vec<&str> = opts.text.lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err(Error::Parse("no text".into()));
    }
    let mut pieces: Vec<Vec<Region>> = vec![];
    let mut overlapping = false;
    for (li, line) in lines.iter().enumerate() {
        // lay out the line
        let mut glyphs: Vec<(GlyphId, f32)> = vec![];
        let mut x = 0.0;
        let mut prev: Option<GlyphId> = None;
        for ch in line.chars() {
            let gid = face.glyph_index(ch).unwrap_or(GlyphId(0));
            if let Some(p) = prev {
                x += kern(p, gid) * scale;
            }
            glyphs.push((gid, x));
            x += face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale + opts.spacing;
            prev = Some(gid);
        }
        let width = (x - opts.spacing).max(0.0);
        let x0 = match opts.align {
            Align::Left => 0.0,
            Align::Center => -width * 0.5,
            Align::Right => -width,
        };
        let y0 = -(li as f32) * line_h;
        for (gid, gx) in glyphs {
            let mut c = Contours { all: vec![], cur: vec![], tol: upem * 0.0004 };
            if face.outline_glyph(gid, &mut c).is_none() {
                continue; // space etc.
            }
            c.finish();
            let contours: Vec<Vec<Vec2>> =
                c.all.into_iter().map(|ct| ct.into_iter().map(|p| Vec2::new(x0 + gx + p.x * scale, y0 + p.y * scale)).collect()).collect();
            let rs = regions(separate_touching(contours));
            // overlapping outers inside one glyph (variable / some display fonts) need a union
            for i in 0..rs.len() {
                for j in i + 1..rs.len() {
                    if segments_intersect(&rs[i].outer, &rs[j].outer) {
                        overlapping = true;
                    }
                }
            }
            pieces.push(rs);
        }
    }
    let depth = opts.depth.max(0.01);
    let union_all = || -> Result<Mesh> {
        // union every region separately: handles overlapping / touching contours
        let mut solids = vec![];
        for r in pieces.iter().flatten() {
            let mut soup = vec![];
            extrude(r, depth, &mut soup)?;
            solids.push(Mesh::from_soup(&soup));
        }
        if solids.is_empty() {
            return Ok(Mesh::default());
        }
        let first = solids.remove(0);
        boolean_many(&first, &solids, BoolOp::Union)
    };
    let mut mesh = if overlapping {
        union_all()?
    } else {
        let mut soup = vec![];
        for r in pieces.iter().flatten() {
            extrude(r, depth, &mut soup)?;
        }
        let m = Mesh::from_soup(&soup);
        if crate::analysis::analyze(&m).is_watertight() {
            m
        } else {
            union_all().unwrap_or(m) // touching contours (pixel fonts) etc.
        }
    };
    if mesh.is_empty() {
        return Err(Error::Parse("the text has no visible characters in this font".into()));
    }
    // centre in X/Y
    let b = mesh.bounds();
    mesh.translate(Vec3::new(-b.center().x, -b.center().y, 0.0));
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    fn test_font() -> Option<Vec<u8>> {
        for p in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
        ] {
            if let Ok(b) = std::fs::read(p) {
                return Some(b);
            }
        }
        None
    }

    #[test]
    fn text_is_closed_and_sized() {
        let Some(font) = test_font() else { return };
        let opts = TextOptions { text: "Hello B3D\nAgo 80%".into(), size: 10.0, depth: 2.0, ..Default::default() };
        let m = text_mesh(&font, 0, &opts).unwrap();
        let r = analyze(&m);
        assert!(r.is_watertight(), "{r:?}");
        assert!(m.volume() > 0.0);
        let b = m.bounds();
        assert!((b.size().z - 2.0).abs() < 1e-4);
        assert!(b.size().x > 50.0 && b.size().y > 20.0, "{:?}", b.size());
        // 'o', 'B', '8', 'A', 'g', '%' have holes: shells > letters only works if holes are holes
        assert!(r.components >= 14, "{}", r.components);
    }

    #[test]
    fn capital_height_matches_size() {
        let Some(font) = test_font() else { return };
        let m = text_mesh(&font, 0, &TextOptions { text: "H".into(), size: 12.0, ..Default::default() }).unwrap();
        assert!((m.bounds().size().y - 12.0).abs() < 0.05, "{}", m.bounds().size().y);
    }
}

#[cfg(test)]
mod font_sweep {
    #[test]
    #[ignore]
    fn all_system_fonts() {
        let mut ok = 0;
        let mut bad = vec![];
        let mut stack = vec![std::path::PathBuf::from("/usr/share/fonts")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if !matches!(ext.as_str(), "ttf" | "otf") {
                    continue;
                }
                let data = std::fs::read(&p).unwrap();
                let opts = super::TextOptions { text: "Ag8%@B&Q".into(), ..Default::default() };
                match super::text_mesh(&data, 0, &opts) {
                    Ok(m) if crate::analysis::analyze(&m).is_watertight() => ok += 1,
                    Ok(m) => bad.push(format!("{} not watertight {:?}", p.display(), crate::analysis::analyze(&m).problems())),
                    Err(e) => bad.push(format!("{} error {e}", p.display())),
                }
            }
        }
        eprintln!("SWEEP ok={ok} bad={}", bad.len());
        for b in bad.iter().take(15) {
            eprintln!("SWEEP {b}");
        }
    }
}
