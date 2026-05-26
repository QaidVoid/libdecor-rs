//! Title text rendering for the CSD titlebar.
//!
//! Discovers a sans-serif font on the host system (via `fc-match` or a
//! handful of well-known paths) and rasterises strings into the
//! titlebar's ARGB buffer using [`ab_glyph`].
//!
//! When no usable font is found the titlebar is drawn without a title;
//! the rest of libdecor's UI still works.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

/// Returns a globally cached sans-serif [`FontVec`], or `None` if no
/// font could be loaded.
pub(crate) fn default_font() -> Option<&'static FontVec> {
    FONT.get_or_init(load_default_font).as_ref()
}

fn load_default_font() -> Option<FontVec> {
    if let Some(path) = std::env::var_os("LIBDECOR_FONT")
        && let Ok(bytes) = std::fs::read(&path)
        && let Ok(font) = FontVec::try_from_vec(bytes)
    {
        return Some(font);
    }
    if let Some(path) = fc_match("sans-serif")
        && let Ok(bytes) = std::fs::read(&path)
        && let Ok(font) = FontVec::try_from_vec(bytes)
    {
        return Some(font);
    }
    for candidate in FALLBACK_FONT_PATHS {
        if let Ok(bytes) = std::fs::read(candidate)
            && let Ok(font) = FontVec::try_from_vec(bytes)
        {
            return Some(font);
        }
    }
    None
}

fn fc_match(family: &str) -> Option<PathBuf> {
    let output = Command::new("fc-match")
        .arg("-f")
        .arg("%{file}")
        .arg(format!(":family={family}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

const FALLBACK_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
];

/// Draw `text` into `pixels` (32-bit ARGB, stride = `stride_pixels`)
/// starting at `(x, y)`, in `color`. Glyphs are clipped to the buffer
/// bounds.
///
/// Returns the number of pixels actually advanced (the width of the
/// rendered run).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_text(
    pixels: &mut [u32],
    stride_pixels: i32,
    buffer_height: i32,
    x: i32,
    y_baseline: i32,
    px_size: f32,
    text: &str,
    color: u32,
) -> i32 {
    let Some(font) = default_font() else {
        return 0;
    };
    let scaled = font.as_scaled(PxScale::from(px_size));
    let mut cursor = x as f32;
    let mut prev_glyph_id: Option<ab_glyph::GlyphId> = None;
    let fg_rgb = (
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    );

    for ch in text.chars() {
        let glyph_id = scaled.glyph_id(ch);
        if let Some(prev) = prev_glyph_id {
            cursor += scaled.kern(prev, glyph_id);
        }
        let glyph = glyph_id
            .with_scale_and_position(scaled.scale(), ab_glyph::point(cursor, y_baseline as f32));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let px = bb.min.x as i32 + gx as i32;
                let py = bb.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= stride_pixels || py >= buffer_height {
                    return;
                }
                let idx = (py * stride_pixels + px) as usize;
                if idx >= pixels.len() {
                    return;
                }
                let dst = pixels[idx];
                let bg_r = ((dst >> 16) & 0xff) as u8;
                let bg_g = ((dst >> 8) & 0xff) as u8;
                let bg_b = (dst & 0xff) as u8;
                let alpha = cov.clamp(0.0, 1.0);
                let inv = 1.0 - alpha;
                let r = (fg_rgb.0 as f32 * alpha + bg_r as f32 * inv) as u32;
                let g = (fg_rgb.1 as f32 * alpha + bg_g as f32 * inv) as u32;
                let b = (fg_rgb.2 as f32 * alpha + bg_b as f32 * inv) as u32;
                pixels[idx] = (r << 16) | (g << 8) | b;
            });
        }
        cursor += scaled.h_advance(glyph_id);
        prev_glyph_id = Some(glyph_id);
    }
    (cursor as i32) - x
}

/// Measure the on-screen width of `text` at `px_size` pixels.
#[allow(dead_code)]
pub(crate) fn measure(text: &str, px_size: f32) -> i32 {
    let Some(font) = default_font() else {
        return 0;
    };
    let scaled = font.as_scaled(PxScale::from(px_size));
    let mut total = 0.0_f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            total += scaled.kern(p, id);
        }
        total += scaled.h_advance(id);
        prev = Some(id);
    }
    total as i32
}
