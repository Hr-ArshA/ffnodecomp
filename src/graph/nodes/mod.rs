//! Per-node-kind modules that own the FFmpeg mapping for source/layer nodes.
//! `compiler.rs` calls into them so filter strings have one home each.

pub mod image;
pub mod solid_color;
pub mod text;
