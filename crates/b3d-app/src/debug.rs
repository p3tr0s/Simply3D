//! Input diagnostics (F12): shows what the viewport sees under the mouse each frame and
//! logs every change to `builder3d-debug.log` next to the exe.

use crate::app::App;
use std::io::Write;

#[derive(Default)]
pub struct DebugHud {
    pub on: bool,
    log: Option<std::fs::File>,
    last_sig: String,
    pub path: Option<std::path::PathBuf>,
}

impl DebugHud {
    pub fn toggle(&mut self) {
        self.on = !self.on;
        self.last_sig.clear();
        if self.on {
            let path = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("builder3d-debug.log")));
            self.log = path.as_ref().and_then(|p| std::fs::File::create(p).ok());
            self.path = path;
        } else {
            self.log = None;
        }
    }
}

pub struct Probe<'a> {
    pub response: &'a egui::Response,
    pub gizmo_focused: bool,
    pub moved: bool,
    pub total_tris: usize,
}

pub fn frame(app: &mut App, ui: &egui::Ui, p: Probe) {
    if !app.debug.on {
        return;
    }
    let ctx = ui.ctx();
    let r = p.response;
    let latest = ctx.input(|i| i.pointer.latest_pos());
    let (hov_ids, contains_ids, clicked) = ctx.interaction_snapshot(|s| (s.hovered.clone(), s.contains_pointer.clone(), s.clicked));
    let describe = |ids: &egui::IdSet| -> String {
        let mut v: Vec<String> = ids
            .iter()
            .map(|id| {
                if *id == r.id {
                    "VIEWPORT".to_string()
                } else {
                    match ctx.read_response(*id) {
                        Some(o) => format!(
                            "other[{:.0},{:.0} {:.0}x{:.0} {:?}]",
                            o.rect.min.x,
                            o.rect.min.y,
                            o.rect.width(),
                            o.rect.height(),
                            o.layer_id.order
                        ),
                        None => format!("other[{id:?}]"),
                    }
                }
            })
            .collect();
        v.sort();
        v.join(" ")
    };
    let layer = latest.and_then(|pos| ctx.layer_id_at(pos)).map(|l| format!("{:?}", l.order)).unwrap_or("-".into());
    let fresh = latest.and_then(|pos| {
        let v = egui::Rect::from(r.rect);
        if !v.contains(pos) {
            return None;
        }
        let ndc = glam::Vec2::new((pos.x - v.left()) / v.width() * 2.0 - 1.0, 1.0 - (pos.y - v.top()) / v.height() * 2.0);
        let (o, d) = app.settings.camera.ray(ndc, app.viewport_aspect);
        app.scene.pick(o, d).map(|h| h.0)
    });
    let sig = format!(
        "resp.hovered={} contains={} hover_pos={} | egui hovered=[{}] contains=[{}] layer={} | app.hovered={:?} fresh_pick={:?} gizmo_focus={} gizmo_drag={} sel={} drag={:?} tool={:?} tris={}",
        r.hovered(),
        r.contains_pointer(),
        r.hover_pos().is_some(),
        describe(&hov_ids),
        describe(&contains_ids),
        layer,
        app.hovered,
        fresh,
        p.gizmo_focused,
        app.gizmo_dragging,
        app.selection.len(),
        std::mem::discriminant(&app.drag),
        app.tool,
        p.total_tris,
    );
    let click =
        clicked.map(|c| if c == r.id { " CLICK->viewport".to_string() } else { format!(" CLICK->other {c:?}") }).unwrap_or_default();
    if sig != app.debug.last_sig || !click.is_empty() {
        if let Some(f) = app.debug.log.as_mut() {
            let _ = writeln!(
                f,
                "f{} p{} pos={:?} moved={} {}{}",
                ctx.cumulative_frame_nr(),
                ctx.cumulative_pass_nr(),
                latest.map(|p| (p.x, p.y)),
                p.moved,
                sig,
                click
            );
        }
        app.debug.last_sig = sig.clone();
    }
    // on-screen readout
    let text = sig.replace(" | ", "\n").replace(" app.", "\napp.");
    let painter = ui.painter_at(r.rect);
    let galley = painter.layout(text, egui::FontId::monospace(11.0), egui::Color32::YELLOW, r.rect.width() - 20.0);
    let at = r.rect.left_top() + egui::vec2(10.0, 40.0);
    painter.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand(4.0), 3.0, egui::Color32::from_black_alpha(200));
    painter.galley(at, galley, egui::Color32::YELLOW);
}
