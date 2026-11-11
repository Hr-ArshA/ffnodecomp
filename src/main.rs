#[cfg(not(target_arch = "wasm32"))]
use ffnodecomp_web::ui;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "FFNodeComp — node video compositor",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}

/// The browser entry point is `WebHandle` in the library; this binary target
/// only exists so Cargo has a `main` on every platform.
#[cfg(target_arch = "wasm32")]
fn main() {}
