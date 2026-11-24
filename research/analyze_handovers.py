#!/usr/bin/env python3
"""Did the session that took over pick up what the handover note handed on?

For every supervisor session started by a handover (its first prompt names
the archived note), reads that note and the session's conversation
(<calls>-talk.jsonl from extract_calls.py) and reports:

- the note's size against the 6 KB the handover instructions ask for, and
  which of its sections are "none";
- each open question the note hands on (`### Q<n>:`): was it restated in the
  successor's first reply, as the role requires, and was it named anywhere
  in the successor's session;
- each item under "Promises to the user" and "Proposed plans not yet
  approved": listed with the successor's session id for a hand check (pass
  `--list NAME`), since whether a promise was kept cannot be scripted.

    python3 research/analyze_handovers.py /scratch/calls.jsonl [--list corgi]

Prints JSON keyed by the write-up's project names. Reads only.
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402
from analyze_reads import dist  # noqa: E402

SECTIONS = ("open threads", "proposed plans", "prompts since", "promises")


def parts(note):
    """The note's sections by the heading words the handover instructions use."""
    found = {}
    marks = []
    for match in re.finditer(r"^(?:#+\s*|\*\*|- \*\*)([^\n]*)$", note, re.M):
        lowered = match.group(1).lower()
        for name in SECTIONS:
            if name in lowered and name not in [m[1] for m in marks]:
                marks.append((match.start(), name, match.end()))
    marks.sort()
    for index, (start, name, body) in enumerate(marks):
        end = marks[index + 1][0] if index + 1 < len(marks) else len(note)
        found[name] = note[body:end].strip()
    return found


def is_none(text):
    return bool(re.fullmatch(r"[\s\-*_:.]*(none|nothing|n/a)[\s.\-*_]*.{0,80}", text.strip(), re.I | re.S)) and len(text) < 120


def main():
    talk_path = sys.argv[1].replace(".jsonl", "") + "-talk.jsonl"
    talk = [json.loads(line) for line in open(talk_path)]
    listed = sys.argv[3] if len(sys.argv) > 3 and sys.argv[2] == "--list" else None
    report = {}
    for directory, project in written.names().items():
        mine = sorted((t for t in talk if t["project"] == directory), key=lambda t: t["ts"] or "")
        sessions = {}
        for turn in mine:
            sessions.setdefault(turn["session"], []).append(turn)
        notes, sizes, empties = 0, [], {name: 0 for name in SECTIONS}
        missing_sections = {name: 0 for name in SECTIONS}
        handed, restated, named_later, never_named = 0, 0, 0, 0
        promise_notes, plan_notes, rows = 0, 0, []
        for session, turns in sessions.items():
            first = turns[0]["text"]
            found = re.search(r"move it to (\S+/handovers/(\d{8}-\d{6})\.md)", first)
            if not found:
                continue
            note = written.read(os.path.join(written.ROOT, directory, "handovers", found.group(2) + ".md"))
            if not note:
                continue
            notes += 1
            sizes.append(len(note.encode()))
            sections = parts(note)
            for name in SECTIONS:
                if name not in sections:
                    missing_sections[name] += 1
                elif is_none(sections[name]):
                    empties[name] += 1
            # The successor's first reply: every assistant text before the user's first typed prompt.
            opening, rest = [], []
            typed = False
            for turn in turns[1:]:
                text = re.sub(r"</?pasted_content[^>]*>", "", turn["text"]).strip()
                if turn["role"] == "user" and not turn["meta"] and not text.startswith(("[Corgi]", "<")):
                    typed = True
                if turn["role"] == "assistant":
                    (rest if typed else opening).append(turn["text"])
            opening_text, rest_text = "\n".join(opening), "\n".join(rest)
            for number in sorted(set(re.findall(r"^#+ (Q\d+):", sections.get("open threads", note), re.M))):
                handed += 1
                if re.search(r"#+ " + number + r":", opening_text):
                    restated += 1
                elif re.search(r"\b" + number + r"\b", opening_text + rest_text):
                    named_later += 1
                else:
                    never_named += 1
            promises = sections.get("promises", "")
            plans = sections.get("proposed plans", "")
            if promises and not is_none(promises):
                promise_notes += 1
                rows.append((session, found.group(2), "promises", promises[:1500]))
            if plans and not is_none(plans):
                plan_notes += 1
                rows.append((session, found.group(2), "plans", plans[:1500]))
        report[project] = {
            "handovers with a note and a successor transcript": notes,
            "note bytes": dist(sizes),
            "notes over 6 KB": sum(1 for s in sizes if s > 6 * 1024),
            "notes over 5 KB": sum(1 for s in sizes if s > 5 * 1024),
            "sections that say none": empties,
            "sections absent": missing_sections,
            "open questions handed on": handed,
            "restated in full in the successor's first reply": restated,
            "not restated, but named later in the session": named_later,
            "never named by the successor": never_named,
            "notes with promises": promise_notes,
            "notes with unapproved plans": plan_notes,
        }
        if listed == project:
            report[project]["rows"] = rows
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
