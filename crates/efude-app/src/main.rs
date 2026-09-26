// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/Efude_sub.png"
    )))
    .ok()
    .map(std::sync::Arc::new);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Efude")
        .with_inner_size([1280., 820.])
        .with_min_inner_size([900., 600.]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let opts = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            // AutoNoVsync prefers Mailbox when the adapter supports it and
            // safely falls back on surfaces that do not expose that mode.
            present_mode: wgpu::PresentMode::AutoNoVsync,
            ..Default::default()
        },
        // Dithering adds noise to every drawn colour; the canvas must show
        // its pixels exactly.
        dithering: false,
        ..Default::default()
    };
    eframe::run_native(
        "Efude",
        opts,
        Box::new(|cc| Ok(Box::new(efude_ui::EfudeApp::from_creation_context(cc)))),
    )
}
