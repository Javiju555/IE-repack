use eframe::egui;
use ie_gui::App;

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 600.0])
            .with_min_inner_size([640.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("IE Galaxy Repack", opts, Box::new(|_| Ok(Box::new(App::default()))))
}
