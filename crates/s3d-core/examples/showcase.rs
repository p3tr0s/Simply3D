//! Writes the scenes used for the README screenshots:
//! `cargo run --release -p s3d-core --example showcase -- <folder> [font.ttf]`
use glam::{Mat4, Vec3};
use s3d_core::bricks::Brick;
use s3d_core::io::{save, IoObject};
use s3d_core::mesh::Mesh;
use s3d_core::organizers::{enclosure_mesh, gridfinity_baseplate, gridfinity_bin, Enclosure, GridBin, GridPlate, LidKind};
use s3d_core::paint::pack;
use s3d_core::parts::{gear_mesh, hinge_mesh, snap_clip_mesh, spring_mesh, Gear, Hinge, SnapClip, Spring};
use s3d_core::primitives::{beveled_box, beveled_cylinder, revolve, sphere};
use s3d_core::progress::Progress;
use s3d_core::sdf::{pattern, Pattern, PatternOptions};
use s3d_core::texture::{builtin, displace, Builtin, Mapping, TextureOptions};
use s3d_core::threads::{fastener_mesh, Fastener, FastenerKind};

/// Centre on (x, y), standing on the plate.
fn place(mut m: Mesh, x: f32, y: f32) -> Mesh {
    let b = m.bounds();
    let c = b.center();
    m.translate(Vec3::new(x - c.x, y - c.y, -b.min.z));
    m
}
fn rot_z(m: &Mesh, deg: f32) -> Mesh {
    m.transformed(&Mat4::from_rotation_z(deg.to_radians()))
}
fn obj(name: &str, m: Mesh, c: [u8; 3]) -> IoObject {
    IoObject::new(name, m, Some([c[0], c[1], c[2], 255]))
}

const BLUE: [u8; 3] = [72, 140, 230];
const ORANGE: [u8; 3] = [245, 140, 50];
const GREEN: [u8; 3] = [80, 200, 120];
const GREY: [u8; 3] = [150, 158, 170];
const RED: [u8; 3] = [225, 70, 70];
const YELLOW: [u8; 3] = [245, 200, 60];
const PURPLE: [u8; 3] = [160, 110, 220];
const TEAL: [u8; 3] = [60, 190, 200];

fn vase() -> Mesh {
    let mut prof = vec![(0.0, 0.0)];
    for i in 0..=24 {
        let t = i as f32 / 24.0;
        let r = 16.0 + 7.0 * (t * 5.5).sin() - 4.0 * t;
        prof.push((r, t * 60.0));
    }
    prof.push((0.0, 60.0));
    revolve(&prof, 96)
}

fn paint_bands(mut m: Mesh, colors: &[[u8; 3]], band: f32) -> Mesh {
    let paint: Vec<u32> = (0..m.triangles.len())
        .map(|i| {
            let [a, b, c] = m.tri(i);
            let z = (a.z + b.z + c.z) / 3.0;
            let k = ((z / band) as usize) % colors.len();
            let c = colors[k];
            pack([c[0], c[1], c[2], 255])
        })
        .collect();
    m.paint = paint;
    m
}

fn textured(m: &Mesh, t: Builtin, mapping: Mapping, tile: f32, depth: f32) -> Mesh {
    let o = TextureOptions { tile, depth, mapping, ..Default::default() };
    displace(m, None, &builtin(t), &o, &Progress::new()).unwrap().0
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&out).unwrap();

    // --- mechanical parts
    let g1 = Gear { teeth: 24, module: 2.0, thickness: 8.0, bore: 6.0, ..Default::default() };
    let g2 = Gear { teeth: 14, module: 2.0, thickness: 8.0, bore: 5.0, ..Default::default() };
    let centre = 2.0 * (24.0 + 14.0) / 2.0;
    let mut gear1 = gear_mesh(&g1);
    gear1.translate(Vec3::new(-60.0, 20.0, 0.0));
    let mut gear2 = rot_z(&gear_mesh(&g2), 180.0 / 14.0);
    gear2.translate(Vec3::new(-60.0 + centre, 20.0, 0.0));
    let bolt = Fastener { kind: FastenerKind::Bolt, length: 30.0, ..Default::default() };
    let nut = Fastener { kind: FastenerKind::Nut, ..Default::default() };
    let rod = Fastener { kind: FastenerKind::Rod, length: 40.0, ..Default::default() };
    let parts = vec![
        obj("Gear 24T", gear1, BLUE),
        obj("Gear 14T", gear2, ORANGE),
        obj("Spring", place(spring_mesh(&Spring { length: 35.0, ..Default::default() }), 50.0, 25.0), GREEN),
        obj("Hinge", place(hinge_mesh(&Hinge { length: 50.0, leaf: 18.0, ..Default::default() }).unwrap(), -40.0, -40.0), GREY),
        obj("Bolt", place(fastener_mesh(&bolt).unwrap(), 20.0, -35.0), YELLOW),
        obj("Nut", place(fastener_mesh(&nut).unwrap(), 45.0, -35.0), YELLOW),
        obj("Threaded rod", place(fastener_mesh(&rod).unwrap(), 70.0, -35.0), PURPLE),
        obj("Snap clip", place(snap_clip_mesh(&SnapClip::default()), 80.0, 20.0), RED),
    ];
    save(&out.join("parts.3mf"), &parts).unwrap();

    // --- storage and bricks
    let encl = Enclosure { size: Vec3::new(60.0, 40.0, 30.0), lid: LidKind::Hinged, dividers_x: 1, ..Default::default() };
    let plate = gridfinity_baseplate(&GridPlate { units_x: 3, units_y: 2 }).unwrap();
    let pb = plate.bounds();
    let plate = place(plate, 55.0, -5.0);
    let bin_a = gridfinity_bin(&GridBin { units_x: 2, units_y: 1, height_units: 4, compartments_x: 2, ..Default::default() }).unwrap();
    let bin_b = gridfinity_bin(&GridBin { units_x: 1, units_y: 1, height_units: 6, ..Default::default() }).unwrap();
    let pitch = 42.0;
    let x0 = 55.0 - pb.size().x * 0.5;
    let y0 = -5.0 - pb.size().y * 0.5;
    let mut a = place(bin_a, x0 + pitch, y0 + pitch * 0.5);
    a.translate(Vec3::Z * 5.0);
    let mut b = place(bin_b, x0 + pitch * 2.5, y0 + pitch * 1.5);
    b.translate(Vec3::Z * 5.0);
    let bricks = [
        (Brick::lego(4, 2, 3, true), -100.0, -15.0, RED),
        (Brick::lego(2, 2, 3, true), -68.0, -15.0, YELLOW),
        (Brick::lego(4, 2, 1, true), -100.0, -50.0, GREEN),
        (Brick::lego(2, 2, 1, false), -70.0, -50.0, BLUE),
        (Brick::duplo(2, 2), -45.0, -85.0, ORANGE),
    ];
    let mut storage = vec![
        obj("Box with lid", place(enclosure_mesh(&encl).unwrap(), -50.0, 60.0), TEAL),
        obj("Baseplate", plate, GREY),
        obj("Bin 2×1", a, BLUE),
        obj("Bin 1×1", b, ORANGE),
    ];
    for (i, (br, x, y, c)) in bricks.iter().enumerate() {
        storage.push(obj(&format!("Brick {}", i + 1), place(s3d_core::bricks::brick_mesh(br).unwrap(), *x, *y), *c));
    }
    save(&out.join("storage.3mf"), &storage).unwrap();

    // --- mesh tools: textures and patterns
    let knob = textured(&beveled_cylinder(18.0, 30.0, 96, 2.0, 4), Builtin::Knurl, Mapping::Cylinder, 4.0, 0.8);
    let wood = textured(&beveled_box(Vec3::new(50.0, 35.0, 20.0), 3.0, 4, false), Builtin::Bricks, Mapping::Triplanar, 10.0, 0.8);
    let carbon = textured(&beveled_box(Vec3::new(40.0, 40.0, 6.0), 2.0, 4, false), Builtin::Carbon, Mapping::Triplanar, 8.0, 0.4);
    let vor = pattern(&sphere(24.0, 96, 48), PatternOptions { pattern: Pattern::Voronoi, cell: 10.0, thickness: 1.8, ..Default::default() }, &Progress::new())
        .unwrap()
        .0;
    let gyr = pattern(
        &beveled_box(Vec3::splat(36.0), 4.0, 4, false),
        PatternOptions { pattern: Pattern::Gyroid, cell: 12.0, thickness: 1.6, skin: 0.0, ..Default::default() },
        &Progress::new(),
    )
    .unwrap()
    .0;
    let mesh = vec![
        obj("Knurled knob", place(knob, -70.0, 25.0), ORANGE),
        obj("Brick-pattern box", place(wood, -10.0, 25.0), [205, 120, 90]),
        obj("Carbon plate", place(carbon, 55.0, 25.0), [120, 126, 138]),
        obj("Voronoi ball", place(vor, -40.0, -40.0), TEAL),
        obj("Gyroid cube", place(gyr, 30.0, -40.0), PURPLE),
    ];
    save(&out.join("mesh.3mf"), &mesh).unwrap();

    // --- paint
    let v = paint_bands(vase(), &[BLUE, [240, 240, 240], ORANGE, [240, 240, 240]], 7.5);
    let mut ball = sphere(20.0, 96, 48);
    ball.translate(Vec3::Z * 20.0);
    let ball = paint_bands(ball, &[RED, YELLOW, GREEN, BLUE, PURPLE], 8.0);
    let painted = vec![obj("Vase", place(v, -30.0, 0.0), [240, 240, 240]), obj("Ball", place(ball, 35.0, 0.0), [240, 240, 240])];
    save(&out.join("paint.3mf"), &painted).unwrap();

    // --- overview
    let font_path = std::env::args().nth(2).unwrap_or_else(|| "/usr/share/fonts/truetype/google-fonts/Poppins-Bold.ttf".into());
    let font = std::fs::read(&font_path).expect("pass a .ttf font as the second argument");
    let text = s3d_core::text::text_mesh(
        &font,
        0,
        &s3d_core::text::TextOptions { text: "Simply3D".into(), size: 14.0, depth: 4.0, ..Default::default() },
    )
    .unwrap();
    let base = beveled_box(Vec3::new(100.0, 28.0, 4.0), 1.5, 4, false);
    let base = place(base, 0.0, -70.0);
    let mut text = place(text, 0.0, -70.0);
    text.translate(Vec3::Z * 4.0);
    let mut g1 = gear_mesh(&g1);
    g1.translate(Vec3::new(55.0, 25.0, 0.0));
    let mut g2 = rot_z(&gear_mesh(&g2), 180.0 / 14.0);
    g2.translate(Vec3::new(55.0 + centre, 25.0, 0.0));
    let hero = vec![
        obj("Vase", place(paint_bands(vase(), &[BLUE, [240, 240, 240], ORANGE, [240, 240, 240]], 7.5), -70.0, 25.0), [240, 240, 240]),
        obj("Knurled knob", place(textured(&beveled_cylinder(16.0, 26.0, 96, 2.0, 4), Builtin::Knurl, Mapping::Cylinder, 4.0, 0.8), -20.0, 30.0), ORANGE),
        obj("Gear 24T", g1, BLUE),
        obj("Gear 14T", g2, GREEN),
        obj("Bolt", place(fastener_mesh(&bolt).unwrap(), 10.0, -15.0), YELLOW),
        obj("Nut", place(fastener_mesh(&nut).unwrap(), 35.0, -15.0), YELLOW),
        obj("Voronoi ball", place(pattern(&sphere(18.0, 96, 48), PatternOptions { cell: 8.0, ..Default::default() }, &Progress::new()).unwrap().0, -45.0, -20.0), TEAL),
        obj("Name plate", base, [40, 44, 52]),
        obj("Text", text, [240, 240, 240]),
    ];
    save(&out.join("overview.3mf"), &hero).unwrap();
    println!("wrote scenes to {}", out.display());
}
