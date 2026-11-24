#!/usr/bin/env python3
"""When a digest bound cut something, was the cut thing needed afterwards?

From the plain digests the supervisors actually read (calls.jsonl from
extract_calls.py), per project:

- open work "left out for size" (OPEN_WORK_BYTES) and the one-line count of
  stale `reported` work (STALE_REPORTED_SECS): how often each line appeared,
  how many ids it named, and how many of those ids the same session then
  looked up (`--ledger`, `--brief`, `--search`, `report`, a raw read);
- ledger text cut at LEDGER_TEXT_CHARS: entries shown cut, and how many of
  those the session then read whole with `--ledger`;
- warnings (an unreadable ledger line, a `Supersedes:` line matching no
  heading): in how many digests in a row the same warning was shown before
  it went away, or whether it is still there;
- how much of each handover note repeats the note before it, line for line.

    python3 research/analyze_bounds.py /scratch/calls.jsonl

Prints JSON keyed by the write-up's project names. Reads only.
"""

import difflib
import glob
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402
from analyze_reads import digest_text, dist, section  # noqa: E402


def ids_in(line):
    return re.findall(r"\b\d{8}-[\w.-]+", line.split(":", 1)[1] if ":" in line else line)


def main():
    calls = [json.loads(line) for line in open(sys.argv[1])]
    report = {}
    for directory, project in written.names().items():
        own = sorted((c for c in calls if c["project"] == directory), key=lambda c: c["ts"])
        digests = []
        for call in own:
            if call["form"] == "digest":
                text = digest_text(call.get("result") or "")
                if text and call["ts"] >= "2026-10-28":
                    digests.append((call, text))
        if not digests:
            continue

        def looked_up(session, after, names):
            found = set()
            for call in own:
                if call["session"] != session or call["ts"] <= after:
                    continue
                if call["form"] in ("digest--ledger", "digest--brief", "digest--search", "digest--show", "report", "raw-read", "tool-read"):
                    text = call.get("arg", "") + " " + call.get("command", "")
                    for name in names:
                        agent = re.sub(r"^\d{8}-", "", name)
                        if name in text or re.search(r"(?<![\w-])" + re.escape(agent) + r"(?![\w-])", text):
                            found.add(name)
            return found

        left_lines, left_ids, left_used = 0, 0, 0
        stale_lines, stale_ids, stale_used = 0, 0, 0
        clipped, clipped_then_read = 0, 0
        warnings = {}
        for call, text in digests:
            work = section(text, "# Open work", ["\n# Recently finished"])
            for line in work.splitlines():
                if "older left out for size" in line:
                    names = ids_in(line)
                    left_lines += 1
                    left_ids += len(names)
                    left_used += len(looked_up(call["session"], call["ts"], names))
                elif "more `reported`, agent not running" in line:
                    names = ids_in(line)
                    stale_lines += 1
                    stale_ids += len(names)
                    stale_used += len(looked_up(call["session"], call["ts"], names))
            for block in re.split(r"\n(?=- \d{8}-)", work):
                if "…" in block:
                    head = re.match(r"- (\S+)", block.strip())
                    if head:
                        clipped += 1
                        if any(
                            c["session"] == call["session"] and c["ts"] > call["ts"] and c["form"] == "digest--ledger"
                            and (head.group(1) in c["arg"] or c["arg"] and c["arg"] in block.split("\n")[0])
                            for c in own
                        ):
                            clipped_then_read += 1
            for line in text.splitlines():
                if line.startswith("warning:"):
                    key = re.sub(r"\s+", " ", line)[:160]
                    warnings.setdefault(key, []).append(call["ts"])
        last = digests[-1][0]["ts"]
        report[project] = {
            "digests read since the bounds (2026-10-28)": len(digests),
            "'left out for size' lines": left_lines,
            "ids those lines named": left_ids,
            "... looked up later in the same session": left_used,
            "stale `reported` lines": stale_lines,
            "ids those lines named (an id is counted once per digest)": stale_ids,
            "... looked up later in the same session ": stale_used,
            "open-work entries shown with cut text": clipped,
            "... then read whole with --ledger in that session": clipped_then_read,
            "distinct warnings": len(warnings),
            "digests each warning was shown in": sorted((len(v) for v in warnings.values()), reverse=True),
            "warnings still shown in the newest digest": sum(1 for v in warnings.values() if v[-1] == last),
        }

        # Handover notes: how much of each repeats the one before.
        notes = sorted(glob.glob(os.path.join(written.ROOT, directory, "handovers", "*.md")))
        notes = [n for n in notes if os.path.basename(n) >= "20261020"]
        shares = []
        previous = None
        for path in notes:
            lines = [l.strip() for l in written.read(path).splitlines() if len(l.strip()) >= 40]
            if previous and lines:
                repeated = 0
                for line in lines:
                    if any(difflib.SequenceMatcher(None, line, old).ratio() >= 0.8 for old in previous):
                        repeated += len(line)
                shares.append(round(100 * repeated / max(sum(len(l) for l in lines), 1)))
            previous = lines
        report[project]["handover notes compared with the one before"] = len(shares)
        report[project]["percent of a note's text repeated from the previous note"] = dist(shares)
        report[project]["notes more than half repeated"] = sum(1 for s in shares if s > 50)
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
