//! Corgi's desktop window: what the dashboard shows and does, drawn with
//! GPUI and fed by `corgi serve`, run on this machine or over `ssh`. It
//! never sends the commands that move Herdr's focus.

mod dialogs;
mod endpoint;
mod form;
mod format;
mod herdr_ui;
mod layout;
mod link;
mod options;
mod store;
mod text_input;
mod theme;
mod view;
mod window;

#[cfg(feature = "capture")]
mod capture;

use std::process::ExitCode;

use gpui::{
    App, AppContext, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

use crate::{
    layout::{Layout, WINDOW_MIN},
    options::{Options, Parsed, USAGE},
    window::CorgiWindow,
};

/// The Wayland app id, and the X11 class: what a compositor's window rules
/// match, such as Hyprland's `class`.
const APP_ID: &str = "corgi-gui";

/// Monospace fonts tried in order off macOS: the user's Nerd Font first,
/// then common ones. `CORGI_GUI_FONT` names another.
const LINUX_FONTS: &[&str] = &[
    "CaskaydiaMono Nerd Font",
    "CaskaydiaMono Nerd Font Mono",
    "CaskaydiaCove Nerd Font",
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
];

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("corgi-gui: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // For test runs: open without taking focus from the app in front.
    let background = std::env::var_os("CORGI_GUI_BACKGROUND").is_some();
    gpui_platform::application().run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        window::bind_app_keys(cx);
        let font = pick_font(cx);
        eprintln!("corgi-gui: drawing with the font {font}");
        let saved = Layout::load(Layout::path().as_deref()).window;
        let (width, height) = saved.unwrap_or((1280.0, 820.0));
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let title = format!("Corgi — {}", options.place());
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Wayland: without an app id no compositor rule can
                    // match the window.
                    app_id: Some(APP_ID.into()),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(WINDOW_MIN.0), px(WINDOW_MIN.1))),
                    focus: !background,
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| CorgiWindow::new(options, font, window, cx));
                    let kept = view.downgrade();
                    // Keep the window's size for the next run.
                    window.on_window_should_close(cx, move |window, cx| {
                        let size = window.bounds().size;
                        let _ = kept.update(cx, |view, _| {
                            view.layout.window =
                                Some((f32::from(size.width), f32::from(size.height)));
                            view.save_layout();
                        });
                        true
                    });
                    view
                },
            )
            .expect("the window opens");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        if !background {
            cx.activate(true);
        }
        #[cfg(feature = "capture")]
        capture::start(window, cx);
        let _ = window;
    });
    ExitCode::SUCCESS
}

/// The monospace font for this platform: `CORGI_GUI_FONT`, else Menlo on
/// macOS, else the first of `LINUX_FONTS` installed (Menlo does not exist
/// elsewhere, and a proportional fallback breaks the rows' alignment).
fn pick_font(cx: &App) -> SharedString {
    if let Ok(font) = std::env::var("CORGI_GUI_FONT")
        && !font.trim().is_empty()
    {
        return font.into();
    }
    if cfg!(target_os = "macos") {
        return "Menlo".into();
    }
    let installed = cx.text_system().all_font_names();
    LINUX_FONTS
        .iter()
        .find(|wanted| installed.iter().any(|name| name == *wanted))
        .map(|font| SharedString::from(*font))
        .unwrap_or_else(|| "monospace".into())
}
