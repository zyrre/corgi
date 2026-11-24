#!/usr/bin/env python3
"""Index every Claude Code transcript on this machine.

One JSON line per transcript: where it ran, when, how it started, and whether
it is a supervisor session ("Start your supervisor session for <project>").
Reads only. Output goes to the path given as argv[1] (keep it outside the
repository: it names every project on the machine).

    python3 research/index_sessions.py /scratch/sessions.jsonl
"""

import glob
import json
import os
import re
import sys

PROJECTS = os.path.expanduser("~/.claude/projects")
SUPERVISOR_START = re.compile(r"Start your supervisor session for ([^\s.]+(?:\.[^\s.]+)*?)\. ")


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            part.get("text", "") for part in content if isinstance(part, dict) and part.get("type") == "text"
        )
    return ""


def index(path):
    row = {
        "file": path,
        "slug": os.path.basename(os.path.dirname(path)),
        "bytes": os.path.getsize(path),
        "cwd": None,
        "first_ts": None,
        "last_ts": None,
        "first_prompt": None,
        "supervisor_of": None,
        "users": 0,
        "assistants": 0,
        "compactions": 0,
        "first_context": None,
        "max_context": 0,
        "model": None,
    }
    with open(path, errors="replace") as handle:
        for line in handle:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            kind = record.get("type")
            stamp = record.get("timestamp")
            if stamp and kind in ("user", "assistant"):
                row["first_ts"] = row["first_ts"] or stamp
                row["last_ts"] = stamp
            if row["cwd"] is None and record.get("cwd"):
                row["cwd"] = record["cwd"]
            if kind == "user" and not record.get("isSidechain"):
                message = record.get("message") or {}
                content = message.get("content")
                is_result = isinstance(content, list) and any(
                    isinstance(part, dict) and part.get("type") == "tool_result" for part in content
                )
                if record.get("isCompactSummary"):
                    row["compactions"] += 1
                if not is_result:
                    row["users"] += 1
                    text = text_of(content)
                    if row["first_prompt"] is None and text.strip() and not record.get("isMeta"):
                        row["first_prompt"] = text[:600]
                        found = SUPERVISOR_START.search(text[:400])
                        if found:
                            row["supervisor_of"] = found.group(1)
            elif kind == "assistant" and not record.get("isSidechain"):
                row["assistants"] += 1
                message = record.get("message") or {}
                row["model"] = message.get("model") or row["model"]
                usage = message.get("usage") or {}
                context = (
                    usage.get("input_tokens", 0)
                    + usage.get("cache_creation_input_tokens", 0)
                    + usage.get("cache_read_input_tokens", 0)
                )
                if context:
                    if row["first_context"] is None:
                        row["first_context"] = context
                    row["max_context"] = max(row["max_context"], context)
    return row


def main():
    out = sys.argv[1]
    files = sorted(glob.glob(os.path.join(PROJECTS, "*", "*.jsonl")))
    with open(out, "w") as handle:
        for path in files:
            handle.write(json.dumps(index(path)) + "\n")
    print(f"{len(files)} transcripts indexed to {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
