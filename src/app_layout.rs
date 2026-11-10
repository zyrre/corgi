//! Corgi's sidebar in the Herdr app's (herdr-app's) layout: Corgi's
//! `projects` mod where Herdr's `spaces` mod was, and no `agents` mod, since
//! the Projects list shows each project's agents; and the sidebar's `brand`
//! block, the row at its top, showing Corgi's `corgi-brand` ("Herdr +
//! Corgi") where Herdr's `workspace-chrome` showed "Herdr". That is the one
//! change Corgi makes to a layout, all of it in the left sidebar (its
//! footer stays `workspace-chrome`'s); everything else stays as it was
//! found, so other mods (herdr-gitlab's panel in `panel-body`, a cloned tab
//! bar) can change the same layout, before or after Corgi.
//!
//! A layout is a person's own (personal) or one Herdr ships
//! (`installed.json` lists them as `layout:<name>`). Herdr replaces a
//! shipped layout whole on every start and moves an edited one aside, so
//! Corgi edits only personal ones ([`install`]):
//!
//! - on a personal layout, it edits that layout in place;
//! - on a shipped one (`main`), it clones it to `corgi` as `herdr-app do
//!   layout.clone` does (which it runs when the window is open, so the
//!   clone gets the shipped layout's clicks too), edits the clone and
//!   switches the window to it. A `corgi` layout that is there already is
//!   used instead of a new clone.
//!
//! After that Corgi never builds a layout again and does not follow Herdr's
//! later changes to `main`: running [`install`] again only makes sure the
//! Projects list is there, and changes nothing when it is. A layout with no
//! Spaces or Projects list to put it in is left alone, with why.
//!
//! [`restore`] (`corgi app-layout remove`, which the uninstall runs) puts
//! Herdr's Spaces and Agents lists back where Corgi's Projects list is, and
//! Herdr's brand row where Corgi's is, so a layout does not go on naming a
//! mod that is gone. It deletes no layout.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::app_space::HerdrApp;

/// The layout Corgi clones a shipped one to: `layouts/corgi.json`, which
/// the app lists as "Corgi".
pub const LAYOUT: &str = "corgi";
/// Herdr's default layout, the one shown when the app has no other.
pub const DEFAULT: &str = "main";
/// Corgi's mod that takes the place of Herdr's `spaces`:
/// `herdr-app/mods/projects`.
pub const PROJECTS_MOD: &str = "projects";
/// Corgi's mod for the sidebar's brand row ("Herdr + Corgi"):
/// `herdr-app/mods/corgi-brand`.
pub const BRAND_MOD: &str = "corgi-brand";
/// The block of Herdr's layouts that holds the brand row, and the mod that
/// draws it there (and the sidebar's footer, which Corgi leaves).
const BRAND: &str = "brand";
const CHROME_MOD: &str = "workspace-chrome";
/// The node of Herdr's layouts that holds the lists.
const SIDEBAR: &str = "sidebar";
const SPACES_MOD: &str = "spaces";
const AGENTS_MOD: &str = "agents";
/// The layouts Herdr 0.1.14 ships, for a home whose `installed.json`
/// cannot be read.
const SHIPPED: [&str; 3] = ["main", "terminals", "win98"];
/// How long `layout.clone` may take to write its file.
const CLONE_WAIT: Duration = Duration::from_secs(5);

/// What one layout needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// It had the Projects list already.
    Unchanged,
    /// The Projects list was put in.
    Edited,
}

/// A layout that has the Projects list now, and how it came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub layout: String,
    pub change: Change,
    /// The shipped layout asked for, when Corgi went to its own `corgi`
    /// instead.
    pub instead_of: Option<String>,
    /// Whether `corgi` was cloned from it by this run.
    pub cloned: bool,
    /// Whether the window shows the layout (or was switched to it), or,
    /// when it is closed, opens on it.
    pub shown: bool,
    /// Whether the window is open.
    pub window: bool,
}

/// What [`install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    Done(Done),
    /// Nothing was changed, and why.
    Refused {
        layout: String,
        why: String,
    },
}

impl Installed {
    /// The word `corgi app-layout --json` prints.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Done(Done { cloned: true, .. }) => "cloned",
            Self::Done(Done {
                change: Change::Edited,
                ..
            }) => "edited",
            Self::Done(_) => "unchanged",
            Self::Refused { .. } => "refused",
        }
    }

    /// The layout it is about.
    pub fn layout(&self) -> &str {
        match self {
            Self::Done(Done { layout, .. }) | Self::Refused { layout, .. } => layout,
        }
    }
}

/// Where the layout `name` is kept in the app home `home`.
pub fn layout_path(home: &Path, name: &str) -> PathBuf {
    home.join("layouts").join(format!("{name}.json"))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Whether the layout `name` is one Herdr ships, by the app's own record of
/// what it installed.
pub fn is_shipped(home: &Path, name: &str) -> bool {
    match read_json(&home.join("installed.json")) {
        Some(Value::Object(installed)) => installed.contains_key(&format!("layout:{name}")),
        _ => SHIPPED.contains(&name),
    }
}

/// The layout the window shows: `herdr-app status` while it is open, else
/// the one it showed last (`shared.json`, which is `shared("layout")`),
/// else Herdr's default.
pub fn current_layout(app: &HerdrApp) -> Result<String> {
    if let Some(shown) = app.shown_layout()? {
        return Ok(shown);
    }
    Ok(read_json(&app.home.join("shared.json"))
        .and_then(|shared| shared.get("layout")?.as_str().map(str::to_string))
        .unwrap_or_else(|| DEFAULT.to_string()))
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

fn has_id(node: &Value, id: &str) -> bool {
    node.get("id").and_then(Value::as_str) == Some(id)
        || children(node).iter().any(|child| has_id(child, id))
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

/// The first block under `node` that shows the mod `name`.
fn find_block<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    if shown_mod(node) == Some(name) {
        return Some(node);
    }
    children(node)
        .iter()
        .find_map(|child| find_block(child, name))
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

/// Makes the `brand` block under `node` show Corgi's brand row when it
/// shows `workspace-chrome`'s: whether it changed. No `brand` block, or one
/// showing another mod (Corgi's already, or someone's own), is left alone.
fn put_brand(node: &mut Value) -> bool {
    match find_mut(node, BRAND) {
        Some(brand) if shown_mod(brand) == Some(CHROME_MOD) => {
            brand["content"]["mod"] = Value::String(BRAND_MOD.to_string());
            true
        }
        _ => false,
    }
}

/// Puts Corgi's sidebar into `layout`: the one `spaces` block of its
/// sidebar (the node `sidebar`, or the whole layout when it has none) shows
/// `projects` instead, the sidebar's `agents` blocks go, and its `brand`
/// block shows `corgi-brand` (see [`put_brand`]). A sidebar that shows
/// `projects` already keeps its lists as they are, whatever else it holds.
/// `Err` says why there is nowhere to put the Projects list; the layout is
/// then unchanged.
pub fn put_projects(layout: &mut Value) -> Result<Change, String> {
    let has_sidebar = has_id(layout, SIDEBAR);
    let sidebar = match find_mut(layout, SIDEBAR) {
        Some(sidebar) => sidebar,
        None => layout,
    };
    if count(sidebar, PROJECTS_MOD) > 0 {
        // A layout from before Corgi had a brand row gets it now.
        return Ok(if put_brand(sidebar) {
            Change::Edited
        } else {
            Change::Unchanged
        });
    }
    let place = if has_sidebar {
        "its sidebar"
    } else {
        "it, which has no sidebar"
    };
    match count(sidebar, SPACES_MOD) {
        1 => {}
        0 => {
            return Err(format!(
                "there is no Spaces or Projects list in {place} to put Corgi's Projects list in"
            ));
        }
        n => {
            return Err(format!(
                "there are {n} Spaces lists in {place}, and Corgi would not guess which one \
                 its Projects list replaces"
            ));
        }
    }
    retarget(sidebar, SPACES_MOD, PROJECTS_MOD);
    remove(sidebar, AGENTS_MOD);
    put_brand(sidebar);
    Ok(Change::Edited)
}

/// The Agents block to put back: the one in Herdr's shipped `main` when it
/// has one, else the one Herdr 0.1.14 ships. Its id is `agents`, or another
/// one when `layout` already has a node of that id.
fn agents_block(home: &Path, layout: &Value) -> Value {
    let mut block = read_json(&layout_path(home, DEFAULT))
        .and_then(|main| find_block(&main, AGENTS_MOD).cloned())
        .unwrap_or_else(|| {
            json!({ "id": "agents", "type": "block", "size": { "weight": 1 }, "min": 90,
                    "content": { "mod": "agents" } })
        });
    let base = block
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("agents")
        .to_string();
    let mut id = base.clone();
    let mut n = 2;
    while has_id(layout, &id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    block["id"] = Value::String(id);
    block
}

/// Inserts `block` right after the first block under `node` that shows
/// `after`; gives it back when there is none.
fn insert_after(node: &mut Value, after: &str, block: Value) -> Option<Value> {
    let Some(children) = children_mut(node) else {
        return Some(block);
    };
    if let Some(at) = children.iter().position(|c| shown_mod(c) == Some(after)) {
        children.insert(at + 1, block);
        return None;
    }
    children
        .iter_mut()
        .try_fold(block, |block, child| insert_after(child, after, block))
}

/// Takes Corgi's sidebar out of `layout` again: the Projects list's block
/// shows `spaces` again (or goes, when the layout has a Spaces list besides
/// it), an `agents` block (`agents`) follows the Spaces list when the layout
/// has none, and every block showing Corgi's brand row shows
/// `workspace-chrome` again. Whether there was anything of Corgi's to take
/// out.
fn take_projects(layout: &mut Value, agents: impl FnOnce(&Value) -> Value) -> bool {
    let brand = count(layout, BRAND_MOD) > 0;
    retarget(layout, BRAND_MOD, CHROME_MOD);
    if count(layout, PROJECTS_MOD) == 0 {
        return brand;
    }
    if count(layout, SPACES_MOD) == 0 {
        // The first goes back to Spaces; any other was a second list.
        retarget_first(layout, PROJECTS_MOD, SPACES_MOD);
    }
    remove(layout, PROJECTS_MOD);
    if count(layout, AGENTS_MOD) == 0 {
        let block = agents(layout);
        insert_after(layout, SPACES_MOD, block);
    }
    true
}

/// Makes the first block under `node` that shows `from` show `to`: whether
/// there was one.
fn retarget_first(node: &mut Value, from: &str, to: &str) -> bool {
    if shown_mod(node) == Some(from) {
        node["content"]["mod"] = Value::String(to.to_string());
        return true;
    }
    children_mut(node).is_some_and(|children| {
        children
            .iter_mut()
            .any(|child| retarget_first(child, from, to))
    })
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

fn read_layout(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))
}

/// Edits the personal layout `name` in place.
fn edit(home: &Path, name: &str) -> Result<Result<Change, String>> {
    let path = layout_path(home, name);
    if !path.is_file() {
        return Ok(Err(format!("there is no layout {}", path.display())));
    }
    let mut layout = read_layout(&path)?;
    let change = match put_projects(&mut layout) {
        Ok(change) => change,
        Err(why) => return Ok(Err(why)),
    };
    if change == Change::Edited {
        write(&path, &layout).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(Ok(change))
}

/// Herdr's version, for a clone's `cloned_from`: the version the app last
/// ran (`update-notes.json`), else what its binary says.
fn herdr_version(app: &HerdrApp) -> Option<String> {
    if let Some(version) = read_json(&app.home.join("update-notes.json"))
        .and_then(|notes| notes.get("last_run")?.as_str().map(str::to_string))
    {
        return Some(version);
    }
    let out = std::process::Command::new(&app.bin)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.split_whitespace().nth(1).map(str::to_string)
}

/// The shipped layout `from` cloned to [`LAYOUT`] by hand, for a closed
/// window: a copy that records `cloned_from`, as the app's clone does.
fn clone_by_hand(app: &HerdrApp, from: &str, layout: &Value) -> Value {
    let mut clone = layout.clone();
    if let (Some(version), Some(fields)) = (herdr_version(app), clone.as_object_mut()) {
        fields.insert(
            "cloned_from".into(),
            json!({ "name": from, "version": version }),
        );
    }
    clone
}

/// Waits for the file `layout.clone` writes.
fn wait_for(path: &Path) -> Result<Value> {
    let deadline = Instant::now() + CLONE_WAIT;
    loop {
        if let Some(layout) = read_json(path) {
            return Ok(layout);
        }
        if Instant::now() >= deadline {
            bail!(
                "herdr-app do layout.clone wrote no {} within {}s",
                path.display(),
                CLONE_WAIT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Puts Corgi's Projects list into the Herdr app's layout `name`, or the
/// one the window shows (see the module's documentation): edits it in
/// place when it is personal; for a shipped one, clones it to [`LAYOUT`]
/// (or uses the `corgi` layout that is there), edits that, and switches an
/// open window to it.
pub fn install(app: &HerdrApp, name: Option<&str>) -> Result<Installed> {
    let home = app.home.as_path();
    let current = current_layout(app)?;
    let open = app.window_open();
    let name = name.unwrap_or(&current).to_string();
    let shown = |layout: &str| layout == current;

    if !is_shipped(home, &name) {
        return Ok(match edit(home, &name)? {
            Ok(change) => Installed::Done(Done {
                shown: shown(&name),
                window: open,
                layout: name,
                change,
                instead_of: None,
                cloned: false,
            }),
            Err(why) => Installed::Refused { layout: name, why },
        });
    }
    if is_shipped(home, LAYOUT) {
        return Ok(Installed::Refused {
            why: format!("{name} is Herdr's own layout, and so is {LAYOUT}, to clone it to"),
            layout: name,
        });
    }
    let target = layout_path(home, LAYOUT);
    let mut done = Done {
        layout: LAYOUT.into(),
        change: Change::Edited,
        instead_of: Some(name.clone()),
        cloned: false,
        shown: shown(LAYOUT),
        window: open,
    };
    if target.exists() {
        // A `corgi` layout from before: the person's, used as it is.
        match edit(home, LAYOUT)? {
            Ok(change) => done.change = change,
            Err(why) => {
                return Ok(Installed::Refused {
                    layout: LAYOUT.into(),
                    why,
                });
            }
        }
    } else {
        // Nothing is cloned that Corgi could not edit after.
        let mut edited = read_layout(&layout_path(home, &name))?;
        if let Err(why) = put_projects(&mut edited) {
            return Ok(Installed::Refused { layout: name, why });
        }
        let by_app = open
            && app
                .command("layout.clone", json!({ "name": name, "as": LAYOUT }))?
                .is_some();
        if by_app {
            // The app wrote the clone, with the clicks of the one it came
            // from, and shows it.
            let mut clone = wait_for(&target)?;
            if let Err(why) = put_projects(&mut clone) {
                return Ok(Installed::Refused {
                    layout: LAYOUT.into(),
                    why,
                });
            }
            write(&target, &clone).with_context(|| format!("write {}", target.display()))?;
            done.shown = true;
        } else {
            let clone = clone_by_hand(app, &name, &edited);
            write(&target, &clone).with_context(|| format!("write {}", target.display()))?;
            done.shown = false;
        }
        done.cloned = true;
    }
    if !done.shown
        && open
        && app
            .command("layout.switch", json!({ "name": LAYOUT }))?
            .is_some()
    {
        done.shown = true;
    }
    Ok(Installed::Done(done))
}

/// The personal layouts in `home` that show Corgi's Projects list or its
/// brand row.
fn layouts_with_projects(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(home.join("layouts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let name = name.strip_suffix(".json")?.to_string();
            let layout = read_json(&entry.path())?;
            let corgis = count(&layout, PROJECTS_MOD) + count(&layout, BRAND_MOD);
            (corgis > 0 && !is_shipped(home, &name)).then_some(name)
        })
        .collect();
    names.sort();
    names
}

/// Puts Herdr's Spaces and Agents lists and its brand row back in the
/// layout `name`, or in every personal layout that shows Corgi's Projects
/// list or brand row: the names of
/// those it changed. No layout is deleted and the window is not switched.
pub fn restore(home: &Path, name: Option<&str>) -> Result<Vec<String>> {
    let names = match name {
        Some(name) => vec![name.to_string()],
        None => layouts_with_projects(home),
    };
    let mut restored = Vec::new();
    for name in names {
        let path = layout_path(home, &name);
        if !path.is_file() {
            continue;
        }
        let mut layout = read_layout(&path)?;
        if take_projects(&mut layout, |layout| agents_block(home, layout)) {
            write(&path, &layout).with_context(|| format!("write {}", path.display()))?;
            restored.push(name);
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests;
