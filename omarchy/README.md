# Corgi for the Omarchy bar

A corgi icon opens a compact dashboard with projects, agent state, model and
context usage, cache estimates, latest messages and commands, and plan usage.
Colors follow the active Omarchy shell theme.

The bar keeps the full mascot visible, with a small underline for running
agents (accent), agents needing attention (urgent), or completed agents (green).
Attention takes priority; idle agents have no underline. Hover for agent counts.

Requires Omarchy's Quickshell-based shell, Herdr, Python 3, and a Corgi binary built from
this repository that supports `--bar-stream`. No separate backend service is
installed: the widget owns a reader process while it is loaded.

## Local development

Build with `cargo build --release`. Copy or link this directory to
`~/.config/omarchy/plugins/io.github.zyrre.corgi`, then run:

```sh
omarchy plugin validate ~/.config/omarchy/plugins/io.github.zyrre.corgi
omarchy-shell shell rescanPlugins
omarchy plugin enable io.github.zyrre.corgi --section center
```

Copy `target/release/corgi` to the installed plugin directory as `corgi-actions`
after building, alongside the QML files.

Set `binaryPath` in the widget settings to the absolute release binary path.
Set `socketPath` to the desired Herdr session socket; by default the widget
uses `~/.config/herdr/herdr.sock`. Named sessions have their own socket.
Avoid leaving permanent links to disposable worktrees.

## Controls

- Left click the icon to toggle the popup; right click to reconnect the reader.
- Each session uses three lines: status and task with model/context/cache/checkout,
  latest message, and current or latest command/tool. Long text is clipped.
- Left click an agent row or press Space to expand/collapse its newest-first
  transcript. The newest entry and latest reply retain their full text and
  paragraphs; older entries use two lines, like the dashboard. The popup grows
  to fit the latest reply, capped by the screen height, with scrolling for overflow.
- Right click an agent row to show its workspace, tab, and pane, then switch
  to the desktop containing the Herdr terminal.
- Click a session’s **Prompt**, **Merge**, or **X** button to act directly on that
  session. Close and Merge retain their confirmation dialogs. Prompt is disabled
  for blocked agents.
- Press `p` to prompt the expanded or hovered session (the first session when
  none is selected). Enter sends, Shift+Enter inserts a newline, and Escape
  or clicking outside cancels. Failed sends preserve your draft; blocked agents need attention in
  their own pane.
- Click the popup's Corgi logo, wordmark, or Open dashboard button to launch or
  focus the stable registered Corgi dashboard and bring its Herdr terminal into view.
- Click **New agent +**, to the left of Open dashboard, to open an agent creation form inside the popup with project, harness, model, and worktree settings. The popup closes once the agent accepts the first prompt; errors keep it open.
- Escape collapses an expanded session first; otherwise it closes the popup.
  Clicking outside closes the popup.

- Hover or expand an agent, then press `x` (or click its X button) to close it with the
  dashboard's confirmation and dirty-worktree protection.
- Press `m` (or click its Merge button) to review commits, merge and push from the primary
  checkout, then optionally close the worktree. Enter confirms; Escape or
  clicking outside dismisses the dialog immediately without a cancellation popup.
- Install the release binary as `corgi-actions` beside `Panel.qml` for these actions.

The popup supports inspecting, navigating, prompting, merging, and closing
existing agents. The New agent button opens a native popup form using the same Rust launch code as the dashboard.

Desktop navigation uses Hyprland and selects a terminal running a local Herdr
client attached to the configured session. If none is open, the popup reports
that the terminal could not be found.

## Distribution

Publish this directory at the root of a dedicated distribution repository,
including a copy of the parent repository's LICENSE. Keep publication history
fast-forwardable so `omarchy plugin update` works. Consumers also need a
compatible Corgi binary; the Omarchy installer does not compile Rust.

```sh
omarchy plugin update io.github.zyrre.corgi
omarchy plugin remove io.github.zyrre.corgi
```
