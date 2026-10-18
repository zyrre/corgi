#!/usr/bin/env python3
"""Exercise the actual widget and release binary in an isolated QML preview.

Requires Linux /proc, Quickshell, a graphical session, and Omarchy shell QML.
No installed files, registrations, workspaces, or live Herdr sockets are used.
Synthetic private text is generated in QML memory, sent only via stdin/socket.
"""
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/release/corgi"
OMARCHY = Path(os.environ.get("OMARCHY_PATH", "/usr/share/omarchy"))


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def descendants(pid):
    result = [pid]
    for child in Path(f"/proc/{pid}/task/{pid}/children").read_text().split():
        result += descendants(int(child))
    return result


def run():
    check(BINARY.is_file(), "run cargo build --release first")
    with tempfile.TemporaryDirectory(prefix="corgi-prompt-") as scratch:
        preview = Path(scratch)
        shutil.copytree(ROOT / "omarchy", preview / "widget")
        # Test-only handles; submission and Process lifecycle code are real.
        panel = preview / "widget/Panel.qml"
        source = panel.read_text().replace(
            "  id: root\n",
            "  id: root\n  property alias testInput: promptInput\n  property alias testProcess: promptProcess\n",
            1,
        )
        (preview / "widget/PromptPreview.qml").write_text(source)
        panel.unlink()
        (preview / "Commons").symlink_to(OMARCHY / "shell/Commons")
        (preview / "Ui").symlink_to(OMARCHY / "shell/Ui")
        # Only prompt requests reach the release binary. Other routes are a
        # synthetic read-only stream, never real session or account data.
        router = preview / "fixture-reader"
        router.write_text("#!/usr/bin/env python3\n" + r'''
import json, os, sys, time
if sys.argv[1] == "--bar-prompt":
    os.execv(BINARY, [BINARY] + sys.argv[1:])
assert sys.argv[1] == "--bar-stream"
print(json.dumps({"connected": True, "agents": [{"pane_id": "pane-synthetic", "project_group": "Synthetic preview", "task": "Private stdin verification", "state": "idle", "status": "idle", "scratch": True}]}), flush=True)
time.sleep(60)
'''.replace("BINARY", repr(str(BINARY))))
        router.chmod(0o755)
        sockpath = preview / "fake.sock"
        server = socket.socket(socket.AF_UNIX)
        server.bind(str(sockpath))
        server.listen()
        server.settimeout(15)
        qml = r'''
import QtQuick
import Quickshell
import qs.Commons
import "widget" as Widget
Window {
  id: host
  visible: true; width: 1000; height: 40
  title: "🧪 TEMP CORGI PREVIEW — stdin prompt privacy"
  property int stage: 0
  property int pauseTicks: 0
  property string original: ""
  property var modes: ["accepted", "blocked", "accepted", "disconnect", "error", "accepted"]
  function check(ok, label) { if (!ok) { console.error("FAIL: " + label); Qt.exit(1) } }
  Widget.PromptPreview {
    id: widget
    settings: ({binaryPath: ROUTER, socketPath: SOCKET, refreshIntervalSec: 3600})
  }
  Timer {
    interval: 50; running: true; repeat: true
    onTriggered: {
      if (!widget.connected) return
      if (stage === 0) {
        widget.open()
        widget.snapshot.agents[0].state = "blocked"
        widget.beginPrompt("pane-synthetic")
        host.check(widget.promptPaneId === "", "initially blocked guard")
        widget.snapshot.agents[0].state = "idle"
        widget.beginPrompt("pane-synthetic")
        widget.testInput.text = "  \n\t "
        widget.submitPrompt()
        host.check(!widget.testProcess.running, "empty guard")
        host.original = " \n --PRIVATE_WIDGET_" + Date.now() + "_" + Math.random().toString().slice(2) + "\n\"quoted\" \\ café 日本語\nlast line \n\t "
        widget.testInput.text = host.original
        widget.submitPrompt()
        widget.submitPrompt()
        host.check(widget.testProcess.running, "process starts")
        stage = 1
        return
      }
      if (stage % 2 === 1) {
        if (widget.testProcess.running) {
          host.check(widget.testInput.readOnly, "input locked while sending")
          host.check(widget.testInput.text === host.original, "retain until acceptance")
          host.check(widget.noticeText === "", "no early success notice")
          return
        }
        var mode = modes[(stage - 1) / 2]
        if (mode === "accepted") {
          host.check(widget.noticeText === "Prompt sent", "success notice")
          host.check(widget.promptPaneId === "" && widget.testInput.text === "", "success clears form in onExited")
        } else {
          host.check(widget.promptPaneId === "pane-synthetic" && widget.testInput.text === host.original, "failure preserves form")
          host.check(widget.errorText.length > 0 && widget.errorText.indexOf("PRIVATE_WIDGET_") === -1, "safe visible error")
          host.check(!widget.testInput.readOnly, "retry enabled")
          if (mode === "blocked") host.check(widget.errorText.indexOf("Blocked agents") >= 0, "state changed after selection")
        }
        console.log("PASS lifecycle: " + mode)
        pauseTicks = 0
        stage++
        return
      }
      // Leave a quiet interval to catch accidental automatic retries.
      if (++pauseTicks < 8) return
      if (stage === 12) { console.log("PASS all widget prompt scenarios"); Qt.quit(); return }
      widget.noticeText = ""
      if (!widget.promptPaneId) widget.beginPrompt("pane-synthetic")
      widget.testInput.text = host.original
      widget.submitPrompt()
      widget.submitPrompt()
      stage++
    }
  }
  Timer { interval: 25000; running: true; onTriggered: { console.error("FAIL timeout"); Qt.exit(1) } }
}
'''.replace("ROUTER", json.dumps(str(router))).replace("SOCKET", json.dumps(str(sockpath)))
        (preview / "shell.qml").write_text(qml)
        modes = ["accepted", "blocked", "accepted", "disconnect", "error", "accepted"]
        requests = []
        markers = []
        failures = []
        process = None

        def serve():
            try:
                for mode in modes:
                    stream, _ = server.accept()
                    with stream:
                        request = json.loads(stream.makefile("rb").readline())
                        text = request["params"]["text"]
                        match = re.fullmatch(r'--(PRIVATE_WIDGET_\d+_\d+)\n"quoted" \\ café 日本語\nlast line', text)
                        check(match is not None, "trimmed multiline/Unicode/quotes/backslash/hyphen text intact")
                        marker = match[1]
                        markers.append(marker)
                        check(request["method"] == "agent.prompt", "only prompt request")
                        check(request["params"] == {"target": "pane-synthetic", "text": text, "wait": None}, "correct target/text/wait")
                        check(json.dumps(request).count(marker) == 1, "one marker per request")
                        requests.append(request)
                        # Hold real Corgi in flight and inspect the whole
                        # isolated preview process tree, including the router.
                        corgi_pids = []
                        for pid in descendants(process.pid):
                            proc = Path(f"/proc/{pid}")
                            for name in ["cmdline", "environ"]:
                                check(marker.encode() not in (proc / name).read_bytes(), "marker absent from process " + name)
                            if (proc / "exe").resolve() == BINARY.resolve():
                                corgi_pids.append(pid)
                                check(descendants(pid) == [pid], "Corgi creates no child processes")
                        check(len(corgi_pids) == 1, "one real release process held in flight")
                        time.sleep(0.6)
                        if mode == "disconnect":
                            continue
                        if mode == "accepted":
                            reply = {"id": request["id"], "result": {"type": "agent_prompted", "agent": {"pane_id": "pane-synthetic", "workspace_id": "workspace-synthetic", "tab_id": "tab-synthetic"}}}
                        else:
                            reply = {"id": request["id"], "error": {"code": "agent_blocked" if mode == "blocked" else marker, "message": text}}
                        stream.sendall((json.dumps(reply) + "\n").encode())
                server.settimeout(0.6)
                try:
                    extra, _ = server.accept()
                    extra.close()
                    raise AssertionError("unexpected automatic duplicate request")
                except socket.timeout:
                    pass
            except Exception as error:
                failures.append(str(error))

        environment = dict(os.environ, HERDR_SOCKET_PATH=str(sockpath), QT_QUICK_CONTROLS_STYLE="Basic")
        process = subprocess.Popen(["quickshell", "--no-color", "--path", str(preview / "shell.qml")], env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        try:
            output = process.communicate(timeout=30)[0].decode(errors="replace")
            worker.join(timeout=2)
            check(all(marker not in output for marker in markers), "no prompt echo in Quickshell/error logs")
            check(process.returncode == 0, "Quickshell failed: " + output)
            check(not failures, "; ".join(failures))
            check(not worker.is_alive(), "fake Herdr completed")
            check("PASS all widget prompt scenarios" in output, "widget verification incomplete: " + output)
            check(len(requests) == 6, "exactly six explicitly activated submissions")
            print(output, end="")
            print("PASS privacy: preview argv/environ clean; release process has no children; exactly six controlled requests; no marker in logs")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            server.close()


if __name__ == "__main__":
    run()
