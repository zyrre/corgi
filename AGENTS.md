# Development handoff

After making a change, ensure it is visible in the running environment before
handing off. Rebuild, relink, reload, or restart the affected component when
needed, then verify the change rather than assuming a source edit is live.

For Corgi, code changes normally require `cargo build --release`; plugin
metadata or link changes may also require `herdr plugin link "$PWD"`. Do not
restart Herdr or close panes unnecessarily—only do so when the affected change
cannot take effect otherwise.

## Testing worktree changes

Git worktrees are isolated: building in a worktree does not update the Corgi
plugin linked from the primary checkout, and an already running Corgi process
does not hot-reload. Before handing off Rust changes from any checkout, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

To show worktree changes interactively without replacing the stable
`prefix+d` plugin registration, launch that worktree's release binary in a
dedicated Herdr preview workspace. Every temporary preview must use the exact
visible prefix `🧪 TEMP CORGI PREVIEW` in its workspace, tab, and pane labels;
append the branch name so multiple previews remain distinguishable. Never name
a temporary preview only `Corgi` or `Corgi dashboard`.

```bash
corgi_preview_label="🧪 TEMP CORGI PREVIEW — $(git branch --show-current)"
corgi_preview_result="$(herdr workspace create --cwd "$PWD" --label "$corgi_preview_label" --no-focus)"
corgi_preview_workspace="$(jq -r '.result.workspace.workspace_id' <<<"$corgi_preview_result")"
corgi_preview_tab="$(jq -r '.result.tab.tab_id' <<<"$corgi_preview_result")"
corgi_preview_pane="$(jq -r '.result.root_pane.pane_id' <<<"$corgi_preview_result")"
herdr tab rename "$corgi_preview_tab" "$corgi_preview_label"
herdr pane rename "$corgi_preview_pane" "$corgi_preview_label"
herdr pane run "$corgi_preview_pane" "./target/release/corgi"
herdr workspace focus "$corgi_preview_workspace"
```

Report the temporary workspace label and ID in the handoff. Leave it available
when the user needs to inspect the result; otherwise close only the preview
workspace created for the test. Do not run `herdr plugin link "$PWD"` from a
disposable worktree during normal testing, because deleting that worktree would
leave `prefix+d` registered to a missing path.

Before handing off changes to `scripts/herdr-corgi-dashboard`, also run its
shell test, which drives the script against a fake `herdr` binary (no real
Herdr session needed) and checks `shellcheck` if it is installed:

```bash
bash scripts/test-herdr-corgi-dashboard
shellcheck scripts/herdr-corgi-dashboard scripts/test-herdr-corgi-dashboard
```

After reviewed changes are merged into the primary checkout, make them live
from that primary checkout with `cargo build --release` and
`herdr plugin link "$PWD"`. Reload Herdr configuration when actions or plugin
metadata changed. Quit and reopen only the Corgi dashboard when the running
process must pick up a rebuilt binary; do not restart Herdr.

## Applying Omarchy popup changes

The running Omarchy bar loads `~/.config/omarchy/plugins/io.github.zyrre.corgi/`,
not the worktree's `omarchy/` directory. Apply the changed files to that installed
directory, preserving its local manifest defaults and `corgi-reader` binary.
Avoid overwriting unrelated changes from other agents.

For QML changes, plugin rescans can retain cached components even when
`omarchy-shell shell rescanPlugins` succeeds. Once the installed files are
updated, use these two commands to load fresh QML and open the popup:

```bash
omarchy restart shell
omarchy-shell shell summon io.github.zyrre.corgi
```

This restarts only the Omarchy shell, not Herdr. Wait for the shell to become
ready before summoning, then verify the visible popup after its opening
animation finishes. Do not assume that a successful rescan made QML edits live.
If sandbox restrictions block shell IPC, run these commands with the required
sandbox escalation rather than interpreting the failure as a stopped shell.

Although Corgi is a space-constrained TUI, use clear icons and small ASCII art
where they improve scanning or personality. Keep text labels and contrast
strong enough that decoration never interferes with readability.

Use color often to clarify state, hierarchy, and actions. Prefer semantic ANSI
terminal colors over fixed RGB values so Corgi automatically follows the active
Omarchy theme palette; reserve high-contrast colors for important information.
