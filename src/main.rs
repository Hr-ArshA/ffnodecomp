mod graph;
mod preview;
mod timeline;
mod ui;

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
