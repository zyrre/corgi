//! The "Find orphaned sessions" button and dialog: each candidate `corgi
//! serve`'s `orphans` found, with its evidence and its choices as buttons,
//! the chosen one marked; Enter or Apply sends `orphan_apply` for the
//! selected one. Its state is `dialogs::Orphans`, around the core's
//! `orphans::Choices`, which the dashboard's dialog shares.

use corgi_core::{
    orphans::{Candidate, Confidence},
    paths::tilde,
};
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use crate::{
    dialogs::{Dialog, Orphans},
    keymap,
    view::dialog_room,
    window::CorgiWindow,
};

impl CorgiWindow {
    /// The header's button under "Hotkeys", "Sessions": opens "Find sessions" (orphaned
    /// sessions), as its key does. Nothing is looked for until it is pressed.
    pub fn orphans_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        div()
            .id("orphans-button")
            .flex()
            .justify_center()
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(colors.roles.rule)
            .text_color(colors.roles.text)
            .font_weight(FontWeight::BOLD)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| {
                style
                    .bg(colors.roles.keycap)
                    .border_color(colors.roles.accent)
            })
            .child("Sessions")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.find_orphans(cx);
                cx.stop_propagation();
            }))
    }

    pub fn orphans_dialog(
        &self,
        orphans: &Orphans,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.colors;
        let secondary = colors.secondary(colors.roles.popup_fill);
        let choices = &orphans.choices;
        let tallest = dialog_room(window, self.layout.text_scale());
        let note = |text: String| div().text_color(secondary).child(text);
        let list = div()
            .id("orphans-list")
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.orphans_scroll)
            .children(
                choices
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| self.orphan_card(orphans, index, candidate, cx)),
            );
        let apply = choices
            .choice()
            .map(|(_, action)| format!("Apply: {}", action.label()))
            .unwrap_or_else(|| "Apply".into());
        let can_apply = orphans.idle() && choices.choice().is_some();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .max_h(px(tallest.max(0.0)))
            .child(div().flex_none().child(self.dialog_title("Find sessions".into())))
            .child(note(
                "Agents Corgi gets wrong, and leftover workspaces. Nothing changes until you apply a choice."
                    .into(),
            ))
            .when(orphans.looking, |d| {
                d.child(note(
                    "Looking at Herdr's panes, their processes and the supervisors' files…".into(),
                ))
            })
            .when(!orphans.looking && choices.candidates.is_empty(), |d| {
                d.child(
                    div().child(
                        "Nothing to find: every agent Herdr lists has its row, role and project.",
                    ),
                )
            })
            .child(list)
            .children(choices.done.iter().map(|done| {
                div()
                    .flex_none()
                    .text_color(colors.roles.success)
                    .child(format!("✓ {done}"))
            }))
            .children(choices.error.as_ref().map(|error| {
                div()
                    .flex_none()
                    .text_color(colors.roles.danger)
                    .font_weight(FontWeight::BOLD)
                    .child(error.clone())
            }))
            .when(orphans.applying.is_some(), |d| d.child(note("Applying…".into())))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .when(!choices.candidates.is_empty(), |row| {
                        row.child(self.button(
                            keymap::ENTER,
                            &apply,
                            can_apply,
                            true,
                            cx,
                            |this, _, cx| this.apply_orphan(cx),
                        ))
                    })
                    .child(self.button(keymap::ESCAPE, "Done", true, false, cx, |this, _, cx| {
                        this.dialogs.open = None;
                        cx.notify();
                    }))
                    .child(div().flex_1())
                    .child(note("j/k select · ←/→ or s/w/x/l choose".into())),
            )
            .into_any_element()
    }

    /// One candidate: what it is and where, its evidence, the fix outside
    /// Corgi, and its choices, the chosen one marked. The selected card is
    /// outlined in the accent, as the overview's selected row.
    fn orphan_card(
        &self,
        orphans: &Orphans,
        index: usize,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.colors;
        let roles = &colors.roles;
        let secondary = colors.secondary(roles.popup_fill);
        let choices = &orphans.choices;
        let selected = index == choices.selected;
        let chosen = choices
            .chosen
            .get(index)
            .copied()
            .unwrap_or(candidate.suggested);
        let place = match &candidate.pane_id {
            Some(pane) => format!("{} · {pane}", candidate.workspace),
            None => candidate.workspace_id.clone(),
        };
        let who = if candidate.harness.is_empty() {
            "(empty)".to_string()
        } else {
            candidate.harness.clone()
        };
        let confidence = match candidate.confidence {
            Confidence::Certain => roles.success,
            Confidence::Likely => roles.accent,
            Confidence::Guess => secondary,
        };
        let idle = orphans.idle();
        let actions = candidate
            .actions
            .iter()
            .enumerate()
            .map(|(action_index, action)| {
                let key = action.key().to_string();
                let label = if action_index == chosen {
                    format!("{} ✓", action.label())
                } else {
                    action.label()
                };
                div()
                    .id(SharedString::from(format!(
                        "orphan-{index}-action-{action_index}"
                    )))
                    .child(self.button(
                        &key,
                        &label,
                        idle,
                        action_index == chosen,
                        cx,
                        move |this, _, cx| {
                            if let Some(Dialog::Orphans(orphans)) = &mut this.dialogs.open {
                                orphans.choices.pick(index, action_index);
                            }
                            cx.notify();
                        },
                    ))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let applying = orphans.applying.as_deref() == Some(candidate.key.as_str());
        div()
            .id(SharedString::from(format!("orphan-{index}")))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_2()
            .border_color(if selected { roles.accent } else { roles.rule })
            .when(selected, |card| card.bg(roles.accent.opacity(0.06)))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Some(Dialog::Orphans(orphans)) = &mut this.dialogs.open
                    && orphans.idle()
                {
                    let action = orphans.choices.chosen.get(index).copied().unwrap_or(0);
                    orphans.choices.pick(index, action);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if selected { roles.accent } else { roles.text })
                            .child(format!("{who}  {}", candidate.name)),
                    )
                    .child(
                        div()
                            .text_color(secondary)
                            .child(format!("{place}  {}", tilde(&candidate.cwd))),
                    )
                    .child(div().flex_1())
                    .child(self.orphan_chip(candidate.cause.label(), roles.warning))
                    .child(self.orphan_chip(candidate.confidence.label(), confidence)),
            )
            .children(
                candidate
                    .evidence
                    .iter()
                    .map(|line| div().text_color(secondary).child(line.clone())),
            )
            .children(
                candidate
                    .fix
                    .as_ref()
                    .map(|fix| div().text_color(roles.warning).child(fix.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .pt_1()
                    .children(actions)
                    .when(applying, |row| {
                        row.child(div().text_color(roles.accent).child("applying…"))
                    }),
            )
            .into_any_element()
    }

    /// A small label on a tinted fill, as the question's `[blocks: …]` chip.
    fn orphan_chip(&self, text: &str, color: Hsla) -> AnyElement {
        let roles = &self.colors.roles;
        div()
            .flex_none()
            .px_2()
            .rounded_sm()
            .bg(roles.keycap)
            .text_color(color)
            .font_weight(FontWeight::BOLD)
            .child(text.to_string())
            .into_any_element()
    }
}
