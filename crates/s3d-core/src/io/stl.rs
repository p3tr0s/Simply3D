use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::Vec3;

pub fn read(bytes: &[u8]) -> Result<Mesh> {
    if is_binary(bytes) {
        read_binary(bytes)
    } else {
        read_ascii(bytes)
    }
}

fn is_binary(b: &[u8]) -> bool {
    if b.len() >= 84 {
        let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
        if 84 + n * 50 == b.len() {
            return true;
        }
    }
    !b.trim_ascii_start().starts_with(b"solid")
}

fn read_binary(b: &[u8]) -> Result<Mesh> {
    if b.len() < 84 {
        return Err(Error::Parse("STL too short".into()));
    }
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    let n = n.min((b.len() - 84) / 50);
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut soup = Vec::with_capacity(n);
    for i in 0..n {
        let o = 84 + i * 50 + 12;
        let v = |k: usize| Vec3::new(f(o + k * 12), f(o + k * 12 + 4), f(o + k * 12 + 8));
        soup.push([v(0), v(1), v(2)]);
    }
    Ok(Mesh::from_soup(&soup))
}

fn read_ascii(b: &[u8]) -> Result<Mesh> {
    let s = String::from_utf8_lossy(b);
    let mut verts = Vec::new();
    for line in s.lines() {
        let mut it = line.split_ascii_whitespace();
        if it.next() == Some("vertex") {
            let mut c = [0f32; 3];
            for v in &mut c {
                *v = it.next().and_then(|x| x.parse().ok()).ok_or_else(|| Error::Parse(format!("bad vertex: {line}")))?;
            }
            verts.push(Vec3::from(c));
        }
    }
    let soup: Vec<[Vec3; 3]> = verts.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    Ok(Mesh::from_soup(&soup))
}

pub fn write_binary(m: &Mesh, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + m.triangles.len() * 50);
    let mut header = [0u8; 80];
    let h = format!("Simply3D {name}");
    let hb = h.as_bytes();
    header[..hb.len().min(80)].copy_from_slice(&hb[..hb.len().min(80)]);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    for i in 0..m.triangles.len() {
        let n = m.face_normal(i);
        for v in [n].iter().chain(m.tri(i).iter()) {
            for c in v.to_array() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_stl() {
        let src = b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let m = read(src).unwrap();
        assert_eq!(m.triangles.len(), 1);
        assert_eq!(m.positions.len(), 3);
    }

    #[test]
    fn binary_stl_with_solid_header() {
        // Some exporters write "solid" at the start of binary headers; size check must win.
        let m = crate::primitives::cuboid(Vec3::ONE);
        let mut b = write_binary(&m, "x");
        b[..5].copy_from_slice(b"solid");
        assert_eq!(read(&b).unwrap().triangles.len(), 12);
    }
}
