//! Is there a newer Simply3D? Asks GitHub for the latest release, in the background, at
//! most once a day at start-up (it can be turned off in Settings), or when asked.

use crate::app::App;
use egui::RichText;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "p3tr0s/Simply3D";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const DAY: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "0.4.2"
    pub version: String,
    /// The release page.
    pub url: String,
    pub notes: String,
}

#[derive(Debug)]
pub enum Check {
    Newer(Release),
    UpToDate,
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    /// A check under way; true when asked for (it then always reports back).
    pending: Option<(Receiver<Check>, bool)>,
    /// A newer release than this one.
    pub available: Option<Release>,
    /// Its window is open.
    pub show: bool,
}

/// "v0.4.2" or "0.4.2" as numbers (anything after a dash, like "-beta", is ignored).
fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|x| x.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(a), Some(b)) if a > b)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Ask GitHub (blocking; run it on a thread).
fn fetch() -> Check {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .try_proxy_from_env(true)
        .user_agent(&format!("Simply3D/{CURRENT}"))
        .build();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let json: serde_json::Value = match agent.get(&url).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => match r.into_json() {
            Ok(j) => j,
            Err(e) => return Check::Failed(format!("unexpected answer from GitHub: {e}")),
        },
        Err(ureq::Error::Status(code, _)) => return Check::Failed(format!("GitHub answered {code}")),
        Err(e) => return Check::Failed(format!("couldn't reach GitHub ({e})")),
    };
    let tag = json["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, CURRENT) {
        return Check::UpToDate;
    }
    Check::Newer(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: json["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
    })
}

/// Open a web page in the browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(url);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

/// Release notes as plain lines for a small window (true = a heading): no markdown marks,
/// no download section.
fn summary(notes: &str, max_lines: usize) -> Vec<(String, bool)> {
    let mut out = vec![];
    for line in notes.lines() {
        let l = line.trim();
        if l.starts_with("### Download") || l.starts_with("## Download") {
            break;
        }
        if l.is_empty() {
            continue;
        }
        let heading = l.starts_with('#');
        let l = l.trim_start_matches('#').trim().replace("**", "").replace('`', "");
        let l = if let Some(rest) = l.strip_prefix("- ") { format!("•  {rest}") } else { l };
        out.push((l, heading));
        if out.len() == max_lines {
            out.push(("…".into(), false));
            break;
        }
    }
    out
}

impl App {
    /// Look for a newer release in the background. `manual`: asked for, so say what was found.
    pub fn check_updates(&mut self, manual: bool) {
        if self.updates.pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
        });
        self.updates.pending = Some((rx, manual));
        if manual {
            self.note("Checking for updates…");
        }
    }

    /// At start-up: at most once a day, if allowed.
    pub fn startup_update_check(&mut self) {
        if self.settings.check_updates && now().saturating_sub(self.settings.last_update_check) >= DAY {
            self.check_updates(false);
        }
    }

    pub fn poll_updates(&mut self, ctx: &egui::Context) {
        let Some((rx, manual)) = &self.updates.pending else { return };
        let manual = *manual;
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(250));
                return;
            }
            Err(_) => Check::Failed("the check stopped".into()),
        };
        self.updates.pending = None;
        match result {
            Check::Newer(r) => {
                self.settings.last_update_check = now();
                // a version you chose to skip stays quiet unless you ask
                if manual || r.version != self.settings.skipped_version {
                    self.updates.show = manual;
                    self.updates.available = Some(r);
                }
            }
            Check::UpToDate => {
                self.settings.last_update_check = now();
                self.updates.available = None;
                if manual {
                    self.info(format!("You have the latest version (v{CURRENT})"));
                }
            }
            Check::Failed(e) => {
                log::warn!("update check: {e}");
                if manual {
                    self.error(format!("Couldn't check for updates: {e}"));
                }
            }
        }
    }
}

/// The "new version" window.
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.updates.show {
        return;
    }
    let Some(r) = app.updates.available.clone() else {
        app.updates.show = false;
        return;
    };
    let mut open = true;
    let mut action = None;
    egui::Window::new(RichText::new(format!("Simply3D v{} is available", r.version)).strong())
        .id(egui::Id::new("update_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.weak(format!("You have v{CURRENT}."));
            ui.add_space(6.0);
            for (l, heading) in summary(&r.notes, 12) {
                if heading {
                    ui.add_space(4.0);
                    ui.label(RichText::new(l).strong());
                } else {
                    ui.add(egui::Label::new(l).wrap());
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Download").strong().color(egui::Color32::WHITE)).fill(crate::ui::ACCENT);
                if ui.add(b).on_hover_text("Opens the release page with the Windows and Linux downloads").clicked() {
                    action = Some(0);
                }
                if ui.button("Later").clicked() {
                    action = Some(1);
                }
                if ui.button("Skip this version").on_hover_text("Don't remind me about this one").clicked() {
                    action = Some(2);
                }
            });
        });
    match action {
        Some(0) => {
            if let Err(e) = open_url(&r.url) {
                app.error(format!("Couldn't open the browser: {e}. The page is {}", r.url));
            }
            app.updates.show = false;
        }
        Some(1) => app.updates.show = false,
        Some(2) => {
            app.settings.skipped_version = r.version.clone();
            app.updates.available = None;
            app.updates.show = false;
        }
        _ => app.updates.show = open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.4.2", "0.4.1"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("v0.4.1", "0.4.1"));
        assert!(!is_newer("v0.4.0", "0.4.1"));
        assert!(!is_newer("nonsense", "0.4.1"));
        assert!(is_newer("v0.5", "0.4.9"));
        assert!(!is_newer("v0.4.1-beta", "0.4.1"));
    }

    #[test]
    fn notes_summary() {
        let notes = "Intro line.\n\n### What's new\n- **Bold** thing with `code`\n\n### Download\n- **Windows:** zip";
        let s = summary(notes, 10);
        let lines: Vec<(&str, bool)> = s.iter().map(|(l, h)| (l.as_str(), *h)).collect();
        assert_eq!(lines, vec![("Intro line.", false), ("What's new", true), ("•  Bold thing with code", false)]);
    }
}
