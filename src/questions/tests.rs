use super::*;
use crate::test_support::ScratchDir;

const Q7: &str =
    "### Q7: Keep the old column names for one release?   [blocks: Rename the export columns merge]
Context: The export's columns get clearer names (cust_id becomes
customer_id). Two scheduled reports read the CSV by column name.
Options:
  A. Write both names for one release: the reports keep working; the CSV is
     wider until 0.9
  B. Rename outright: simpler; both reports break
  C. Keep the old names: nothing breaks
Recommendation: A, because nothing breaks, and the old names have a date
to go.
Until answered: The worker waits at the merge.
";

#[test]
fn a_question_in_the_roles_form_is_parsed_part_by_part() {
    let q = parse("Q7", 7, Q7);
    assert!(q.parsed);
    assert_eq!(q.title, "Keep the old column names for one release?");
    assert_eq!(
        q.blocks,
        Some(Blocks {
            kind: "merge",
            what: "Rename the export columns".into()
        })
    );
    assert_eq!(
        q.context,
        "The export's columns get clearer names (cust_id becomes customer_id). Two scheduled \
         reports read the CSV by column name."
    );
    assert_eq!(q.options.len(), 3);
    assert_eq!(
        q.options[0],
        Choice {
            key: "A".into(),
            choice: "Write both names for one release".into(),
            cost: "the reports keep working; the CSV is wider until 0.9".into(),
        }
    );
    assert_eq!(q.options[2].cost, "nothing breaks");
    assert_eq!(q.recommended.as_deref(), Some("A"));
    assert_eq!(
        q.recommendation,
        "A, because nothing breaks, and the old names have a date to go."
    );
    assert_eq!(q.until, "The worker waits at the merge.");
    assert_eq!(q.raw, Q7.trim());
}

#[test]
fn the_blocks_tag_is_a_merge_a_plan_or_none() {
    let tag = |heading: &str| parse("Q2", 2, &format!("{heading}\nOptions:\n  A. Yes\n")).blocks;
    assert_eq!(
        tag("### Q2: Go?   [blocks: importer-retries]"),
        Some(Blocks {
            kind: "plan",
            what: "importer-retries".into()
        })
    );
    assert_eq!(
        tag("### Q2: Go? [no block]"),
        Some(Blocks {
            kind: "none",
            what: String::new()
        })
    );
    assert_eq!(tag("### Q2: Go?"), None);
}

#[test]
fn a_question_that_strays_from_the_form_keeps_its_markdown() {
    let q = parse("Q3", 3, "Should we drop Node 20?\nI think so.\n");
    assert!(!q.parsed);
    assert_eq!(q.title, "");
    assert_eq!(q.raw, "Should we drop Node 20?\nI think so.");
    // A heading with no options is no question to answer by letter either.
    let q = parse("Q4", 4, "### Q4: Drop Node 20?\nContext: CI is slow.\n");
    assert!(!q.parsed);
    assert_eq!(q.title, "Drop Node 20?");
    // A recommendation of a letter no option has names none.
    let q = parse(
        "Q5",
        5,
        "### Q5: X?\nOptions:\n  A. One\nRecommendation: C.\n",
    );
    assert!(q.parsed);
    assert_eq!(q.recommended, None);
}

#[test]
fn the_questions_folder_is_read_by_number_and_other_files_are_skipped() {
    let scratch = ScratchDir::new("questions-read");
    let dir = scratch.join("state");
    fs::create_dir_all(dir.join(QUESTIONS_DIR)).unwrap();
    fs::write(dir.join(QUESTIONS_DIR).join("Q10.md"), "### Q10: Later?\n").unwrap();
    fs::write(dir.join(QUESTIONS_DIR).join("Q7.md"), Q7).unwrap();
    fs::write(dir.join(QUESTIONS_DIR).join("notes.md"), "not a question").unwrap();
    fs::write(dir.join(QUESTIONS_DIR).join("Qx.md"), "not one either").unwrap();
    let ids: Vec<String> = read(&dir).into_iter().map(|q| q.id).collect();
    assert_eq!(ids, ["Q7", "Q10"]);
    assert!(read(&scratch.join("none")).is_empty());
}

#[test]
fn an_answer_is_a_key_or_text_of_the_users_own_on_one_line() {
    let q = parse("Q7", 7, Q7);
    assert_eq!(normalize(&q, "b"), Ok("B".into()));
    assert_eq!(normalize(&q, "D"), Err("Q7 has no option D".into()));
    assert_eq!(
        normalize(&q, "other: keep both,\nbut only until March"),
        Ok("other: keep both, but only until March".into())
    );
    assert_eq!(normalize(&q, "Keep both"), Ok("other: Keep both".into()));
    assert!(normalize(&q, "  ").is_err());
    assert!(normalize(&q, "other:  ").is_err());
    assert!(normalize(&q, &"x".repeat(OTHER_MAX + 1)).is_err());
    // A question that could not be parsed takes any letter.
    let raw = parse("Q3", 3, "Should we?");
    assert_eq!(normalize(&raw, "d"), Ok("D".into()));
}

#[test]
fn the_line_says_the_answer_is_the_users_and_names_the_choice() {
    let q = parse("Q7", 7, Q7);
    assert_eq!(
        answer_text(&q, "A"),
        "[Corgi] The user's answer to Q7 (Keep the old column names for one release?), given in \
         the Herdr app's Questions panel: \"Q7: A, Write both names for one release\". Handle it \
         as if they had typed it."
    );
    assert_eq!(
        answer_text(&q, "other: keep both"),
        "[Corgi] The user's answer to Q7 (Keep the old column names for one release?), given in \
         the Herdr app's Questions panel: \"Q7: other: keep both\". Handle it as if they had \
         typed it."
    );
    assert_eq!(
        start_text("Export to\nParquet"),
        "[Corgi] The user's request, from the Herdr app's backlog: plan the backlog entry \
         \"Export to Parquet\". Propose the plan to them; start nothing until they agree."
    );
}

#[test]
fn an_answer_is_queued_until_delivered_and_a_newer_one_replaces_it() {
    let scratch = ScratchDir::new("questions-answer");
    let dir = scratch.join("state");
    fs::create_dir_all(&dir).unwrap();
    let root = "/repos/weather";
    let send = |answer: &str| {
        queue(
            &dir,
            root,
            "answer",
            "Q7",
            answer,
            answer_key("Q7"),
            format!("[Corgi] Q7: {answer}"),
        )
        .unwrap()
    };
    let first = send("A");
    let latest_of = |kind| latest(&dir, kind);
    assert_eq!(latest_of("answer").len(), 1);
    assert_eq!(latest_of("answer")[0].1, AnswerState::Queued);
    let second = send("B");
    let found = latest_of("answer");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0.answer, "B");
    assert_eq!(found[0].0.item, second.item);
    // Both wait; only the newer goes out, as one item of their key.
    let inbox = Inbox::read(&dir);
    let pending = inbox.undelivered_for(root);
    assert_eq!(pending.len(), 2);
    assert_eq!(inbox::prompt_text(&pending), "[Corgi] Q7: B");
    inbox::mark_delivered(&dir, &[first.item, second.item], "supervisor-weather").unwrap();
    assert_eq!(latest_of("answer")[0].1, AnswerState::Sent);
    assert!(latest_of("start").is_empty());
    let value = state_value(latest_of("answer").first());
    assert_eq!(value["answer"], "B");
    assert_eq!(value["state"], "sent");
    assert_eq!(state_value(None), Value::Null);
}

#[test]
fn the_role_takes_the_answer_line_for_the_users_own() {
    let line = answer_text(&parse("Q7", 7, Q7), "A");
    assert!(line.starts_with("[Corgi] The user's answer to Q7"));
    assert!(crate::supervisor::ROLE.contains(
        "unquoted `[Corgi]` lines come from Corgi, not the user, but one giving\n\
         \"The user's answer\" is theirs, as if typed."
    ));
}
