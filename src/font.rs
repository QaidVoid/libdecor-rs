//! Title text rendering for the CSD titlebar.
//!
//! Discovers a sans-serif font on the host system by walking the
//! standard font directories (including those parsed out of
//! fontconfig). When the discovery process finds nothing usable
//! (sandboxed AppImages, minimal containers, broken `/usr/share/fonts`)
//! the rasteriser falls back to Cantarell, which is bundled into the
//! library itself.
//!
//! Glyphs are rasterised into the titlebar's ARGB buffer using
//! [`ab_glyph`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, FontVec, PxScale, ScaleFont};

/// Cantarell Regular, bundled as the ultimate fallback so text always
/// renders even when no system font directory is reachable.
const BUNDLED_FONT: &[u8] = include_bytes!("../assets/Cantarell-Regular.ttf");

enum LoadedFont {
    Owned(FontVec),
    Bundled(Box<FontRef<'static>>),
}

static FONT: OnceLock<LoadedFont> = OnceLock::new();

fn loaded_font() -> &'static LoadedFont {
    FONT.get_or_init(|| {
        if let Some(path) = std::env::var_os("LIBDECOR_FONT")
            && let Some(font) = try_load_path(Path::new(&path))
        {
            return LoadedFont::Owned(font);
        }
        if let Some(font) = find_system_font() {
            return LoadedFont::Owned(font);
        }
        LoadedFont::Bundled(Box::new(
            FontRef::try_from_slice(BUNDLED_FONT).expect("bundled Cantarell is valid"),
        ))
    })
}

fn try_load_path(path: &Path) -> Option<FontVec> {
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec(bytes).ok()
}

fn find_system_font() -> Option<FontVec> {
    let mut candidates: Vec<(u8, PathBuf)> = Vec::new();
    for dir in font_search_dirs() {
        collect_fonts(&dir, &mut candidates);
    }
    candidates.sort_by_key(|(prio, _)| *prio);
    for (_, path) in candidates {
        if let Some(font) = try_load_path(&path) {
            return Some(font);
        }
    }
    None
}

fn font_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".nix-profile/share/fonts"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_DIRS") {
        for d in xdg.split(':').filter(|s| !s.is_empty()) {
            dirs.push(PathBuf::from(d).join("fonts"));
            dirs.push(PathBuf::from(d).join("X11/fonts"));
        }
    }
    for d in fontconfig_dirs() {
        dirs.push(d);
    }
    dirs
}

fn fontconfig_dirs() -> Vec<PathBuf> {
    let config_dir = std::env::var_os("FONTCONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/fonts"));
    let conf_paths: Vec<PathBuf> = if let Ok(file) = std::env::var("FONTCONFIG_FILE") {
        let p = PathBuf::from(&file);
        if p.is_absolute() {
            vec![p]
        } else {
            vec![config_dir.join(p)]
        }
    } else {
        vec![config_dir.join("fonts.conf"), config_dir.join("conf.d")]
    };

    let mut out = Vec::new();
    for conf in &conf_paths {
        if conf.is_file() {
            extract_dirs(conf, &mut out);
        } else if conf.is_dir()
            && let Ok(rd) = std::fs::read_dir(conf)
        {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("conf") {
                    extract_dirs(&path, &mut out);
                }
            }
        }
    }
    out
}

fn extract_dirs(file: &Path, out: &mut Vec<PathBuf>) {
    let Ok(content) = std::fs::read_to_string(file) else {
        return;
    };
    let mut rest = content.as_str();
    while let Some(start) = rest.find("<dir>") {
        let after = &rest[start + 5..];
        let Some(end) = after.find("</dir>") else {
            break;
        };
        let dir = after[..end].trim();
        let expanded = if let Some(stripped) = dir.strip_prefix('~') {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(stripped.strip_prefix('/').unwrap_or(stripped)))
        } else {
            Some(PathBuf::from(dir))
        };
        if let Some(p) = expanded {
            out.push(p);
        }
        rest = &after[end + 6..];
    }
}

fn collect_fonts(dir: &Path, out: &mut Vec<(u8, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fonts(&path, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc")
        {
            let prio = font_priority(&path);
            if prio < 255 {
                out.push((prio, path));
            }
        }
    }
}

fn font_priority(path: &Path) -> u8 {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let exotic = name.contains('[')
        || name.contains(']')
        || name.contains("adlam")
        || name.contains("arabic")
        || name.contains("hebrew")
        || name.contains("thai")
        || name.contains("cjk")
        || name.contains("korean")
        || name.contains("japanese")
        || name.contains("indic")
        || name.contains("syriac")
        || name.contains("myanmar")
        || name.contains("ethiopic");
    if exotic {
        return 255;
    }

    if name.contains("emoji") || name.contains("color") || name.contains("symbol") {
        return 255;
    }

    let variant = name.contains("bold")
        || name.contains("italic")
        || name.contains("oblique")
        || name.contains("mono")
        || name.contains("condensed")
        || name.contains("light")
        || name.contains("thin")
        || name.contains("black")
        || name.contains("semibold")
        || name.contains("extrabold");

    let base = if name == "cantarell" || name == "cantarell-regular" {
        1
    } else if name == "notosans" || name == "notosans-regular" {
        2
    } else if name == "dejavusans" {
        3
    } else if name.contains("liberation") && name.contains("sans") {
        4
    } else if name.contains("ubuntu") && name.contains("regular") {
        5
    } else if name.contains("noto") && name.contains("sans") {
        6
    } else if name.contains("sans") {
        10
    } else {
        50
    };

    if variant { base + 50 } else { base }
}

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
    match loaded_font() {
        LoadedFont::Owned(f) => draw_with(
            f,
            pixels,
            stride_pixels,
            buffer_height,
            x,
            y_baseline,
            px_size,
            text,
            color,
        ),
        LoadedFont::Bundled(f) => draw_with(
            f.as_ref(),
            pixels,
            stride_pixels,
            buffer_height,
            x,
            y_baseline,
            px_size,
            text,
            color,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_with<F: Font>(
    font: &F,
    pixels: &mut [u32],
    stride_pixels: i32,
    buffer_height: i32,
    x: i32,
    y_baseline: i32,
    px_size: f32,
    text: &str,
    color: u32,
) -> i32 {
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
    match loaded_font() {
        LoadedFont::Owned(f) => measure_with(f, text, px_size),
        LoadedFont::Bundled(f) => measure_with(f.as_ref(), text, px_size),
    }
}

fn measure_with<F: Font>(font: &F, text: &str, px_size: f32) -> i32 {
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
