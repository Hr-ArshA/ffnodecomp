//! `ImageNode`: a still image looped into a video stream. It is an FFmpeg
//! *input*, not a filter, so the compiler adds the demuxer options.

/// Options placed before `-i <path>`. Looping keeps the still alive for the
/// whole `duration`, matching what a video source would supply.
pub fn input_options(duration: f32, framerate: u32) -> Vec<String> {
    [
        "-loop".to_string(),
        "1".to_string(),
        "-framerate".to_string(),
        clamp_fps(framerate).to_string(),
        "-t".to_string(),
        format!("{:.3}", clamp_duration(duration)),
    ]
    .to_vec()
}

/// Normalises whatever the image decoder emits into the graph's RGBA format.
pub fn filter() -> &'static str {
    "format=rgba,setsar=1"
}

fn clamp_duration(duration: f32) -> f32 {
    duration.clamp(0.1, 600.0)
}

fn clamp_fps(fps: u32) -> u32 {
    fps.clamp(1, 120)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_options_loop_and_clamp() {
        assert_eq!(
            input_options(3.0, 30),
            ["-loop", "1", "-framerate", "30", "-t", "3.000"]
                .map(String::from)
                .to_vec()
        );
        // Zero fps or duration would make FFmpeg reject the input.
        assert_eq!(input_options(0.0, 0)[3], "1");
        assert_eq!(input_options(0.0, 0)[5], "0.100");
    }
}
