//! System theme detection and colour palette selection.
//!
//! Determines whether the desktop prefers a dark or light appearance
//! without linking against D-Bus. GNOME's theme preference lives in
//! GSettings / dconf, so we shell out to `gsettings` (a tiny CLI that
//! every GNOME installation has) rather than pulling in a D-Bus library.
//!
//! Detection order:
//!
//! 1. `GTK_THEME` environment variable (explicit override).
//! 2. `gsettings org.gnome.desktop.interface color-scheme` (GNOME 42+).
//! 3. `gsettings org.gnome.desktop.interface gtk-theme` (older GNOME).
//! 4. GTK 3/4 `settings.ini` files (non-GNOME GTK desktops).
//! 5. Defaults to **dark** when nothing is found.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Colour palette for CSD rendering.
///
/// All values are ARGB8888 (`0xAARRGGBB`).
pub(crate) struct Palette {
    pub titlebar_active: u32,
    pub titlebar_inactive: u32,
    pub title_fg_active: u32,
    pub title_fg_inactive: u32,
    pub button_hover_active: u32,
    pub button_hover_inactive: u32,
    pub button_pressed_active: u32,
    pub button_pressed_inactive: u32,
    pub close_pressed_active: u32,
    pub close_pressed_inactive: u32,
    pub shadow_peak_alpha: u32,
}

/// Catppuccin Mocha (dark) palette.
pub(crate) const DARK: Palette = Palette {
    titlebar_active: 0xff1e_1e2e,
    titlebar_inactive: 0xff18_1825,
    title_fg_active: 0xffcd_d6f4,
    title_fg_inactive: 0xff6c_7086,
    button_hover_active: 0xff31_3244,
    button_hover_inactive: 0xff24_2436,
    button_pressed_active: 0xff45_475a,
    button_pressed_inactive: 0xff31_3144,
    close_pressed_active: 0xffe0_6c75,
    close_pressed_inactive: 0xff8b_3c3c,
    shadow_peak_alpha: 60,
};

/// Catppuccin Latte (light) palette.
pub(crate) const LIGHT: Palette = Palette {
    titlebar_active: 0xffdc_e0e8,
    titlebar_inactive: 0xffcc_d0da,
    title_fg_active: 0xff4c_4f69,
    title_fg_inactive: 0xff7c_7f93,
    button_hover_active: 0xffcc_d0da,
    button_hover_inactive: 0xffbc_c0cc,
    button_pressed_active: 0xffac_b5c0,
    button_pressed_inactive: 0xffbc_c0cc,
    close_pressed_active: 0xffbc_4c55,
    close_pressed_inactive: 0xffa3_3c45,
    shadow_peak_alpha: 40,
};

/// Return the palette that best matches the current desktop theme.
///
/// Detection runs once per process; subsequent calls return the cached
/// result so repeated window creation does not re-spawn `gsettings`.
pub(crate) fn palette() -> &'static Palette {
    static IS_DARK: OnceLock<bool> = OnceLock::new();
    if *IS_DARK.get_or_init(prefer_dark) {
        &DARK
    } else {
        &LIGHT
    }
}

fn prefer_dark() -> bool {
    if let Ok(theme) = env::var("GTK_THEME") {
        return theme.to_ascii_lowercase().contains("dark");
    }
    if let Some(dark) = gnome_color_scheme() {
        return dark;
    }
    if let Some(dark) = gtk_ini_prefer_dark() {
        return dark;
    }
    true
}

/// Ask `gsettings` for GNOME's colour-scheme preference, falling back to
/// the older `gtk-theme` key if `color-scheme` is unset (pre-GNOME 42).
fn gnome_color_scheme() -> Option<bool> {
    gsettings_dark("color-scheme").or_else(|| gsettings_dark("gtk-theme"))
}

/// Read a single `org.gnome.desktop.interface` key via `gsettings` and
/// classify its value as dark/light. Returns `None` when `gsettings` is
/// unavailable, the schema/key is missing, or the value is empty.
fn gsettings_dark(key: &str) -> Option<bool> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let val = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches('\'')
        .to_ascii_lowercase();
    if val.is_empty() || val == "default" {
        return Some(false);
    }
    Some(val.contains("dark"))
}

/// Walk GTK 3/4 `settings.ini` for an explicit dark-theme preference.
/// Returns `None` when no relevant key is present.
fn gtk_ini_prefer_dark() -> Option<bool> {
    for config_dir in xdg_config_dirs() {
        for ver in &["gtk-4.0", "gtk-3.0"] {
            let path = config_dir.join(ver).join("settings.ini");
            if let Ok(content) = fs::read_to_string(&path)
                && let Some(dark) = parse_gtk_ini(&content)
            {
                return Some(dark);
            }
        }
    }
    None
}

/// `gtk-application-prefer-dark-theme=1` is authoritative; otherwise
/// infer from `gtk-theme-name` containing "dark".
fn parse_gtk_ini(content: &str) -> Option<bool> {
    let mut from_theme_name = None;
    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().to_ascii_lowercase();
        if key == "gtk-application-prefer-dark-theme" {
            return Some(value == "1");
        }
        if key == "gtk-theme-name" {
            from_theme_name = Some(value.contains("dark"));
        }
    }
    from_theme_name
}

/// Return the XDG config directories in priority order:
/// `$XDG_CONFIG_HOME` (default `~/.config`) first, then each colon-
/// separated entry in `$XDG_CONFIG_DIRS` (default `/etc/xdg`).
fn xdg_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    dirs.push(
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|h| h.join(".config"))
                    .unwrap_or_else(|| PathBuf::from(".config"))
            }),
    );

    if let Ok(xdg) = env::var("XDG_CONFIG_DIRS") {
        for entry in xdg.split(':') {
            if !entry.is_empty() {
                dirs.push(PathBuf::from(entry));
            }
        }
    } else {
        dirs.push(PathBuf::from("/etc/xdg"));
    }

    dirs
}
