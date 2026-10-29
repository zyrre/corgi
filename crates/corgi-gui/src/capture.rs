//! Screenshots for checking the window without screen-recording permission
//! (the `capture` feature): `CORGI_GUI_SCRIPT` lists steps separated by
//! `;`, run once the window is up, and `CORGI_GUI_CAPTURE_DIR` is where
//! `shot` writes its PNG. Steps:
//!
//! - `wait:MS` waits;
//! - `key:KEYS` types each space-separated keystroke (`p`, `escape`,
//!   `shift-tab`), as GPUI parses them;
//! - `type:TEXT` types the text, character by character;
//! - `click:X,Y`, `right:X,Y` and `double:X,Y` click the left button, the
//!   right one, or the left one twice, at window point (X, Y); `link:X,Y`
//!   clicks with the link modifier held (Cmd on macOS, Ctrl elsewhere);
//! - `drag:X1,Y1,X2,Y2` presses the left button at the first point, moves to
//!   the second with it held, and lets go there;
//! - `wheel:X,Y,LINES` turns the wheel over the point, up for positive lines;
//! - `ime:MARKED>TEXT` has the focus view's terminal compose MARKED, as an
//!   input method or a dead key shows it, then commit TEXT (`ime:´>é`);
//! - `hide` and `show` tell the window it was hidden or shown, as the
//!   platform does when it is minimised or occluded;
//! - `size:WxH` resizes the window, as a tiling window manager may have
//!   sized it otherwise (float it there first);
//! - `glyphs:TEXT` logs the font each character of TEXT draws from in the
//!   focus view's terminal: its own font, a fallback, or one the system
//!   picked;
//! - `shot:NAME` writes `NAME.png`, the window as last drawn;
//! - `quit` ends the app.
//!
//! `CORGI_GUI_FONT_FILES`, font files separated by `:`, are installed for
//! this process alone on macOS, so a check can draw with a font the machine
//! does not have without installing it for its other apps.

use std::{path::PathBuf, time::Duration};

use gpui::{
    AnyWindowHandle, App, AppContext, EntityInputHandler, Font, Hsla, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta,
    ScrollWheelEvent, TextRun, Window, WindowHandle, point, px, size,
};

use crate::window::CorgiWindow;

/// Installs `CORGI_GUI_FONT_FILES` for this process, before the window's
/// fonts are chosen.
pub fn register_fonts() {
    let Some(files) = std::env::var_os("CORGI_GUI_FONT_FILES") else {
        return;
    };
    for file in std::env::split_paths(&files) {
        if !crate::fonts::register_for_process(&file) {
            eprintln!("corgi-gui: could not load the font file {}", file.display());
        }
    }
}

pub fn start(window: WindowHandle<CorgiWindow>, cx: &mut App) {
    let Ok(script) = std::env::var("CORGI_GUI_SCRIPT") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("CORGI_GUI_CAPTURE_DIR").unwrap_or_else(|_| ".".into()));
    let any: AnyWindowHandle = window.into();
    cx.spawn(async move |cx| {
        for step in script
            .split(';')
            .map(str::trim)
            .filter(|step| !step.is_empty())
        {
            let (verb, argument) = step.split_once(':').unwrap_or((step, ""));
            match verb {
                "wait" => {
                    let ms = argument.parse().unwrap_or(500);
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                }
                "key" | "type" => {
                    let keys: Vec<String> = if verb == "key" {
                        argument.split_whitespace().map(String::from).collect()
                    } else {
                        argument
                            .chars()
                            .map(|c| {
                                if c == ' ' {
                                    "space".into()
                                } else {
                                    c.to_string()
                                }
                            })
                            .collect()
                    };
                    for key in keys {
                        // The untyped handle leaves the root view free for the
                        // key's listeners to update.
                        let _ =
                            cx.update_window(any, |_, window, cx| match Keystroke::parse(&key) {
                                Ok(keystroke) => {
                                    window.dispatch_keystroke(keystroke, cx);
                                }
                                Err(error) => eprintln!("capture: bad key {key}: {error}"),
                            });
                        cx.background_executor()
                            .timer(Duration::from_millis(30))
                            .await;
                    }
                }
                "click" | "right" | "double" | "link" => {
                    let Some([x, y]) = numbers::<2>(argument) else {
                        eprintln!("capture: bad point {argument}");
                        continue;
                    };
                    let position = point(px(x), px(y));
                    let button = if verb == "right" {
                        MouseButton::Right
                    } else {
                        MouseButton::Left
                    };
                    let clicks = if verb == "double" { 2 } else { 1 };
                    let modifiers = if verb == "link" {
                        link_modifier()
                    } else {
                        Modifiers::default()
                    };
                    let _ = cx.update_window(any, |_, window, cx| {
                        // Over the point first, so its elements are hovered.
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                modifiers,
                                ..Default::default()
                            }),
                            cx,
                        );
                        for click_count in 1..=clicks {
                            window.dispatch_event(
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button,
                                    position,
                                    modifiers,
                                    click_count,
                                    ..Default::default()
                                }),
                                cx,
                            );
                            window.dispatch_event(
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button,
                                    position,
                                    modifiers,
                                    click_count,
                                }),
                                cx,
                            );
                        }
                    });
                }
                "drag" => {
                    let Some([x1, y1, x2, y2]) = numbers::<4>(argument) else {
                        eprintln!("capture: bad drag {argument}");
                        continue;
                    };
                    let (from, to) = (point(px(x1), px(y1)), point(px(x2), px(y2)));
                    let _ = cx.update_window(any, |_, window, cx| {
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position: from,
                                ..Default::default()
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button: MouseButton::Left,
                                position: from,
                                click_count: 1,
                                ..Default::default()
                            }),
                            cx,
                        );
                        for step in 1..=8 {
                            let along = step as f32 / 8.0;
                            window.dispatch_event(
                                PlatformInput::MouseMove(MouseMoveEvent {
                                    position: point(
                                        px(x1 + (x2 - x1) * along),
                                        px(y1 + (y2 - y1) * along),
                                    ),
                                    pressed_button: Some(MouseButton::Left),
                                    ..Default::default()
                                }),
                                cx,
                            );
                        }
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button: MouseButton::Left,
                                position: to,
                                click_count: 1,
                                ..Default::default()
                            }),
                            cx,
                        );
                    });
                }
                "wheel" => {
                    let Some([x, y, lines]) = numbers::<3>(argument) else {
                        eprintln!("capture: bad wheel {argument}");
                        continue;
                    };
                    let position = point(px(x), px(y));
                    let _ = cx.update_window(any, |_, window, cx| {
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                ..Default::default()
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::ScrollWheel(ScrollWheelEvent {
                                position,
                                delta: ScrollDelta::Lines(point(0.0, lines)),
                                ..Default::default()
                            }),
                            cx,
                        );
                    });
                }
                "ime" => {
                    let (marked, text) = argument.split_once('>').unwrap_or((argument, argument));
                    let _ = cx.update_window(any, |root, window, cx| {
                        let Ok(root) = root.downcast::<CorgiWindow>() else {
                            return;
                        };
                        let terminal = root.read(cx).terminal.clone();
                        terminal.update(cx, |terminal, cx| {
                            terminal.replace_and_mark_text_in_range(None, marked, None, window, cx);
                        });
                        window.refresh();
                        terminal.update(cx, |terminal, cx| {
                            terminal.replace_text_in_range(None, text, window, cx);
                        });
                    });
                }
                "glyphs" => {
                    let _ = cx.update_window(any, |root, window, cx| {
                        if let Ok(root) = root.downcast::<CorgiWindow>() {
                            let font = root.read(cx).terminal.read(cx).font().clone();
                            eprintln!("capture: {}", glyph_fonts(&font, argument, window));
                        }
                    });
                }
                "hide" | "show" => {
                    let visible = verb == "show";
                    let _ = cx.update_window(any, |root, _, cx| {
                        if let Ok(root) = root.downcast::<CorgiWindow>() {
                            root.update(cx, |root, cx| root.set_visible(visible, cx));
                        }
                    });
                }
                "size" => {
                    let parsed = argument
                        .split_once('x')
                        .and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)));
                    match parsed {
                        Some((width, height)) => {
                            let _ = cx.update_window(any, |_, window, _| {
                                window.resize(size(px(width), px(height)))
                            });
                        }
                        None => eprintln!("capture: bad size {argument}"),
                    }
                }
                "shot" => {
                    // Let the last change draw first.
                    let _ = cx.update_window(any, |_, window, _| window.refresh());
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    let path = dir.join(format!("{argument}.png"));
                    let _ = cx.update_window(any, |_, window, _| match window.render_to_image() {
                        Ok(image) => match image.save(&path) {
                            Ok(()) => eprintln!("capture: wrote {}", path.display()),
                            Err(error) => eprintln!("capture: {}: {error}", path.display()),
                        },
                        Err(error) => eprintln!("capture: render failed: {error}"),
                    });
                }
                "quit" => {
                    cx.update(|cx| cx.quit());
                }
                other => eprintln!("capture: unknown step {other}"),
            }
        }
    })
    .detach();
}

/// `N` numbers separated by commas.
fn numbers<const N: usize>(argument: &str) -> Option<[f32; N]> {
    let parsed: Vec<f32> = argument
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect::<Option<_>>()?;
    parsed.try_into().ok()
}

/// The modifier a link click is made with.
fn link_modifier() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::default()
        }
    } else {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }
}

/// Which font draws each character of `text` in `font`.
fn glyph_fonts(font: &Font, text: &str, window: &Window) -> String {
    let system = window.text_system();
    let own = system.resolve_font(font);
    let fallbacks: Vec<_> = font
        .fallbacks
        .iter()
        .flat_map(|fallbacks| fallbacks.fallback_list())
        .map(|name| (system.resolve_font(&gpui::font(name.clone())), name.clone()))
        .collect();
    let line = system.layout_line(
        text,
        px(13.0),
        &[TextRun {
            len: text.len(),
            font: font.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let mut drawn = Vec::new();
    for run in &line.runs {
        for glyph in &run.glyphs {
            let symbol = text[glyph.index..].chars().next().unwrap_or(' ');
            let from = if run.font_id == own {
                format!("{} itself", font.family)
            } else if let Some((_, name)) = fallbacks.iter().find(|(id, _)| *id == run.font_id) {
                format!("the fallback {name}")
            } else {
                format!("a font the system picked ({:?})", run.font_id)
            };
            drawn.push(format!("U+{:04X} from {from}", u32::from(symbol)));
        }
    }
    format!("glyphs in {}: {}", font.family, drawn.join(", "))
}
