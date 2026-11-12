# Media for the Herdr app

A panel in the [Herdr app](https://herdr.dev) where your coding agents show
you images, GIFs and short videos: a screenshot of a UI change, a chart, a
screen recording of a bug. An agent needs no tool or command for it, with
or without Corgi: it copies a file into a drop folder, as its instructions
say (below), and the file appears in a Media panel on the right side of the
window, with a toast and a count on the tab bar's Media button.

The panel shows the newest item large, scaled to fit, with its caption,
file name and when it came. Earlier items are listed below it; click one
to see it large. It keeps the last 50.

## Install

You need the Herdr app, and `python3` for the layout change. From a
checkout of this repository:

```bash
herdr-app/mods/media/install.sh --panel right
```

That copies the mod to `mods/media` in the app's home (`~/.config/herdr-app`,
or `--home DIR`) and changes the layout the window shows (or `--layout
NAME`; Herdr's own `main` is refused, since Herdr replaces it on every
start: clone it first with `herdr-app do layout.clone
'{"name":"main","as":"mine"}'`):

- **The panel**, one more child of the right-hand panel stack beside Files
  and Changes (and GitLab or Jenkins, if you have them). One of them shows
  at a time, and the tab bar keeps its width.
- **The Media button** in the tab bar, which shows and hides the panel as
  the Files button does, with the number of items that came since you last
  looked at the panel. When the layout shows Herdr's own tab bar, the
  installer makes a copy of it (`mods/tabs-media`) with the button; when it
  shows a copy already (herdr-gitlab's, the Jenkins mod's, Corgi's), the
  button goes into that copy between marker lines. If another mod's install
  later replaces the copy, run `--panel right` again.
- **A watcher**, a floating block that draws nothing. The app starts a
  panel only once it has been shown, so without it nothing would watch the
  drop folder (no toast, no count) until you opened the panel after each
  start of the app.

Running it again changes nothing. Other options:

```bash
herdr-app/mods/media/install.sh --folder ~/shots     # another drop folder (kept in media/settings.json)
herdr-app/mods/media/install.sh --panel off          # the panel, button and watcher out of the layout; the mod stays
herdr-app/mods/media/install.sh --uninstall          # all of it out, with the mod's copies of what it showed
```

`--uninstall` leaves the drop folder and its files alone. Without `--panel`
the installer changes no layout: open the mod where you like, say `herdr-app
do tab.new --name Media --mod media`. It then watches the folder only while
it is open.

## The drop folder

`~/.local/share/herdr-media/`, made when the mod starts if it is missing.
`--folder DIR` (or `{"folder": "DIR"}` in `media/settings.json` of the app's
home; the mod reads it again within ten seconds) names another. It must be
outside the app's home: agents in Claude Code's auto mode may not write
into `~/.config/herdr-app`, which the app's own files are in.

Any PNG, JPEG, GIF, WebP, SVG or BMP file, and any MP4, MOV, M4V or WebM
video, copied there shows in the panel. Hidden files (`.name`) and files
still being written (`.tmp`, `.part`, `~`) are left alone, and a file is
taken in once its size has stayed the same for a second, so one that is
still being copied does not show half. The mod copies each picture into
`media/items/` of the app's home, since the app draws images only from
there; those copies leave with the item.

**Captions.** A text file with the same name and `.txt`
(`login-after.png` and `login-after.txt`) gives the item a caption: its
first line. A file without one shows its file name. The caption may come
a moment before or after the file; a caption added later is picked up too.

**Remove and Clear all.** The ✕ on an item (or Remove) takes it out of the
panel and deletes the mod's copy; the file stays in the drop folder, and
does not come back. Clear all (the bin, asked first) takes every item out
and deletes the files they came from, and their captions, from the drop
folder: the only time the mod deletes a file there. Other files in the
folder are never touched.

## GIFs, animated WebP and video

The Herdr app draws an animated GIF, WebP or PNG as its first frame (seen
in Herdr 0.1.15): the panel shows that frame, says it is animated, and
**Open** plays it in your system's viewer. Clicking the picture opens any
item there too.

A video is never copied. With `ffmpeg` on your `PATH`, the mod makes an
animated WebP of it in the background (at most 10 seconds, 640 pixels wide,
12 frames a second), which the panel shows as its first frame for now, and
will play once the app animates WebP. Without ffmpeg it shows a still frame
made with macOS's Quick Look (`qlmanage -t`); where neither works, a
placeholder. **Open** plays the clip in your default player.

## Telling your agents about it

Paste this into `~/.claude/CLAUDE.md` for Claude Code, `~/.codex/AGENTS.md`
for Codex, or `~/.config/opencode/AGENTS.md` for OpenCode (change the folder
if you used `--folder`):

```markdown
## Showing me images and videos (Herdr's Media panel)

When you have something visual I should look at, show it to me in the
Media panel of the Herdr app: a screenshot of a UI change you made, a
chart or diagram you made for me, a screen recording of a bug or a fix.
Not for routine output, logs, or pictures I do not need to see.

- Copy the finished file into `~/.local/share/herdr-media/` (`mkdir -p`
  it first). Write it elsewhere and copy or move it in; do not write it
  there bit by bit.
- Give it a new, unique, descriptive name each time, such as
  `2026-11-12-login-form-after.png`; never overwrite a file there.
- PNG, JPEG, GIF, WebP, SVG or BMP; a video as MP4, MOV, M4V or WebM
  (keep it short).
- For a caption, put a text file with the same name and `.txt` beside it,
  its first line the caption: `2026-11-12-login-form-after.txt` holding
  "The login form after the fix: the error now shows under the field".
- Do not edit or delete files there afterwards: I clear them from the panel.
- Say in your reply that it is in the Media panel, and what to look at.
```

## The Media button, by hand

When the installer cannot put the button into your tab bar (one not like
Herdr's), add it yourself, in the tab bar's `main.js`: the button just
before its window buttons (`.when(w?.buttons, …)`), and the line that lets
the tab bar remember the panel per space just above its
`this.syncPanel(space, panel, body, enabled);`. Keep the marker comments,
so that `--panel off` and `--uninstall` find and take them out:

```js
    // >>> Media panel: install.sh of mods/media adds the Media panel to the panels remembered per space; --panel off takes it out
    try { if (find(body, "media-panel") && !enabled.includes("media-panel")) enabled.push("media-panel"); } catch (_) {} // <<< Media panel
```

```js
      // >>> Media button: install.sh of mods/media puts it here, and takes it out (--panel off)
      .when(Boolean(find(body, "media-panel")), el => el.child(panelButton("media-panel", "Media", "mods/media/icons/media.svg")
        .when(Number(shared("media.unseen")) > 0, b => b.child(div().px(5).rounded(4).bg(t.accent).text_color(t.accent_foreground).text_size(10.5).font_semibold().child(String(Number(shared("media.unseen"))))))))
      // <<< Media button
```

And the blocks, if you change the layout by hand: `{"id": "media-panel",
"type": "block", "content": {"mod": "media", "props": {"panel": true}}}` as
a child of the stack `panel-body`, and `{"id": "media-watch", "content":
{"mod": "media", "props": {"watch": true}}, "edge": "bottom", "align":
"end"}` in the layout root's `floating` list.

## Keys

`media.show` shows the panel (what the toast's click runs) and
`media.refresh` looks at the folder again, for a key in `keys.json`:
`"global": { "ctrl-alt-m": "media.show" }`.

## Limits

- Agents on other machines (spaces over SSH) cannot reach the folder: the
  panel shows what is dropped on the machine the window runs on.
- The app draws only images inside its own home, so every picture is
  copied there; 50 items of large screenshots take that much disk.
