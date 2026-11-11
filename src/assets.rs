//! In-memory asset pool for the web build. Browsers cannot hand absolute
//! filesystem paths to ffmpeg.wasm, so picked files are kept here under the
//! virtual name stored in the node and copied into MEMFS before each exec.

#[cfg(target_arch = "wasm32")]
mod store {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static ASSETS: RefCell<HashMap<String, Vec<u8>>> = RefCell::new(HashMap::new());
        static VERSION: RefCell<u64> = const { RefCell::new(0) };
    }

    pub fn put(path: &str, bytes: Vec<u8>) {
        ASSETS.with(|assets| {
            assets.borrow_mut().insert(path.to_string(), bytes);
        });
        VERSION.with(|v| *v.borrow_mut() += 1);
    }

    pub fn version() -> u64 {
        VERSION.with(|v| *v.borrow())
    }

    pub fn snapshot() -> Vec<(String, Vec<u8>)> {
        ASSETS.with(|assets| {
            assets
                .borrow()
                .iter()
                .map(|(name, bytes)| (name.clone(), bytes.clone()))
                .collect()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod store {
    pub fn put(_path: &str, _bytes: Vec<u8>) {}
    pub fn version() -> u64 {
        0
    }
    pub fn snapshot() -> Vec<(String, Vec<u8>)> {
        Vec::new()
    }
}

pub use store::{put, snapshot, version};
