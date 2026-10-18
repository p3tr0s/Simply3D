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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FastenerKind {
    Bolt,
    Nut,
    Rod,
    /// A thread-shaped cutter, meant to be used as a hole object.
    ThreadedHole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Head {
    Hex,
    Socket,
    /// Flat-topped cone (90° by default); prints standing on its flat top.
    Countersunk,
    /// Plain cylinder.
    Round,
    /// Cylinder with grip ridges (thumb screw).
    Knurled,
    None,
}

impl Head {
    pub const ALL: [(Head, &'static str); 6] = [
        (Head::Hex, "Hex"),
        (Head::Socket, "Socket"),
        (Head::Countersunk, "Countersunk"),
        (Head::Round, "Round"),
        (Head::Knurled, "Knurled"),
        (Head::None, "None"),
    ];
    /// Whether the head has a hex key socket.
    pub fn has_key(self) -> bool {
        matches!(self, Head::Socket | Head::Countersunk)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NutShape {
    Hex,
    Square,
    Round,
    /// Cylinder with grip ridges (thumb nut, jar lid).
    Knurled,
}

impl NutShape {
    pub const ALL: [(NutShape, &'static str); 4] =
        [(NutShape::Hex, "Hex"), (NutShape::Square, "Square"), (NutShape::Round, "Round"), (NutShape::Knurled, "Knurled")];
}

/// Thread form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThreadProfile {
    /// 60° V thread (ISO metric, UNC / UNF).
    Metric,
    /// 30° trapezoidal (Tr / ACME style): lead screws, vices, clamps.
    Trapezoid,
    /// Square: strong, low friction, for jacks and presses.
    Square,
    /// Buttress (7° / 45°): takes load in one direction.
    Buttress,
    /// Rounded: bottles, jars and lids; prints and turns easily.
    Round,
}

impl ThreadProfile {
    pub const ALL: [(ThreadProfile, &'static str); 5] = [
        (ThreadProfile::Metric, "60° V (metric / UNC)"),
        (ThreadProfile::Trapezoid, "Trapezoidal 30°"),
        (ThreadProfile::Square, "Square"),
        (ThreadProfile::Buttress, "Buttress"),
        (ThreadProfile::Round, "Round (bottles, jars)"),
    ];
    /// Standard thread depth for a pitch.
    pub fn depth(self, pitch: f32) -> f32 {
        match self {
            ThreadProfile::Metric => 0.625 * 0.866_025_4 * pitch,
            ThreadProfile::Trapezoid | ThreadProfile::Square => 0.5 * pitch,
            ThreadProfile::Buttress => 0.6 * pitch,
            ThreadProfile::Round => 0.35 * pitch,
        }
    }
    /// Crest flat, and the widths of the flank before and after it, as fractions of the
    /// pitch (the rest is the root).
    fn shape(self) -> (f32, f32, f32) {
        match self {
            ThreadProfile::Metric => (1.0 / 8.0, 5.0 / 16.0, 5.0 / 16.0),
            ThreadProfile::Trapezoid => (0.366, 0.134, 0.134),
            ThreadProfile::Square => (0.48, 0.02, 0.02),
            ThreadProfile::Buttress => (0.16, 0.074, 0.6),
            ThreadProfile::Round => (0.0, 0.5, 0.5),
        }
    }
}

/// The thread itself: any diameter, pitch, form, number of starts and hand.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Thread {
    /// Major (outside) diameter, mm.
    pub diameter: f32,
    /// Distance between neighbouring crests, mm.
    pub pitch: f32,
    pub profile: ThreadProfile,
    /// Thread depth, mm (0 = the standard depth for the profile).
    pub depth: f32,
    /// Separate threads wound side by side; the nut advances pitch × starts per turn.
    pub starts: u32,
    pub left_hand: bool,
}

impl Thread {
    pub fn metric(d: f32, pitch: f32) -> Thread {
        Thread { diameter: d, pitch, profile: ThreadProfile::Metric, depth: 0.0, starts: 1, left_hand: false }
    }
    /// The depth actually used.
    pub fn depth(&self) -> f32 {
        let d = if self.depth > 0.0 { self.depth } else { self.profile.depth(self.pitch) };
        d.min(self.diameter * 0.45)
    }
    pub fn minor(&self) -> f32 {
        self.diameter - 2.0 * self.depth()
    }
    /// Advance per turn.
    pub fn lead(&self) -> f32 {
        self.pitch * self.starts.max(1) as f32
    }
    fn valid(&self) -> Thread {
        let mut t = *self;
        t.diameter = t.diameter.clamp(0.5, 1000.0);
        t.pitch = t.pitch.clamp(0.1, t.diameter.max(0.2));
        t.starts = t.starts.clamp(1, 12);
        t
    }
}

/// Standard sizes, metric coarse then UNC (inch).
#[derive(Debug, Clone, Copy)]
pub struct Standard {
    pub name: &'static str,
    pub d: f32,
    pub pitch: f32,
    pub hex: f32,
    pub nut: f32,
    pub head: f32,
    pub sock_d: f32,
    pub sock_k: f32,
    pub sock_s: f32,
}

impl From<MetricSize> for Standard {
    fn from(m: MetricSize) -> Self {
        Standard { name: m.name, d: m.d, pitch: m.pitch, hex: m.hex, nut: m.nut, head: m.head, sock_d: m.sock_d, sock_k: m.sock_k, sock_s: m.sock_s }
    }
}

const IN: f32 = 25.4;
macro_rules! unc {
    ($n:expr, $d:expr, $tpi:expr, $hex:expr, $nut:expr, $head:expr, $sd:expr, $sk:expr, $ss:expr) => {
        Standard {
            name: $n,
            d: $d * IN,
            pitch: IN / $tpi,
            hex: $hex * IN,
            nut: $nut * IN,
            head: $head * IN,
            sock_d: $sd * IN,
            sock_k: $sk * IN,
            sock_s: $ss * IN,
        }
    };
}

/// UNC sizes (ASME B1.1, B18.2, B18.3).
pub const UNC: [Standard; 8] = [
    unc!("#4-40 UNC", 0.112, 40.0, 0.25, 0.094, 0.08, 0.183, 0.112, 0.094),
    unc!("#6-32 UNC", 0.138, 32.0, 0.3125, 0.109, 0.095, 0.226, 0.138, 0.109),
    unc!("#8-32 UNC", 0.164, 32.0, 0.34375, 0.125, 0.11, 0.27, 0.164, 0.141),
    unc!("#10-24 UNC", 0.19, 24.0, 0.375, 0.125, 0.12, 0.312, 0.19, 0.156),
    unc!("1/4\"-20 UNC", 0.25, 20.0, 0.4375, 0.21875, 0.15625, 0.375, 0.25, 0.1875),
    unc!("5/16\"-18 UNC", 0.3125, 18.0, 0.5, 0.265625, 0.203125, 0.469, 0.3125, 0.25),
    unc!("3/8\"-16 UNC", 0.375, 16.0, 0.5625, 0.328125, 0.234375, 0.5625, 0.375, 0.3125),
    unc!("1/2\"-13 UNC", 0.5, 13.0, 0.75, 0.4375, 0.3125, 0.75, 0.5, 0.375),
];

/// Every standard size: metric coarse, then UNC.
pub fn standards() -> impl Iterator<Item = Standard> {
    SIZES.iter().map(|&m| Standard::from(m)).chain(UNC.iter().copied())
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fastener {
    pub kind: FastenerKind,
    pub thread: Thread,
    /// Thread length (bolt shaft, rod, hole depth), mm.
    pub length: f32,
    /// Plain, unthreaded part of a bolt between the head and the thread, mm.
    pub shank: f32,
    pub head: Head,
    /// Across flats (hex) or diameter (other heads), mm.
    pub head_size: f32,
    pub head_height: f32,
    /// Hex key size of socket and countersunk heads, across flats, mm.
    pub key: f32,
    pub nut_shape: NutShape,
    /// Across flats (hex, square) or diameter (round, knurled), mm.
    pub nut_size: f32,
    pub nut_height: f32,
    /// Closed top on a nut, mm (0 = open through): a cap or jar lid.
    pub cap: f32,
    /// Radial play added to internal threads (nuts, holes), mm.
    pub clearance: f32,
    /// 45° chamfer at the free ends of bolts and rods.
    pub chamfer: bool,
    /// Hole along the axis of bolts and rods, mm (0 = solid): threaded tubes, jar necks,
    /// hollow bolts for cables.
    pub bore: f32,
}

impl Default for Fastener {
    fn default() -> Self {
        Fastener::standard(FastenerKind::Bolt, 5)
    }
}

impl Fastener {
    /// A standard fastener from [`standards`] (index), 20 mm long.
    pub fn standard(kind: FastenerKind, index: usize) -> Fastener {
        let mut f = Fastener {
            kind,
            thread: Thread::metric(8.0, 1.25),
            length: 20.0,
            shank: 0.0,
            head: Head::Hex,
            head_size: 0.0,
            head_height: 0.0,
            key: 0.0,
            nut_shape: NutShape::Hex,
            nut_size: 0.0,
            nut_height: 0.0,
            cap: 0.0,
            clearance: 0.2,
            chamfer: true,
            bore: 0.0,
        };
        f.apply_standard(index);
        f
    }

    /// A screw-on cap (jar lid) with a coarse rounded two-start thread.
    pub fn jar_lid() -> Fastener {
        let mut f = Fastener::standard(FastenerKind::Nut, 0);
        f.thread = Thread { diameter: 40.0, pitch: 4.0, profile: ThreadProfile::Round, depth: 0.0, starts: 2, left_hand: false };
        f.nut_shape = NutShape::Knurled;
        f.reset_nut();
        f.nut_size = 48.0;
        f.nut_height = 12.0;
        f.cap = 2.0;
        f.clearance = 0.4;
        f
    }
    /// The matching neck: a threaded tube.
    pub fn jar_neck() -> Fastener {
        let mut f = Fastener::jar_lid();
        f.kind = FastenerKind::Rod;
        f.length = 14.0;
        f.bore = 32.0;
        f
    }

    /// Thread and head / nut sizes from a standard size.
    pub fn apply_standard(&mut self, index: usize) {
        let all: Vec<Standard> = standards().collect();
        let s = all[index.min(all.len() - 1)];
        self.thread = Thread::metric(s.d, s.pitch);
        self.reset_head();
        self.reset_nut();
    }

    /// The standard size this thread is, if any.
    pub fn standard_index(&self) -> Option<usize> {
        let t = &self.thread;
        if t.profile != ThreadProfile::Metric || t.starts != 1 || t.left_hand || t.depth > 0.0 {
            return None;
        }
        standards().position(|s| (s.d - t.diameter).abs() < 1e-3 && (s.pitch - t.pitch).abs() < 1e-3)
    }

    fn standard_size(&self) -> Option<Standard> {
        self.standard_index().and_then(|i| standards().nth(i))
    }

    /// Head sizes to suit the thread and head type (standard sizes when the thread is one).
    pub fn reset_head(&mut self) {
        let d = self.thread.diameter;
        let s = self.standard_size();
        let (size, height, key) = match self.head {
            Head::Hex => s.map(|s| (s.hex, s.head, 0.0)).unwrap_or((1.6 * d, 0.65 * d, 0.0)),
            Head::Socket => s.map(|s| (s.sock_d, s.sock_k, s.sock_s)).unwrap_or((1.5 * d, d, 0.8 * d)),
            Head::Countersunk => (2.0 * d, 0.5 * d, s.map(|s| s.sock_s * 0.8).unwrap_or(0.6 * d)),
            Head::Round => (1.8 * d, 0.6 * d, 0.0),
            Head::Knurled => (2.5 * d + 4.0, 0.6 * d + 2.0, 0.0),
            Head::None => (d, 0.0, 0.0),
        };
        (self.head_size, self.head_height, self.key) = (size, height, key);
    }

    /// Nut sizes to suit the thread and nut shape.
    pub fn reset_nut(&mut self) {
        let d = self.thread.diameter;
        let s = self.standard_size();
        let (size, height) = match self.nut_shape {
            NutShape::Hex | NutShape::Square => s.map(|s| (s.hex, s.nut)).unwrap_or((1.6 * d, 0.8 * d)),
            NutShape::Round => (1.8 * d, 0.8 * d),
            NutShape::Knurled => (1.2 * d + 4.0, 0.8 * d + 2.0),
        };
        (self.nut_size, self.nut_height) = (size, height);
    }

    /// Short name of the thread: "M8", "1/4\"-20 UNC", "Tr 10 × 2", "Ø28 P3 2-start LH".
    pub fn thread_name(&self) -> String {
        if let Some(s) = self.standard_size() {
            return s.name.to_string();
        }
        let t = &self.thread;
        let base = match t.profile {
            ThreadProfile::Metric => format!("M{} × {}", fmt(t.diameter), fmt(t.pitch)),
            ThreadProfile::Trapezoid => format!("Tr {} × {}", fmt(t.diameter), fmt(t.pitch)),
            _ => format!("Ø{} P{}", fmt(t.diameter), fmt(t.pitch)),
        };
        let mut out = base;
        if t.starts > 1 {
            out += &format!(" {}-start", t.starts);
        }
        if t.left_hand {
            out += " LH";
        }
        out
    }

    pub fn label(&self) -> String {
        let n = self.thread_name();
        match self.kind {
            FastenerKind::Bolt => format!("{n} × {} bolt", fmt(self.length)),
            FastenerKind::Nut if self.cap > 0.0 => format!("{n} cap"),
            FastenerKind::Rod if self.bore > 0.0 => format!("{n} × {} threaded tube", fmt(self.length)),
            FastenerKind::Nut => format!("{n} nut"),
            FastenerKind::Rod => format!("{n} × {} threaded rod", fmt(self.length)),
            FastenerKind::ThreadedHole => format!("{n} threaded hole"),
        }
    }
}

fn fmt(v: f32) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Radial position on the profile at phase `u` (0 = middle of a crest).
fn profile(p: ThreadProfile, u: f32, r_maj: f32, r_min: f32) -> f32 {
    let a = u - u.round(); // −0.5 … 0.5 from the crest centre
    if p == ThreadProfile::Round {
        return r_min + (r_maj - r_min) * (0.5 + 0.5 * (std::f32::consts::TAU * a).cos());
    }
    let (crest, before, after) = p.shape();
    let half = crest * 0.5;
    let (dist, flank) = if a >= 0.0 { (a, after) } else { (-a, before) };
    if dist <= half {
        r_maj
    } else if dist <= half + flank {
        r_maj - (dist - half) / flank * (r_maj - r_min)
    } else {
        r_min
    }
}

/// Threaded cylinder along +Z from z = 0 to `length`. `extra` widens the whole profile
/// (clearance for cutters). `chamfer` = (bottom, top) 45° chamfers down to the minor
/// diameter; `flare` = (bottom, top) widening entries for cutters.
pub fn threaded(t: &Thread, length: f32, extra: f32, chamfer: (bool, bool), flare: (bool, bool)) -> Mesh {
    let t = t.valid();
    let pitch = t.pitch;
    let r_maj = t.diameter * 0.5 + extra;
    let r_min = (t.diameter * 0.5 - t.depth() + extra).max(0.1);
    let starts = t.starts as f32;
    let hand = if t.left_hand { -1.0 } else { 1.0 };
    // enough samples around for every start, and to follow the helix on big threads
    let around = (t.diameter * 12.0).max(32.0 * starts).max(std::f32::consts::PI * t.diameter / (pitch * 0.25));
    let segs = (around as usize).clamp(48, 512);
    let per_pitch = 20.0;
    let rings = ((length / pitch * per_pitch).ceil() as usize).max(2);
    let mut m = Mesh::default();
    let radius_at = |theta: f32, z: f32| -> f32 {
        let u = z / pitch - hand * starts * theta / TAU;
        let mut r = profile(t.profile, u, r_maj, r_min);
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

/// Right-hand single-start ISO metric thread (see [`threaded`]).
pub fn thread_solid(d: f32, pitch: f32, length: f32, extra: f32, chamfer: (bool, bool), flare: (bool, bool)) -> Mesh {
    threaded(&Thread::metric(d, pitch), length, extra, chamfer, flare)
}

/// Hexagonal prism, `af` across flats, from z = z0 to z0 + h, flats facing ±Y.
fn hex_prism(af: f32, h: f32, z0: f32) -> Mesh {
    let r = af / 3f32.sqrt(); // circumradius
    let mut m = frustum(r, r, h, 6);
    m.translate(Vec3::new(0.0, 0.0, z0 + h * 0.5));
    m
}

fn square_prism(side: f32, h: f32, z0: f32) -> Mesh {
    let r = side / 2f32.sqrt();
    let m = frustum(r, r, h, 4).transformed(&glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4));
    let mut m = m;
    m.translate(Vec3::new(0.0, 0.0, z0 + h * 0.5));
    m
}

fn cylinder(r: f32, h: f32, z0: f32) -> Mesh {
    let mut m = frustum(r, r, h, 96);
    m.translate(Vec3::new(0.0, 0.0, z0 + h * 0.5));
    m
}

/// Cylinder with rounded-off grip ridges around it.
fn knurled(d: f32, h: f32, z0: f32) -> Result<Mesh> {
    let r = d * 0.5;
    let n = ((std::f32::consts::PI * d / 3.0).round() as usize).clamp(10, 120);
    let depth = (d * 0.04).clamp(0.4, 1.2);
    let steps = 8;
    let pts: Vec<glam::Vec2> = (0..n * steps)
        .map(|i| {
            let a = TAU * i as f32 / (n * steps) as f32;
            let w = (a * n as f32).cos() * 0.5 + 0.5; // 1 on a ridge, 0 in a groove
            let rr = r - depth * (1.0 - w.powf(0.6));
            glam::Vec2::new(rr * a.cos(), rr * a.sin())
        })
        .collect();
    let mut m = crate::text::extrude_contours(vec![pts], h)?;
    m.translate(Vec3::new(0.0, 0.0, z0));
    Ok(m)
}

fn nut_body(f: &Fastener, size: f32, h: f32) -> Result<Mesh> {
    Ok(match f.nut_shape {
        NutShape::Hex => hex_prism(size, h, 0.0),
        NutShape::Square => square_prism(size, h, 0.0),
        NutShape::Round => cylinder(size * 0.5, h, 0.0),
        NutShape::Knurled => knurled(size, h, 0.0)?,
    })
}

/// Build the fastener, standing on the plate (z ≥ 0), centred on the Z axis.
/// Bolts stand on their head so they print without supports.
pub fn fastener_mesh(f: &Fastener) -> Result<Mesh> {
    let m = fastener_body(f)?;
    let t = f.thread.valid();
    let bore = f.bore.min(t.minor() - 0.8);
    if bore > 0.2 && matches!(f.kind, FastenerKind::Rod | FastenerKind::Bolt) {
        let h = m.bounds().max.z;
        return boolean(&m, &cylinder(bore * 0.5, h + 2.0, -1.0), BoolOp::Subtract);
    }
    Ok(m)
}

fn fastener_body(f: &Fastener) -> Result<Mesh> {
    let t = f.thread.valid();
    let d = t.diameter;
    let len = f.length.max(t.pitch * 2.0);
    Ok(match f.kind {
        FastenerKind::Rod => threaded(&t, len, 0.0, (f.chamfer, f.chamfer), (false, false)),
        FastenerKind::ThreadedHole => {
            // slightly longer than asked so it opens cleanly at the top
            let mut m = threaded(&t, len + 0.4, f.clearance, (false, false), (false, true));
            m.translate(Vec3::new(0.0, 0.0, -0.2));
            m
        }
        FastenerKind::Nut => {
            // keep a wall around the thread
            let size = f.nut_size.max(d + 2.0 * f.clearance + 1.2);
            let cap = f.cap.max(0.0);
            let h = f.nut_height.max(t.pitch * 2.0) + cap;
            let body = nut_body(f, size, h)?;
            let cutter = if cap <= 0.0 {
                let mut c = threaded(&t, h + 2.0, f.clearance, (false, false), (false, false));
                c.translate(Vec3::new(0.0, 0.0, -1.0));
                c
            } else {
                // a cap prints on its closed face, so the thread starts above it
                let mut c = threaded(&t, h - cap + 1.0, f.clearance, (false, false), (false, false));
                c.translate(Vec3::new(0.0, 0.0, cap));
                c
            };
            boolean(&body, &cutter, BoolOp::Subtract)?
        }
        FastenerKind::Bolt => {
            let hs = f.head_size.max(d * 1.05);
            let hh = if f.head == Head::None { 0.0 } else { f.head_height.max(0.4) };
            let head = match f.head {
                Head::Hex => Some(hex_prism(hs, hh, 0.0)),
                Head::Round => Some(cylinder(hs * 0.5, hh, 0.0)),
                Head::Knurled => Some(knurled(hs, hh, 0.0)?),
                Head::Socket | Head::Countersunk => {
                    let mut cap = if f.head == Head::Socket {
                        cylinder(hs * 0.5, hh, 0.0)
                    } else {
                        let mut c = frustum(hs * 0.5, d * 0.5, hh, 96);
                        c.translate(Vec3::new(0.0, 0.0, hh * 0.5));
                        c
                    };
                    let key = f.key.min(hs * 0.7);
                    if key > 0.2 {
                        // hex key socket, open at the bottom (the side on the plate)
                        let depth = hh * if f.head == Head::Countersunk { 0.75 } else { 0.6 };
                        cap = boolean(&cap, &hex_prism(key, depth + 0.01, -0.01), BoolOp::Subtract)?;
                    }
                    Some(cap)
                }
                Head::None => None,
            };
            let overlap = if head.is_some() { 0.5 } else { 0.0 };
            let shank = f.shank.max(0.0);
            let mut parts = vec![];
            if shank > 0.0 {
                parts.push(cylinder(d * 0.5, shank + overlap + 0.3, hh - overlap));
            }
            let thread_overlap = if shank > 0.0 { 0.3 } else { overlap };
            let mut shaft = threaded(&t, len + thread_overlap, 0.0, (head.is_none() && shank <= 0.0 && f.chamfer, f.chamfer), (false, false));
            shaft.translate(Vec3::new(0.0, 0.0, hh + shank - thread_overlap));
            parts.push(shaft);
            match head {
                Some(h) => crate::csg::boolean_many(&h, &parts, BoolOp::Union)?,
                None if parts.len() == 1 => parts.pop().unwrap(),
                None => {
                    let first = parts.remove(0);
                    crate::csg::boolean_many(&first, &parts, BoolOp::Union)?
                }
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
        let m = ThreadProfile::Metric;
        assert_eq!(profile(m, 0.0, 3.0, 2.0), 3.0);
        assert_eq!(profile(m, 0.5, 3.0, 2.0), 2.0);
        assert!((profile(m, 0.25, 3.0, 2.0) - (3.0 - (0.25 - 1.0 / 16.0) / (5.0 / 16.0))).abs() < 1e-6);
        for (p, _) in ThreadProfile::ALL {
            let (c, a, b) = p.shape();
            assert!(c + a + b <= 1.0 + 1e-6, "{p:?}");
            assert_eq!(profile(p, 0.0, 3.0, 2.0), 3.0);
            assert!((profile(p, 0.5, 3.0, 2.0) - 2.0).abs() < 1e-5, "{p:?}");
        }
    }

    #[test]
    fn all_kinds_are_closed() {
        let mut cases = vec![];
        for kind in [FastenerKind::Bolt, FastenerKind::Nut, FastenerKind::Rod, FastenerKind::ThreadedHole] {
            let mut f = Fastener::standard(kind, 5);
            f.length = 12.0;
            match kind {
                FastenerKind::Bolt => {
                    for (head, _) in Head::ALL {
                        f.head = head;
                        f.reset_head();
                        cases.push(f);
                        let mut g = f;
                        g.shank = 5.0;
                        cases.push(g);
                    }
                }
                FastenerKind::Nut => {
                    for (shape, _) in NutShape::ALL {
                        f.nut_shape = shape;
                        f.reset_nut();
                        cases.push(f);
                        let mut g = f;
                        g.cap = 2.0;
                        cases.push(g);
                    }
                }
                _ => cases.push(f),
            }
        }
        cases.push(Fastener::jar_lid());
        cases.push(Fastener::jar_neck());
        let mut hollow = Fastener::standard(FastenerKind::Bolt, 7);
        hollow.bore = 4.0;
        cases.push(hollow);
        // custom threads: every profile, multi-start, left hand
        for (profile, _) in ThreadProfile::ALL {
            for kind in [FastenerKind::Rod, FastenerKind::Nut] {
                let mut f = Fastener::standard(kind, 0);
                f.thread = Thread { diameter: 14.0, pitch: 3.0, profile, depth: 0.0, starts: 2, left_hand: true };
                f.reset_nut();
                f.length = 15.0;
                cases.push(f);
            }
        }
        for f in cases {
            let m = fastener_mesh(&f).unwrap();
            let r = analyze(&m);
            assert!(r.is_watertight(), "{f:?}: {:?}", r.problems());
            assert_eq!(m.connected_components().len(), 1, "{f:?}");
            assert!(m.volume() > 0.0);
        }
    }

    #[test]
    fn standards_and_names() {
        let n = standards().count();
        assert_eq!(n, SIZES.len() + UNC.len());
        for i in 0..n {
            let f = Fastener::standard(FastenerKind::Bolt, i);
            assert_eq!(f.standard_index(), Some(i));
        }
        let mut f = Fastener::standard(FastenerKind::Bolt, 6);
        assert_eq!(f.thread_name(), "M8");
        f.thread.pitch = 1.0;
        assert_eq!(f.thread_name(), "M8 × 1");
        f.thread = Thread { diameter: 10.0, pitch: 2.0, profile: ThreadProfile::Trapezoid, depth: 0.0, starts: 2, left_hand: true };
        assert_eq!(f.thread_name(), "Tr 10 × 2 2-start LH");
        assert_eq!(f.thread.lead(), 4.0);
        let u = Fastener::standard(FastenerKind::Nut, SIZES.len() + 4);
        assert_eq!(u.thread_name(), "1/4\"-20 UNC");
        assert!((u.thread.diameter - 6.35).abs() < 1e-3 && (u.thread.pitch - 1.27).abs() < 1e-3);
    }

    #[test]
    fn left_hand_mirrors_right_hand() {
        let r = Thread { diameter: 10.0, pitch: 2.0, profile: ThreadProfile::Trapezoid, depth: 0.0, starts: 1, left_hand: false };
        let l = Thread { left_hand: true, ..r };
        let (mr, ml) = (threaded(&r, 10.0, 0.0, (false, false), (false, false)), threaded(&l, 10.0, 0.0, (false, false), (false, false)));
        assert!((mr.volume() - ml.volume()).abs() < 1e-3 * mr.volume());
        // the left-hand thread is the right-hand one mirrored: radius at θ equals radius at −θ
        let rad = |m: &Mesh, i: usize| m.positions[i].truncate().length();
        let n = 48usize.max(((10.0f32 * 12.0).max(32.0).max(std::f32::consts::PI * 10.0 / 0.5)) as usize);
        let mut differs = false;
        for k in [0usize, 7, 13] {
            for j in 0..n {
                let (a, b) = (rad(&ml, k * n + j), rad(&mr, k * n + (n - j) % n));
                assert!((a - b).abs() < 1e-3, "ring {k} seg {j}: {a} vs {b}");
                differs |= (rad(&ml, k * n + j) - rad(&mr, k * n + j)).abs() > 0.1;
            }
        }
        assert!(differs);
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
