use super::*;
use crate::terminal::selection::tests::{frame, pane};

fn link(rows: &[&str], column: u16, row: u16) -> Option<Link> {
    let frame = frame(rows);
    at(&frame, &Region::of(&pane(&frame, 0, 0)), column, row)
}

#[test]
fn a_printed_web_address_is_found_without_the_punctuation_after_it() {
    let rows = &["see https://example.com/a?b=1. ok"];
    let found = link(rows, 10, 0).unwrap();
    assert_eq!(found.url, "https://example.com/a?b=1");
    assert_eq!(found.cells, vec![(0, 4..29)]);
    assert_eq!(link(rows, 1, 0), None, "no link on plain text");
    assert_eq!(link(rows, 31, 0), None);
    // Brackets it did not open are not its own; ones it opened are.
    let wiki = &["(https://en.wikipedia.org/wiki/Corgi_(dog))"];
    assert_eq!(
        link(wiki, 5, 0).unwrap().url,
        "https://en.wikipedia.org/wiki/Corgi_(dog)"
    );
    let quoted = &["\"http://localhost:8080/x\""];
    assert_eq!(link(quoted, 3, 0).unwrap().url, "http://localhost:8080/x");
}

#[test]
fn an_address_that_runs_to_the_edge_goes_on_on_the_next_row() {
    // Wrapped at the pane's width, as Claude Code's long links are.
    let rows = &["Docs: https://code.claude", ".com/docs/en/overview    "];
    let first = link(rows, 10, 0).unwrap();
    let second = link(rows, 3, 1).unwrap();
    assert_eq!(first.url, "https://code.claude.com/docs/en/overview");
    assert_eq!(first, second, "either row opens the whole address");
    assert_eq!(first.cells, vec![(0, 6..25), (1, 0..21)]);
    // A row that stops short of the edge ends the address.
    let rows = &["https://a.example  ", "next line          "];
    assert_eq!(link(rows, 2, 0).unwrap().url, "https://a.example");
}

#[test]
fn a_hyperlink_a_program_set_takes_its_cells() {
    let mut frame = frame(&["click here please"]);
    frame.hyperlinks = vec!["https://example.com/osc8".into()];
    for cell in &mut frame.cells[6..10] {
        cell.hyperlink = Some(0);
    }
    let region = Region::of(&pane(&frame, 0, 0));
    let found = at(&frame, &region, 7, 0).unwrap();
    assert_eq!(found.url, "https://example.com/osc8");
    assert_eq!(found.cells, vec![(0, 6..10)]);
    assert_eq!(at(&frame, &region, 2, 0), None);
}

#[test]
fn only_web_and_mail_addresses_open() {
    assert!(openable("https://example.com"));
    assert!(openable("HTTP://example.com/x"));
    assert!(openable("mailto:someone@example.com"));
    assert!(!openable("file:///etc/passwd"));
    assert!(!openable("javascript:alert(1)"));
    assert!(!openable("https://"));
    assert!(!openable("https://exa mple.com"));
}
