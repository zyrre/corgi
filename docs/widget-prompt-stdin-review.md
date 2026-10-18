# Widget prompt stdin review

Base: `e0ee463`, version remains 0.2.2. Prepared on
`worktree/quiet-field-d055`; no release, push, stable installation or primary
checkout changes. This records the single worker's code audit and verification
for the subsequent reviewer; it is not an independent approval.

## Contract and audit

The widget runs `[binaryPath, "--bar-prompt", socketPath, promptPaneId]`.
It captures the existing trimmed input in memory before starting the process,
then writes `JSON.stringify({text: pendingText}) + "\n"` from `onStarted`.
The command contains only routing information; no prompt environment variable,
shell, prompt file or Herdr CLI is involved. Capturing before startup also
prevents a popup dismissal from changing the queued submission.

Corgi reads one newline-terminated JSON record without waiting for EOF. JSON
escapes embedded newlines, so the entire multiline string reaches the existing
`HerdrClient::prompt_agent`, with `wait: null`, over a Unix socket. Missing,
empty, malformed, unterminated, wrongly typed and unknown-field input is
rejected. This is a one-shot protocol; later records are never submitted.

The endpoint writes nothing to stdout. Parser and socket/Herdr errors are
replaced with fixed messages, with no original error chained as a cause.
This includes error messages, error codes, mismatched response IDs and invalid
responses containing the synthetic private text. `agent_blocked` gets a fixed
instruction to use the agent's pane; other failures ask the user to check the
agent before retrying. There is exactly one socket submission and no automatic
retry, including on ambiguous disconnects. The existing socket client starts
no child processes. The shared client's behavior was not changed.

The existing UI's success/clear branch still requires a zero exit status after
Herdr's acceptance. Failure retains the original editor text and allows an
explicit retry. The existing blocked guard remains, and Herdr's rejection
handles an agent becoming blocked after selection.

## Executed checks

All final checks passed:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`: 463 library tests, 1 binary test and 2 new endpoint integration
  tests passed; 5 existing library tests ignored.
- `cargo build --release`
- `python3 scripts/test-omarchy-prompt.py`
- `node scripts/test-omarchy-model.mjs`
- `bash scripts/test-omarchy-header`: 5 Qt test cases passed.
- `bash /usr/share/omarchy/bin/omarchy-plugin-validate "$PWD/omarchy"`
- Marketplace `validateManifest(..., {community: true})` from
  `/tmp/corgi-release-0.2.2-prep/marketplace-validator/scripts/build-catalog.mjs`,
  after `npm ci --ignore-scripts --offline` in that validator checkout.
- `git diff --check`

Command logs are under `/tmp/corgi-prompt-evidence/`: `fmt.log`, `clippy.log`,
`test.log`, `build-release.log`, `widget.log`, `model.log`, `header.log`,
`omarchy-validator.log`, `marketplace-npm.log`, `marketplace-validator.log`.
The sandbox initially refused test socket binding; the successful Rust and
Quickshell tests ran with sandbox escalation to permit local IPC.

## Running preview evidence and limits

The isolated Quickshell window was labeled
`🧪 TEMP CORGI PREVIEW — stdin prompt privacy`. It loaded a fresh copy of the
actual widget, with only two test aliases added, real Omarchy popup components,
and this worktree's release binary for prompt delivery. A read-only fixture
replaced the unrelated snapshot reader. All socket traffic used a fake Herdr;
no message went to a user's real agent. No Herdr workspace/tab/pane was created,
so there is no workspace ID. The preview closed itself and its scratch copy
was removed; no preview is retained.

Six explicitly activated submissions passed: accepted, blocked rejection,
accepted retry, disconnect, general Herdr failure, accepted retry. The real
QML `Process` wrote stdin while keeping it open and ran its stderr/exit handlers.
The editor stayed locked with text intact while each submission was pending,
cleared with “Prompt sent” only after acceptance, and preserved text with retry
enabled after failures. Duplicate activation while starting produced no extra
request; quiet intervals produced no automatic retry. Empty input and agents
already known blocked produced no request.

The synthetic unique marker was generated in QML memory. While every submission
was held awaiting a controlled response, `/proc` inspection found it absent
from argv and environment throughout the isolated preview process tree,
including the running release binary. Corgi had no child processes. Each fake
request contained the marker exactly once, with the expected target and intact
trimmed multiline text, quotes, backslash, non-ASCII and leading hyphens. All
six requests were the explicit submissions; no marker appeared in preview logs
or displayed error text. Binary integration tests separately checked malformed
stdin and responses, wrong IDs, rejection, disconnection and no resend.

This validates the actual UI process lifecycle and controlled socket protocol,
not delivery to a real harness. No installed widget, PATH registration or live
settings were changed and neither Herdr nor the Omarchy shell was restarted.
The isolated scene emitted existing `Color`/palette warnings also seen in the
unchanged HeaderCard test; no visual redesign or screenshot assets were changed.
The graphical test invokes the widget's submit functions rather than synthesizing
physical keyboard input. Graphical session access and Linux `/proc` are needed
to repeat its complete privacy check.
