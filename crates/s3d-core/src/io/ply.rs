use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::Vec3;

#[derive(Clone, Copy, PartialEq)]
enum Enc {
    Ascii,
    Le,
    Be,
}

#[derive(Clone, Copy, Debug)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
        })
    }
    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }
}

struct Prop {
    name: String,
    ty: Ty,
    list_count: Option<Ty>,
}

struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
    enc: Enc,
    tokens: std::str::SplitAsciiWhitespace<'a>,
}

impl<'a> Cursor<'a> {
    fn read(&mut self, ty: Ty) -> Result<f64> {
        if self.enc == Enc::Ascii {
            return self.tokens.next().and_then(|t| t.parse::<f64>().ok()).ok_or_else(|| Error::Parse("PLY: truncated ascii data".into()));
        }
        let n = ty.size();
        let s = self.b.get(self.pos..self.pos + n).ok_or_else(|| Error::Parse("PLY: truncated binary data".into()))?;
        self.pos += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(s);
        if self.enc == Enc::Be {
            a[..n].reverse();
        }
        Ok(match ty {
            Ty::I8 => a[0] as i8 as f64,
            Ty::U8 => a[0] as f64,
            Ty::I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
            Ty::I32 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::U32 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F32 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Ty::F64 => f64::from_le_bytes(a),
        })
    }
}

pub fn read(b: &[u8]) -> Result<Mesh> {
    let end = b.windows(10).position(|w| w == b"end_header").ok_or_else(|| Error::Parse("PLY: no end_header".into()))?;
    let header = std::str::from_utf8(&b[..end]).map_err(|_| Error::Parse("PLY: bad header".into()))?;
    let mut body = end + 10;
    while body < b.len() && (b[body] == b'\r' || b[body] == b'\n') {
        body += 1;
        if b[body - 1] == b'\n' {
            break;
        }
    }
    let mut enc = Enc::Ascii;
    let mut elems: Vec<Element> = vec![];
    for line in header.lines() {
        let t: Vec<&str> = line.split_ascii_whitespace().collect();
        match t.as_slice() {
            ["format", f, ..] => {
                enc = match *f {
                    "ascii" => Enc::Ascii,
                    "binary_little_endian" => Enc::Le,
                    "binary_big_endian" => Enc::Be,
                    _ => return Err(Error::Parse(format!("PLY: unknown format {f}"))),
                }
            }
            ["element", name, count] => elems.push(Element { name: name.to_string(), count: count.parse().unwrap_or(0), props: vec![] }),
            ["property", "list", ct, it, name] => {
                if let Some(e) = elems.last_mut() {
                    e.props.push(Prop {
                        name: name.to_string(),
                        ty: Ty::parse(it).ok_or_else(|| Error::Parse("PLY type".into()))?,
                        list_count: Ty::parse(ct),
                    });
                }
            }
            ["property", ty, name] => {
                if let Some(e) = elems.last_mut() {
                    e.props.push(Prop {
                        name: name.to_string(),
                        ty: Ty::parse(ty).ok_or_else(|| Error::Parse("PLY type".into()))?,
                        list_count: None,
                    });
                }
            }
            _ => {}
        }
    }
    let ascii_body = if enc == Enc::Ascii { std::str::from_utf8(&b[body..]).unwrap_or("") } else { "" };
    let mut cur = Cursor { b, pos: body, enc, tokens: ascii_body.split_ascii_whitespace() };
    let mut m = Mesh::default();
    for e in &elems {
        for _ in 0..e.count {
            let mut xyz = [0f32; 3];
            for p in &e.props {
                if let Some(ct) = p.list_count {
                    let n = cur.read(ct)? as usize;
                    let mut idx = Vec::with_capacity(n);
                    for _ in 0..n {
                        idx.push(cur.read(p.ty)? as u32);
                    }
                    if e.name == "face" && (p.name == "vertex_indices" || p.name == "vertex_index") {
                        for k in 1..n.saturating_sub(1) {
                            m.triangles.push([idx[0], idx[k], idx[k + 1]]);
                        }
                    }
                } else {
                    let v = cur.read(p.ty)?;
                    if e.name == "vertex" {
                        match p.name.as_str() {
                            "x" => xyz[0] = v as f32,
                            "y" => xyz[1] = v as f32,
                            "z" => xyz[2] = v as f32,
                            _ => {}
                        }
                    }
                }
            }
            if e.name == "vertex" {
                m.positions.push(Vec3::from(xyz));
            }
        }
    }
    let nv = m.positions.len() as u32;
    if m.triangles.iter().flatten().any(|&i| i >= nv) {
        return Err(Error::Parse("PLY: face index out of range".into()));
    }
    Ok(m)
}

pub fn write_binary(m: &Mesh) -> Vec<u8> {
    let header = format!(
        "ply\nformat binary_little_endian 1.0\ncomment Simply3D export, units: mm\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face {}\nproperty list uchar uint vertex_indices\nend_header\n",
        m.positions.len(),
        m.triangles.len()
    );
    let mut out = header.into_bytes();
    for p in &m.positions {
        for c in p.to_array() {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    for t in &m.triangles {
        out.push(3);
        for i in t {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_ply_with_extra_props() {
        let src = b"ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n1 1 0 0 0 0\n0 1 0 1 1 1\n4 0 1 2 3\n";
        let m = read(src).unwrap();
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.triangles.len(), 2);
    }
}
