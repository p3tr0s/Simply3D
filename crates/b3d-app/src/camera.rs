//! Z-up orbit camera with perspective / orthographic projection.

use b3d_core::Aabb;
use glam::{Mat4, Vec2, Vec3, Vec4};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Radians, around +Z, 0 = looking from +X.
    pub yaw: f32,
    /// Radians, elevation above the XY plane.
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    pub ortho: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum ViewPreset {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Iso,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0.0, 0.0, 10.0),
            yaw: -60f32.to_radians(),
            pitch: 30f32.to_radians(),
            distance: 350.0,
            fov_y: 35f32.to_radians(),
            ortho: false,
        }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * cy, cp * sy, sp) * self.distance
    }
    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye()).normalize()
    }
    fn up_hint(&self) -> Vec3 {
        // near the poles use yaw-based up so look_at stays well-defined
        if self.pitch.abs() > 89.0f32.to_radians() {
            let (sy, cy) = self.yaw.sin_cos();
            -Vec3::new(cy, sy, 0.0) * self.pitch.signum()
        } else {
            Vec3::Z
        }
    }
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, self.up_hint())
    }
    pub fn right_up(&self) -> (Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(self.up_hint()).normalize();
        (r, r.cross(f))
    }
    pub fn near_far(&self) -> (f32, f32) {
        let far = self.distance * 20.0 + 5000.0;
        if self.ortho {
            (-far, far)
        } else {
            ((self.distance * 0.005).max(0.01), far)
        }
    }
    pub fn proj(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.near_far();
        if self.ortho {
            let h = self.distance * (self.fov_y * 0.5).tan();
            let w = h * aspect;
            glam::camera::rh::proj::directx::orthographic(-w, w, -h, h, near, far)
        } else {
            glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, near, far)
        }
    }
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    /// World ray through a point given in normalised device coords (-1..1, y up).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let p0 = inv * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let p1 = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let a = p0.truncate() / p0.w;
        let b = p1.truncate() / p1.w;
        (a, (b - a).normalize())
    }

    /// Project world point to NDC (x, y in -1..1). None if behind camera.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<Vec2> {
        let c = self.view_proj(aspect) * p.extend(1.0);
        (c.w > 1e-6).then(|| Vec2::new(c.x / c.w, c.y / c.w))
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-89.9f32.to_radians(), 89.9f32.to_radians());
    }

    /// Pan by a screen-space delta (pixels), given the viewport height in pixels.
    pub fn pan(&mut self, delta: Vec2, viewport_h: f32) {
        let (r, u) = self.right_up();
        let world_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += (-r * delta.x + u * delta.y) * world_per_px;
    }

    /// Zoom toward a world point (cursor), `steps` > 0 zooms in.
    pub fn zoom(&mut self, steps: f32, toward: Option<Vec3>) {
        let factor = (-steps * 0.0015).exp().clamp(0.5, 2.0);
        let new_dist = (self.distance * factor).clamp(0.5, 50_000.0);
        if let Some(p) = toward {
            let k = 1.0 - new_dist / self.distance;
            self.target += (p - self.target) * k;
        }
        self.distance = new_dist;
    }

    pub fn fit(&mut self, b: &Aabb, aspect: f32) {
        if b.is_empty() {
            return;
        }
        self.target = b.center();
        let radius = (b.size().length() * 0.5).max(1.0);
        let half = (self.fov_y * 0.5).tan() * aspect.min(1.0);
        self.distance = (radius / half.atan().sin()) * 1.1;
    }

    pub fn set_preset(&mut self, v: ViewPreset) {
        let d = |x: f32| x.to_radians();
        let (yaw, pitch) = match v {
            ViewPreset::Front => (d(-90.0), 0.0),
            ViewPreset::Back => (d(90.0), 0.0),
            ViewPreset::Left => (d(180.0), 0.0),
            ViewPreset::Right => (0.0, 0.0),
            ViewPreset::Top => (d(-90.0), d(89.95)),
            ViewPreset::Bottom => (d(-90.0), d(-89.95)),
            ViewPreset::Iso => (d(-60.0), d(30.0)),
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_ray_hits_target() {
        for ortho in [false, true] {
            let c = OrbitCamera { ortho, ..Default::default() };
            let (o, d) = c.ray(Vec2::ZERO, 1.5);
            let to_target = c.target - o;
            assert!(to_target.cross(d).length() < 1e-2 * to_target.length(), "ortho={ortho}");
        }
    }

    #[test]
    fn project_roundtrip() {
        let c = OrbitCamera::default();
        let p = Vec3::new(12.0, -7.0, 3.0);
        let ndc = c.project(p, 1.3).unwrap();
        let (o, d) = c.ray(ndc, 1.3);
        assert!((p - o).cross(d).length() < 1e-2);
    }
}

#[cfg(test)]
mod pick_sweep {
    use super::*;
    use b3d_core::primitives::Primitive;
    use b3d_core::scene::{Scene, Transform};

    #[test]
    fn no_holes_in_picking() {
        let mut s = Scene::default();
        for (i, p) in ["Box", "Sphere", "Cylinder", "Torus"].iter().enumerate() {
            let m = Primitive::default_for(p).build();
            let rotation = glam::Quat::from_euler(glam::EulerRot::XYZ, 0.5 * i as f32, 0.3, 0.7 * i as f32);
            let scale = Vec3::new(1.0 + 0.3 * i as f32, 0.8, 1.2);
            s.add_object(*p, m, Transform { translation: Vec3::new(i as f32 * 40.0 - 60.0, 0.0, 10.0), rotation, scale }, None);
        }
        let mut cam = OrbitCamera::default();
        cam.fit(&s.bounds(), 1.5);
        let (w, h) = (300, 200);
        let hit = |x: i32, y: i32| {
            let ndc = Vec2::new(x as f32 / w as f32 * 2.0 - 1.0, 1.0 - y as f32 / h as f32 * 2.0);
            let (o, d) = cam.ray(ndc, 1.5);
            s.pick(o, d).map(|h| h.0)
        };
        let mut holes = 0;
        let mut total = 0;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let c = hit(x, y);
                if c.is_some() {
                    total += 1;
                }
                // a miss surrounded by hits on the same object = hole
                if c.is_none() {
                    let n = [hit(x - 1, y), hit(x + 1, y), hit(x, y - 1), hit(x, y + 1)];
                    if n.iter().all(|v| v.is_some() && *v == n[0]) {
                        holes += 1;
                    }
                }
            }
        }
        eprintln!("hits {total} holes {holes}");
        assert_eq!(holes, 0);
    }
}
