//! Metric threads and fasteners: bolts (hex / socket head), nuts, threaded rods and
//! threaded-hole cutters. ISO 68-1 basic profile (60°), coarse pitches, ISO 4017 / 4032 /
//! 4762 head and nut sizes. Internal threads get a radial clearance for printing.

use crate::csg::{boolean, BoolOp};
use crate::mesh::Mesh;
use crate::primitives::frustum;
use crate::Result;
use glam::Vec3;
use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy)]
pub struct MetricSize {
    pub name: &'static str,
    /// Nominal (major) diameter, mm.
    pub d: f32,
    /// Coarse pitch, mm.
    pub pitch: f32,
    /// Hex across flats (bolt head and nut).
    pub hex: f32,
    /// Nut height.
    pub nut: f32,
    /// Hex head height.
    pub head: f32,
    /// Socket head: diameter, height, hex key size.
    pub sock_d: f32,
    pub sock_k: f32,
    pub sock_s: f32,
}

macro_rules! m {
    ($n:expr, $d:expr, $p:expr, $hex:expr, $nut:expr, $head:expr, $sd:expr, $sk:expr, $ss:expr) => {
        MetricSize { name: $n, d: $d, pitch: $p, hex: $hex, nut: $nut, head: $head, sock_d: $sd, sock_k: $sk, sock_s: $ss }
    };
}

pub const SIZES: [MetricSize; 13] = [
    m!("M2", 2.0, 0.4, 4.0, 1.6, 1.4, 3.8, 2.0, 1.5),
    m!("M2.5", 2.5, 0.45, 5.0, 2.0, 1.7, 4.5, 2.5, 2.0),
    m!("M3", 3.0, 0.5, 5.5, 2.4, 2.0, 5.5, 3.0, 2.5),
    m!("M4", 4.0, 0.7, 7.0, 3.2, 2.8, 7.0, 4.0, 3.0),
    m!("M5", 5.0, 0.8, 8.0, 4.7, 3.5, 8.5, 5.0, 4.0),
    m!("M6", 6.0, 1.0, 10.0, 5.2, 4.0, 10.0, 6.0, 5.0),
    m!("M8", 8.0, 1.25, 13.0, 6.8, 5.3, 13.0, 8.0, 6.0),
    m!("M10", 10.0, 1.5, 16.0, 8.4, 6.4, 16.0, 10.0, 8.0),
    m!("M12", 12.0, 1.75, 18.0, 10.8, 7.5, 18.0, 12.0, 10.0),
    m!("M14", 14.0, 2.0, 21.0, 12.8, 8.8, 21.0, 14.0, 12.0),
    m!("M16", 16.0, 2.0, 24.0, 14.8, 10.0, 24.0, 16.0, 14.0),
    m!("M20", 20.0, 2.5, 30.0, 18.0, 12.5, 30.0, 20.0, 17.0),
    m!("M24", 24.0, 3.0, 36.0, 21.5, 15.0, 36.0, 24.0, 19.0),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastenerKind {
    Bolt,
    Nut,
    Rod,
    /// A thread-shaped cutter, meant to be used as a hole object.
    ThreadedHole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    Hex,
    Socket,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fastener {
    pub kind: FastenerKind,
    /// Index into [`SIZES`].
    pub size: usize,
    /// Thread length (bolt shaft, rod, hole depth), mm.
    pub length: f32,
    pub head: Head,
    /// Radial play added to internal threads (nuts, holes), mm.
    pub clearance: f32,
}

impl Default for Fastener {
    fn default() -> Self {
        Fastener { kind: FastenerKind::Bolt, size: 5, length: 20.0, head: Head::Hex, clearance: 0.2 }
    }
}

impl Fastener {
    pub fn spec(&self) -> MetricSize {
        SIZES[self.size.min(SIZES.len() - 1)]
    }
    pub fn label(&self) -> String {
        let s = self.spec();
        match self.kind {
            FastenerKind::Bolt => format!("{} × {} bolt", s.name, self.length),
            FastenerKind::Nut => format!("{} nut", s.name),
            FastenerKind::Rod => format!("{} × {} threaded rod", s.name, self.length),
            FastenerKind::ThreadedHole => format!("{} threaded hole", s.name),
        }
    }
}

/// Radial position on the basic profile at phase `u` (0 = middle of a crest), for a
/// thread with major radius `r_maj` and minor radius `r_min`.
fn profile(u: f32, r_maj: f32, r_min: f32) -> f32 {
    let a = (u - u.round()).abs(); // 0 … 0.5 from the crest centre
    const CREST: f32 = 1.0 / 16.0; // crest flat P/8 in total
    const FLANK: f32 = 5.0 / 16.0;
    if a <= CREST {
        r_maj
    } else if a <= CREST + FLANK {
        r_maj - (a - CREST) / FLANK * (r_maj - r_min)
    } else {
        r_min
    }
}

/// Threaded cylinder along +Z from z = 0 to `length` (right-hand thread). `extra` widens
/// the whole profile (clearance for cutters). `chamfer` = (bottom, top) 45° chamfers down
/// to the minor diameter; `flare` = (bottom, top) widening entries for cutters.
pub fn thread_solid(d: f32, pitch: f32, length: f32, extra: f32, chamfer: (bool, bool), flare: (bool, bool)) -> Mesh {
    let h = 0.866_025_4 * pitch;
    let r_maj = d * 0.5 + extra;
    let r_min = d * 0.5 - 0.625 * h + extra;
    let segs = ((d * 12.0) as usize).clamp(48, 128);
    let per_pitch = 16.0;
    let rings = ((length / pitch * per_pitch).ceil() as usize).max(2);
    let mut m = Mesh::default();
    let radius_at = |theta: f32, z: f32| -> f32 {
        let u = z / pitch - theta / TAU;
        let mut r = profile(u, r_maj, r_min);
        let from_bottom = z;
        let from_top = length - z;
        if chamfer.0 {
            r = r.min(r_min + from_bottom);
        }
        if chamfer.1 {
            r = r.min(r_min + from_top);
        }
        if flare.0 {
            r = r.max(r_maj + (pitch * 0.6 - from_bottom));
        }
        if flare.1 {
            r = r.max(r_maj + (pitch * 0.6 - from_top));
        }
        r.max(0.05)
    };
    for k in 0..=rings {
        let z = length * k as f32 / rings as f32;
        for i in 0..segs {
            let theta = TAU * i as f32 / segs as f32;
            let r = radius_at(theta, z);
            m.positions.push(Vec3::new(r * theta.cos(), r * theta.sin(), z));
        }
    }
    let idx = |k: usize, i: usize| (k * segs + i % segs) as u32;
    for k in 0..rings {
        for i in 0..segs {
            let (a, b, c, e) = (idx(k, i), idx(k, i + 1), idx(k + 1, i + 1), idx(k + 1, i));
            m.triangles.push([a, b, c]);
            m.triangles.push([a, c, e]);
        }
    }
    let bottom = m.positions.len() as u32;
    m.positions.push(Vec3::ZERO);
    let top = m.positions.len() as u32;
    m.positions.push(Vec3::new(0.0, 0.0, length));
    for i in 0..segs {
        m.triangles.push([bottom, idx(0, i + 1), idx(0, i)]);
        m.triangles.push([top, idx(rings, i), idx(rings, i + 1)]);
    }
    m
}

/// Hexagonal prism, `af` across flats, from z = z0 to z0 + h, flats facing ±Y.
fn hex_prism(af: f32, h: f32, z0: f32) -> Mesh {
    let r = af / 3f32.sqrt(); // circumradius
    let mut m = frustum(r, r, h, 6);
    m.translate(Vec3::new(0.0, 0.0, z0 + h * 0.5));
    m
}

fn cylinder(r: f32, h: f32, z0: f32) -> Mesh {
    let mut m = frustum(r, r, h, 96);
    m.translate(Vec3::new(0.0, 0.0, z0 + h * 0.5));
    m
}

/// Build the fastener, standing on the plate (z ≥ 0), centred on the Z axis.
/// Bolts stand on their head so they print without supports.
pub fn fastener_mesh(f: &Fastener) -> Result<Mesh> {
    let s = f.spec();
    let len = f.length.max(s.pitch * 2.0);
    Ok(match f.kind {
        FastenerKind::Rod => thread_solid(s.d, s.pitch, len, 0.0, (true, true), (false, false)),
        FastenerKind::ThreadedHole => {
            // slightly longer than asked so it opens cleanly at the top
            let mut m = thread_solid(s.d, s.pitch, len + 0.4, f.clearance, (false, false), (false, true));
            m.translate(Vec3::new(0.0, 0.0, -0.2));
            m
        }
        FastenerKind::Nut => {
            let body = hex_prism(s.hex, s.nut, 0.0);
            let mut cutter = thread_solid(s.d, s.pitch, s.nut + 2.0, f.clearance, (false, false), (false, false));
            cutter.translate(Vec3::new(0.0, 0.0, -1.0));
            boolean(&body, &cutter, BoolOp::Subtract)?
        }
        FastenerKind::Bolt => {
            let (head, hk) = match f.head {
                Head::Hex => (Some(hex_prism(s.hex, s.head, 0.0)), s.head),
                Head::Socket => {
                    let cap = cylinder(s.sock_d * 0.5, s.sock_k, 0.0);
                    // hex key socket, open at the bottom (the side on the plate)
                    let key = hex_prism(s.sock_s, s.sock_k * 0.6 + 0.01, -0.01);
                    (Some(boolean(&cap, &key, BoolOp::Subtract)?), s.sock_k)
                }
                Head::None => (None, 0.0),
            };
            let overlap = if head.is_some() { 0.5 } else { 0.0 };
            let mut shaft = thread_solid(s.d, s.pitch, len + overlap, 0.0, (head.is_none(), true), (false, false));
            shaft.translate(Vec3::new(0.0, 0.0, hk - overlap));
            match head {
                Some(h) => boolean(&h, &shaft, BoolOp::Union)?,
                None => shaft,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    #[test]
    fn profile_shape() {
        assert_eq!(profile(0.0, 3.0, 2.0), 3.0);
        assert_eq!(profile(0.5, 3.0, 2.0), 2.0);
        assert!((profile(0.25, 3.0, 2.0) - (3.0 - (0.25 - 1.0 / 16.0) / (5.0 / 16.0))).abs() < 1e-6);
    }

    #[test]
    fn all_kinds_are_closed() {
        for kind in [FastenerKind::Bolt, FastenerKind::Nut, FastenerKind::Rod, FastenerKind::ThreadedHole] {
            for head in [Head::Hex, Head::Socket, Head::None] {
                if kind != FastenerKind::Bolt && head != Head::Hex {
                    continue;
                }
                let f = Fastener { kind, head, size: 5, length: 12.0, clearance: 0.2 };
                let m = fastener_mesh(&f).unwrap();
                let r = analyze(&m);
                assert!(r.is_watertight(), "{kind:?} {head:?}: {:?}", r.problems());
                assert!(m.volume() > 0.0);
            }
        }
    }

    #[test]
    fn m6_rod_dimensions() {
        let m = thread_solid(6.0, 1.0, 20.0, 0.0, (false, false), (false, false));
        let b = m.bounds();
        assert!((b.size().x - 6.0).abs() < 0.02, "{}", b.size().x);
        assert!((b.size().z - 20.0).abs() < 1e-4);
        // volume between the minor (≈4.92) and major (6) cylinders
        let v = m.volume() as f32;
        let (vmin, vmaj) = (std::f32::consts::PI * 2.459f32.powi(2) * 20.0, std::f32::consts::PI * 9.0 * 20.0);
        assert!(v > vmin && v < vmaj, "{vmin} < {v} < {vmaj}");
    }
}
