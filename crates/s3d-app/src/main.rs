#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod arrange_tool;
mod autosave;
mod camera;
mod commands;
mod context;
mod debug;
mod expr;
mod faces_tool;
mod holes;
mod icons;
mod image_tool;
mod measure_tool;
mod organize;
mod paint_tool;
mod place_tool;
mod preview;
mod print_dialog;
mod render;
mod ribbon;
mod sculpt_tool;
mod sketch_tool;
mod slicers;
mod snap;
mod text_tool;
mod toasts;
mod texture_tool;
mod typed_move;
mod ui;
mod updates;
mod viewport;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,s3d_app=info")).init();
    // app id: Linux desktops match the window to its menu entry (simply3d.desktop) by it
    let mut vp = egui::ViewportBuilder::default().with_title("Simply3D").with_app_id("simply3d");
    if let Some(icon) = icons::window_icon() {
        vp = vp.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport: vp.with_inner_size([1400.0, 900.0]).with_min_inner_size([800.0, 500.0]).with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        multisampling: render::MSAA as u16,
        ..Default::default()
    };
    eframe::run_native("Simply3D", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
