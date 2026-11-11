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
herdr-app/mods/jenkins/install.sh --panel right       # and as a panel on the right of the window
herdr-app/mods/jenkins/install.sh --panel off         # takes the panel out again; the mod stays
herdr-app/mods/jenkins/install.sh --home DIR          # into another app home
herdr-app/mods/jenkins/install.sh --uninstall         # the mod, and its panel; --forget also removes the settings and the token
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

`main.js` (the views), `api.js` (Jenkins and the settings), `util.js` (pure
helpers), `ui.js` (marks and buttons), `icons/`. After a change:

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
`http://127.0.0.1:48765`. Never try Build or Abort against a real Jenkins
from a test.

The icons are drawn after [Lucide](https://lucide.dev) (ISC license).
