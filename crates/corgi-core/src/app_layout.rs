//! The Corgi layout of the Herdr app (herdr-app): `layouts/corgi.json` in
//! the app's home, which the app lists as "Corgi" (a layout is called after
//! its file). It is Herdr's shipped `main` layout with two changes in its
//! sidebar: the block that shows Herdr's `spaces` mod shows Corgi's
//! `projects` mod instead, and the block that shows its `agents` mod is
//! gone. Everything else is as Herdr ships it.
//!
//! Herdr replaces its shipped layouts whole on every start, and moves a
//! hand-edited one aside, so Corgi never edits `main`: it builds a layout of
//! its own from it ([`build`]), at install and again whenever `main` has
//! changed ([`refresh`], which the Corgi mod runs when it starts). The build
//! is a function of `main` alone, so comparing what it gives with the file
//! that is there says whether `main` changed since the last build. A `main`
//! whose sidebar is not the shape Corgi knows is not guessed at: the last
//! good build stays, and the caller is told why.
//!
//! Nothing here switches the window to the layout: that is the person's
//! choice (`herdr-app do layout.switch '{"name":"corgi"}'`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::app_space::HerdrApp;

/// The layout's name: `layouts/corgi.json`, shown as "Corgi".
pub const LAYOUT: &str = "corgi";
/// The shipped layout it is built from.
pub const SOURCE: &str = "main";
/// Corgi's mod that takes the place of Herdr's `spaces`:
/// `herdr-app/mods/projects`.
pub const PROJECTS_MOD: &str = "projects";
/// The node of `main` that holds the lists.
const SIDEBAR: &str = "sidebar";
const SPACES_MOD: &str = "spaces";
const AGENTS_MOD: &str = "agents";

/// What [`refresh`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refreshed {
    /// There was no Corgi layout, and now there is one.
    Made,
    /// `main` changed since the last build, and the layout follows it.
    Rebuilt,
    /// The layout is what `main` gives.
    Unchanged,
    /// No layout could be built, and why: the last build is kept as it is.
    Kept(String),
    /// No layout could be built, and why, and there is none from before.
    Missing(String),
}

impl Refreshed {
    /// The word `corgi app-layout --json` prints.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Made => "made",
            Self::Rebuilt => "rebuilt",
            Self::Unchanged => "unchanged",
            Self::Kept(_) => "kept",
            Self::Missing(_) => "missing",
        }
    }

    /// Why no layout was built, when none was.
    pub fn problem(&self) -> Option<&str> {
        match self {
            Self::Kept(why) | Self::Missing(why) => Some(why),
            _ => None,
        }
    }
}

/// Where the Corgi layout is kept in the app home `home`.
pub fn layout_path(home: &Path) -> PathBuf {
    home.join("layouts").join(format!("{LAYOUT}.json"))
}

fn source_path(home: &Path) -> PathBuf {
    home.join("layouts").join(format!("{SOURCE}.json"))
}

/// The Corgi layout made from Herdr's `main`, or why `main` is not the
/// shape Corgi knows: a node `sidebar` holding exactly one block for the
/// `spaces` mod and exactly one for the `agents` mod.
pub fn build(main: &Value) -> Result<Value, String> {
    let mut layout = main.clone();
    let sidebar = find_mut(&mut layout, SIDEBAR)
        .ok_or_else(|| format!("Herdr's {SOURCE} layout has no node called {SIDEBAR}"))?;
    let (spaces, agents) = (count(sidebar, SPACES_MOD), count(sidebar, AGENTS_MOD));
    if spaces != 1 || agents != 1 {
        return Err(format!(
            "the sidebar of Herdr's {SOURCE} layout has {spaces} {SPACES_MOD} and {agents} \
             {AGENTS_MOD} blocks, where Corgi knows one of each"
        ));
    }
    remove(sidebar, AGENTS_MOD);
    retarget(sidebar, SPACES_MOD, PROJECTS_MOD);
    Ok(layout)
}

/// The mod a node shows, when it is a block showing one.
fn shown_mod(node: &Value) -> Option<&str> {
    node.get("content")?.get("mod")?.as_str()
}

fn children(node: &Value) -> &[Value] {
    node.get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn children_mut(node: &mut Value) -> Option<&mut Vec<Value>> {
    node.get_mut("children")?.as_array_mut()
}

/// The node with id `id` in the tree under `node`.
fn find_mut<'a>(node: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return Some(node);
    }
    children_mut(node)?
        .iter_mut()
        .find_map(|child| find_mut(child, id))
}

/// How many blocks under `node` show the mod `name`.
fn count(node: &Value, name: &str) -> usize {
    usize::from(shown_mod(node) == Some(name))
        + children(node)
            .iter()
            .map(|child| count(child, name))
            .sum::<usize>()
}

/// Takes the blocks that show the mod `name` out of the tree under `node`.
fn remove(node: &mut Value, name: &str) {
    if let Some(children) = children_mut(node) {
        children.retain(|child| shown_mod(child) != Some(name));
        for child in children {
            remove(child, name);
        }
    }
}

/// Makes the blocks under `node` that show the mod `from` show `to`. The
/// block keeps its id, size and everything else, so what names it (a key
/// that toggles it, the person's sizes) still does.
fn retarget(node: &mut Value, from: &str, to: &str) {
    if shown_mod(node) == Some(from)
        && let Some(content) = node.get_mut("content")
    {
        content["mod"] = Value::String(to.to_string());
    }
    if let Some(children) = children_mut(node) {
        for child in children {
            retarget(child, from, to);
        }
    }
}

/// Builds the Corgi layout in the app home `home` from the `main` layout
/// there, and writes it when it differs from the one in place. It writes
/// nothing when `main` cannot be read or has another shape than Corgi
/// knows, or when the `projects` mod the layout shows is not installed in
/// that home.
pub fn refresh(home: &Path) -> Refreshed {
    let path = layout_path(home);
    let current = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let had = current.is_some();
    let failed = |why: String| match had {
        true => Refreshed::Kept(why),
        false => Refreshed::Missing(why),
    };
    let projects = home.join("mods").join(PROJECTS_MOD).join("main.js");
    if !projects.is_file() {
        return failed(format!(
            "Corgi's Projects mod is not installed ({} is missing); run Corgi's \
             herdr-app/install.sh",
            projects.display()
        ));
    }
    let source = source_path(home);
    let main = match fs::read_to_string(&source) {
        Ok(text) => text,
        Err(error) => {
            return failed(format!(
                "Herdr's {SOURCE} layout could not be read ({}: {error})",
                source.display()
            ));
        }
    };
    let main: Value = match serde_json::from_str(&main) {
        Ok(main) => main,
        Err(error) => {
            return failed(format!(
                "Herdr's {SOURCE} layout is not JSON ({}: {error})",
                source.display()
            ));
        }
    };
    let layout = match build(&main) {
        Ok(layout) => layout,
        Err(why) => return failed(why),
    };
    if current.as_ref() == Some(&layout) {
        return Refreshed::Unchanged;
    }
    match write(&path, &layout) {
        Ok(()) if had => Refreshed::Rebuilt,
        Ok(()) => Refreshed::Made,
        Err(error) => failed(format!("could not write {}: {error}", path.display())),
    }
}

/// Writes `layout` to `path` in one step: the app reads a layout the moment
/// its file changes, and must never read half of one.
fn write(path: &Path, layout: &Value) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(layout)?;
    text.push('\n');
    // Not `*.json`, so the app does not take the unfinished file for a layout.
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Whether the file at `path` is a layout Corgi built: one that shows the
/// `projects` mod. Only such a file is ever removed.
fn is_corgis(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|layout| count(&layout, PROJECTS_MOD) > 0)
}

/// Removes the Corgi layout from the home of `app`: whether there was one
/// to remove. A `corgi.json` Corgi did not build is left. A window that
/// shows the layout is switched to Herdr's `main` first, so it is not left
/// showing a file that is gone.
pub fn remove_layout(app: &HerdrApp) -> Result<bool> {
    let path = layout_path(&app.home);
    if !is_corgis(&path) {
        return Ok(false);
    }
    if app.shown_layout()?.as_deref() == Some(LAYOUT) {
        app.command("layout.switch", json!({ "name": SOURCE }))?;
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
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
                    { "id": "main", "type": "block", "content": { "tab": "active" } } ] } ]
        })
    }

    /// An app home holding `main` and the `projects` mod.
    fn home(label: &str, main: Option<&Value>) -> ScratchDir {
        let home = ScratchDir::new(label);
        fs::create_dir_all(home.join("layouts")).unwrap();
        fs::create_dir_all(home.join("mods/projects")).unwrap();
        fs::write(home.join("mods/projects/main.js"), "// the mod").unwrap();
        if let Some(main) = main {
            set_main(&home, main);
        }
        home
    }

    fn set_main(home: &Path, main: &Value) {
        fs::write(source_path(home), main.to_string()).unwrap();
    }

    fn built(home: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(layout_path(home)).unwrap()).unwrap()
    }

    #[test]
    fn the_layout_is_main_with_projects_for_spaces_and_no_agents() {
        let main = main_layout();
        let layout = build(&main).unwrap();
        let lists = &layout["children"][0]["children"][1];
        assert_eq!(
            lists["children"],
            json!([{ "id": "spaces", "type": "block", "size": { "weight": 1 }, "min": 90,
                     "content": { "mod": "projects" } }])
        );
        // Everything else is Herdr's, untouched.
        let mut expected = main.clone();
        expected["children"][0]["children"][1]["children"] = lists["children"].clone();
        assert_eq!(layout, expected);
        assert_eq!(count(&layout, AGENTS_MOD), 0);
        // The source is not changed.
        assert_eq!(main, main_layout());
    }

    #[test]
    fn a_sidebar_of_another_shape_is_not_guessed_at() {
        let mut none = main_layout();
        none["children"][0]["id"] = "left".into();
        assert!(build(&none).unwrap_err().contains("no node called sidebar"));

        let mut no_agents = main_layout();
        no_agents["children"][0]["children"][1]["children"]
            .as_array_mut()
            .unwrap()
            .pop();
        let why = build(&no_agents).unwrap_err();
        assert!(why.contains("1 spaces and 0 agents blocks"), "{why}");

        // A spaces block outside the sidebar is not the sidebar's.
        let mut elsewhere = main_layout();
        elsewhere["children"][0]["children"][1]["children"][0]["content"]["mod"] = "files".into();
        elsewhere["children"][1]["children"][0]["content"]["mod"] = "spaces".into();
        let why = build(&elsewhere).unwrap_err();
        assert!(why.contains("0 spaces and 1 agents blocks"), "{why}");
    }

    #[test]
    fn a_refresh_makes_the_layout_once_and_rebuilds_it_when_main_changes() {
        let home = home("app-layout", Some(&main_layout()));
        assert_eq!(refresh(&home), Refreshed::Made);
        assert_eq!(built(&home), build(&main_layout()).unwrap());
        let text = fs::read_to_string(layout_path(&home)).unwrap();
        assert!(text.ends_with("}\n"), "{text}");
        // Nothing but the layout is written beside main.
        let mut files: Vec<String> = fs::read_dir(home.join("layouts"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["corgi.json", "main.json"]);

        // The same main again writes nothing: the app reloads a changed file.
        let before = fs::metadata(layout_path(&home))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(refresh(&home), Refreshed::Unchanged);
        let after = fs::metadata(layout_path(&home))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);

        // Herdr ships a new main: the layout follows it.
        let mut newer = main_layout();
        newer["children"][0]["size"]["px"] = 300.into();
        newer["children"][1]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "status", "type": "block", "content": { "mod": "status" } }));
        set_main(&home, &newer);
        assert_eq!(refresh(&home), Refreshed::Rebuilt);
        assert_eq!(built(&home), build(&newer).unwrap());
        assert_eq!(built(&home)["children"][0]["size"]["px"], 300);
    }

    #[test]
    fn a_main_corgi_cannot_build_from_keeps_the_last_build_and_says_why() {
        let home = home("app-layout-kept", Some(&main_layout()));
        assert_eq!(refresh(&home), Refreshed::Made);
        let good = fs::read_to_string(layout_path(&home)).unwrap();

        let mut broken = main_layout();
        broken["children"][0]["children"][1]["children"] = json!([]);
        set_main(&home, &broken);
        let kept = refresh(&home);
        assert_eq!(kept.word(), "kept");
        assert!(
            kept.problem()
                .unwrap()
                .contains("0 spaces and 0 agents blocks"),
            "{kept:?}"
        );
        assert_eq!(fs::read_to_string(layout_path(&home)).unwrap(), good);

        fs::write(source_path(&home), "{ not json").unwrap();
        let kept = refresh(&home);
        assert!(matches!(&kept, Refreshed::Kept(why) if why.contains("is not JSON")));
        fs::remove_file(source_path(&home)).unwrap();
        let kept = refresh(&home);
        assert!(matches!(&kept, Refreshed::Kept(why) if why.contains("could not be read")));
        assert_eq!(fs::read_to_string(layout_path(&home)).unwrap(), good);

        // A good main again is built from.
        set_main(&home, &main_layout());
        assert_eq!(refresh(&home), Refreshed::Unchanged);
    }

    #[test]
    fn with_no_build_to_keep_or_no_projects_mod_there_is_no_layout() {
        let home = home("app-layout-missing", None);
        let missing = refresh(&home);
        assert_eq!(missing.word(), "missing");
        assert!(missing.problem().unwrap().contains("could not be read"));
        assert!(!layout_path(&home).exists());

        // The layout would show a mod the app does not have.
        set_main(&home, &main_layout());
        fs::remove_file(home.join("mods/projects/main.js")).unwrap();
        let missing = refresh(&home);
        assert!(
            matches!(&missing, Refreshed::Missing(why) if why.contains("Projects mod is not installed")),
            "{missing:?}"
        );
        assert!(!layout_path(&home).exists());
    }

    #[test]
    fn only_a_layout_corgi_built_is_removed() {
        let home = home("app-layout-remove", Some(&main_layout()));
        // No window is open in a scratch home, so none is asked or switched.
        let app = HerdrApp::new("/nonexistent/herdr-app", &*home);
        assert!(!remove_layout(&app).unwrap());
        assert_eq!(refresh(&home), Refreshed::Made);
        assert!(remove_layout(&app).unwrap());
        assert!(!layout_path(&home).exists());
        assert!(source_path(&home).exists());

        // Someone's own layout of the same name stays.
        fs::write(layout_path(&home), main_layout().to_string()).unwrap();
        assert!(!remove_layout(&app).unwrap());
        assert!(layout_path(&home).exists());
    }
}
