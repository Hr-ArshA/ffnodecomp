//! `TextNode`: draws title/subtitle text with FFmpeg `drawtext`.
//!
//! With nothing plugged into `video` the node becomes a source: a transparent
//! canvas at the Output resolution with the text on top.

use crate::graph::node::{hex_color, TextPos};

fn position_expr(pos: TextPos, custom_x: &str, custom_y: &str) -> (String, String) {
    match pos {
        TextPos::Center => ("(w-text_w)/2".into(), "(h-text_h)/2".into()),
        TextPos::TopCenter => ("(w-text_w)/2".into(), "24".into()),
        TextPos::BottomCenter => ("(w-text_w)/2".into(), "h-text_h-24".into()),
        TextPos::CustomXY => (
            sanitize_expr(custom_x, "(w-text_w)/2"),
            sanitize_expr(custom_y, "(h-text_h)/2"),
        ),
    }
}

fn sanitize_expr(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return default.to_string();
    }
    let valid = trimmed.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')' | '.' | '_' | ' ')
    });
    if valid {
        trimmed.to_string()
    } else {
        default.to_string()
    }
}

pub struct TextSpec<'a> {
    pub text: &'a str,
    pub font_size: f32,
    pub font_color: [u8; 3],
    pub font_file: &'a str,
    pub position: TextPos,
    pub custom_x: &'a str,
    pub custom_y: &'a str,
    pub width: u32,
    pub height: u32,
    pub duration: f32,
    pub framerate: u32,
    /// True when a video stream is fed in; the node then filters instead of
    /// generating a canvas.
    pub has_video: bool,
}

/// Escapes user text inside single quotes `'...'` for FFmpeg `drawtext`:
/// - `'` -> `''` (doubled quote inside `'...'`)
/// - `\` -> `\\`
/// - `:` -> `\:` (filter option delimiter)
/// - `%` -> `\%`
pub fn escape_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\'' => escaped.push_str("''"),
            '\\' => escaped.push_str("\\\\"),
            ':' => escaped.push_str("\\:"),
            '%' => escaped.push_str("\\%"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub fn filter(spec: &TextSpec<'_>) -> String {
    let (x, y) = position_expr(spec.position, spec.custom_x, spec.custom_y);
    let mut parts = vec![
        format!("text='{}'", escape_text(spec.text)),
        format!("fontsize={:.0}", spec.font_size.clamp(6.0, 400.0)),
        format!(
            "fontcolor={}",
            hex_color(spec.font_color).replace('#', "0x")
        ),
        // No `%{...}` or `\n` expansion: the user's text is literal.
        "expansion=none".to_string(),
        format!("x={x}"),
        format!("y={y}"),
    ];
    if !spec.font_file.trim().is_empty() {
        parts.insert(
            0,
            format!("fontfile='{}'", escape_text(spec.font_file.trim())),
        );
    }
    let drawtext = format!("drawtext={}", parts.join(":"));
    if spec.has_video {
        return format!("{drawtext},format=rgba");
    }

    let (width, height) = (even(spec.width), even(spec.height));
    format!(
        "color=c=black@0.0:s={width}x{height}:d={:.3}:r={},{drawtext},format=rgba,setsar=1",
        clamp_duration(spec.duration),
        clamp_fps(spec.framerate)
    )
}

fn even(value: u32) -> u32 {
    (value.max(2) & !1).min(7680)
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
    fn escapes_special_characters_without_touching_letters() {
        assert_eq!(escape_text("hello"), "hello");
        assert_eq!(escape_text("it's"), "it''s");
        assert_eq!(escape_text("a:b,c[1]"), "a\\:b,c[1]");
        assert_eq!(escape_text("100%"), "100\\%");
        assert_eq!(escape_text("a\\b"), "a\\\\b");
    }

    #[test]
    fn position_expressions_cover_each_mode() {
        assert_eq!(
            position_expr(TextPos::Center, "", ""),
            ("(w-text_w)/2".into(), "(h-text_h)/2".into())
        );
        assert_eq!(position_expr(TextPos::TopCenter, "", "").1, "24");
        assert_eq!(
            position_expr(TextPos::BottomCenter, "", "").1,
            "h-text_h-24"
        );
        assert_eq!(
            position_expr(TextPos::CustomXY, "10", "20"),
            ("10".into(), "20".into())
        );
        // Empty custom values fall back to centering instead of breaking.
        assert_eq!(
            position_expr(TextPos::CustomXY, " ", ""),
            ("(w-text_w)/2".into(), "(h-text_h)/2".into())
        );
        // Disallowed injection characters fall back to default.
        assert_eq!(
            position_expr(TextPos::CustomXY, "10:y=20", "20;[0:v]"),
            ("(w-text_w)/2".into(), "(h-text_h)/2".into())
        );
    }

    #[test]
    fn source_mode_generates_a_transparent_canvas() {
        let filter = filter(&TextSpec {
            text: "hi",
            font_size: 36.0,
            font_color: [255, 255, 255],
            font_file: "",
            position: TextPos::Center,
            custom_x: "",
            custom_y: "",
            width: 1280,
            height: 720,
            duration: 3.0,
            framerate: 30,
            has_video: false,
        });
        assert!(filter.starts_with("color=c=black@0.0:s=1280x720:d=3.000:r=30,drawtext="));
        assert!(filter.contains("fontcolor=0xFFFFFF"));
        assert!(filter.contains("text='hi'"));
        assert!(filter.ends_with("format=rgba,setsar=1"));
    }
}
