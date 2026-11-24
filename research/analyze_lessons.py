#!/usr/bin/env python3
"""Follow the `Learned` lines of workers' reports: reported, judged by the
supervisor, committed to AGENTS.md or ARCHITECTURE.md, merged.

For the project whose repository this is run in:

- kept reports (`reports/`) per ledger id: does its newest report with a
  `### Report` block have a `Learned` line, and is it "none";
- what the supervisor told the user about lessons (`Lessons: ...` lines in
  its replies, from <calls>-talk.jsonl);
- for each ledger id whose branch a merge commit on the base branch names:
  did that merge change AGENTS.md or ARCHITECTURE.md, and by how many lines;
- how AGENTS.md grew, week by week.

    python3 research/analyze_lessons.py corgi . main /scratch/calls.jsonl

Reads only (Git is asked with log, show and diff).
"""

import collections
import glob
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402

LEARNED = re.compile(r"^\s*[-*]?\s*\**Learned\**\s*:\**\s*(.*)$", re.M | re.I)


def git(repo, *args):
    done = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    return done.stdout if done.returncode == 0 else ""


def main():
    directory, repo, base, calls_path = sys.argv[1:5]
    state = os.path.join(written.ROOT, directory)
    out = {}

    # Reports: the newest one with a Report block, per ledger id.
    newest = {}
    for path in sorted(glob.glob(os.path.join(state, "reports", "*.md"))):
        name = os.path.basename(path)
        subject = re.sub(r"^\d{8}-\d{6}-", "", name)[:-3]
        text = written.read(path)
        if "### Report" in text or LEARNED.search(text):
            newest[subject] = (name, text)
    with_line, none, lesson_chars = 0, 0, []
    lessons = {}
    for subject, (name, text) in newest.items():
        found = LEARNED.findall(text)
        if not found:
            continue
        with_line += 1
        start = LEARNED.search(text).start()
        body = text[start:start + 3000]
        # The line and its continuation lines, up to the next blank line or bullet at the same level.
        block = re.split(r"\n\s*\n|\n[-*] \**(?:Result|Changes|Verified|Open questions|Risks)", body)[0]
        first = found[-1].strip().strip("*").strip()
        if re.match(r"[\"'`]?none\b", first, re.I) and len(block) < 160:
            none += 1
        else:
            lessons[subject] = block
            lesson_chars.append(len(block))
    out["ledger ids with a kept report that has a Report block"] = len(newest)
    out["... whose newest report has a Learned line"] = with_line
    out["... saying none"] = none
    out["... naming at least one lesson"] = len(lessons)
    ordered = sorted(lesson_chars)
    out["characters of a lesson block: median, p90"] = [ordered[len(ordered) // 2], ordered[int(len(ordered) * 0.9)]] if ordered else None

    # What the supervisor said about lessons.
    talk = [json.loads(line) for line in open(calls_path.replace(".jsonl", "") + "-talk.jsonl")]
    said = collections.Counter()
    for turn in talk:
        if turn["project"] != directory or turn["role"] != "assistant":
            continue
        for line in re.findall(r"\**Lessons?\**:\**\s*([^\n]{0,200})", turn["text"]):
            lowered = line.lower()
            if "none" in lowered or "nothing" in lowered or "no lesson" in lowered:
                said["none worth keeping"] += 1
            elif "agents.md" in lowered and "architecture.md" in lowered:
                said["to AGENTS.md and ARCHITECTURE.md"] += 1
            elif "agents.md" in lowered:
                said["to AGENTS.md"] += 1
            elif "architecture.md" in lowered:
                said["to ARCHITECTURE.md"] += 1
            else:
                said["other wording"] += 1
    out["supervisor's Lessons lines to the user"] = dict(said.most_common())

    # Merges: did the branch bring a change to the two files?
    lines = written.ledger_lines(written.read(os.path.join(state, "ledger-archive.jsonl"))) + written.ledger_lines(written.read(os.path.join(state, "ledger.jsonl")))
    branch_of = {}
    for line in lines:
        if not line.get("_bad") and line.get("branch"):
            branch_of[line["id"]] = line["branch"]
    merges = {}
    for row in git(repo, "log", base, "--merges", "--format=%H\t%s").splitlines():
        commit, subject = row.split("\t", 1)
        found = re.search(r"Merge (?:remote-tracking )?branch '([^']+)'", subject)
        if found:
            merges.setdefault(found.group(1), commit)
    merged_ids, touched = 0, collections.Counter()
    added = {"AGENTS.md": [], "ARCHITECTURE.md": []}
    lesson_merged, lesson_landed, none_merged, none_landed = 0, 0, 0, 0
    for ledger_id, branch in branch_of.items():
        commit = merges.get(branch)
        if not commit:
            continue
        merged_ids += 1
        stat = git(repo, "diff", "--numstat", f"{commit}^1", commit, "--", "AGENTS.md", "ARCHITECTURE.md")
        files = {}
        for row in stat.splitlines():
            plus, minus, name = row.split("\t")
            files[name] = int(plus) if plus.isdigit() else 0
            added[name].append(files[name])
        for name in files:
            touched[name] += 1
        if files:
            touched["either"] += 1
        if ledger_id in lessons:
            lesson_merged += 1
            lesson_landed += 1 if files else 0
        elif ledger_id in newest:
            none_merged += 1
            none_landed += 1 if files else 0
    out["ledger ids merged by a merge commit naming their branch"] = merged_ids
    out["... whose merge changed AGENTS.md"] = touched["AGENTS.md"]
    out["... whose merge changed ARCHITECTURE.md"] = touched["ARCHITECTURE.md"]
    out["... whose merge changed either"] = touched["either"]
    out["merged ids whose report named a lesson"] = lesson_merged
    out["... of those, merge changed AGENTS.md or ARCHITECTURE.md"] = lesson_landed
    out["merged ids whose report said none or had no Learned line"] = none_merged
    out["... of those, merge changed one of the files anyway"] = none_landed
    for name, values in added.items():
        ordered = sorted(values)
        if ordered:
            out[f"lines a merge added to {name}: median, p90, max, total"] = [ordered[len(ordered) // 2], ordered[int(len(ordered) * 0.9)], ordered[-1], sum(ordered)]

    # Growth of AGENTS.md by ISO week (bytes at the last commit of each week).
    growth = {}
    for row in git(repo, "log", base, "--format=%H %cs", "--", "AGENTS.md").splitlines():
        commit, day = row.split()
        week = written.week(day)
        if week not in growth:
            growth[week] = len(git(repo, "show", f"{commit}:AGENTS.md").encode())
    out["AGENTS.md bytes at the end of each week"] = dict(sorted(growth.items()))
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
