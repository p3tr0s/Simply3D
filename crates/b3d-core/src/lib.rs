//! Builder3D geometry kernel.
//!
//! Everything in here is UI-agnostic so it can be unit-tested and reused
//! from a CLI or another front end.
//!
//! Conventions: millimetres, right-handed, **Z up** (same as 3MF / slicers).

pub mod analysis;
pub mod csg;
pub mod fonts;
pub mod history;
pub mod io;
pub mod mesh;
pub mod primitives;
pub mod progress;
pub mod repair;
pub mod scene;
pub mod sdf;
pub mod simplify;
pub mod smooth;
pub mod text;

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
