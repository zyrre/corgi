#!/usr/bin/env python3
"""Do agents read the repository's AGENTS.md and ARCHITECTURE.md, where the
kept `Learned` lessons go?

Claude Code loads CLAUDE.md by itself; this repository's CLAUDE.md only
points at AGENTS.md, so an agent has AGENTS.md in its context only if it
reads it. Per transcript of the named repository (its primary checkout and
its worktrees), this counts the reads of each file: whole (the Read tool
without a range), a range, a search (Grep, or grep/rg in Bash), and reads the
tool refused for size.

    python3 research/analyze_docs.py /scratch/sessions.jsonl corgi

Reads only.
"""

import collections
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from extract_calls import strip_heredocs  # noqa: E402

FILES = ("AGENTS.md", "ARCHITECTURE.md")
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_lines = {}


def lines_on(day, doc):
    """How many lines `doc` had on the base branch at the end of `day`."""
    if (day, doc) not in _lines:
        commit = subprocess.run(["git", "-C", REPO, "rev-list", "-1", f"--before={day} 23:59", "HEAD"], capture_output=True, text=True).stdout.strip()
        text = subprocess.run(["git", "-C", REPO, "show", f"{commit}:{doc}"], capture_output=True, text=True).stdout if commit else ""
        _lines[(day, doc)] = text.count("\n")
    return _lines[(day, doc)]


def covered(name, args, doc):
    """The line ranges of `doc` a call asks for, as (from, to) pairs; (1, None) is the whole file."""
    if name == "Read":
        if not (args.get("offset") or args.get("limit")):
            return [(1, None)]
        start = int(args.get("offset") or 1)
        return [(start, start + int(args.get("limit") or 2000) - 1)]
    command = strip_heredocs(args.get("command", ""))
    ranges = []
    for segment in re.split(r"[;&|\n]+", command):
        if doc not in segment:
            continue
        found = re.findall(r"sed -n ['\"]?(\d+),(\d+)p", segment)
        ranges += [(int(a), int(b)) for a, b in found]
        if not found and re.search(r"\bcat\b", segment):
            ranges.append((1, None))
        head = re.search(r"\bhead -n? ?(\d+)", segment)
        if head and not found:
            ranges.append((1, int(head.group(1))))
    # `cat AGENTS.md | head -150` splits at the pipe: the head limits the cat.
    piped = re.search(r"cat [^|;&]*" + re.escape(doc) + r"\s*\|\s*head -n? ?(\d+)", command)
    if piped:
        ranges = [r for r in ranges if r != (1, None)] + [(1, int(piped.group(1)))]
    return ranges


def main():
    sessions = [json.loads(line) for line in open(sys.argv[1])]
    repo = sys.argv[2]
    primary = re.compile(r"-repos-" + re.escape(repo) + r"$")
    worktree = re.compile(r"-herdr-worktrees-" + re.escape(repo) + r"-worktree-")
    rows = []
    for session in sessions:
        role = "supervisor" if session["supervisor_of"] == repo else "worker" if worktree.search(session["slug"]) else "other-primary" if primary.search(session["slug"]) else None
        if role is None:
            continue
        seen = {name: collections.Counter() for name in FILES}
        spans = {name: [] for name in FILES}
        pending = {}
        sizes = {name: [] for name in FILES}
        with open(session["file"], errors="replace") as handle:
            for line in handle:
                if "AGENTS.md" not in line and "ARCHITECTURE.md" not in line and '"tool_result"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if record.get("type") not in ("user", "assistant") or record.get("isSidechain"):
                    continue
                content = (record.get("message") or {}).get("content")
                if not isinstance(content, list):
                    continue
                for part in content:
                    if not isinstance(part, dict):
                        continue
                    if part.get("type") == "tool_use":
                        name, args = part.get("name"), part.get("input") or {}
                        for doc in FILES:
                            how = None
                            if name == "Read" and str(args.get("file_path", "")).endswith("/" + doc):
                                how = "range" if args.get("offset") or args.get("limit") else "whole"
                            elif name == "Grep" and str(args.get("path", "")).endswith(doc):
                                how = "search"
                            elif name == "Bash" and doc in strip_heredocs(args.get("command", "")):
                                command = strip_heredocs(args["command"])
                                if re.search(r"\b(grep|rg)\b[^|;&]*" + re.escape(doc), command):
                                    how = "search"
                                elif re.search(r"\b(cat|sed|head|tail|awk|less)\b[^|;&]*" + re.escape(doc), command):
                                    how = "range" if re.search(r"\b(sed|head|tail|awk)\b", command) else "whole"
                            elif name in ("Edit", "Write") and str(args.get("file_path", "")).endswith("/" + doc):
                                how = "edit"
                            if how in ("whole", "range"):
                                spans[doc] += covered(name, args, doc)
                            if how:
                                seen[doc][how] += 1
                                pending[part.get("id")] = (doc, how)
                    elif part.get("type") == "tool_result" and part.get("tool_use_id") in pending:
                        doc, how = pending.pop(part["tool_use_id"])
                        body = part.get("content")
                        text = body if isinstance(body, str) else "\n".join(p.get("text", "") for p in body or [] if isinstance(p, dict))
                        if how == "whole" and (part.get("is_error") or "exceeds maximum" in text[:300]):
                            seen[doc]["whole"] -= 1
                            seen[doc]["refused"] += 1
                        elif how in ("whole", "range"):
                            sizes[doc].append(len(text.encode()))
        share = {}
        for doc in FILES:
            total = lines_on((session["first_ts"] or "2026-11-24")[:10], doc) or 1
            lines = set()
            for start, end in spans[doc]:
                lines.update(range(start, min(end or total, total) + 1))
            share[doc] = len(lines) / total
        rows.append({"share": share, "role": role, "week": (session["first_ts"] or "")[:10], "seen": seen, "sizes": sizes, "first_context": session["first_context"], "max_context": session["max_context"]})

    for role in ("supervisor", "worker", "other-primary"):
        mine = [r for r in rows if r["role"] == role]
        if not mine:
            continue
        print(f"== {role}: {len(mine)} transcripts")
        contexts = sorted(r["first_context"] for r in mine if r["first_context"])
        if contexts:
            print(f"   context at the first request: median {contexts[len(contexts) // 2]} tokens")
        for doc in FILES:
            any_read = sum(1 for r in mine if any(r["seen"][doc][how] for how in ("whole", "range", "search")))
            whole = sum(1 for r in mine if r["seen"][doc]["whole"])
            ranged = sum(1 for r in mine if r["seen"][doc]["range"] and not r["seen"][doc]["whole"])
            only_search = sum(1 for r in mine if r["seen"][doc]["search"] and not r["seen"][doc]["whole"] and not r["seen"][doc]["range"])
            refused = sum(1 for r in mine if r["seen"][doc]["refused"])
            edited = sum(1 for r in mine if r["seen"][doc]["edit"])
            read_bytes = sorted(sum(r["sizes"][doc]) for r in mine if r["sizes"][doc])
            print(
                f"   {doc}: read at all in {any_read} ({100 * any_read // len(mine)}%), whole in {whole}, "
                f"ranges only in {ranged}, searched only in {only_search}, a whole read refused in {refused}, edited in {edited}; "
                f"bytes read per reading transcript median {read_bytes[len(read_bytes) // 2] if read_bytes else 0}"
            )
        # By start week, for the workers: does reading fall as the file grows?
        if role == "worker":
            weeks = collections.defaultdict(lambda: [0, 0, 0])
            for r in mine:
                key = r["week"][:7] + ("-a" if r["week"][8:10] < "16" else "-b")
                weeks[key][0] += 1
                weeks[key][1] += 1 if r["seen"]["AGENTS.md"]["whole"] else 0
                weeks[key][2] += 1 if any(r["seen"]["AGENTS.md"][how] for how in ("whole", "range", "search")) else 0
            for key in sorted(weeks):
                total, whole, any_read = weeks[key]
                shares = sorted(r["share"]["AGENTS.md"] for r in mine if r["week"][:7] + ("-a" if r["week"][8:10] < "16" else "-b") == key)
                print(
                    f"      {key}: {total} transcripts, AGENTS.md whole in {whole}, at all in {any_read}; "
                    f"share of its lines read: median {shares[len(shares) // 2]:.0%}, "
                    f"under half in {sum(1 for x in shares if x < 0.5)}, under a quarter in {sum(1 for x in shares if x < 0.25)}"
                )


if __name__ == "__main__":
    main()
