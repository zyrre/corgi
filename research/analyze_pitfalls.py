#!/usr/bin/env python3
"""Did workers still walk into two pitfalls after AGENTS.md warned of them?

A lesson a worker reports is committed to AGENTS.md so that later workers
avoid it. Two lessons have a signature a transcript shows: running a command
under `timeout` (macOS has none, so the command silently never runs), and
killing processes by name or pattern (`pkill`, `killall`, `kill $(pgrep ...)`).
For the worker transcripts of the named repository's worktrees, this counts
the Bash calls with each signature before and after the day the lesson
reached AGENTS.md, and the transcripts they were in.

    python3 research/analyze_pitfalls.py /scratch/sessions.jsonl corgi timeout=2026-11-02 kill=2026-11-03

Reads only.
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from extract_calls import segments, strip_heredocs, first_word  # noqa: E402

SIGNS = {
    "timeout": lambda segment: first_word(segment) == "timeout",
    "kill": lambda segment: first_word(segment) in ("pkill", "killall") or bool(re.search(r"\bkill\b[^|;&]*\$\((?:pgrep|ps )|pgrep[^|;&]*\|\s*xargs\s+kill", segment)),
}


def main():
    sessions = [json.loads(line) for line in open(sys.argv[1])]
    repo = sys.argv[2]
    since = dict(arg.split("=") for arg in sys.argv[3:])
    worktree = re.compile(r"-herdr-worktrees-" + re.escape(repo) + r"-worktree-")
    counts = {name: {"before": [0, set(), 0], "after": [0, set(), 0]} for name in since}
    for session in sessions:
        if not worktree.search(session["slug"]):
            continue
        for name in since:
            when = "before" if (session["first_ts"] or "")[:10] < since[name] else "after"
            counts[name][when][2] += 1
        with open(session["file"], errors="replace") as handle:
            for line in handle:
                if '"Bash"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if record.get("type") != "assistant" or record.get("isSidechain"):
                    continue
                for part in (record.get("message") or {}).get("content") or []:
                    if not isinstance(part, dict) or part.get("type") != "tool_use" or part.get("name") != "Bash":
                        continue
                    command = strip_heredocs((part.get("input") or {}).get("command", ""))
                    for name, test in SIGNS.items():
                        if name in since and any(test(segment) for segment in segments(command)):
                            when = "before" if (record.get("timestamp") or "")[:10] < since[name] else "after"
                            counts[name][when][0] += 1
                            counts[name][when][1].add(session["file"])
    for name, periods in counts.items():
        for when, (calls, files, total) in periods.items():
            print(f"{name}: {when} {since[name]}: {calls} Bash calls in {len(files)} of {total} worker transcripts")


if __name__ == "__main__":
    main()
