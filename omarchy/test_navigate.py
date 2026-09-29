import contextlib
import io
import json
import subprocess
import unittest
from unittest.mock import call, patch

import navigate


class NavigationTests(unittest.TestCase):
    def test_agent_navigation_crosses_workspace_and_tab_before_pane(self):
        pane = {"pane_id": "w9:p2", "workspace_id": "w9", "tab_id": "w9:t3"}
        with patch.object(navigate, "herdr", return_value={"snapshot": {"panes": [pane]}}) as cli:
            self.assertEqual(navigate.go_to_pane("w9:p2", agent=True), pane)
        self.assertEqual(cli.call_args_list, [
            call("api", "snapshot"), call("workspace", "focus", "w9"),
            call("tab", "focus", "w9:t3"), call("agent", "focus", "w9:p2"),
        ])

    def test_stale_pane_does_not_change_focus(self):
        with patch.object(navigate, "herdr", return_value={"snapshot": {"panes": []}}) as cli:
            with self.assertRaises(LookupError):
                navigate.go_to_pane("w9:p2")
        cli.assert_called_once_with("api", "snapshot")

    def test_only_local_interactive_clients_match(self):
        for args, expected in [
            (["herdr"], (True, None)),
            (["/usr/bin/herdr", "--session", "dev"], (True, "dev")),
            (["herdr", "--session=dev", "--handoff"], (True, "dev")),
            (["herdr", "session", "attach", "dev"], (True, "dev")),
            (["herdr", "server"], (False, None)),
            (["herdr", "--session", "dev", "api", "snapshot"], (False, None)),
            (["herdr", "--remote", "host"], (False, None)),
            (["herdr", "--no-session"], (False, None)),
        ]:
            with self.subTest(args=args):
                self.assertEqual(navigate.client_session(args), expected)

    def test_focus_selects_attached_session_and_falls_back_to_older_hyprland(self):
        def result(stdout="", returncode=0):
            return subprocess.CompletedProcess([], returncode, stdout, "")
        windows = [
            {"pid": 11, "mapped": True, "address": "0xabc", "focusHistoryID": 2},
            {"pid": 12, "mapped": True, "address": "0xdef", "focusHistoryID": 0},
        ]
        with patch.object(navigate, "attached_terminal_pids", return_value={11}) as pids, \
                patch.object(navigate.subprocess, "run", side_effect=[
                    result('{"session":"dev"}'), result(json.dumps(windows)),
                    result(returncode=1), result(),
                ]) as run:
            navigate.focus_terminal()
        pids.assert_called_once_with("dev")
        self.assertEqual(run.call_args_list[-1].args[0],
                         ["hyprctl", "dispatch", "focuswindow", "address:0xabc"])

    def test_open_reuses_stable_dashboard_and_ignores_previews(self):
        pane = {"pane_id": "w9:p2", "workspace_id": "w9", "tab_id": "w9:t3", "label": "Corgi"}
        preview = {"pane_id": "w8:p1", "workspace_id": "w8", "label": "Corgi"}
        agent = {"pane_id": "w9:p1", "workspace_id": "w9", "label": "Corgi"}
        snapshot = {"panes": [preview, agent, pane], "agents": [agent], "workspaces": [
            {"workspace_id": "w8", "label": "🧪 TEMP CORGI PREVIEW — test"},
            {"workspace_id": "w9", "label": "Corgi dashboard"},
        ]}
        with patch.object(navigate.sys, "argv", ["navigate.py", "open"]), \
                patch.object(navigate, "herdr", return_value={"snapshot": snapshot}) as cli, \
                patch.object(navigate, "focus_terminal") as focus, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            navigate.main()
        self.assertFalse(any(item.args[:3] == ("plugin", "pane", "open") for item in cli.call_args_list))
        self.assertEqual(json.loads(output.getvalue())["result"]["plugin_pane"]["pane"], pane)
        focus.assert_called_once()
        self.assertEqual(cli.call_args_list[-1], call("plugin", "pane", "focus", "w9:p2"))


if __name__ == "__main__":
    unittest.main()
