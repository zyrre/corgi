//! Screenshots for checking the window without screen-recording permission
//! (the `capture` feature): `CORGI_GUI_SCRIPT` lists steps separated by
//! `;`, run once the window is up, and `CORGI_GUI_CAPTURE_DIR` is where
//! `shot` writes its PNG. Steps:
//!
//! - `wait:MS` waits;
//! - `key:KEYS` types each space-separated keystroke (`p`, `escape`,
//!   `shift-tab`), as GPUI parses them;
//! - `type:TEXT` types the text, character by character;
//! - `shot:NAME` writes `NAME.png`, the window as last drawn;
//! - `quit` ends the app.

use std::{path::PathBuf, time::Duration};

use gpui::{App, Keystroke, WindowHandle};

use crate::window::CorgiWindow;

pub fn start(window: WindowHandle<CorgiWindow>, cx: &mut App) {
    let Ok(script) = std::env::var("CORGI_GUI_SCRIPT") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("CORGI_GUI_CAPTURE_DIR").unwrap_or_else(|_| ".".into()));
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
                        let _ = window.update(cx, |_, window, cx| match Keystroke::parse(&key) {
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
                "shot" => {
                    // Let the last change draw first.
                    let _ = window.update(cx, |_, window, _| window.refresh());
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    let path = dir.join(format!("{argument}.png"));
                    let _ = window.update(cx, |_, window, _| match window.render_to_image() {
                        Ok(image) => match image.save(&path) {
                            Ok(()) => eprintln!("capture: wrote {}", path.display()),
                            Err(error) => eprintln!("capture: {}: {error}", path.display()),
                        },
                        Err(error) => eprintln!("capture: render failed: {error}"),
                    });
                }
                "quit" => {
                    let _ = cx.update(|cx| cx.quit());
                }
                other => eprintln!("capture: unknown step {other}"),
            }
        }
    })
    .detach();
}
