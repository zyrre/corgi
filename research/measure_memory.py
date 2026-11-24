#!/usr/bin/env python3
"""Measure the Claude Code agent memory folders and who touched them.

Per `~/.claude/projects/<slug>/memory/` that holds anything: files, bytes,
the index's lines and longest line (Claude Code loads MEMORY.md at session
start), entries per type, files the index does not name and index lines
naming no file, the age of each file; then, from
`<calls>-agent-memory.jsonl` (extract_calls.py), the reads and writes of
memory files by supervisor sessions and by other sessions.

    python3 research/measure_memory.py /scratch/sessions.jsonl /scratch/calls.jsonl

Folders are named `corgi` or by a letter, as in the write-up. Reads only.
"""

import collections
import datetime
import glob
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402

ROOT = os.path.expanduser("~/.claude/projects")


def label(slug, names):
    for directory, project in names.items():
        if slug.endswith("-repos-" + directory):
            return project
    return None


def main():
    sessions = [json.loads(line) for line in open(sys.argv[1])]
    rows = [json.loads(line) for line in open(sys.argv[2].replace(".jsonl", "") + "-agent-memory.jsonl")]
    supervisor = {s["file"].split("/")[-1][:8] for s in sessions if s["supervisor_of"]}
    names = written.names()
    now = datetime.datetime.now().timestamp()
    out = {}
    other = 0
    for folder in sorted(glob.glob(os.path.join(ROOT, "*", "memory"))):
        files = [f for f in glob.glob(os.path.join(folder, "*")) if os.path.isfile(f)]
        if not files:
            continue
        slug = os.path.basename(os.path.dirname(folder))
        name = label(slug, names)
        if name is None:
            other += 1
            name = f"other folder {other}"
        index = written.read(os.path.join(folder, "MEMORY.md"))
        entries = [f for f in files if os.path.basename(f) != "MEMORY.md"]
        linked = set(re.findall(r"\]\(([^)]+\.md)\)", index))
        types = collections.Counter()
        for entry in entries:
            found = re.search(r"^\s*type:\s*(\w+)", written.read(entry), re.M)
            types[found.group(1) if found else "untyped"] += 1
        ages = sorted(int((now - os.path.getmtime(f)) / 86400) for f in entries)
        mine = [r for r in rows if r["memory_slug"] == slug or r["slug"] == slug]
        touch = collections.Counter()
        files_read = collections.Counter()
        for row in mine:
            who = "supervisor" if row["session"] in supervisor else "other session"
            kind = "write" if row["form"] in ("tool-write", "raw-write", "raw-manage") else "list" if row["form"] in ("raw-list", "tool-glob") else "read"
            touch[f"{who}: {kind}"] += 1
            if kind == "read":
                files_read[os.path.basename(row["arg"])] += 1
        out[name] = {
            "entry files": len(entries),
            "bytes in all": sum(os.path.getsize(f) for f in files),
            "index bytes": len(index.encode()),
            "index lines": index.count("\n"),
            "longest index line": max((len(line) for line in index.splitlines()), default=0),
            "entry bytes: median, max": [sorted(os.path.getsize(f) for f in entries)[len(entries) // 2], max(os.path.getsize(f) for f in entries)] if entries else None,
            "entries by type": dict(types),
            "entry files the index does not name": len([f for f in entries if os.path.basename(f) not in linked]),
            "index links to no file": len([l for l in linked if not os.path.exists(os.path.join(folder, l))]),
            "days since an entry was last written: median, oldest": [ages[len(ages) // 2], ages[-1]] if ages else None,
            "entries untouched for 30+ days": sum(1 for a in ages if a >= 30),
            "tool calls on the folder": dict(touch),
            "distinct entry files read by a tool": len([f for f in files_read if f and f != "MEMORY.md"]),
        }
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
