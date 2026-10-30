//! What the window draws: the header (the corgi and the wordmark, the
//! herd's counts, plan usage), the overview of framed projects and their
//! agent rows, the transcript under it, the supervisor's side panel, the
//! focus view (an agent's terminal beside the side panel), the status bar
//! and the dialogs. Every color comes from `theme::Colors`.

use corgi_core::protocol::{Activity, Agent, MergeOutcome, Project};
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, relative,
};

use crate::{
    dialogs::{Dialog, Merge, MergeStage, Steps, outcome_line},
    form::Row,
    format::{self, Piece, Tone},
    layout::{
        CARD_GAP, CARD_HEIGHT, CARD_LINE, CARD_TITLE_ROW, CORGI_GAP, HEADER_HEIGHT, HEADER_PADDING,
        PIXEL, Split, TRANSCRIPT_MIN, WORDMARK_CARD_GAP, WORDMARK_TOP,
    },
    store::Connection,
    terminal::keys,
    theme,
    window::{CorgiWindow, Drag},
};

const TEXT_SIZE: f32 = 13.0;
/// The space above a dialog.
const DIALOG_TOP: f32 = 110.0;
/// The body text's line height, in text sizes.
const LINE_HEIGHT: f32 = 1.45;

/// What a toolbar button does.
type Action = fn(&mut CorgiWindow, &mut Window, &mut Context<CorgiWindow>);
/// What a click on a frame's top edge does.
type EdgeClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App)>;
/// The width of a frame's rounded corner piece in its top edge.
const FRAME_CORNER: f32 = 8.0;
/// A header card's narrowest, about the dashboard's fifteen cells.
const CARD_MIN_WIDTH: f32 = 15.0 * PIXEL;

impl Render for CorgiWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.take_refocus() {
            self.focus_dialog_now(window, cx);
        }
        let colors = self.colors.clone();
        let connected = self.store.connected();
        let has_state = self.store.seq.is_some() || !self.store.state.agents.is_empty();
        let dialog = self.dialogs.open.is_some();
        let viewport = window.viewport_size();
        let focus_view = self.focus_view.is_some();

        let body = if focus_view {
            // The terminal stays bright while `corgi serve` reconnects: it
            // has a connection of its own.
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.terminal.clone()),
                )
                .when(self.layout.focus_side_open, |body| {
                    body.child(self.divider(Split::FocusSide, cx))
                        .child(self.side_panel(window, cx))
                })
        } else {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .when(!connected, |body| body.opacity(0.45))
                .child(self.main_column(f32::from(viewport.height), cx))
                .when(self.layout.side_open, |body| {
                    body.child(self.divider(Split::Side, cx))
                        .child(self.side_panel(window, cx))
                })
        };

        div()
            .id("corgi")
            .key_context("Corgi")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.roles.text)
            .font(self.font.clone())
            .text_size(px(TEXT_SIZE))
            .line_height(px(TEXT_SIZE * LINE_HEIGHT))
            .on_key_down(cx.listener(|this, event, window, cx| {
                if this.dialogs.open.is_some() {
                    this.dialog_key(event, window, cx);
                } else if this.focus_view.is_some() {
                    this.focus_key(event, window, cx);
                } else {
                    this.overview_key(event, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.end_drag();
                    } else {
                        this.drag_to(event.position, cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.end_drag()),
            )
            .child(
                div()
                    .when(!connected, |header| header.opacity(0.45))
                    .child(self.header(cx)),
            )
            .map(|root| {
                if focus_view {
                    root.child(self.focus_bar(cx))
                } else {
                    root.child(self.toolbar(cx))
                }
            })
            .when(!connected, |root| {
                root.child(self.connection_banner(has_state, cx))
            })
            .child(body)
            .child(self.status_bar(cx))
            .when(dialog, |root| root.child(self.dialog_layer(window, cx)))
    }
}

impl CorgiWindow {
    fn tone(&self, tone: Tone, model: Option<&str>) -> Hsla {
        let colors = &self.colors;
        match tone {
            Tone::Text => colors.roles.text,
            Tone::Muted => colors.roles.muted,
            Tone::Accent => colors.roles.accent,
            Tone::Success => colors.roles.success,
            Tone::Warning => colors.roles.warning,
            Tone::Danger => colors.roles.danger,
            Tone::Merge => colors.roles.merge,
            Tone::Model => model.and_then(theme::vendor).unwrap_or(colors.roles.muted),
            Tone::Context(percent) => colors.context(percent),
            Tone::State(state) => colors.state(state, false),
        }
    }

    fn piece(&self, piece: &Piece, model: Option<&str>) -> AnyElement {
        div()
            .text_color(self.tone(piece.tone, model))
            .when(piece.bold, |d| d.font_weight(FontWeight::BOLD))
            .child(piece.text.clone())
            .into_any_element()
    }

    // ---- Header ----

    /// The dashboard's header in the same order: the corgi, the wordmark,
    /// then HERD and a card per plan, packed left at one height. A window
    /// too narrow for them all wraps the cards under the letters.
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let now = self.store.now();
        let state = &self.store.state;
        let card = |title: SharedString, color: Hsla, lines: Vec<AnyElement>| {
            framed(
                SharedString::from(format!("card-{title}")),
                CARD_TITLE_ROW,
                color,
                div()
                    .text_color(color)
                    .font_weight(FontWeight::BOLD)
                    .child(title)
                    .into_any_element(),
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .px_2()
                    .line_height(px(CARD_LINE))
                    .children(lines),
            )
            .flex_none()
            .h(px(CARD_HEIGHT))
            .min_w(px(CARD_MIN_WIDTH))
        };
        let herd_card = card(
            "HERD".into(),
            self.tone(format::herd_tone(&state.agents), None),
            format::herd(&state.agents)
                .iter()
                .map(|piece| self.piece(piece, None))
                .collect(),
        );
        let usage_cards = state.usage.iter().map(|usage| {
            let lines = format::usage_lines(usage, now)
                .into_iter()
                .map(|line| {
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .children(line.iter().map(|piece| self.piece(piece, None)))
                        .into_any_element()
                })
                .collect();
            let title_color = theme::vendor(&usage.title).unwrap_or(colors.roles.accent);
            card(usage.title.clone().into(), title_color, lines)
        });
        let lockup = div()
            .flex()
            .flex_row()
            .flex_none()
            .gap(px(CORGI_GAP))
            .h(px(HEADER_HEIGHT))
            .mr(px(WORDMARK_CARD_GAP - CARD_GAP))
            // A light theme's background all but hides the white blaze, so
            // the corgi stands on a grey tile there.
            .child(
                corgi_art()
                    .rounded_lg()
                    .when_some(colors.corgi_tile(), |corgi, tile| corgi.bg(tile)),
            )
            .child(
                div()
                    .mt(px(WORDMARK_TOP))
                    .child(wordmark_art(colors.supervisor())),
            );
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .gap_x(px(CARD_GAP))
            .gap_y(px(PIXEL))
            .px(px(HEADER_PADDING))
            .py_2()
            .border_b_1()
            .border_color(colors.roles.rule)
            .child(lockup)
            .child(herd_card)
            .children(usage_cards)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                }),
            )
    }

    // ---- Toolbar ----

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_agent();
        let can_prompt = selected.is_some_and(|agent| agent.can_prompt);
        let can_merge = selected.is_some_and(|agent| agent.can_merge);
        let tagged = selected.is_some_and(|agent| agent.ready_to_merge);
        let has_agent = selected.is_some();
        let mut bar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_4()
            .py_1()
            .border_b_1()
            .border_color(self.colors.roles.rule);
        let buttons: [(&str, &str, bool, Action); 7] = [
            ("n", "New agent", true, |this, _, cx| this.new_agent(cx)),
            ("p", "Prompt", can_prompt, |this, window, cx| {
                this.open_prompt(None, window, cx)
            }),
            ("s", "Supervisor", has_agent, |this, _, cx| {
                this.start_supervisor(cx)
            }),
            ("m", "Merge", can_merge, |this, _, cx| this.merge_check(cx)),
            (
                "t",
                if tagged { "Untag" } else { "Tag merge" },
                has_agent,
                |this, _, cx| this.toggle_tag(cx),
            ),
            ("x", "Close", has_agent, |this, _, cx| {
                this.close_check(None, cx)
            }),
            ("r", "Refresh", true, |this, _, cx| this.refresh(cx)),
        ];
        for (key, label, enabled, action) in buttons {
            bar =
                bar.child(
                    self.button(key, label, enabled, false, cx, move |this, window, cx| {
                        action(this, window, cx)
                    }),
                );
        }
        bar.child(self.button(
            "↵",
            "Terminal",
            has_agent,
            false,
            cx,
            |this, window, cx| {
                if let Some(pane_id) = this.selected.clone() {
                    this.open_agent(pane_id, window, cx);
                }
            },
        ))
        .child(div().flex_1())
        .child(self.button(
            "␣",
            "Transcript",
            true,
            self.layout.transcript_open,
            cx,
            |this, _, cx| this.toggle_transcript(cx),
        ))
        .child(self.button(
            "\\",
            "Side panel",
            true,
            self.layout.side_open,
            cx,
            |this, _, cx| this.toggle_side(cx),
        ))
    }

    /// The focus view's bar in place of the toolbar: the way back, the agent
    /// and how its terminal is, and the side panel's switch.
    fn focus_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let mac = cfg!(target_os = "macos");
        let terminal = self.terminal.read(cx);
        let status = terminal.status_text();
        let (live, gone) = (terminal.live(), terminal.gone());
        let agent = self
            .focus_view
            .as_deref()
            .and_then(|id| self.store.agent(id));
        let badge = agent.map(|agent| {
            let (badge, tone) = format::badge(agent);
            let fill = self.tone(tone, None);
            div()
                .px_1()
                .bg(fill)
                .text_color(colors.badge_text(fill))
                .font_weight(FontWeight::BOLD)
                .child(badge)
        });
        let task = agent.map(|agent| {
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::BOLD)
                .text_color(if agent.supervisor {
                    colors.supervisor()
                } else {
                    colors.roles.text
                })
                .child(if agent.task.is_empty() {
                    agent.name.clone()
                } else {
                    agent.task.clone()
                })
        });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_1()
            .border_b_1()
            .border_color(colors.roles.rule)
            .child(self.button(
                keys::leave_label(mac),
                "← Overview",
                true,
                false,
                cx,
                |this, _, cx| this.leave_focus(cx),
            ))
            .children(badge)
            .children(task)
            .child(
                div()
                    .flex_none()
                    .text_color(if live {
                        colors.roles.success
                    } else if gone {
                        colors.roles.warning
                    } else {
                        colors.roles.muted
                    })
                    .child(status),
            )
            .when(gone, |bar| {
                bar.child(
                    self.button("", "Open again", true, false, cx, |this, window, cx| {
                        this.reopen(window, cx)
                    }),
                )
            })
            .child(div().flex_1())
            .child(self.button(
                keys::side_label(mac),
                "Side panel",
                true,
                self.layout.focus_side_open,
                cx,
                |this, _, cx| this.toggle_side(cx),
            ))
    }

    /// A button with its key drawn on a cap before its label.
    fn button(
        &self,
        key: &str,
        label: &str,
        enabled: bool,
        on: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut CorgiWindow, &mut Window, &mut Context<CorgiWindow>) + 'static,
    ) -> impl IntoElement {
        let colors = &self.colors;
        let text = if enabled {
            colors.roles.text
        } else {
            colors.roles.muted
        };
        div()
            .id(SharedString::from(format!("button-{label}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .rounded_sm()
            .when(on, |b| b.bg(colors.roles.focus_bar))
            .when(enabled, |b| {
                b.cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(colors.roles.keycap))
            })
            .child(keycap(key, colors.roles.keycap, colors.roles.accent))
            .child(div().text_color(text).child(label.to_string()))
            .when(enabled, |b| {
                b.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    action(this, window, cx);
                    cx.stop_propagation();
                }))
            })
    }

    fn connection_banner(&self, has_state: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let (text, retry) = match self.connection() {
            Connection::Connecting => (
                format!("Connecting to corgi serve on {}…", self.options.place()),
                false,
            ),
            Connection::Connected => (String::new(), false),
            Connection::Lost { why, retry_at } => {
                let seconds = retry_at
                    .saturating_duration_since(std::time::Instant::now())
                    .as_secs()
                    + 1;
                let shown = if has_state {
                    " · showing the last state"
                } else {
                    ""
                };
                (
                    format!(
                        "Disconnected from corgi serve{shown} · reconnecting in {seconds}s · {}",
                        why.lines().last().unwrap_or_default()
                    ),
                    true,
                )
            }
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_1()
            .bg(colors.roles.warning.opacity(0.18))
            .text_color(colors.roles.warning)
            .child(div().flex_1().truncate().child(text))
            .when(retry, |banner| {
                banner.child(
                    self.button("⏎", "Retry now", true, false, cx, |this, _, cx| {
                        this.retry_now(cx)
                    }),
                )
            })
    }

    // ---- The overview and the transcript ----

    fn main_column(&self, window_height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let transcript_open = self.layout.transcript_open;
        // The header, toolbar and status bar take about this much.
        let available = (window_height - 180.0).max(TRANSCRIPT_MIN * 2.0);
        let transcript_height = self.layout.transcript_height_in(available);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(self.overview(cx))
            .when(transcript_open, |column| {
                column
                    .child(self.divider(Split::Transcript, cx))
                    .child(self.transcript(transcript_height))
            })
    }

    fn overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.store.state;
        let mut list = div()
            .id("overview")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p_3()
            .overflow_y_scroll()
            .track_scroll(&self.overview_scroll);
        if state.agents.is_empty() {
            let text = if self.store.seq.is_none() {
                "Waiting for corgi serve…"
            } else {
                "No agents. Press n to start one."
            };
            return list.child(div().text_color(self.colors.roles.muted).child(text));
        }
        for project in &state.projects {
            list = list.child(self.project(project, cx));
        }
        list
    }

    fn project(&self, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let collapsed = project.card && self.collapsed.contains(&project.group);
        let rows: Vec<AnyElement> = project
            .agents
            .iter()
            .filter(|id| !collapsed || project.supervisor.as_deref() == Some(id.as_str()))
            .filter_map(|id| self.store.agent(id))
            .map(|agent| self.agent_row(agent, cx))
            .collect();
        let title_color = if project.card {
            colors.supervisor()
        } else {
            colors.roles.accent
        };
        // The name in the middle of the frame's top edge, the checkout
        // dimmed after it.
        let title = div()
            .flex()
            .flex_row()
            .gap_2()
            .items_center()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .text_color(title_color)
                    .font_weight(FontWeight::BOLD)
                    .child(project.group.clone()),
            )
            .when(!project.root.is_empty(), |title| {
                title.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.roles.muted)
                        .child(corgi_core::paths::tilde(&project.root)),
                )
            })
            .children(format::held(project.held).map(|held| self.piece(&held, None)))
            .into_any_element();
        let corner = project.card.then(|| {
            div()
                .text_color(colors.roles.muted)
                .child(if collapsed {
                    "→ expand"
                } else {
                    "← collapse"
                })
                .into_any_element()
        });
        let group = project.group.clone();
        let right_click_group = group.clone();
        let toggle = project.card.then(|| -> EdgeClick {
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.toggle_card(&group, cx);
            }))
        });
        let footer = collapsed.then(|| {
            div()
                .flex()
                .flex_col()
                .pt_1()
                .border_t_1()
                .border_color(colors.roles.rule)
                .child(
                    div().flex().flex_row().gap_4().children(
                        format::worker_summary(&project.workers)
                            .iter()
                            .map(|piece| self.piece(piece, None)),
                    ),
                )
                .children(project.blocked.iter().map(|blocked| {
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .child(
                            div()
                                .text_color(colors.roles.warning)
                                .font_weight(FontWeight::BOLD)
                                .child("▲ blocked:"),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .child(blocked.task.clone()),
                        )
                        .child(div().text_color(colors.roles.muted).child("—"))
                        .child(
                            div()
                                .text_color(colors.roles.warning)
                                .truncate()
                                .child(blocked.waiting_on.clone()),
                        )
                }))
        });
        framed(
            SharedString::from(format!("heading-{}", project.group)),
            TEXT_SIZE * LINE_HEIGHT,
            colors.roles.rule,
            title,
            corner,
            toggle,
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .pb_2()
                .children(rows)
                .children(footer),
        )
        .id(SharedString::from(format!("project-{}", project.group)))
        // A right click anywhere in the frame collapses or expands it,
        // leaving the selection be. Rows take only left clicks.
        .on_aux_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            if event.is_right_click() {
                this.toggle_card(&right_click_group, cx);
                cx.stop_propagation();
            }
        }))
        .into_any_element()
    }

    fn agent_row(&self, agent: &Agent, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let now = self.store.now();
        let selected = self.selected.as_deref() == Some(agent.pane_id.as_str());
        let (badge, badge_tone) = format::badge(agent);
        let badge_color = self.tone(badge_tone, None);
        let secondary = colors.secondary(if selected {
            colors.roles.focus_bar
        } else {
            colors.background
        });
        let model = agent.model.as_deref().unwrap_or(&agent.harness);
        let task_color = if agent.supervisor {
            colors.supervisor()
        } else {
            colors.roles.text
        };
        // A narrow window wraps the fields after the task onto the next
        // line rather than cutting them off.
        let mut identity = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .child(
                div()
                    .px_1()
                    .min_w(px(68.))
                    .flex()
                    .justify_center()
                    .bg(badge_color)
                    .text_color(colors.badge_text(badge_color))
                    .font_weight(FontWeight::BOLD)
                    .child(badge),
            )
            .children(
                format::last_activity(agent).map(|time| div().text_color(secondary).child(time)),
            )
            .child(
                div()
                    .text_color(task_color)
                    .when(selected || agent.supervisor, |d| {
                        d.font_weight(FontWeight::BOLD)
                    })
                    .min_w_0()
                    .max_w_full()
                    .truncate()
                    .child(if agent.task.is_empty() {
                        agent.name.clone()
                    } else {
                        agent.task.clone()
                    }),
            );
        for piece in format::fields(agent, now) {
            identity = identity.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap_2()
                    .child(div().text_color(secondary).child("·"))
                    .child(
                        div()
                            .text_color(match piece.tone {
                                Tone::Muted => secondary,
                                tone => self.tone(tone, Some(model)),
                            })
                            .when(piece.bold, |d| d.font_weight(FontWeight::BOLD))
                            .child(piece.text),
                    ),
            );
        }
        let pane_id = agent.pane_id.clone();
        div()
            .id(SharedString::from(format!("row-{}", agent.pane_id)))
            .flex()
            .flex_col()
            .pl_2()
            .py_1()
            .rounded_sm()
            .border_l_4()
            .border_color(if agent.supervisor {
                colors.supervisor()
            } else if selected {
                colors.roles.accent
            } else {
                gpui::transparent_black()
            })
            .when(selected, |row| row.bg(colors.roles.focus_bar))
            .cursor(CursorStyle::PointingHand)
            .child(identity)
            .child(self.activity_line(&agent.message, 2, selected))
            .child(self.activity_line(&agent.tool, 1, selected))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select(pane_id.clone(), cx);
                window.focus(&this.focus, cx);
                if event.click_count() == 2 {
                    this.open_agent(pane_id.clone(), window, cx);
                }
            }))
            .into_any_element()
    }

    /// An activity line; on the `selected` row, muted parts take the
    /// colors' secondary so they read on the selection bar.
    fn activity_line(&self, activity: &Activity, lines: usize, selected: bool) -> impl IntoElement {
        let colors = &self.colors;
        let secondary = colors.secondary(if selected {
            colors.roles.focus_bar
        } else {
            colors.background
        });
        let marker_color = match format::marker_tone(activity.kind) {
            Tone::Muted => secondary,
            tone => self.tone(tone, None),
        };
        let text_color = match activity.kind {
            corgi_core::protocol::ActivityKind::Thinking => self.colors.roles.thinking,
            corgi_core::protocol::ActivityKind::Question => self.colors.roles.warning,
            corgi_core::protocol::ActivityKind::Ready => secondary,
            _ => self.colors.roles.text,
        };
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .text_color(marker_color)
                    .child(format::marker(activity.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(text_color)
                    .line_clamp(lines)
                    .when(lines == 1, |d| d.truncate())
                    .child(if activity.text.is_empty() {
                        "nothing yet".to_string()
                    } else {
                        activity.text.clone()
                    }),
            )
    }

    fn transcript(&self, height: f32) -> impl IntoElement {
        let colors = &self.colors;
        let agent = self.selected_agent();
        let title = agent.map_or("No agent selected".to_string(), |agent| {
            format!(
                "Session · {}",
                agent.herdr_name.clone().unwrap_or(agent.name.clone())
            )
        });
        let transcript = self
            .store
            .state
            .transcript
            .as_ref()
            .filter(|transcript| Some(&transcript.pane_id) == self.selected.as_ref());
        let entries: Vec<AnyElement> = match transcript {
            None if agent.is_some() => vec![
                div()
                    .text_color(colors.roles.muted)
                    .child("Reading the session…")
                    .into_any_element(),
            ],
            None => Vec::new(),
            Some(transcript) if transcript.entries.is_empty() => vec![
                div()
                    .text_color(colors.roles.muted)
                    .child("Corgi cannot read this harness's session file.")
                    .into_any_element(),
            ],
            Some(transcript) => transcript
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    // The newest turn in full, older ones two lines each.
                    let lines = if index == 0 { 0 } else { 2 };
                    self.transcript_entry(entry, lines)
                })
                .collect(),
        };
        div()
            .flex()
            .flex_col()
            .h(px(height))
            .flex_none()
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(colors.roles.accent)
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            )
            .child(
                div()
                    .id("transcript")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .children(entries),
            )
    }

    fn transcript_entry(&self, entry: &Activity, lines: usize) -> AnyElement {
        let colors = &self.colors;
        let text_color = match entry.kind {
            corgi_core::protocol::ActivityKind::Prompt => colors.roles.accent,
            corgi_core::protocol::ActivityKind::Command
            | corgi_core::protocol::ActivityKind::Tool => colors.roles.quiet,
            _ => colors.roles.text,
        };
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .text_color(self.tone(format::marker_tone(entry.kind), None))
                    .child(format::marker(entry.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(text_color)
                    .when(lines > 0, |d| d.line_clamp(lines))
                    .child(entry.text.clone()),
            )
            .into_any_element()
    }

    // ---- The side panel ----

    fn side_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let colors = &self.colors;
        let project = self
            .selected
            .as_deref()
            .and_then(|id| self.store.project_of(id));
        let mut panel = div()
            .id("side")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if self.focus_view.is_some() {
                self.layout.focus_side_width
            } else {
                self.layout.side_width
            }))
            .gap_2()
            .p_3()
            .bg(colors.panel)
            .overflow_y_scroll();
        let Some(project) = project else {
            return panel.child(
                div()
                    .text_color(colors.roles.muted)
                    .child("Select an agent to see its project's supervisor."),
            );
        };
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    div()
                        .text_color(colors.supervisor())
                        .font_weight(FontWeight::BOLD)
                        .child("SUPERVISOR"),
                )
                .child(
                    div()
                        .text_color(colors.roles.muted)
                        .child(project.group.clone()),
                ),
        );
        if let Some(held) = format::held(project.held) {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.roles.warning)
                    .child(self.piece(&held, None))
                    .child(div().text_color(colors.roles.muted).child(format!(
                        "Herdr never confirmed their delivery, so they may already be in the supervisor's box. Nothing more is typed in until they are read: corgi inbox {} --take",
                        corgi_core::paths::tilde(&project.root)
                    ))),
            );
        }
        let supervisor = project
            .supervisor
            .as_deref()
            .and_then(|id| self.store.agent(id));
        match supervisor {
            Some(supervisor) => {
                let pane_id = supervisor.pane_id.clone();
                let prompt_id = pane_id.clone();
                let now = self.store.now();
                let model = supervisor.model.as_deref().unwrap_or(&supervisor.harness);
                let (badge, tone) = format::badge(supervisor);
                let badge_color = self.tone(tone, None);
                panel = panel
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .px_1()
                                    .bg(badge_color)
                                    .text_color(colors.badge_text(badge_color))
                                    .font_weight(FontWeight::BOLD)
                                    .child(badge),
                            )
                            .children(format::fields(supervisor, now).into_iter().map(|piece| {
                                div()
                                    .text_color(self.tone(piece.tone, Some(model)))
                                    .child(piece.text)
                            })),
                    )
                    .child(self.activity_block(&supervisor.message))
                    .child(self.activity_line(&supervisor.tool, 2, false))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(self.button(
                                "p",
                                "Prompt it",
                                supervisor.can_prompt,
                                false,
                                cx,
                                move |this, window, cx| {
                                    this.open_prompt(Some(prompt_id.clone()), window, cx)
                                },
                            ))
                            .child(self.button(
                                "",
                                // The focus view opens the supervisor's own terminal.
                                if self.focus_view.is_some() {
                                    "Open its terminal"
                                } else {
                                    "Select it"
                                },
                                true,
                                false,
                                cx,
                                move |this, window, cx| {
                                    if this.focus_view.is_some() {
                                        this.open_agent(pane_id.clone(), window, cx)
                                    } else {
                                        this.select(pane_id.clone(), cx)
                                    }
                                },
                            )),
                    );
            }
            None if project.scratch => {
                panel = panel.child(
                    div()
                        .text_color(colors.roles.muted)
                        .child("Scratch sessions belong to no project and have no supervisor."),
                );
            }
            None => {
                panel = panel
                    .child(
                        div()
                            .text_color(colors.roles.muted)
                            .child("No supervisor runs for this project."),
                    )
                    .child(self.button(
                        "s",
                        "Start its supervisor",
                        true,
                        false,
                        cx,
                        |this, _, cx| this.start_supervisor(cx),
                    ));
            }
        }

        // The numbered questions, each answered in place.
        panel = panel.child(section_title(
            format!("OPEN QUESTIONS ({})", project.questions.len()),
            colors.roles.warning,
        ));
        if project.questions.is_empty() {
            panel = panel.child(div().text_color(colors.roles.muted).child("None open."));
        }
        let root = project.root.clone();
        for (index, question) in project.questions.iter().enumerate() {
            let key = if index < 9 {
                (index + 1).to_string()
            } else {
                String::new()
            };
            let (root, asked) = (root.clone(), question.clone());
            // Closed with × and still in the ledger: greyed, nothing to click.
            let closing = !question.id.is_empty() && self.closing.contains(&root, &question.id);
            let close = (!question.id.is_empty() && !closing).then(|| {
                let (root, handle) = (root.clone(), question.id.clone());
                div()
                    .id(SharedString::from(format!("question-close-{index}")))
                    .flex_none()
                    .px_1()
                    .rounded_sm()
                    .text_color(colors.roles.muted)
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| {
                        style
                            .bg(colors.roles.keycap)
                            .text_color(colors.roles.danger)
                    })
                    .child("×")
                    // The card's own click opens the answer box.
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.close_question(root.clone(), handle.clone(), cx);
                        cx.stop_propagation();
                    }))
            });
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("question-{index}")))
                    .flex()
                    .flex_col()
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(colors.roles.rule)
                    .when(closing, |card| card.text_color(colors.roles.muted))
                    .when(!closing, |card| {
                        card.cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(colors.roles.focus_bar))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .when(!key.is_empty(), |row| {
                                row.child(keycap(&key, colors.roles.keycap, colors.roles.accent))
                            })
                            .child(
                                div()
                                    .text_color(if closing {
                                        colors.roles.muted
                                    } else {
                                        colors.roles.warning
                                    })
                                    .font_weight(FontWeight::BOLD)
                                    .child(if question.id.is_empty() {
                                        "—".to_string()
                                    } else {
                                        question.id.clone()
                                    }),
                            )
                            .child(div().flex_1().min_w_0().child(question.title.clone()))
                            .children(close),
                    )
                    .when(!question.agent.is_empty() || closing, |card| {
                        let about = (!question.agent.is_empty())
                            .then(|| format!("about {}", question.agent));
                        let line = match (about, closing) {
                            (Some(about), true) => format!("{about} · closing…"),
                            (Some(about), false) => about,
                            (None, _) => "closing…".into(),
                        };
                        card.child(div().text_color(colors.roles.muted).child(line))
                    })
                    .when(!closing, |card| {
                        card.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.open_answer(root.clone(), asked.clone(), window, cx);
                        }))
                    }),
            );
        }

        if !project.blocked.is_empty() {
            panel = panel.child(section_title("BLOCKED".into(), colors.roles.warning));
            for blocked in &project.blocked {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .child(format!("▲ {}", blocked.task)),
                        )
                        .child(
                            div()
                                .text_color(colors.roles.warning)
                                .child(blocked.waiting_on.clone()),
                        ),
                );
            }
        }
        panel
    }

    fn activity_block(&self, activity: &Activity) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .text_color(self.tone(format::marker_tone(activity.kind), None))
                    .child(format::marker(activity.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .line_clamp(12)
                    .child(activity.text.clone()),
            )
    }

    /// The handle between two panes, dragged to resize them.
    fn divider(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let dragging = self.drag.is_some_and(|drag| drag.split == split);
        let line = if dragging {
            colors.roles.accent
        } else {
            colors.roles.rule
        };
        let handle = div()
            .id(SharedString::from(format!("divider-{split:?}")))
            .flex_none()
            .bg(line)
            .hover(|style| style.bg(colors.roles.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let last = match split {
                        Split::Side | Split::FocusSide => event.position.x,
                        Split::Transcript => event.position.y,
                    };
                    this.drag = Some(Drag { split, last });
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        match split {
            Split::Side | Split::FocusSide => handle
                .w(px(4.))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight),
            Split::Transcript => handle.h(px(4.)).w_full().cursor(CursorStyle::ResizeUpDown),
        }
    }

    // ---- The status bar ----

    /// The keys the status bar reminds of.
    fn status_hints(&self) -> String {
        if self.focus_view.is_none() {
            return "j/k select · ↵ terminal · ←/→ or right-click card · 1–9 answer".into();
        }
        let mac = cfg!(target_os = "macos");
        let (modifier, copy) = if mac {
            ("⌘", "⌘C/⌘V")
        } else {
            ("Ctrl", "Ctrl+Shift+C/V")
        };
        format!(
            "{} overview · {} side panel · {copy} copy/paste · {modifier}-click a link",
            keys::leave_label(mac),
            keys::side_label(mac),
        )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let state = &self.store.state;
        let (dot, dot_color, place) = match self.connection() {
            Connection::Connected => ("●", colors.roles.success, self.options.place()),
            Connection::Connecting => ("○", colors.roles.warning, "connecting".into()),
            Connection::Lost { .. } => ("○", colors.roles.danger, "disconnected".into()),
        };
        let herdr = (!state.herdr.connected && self.store.seq.is_some()).then(|| {
            div().text_color(colors.roles.danger).child(format!(
                "Herdr not answering: {}",
                state.herdr.error.clone().unwrap_or_default()
            ))
        });
        let flash = self
            .dialogs
            .flash
            .as_ref()
            .filter(|flash| flash.at.elapsed().as_secs() < 8)
            .map(|flash| {
                div()
                    .text_color(if flash.error {
                        colors.roles.danger
                    } else {
                        colors.roles.success
                    })
                    .truncate()
                    .child(flash.text.clone())
            });
        let status = state.status.clone().map(|status| {
            div()
                .text_color(colors.roles.muted)
                .truncate()
                .child(status)
        });
        let count = state.agents.len();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_1()
            .border_t_1()
            .border_color(colors.roles.rule)
            .child(div().text_color(dot_color).child(dot))
            .child(div().text_color(colors.roles.muted).child(place))
            .child(div().text_color(colors.roles.text).child(format!(
                "{count} agent{}",
                if count == 1 { "" } else { "s" }
            )))
            .children(herdr)
            .children(flash.or(status))
            .child(div().flex_1())
            .child(
                div()
                    .text_color(colors.roles.muted)
                    .child(self.status_hints()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                }),
            )
    }

    // ---- Dialogs ----

    fn dialog_layer(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let content = match &self.dialogs.open {
            Some(Dialog::Prompt {
                agent,
                sending,
                error,
                ..
            }) => self.text_dialog(format!("Prompt {agent}"), None, *sending, error.as_deref()),
            Some(Dialog::Answer {
                question,
                sending,
                error,
                ..
            }) => self.text_dialog(
                format!(
                    "Answer {}",
                    if question.id.is_empty() {
                        "the question"
                    } else {
                        &question.id
                    }
                ),
                Some(if question.text.is_empty() {
                    div().child(question.title.clone()).into_any_element()
                } else {
                    self.question_text(question.text.clone(), window)
                }),
                *sending,
                error.as_deref(),
            ),
            Some(Dialog::NewAgent(new)) => self.form_dialog(new, cx),
            Some(Dialog::NewAgentLoading) => self.loading_dialog(),
            Some(Dialog::Merge(merge)) => self.merge_dialog(merge, cx),
            Some(Dialog::Close(close)) => self.close_dialog(close, cx),
            None => div().into_any_element(),
        };
        // A refused prompt keeps the keyboard; its frame says so.
        let frame = if self.dialogs.open.as_ref().is_some_and(Dialog::refused) {
            colors.roles.danger
        } else {
            colors.roles.accent
        };
        div()
            .id("dialog-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(DIALOG_TOP))
            .bg(colors.background.opacity(0.55))
            .occlude()
            .child(
                div()
                    .id("dialog")
                    .key_context("Dialog")
                    .track_focus(&self.form_focus)
                    .w(px(720.))
                    .max_w(relative(0.92))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(frame)
                    .bg(colors.roles.popup_fill)
                    .shadow_lg()
                    .child(content),
            )
    }

    fn dialog_title(&self, title: String) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .text_color(self.colors.roles.accent)
            .font_weight(FontWeight::BOLD)
            .child("◆")
            .child(title)
    }

    fn field_box(&self, field: AnyElement) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(self.colors.roles.rule)
            .bg(self.colors.background)
            .child(field)
    }

    fn hints(&self, hints: &[(&str, &str)]) -> impl IntoElement {
        let colors = &self.colors;
        div()
            .flex()
            .flex_row()
            .gap_3()
            .pt_1()
            .border_t_1()
            .border_color(colors.roles.rule)
            .children(hints.iter().map(|(key, label)| {
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(keycap(key, colors.roles.keycap, colors.roles.accent))
                    .child(
                        div()
                            .text_color(colors.secondary(colors.roles.popup_fill))
                            .child(label.to_string()),
                    )
            }))
    }

    /// A question as the supervisor asked it, its lines kept, scrolling
    /// when it is taller than the window leaves room for.
    fn question_text(&self, text: String, window: &Window) -> AnyElement {
        // What the dialog's top margin, title, answer field and hints take.
        let around = DIALOG_TOP + 190.0;
        let room = (f32::from(window.viewport_size().height) - around).clamp(80.0, 420.0);
        div()
            .id("question-text")
            .max_h(px(room))
            .overflow_y_scroll()
            .track_scroll(&self.question_scroll)
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(self.colors.roles.rule)
            .child(text)
            .into_any_element()
    }

    fn text_dialog(
        &self,
        title: String,
        question: Option<AnyElement>,
        sending: bool,
        error: Option<&str>,
    ) -> AnyElement {
        let colors = &self.colors;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(title))
            .children(question)
            .child(self.field_box(self.prompt_input.clone().into_any_element()))
            .when(sending, |d| {
                d.child(
                    div()
                        .text_color(colors.secondary(colors.roles.popup_fill))
                        .child("Sending…"),
                )
            })
            .children(error.map(|error| {
                div()
                    .text_color(colors.roles.danger)
                    .child(error.to_string())
            }))
            .children(
                self.dialogs
                    .open
                    .as_ref()
                    .map(|dialog| self.hints(dialog.text_hints())),
            )
            .into_any_element()
    }

    /// The new-agent box while its form loads.
    fn loading_dialog(&self) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title("New agent".into()))
            .child(
                div()
                    .text_color(self.colors.secondary(self.colors.roles.popup_fill))
                    .child("Loading the form and the harnesses' models…"),
            )
            .child(self.hints(&[("Esc", "cancel")]))
            .into_any_element()
    }

    fn form_dialog(&self, new: &crate::form::NewAgent, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let mut rows = div().flex().flex_col();
        for row in new.rows() {
            let focused = new.row == row;
            let value: AnyElement = match row {
                Row::Task => self
                    .field_box(self.task_input.clone().into_any_element())
                    .into_any_element(),
                _ => {
                    let shown = new.shown(row);
                    let color = match row {
                        Row::Harness | Row::Model => {
                            let name = if row == Row::Model && new.model.is_empty() {
                                new.form
                                    .default_model
                                    .clone()
                                    .unwrap_or(new.harness.clone())
                            } else if row == Row::Model {
                                new.model.clone()
                            } else {
                                new.harness.clone()
                            };
                            theme::vendor(&name).unwrap_or(colors.roles.text)
                        }
                        _ => colors.roles.text,
                    };
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(div().text_color(color).child(shown))
                        .child(div().text_color(colors.roles.muted).child("▾"))
                        .child(
                            div()
                                .text_color(colors.roles.muted)
                                .truncate()
                                .child(new.detail(row)),
                        )
                        .into_any_element()
                }
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("form-{}", row.label())))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py(px(2.))
                    .rounded_sm()
                    .when(focused, |r| r.bg(colors.roles.focus_bar))
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(colors.roles.accent)
                            .child(if focused { "▶" } else { "" }),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .text_color(if focused {
                                colors.roles.text
                            } else {
                                colors.roles.muted
                            })
                            .when(focused, |d| d.font_weight(FontWeight::BOLD))
                            .child(row.label()),
                    )
                    .child(div().flex_1().min_w_0().child(value))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(Dialog::NewAgent(new)) = &mut this.dialogs.open
                            && new.running.is_none()
                        {
                            new.row = row;
                            new.picker = None;
                        }
                        this.focus_dialog_now(window, cx);
                        cx.notify();
                    })),
            );
            if focused && new.picker.is_some() {
                rows = rows.child(self.picker(new));
            }
        }
        let starts = new
            .form
            .starts_supervisor
            .then(|| {
                div().text_color(colors.supervisor()).child(
                    "This project has no agent yet: this starts its supervisor, with the task as its first request.",
                )
            });
        let running = new.running.as_ref().map(|steps| self.steps(steps, true));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(new.title().to_string()))
            .child(rows)
            .children(starts)
            .children(
                new.error
                    .as_ref()
                    .map(|error| div().text_color(colors.roles.danger).child(error.clone())),
            )
            .children(running)
            .child(self.hints(&[
                ("↵", "create agent"),
                ("↑↓ ⇥", "rows"),
                ("←→", "change"),
                ("Space", "list"),
                ("Esc", "cancel"),
            ]))
            .into_any_element()
    }

    /// The focused row's list, inline under it.
    fn picker(&self, new: &crate::form::NewAgent) -> AnyElement {
        let colors = &self.colors;
        let picker = new.picker.as_ref().expect("an open list");
        let rows = new.picker_rows();
        let cursor = picker.cursor.min(rows.len().saturating_sub(1));
        div()
            .flex()
            .flex_col()
            .ml(px(104.))
            .mb_1()
            .p_1()
            .rounded_sm()
            .border_1()
            .border_color(colors.roles.accent)
            .bg(colors.background)
            .child(
                div()
                    .text_color(colors.roles.muted)
                    .child(format!("filter: {}▏", picker.filter)),
            )
            .when(rows.is_empty(), |list| {
                list.child(
                    div()
                        .text_color(colors.roles.muted)
                        .child("nothing matches"),
                )
            })
            .children(rows.iter().enumerate().take(12).map(|(index, choice)| {
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_1()
                    .when(index == cursor, |row| row.bg(colors.roles.focus_bar))
                    .child(div().child(choice.label.clone()))
                    .child(
                        div()
                            .text_color(colors.roles.muted)
                            .truncate()
                            .child(choice.detail.clone()),
                    )
                    .when(!choice.badge.is_empty(), |row| {
                        row.child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(colors.roles.keycap)
                                .text_color(colors.roles.accent)
                                .child(choice.badge.clone()),
                        )
                    })
            }))
            .into_any_element()
    }

    /// A command's steps: done, running, still to come.
    fn steps(&self, steps: &Steps, running: bool) -> AnyElement {
        let colors = &self.colors;
        div()
            .flex()
            .flex_col()
            .children(steps.steps.iter().enumerate().map(|(index, step)| {
                let (mark, color) = match steps.step {
                    Some(at) if index < at => ("✓", colors.roles.success),
                    Some(at) if index == at && running => ("●", colors.roles.accent),
                    _ => ("○", colors.roles.muted),
                };
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(div().text_color(color).child(mark))
                    .child(step.clone())
            }))
            .children(
                steps
                    .status
                    .as_ref()
                    .filter(|_| running)
                    .map(|status| div().text_color(colors.roles.muted).child(status.clone())),
            )
            .into_any_element()
    }

    fn merge_dialog(&self, merge: &Merge, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let form = &merge.form;
        let running = merge.stage == MergeStage::Running;
        let mut dialog = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(format!("Merge {}", form.agent)))
            .child(div().font_weight(FontWeight::BOLD).child(form.task.clone()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .text_color(colors.roles.accent)
                            .child(form.source_branch.clone()),
                    )
                    .child(div().text_color(colors.roles.muted).child("→"))
                    .child(
                        div()
                            .text_color(colors.roles.success)
                            .child(form.target_branch.clone()),
                    )
                    .children(form.upstream.as_ref().map(|upstream| {
                        div()
                            .text_color(colors.roles.muted)
                            .child(format!("(checks {upstream} first)"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(form.commits.iter().take(8).map(|commit| {
                        div()
                            .text_color(colors.roles.quiet)
                            .child(format!("  {commit}"))
                    }))
                    .when(form.commits.len() > 8, |d| {
                        d.child(
                            div()
                                .text_color(colors.roles.muted)
                                .child(format!("  … and {} more", form.commits.len() - 8)),
                        )
                    }),
            );
        if merge.stage != MergeStage::Confirm {
            // A merge that succeeded went through every step.
            let mut progress = merge.progress.clone();
            if matches!(
                merge.stage,
                MergeStage::Outcome(MergeOutcome::Succeeded { .. })
            ) {
                progress.step = Some(progress.steps.len());
            }
            dialog = dialog.child(self.steps(&progress, running));
        }
        match &merge.stage {
            MergeStage::Outcome(outcome) => {
                let good = matches!(outcome, MergeOutcome::Succeeded { .. });
                let color = if good {
                    colors.roles.success
                } else {
                    colors.roles.warning
                };
                dialog = dialog.child(
                    div()
                        .p_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(color)
                        .text_color(color)
                        .font_weight(FontWeight::BOLD)
                        .child(format!(
                            "{} {}",
                            if good { "✓" } else { "!" },
                            outcome_line(outcome)
                        )),
                );
                if let MergeOutcome::Behind { commits } = outcome {
                    dialog = dialog.children(commits.iter().take(6).map(|commit| {
                        div()
                            .text_color(colors.roles.quiet)
                            .child(format!("  {commit}"))
                    }));
                }
            }
            MergeStage::Failed(error) => {
                dialog = dialog.child(div().text_color(colors.roles.danger).child(error.clone()));
            }
            MergeStage::Running => {
                dialog = dialog.child(
                    div()
                        .text_color(colors.roles.muted)
                        .child("Merging… Esc hides this; the outcome shows when it ends."),
                );
            }
            MergeStage::Confirm => {}
        }
        let actions = merge.actions();
        let mut buttons = div().flex().flex_row().gap_2().pt_1();
        for (index, (label, action)) in actions.into_iter().enumerate() {
            let key = if index == 0 { "↵" } else { "" };
            buttons =
                buttons.child(
                    self.button(key, label, true, index == 0, cx, move |this, _, cx| {
                        this.merge_action(action, cx)
                    }),
                );
        }
        dialog.child(buttons).into_any_element()
    }

    fn close_dialog(&self, close: &crate::dialogs::Close, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let form = &close.form;
        let what = match form.target.as_str() {
            "worktree" => format!(
                "Removes the worktree {} from disk; its branch stays.",
                form.checkout.clone().unwrap_or_default()
            ),
            "tab" => "Closes the agent's tab in its project's workspace.".to_string(),
            _ => "Closes the agent's workspace.".to_string(),
        };
        let confirm = if close.dirty.is_some() {
            "Discard changes and close".to_string()
        } else {
            form.confirm.clone()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(format!("Close {}", form.label)))
            .child(div().child(what))
            .children(close.dirty.as_ref().map(|dirty| {
                div()
                    .text_color(colors.roles.danger)
                    .font_weight(FontWeight::BOLD)
                    .child(dirty.clone())
            }))
            .children(
                close
                    .error
                    .as_ref()
                    .map(|error| div().text_color(colors.roles.danger).child(error.clone())),
            )
            .when(close.running, |d| {
                d.child(div().text_color(colors.roles.muted).child("Closing…"))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .pt_1()
                    .child(
                        self.button("↵", &confirm, !close.running, true, cx, |this, _, cx| {
                            this.confirm_close(cx)
                        }),
                    )
                    .child(
                        self.button("Esc", "Cancel", true, false, cx, |this, _, cx| {
                            this.dialogs.open = None;
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }
}

fn section_title(text: String, color: Hsla) -> impl IntoElement {
    div()
        .pt_2()
        .text_color(color)
        .font_weight(FontWeight::BOLD)
        .child(text)
}

fn keycap(key: &str, fill: Hsla, text: Hsla) -> AnyElement {
    if key.is_empty() {
        return div().into_any_element();
    }
    div()
        .px_1()
        .rounded_sm()
        .bg(fill)
        .text_color(text)
        .font_weight(FontWeight::BOLD)
        .child(key.to_string())
        .into_any_element()
}

/// A rounded frame closed on all four sides, as the dashboard draws its
/// boxes: `title` in the middle of the top edge, `corner` at its right end,
/// `body` inside. The top edge is `row` tall, its rule running through the
/// middle behind the title; a click anywhere on it does `on_edge_click`.
fn framed(
    id: SharedString,
    row: f32,
    edge: Hsla,
    title: AnyElement,
    corner: Option<AnyElement>,
    on_edge_click: Option<EdgeClick>,
    body: impl IntoElement,
) -> Div {
    let half = px(row / 2.0);
    let rule = || div().h(half).mt(half).border_t_1().border_color(edge);
    // Two equal halves either side of the title keep it in the middle of
    // the frame, whatever the corner holds.
    let left = div()
        .flex()
        .flex_row()
        .flex_1()
        .child(
            rule()
                .flex_none()
                .w(px(FRAME_CORNER))
                .border_l_1()
                .rounded_tl_md(),
        )
        .child(rule().flex_1());
    let right = div()
        .flex()
        .flex_row()
        .flex_1()
        .child(rule().flex_1())
        .children(corner.map(|corner| div().flex_none().px_1().child(corner)))
        .child(
            rule()
                .flex_none()
                .w(px(FRAME_CORNER))
                .border_r_1()
                .rounded_tr_md(),
        );
    let top = div()
        .id(id)
        .flex()
        .flex_row()
        .h(px(row))
        .line_height(px(row))
        .child(left)
        .child(div().flex().min_w_0().overflow_hidden().px_1().child(title))
        .child(right)
        .when_some(on_edge_click, |top, click| {
            top.cursor(CursorStyle::PointingHand).on_click(click)
        });
    div().flex().flex_col().child(top).child(
        div()
            .flex_1()
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .rounded_b_md()
            .border_color(edge)
            .child(body),
    )
}

/// Pixel art, one square per pixel, `None` left to the background. A run
/// of one color is one box, so the art shows no seams between its squares.
fn pixel_art(rows: impl Iterator<Item = Vec<Option<Hsla>>>) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .children(rows.map(|row| {
            let mut runs: Vec<(Option<Hsla>, usize)> = Vec::new();
            for pixel in row {
                match runs.last_mut() {
                    Some((color, count)) if *color == pixel => *count += 1,
                    _ => runs.push((pixel, 1)),
                }
            }
            div()
                .flex()
                .flex_row()
                .h(px(PIXEL))
                .children(runs.into_iter().map(|(color, count)| {
                    div()
                        .flex_none()
                        .w(px(count as f32 * PIXEL))
                        .h(px(PIXEL))
                        .when_some(color, |square, color| square.bg(color))
                }))
        }))
}

/// The supervisor's corgi in the brand's fixed colors.
fn corgi_art() -> Div {
    pixel_art(corgi_core::art::CORGI_PIXELS.iter().map(|row| {
        row.chars()
            .map(|pixel| corgi_core::art::corgi_color(pixel).and_then(theme::brand_color))
            .collect()
    }))
}

/// The CORGI wordmark in `color`.
fn wordmark_art(color: Hsla) -> Div {
    pixel_art(
        corgi_core::art::wordmark_pixels()
            .into_iter()
            .map(|row| row.into_iter().map(|on| on.then_some(color)).collect()),
    )
}
