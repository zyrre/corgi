# Jenkins for the Herdr app

A mod for the [Herdr](https://herdr.dev) app that shows your Jenkins jobs and
builds and lets you act on them: the jobs of the folders you name under
large headings that fold away, with their last build's state; a Pipeline
job's stages (the one at work, the ones that failed and why) and each
stage's part of the console log; a job's recent builds; a build's whole
console log (followed while it runs); and Build (with a form for the job's
parameters) and Abort, each asked about first. It can live as a panel on the
right of the window.

It reaches Jenkins with **an API token alone**: no password, no SSO, no
`jenkins-cli`. **The token's Jenkins user decides what the mod may do.** Give
it the token of a user who may only read, and it only reads; what Jenkins
refuses shows as "This token may not build …", and nothing works around it.

It stands on its own: it needs no Corgi and no other mod, and Corgi's
`herdr-app/install.sh` does not install it.

## Install

```sh
herdr-app/mods/jenkins/install.sh                     # into ~/.config/herdr-app (or $HERDR_APP_HOME)
herdr-app/mods/jenkins/install.sh --panel right       # and as a panel on the right of the window, with a Jenkins button in the tab bar
herdr-app/mods/jenkins/install.sh --panel off         # takes the panel and its button out again; the mod stays
herdr-app/mods/jenkins/install.sh --home DIR          # into another app home
herdr-app/mods/jenkins/install.sh --uninstall         # the mod, its panel and button; --forget also removes the settings and the token
```

It copies the mod to `mods/jenkins` in the app's home, after `herdr-app check`
has passed it. Run it again to update.

### As a panel on the right

`--panel right` changes the layout the window shows (or `--layout NAME`,
`layouts/NAME.json`) in place: it adds one block at the right end of the
layout's top row, after everything that was there, so the panel runs the
window's full height in the sidebar's colour, and the Files and Changes
panel (or whatever else the layout has) stays where it was and opens as
before. Drag the panel's edge to size it; the app remembers. Running it
again changes nothing, and it adds no second panel to a layout that shows
Jenkins already. `--panel off` takes out the panel it added; `--uninstall`
takes every block showing the mod out of your layouts (only out of NAME's
with `--layout`), since a block naming a mod that is gone shows an error.

Herdr's own layouts (`main`, `terminals`, `win98`) are refused: Herdr
replaces them on every start. Clone one first and name the clone:

```sh
herdr-app do layout.clone '{"name":"main","as":"mine"}'
herdr-app/mods/jenkins/install.sh --panel right --layout mine
```

Changing a layout needs `python3` (to read and write its JSON). Without
it, add the block yourself as the last child of the layout's top row:
`{ "id": "jenkins", "type": "block", "content": { "mod": "jenkins", "props": { "panel": true } }, "size": { "px": 320 }, "min": 260, "max": 640, "background": "chrome" }`.
`props.panel` tells the mod to paint no background, so the block's shows.

### The Jenkins button

`--panel right` also puts a **Jenkins** button into the layout's tab bar,
beside Files and Changes (and GitLab, when that mod is installed), that
shows and hides the panel. The tab bar is the mod the layout's `tabs` block
shows:

- **Herdr's own (`tabs`).** Herdr replaces it on every start and it has no
  place for another button, so the installer makes a copy of it,
  `mods/tabs-jenkins` (as `herdr-app do mod.clone` does: its own asset
  paths, `cloned_from` in its `mod.json`), puts the button in, and points
  the `tabs` block at the copy. Running it again after Herdr updated its tab
  bar makes the copy again from the new one, while the copy is as it made
  it.
- **A copy already** (such as `tabs-gitlab`, which
  [herdr-gitlab](https://github.com/rem-aster/herdr-gitlab) makes, or your
  own). The installer puts the button into that copy's `main.js`, just
  before the window buttons, between two marker lines, and changes nothing
  else in it. A copy that has a Jenkins button already, not between the
  markers (one put in by hand), gets no second one.
- **Another of Herdr's** (the Terminals layout's `terminals-bar`), or no
  `tabs` block: no button. `herdr-app do layout.toggle --id jenkins` shows
  and hides the panel, from a key in `keys.json` as well.

`--panel off` and `--uninstall` take the button out again. If the
installer made `tabs-jenkins` and nothing else in it was changed since,
they point the layout back at Herdr's `tabs` and remove the copy (once no
other layout shows it); a copy changed since (another mod's installer may
put its own button in) stays the layout's, without the Jenkins button. A
second layout that shows the same copy and has the panel keeps the button.

**Another mod's install can take the button away.** One that replaces the
tab bar copy's `main.js`, or points the layout's `tabs` block at another tab
bar, leaves a layout with the panel but no button: run `install.sh --panel
right` again to bring it back. (herdr-gitlab's installer, as of 2026-11-11,
leaves a layout that has its panel alone, and otherwise edits the copy the
`tabs` block shows in place, `tabs-jenkins` included, so it keeps the
button.)

When the copy is not like Herdr's tab bar (no `.when(w?.buttons, …)` for
the window buttons, or not the names below), the installer changes nothing
and says so. Put the button in by hand, where the other panel buttons are,
with the marker lines, so the installer can update it and take it out; then
run `--panel right` again. It uses the tab bar's own `find`, `root` (the
`layout()`), `t` (the `theme()`), `stop`, `h_flex`, `svg` and `run`; `jenkins`
is the panel block's id:

```js
      // >>> Jenkins button: install.sh of mods/jenkins puts it here, and takes it out (--panel off)
      .when(Boolean(find(root, "jenkins")), el => el.child((() => {
        // Jenkins (mods/jenkins) is its own panel on the right: this button shows and hides it.
        const on = !find(root, "jenkins").hidden;
        return h_flex().id("panel-jenkins").h(26).px(8).gap(5).flex_none().items_center().rounded(6)
          .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
          .hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
          .on_click(() => run("layout.toggle", { id: "jenkins" }))
          .child(svg("mods/jenkins/icons/jenkins.svg").size(14)).child("Jenkins");
      })()))
      // <<< Jenkins button
```

Before it changes a tab bar, the installer checks the changed copy with
`herdr-app check`, and it writes the tab bar before the layout, each file
whole with one rename; a step that fails leaves both as they were.

### Elsewhere

```sh
herdr-app do mod.open --name jenkins                 # over the window; again, or Esc, closes it
herdr-app do tab.new --name Jenkins --mod jenkins    # as a tab of the space
```

or put a block `{ "id": "jenkins", "type": "block", "content": { "mod": "jenkins" } }`
in a layout of your own, or bind a key in `keys.json`:
`"cmd-j": {"command": "mod.open", "args": {"name": "jenkins"}}`. It draws
itself for a narrow side panel as well as a wide tab.

## Settings

The first time it shows, it asks for them; the sliders button in its header
shows them again.

| Setting | What it is |
|---|---|
| Jenkins URL | The base URL, such as `https://jenkins.example.com` |
| User | The Jenkins user the token belongs to |
| API token | A token of that user (Jenkins: your name, Security, API Token) |
| Folders | Whose jobs to list, commas apart: a folder (`team-dev`), a folder in a folder (`team/tools`), a view (`view/Deploys`), or a whole URL. Empty: the top level |

**Check** asks Jenkins who the token is (`Authenticated as Jane Doe (jane)`,
or why not) and saves nothing; **Save** saves and then checks.

They are kept in `jenkins/settings.json` in the app's home
(`~/.config/herdr-app/jenkins/settings.json`), outside the mod's own folder,
readable by you alone (mode 600). The token is in that file in plain text,
as a `~/.netrc` holds one; it goes nowhere else: not into logs, not into a
URL, only into the `Authorization` header of requests to the Jenkins URL you
gave. With nothing saved, `JENKINS_URL`, `JENKINS_USER`, `JENKINS_API_TOKEN`
(and `JENKINS_FOLDERS`) are used when the app has them in its environment,
which an app started from the Dock usually has not.

## Use

- The list: each folder of the settings under a large heading, tagged `DEV`
  or `PROD` when its name says so (`dev`, `develop`, `development`; `prod`,
  `production`, `master`, `live`, as a word of the name: `instat-develop`,
  `instat-master`), with how many of its jobs run and failed and how many
  there are. A click on the heading folds the folder away, or shows it
  again; the app remembers which are folded. Under it, each job with a mark
  for the last build (success, failed, unstable, aborted, never built,
  disabled), its number and age; a running build shows how far it is by
  Jenkins' estimate. A folder inside opens as a list of its own. Read again
  every minute, every ten seconds while a build runs or waits; the header's
  arrows read it now.
- Click a job for the stages of its newest build, and its last 20 builds
  under them; click a build there for its stages instead. Each stage shows
  its state (passed, failed, unstable, aborted, running, waiting for input,
  skipped), how long it took, and why it failed when Jenkins says. The one
  at work turns and has a bar filled by what it took last time; while a
  build runs, the stages still to come show dimmed with the time they took
  last time. A running build's stages are read every five seconds.
- Click a stage for its part of the console log, from where it starts to
  where it ends, followed while it runs. `Whole log` there, or `Console
  log` above the stages, shows the whole of it. A running log is followed;
  scrolling up stops that, `Follow` takes it up again. Only the last 5000
  lines are kept.
- A freestyle job has no stages: its builds open their console log, as do
  all builds on a Jenkins without the Pipeline Stage View plugin.
- `Build…` shows the job's parameters, each preset to its default (text, a
  choice, yes or no, a password); `Build` there starts it, `Cancel` or Esc
  does not. A file or credential parameter cannot be given: Jenkins uses its
  default.
- The square on a running build asks `Abort #57 of …?` before it stops it.
- The arrow-out buttons open the folder, job or build in the browser.
- Esc goes back a view. `jenkins.refresh` and `jenkins.settings` are its
  commands for `keys.json`.

## When Jenkins does not answer

A VPN that drops for a moment, or a Jenkins that is slow or restarting,
shows quietly: what was read last stays, with a dim `Last read 40s ago · no
answer since` above it. The error (`Could not reach Jenkins to …`) shows
only after three failed reads in a row, or a minute of failing, and goes
with the next good read. With nothing read yet it says `Reading…` until
then. What will not pass by itself shows at once: a token Jenkins does not
accept (401), one that may not see a folder (403), a folder that is not
there (404).

A read that failed waits before it is tried again: 10 seconds, then 30,
then a minute for as long as it fails (never sooner than it would be read
anyway), so a Jenkins that is down gets a request a minute for each list,
job or log shown. A good read puts it back to its usual pace. The
header's arrows read everything now, waiting or not. A read with no answer
in 45 seconds counts as failed.

The panel, an overlay and a tab of the mod share what they read: a list,
job, build's stages or log shown in all of them is read once, as often as
one of them would. Each keeps its own view. Settings saved in one are taken
up by the others.

## How it talks to Jenkins

Plain HTTP with the app's `fetch`, HTTP Basic auth `<user>:<token>`:
`whoAmI/api/json` and `me/api/json`, `<folder>/api/json?tree=jobs[…]`,
`<job>/api/json?tree=…builds[…]{0,20}`, `<build>/api/json`,
`<build>/logText/progressiveText?start=<byte>`, Pipeline Stage View's
`<build>/wfapi/describe` (a build's stages) and `<job>/wfapi/runs` (what
they took last time), and, only from the two
buttons, `POST <job>/build` or `<job>/buildWithParameters` and
`POST <build>/stop`, with the crumb from `crumbIssuer/api/json` when the
Jenkins has one.

A stage's log is cut out of the console log, between the lines Pipeline
writes where a stage starts and ends (`[Pipeline] { (Build)` …
`[Pipeline] // stage`). The plugin's own `execution/node/<id>/wfapi/log` was
no use for it on the Jenkins this was made with: a stage's node has no text
of its own, each step's comes as HTML, and a stage lists 100 steps at most,
which a long deploy stage passes. The console is one read, already followed
while the build runs. Parallel branches write into the console together, so
a stage with branches inside shows all of them.

## Develop

`main.js` (the views), `data.js` (what is read, shared by every copy of
the mod, with the back-off and when an error shows), `api.js` (Jenkins and
the settings), `util.js` (pure helpers), `ui.js` (marks and buttons),
`icons/`. After a change:

```sh
herdr-app check herdr-app/mods/jenkins
node herdr-app/mods/jenkins/tests/test.mjs
```

The tests run the mod under Node against `tests/fake-jenkins.mjs`, a made-up
Jenkins with canned jobs and two users (`jane:fake-token` may build and
abort, `viewer:viewer-token` may only read). Its Pipeline jobs have stages,
with Pipeline Stage View's `wfapi` in the shapes a real Jenkins gives and a
console with the markers a real one writes: in `team-dev` a running build,
a failed one, an aborted one, an unstable one and one with a stage skipped;
in `team-prod` one that waits for approval; `nightly-cleanup` is freestyle.
`node tests/fake-jenkins.mjs
48765` serves it for a look at the mod in a window: point the settings at
`http://127.0.0.1:48765`. It fails on demand: `curl -X POST
'http://127.0.0.1:48765/__fake/fail?mode=status&status=503'` (or
`mode=refuse`, `mode=stall`; `&count=2` for the next two requests,
`&path=<regex>` for some paths), and `/__fake/recover` ends it. Never try Build or Abort against a real Jenkins
from a test.

The icons are drawn after [Lucide](https://lucide.dev) (ISC license).
