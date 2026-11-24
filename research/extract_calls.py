#!/usr/bin/env python3
"""Pull every memory-related tool call out of the supervisor transcripts.

Reads the index written by index_sessions.py and writes, per call, one JSON
line: which project and session, when, which command form (plain digest,
--decision, --search, --show, --ledger, --brief, report, inbox, fleet, spawn,
tag, a raw read or write of a state file, a file-tool read or write), which
artifact kind it touched, its argument and the size of what came back. The
full text that came back is kept beside it for the digest forms and raw reads
of decisions, so later scripts can see what a call showed.

Beside it go `<out>-talk.jsonl`, every prompt and reply of the supervisor
sessions without the tool traffic (what the user, Corgi and the supervisor
said), and `<out>-agent-memory.jsonl` (below).

Also writes, for every transcript (supervisor or not), each read or write of
a Claude Code agent memory folder (~/.claude/projects/<slug>/memory/).

    python3 research/extract_calls.py /scratch/sessions.jsonl /scratch/calls.jsonl

Reads only. The output names projects and quotes their content: keep it out
of the repository.
"""

import json
import re
import sys

STATE = re.compile(
    r"(?:~|\$HOME|\$\{HOME\}|/Users/[^/\s'\"]+)/\.local/state/corgi/supervisors/([\w.-]+)(/[^\s'\"`;|&)<>]*)?"
)
MEMORY = re.compile(
    r"(?:~|\$HOME|/Users/[^/\s'\"]+)/\.claude/projects/([^/\s'\"]+)/memory(/[^\s'\"`;|&)<>]*)?"
)
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1[^\n]*\n.*?\n[ \t]*\2[ \t]*(?=\n|$)", re.S)
ASSIGN = re.compile(r"(?:^|[\s;&(])(?:export\s+)?([A-Za-z_]\w*)=(['\"]?)([^\s'\";&|]+)\2")
CORGI = re.compile(
    r"(?:^|[\s;&|(`])(?:[\w./~${}-]*/)?corgi\s+"
    r"(digest|report|inbox|fleet|spawn|tag|role|status|notify|merge|supervisor|usage)\b"
)
LOOKUPS = ("--decision", "--search", "--show", "--ledger", "--brief")
READERS = {"cat", "head", "tail", "grep", "rg", "egrep", "sed", "awk", "jq", "wc", "less", "bat", "python3", "python", "cut", "sort", "tac", "nl", "diff"}
LISTERS = {"ls", "find", "stat", "test", "[", "[[", "du", "tree"}
MANAGERS = {"mv", "rm", "mkdir", "cp", "touch", "tee", "chmod", "rmdir"}


def artifact(rest):
    """The artifact kind a path under a state directory names."""
    rest = (rest or "").strip("/")
    if not rest:
        return "state-dir"
    head = rest.split("/")[0]
    names = {
        "decisions.md": "decisions",
        "ledger.jsonl": "ledger",
        "ledger-archive.jsonl": "ledger-archive",
        "briefs": "briefs",
        "reports": "reports",
        "inbox.jsonl": "inbox",
        "inbox-reports": "inbox-reports",
        "handover.md": "handover",
        "handovers": "handovers",
        "notes": "notes",
        "questions": "questions",
        "backlog.md": "backlog",
        "answers.jsonl": "answers",
        "ROLE.md": "role",
    }
    if head in names:
        return names[head]
    if head.startswith(("ledger", "decisions", "handover", "inbox", "brief", "report", "question", "backlog", "note")):
        # A glob or brace such as ledger*.jsonl or {ledger,ledger-archive}.jsonl.
        for prefix, kind in (("ledger", "ledger"), ("decisions", "decisions"), ("handovers", "handovers"),
                             ("handover", "handover"), ("inbox-r", "inbox-reports"), ("inbox", "inbox"),
                             ("brief", "briefs"), ("report", "reports"), ("question", "questions"),
                             ("backlog", "backlog"), ("note", "notes")):
            if head.startswith(prefix):
                return kind
    return "other-state"


def strip_heredocs(command):
    return HEREDOC.sub("<<HEREDOC", command)


def substitute(command):
    """Replaces $VAR and ${VAR} by the value a VAR=value earlier in the command gave it."""
    values = {}
    for found in ASSIGN.finditer(command):
        name, value = found.group(1), found.group(3)
        for known, text in values.items():
            value = value.replace("${" + known + "}", text).replace("$" + known, text)
        values[name] = value
    for name in sorted(values, key=len, reverse=True):
        command = command.replace("${" + name + "}", values[name])
        command = re.sub(r"\$" + re.escape(name) + r"\b", values[name].replace("\\", "\\\\"), command)
    return command


def segments(command):
    """The simple commands of a line, split at ; && || | and newlines outside quotes."""
    parts, current, quote = [], [], None
    index = 0
    while index < len(command):
        char = command[index]
        if quote:
            current.append(char)
            if char == "\\" and quote == '"' and index + 1 < len(command):
                index += 1
                current.append(command[index])
            elif char == quote:
                quote = None
        elif char in "'\"":
            quote = char
            current.append(char)
        elif char in ";\n|&":
            if char == "&" and index + 1 < len(command) and command[index + 1] == ">":
                current.append(char)
            elif char == "&" and index > 0 and command[index - 1] in "<>":
                current.append(char)
            else:
                parts.append("".join(current))
                current = []
        else:
            current.append(char)
        index += 1
    parts.append("".join(current))
    return [part.strip() for part in parts if part.strip()]


def first_word(segment):
    words = segment.split()
    while words and (re.fullmatch(r"[A-Za-z_]\w*=\S*", words[0]) or words[0] in ("then", "do", "else", "if", "while", "time", "!", "(", "{", "for", "command", "sudo", "exec")):
        words = words[1:]
    return words[0].split("/")[-1] if words else ""


def quoted_value(segment, flag):
    found = re.search(re.escape(flag) + r"\s+(?:\"((?:[^\"\\]|\\.)*)\"|'([^']*)'|(\S+))", segment)
    if not found:
        return ""
    return next(group for group in found.groups() if group is not None)


def bash_calls(command):
    """(form, artifact, argument, state project) for each memory-related part of a Bash command."""
    calls = []
    command = substitute(strip_heredocs(command))
    for segment in segments(command):
        found = CORGI.search(" " + segment)
        if found:
            sub = found.group(1)
            rest = segment[segment.find(sub, found.start(1) - 1):]
            if sub == "digest":
                form = next((flag for flag in LOOKUPS if re.search(r"(?:^|\s)" + flag + r"\b", rest)), None)
                if form:
                    calls.append(("digest" + form, "memory", quoted_value(rest, form), None))
                elif "--help" in rest:
                    calls.append(("help", "-", "digest", None))
                else:
                    calls.append(("digest", "memory", "", None))
            elif sub == "inbox":
                form = "inbox-take" if "--take" in rest else "inbox-delivered" if "--delivered" in rest else "inbox"
                calls.append((form, "inbox", "", None))
            elif sub == "report":
                words = rest.split()
                calls.append(("report", "reports", words[1] if len(words) > 1 else "", None))
            elif sub == "role":
                calls.append(("role-handover", "role", "", None))
            else:
                words = rest.split()
                calls.append((sub, "-", words[1] if len(words) > 1 and sub == "tag" else "", None))
            continue
        word = first_word(segment)
        for path in STATE.finditer(segment):
            kind = artifact(path.group(2))
            before = segment[: path.start()].rstrip()
            if re.search(r"(?:^|\s)[A-Za-z_]\w*=['\"]?$", before) or word == "cd":
                # NAME=<path> only names the path; its uses are counted where
                # the variable was substituted.
                continue
            if before.endswith(">") or before.endswith(">>") or re.search(r">>?\s*['\"]?$", before):
                form = "raw-write"
            elif word in MANAGERS:
                form = "raw-manage"
            elif word in LISTERS:
                form = "raw-list"
            elif word in READERS or word in ("for", "while", "echo", "printf", "xargs", "open"):
                form = "raw-read" if word in READERS else "raw-other"
            else:
                form = "raw-other"
            calls.append((form, kind, (path.group(2) or "")[:200], path.group(1)))
    return calls


def memory_calls(command):
    calls = []
    command = substitute(strip_heredocs(command))
    for segment in segments(command):
        word = first_word(segment)
        for path in MEMORY.finditer(segment):
            before = segment[: path.start()].rstrip()
            if re.search(r">>?\s*['\"]?$", before):
                form = "raw-write"
            elif word in MANAGERS:
                form = "raw-manage"
            elif word in LISTERS:
                form = "raw-list"
            else:
                form = "raw-read"
            calls.append((form, path.group(1), (path.group(2) or "")[:200]))
    return calls


def result_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict) and part.get("type") == "text")
    return ""


def main():
    index_path, out_path = sys.argv[1], sys.argv[2]
    sessions = [json.loads(line) for line in open(index_path)]
    out = open(out_path, "w")
    memory_out = open(out_path.replace(".jsonl", "") + "-agent-memory.jsonl", "w")
    talk_out = open(out_path.replace(".jsonl", "") + "-talk.jsonl", "w")
    for session in sessions:
        project = session["supervisor_of"]
        pending = {}
        with open(session["file"], errors="replace") as handle:
            for line in handle:
                if not project and '"tool_use"' not in line and '"tool_result"' not in line:
                    continue
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if record.get("isSidechain") or record.get("type") not in ("user", "assistant"):
                    # Other record kinds (api-request and the like) repeat the
                    # same tool calls without a time: counting them doubles.
                    continue
                message = record.get("message") or {}
                content = message.get("content")
                if project:
                    # The conversation without tool traffic: prompts (the user's
                    # and Corgi's) and what the supervisor said.
                    said = result_text(content)
                    if said.strip():
                        usage = message.get("usage") or {}
                        talk_out.write(json.dumps({
                            "project": project,
                            "session": session["file"].split("/")[-1][:8],
                            "ts": record.get("timestamp"),
                            "role": record["type"],
                            "meta": bool(record.get("isMeta") or record.get("isCompactSummary")),
                            "context": usage.get("input_tokens", 0) + usage.get("cache_creation_input_tokens", 0) + usage.get("cache_read_input_tokens", 0),
                            "text": said,
                        }) + "\n")
                if not isinstance(content, list):
                    continue
                for part in content:
                    if not isinstance(part, dict):
                        continue
                    if part.get("type") == "tool_use":
                        name, args = part.get("name"), part.get("input") or {}
                        rows, memory_rows = [], []
                        if name == "Bash":
                            command = args.get("command", "")
                            if project:
                                rows = [
                                    {"form": form, "artifact": kind, "arg": arg, "state_project": state}
                                    for form, kind, arg, state in bash_calls(command)
                                ]
                            memory_rows = [
                                {"form": form, "memory_slug": slug, "arg": arg}
                                for form, slug, arg in memory_calls(command)
                            ]
                        elif name in ("Read", "Grep", "Glob", "Write", "Edit", "NotebookEdit"):
                            path = args.get("file_path") or args.get("path") or ""
                            form = {"Read": "tool-read", "Grep": "tool-grep", "Glob": "tool-glob"}.get(name, "tool-write")
                            found = STATE.search(path)
                            if found and project:
                                arg = found.group(2) or ""
                                if name == "Read" and (args.get("offset") or args.get("limit")):
                                    arg += f"#{args.get('offset') or 1}+{args.get('limit') or ''}"
                                if name == "Grep":
                                    arg += " ~ " + str(args.get("pattern", ""))[:120]
                                rows = [{"form": form, "artifact": artifact(found.group(2)), "arg": arg, "state_project": found.group(1)}]
                            found = MEMORY.search(path)
                            if found:
                                memory_rows = [{"form": form, "memory_slug": found.group(1), "arg": found.group(2) or ""}]
                        for row in rows + memory_rows:
                            row.update(
                                session=session["file"].split("/")[-1][:8],
                                slug=session["slug"],
                                project=project,
                                ts=record.get("timestamp"),
                                tool=name,
                                id=part.get("id"),
                                command=(args.get("command") or "")[:400] if name == "Bash" else "",
                                parts=len(rows) + len(memory_rows),
                            )
                            if row["form"] in ("raw-write", "tool-write", "raw-other") and row.get("artifact") != "briefs":
                                # What was written, to date each entry by the call that wrote it.
                                row["payload"] = (
                                    args.get("command") or args.get("content") or args.get("new_string") or ""
                                )[:20000]
                        if rows or memory_rows:
                            pending[part.get("id")] = (rows, memory_rows)
                    elif part.get("type") == "tool_result" and part.get("tool_use_id") in pending:
                        rows, memory_rows = pending.pop(part["tool_use_id"])
                        text = result_text(part.get("content"))
                        for row in rows:
                            row["result_bytes"] = len(text.encode())
                            row["error"] = bool(part.get("is_error"))
                            keep = row["form"].startswith("digest") or row["artifact"] in ("decisions", "handover", "handovers", "notes", "questions", "backlog", "reports", "inbox-reports", "ledger", "ledger-archive", "briefs") or row["form"].startswith("inbox")
                            row["result"] = text if keep else text[:300]
                            out.write(json.dumps(row) + "\n")
                        for row in memory_rows:
                            row["result_bytes"] = len(text.encode())
                            memory_out.write(json.dumps(row) + "\n")
        for rows, memory_rows in pending.values():
            for row in rows:
                row["result_bytes"] = None
                out.write(json.dumps(row) + "\n")
            for row in memory_rows:
                memory_out.write(json.dumps(row) + "\n")
    out.close()
    memory_out.close()


if __name__ == "__main__":
    main()
