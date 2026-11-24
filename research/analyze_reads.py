#!/usr/bin/env python3
"""Count how supervisors read their memory, from the calls extract_calls.py
found, and follow each decision from the call that wrote it to every later
time it was shown or looked up.

    python3 research/analyze_reads.py /scratch/sessions.jsonl /scratch/calls.jsonl [/scratch/reads.json]

Prints JSON keyed by the write-up's project names (`corgi`, project A, ...).
Reads only. `decisions` under each project lists every decision by heading:
keep that part out of the repository for projects other than corgi.
"""

import collections
import datetime
import json
import os
import re
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402

FORMS = ["digest", "digest--decision", "digest--search", "digest--show", "digest--ledger", "digest--brief", "report", "inbox", "inbox-take", "inbox-delivered", "fleet", "spawn", "tag", "role-handover"]
HIT = re.compile(r"^\[(\d+)\] (\S+?):(\d+)  ", re.M)
SECTIONS = ["# Project", "# Handover note", "# Open threads from the previous", "# Open work", "# Recently finished", "# Recent decisions", "# Older decisions", "# Notes"]


def dist(values):
    values = sorted(v for v in values if v is not None)
    if not values:
        return {"n": 0}
    return {"n": len(values), "sum": sum(values), "median": int(statistics.median(values)), "p90": values[min(len(values) - 1, int(len(values) * 0.9))], "max": values[-1]}


def week(stamp):
    day = datetime.date.fromisoformat(stamp[:10])
    year, number, _ = day.isocalendar()
    return f"{year}-W{number:02d}"


def digest_text(result):
    """The digest inside a Bash result that may hold other commands' output."""
    start = result.find("# Project ")
    if start < 0:
        return None
    text = result[start:]
    end = text.find("\nSearch all of your memory")
    if end >= 0:
        stop = text.find("\n", end + 1)
        text = text if stop < 0 else text[:stop]
        # Warnings follow the search line.
        rest = result[start + len(text):]
        for line in rest.splitlines()[1:]:
            if line.startswith("warning:"):
                text += "\n" + line
            else:
                break
    return text


def sections(text):
    """Bytes of each digest section, by its heading."""
    marks = []
    for name in SECTIONS:
        found = re.search(r"^" + re.escape(name), text, re.M)
        if found:
            marks.append((found.start(), name))
    marks.sort()
    sizes = {}
    for index, (start, name) in enumerate(marks):
        end = marks[index + 1][0] if index + 1 < len(marks) else len(text)
        sizes[name] = len(text[start:end].encode())
    return sizes


def section(text, name, until):
    start = text.find(name)
    if start < 0:
        return ""
    end = min([text.find(u, start + 1) for u in until if text.find(u, start + 1) >= 0] or [len(text)])
    return text[start:end]


def source_kind(path):
    if path.startswith("decisions"):
        return "decisions"
    if path.startswith("ledger-archive"):
        return "ledger-archive"
    if path.startswith("ledger"):
        return "ledger"
    for kind in ("briefs", "reports", "notes", "handovers"):
        if path.startswith(kind + "/"):
            return kind
    if path.startswith("handover.md"):
        return "handover"
    return "other"


def analyse(project, directory, sessions, calls):
    out = {}
    own = [c for c in calls if c["project"] == directory]
    mine = [s for s in sessions if s["supervisor_of"] == directory]
    span_days = 0
    if mine:
        first = min(s["first_ts"] for s in mine if s["first_ts"])
        last = max(s["last_ts"] for s in mine if s["last_ts"])
        span_days = (datetime.datetime.fromisoformat(last.replace("Z", "+00:00")) - datetime.datetime.fromisoformat(first.replace("Z", "+00:00"))).days + 1
        out["sessions"] = {
            "n": len(mine),
            "first": first[:10],
            "last": last[:10],
            "weeks": round(span_days / 7, 1),
            "started_by_handover": sum(1 for s in mine if "take over from the previous" in (s["first_prompt"] or "")),
            "first_context_tokens": dist([s["first_context"] for s in mine]),
            "max_context_tokens": dist([s["max_context"] for s in mine]),
            "over_200k": sum(1 for s in mine if s["max_context"] > 200_000),
            "compactions": sum(s["compactions"] for s in mine),
            "user_prompts": dist([s["users"] for s in mine]),
            "transcript_bytes": sum(s["bytes"] for s in mine),
        }
    weeks = max(span_days / 7, 1e-9)

    # Each command form: calls, sessions that used it, per week, bytes back.
    forms = {}
    for form in FORMS:
        rows = [c for c in own if c["form"] == form]
        forms[form] = {
            "calls": len(rows),
            "sessions_using": len({c["session"] for c in rows}),
            "per_session": round(len(rows) / max(len(mine), 1), 2),
            "per_week": round(len(rows) / weeks, 1),
            "result_bytes": dist([c["result_bytes"] for c in rows if c.get("parts") == 1]),
            "by_week": dict(sorted(collections.Counter(week(c["ts"]) for c in rows).items())),
        }
    out["forms"] = forms

    # Raw reads and writes of state files, by artifact kind, own state only.
    raw = collections.defaultdict(lambda: collections.Counter())
    for call in own:
        if call["form"].startswith(("raw-", "tool-")):
            where = "own" if call.get("state_project") == directory else "other-project"
            raw[where + ":" + call["artifact"]][call["form"]] += 1
    out["raw"] = {key: dict(value) for key, value in sorted(raw.items())}
    raw_sessions = collections.defaultdict(set)
    for call in own:
        if call["form"] in ("raw-read", "tool-read", "tool-grep") and call.get("state_project") == directory:
            raw_sessions[call["artifact"]].add(call["session"])
    out["raw_read_sessions"] = {key: len(value) for key, value in sorted(raw_sessions.items())}

    # The plain digest: size and what each section takes.
    digests = []
    for call in own:
        if call["form"] != "digest":
            continue
        text = digest_text(call.get("result") or "")
        if text is None:
            continue
        digests.append({"ts": call["ts"], "session": call["session"], "text": text, "bytes": len(text.encode()), "sections": sections(text)})
    digests.sort(key=lambda d: d["ts"])
    seen_sessions = set()
    for digest in digests:
        digest["first_in_session"] = digest["session"] not in seen_sessions
        seen_sessions.add(digest["session"])
    recent = [d for d in digests if d["ts"] >= "2026-10-28"]
    shares = collections.defaultdict(list)
    for digest in recent:
        for name in SECTIONS:
            shares[name].append(digest["sections"].get(name, 0))
    out["digest"] = {
        "runs": len(digests),
        "session_starts": sum(1 for d in digests if d["first_in_session"]),
        "reruns_in_session": sum(1 for d in digests if not d["first_in_session"]),
        "sessions_without": len(mine) - len(seen_sessions),
        "bytes": dist([d["bytes"] for d in digests]),
        "bytes_since_bounds": dist([d["bytes"] for d in recent]),
        "tokens_since_bounds_est": dist([round(d["bytes"] / 3.6) for d in recent]),
        "section_bytes_since_bounds": {name: dist(values) for name, values in shares.items()},
        "section_share_since_bounds": {
            name: round(sum(values) / max(sum(d["bytes"] for d in recent), 1), 3) for name, values in shares.items()
        },
        "with_handover_note": sum(1 for d in recent if "# Handover note" in d["sections"]),
        "with_previous_threads": sum(1 for d in recent if "# Open threads from the previous" in d["sections"]),
        "left_out_lines": sum(1 for d in recent if "older left out for size" in d["text"]),
        "stale_reported_lines": sum(1 for d in recent if "more `reported`, agent not running" in d["text"]),
        "not_running_marks": dist([d["text"].count("**NOT RUNNING**") for d in recent]),
        "open_work_count": dist([int(m.group(1)) for d in recent for m in [re.search(r"^# Open work \((\d+)\)", d["text"], re.M)] if m]),
        "titles_left_out": dist([int(m.group(1)) for d in recent for m in [re.search(r"and (\d+) older titles left out", d["text"])] if m]),
        "full_decisions_shown": dist([len(re.findall(r"^## ", section(d["text"], "# Recent decisions", ["# Older decisions"]), re.M)) for d in recent]),
        "titles_shown": dist([len(re.findall(r"^\d{4}-\d\d-\d\d  ", section(d["text"], "# Older decisions", ["\n# Notes"]), re.M)) for d in recent]),
        "superseded_warnings": sum(d["text"].count("matches no earlier heading") for d in recent),
        "unreadable_ledger_warnings": sum(d["text"].count("so it is skipped") for d in recent),
        "clipped_ledger_texts": dist([section(d["text"], "# Open work", ["# Recent decisions"]).count("…") for d in recent]),
    }

    # Searches.
    searches = [c for c in own if c["form"] == "digest--search"]
    kinds, cut, folded, nohit, totals, shown_counts = collections.Counter(), 0, 0, 0, [], []
    top_kind = collections.Counter()
    for call in searches:
        text = call.get("result") or ""
        start = text.find("# Search ")
        if "No hits for" in text and start < 0:
            nohit += 1
            continue
        body = text[start:] if start >= 0 else text
        hits = HIT.findall(body)
        shown_counts.append(len(hits))
        for position, (_, path, _) in enumerate(hits):
            kinds[source_kind(path)] += 1
            if position == 0:
                top_kind[source_kind(path)] += 1
        head = re.search(r": (\d+) hits?(?:, and (\d+) near-duplicates)?", body)
        if head:
            totals.append(int(head.group(1)))
            folded += 1 if head.group(2) else 0
        if "more hit" in body and "left out" in body:
            cut += 1
    by_time = sorted(own, key=lambda c: c["ts"])
    followed = collections.Counter()
    for index, call in enumerate(by_time):
        if call["form"] != "digest--search":
            continue
        after = [n for n in by_time[index + 1: index + 12] if n["session"] == call["session"] and n["id"] != call["id"]][:6]
        kinds_after = [n["form"] for n in after]
        if "digest--search" in kinds_after[:2]:
            followed["another search within 2 calls"] += 1
        if any(k in ("digest--show", "digest--ledger", "digest--brief", "digest--decision") for k in kinds_after[:3]):
            followed["a lookup command within 3 calls"] += 1
        if any(k in ("raw-read", "tool-read", "tool-grep") for k in kinds_after[:3]):
            followed["a raw read within 3 calls"] += 1
    out["search"] = {
        "runs": len(searches),
        "no_hits": nohit,
        "hits_total": dist(totals),
        "over_100_hits": sum(1 for t in totals if t > 100),
        "hits_shown": dist(shown_counts),
        "cut_by_cap": cut,
        "with_folding": folded,
        "result_bytes": dist([c["result_bytes"] for c in searches if c.get("parts") == 1]),
        "shown_hits_by_source": dict(kinds.most_common()),
        "top_hit_source": dict(top_kind.most_common()),
        "words_per_query": dist([len(c["arg"].split()) for c in searches]),
        "followed_by": dict(followed),
    }

    # Decisions: from the call that wrote each to every later showing.
    text = written.read(os.path.join(written.ROOT, directory, "decisions.md"))
    entries = written.parse_decisions(text)
    line_ends = [e["line"] for e in entries[1:]] + [text.count("\n") + 2]
    by_heading = {e["heading"]: i for i, e in enumerate(entries)}

    def at_line(line):
        for i, entry in enumerate(entries):
            if entry["line"] <= line < line_ends[i]:
                return i
        return None

    track = [
        {"heading": e["heading"], "bytes": len(e["text"].encode()), "written": None, "written_session": None, "full": [], "title": [], "lookups": []}
        for e in entries
    ]
    writes = sorted((c for c in own if c["artifact"] == "decisions" and c.get("payload")), key=lambda c: c["ts"])
    for call in writes:
        for heading in re.findall(r"^## (.+)$", call["payload"], re.M):
            i = by_heading.get(heading.strip())
            if i is not None and track[i]["written"] is None:
                track[i]["written"], track[i]["written_session"] = call["ts"], call["session"]
    for digest in digests:
        for heading in re.findall(r"^## (.+)$", section(digest["text"], "# Recent decisions", ["# Older decisions"]), re.M):
            i = by_heading.get(heading.strip())
            if i is not None:
                track[i]["full"].append((digest["ts"], digest["session"]))
        for date, title in re.findall(r"^(\d{4}-\d\d-\d\d)  (.+)$", section(digest["text"], "# Older decisions", ["\n# Notes"]), re.M):
            i = by_heading.get(f"{date}: {title.strip()}")
            if i is not None:
                track[i]["title"].append((digest["ts"], digest["session"]))
    for call in own:
        result = call.get("result") or ""
        found = set()
        if call["form"] == "digest--decision" and not result.startswith("No decision contains"):
            found = {by_heading.get(h.strip()) for h in re.findall(r"^## (.+)$", result, re.M)}
            how = "decision"
        elif call["form"] in ("digest--search", "digest--decision"):
            found = {at_line(int(line)) for _, path, line in HIT.findall(result) if path == "decisions.md"}
            how = "search"
        elif call["form"] == "digest--show" and call["arg"].startswith("decisions.md"):
            found = {by_heading.get(h.strip()) for h in re.findall(r"^## (.+)$", result, re.M)}
            how = "show"
        elif call["form"] in ("raw-read", "tool-read", "tool-grep") and call["artifact"] == "decisions" and call.get("state_project") == directory:
            found = {by_heading.get(h.strip()) for h in re.findall(r"## (\d{4}-\d\d-\d\d: [^\n]+)", result)}
            how = "raw"
        for i in found:
            if i is not None:
                track[i]["lookups"].append((call["ts"], call["session"], how))
    first_ts = min((s["first_ts"] for s in mine if s["first_ts"]), default="9")
    summary = collections.Counter()
    lookup_age_days, lookup_region = [], collections.Counter()
    for i, item in enumerate(track):
        date = item["heading"][:10] if re.match(r"\d{4}-\d\d-\d\d", item["heading"]) else None
        in_window = item["written"] is not None
        later = [l for l in item["lookups"] if item["written"] is None or l[0] > item["written"]]
        active = [l for l in later if l[2] != "raw"]
        other_session = [l for l in active if l[1] != item["written_session"]]
        item["active_lookups"] = len(active)
        item["active_lookups_later_session"] = len(other_session)
        item["raw_lookups"] = len(later) - len(active)
        full_after = [f for f in item["full"] if item["written"] is None or f[0] > item["written"]]
        item["full_shown"] = len(full_after)
        item["title_shown"] = len(item["title"])
        later_digests = [d for d in digests if item["written"] and d["ts"] > item["written"]]
        aged_out = bool(later_digests) and not any(f[0] == later_digests[-1]["ts"] for f in item["full"])
        item["aged_out"] = aged_out
        group = "written_in_window" if in_window else "older"
        summary[group] += 1
        if active:
            summary[group + ":looked_up"] += 1
        if other_session:
            summary[group + ":looked_up_in_later_session"] += 1
        if in_window:
            if not active and full_after:
                summary["written_in_window:only_seen_in_digest"] += 1
            if not active and not full_after:
                summary["written_in_window:never_shown_in_full_nor_looked_up"] += 1
            if aged_out:
                summary["written_in_window:aged_out_of_full_text"] += 1
                if not active:
                    summary["written_in_window:aged_out_never_looked_up"] += 1
                if not other_session:
                    summary["written_in_window:aged_out_never_looked_up_in_later_session"] += 1
            summary["written_in_window:full_showings"] += len(full_after)
        for stamp, session, how in active:
            if date:
                lookup_age_days.append((datetime.date.fromisoformat(stamp[:10]) - datetime.date.fromisoformat(date)).days)
            before = [d for d in digests if d["session"] == session and d["ts"] <= stamp]
            if not before:
                lookup_region["no digest in session"] += 1
            elif any(f[0] == before[-1]["ts"] for f in item["full"]):
                lookup_region["shown in full by the session's digest"] += 1
            elif any(t[0] == before[-1]["ts"] for t in item["title"]):
                lookup_region["shown as a title"] += 1
            else:
                lookup_region["not in the digest (left out, superseded or newer)"] += 1
    full_counts = [item["full_shown"] for item in track if item["written"]]
    out["decisions"] = {
        "entries": len(entries),
        "summary": dict(sorted(summary.items())),
        "full_showings_per_decision": dist(full_counts),
        "lookup_age_days": dist(lookup_age_days),
        "lookup_age_over_14d": sum(1 for d in lookup_age_days if d > 14),
        "lookup_age_over_30d": sum(1 for d in lookup_age_days if d > 30),
        "lookup_events": len(lookup_age_days),
        "lookup_region": dict(lookup_region),
        "lookups_by_form": dict(collections.Counter(l[2] for item in track for l in item["lookups"])),
        "decision_queries": [c["arg"] for c in own if c["form"] == "digest--decision"] if project == "corgi" else len([c for c in own if c["form"] == "digest--decision"]),
        "decision_fallbacks": sum(1 for c in own if c["form"] == "digest--decision" and (c.get("result") or "").startswith("No decision")),
        "track": track if project == "corgi" else None,
    }
    return out


def main():
    sessions = [json.loads(line) for line in open(sys.argv[1])]
    calls = [json.loads(line) for line in open(sys.argv[2])]
    report = {}
    for directory, project in written.names().items():
        report[project] = analyse(project, directory, sessions, calls)
    text = json.dumps(report, indent=1)
    if len(sys.argv) > 3:
        open(sys.argv[3], "w").write(text)
    else:
        print(text)


if __name__ == "__main__":
    main()
