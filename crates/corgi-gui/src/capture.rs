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
//!   right one, or the left one twice, at window point (X, Y);
//! - `size:WxH` resizes the window, as a tiling window manager may have
//!   sized it otherwise (float it there first);
//! - `shot:NAME` writes `NAME.png`, the window as last drawn;
//! - `quit` ends the app.

use std::{path::PathBuf, time::Duration};

use gpui::{
    AnyWindowHandle, App, AppContext, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, WindowHandle, point, px, size,
};

use crate::window::CorgiWindow;

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
                "click" | "right" | "double" => {
                    let parsed = argument
                        .split_once(',')
                        .and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?)));
                    let Some((x, y)) = parsed else {
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
                    let _ = cx.update_window(any, |_, window, cx| {
                        // Over the point first, so its elements are hovered.
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                ..Default::default()
                            }),
                            cx,
                        );
                        for click_count in 1..=clicks {
                            window.dispatch_event(
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button,
                                    position,
                                    click_count,
                                    ..Default::default()
                                }),
                                cx,
                            );
                            window.dispatch_event(
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button,
                                    position,
                                    click_count,
                                    ..Default::default()
                                }),
                                cx,
                            );
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
