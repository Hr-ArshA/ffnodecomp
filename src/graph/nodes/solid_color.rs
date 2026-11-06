//! `SolidColorNode`: a generated canvas with no input file.

use crate::graph::node::hex_color;

/// `color` is a `lavfi` source filter, so the whole pipeline can start here.
pub fn filter(color: [u8; 3], width: u32, height: u32, duration: f32, framerate: u32) -> String {
    format!(
        "color=c={}:s={}x{}:d={:.3}:r={},format=rgba,setsar=1",
        hex_color(color).replace('#', "0x"),
        even(width),
        even(height),
        duration.clamp(0.1, 600.0),
        framerate.clamp(1, 120)
    )
}

fn even(value: u32) -> u32 {
    (value.max(2) & !1).min(7680)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_the_lavfi_pattern() {
        assert_eq!(
            filter([0x33, 0x66, 0xCC], 1281, 720, 3.0, 30),
            "color=c=0x3366CC:s=1280x720:d=3.000:r=30,format=rgba,setsar=1"
        );
    }
}
