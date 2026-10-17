//! A small picture of the plate (software-rendered, so it works while saving in the
//! background): stored in 3MF files so slicers and the file manager can show a preview.

use crate::io::IoObject;
use crate::mesh::Mesh;
use glam::{Vec2, Vec3};
use image::{imageops, Rgba, RgbaImage};

/// Render the solids (holes left out) from the front-left, above, on a transparent
/// background. `size` is the width and height in pixels.
pub fn render(objects: &[IoObject], size: u32) -> RgbaImage {
    let ss = 2; // supersampling
    let n = size * ss;
    let meshes: Vec<(Mesh, [u8; 4])> = objects
        .iter()
        .filter(|o| !o.hole && !o.hidden && !o.mesh.is_empty())
        .map(|o| {
            let m = match o.transform {
                Some(t) => o.mesh.transformed(&t),
                None => o.mesh.clone(),
            };
            (m, o.color.unwrap_or([150, 160, 175, 255]))
        })
        .collect();
    let eye = Vec3::new(-0.62, -1.0, 0.85).normalize();
    let fwd = -eye;
    let right = fwd.cross(Vec3::Z).normalize();
    let up = right.cross(fwd);
    let light = (eye + right * 0.45 + up * 0.6).normalize();
    let project = |p: Vec3| Vec2::new(p.dot(right), p.dot(up));
    // fit everything into the picture with a margin
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for (m, _) in &meshes {
        for p in &m.positions {
            let q = project(*p);
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    let mut img = RgbaImage::new(n, n);
    if meshes.is_empty() || !lo.is_finite() {
        return imageops::resize(&img, size, size, imageops::FilterType::Triangle);
    }
    let span = (hi - lo).max_element().max(1e-6);
    let scale = n as f32 * 0.88 / span;
    let centre = (lo + hi) * 0.5;
    let to_px = |p: Vec3| -> Vec3 {
        let q = (project(p) - centre) * scale;
        Vec3::new(n as f32 * 0.5 + q.x, n as f32 * 0.5 - q.y, p.dot(fwd))
    };
    let mut depth = vec![f32::INFINITY; (n * n) as usize];
    for (m, color) in &meshes {
        let pts: Vec<Vec3> = m.positions.iter().map(|p| to_px(*p)).collect();
        for (i, t) in m.triangles.iter().enumerate() {
            let [a, b, c] = t.map(|v| pts[v as usize]);
            let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if area.abs() < 1e-9 {
                continue;
            }
            let normal = m.face_normal(i);
            // seen from the back (open or inverted meshes): light the side facing us
            let nv = if normal.dot(eye) < 0.0 { -normal } else { normal };
            let paint = m.paint_of(i);
            let base = if paint != 0 { crate::paint::unpack(paint) } else { *color };
            let k = 0.38 + 0.62 * nv.dot(light).max(0.0) + 0.12 * nv.z.max(0.0);
            let shade = |c: u8| ((c as f32 * k).round()).clamp(0.0, 255.0) as u8;
            let px = Rgba([shade(base[0]), shade(base[1]), shade(base[2]), 255]);
            let x0 = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
            let x1 = (a.x.max(b.x).max(c.x).ceil() as i64).clamp(0, n as i64 - 1) as u32;
            let y0 = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
            let y1 = (a.y.max(b.y).max(c.y).ceil() as i64).clamp(0, n as i64 - 1) as u32;
            if x0 > x1 || y0 > y1 {
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px_x, px_y) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b.x - px_x) * (c.y - px_y) - (b.y - px_y) * (c.x - px_x)) / area;
                    let w1 = ((c.x - px_x) * (a.y - px_y) - (c.y - px_y) * (a.x - px_x)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a.z + w1 * b.z + w2 * c.z;
                    let d = &mut depth[(y * n + x) as usize];
                    if z < *d {
                        *d = z;
                        img.put_pixel(x, y, px);
                    }
                }
            }
        }
    }
    imageops::resize(&img, size, size, imageops::FilterType::Triangle)
}

/// The picture as PNG bytes.
pub fn png(objects: &[IoObject], size: u32) -> Vec<u8> {
    let img = render(objects, size);
    let mut out = std::io::Cursor::new(Vec::new());
    // encoding an in-memory RGBA image can't fail
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cuboid;

    #[test]
    fn draws_the_model_on_a_clear_background() {
        let o = IoObject::new("box", cuboid(Vec3::splat(20.0)), Some([200, 40, 40, 255]));
        let img = render(&[o], 128);
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.get_pixel(1, 1)[3], 0, "corner is transparent");
        let c = img.get_pixel(64, 64);
        assert_eq!(c[3], 255);
        assert!(c[0] > c[1] && c[0] > c[2], "red box: {c:?}");
        assert!(!png(&[], 64).is_empty());
    }
}
