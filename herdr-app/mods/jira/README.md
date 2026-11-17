# Jira for the Herdr app

A mod for the [Herdr](https://herdr.dev) app that shows a Jira board's
active sprint and lets you act on its issues without leaving the window:
your issues first, the rest grouped by status under headings that fold; an
issue with its type, status, assignee, description (Jira's wiki markup made
readable: headings, lists, code, quotes, tables, links), subtasks, links
and comments; and four actions, each asked about first: change its status,
assign it to you, add a comment, log work. Popups tell you when an issue is
assigned to you, or when someone else comments on or moves one of yours. It
can live in the window's right-hand panel, beside Files and Changes.

It is made for **Jira Server and Data Center** (the REST API `/rest/api/2`
and the agile API `/rest/agile/1.0`) and reaches Jira with **a personal
access token alone**: no password, no SSO, no `jira` CLI. **The token's Jira
user decides what the mod may do.** Give it the token of a user who may only
read, and it only reads; what Jira refuses shows as "Jira refused to let
this token as … move DEMO-12 to Done (…)", and nothing works around it.

It stands on its own: it needs no Corgi and no other mod, and Corgi's
`herdr-app/install.sh` does not install it.

## Install

```sh
herdr-app/mods/jira/install.sh                     # into ~/.config/herdr-app (or $HERDR_APP_HOME)
herdr-app/mods/jira/install.sh --panel right       # and in the right-hand panel beside Files and Changes, with a Jira button in the tab bar
herdr-app/mods/jira/install.sh --panel off         # takes the panel and its button out again; the mod stays
herdr-app/mods/jira/install.sh --home DIR          # into another app home
herdr-app/mods/jira/install.sh --uninstall         # the mod, its panel and button; --forget also removes the settings and the token
```

It copies the mod to `mods/jira` in the app's home, after `herdr-app check`
has passed it. Run it again to update.

### In the right-hand panel

`--panel right` changes the layout the window shows (or `--layout NAME`,
`layouts/NAME.json`) in place: it adds Jira to the layout's right-hand
panel, the one under the tab bar where Files and Changes (and GitLab,
Jenkins or Media, when those mods are installed) show, as one more child of
its stack `panel-body`. One of them shows at a time, and opening Jira
changes nothing else in the window. It looks as they do there: a card with
a border, its own header under the tab bar. Running it again changes
nothing. `--panel off` takes out the one it added; `--uninstall` takes every
block showing the mod out of your layouts (only out of NAME's with
`--layout`), since a block naming a mod that is gone shows an error.

A layout with no such panel (a stack `panel-body` in a column `panel`, as
in Herdr's `main`) is left alone. Herdr's own layouts (`main`, `terminals`,
`win98`) are refused: Herdr replaces them on every start. Clone one first
and name the clone:

```sh
herdr-app do layout.clone '{"name":"main","as":"mine"}'
herdr-app/mods/jira/install.sh --panel right --layout mine
```

Changing a layout needs `python3` (to read and write its JSON). Without
it, add the block yourself as the last child of the stack `panel-body`:
`{ "id": "jira-panel", "type": "block", "content": { "mod": "jira", "props": { "panel": true } } }`.
`props.panel` tells the mod it is in the panel, so it draws itself as Files
and Changes do, and tells the installer the block is the one it manages.

### The Jira button

`--panel right` also puts a **Jira** button into the layout's tab bar,
after Files and Changes (and the others'), that works as theirs do: it
shows Jira in the panel, switches the panel to Jira from another of them,
and hides the panel while Jira shows; it is lit only while Jira shows. The
tab bar also remembers, per space, that Jira was the open panel. The tab bar
is the mod the layout's `tabs` block shows:

- **Herdr's own (`tabs`).** Herdr replaces it on every start and it has no
  place for another button, so the installer makes a copy of it,
  `mods/tabs-jira` (as `herdr-app do mod.clone` does: its own asset paths,
  `cloned_from` in its `mod.json`), puts the button in, and points the
  `tabs` block at the copy.
- **A copy already** (such as `tabs-gitlab`, which
  [herdr-gitlab](https://github.com/rem-aster/herdr-gitlab) makes, the
  Jenkins mod's `tabs-jenkins`, the Media mod's `tabs-media`, or your own).
  The installer adds two places to that copy's `main.js`, each between
  marker lines, and changes nothing else in it, so the other installers'
  lines stay as they are and each can be run again, in any order: a line
  above the tab bar's `this.syncPanel(…)` call that adds Jira to the panels
  it remembers per space, and the button just before the window buttons.
- **Another of Herdr's** (the Terminals layout's `terminals-bar`), or no
  `tabs` block: no button. `herdr-app do layout.select --stack panel-body
  --child jira-panel`, then `herdr-app do layout.show --id panel`, shows
  it, from keys in `keys.json` as well.

`--panel off` and `--uninstall` take the button out again. If the
installer made `tabs-jira` and nothing else in it was changed since, they
point the layout back at Herdr's `tabs` and remove the copy (once no other
layout shows it). Another mod's install that replaces the tab bar copy, or
points the layout at another tab bar, takes the button away: run
`install.sh --panel right` again to bring it back.

When the copy is not like Herdr's tab bar (no single `this.syncPanel(…)`
call, no `.when(w?.buttons, …)` for the window buttons, or not the names
below), the installer changes nothing and says so. Add both by hand, with
the marker lines, so the installer can update them and take them out. They
use the tab bar's own `find`, `body` (the stack, `find(root,
"panel-body")`), `enabled` and `panelButton`. Above `this.syncPanel(…)`:

```js
    // >>> Jira panel: install.sh of mods/jira adds the Jira panel to the panels remembered per space; --panel off takes it out
    try { if (find(body, "jira-panel") && !enabled.includes("jira-panel")) enabled.push("jira-panel"); } catch (_) {} // <<< Jira panel
```

and the button, after the other panel buttons:

```js
      // >>> Jira button: install.sh of mods/jira puts it here, and takes it out (--panel off)
      .when(Boolean(find(body, "jira-panel")), el => el.child(panelButton("jira-panel", "Jira", "mods/jira/icons/jira.svg")))
      // <<< Jira button
```

Before it changes a tab bar, the installer checks the changed copy with
`herdr-app check`, and it writes the tab bar before the layout, each file
whole with one rename; a step that fails leaves both as they were.

### Elsewhere

```sh
herdr-app do mod.open --name jira                 # over the window; again, or Esc, closes it
herdr-app do tab.new --name Jira --mod jira       # as a tab of the space
```

or put a block `{ "id": "jira", "type": "block", "content": { "mod": "jira" } }`
in a layout of your own. It draws itself for a narrow side panel as well as
a wide tab.

## Settings

The first time it shows, it asks for them; the sliders button in its header
shows them again.

| Setting | What it is |
|---|---|
| Jira URL | The base URL, such as `https://jira.example.com` |
| Personal access token | A token of your Jira user (Jira: your profile, Personal Access Tokens) |
| Board | A board's id (`42`, or the board's address with `rapidView=42` in it), or a project's key (`DEMO`): its first Scrum board |

**Check** asks Jira who the token is and finds the board (`Authenticated as
Jane Doe (jane) · board 42, DEMO board`, or why not) and saves nothing;
**Save** saves and then checks.

They are kept in `jira/settings.json` in the app's home
(`~/.config/herdr-app/jira/settings.json`), outside the mod's own folder,
readable by you alone (mode 600). The token is in that file in plain text,
as a `~/.netrc` holds one; it goes nowhere else: not into logs, not into a
URL, only into the `Authorization: Bearer` header of requests to the Jira
URL you gave. With nothing saved, `JIRA_URL` (or `JIRA_SERVER`),
`JIRA_API_TOKEN` and `JIRA_BOARD` are used when the app has them in its
environment, which an app started from the Dock usually has not.

## Use

- **The list**: the board's active sprint (each, when it runs more than
  one), with when it ends and how many issues it has. Under it, the issues
  assigned to you, those at work first, then to do, then done; then
  everyone else's, under a heading for each status: the statuses at work
  first, then to do, then done. Each row shows the issue's type, key and
  summary, and the assignee (or, among yours, the status). A click on a
  heading folds it away or shows it again; the done ones start folded, and
  the app remembers what you fold. A heading with more than 40 issues shows
  40 and `Show all`. Read again every minute while the list shows (every
  three minutes while an issue shows over it); the header's arrows read it
  now.
- **An issue**: click a row. Its summary, type, priority, assignee,
  reporter, when it changed, the work logged; its description with Jira's
  wiki markup made readable (headings, `*bold*` without its stars, lists,
  `{code}` and `{noformat}` blocks, quotes and panels, tables, links by
  their text, `[~name]` as `@name`, images as `[image: name]`); its
  subtasks, its links (`blocks DEMO-120`) and its parent, each a click away;
  and its last 20 comments. Read again every half minute while it shows.
  The arrow-out button opens it in the browser.
- **Status…** lists the steps Jira offers your token from where the issue
  is (`Send to Review → IN REVIEW`). Pick one and it asks `Send to Review:
  move DEMO-12 from In Progress to In Review?` first; `Move` (or Enter)
  moves it, `Cancel` (or Esc) does not. A step whose screen requires a field
  (a resolution, say) is refused here, with the way to the browser: this
  panel does not fill in screens.
- **Assign to me** (on an issue that is not yours) asks first: `Assign
  DEMO-12 to you (Jane Doe)? It is Bob's now.`
- **Comment…** opens a form for the comment (Jira's wiki markup works);
  `Add comment` posts it as you, `Cancel` or Esc does not.
- **Log work…** opens a form: time spent (`1h 30m`, `45m`, `1d`; Jira's
  units w, d, h, m), the day (today, or a day before as `YYYY-MM-DD`; work
  on another day is logged at 09:00 of it), a comment if you like. `Log 1h
  30m` (or Enter) logs it.
- After each action a line says what it came to (`Moved DEMO-12 to In
  Review.`), or why Jira refused it, and the issue is read again.
- **Popups.** When an issue is assigned to you by someone else, or someone
  else comments on one of your issues or changes its status, a popup says
  so (`DEMO-12 is yours now`, `Bob commented on DEMO-12`, `DEMO-12 is In
  Review`); more than three at once make one popup for all of them. None
  comes for what happened before the app started (or the settings changed),
  or for what you did yourself. A click on it opens the panel on that issue.
  They come whether the panel shows or not: the panel reads your issues for
  them every minute while it shows, every three minutes while it is hidden
  or another panel shows. The app starts the panel only once it is first
  shown, so after the app starts the popups begin once you have opened the
  panel. A hidden panel gets no commands, so a popup it posted while hidden
  shows the panel when clicked (it selects Jira in the panel, or shows the
  panel), and the panel then opens on that issue.
- **Mute.** The bell on an issue's row (it shows while the pointer is over
  the row) and in its view mutes its popups, or unmutes them; a muted issue
  shows a crossed-out bell. The app remembers it across restarts.
- Esc goes back a view, or drops the question asked.

### Commands

For `keys.json` and other mods:

- `jira.open-<KEY>`, such as `jira.open-DEMO-12` (any case: `jira.open-demo-12`
  is the same), shows the panel on that issue. The app gives a mod's
  command to the copies that show only, so it does nothing while the panel
  is hidden or another panel shows; show the panel first (`layout.select`
  and `layout.show`, as above), or, from another mod, use the shared value
  below.
- `setShared("jira.open", { key: "DEMO-12", at: Date.now() })`, from
  another mod, shows the panel on that issue whether it shows or not: the
  panel looks for it every two seconds, also while hidden (once it has been
  shown since the app started), and takes up one set in the last 20 seconds,
  once.
- `jira.show` shows the panel on the list; `jira.refresh` reads what it
  shows now; `jira.settings` opens the settings.

## When Jira does not answer

A VPN that drops for a moment, or a Jira that is slow or restarting, shows
quietly while there is something read before to show: that stays, with a
dim `Last read 40s ago · no answer since` above it. The error (`Could not
reach Jira to …`) shows after three failed reads in a row, or a minute of
failing, and goes with the next good read. With nothing read yet, the error
shows at once, never a `Reading…` that waits for ever. What will not pass
by itself shows at once too: a token Jira does not accept (401), one that
may not see a thing (403), a thing that is not there (404), what Jira did
not take (400).

A read that failed waits before it is tried again: 10 seconds, then 30,
then a minute for as long as it fails (never sooner than it would be read
anyway). A read with no answer in 30 seconds counts as failed, and the view
says so at once. The header's arrows read everything now.

While the panel is hidden or another panel shows, it reads what it shows
every ten minutes at most, and reads it at once when it shows again.

The panel, an overlay and a tab of the mod share what they read: a sprint
or an issue shown in all of them is read once, as often as one of them
would. Each keeps its own view. Settings saved in one are taken up by the
others.

## How it talks to Jira

Plain HTTP with the app's `fetch`, `Authorization: Bearer <token>`:
`/rest/api/2/myself` (who the token is), the agile API's
`/rest/agile/1.0/board/<id>` (or `board?projectKeyOrId=<KEY>`) and
`board/<id>/sprint?state=active`, `/rest/api/2/search` with
`sprint = <id> ORDER BY Rank ASC` (500 at a time; the agile API's
`sprint/<id>/issue` gives the same but took twenty times as long on a
sprint of 450 issues), `/rest/api/2/issue/<key>` and its
`transitions?expand=transitions.fields`, and for the popups
`search` with `assignee = currentUser() AND updated >= -<minutes>m` and
`expand=changelog` (the history, for assignments and status changes, and
the comments). Only from the buttons: `POST issue/<key>/transitions`,
`PUT issue/<key>/assignee`, `POST issue/<key>/comment` and `POST
issue/<key>/worklog`, each with `X-Atlassian-Token: no-check`. Jira Server
answers an assignment the user may not make with 401: with a token that
reads well, the mod shows that as a refusal, not as a bad token.

## Develop

`main.js` (the views), `data.js` (what is read, shared by every copy of
the mod, with the back-off and when an error shows), `alerts.js` (what
makes a popup and what it says; the panel in the layout), `api.js` (Jira
and the settings), `util.js` (pure helpers), `wiki.js` (wiki markup made
readable), `ui.js` (marks and buttons), `icons/`. After a change:

```sh
herdr-app check herdr-app/mods/jira
node herdr-app/mods/jira/tests/test.mjs
shellcheck herdr-app/mods/jira/install.sh
```

The tests run the mod under Node against `tests/fake-jira.mjs`, a made-up
Jira with a project DEMO: a Scrum board 7 whose active sprint has 64 issues
(read in two pages, as the fake gives 50 at a time), a Kanban board 8,
issues with descriptions, comments, subtasks, links, transitions (one,
Approve, whose screen requires a resolution) and work logged, and two
users (`fake-token` is Jane Doe, who may act; `viewer-token` may only
read). `node tests/fake-jira.mjs 48766` serves it for a look at the mod in
a window: point the settings at `http://127.0.0.1:48766`, token
`fake-token`, board `7`. `curl -X POST
'http://127.0.0.1:48766/__fake/assign?key=DEMO-104&to=jane&by=bob'` (or
`/__fake/comment?key=DEMO-102&by=alice&body=Hello`,
`/__fake/move?key=DEMO-103&to=In%20Review&by=bob`) makes a popup. It fails
on demand: `curl -X POST
'http://127.0.0.1:48766/__fake/fail?mode=status&status=503'` (or
`mode=refuse`, `mode=stall`; `&count=2`, `&path=<regex>`), and
`/__fake/recover` ends it. Never try an action against a real Jira from a
test.

The icons are drawn after [Lucide](https://lucide.dev) (ISC license).
