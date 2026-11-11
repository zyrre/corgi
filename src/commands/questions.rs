//! `corgi questions`, `corgi backlog` and `corgi answer`: a supervisor's
//! open questions and backlog as JSON, and the user's answers and backlog
//! Starts sent back to it ([`crate::questions`]), for the Herdr app's
//! Questions panel.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    backlog,
    digest::lookup::QUESTIONS_DIR,
    questions::{self, AnswerState, Question, Record},
};

use super::project_root;

pub const QUESTIONS_USAGE: &str = "\
Usage: corgi questions [--project DIR] [--json]

Prints the open questions of the supervisor of the project DIR (default: the
project containing the current directory) as JSON: each questions/Q<n>.md
of its state directory, parsed (title, what it blocks, context, options,
recommendation, until answered; parsed is false, and raw holds the
Markdown, for a file that strays from that form), with the answer given in
the Herdr app (queued until the supervisor is between turns, then sent),
by number. Reads only.";

pub const BACKLOG_USAGE: &str = "\
Usage: corgi backlog [--project DIR] [--json]

Prints the backlog of the supervisor of the project DIR (default: the
project containing the current directory) as JSON: its title and intro and
one entry per ## heading, with its body and the Start asked for it in the
Herdr app (queued, then sent). Reads only.";

pub const ANSWER_USAGE: &str = "\
Usage: corgi answer [--project DIR] [--json] Q<n>=<answer>
       corgi answer [--project DIR] [--json] --start HEADING

Sends the user's answer to an open question to the supervisor of the
project DIR (default: the project containing the current directory): an
option's letter (Q7=B), or text of their own (Q7=\"other: …\"). It goes into
the supervisor's inbox as one [Corgi] line saying the answer is the user's,
which corgi daemon types into its box once it is between turns and its box
holds no draft; a newer answer to the same question replaces one not sent
yet. --start HEADING asks the supervisor, the same way, to plan the backlog
entry HEADING and propose the plan (nothing is started). Each is recorded
in answers.jsonl in the supervisor's state directory, which corgi questions
and corgi backlog read for whether it is queued or sent. --json prints
{\"result\": \"queued\" | \"refused\", \"message\", \"key\"}, and exits 0 either way.";

/// `--project DIR` and `--json`, the options the three share, and the
/// rest.
struct Args<'a> {
    project: Option<&'a str>,
    json: bool,
    rest: Vec<&'a str>,
}

fn args<'a>(args: &'a [String], usage: &str) -> Result<Args<'a>> {
    let mut parsed = Args {
        project: None,
        json: false,
        rest: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--project" => {
                parsed.project = Some(
                    iter.next()
                        .with_context(|| format!("--project needs a value\n\n{usage}"))?,
                );
            }
            "--json" => parsed.json = true,
            other => parsed.rest.push(other),
        }
    }
    Ok(parsed)
}

/// The project's root and its supervisor's state directory.
fn project(project: Option<&str>) -> Result<(String, std::path::PathBuf)> {
    let root = project_root(project)?;
    let dir = crate::supervisor::state_dir(&root)?;
    Ok((root, dir))
}

/// A question as `corgi questions --json` prints it.
fn question_value(
    dir: &Path,
    question: &Question,
    answer: Option<&(Record, AnswerState)>,
) -> Value {
    json!({
        "id": question.id,
        "number": question.number,
        "title": question.title,
        "blocks": question.blocks,
        "context": question.context,
        "options": question.options,
        "recommended": question.recommended,
        "recommendation": question.recommendation,
        "until": question.until,
        "parsed": question.parsed,
        "raw": question.raw,
        "file": dir.join(QUESTIONS_DIR).join(format!("{}.md", question.id)),
        "answer": questions::state_value(answer),
    })
}

/// What `corgi questions --json` prints for the state directory `dir`.
pub(crate) fn questions_value(root: &str, dir: &Path) -> Value {
    let answers = questions::latest(dir, "answer");
    let list: Vec<Value> = questions::read(dir)
        .iter()
        .map(|question| {
            let answer = answers.iter().find(|(r, _)| r.subject == question.id);
            question_value(dir, question, answer)
        })
        .collect();
    let open = list.iter().filter(|q| q["answer"].is_null()).count();
    json!({ "project": root, "state_dir": dir, "open": open, "questions": list })
}

/// What `corgi backlog --json` prints for the state directory `dir`.
pub(crate) fn backlog_value(root: &str, dir: &Path) -> Value {
    let starts = questions::latest(dir, "start");
    let (text, cut) = backlog::read_text(dir).unwrap_or_default();
    let (title, intro) = backlog::head(&text);
    let entries: Vec<Value> = backlog::entries_of(&text, cut)
        .into_iter()
        .map(|entry| {
            let start = starts.iter().find(|(r, _)| r.subject == entry.heading);
            json!({
                "heading": entry.heading,
                "body": entry.text,
                "start": questions::state_value(start),
            })
        })
        .collect();
    json!({ "project": root, "title": title, "intro": intro, "entries": entries })
}

pub fn questions_command(args: &[String]) -> Result<()> {
    let parsed = self::args(args, QUESTIONS_USAGE)?;
    if let Some(other) = parsed.rest.first() {
        bail!("unknown argument {other:?}\n\n{QUESTIONS_USAGE}");
    }
    let (root, dir) = project(parsed.project)?;
    println!("{}", serde_json::to_string(&questions_value(&root, &dir))?);
    Ok(())
}

pub fn backlog_command(args: &[String]) -> Result<()> {
    let parsed = self::args(args, BACKLOG_USAGE)?;
    if let Some(other) = parsed.rest.first() {
        bail!("unknown argument {other:?}\n\n{BACKLOG_USAGE}");
    }
    let (root, dir) = project(parsed.project)?;
    println!("{}", serde_json::to_string(&backlog_value(&root, &dir))?);
    Ok(())
}

/// What `corgi answer` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask {
    /// The question's id (`Q7`) and the answer as given.
    Answer(String, String),
    /// A backlog entry's heading.
    Start(String),
}

fn ask_of(rest: &[&str]) -> Result<Ask> {
    match rest {
        ["--start", heading] => Ok(Ask::Start(heading.trim().to_string())),
        [given] if !given.starts_with('-') => {
            let (id, answer) = given
                .split_once('=')
                .with_context(|| format!("{given:?} is not Q<n>=<answer>\n\n{ANSWER_USAGE}"))?;
            let id = id.trim().to_ascii_uppercase();
            anyhow::ensure!(
                id.len() > 1 && id.starts_with('Q') && id[1..].bytes().all(|b| b.is_ascii_digit()),
                "{id:?} is not a question's number, such as Q7\n\n{ANSWER_USAGE}"
            );
            Ok(Ask::Answer(id, answer.to_string()))
        }
        _ => bail!("{ANSWER_USAGE}"),
    }
}

/// Queues what `ask` asks for in the inbox of the project `root` (state
/// directory `dir`): the record, or why nothing was queued.
pub(crate) fn answer(root: &str, dir: &Path, ask: &Ask) -> Result<Result<Record, String>> {
    match ask {
        Ask::Answer(id, given) => {
            let Some(question) = questions::read(dir).into_iter().find(|q| &q.id == id) else {
                return Ok(Err(format!(
                    "{id} is not open: the supervisor has no questions/{id}.md"
                )));
            };
            let answer = match questions::normalize(&question, given) {
                Ok(answer) => answer,
                Err(why) => return Ok(Err(why)),
            };
            let text = questions::answer_text(&question, &answer);
            questions::queue(
                dir,
                root,
                "answer",
                id,
                &answer,
                questions::answer_key(id),
                text,
            )
            .map(Ok)
        }
        Ask::Start(heading) => {
            if !backlog::read(dir).iter().any(|e| &e.heading == heading) {
                return Ok(Err(format!("the backlog has no entry {heading:?}")));
            }
            questions::queue(
                dir,
                root,
                "start",
                heading,
                "",
                questions::start_key(heading),
                questions::start_text(heading),
            )
            .map(Ok)
        }
    }
}

pub fn answer_command(args: &[String]) -> Result<()> {
    let parsed = self::args(args, ANSWER_USAGE)?;
    let ask = ask_of(&parsed.rest)?;
    let outcome = project(parsed.project).and_then(|(root, dir)| answer(&root, &dir, &ask));
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) if parsed.json => Err(format!("{error:#}")),
        Err(error) => return Err(error),
    };
    match (outcome, parsed.json) {
        (Ok(record), true) => println!(
            "{}",
            json!({
                "result": "queued",
                "message": "Queued: it goes to the supervisor when it is between turns.",
                "item": record.item,
                "subject": record.subject,
                "answer": record.answer,
            })
        ),
        (Ok(record), false) => println!(
            "Queued {}: it goes to the supervisor when it is between turns.",
            record.subject
        ),
        (Err(why), true) => println!("{}", json!({ "result": "refused", "message": why })),
        (Err(why), false) => bail!("{why}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;
    use std::fs;

    #[test]
    fn answers_and_starts_go_to_open_questions_and_known_entries_and_show_as_queued() {
        let scratch = ScratchDir::new("answer-command");
        let dir = scratch.join("state");
        fs::create_dir_all(dir.join(QUESTIONS_DIR)).unwrap();
        fs::write(
            dir.join(QUESTIONS_DIR).join("Q7.md"),
            "### Q7: Keep it?   [no block]\nOptions:\n  A. Yes: fine\n  B. No: also fine\n",
        )
        .unwrap();
        fs::write(dir.join(QUESTIONS_DIR).join("Q8.md"), "### Q8: And this?\n").unwrap();
        fs::write(
            dir.join("backlog.md"),
            "# Later\nThings.\n\n## Export to Parquet\n- asked for\n\n## Dark charts\n",
        )
        .unwrap();
        let root = "/repos/weather";
        let before = questions_value(root, &dir);
        assert_eq!(before["open"], 2);
        assert_eq!(before["questions"][0]["options"][1]["choice"], "No");
        assert_eq!(before["questions"][1]["parsed"], false);

        let ask =
            |given: &str| answer(root, &dir, &Ask::Answer("Q7".into(), given.into())).unwrap();
        assert_eq!(ask("C"), Err("Q7 has no option C".into()));
        assert_eq!(ask("b").unwrap().answer, "B");
        assert_eq!(
            answer(root, &dir, &Ask::Answer("Q9".into(), "A".into())).unwrap(),
            Err("Q9 is not open: the supervisor has no questions/Q9.md".into())
        );
        let after = questions_value(root, &dir);
        assert_eq!(after["open"], 1);
        assert_eq!(after["questions"][0]["answer"]["state"], "queued");
        assert_eq!(after["questions"][0]["answer"]["answer"], "B");
        assert!(after["questions"][1]["answer"].is_null());

        assert!(
            answer(root, &dir, &Ask::Start("Nope".into()))
                .unwrap()
                .is_err()
        );
        answer(root, &dir, &Ask::Start("Export to Parquet".into()))
            .unwrap()
            .unwrap();
        let backlog = backlog_value(root, &dir);
        assert_eq!(backlog["title"], "Later");
        assert_eq!(backlog["intro"], "Things.");
        assert_eq!(backlog["entries"][0]["body"], "- asked for");
        assert_eq!(backlog["entries"][0]["start"]["state"], "queued");
        assert!(backlog["entries"][1]["start"].is_null());
    }

    #[test]
    fn an_answer_names_its_question_and_a_start_its_heading() {
        assert_eq!(
            ask_of(&["q7=b"]).unwrap(),
            Ask::Answer("Q7".into(), "b".into())
        );
        assert_eq!(
            ask_of(&["Q12=other: keep both"]).unwrap(),
            Ask::Answer("Q12".into(), "other: keep both".into())
        );
        assert_eq!(
            ask_of(&["--start", "Export to Parquet"]).unwrap(),
            Ask::Start("Export to Parquet".into())
        );
        assert!(ask_of(&["X7=B"]).is_err());
        assert!(ask_of(&["Q7"]).is_err());
        assert!(ask_of(&[]).is_err());
    }
}
