# Jenkins for the Herdr app

A mod for the [Herdr](https://herdr.dev) app that shows your Jenkins jobs and
builds and lets you act on them: the jobs of the folders you name under
large headings that fold away, with their last build's state; a Pipeline
job's stages (the one at work, the ones that failed and why) and each
stage's part of the console log; a job's recent builds; a build's whole
console log (followed while it runs); Build (with a form for the job's
parameters) and Abort, each asked about first; and the answer to a
Pipeline `input` step a build waits at (a release notes step, an
approval): its proceed button, with the values it asks for, or Abort. It
can live in the window's right-hand panel, beside Files and Changes.

It reaches Jenkins with **an API token alone**: no password, no SSO, no
`jenkins-cli`. **The token's Jenkins user decides what the mod may do.** Give
it the token of a user who may only read, and it only reads; what Jenkins
refuses shows as "This token may not build …", and nothing works around it.

It stands on its own: it needs no Corgi and no other mod, and Corgi's
`herdr-app/install.sh` does not install it.

## Install

```sh
herdr-app/mods/jenkins/install.sh                     # into ~/.config/herdr-app (or $HERDR_APP_HOME)
herdr-app/mods/jenkins/install.sh --panel right       # and in the right-hand panel beside Files and Changes, with a Jenkins button in the tab bar
herdr-app/mods/jenkins/install.sh --panel off         # takes the panel and its button out again; the mod stays
herdr-app/mods/jenkins/install.sh --home DIR          # into another app home
herdr-app/mods/jenkins/install.sh --uninstall         # the mod, its panel and button; --forget also removes the settings and the token
```

It copies the mod to `mods/jenkins` in the app's home, after `herdr-app check`
has passed it. Run it again to update.

### In the right-hand panel

`--panel right` changes the layout the window shows (or `--layout NAME`,
`layouts/NAME.json`) in place: it adds Jenkins to the layout's right-hand
panel, the one under the tab bar where Files and Changes (and GitLab, when
that mod is installed) show, as one more child of its stack `panel-body`.
One of them shows at a time, and opening Jenkins changes nothing else in
the window: the tab bar keeps its width and its buttons their places. It
looks as they do there: a card with a border, its own header under the tab
bar. Drag the panel's edge to size it; the app remembers, for all of them.
Running it again changes nothing, and it adds no second Jenkins to a stack
that has one already. `--panel off` takes out the one it added;
`--uninstall` takes every block showing the mod out of your layouts (only
out of NAME's with `--layout`), since a block naming a mod that is gone
shows an error.

An earlier version of the installer put the panel in a block of its own at
the right end of the layout's top row, full height, which made the tab bar
narrower while it showed. Running `--panel right` again moves it into the
stack and updates the button (below); nothing else in the layout changes.

A layout with no such panel (a stack `panel-body` in a column `panel`, as
in Herdr's `main`) is left alone. Herdr's own layouts (`main`, `terminals`,
`win98`) are refused: Herdr replaces them on every start. Clone one first
and name the clone:

```sh
herdr-app do layout.clone '{"name":"main","as":"mine"}'
herdr-app/mods/jenkins/install.sh --panel right --layout mine
```

Changing a layout needs `python3` (to read and write its JSON). Without
it, add the block yourself as the last child of the stack `panel-body`:
`{ "id": "jenkins-panel", "type": "block", "content": { "mod": "jenkins", "props": { "panel": true } } }`.
`props.panel` tells the mod it is in the panel, so it draws itself as Files
and Changes do, and tells the installer the block is the one it manages.
The id is `jenkins-panel`, not `jenkins`, because the app keeps a block's
state (hidden, size) by id after the block is gone, and the earlier
version's block `jenkins`, hidden by its button, would hide a stack child
of that id.

### The Jenkins button

`--panel right` also puts a **Jenkins** button into the layout's tab bar,
after Files and Changes (and GitLab), that works as theirs do: it shows
Jenkins in the panel, switches the panel to Jenkins from another of them,
and hides the panel while Jenkins shows; it is lit only while Jenkins
shows. The tab bar also remembers, per space, that Jenkins was the open
panel, as it does for Files and Changes. The tab bar is the mod the
layout's `tabs` block shows:

- **Herdr's own (`tabs`).** Herdr replaces it on every start and it has no
  place for another button, so the installer makes a copy of it,
  `mods/tabs-jenkins` (as `herdr-app do mod.clone` does: its own asset
  paths, `cloned_from` in its `mod.json`), puts the button in, and points
  the `tabs` block at the copy. Running it again after Herdr updated its tab
  bar makes the copy again from the new one, while the copy is as it made
  it.
- **A copy already** (such as `tabs-gitlab`, which
  [herdr-gitlab](https://github.com/rem-aster/herdr-gitlab) makes, or your
  own). The installer changes two places in that copy's `main.js`, each
  between two marker lines, and nothing else in it: the button, just
  before the window buttons, and the tab bar's `this.syncPanel(…,
  enabled);` line (which remembers per space which panel was open), whose
  list gets Jenkins while the stack has it. It leaves the `enabled` line
  alone, which herdr-gitlab's installer looks for, so herdr-gitlab can still
  be installed after it. A copy that has a Jenkins button already, not
  between the markers (one put in by hand), is left as it is.
- **Another of Herdr's** (the Terminals layout's `terminals-bar`), or no
  `tabs` block: no button. `herdr-app do layout.select --stack panel-body
  --child jenkins-panel`, then `herdr-app do layout.show --id panel`, shows
  it, from keys in `keys.json` as well.

`--panel off` and `--uninstall` take the button out again, and put the
`syncPanel` line back as it was. If the installer made `tabs-jenkins` and
nothing else in it was changed since, they point the layout back at Herdr's
`tabs` and remove the copy (once no other layout shows it); a copy changed
since (another mod's installer may put its own button in) stays the
layout's, without the Jenkins button. A second layout that shows the same
copy and has the panel keeps the button.

**Another mod's install can take the button away.** One that replaces the
tab bar copy's `main.js`, or points the layout's `tabs` block at another tab
bar, leaves a layout with the panel but no button: run `install.sh --panel
right` again to bring it back. (herdr-gitlab's installer, as of 2026-11-11,
leaves a layout that has its panel alone, and otherwise edits the copy the
`tabs` block shows in place, `tabs-jenkins` included, so it keeps the
button.)

When the copy is not like Herdr's tab bar (no single `this.syncPanel(…,
enabled);` line, no `.when(w?.buttons, …)` for the window buttons, or not
the names below), the installer changes nothing and says so. Make both
changes by hand, with the marker lines, so the installer can update them
and take them out; then run `--panel right` again. They use the tab bar's
own `find`, `body` (the stack, `find(root, "panel-body")`), `enabled` and
`panelButton`; `jenkins-panel` is the stack child's id. The `syncPanel`
line, with the tab bar's own line kept after `was:`:

```js
    // >>> Jenkins panel: install.sh of mods/jenkins adds it to the panels remembered per space; --panel off puts back the line after was:
    // was: this.syncPanel(space, panel, body, enabled);
    this.syncPanel(space, panel, body, [...enabled, ...(find(body, "jenkins-panel") ? ["jenkins-panel"] : [])]);
    // <<< Jenkins panel
```

and the button, after the other panel buttons:

```js
      // >>> Jenkins button: install.sh of mods/jenkins puts it here, and takes it out (--panel off)
      .when(Boolean(find(body, "jenkins-panel")), el => el.child(panelButton("jenkins-panel", "Jenkins", "mods/jenkins/icons/jenkins.svg")))
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
  `production`, `master`, `live`, as a word of the name: `acme-develop`,
  `acme-master`), with how many of its jobs run and failed and how many
  there are. A click on the heading folds the folder away, or shows it
  again; the app remembers which are folded. Under it, each job with a mark
  for the last build (success, failed, unstable, aborted, never built,
  disabled), its number and age; a running build shows how far it is by
  Jenkins' estimate, and one that waits at an input step says
  `#41 · waiting for input` instead (the heading counts those apart from
  the ones that run). A folder inside opens as a list of its own. Read
  again every minute, every ten seconds while a build runs or waits; the
  header's arrows read it now. Whether a running Pipeline build waits is
  read from its stages every half minute.
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
- A build that waits at an input step shows the step under the stage that
  waits, in the job's stages and in that stage's (or the build's) log: its
  message and two buttons named as the step names them. One that asks for
  nothing proceeds at once with its button (`Proceed`, or the step's own
  `ok` text): the step is the question already. One that asks for values
  says which, and its button (`Release…`) opens a form with them, each
  preset to its default, as the build form does; its button there (or
  Enter) answers, `Cancel` or Esc does not. `Abort…` asks `Abort #12 of …
  at its input?` first, then Jenkins ends the build as aborted; while the
  build waits, it is the build view's only Abort. What
  Jenkins refuses, or an input answered meanwhile in the browser, shows as
  an error, and the view reads the build again. The token's user must be
  one the step accepts (its `submitter`).
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
`<build>/wfapi/describe` (a build's stages), `<job>/wfapi/runs` (what
they took last time) and `<build>/wfapi/pendingInputActions` (the input
steps a build waits at, asked only of a build whose stages say it waits,
while it is shown), and, only from the buttons, `POST <job>/build` or
`<job>/buildWithParameters`, `POST <build>/stop`, and an input step's
`POST <build>/input/<id>/proceedEmpty`, `…/submit` (`json` with the values
and `proceed`, the button) or `…/abort`, with the crumb from
`crumbIssuer/api/json` when the Jenkins has one.

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
Jenkins with canned jobs and two users (`jane:fake-token` may build, abort
and answer an input, `viewer:viewer-token` may only read). Its Pipeline jobs have stages,
with Pipeline Stage View's `wfapi` in the shapes a real Jenkins gives and a
console with the markers a real one writes: in `team-dev` a running build,
a failed one, an aborted one, an unstable one and one with a stage skipped;
in `team-prod` two that wait at an input step, `alarm-notify_master` at one
that asks nothing and `exporter_master` at release notes that ask for three
values, with `wfapi/pendingInputActions` and the step's answers;
`nightly-cleanup` is freestyle. `tests/fixtures/real-input.json` is what a
real Jenkins served for an answered release notes input (read only).
`node tests/fake-jenkins.mjs
48765` serves it for a look at the mod in a window: point the settings at
`http://127.0.0.1:48765`. It fails on demand: `curl -X POST
'http://127.0.0.1:48765/__fake/fail?mode=status&status=503'` (or
`mode=refuse`, `mode=stall`; `&count=2` for the next two requests,
`&path=<regex>` for some paths), and `/__fake/recover` ends it. Never try
Build, Abort or an input's answer against a real Jenkins from a test.

The icons are drawn after [Lucide](https://lucide.dev) (ISC license).
