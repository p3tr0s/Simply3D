//! Shared progress reporting + cooperative cancellation for long operations.

use crate::{Error, Result};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inner {
    /// fraction × 1e6
    value: AtomicU32,
    cancel: AtomicBool,
    stage: Mutex<String>,
}

/// Cheap to clone. `range` gives a view that maps 0..1 onto a sub-range of the parent.
#[derive(Clone)]
pub struct Progress {
    inner: Arc<Inner>,
    a: f32,
    b: f32,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    pub fn new() -> Self {
        Progress { inner: Arc::new(Inner::default()), a: 0.0, b: 1.0 }
    }
    /// A child reporter covering `[a, b]` of this one (both 0..1, relative).
    pub fn range(&self, a: f32, b: f32) -> Progress {
        let w = self.b - self.a;
        Progress { inner: self.inner.clone(), a: self.a + w * a, b: self.a + w * b }
    }
    pub fn set(&self, f: f32) {
        let v = self.a + (self.b - self.a) * f.clamp(0.0, 1.0);
        self.inner.value.store((v * 1e6) as u32, Ordering::Relaxed);
    }
    pub fn stage(&self, s: &str) {
        if let Ok(mut g) = self.inner.stage.lock() {
            s.clone_into(&mut g);
        }
    }
    pub fn fraction(&self) -> f32 {
        self.inner.value.load(Ordering::Relaxed) as f32 / 1e6
    }
    pub fn stage_text(&self) -> String {
        self.inner.stage.lock().map(|g| g.clone()).unwrap_or_default()
    }
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancel.load(Ordering::Relaxed)
    }
    /// `Err(Cancelled)` once cancel was requested.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_ranges() {
        let p = Progress::new();
        let half = p.range(0.5, 1.0);
        half.range(0.0, 0.5).set(1.0);
        assert!((p.fraction() - 0.75).abs() < 1e-5);
        p.cancel();
        assert!(half.check().is_err());
    }
}
