//! System theme detection and colour palette selection.
//!
//! Determines whether the desktop prefers a dark or light appearance
//! without linking against D-Bus.  GNOME's theme preference lives in
//! GSettings / dconf, so we shell out to `gsettings` (a tiny CLI that
//! every GNOME installation has) rather than pulling in a D-Bus library.
//!
//! Detection order:
//!
//! 1. `GTK_THEME` environment variable (explicit override).
//! 2. `gsettings org.gnome.desktop.interface color-scheme` (GNOME 42+).
//! 3. `gsettings org.gnome.desktop.interface gtk-theme` (older GNOME).
//! 4. GTK 3/4 `settings.ini` files (non-GNOME GTK desktops).
//! 5. KDE `kdeglobals` `ColorScheme`.
//! 6. Defaults to **dark** when nothing is found.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
pub(crate) fn palette() -> &'static Palette {
    if prefer_dark() { &DARK } else { &LIGHT }
}

fn prefer_dark() -> bool {
    if let Ok(theme) = env::var("GTK_THEME") {
        return theme.to_ascii_lowercase().contains("dark");
    }

    if let Some(dark) = gnome_color_scheme() {
        return dark;
    }

    if gtk_prefer_dark() {
        return true;
    }

    if kde_prefer_dark() {
        return true;
    }

    true
}

/// Ask `gsettings` for GNOME's colour-scheme or theme preference.
///
/// Returns `Some(bool)` when GNOME gave a definitive answer,
/// `None` when `gsettings` is unavailable or returned nothing useful.
fn gnome_color_scheme() -> Option<bool> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok();

    if let Some(out) = &out
        && out.status.success()
    {
        let val = String::from_utf8_lossy(&out.stdout)
            .trim()
            .to_ascii_lowercase();
        if val.contains("dark") {
            return Some(true);
        }
        if val.contains("light") || val == "'default'" || val == "default" || val == "''" {
            return Some(false);
        }
    }

    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
        .ok();

    if let Some(out) = &out
        && out.status.success()
    {
        let val = String::from_utf8_lossy(&out.stdout)
            .trim()
            .to_ascii_lowercase();
        if val.contains("dark") {
            return Some(true);
        }
        if !val.is_empty() {
            return Some(false);
        }
    }

    None
}

/// Check GTK settings INI files for a dark-theme preference.
fn gtk_prefer_dark() -> bool {
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        for path in [
            home.join(".config/gtk-4.0/settings.ini"),
            home.join(".config/gtk-3.0/settings.ini"),
        ] {
            if let Ok(content) = fs::read_to_string(&path)
                && ini_contains_dark_preference(&content)
            {
                return true;
            }
        }
    }

    if let Ok(xdg) = env::var("XDG_CONFIG_DIRS") {
        for dir in xdg.split(':') {
            if dir.is_empty() {
                continue;
            }
            for ver in &["gtk-4.0", "gtk-3.0"] {
                let path = PathBuf::from(dir).join(ver).join("settings.ini");
                if let Ok(content) = fs::read_to_string(&path)
                    && ini_contains_dark_preference(&content)
                {
                    return true;
                }
            }
        }
    }

    false
}

fn ini_contains_dark_preference(content: &str) -> bool {
    for line in content.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().to_ascii_lowercase();
            if key == "gtk-application-prefer-dark-theme" && value == "1" {
                return true;
            }
            if key == "gtk-theme-name" && value.contains("dark") {
                return true;
            }
        }
    }
    false
}

/// Check KDE `kdeglobals` for a dark colour scheme.
fn kde_prefer_dark() -> bool {
    let path = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".config/kdeglobals"))
        .unwrap_or_else(|| PathBuf::from("/etc/xdg/kdeglobals"));

    let Ok(content) = fs::read_to_string(&path) else {
        return false;
    };

    for line in content.lines() {
        if let Some((key, value)) = line.trim().split_once('=')
            && key.trim() == "ColorScheme"
        {
            return value.trim().to_ascii_lowercase().contains("dark");
        }
    }

    false
}
