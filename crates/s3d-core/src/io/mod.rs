//! File formats. Import returns one or more objects in world coordinates (mm).

pub mod obj;
pub mod ply;
pub mod stl;
pub mod threemf;

use crate::mesh::Mesh;
use crate::{Error, Result};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct IoObject {
    pub name: String,
    /// World-space mesh, or local-space when `transform` is set.
    pub mesh: Mesh,
    /// sRGB colour, if the file carries one.
    pub color: Option<[u8; 4]>,
    /// Object placement (3MF build item). `None` = mesh is already in world space.
    pub transform: Option<glam::Mat4>,
    /// Group name (Simply3D project metadata).
    pub group: Option<String>,
    pub hidden: bool,
    pub locked: bool,
    /// Written by Simply3D: positions are meaningful as-is.
    pub keep_position: bool,
    /// A negative object: removes material from the solids it overlaps, never printed.
    pub hole: bool,
}

/// How holes are written to 3MF files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// Cut the holes into the solids (works in every slicer) instead of writing them as
    /// negative parts (Bambu Studio / OrcaSlicer).
    pub cut_holes: bool,
    /// Store a picture of the plate (3MF), shown by slicers and the file manager.
    pub thumbnail: bool,
}

/// Bake every hole into the solids it overlaps (world space) and drop the holes.
/// Names, colours, groups and flags of the solids are kept.
pub fn apply_holes(objects: &[IoObject]) -> Result<Vec<IoObject>> {
    let holes: Vec<(Mesh, crate::Aabb)> = objects
        .iter()
        .filter(|o| o.hole)
        .map(|o| {
            let m = o.world_mesh();
            let b = m.bounds();
            (m, b)
        })
        .collect();
    let mut out = vec![];
    for o in objects.iter().filter(|o| !o.hole) {
        let mut mesh = o.world_mesh();
        let b = mesh.bounds();
        let cutters: Vec<Mesh> = holes.iter().filter(|(_, hb)| overlaps(&b, hb)).map(|(m, _)| m.clone()).collect();
        if !cutters.is_empty() {
            let before = mesh;
            mesh = crate::csg::boolean_many(&before, &cutters, crate::csg::BoolOp::Subtract)?;
            let tol = (mesh.bounds().size().length() * 0.002).clamp(0.05, 0.5);
            crate::paint::transfer(&before, &mut mesh, tol);
        }
        if mesh.is_empty() {
            continue;
        }
        out.push(IoObject { mesh, transform: None, ..o.clone() });
    }
    Ok(out)
}

/// Bounding boxes overlap (with a little tolerance).
pub fn overlaps(a: &crate::Aabb, b: &crate::Aabb) -> bool {
    (0..3).all(|k| a.min[k] < b.max[k] + 1e-4 && b.min[k] < a.max[k] + 1e-4)
}

impl IoObject {
    pub fn new(name: impl Into<String>, mesh: Mesh, color: Option<[u8; 4]>) -> Self {
        IoObject { name: name.into(), mesh, color, ..Default::default() }
    }
    /// Mesh with the placement applied.
    pub fn world_mesh(&self) -> Mesh {
        match self.transform {
            Some(m) => self.mesh.transformed(&m),
            None => self.mesh.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Stl,
    Obj,
    Ply,
    ThreeMf,
}

impl Format {
    pub const IMPORT_EXTS: [&'static str; 4] = ["stl", "obj", "ply", "3mf"];

    pub fn from_path(p: &Path) -> Option<Format> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "stl" => Some(Format::Stl),
            "obj" => Some(Format::Obj),
            "ply" => Some(Format::Ply),
            "3mf" => Some(Format::ThreeMf),
            _ => None,
        }
    }
    pub fn ext(&self) -> &'static str {
        match self {
            Format::Stl => "stl",
            Format::Obj => "obj",
            Format::Ply => "ply",
            Format::ThreeMf => "3mf",
        }
    }
    /// Whether the format keeps several separate objects (and colours).
    pub fn multi_object(&self) -> bool {
        matches!(self, Format::ThreeMf | Format::Obj)
    }
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("Object").to_string()
}

pub fn load(path: &Path) -> Result<Vec<IoObject>> {
    let fmt = Format::from_path(path).ok_or_else(|| Error::Unsupported(path.display().to_string()))?;
    let bytes = std::fs::read(path)?;
    load_bytes(&bytes, fmt, &stem(path))
}

pub fn load_bytes(bytes: &[u8], fmt: Format, name: &str) -> Result<Vec<IoObject>> {
    let objs = match fmt {
        Format::Stl => vec![IoObject::new(name, stl::read(bytes)?, None)],
        Format::Obj => obj::read(bytes, name)?,
        Format::Ply => vec![IoObject::new(name, ply::read(bytes)?, None)],
        Format::ThreeMf => threemf::read(bytes)?,
    };
    let objs: Vec<_> = objs.into_iter().filter(|o| !o.mesh.is_empty()).collect();
    if objs.is_empty() {
        return Err(Error::Parse("file contains no triangles".into()));
    }
    Ok(objs)
}

pub fn save(path: &Path, objects: &[IoObject]) -> Result<()> {
    save_with(path, objects, SaveOptions::default())
}

pub fn save_with(path: &Path, objects: &[IoObject], opts: SaveOptions) -> Result<()> {
    let fmt = Format::from_path(path).ok_or_else(|| Error::Unsupported(path.display().to_string()))?;
    let bytes = save_bytes_with(objects, fmt, path, opts)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Encode. For OBJ a companion `.mtl` is written next to `path` when colours exist.
pub fn save_bytes(objects: &[IoObject], fmt: Format, path: &Path) -> Result<Vec<u8>> {
    save_bytes_with(objects, fmt, path, SaveOptions::default())
}

pub fn save_bytes_with(objects: &[IoObject], fmt: Format, path: &Path, opts: SaveOptions) -> Result<Vec<u8>> {
    if fmt == Format::ThreeMf {
        return threemf::write_with(objects, opts);
    }
    // the other formats have no placements or holes: bake them
    let baked: Vec<IoObject> = apply_holes(objects)?
        .into_iter()
        .map(|o| IoObject { name: o.name.clone(), mesh: o.world_mesh(), color: o.color, ..Default::default() })
        .collect();
    let objects = &baked[..];
    Ok(match fmt {
        Format::Stl => {
            let mut all = Mesh::default();
            for o in objects {
                all.append(&o.mesh);
            }
            stl::write_binary(&all, &stem(path))
        }
        Format::Ply => {
            let mut all = Mesh::default();
            for o in objects {
                all.append(&o.mesh);
            }
            ply::write_binary(&all)
        }
        Format::Obj => {
            let mtl_name = format!("{}.mtl", stem(path));
            let (obj, mtl) = obj::write(objects, &mtl_name);
            if let Some(mtl) = mtl {
                if path.parent().is_some() {
                    let _ = std::fs::write(path.with_extension("mtl"), mtl);
                }
            }
            obj
        }
        Format::ThreeMf => unreachable!(),
    })
}

pub(crate) fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim().trim_start_matches('#');
    let h = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    match s.len() {
        6 => Some([h(0)?, h(2)?, h(4)?, 255]),
        8 => Some([h(0)?, h(2)?, h(4)?, h(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use crate::primitives::{cuboid, sphere};
    use glam::Vec3;

    fn objs() -> Vec<IoObject> {
        let mut b = sphere(5.0, 24, 12);
        b.translate(Vec3::new(20.0, 0.0, 5.0));
        vec![
            IoObject::new("Cube & <friends>", cuboid(Vec3::splat(10.0)), Some([255, 0, 0, 255])),
            IoObject::new("Ball", b, Some([0, 128, 255, 255])),
        ]
    }

    #[test]
    fn roundtrip_all_formats() {
        let dir = std::env::temp_dir().join("b3d_io_test");
        std::fs::create_dir_all(&dir).unwrap();
        for fmt in [Format::Stl, Format::Obj, Format::Ply, Format::ThreeMf] {
            let p = dir.join(format!("t.{}", fmt.ext()));
            save(&p, &objs()).unwrap();
            let back = load(&p).unwrap();
            let total_vol: f64 = back.iter().map(|o| o.mesh.volume()).sum();
            let expect: f64 = objs().iter().map(|o| o.mesh.volume()).sum();
            assert!((total_vol - expect).abs() < 1e-2, "{fmt:?}: {total_vol} vs {expect}");
            for o in &back {
                assert!(analyze(&o.mesh).is_watertight(), "{fmt:?} {}", o.name);
            }
            if fmt.multi_object() {
                assert_eq!(back.len(), 2, "{fmt:?}");
                assert_eq!(back[0].name, "Cube & <friends>", "{fmt:?}");
                assert_eq!(back[1].color, Some([0, 128, 255, 255]), "{fmt:?}");
            }
        }
    }
}
