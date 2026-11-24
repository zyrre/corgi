#!/usr/bin/env python3
"""Do briefs carry the decisions the role tells supervisors to put in them?

Per state directory: how many briefs name a decision (its title, or its date
beside the word "decision"), how old the named decision was when the brief
was written, and whether the digest of that day would have shown it in full,
as a title, or not at all (the digest's arithmetic from measure_written.py,
run on the decisions that existed on the brief's day).

    python3 research/analyze_briefs.py [--list corgi]

Prints JSON keyed by the write-up's project names. Reads only.
"""

import datetime
import glob
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402


def norm(text):
    return re.sub(r"[^a-z0-9]+", " ", text.lower()).strip()


def main():
    listed = sys.argv[2] if len(sys.argv) > 2 and sys.argv[1] == "--list" else None
    report = {}
    for directory, project in written.names().items():
        path = os.path.join(written.ROOT, directory)
        entries = written.parse_decisions(written.read(os.path.join(path, "decisions.md")))
        dated = []
        for entry in entries:
            found = re.match(r"(\d{4}-\d\d-\d\d): (.+)", entry["heading"])
            if found:
                dated.append((found.group(1), found.group(2), entry))
        briefs = sorted(glob.glob(os.path.join(path, "briefs", "*.md")))
        counts = {"briefs": 0, "name a decision by title": 0, "name a decision by date": 0, "name any decision": 0, "say 'decision' at all": 0}
        ages, regions, rows = [], {"in full": 0, "as a title": 0, "left out": 0, "superseded": 0}, []
        by_week = {}
        for brief in briefs:
            name = os.path.basename(brief)
            if not re.match(r"\d{8}-", name):
                continue
            day = f"{name[:4]}-{name[4:6]}-{name[6:8]}"
            text = written.read(brief)
            flat = norm(text)
            counts["briefs"] += 1
            if re.search(r"decision", text, re.I):
                counts["say 'decision' at all"] += 1
            cited = []
            for date, title, entry in dated:
                if date > day:
                    continue
                key = norm(title)
                by_title = len(key) >= 20 and (key in flat or key[:45] in flat and len(key) >= 45)
                by_date = bool(re.search(r"decision[^.\n]{0,80}" + re.escape(date) + r"|" + re.escape(date) + r"[^.\n]{0,80}decision", text, re.I))
                if by_title:
                    cited.append((date, title, entry, "title"))
                elif by_date:
                    cited.append((date, title, entry, "date"))
            titles = [c for c in cited if c[3] == "title"]
            if titles:
                counts["name a decision by title"] += 1
            elif cited:
                counts["name a decision by date"] += 1
            if cited:
                counts["name any decision"] += 1
            week = written.week(day)
            by_week.setdefault(week, [0, 0])
            by_week[week][0] += 1
            by_week[week][1] += 1 if cited else 0
            # Where the digest of that day would have shown each named decision.
            then = [e for d, _, e in dated if d <= day]
            view = written.decision_view(then)
            full = {e["heading"] for e in view["recent"]}
            shown = {e["heading"] for e in view["titles"]}
            for date, title, entry, how in titles:
                age = (datetime.date.fromisoformat(day) - datetime.date.fromisoformat(date)).days
                ages.append(age)
                if entry["heading"] in full:
                    regions["in full"] += 1
                elif entry["heading"] in shown:
                    regions["as a title"] += 1
                elif any(entry["heading"] in e["supersedes"] for e in then):
                    regions["superseded"] += 1
                else:
                    regions["left out"] += 1
                rows.append((name, date, title[:70], age))
        ordered = sorted(ages)
        report[project] = {
            **counts,
            "decisions named by title, in all": len(ages),
            "age in days of a named decision: median": ordered[len(ordered) // 2] if ordered else None,
            "age: p90": ordered[int(len(ordered) * 0.9)] if ordered else None,
            "named decisions older than 7 days": sum(1 for a in ages if a > 7),
            "named decisions older than 30 days": sum(1 for a in ages if a > 30),
            "where that day's digest showed the named decision": regions,
            "briefs naming a decision, by week (briefs, naming)": dict(sorted(by_week.items())),
        }
        if listed == project:
            report[project]["rows"] = rows
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
