//! 3MF (core spec + materials colours + production-extension split model files,
//! as written by Bambu Studio / PrusaSlicer / Orca).

use super::{parse_hex_color, IoObject};
use crate::mesh::Mesh;
use crate::{Error, Result};
use glam::{Mat4, Vec3, Vec4};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Cursor, Read, Write};

#[derive(Default)]
struct Obj3mf {
    name: Option<String>,
    mesh: Option<Mesh>,
    prop: Option<(u32, u32)>,
    components: Vec<(Option<String>, u32, Mat4)>,
}

#[derive(Default)]
struct ModelFile {
    unit_scale: f32,
    objects: HashMap<u32, Obj3mf>,
    build: Vec<(Option<String>, u32, Mat4)>,
    colors: HashMap<u32, Vec<[u8; 4]>>,
}

fn norm_path(p: &str) -> String {
    p.trim_start_matches('/').to_string()
}

fn attrs(e: &BytesStart) -> HashMap<String, String> {
    e.attributes()
        .flatten()
        .map(|a| {
            let k = a.key.local_name().as_ref().to_string();
            let v = a.normalized_value(XmlVersion::Implicit1_0).map(|c| c.into_owned()).unwrap_or_default();
            (k, v)
        })
        .collect()
}

fn parse_transform(s: Option<&String>) -> Mat4 {
    let Some(s) = s else { return Mat4::IDENTITY };
    let v: Vec<f32> = s.split_ascii_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() != 12 {
        return Mat4::IDENTITY;
    }
    Mat4::from_cols(
        Vec4::new(v[0], v[1], v[2], 0.0),
        Vec4::new(v[3], v[4], v[5], 0.0),
        Vec4::new(v[6], v[7], v[8], 0.0),
        Vec4::new(v[9], v[10], v[11], 1.0),
    )
}

fn unit_scale(u: &str) -> f32 {
    match u {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    }
}

fn parse_model(xml: &[u8]) -> Result<ModelFile> {
    let mut r = Reader::from_reader(xml);
    r.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut mf = ModelFile { unit_scale: 1.0, ..Default::default() };
    let mut cur_obj: Option<(u32, Obj3mf)> = None;
    let mut cur_group: Option<u32> = None;
    loop {
        let ev = r.read_event_into(&mut buf).map_err(|e| Error::Parse(format!("3MF xml: {e}")))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = e.local_name();
                let a = attrs(e);
                let num = |k: &str| a.get(k).and_then(|v| v.parse::<f32>().ok());
                let id = |k: &str| a.get(k).and_then(|v| v.parse::<u32>().ok());
                match name.as_ref() {
                    "model" => mf.unit_scale = a.get("unit").map(|u| unit_scale(u)).unwrap_or(1.0),
                    "basematerials" | "colorgroup" => {
                        cur_group = id("id");
                        if let Some(g) = cur_group {
                            mf.colors.entry(g).or_default();
                        }
                    }
                    "base" | "color" => {
                        let c = a
                            .get("displaycolor")
                            .or_else(|| a.get("color"))
                            .and_then(|c| parse_hex_color(c))
                            .unwrap_or([200, 200, 200, 255]);
                        if let Some(g) = cur_group {
                            mf.colors.entry(g).or_default().push(c);
                        }
                    }
                    "object" => {
                        let mut o = Obj3mf { name: a.get("name").cloned(), ..Default::default() };
                        if let (Some(p), i) = (id("pid"), id("pindex")) {
                            o.prop = Some((p, i.unwrap_or(0)));
                        }
                        cur_obj = Some((id("id").unwrap_or(0), o));
                    }
                    "mesh" => {
                        if let Some((_, o)) = &mut cur_obj {
                            o.mesh = Some(Mesh::default());
                        }
                    }
                    "vertex" => {
                        if let Some((_, Obj3mf { mesh: Some(m), .. })) = &mut cur_obj {
                            m.positions.push(Vec3::new(num("x").unwrap_or(0.0), num("y").unwrap_or(0.0), num("z").unwrap_or(0.0)));
                        }
                    }
                    "triangle" => {
                        if let Some((_, o)) = &mut cur_obj {
                            if o.prop.is_none() {
                                if let Some(p) = id("pid") {
                                    o.prop = Some((p, id("p1").unwrap_or(0)));
                                }
                            }
                            if let (Some(m), Some(v1), Some(v2), Some(v3)) = (&mut o.mesh, id("v1"), id("v2"), id("v3")) {
                                m.triangles.push([v1, v2, v3]);
                            }
                        }
                    }
                    "component" => {
                        if let Some((_, o)) = &mut cur_obj {
                            if let Some(oid) = id("objectid") {
                                o.components.push((a.get("path").map(|p| norm_path(p)), oid, parse_transform(a.get("transform"))));
                            }
                        }
                    }
                    "item" => {
                        if let Some(oid) = id("objectid") {
                            mf.build.push((a.get("path").map(|p| norm_path(p)), oid, parse_transform(a.get("transform"))));
                        }
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => match e.local_name().as_ref() {
                "object" => {
                    if let Some((id, o)) = cur_obj.take() {
                        mf.objects.insert(id, o);
                    }
                }
                "basematerials" | "colorgroup" => cur_group = None,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(mf)
}

struct Package {
    files: HashMap<String, ModelFile>,
}

impl Package {
    fn flatten(&self, path: &str, id: u32, xf: Mat4, depth: u32, out: &mut Mesh, color: &mut Option<[u8; 4]>) {
        if depth > 16 {
            return;
        }
        let Some(mf) = self.files.get(path) else { return };
        let Some(o) = mf.objects.get(&id) else { return };
        if color.is_none() {
            if let Some((g, i)) = o.prop {
                *color = mf.colors.get(&g).and_then(|c| c.get(i as usize)).copied();
            }
        }
        let xf_units = xf * Mat4::from_scale(Vec3::splat(mf.unit_scale));
        if let Some(m) = &o.mesh {
            let n = m.positions.len() as u32;
            let mut part = m.clone();
            part.triangles.retain(|t| t.iter().all(|&i| i < n));
            part.transform(&xf_units);
            out.append(&part);
        }
        for (p, cid, cxf) in &o.components {
            let p = p.as_deref().unwrap_or(path);
            self.flatten(p, *cid, xf * *cxf, depth + 1, out, color);
        }
    }
}

pub fn read(bytes: &[u8]) -> Result<Vec<IoObject>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let read_entry = |zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    };
    // main model from relationships
    let mut main = "3D/3dmodel.model".to_string();
    if let Some(rels) = read_entry(&mut zip, "_rels/.rels") {
        let s = String::from_utf8_lossy(&rels);
        let mut r = Reader::from_str(&s);
        while let Ok(ev) = r.read_event() {
            match ev {
                Event::Empty(ref e) | Event::Start(ref e) if e.local_name().as_ref() == "Relationship" => {
                    let a = attrs(e);
                    if a.get("Type").is_some_and(|t| t.ends_with("3dmodel")) {
                        if let Some(t) = a.get("Target") {
                            main = norm_path(t);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    let names: Vec<String> =
        zip.file_names().flatten().map(|c| c.into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".model")).collect();
    let mut files = HashMap::new();
    for n in names {
        if let Some(b) = read_entry(&mut zip, &n) {
            files.insert(norm_path(&n), parse_model(&b)?);
        }
    }
    let pkg = Package { files };
    let mf = pkg.files.get(&main).ok_or_else(|| Error::Parse(format!("3MF: missing {main}")))?;
    let mut out = Vec::new();
    for (i, (path, oid, xf)) in mf.build.iter().enumerate() {
        let path = path.clone().unwrap_or_else(|| main.clone());
        let mut mesh = Mesh::default();
        let mut color = None;
        pkg.flatten(&path, *oid, *xf, 0, &mut mesh, &mut color);
        let name = pkg
            .files
            .get(&path)
            .and_then(|f| f.objects.get(oid))
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| format!("Object {}", i + 1));
        out.push(IoObject { name, mesh, color });
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn write(objects: &[IoObject]) -> Result<Vec<u8>> {
    let mut model = String::with_capacity(1 << 20);
    model.push_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<metadata name="Application">Builder3D</metadata>
<resources>
<basematerials id="1">
"#,
    );
    for o in objects {
        let c = o.color.unwrap_or([200, 200, 200, 255]);
        let _ = writeln!(model, r##"<base name="{}" displaycolor="#{:02X}{:02X}{:02X}{:02X}"/>"##, esc(&o.name), c[0], c[1], c[2], c[3]);
    }
    model.push_str("</basematerials>\n");
    for (i, o) in objects.iter().enumerate() {
        let _ = writeln!(model, r#"<object id="{}" type="model" name="{}" pid="1" pindex="{}"><mesh><vertices>"#, i + 2, esc(&o.name), i);
        for p in &o.mesh.positions {
            let _ = writeln!(model, r#"<vertex x="{}" y="{}" z="{}"/>"#, p.x, p.y, p.z);
        }
        model.push_str("</vertices><triangles>\n");
        for t in &o.mesh.triangles {
            let _ = writeln!(model, r#"<triangle v1="{}" v2="{}" v3="{}"/>"#, t[0], t[1], t[2]);
        }
        model.push_str("</triangles></mesh></object>\n");
    }
    model.push_str("</resources>\n<build>\n");
    for i in 0..objects.len() {
        let _ = writeln!(model, r#"<item objectid="{}"/>"#, i + 2);
    }
    model.push_str("</build>\n</model>\n");

    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("[Content_Types].xml", opt)?;
    zw.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#)?;
    zw.start_file("_rels/.rels", opt)?;
    zw.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#)?;
    zw.start_file("3D/3dmodel.model", opt)?;
    zw.write_all(model.as_bytes())?;
    Ok(zw.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production-extension layout: build item -> component in a separate model file, with transforms.
    #[test]
    fn split_model_files_with_transforms() {
        let cube = crate::primitives::cuboid(Vec3::splat(2.0));
        let mut verts = String::new();
        for p in &cube.positions {
            let _ = write!(verts, r#"<vertex x="{}" y="{}" z="{}"/>"#, p.x, p.y, p.z);
        }
        let mut tris = String::new();
        for t in &cube.triangles {
            let _ = write!(tris, r#"<triangle v1="{}" v2="{}" v3="{}"/>"#, t[0], t[1], t[2]);
        }
        let part = format!(
            r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model"><mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh></object></resources><build/></model>"#
        );
        let main = r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"><resources><object id="5" name="Part A" type="model"><components><component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 10 0 0"/></components></object></resources><build><item objectid="5" transform="1 0 0 0 1 0 0 0 1 100 100 1"/></build></model>"#;
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opt = zip::write::SimpleFileOptions::default();
        zw.start_file("3D/3dmodel.model", opt).unwrap();
        zw.write_all(main.as_bytes()).unwrap();
        zw.start_file("3D/Objects/object_1.model", opt).unwrap();
        zw.write_all(part.as_bytes()).unwrap();
        let bytes = zw.finish().unwrap().into_inner();
        let objs = read(&bytes).unwrap();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].name, "Part A");
        let c = objs[0].mesh.bounds().center();
        assert!((c - Vec3::new(110.0, 100.0, 1.0)).length() < 1e-4, "{c}");
    }
}
