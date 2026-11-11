use std::os::unix::{fs::PermissionsExt, net::UnixListener};

use super::*;
use crate::test_support::ScratchDir;

/// The sidebar and a little of the rest of the `main` Herdr 0.1.14 ships.
fn main_layout() -> Value {
    json!({
        "id": "root", "type": "row", "titlebar": "none",
        "children": [
            { "id": "sidebar", "type": "column", "size": { "px": 268 }, "children": [
                { "id": "brand", "type": "block", "content": { "mod": "workspace-chrome" } },
                { "id": "sidebar-lists", "type": "column", "children": [
                    { "id": "spaces", "type": "block", "size": { "weight": 1 }, "min": 90,
                      "content": { "mod": "spaces" } },
                    { "id": "agents", "type": "block", "size": { "weight": 1 }, "min": 90,
                      "content": { "mod": "agents" } } ] },
                { "id": "sidebar-footer", "type": "block",
                  "content": { "mod": "workspace-chrome", "props": { "footer": true } } } ] },
            { "id": "work", "type": "column", "children": [
                { "id": "tabs", "type": "block", "content": { "mod": "tabs" } },
                { "id": "work-body", "type": "row", "children": [
                    { "id": "main", "type": "block", "content": { "tab": "active" } },
                    { "id": "panel", "type": "column", "hidden": true, "children": [
                        { "id": "panel-body", "type": "stack", "active": "files", "children": [
                            { "id": "files", "type": "block", "content": { "mod": "files" } },
                            { "id": "changes", "type": "block", "content": { "mod": "changes" } }
                        ] } ] } ] } ] } ]
    })
}

/// The lists of a layout's sidebar, as `main_layout` holds them.
fn lists(layout: &Value) -> &Value {
    &layout["children"][0]["children"][1]["children"]
}

/// A fake herdr-app in a scratch home holding Herdr's shipped layouts. It
/// answers `status` with the layout `shared.json` names, `do layout.clone`
/// by copying `main.json` to `corgi.json` with a `cloned_from`, and `do
/// layout.switch` by writing `shared.json`; every call goes to `calls.log`.
/// The window is open while `window` holds its socket.
struct Fake {
    _dir: ScratchDir,
    app: HerdrApp,
    window: Option<UnixListener>,
}

const SCRIPT: &str = r#"#!/bin/sh
h=$HERDR_APP_HOME
printf '%s\n' "$*" >>"$h/calls.log"
layout() { sed -n 's/.*"layout":"\([^"]*\)".*/\1/p' "$h/shared.json"; }
case $1 in
  status) printf '{"layout":"%s"}\n' "$(layout)" ;;
  do)
    case $2 in
      layout.clone)
        sed 's/"id":"root",/"id":"root","cloned_from":{"name":"main","version":"0.1.14"},"clicks":"kept",/' \
          "$h/layouts/main.json" >"$h/layouts/corgi.json"
        printf '{"layout":"corgi"}\n' >"$h/shared.json" ;;
      layout.switch)
        name=$(printf '%s' "$3" | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
        printf '{"layout":"%s"}\n' "$name" >"$h/shared.json" ;;
    esac
    echo '{"ok": true}' ;;
  *) exit 2 ;;
esac
"#;

impl Fake {
    fn new(label: &str) -> Self {
        let dir = ScratchDir::new(label);
        let bin = dir.join("herdr-app");
        fs::write(&bin, SCRIPT).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let home = dir.join("home");
        fs::create_dir_all(home.join("layouts")).unwrap();
        fs::write(
            home.join("installed.json"),
            r#"{"layout:main":{},"layout:terminals":{},"mod:spaces":{}}"#,
        )
        .unwrap();
        fs::write(
            home.join("update-notes.json"),
            r#"{"last_run":"0.1.14","presented":[]}"#,
        )
        .unwrap();
        let fake = Self {
            app: HerdrApp::new(bin, &home),
            window: None,
            _dir: dir,
        };
        fake.put("main", &main_layout());
        fake.shows("main");
        fake
    }

    fn open(mut self) -> Self {
        self.window = Some(UnixListener::bind(self.app.home.join("herdr-app.sock")).unwrap());
        self
    }

    fn home(&self) -> &Path {
        &self.app.home
    }

    /// The window shows (or showed last) the layout `name`.
    fn shows(&self, name: &str) {
        fs::write(
            self.home().join("shared.json"),
            format!(r#"{{"layout":"{name}"}}"#),
        )
        .unwrap();
    }

    fn put(&self, name: &str, layout: &Value) {
        // Compact, as the fake's sed expects.
        fs::write(layout_path(self.home(), name), layout.to_string()).unwrap();
    }

    fn get(&self, name: &str) -> Value {
        read_json(&layout_path(self.home(), name)).unwrap()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.home().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|call| *call != "status")
            .map(str::to_string)
            .collect()
    }

    fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = fs::read_dir(self.home().join("layouts"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        files
    }
}

fn done(installed: Installed) -> Done {
    match installed {
        Installed::Done(done) => done,
        refused => panic!("{refused:?}"),
    }
}

/// What herdr-gitlab's install.py does to the layout `name`: the tab bar
/// shows its clone of `tabs`, and a GitLab block joins `panel-body`.
fn add_gitlab(layout: &mut Value) {
    find_mut(layout, "tabs").unwrap()["content"]["mod"] = "tabs-gitlab".into();
    find_mut(layout, "panel-body").unwrap()["children"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "gitlab", "type": "block", "content": { "mod": "gitlab" } }));
}

/// The content of a block showing the `corgi` mod's view `view`.
fn corgi(view: &str) -> Value {
    json!({ "mod": "corgi", "props": { "view": view } })
}

/// `main` with Corgi's one change: Projects for Spaces, no Agents, and
/// Corgi's brand row.
fn with_projects(mut layout: Value) -> Value {
    *find_mut(&mut layout, "sidebar-lists").unwrap() = json!({
        "id": "sidebar-lists", "type": "column", "children": [
            { "id": "spaces", "type": "block", "size": { "weight": 1 }, "min": 90,
              "content": corgi("projects") } ] });
    find_mut(&mut layout, "brand").unwrap()["content"] = corgi("brand");
    layout
}

/// `main` as Corgi changed it while it had a mod per block: `projects`
/// for Spaces, no Agents, and `corgi-brand` for the brand row.
fn with_old_mods(mut layout: Value) -> Value {
    *find_mut(&mut layout, "sidebar-lists").unwrap() = json!({
        "id": "sidebar-lists", "type": "column", "children": [
            { "id": "spaces", "type": "block", "size": { "weight": 1 }, "min": 90,
              "content": { "mod": "projects" } } ] });
    find_mut(&mut layout, "brand").unwrap()["content"]["mod"] = "corgi-brand".into();
    layout
}

/// The content of the block `id` of `layout`.
fn content_of<'a>(layout: &'a mut Value, id: &str) -> &'a Value {
    &find_mut(layout, id).unwrap()["content"]
}

/// The mod a node of `layout` shows.
fn mod_of<'a>(layout: &'a mut Value, id: &str) -> &'a str {
    shown_mod(find_mut(layout, id).unwrap()).unwrap()
}

#[test]
fn the_brand_row_is_corgis_once_and_the_footer_and_other_mods_stay() {
    let mut layout = main_layout();
    add_gitlab(&mut layout);
    assert_eq!(put_projects(&mut layout), Ok(Change::Edited));
    assert_eq!(*content_of(&mut layout, "brand"), corgi("brand"));
    // The footer is the same mod's, and stays Herdr's.
    assert_eq!(mod_of(&mut layout, "sidebar-footer"), "workspace-chrome");
    assert_eq!(
        find_mut(&mut layout, "sidebar-footer").unwrap()["content"]["props"],
        json!({ "footer": true })
    );
    assert_eq!(mod_of(&mut layout, "tabs"), "tabs-gitlab");
    assert_eq!(mod_of(&mut layout, "gitlab"), "gitlab");
    let mut expected = with_projects(main_layout());
    add_gitlab(&mut expected);
    assert_eq!(layout, expected);
    // Again: nothing to do.
    assert_eq!(put_projects(&mut layout), Ok(Change::Unchanged));
    assert_eq!(layout, expected);

    // A layout Corgi changed before it had a brand row gets it, once.
    let mut older = with_projects(main_layout());
    find_mut(&mut older, "brand").unwrap()["content"] = json!({ "mod": "workspace-chrome" });
    add_gitlab(&mut older);
    assert_eq!(put_projects(&mut older), Ok(Change::Edited));
    assert_eq!(older, expected);
    assert_eq!(put_projects(&mut older), Ok(Change::Unchanged));

    // A brand row of someone else's mod is theirs.
    let mut theirs = main_layout();
    find_mut(&mut theirs, "brand").unwrap()["content"]["mod"] = "my-brand".into();
    assert_eq!(put_projects(&mut theirs), Ok(Change::Edited));
    assert_eq!(mod_of(&mut theirs, "brand"), "my-brand");
}

#[test]
fn a_layout_without_a_brand_block_gets_the_projects_list_alone() {
    let mut layout = main_layout();
    let sidebar = &mut layout["children"][0]["children"];
    sidebar.as_array_mut().unwrap().remove(0);
    let before = layout.clone();
    assert_eq!(put_projects(&mut layout), Ok(Change::Edited));
    assert_eq!(count_view(&layout, View::Brand), 0);
    assert_eq!(count_view(&layout, View::Projects), 1);
    assert_eq!(count(&layout, "corgi"), 1);
    assert_eq!(count(&layout, "workspace-chrome"), 1, "the footer");
    assert_eq!(put_projects(&mut layout), Ok(Change::Unchanged));
    // And remove puts back what it had, no brand row added.
    assert!(take_projects(&mut layout, |_| json!({
        "id": "agents", "type": "block", "size": { "weight": 1 }, "min": 90,
        "content": { "mod": "agents" } })));
    assert_eq!(layout, before);
}

#[test]
fn the_sidebar_gets_projects_for_spaces_and_loses_agents_and_nothing_else_changes() {
    let mut layout = main_layout();
    assert_eq!(put_projects(&mut layout), Ok(Change::Edited));
    assert_eq!(layout, with_projects(main_layout()));
    // Again: nothing to do.
    assert_eq!(put_projects(&mut layout), Ok(Change::Unchanged));
    assert_eq!(layout, with_projects(main_layout()));

    // A sidebar with Projects is left whole, even an Agents list put back.
    let mut kept = with_projects(main_layout());
    lists_mut(&mut kept).push(json!({ "id": "agents", "type": "block",
                                      "content": { "mod": "agents" } }));
    let before = kept.clone();
    assert_eq!(put_projects(&mut kept), Ok(Change::Unchanged));
    assert_eq!(kept, before);
}

fn lists_mut(layout: &mut Value) -> &mut Vec<Value> {
    layout["children"][0]["children"][1]["children"]
        .as_array_mut()
        .unwrap()
}

#[test]
fn a_layout_with_no_list_to_replace_is_left_alone_and_says_why() {
    let mut no_lists = main_layout();
    lists_mut(&mut no_lists).clear();
    let before = no_lists.clone();
    let why = put_projects(&mut no_lists).unwrap_err();
    assert!(
        why.contains("no Spaces or Projects list in its sidebar"),
        "{why}"
    );
    assert_eq!(no_lists, before);

    // A Spaces list outside the sidebar is not the sidebar's.
    let mut elsewhere = main_layout();
    lists_mut(&mut elsewhere)[0]["content"]["mod"] = "files".into();
    find_mut(&mut elsewhere, "tabs").unwrap()["content"]["mod"] = "spaces".into();
    assert!(put_projects(&mut elsewhere).is_err());

    let mut two = main_layout();
    lists_mut(&mut two)
        .push(json!({ "id": "more", "type": "block", "content": { "mod": "spaces" } }));
    let why = put_projects(&mut two).unwrap_err();
    assert!(why.contains("2 Spaces lists"), "{why}");

    // A layout of one's own with no node called sidebar: its Spaces list.
    let mut bare = json!({ "id": "root", "type": "row", "children": [
        { "id": "left", "type": "block", "content": { "mod": "spaces" } },
        { "id": "main", "type": "block", "content": { "tab": "active" } } ] });
    assert_eq!(put_projects(&mut bare), Ok(Change::Edited));
    assert_eq!(bare["children"][0]["content"], corgi("projects"));
}

#[test]
fn on_a_shipped_layout_with_the_window_open_the_app_clones_it_to_corgi() {
    let fake = Fake::new("app-layout-clone").open();
    let done = done(install(&fake.app, None).unwrap());
    assert_eq!(
        done,
        Done {
            layout: "corgi".into(),
            change: Change::Edited,
            instead_of: Some("main".into()),
            cloned: true,
            shown: true,
            window: true,
        }
    );
    assert_eq!(
        fake.calls(),
        [r#"do layout.clone {"as":"corgi","name":"main"}"#]
    );
    // The clone the app wrote, clicks and all, with Corgi's change alone.
    let mut expected = with_projects(main_layout());
    expected["cloned_from"] = json!({ "name": "main", "version": "0.1.14" });
    expected["clicks"] = "kept".into();
    assert_eq!(fake.get("corgi"), expected);
    // Herdr's own is untouched, and nothing else is written beside it.
    assert_eq!(fake.get("main"), main_layout());
    assert_eq!(fake.files(), ["corgi.json", "main.json"]);

    // Again: the window is on corgi, which is the person's now.
    assert_eq!(install(&fake.app, None).unwrap().word(), "unchanged");
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn on_a_shipped_layout_with_the_window_closed_corgi_clones_it_and_says_so() {
    let fake = Fake::new("app-layout-clone-closed");
    let done = done(install(&fake.app, None).unwrap());
    assert!(done.cloned);
    assert_eq!((done.shown, done.window), (false, false));
    assert!(fake.calls().is_empty());
    let clone = fake.get("corgi");
    let mut expected = with_projects(main_layout());
    expected["cloned_from"] = json!({ "name": "main", "version": "0.1.14" });
    assert_eq!(clone, expected);
    let text = fs::read_to_string(layout_path(fake.home(), "corgi")).unwrap();
    assert!(text.ends_with("}\n"));
    // The window still shows main next time; nothing was switched for it.
    assert_eq!(current_layout(&fake.app).unwrap(), "main");

    // Run again with the window still closed, on main: corgi is used.
    let again = done_of(&fake);
    assert_eq!((again.cloned, again.change), (false, Change::Unchanged));
    assert_eq!(again.instead_of.as_deref(), Some("main"));
}

fn done_of(fake: &Fake) -> Done {
    done(install(&fake.app, None).unwrap())
}

#[test]
fn a_corgi_layout_that_is_there_is_used_and_switched_to() {
    let fake = Fake::new("app-layout-reuse").open();
    let mut own = main_layout();
    add_gitlab(&mut own);
    fake.put("corgi", &own);
    let done = done_of(&fake);
    assert_eq!(
        (done.cloned, done.change, done.shown),
        (false, Change::Edited, true)
    );
    assert_eq!(fake.calls(), [r#"do layout.switch {"name":"corgi"}"#]);
    let mut expected = with_projects(main_layout());
    add_gitlab(&mut expected);
    assert_eq!(fake.get("corgi"), expected);
}

#[test]
fn a_personal_layout_is_edited_in_place_once_and_keeps_everything_else() {
    let fake = Fake::new("app-layout-personal").open();
    let mut own = main_layout();
    add_gitlab(&mut own);
    own["children"][0]["size"]["px"] = 320.into();
    fake.put("mine", &own);
    fake.shows("mine");

    let done = done_of(&fake);
    assert_eq!(
        done,
        Done {
            layout: "mine".into(),
            change: Change::Edited,
            instead_of: None,
            cloned: false,
            shown: true,
            window: true,
        }
    );
    let mut expected = with_projects(own.clone());
    expected["children"][0]["size"]["px"] = 320.into();
    assert_eq!(fake.get("mine"), expected);
    assert!(fake.calls().is_empty(), "nothing cloned or switched");
    assert_eq!(fake.files(), ["main.json", "mine.json"]);

    // Idempotent: the same file, not even written again.
    let path = layout_path(fake.home(), "mine");
    let before = fs::read_to_string(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(done_of(&fake).change, Change::Unchanged);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
}

#[test]
fn a_layout_named_is_the_one_edited_and_the_window_is_not_switched_to_it() {
    let fake = Fake::new("app-layout-named").open();
    fake.put("other", &main_layout());
    let done = done(install(&fake.app, Some("other")).unwrap());
    assert_eq!(done.layout, "other");
    assert!(!done.shown);
    assert!(fake.calls().is_empty());
    assert_eq!(fake.get("other"), with_projects(main_layout()));

    let missing = install(&fake.app, Some("nope")).unwrap();
    assert_eq!(missing.word(), "refused");
    assert!(matches!(&missing, Installed::Refused { why, .. } if why.contains("no layout")));
}

#[test]
fn a_layout_without_a_sidebar_to_change_is_left_alone() {
    let fake = Fake::new("app-layout-refused").open();
    let mut odd = main_layout();
    lists_mut(&mut odd).clear();
    fake.put("main", &odd);
    // Shipped: no clone is made of a layout Corgi could not change.
    let refused = install(&fake.app, None).unwrap();
    assert_eq!(refused.word(), "refused");
    assert_eq!(refused.layout(), "main");
    assert!(fake.calls().is_empty());
    assert_eq!(fake.files(), ["main.json"]);

    // Personal: the file is as it was.
    fake.put("mine", &odd);
    fake.shows("mine");
    let text = fs::read_to_string(layout_path(fake.home(), "mine")).unwrap();
    assert_eq!(install(&fake.app, None).unwrap().word(), "refused");
    assert_eq!(
        fs::read_to_string(layout_path(fake.home(), "mine")).unwrap(),
        text
    );
}

#[test]
fn herdr_gitlab_after_corgi_and_corgi_after_herdr_gitlab_keep_both() {
    // Corgi first, then herdr-gitlab edits the personal corgi in place,
    // then Corgi's install runs again (an update).
    let fake = Fake::new("app-layout-gitlab-after").open();
    assert!(done_of(&fake).cloned);
    let mut layout = fake.get("corgi");
    add_gitlab(&mut layout);
    fake.put("corgi", &layout);
    assert_eq!(done_of(&fake).change, Change::Unchanged);
    let after = fake.get("corgi");
    assert_eq!(after, layout);
    assert_eq!(count(&after, "gitlab"), 1);
    assert_eq!(count_view(&after, View::Projects), 1);
    assert_eq!(count(&after, "agents"), 0);

    // herdr-gitlab first: it clones main to main-gitlab and switches to it,
    // and Corgi then edits that in place.
    let fake = Fake::new("app-layout-gitlab-before").open();
    let mut clone = main_layout();
    clone["cloned_from"] = json!({ "name": "main", "version": "0.1.14" });
    add_gitlab(&mut clone);
    fake.put("main-gitlab", &clone);
    fake.shows("main-gitlab");
    let done = done_of(&fake);
    assert_eq!(
        (done.layout.as_str(), done.change, done.cloned),
        ("main-gitlab", Change::Edited, false)
    );
    assert_eq!(fake.get("main-gitlab"), with_projects(clone));
    assert_eq!(fake.files(), ["main-gitlab.json", "main.json"]);
    let edited = fake.get("main-gitlab");
    assert_eq!(find_block(&edited, "gitlab").unwrap()["id"], "gitlab");
    assert_eq!(find_block(&edited, "tabs-gitlab").unwrap()["id"], "tabs");
}

#[test]
fn restore_puts_spaces_and_agents_back_where_projects_was() {
    let fake = Fake::new("app-layout-restore").open();
    assert!(done_of(&fake).cloned);
    let mut layout = fake.get("corgi");
    add_gitlab(&mut layout);
    fake.put("corgi", &layout);
    let mut mine = with_projects(main_layout());
    mine["children"][0]["size"]["px"] = 300.into();
    fake.put("mine", &mine);
    fake.put("plain", &json!({ "id": "root", "type": "row" }));

    // A layout with Corgi's brand row alone (its lists put back by hand).
    let mut brand_only = main_layout();
    find_mut(&mut brand_only, "brand").unwrap()["content"] = corgi("brand");
    fake.put("brand-only", &brand_only);

    assert_eq!(
        restore(fake.home(), None).unwrap(),
        ["brand-only", "corgi", "mine"]
    );
    assert_eq!(fake.get("brand-only"), main_layout());
    let mut restored = fake.get("corgi");
    assert_eq!(lists(&restored), lists(&main_layout()));
    assert_eq!(mod_of(&mut restored, "brand"), "workspace-chrome");
    assert_eq!(mod_of(&mut restored, "sidebar-footer"), "workspace-chrome");
    assert_eq!(mod_of(&mut restored, "tabs"), "tabs-gitlab");
    assert_eq!(count(&restored, "corgi"), 0);
    assert_eq!(count(&restored, "gitlab"), 1, "herdr-gitlab's panel stays");
    assert!(restored.get("cloned_from").is_some());
    let mut mine = fake.get("mine");
    assert_eq!(lists(&mine), lists(&main_layout()));
    assert_eq!(mine["children"][0]["size"]["px"], 300);
    assert_eq!(mod_of(&mut mine, "brand"), "workspace-chrome");
    // No layout is deleted, and the window is not switched.
    assert_eq!(
        fake.files(),
        [
            "brand-only.json",
            "corgi.json",
            "main.json",
            "mine.json",
            "plain.json"
        ]
    );
    assert!(fake.calls().iter().all(|c| !c.contains("switch")));
    // Nothing left to restore.
    assert!(restore(fake.home(), None).unwrap().is_empty());

    // An Agents list the person kept, and a Spaces list besides Projects.
    let mut both = main_layout();
    lists_mut(&mut both).insert(
        0,
        json!({ "id": "p", "type": "block", "content": corgi("projects") }),
    );
    fake.put("both", &both);
    assert_eq!(restore(fake.home(), Some("both")).unwrap(), ["both"]);
    assert_eq!(fake.get("both"), main_layout());

    // An id that is taken gets another.
    let mut taken = with_projects(main_layout());
    find_mut(&mut taken, "brand").unwrap()["id"] = "agents".into();
    let block = agents_block(fake.home(), &taken);
    assert_eq!(block["id"], "agents-2");
    assert_eq!(block["content"]["mod"], "agents");
}

#[test]
fn shipped_and_current_layouts_come_from_the_apps_files() {
    let fake = Fake::new("app-layout-files");
    assert!(is_shipped(fake.home(), "main"));
    assert!(!is_shipped(fake.home(), "win98"), "by installed.json");
    assert!(!is_shipped(fake.home(), "corgi"));
    fs::remove_file(fake.home().join("installed.json")).unwrap();
    assert!(is_shipped(fake.home(), "win98"), "Herdr's names without it");

    fake.shows("mine");
    assert_eq!(current_layout(&fake.app).unwrap(), "mine");
    fs::remove_file(fake.home().join("shared.json")).unwrap();
    assert_eq!(current_layout(&fake.app).unwrap(), "main");
}

#[test]
fn a_layout_with_corgis_old_mods_gets_the_one_mod_in_their_blocks() {
    // The sidebar of an earlier install: `projects` and `corgi-brand`.
    let mut old = with_old_mods(main_layout());
    add_gitlab(&mut old);
    old["children"][0]["size"]["px"] = 300.into();
    let mut expected = with_projects(main_layout());
    add_gitlab(&mut expected);
    expected["children"][0]["size"]["px"] = 300.into();
    assert_eq!(put_projects(&mut old), Ok(Change::Upgraded));
    assert_eq!(
        old, expected,
        "the same blocks, ids and sizes, showing the one mod"
    );
    assert_eq!(put_projects(&mut old), Ok(Change::Unchanged));
    assert_eq!(old, expected);

    // Without the brand row: the Projects list's block alone changes, and
    // workspace-chrome's row becomes Corgi's.
    let mut half = with_old_mods(main_layout());
    find_mut(&mut half, "brand").unwrap()["content"] = json!({ "mod": "workspace-chrome" });
    assert_eq!(put_projects(&mut half), Ok(Change::Upgraded));
    assert_eq!(half, with_projects(main_layout()));

    // A brand row alone, its lists Herdr's: both are Corgi's after.
    let mut brand_only = main_layout();
    find_mut(&mut brand_only, "brand").unwrap()["content"]["mod"] = "corgi-brand".into();
    assert_eq!(put_projects(&mut brand_only), Ok(Change::Edited));
    assert_eq!(brand_only, with_projects(main_layout()));

    // A layout refused for want of a list keeps even its old blocks as
    // they were.
    let mut odd = main_layout();
    lists_mut(&mut odd).clear();
    find_mut(&mut odd, "brand").unwrap()["content"]["mod"] = "corgi-brand".into();
    let before = odd.clone();
    assert!(put_projects(&mut odd).is_err());
    assert_eq!(odd, before);
}

#[test]
fn a_view_is_read_from_the_blocks_props_and_its_other_props_stay() {
    let block = |content: Value| json!({ "id": "b", "type": "block", "content": content });
    assert_eq!(shown_view(&block(corgi("projects"))), Some(View::Projects));
    assert_eq!(shown_view(&block(corgi("brand"))), Some(View::Brand));
    // The mod's own default.
    assert_eq!(
        shown_view(&block(json!({ "mod": "corgi" }))),
        Some(View::Projects)
    );
    // A view of the mod's that is not the sidebar's is not Corgi's to move.
    assert_eq!(shown_view(&block(corgi("questions"))), None);
    assert_eq!(
        shown_view(&block(json!({ "mod": "projects" }))),
        Some(View::Projects)
    );
    assert_eq!(
        shown_view(&block(json!({ "mod": "corgi-brand" }))),
        Some(View::Brand)
    );
    assert_eq!(shown_view(&block(json!({ "mod": "spaces" }))), None);

    // A brand block with props of its own keeps them both ways.
    let mut layout = main_layout();
    find_mut(&mut layout, "brand").unwrap()["content"]["props"] = json!({ "compact": true });
    let before = layout.clone();
    assert_eq!(put_projects(&mut layout), Ok(Change::Edited));
    assert_eq!(
        *content_of(&mut layout, "brand"),
        json!({ "mod": "corgi", "props": { "compact": true, "view": "brand" } })
    );
    assert!(take_projects(&mut layout, |_| json!({
        "id": "agents", "type": "block", "size": { "weight": 1 }, "min": 90,
        "content": { "mod": "agents" } })));
    assert_eq!(layout, before);
}

#[test]
fn restore_gives_back_herdrs_sidebar_from_the_old_mods_too() {
    let fake = Fake::new("app-layout-restore-old");
    let mut old = with_old_mods(main_layout());
    add_gitlab(&mut old);
    fake.put("old", &old);
    let mut mixed = with_projects(main_layout());
    find_mut(&mut mixed, "brand").unwrap()["content"]["mod"] = "corgi-brand".into();
    fake.put("mixed", &mixed);
    assert_eq!(restore(fake.home(), None).unwrap(), ["mixed", "old"]);
    let mut expected = main_layout();
    add_gitlab(&mut expected);
    assert_eq!(fake.get("old"), expected);
    assert_eq!(fake.get("mixed"), main_layout());
    assert!(restore(fake.home(), None).unwrap().is_empty());
}

#[test]
fn every_personal_layout_naming_the_old_mods_gets_the_one_mod_and_nothing_else() {
    let fake = Fake::new("app-layout-upgrade");
    let mut other = with_old_mods(main_layout());
    add_gitlab(&mut other);
    other["children"][0]["size"]["px"] = 300.into();
    fake.put("other", &other);
    // A layout with its lists put back by hand and the old brand row.
    let mut brand_only = main_layout();
    find_mut(&mut brand_only, "brand").unwrap()["content"]["mod"] = "corgi-brand".into();
    fake.put("brand-only", &brand_only);
    fake.put("done", &with_projects(main_layout()));
    // Herdr's own is never edited, whatever it names.
    fake.put("main", &with_old_mods(main_layout()));

    assert_eq!(
        upgrade_layouts(fake.home()).unwrap(),
        ["brand-only", "other"]
    );
    let mut expected = with_projects(main_layout());
    add_gitlab(&mut expected);
    expected["children"][0]["size"]["px"] = 300.into();
    assert_eq!(fake.get("other"), expected);
    // Only the old blocks change: its lists stay Herdr's.
    let mut brand = main_layout();
    find_mut(&mut brand, "brand").unwrap()["content"] = corgi("brand");
    assert_eq!(fake.get("brand-only"), brand);
    assert_eq!(fake.get("done"), with_projects(main_layout()));
    assert_eq!(fake.get("main"), with_old_mods(main_layout()));
    assert!(upgrade_layouts(fake.home()).unwrap().is_empty());
}
