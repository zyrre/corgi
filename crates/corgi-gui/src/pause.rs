//! The header's "Pause all" button, under "Sessions", and its key: the
//! confirmation `corgi serve`'s `pause_check` fills in, then `pause_all`.
//! While any agent is paused (a row with `paused`) the same button reads
//! "Resume all" and sends `resume_all`. The pause lives in Corgi's state,
//! so the button finds it again after the window was closed.

use corgi_core::{pause, protocol::Command};
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
};

use crate::{
    dialogs::{Dialog, PauseAll},
    keymap,
    store::Purpose,
    window::CorgiWindow,
};

impl CorgiWindow {
    /// Whether any agent is paused, so the button resumes them all.
    pub fn any_paused(&self) -> bool {
        self.store().state.agents.iter().any(|agent| agent.paused)
    }

    pub fn pause_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let running = self.dialogs.all_running;
        let paused = self.any_paused();
        let label = running.unwrap_or(if paused { "Resume all" } else { "Pause all" });
        // Paused, it stands out: the agents wait on it.
        let border = if paused && running.is_none() {
            colors.roles.warning
        } else {
            colors.roles.rule
        };
        div()
            .id("pause-button")
            .flex()
            .justify_center()
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_color(if running.is_some() {
                colors.roles.muted
            } else {
                colors.roles.text
            })
            .font_weight(FontWeight::BOLD)
            .when(running.is_none(), |button| {
                button
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| {
                        style
                            .bg(colors.roles.keycap)
                            .border_color(colors.roles.accent)
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.pause_or_resume_all(cx);
                        cx.stop_propagation();
                    }))
            })
            .child(label)
    }

    /// The button and its key: Resume all while agents are paused, else
    /// asks what Pause all would stop, for the confirmation.
    pub fn pause_or_resume_all(&mut self, cx: &mut Context<Self>) {
        if self.dialogs.all_running.is_some() || self.dialogs.open.is_some() {
            return;
        }
        if self.any_paused() {
            if self.send(Command::ResumeAll, Purpose::ResumeAll) {
                self.dialogs.all_running = Some("Resuming…");
                self.dialogs.say("Resuming the paused agents…", false);
            }
        } else {
            self.send(Command::PauseCheck, Purpose::PauseCheck);
        }
        cx.notify();
    }

    pub fn confirm_pause(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Pause(pause)) = &mut self.dialogs.open else {
            return;
        };
        if pause.running {
            return;
        }
        pause.running = true;
        pause.error = None;
        if self.send(Command::PauseAll, Purpose::PauseAll) {
            self.dialogs.all_running = Some("Pausing…");
        } else if let Some(Dialog::Pause(pause)) = &mut self.dialogs.open {
            pause.running = false;
        }
        cx.notify();
    }

    pub fn pause_dialog(&self, pause: &PauseAll, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let secondary = colors.secondary(colors.roles.popup_fill);
        let lines = pause::confirmation(&pause.form);
        let (first, rest) = lines
            .split_first()
            .map_or(("", &[][..]), |(f, r)| (f.as_str(), r));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title("Pause all agents".into()))
            .child(div().child(first.to_string()))
            .child(div().flex().flex_col().children(rest.iter().map(|line| {
                let warning = line.contains("current turn is lost");
                div()
                    .text_color(if warning {
                        colors.roles.warning
                    } else {
                        secondary
                    })
                    .when(warning, |d| d.font_weight(FontWeight::BOLD))
                    .child(line.clone())
            })))
            .children(pause.error.as_ref().map(|error| {
                div()
                    .text_color(colors.roles.danger)
                    .font_weight(FontWeight::BOLD)
                    .child(error.clone())
            }))
            .when(pause.running, |d| {
                d.child(
                    div()
                        .text_color(secondary)
                        .child(pause.progress.clone().unwrap_or_else(|| "Pausing…".into())),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .pt_1()
                    .child(self.button(
                        keymap::ENTER,
                        "Pause all",
                        !pause.running,
                        true,
                        cx,
                        |this, _, cx| this.confirm_pause(cx),
                    ))
                    .child(self.button(
                        keymap::ESCAPE,
                        "Cancel",
                        true,
                        false,
                        cx,
                        |this, _, cx| {
                            this.dialogs.open = None;
                            cx.notify();
                        },
                    )),
            )
            .into_any_element()
    }
}
