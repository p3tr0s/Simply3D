//! Toolbar icons: Lucide (ISC licence, see icons/LUCIDE-LICENSE.txt) plus a few of our own
//! (booleans, logo). Drawn white so they can be tinted to any colour.

use egui::{Color32, Image, ImageSource, Vec2};

pub fn source(name: &str) -> ImageSource<'static> {
    match name {
        "align-center-horizontal" => egui::include_image!("../icons/align-center-horizontal.svg"),
        "arrow-down-from-line" => egui::include_image!("../icons/arrow-down-from-line.svg"),
        "arrow-down-to-dot" => egui::include_image!("../icons/arrow-down-to-dot.svg"),
        "arrow-down-to-line" => egui::include_image!("../icons/arrow-down-to-line.svg"),
        "axis-3d" => egui::include_image!("../icons/axis-3d.svg"),
        "blocks" => egui::include_image!("../icons/blocks.svg"),
        "bolt" => egui::include_image!("../icons/bolt.svg"),
        "box" => egui::include_image!("../icons/box.svg"),
        "boxes" => egui::include_image!("../icons/boxes.svg"),
        "brick-wall" => egui::include_image!("../icons/brick-wall.svg"),
        "check" => egui::include_image!("../icons/check.svg"),
        "chevron-down" => egui::include_image!("../icons/chevron-down.svg"),
        "chevron-right" => egui::include_image!("../icons/chevron-right.svg"),
        "circle-check" => egui::include_image!("../icons/circle-check.svg"),
        "circle-dashed" => egui::include_image!("../icons/circle-dashed.svg"),
        "circle-help" => egui::include_image!("../icons/circle-help.svg"),
        "cog" => egui::include_image!("../icons/cog.svg"),
        "combine" => egui::include_image!("../icons/combine.svg"),
        "command" => egui::include_image!("../icons/command.svg"),
        "compass" => egui::include_image!("../icons/compass.svg"),
        "container" => egui::include_image!("../icons/container.svg"),
        "copy" => egui::include_image!("../icons/copy.svg"),
        "copy-plus" => egui::include_image!("../icons/copy-plus.svg"),
        "cylinder" => egui::include_image!("../icons/cylinder.svg"),
        "eraser" => egui::include_image!("../icons/eraser.svg"),
        "external-link" => egui::include_image!("../icons/external-link.svg"),
        "eye" => egui::include_image!("../icons/eye.svg"),
        "eye-off" => egui::include_image!("../icons/eye-off.svg"),
        "file-input" => egui::include_image!("../icons/file-input.svg"),
        "file-plus" => egui::include_image!("../icons/file-plus.svg"),
        "flip-horizontal-2" => egui::include_image!("../icons/flip-horizontal-2.svg"),
        "focus" => egui::include_image!("../icons/focus.svg"),
        "folder-open" => egui::include_image!("../icons/folder-open.svg"),
        "globe" => egui::include_image!("../icons/globe.svg"),
        "grid-2x2" => egui::include_image!("../icons/grid-2x2.svg"),
        "grid-3x3" => egui::include_image!("../icons/grid-3x3.svg"),
        "group" => egui::include_image!("../icons/group.svg"),
        "hand" => egui::include_image!("../icons/hand.svg"),
        "hexagon" => egui::include_image!("../icons/hexagon.svg"),
        "house" => egui::include_image!("../icons/house.svg"),
        "image" => egui::include_image!("../icons/image.svg"),
        "info" => egui::include_image!("../icons/info.svg"),
        "intersect" => egui::include_image!("../icons/intersect.svg"),
        "keyboard" => egui::include_image!("../icons/keyboard.svg"),
        "layers" => egui::include_image!("../icons/layers.svg"),
        "layout-grid" => egui::include_image!("../icons/layout-grid.svg"),
        "list-tree" => egui::include_image!("../icons/list-tree.svg"),
        "locate-fixed" => egui::include_image!("../icons/locate-fixed.svg"),
        "lock" => egui::include_image!("../icons/lock.svg"),
        "lock-open" => egui::include_image!("../icons/lock-open.svg"),
        "logo" => egui::include_image!("../icons/logo.svg"),
        "magnet" => egui::include_image!("../icons/magnet.svg"),
        "maximize" => egui::include_image!("../icons/maximize.svg"),
        "menu" => egui::include_image!("../icons/menu.svg"),
        "mouse-pointer-2" => egui::include_image!("../icons/mouse-pointer-2.svg"),
        "move" => egui::include_image!("../icons/move.svg"),
        "package" => egui::include_image!("../icons/package.svg"),
        "package-plus" => egui::include_image!("../icons/package-plus.svg"),
        "paintbrush" => egui::include_image!("../icons/paintbrush.svg"),
        "panel-right" => egui::include_image!("../icons/panel-right.svg"),
        "printer" => egui::include_image!("../icons/printer.svg"),
        "ratio" => egui::include_image!("../icons/ratio.svg"),
        "redo-2" => egui::include_image!("../icons/redo-2.svg"),
        "rotate-3d" => egui::include_image!("../icons/rotate-3d.svg"),
        "ruler" => egui::include_image!("../icons/ruler.svg"),
        "save" => egui::include_image!("../icons/save.svg"),
        "scale-3d" => egui::include_image!("../icons/scale-3d.svg"),
        "scan" => egui::include_image!("../icons/scan.svg"),
        "scissors" => egui::include_image!("../icons/scissors.svg"),
        "search" => egui::include_image!("../icons/search.svg"),
        "settings" => egui::include_image!("../icons/settings.svg"),
        "shapes" => egui::include_image!("../icons/shapes.svg"),
        "shield-check" => egui::include_image!("../icons/shield-check.svg"),
        "sparkles" => egui::include_image!("../icons/sparkles.svg"),
        "split" => egui::include_image!("../icons/split.svg"),
        "square-dashed-mouse-pointer" => egui::include_image!("../icons/square-dashed-mouse-pointer.svg"),
        "square-split-horizontal" => egui::include_image!("../icons/square-split-horizontal.svg"),
        "squircle" => egui::include_image!("../icons/squircle.svg"),
        "subtract" => egui::include_image!("../icons/subtract.svg"),
        "sun-moon" => egui::include_image!("../icons/sun-moon.svg"),
        "trash-2" => egui::include_image!("../icons/trash-2.svg"),
        "triangle" => egui::include_image!("../icons/triangle.svg"),
        "triangle-alert" => egui::include_image!("../icons/triangle-alert.svg"),
        "type" => egui::include_image!("../icons/type.svg"),
        "undo-2" => egui::include_image!("../icons/undo-2.svg"),
        "ungroup" => egui::include_image!("../icons/ungroup.svg"),
        "union" => egui::include_image!("../icons/union.svg"),
        "waves" => egui::include_image!("../icons/waves.svg"),
        "wrench" => egui::include_image!("../icons/wrench.svg"),
        "x" => egui::include_image!("../icons/x.svg"),
        _ => egui::include_image!("../icons/circle-help.svg"),
    }
}

/// An icon of `size` points, tinted.
pub fn image(name: &str, size: f32, tint: Color32) -> Image<'static> {
    Image::new(source(name)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

/// The app logo in its own colours.
pub fn logo(size: f32) -> Image<'static> {
    Image::new(source("logo")).fit_to_exact_size(Vec2::splat(size))
}

/// The logo as window icon pixels.
pub fn window_icon() -> Option<egui::IconData> {
    let svg = include_bytes!("../icons/logo.svg");
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?;
    let size = 64;
    let mut pix = resvg::tiny_skia::Pixmap::new(size, size)?;
    let s = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
    // tiny-skia stores premultiplied alpha
    let mut rgba = pix.take();
    for p in rgba.chunks_mut(4) {
        let a = p[3] as f32 / 255.0;
        if a > 0.0 {
            for c in &mut p[..3] {
                *c = (*c as f32 / a).round().min(255.0) as u8;
            }
        }
    }
    Some(egui::IconData { rgba, width: size, height: size })
}
