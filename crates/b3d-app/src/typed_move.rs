//! Exact moves: type a distance while dragging (e.g. `25` then Enter, or `10,5` for X and
//! Y), and nudge the selection with the arrow keys.

use crate::app::{App, DragMode, Tool};
use egui::{Event, Key};
use glam::Vec3;
use std::time::{Duration, Instant};

/// Parse what was typed into a move: one number goes along `dir` (snapped to its main
/// axis), `x,y` or `x,y,z` set the axes directly.
pub fn parse_typed(s: &str, dir: Vec3, vertical: bool) -> Option<Vec3> {
    let parts: Vec<f32> = s.split([',', ';', ' ']).filter(|p| !p.is_empty()).map(|p| p.parse::<f32>()).collect::<Result<_, _>>().ok()?;
    match parts[..] {
        [d] if vertical => Some(Vec3::Z * d),
        [d] => Some(main_axis(dir) * d),
        [x, y] => Some(Vec3::new(x, y, 0.0)),
        [x, y, z] => Some(Vec3::new(x, y, z)),
        _ => None,
    }
}

/// The world X or Y axis closest to a horizontal direction (X when there's none).
pub fn main_axis(dir: Vec3) -> Vec3 {
    if dir.x.abs() < 1e-6 && dir.y.abs() < 1e-6 {
        Vec3::X
    } else if dir.x.abs() >= dir.y.abs() {
        Vec3::X * dir.x.signum()
    } else {
        Vec3::Y * dir.y.signum()
    }
}

fn axis_name(v: Vec3) -> &'static str {
    match (v.x.abs() > 0.5, v.y.abs() > 0.5) {
        (true, _) => "X",
        (_, true) => "Y",
        _ => "Z",
    }
}

impl App {
    /// While dragging objects: collect typed numbers, Enter finishes, Esc cancels.
    /// Returns true if it took the keyboard this frame.
    pub fn typed_move_input(&mut self, ctx: &egui::Context) -> bool {
        let DragMode::Move { .. } = &self.drag else {
            self.move_typed.clear();
            return false;
        };
        let (mut enter, mut esc, mut changed) = (false, false, false);
        let mut typed = std::mem::take(&mut self.move_typed);
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Text(t) if t.chars().all(|c| c.is_ascii_digit() || ".,-+; ".contains(c)) => {
                    typed.push_str(t);
                    changed = true;
                    false
                }
                Event::Key { key: Key::Backspace, pressed, .. } => {
                    if *pressed {
                        typed.pop();
                        changed = true;
                    }
                    false
                }
                Event::Key { key: Key::Enter, pressed, .. } => {
                    enter |= *pressed;
                    false
                }
                Event::Key { key: Key::Escape, pressed, .. } => {
                    esc |= *pressed;
                    false
                }
                // the keys behind typed characters must not trigger shortcuts
                Event::Key { key, .. } => !matches!(
                    key,
                    Key::Num0
                        | Key::Num1
                        | Key::Num2
                        | Key::Num3
                        | Key::Num4
                        | Key::Num5
                        | Key::Num6
                        | Key::Num7
                        | Key::Num8
                        | Key::Num9
                        | Key::Minus
                        | Key::Period
                        | Key::Comma
                        | Key::Plus
                        | Key::Semicolon
                        | Key::Space
                ),
                _ => true,
            })
        });
        self.move_typed = typed;
        if esc {
            self.cancel_drag_move();
            return true;
        }
        if changed {
            self.apply_typed_move();
        }
        if enter {
            if !self.move_typed.is_empty() {
                self.apply_typed_move();
            }
            if let Some(l) = self.drag_label.take() {
                self.info(format!("Moved {}", l.replace('|', "")));
            }
            self.move_typed.clear();
            self.drag = DragMode::Finished;
        }
        true
    }

    /// Put the dragged objects where the typed distance says.
    pub fn apply_typed_move(&mut self) {
        let DragMode::Move { start, vertical, .. } = self.drag.clone() else { return };
        let dec = self.settings.decimals;
        let Some(delta) = parse_typed(&self.move_typed, self.move_dir, vertical) else {
            self.drag_label = Some(format!("Type: {}| mm", self.move_typed));
            return;
        };
        for (id, t0) in &start {
            if let Some(ob) = self.scene.get_mut(*id) {
                ob.transform.translation = t0.translation + delta;
            }
        }
        self.snap_guides.clear();
        let along = if self.move_typed.contains([',', ';', ' ']) {
            String::new()
        } else {
            let axis = if vertical { Vec3::Z } else { main_axis(self.move_dir) };
            format!(" along {}", axis_name(axis))
        };
        self.drag_label =
            Some(format!("{}| mm{along}  (ΔX {:+.*}  ΔY {:+.*}  ΔZ {:+.*})", self.move_typed, dec, delta.x, dec, delta.y, dec, delta.z));
    }

    /// Esc while dragging: put everything back.
    pub fn cancel_drag_move(&mut self) {
        if let DragMode::Move { start, .. } = self.drag.clone() {
            for (id, t0) in &start {
                if let Some(ob) = self.scene.get_mut(*id) {
                    ob.transform = *t0;
                }
            }
            self.info("Move cancelled");
        }
        self.move_typed.clear();
        self.drag_label = None;
        self.snap_guides.clear();
        self.drag = DragMode::Finished;
    }

    /// Arrow keys move the selection one grid step (Shift: ×10, Alt: ×0.1) in the
    /// direction they point on screen; Page Up / Down move it up and down.
    pub fn arrow_nudge(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Select || self.selection.is_empty() || !matches!(self.drag, DragMode::None) {
            return;
        }
        let (r, _) = self.settings.camera.right_up();
        let f = self.settings.camera.forward();
        let right = main_axis(r);
        let ahead = main_axis(Vec3::new(f.x, f.y, 0.0));
        let keys = [
            (Key::ArrowRight, right),
            (Key::ArrowLeft, -right),
            (Key::ArrowUp, ahead),
            (Key::ArrowDown, -ahead),
            (Key::PageUp, Vec3::Z),
            (Key::PageDown, -Vec3::Z),
        ];
        let base = if self.settings.snap_enabled { self.settings.snap_move.max(0.01) } else { 1.0 };
        let mut delta = Vec3::ZERO;
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Key { key, pressed: true, modifiers, .. } if !modifiers.command => match keys.iter().find(|k| k.0 == *key) {
                    Some((_, d)) => {
                        let step = base
                            * if modifiers.shift {
                                10.0
                            } else if modifiers.alt {
                                0.1
                            } else {
                                1.0
                            };
                        delta += *d * step;
                        false
                    }
                    None => true,
                },
                _ => true,
            })
        });
        if delta == Vec3::ZERO {
            return;
        }
        // one undo step per burst of nudges
        if self.last_nudge.is_none_or(|t| t.elapsed() > Duration::from_millis(1200)) {
            self.checkpoint("Nudge");
        }
        self.last_nudge = Some(Instant::now());
        let ids: Vec<_> = self.selection.clone();
        for id in ids {
            if let Some(ob) = self.scene.get_mut(id).filter(|o| !o.locked) {
                ob.transform.translation += delta;
            }
        }
        self.dirty = true;
        self.edit_gen += 1;
        let dec = self.settings.decimals;
        self.info(format!("Nudged ΔX {:+.*}  ΔY {:+.*}  ΔZ {:+.*} mm (Shift ×10, Alt ×0.1)", dec, delta.x, dec, delta.y, dec, delta.z));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_distances() {
        assert_eq!(parse_typed("25", Vec3::new(0.2, -3.0, 0.0), false), Some(Vec3::new(0.0, -25.0, 0.0)));
        assert_eq!(parse_typed("-5", Vec3::new(4.0, 1.0, 0.0), false), Some(Vec3::new(-5.0, 0.0, 0.0)));
        assert_eq!(parse_typed("10,5", Vec3::X, false), Some(Vec3::new(10.0, 5.0, 0.0)));
        assert_eq!(parse_typed("1,2,3", Vec3::X, false), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(parse_typed("7.5", Vec3::X, true), Some(Vec3::Z * 7.5));
        assert_eq!(parse_typed("", Vec3::X, false), None);
        assert_eq!(parse_typed("1-", Vec3::X, false), None);
    }
}
