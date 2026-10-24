//! Finding the slicers installed on this computer and opening a file in one.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// A program file.
    Exe(PathBuf),
    /// A Flatpak app (Linux), run as `flatpak run <id> <file>`.
    Flatpak(String),
    /// Whatever the system opens 3MF/STL files with.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slicer {
    pub name: String,
    pub launch: Launch,
}

/// (name, program file names on Windows, commands on Linux, Flatpak ids)
const KNOWN: &[(&str, &[&str], &[&str], &[&str])] = &[
    ("Bambu Studio", &["bambu-studio.exe"], &["bambu-studio", "BambuStudio"], &["com.bambulab.BambuStudio"]),
    ("OrcaSlicer", &["orca-slicer.exe"], &["orca-slicer", "OrcaSlicer"], &["io.github.softfever.OrcaSlicer", "io.github.orcaslicer.OrcaSlicer"]),
    ("ElegooSlicer", &["elegoo-slicer.exe", "ElegooSlicer.exe"], &["elegoo-slicer", "ElegooSlicer"], &[]),
    ("PrusaSlicer", &["prusa-slicer.exe"], &["prusa-slicer", "PrusaSlicer"], &["com.prusa3d.PrusaSlicer"]),
    ("UltiMaker Cura", &["UltiMaker-Cura.exe", "Cura.exe"], &["cura", "UltiMaker-Cura"], &["com.ultimaker.cura"]),
    ("Creality Print", &["CrealityPrint.exe", "Creality Print.exe"], &["creality-print", "CrealityPrint"], &[]),
    ("Anycubic Slicer Next", &["AnycubicSlicerNext.exe"], &["AnycubicSlicerNext"], &[]),
    ("QIDI Studio", &["qidi-studio.exe", "QIDIStudio.exe"], &["qidi-studio"], &[]),
    ("Snapmaker Orca", &["snapmaker-orca.exe"], &["snapmaker-orca"], &[]),
    ("SuperSlicer", &["superslicer.exe"], &["superslicer", "SuperSlicer"], &[]),
    ("FlashPrint", &["FlashPrint.exe"], &["FlashPrint"], &[]),
];

/// Every known slicer found on this computer, then the system's default app.
pub fn detect() -> Vec<Slicer> {
    let mut out: Vec<Slicer> = vec![];
    let mut add = |name: &str, launch: Launch| {
        if !out.iter().any(|s| s.name == name) {
            out.push(Slicer { name: name.to_string(), launch });
        }
    };
    if cfg!(windows) {
        // program folders, one or two levels down ("Prusa3D\PrusaSlicer")
        let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .iter()
            .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
            .collect();
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(l).join("Programs"));
        }
        roots.dedup();
        let mut dirs = vec![];
        for r in &roots {
            for d in subdirs(r) {
                dirs.extend(subdirs(&d));
                dirs.push(d);
            }
        }
        for (name, exes, _, _) in KNOWN {
            if let Some(p) = dirs.iter().flat_map(|d| exes.iter().map(move |e| d.join(e))).find(|p| p.is_file()) {
                add(name, Launch::Exe(p));
            }
        }
    } else {
        let path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let flatpak_dirs: Vec<PathBuf> = [Some(PathBuf::from("/var/lib/flatpak/app")), home.as_ref().map(|h| h.join(".local/share/flatpak/app"))]
            .into_iter()
            .flatten()
            .collect();
        for (name, _, cmds, flatpaks) in KNOWN {
            if let Some(p) = path.iter().flat_map(|d| cmds.iter().map(move |c| d.join(c))).find(|p| p.is_file()) {
                add(name, Launch::Exe(p));
            } else if let Some(id) = flatpaks.iter().find(|id| flatpak_dirs.iter().any(|d| d.join(id).is_dir())) {
                add(name, Launch::Flatpak(id.to_string()));
            }
        }
    }
    add("Default app", Launch::Default);
    out
}

fn subdirs(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// Open `file` in the slicer.
pub fn open(s: &Slicer, file: &Path) -> std::io::Result<()> {
    let mut cmd = match &s.launch {
        Launch::Exe(p) => {
            let mut c = Command::new(p);
            c.arg(file);
            if let Some(dir) = p.parent() {
                c.current_dir(dir);
            }
            c
        }
        Launch::Flatpak(id) => {
            let mut c = Command::new("flatpak");
            c.args(["run", id.as_str()]).arg(file);
            c
        }
        Launch::Default => {
            if cfg!(windows) {
                let mut c = Command::new("explorer");
                c.arg(file);
                c
            } else if cfg!(target_os = "macos") {
                let mut c = Command::new("open");
                c.arg(file);
                c
            } else {
                let mut c = Command::new("xdg-open");
                c.arg(file);
                c
            }
        }
    };
    cmd.spawn().map(|_| ())
}

/// A folder for files handed to a slicer: somewhere the slicer can read (a Flatpak slicer
/// can't see /tmp), cleaned of old files.
pub fn handoff_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::temp_dir()
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
    };
    let dir = base.join("Simply3D").join("to-slicer");
    let _ = std::fs::create_dir_all(&dir);
    // files older than a day are from earlier sessions
    if let Ok(r) = std::fs::read_dir(&dir) {
        for e in r.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 86_400);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_is_always_offered() {
        let all = detect();
        assert_eq!(all.last().map(|s| s.launch.clone()), Some(Launch::Default));
        let names: Vec<_> = all.iter().map(|s| &s.name).collect();
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(names, dedup);
    }

    #[test]
    fn finds_a_slicer_on_the_path() {
        if cfg!(windows) {
            return; // Windows looks in the program folders instead
        }
        let dir = std::env::temp_dir().join(format!("s3d_slicer_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("orca-slicer");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        let old = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![dir.clone()];
        paths.extend(std::env::split_paths(&old));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let found = detect();
        std::env::set_var("PATH", old);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found.iter().any(|s| s.name == "OrcaSlicer" && s.launch == Launch::Exe(exe.clone())), "{found:?}");
    }
}
