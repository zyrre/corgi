//! A question's full text in the answer dialog, as the supervisor wrote it
//! (`supervisor/ROLE.md`, "Questions to the user"), drawn as a document: the
//! heading as a title with its `[blocks: …]` tag as a chip, the `Context:`,
//! `Options:`, `Recommendation:` and `Until answered:` labels in bold, each
//! option beside its letter, and the Markdown inside them rendered in the
//! proportional font, code in the window's. Text off that form is plain
//! Markdown, its line breaks kept.

use std::ops::Range;

use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, StrikethroughStyle, Styled, StyledText,
    UnderlineStyle, div, px, relative,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::window::CorgiWindow;

/// The body text's size, and its line height in sizes.
const BODY_SIZE: f32 = 14.0;
const BODY_LINE: f32 = 1.55;
/// The heading's size.
const TITLE_SIZE: f32 = 17.0;
/// What goes either side of inline code, inside its fill.
const CODE_PAD: char = '\u{2009}';
/// What follows a `/` in inline code, so a path wraps as `src/serve/` and
/// `state.rs`: GPUI breaks before any character it does not count as part
/// of a word, the `/` itself included, and finds them among the shaped
/// glyphs, so a zero-width space, which shapes to none, is not seen.
const AFTER_SLASH: char = '\u{200A}';

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
}

impl Heading {
    /// A heading's line, its `#`s gone; only the form's own tags,
    /// `[blocks: …]` and `[no block]`, become a chip.
    fn parse(line: &str) -> Self {
        if line.ends_with(']')
            && let Some(open) = line.rfind('[')
        {
            let title = line[..open].trim_end();
            let tag = line[open + 1..line.len() - 1].trim();
            let lower = tag.to_ascii_lowercase();
            if !title.is_empty() && (lower.starts_with("blocks") || lower.starts_with("no block")) {
                return Heading {
                    title: title.to_string(),
                    tag: Some(tag.to_string()),
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

/// How marked text is set: its color, and inline code's fill.
struct Ink {
    text: Hsla,
    code_fill: Option<Hsla>,
}

impl CorgiWindow {
    /// A question as the supervisor asked it, rendered; the dialog gives
    /// it what height the answer field and the hints leave, and it scrolls
    /// beyond that.
    pub fn question_text(&self, text: &str) -> AnyElement {
        let colors = &self.colors;
        div()
            .id("question-text")
            .flex_shrink(1.)
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.question_scroll)
            .px_3()
            .py_2()
            .rounded_sm()
            .border_1()
            .border_color(colors.roles.rule)
            .font_family(self.ui_font.clone())
            .text_size(px(BODY_SIZE))
            .line_height(relative(BODY_LINE))
            .text_color(colors.roles.text)
            .child(self.document(&Form::parse(text)))
            .into_any_element()
    }

    fn body_ink(&self) -> Ink {
        Ink {
            text: self.colors.roles.text,
            code_fill: Some(self.colors.roles.keycap.opacity(0.6)),
        }
    }

    /// Marked text in the inherited font, its code in the window's.
    fn inline(&self, inline: &Inline, ink: &Ink) -> StyledText {
        let link = self.colors.roles.accent;
        let mut text = String::with_capacity(inline.text.len());
        let mut highlights = Vec::new();
        let mut code_ranges = Vec::new();
        for (range, marks) in &inline.spans {
            let piece = &inline.text[range.clone()];
            let start = text.len();
            if marks.code {
                text.push(CODE_PAD);
                for c in piece.chars() {
                    text.push(c);
                    if c == '/' {
                        text.push(AFTER_SLASH);
                    }
                }
                text.push(CODE_PAD);
                code_ranges.push((start..text.len(), self.font.family.clone()));
            } else {
                text.push_str(piece);
            }
            let style = HighlightStyle {
                color: Some(if marks.link { link } else { ink.text }),
                font_weight: marks.bold.then_some(FontWeight::SEMIBOLD),
                font_style: marks.italic.then_some(FontStyle::Italic),
                background_color: if marks.code { ink.code_fill } else { None },
                underline: marks.link.then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(link),
                    wavy: false,
                }),
                strikethrough: marks.strike.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(ink.text),
                }),
                fade_out: None,
            };
            highlights.push((start..text.len(), style));
        }
        StyledText::new(text)
            .with_highlights(highlights)
            .with_font_family_overrides(code_ranges)
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
                        .font_weight(FontWeight::BOLD)
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
                        .bg(colors.roles.keycap.opacity(0.6))
                        .font(self.font.clone())
                        .text_size(px(13.))
                        .line_height(relative(1.45))
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
                let color = if tag.to_ascii_lowercase().starts_with("no block") {
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
                    .text_size(px(12.))
                    .line_height(relative(1.4))
                    .child(self.inline(
                        &flat(tag),
                        &Ink {
                            text: color,
                            code_fill: None,
                        },
                    ))
            }))
    }

    /// The form as a document: the title and chip, bold labels, the options
    /// as a list with their letters as badges.
    fn document(&self, form: &Form) -> AnyElement {
        let ink = self.body_ink();
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
- one bullet
Options:
  A. **The supervisor saves it:** to `questions/Q45.md`.
     - and a nested bullet
  B. **Corgi searches** the transcript.
Recommendation: A. It's the most reliable source.
Until answered: no brief.";

    fn labeled(label: &str, body: &str) -> Part {
        Part::Labeled {
            label: label.into(),
            body: body.into(),
        }
    }

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
        assert_eq!(
            form.parts,
            vec![
                labeled(
                    "Context",
                    "The list is built from `questions`.\n- one bullet"
                ),
                Part::Options(vec![
                    Choice {
                        letter: "A".into(),
                        // Dedented to the option's text, so the bullet
                        // stays a list rather than a continued line.
                        text: "**The supervisor saves it:** to `questions/Q45.md`.\n\
                               - and a nested bullet"
                            .into(),
                    },
                    Choice {
                        letter: "B".into(),
                        text: "**Corgi searches** the transcript.".into(),
                    },
                ]),
                labeled("Recommendation", "A. It's the most reliable source."),
                labeled("Until answered", "no brief."),
            ]
        );
        let Part::Options(choices) = &form.parts[1] else {
            unreachable!()
        };
        let option = blocks(&choices[0].text);
        assert!(matches!(&option[1], Block::List { items, .. } if items.len() == 1));
    }

    #[test]
    fn only_the_forms_own_tags_become_a_chip() {
        let tag = |line: &str| Heading::parse(line).tag;
        assert_eq!(tag("Q2: Keep it?  [no block]"), Some("no block".into()));
        assert_eq!(
            tag("Q3: Which? [blocks: merge]"),
            Some("blocks: merge".into())
        );
        assert_eq!(tag("See the [docs]"), None);
        assert_eq!(tag("[blocks: x]"), None);
    }

    #[test]
    fn markdown_keeps_lines_and_marks_code_and_bold() {
        let blocks = blocks("**Bold** and `code`\nnext line\n- one\n- two");
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(inline.text, "Bold and code\nnext line");
        assert!(inline.spans[0].1.bold);
        assert_eq!(&inline.text[inline.spans[2].0.clone()], "code");
        assert!(inline.spans[2].1.code);
        assert!(matches!(&blocks[1], Block::List { items, .. } if items.len() == 2));
    }

    #[test]
    fn text_off_the_form_is_plain_markdown() {
        let text = "A note, not a question.\nIts second line.\n\n\
                    1. first\n2. second\n\n```sh\ncargo test\n```\n\n> quoted\n\n---";
        let form = Form::parse(text);
        assert_eq!(form.heading, None);
        assert_eq!(form.parts, vec![Part::Free(text.into())]);
        let blocks = blocks(text);
        assert!(
            matches!(&blocks[0], Block::Paragraph(inline) if inline.text == "A note, not a question.\nIts second line.")
        );
        assert!(matches!(&blocks[1], Block::List { start: Some(1), items } if items.len() == 2));
        assert_eq!(blocks[2], Block::Code("cargo test".into()));
        assert!(matches!(&blocks[3], Block::Quote(inner) if inner.len() == 1));
        assert_eq!(blocks[4], Block::Rule);
    }

    #[test]
    fn a_heading_off_the_form_is_a_title_and_the_rest_markdown() {
        let form = Form::parse("## Notes\nSome **text**.");
        assert_eq!(
            form.heading,
            Some(Heading {
                title: "Notes".into(),
                tag: None,
            })
        );
        assert_eq!(form.parts, vec![Part::Free("Some **text**.".into())]);
    }
}
