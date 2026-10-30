//! The fonts the window and its terminal draw with. Both are monospace (a
//! proportional font breaks the rows' alignment), and both prefer a Nerd
//! Font, whose icons prompts and status lines (Starship, cship) draw. Every
//! other Nerd Font installed is a font's fallback, so the icons still show
//! when the font itself, such as one `CORGI_GUI_FONT` names, lacks them.
//! Running text, a question's in the answer dialog, is set in a
//! proportional font.

use gpui::{Font, FontFallbacks, SharedString};

/// Monospace Nerd Fonts, tried in order: Omarchy's first, then common ones.
const NERD_FONTS: &[&str] = &[
    "CaskaydiaMono Nerd Font",
    "CaskaydiaMono Nerd Font Mono",
    "CaskaydiaCove Nerd Font",
    "CaskaydiaCove Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "JetBrainsMono Nerd Font Mono",
    "MesloLGS Nerd Font",
    "MesloLGS Nerd Font Mono",
    "MesloLGS NF",
    "Hack Nerd Font",
    "Hack Nerd Font Mono",
    "FiraCode Nerd Font",
    "FiraCode Nerd Font Mono",
    "SauceCodePro Nerd Font",
    "SauceCodePro Nerd Font Mono",
];

/// Nerd Fonts with icons and no letters: a fallback, never a font itself.
const SYMBOL_FONTS: &[&str] = &["Symbols Nerd Font Mono", "Symbols Nerd Font"];

/// Plain monospace fonts tried after the Nerd Fonts off macOS, where Menlo
/// does not exist.
const LINUX_FONTS: &[&str] = &[
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
];

/// The window's font: `wanted` (`CORGI_GUI_FONT`), else the first Nerd Font
/// installed, else Menlo on macOS, else the first of `LINUX_FONTS`
/// installed.
pub fn window_font(wanted: Option<String>, installed: &[String], macos: bool) -> SharedString {
    if let Some(wanted) = wanted.filter(|wanted| !wanted.trim().is_empty()) {
        return wanted.into();
    }
    let has = |name: &&&str| installed.iter().any(|font| font == **name);
    if let Some(font) = NERD_FONTS.iter().find(has) {
        return (*font).into();
    }
    if macos {
        return "Menlo".into();
    }
    LINUX_FONTS
        .iter()
        .find(has)
        .map_or_else(|| "monospace".into(), |font| (*font).into())
}

/// Proportional fonts tried for running text off macOS, in order. GPUI
/// takes `.SystemUIFont` to mean IBM Plex Sans there, which Zed bundles and
/// a desktop seldom has; its own fallbacks start with a monospace font.
const UI_FONTS: &[&str] = &[
    "Inter",
    "IBM Plex Sans",
    "Adwaita Sans",
    "Cantarell",
    "Ubuntu",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];

/// The font for running text: the system's on macOS (San Francisco), else
/// the first of `UI_FONTS` installed, else `window`, the window's own.
pub fn ui_font(installed: &[String], macos: bool, window: &str) -> SharedString {
    if macos {
        return ".SystemUIFont".into();
    }
    UI_FONTS
        .iter()
        .find(|name| installed.iter().any(|font| font == **name))
        .map_or_else(
            || SharedString::from(window.to_string()),
            |font| (*font).into(),
        )
}

/// The terminal's font: `wanted` (`CORGI_GUI_TERMINAL_FONT`) if installed,
/// else the first Nerd Font installed, else `window`, the window's.
pub fn terminal_font(wanted: Option<String>, installed: &[String], window: &str) -> SharedString {
    let has = |name: &str| installed.iter().any(|font| font == name);
    if let Some(wanted) = wanted.filter(|wanted| has(wanted)) {
        return wanted.into();
    }
    NERD_FONTS.iter().find(|font| has(font)).map_or_else(
        || SharedString::from(window.to_string()),
        |font| (*font).into(),
    )
}

/// `family`, falling back to every Nerd Font installed but itself, in the
/// order they are preferred, for the glyphs it lacks.
pub fn with_fallbacks(family: SharedString, installed: &[String]) -> Font {
    let fallbacks: Vec<String> = NERD_FONTS
        .iter()
        .chain(SYMBOL_FONTS)
        .filter(|font| **font != family.as_ref() && installed.iter().any(|name| name == *font))
        .map(|font| font.to_string())
        .collect();
    Font {
        fallbacks: (!fallbacks.is_empty()).then(|| FontFallbacks::from_fonts(fallbacks)),
        ..gpui::font(family)
    }
}

/// Installs the font file for this process alone, for checks that need a
/// font the machine lacks; macOS only.
#[cfg(all(target_os = "macos", feature = "capture"))]
pub fn register_for_process(file: &std::path::Path) -> bool {
    use std::{ffi::c_void, os::unix::ffi::OsStrExt, ptr};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: *const c_void,
            path: *const u8,
            length: isize,
            is_directory: bool,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontManagerRegisterFontsForURL(
            url: *const c_void,
            scope: u32,
            error: *mut *const c_void,
        ) -> bool;
    }
    /// `kCTFontManagerScopeProcess`.
    const PROCESS: u32 = 1;

    let path = file.as_os_str().as_bytes();
    let Ok(length) = isize::try_from(path.len()) else {
        return false;
    };
    // SAFETY: the path outlives the call, and the URL is released once.
    unsafe {
        let url =
            CFURLCreateFromFileSystemRepresentation(ptr::null(), path.as_ptr(), length, false);
        if url.is_null() {
            return false;
        }
        let registered = CTFontManagerRegisterFontsForURL(url, PROCESS, ptr::null_mut());
        CFRelease(url);
        registered
    }
}

#[cfg(all(not(target_os = "macos"), feature = "capture"))]
pub fn register_for_process(_file: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_window_prefers_a_named_font_then_a_nerd_font_then_the_platform_s() {
        let fonts = installed(&["Menlo", "DejaVu Sans Mono", "JetBrainsMono Nerd Font"]);
        assert_eq!(window_font(None, &fonts, true), "JetBrainsMono Nerd Font");
        assert_eq!(window_font(None, &fonts, false), "JetBrainsMono Nerd Font");
        assert_eq!(window_font(Some("Menlo".into()), &fonts, true), "Menlo");
        assert_eq!(
            window_font(Some(" ".into()), &fonts, true),
            "JetBrainsMono Nerd Font"
        );
        let plain = installed(&["Menlo", "DejaVu Sans Mono"]);
        assert_eq!(window_font(None, &plain, true), "Menlo");
        assert_eq!(window_font(None, &plain, false), "DejaVu Sans Mono");
        assert_eq!(window_font(None, &[], false), "monospace");
        // Icons alone are no font to draw text with.
        assert_eq!(
            window_font(None, &installed(&["Symbols Nerd Font Mono"]), true),
            "Menlo"
        );
    }

    #[test]
    fn running_text_takes_the_system_font_on_macos_else_an_installed_sans() {
        let fonts = installed(&["DejaVu Sans", "Noto Sans", "JetBrains Mono"]);
        assert_eq!(ui_font(&fonts, true, "Menlo"), ".SystemUIFont");
        assert_eq!(ui_font(&fonts, false, "JetBrains Mono"), "Noto Sans");
        // Without one, the window's font still draws it.
        assert_eq!(
            ui_font(&installed(&["JetBrains Mono"]), false, "JetBrains Mono"),
            "JetBrains Mono"
        );
    }

    #[test]
    fn the_terminal_font_prefers_a_named_one_then_a_nerd_font_then_the_windows() {
        let fonts = installed(&[
            "Menlo",
            "Hack Nerd Font Mono",
            "JetBrainsMono Nerd Font Mono",
        ]);
        assert_eq!(
            terminal_font(None, &fonts, "Menlo"),
            "JetBrainsMono Nerd Font Mono"
        );
        assert_eq!(
            terminal_font(Some("Menlo".into()), &fonts, "Menlo"),
            "Menlo"
        );
        // A name that is not installed is passed over.
        assert_eq!(
            terminal_font(Some("Nope".into()), &fonts, "Menlo"),
            "JetBrainsMono Nerd Font Mono"
        );
        assert_eq!(
            terminal_font(None, &installed(&["Menlo"]), "Menlo"),
            "Menlo"
        );
    }

    #[test]
    fn a_font_falls_back_to_the_other_nerd_fonts_installed() {
        let fonts = installed(&[
            "Menlo",
            "Symbols Nerd Font Mono",
            "JetBrainsMono Nerd Font",
            "Hack Nerd Font Mono",
        ]);
        let fallbacks = |font: Font| font.fallbacks.map(|list| list.fallback_list().to_vec());
        assert_eq!(
            fallbacks(with_fallbacks("Menlo".into(), &fonts)),
            Some(installed(&[
                "JetBrainsMono Nerd Font",
                "Hack Nerd Font Mono",
                "Symbols Nerd Font Mono"
            ]))
        );
        let font = with_fallbacks("JetBrainsMono Nerd Font".into(), &fonts);
        assert_eq!(font.family, "JetBrainsMono Nerd Font");
        assert_eq!(
            fallbacks(font),
            Some(installed(&[
                "Hack Nerd Font Mono",
                "Symbols Nerd Font Mono"
            ]))
        );
        assert_eq!(
            fallbacks(with_fallbacks("Menlo".into(), &installed(&["Menlo"]))),
            None
        );
    }
}
