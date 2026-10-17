//! Periodic autosave to the app's data folder and crash recovery on next start.
//!
//! Windows: %APPDATA%\Simply3D\data\recovery\  (via eframe::storage_dir)

use crate::app::RecoveryInfo;
use s3d_core::io::{self, IoObject};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn dir() -> Option<PathBuf> {
    eframe::storage_dir("Simply3D").map(|d| d.join("recovery"))
}

fn model_path() -> Option<PathBuf> {
    dir().map(|d| d.join("autosave.3mf"))
}

fn meta_path() -> Option<PathBuf> {
    dir().map(|d| d.join("autosave.txt"))
}

/// Write the scene in a background thread (atomically via a temp file).
pub fn write_async(objs: Vec<IoObject>, original: Option<PathBuf>, busy: Arc<AtomicBool>) {
    let (Some(d), Some(model), Some(meta)) = (dir(), model_path(), meta_path()) else { return };
    busy.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let res = (|| -> s3d_core::Result<()> {
            std::fs::create_dir_all(&d)?;
            let tmp = d.join("autosave.tmp.3mf");
            io::save(&tmp, &objs)?;
            std::fs::rename(&tmp, &model)?;
            std::fs::write(&meta, original.map(|p| p.display().to_string()).unwrap_or_default())?;
            Ok(())
        })();
        if let Err(e) = res {
            log::warn!("autosave failed: {e}");
        }
        busy.store(false, Ordering::Relaxed);
    });
}

/// Remove recovery files (after a clean save / exit).
pub fn clear() {
    for p in [model_path(), meta_path()].into_iter().flatten() {
        let _ = std::fs::remove_file(p);
    }
}

/// A leftover autosave from a session that didn't close cleanly.
pub fn find_recovery() -> Option<RecoveryInfo> {
    let file = model_path()?;
    let modified = std::fs::metadata(&file).ok()?.modified().ok()?;
    let age_minutes = modified.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
    let original = meta_path()
        .and_then(|m| std::fs::read_to_string(m).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    Some(RecoveryInfo { file, original, age_minutes })
}

pub fn is_recovery_file(p: &std::path::Path) -> bool {
    model_path().is_some_and(|m| m == p)
}
