//! Notices in the top-right corner of the view: what just happened (with Undo while it's
//! still the last step), quick feedback that replaces itself, and errors. They fade out on
//! their own; hovering one keeps it.

use crate::app::App;
use egui::{Align, Align2, Color32, Layout, Rect, RichText};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A result: "Cut into 2 pieces".
    Info,
    /// Quick feedback that replaces the previous one: "Nudged…", "Undid: Move".
    Note,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: Kind,
    born: Instant,
    /// Undo depth right after the step it reports; Undo shows while it's still the last step.
    undo: Option<usize>,
    id: u64,
}

impl Toast {
    fn life(&self) -> Duration {
        Duration::from_secs_f32(match self.kind {
            Kind::Note => 2.5,
            Kind::Info => 7.0 + (self.text.len() as f32 / 40.0).min(4.0),
            Kind::Error => 10.0,
        })
    }
}

#[derive(Default)]
pub struct Toasts {
    pub list: Vec<Toast>,
    next: u64,
    /// When the last undo step was recorded (a result reported right after it can be undone).
    pub last_checkpoint: Option<Instant>,
}

const MAX: usize = 4;
const FADE: f32 = 0.35;

impl Toasts {
    pub fn push(&mut self, kind: Kind, text: String, undo: Option<usize>) {
        // quick feedback replaces itself; the same message again just refreshes
        self.list.retain(|t| !(kind == Kind::Note && t.kind == Kind::Note) && t.text != text);
        self.next += 1;
        self.list.push(Toast { text, kind, born: Instant::now(), undo, id: self.next });
        if self.list.len() > MAX {
            self.list.remove(0);
        }
    }
}

impl App {
    /// Report a result. If an undo step was just recorded, the notice offers Undo.
    pub fn info(&mut self, s: impl Into<String>) {
        let recent = self.toasts.last_checkpoint.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        let undo = (recent && self.history.can_undo()).then(|| self.history.undo_len());
        self.toasts.push(Kind::Info, s.into(), undo);
    }
    /// Quick feedback (nudges, undo/redo, cancelled): replaces the previous one.
    pub fn note(&mut self, s: impl Into<String>) {
        self.toasts.push(Kind::Note, s.into(), None);
    }
    pub fn error(&mut self, s: impl Into<String>) {
        let s = s.into();
        log::error!("{s}");
        self.toasts.push(Kind::Error, s, None);
    }
}

enum Act {
    Undo(u64),
    Close(u64),
}

pub fn show(app: &mut App, ui: &egui::Ui, view: Rect) {
    let now = Instant::now();
    app.toasts.list.retain(|t| now.duration_since(t.born) < t.life());
    if app.toasts.list.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    ctx.request_repaint_after(Duration::from_millis(100));
    let dark = app.settings.dark;
    let undo_len = app.history.undo_len();
    let mut act = None;
    let mut hovered = vec![];
    egui::Area::new(egui::Id::new("toasts"))
        .fixed_pos(view.right_top() + egui::vec2(-12.0, 12.0))
        .pivot(Align2::RIGHT_TOP)
        // under windows and menus, over the view
        .order(egui::Order::Middle)
        .interactable(true)
        .show(&ctx, |ui| {
            // one column, all notices as wide as the widest
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                // newest on top
                for t in app.toasts.list.iter().rev() {
                    let left = t.life().as_secs_f32() - now.duration_since(t.born).as_secs_f32();
                    let fade = (left / FADE).clamp(0.0, 1.0);
                    let (icon, tint) = match t.kind {
                        Kind::Error => (Some("triangle-alert"), Color32::from_rgb(255, 120, 100)),
                        Kind::Info => (Some("circle-check"), if dark { Color32::from_rgb(110, 210, 130) } else { Color32::from_rgb(30, 140, 60) }),
                        Kind::Note => (None, ui.visuals().text_color()),
                    };
                    let r = ui
                        .scope(|ui| {
                            ui.multiply_opacity(fade);
                            egui::Frame::popup(ui.style()).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                                ui.set_max_width(380.0);
                                ui.horizontal(|ui| {
                                    if let Some(i) = icon {
                                        ui.add(crate::icons::image(i, 16.0, tint));
                                    }
                                    let text = RichText::new(&t.text);
                                    let text = if t.kind == Kind::Error { text.color(tint) } else { text };
                                    ui.add(egui::Label::new(text).wrap());
                                    if t.undo == Some(undo_len) && ui.small_button("Undo").on_hover_text("Ctrl+Z").clicked() {
                                        act = Some(Act::Undo(t.id));
                                    }
                                    let x = ui.add(
                                        egui::Button::image(crate::icons::image("x", 12.0, ui.visuals().weak_text_color())).frame(false),
                                    );
                                    if x.on_hover_text("Dismiss").clicked() {
                                        act = Some(Act::Close(t.id));
                                    }
                                });
                            })
                        })
                        .inner
                        .response;
                    if r.contains_pointer() {
                        hovered.push(t.id);
                    }
                    ui.add_space(6.0);
                }
            });
        });
    // a notice you're reading doesn't go away
    for t in app.toasts.list.iter_mut().filter(|t| hovered.contains(&t.id)) {
        if let Some(b) = now.checked_sub(Duration::from_millis(300)) {
            t.born = b;
        }
    }
    match act {
        Some(Act::Undo(id)) => {
            app.toasts.list.retain(|t| t.id != id);
            app.undo();
        }
        Some(Act::Close(id)) => app.toasts.list.retain(|t| t.id != id),
        None => {}
    }
}
