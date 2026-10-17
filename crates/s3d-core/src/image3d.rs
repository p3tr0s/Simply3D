//! Pictures → 3D, like 3D Builder's *Load image* (Contour, Height map, Edge, Stamp)
//! plus a lithophane mode. Sizes are in millimetres.

use crate::mesh::Mesh;
use crate::simplify::{self, SimplifyOptions};
use crate::text::extrude_contours;
use crate::{Error, Progress, Result};
use glam::{Vec2, Vec3};
use std::collections::HashMap;

/// A greyscale picture: `v` 0 = black … 1 = white, `a` = opacity, row 0 at the top.
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
    pub a: Vec<f32>,
}

impl Gray {
    pub fn has_alpha(&self) -> bool {
        self.a.iter().any(|&a| a < 0.99)
    }
}

pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "tga", "gif", "webp"];

pub fn is_image_path(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Decode PNG / JPEG / BMP / TGA / GIF / WebP. Transparent areas count as white.
pub fn load_image(bytes: &[u8]) -> Result<Gray> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Parse(format!("image: {e}")))?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut v = Vec::with_capacity(w * h);
    let mut a = Vec::with_capacity(w * h);
    for p in img.pixels() {
        let [r, g, b, al] = p.0.map(|c| c as f32 / 255.0);
        let luma = 0.299 * r + 0.587 * g + 0.114 * b;
        v.push(luma * al + (1.0 - al));
        a.push(al);
    }
    Ok(Gray { w, h, v, a })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageMode {
    /// Outline of the dark areas, extruded (logos, icons, silhouettes).
    Contour,
    /// Brightness becomes height (white = high).
    HeightMap,
    /// Edges become ridges (height from the brightness gradient).
    Edge,
    /// Mirrored raised contour on a base plate, for rubber-stamp style printing.
    Stamp,
    /// Thin panel that shows the picture when lit from behind (dark = thick).
    Lithophane,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageOptions {
    pub mode: ImageMode,
    /// Width of the result in mm (height follows the picture's aspect).
    pub width: f32,
    /// Longest side in pixels after downsampling (detail vs. speed).
    pub resolution: u32,
    /// Blur radius in pixels (0–8).
    pub smooth: u32,
    /// Height map: number of height steps (2–256).
    pub levels: u32,
    /// Contour / stamp: brightness that separates solid (darker) from empty.
    pub threshold: f32,
    pub invert: bool,
    /// Relief height / extrusion height in mm.
    pub height: f32,
    /// Base thickness in mm.
    pub base: f32,
    /// Lithophane border width in mm.
    pub frame: f32,
    /// Use the picture's transparency as its outline.
    pub use_alpha: bool,
    /// Merge flat areas of height fields into fewer triangles (slower).
    pub reduce: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            mode: ImageMode::Contour,
            width: 60.0,
            resolution: 250,
            smooth: 1,
            levels: 256,
            threshold: 0.5,
            invert: false,
            height: 3.0,
            base: 0.0,
            frame: 3.0,
            use_alpha: true,
            reduce: true,
        }
    }
}

impl ImageOptions {
    /// Sensible starting values when switching mode.
    pub fn for_mode(mode: ImageMode) -> (f32, f32) {
        // (height, base)
        match mode {
            ImageMode::Contour => (3.0, 0.0),
            ImageMode::HeightMap => (4.0, 1.0),
            ImageMode::Edge => (2.0, 1.0),
            ImageMode::Stamp => (2.0, 3.0),
            ImageMode::Lithophane => (2.2, 0.8),
        }
    }
}

/// The picture after resizing, inverting, smoothing and (per mode) posterising or edge
/// detection. `v` is what the mode turns into geometry; `mask` is which pixels exist.
pub struct Field {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
    pub mask: Vec<bool>,
    /// mm per pixel
    pub px: f32,
}

fn downsample(g: &Gray, max_side: usize) -> Gray {
    let k = (g.w.max(g.h) as f32 / max_side.max(8) as f32).max(1.0);
    if k <= 1.0 {
        return g.clone();
    }
    let (w, h) = (((g.w as f32 / k).round() as usize).max(1), ((g.h as f32 / k).round() as usize).max(1));
    let mut v = vec![0.0; w * h];
    let mut a = vec![0.0; w * h];
    for y in 0..h {
        let (y0, y1) = ((y * g.h) / h, (((y + 1) * g.h) / h).max((y * g.h) / h + 1));
        for x in 0..w {
            let (x0, x1) = ((x * g.w) / w, (((x + 1) * g.w) / w).max((x * g.w) / w + 1));
            let (mut sv, mut sa, mut n) = (0.0, 0.0, 0.0);
            for yy in y0..y1.min(g.h) {
                for xx in x0..x1.min(g.w) {
                    sv += g.v[yy * g.w + xx];
                    sa += g.a[yy * g.w + xx];
                    n += 1.0;
                }
            }
            v[y * w + x] = sv / n;
            a[y * w + x] = sa / n;
        }
    }
    Gray { w, h, v, a }
}

/// Separable box blur, applied twice (≈ Gaussian).
fn blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0.0; v.len()];
    for _ in 0..2 {
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
                tmp[y * w + x] = (a..=b).map(|i| v[y * w + i]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            for x in 0..w {
                v[y * w + x] = (a..=b).map(|j| tmp[j * w + x]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
    }
}

pub fn prepare(img: &Gray, o: &ImageOptions) -> Field {
    let g = downsample(img, o.resolution as usize);
    let (w, h) = (g.w, g.h);
    let mut v = g.v.clone();
    if o.invert {
        v.iter_mut().for_each(|x| *x = 1.0 - *x);
    }
    blur(&mut v, w, h, o.smooth.min(16) as usize);
    match o.mode {
        ImageMode::Edge => {
            let at = |x: isize, y: isize| v[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
            let mut e = vec![0.0f32; w * h];
            for y in 0..h as isize {
                for x in 0..w as isize {
                    let gx =
                        at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x - 1, y) - at(x - 1, y + 1);
                    let gy =
                        at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x, y - 1) - at(x + 1, y - 1);
                    e[y as usize * w + x as usize] = (gx * gx + gy * gy).sqrt();
                }
            }
            let max = e.iter().cloned().fold(1e-6, f32::max);
            v = e.into_iter().map(|x| x / max).collect();
        }
        _ => {}
    }
    if matches!(o.mode, ImageMode::HeightMap | ImageMode::Edge | ImageMode::Lithophane) && o.levels >= 2 && o.levels < 256 {
        let l = (o.levels - 1) as f32;
        v.iter_mut().for_each(|x| *x = (*x * l).round() / l);
    }
    let alpha = o.use_alpha && g.has_alpha();
    let mask = (0..w * h).map(|i| !alpha || g.a[i] > 0.5).collect();
    Field { w, h, v, mask, px: o.width.max(1.0) / w as f32 }
}

// ---------- contours (marching squares) ----------

/// Closed outlines (in mm, y up) around pixels with v < threshold.
pub fn contours(f: &Field, threshold: f32) -> Vec<Vec<Vec2>> {
    let (w, h) = (f.w + 2, f.h + 2);
    // signed field on the padded grid: > 0 inside (solid)
    let g = |i: usize, j: usize| -> f32 {
        if i == 0 || j == 0 || i == w - 1 || j == h - 1 {
            return -1.0;
        }
        let k = (j - 1) * f.w + (i - 1);
        if !f.mask[k] {
            return -1.0;
        }
        let d = threshold - f.v[k];
        if d == 0.0 {
            -1e-6
        } else {
            d
        }
    };
    let pos = |i: f32, j: f32| Vec2::new((i - 0.5) * f.px, (f.h as f32 - j + 0.5) * f.px);
    // crossing point on a grid edge, identified by (edge id)
    let hedge = |i: usize, j: usize| (j * w + i) * 2; // (i,j)-(i+1,j)
    let vedge = |i: usize, j: usize| (j * w + i) * 2 + 1; // (i,j)-(i,j+1)
    let mut points: HashMap<usize, Vec2> = HashMap::new();
    let mut point = |id: usize, i0: usize, j0: usize, i1: usize, j1: usize| -> usize {
        points.entry(id).or_insert_with(|| {
            let (a, b) = (g(i0, j0), g(i1, j1));
            let t = a / (a - b);
            pos(i0 as f32 + (i1 as f32 - i0 as f32) * t, j0 as f32 + (j1 as f32 - j0 as f32) * t)
        });
        id
    };
    let mut segs: Vec<(usize, usize)> = vec![];
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            let c = [g(i, j) > 0.0, g(i + 1, j) > 0.0, g(i + 1, j + 1) > 0.0, g(i, j + 1) > 0.0];
            let idx = c[0] as u8 | (c[1] as u8) << 1 | (c[2] as u8) << 2 | (c[3] as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            let top = point(hedge(i, j), i, j, i + 1, j);
            let right = point(vedge(i + 1, j), i + 1, j, i + 1, j + 1);
            let bottom = point(hedge(i, j + 1), i, j + 1, i + 1, j + 1);
            let left = point(vedge(i, j), i, j, i, j + 1);
            let center = (g(i, j) + g(i + 1, j) + g(i + 1, j + 1) + g(i, j + 1)) > 0.0;
            match idx {
                1 | 14 => segs.push((left, top)),
                2 | 13 => segs.push((top, right)),
                3 | 12 => segs.push((left, right)),
                4 | 11 => segs.push((right, bottom)),
                6 | 9 => segs.push((top, bottom)),
                7 | 8 => segs.push((left, bottom)),
                5 => {
                    // saddle: corners 0 and 2 inside
                    if center {
                        segs.push((left, bottom));
                        segs.push((top, right));
                    } else {
                        segs.push((left, top));
                        segs.push((right, bottom));
                    }
                }
                10 => {
                    if center {
                        segs.push((left, top));
                        segs.push((right, bottom));
                    } else {
                        segs.push((left, bottom));
                        segs.push((top, right));
                    }
                }
                _ => {}
            }
        }
    }
    // chain segments into loops (every crossing point is shared by exactly two segments)
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, &(a, b)) in segs.iter().enumerate() {
        adj.entry(a).or_default().push(k);
        adj.entry(b).or_default().push(k);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = vec![];
    for s in 0..segs.len() {
        if used[s] {
            continue;
        }
        used[s] = true;
        let start = segs[s].0;
        let mut cur = segs[s].1;
        let mut lp = vec![points[&start]];
        let mut guard = 0;
        while cur != start && guard < segs.len() + 2 {
            guard += 1;
            lp.push(points[&cur]);
            let Some(&next) = adj[&cur].iter().find(|&&k| !used[k]) else { break };
            used[next] = true;
            cur = if segs[next].0 == cur { segs[next].1 } else { segs[next].0 };
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    // tidy: drop specks, straighten pixel noise
    let min_area = 2.0 * f.px * f.px;
    loops.into_iter().map(|l| simplify_loop(&l, 0.12 * f.px)).filter(|l| l.len() >= 3 && area(l).abs() > min_area).collect()
}

fn area(c: &[Vec2]) -> f32 {
    (0..c.len()).map(|i| c[i].perp_dot(c[(i + 1) % c.len()])).sum::<f32>() * 0.5
}

fn dp(pts: &[Vec2], tol: f32, out: &mut Vec<Vec2>) {
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let d = b - a;
    let len = d.length().max(1e-12);
    let (mut best, mut bi) = (0.0, 0);
    for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let dist = (*p - a).perp_dot(d).abs() / len;
        if dist > best {
            best = dist;
            bi = i;
        }
    }
    if best > tol {
        dp(&pts[..=bi], tol, out);
        dp(&pts[bi..], tol, out);
    } else {
        out.push(a);
    }
}

/// Douglas–Peucker on a closed loop.
fn simplify_loop(l: &[Vec2], tol: f32) -> Vec<Vec2> {
    let far = (0..l.len()).max_by(|&i, &j| l[i].distance_squared(l[0]).total_cmp(&l[j].distance_squared(l[0]))).unwrap_or(0);
    if far == 0 {
        return l.to_vec();
    }
    let mut out = vec![];
    dp(&l[..=far], tol, &mut out);
    let mut second: Vec<Vec2> = l[far..].to_vec();
    second.push(l[0]);
    dp(&second, tol, &mut out);
    out
}

// ---------- height fields ----------

/// Closed solid: one column per pixel in the mask, top at `heights` (mm), bottom at z = 0.
fn height_field(f: &Field, heights: &[f32]) -> Mesh {
    let (w, h) = (f.w, f.h);
    let mut mask = f.mask.clone();
    // pixels touching only diagonally would share a lone vertex (non-manifold): fill one side
    for _ in 0..8 {
        let mut changed = false;
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let (a, b, c, d) = (mask[y * w + x], mask[y * w + x + 1], mask[(y + 1) * w + x], mask[(y + 1) * w + x + 1]);
                if a && d && !b && !c {
                    mask[y * w + x + 1] = true;
                    changed = true;
                } else if b && c && !a && !d {
                    mask[y * w + x] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let inside = |x: isize, y: isize| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    let hz = |x: usize, y: usize| heights[y * w + x].max(0.05);
    // height of a pixel corner = mean of the existing pixels around it
    let corner = |cx: usize, cy: usize| -> f32 {
        let (mut s, mut n) = (0.0, 0.0);
        for (dx, dy) in [(-1isize, -1isize), (0, -1), (-1, 0), (0, 0)] {
            let (x, y) = (cx as isize + dx, cy as isize + dy);
            if inside(x, y) {
                s += hz(x as usize, y as usize);
                n += 1.0;
            }
        }
        if n > 0.0 {
            s / n
        } else {
            0.05
        }
    };
    let s = f.px;
    let top = |cx: usize, cy: usize| Vec3::new(cx as f32 * s, (h - cy) as f32 * s, corner(cx, cy));
    let bot = |cx: usize, cy: usize| Vec3::new(cx as f32 * s, (h - cy) as f32 * s, 0.0);
    let mut soup: Vec<[Vec3; 3]> = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            if !mask[y * w + x] {
                continue;
            }
            // corners: bl, br, tr, tl (counter-clockwise seen from above)
            let c = [(x, y + 1), (x + 1, y + 1), (x + 1, y), (x, y)];
            let t = c.map(|(a, b)| top(a, b));
            let b = c.map(|(a, b)| bot(a, b));
            soup.push([t[0], t[1], t[2]]);
            soup.push([t[0], t[2], t[3]]);
            soup.push([b[0], b[2], b[1]]);
            soup.push([b[0], b[3], b[2]]);
            // walls where the neighbour is missing; edge p→q has the pixel on its left
            let (xi, yi) = (x as isize, y as isize);
            for (k, (nx, ny)) in [(xi, yi + 1), (xi + 1, yi), (xi, yi - 1), (xi - 1, yi)].into_iter().enumerate() {
                if !inside(nx, ny) {
                    let (p, q) = (k, (k + 1) % 4);
                    soup.push([b[p], b[q], t[q]]);
                    soup.push([b[p], t[q], t[p]]);
                }
            }
        }
    }
    Mesh::from_soup(&soup)
}

fn plate(x0: f32, y0: f32, x1: f32, y1: f32, z0: f32, z1: f32) -> Mesh {
    let mut m = crate::primitives::cuboid(Vec3::new(x1 - x0, y1 - y0, z1 - z0));
    m.translate(Vec3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, (z0 + z1) * 0.5));
    m
}

/// Build the 3D model. Result lies on the plate (z ≥ 0), centred on the origin in X/Y,
/// reading correctly from the top view.
pub fn image_mesh(img: &Gray, o: &ImageOptions, progress: &Progress) -> Result<Mesh> {
    let f = prepare(img, o);
    progress.check()?;
    let (wmm, hmm) = (f.w as f32 * f.px, f.h as f32 * f.px);
    let mut mesh = match o.mode {
        ImageMode::HeightMap | ImageMode::Edge => {
            let hs: Vec<f32> = f.v.iter().map(|v| o.base + v * o.height).collect();
            height_field(&f, &hs)
        }
        ImageMode::Lithophane => {
            let fr = (o.frame / f.px).round() as usize;
            let hs: Vec<f32> = (0..f.w * f.h)
                .map(|i| {
                    let (x, y) = (i % f.w, i / f.w);
                    let border = x < fr || y < fr || x + fr >= f.w || y + fr >= f.h;
                    if border {
                        o.base + o.height
                    } else {
                        o.base + (1.0 - f.v[i]) * o.height
                    }
                })
                .collect();
            let mut f2 = Field { w: f.w, h: f.h, v: vec![], mask: vec![true; f.w * f.h], px: f.px };
            if o.use_alpha {
                f2.mask = f.mask.clone();
            }
            height_field(&f2, &hs)
        }
        ImageMode::Contour | ImageMode::Stamp => {
            let mut loops = contours(&f, o.threshold);
            if loops.is_empty() {
                return Err(Error::Parse("nothing is darker than the threshold — move the slider or turn on Invert".into()));
            }
            if o.mode == ImageMode::Stamp {
                for l in loops.iter_mut() {
                    for p in l.iter_mut() {
                        p.x = wmm - p.x; // a stamp prints mirrored
                    }
                }
            }
            progress.set(0.3);
            let mut m = extrude_contours(loops, o.height.max(0.05))?;
            if o.base > 0.0 {
                m.translate(Vec3::new(0.0, 0.0, o.base));
                let b = plate(0.0, 0.0, wmm, hmm, 0.0, o.base);
                m = crate::csg::boolean(&b, &m, crate::csg::BoolOp::Union)?;
            }
            m
        }
    };
    progress.check()?;
    if mesh.is_empty() {
        return Err(Error::Parse("the picture produced no geometry".into()));
    }
    // height fields have one triangle pair per pixel: merge flat areas
    if o.reduce && matches!(o.mode, ImageMode::HeightMap | ImageMode::Edge | ImageMode::Lithophane) {
        progress.set(0.4);
        let tol = (o.height as f64 * 0.004).clamp(0.002, 0.02);
        simplify::simplify_with(&mut mesh, SimplifyOptions { target_triangles: 0, max_error: tol }, &progress.range(0.4, 1.0))?;
    }
    // the requested width is the width of the result (margins and transparent areas
    // around the shape don't count)
    let b = mesh.bounds();
    let k = if b.size().x > 1e-6 { o.width.max(1.0) / b.size().x } else { 1.0 };
    mesh.translate(Vec3::new(-b.center().x, -b.center().y, -b.min.z));
    if (k - 1.0).abs() > 1e-6 {
        mesh.transform(&glam::Mat4::from_scale(Vec3::new(k, k, 1.0)));
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    /// 100×60 picture: white background, black ring, dark grey square with a white hole.
    fn picture() -> Gray {
        let (w, h) = (100, 60);
        let mut v = vec![1.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 - 30.0, y as f32 - 30.0);
                let r = (fx * fx + fy * fy).sqrt();
                if (12.0..20.0).contains(&r) {
                    v[y * w + x] = 0.0;
                }
                if (60..90).contains(&x) && (15..45).contains(&y) && !((70..80).contains(&x) && (25..35).contains(&y)) {
                    v[y * w + x] = 0.2;
                }
            }
        }
        Gray { w, h, a: vec![1.0; w * h], v }
    }

    #[test]
    fn contour_mode() {
        let o = ImageOptions { mode: ImageMode::Contour, width: 100.0, smooth: 0, ..Default::default() };
        let m = image_mesh(&picture(), &o, &Progress::new()).unwrap();
        let r = analyze(&m);
        assert!(r.is_watertight(), "{:?}", r.problems());
        assert_eq!(m.connected_components().len(), 2, "ring + square-with-hole");
        assert!((m.bounds().size().x - 100.0).abs() < 1e-3);
        // compare in picture units: the shapes span x 10..90 (80 px) of the 100 px picture
        let k = 80.0 / 100.0;
        let m = m.transformed(&glam::Mat4::from_scale(Vec3::new(k, k, 1.0)));
        let b = m.bounds();
        assert!((b.size().z - 3.0).abs() < 1e-4);
        // ring area π(20²−12²) + square 30² − 10² (1 px = 1 mm), times 3 mm
        let expect = (std::f64::consts::PI * (400.0 - 144.0) + 800.0) * 3.0;
        assert!((m.volume() - expect).abs() / expect < 0.05, "{} vs {expect}", m.volume());
    }

    #[test]
    fn stamp_has_base_and_is_mirrored() {
        let o = ImageOptions { mode: ImageMode::Stamp, width: 100.0, smooth: 0, height: 2.0, base: 3.0, ..Default::default() };
        let m = image_mesh(&picture(), &o, &Progress::new()).unwrap();
        assert!(analyze(&m).is_watertight());
        assert_eq!(m.connected_components().len(), 1);
        assert!((m.bounds().size().z - 5.0).abs() < 1e-3);
        // the ring (left in the picture) is on the right in the stamp: the raised part
        // above x > 0 should hold the ring's volume
        let right_high = m.positions.iter().filter(|p| p.z > 4.0 && p.x > 0.0).count();
        let left_high = m.positions.iter().filter(|p| p.z > 4.0 && p.x < 0.0).count();
        assert!(right_high > left_high, "{right_high} {left_high}");
    }

    #[test]
    fn height_map_and_lithophane() {
        for mode in [ImageMode::HeightMap, ImageMode::Lithophane, ImageMode::Edge] {
            let (height, base) = ImageOptions::for_mode(mode);
            let o = ImageOptions { mode, width: 100.0, height, base, smooth: 1, reduce: true, ..Default::default() };
            let m = image_mesh(&picture(), &o, &Progress::new()).unwrap();
            let r = analyze(&m);
            assert!(r.is_watertight(), "{mode:?}: {:?}", r.problems());
            let s = m.bounds().size();
            assert!((s.x - 100.0).abs() < 1e-3 && (s.y - 60.0).abs() < 1e-3, "{mode:?} {s}");
            assert!(s.z <= base + height + 1e-3 && s.z > base, "{mode:?} {s}");
            // flat areas are merged: far fewer triangles than 4 per pixel
            assert!(m.triangles.len() < 100 * 60 * 2, "{mode:?} {}", m.triangles.len());
        }
    }

    #[test]
    fn transparency_cuts_the_outline() {
        let (w, h) = (40, 40);
        let mut a = vec![0.0; w * h];
        for y in 10..30 {
            for x in 10..30 {
                a[y * w + x] = 1.0;
            }
        }
        let g = Gray { w, h, v: vec![0.5; w * h], a };
        let o = ImageOptions { mode: ImageMode::HeightMap, width: 40.0, ..Default::default() };
        let m = image_mesh(&g, &o, &Progress::new()).unwrap();
        assert!(analyze(&m).is_watertight());
        let s = m.bounds().size();
        // the visible square is scaled to the requested width
        assert!((s.x - 40.0).abs() < 1e-3 && (s.y - 40.0).abs() < 1e-3, "{s}");
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    #[test]
    #[ignore]
    fn photo_sized() {
        let (w, h) = (1600, 1200);
        let v: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                (0.5 + 0.5 * (x * 0.02).sin() * (y * 0.015).cos()).clamp(0.0, 1.0)
            })
            .collect();
        let g = Gray { w, h, a: vec![1.0; w * h], v };
        for (mode, res) in [(ImageMode::HeightMap, 250), (ImageMode::Lithophane, 500), (ImageMode::Contour, 500)] {
            let (height, base) = ImageOptions::for_mode(mode);
            let o = ImageOptions { mode, resolution: res, height, base, width: 100.0, reduce: false, ..Default::default() };
            let t = std::time::Instant::now();
            let m = image_mesh(&g, &o, &Progress::new()).unwrap();
            eprintln!(
                "{mode:?} res {res}: {} tris, {:?}, watertight {}",
                m.triangles.len(),
                t.elapsed(),
                crate::analysis::analyze(&m).is_watertight()
            );
        }
    }
}
