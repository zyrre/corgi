#!/usr/bin/env python3
"""What Corgi typed into the supervisors' boxes, and when it handed them over.

From the conversation skeleton extract_calls.py wrote (<calls>-talk.jsonl):
each `[Corgi]` prompt by kind (a wake with its report quoted, a wake that
names `report` instead, a merge line, a handover request, an answer), its
size, and for handover requests the context size the session had reached.
Then, from the calls, how often `corgi report` was run and whether the wake
before it had already carried the report.

    python3 research/analyze_wakes.py /scratch/sessions.jsonl /scratch/calls.jsonl

Prints JSON keyed by the write-up's project names. Reads only.
"""

import collections
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_written as written  # noqa: E402
from analyze_reads import dist  # noqa: E402

INLINE_REPORT_MAX = 4 * 1024
PROMPT_REPORT_BUDGET = 12 * 1024
PASTED = re.compile(r"</?pasted_content[^>]*>")


def corgi_lines(text):
    """The `[Corgi]` items of one prompt: (first line, quoted report bytes)."""
    text = PASTED.sub("", text).strip()
    if not text.startswith("[Corgi]"):
        return None
    items, current = [], None
    for line in text.splitlines():
        if line.startswith("[Corgi]"):
            current = [line, 0]
            items.append(current)
        elif line.startswith(">") and current:
            current[1] += len(line.encode()) - 1
    return items


def kind(line):
    if "fresh supervisor session takes over" in line or "handover" in line.lower() and "write" in line.lower():
        return "handover request: context" if "Your context is" in line else "handover request: idle" if "idle" in line else "handover request: other"
    if "report follows" in line:
        return "wake with report quoted"
    if re.search(r"\bRun: .*\breport\b", line):
        return "wake naming `report`"
    if "The user merged" in line or "merge" in line.lower() and "user" in line.lower():
        return "merge line"
    if "The user's answer" in line or "closed" in line and "question" in line:
        return "answer or closed question"
    if "backlog" in line.lower():
        return "backlog line"
    return "other"


def main():
    sessions = [json.loads(line) for line in open(sys.argv[1])]
    calls = [json.loads(line) for line in open(sys.argv[2])]
    talk = [json.loads(line) for line in open(sys.argv[2].replace(".jsonl", "") + "-talk.jsonl")]
    report = {}
    for directory, project in written.names().items():
        mine = sorted((t for t in talk if t["project"] == directory), key=lambda t: t["ts"] or "")
        if not mine:
            continue
        kinds = collections.Counter()
        prompt_bytes, quoted, items_per_prompt, quoted_per_prompt = [], [], [], []
        handover_context = collections.defaultdict(list)
        named_report = []
        user_prompts = 0
        for index, turn in enumerate(mine):
            if turn["role"] != "user" or turn["meta"]:
                continue
            items = corgi_lines(turn["text"])
            if items is None:
                if not turn["text"].startswith(("<", "Start your supervisor", "[Image", "Another Claude session")):
                    user_prompts += 1
                continue
            prompt_bytes.append(len(turn["text"].encode()))
            items_per_prompt.append(len(items))
            quoted_per_prompt.append(sum(size for _, size in items))
            for line, size in items:
                what = kind(line)
                kinds[what] += 1
                if what == "wake with report quoted":
                    quoted.append(size)
                if what == "wake naming `report`":
                    named_report.append((turn["ts"], turn["session"], line))
                if what.startswith("handover request"):
                    context = next((t["context"] for t in mine[index + 1:] if t["session"] == turn["session"] and t["role"] == "assistant" and t["context"]), None)
                    handover_context[what].append(context)
        own_sessions = [s for s in sessions if s["supervisor_of"] == directory]
        reports = [c for c in calls if c["project"] == directory and c["form"] == "report"]
        # A `report` run within ten minutes after a wake that named it.
        prompted = 0
        for call in reports:
            for stamp, session, line in named_report:
                if session == call["session"] and stamp <= call["ts"] and call["arg"] and call["arg"] in line:
                    prompted += 1
                    break
        report[project] = {
            "typed_user_prompts": user_prompts,
            "corgi_prompts": len(prompt_bytes),
            "corgi_items_by_kind": dict(kinds.most_common()),
            "corgi_prompt_bytes": dist(prompt_bytes),
            "items_per_prompt": dist(items_per_prompt),
            "quoted_report_bytes": dist(quoted),
            "quoted_bytes_per_prompt": dist([q for q in quoted_per_prompt if q]),
            "prompts_near_12k_budget": sum(1 for q in quoted_per_prompt if q > PROMPT_REPORT_BUDGET - INLINE_REPORT_MAX),
            "report_runs": len(reports),
            "report_runs_after_a_wake_naming_it": prompted,
            "report_result_bytes": dist([c["result_bytes"] for c in reports if c.get("parts") == 1]),
            "report_results_over_4k": sum(1 for c in reports if c.get("parts") == 1 and (c["result_bytes"] or 0) > INLINE_REPORT_MAX),
            "context_at_handover_request": {name: dist(values) for name, values in handover_context.items()},
            "context_at_handover_request_under_100k": sum(1 for values in handover_context.values() for v in values if v and v < 100_000),
            "context_at_handover_request_over_300k": sum(1 for values in handover_context.values() for v in values if v and v > 300_000),
            "session_peak_context": dist([s["max_context"] for s in own_sessions]),
            "session_start_context": dist([s["first_context"] for s in own_sessions]),
        }
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
