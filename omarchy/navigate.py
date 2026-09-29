#!/usr/bin/env python3
"""Show a Herdr pane and bring the terminal attached to its session into view."""

import json
from pathlib import Path
import subprocess
import sys


def herdr(*args):
    result = subprocess.run(
        ["herdr", *args], check=True, capture_output=True, text=True
    )
    return json.loads(result.stdout)["result"]


def go_to_pane(pane_id, agent=False):
    snapshot = herdr("api", "snapshot")["snapshot"]
    pane = next((pane for pane in snapshot["panes"] if pane["pane_id"] == pane_id), None)
    if pane is None:
        raise LookupError("That pane is no longer in the Herdr session")
    herdr("workspace", "focus", pane["workspace_id"])
    herdr("tab", "focus", pane["tab_id"])
    if agent:
        herdr("agent", "focus", pane_id)
    else:
        herdr("plugin", "pane", "focus", pane_id)
    return pane


def client_session(args):
    """Return the session selected by a local interactive Herdr client."""
    if not args or Path(args[0]).name != "herdr":
        return False, None
    tail = args[1:]
    if tail[:2] == ["session", "attach"] and len(tail) == 3:
        return True, tail[2]
    session = None
    index = 0
    while index < len(tail):
        arg = tail[index]
        if arg == "--session" and index + 1 < len(tail):
            index += 1
            session = tail[index]
        elif arg.startswith("--session="):
            session = arg.split("=", 1)[1]
        elif arg not in ("--handoff",):
            # Exclude servers, CLI commands, and remote/monolithic clients.
            return False, None
        index += 1
    return True, session


def attached_terminal_pids(session):
    terminals = set()
    for process in Path("/proc").iterdir():
        if not process.name.isdigit():
            continue
        try:
            args = process.joinpath("cmdline").read_bytes().rstrip(b"\0").decode().split("\0")
            is_client, selected_session = client_session(args)
            if not is_client or selected_session != session:
                continue
            # Walk from Herdr through its shell to the compositor's terminal PID.
            pid = int(process.name)
            while pid > 1 and pid not in terminals:
                terminals.add(pid)
                stat = Path(f"/proc/{pid}/stat").read_text()
                pid = int(stat.rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, UnicodeError):
            continue
    return terminals


def focus_terminal():
    status = subprocess.run(
        ["herdr", "status", "server", "--json"],
        check=True, capture_output=True, text=True,
    )
    session = json.loads(status.stdout).get("session")
    pids = attached_terminal_pids(session)
    clients = subprocess.run(
        ["hyprctl", "clients", "-j"], check=True, capture_output=True, text=True
    )
    windows = [window for window in json.loads(clients.stdout)
               if window.get("pid") in pids and window.get("mapped")]
    if not windows:
        raise RuntimeError("Herdr selected the pane, but no attached terminal window was found")
    window = min(windows, key=lambda item: item.get("focusHistoryID", sys.maxsize))
    address = window["address"]
    if not address.startswith("0x") or any(char not in "0123456789abcdefABCDEF" for char in address[2:]):
        raise RuntimeError("Hyprland returned an invalid window address")
    # Omarchy's Lua Hyprland and the older dispatcher use different syntax.
    result = subprocess.run(
        ["hyprctl", "dispatch", f'hl.dsp.focus({{ window = "address:{address}" }})'],
        capture_output=True, text=True,
    )
    if result.returncode:
        subprocess.run(
            ["hyprctl", "dispatch", "focuswindow", f"address:{address}"],
            check=True, capture_output=True, text=True,
        )


def main():
    action = sys.argv[1]
    if action == "open":
        snapshot = herdr("api", "snapshot")["snapshot"]
        workspaces = {workspace["workspace_id"] for workspace in snapshot["workspaces"]
                      if workspace.get("label") == "Corgi dashboard"}
        agents = {agent["pane_id"] for agent in snapshot["agents"]}
        pane_id = next((pane["pane_id"] for pane in snapshot["panes"]
                        if pane.get("label") == "Corgi" and pane["workspace_id"] in workspaces
                        and pane["pane_id"] not in agents), None)
        if pane_id is None:
            opened = herdr("plugin", "pane", "open", "--plugin", "io.github.zyrre.corgi",
                           "--entrypoint", "dashboard", "--placement", "tab", "--no-focus")
            pane_id = opened["plugin_pane"]["pane"]["pane_id"]
    else:
        pane_id = sys.argv[2]
    pane = go_to_pane(pane_id, agent=action == "agent")
    # Preserve the pane ID even if desktop focus fails, so retrying reuses it.
    print(json.dumps({"result": {"plugin_pane": {"pane": pane}}}), flush=True)
    focus_terminal()


if __name__ == "__main__":
    try:
        main()
    except LookupError as error:
        print(str(error), file=sys.stderr)
        sys.exit(3)
    except (subprocess.CalledProcessError, RuntimeError, KeyError, ValueError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) and error.stderr else str(error)
        print(detail, file=sys.stderr)
        sys.exit(1)
