#!/usr/bin/env python3
"""Exercise the actual widget and release binary in an isolated QML preview.

Requires Linux /proc, Quickshell, Omarchy QML, and a separate Wayland compositor.
Set CORGI_PROMPT_TEST_DISPLAY to its absolute socket path. The test refuses the
inherited desktop display, so the real popup cannot consume desktop clicks.
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
    display = Path(os.environ.get("CORGI_PROMPT_TEST_DISPLAY", ""))
    check(display.is_absolute() and display.is_socket(), "set CORGI_PROMPT_TEST_DISPLAY to a separate compositor's absolute socket path")
    desktop = Path(os.environ.get("WAYLAND_DISPLAY", ""))
    if not desktop.is_absolute():
        desktop = Path(os.environ.get("XDG_RUNTIME_DIR", "/")) / desktop
    check(display.resolve() != desktop.resolve(), "the prompt test requires a separate display, not the desktop")
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
  property int scenario: -1
  property string phase: "loading"
  property bool started: false
  property bool dismissed: false
  property int heldSamples: 0
  property string original: ""
  property var modes: ["accepted", "blocked", "accepted", "disconnect", "error", "accepted", "dismiss-starting", "dismiss-inflight"]
  function check(ok, label) { if (!ok) { console.error("FAIL: " + label); Qt.exit(1); throw new Error(label) } }
  function trace(label) {
    console.log("TRACE " + Date.now() + " " + label + " scenario=" + scenario + " phase=" + phase
      + " running=" + widget.testProcess.running + " readOnly=" + widget.testInput.readOnly
      + " opened=" + widget.opened + " focus=" + widget.testInput.activeFocus)
  }
  function checkHeld() {
    check(started && widget.testProcess.running, "real process held in flight")
    check(widget.testInput.readOnly, "input locked after started and while sending")
    check(widget.noticeText === "", "no early success notice")
    if (dismissed) {
      check(!widget.opened && widget.promptPaneId === "" && widget.testInput.text === "", "dismissal clears form while delivery continues")
    } else {
      check(widget.opened, "popup must remain open during retention scenario")
      check(widget.promptPaneId === "pane-synthetic" && widget.testInput.text === original, "retain original until acceptance while open")
    }
  }
  function dismiss() {
    dismissed = true
    widget.close()
    check(!widget.opened && widget.promptPaneId === "" && widget.testInput.text === "", "real close handler clears dismissed form")
    check(widget.testProcess.running, "dismissal does not cancel captured delivery")
    trace("controlled-dismissal")
  }
  function activate() {
    if (++scenario === modes.length) {
      phase = "done"
      console.log("PASS all widget prompt scenarios")
      Qt.quit()
      return
    }
    phase = "preparing"
    started = false
    dismissed = false
    heldSamples = 0
    widget.open()
    if (scenario === 0) {
      widget.snapshot.agents[0].state = "blocked"
      widget.beginPrompt("pane-synthetic")
      check(widget.promptPaneId === "", "initially blocked guard")
      widget.snapshot.agents[0].state = "idle"
      widget.beginPrompt("pane-synthetic")
      widget.testInput.text = "  \n\t "
      widget.submitPrompt()
      check(!widget.testProcess.running, "empty guard")
    } else if (!widget.promptPaneId) widget.beginPrompt("pane-synthetic")
    if (scenario === 2 || scenario === 5) {
      check(widget.testInput.text === original && widget.promptPaneId === "pane-synthetic", "explicit retry uses retained original without editing")
    } else {
      original = " \n --PRIVATE_WIDGET_" + Date.now() + "_" + Math.random().toString().slice(2) + "_" + scenario + "\n\"quoted\" \\ café 日本語\nlast line \n\t "
      widget.testInput.text = original
    }
    widget.noticeText = ""
    phase = "starting"
    widget.submitPrompt()
    widget.submitPrompt()
    check(widget.testProcess.running, "process requested once; duplicate activation guarded")
    trace("submitted")
    if (modes[scenario] === "dismiss-starting") dismiss()
  }
  function finish(exitCode) {
    check(phase === "exiting" && !widget.testProcess.running, "exit handlers completed")
    check(heldSamples >= 3, "in-flight invariants checked repeatedly before reply")
    check(!widget.testInput.readOnly, "editor unlocks after exit")
    var mode = modes[scenario]
    var accepted = mode === "accepted" || mode.indexOf("dismiss-") === 0
    check((exitCode === 0) === accepted, "correct exit status")
    if (accepted) {
      check(widget.noticeText === "Prompt sent", "success notice")
      check(widget.promptPaneId === "" && widget.testInput.text === "", "success clears form in onExited")
    } else {
      check(widget.opened && widget.promptPaneId === "pane-synthetic" && widget.testInput.text === original, "failure preserves form")
      check(widget.errorText.length > 0 && widget.errorText.indexOf("PRIVATE_WIDGET_") === -1, "safe visible error")
      if (mode === "blocked") check(widget.errorText.indexOf("Blocked agents") >= 0, "state changed after selection")
    }
    console.log("PASS lifecycle: " + mode)
    phase = "quiet"
    quiet.restart()
  }
  Widget.PromptPreview {
    id: widget
    settings: ({binaryPath: ROUTER, socketPath: SOCKET, refreshIntervalSec: 3600})
  }
  Connections {
    target: widget
    function onConnectedChanged() {
      if (widget.connected && host.phase === "loading") Qt.callLater(host.activate)
    }
    function onOpenedChanged() {
      host.trace("openedChanged")
      if (!widget.opened && host.phase !== "done") host.check(host.dismissed, "unexpected external popup dismissal")
    }
  }
  Connections {
    target: widget.testInput
    function onReadOnlyChanged() {
      host.trace("readOnlyChanged")
      if (host.started && widget.testProcess.running)
        host.check(widget.testInput.readOnly, "editor stays locked throughout submission")
    }
    function onActiveFocusChanged() { host.trace("focusChanged") }
    function onTextChanged() {
      if (host.started && widget.testProcess.running && !host.dismissed)
        host.check(widget.testInput.text === host.original, "text retained throughout in-flight submission")
    }
  }
  Connections {
    target: widget.testProcess
    function onRunningChanged() { host.trace("runningChanged") }
    function onStarted() {
      host.trace("started")
      host.check(host.phase === "starting" && !host.started, "one started event per activation")
      host.started = true
      // running's getter changes before its notify signal during startup.
      // Check the real binding after started, never poll the setter's value.
      Qt.callLater(function() {
        host.phase = "sending"
        host.checkHeld()
        held.start()
      })
    }
    function onExited(exitCode) {
      host.trace("exited=" + exitCode)
      host.check(host.phase === "sending" && host.started, "exit follows real startup")
      host.phase = "exiting"
      held.stop()
      // The widget's onExited and the readOnly binding must finish first.
      Qt.callLater(function() { host.finish(exitCode) })
    }
  }
  Timer {
    id: held
    interval: 25; repeat: true
    onTriggered: {
      host.checkHeld()
      if (++host.heldSamples === 3) {
        if (host.modes[host.scenario] === "dismiss-inflight") { host.dismiss(); host.checkHeld() }
        // The fake server cannot reply until these assertions have passed.
        console.log("HELD " + host.scenario)
      }
    }
  }
  // A quiet observation interval is intentional: any automatic retry still
  // causes an unexpected started event outside the explicit starting phase.
  Timer { id: quiet; interval: 500; onTriggered: host.activate() }
  Timer { interval: 30000; running: true; onTriggered: { console.error("FAIL timeout"); Qt.exit(1) } }
}
'''.replace("ROUTER", json.dumps(str(router))).replace("SOCKET", json.dumps(str(sockpath)))
        (preview / "shell.qml").write_text(qml)
        modes = ["accepted", "blocked", "accepted", "disconnect", "error", "accepted", "dismiss-starting", "dismiss-inflight"]
        held = [threading.Event() for _ in modes]
        output_lines = []
        requests = []
        markers = []
        failures = []
        process = None

        def collect_output():
            for line in process.stdout:
                output_lines.append(line)
                match = re.search(rb"\bHELD (\d+)$", line.strip())
                if match and int(match[1]) < len(held):
                    held[int(match[1])].set()

        def serve():
            try:
                for scenario, mode in enumerate(modes):
                    stream, _ = server.accept()
                    with stream:
                        stream.settimeout(5)
                        request = json.loads(stream.makefile("rb").readline())
                        text = request["params"]["text"]
                        match = re.fullmatch(r'--(PRIVATE_WIDGET_\d+_\d+_(\d+))\n"quoted" \\ café 日本語\nlast line', text)
                        check(match is not None, "trimmed multiline/Unicode/quotes/backslash/hyphen text intact")
                        check(int(match[2]) == [0, 1, 1, 3, 4, 4, 6, 7][scenario], "text belongs to this explicit activation")
                        if scenario in [2, 5]:
                            check(text == requests[-1]["params"]["text"], "explicit retry sends identical retained text")
                        marker = match[1]
                        markers.append(marker)
                        check(request["method"] == "agent.prompt", "only prompt request")
                        check(request["params"] == {"target": "pane-synthetic", "text": text, "wait": None}, "correct target/text/wait")
                        check(json.dumps(request).count(marker) == 1, "one marker per request")
                        requests.append(request)
                        check(held[scenario].wait(timeout=5), "QML completed real startup and repeated in-flight assertions")
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
                        if mode == "disconnect":
                            continue
                        if mode in ["accepted", "dismiss-starting", "dismiss-inflight"]:
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

        runtime = preview / "runtime"
        runtime.mkdir(mode=0o700)
        environment = dict(os.environ, HERDR_SOCKET_PATH=str(sockpath), QT_QUICK_CONTROLS_STYLE="Basic", QT_QPA_PLATFORM="wayland", WAYLAND_DISPLAY=str(display), XDG_RUNTIME_DIR=str(runtime))
        for name in ["HYPRLAND_INSTANCE_SIGNATURE", "DBUS_SESSION_BUS_ADDRESS", "DISPLAY"]:
            environment.pop(name, None)
        process = subprocess.Popen(["quickshell", "--no-color", "--path", str(preview / "shell.qml")], env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        reader = threading.Thread(target=collect_output, daemon=True)
        reader.start()
        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        try:
            process.wait(timeout=35)
            reader.join(timeout=2)
            check(not reader.is_alive(), "Quickshell output completed")
            output = b"".join(output_lines).decode(errors="replace")
            worker.join(timeout=2)
            check(all(marker not in output for marker in markers), "no prompt echo in Quickshell/error logs")
            check(process.returncode == 0, "Quickshell failed: " + output)
            check(not failures, "; ".join(failures))
            check(not worker.is_alive(), "fake Herdr completed")
            check("PASS all widget prompt scenarios" in output, "widget verification incomplete: " + output)
            check(len(requests) == len(modes) and len(set(markers)) == 6, "exactly eight explicit submissions, including two unchanged retries")
            print(output, end="")
            print("PASS privacy: preview argv/environ clean; release process has no children; exactly eight controlled requests, including two dismissals; no marker in logs")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            server.close()


if __name__ == "__main__":
    run()
