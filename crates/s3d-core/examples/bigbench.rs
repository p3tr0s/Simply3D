use glam::Vec3;
use s3d_core::scene::{MeshHandle, Transform};
use s3d_core::sculpt::{Brush, Sculpt};
use std::time::Instant;
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
fn main() {
    let m = s3d_core::primitives::sphere(50.0, 1450, 1000);
    println!("{} tris", m.triangles.len());
    let h = MeshHandle::new(m.clone());
    let t = Instant::now();
    let _ = h.bvh();
    println!("bvh() first call (bg start) {:.1} ms", ms(t));
    let t = Instant::now();
    let b = s3d_core::bvh::Bvh::build(&m);
    println!("Bvh::build {:.1} ms", ms(t));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let t = Instant::now();
    for i in 0..1000 {
        let o = Vec3::new(i as f32 * 0.05 - 25.0, -200.0, 3.0);
        h.ray_cast_where(o, Vec3::Y, |_| true);
    }
    println!("pick {:.4} ms each", ms(t) / 1000.0);
    let t = Instant::now();
    let wb = h.world_bounds(&Transform::default());
    println!("world_bounds first {:.1} ms {:?}", ms(t), wb.size());
    let t = Instant::now();
    h.world_bounds(&Transform { translation: Vec3::X, ..Default::default() });
    println!("world_bounds moved {:.3} ms", ms(t));
    let t = Instant::now();
    let v = s3d_core::paint::brush_stroke(&m, Some(&b), Vec3::new(0.0, -50.0, 0.0), Vec3::new(1.0, -50.0, 0.0), 3.0, -Vec3::Y);
    println!("paint brush_stroke bvh {:.2} ms ({})", ms(t), v.len());
    let t = Instant::now();
    let v = s3d_core::paint::brush_stroke(&m, None, Vec3::new(0.0, -50.0, 0.0), Vec3::new(1.0, -50.0, 0.0), 3.0, -Vec3::Y);
    println!("paint brush_stroke brute {:.2} ms ({})", ms(t), v.len());
    let t = Instant::now();
    let sh = s3d_core::normals::shading(&m, 35.0);
    println!("smooth normals {:.1} ms ({} groups)", ms(t), sh.normals.len());
    let t = Instant::now();
    let mut sc = Sculpt::with_bvh(m.clone(), h.bvh());
    println!("Sculpt::with_bvh {:.1} ms", ms(t));
    let t = Instant::now();
    for k in 0..50 {
        let c = Vec3::new(k as f32 * 0.2, -50.0, 0.0);
        sc.ensure_detail(c, 3.0, 3_000_000);
        sc.dab(Brush::Draw, c, -Vec3::Y, 3.0, 0.5, false);
    }
    println!("sculpt dab {:.2} ms each", ms(t) / 50.0);
    let t = Instant::now();
    let _h2 = MeshHandle::with_bvh(sc.mesh.clone(), sc.bvh.clone());
    println!("sculpt push (mesh clone) {:.1} ms", ms(t));
    let t = Instant::now();
    std::sync::Arc::make_mut(&mut sc.bvh);
    println!("bvh clone on shared refit {:.1} ms", ms(t));
}
