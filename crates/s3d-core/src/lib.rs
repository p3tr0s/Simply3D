//! Simply3D geometry kernel.
//!
//! Everything in here is UI-agnostic so it can be unit-tested and reused
//! from a CLI or another front end.
//!
//! Conventions: millimetres, right-handed, **Z up** (same as 3MF / slicers).

pub mod analysis;
pub mod arrange;
pub mod bvh;
pub mod connectors;
pub mod csg;
pub mod extrude;
pub mod faces;
pub mod fonts;
pub mod history;
pub mod hollow;
pub mod image3d;
pub mod io;
pub mod measure;
pub mod mesh;
pub mod normals;
pub mod organizers;
pub mod paint;
pub mod parts;
pub mod primitives;
pub mod progress;
pub mod repair;
pub mod scene;
pub mod sculpt;
pub mod sdf;
pub mod simplify;
pub mod smooth;
pub mod text;
pub mod texture;
pub mod threads;
pub mod thumbnail;
pub mod wrap;

pub use glam;
pub use mesh::{Aabb, Mesh};
pub use progress::Progress;
pub use scene::{Object, ObjectId, Scene, Transform};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported format: {0}")]
    Unsupported(String),
    #[error("boolean failed: {0}")]
    Boolean(String),
    #[error("cancelled")]
    Cancelled,
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

pub type Result<T> = std::result::Result<T, Error>;
