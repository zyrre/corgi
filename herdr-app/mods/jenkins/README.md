# Jenkins for the Herdr app

A mod for the [Herdr](https://herdr.dev) app that shows your Jenkins jobs and
builds and lets you act on them: the jobs of the folders you name with their
last build's state, a job's recent builds, a build's console log (followed
while it runs), and Build (with a form for the job's parameters) and Abort,
each asked about first.

It reaches Jenkins with **an API token alone**: no password, no SSO, no
`jenkins-cli`. **The token's Jenkins user decides what the mod may do.** Give
it the token of a user who may only read, and it only reads; what Jenkins
refuses shows as "This token may not build …", and nothing works around it.

It stands on its own: it needs no Corgi and no other mod, and Corgi's
`herdr-app/install.sh` does not install it.

## Install

```sh
herdr-app/mods/jenkins/install.sh              # into ~/.config/herdr-app (or $HERDR_APP_HOME)
herdr-app/mods/jenkins/install.sh --home DIR   # into another app home
herdr-app/mods/jenkins/install.sh --uninstall  # --forget also removes the settings and the token
```

It copies the mod to `mods/jenkins` in the app's home, after `herdr-app check`
has passed it, and changes no layout and no key. Run it again to update.
Then show it where you like:

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

- The list: each folder's jobs with a mark for the last build (success,
  failed, unstable, aborted, never built, disabled), its number and age; a
  running build shows how far it is by Jenkins' estimate. A folder inside
  opens as a list of its own. Read again every minute, every ten seconds
  while a build runs or waits; the header's arrows read it now.
- Click a job for its last 20 builds, a build for its console log. A running
  build's log is followed; scrolling up stops that, `Follow` takes it up
  again. Only the last 5000 lines are kept.
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
`<build>/logText/progressiveText?start=<byte>`, and, only from the two
buttons, `POST <job>/build` or `<job>/buildWithParameters` and
`POST <build>/stop`, with the crumb from `crumbIssuer/api/json` when the
Jenkins has one.

## Develop

`main.js` (the views), `api.js` (Jenkins and the settings), `util.js` (pure
helpers), `ui.js` (marks and buttons), `icons/`. After a change:

```sh
herdr-app check herdr-app/mods/jenkins
node herdr-app/mods/jenkins/tests/test.mjs
```

The tests run the mod under Node against `tests/fake-jenkins.mjs`, a made-up
Jenkins with canned jobs and two users (`jane:fake-token` may build and
abort, `viewer:viewer-token` may only read). `node tests/fake-jenkins.mjs
48765` serves it for a look at the mod in a window: point the settings at
`http://127.0.0.1:48765`. Never try Build or Abort against a real Jenkins
from a test.

The icons are drawn after [Lucide](https://lucide.dev) (ISC license).
