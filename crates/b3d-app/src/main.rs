#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod autosave;
mod camera;
mod commands;
mod expr;
mod organize;
mod preview;
mod render;
mod snap;
mod text_tool;
mod ui;
mod viewport;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,b3d_app=info")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Builder3D")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        multisampling: render::MSAA as u16,
        ..Default::default()
    };
    eframe::run_native("Builder3D", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
