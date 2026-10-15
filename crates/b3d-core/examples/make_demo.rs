//! Writes demo files: a multi-object 3MF and a broken STL (for repair testing).
use b3d_core::io::{save, IoObject};
use b3d_core::primitives::{cuboid, frustum, sphere, torus};
use glam::Vec3;

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&out).unwrap();
    let mut base = cuboid(Vec3::new(40.0, 40.0, 20.0));
    base.translate(Vec3::new(-40.0, 0.0, 10.0));
    let mut ball = sphere(15.0, 64, 32);
    ball.translate(Vec3::new(10.0, 0.0, 15.0));
    let mut ring = torus(15.0, 4.0, 96, 32);
    ring.translate(Vec3::new(55.0, 0.0, 4.0));
    let mut post = frustum(6.0, 6.0, 40.0, 48);
    post.translate(Vec3::new(-40.0, 0.0, 20.0));
    save(
        &out.join("demo.3mf"),
        &[
            IoObject { name: "Base block".into(), mesh: base, color: Some([96, 165, 250, 255]) },
            IoObject { name: "Ball".into(), mesh: ball, color: Some([251, 146, 60, 255]) },
            IoObject { name: "Ring".into(), mesh: ring, color: Some([74, 222, 128, 255]) },
            IoObject { name: "Post".into(), mesh: post, color: Some([244, 114, 182, 255]) },
        ],
    )
    .unwrap();
    let mut broken = sphere(20.0, 48, 24);
    let n = broken.triangles.len();
    broken.triangles.drain(n / 2..n / 2 + 40);
    save(&out.join("broken.stl"), &[IoObject { name: "broken".into(), mesh: broken, color: None }]).unwrap();
    println!("wrote demo files to {}", out.display());
}
