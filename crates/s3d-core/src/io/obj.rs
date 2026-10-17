use super::IoObject;
use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::Vec3;
use std::collections::HashMap;
use std::fmt::Write;

/// Parse Wavefront OBJ. Each `o` (or `g` if no `o` is used) becomes an object.
/// Faces are fan-triangulated; texture/normal indices are ignored.
pub fn read(bytes: &[u8], default_name: &str) -> Result<Vec<IoObject>> {
    let s = String::from_utf8_lossy(bytes);
    let mut positions: Vec<Vec3> = Vec::new();
    let mut groups: Vec<(String, Vec<[u32; 3]>, Option<String>)> = vec![(default_name.to_string(), vec![], None)];
    let uses_o = s.lines().any(|l| l.trim_start().starts_with("o "));
    let mut material: Option<String> = None;
    for (ln, line) in s.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_ascii_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|x| x.parse().ok()).collect();
                if c.len() != 3 {
                    return Err(Error::Parse(format!("OBJ line {}: bad vertex", ln + 1)));
                }
                positions.push(Vec3::new(c[0], c[1], c[2]));
            }
            Some(tag @ ("o" | "g")) if (tag == "o") == uses_o => {
                let name = it.collect::<Vec<_>>().join(" ");
                let g = groups.last_mut().unwrap();
                if g.1.is_empty() {
                    g.0 = if name.is_empty() { default_name.into() } else { name };
                } else {
                    groups.push((name, vec![], material.clone()));
                }
            }
            Some("usemtl") => {
                material = it.next().map(str::to_string);
                let g = groups.last_mut().unwrap();
                if g.1.is_empty() || g.2.is_none() {
                    g.2 = material.clone();
                }
            }
            Some("f") => {
                let n = positions.len() as i64;
                let idx: Vec<u32> = it
                    .map(|tok| {
                        let i: i64 = tok
                            .split('/')
                            .next()
                            .unwrap_or("")
                            .parse()
                            .map_err(|_| Error::Parse(format!("OBJ line {}: bad face", ln + 1)))?;
                        let i = if i < 0 { n + i } else { i - 1 };
                        if i < 0 || i >= n {
                            return Err(Error::Parse(format!("OBJ line {}: index out of range", ln + 1)));
                        }
                        Ok(i as u32)
                    })
                    .collect::<Result<_>>()?;
                let g = groups.last_mut().unwrap();
                for k in 1..idx.len().saturating_sub(1) {
                    g.1.push([idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok(groups
        .into_iter()
        .filter(|g| !g.1.is_empty())
        .map(|(name, tris, mat)| {
            let mut m = Mesh::new(positions.clone(), tris);
            m.compact();
            // colour encoded in material name by our own exporter: mat_RRGGBBAA
            let color = mat.and_then(|m| m.strip_prefix("mat_").and_then(super::parse_hex_color));
            IoObject::new(name, m, color)
        })
        .collect())
}

/// Returns (obj bytes, optional mtl text).
pub fn write(objects: &[IoObject], mtl_name: &str) -> (Vec<u8>, Option<String>) {
    let mut s = String::from("# Simply3D export, units: mm\n");
    let has_color = objects.iter().any(|o| o.color.is_some());
    let mut mtl = String::new();
    let mut mats: HashMap<[u8; 4], String> = HashMap::new();
    if has_color {
        let _ = writeln!(s, "mtllib {mtl_name}");
    }
    let mut base = 1usize;
    for o in objects {
        let _ = writeln!(s, "o {}", o.name.replace(['\n', '\r'], " "));
        if let Some(c) = o.color {
            let name = mats.entry(c).or_insert_with(|| {
                let n = format!("mat_{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]);
                let _ = writeln!(
                    mtl,
                    "newmtl {n}\nKd {:.4} {:.4} {:.4}\nd {:.4}\n",
                    c[0] as f32 / 255.0,
                    c[1] as f32 / 255.0,
                    c[2] as f32 / 255.0,
                    c[3] as f32 / 255.0
                );
                n
            });
            let _ = writeln!(s, "usemtl {name}");
        }
        for p in &o.mesh.positions {
            let _ = writeln!(s, "v {} {} {}", p.x, p.y, p.z);
        }
        for t in &o.mesh.triangles {
            let _ = writeln!(s, "f {} {} {}", t[0] as usize + base, t[1] as usize + base, t[2] as usize + base);
        }
        base += o.mesh.positions.len();
    }
    (s.into_bytes(), has_color.then_some(mtl))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_and_negative_indices() {
        let src = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4 -3 -2 -1\n";
        let o = read(src, "x").unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].mesh.triangles.len(), 2);
    }
}
