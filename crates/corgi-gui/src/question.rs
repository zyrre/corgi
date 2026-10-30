//! A question's full text in the answer dialog, as the supervisor wrote it
//! (`supervisor/ROLE.md`, "Questions to the user"): its heading and
//! `[blocks: …]` tag, the `Context:`, `Options:`, `Recommendation:` and
//! `Until answered:` parts, and the Markdown inside them.
//!
//! MOCKUP: three looks behind `CORGI_GUI_QUESTION_STYLE` (`a` a rendered
//! document, `b` a structured card, `c` the monospace layout with the
//! Markdown markers hidden); unset, the dialog shows the text as before.

use std::{ops::Range, sync::OnceLock};

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, FontStyle, FontWeight, HighlightStyle, Hsla,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Styled, StyledText, UnderlineStyle, div, prelude::FluentBuilder, px,
    relative,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::window::CorgiWindow;

/// Which look the dialog gives a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Document,
    Card,
    Conservative,
}

/// `CORGI_GUI_QUESTION_STYLE`, read once.
pub fn look() -> Option<Look> {
    static LOOK: OnceLock<Option<Look>> = OnceLock::new();
    *LOOK.get_or_init(|| {
        match std::env::var("CORGI_GUI_QUESTION_STYLE")
            .ok()?
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "a" => Some(Look::Document),
            "b" => Some(Look::Card),
            "c" => Some(Look::Conservative),
            _ => None,
        }
    })
}

/// The system's proportional font, as GPUI names it.
const UI_FONT: &str = ".SystemUIFont";
/// The proportional body text's size, and its line height in sizes.
const BODY_SIZE: f32 = 14.0;
const BODY_LINE: f32 = 1.55;
/// The heading's size.
const TITLE_SIZE: f32 = 17.0;
/// The labels' size in the card look.
const LABEL_SIZE: f32 = 11.0;

// ---- The supervisor's form ----

/// The parts of `supervisor/ROLE.md`'s form a question was written in.
#[derive(Debug, Default, PartialEq)]
pub struct Form {
    pub heading: Option<Heading>,
    pub parts: Vec<Part>,
}

#[derive(Debug, PartialEq)]
pub struct Heading {
    /// `Q45: Where does …`, the `###` gone.
    pub title: String,
    /// What the bracketed tag says: `blocks: …` or `no block`.
    pub tag: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Part {
    /// Text in no labelled part: Markdown as it is.
    Free(String),
    /// `Context:`, `Recommendation:`, `Until answered:`: its label and the
    /// Markdown after it.
    Labeled {
        label: String,
        body: String,
    },
    Options(Vec<Choice>),
}

#[derive(Debug, PartialEq)]
pub struct Choice {
    /// `A`.
    pub letter: String,
    /// What follows `A. `, with its continuation lines.
    pub text: String,
}

const LABELS: &[&str] = &["Context", "Options", "Recommendation", "Until answered"];

impl Form {
    pub fn parse(text: &str) -> Self {
        let mut form = Form::default();
        let mut lines = text.lines().peekable();
        while lines.peek().is_some_and(|line| line.trim().is_empty()) {
            lines.next();
        }
        if let Some(line) = lines.peek()
            && let Some(rest) = line.trim_start().strip_prefix('#')
        {
            form.heading = Some(Heading::parse(rest.trim_start_matches('#').trim()));
            lines.next();
        }
        // The open choice's content column, for dedenting what follows it.
        let mut column = 0;
        for line in lines {
            if let Some((label, rest)) = label_of(line) {
                form.parts.push(if label == "Options" && rest.is_empty() {
                    Part::Options(Vec::new())
                } else {
                    Part::Labeled {
                        label: label.to_string(),
                        body: rest.to_string(),
                    }
                });
                continue;
            }
            if let Some(Part::Options(choices)) = form.parts.last_mut() {
                if let Some((letter, text, at)) = choice_of(line) {
                    choices.push(Choice {
                        letter: letter.to_string(),
                        text: text.to_string(),
                    });
                    column = at;
                    continue;
                }
                if let Some(choice) = choices.last_mut() {
                    let indent = line.len() - line.trim_start_matches(' ').len();
                    choice.text.push('\n');
                    choice.text.push_str(&line[indent.min(column)..]);
                    continue;
                }
            }
            match form.parts.last_mut() {
                Some(Part::Labeled { body, .. } | Part::Free(body)) => {
                    body.push('\n');
                    body.push_str(line);
                }
                _ => form.parts.push(Part::Free(line.to_string())),
            }
        }
        form
    }

    /// Whether it follows the form far enough for the card look: a heading
    /// and lettered options.
    pub fn is_whole(&self) -> bool {
        self.heading.is_some()
            && self
                .parts
                .iter()
                .any(|part| matches!(part, Part::Options(choices) if !choices.is_empty()))
    }

    /// The letter `Recommendation:` starts with, when an option has it.
    pub fn recommended(&self) -> Option<&str> {
        let body = self.parts.iter().find_map(|part| match part {
            Part::Labeled { label, body } if label == "Recommendation" => Some(body),
            _ => None,
        })?;
        let body = body.trim_start().trim_start_matches(['*', '_']);
        let letter = body.get(..1)?;
        let after = body[1..].chars().next();
        if !letter.chars().all(|c| c.is_ascii_uppercase()) || after.is_some_and(char::is_alphabetic)
        {
            return None;
        }
        self.parts.iter().find_map(|part| match part {
            Part::Options(choices) => choices
                .iter()
                .find(|choice| choice.letter == letter)
                .map(|choice| choice.letter.as_str()),
            _ => None,
        })
    }
}

impl Heading {
    fn parse(line: &str) -> Self {
        if line.ends_with(']')
            && let Some(open) = line.rfind('[')
        {
            let title = line[..open].trim_end();
            if !title.is_empty() {
                return Heading {
                    title: title.to_string(),
                    tag: Some(line[open + 1..line.len() - 1].trim().to_string()),
                };
            }
        }
        Heading {
            title: line.to_string(),
            tag: None,
        }
    }
}

/// `Context: …` at the line's start: the label and what follows it.
fn label_of(line: &str) -> Option<(&'static str, &str)> {
    LABELS.iter().find_map(|label| {
        let rest = line.strip_prefix(label)?.strip_prefix(':')?;
        Some((*label, rest.trim()))
    })
}

/// `  A. …`: the letter, the text, and the column the text starts at.
fn choice_of(line: &str) -> Option<(&str, &str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let mut chars = trimmed.chars();
    chars.next().filter(char::is_ascii_uppercase)?;
    chars.next().filter(|c| matches!(c, '.' | ')'))?;
    let rest = chars.as_str();
    let text = rest.trim_start();
    if indent > 6 || text.len() == rest.len() {
        return None;
    }
    Some((&trimmed[..1], text, line.len() - text.len()))
}

// ---- Markdown ----

/// How a stretch of text is marked.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    pub strike: bool,
}

/// Text with its marks, each stretch once, in order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<(Range<usize>, Marks)>,
}

impl Inline {
    fn push(&mut self, text: &str, marks: Marks) {
        let start = self.text.len();
        self.text.push_str(text);
        match self.spans.last_mut() {
            Some((range, last)) if *last == marks && range.end == start => {
                range.end = self.text.len();
            }
            _ => self.spans.push((start..self.text.len(), marks)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Block {
    Paragraph(Inline),
    Heading(Inline),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Code(String),
    Quote(Vec<Block>),
    Rule,
}

/// `text`'s Markdown blocks, its line breaks kept: the supervisor writes a
/// line where it wants one.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut events = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH).peekable();
    parse_blocks(&mut events)
}

/// All of `text`'s inline Markdown as one line of marked text, its blocks
/// run together.
pub fn flat(text: &str) -> Inline {
    let mut inline = Inline::default();
    let mut marks = MarkCount::default();
    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH) {
        marks.take(&event, &mut inline);
    }
    inline
}

type Events<'a> = std::iter::Peekable<Parser<'a>>;

fn parse_blocks(events: &mut Events) -> Vec<Block> {
    let mut blocks = Vec::new();
    while let Some(event) = events.peek() {
        if is_inline(event) {
            blocks.push(Block::Paragraph(parse_inline(events)));
            continue;
        }
        match events.next().expect("peeked") {
            Event::End(_) => break,
            Event::Start(Tag::Paragraph) => {
                blocks.push(Block::Paragraph(parse_inline(events)));
                events.next();
            }
            Event::Start(Tag::Heading { .. }) => {
                blocks.push(Block::Heading(parse_inline(events)));
                events.next();
            }
            Event::Start(Tag::List(start)) => {
                let mut items = Vec::new();
                while let Some(event) = events.next() {
                    match event {
                        Event::Start(Tag::Item) => items.push(parse_blocks(events)),
                        _ => break,
                    }
                }
                blocks.push(Block::List { start, items });
            }
            Event::Start(Tag::CodeBlock(_)) => {
                let mut code = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::Text(text) => code.push_str(&text),
                        _ => break,
                    }
                }
                blocks.push(Block::Code(code.trim_end().to_string()));
            }
            Event::Start(Tag::BlockQuote(_)) => blocks.push(Block::Quote(parse_blocks(events))),
            Event::Rule => blocks.push(Block::Rule),
            Event::Start(_) => blocks.extend(parse_blocks(events)),
            _ => {}
        }
    }
    blocks
}

fn is_inline(event: &Event) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::TaskListMarker(_)
            | Event::Start(
                Tag::Strong
                    | Tag::Emphasis
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. }
            )
            | Event::End(
                TagEnd::Strong
                    | TagEnd::Emphasis
                    | TagEnd::Strikethrough
                    | TagEnd::Link
                    | TagEnd::Image
            )
    )
}

fn parse_inline(events: &mut Events) -> Inline {
    let mut inline = Inline::default();
    let mut marks = MarkCount::default();
    while events.peek().is_some_and(is_inline) {
        let event = events.next().expect("peeked");
        marks.take(&event, &mut inline);
    }
    inline
}

/// The marks open at a point, nested ones counted.
#[derive(Default)]
struct MarkCount {
    bold: u8,
    italic: u8,
    link: u8,
    strike: u8,
}

impl MarkCount {
    fn marks(&self) -> Marks {
        Marks {
            bold: self.bold > 0,
            italic: self.italic > 0,
            link: self.link > 0,
            strike: self.strike > 0,
            code: false,
        }
    }

    fn take(&mut self, event: &Event, inline: &mut Inline) {
        match event {
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                inline.push(text, self.marks());
            }
            Event::Code(text) | Event::InlineMath(text) => inline.push(
                text,
                Marks {
                    code: true,
                    ..self.marks()
                },
            ),
            Event::SoftBreak | Event::HardBreak => inline.push("\n", self.marks()),
            Event::TaskListMarker(done) => {
                inline.push(if *done { "☑ " } else { "☐ " }, self.marks())
            }
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike = self.strike.saturating_sub(1),
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => self.link += 1,
            Event::End(TagEnd::Link | TagEnd::Image) => self.link = self.link.saturating_sub(1),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item)
                if !inline.text.is_empty() && !inline.text.ends_with(['\n', ' ']) =>
            {
                inline.push(" ", self.marks());
            }
            _ => {}
        }
    }
}

// ---- Drawing ----

/// The colors marked text takes.
struct Ink {
    text: Hsla,
    code: Hsla,
    code_fill: Option<Hsla>,
    link: Hsla,
    /// The family inline code is set in, when the text around it is
    /// another.
    code_font: Option<SharedString>,
    bold: FontWeight,
}

impl CorgiWindow {
    /// A question as the supervisor asked it, its lines kept, scrolling
    /// when it is taller than the window leaves room for.
    pub fn question_text(&self, text: &str, room: f32, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let scroll = div()
            .id("question-text")
            .max_h(px(room))
            .overflow_y_scroll()
            .track_scroll(&self.question_scroll);
        match look() {
            None => scroll
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(colors.roles.rule)
                .child(text.to_string())
                .into_any_element(),
            Some(Look::Conservative) => scroll
                .px_3()
                .py_2()
                .rounded_sm()
                .border_1()
                .border_color(colors.roles.rule)
                .child(self.conservative(text))
                .into_any_element(),
            Some(look) => {
                let form = Form::parse(text);
                let body = if look == Look::Card && form.is_whole() {
                    self.card(&form, cx)
                } else {
                    self.document(&form)
                };
                scroll
                    .px_3()
                    .py_2()
                    .when(look == Look::Document, |scroll| {
                        scroll
                            .rounded_sm()
                            .border_1()
                            .border_color(colors.roles.rule)
                    })
                    .font_family(UI_FONT)
                    .text_size(px(BODY_SIZE))
                    .line_height(relative(BODY_LINE))
                    .text_color(colors.roles.text)
                    .child(body)
                    .into_any_element()
            }
        }
    }

    fn proportional_ink(&self) -> Ink {
        let roles = &self.colors.roles;
        Ink {
            text: roles.text,
            code: roles.text,
            code_fill: Some(roles.keycap.opacity(0.6)),
            link: roles.accent,
            code_font: Some(self.font.family.clone()),
            bold: FontWeight::SEMIBOLD,
        }
    }

    /// Marked text, set in the inherited font with `ink`'s marks.
    fn inline(&self, inline: &Inline, ink: &Ink) -> StyledText {
        // A thin space either side of inline code, inside its fill.
        let mut text = String::with_capacity(inline.text.len());
        let mut highlights = Vec::new();
        let mut code_ranges = Vec::new();
        for (range, marks) in &inline.spans {
            let piece = &inline.text[range.clone()];
            let start = text.len();
            if marks.code && ink.code_fill.is_some() {
                text.push('\u{2009}');
                text.push_str(piece);
                text.push('\u{2009}');
            } else {
                text.push_str(piece);
            }
            let range = start..text.len();
            if marks.code {
                code_ranges.push(range.clone());
            }
            let style = HighlightStyle {
                color: Some(if marks.link {
                    ink.link
                } else if marks.code {
                    ink.code
                } else {
                    ink.text
                }),
                font_weight: marks.bold.then_some(ink.bold),
                font_style: marks.italic.then_some(FontStyle::Italic),
                background_color: if marks.code { ink.code_fill } else { None },
                underline: marks.link.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(ink.link),
                    wavy: false,
                }),
                strikethrough: marks.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(ink.text),
                }),
                fade_out: None,
            };
            highlights.push((range, style));
        }
        let styled = StyledText::new(text).with_highlights(highlights);
        match &ink.code_font {
            Some(family) => styled.with_font_family_overrides(
                code_ranges
                    .into_iter()
                    .map(|range| (range, family.clone()))
                    .collect::<Vec<_>>(),
            ),
            None => styled,
        }
    }

    /// Markdown blocks, a paragraph's space between them.
    fn markdown(&self, blocks: &[Block], ink: &Ink) -> Div {
        let colors = &self.colors;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(blocks.iter().map(|block| -> AnyElement {
                match block {
                    Block::Paragraph(inline) => {
                        div().child(self.inline(inline, ink)).into_any_element()
                    }
                    Block::Heading(inline) => div()
                        .pt_1()
                        .font_weight(ink.bold)
                        .child(self.inline(inline, ink))
                        .into_any_element(),
                    Block::List { start, items } => div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(items.iter().enumerate().map(|(index, item)| {
                            let marker = match start {
                                Some(first) => format!("{}.", first + index as u64),
                                None => "•".to_string(),
                            };
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .min_w(px(12.))
                                        .text_color(colors.secondary(colors.roles.popup_fill))
                                        .child(marker),
                                )
                                .child(div().flex_1().min_w_0().child(self.markdown(item, ink)))
                        }))
                        .into_any_element(),
                    Block::Code(code) => div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(colors.roles.keycap)
                        .font(self.font.clone())
                        .text_size(px(13.))
                        .child(code.clone())
                        .into_any_element(),
                    Block::Quote(inner) => div()
                        .pl_3()
                        .border_l_2()
                        .border_color(colors.roles.rule)
                        .child(self.markdown(inner, ink))
                        .into_any_element(),
                    Block::Rule => div().h(px(1.)).bg(colors.roles.rule).into_any_element(),
                }
            }))
    }

    /// The heading's title and its tag as a chip.
    fn heading(&self, heading: &Heading, ink: &Ink) -> Div {
        let colors = &self.colors;
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(TITLE_SIZE))
                    .line_height(relative(1.3))
                    .font_weight(FontWeight::BOLD)
                    .child(self.inline(&flat(&heading.title), ink)),
            )
            .children(heading.tag.as_ref().map(|tag| {
                let color = if tag.starts_with("no block") {
                    colors.secondary(colors.roles.popup_fill)
                } else {
                    colors.roles.warning
                };
                div()
                    .px(px(7.))
                    .py(px(1.))
                    .rounded_md()
                    .border_1()
                    .border_color(color.opacity(0.6))
                    .bg(color.opacity(0.14))
                    .text_color(color)
                    .text_size(px(12.))
                    .line_height(relative(1.4))
                    .child(self.inline(
                        &flat(tag),
                        &Ink {
                            text: color,
                            code: color,
                            code_fill: None,
                            ..self.proportional_ink()
                        },
                    ))
            }))
    }

    /// A, the rendered document: a title and chip, bold labels, options as
    /// a list with their letters as badges.
    fn document(&self, form: &Form) -> AnyElement {
        let ink = self.proportional_ink();
        let colors = &self.colors;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(
                form.heading
                    .as_ref()
                    .map(|heading| self.heading(heading, &ink)),
            )
            .children(form.parts.iter().map(|part| -> AnyElement {
                match part {
                    Part::Free(text) => self.markdown(&blocks(text), &ink).into_any_element(),
                    Part::Labeled { label, body } => self
                        .markdown(&blocks(&format!("**{label}:** {body}")), &ink)
                        .into_any_element(),
                    Part::Options(choices) => div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(self.markdown(&blocks("**Options:**"), &ink))
                        .children(choices.iter().map(|choice| {
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(10.))
                                .pl(px(6.))
                                .child(letter_badge(
                                    &choice.letter,
                                    colors.roles.accent,
                                    colors.badge_text(colors.roles.accent),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.markdown(&blocks(&choice.text), &ink)),
                                )
                        }))
                        .into_any_element(),
                }
            }))
            .into_any_element()
    }

    /// B, the structured card: sections under small labels, each option a
    /// card of its own that answers with its letter when clicked, the
    /// recommended one marked.
    fn card(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let ink = self.proportional_ink();
        let colors = &self.colors;
        let recommended = form.recommended();
        let label = |text: &str| {
            div()
                .text_size(px(LABEL_SIZE))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.roles.accent)
                .child(text.to_uppercase())
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(
                form.heading
                    .as_ref()
                    .map(|heading| self.heading(heading, &ink)),
            )
            .children(form.parts.iter().map(|part| -> AnyElement {
                match part {
                    Part::Free(text) => self.markdown(&blocks(text), &ink).into_any_element(),
                    Part::Labeled { label: name, body } => div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(label(name))
                        .child(self.markdown(&blocks(body), &ink))
                        .into_any_element(),
                    Part::Options(choices) => div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(label("Options · click one to answer with it"))
                        .children(choices.iter().map(|choice| {
                            let chosen = recommended == Some(choice.letter.as_str());
                            let letter = choice.letter.clone();
                            let edge = if chosen {
                                colors.roles.success
                            } else {
                                colors.roles.rule
                            };
                            div()
                                .id(SharedString::from(format!("option-{letter}")))
                                .flex()
                                .flex_row()
                                .items_start()
                                .gap(px(10.))
                                .px(px(10.))
                                .py(px(8.))
                                .rounded_md()
                                .border_1()
                                .when(chosen, |card| card.border_2())
                                .border_color(edge)
                                .bg(colors.background)
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(colors.roles.focus_bar))
                                .child(letter_badge(
                                    &choice.letter,
                                    if chosen {
                                        colors.roles.success
                                    } else {
                                        colors.roles.accent
                                    },
                                    colors.badge_text(if chosen {
                                        colors.roles.success
                                    } else {
                                        colors.roles.accent
                                    }),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.markdown(&blocks(&choice.text), &ink)),
                                )
                                .when(chosen, |card| {
                                    card.child(
                                        div()
                                            .flex_none()
                                            .px(px(6.))
                                            .rounded_md()
                                            .bg(colors.roles.success)
                                            .text_color(colors.badge_text(colors.roles.success))
                                            .text_size(px(LABEL_SIZE))
                                            .font_weight(FontWeight::BOLD)
                                            .child("RECOMMENDED"),
                                    )
                                })
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.prompt_input
                                        .update(cx, |input, cx| input.set_text(letter.clone(), cx));
                                    this.focus_dialog_now(window, cx);
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                }
            }))
            .into_any_element()
    }

    /// C, the conservative look: the monospace lines as written, with the
    /// Markdown markers hidden, the labels in the accent color and some
    /// space before each part.
    fn conservative(&self, text: &str) -> AnyElement {
        let colors = &self.colors;
        let ink = Ink {
            text: colors.roles.text,
            code: colors.roles.merge,
            code_fill: None,
            link: colors.roles.accent,
            code_font: None,
            bold: FontWeight::BOLD,
        };
        let mut first = true;
        div()
            .flex()
            .flex_col()
            .line_height(px(13.0 * 1.7))
            .text_color(colors.roles.text)
            .children(text.lines().map(|line| -> AnyElement {
                let at_top = std::mem::take(&mut first);
                if line.trim().is_empty() {
                    return div().h(px(8.)).into_any_element();
                }
                if at_top && let Some(rest) = line.trim_start().strip_prefix('#') {
                    let heading = Heading::parse(rest.trim_start_matches('#').trim());
                    return div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_x_2()
                        .pb_1()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .child(self.inline(&flat(&heading.title), &ink)),
                        )
                        .children(heading.tag.map(|tag| {
                            div()
                                .text_color(colors.roles.warning)
                                .child(format!("[{tag}]"))
                        }))
                        .into_any_element();
                }
                if let Some((label, rest)) = label_of(line) {
                    return div()
                        .flex()
                        .flex_row()
                        .pt(px(6.))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.roles.accent)
                                .child(format!("{label}: ")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(self.inline(&flat(rest), &ink)),
                        )
                        .into_any_element();
                }
                // A hanging indent for options and list items.
                let indent = line.len() - line.trim_start().len();
                let trimmed = line.trim_start();
                let (marker, rest, accent) = if let Some((letter, rest, _)) = choice_of(line) {
                    (format!("{letter}. "), rest, true)
                } else if let Some(rest) = trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("* "))
                {
                    ("• ".to_string(), rest, false)
                } else {
                    (String::new(), trimmed, false)
                };
                div()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_none()
                            .when(accent, |d| {
                                d.font_weight(FontWeight::BOLD)
                                    .text_color(colors.roles.accent)
                            })
                            .when(!accent, |d| {
                                d.text_color(colors.secondary(colors.roles.popup_fill))
                            })
                            .child(format!("{}{marker}", " ".repeat(indent))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.inline(&flat(rest), &ink)),
                    )
                    .into_any_element()
            }))
            .into_any_element()
    }
}

/// An option's letter on a filled square.
fn letter_badge(letter: &str, fill: Hsla, text: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .mt(px(2.))
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(fill)
        .text_color(text)
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .child(letter.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q45: &str = "### Q45: Where?   [blocks: plan \"popup\"]
Context: The list is built from `questions`.
Options:
  A. **The supervisor saves it:** to `questions/Q45.md`.
     - and a nested bullet
  B. **Corgi searches** the transcript.
Recommendation: A. It's the most reliable source.
Until answered: no brief.";

    #[test]
    fn the_supervisors_form_splits_into_its_parts() {
        let form = Form::parse(Q45);
        assert_eq!(
            form.heading,
            Some(Heading {
                title: "Q45: Where?".into(),
                tag: Some("blocks: plan \"popup\"".into()),
            })
        );
        assert!(form.is_whole());
        assert_eq!(form.recommended(), Some("A"));
        let Part::Options(choices) = &form.parts[1] else {
            panic!("{:?}", form.parts);
        };
        assert_eq!(
            choices[0].text,
            "**The supervisor saves it:** to `questions/Q45.md`.\n- and a nested bullet"
        );
        assert_eq!(choices[1].letter, "B");
    }

    #[test]
    fn markdown_keeps_lines_and_marks_code_and_bold() {
        let blocks = blocks("**Bold** and `code`\nnext line\n- one\n- two");
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(inline.text, "Bold and code\nnext line");
        assert!(inline.spans[0].1.bold);
        assert!(inline.spans[2].1.code);
        assert!(matches!(&blocks[1], Block::List { items, .. } if items.len() == 2));
    }

    #[test]
    fn text_off_the_form_is_free_markdown() {
        let form = Form::parse("Just a note\nwith two lines");
        assert!(!form.is_whole());
        assert_eq!(
            form.parts,
            vec![Part::Free("Just a note\nwith two lines".into())]
        );
    }
}
