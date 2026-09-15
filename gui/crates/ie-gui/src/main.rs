// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

use eframe::egui;
use ie_gui::App;

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 600.0])
            .with_min_inner_size([640.0, 500.0])
            .with_app_id("ie-repack"),
        ..Default::default()
    };
    eframe::run_native("IE Repack", opts, Box::new(|_| Ok(Box::new(App::default()))))
}
