//! Shared application library. Native `main.rs` and the wasm entry point both
//! build the same `ui::App`, so all graph/compiler logic lives here.

pub mod assets;
pub mod graph;
pub mod preview;
pub mod timeline;
pub mod ui;

#[cfg(target_arch = "wasm32")]
mod web_entry {
    use wasm_bindgen::prelude::*;

    /// Handle exposed to `index.html`; owns the eframe web runner.
    #[wasm_bindgen]
    pub struct WebHandle {
        runner: eframe::WebRunner,
    }

    #[wasm_bindgen]
    impl WebHandle {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                runner: eframe::WebRunner::new(),
            }
        }

        /// Start the egui app on the canvas with id `the_canvas_id`.
        #[wasm_bindgen]
        pub async fn start(
            &self,
            canvas: web_sys::HtmlCanvasElement,
        ) -> Result<(), wasm_bindgen::JsValue> {
            self.runner
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|cc| Ok(Box::new(crate::ui::App::new(cc)))),
                )
                .await
        }

        #[wasm_bindgen]
        pub fn destroy(&self) {
            self.runner.destroy();
        }
    }
}
