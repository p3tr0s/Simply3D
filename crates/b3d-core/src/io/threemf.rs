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
    meta: HashMap<String, String>,
    /// Per triangle while parsing: its own material (pid, p1) and painted filament.
    tri_paint: Vec<(Option<(u32, u32)>, u32)>,
}

#[derive(Default)]
struct ModelFile {
    unit_scale: f32,
    objects: HashMap<u32, Obj3mf>,
    build: Vec<(Option<String>, u32, Mat4)>,
    colors: HashMap<u32, Vec<[u8; 4]>>,
    meta: HashMap<String, String>,
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

/// `filaments`: colours of the slicer's filament slots (for painted triangles).
fn parse_model(xml: &[u8], filaments: &[[u8; 4]]) -> Result<ModelFile> {
    let mut r = Reader::from_reader(xml);
    r.config_mut().trim_text(false); // keep spaces inside metadata values
    let mut buf = Vec::new();
    let mut mf = ModelFile { unit_scale: 1.0, ..Default::default() };
    let mut cur_obj: Option<(u32, Obj3mf)> = None;
    let mut cur_group: Option<u32> = None;
    let mut cur_meta: Option<String> = None;
    let mut meta_text = String::new();
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
                                let own = id("pid").map(|p| (p, id("p1").unwrap_or(0)));
                                // Bambu Studio / OrcaSlicer, else PrusaSlicer
                                let state = a
                                    .get("paint_color")
                                    .or_else(|| a.get("mmu_segmentation"))
                                    .map(|v| crate::paint::bambu_decode(v))
                                    .unwrap_or(0);
                                o.tri_paint.push((own, state));
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
                    "metadata" => {
                        let key = a.get("name").cloned().unwrap_or_default();
                        let key = key.rsplit(':').next().unwrap_or("").to_string();
                        if matches!(ev, Event::Start(_)) {
                            cur_meta = Some(key);
                            meta_text.clear();
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
            Event::Text(ref t) if cur_meta.is_some() => meta_text.push_str(t.as_ref()),
            Event::GeneralRef(ref r) if cur_meta.is_some() => {
                let name: &str = r.as_ref();
                match name {
                    "amp" => meta_text.push('&'),
                    "lt" => meta_text.push('<'),
                    "gt" => meta_text.push('>'),
                    "quot" => meta_text.push('"'),
                    "apos" => meta_text.push('\''),
                    n => {
                        // numeric reference: &#65; / &#x41;
                        let code = n
                            .strip_prefix("#x")
                            .map(|h| u32::from_str_radix(h, 16).ok())
                            .unwrap_or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok()));
                        if let Some(c) = code.and_then(char::from_u32) {
                            meta_text.push(c);
                        }
                    }
                }
            }
            Event::End(ref e) => match e.local_name().as_ref() {
                "metadata" => {
                    if let Some(k) = cur_meta.take() {
                        let v = std::mem::take(&mut meta_text).trim().to_string();
                        match &mut cur_obj {
                            Some((_, o)) => o.meta.insert(k, v),
                            None => mf.meta.insert(k, v),
                        };
                    }
                }
                "object" => {
                    if let Some((id, mut o)) = cur_obj.take() {
                        resolve_paint(&mut o, &mf.colors, filaments);
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

/// Turn the per-triangle materials / painted filaments into the mesh's paint.
fn resolve_paint(o: &mut Obj3mf, colors: &HashMap<u32, Vec<[u8; 4]>>, filaments: &[[u8; 4]]) {
    let tp = std::mem::take(&mut o.tri_paint);
    let Some(m) = &mut o.mesh else { return };
    if tp.len() != m.triangles.len() || tp.iter().all(|(own, st)| *st == 0 && (own.is_none() || *own == o.prop)) {
        return;
    }
    let paint: Vec<u32> = tp
        .iter()
        .map(|(own, state)| match own {
            Some(p) if Some(*p) != o.prop => {
                colors.get(&p.0).and_then(|c| c.get(p.1 as usize)).map(|c| crate::paint::pack(*c)).unwrap_or(0)
            }
            _ if *state > 0 => crate::paint::pack(filament(filaments, *state)),
            _ => 0,
        })
        .collect();
    m.paint = paint;
    m.tidy_paint();
}

/// Colour of filament slot `k` (1-based); a default palette when the file has none.
fn filament(filaments: &[[u8; 4]], k: u32) -> [u8; 4] {
    let i = k.max(1) as usize - 1;
    filaments.get(i).copied().unwrap_or(crate::scene::PALETTE[i % crate::scene::PALETTE.len()])
}

/// Filament colours from Bambu / Orca's `project_settings.config` (JSON).
fn parse_filaments(json: &str) -> Vec<[u8; 4]> {
    let Some(i) = json.find("\"filament_colour\"") else { return vec![] };
    let rest = &json[i..];
    let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) else { return vec![] };
    if b < a {
        return vec![];
    }
    rest[a + 1..b].split(',').filter_map(|v| parse_hex_color(v.trim().trim_matches('"'))).collect()
}

/// Extruder numbers from `model_settings.config`: (per object id, per part id).
fn parse_extruders(xml: &str) -> (HashMap<u32, u32>, HashMap<u32, u32>) {
    let (mut objs, mut parts) = (HashMap::new(), HashMap::new());
    let mut r = Reader::from_str(xml);
    let mut cur: Option<(bool, u32)> = None; // (is part, id)
    let mut obj: Option<u32> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let a = attrs(e);
                let id = a.get("id").and_then(|v| v.parse().ok());
                match e.local_name().as_ref() {
                    "object" => {
                        obj = id;
                        cur = id.map(|i| (false, i));
                    }
                    "part" | "volume" => cur = id.map(|i| (true, i)),
                    "metadata" if a.get("key").map(|k| k.as_str()) == Some("extruder") => {
                        if let (Some((part, i)), Some(v)) = (cur, a.get("value").and_then(|v| v.parse().ok())) {
                            if part {
                                parts.insert(i, v)
                            } else {
                                objs.insert(i, v)
                            };
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => match e.local_name().as_ref() {
                "part" | "volume" => cur = obj.map(|i| (false, i)),
                "object" => {
                    obj = None;
                    cur = None;
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (objs, parts)
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
    let filaments =
        read_entry(&mut zip, "Metadata/project_settings.config").map(|b| parse_filaments(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let (obj_ext, part_ext) =
        read_entry(&mut zip, "Metadata/model_settings.config").map(|b| parse_extruders(&String::from_utf8_lossy(&b))).unwrap_or_default();
    // the slicer's filament colour for an object / part, when the file says which
    let ext_color = |part: Option<u32>, obj: u32| -> Option<[u8; 4]> {
        if filaments.is_empty() {
            return None;
        }
        let k = part.and_then(|p| part_ext.get(&p)).or_else(|| obj_ext.get(&obj))?;
        Some(filament(&filaments, *k))
    };
    let names: Vec<String> =
        zip.file_names().flatten().map(|c| c.into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".model")).collect();
    let mut files = HashMap::new();
    for n in names {
        if let Some(b) = read_entry(&mut zip, &n) {
            files.insert(norm_path(&n), parse_model(&b, &filaments)?);
        }
    }
    // our own editable copy (holes kept) wins over the slicer-facing model
    if let Some(b) = read_entry(&mut zip, PROJECT_COPY) {
        files.insert(PROJECT_COPY.to_string(), parse_model(&b, &filaments)?);
        main = PROJECT_COPY.to_string();
    }
    let pkg = Package { files };
    let mf = pkg.files.get(&main).ok_or_else(|| Error::Parse(format!("3MF: missing {main}")))?;
    let ours = mf.meta.get("Application").is_some_and(|a| a.starts_with("Builder3D"));
    let mut out = Vec::new();
    let mut used_groups: Vec<String> = vec![];
    for (i, (path, oid, xf)) in mf.build.iter().enumerate() {
        let path = path.clone().unwrap_or_else(|| main.clone());
        let file = pkg.files.get(&path);
        let obj = file.and_then(|f| f.objects.get(oid));
        let name = obj.and_then(|o| o.name.clone()).unwrap_or_else(|| format!("Object {}", i + 1));
        // an object made of several parts (our groups, multi-part objects from slicers)
        // comes in as a group of separate objects
        if let (Some(f), Some(o)) = (file, obj) {
            if o.mesh.is_none() && o.components.len() >= 2 {
                let mut gname = name.clone();
                let mut k = 2;
                while used_groups.contains(&gname) {
                    gname = format!("{name} ({k})");
                    k += 1;
                }
                used_groups.push(gname.clone());
                for (pi, (cpath, cid, cxf)) in o.components.iter().enumerate() {
                    let cpath = cpath.clone().unwrap_or_else(|| path.clone());
                    let cfile = pkg.files.get(&cpath);
                    let cobj = cfile.and_then(|cf| cf.objects.get(cid));
                    let cmeta = |k: &str| cobj.and_then(|c| c.meta.get(k)).cloned();
                    let mut part = IoObject {
                        name: cobj.and_then(|c| c.name.clone()).unwrap_or_else(|| format!("{name} part {}", pi + 1)),
                        group: Some(gname.clone()),
                        hidden: cmeta("hidden").is_some_and(|v| v == "1"),
                        locked: cmeta("locked").is_some_and(|v| v == "1"),
                        hole: cmeta("hole").is_some_and(|v| v == "1"),
                        keep_position: ours,
                        ..Default::default()
                    };
                    let placement = *xf * Mat4::from_scale(Vec3::splat(f.unit_scale)) * *cxf;
                    match (cfile, cobj) {
                        (Some(cf), Some(c)) if c.components.is_empty() && c.mesh.is_some() => {
                            let mut m = c.mesh.clone().unwrap();
                            let n = m.positions.len() as u32;
                            m.triangles.retain(|t| t.iter().all(|&i| i < n));
                            if let Some((g, gi)) = c.prop {
                                part.color = cf.colors.get(&g).and_then(|cl| cl.get(gi as usize)).copied();
                            }
                            part.mesh = m;
                            let placement = placement * Mat4::from_scale(Vec3::splat(cf.unit_scale / f.unit_scale));
                            part.transform = (!placement.abs_diff_eq(Mat4::IDENTITY, 1e-7)).then_some(placement);
                        }
                        _ => {
                            let mut color = None;
                            pkg.flatten(&cpath, *cid, *xf * *cxf, 0, &mut part.mesh, &mut color);
                            part.color = color;
                        }
                    }
                    if let Some(c) = ext_color(Some(*cid), *oid) {
                        part.color = Some(c);
                    }
                    if part.color.is_none() {
                        if let Some((g, gi)) = o.prop {
                            part.color = f.colors.get(&g).and_then(|cl| cl.get(gi as usize)).copied();
                        }
                    }
                    if !part.mesh.is_empty() {
                        out.push(part);
                    }
                }
                continue;
            }
        }
        let meta = |k: &str| obj.and_then(|o| o.meta.get(k)).cloned();
        let mut io = IoObject {
            name,
            group: meta("group").filter(|g| !g.is_empty()),
            hidden: meta("hidden").is_some_and(|v| v == "1"),
            locked: meta("locked").is_some_and(|v| v == "1"),
            hole: meta("hole").is_some_and(|v| v == "1"),
            keep_position: ours,
            ..Default::default()
        };
        match (file, obj) {
            // a plain mesh object: keep it local and remember the placement (keeps rotations)
            (Some(f), Some(o)) if o.components.is_empty() && o.mesh.is_some() => {
                let mut m = o.mesh.clone().unwrap();
                let n = m.positions.len() as u32;
                m.triangles.retain(|t| t.iter().all(|&i| i < n));
                if let Some((g, pi)) = o.prop {
                    io.color = f.colors.get(&g).and_then(|c| c.get(pi as usize)).copied();
                }
                let placement = *xf * Mat4::from_scale(Vec3::splat(f.unit_scale));
                if placement.abs_diff_eq(Mat4::IDENTITY, 1e-7) {
                    io.mesh = m;
                } else {
                    io.mesh = m;
                    io.transform = Some(placement);
                }
            }
            _ => {
                let mut color = None;
                pkg.flatten(&path, *oid, *xf, 0, &mut io.mesh, &mut color);
                io.color = color;
            }
        }
        let only_part = obj.filter(|o| o.components.len() == 1).map(|o| o.components[0].1);
        if let Some(c) = ext_color(only_part, *oid) {
            io.color = Some(c);
        }
        out.push(io);
    }
    Ok(out)
}

/// 3MF "m00 m01 m02 m10 … m32" (rows of the 4×3 part, column-vector convention).
fn mat_attr(m: &Mat4) -> String {
    let c = m.to_cols_array();
    format!("{} {} {} {} {} {} {} {} {} {} {} {}", c[0], c[1], c[2], c[4], c[5], c[6], c[8], c[9], c[10], c[12], c[13], c[14])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn write(objects: &[IoObject]) -> Result<Vec<u8>> {
    write_with(objects, super::SaveOptions::default())
}

/// Where Builder3D keeps the fully editable scene (with holes) inside a 3MF. Not a
/// `.model` file, so slicers ignore it.
const PROJECT_COPY: &str = "Metadata/builder3d_project.xml";

/// Write a 3MF. Groups become one object made of parts. Holes are written as negative
/// parts (Bambu Studio / OrcaSlicer read these from `Metadata/model_settings.config`) or,
/// with `cut_holes`, cut into the solids. With holes, the editable scene is stored too.
pub fn write_with(objects: &[IoObject], opts: super::SaveOptions) -> Result<Vec<u8>> {
    let has_holes = objects.iter().any(|o| o.hole);
    let (main, config) = if !has_holes {
        model_xml(objects, false)
    } else if opts.cut_holes {
        model_xml(&super::apply_holes(objects)?, false)
    } else {
        model_xml(objects, true)
    };
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("[Content_Types].xml", opt)?;
    zw.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/><Default Extension="config" ContentType="application/xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#)?;
    zw.start_file("_rels/.rels", opt)?;
    zw.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#)?;
    zw.start_file("3D/3dmodel.model", opt)?;
    zw.write_all(main.as_bytes())?;
    if !config.is_empty() {
        zw.start_file("Metadata/model_settings.config", opt)?;
        zw.write_all(config.as_bytes())?;
    }
    if has_holes {
        zw.start_file(PROJECT_COPY, opt)?;
        zw.write_all(model_xml(objects, false).0.as_bytes())?;
    }
    Ok(zw.finish()?.into_inner())
}

/// A printable unit: a group's members, or one ungrouped solid, plus attached holes.
struct Unit {
    name: String,
    group: Option<String>,
    parts: Vec<usize>,
    holes: Vec<usize>,
}

/// The model XML, plus Bambu/Orca's `model_settings.config` (part names and types) when
/// there are multi-part objects. `negative`: holes become negative parts of the units
/// they overlap; otherwise every object (holes included, flagged `b3d:hole`) is written
/// as-is — the editable layout.
/// Filament slots for a painted scene (see [`crate::paint::slots`]).
pub fn filament_slots(objects: &[IoObject]) -> Vec<u32> {
    let solids: Vec<&IoObject> = objects.iter().filter(|o| !o.hole).collect();
    crate::paint::slots(solids.iter().map(|o| o.color.unwrap_or([200, 200, 200, 255])), solids.iter().map(|o| &o.mesh))
}

use crate::paint::slot_of;

fn model_xml(objects: &[IoObject], negative: bool) -> (String, String) {
    let mut model = String::with_capacity(1 << 20);
    model.push_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:b3d="https://builder3d.app/3mf/2026" xmlns:slic3rpe="http://schemas.slic3r.org/3mf/2017/06">
<metadata name="Application">Builder3D</metadata>
<resources>
<basematerials id="1">
"#,
    );
    for o in objects {
        let c = o.color.unwrap_or([200, 200, 200, 255]);
        let _ = writeln!(model, r##"<base name="{}" displaycolor="#{:02X}{:02X}{:02X}{:02X}"/>"##, esc(&o.name), c[0], c[1], c[2], c[3]);
    }
    // painted colours: standard 3MF materials (p1) and slicer filament slots (paint_color)
    let slots = filament_slots(objects);
    let mut paint_index: HashMap<u32, usize> = HashMap::new();
    for o in objects {
        for c in crate::paint::colors(&o.mesh) {
            if !paint_index.contains_key(&c) {
                paint_index.insert(c, objects.len() + paint_index.len());
                let [r, g, b, _] = crate::paint::unpack(c);
                let _ = writeln!(model, r##"<base name="Paint {}" displaycolor="#{r:02X}{g:02X}{b:02X}FF"/>"##, paint_index.len());
            }
        }
    }
    model.push_str("</basematerials>\n");
    for (i, o) in objects.iter().enumerate() {
        let _ = writeln!(model, r#"<object id="{}" type="model" name="{}" pid="1" pindex="{}">"#, i + 2, esc(&o.name), i);
        if o.hidden || o.locked || o.hole {
            model.push_str("<metadatagroup>");
            if o.hidden {
                model.push_str(r#"<metadata name="b3d:hidden">1</metadata>"#);
            }
            if o.locked {
                model.push_str(r#"<metadata name="b3d:locked">1</metadata>"#);
            }
            if o.hole {
                model.push_str(r#"<metadata name="b3d:hole">1</metadata>"#);
            }
            model.push_str("</metadatagroup>\n");
        }
        model.push_str("<mesh><vertices>\n");
        for p in &o.mesh.positions {
            let _ = writeln!(model, r#"<vertex x="{}" y="{}" z="{}"/>"#, p.x, p.y, p.z);
        }
        model.push_str("</vertices><triangles>\n");
        let painted = o.mesh.is_painted();
        for (ti, t) in o.mesh.triangles.iter().enumerate() {
            let c = if painted { o.mesh.paint[ti] } else { 0 };
            if c == 0 {
                let _ = writeln!(model, r#"<triangle v1="{}" v2="{}" v3="{}"/>"#, t[0], t[1], t[2]);
            } else {
                let code = crate::paint::bambu_encode(slot_of(&slots, c));
                let _ = writeln!(
                    model,
                    r#"<triangle v1="{}" v2="{}" v3="{}" pid="1" p1="{}" paint_color="{code}" slic3rpe:mmu_segmentation="{code}"/>"#,
                    t[0], t[1], t[2], paint_index[&c]
                );
            }
        }
        model.push_str("</triangles></mesh></object>\n");
    }

    // units: groups (all members, holes included) and single objects
    let mut units: Vec<Unit> = vec![];
    for (i, o) in objects.iter().enumerate() {
        let in_group = o.group.as_ref().filter(|g| objects.iter().filter(|x| x.group.as_ref() == Some(g)).count() >= 2);
        match in_group {
            Some(g) => {
                let u = match units.iter().position(|u| u.group.as_ref() == Some(g)) {
                    Some(k) => &mut units[k],
                    None => {
                        units.push(Unit { name: g.clone(), group: Some(g.clone()), parts: vec![], holes: vec![] });
                        units.last_mut().unwrap()
                    }
                };
                if negative && o.hole {
                    u.holes.push(i);
                } else {
                    u.parts.push(i);
                }
            }
            None if negative && o.hole => {} // attached below
            None => units.push(Unit { name: o.name.clone(), group: None, parts: vec![i], holes: vec![] }),
        }
    }
    units.retain(|u| !u.parts.is_empty());
    if negative {
        // a loose hole cuts every unit it overlaps
        let wb: Vec<crate::Aabb> = objects.iter().map(|o| o.world_mesh().bounds()).collect();
        for (h, o) in objects.iter().enumerate() {
            if !o.hole || units.iter().any(|u| u.holes.contains(&h)) {
                continue;
            }
            for u in units.iter_mut() {
                if u.parts.iter().any(|&p| super::overlaps(&wb[p], &wb[h])) {
                    u.holes.push(h);
                }
            }
        }
    }

    let mut config = String::new();
    let mut items: Vec<(usize, Option<Mat4>)> = vec![];
    let mut next_id = objects.len() + 2;
    // which filament slot prints each object (only for painted scenes)
    let extruder = |i: usize| -> String {
        if slots.is_empty() {
            return String::new();
        }
        let c = crate::paint::pack(objects[i].color.unwrap_or([200, 200, 200, 255]));
        format!("\n      <metadata key=\"extruder\" value=\"{}\"/>", slot_of(&slots, c))
    };
    for u in &units {
        if u.parts.len() == 1 && u.holes.is_empty() {
            let i = u.parts[0];
            items.push((i + 2, objects[i].transform));
            if !slots.is_empty() {
                let _ = writeln!(config, r#"  <object id="{}">"#, i + 2);
                let _ = writeln!(
                    config,
                    r#"    <metadata key="name" value="{}"/>{}"#,
                    esc(&objects[i].name),
                    extruder(i).replace("\n  ", "\n")
                );
                config.push_str("  </object>\n");
            }
            continue;
        }
        // one object made of parts; origin at the bottom centre of its solid parts
        let gid = next_id;
        next_id += 1;
        let b = u.parts.iter().fold(crate::Aabb::EMPTY, |b, &i| b.union(&objects[i].world_mesh().bounds()));
        let origin = Mat4::from_translation(Vec3::new(b.center().x, b.center().y, b.min.z));
        let inv = origin.inverse();
        let _ = writeln!(model, r#"<object id="{gid}" type="model" name="{}"><components>"#, esc(&u.name));
        let _ = writeln!(config, r#"  <object id="{gid}">"#);
        let _ = writeln!(config, r#"    <metadata key="name" value="{}"/>"#, esc(&u.name));
        for (&i, kind) in
            u.parts.iter().map(|i| (i, "normal_part")).chain(u.holes.iter().map(|i| (i, "negative_part"))).collect::<Vec<_>>().iter()
        {
            let m = inv * objects[i].transform.unwrap_or(Mat4::IDENTITY);
            let _ = writeln!(model, r#"<component objectid="{}" transform="{}"/>"#, i + 2, mat_attr(&m));
            let _ = writeln!(config, r#"    <part id="{}" subtype="{kind}">"#, i + 2);
            let ext = if kind == &"normal_part" { extruder(i) } else { String::new() };
            let _ = writeln!(config, r#"      <metadata key="name" value="{}"/>{ext}"#, esc(&objects[i].name));
            let _ = writeln!(config, "    </part>");
        }
        model.push_str("</components></object>\n");
        config.push_str("  </object>\n");
        items.push((gid, Some(origin)));
    }
    model.push_str("</resources>\n<build>\n");
    for (id, xf) in items {
        match xf {
            Some(m) => {
                let _ = writeln!(model, r#"<item objectid="{id}" transform="{}"/>"#, mat_attr(&m));
            }
            None => {
                let _ = writeln!(model, r#"<item objectid="{id}"/>"#);
            }
        }
    }
    model.push_str("</build>\n</model>\n");
    let config =
        if config.is_empty() { config } else { format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n{config}</config>\n") };
    (model, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_and_metadata_roundtrip() {
        let m = crate::primitives::cuboid(Vec3::new(10.0, 4.0, 2.0));
        let xf =
            Mat4::from_scale_rotation_translation(Vec3::new(2.0, 1.0, 1.0), glam::Quat::from_rotation_z(0.7), Vec3::new(5.0, -3.0, 1.0));
        let o = IoObject {
            name: "Rotated & co".into(),
            mesh: m.clone(),
            color: Some([1, 2, 3, 255]),
            transform: Some(xf),
            group: Some("Group <A>".into()),
            hidden: true,
            locked: true,
            ..Default::default()
        };
        let mate = IoObject {
            name: "mate".into(),
            mesh: m.clone(),
            transform: Some(Mat4::from_translation(Vec3::new(40.0, 0.0, 1.0))),
            group: Some("Group <A>".into()),
            ..Default::default()
        };
        let bytes = write(&[IoObject::new("plain", m.clone(), None), o, mate]).unwrap();
        // the group is one object made of two parts (what slicers expect)
        let xml = {
            let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
            let mut s = String::new();
            z.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut s).unwrap();
            s
        };
        assert_eq!(xml.matches("<component ").count(), 2);
        assert_eq!(xml.matches("<item ").count(), 2);
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert!(back[0].transform.is_none() && back[0].group.is_none() && !back[0].hidden);
        let a = &back[1];
        assert!(a.transform.unwrap().abs_diff_eq(xf, 1e-4), "{:?}", a.transform);
        assert_eq!(a.mesh.positions, m.positions);
        assert_eq!(a.group.as_deref(), Some("Group <A>"));
        assert!(a.hidden && a.locked && a.keep_position);
        assert_eq!(a.color, Some([1, 2, 3, 255]));
        assert_eq!(back[2].group.as_deref(), Some("Group <A>"));
        assert!(back[2].transform.unwrap().abs_diff_eq(Mat4::from_translation(Vec3::new(40.0, 0.0, 1.0)), 1e-4));
    }

    fn read_model(bytes: &[u8], name: &str) -> Option<String> {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).ok()?.read_to_string(&mut s).unwrap();
        Some(s)
    }

    #[test]
    fn holes_as_negative_parts_or_cut() {
        let block = crate::primitives::cuboid(Vec3::new(20.0, 20.0, 10.0));
        let pin = crate::primitives::cuboid(Vec3::new(4.0, 4.0, 30.0));
        let objs = vec![
            IoObject {
                name: "Block".into(),
                mesh: block.clone(),
                transform: Some(Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
                ..Default::default()
            },
            IoObject {
                name: "Hole".into(),
                mesh: pin.clone(),
                transform: Some(Mat4::from_translation(Vec3::new(3.0, 0.0, 5.0))),
                hole: true,
                ..Default::default()
            },
            IoObject {
                name: "Far".into(),
                mesh: block.clone(),
                transform: Some(Mat4::from_translation(Vec3::new(100.0, 0.0, 5.0))),
                ..Default::default()
            },
        ];
        // negative parts: the block becomes block + negative hole; the far block is untouched
        let bytes = write_with(&objs, crate::io::SaveOptions { cut_holes: false }).unwrap();
        let cfg = read_model(&bytes, "Metadata/model_settings.config").unwrap();
        assert_eq!(cfg.matches("negative_part").count(), 1, "{cfg}");
        assert_eq!(cfg.matches("normal_part").count(), 1, "{cfg}");
        let main = read_model(&bytes, "3D/3dmodel.model").unwrap();
        assert_eq!(main.matches("<item ").count(), 2);
        // Builder3D reads back the editable scene: the hole is still a hole
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert!(back[1].hole && !back[0].hole && back[1].group.is_none());

        // cut: the slicer model has the hole cut out, no negative parts
        let bytes = write_with(&objs, crate::io::SaveOptions { cut_holes: true }).unwrap();
        assert!(read_model(&bytes, "Metadata/model_settings.config").is_none());
        let main = read_model(&bytes, "3D/3dmodel.model").unwrap();
        assert_eq!(main.matches("<item ").count(), 2);
        let back = read(&bytes).unwrap();
        assert!(back.iter().any(|o| o.hole), "editable copy keeps the hole");
        // the cut geometry, as a slicer sees it
        let cut = crate::io::apply_holes(&objs).unwrap();
        assert_eq!(cut.len(), 2);
        let expect = 20.0 * 20.0 * 10.0 - 4.0 * 4.0 * 10.0;
        assert!((cut[0].mesh.volume() - expect).abs() < 1e-3, "{}", cut[0].mesh.volume());
    }

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

    #[test]
    fn paint_roundtrip_and_slicer_slots() {
        use crate::paint::pack;
        let mut m = crate::primitives::cuboid(Vec3::splat(10.0));
        let (red, blue) = (pack([255, 0, 0, 255]), pack([0, 0, 255, 255]));
        let p = m.paint_mut();
        p[0] = red;
        p[1] = red;
        p[5] = blue;
        let objs = vec![
            IoObject::new("Painted", m.clone(), Some([200, 200, 200, 255])),
            IoObject::new("Plain", crate::primitives::cuboid(Vec3::splat(5.0)), Some([0, 255, 0, 255])),
        ];
        let bytes = write(&objs).unwrap();
        let main = read_model(&bytes, "3D/3dmodel.model").unwrap();
        // slots: grey (object 1), green (object 2), red, blue
        assert_eq!(main.matches(r#"paint_color="0C""#).count(), 2, "red is slot 3");
        assert_eq!(main.matches(r#"paint_color="1C""#).count(), 1, "blue is slot 4");
        assert!(main.contains(r#"slic3rpe:mmu_segmentation="0C""#));
        let cfg = read_model(&bytes, "Metadata/model_settings.config").unwrap();
        assert!(cfg.contains(r#"<metadata key="extruder" value="2"/>"#), "{cfg}");
        let back = read(&bytes).unwrap();
        assert_eq!(back[0].mesh.paint, m.paint);
        assert!(!back[1].mesh.is_painted());
        assert_eq!(back[1].color, Some([0, 255, 0, 255]));
    }

    #[test]
    fn bambu_paint_and_filament_colours() {
        let cube = crate::primitives::cuboid(Vec3::splat(2.0));
        let mut verts = String::new();
        for p in &cube.positions {
            let _ = write!(verts, r#"<vertex x="{}" y="{}" z="{}"/>"#, p.x, p.y, p.z);
        }
        let mut tris = String::new();
        for (i, t) in cube.triangles.iter().enumerate() {
            let paint = if i < 2 { r#" paint_color="8""# } else { "" };
            let _ = write!(tris, r#"<triangle v1="{}" v2="{}" v3="{}"{paint}/>"#, t[0], t[1], t[2]);
        }
        let main = format!(
            r#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="3" name="Cube" type="model"><mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh></object></resources><build><item objectid="3"/></build></model>"#
        );
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opt = zip::write::SimpleFileOptions::default();
        zw.start_file("3D/3dmodel.model", opt).unwrap();
        zw.write_all(main.as_bytes()).unwrap();
        zw.start_file("Metadata/project_settings.config", opt).unwrap();
        zw.write_all(br##"{ "filament_colour": [ "#FFFFFF", "#FF0000", "#00AE42" ], "other": [1] }"##).unwrap();
        zw.start_file("Metadata/model_settings.config", opt).unwrap();
        zw.write_all(br#"<config><object id="3"><metadata key="extruder" value="3"/></object></config>"#).unwrap();
        let bytes = zw.finish().unwrap().into_inner();
        let o = &read(&bytes).unwrap()[0];
        assert_eq!(o.color, Some([0x00, 0xAE, 0x42, 255]), "object prints with filament 3");
        assert_eq!(o.mesh.paint[0], crate::paint::pack([255, 0, 0, 255]), "painted with filament 2");
        assert_eq!(o.mesh.paint[2], 0);
    }
}
