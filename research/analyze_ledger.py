#!/usr/bin/env python3
"""Does each ledger id's newest status match what Git says happened?

Reads every supervisor state directory's ledger and its archive for how
statuses moved (how long work stays `reported`, how old open work is). For
the one project whose repository is given (run it from a checkout of that
repository; other projects' checkouts are left alone), it also asks Git,
with read-only commands:

- an id marked `merged`: is its branch named by a merge commit on the base
  branch, or its tip an ancestor of the base branch, or does its outcome name
  a commit that is on the base branch?
- an id not marked `merged`: is its branch merged all the same?
- open ids: how old is the newest line, and does the worktree still exist?

    python3 research/analyze_ledger.py corgi . main [--list]

(the state directory's name, a checkout of its repository, its base branch).
Prints JSON keyed by the write-up's project names; `--list` adds the named
project's mismatching ids. Reads only.
"""

import datetime
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402


def git(repo, *args):
    done = subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)
    return done.stdout if done.returncode == 0 else None


def main():
    checked, checkout, base_branch = sys.argv[1], sys.argv[2], sys.argv[3]
    listed = "--list" in sys.argv
    now = datetime.datetime.now(datetime.timezone.utc).timestamp()
    report = {}
    for directory, project in written.names().items():
        repo = checkout if directory == checked else None
        base = base_branch
        if repo is None:
            # No Git for this project: every check below then finds no evidence.
            subjects = ""
            merged_names, on_base, branches, worktrees = set(), set(), set(), None
        else:
            subjects = git(repo, "log", base, "--merges", "--format=%s") or ""
            merged_names = set(re.findall(r"Merge (?:remote-tracking )?branch '([^']+)'", subjects))
            on_base = set((git(repo, "log", base, "--format=%h", "--abbrev=7") or "").split())
            branches = set((git(repo, "for-each-ref", "--format=%(refname:short)", "refs/heads") or "").split())
            worktrees = set(re.findall(r"^worktree (.+)$", git(repo, "worktree", "list", "--porcelain") or "", re.M))

        path = os.path.join(written.ROOT, directory)
        live = written.ledger_lines(written.read(os.path.join(path, "ledger.jsonl")))
        archive = written.ledger_lines(written.read(os.path.join(path, "ledger-archive.jsonl")))
        history = {}
        for line in archive + live:
            if not line.get("_bad"):
                history.setdefault(line["id"], []).append(line)
        ids = written.newest(live)

        def branch_of(entry):
            return next((l.get("branch") for l in reversed(history[entry["id"]]) if l.get("branch")), "") or ""

        def merged_in_git(entry):
            branch = branch_of(entry)
            if branch and branch in merged_names:
                return "merge commit names the branch"
            for commit in re.findall(r"\b[0-9a-f]{7,10}\b", str(entry.get("outcome") or "")):
                if commit[:7] in on_base:
                    return "outcome names a commit on the base branch"
            if branch in branches:
                ahead = git(repo, "rev-list", "--count", f"{base}..{branch}")
                tip_time = git(repo, "log", "-1", "--format=%ct", branch)
                first = written.seconds(history[entry["id"]][0].get("ts"))
                if ahead is not None and ahead.strip() == "0" and tip_time and first and int(tip_time) > first:
                    return "branch tip is on the base branch"
            return None

        counts, mismatches = {}, []
        bump = lambda key: counts.__setitem__(key, counts.get(key, 0) + 1)
        for entry in ids:
            status = entry.get("status") or "-"
            evidence = merged_in_git(entry)
            age_days = (now - written.seconds(entry.get("ts"))) / 86400 if written.seconds(entry.get("ts")) else None
            if repo is None:
                bump(f"marked {status}")
                if status not in ("merged", "done", "abandoned") and age_days is not None and age_days >= 2:
                    bump(f"open and 2+ days old ({status})")
                continue
            if status == "merged":
                bump("marked merged")
                if evidence:
                    bump("marked merged, Git agrees")
                else:
                    bump("marked merged, no trace in Git (may be a squash, a remote merge, or work without a branch)")
                    mismatches.append((entry["id"], status, branch_of(entry), "no merge found"))
            else:
                bump(f"marked {status}")
                if evidence:
                    bump(f"marked {status}, but merged in Git")
                    mismatches.append((entry["id"], status, branch_of(entry), evidence))
                elif status not in ("done", "abandoned"):
                    bump("open, not merged in Git")
                    cwd = next((l.get("cwd") for l in reversed(history[entry["id"]]) if l.get("cwd")), None)
                    gone = bool(cwd) and worktrees is not None and cwd not in worktrees
                    if age_days is not None and age_days >= 2:
                        bump(f"open and 2+ days old ({status})")
                        if gone:
                            bump(f"open, 2+ days old, worktree gone ({status})")
                        mismatches.append((entry["id"], status, branch_of(entry), f"open {int(age_days)} days" + (", worktree gone" if gone else "")))
        # How statuses moved: every id's sequence of statuses.
        left_reported = sum(1 for lines in history.values() if lines[-1].get("status") == "reported")
        ever_reported = sum(1 for lines in history.values() if any(l.get("status") == "reported" for l in lines))
        reported_days = []
        for lines in history.values():
            for index, line in enumerate(lines[:-1]):
                if line.get("status") == "reported" and lines[index + 1].get("status") != "reported":
                    a, b = written.seconds(line.get("ts")), written.seconds(lines[index + 1].get("ts"))
                    if a and b:
                        reported_days.append((b - a) / 86400)
        counts["ids"] = len(ids)
        counts["ids ever `reported`"] = ever_reported
        counts["ids `reported` now"] = left_reported
        if reported_days:
            ordered = sorted(reported_days)
            counts["days spent `reported` before the next status: median"] = round(ordered[len(ordered) // 2], 2)
            counts["days spent `reported`: p90"] = round(ordered[int(len(ordered) * 0.9)], 2)
            counts["`reported` for 2+ days before moving on"] = sum(1 for d in ordered if d >= 2)
        report[project] = dict(sorted(counts.items()))
        if listed and directory == checked:
            report[project]["mismatches"] = mismatches
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
