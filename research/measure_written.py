#!/usr/bin/env python3
"""Measure what each supervisor state directory holds, and how the digest's
bounds cut it today.

Per project (named `corgi`, else A, B, ... by size, so the output can be
shared): per artifact kind the entries, bytes, lines, size distribution and
entries per ISO week; then a re-run of the digest's own arithmetic on the
files as they are now (src/digest.rs: decision_view, open_work, the recently
finished list), without a fleet, to say what each bound shows and hides.

    python3 research/measure_written.py [--json /scratch/written.json]

Reads only.
"""

import datetime
import glob
import json
import os
import re
import statistics
import sys

ROOT = os.path.expanduser("~/.local/state/corgi/supervisors")

# The digest's bounds, src/digest.rs.
RECENT_DECISION_BYTES = 8 * 1024
MIN_RECENT_DECISIONS = 3
DECISION_TITLE_BYTES = 3 * 1024
RECENT_FINISHED = 10
OPEN_WORK_BYTES = 4 * 1024
LEDGER_TEXT_CHARS = 200
STALE_REPORTED_SECS = 2 * 24 * 3600
# src/inbox.rs
INLINE_REPORT_MAX = 4 * 1024
PROMPT_REPORT_BUDGET = 12 * 1024


def names():
    """State directory -> the name the write-up uses."""
    dirs = [d for d in sorted(os.listdir(ROOT)) if os.path.isdir(os.path.join(ROOT, d))]

    def size(d):
        return sum(os.path.getsize(p) for p in glob.glob(os.path.join(ROOT, d, "**"), recursive=True) if os.path.isfile(p))

    others = sorted((d for d in dirs if d != "corgi"), key=size, reverse=True)
    mapping = {"corgi": "corgi"} if "corgi" in dirs else {}
    mapping.update({d: f"project {chr(65 + i)}" for i, d in enumerate(others)})
    return mapping


def dist(sizes):
    if not sizes:
        return {"n": 0}
    ordered = sorted(sizes)
    return {
        "n": len(ordered),
        "total": sum(ordered),
        "median": int(statistics.median(ordered)),
        "p90": ordered[min(len(ordered) - 1, int(len(ordered) * 0.9))],
        "max": ordered[-1],
    }


def week(date):
    """ISO week of YYYY-MM-DD or YYYYMMDD."""
    digits = re.sub(r"\D", "", date)[:8]
    try:
        day = datetime.date(int(digits[:4]), int(digits[4:6]), int(digits[6:8]))
    except ValueError:
        return "?"
    year, number, _ = day.isocalendar()
    return f"{year}-W{number:02d}"


def per_week(dates):
    weeks = {}
    for date in dates:
        weeks[week(date)] = weeks.get(week(date), 0) + 1
    return dict(sorted(weeks.items()))


def read(path):
    try:
        return open(path, errors="replace").read()
    except OSError:
        return ""


def parse_decisions(text):
    """As src/digest.rs parse_decisions: (heading, text, supersedes, line)."""
    starts, offset, fenced = [], 0, False
    for index, line in enumerate(text.splitlines(keepends=True)):
        if line.lstrip().startswith("```"):
            fenced = not fenced
        elif not fenced and line.startswith("## "):
            starts.append((offset, index + 1))
        offset += len(line.encode())
    raw = text.encode()
    entries = []
    for index, (start, line) in enumerate(starts):
        end = starts[index + 1][0] if index + 1 < len(starts) else len(raw)
        entry = raw[start:end].decode(errors="replace").rstrip()
        heading = entry.splitlines()[0][3:].strip()
        supersedes = [l[len("Supersedes:"):].strip() for l in entry.splitlines() if l.startswith("Supersedes:") and l[len("Supersedes:"):].strip()]
        entries.append({"heading": heading, "text": entry, "supersedes": supersedes, "line": line})
    return entries


def decision_view(entries):
    hidden = [None] * len(entries)
    warnings = []
    for index, entry in enumerate(entries):
        for target in entry["supersedes"]:
            found = False
            for earlier in range(index):
                if entries[earlier]["heading"] == target:
                    hidden[earlier] = entry["heading"]
                    found = True
            if not found:
                warnings.append((entry["heading"], target))
    visible = [e for e, h in zip(entries, hidden) if h is None][::-1]
    recent, size, position = [], 0, 0
    while position < len(visible):
        entry = visible[position]
        length = len(entry["text"].encode())
        if size + length > RECENT_DECISION_BYTES and len(recent) >= MIN_RECENT_DECISIONS:
            break
        size += length
        recent.append(entry)
        position += 1
    titles, left_out, title_bytes = [], 0, 0
    for entry in visible[position:]:
        title = entry["heading"].replace(": ", "  ", 1)
        if left_out == 0 and title_bytes + len(title.encode()) < DECISION_TITLE_BYTES:
            title_bytes += len(title.encode()) + 1
            titles.append(entry)
        else:
            left_out += 1
    return {
        "recent": recent,
        "recent_bytes": size,
        "titles": titles,
        "title_bytes": title_bytes,
        "left_out": left_out,
        "superseded": sum(1 for h in hidden if h is not None),
        "warnings": warnings,
    }


def ledger_lines(text):
    lines = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except ValueError:
            lines.append({"_bad": True, "_line": number, "_bytes": len(line.encode())})
            continue
        if not isinstance(value, dict) or not value.get("id"):
            lines.append({"_bad": True, "_line": number, "_bytes": len(line.encode())})
            continue
        value["_line"] = number
        value["_bytes"] = len(line.encode())
        lines.append(value)
    return lines


def newest(lines):
    by_id = {}
    for line in lines:
        if not line.get("_bad"):
            by_id[line["id"]] = line
    return sorted(by_id.values(), key=lambda line: line["_line"])


def seconds(stamp):
    try:
        return datetime.datetime.fromisoformat(str(stamp).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def clip(text):
    text = str(text or "")
    return text if len(text) <= LEDGER_TEXT_CHARS else text[:LEDGER_TEXT_CHARS].rstrip() + "…"


def open_entry_bytes(entry):
    out = f"- {entry['id']} [{entry.get('status') or '-'}] agent {entry.get('agent')}: **NOT RUNNING**; branch {entry.get('branch') or '-'}\n"
    if entry.get("summary"):
        out += f"  summary: {clip(entry['summary'])}\n"
    if entry.get("outcome"):
        out += f"  outcome: {clip(entry['outcome'])}\n"
    return len(out.encode())


def measure(directory, now):
    result = {}
    path = lambda *parts: os.path.join(ROOT, directory, *parts)

    # Decisions.
    text = read(path("decisions.md"))
    entries = parse_decisions(text)
    sizes = [len(e["text"].encode()) for e in entries]
    view = decision_view(entries)
    dated = [e["heading"][:10] for e in entries if re.match(r"\d{4}-\d\d-\d\d", e["heading"])]
    result["decisions"] = {
        "bytes": len(text.encode()),
        "lines": text.count("\n"),
        "entries": dist(sizes),
        "per_week": per_week(dated),
        "first": min(dated) if dated else None,
        "last": max(dated) if dated else None,
        "with_supersedes": sum(1 for e in entries if e["supersedes"]),
        "supersedes_lines": sum(len(e["supersedes"]) for e in entries),
        "superseded_hidden": view["superseded"],
        "supersedes_unmatched": len(view["warnings"]),
        "observed_source": sum(1 for e in entries if "\nSource: observed" in e["text"]),
        "with_why": sum(1 for e in entries if "\nWhy:" in e["text"]),
        "with_rejected": sum(1 for e in entries if "\nRejected:" in e["text"]),
        "over_8k": sum(1 for s in sizes if s > RECENT_DECISION_BYTES),
        "over_2k": sum(1 for s in sizes if s > 2048),
        "digest_full": len(view["recent"]),
        "digest_full_bytes": view["recent_bytes"],
        "digest_full_span_days": (
            (datetime.date.fromisoformat(view["recent"][0]["heading"][:10]) - datetime.date.fromisoformat(view["recent"][-1]["heading"][:10])).days
            if view["recent"] and all(re.match(r"\d{4}-\d\d-\d\d", e["heading"]) for e in (view["recent"][0], view["recent"][-1]))
            else None
        ),
        "digest_titles": len(view["titles"]),
        "digest_title_bytes": view["title_bytes"],
        "digest_titles_oldest": view["titles"][-1]["heading"][:10] if view["titles"] else None,
        "digest_titles_left_out": view["left_out"],
        "title_len": dist([len(e["heading"].encode()) for e in entries]),
    }

    # Ledger and its archive.
    live = ledger_lines(read(path("ledger.jsonl")))
    archive = ledger_lines(read(path("ledger-archive.jsonl")))
    ids = newest(live)
    finished = [e for e in ids if e.get("status") in ("merged", "abandoned", "done")]
    open_ids = [e for e in ids if e.get("status") not in ("merged", "abandoned", "done")]
    stale = [
        e for e in open_ids
        if e.get("status") == "reported" and seconds(e.get("ts")) is not None and now - seconds(e.get("ts")) >= STALE_REPORTED_SECS
    ]
    listed = [e for e in open_ids if e not in stale]
    blocks = [(open_entry_bytes(e), e.get("status") in ("needs-answer", "blocked")) for e in listed]
    shown_bytes, shown, full = 0, 0, False
    for size, pinned in reversed(blocks):
        if not pinned and (full or shown_bytes + size > OPEN_WORK_BYTES):
            full = True
            continue
        shown_bytes += size
        shown += 1
    statuses = {}
    for entry in ids:
        statuses[entry.get("status") or "-"] = statuses.get(entry.get("status") or "-", 0) + 1
    all_lines = [l for l in archive + live if not l.get("_bad")]
    age_days = lambda e: (now - seconds(e.get("ts"))) / 86400 if seconds(e.get("ts")) else None
    result["ledger"] = {
        "bytes": os.path.getsize(path("ledger.jsonl")) if os.path.exists(path("ledger.jsonl")) else 0,
        "lines": len(live),
        "archive_bytes": os.path.getsize(path("ledger-archive.jsonl")) if os.path.exists(path("ledger-archive.jsonl")) else 0,
        "archive_lines": len(archive),
        "bad_lines": sum(1 for l in live if l.get("_bad")),
        "ids": len(ids),
        "ids_ever": len({l["id"] for l in all_lines}),
        "lines_per_id": dist(list({i: sum(1 for l in all_lines if l["id"] == i) for i in {l["id"] for l in all_lines}}.values())),
        "line_bytes": dist([l["_bytes"] for l in all_lines]),
        "summary_chars": dist([len(str(l.get("summary") or "")) for l in all_lines if l.get("summary")]),
        "outcome_chars": dist([len(str(l.get("outcome") or "")) for l in all_lines if l.get("outcome")]),
        "summary_over_200": sum(1 for l in ids if len(str(l.get("summary") or "")) > LEDGER_TEXT_CHARS),
        "outcome_over_200": sum(1 for l in ids if len(str(l.get("outcome") or "")) > LEDGER_TEXT_CHARS),
        "ids_with_outcome": sum(1 for l in ids if l.get("outcome")),
        "statuses_now": dict(sorted(statuses.items(), key=lambda kv: -kv[1])),
        "open": len(open_ids),
        "open_stale_reported": len(stale),
        "open_listed": len(listed),
        "open_listed_bytes": sum(size for size, _ in blocks),
        "open_shown": shown,
        "open_left_out": len(listed) - shown,
        "open_age_days": dist([int(age_days(e)) for e in open_ids if age_days(e) is not None]),
        "open_by_status": {
            status: {
                "n": sum(1 for e in open_ids if (e.get("status") or "-") == status),
                "median_age_days": int(statistics.median([age_days(e) for e in open_ids if (e.get("status") or "-") == status and age_days(e) is not None] or [0])),
            }
            for status in sorted({e.get("status") or "-" for e in open_ids})
        },
        "finished": len(finished),
        "finished_hidden": max(0, len(finished) - RECENT_FINISHED),
        "finished_shown_span_days": (
            round((seconds(finished[-1].get("ts")) - seconds(finished[-RECENT_FINISHED].get("ts"))) / 86400, 1)
            if len(finished) >= RECENT_FINISHED and seconds(finished[-1].get("ts")) and seconds(finished[-RECENT_FINISHED].get("ts"))
            else None
        ),
        "per_week_new_ids": per_week(sorted({l["id"]: str(l.get("ts") or l["id"])[:10] for l in reversed(all_lines)}.values())),
        "per_week_lines": per_week([str(l.get("ts"))[:10] for l in all_lines if l.get("ts")]),
        "with_model": sum(1 for l in all_lines if l.get("model")),
        "outcome_mentions_model": sum(1 for l in all_lines if re.search(r"sonnet|opus|haiku|model|effort", str(l.get("outcome") or ""), re.I)),
        "with_questions": sum(1 for l in all_lines if l.get("questions")),
    }

    # Directories of files.
    def files(sub, pattern="**/*"):
        return [p for p in glob.glob(path(sub, pattern), recursive=True) if os.path.isfile(p) and not os.path.basename(p).startswith(".")]

    for kind, sub in (("briefs", "briefs"), ("reports", "reports"), ("inbox-reports", "inbox-reports"), ("handovers", "handovers"), ("notes", "notes"), ("questions", "questions")):
        found = files(sub)
        texty = [p for p in found if p.endswith((".md", ".txt", ".json", ".jsonl"))]
        sizes = [os.path.getsize(p) for p in found]
        if kind in ("briefs", "handovers", "reports"):
            dates = [os.path.basename(p)[:8] for p in found]
        else:
            dates = [datetime.date.fromtimestamp(os.path.getmtime(p)).isoformat() for p in found]
        result[kind] = {
            "files": dist(sizes),
            "text_files": len(texty),
            "text_bytes": sum(os.path.getsize(p) for p in texty),
            "lines": sum(read(p).count("\n") for p in texty),
            "per_week": per_week(dates),
        }
    reports = files("reports")
    report_sizes = [os.path.getsize(p) for p in reports]
    result["reports"]["over_inline_4k"] = sum(1 for s in report_sizes if s > INLINE_REPORT_MAX)
    result["reports"]["over_prompt_12k"] = sum(1 for s in report_sizes if s > PROMPT_REPORT_BUDGET)
    result["reports"]["subjects"] = len({re.sub(r"^\d{8}-\d{6}-", "", os.path.basename(p)) for p in reports})
    result["reports"]["with_report_heading"] = sum(1 for p in reports if "### Report" in read(p))
    result["reports"]["with_learned"] = sum(1 for p in reports if re.search(r"^\s*[-*]?\s*\**Learned\**:", read(p), re.M))
    result["reports"]["learned_none"] = sum(1 for p in reports if re.search(r"^\s*[-*]?\s*\**Learned\**:\**\s*\"?none", read(p), re.M | re.I))
    handovers = files("handovers")
    result["handovers"]["over_6k"] = sum(1 for p in handovers if os.path.getsize(p) > 6 * 1024)
    briefs = files("briefs")
    result["briefs"]["quoting_decisions"] = sum(1 for p in briefs if re.search(r"decision", read(p), re.I))
    result["briefs"]["with_sections"] = sum(1 for p in briefs if all(h in read(p) for h in ("## Goal", "## Context", "## Scope", "## Done when")))

    # Single files.
    for kind, name in (("handover", "handover.md"), ("backlog", "backlog.md"), ("inbox", "inbox.jsonl"), ("answers", "answers.jsonl"), ("role", "ROLE.md")):
        text = read(path(name))
        result[kind] = {"bytes": len(text.encode()), "lines": text.count("\n")}
    result["backlog"]["entries"] = len(re.findall(r"^## ", read(path("backlog.md")), re.M))
    inbox = [json.loads(l) for l in read(path("inbox.jsonl")).splitlines() if l.strip().startswith("{")]
    result["inbox"]["items"] = len(inbox)
    result["inbox"]["kinds"] = {}
    for item in inbox:
        kind = item.get("kind", "wake")
        result["inbox"]["kinds"][kind] = result["inbox"]["kinds"].get(kind, 0) + 1
    result["inbox"]["inline_report"] = dist([len(i["report"].encode()) for i in inbox if i.get("report")])
    result["inbox"]["undelivered"] = sum(1 for i in inbox if not i.get("delivered") and not i.get("delivered_at"))
    result["inbox"]["first"] = min((i.get("ts") or "9" for i in inbox), default=None)
    result["inbox"]["keys"] = sorted({k for i in inbox for k in i})
    result["other_files"] = sorted(
        name for name in os.listdir(path())
        if name not in ("decisions.md", "ledger.jsonl", "ledger-archive.jsonl", "briefs", "reports", "inbox.jsonl", "inbox-reports", "inbox.lock", "handover.md", "handovers", "notes", "questions", "backlog.md", "answers.jsonl", "ROLE.md", "launch.json", "launch-prompt.txt", ".DS_Store")
    )
    result["total_bytes"] = sum(os.path.getsize(p) for p in glob.glob(path("**"), recursive=True) if os.path.isfile(p))
    return result


def main():
    now = datetime.datetime.now(datetime.timezone.utc).timestamp()
    report = {name: measure(directory, now) for directory, name in names().items()}
    if len(sys.argv) > 2 and sys.argv[1] == "--json":
        json.dump(report, open(sys.argv[2], "w"), indent=1)
    json.dump(report, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
