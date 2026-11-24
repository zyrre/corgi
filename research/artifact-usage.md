# Which of Corgi's saved artifacts are used, and are their limits right

Measured 2026-11-24 on this machine: four supervisor state directories
(`corgi`, and three of the user's other projects, called A, B and C here) and
every transcript still on disk (2026-10-20 to 2026-11-24, 288 Claude Code
transcripts, 66 of them supervisor sessions). Nothing was changed anywhere.
Numbers for A, B and C are aggregates only.

## Summary

**The question "it says *logged as decision*; is a decision read afterwards?"**
Of 196 decisions whose writing is in the transcripts, 38 (19%) were looked up
later by a command; 132 (67%) were only ever seen in the session-start
digest, in full a median of 2 to 4 times; 26 (13%) were neither shown in full
nor looked up. Once a decision leaves the digest's full-text part it is
almost never read: 125 of the 160 that left it (78%) were not looked up
again. Briefs do carry decisions (58% of `corgi` briefs name one), but the
decision they name is a median of 0 days old: it was written in the same
conversation as the brief. Only 12 of 121 named decisions were more than a
week old. So the log works as a record of the current week. As long-term
memory it is used little: about ten look-ups a week in `corgi` and A, three
in B.

**What works, by the numbers and the samples:**

- The handover note. 66 of 67 open questions handed on were taken up by the
  next session (56 restated in full in its first reply); all 13 one-off
  promises checked by hand in `corgi` were kept. No session was ever
  compacted (0 of 66).
- The ledger in `corgi`: all 141 ids marked `merged` that Git can check were
  merged; nothing is left `reported`.
- `Learned` lines: 87 of 106 final reports named a lesson, 64 of those 76
  merges changed AGENTS.md or ARCHITECTURE.md, and for the two lessons with
  a checkable signature the pitfall stopped (`timeout`: 6 of 27 worker
  transcripts before, none after the first two days; kill by pattern: 6 of
  35 before, 1 of 82 after).
- `--search`: 94 runs, one without hits, six hits shown each time; in
  `corgi` the next step is a brief or a spawn after 16 of 35.

**What does not, most important first:**

1. **AGENTS.md has no limit and has grown twelvefold in six weeks** (6 KB to
   75.6 KB, about 12 KB a week, fed by the lessons). Claude Code does not
   load it (the repository's CLAUDE.md only points at it), so agents read it
   by hand: workers read all of it until mid-November and a median of 50% of
   its lines since (11 of 21 read under half). Each lesson added makes the
   others less likely to be read.
2. **The digest spends its decision budget on the wrong half.** Full text
   takes 8 KB and covers six days; titles get 3 KB and stop after 35, leaving
   126 older titles out in `corgi` and 91 in A. But 75 of 108 look-ups were
   of a decision the digest showed as a title, 24 of one it did not show,
   and only 9 of one shown in full. Titles are what prompts a look-up.
   Proposed: 4 KB of full text, 7 KB of titles.
3. **A third of `corgi`'s wakes cannot carry their report** (89 of 267),
   because the report is over 4 KB; the supervisor then runs `report` (171
   such runs across projects). 8 KB would carry 99% of reports.
4. **Digest warnings and the stale-`reported` list are not acted on.** In A,
   three warnings were shown in 21, 16 and 12 digests in a row and two are
   still there; a 2 KB list of about 50 stale ids was shown 13 times and 2
   ids were ever looked up.
5. **A question was asked that the log had already answered** (Q128,
   2026-11-23, against `decisions.md:359` of 2026-10-20): the decision was
   34 days old, the titles reached back 13 days, and the supervisor asked
   without searching. A search on a copy finds it.
6. **Agent memory in `corgi`: 4 of 13 entries are stale, 4 repeat the
   repository, one index line contradicts its entry.** The index is loaded
   into every worker and supervisor session of the project.

Nothing should stop being written, except the model-choice note in ledger
outcomes, which no code or session reads. The recommendations are at the
end, each marked as following from numbers or from a sample.

## What a session costs in context

Measured from the transcripts' token counts. Tool output of this kind (ids,
paths, dates) costs about 2.4 bytes per token here (median over 160 `report`
and 56 `--search` results), prose about 3 to 3.5.

| | `corgi` | A | B |
| --- | --- | --- | --- |
| Supervisor sessions with a transcript | 29 | 31 | 6 |
| Context at the first request (system prompt, tools, ROLE.md, memory index) | 35.7k tokens | 49.3k | 36.3k |
| of which ROLE.md, 27.0 KB with its paths filled in | about 7.7k tokens (supervisor 35.7k against a worker's 28.1k) | | |
| Plain digest at session start, median | 15.7 KB, about 6.6k tokens | 22.6 KB, about 9.4k | 17.5 KB, about 7.3k |
| Peak context of a session, median / highest | 138k / 556k | 168k / 464k | 119k / 171k |
| Sessions past 200k tokens | 7 | 9 | 0 |

What the digest's bytes go to (median bytes since the bounds of 2026-10-27,
and share of all digest bytes):

| Section | `corgi` | A | B |
| --- | --- | --- | --- |
| Handover note | 3,159 (20%) | 4,574 (20%) | 4,750 (26%) |
| Open work | 365 (3%) | 3,515 (18%) | 460 (2%) |
| Recently finished | 899 (6%) | 1,901 (8%) | 974 (5%) |
| Recent decisions in full | 7,851 (49%) | 7,747 (35%) | 8,006 (46%) |
| Older decision titles | 3,103 (19%) | 3,123 (14%) | 2,908 (17%) |
| Header and footer | 566 (4%) | 980 (4%) | 618 (4%) |

A worker's session starts at 28.1k tokens; it then reads AGENTS.md itself
(a median of 19 KB per transcript, about 7k tokens).

## Per artifact

C has state but no transcripts left, so its reads are unknown ("-").
"Window" is 2026-10-20 to 2026-11-24. Weekly rates are the mean of ISO weeks
44 to 47.

### decisions.md

| | `corgi` | A | B | C |
| --- | --- | --- | --- | --- |
| Written | 191 entries, 29 a week | 153, 15 a week | 60 | 22 |
| Size | 158 KB; entry median 783 B, p90 1,196, largest 2,701 | 113 KB; 716 / 1,017 / 1,568 | 45 KB; 729 / 1,077 / 1,821 | 12 KB; 533 / 801 / 956 |
| Actively read | `--decision` 20 times in 13 sessions; 19 search hits on decisions; 7 raw reads | 14 in 10 sessions; 18; 2 | 0; 7; 3 | - |
| Decisions written in the window, looked up later | 23 of 115 | 13 of 66 | 2 of 15 | - |
| Older decisions looked up in the window | 10 of 76 | 9 of 87 | 3 of 45 | - |
| Passively seen | in full in a median of 2 digests (p90 4); 18 never in full | median 4 (p90 10); 4 never | median 2; 4 never | - |
| Left the full text and never looked up after | 78 of 101 | 44 of 55 | 3 of 4 | - |
| Effect | 126 of 218 briefs name a decision; its age: median 0 days, 12 of 121 over a week | 36 of 195; median 1 day | 24 of 40; median 3 days | 2 of 12 |
| `Supersedes:` | 18 lines, all match | 19 lines, 1 matches nothing | 8, all match | none |
| Verdict | keep; show differently (limits 2 and 3 below) | | | |

How look-ups happen (all 20 `--decision` calls in `corgi` read with their
context): 13 came while writing a brief for work that touched an older
decision, 4 answered a question of the user, 2 followed a worker's report,
1 fetched a heading for a `Supersedes:` line. Age of the decision at
look-up: median 3 days in `corgi` (p90 28), 17 days in A (28 of 46 over two
weeks).

Asked again or contradicted: one case found (Q128, under *Misses*). The
screens behind that: 78 `corgi` questions matched against earlier decisions
by their titles' words, the 22 closest read; 878 typed prompts of the user
searched for pushback ("we decided", "already", "again?" and the like), 7
hits read, none a complaint about a repeated question.

Reversals: among the 118 `corgi` decisions of the window, 17 use reversal
words without a `Supersedes:` line. Read by hand, none replaces a whole
entry without saying so. Four change part of an earlier entry and say so in
prose (`decisions.md:399`, `:594`, `:656`, `:900`); the format has no way to
supersede part of an entry, so both stay visible and the reader reconciles
them.

### ledger.jsonl and ledger-archive.jsonl

| | `corgi` | A | B | C |
| --- | --- | --- | --- | --- |
| Written | 229 ids, 911 lines, 34 new ids a week | 212 ids, 1,025 lines, 21 a week | 40 ids, 195 lines | 12 ids, 54 lines |
| Size | live 65 KB (235 lines), archive 301 KB; line median 395 B | 68 KB (213), 362 KB; 399 B | 11 KB, 63 KB | 27 KB, no archive |
| Actively read | `--ledger` 6 times; 19 raw reads (`grep` by agent, as the role says) | 42 times in 22 sessions; 57 raw reads | 1; 1 | - |
| Passively seen | open work and the 10 newest finished in every digest: 9% of it | 26% | 7% | - |
| Effect: status against Git | 141 of 141 checkable `merged` ids are merged; 41 older ones predate the repository's rewritten history; 3 ids are `done` though merged; 1 `ready-to-merge` for 6 days | Git not asked | Git not asked | - |
| Effect: left `reported` | median 0.01 days before the next status; none 2+ days | median 13 days; 58 ids 2+ days; open now: 7 `ready-to-merge` (median 24 days old), 3 `needs-answer`, 1 `reported` | median 0.04 days; 1 id 2+ days | 1 `blocked` for 50 days |
| Verdict | keep | keep; see limit 7 | keep | |

The `model` field is on 225 of 249 `dispatched` lines in `corgi`, and 17 ids
have an outcome judging the choice ("default model was right"), as the role
asks "for a later cost policy to learn from". No code reads either, and no
session searched for them.

### briefs/

| | `corgi` | A | B | C |
| --- | --- | --- | --- | --- |
| Written | 220 files, 29 a week | 198, 19 a week | 40 | 12 |
| Size | 1.06 MB; median 4.7 KB, p90 7.2, largest 14.8 | 918 KB; 4.4 / 7.0 / 12.0 | 209 KB; 5.1 / 6.9 / 8.7 | 58 KB; 4.7 / 6.3 / 8.0 |
| Actively read | `--brief` 5 times; 2 raw reads; 49 search hits shown | 26; 10; 68 | 0; 1; 9 | - |
| Passively seen | never by a supervisor; once by its worker, as the prompt | | | |
| Effect | the worker's whole instruction; re-read mostly for work just finished (17 of 21 `--brief` look-ups of finished work in A were of the 10 newest) | | | |
| Verdict | keep; no limit needed | | | |

### reports/ (kept reports) and the inbox

| | `corgi` | A | B | C |
| --- | --- | --- | --- | --- |
| Written (since 2026-10-27) | 254 files for 113 ids, 63 a week | 618 for 73 ids, 150 a week | 40 for 9 ids | none; 1 inbox report |
| Size | 781 KB; median 3.1 KB, p90 5.6, largest 12.1 | 1.04 MB; 1.1 / 4.2 / 15.1 | 68 KB; 1.3 / 4.8 / 6.0 | |
| Actively read | `report` 91 times (84 after a wake that named it); 0 raw reads of `reports/`; 68 search hits shown | 90 (80); 8; 73 | 9 (7); 1; 15 | - |
| Passively seen | quoted with the wake 178 times | 625 | 34 | - |
| `corgi inbox` (any form) | 10 runs, largest 3.9 KB | 7 | 2 | - |
| Effect | reports are the largest source of shown search hits in `corgi` and B, second in A | | | |
| Verdict | keep; raise the inline limit (limit 11) | | | |

### handover.md and handovers/

| | `corgi` | A | B | C |
| --- | --- | --- | --- | --- |
| Written | 41 notes, 7 a week | 51, 7 a week | 17 | 7 |
| Size | median 2.9 KB, p90 4.3, largest 6.0 | 4.4 / 5.7 / 6.4 | 3.6 / 4.7 / 5.7 | 3.4 / 4.3 / 4.3 |
| Actively read | 12 raw reads of `handover.md`; 35 search hits from archived notes, the top hit 12 times | 13; 105, top hit 18 times | 1; 8, top hit twice | - |
| Passively seen | whole in 25 of 27 digests | 27 of 28 | 3 of 3 | - |
| Effect: open questions handed on | 16: 14 restated in full in the first reply, 2 named later, 0 dropped | 45: 36, 8, 1 | 6: 6, 0, 0 | - |
| Effect: promises | 27 of 28 notes have some; 13 one-off promises checked by hand, 13 kept | 29 of 29 have some; not sampled | 6 of 6; not sampled | - |
| Verdict | keep | | | |

Sections that say "none": unapproved plans in 10 of 28 `corgi` notes,
`[supervisor]` prompts in 15, promises in 1, open threads in 0.

What the sample also showed: the *Promises* section carries standing rules
from note to note. "After each merge into Corgi's main: reinstall from
source" is in 9 notes in a row (2026-11-10 to 2026-11-20) although it is a
decision (`decisions.md:706`), and the handover instructions say to write
only what the other files do not hold. A check never made (the GitHub `gui`
and `msrv` CI jobs) was listed as unverified in 13 notes (2026-10-28 to
2026-11-06) and is absent from 2026-11-09 on, with no record that it was
done or given up; the GUI it concerned was removed two days later.

### notes/

| | `corgi` | A | B, C |
| --- | --- | --- | --- |
| Written | 167 files, 132 of them PNG | 74 files, 24 text | none |
| Size | 26.4 MB; text 166 KB in 29 files | 15.5 MB; 2 files over 1 MB | |
| Actively read | 28 reads in 8 sessions; 9 search hits shown | 16 in 8 sessions; 7 | |
| Passively seen | never | | |
| Verdict | keep for text; it is mostly a screenshot folder | | |

### questions/

| | `corgi` | A | B |
| --- | --- | --- | --- |
| Written (window) | 78 questions; 0 files now | 47; 4 files | 8; 2 files |
| Size | largest 2.4 KB | | |
| Actively read by a supervisor | 7 raw reads | 34 in 21 sessions | 0 |
| Read by the app | every open question, for the Questions panel; 6 answers came through it (since 2026-11-16) | 3 answers | 0 |
| Verdict | keep; it exists for the panel, and files are deleted as told | | |

### backlog.md

| | `corgi` | A |
| --- | --- | --- |
| Written | 6 entries, 2.3 KB, last written 2026-11-11 | 4 entries, 2.1 KB |
| Actively read | 8 reads in 5 sessions | 1 |
| Passively seen | never (the digest does not show it) | |
| Effect | candidates raised after 2026-11-11 went into handover notes instead: four notes list "backlog candidates (not in backlog.md)" | not sampled |
| Verdict | surface differently | |

### Agent memory folders (`~/.claude/projects/<slug>/memory/`)

One folder per repository, shared by its worktrees, so a project's workers
and supervisor load the same index.

| | `corgi` | A |
| --- | --- | --- |
| Written | 13 entries; 22 tool writes in the window, 9 by supervisors, 13 by other sessions | 15 entries; none written in the window |
| Size | 20 KB; index 2.0 KB, 13 lines | 20 KB; index 2.0 KB, 15 lines |
| Actively read | 45 tool reads of 8 distinct entries (29 by supervisors) | none in five weeks |
| Passively seen | the index in every session of the repository (146 with transcripts) | every session |
| Effect (all 13 `corgi` entries read by hand) | 4 unique and current; 4 repeat AGENTS.md or ROLE.md; 4 about parts since removed; 1 personal desktop setup whose index line is out of date | all 15 untouched for 30+ days; content not judged |
| Verdict | prune | review |

Three more memory folders exist for other directories (2 to 4 entries each,
untouched for 80+ days, no tool call in the window). B and C have none.

### AGENTS.md and ARCHITECTURE.md (where kept lessons go)

| | `corgi` repository |
| --- | --- |
| Written | 89 of 140 merges changed AGENTS.md (median 12 lines added, 2,124 in all), 87 changed ARCHITECTURE.md |
| Size | AGENTS.md 6.1 KB on 2026-10-13, 25 KB on 10-28, 54 KB on 11-11, 75.6 KB (1,232 lines) now; ARCHITECTURE.md 31.9 KB after a rewrite from 163 KB |
| Actively read by workers (117 transcripts) | AGENTS.md in 115; ARCHITECTURE.md in 65, never whole |
| Share of AGENTS.md's lines a worker read | median 100% until mid-November (14 of 96 under half); median 50% since (11 of 21 under half, 5 under a quarter) |
| Read by supervisors (29) | AGENTS.md opened in 20, whole in 5; ARCHITECTURE.md in 5 |
| Passively seen | by no Claude Code session: CLAUDE.md is a link to AGENTS.md, not an import |
| Effect | two lessons with a signature stopped their pitfall (see *Summary*) |
| Verdict | keep the pipeline; the file needs a bound and an index |

The supervisor told the user "Lessons: to AGENTS.md" 63 times, "to
ARCHITECTURE.md" 9 times (4 of them both) and "none worth keeping" 28
times: about seven in ten of its verdicts add text. The kill-by-pattern
rule is also in agent memory, so that pitfall had two warnings; `timeout`
had only AGENTS.md.

## Limits and bounds

"Bites" means the bound cut, folded, hid or moved something.

| # | Limit | Value | Where set | Observed | Verdict | Proposed |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | ROLE.md size | 25,887 B | test at `src/supervisor.rs:2669`; AGENTS.md:88 | 25,887 B: no slack at all. About 7.7k tokens in each of 66 sessions | right in kind, binding | keep the number; make room by moving a part out (below) |
| 2 | Recent decisions in full | 8 KB, at least 3 | `src/digest.rs:14-16` | 9 to 12 entries, six to nine days' worth; 35 to 49% of the digest; 9 of 108 look-ups were of an entry shown in full | too loose | 4 KB, at least 3 |
| 3 | Older decision titles | 3 KB | `src/digest.rs:18` | 33 to 39 titles; reaches back 13 days in `corgi`, 26 in A; 126 and 91 titles left out, 4 in B; 75 of 108 look-ups were of a titled entry | too tight | 7 KB (about 80 titles) |
| 4 | Recently finished | 10 ids | `src/digest.rs:20` | spans 5.8 days in `corgi`, 3.2 in A, 14 in B; 43 look-ups of finished work were of these 10, 7 of older work | right | keep |
| 5 | Open work | 4 KB | `src/digest.rs:23` | bit only in A: 12 of 28 digests, 48 ids named, none looked up in that session | right | keep |
| 6 | Ledger text in the digest | 200 characters | `src/digest.rs:26` | 10 entries cut in `corgi`, 120 in A; read whole afterwards 1 and 11 times | right | keep |
| 7 | Stale `reported` counted on one line | 2 days, ids up to 2 KB | `src/digest.rs:29`, `:31` | A: the line in 13 digests, about 50 ids each time, 2 ids looked up; never in `corgi` or B | hiding is right; the id list is too loose | the count and the three oldest ids |
| 8 | Search output | 7 KB, 6 hits, 10 lines and 1,200 B a hit, 2 hits a file | `src/digest/search.rs:31-44` | 94 runs; result median 4.6 to 5.6 KB; hits cut off in 58; another search within two calls after 14; `--show` used 4 times in all | right | keep |
| 9 | Search hit count in the header | every block with any one word | `src/digest/search.rs:666` | median 603 "hits" in `corgi`, 465 in A, 6 shown | misleading | count blocks with all the words |
| 10 | Largest note searched | 1 MB | `src/digest/memory.rs:124` | 2 files over it in A; `corgi`'s 132 PNGs are read and dropped on every search (0.7 to 3 s warm, 22 s cold on a copy) | right | keep; skip non-text by name first |
| 11 | Report typed in with a wake | 4 KB | `src/inbox.rs:50` | 89 of 267 wakes in `corgi` named `report` instead, 81 of 706 in A, 8 of 42 in B; reports at most 4 KB: 68%, 90%, 85%; at most 8 KB: 99% everywhere | too tight | 8 KB |
| 12 | Report text in one prompt | 12 KB | `src/inbox.rs:53` | most ever quoted in one prompt: 4,089 B | never bit | 16 KB if limit 11 goes to 8 KB |
| 13 | `corgi inbox` output | 16 KB of reports, 20 items, 10 delivered | `src/commands/memory.rs:172-179` | 19 runs in 66 sessions, largest 3.9 KB | never bit | keep |
| 14 | Inbox compaction | at 256 KB, keeps 100 delivered | `src/inbox.rs:59-62` | files are 134 to 167 KB | no evidence either way | keep |
| 15 | Ledger lines to the archive | all but each id's newest, at session start | `src/supervisor.rs:147` | 676 of 911 lines archived in `corgi`, 813 of 1,025 in A; the archive is reached through `--ledger` and search, 4 raw reads | right | keep |
| 16 | Handover: context | half the window, or 200k tokens on Claude Code, then 50 idle minutes | `src/supervisor.rs:302-337` | 65 requests, 10 by the context rule; context at those: median 242k and 256k, highest 556k and 464k; 16 of 66 sessions passed 200k; no compaction | right for safety; not a ceiling | keep; consider a ceiling (below) |
| 17 | Handover: idle | 50 minutes with context twice the session's start | `src/supervisor.rs:337`, `:371` | 55 requests; context at those: median 136k, 167k, 132k; 5 under 100k | right | keep |
| 18 | Handover note | under 6 KB (asked, not enforced) | `supervisor/HANDOVER.md:33` | 1 of 63 over (6.35 KB); 11 over 5 KB, 9 of them in A | right | keep |
| 19 | Question file, answer text, backlog | 64 KB, 4 KB, 256 KB | `src/questions.rs:51-53`, `src/backlog.rs:31` | largest question 2.4 KB, backlog 2.3 KB | safety caps, never near | keep |
| 20 | Claude Code's memory index | 200 lines or 25 KB loaded | Claude Code documentation | 13 and 15 lines, 2 KB | far below | none |
| 21 | AGENTS.md | none | - | 75.6 KB, +12 KB a week; Claude Code's Read returns at most 25,000 tokens a call, which the file is at or near | missing | an index and a cap (below) |
| 22 | Digest warnings | shown every time, no limit | `src/digest.rs:584-594` | A: three warnings shown 21, 16 and 12 times; two still shown | missing: nothing makes a session act | ask for the fix in the first prompt |
| 23 | `reports/`, `briefs/`, `handovers/`, `decisions.md`, the archive | none | - | 0.2 to 1.1 MB each; grow by 60 to 150 files a week; only search reads them whole | fine for now | none |

Not measured: the greeting's five lines, three questions at once, the
80-character request id.

## Misses found

In `corgi`, each with a handle:

1. **A decided question asked again.** `decisions.md:359` (2026-10-20, "The
   row's last-activity time reads `HH:MM DD Mon`", after Q25) settled clock
   time with the date. On 2026-11-23 07:11 UTC, session `2ab61c74`, the
   supervisor asked Q128 (clock time or relative time on the agent card)
   within the minute of the user's request and with no memory command before
   it. The user gave the same answer; the new entry is `decisions.md:946`.
   The old title had left the digest (titles reached 2026-11-10). On a copy
   of the state, `--search "last activity time row"` returns the old brief
   first and `decisions.md:359` fourth. Cost: one round trip of a minute.
2. **A rule repeated in nine handover notes although it is a decision.**
   `decisions.md:706` (reinstall after a merge) against the *Promises*
   sections of `handovers/20261110-180822.md` to `handovers/20261120-115421.md`;
   on 2026-11-20 it was saved a third time, as agent memory
   (`feedback-supervisor-runs-post-merge-installs.md`).
3. **Backlog candidates kept in handover notes, not in `backlog.md`.**
   `handovers/20261112-120814.md:15`, `20261113-104228.md:13`,
   `20261116-083323.md:12`, `20261117-081559.md:13`. One dropped note loses
   them.
4. **A promise carried thirteen times, then gone without a word.** The
   GitHub `gui` and `msrv` CI jobs are listed as never tested from
   `handovers/20261028-183146.md` to `handovers/20261106-102415.md` and in
   no later note; nothing records the check or the choice to drop it.
5. **The role says AGENTS.md is loaded; on Claude Code it is not.**
   `supervisor/ROLE.md:116`. 9 of 29 supervisor sessions never opened
   AGENTS.md, and all of them route lessons into it.
6. **A partial reversal with both entries shown in full.**
   `decisions.md:900` (2026-11-17) takes back the follow-focus part of
   `decisions.md:895` (2026-11-16) in prose.
7. **Three ids `done` although merged:** `20261103-w-orphan-sessions`,
   `20261104-marker-cleanup`, `20261111-w-questions-mock`.
8. **Agent memory out of date.** `MEMORY.md:12` says "Catppuccin Latte/Mocha
   everywhere"; its entry says Gruvbox since 2026-11-12.
   `project-gui-env-lacks-gitlab-token.md`, `reference-corgi-app-launcher.md`
   (marked stale by itself), `feedback-herdr-restore-resumes-real-claude.md`
   (cites an AGENTS.md section that is gone) and `user-herdr-over-ssh.md`
   (ANSI mockups of a TUI that is gone) describe removed parts.
   `feedback-never-kill-by-pattern.md` repeats AGENTS.md:165,
   `feedback-live-install-no-answer-overrides.md` AGENTS.md:187,
   `feedback-install-home-is-not-a-sandbox.md` AGENTS.md:184-187,
   `feedback-spawn-always-pass-project.md` `supervisor/ROLE.md:19`.

In the other projects, in aggregate: three digest warnings unfixed through
21, 16 and 12 session starts (two still shown); work left `reported` a
median of 13 days in A, 58 ids for two days or more; one handed-on question
never named by the successor (1 of 51); 15 memory entries that no session
opened in five weeks.

Things supervisors had to find again because no artifact held them: no
case identified, and none was looked for beyond the screens above. The 35
`corgi` searches were read with their context: 16 are followed within three
calls by a brief or a spawn, most of the rest by a question, a decision or
an answer to the user. Whether each search found what was wanted was not
judged.

## Recommendations

**From the numbers:**

1. **Decisions in the digest: 4 KB in full, 7 KB of titles** (limits 2, 3).
   Same total; titles reach about 2.3 times further back. Look-ups follow
   titles, and full text is shown to two later sessions that rarely use it.
2. **Report with a wake: 8 KB; per prompt 16 KB** (limits 11, 12). Saves a
   tool round trip on a third of `corgi`'s wakes. Check first that typing
   8 KB into the box is safe; the 4 KB may have had that reason.
3. **Stale `reported` line: a count and the three oldest ids** (limit 7).
   Saves up to 2 KB a digest where it applies; the full list was acted on
   twice in 644 showings.
4. **Make digest warnings cost something** (limit 22): put the warning in
   the session's first prompt, as the handover path is, or have `corgi
   status` show it to the user. Shown in the digest's footer it was ignored
   for 21 sessions.
5. **Give AGENTS.md a bound and an index** (limit 21): a short top part
   every worker reads (the handoff and the rules that apply to all work),
   then one section or file per area, with a test capping the top part as
   ROLE.md is capped. Workers already read the headings and then pick
   sections. Either fix `supervisor/ROLE.md:116` or make CLAUDE.md import
   the top part only; importing all 75 KB would add about 25k tokens to
   every session.
6. **Search header: count blocks holding all the words** (limit 9), and at
   equal rank put a decision, ledger line, brief or report before an
   archived handover note, which is the top hit in 32 of 94 searches and is
   "a past session's view".
7. **Stop asking for the model-choice note in outcomes**, or build what
   reads it. It also gives ROLE.md about 150 bytes back.
8. **Make room in ROLE.md by moving *Bootstrapping a new project* (999 B)
   behind `corgi role bootstrap`**, as handing over was moved: `role
   handover` ran 59 times for 65 requests, so on-demand parts are read;
   bootstrapping happened at most once in 66 sessions.
9. **Keep as they are:** recently finished (10), open work (4 KB), ledger
   text (200 characters), the two-day stale rule, the search sizes, the
   1 MB note cap, the inbox budgets, the 6 KB note, the handover thresholds.
10. **Consider a ceiling for handover** (limit 16): past about 350k tokens,
    ask at the next rest whatever the idle time. Six requests came above
    300k. Each turn then re-reads more cached tokens than a fresh session
    writes; whether that pays depends on prices this study did not check.

**From the samples:**

11. **Before asking a question, search** (one case, miss 1). The role says
    to search before saying something is unknown; add "or before asking the
    user to decide something". Costs about 60 bytes of the room from 7
    and 8.
12. **Promises that repeat are rules** (misses 2, 4): in `corgi role
    handover`, say that a promise copied from the previous note is logged
    as a decision or dropped, and that a check never made is either put to
    the user or closed. Not in ROLE.md, so no cap cost.
13. **Show the backlog in the digest**: its headings, about 300 B today
    (miss 3). Then candidates have one place and survive a handover.
14. **Prune `corgi`'s agent memory** (miss 8): delete the four stale
    entries, fix the index line, drop the four that repeat the repository,
    and move the desktop setup out of the project's memory, which every
    worker loads.
15. **Partial reversals** (miss 6): no change proposed. Four in 118 entries,
    each explained in prose.

**Per artifact:** decisions, ledger, briefs, reports, handover notes,
questions: keep. Notes: keep. Backlog: surface (13). Agent memory: prune
(14). `Learned` lines: keep, with 5.

## Method

All counts come from scripts in this directory, run in this order. Their
output names projects, so it was kept in a scratch directory.

| Script | What it does |
| --- | --- |
| `index_sessions.py` | One line per Claude Code transcript: where, when, first prompt, whether it is a supervisor session, context at the first request and at its peak. |
| `extract_calls.py` | Every memory-related tool call of the supervisor sessions: each `corgi` command form, raw reads and writes of state files by artifact, file-tool calls, what came back. Also the conversation without tool traffic, and every touch of a memory folder by any session. |
| `measure_written.py` | Each state directory: entries, bytes, size distribution, per week; the digest's arithmetic re-run on today's files. |
| `analyze_reads.py` | Calls per form, session and week; the digest's size and sections; searches; each decision from the call that wrote it to every showing and look-up. |
| `analyze_wakes.py` | `[Corgi]` prompts by kind and size; `report` runs; context at each handover request. |
| `analyze_bounds.py` | Whether what a digest bound cut was looked up later; how long warnings stayed; how much a handover note repeats the one before. |
| `analyze_briefs.py` | Briefs naming a decision, and how old it was. |
| `analyze_handovers.py` | Questions and promises handed on, against the successor's session. |
| `analyze_ledger.py` | Ledger statuses against Git (this repository only) and how long work stays `reported`. |
| `analyze_lessons.py` | `Learned` lines to merges that changed AGENTS.md or ARCHITECTURE.md. |
| `analyze_docs.py` | Reads of AGENTS.md and ARCHITECTURE.md per transcript, with line coverage. |
| `analyze_pitfalls.py` | Two lessons' pitfalls before and after they reached AGENTS.md. |
| `measure_memory.py` | Agent memory folders and tool calls on them. |

Definitions:

- A **supervisor session** is a transcript whose first prompt starts "Start
  your supervisor session for". All 66 are Claude Code; the Codex store has
  no rollouts and OpenCode's database no supervisor session.
- **Actively read** is a command or file read the supervisor chose.
  **Passively seen** is what the session-start digest or a wake put in
  front of it.
- A decision is **looked up** when a `--decision` or `--show` result prints
  it or a `--search` result shows it as a hit, after the call that wrote it.
  Raw reads are counted apart (12 in all, mostly `tail` before an append).
- A decision is **written in the window** when a call in a transcript
  appended its heading: 196 of the 205 dated from each project's first
  transcript day on.
- Bytes per token were measured from the growth of the context across a
  tool result.
- The digest and search were never run against a real state directory. One
  search timing used a copy in the scratch directory, since removed.

Samples checked by hand, all in `corgi` unless said: every `--decision` call
(20) and every search (35) with the prompt before and the reply after; the
22 questions closest to an earlier decision, of 78; 7 pushback prompts in
all projects; 17 reversal candidates of 118 decisions; 13 one-off promises
from 28 notes; all 13 memory entries. Questions handed on were checked by
script for all 63 handovers.

Checks on the numbers: the call extractor's classes were sampled per class
and two errors fixed (lines that only assign a path to a variable; records
that repeat a tool call without a time, which had doubled 17% of calls); no
tool call id appears in two sessions; the digest arithmetic in
`measure_written.py` gives 35 titles and 126 left out for `corgi`, which the
newest real digests also show (35 to 43 shown, up to 124 left out); the
ledger check's 41 unconfirmed merges were listed and all predate the
repository's first commit.

## What the data cannot show

- **Whether passive reading changes anything.** A decision shown in full
  and then followed leaves no trace. The case for shrinking the full text
  rests on look-ups and brief citations, not on proof that the text went
  unread.
- **Anything before 2026-10-20.** Claude Code had removed older
  transcripts. C has no transcripts at all. Decisions older than the window
  may have been looked up before it.
- **Whether a search result was good enough.** Only that the supervisor
  went on to act, rarely searched again, and almost never opened a hit
  whole.
- **Quiet re-asks and contradictions.** The screens catch a question whose
  title shares words with an earlier decision, and a user who objects in
  words. A brief that silently goes against an old decision would pass
  both. One case in 78 questions is a floor.
- **Other projects' effect samples.** For A and B only the scripted checks
  ran; their ledgers were not compared with Git, since that would have
  meant commands in their checkouts.
- **Prices.** Token counts are measured; what a cold or warm re-read costs
  was not looked up.
- **Limits changed in the window.** The digest bounds date from 2026-10-27,
  kept reports and `--show`, `--ledger`, `--brief` too, the 200k rule from
  2026-11-04. Digest figures use only digests from 2026-10-28 on.
- This session is one of the 21 late-November worker transcripts in the
  AGENTS.md reading figures (it read 3% of the file).
