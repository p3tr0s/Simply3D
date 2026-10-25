//! Sketch: draw rectangles, circles, polygons and outlines (straight and curved segments)
//! on the plate or on a face, then extrude or revolve them into a solid: a new object,
//! added onto the object it was drawn on, or cut into it.

use crate::app::{App, JobOutput, Tool};
use crate::expr::Kind;
use crate::preview::BgTask;
use crate::ui::{num, row, ACCENT};
use egui::{Color32, Pos2, RichText, Stroke};
use glam::{Mat4, Vec2, Vec3};
use s3d_core::csg::{self, BoolOp};
use s3d_core::scene::MeshHandle;
use s3d_core::sketch::{self, Item, Shape};
use s3d_core::{Mesh, ObjectId};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Draw {
    Select,
    Rect,
    Circle,
    Polygon,
    Line,
}

impl Draw {
    pub const ALL: [Draw; 5] = [Draw::Select, Draw::Rect, Draw::Circle, Draw::Polygon, Draw::Line];
    fn icon(self) -> &'static str {
        match self {
            Draw::Select => "mouse-pointer-2",
            Draw::Rect => "square",
            Draw::Circle => "circle",
            Draw::Polygon => "hexagon",
            Draw::Line => "pen-line",
        }
    }
    fn tip(self) -> &'static str {
        match self {
            Draw::Select => "Select, move and edit shapes (V)",
            Draw::Rect => "Rectangle: click two corners (R). Corner radius rounds it; half the height makes a slot",
            Draw::Circle => "Circle: click the centre, then the edge (C)",
            Draw::Polygon => "Polygon: click the centre, then a corner (P)",
            Draw::Line => "Outline: click point after point, then the first point again to close it (L). A switches to arcs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Make {
    /// Pushed straight out of the plane.
    Extrude,
    /// Turned around an axis into an upright object.
    Revolve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SketchOp {
    /// A separate object.
    New,
    /// Joined onto the object it was drawn on.
    Add,
    /// Cut into the object it was drawn on.
    Cut,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SketchSettings {
    pub draw: Draw,
    pub make: Make,
    pub op: SketchOp,
    pub depth: f32,
    pub angle: f32,
    pub sides: u32,
    /// Corner radius of new rectangles.
    pub corner: f32,
    /// Grid the pointer snaps to (0 = off).
    pub snap: f32,
    /// Cut all the way through the object.
    pub through: bool,
    /// Outline: the next segment is an arc.
    pub arc: bool,
}

impl Default for SketchSettings {
    fn default() -> Self {
        Self {
            draw: Draw::Rect,
            make: Make::Extrude,
            op: SketchOp::Add,
            depth: 10.0,
            angle: 360.0,
            sides: 6,
            corner: 0.0,
            snap: 1.0,
            through: false,
            arc: false,
        }
    }
}

/// The plane a sketch is drawn on.
#[derive(Debug, Clone, Copy)]
pub struct Plane {
    /// Sketch space → world: X/Y on the plane, Z its normal.
    pub frame: Mat4,
    /// The object it lies on (None: the plate).
    pub target: Option<ObjectId>,
}

impl Plane {
    pub fn to_world(self, p: Vec2) -> Vec3 {
        self.frame.transform_point3(p.extend(0.0))
    }
    pub fn normal(&self) -> Vec3 {
        self.frame.z_axis.truncate()
    }
    /// Where a ray meets the plane, in sketch coordinates.
    pub fn hit(&self, o: Vec3, d: Vec3) -> Option<Vec2> {
        let n = self.normal();
        let den = n.dot(d);
        if den.abs() < 1e-6 {
            return None;
        }
        let t = n.dot(self.frame.w_axis.truncate() - o) / den;
        (t > 0.0).then(|| self.frame.inverse().transform_point3(o + d * t).truncate())
    }
}

/// Sketch axes on a face: on top and bottom faces X follows the world X; on side faces
/// Y points up.
pub fn sketch_frame(point: Vec3, normal: Vec3) -> Mat4 {
    let n = normal.normalize_or(Vec3::Z);
    let (x, y) = if n.z.abs() > 0.9 {
        let x = (Vec3::X - n * n.x).normalize();
        (x, n.cross(x))
    } else {
        let y = (Vec3::Z - n * n.z).normalize();
        (y.cross(n), y)
    };
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), n.extend(0.0), point.extend(1.0))
}

/// A shape being drawn.
#[derive(Debug, Clone)]
pub enum Drawing {
    /// First corner.
    Rect(Vec2),
    /// Centre.
    Circle(Vec2),
    Polygon(Vec2),
    /// Points so far, the curve of each segment between them, and the end of an arc
    /// whose curve is being chosen.
    Line { pts: Vec<Vec2>, bulge: Vec<Option<Vec2>>, arc_end: Option<Vec2> },
}

#[derive(Default)]
pub struct SketchRuntime {
    pub plane: Option<Plane>,
    pub items: Vec<Item>,
    pub selected: Option<usize>,
    pub drawing: Option<Drawing>,
    /// The pointer on the plane (snapped), and whether it sits on a point of a shape.
    pub cursor: Option<(Vec2, bool)>,
    /// Before a plane is chosen: the spot under the pointer (point, normal, object).
    pub hover_start: Option<(Vec3, Vec3, Option<ObjectId>)>,
    /// Sketch units per screen point at the pointer.
    pub px: f32,
    /// The pointer isn't snapped to the grid (Ctrl, or it sits on a point of a shape).
    pub free: bool,
    /// Revolve axis (sketch X); None = the sketch's left edge.
    pub axis: Option<f32>,
    undo: Vec<Vec<Item>>,
    redo: Vec<Vec<Item>>,
    /// A bar field is being changed (one undo step for the whole change).
    editing: bool,
    /// The sketch last turned into a solid, to bring it back.
    last: Option<(Plane, Vec<Item>)>,
    /// Shape being moved: (index, grab point, shape before).
    pub moving: Option<(usize, Vec2, Shape)>,
    preview: Option<(u64, MeshHandle)>,
    task: Option<BgTask<Result<Mesh, String>>>,
    pub error: Option<String>,
    error_key: u64,
}

/// Everything needed to build the solid, away from the UI thread.
#[derive(Debug, Clone)]
struct Recipe {
    items: Vec<Item>,
    make: Make,
    op: SketchOp,
    depth: f32,
    angle: f32,
    axis: f32,
    frame: Mat4,
}

impl Recipe {
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{self:?}").hash(&mut h);
        h.finish()
    }

    /// The new solid (op New) or the piece added / cut away, in world space.
    fn body(&self) -> Result<Mesh, String> {
        match self.make {
            Make::Extrude => {
                let d = self.depth.max(0.05);
                // pieces joined on sink in a little; cuts start a little above the face
                let (z0, z1) = match self.op {
                    SketchOp::New => (0.0, d),
                    SketchOp::Add => (-0.3, d),
                    SketchOp::Cut => (-d, 0.3),
                };
                let mut m = sketch::extrude(&self.items, z1 - z0).map_err(|e| plain(&e))?;
                m.translate(Vec3::Z * z0);
                Ok(m.transformed(&self.frame))
            }
            Make::Revolve => {
                let mut m = sketch::revolve(&self.items, self.axis, self.angle).map_err(|e| plain(&e))?;
                // stands on the plate where the axis was drawn
                let (lo, hi) = bounds(&self.items);
                let spot = self.frame.transform_point3(Vec3::new(self.axis, (lo.y + hi.y) * 0.5, 0.0));
                let b = m.bounds();
                m.translate(Vec3::new(spot.x, spot.y, -b.min.z));
                Ok(m)
            }
        }
    }
}

fn plain(e: &s3d_core::Error) -> String {
    match e {
        s3d_core::Error::Parse(s) => s.clone(),
        e => e.to_string(),
    }
}

/// (min, max) over all shapes.
fn bounds(items: &[Item]) -> (Vec2, Vec2) {
    items.iter().map(|i| i.shape.bounds()).fold((Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)), |(lo, hi), (a, b)| {
        (lo.min(a), hi.max(b))
    })
}

impl App {
    pub fn start_sketch_tool(&mut self) {
        self.tool = Tool::Sketch;
        // the selection's box and labels would only get in the way
        self.selection.clear();
        if self.sketch.items.is_empty() && self.sketch.drawing.is_none() {
            self.sketch.plane = None;
        }
    }

    pub fn toggle_sketch_tool(&mut self) {
        if self.tool == Tool::Sketch {
            self.tool = Tool::Select;
        } else {
            self.start_sketch_tool();
        }
    }

    /// What the solid does, given where the sketch lies.
    pub fn sketch_op(&self) -> SketchOp {
        let on_object = self.sketch.plane.is_some_and(|p| p.target.is_some_and(|t| self.scene.get(t).is_some()));
        if !on_object || self.settings.sketch.make == Make::Revolve {
            SketchOp::New
        } else {
            self.settings.sketch.op
        }
    }

    /// Revolve axis: chosen, or the left edge of the solid shapes.
    pub fn sketch_axis(&self) -> f32 {
        self.sketch.axis.unwrap_or_else(|| {
            let solids: Vec<Item> = self.sketch.items.iter().filter(|i| !i.hole).cloned().collect();
            if solids.is_empty() {
                0.0
            } else {
                bounds(&solids).0.x
            }
        })
    }

    fn sketch_recipe(&self) -> Option<Recipe> {
        let plane = self.sketch.plane?;
        let s = &self.settings.sketch;
        let op = self.sketch_op();
        let mut depth = s.depth;
        if op == SketchOp::Cut && s.through {
            // deep enough to come out of the far side whatever the angle
            depth = plane.target.and_then(|t| self.scene.get(t)).map(|o| o.world_bounds().size().length() * 2.0 + 2.0).unwrap_or(depth);
        }
        Some(Recipe { items: self.sketch.items.clone(), make: s.make, op, depth, angle: s.angle, axis: self.sketch_axis(), frame: plane.frame })
    }

    fn sketch_push_undo(&mut self) {
        let items = self.sketch.items.clone();
        self.sketch.undo.push(items);
        if self.sketch.undo.len() > 200 {
            self.sketch.undo.remove(0);
        }
        self.sketch.redo.clear();
    }

    /// Ctrl+Z while sketching: the last point or shape. False when there's nothing to undo here.
    pub fn sketch_undo(&mut self) -> bool {
        if self.sketch.drawing.is_some() {
            self.sketch_back();
            return true;
        }
        let Some(prev) = self.sketch.undo.pop() else { return false };
        let now = std::mem::replace(&mut self.sketch.items, prev);
        self.sketch.redo.push(now);
        self.sketch.selected = None;
        true
    }

    pub fn sketch_redo(&mut self) -> bool {
        let Some(next) = self.sketch.redo.pop() else { return false };
        let now = std::mem::replace(&mut self.sketch.items, next);
        self.sketch.undo.push(now);
        self.sketch.selected = None;
        true
    }

    /// Choose the plane (the first click). Shapes already drawn come along, centred on
    /// the click.
    pub fn sketch_set_plane(&mut self, point: Vec3, normal: Vec3, target: Option<ObjectId>) {
        let frame = match target {
            // the plate: sketch coordinates are the plate's
            None => Mat4::IDENTITY,
            Some(_) => sketch_frame(point, normal),
        };
        let plane = Plane { frame, target };
        if !self.sketch.items.is_empty() {
            self.sketch_push_undo();
            let (lo, hi) = bounds(&self.sketch.items);
            let at = frame.inverse().transform_point3(point).truncate();
            let d = at - (lo + hi) * 0.5;
            for it in &mut self.sketch.items {
                it.shape.translate(d);
            }
            self.sketch.axis = None;
        }
        self.sketch.plane = Some(plane);
    }

    /// Choose a new plane with the next click.
    pub fn sketch_change_plane(&mut self) {
        self.sketch.plane = None;
        self.sketch.drawing = None;
    }

    /// Snap a point: onto the points of the shapes when near one, else to the grid
    /// (unless `free`, Ctrl held).
    pub fn sketch_snap(&self, p: Vec2, free: bool) -> (Vec2, bool) {
        let tol = self.sketch.px.max(1e-6) * 9.0;
        let mut best: Option<(f32, Vec2)> = None;
        let mut consider = |q: Vec2| {
            let d = q.distance(p);
            if d < tol && best.is_none_or(|b| d < b.0) {
                best = Some((d, q));
            }
        };
        for (i, it) in self.sketch.items.iter().enumerate() {
            if self.sketch.moving.as_ref().is_some_and(|m| m.0 == i) {
                continue;
            }
            for q in it.shape.snap_points() {
                consider(q);
            }
        }
        if let Some(Drawing::Line { pts, .. }) = &self.sketch.drawing {
            for q in pts {
                consider(*q);
            }
        }
        if let Some((_, q)) = best {
            return (q, true);
        }
        let step = self.settings.sketch.snap;
        let mut q = if free || step <= 0.0 { p } else { (p / step).round() * step };
        // lines: straight across or up when nearly so
        if let Some(Drawing::Line { pts, arc_end: None, .. }) = &self.sketch.drawing {
            if let Some(last) = pts.last() {
                if (q.x - last.x).abs() < tol {
                    q.x = last.x;
                }
                if (q.y - last.y).abs() < tol {
                    q.y = last.y;
                }
            }
        }
        (q, false)
    }

    /// The shape under a point: the nearest outline within reach, else the smallest
    /// shape containing it.
    pub fn sketch_shape_at(&self, p: Vec2) -> Option<usize> {
        let tol = self.sketch.px.max(1e-6) * 8.0;
        let near = self
            .sketch
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (i, it.shape.distance_to(p)))
            .filter(|(_, d)| *d < tol)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0);
        near.or_else(|| {
            self.sketch
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.shape.contains(p))
                .min_by(|a, b| {
                    let area = |s: &Shape| {
                        let (lo, hi) = s.bounds();
                        (hi - lo).x * (hi - lo).y
                    };
                    area(&a.1.shape).total_cmp(&area(&b.1.shape))
                })
                .map(|x| x.0)
        })
    }

    /// A radius from a centre to the pointer, in whole grid steps when snapping.
    pub fn sketch_radius(&self, c: Vec2, p: Vec2) -> f32 {
        let r = c.distance(p);
        let step = self.settings.sketch.snap;
        if self.sketch.free || step <= 0.0 {
            r
        } else {
            ((r / step).round() * step).max(step)
        }
    }

    /// Direction from a centre to the pointer, in 15° steps when snapping.
    pub fn sketch_angle(&self, c: Vec2, p: Vec2) -> f32 {
        let d = p - c;
        let a = d.y.atan2(d.x);
        if self.sketch.free || self.settings.sketch.snap <= 0.0 {
            a
        } else {
            let step = 15f32.to_radians();
            (a / step).round() * step
        }
    }

    /// A click on the plane.
    pub fn sketch_click(&mut self, at: Vec2) {
        let s = self.settings.sketch.clone();
        let tiny = self.sketch.px.max(1e-4) * 2.0;
        match (s.draw, self.sketch.drawing.take()) {
            (Draw::Select, _) => self.sketch.selected = self.sketch_shape_at(at),
            (Draw::Rect, None) => self.sketch.drawing = Some(Drawing::Rect(at)),
            (Draw::Rect, Some(Drawing::Rect(a))) => {
                if (at.x - a.x).abs() < tiny || (at.y - a.y).abs() < tiny {
                    self.sketch.drawing = Some(Drawing::Rect(a)); // a double click: keep going
                } else {
                    self.sketch_add(Shape::Rect { a, b: at, radius: s.corner });
                }
            }
            (Draw::Circle, None) => self.sketch.drawing = Some(Drawing::Circle(at)),
            (Draw::Circle, Some(Drawing::Circle(c))) => {
                if c.distance(at) < tiny {
                    self.sketch.drawing = Some(Drawing::Circle(c));
                } else {
                    self.sketch_add(Shape::Circle { c, r: self.sketch_radius(c, at) });
                }
            }
            (Draw::Polygon, None) => self.sketch.drawing = Some(Drawing::Polygon(at)),
            (Draw::Polygon, Some(Drawing::Polygon(c))) => {
                if c.distance(at) < tiny {
                    self.sketch.drawing = Some(Drawing::Polygon(c));
                } else {
                    self.sketch_add(Shape::Polygon { c, r: self.sketch_radius(c, at), sides: s.sides.clamp(3, 64), angle: self.sketch_angle(c, at) });
                }
            }
            (Draw::Line, None) => self.sketch.drawing = Some(Drawing::Line { pts: vec![at], bulge: vec![], arc_end: None }),
            (Draw::Line, Some(Drawing::Line { mut pts, mut bulge, arc_end })) => {
                let first = pts[0];
                let last = *pts.last().unwrap();
                if let Some(end) = arc_end {
                    // this click chooses how the arc bends
                    if at.distance(end) < tiny || at.distance(last) < tiny {
                        self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end });
                        return;
                    }
                    bulge.push(Some(at));
                    if end.distance(first) < tiny {
                        self.sketch_close(pts, bulge);
                    } else {
                        pts.push(end);
                        self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end: None });
                    }
                    return;
                }
                let closing = at.distance(first) < tiny && pts.len() >= 2;
                if at.distance(last) < tiny {
                    // clicked the last point again (a double click): finish
                    if pts.len() >= 3 || bulge.iter().any(Option::is_some) {
                        bulge.push(None);
                        self.sketch_close(pts, bulge);
                    } else {
                        self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end });
                    }
                } else if s.arc {
                    self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end: Some(if closing { first } else { at }) });
                } else if closing {
                    if pts.len() >= 3 || bulge.iter().any(Option::is_some) {
                        bulge.push(None);
                        self.sketch_close(pts, bulge);
                    } else {
                        self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end });
                    }
                } else {
                    bulge.push(None);
                    pts.push(at);
                    self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end });
                }
            }
            // the tool changed while drawing: start over with the new one
            (_, Some(_)) => self.sketch_click(at),
        }
    }

    /// Close the outline being drawn (Enter).
    pub fn sketch_close_line(&mut self) -> bool {
        let Some(Drawing::Line { pts, mut bulge, arc_end: None }) = self.sketch.drawing.clone() else { return false };
        if pts.len() < 3 && !bulge.iter().any(Option::is_some) {
            self.error("An outline needs at least three points");
            return true;
        }
        bulge.push(None);
        self.sketch.drawing = None;
        self.sketch_close(pts, bulge);
        true
    }

    fn sketch_close(&mut self, pts: Vec<Vec2>, bulge: Vec<Option<Vec2>>) {
        let shape = Shape::Outline { pts: pts.clone(), bulge: bulge.clone() };
        if let Some(p) = shape.problem() {
            self.error(format!("This outline can't be used: {p}. Backspace takes points off"));
            let mut b = bulge;
            b.pop();
            self.sketch.drawing = Some(Drawing::Line { pts, bulge: b, arc_end: None });
            return;
        }
        self.sketch_add(shape);
    }

    /// Backspace while drawing: take the last point (or the pending arc) off.
    pub fn sketch_back(&mut self) {
        if let Some(Drawing::Line { mut pts, mut bulge, arc_end }) = self.sketch.drawing.take() {
            if arc_end.is_some() {
                self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end: None });
            } else if pts.len() > 1 {
                pts.pop();
                bulge.pop();
                self.sketch.drawing = Some(Drawing::Line { pts, bulge, arc_end: None });
            }
        }
    }

    fn sketch_add(&mut self, shape: Shape) {
        self.sketch_push_undo();
        // drawn inside a solid shape: it's a hole in it
        let outline = shape.outline();
        let hole = self.sketch.items.iter().any(|it| !it.hole && outline.iter().all(|p| it.shape.contains(*p)));
        if hole {
            self.note(format!("{} inside another shape: it's a hole (H makes it solid)", shape.name()));
        }
        self.sketch.items.push(Item { shape, hole });
        self.sketch.selected = Some(self.sketch.items.len() - 1);
    }

    pub fn sketch_delete_selected(&mut self) -> bool {
        let Some(i) = self.sketch.selected.filter(|i| *i < self.sketch.items.len()) else { return false };
        self.sketch_push_undo();
        self.sketch.items.remove(i);
        self.sketch.selected = None;
        true
    }

    pub fn sketch_toggle_hole(&mut self) {
        if let Some(i) = self.sketch.selected.filter(|i| *i < self.sketch.items.len()) {
            self.sketch_push_undo();
            self.sketch.items[i].hole = !self.sketch.items[i].hole;
        }
    }

    pub fn sketch_begin_move(&mut self, i: usize, at: Vec2) {
        self.sketch_push_undo();
        self.sketch.selected = Some(i);
        self.sketch.moving = Some((i, at, self.sketch.items[i].shape.clone()));
    }

    pub fn sketch_move_to(&mut self, at: Vec2, free: bool) {
        let Some((i, grab, orig)) = self.sketch.moving.clone() else { return };
        let mut d = at - grab;
        let step = self.settings.sketch.snap;
        if !free && step > 0.0 {
            d = (d / step).round() * step;
        }
        let mut s = orig;
        s.translate(d);
        if let Some(it) = self.sketch.items.get_mut(i) {
            it.shape = s;
        }
    }

    /// Clear the sketch (undoable while sketching).
    pub fn sketch_discard(&mut self) {
        if !self.sketch.items.is_empty() {
            self.sketch_push_undo();
        }
        self.sketch.items.clear();
        self.sketch.selected = None;
        self.sketch.drawing = None;
        self.sketch.axis = None;
        self.sketch.plane = None;
    }

    /// Put the last sketch back to work on it again.
    pub fn sketch_restore_last(&mut self) {
        if let Some((plane, items)) = self.sketch.last.take() {
            self.sketch_push_undo();
            self.sketch.plane = Some(plane);
            self.sketch.items = items;
            self.sketch.selected = None;
        }
    }

    pub fn sketch_has_last(&self) -> bool {
        self.sketch.last.is_some()
    }

    /// Keys of the sketch tool (consumed so the app's own shortcuts don't fire).
    pub fn sketch_keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let key = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        for (k, d) in [(Key::V, Draw::Select), (Key::R, Draw::Rect), (Key::C, Draw::Circle), (Key::P, Draw::Polygon), (Key::L, Draw::Line)] {
            if key(k) {
                self.sketch_set_draw(d);
            }
        }
        if key(Key::A) {
            self.settings.sketch.arc = !self.settings.sketch.arc;
            if self.settings.sketch.draw != Draw::Line {
                self.sketch_set_draw(Draw::Line);
                self.settings.sketch.arc = true;
            }
        }
        if key(Key::H) {
            self.sketch_toggle_hole();
        }
        if key(Key::Enter) && !self.sketch_close_line() && self.sketch.drawing.is_none() && !self.sketch.items.is_empty() {
            let ctx = ctx.clone();
            self.sketch_finish(&ctx);
        }
        if key(Key::Delete) || key(Key::Backspace) {
            if self.sketch.drawing.is_some() {
                self.sketch_back();
            } else {
                self.sketch_delete_selected();
            }
        }
        // Esc: stop the shape being drawn, then deselect; the app closes the tool after that
        if self.sketch.drawing.is_some() && key(Key::Escape) {
            self.sketch.drawing = None;
        } else if self.sketch.selected.is_some() && key(Key::Escape) {
            self.sketch.selected = None;
        }
    }

    pub fn sketch_set_draw(&mut self, d: Draw) {
        if self.settings.sketch.draw != d {
            self.sketch.drawing = None;
        }
        self.settings.sketch.draw = d;
    }

    /// Keep the 3D preview up to date (every frame).
    pub fn tick_sketch(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Sketch {
            self.sketch.cursor = None;
            self.sketch.hover_start = None;
            self.sketch.drawing = None;
            self.sketch.moving = None;
            return;
        }
        if self.sketch.editing && !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
            self.sketch.editing = false;
        }
        if let Some(sel) = self.sketch.selected {
            if sel >= self.sketch.items.len() {
                self.sketch.selected = None;
            }
        }
        if let Some(out) = self.sketch.task.as_ref().and_then(|t| t.poll()) {
            let t = self.sketch.task.take().unwrap();
            match out {
                Some(Ok(m)) => {
                    self.sketch.preview = Some((t.key, MeshHandle::new(m)));
                    self.sketch.error = None;
                }
                Some(Err(e)) => {
                    self.sketch.preview = None;
                    self.sketch.error = Some(e);
                    self.sketch.error_key = t.key;
                }
                None => {
                    self.sketch.preview = None;
                    self.sketch.error = Some("this sketch couldn't be made into a solid".into());
                    self.sketch.error_key = t.key;
                }
            }
        }
        let Some(r) = self.sketch_recipe().filter(|r| !r.items.is_empty()) else {
            self.sketch.preview = None;
            self.sketch.error = None;
            self.sketch.task = None;
            return;
        };
        let key = r.key();
        let current =
            self.sketch.preview.as_ref().is_some_and(|p| p.0 == key) || (self.sketch.error.is_some() && self.sketch.error_key == key);
        let running = self.sketch.task.as_ref().is_some_and(|t| t.key == key);
        if !current && !running {
            self.sketch.task = Some(BgTask::spawn(ctx, key, move |_| r.body()));
        }
        if self.sketch.task.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The preview of the solid, if it matches the sketch.
    pub fn sketch_preview(&self) -> Option<&MeshHandle> {
        let key = self.sketch_recipe()?.key();
        self.sketch.preview.as_ref().filter(|p| p.0 == key).map(|p| &p.1)
    }

    /// Extrude / revolve: make the solid (in the background) and close the tool.
    pub fn sketch_finish(&mut self, ctx: &egui::Context) {
        let Some(r) = self.sketch_recipe() else { return };
        if r.items.is_empty() {
            self.error("Sketch: draw a shape first");
            return;
        }
        if let Some(p) = r.items.iter().find_map(|i| i.shape.problem().map(|p| (i.shape.name(), p))) {
            self.error(format!("Sketch: a {} can't be used ({})", p.0.to_lowercase(), p.1));
            return;
        }
        let label = match (r.make, r.op) {
            (Make::Revolve, _) => "Revolve",
            (_, SketchOp::New) => "Extrude",
            (_, SketchOp::Add) => "Extrude onto",
            (_, SketchOp::Cut) => "Cut",
        };
        let target = self.sketch.plane.and_then(|p| p.target).and_then(|t| self.scene.get(t)).cloned();
        match (r.op, target) {
            (SketchOp::New, _) => {
                let name = self.scene.unique_name(if r.make == Make::Revolve { "Revolved" } else { "Extrusion" });
                let color = s3d_core::scene::PALETTE[self.scene.objects.len() % s3d_core::scene::PALETTE.len()];
                self.spawn_job(ctx, format!("{label}…"), move |_| match r.body() {
                    Ok(m) => JobOutput::Replace { label: label.into(), remove: vec![], add: vec![(name, m, color)], message: format!("{label}: done") },
                    Err(e) => JobOutput::Error(format!("{label}: {e}")),
                });
            }
            (op, Some(obj)) => {
                if obj.locked {
                    self.error(format!("{} is locked", obj.name));
                    return;
                }
                let base = obj.world_mesh();
                let (id, name, color) = (obj.id, obj.name.clone(), obj.color);
                let bop = if op == SketchOp::Cut { BoolOp::Subtract } else { BoolOp::Union };
                self.spawn_job(ctx, format!("{label}…"), move |_| {
                    let body = match r.body() {
                        Ok(b) => b,
                        Err(e) => return JobOutput::Error(format!("{label}: {e}")),
                    };
                    match csg::boolean_many(&base, &[body], bop) {
                        Ok(m) if m.is_empty() => JobOutput::Error(format!("{label}: nothing would be left")),
                        Ok(m) => JobOutput::Replace { label: label.into(), remove: vec![id], add: vec![(name, m, color)], message: format!("{label}: done") },
                        Err(e) => JobOutput::Error(format!("{label} failed: {e}")),
                    }
                });
            }
            (_, None) => {
                self.error("Sketch: the object it was drawn on is gone");
                return;
            }
        }
        if self.job.is_some() {
            // kept so it can be brought back (Sketch ▸ Last sketch)
            if let Some(p) = self.sketch.plane {
                self.sketch.last = Some((p, std::mem::take(&mut self.sketch.items)));
            }
            self.sketch.undo.clear();
            self.sketch.redo.clear();
            self.sketch.selected = None;
            self.sketch.drawing = None;
            self.sketch.axis = None;
            self.sketch.plane = None;
            self.tool = Tool::Select;
        }
    }

    fn sketch_edit_begin(&mut self) {
        if !self.sketch.editing {
            self.sketch_push_undo();
            self.sketch.editing = true;
        }
    }
}

/// What to do next, shown at the top of the view.
pub fn hint(app: &App) -> &'static str {
    if app.sketch.plane.is_none() {
        return "Sketch: click the plate or a face to draw on it";
    }
    let drawing = app.sketch.drawing.is_some();
    match app.settings.sketch.draw {
        Draw::Select => "Click a shape to edit it · drag to move it · Del removes it · Enter extrudes",
        Draw::Rect if drawing => "Click the opposite corner · Esc stops",
        Draw::Rect => "Rectangle: click a corner · Enter extrudes",
        Draw::Circle if drawing => "Click the edge · Esc stops",
        Draw::Circle => "Circle: click the centre · Enter extrudes",
        Draw::Polygon if drawing => "Click a corner · Esc stops",
        Draw::Polygon => "Polygon: click the centre · Enter extrudes",
        Draw::Line if matches!(app.sketch.drawing, Some(Drawing::Line { arc_end: Some(_), .. })) => "Click where the arc should bend",
        Draw::Line if drawing => "Click the next point · the first point or Enter closes it · A: arc · Backspace: undo point",
        Draw::Line => "Outline: click the first point · A: arcs",
    }
}

/// Size label for what is being drawn.
pub fn drawing_label(app: &App) -> Option<String> {
    let (p, _) = app.sketch.cursor?;
    let dec = app.settings.decimals.min(2);
    Some(match app.sketch.drawing.as_ref()? {
        Drawing::Rect(a) => format!("{:.*} × {:.*} mm", dec, (p.x - a.x).abs(), dec, (p.y - a.y).abs()),
        Drawing::Circle(c) => format!("Ø {:.*} mm", dec, app.sketch_radius(*c, p) * 2.0),
        Drawing::Polygon(c) => format!("R {:.*} mm", dec, app.sketch_radius(*c, p)),
        Drawing::Line { pts, arc_end: Some(_), .. } if !pts.is_empty() => "click to bend the arc".into(),
        Drawing::Line { pts, .. } => {
            let last = *pts.last()?;
            let d = p - last;
            format!("{:.*} mm  {:.0}°", dec, d.length(), d.y.atan2(d.x).to_degrees())
        }
    })
}

/// The outline the shape being drawn would have with the pointer at `p`.
pub fn drawing_outline(app: &App, p: Vec2) -> Option<(Vec<Vec2>, bool)> {
    let s = &app.settings.sketch;
    Some(match app.sketch.drawing.as_ref()? {
        Drawing::Rect(a) => (Shape::Rect { a: *a, b: p, radius: s.corner }.outline(), true),
        Drawing::Circle(c) => (Shape::Circle { c: *c, r: app.sketch_radius(*c, p).max(1e-3) }.outline(), true),
        Drawing::Polygon(c) => {
            let shape = Shape::Polygon { c: *c, r: app.sketch_radius(*c, p).max(1e-3), sides: s.sides.clamp(3, 64), angle: app.sketch_angle(*c, p) };
            (shape.outline(), true)
        }
        Drawing::Line { pts, bulge, arc_end } => {
            let mut out = vec![pts[0]];
            for i in 0..pts.len() - 1 {
                match bulge.get(i).copied().flatten() {
                    Some(m) => out.extend(sketch::arc_points(pts[i], m, pts[i + 1])),
                    None => out.push(pts[i + 1]),
                }
            }
            let last = *pts.last().unwrap();
            match arc_end {
                Some(e) => out.extend(sketch::arc_points(last, p, *e)),
                None => out.push(p),
            }
            (out, false)
        }
    })
}

const SOLID: Color32 = Color32::from_rgb(90, 170, 255);
const HOLE: Color32 = Color32::from_rgb(255, 120, 95);
const PICKED: Color32 = Color32::from_rgb(255, 214, 70);

/// Shapes, the one being drawn, the snap marker and the revolve axis, drawn over the view.
pub fn paint(app: &App, p: &egui::Painter, to_screen: &dyn Fn(Vec3) -> Option<Pos2>) {
    let Some(plane) = app.sketch.plane else { return };
    let scr = |q: Vec2| to_screen(plane.to_world(q));
    let poly = |pts: &[Vec2], closed: bool| -> Vec<Pos2> {
        let mut v: Vec<Pos2> = pts.iter().filter_map(|q| scr(*q)).collect();
        if closed && !v.is_empty() {
            v.push(v[0]);
        }
        v
    };
    for (i, it) in app.sketch.items.iter().enumerate() {
        let sel = app.sketch.selected == Some(i);
        let col = if sel { PICKED } else if it.hole { HOLE } else { SOLID };
        let pts = poly(&it.shape.outline(), true);
        let w = if sel { 2.5 } else { 2.0 };
        if it.hole {
            p.extend(egui::Shape::dashed_line(&pts, Stroke::new(w, col), 7.0, 4.0));
        } else {
            p.add(egui::Shape::line(pts, Stroke::new(w, col)));
        }
        if sel {
            if let Shape::Outline { pts, .. } = &it.shape {
                for q in pts.iter().filter_map(|q| scr(*q)) {
                    p.circle_filled(q, 3.5, col);
                }
            }
        }
    }
    // the revolve axis
    if app.settings.sketch.make == Make::Revolve && !app.sketch.items.is_empty() {
        let (lo, hi) = bounds(&app.sketch.items);
        let pad = (hi.y - lo.y) * 0.15 + 3.0;
        let x = app.sketch_axis();
        if let (Some(a), Some(b)) = (scr(Vec2::new(x, lo.y - pad)), scr(Vec2::new(x, hi.y + pad))) {
            let c = Color32::from_rgb(255, 170, 60);
            p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.5, c), 10.0, 5.0));
            p.text(b + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, "axis", egui::FontId::proportional(12.0), c);
        }
    }
    // the shape being drawn, and the pointer
    if let Some((c, on_point)) = app.sketch.cursor {
        if let Some((pts, closed)) = drawing_outline(app, c) {
            let v = poly(&pts, closed);
            p.extend(egui::Shape::dashed_line(&v, Stroke::new(2.0, ACCENT), 8.0, 4.0));
            if let Some(Drawing::Line { pts, .. }) = &app.sketch.drawing {
                for q in pts.iter().filter_map(|q| scr(*q)) {
                    p.circle_filled(q, 3.0, ACCENT);
                }
                // the first point closes it
                if let Some(f) = pts.first().and_then(|q| scr(*q)) {
                    p.circle_stroke(f, 6.0, Stroke::new(1.5, ACCENT));
                }
            }
        }
        if let Some(s) = scr(c) {
            if on_point {
                p.rect_filled(egui::Rect::from_center_size(s, egui::vec2(9.0, 9.0)), 1.5, Color32::from_rgb(80, 230, 120));
            } else {
                p.circle_stroke(s, 4.0, Stroke::new(1.5, Color32::WHITE));
            }
        }
    }
}

/// A grid line: ends and opacity.
pub type GridLine = (Vec3, Vec3, f32);
/// A fill triangle: corners, part of a hole, part of the selected shape.
pub type FillTri = ([Vec3; 3], bool, bool);

/// Grid lines around the pointer, and the shapes' fills, in world space.
pub fn plane_geometry(app: &App) -> (Vec<GridLine>, Vec<FillTri>) {
    let mut lines = vec![];
    let mut tris = vec![];
    let Some(plane) = app.sketch.plane else { return (lines, tris) };
    let lift = plane.normal() * (app.sketch.px * 0.5).clamp(0.01, 0.2);
    let w = |q: Vec2| plane.to_world(q) + lift;
    // a fine grid around the pointer (or the shapes), coarser when it would be too dense
    let centre = app.sketch.cursor.map(|c| c.0).or_else(|| (!app.sketch.items.is_empty()).then(|| {
        let (lo, hi) = bounds(&app.sketch.items);
        (lo + hi) * 0.5
    }));
    if let Some(c) = centre {
        let mut g = if app.settings.sketch.snap > 0.0 { app.settings.sketch.snap } else { 5.0 };
        let px = app.sketch.px.max(1e-6);
        while g / px < 9.0 {
            g *= 5.0;
        }
        let n: i32 = 14;
        let (cx, cy) = ((c.x / g).round() * g, (c.y / g).round() * g);
        let half = g * n as f32;
        for i in -n..=n {
            let t = 1.0 - (i.abs() as f32 / n as f32);
            let major = ((cx / g).round() as i64 + i as i64) % 5 == 0;
            let a = t * if major { 0.32 } else { 0.18 };
            let x = cx + i as f32 * g;
            lines.push((w(Vec2::new(x, cy - half)), w(Vec2::new(x, cy + half)), a));
            let major = ((cy / g).round() as i64 + i as i64) % 5 == 0;
            let a = t * if major { 0.32 } else { 0.18 };
            let y = cy + i as f32 * g;
            lines.push((w(Vec2::new(cx - half, y)), w(Vec2::new(cx + half, y)), a));
        }
    }
    for (i, it) in app.sketch.items.iter().enumerate() {
        let o = it.shape.outline();
        for t in sketch::triangulate(&o) {
            tris.push(([w(o[t[0]]), w(o[t[1]]), w(o[t[2]])], it.hole, app.sketch.selected == Some(i)));
        }
    }
    (lines, tris)
}

/// The tool's bar at the bottom of the view.
pub fn bar(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context, width: f32) {
    ui.set_width(width);
    ui.horizontal_wrapped(|ui| {
        row(ui, |ui| {
            ui.label(RichText::new("Sketch").strong());
            ui.add_space(4.0);
            for d in Draw::ALL {
                let on = app.settings.sketch.draw == d;
                let col = if on { Color32::WHITE } else { ui.visuals().text_color() };
                let b = egui::Button::image(crate::icons::image(d.icon(), 16.0, col)).selected(on).min_size(egui::vec2(28.0, 26.0));
                if ui.add(b).on_hover_text(d.tip()).clicked() {
                    app.sketch_set_draw(d);
                }
            }
        });
        // the selected shape's own fields stand in for these
        let sel_kind = app.sketch.selected.and_then(|i| app.sketch.items.get(i)).map(|it| std::mem::discriminant(&it.shape));
        let s = &mut app.settings.sketch;
        let same = |shape: Shape| sel_kind == Some(std::mem::discriminant(&shape));
        match s.draw {
            Draw::Rect if same(Shape::Rect { a: Vec2::ZERO, b: Vec2::ZERO, radius: 0.0 }) => {}
            Draw::Polygon if same(Shape::Polygon { c: Vec2::ZERO, r: 0.0, sides: 3, angle: 0.0 }) => {}
            Draw::Rect => row(ui, |ui| {
                ui.label("Corners");
                ui.add(num(&mut s.corner, Kind::Length).speed(0.1).range(0.0..=1000.0).suffix(" mm"))
                    .on_hover_text("Round the corners of new rectangles (half the height makes a slot)");
            }),
            Draw::Polygon => row(ui, |ui| {
                ui.label("Sides");
                ui.add(egui::DragValue::new(&mut s.sides).range(3..=64).speed(0.1));
            }),
            Draw::Line => row(ui, |ui| {
                ui.toggle_value(&mut s.arc, "Arc").on_hover_text("Curved segments: click where it ends, then where it bends (A)");
            }),
            _ => {}
        }
        row(ui, |ui| {
            ui.label("Snap");
            ui.add(num(&mut s.snap, Kind::Length).speed(0.1).range(0.0..=100.0).suffix(" mm"))
                .on_hover_text("Grid the pointer snaps to (0 = off). Ctrl: don't snap. Corners and centres of shapes always snap");
        });
    });
    // the selected shape: exact sizes
    if let Some(i) = app.sketch.selected.filter(|i| *i < app.sketch.items.len()) {
        ui.horizontal_wrapped(|ui| selected_fields(app, ui, i));
    }
    // what it becomes
    ui.horizontal_wrapped(|ui| {
        let on_object = app.sketch.plane.is_some_and(|p| p.target.is_some_and(|t| app.scene.get(t).is_some()));
        let axis_now = app.sketch_axis();
        let s = &mut app.settings.sketch;
        row(ui, |ui| {
            ui.selectable_value(&mut s.make, Make::Extrude, "Extrude").on_hover_text("Push the shapes straight out of the plane");
            ui.selectable_value(&mut s.make, Make::Revolve, "Revolve")
                .on_hover_text("Turn the shapes around the axis (the dashed line): vases, knobs, wheels. Makes an upright object");
        });
        match s.make {
            Make::Extrude => {
                let op = if on_object { s.op } else { SketchOp::New };
                if !(op == SketchOp::Cut && s.through) {
                    row(ui, |ui| {
                        ui.label(if op == SketchOp::Cut { "Depth" } else { "Height" });
                        ui.add(num(&mut s.depth, Kind::Length).speed(0.1).range(0.05..=2000.0).suffix(" mm"));
                    });
                }
                if on_object {
                    row(ui, |ui| {
                        ui.selectable_value(&mut s.op, SketchOp::Add, "Add").on_hover_text("Joined onto the object it's drawn on");
                        ui.selectable_value(&mut s.op, SketchOp::Cut, "Cut").on_hover_text("Cut into the object it's drawn on");
                        ui.selectable_value(&mut s.op, SketchOp::New, "New object").on_hover_text("A separate object");
                    });
                    if s.op == SketchOp::Cut {
                        ui.checkbox(&mut s.through, "Through").on_hover_text("Cut all the way through");
                    }
                }
            }
            Make::Revolve => {
                row(ui, |ui| {
                    ui.label("Angle");
                    ui.add(num(&mut s.angle, Kind::Angle).speed(1.0).range(1.0..=360.0).suffix("°"));
                });
                let mut x = axis_now;
                let mut reset = false;
                row(ui, |ui| {
                    ui.label("Axis at X");
                    ui.add(num(&mut x, Kind::Length).speed(0.1).suffix(" mm")).on_hover_text("Where the dashed axis line is, across the sketch");
                    if app.sketch.axis.is_some() && ui.small_button("Left edge").clicked() {
                        reset = true;
                    }
                });
                if reset {
                    app.sketch.axis = None;
                } else if (x - axis_now).abs() > 1e-6 {
                    app.sketch.axis = Some(x);
                }
            }
        }
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let w = (width - 250.0).max(120.0);
        ui.allocate_ui_with_layout(egui::vec2(w, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(w);
            ui.set_max_width(w);
            status(app, ui);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let make = app.settings.sketch.make;
            let label = match (make, app.sketch_op()) {
                (Make::Revolve, _) => "Revolve",
                (_, SketchOp::Cut) => "Cut",
                _ => "Extrude",
            };
            let ready = !app.sketch.items.is_empty() && app.sketch.error.is_none() && app.sketch.plane.is_some();
            let b = egui::Button::new(RichText::new(format!("✔ {label}")).strong().color(if ready { Color32::WHITE } else { ui.visuals().weak_text_color() }))
                .fill(if ready { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill });
            if ui.add_enabled(ready, b).on_hover_text("Enter").clicked() {
                app.sketch_finish(ctx);
            }
            if ui.button("Done").on_hover_text("Close the sketch tool; the sketch stays for next time (Esc)").clicked() {
                app.tool = Tool::Select;
            }
            if !app.sketch.items.is_empty() && ui.button("Clear").on_hover_text("Remove every shape (Ctrl+Z brings them back)").clicked() {
                app.sketch_discard();
            }
            if app.sketch.items.is_empty() && app.sketch_has_last() && ui.button("Last sketch").on_hover_text("Bring back the sketch you last used").clicked() {
                app.sketch_restore_last();
            }
        });
    });
}

fn status(app: &mut App, ui: &mut egui::Ui) {
    if let Some(e) = &app.sketch.error {
        ui.add(egui::Label::new(RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(255, 130, 110))).truncate()).on_hover_text(e);
        return;
    }
    let Some(plane) = app.sketch.plane else {
        ui.weak("Click the plate or a face to draw on it");
        return;
    };
    let on = match plane.target.and_then(|t| app.scene.get(t)) {
        Some(o) => format!("On {}", o.name),
        None => "On the plate".to_string(),
    };
    ui.weak(on);
    if ui.small_button("Change").on_hover_text("Pick another face or the plate with the next click").clicked() {
        app.sketch_change_plane();
    }
    if app.sketch.task.is_some() {
        ui.spinner();
    } else if let Some(m) = app.sketch_preview() {
        let s = m.mesh.bounds().size();
        ui.weak(format!("{:.1} × {:.1} × {:.1} mm", s.x, s.y, s.z));
    }
}

/// Exact numbers for the selected shape.
fn selected_fields(app: &mut App, ui: &mut egui::Ui, i: usize) {
    let mut it = app.sketch.items[i].clone();
    let before = it.clone();
    let mut delete = false;
    row(ui, |ui| {
        ui.label(RichText::new(it.shape.name()).strong());
    });
    match &mut it.shape {
        Shape::Rect { a, b, radius } => {
            let (lo, hi) = (a.min(*b), a.max(*b));
            let (mut w, mut h, mut c) = ((hi - lo).x, (hi - lo).y, (lo + hi) * 0.5);
            row(ui, |ui| {
                ui.label("W");
                ui.add(num(&mut w, Kind::Length).speed(0.1).range(0.01..=10000.0).suffix(" mm"));
                ui.label("H");
                ui.add(num(&mut h, Kind::Length).speed(0.1).range(0.01..=10000.0).suffix(" mm"));
            });
            row(ui, |ui| {
                ui.label("Corners");
                ui.add(num(radius, Kind::Length).speed(0.1).range(0.0..=5000.0).suffix(" mm"));
            });
            centre_fields(ui, &mut c);
            *a = c - Vec2::new(w, h) * 0.5;
            *b = c + Vec2::new(w, h) * 0.5;
        }
        Shape::Circle { c, r } => {
            let mut d = *r * 2.0;
            row(ui, |ui| {
                ui.label("Ø");
                ui.add(num(&mut d, Kind::Length).speed(0.1).range(0.02..=10000.0).suffix(" mm"));
            });
            *r = d * 0.5;
            centre_fields(ui, c);
        }
        Shape::Polygon { c, r, sides, angle } => {
            let mut deg = angle.to_degrees();
            row(ui, |ui| {
                ui.label("Sides");
                ui.add(egui::DragValue::new(sides).range(3..=64).speed(0.1));
                ui.label("Radius");
                ui.add(num(r, Kind::Length).speed(0.1).range(0.01..=10000.0).suffix(" mm")).on_hover_text("Centre to a corner");
                ui.label("Turn");
                ui.add(num(&mut deg, Kind::Angle).speed(1.0).suffix("°"));
            });
            *angle = deg.to_radians();
            centre_fields(ui, c);
        }
        Shape::Outline { .. } => {
            let c0 = it.shape.center();
            let mut c = c0;
            centre_fields(ui, &mut c);
            it.shape.translate(c - c0);
        }
    }
    row(ui, |ui| {
        ui.checkbox(&mut it.hole, "Hole").on_hover_text("Cut out of the other shapes (H)");
        if ui.add(egui::Button::image(crate::icons::image("trash-2", 14.0, ui.visuals().text_color()))).on_hover_text("Delete (Del)").clicked() {
            delete = true;
        }
    });
    if delete {
        app.sketch_delete_selected();
    } else if it != before {
        // new shapes of this kind follow
        match &it.shape {
            Shape::Rect { radius, .. } => app.settings.sketch.corner = *radius,
            Shape::Polygon { sides, .. } => app.settings.sketch.sides = *sides,
            _ => {}
        }
        app.sketch_edit_begin();
        app.sketch.items[i] = it;
    }
}

fn centre_fields(ui: &mut egui::Ui, c: &mut Vec2) {
    row(ui, |ui| {
        ui.label("Centre");
        ui.add(num(&mut c.x, Kind::Length).speed(0.1).prefix("X ").suffix(" mm"));
        ui.add(num(&mut c.y, Kind::Length).speed(0.1).prefix("Y ").suffix(" mm"));
    });
}

/// Side panel: the shapes, and how to use the tool.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Sketch");
    if app.sketch.items.is_empty() {
        ui.label("Draw shapes on the plate or a face, then extrude or revolve them into a solid.");
    }
    let mut pick = None;
    for (i, it) in app.sketch.items.iter().enumerate() {
        let (lo, hi) = it.shape.bounds();
        let size = match &it.shape {
            Shape::Circle { r, .. } => format!("Ø {:.1}", r * 2.0),
            _ => format!("{:.1} × {:.1}", (hi - lo).x, (hi - lo).y),
        };
        let text = format!("{} {size}{}", it.shape.name(), if it.hole { "  (hole)" } else { "" });
        if ui.selectable_label(app.sketch.selected == Some(i), text).clicked() {
            pick = Some(i);
        }
    }
    if let Some(i) = pick {
        app.sketch.selected = Some(i);
        app.settings.sketch.draw = Draw::Select;
    }
    ui.add_space(6.0);
    ui.weak("Keys: R rectangle · C circle · P polygon · L outline · A arc · V select · H hole · Del delete · Ctrl+Z undo · Enter extrude");
    ui.weak("Shapes drawn inside another become holes. Overlapping shapes merge.");
    ui.separator();
}
