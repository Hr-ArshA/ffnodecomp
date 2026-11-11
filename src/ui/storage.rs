//! `graph.json` persistence. Native uses the working directory; wasm uses
//! localStorage because the browser has no ambient filesystem.

#[cfg(not(target_arch = "wasm32"))]
pub fn load() -> Option<String> {
    std::fs::read_to_string(crate::ui::GRAPH_FILE).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save(json: &str) -> Result<(), String> {
    std::fs::write(crate::ui::GRAPH_FILE, json).map_err(|error| error.to_string())
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(target_arch = "wasm32")]
pub fn load() -> Option<String> {
    storage()?.get_item(crate::ui::GRAPH_FILE).ok().flatten()
}

#[cfg(target_arch = "wasm32")]
pub fn save(json: &str) -> Result<(), String> {
    let storage = storage().ok_or("localStorage is unavailable")?;
    storage
        .set_item(crate::ui::GRAPH_FILE, json)
        .map_err(|_| "could not write localStorage (quota exceeded?)".to_string())
}
