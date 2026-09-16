// SPDX-FileCopyrightText: 2026 Javiju
//
// SPDX-License-Identifier: MIT

use eframe::egui;
use ie_gui::App;

fn main() -> eframe::Result<()> {
    // Icono propio (balón + rayo + rojigualda), pregenerado a RGBA 256 px
    // para no meter un decoder de PNG en el binario.
    const ICON_RGBA: &[u8] = include_bytes!("../../../assets/icon-256.rgba");
    let icon = egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: 256,
        height: 256,
    };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([860.0, 640.0])
            .with_min_inner_size([640.0, 500.0])
            .with_app_id("ie-repack")
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "IE Repack",
        opts,
        Box::new(|cc| {
            // Tema oscuro FIJO en todas las plataformas, con acento azul
            // acero (contenido, no agresivo) y botones con relleno sólido.
            // El seleccionado va oscuro para que el texto claro contraste.
            let mut visuals = egui::Visuals::dark();
            visuals.selection.bg_fill = egui::Color32::from_rgb(33, 84, 136);
            visuals.selection.stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(130, 180, 225));
            visuals.hyperlink_color = egui::Color32::from_rgb(130, 180, 225);
            visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(54, 54, 58);
            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(72, 72, 78);
            cc.egui_ctx.set_visuals(visuals);
            Ok(Box::new(App::default()))
        }),
    )
}
