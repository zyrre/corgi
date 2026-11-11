//! Corgi's Questions panel in the Herdr app's layout: the `corgi` mod's
//! `questions` view as a child of the right-hand panel stack `panel-body`
//! (beside Files, Changes and other mods' panels), and a "Questions" button
//! in the tab bar that shows it there, with the count of open questions.
//!
//! The panel is one block of the layout, `corgi-questions`, which
//! [`put_questions`] adds and [`take_questions`] takes out with the rest of
//! Corgi's. Its id was never another block's: the app keeps a block's
//! `hidden` and `size` by id after the block left the layout.
//!
//! The button lives in the tab bar's mod, which the layout's `tabs` block
//! names. Herdr's own `tabs` is never edited (Herdr replaces it on every
//! start): [`put_button`] clones it to `tabs-corgi`, as `herdr-app do
//! mod.clone` would, with a first line recording what it held, and points
//! the layout's tab bar at the clone. A tab bar that is a copy already
//! (herdr-gitlab's `tabs-gitlab`, the Jenkins mod's `tabs-jenkins`, the
//! person's own) is edited in place. Either way two places change, each
//! between marker lines, so that other installers' marked lines (Jenkins')
//! stay as they are and [`take_button`] takes out exactly Corgi's:
//!
//! - just before the tab bar's call that remembers per space which panel
//!   was open (`this.syncPanel(…)`, or the marked lines another installer
//!   put in its place), a line adds `corgi-questions` to that call's list
//!   while the layout's stack has the panel;
//! - just before the window buttons (`.when(w?.buttons,`), the button: the
//!   tab bar's own `panelButton`, as Files' and Changes', with the count the
//!   Corgi mod publishes in `shared("corgi.questions.count")` as a pill.
//!
//! [`take_button`] puts the clone's layouts back on `tabs`, and removes the
//! clone once no layout names it, only while the clone is as it was cloned
//! but for Corgi's lines; one another installer changed stays, without
//! Corgi's lines.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{CORGI_MOD, children, children_mut, find_mut, layout_path, read_json, read_layout};

/// The panel's block id, and the view of the Corgi mod it shows.
pub const QUESTIONS_BLOCK: &str = "corgi-questions";
pub const QUESTIONS_VIEW: &str = "questions";
/// The column and the stack of Herdr's layouts that hold the panels.
const COLUMN: &str = "panel";
const STACK: &str = "panel-body";
/// The block that shows the tab bar, Herdr's mod for it and Corgi's clone.
const TABS_BLOCK: &str = "tabs";
pub const STOCK: &str = "tabs";
pub const CLONE: &str = "tabs-corgi";
/// The first line of the clone's main.js: Corgi made it, and what its
/// main.js held then without this line and Corgi's lines (a hash).
const CLONED: &str = "// Herdr's tabs, cloned by corgi app-layout for Corgi's Questions button; \
                      corgi app-layout remove puts the layout back on tabs while the rest is as \
                      cloned. As cloned: ";
const LIST_BEGIN: &str = "// >>> Corgi questions panel";
const LIST_END: &str = "// <<< Corgi questions panel";
const BUTTON_BEGIN: &str = "// >>> Corgi questions button";
const BUTTON_END: &str = "// <<< Corgi questions button";
/// The button goes just before the window buttons.
const ANCHOR: &str = ".when(w?.buttons,";
/// The call that remembers the open panel per space.
const SYNC: &str = "this.syncPanel(";
/// What the lines use of the tab bar around them, as Herdr's has them.
const NEEDS: [(&str, &str); 5] = [
    ("find", "find("),
    ("body", "body = find("),
    ("panelButton", "panelButton = "),
    ("enabled", "enabled = "),
    ("shared", "shared"),
];
const BY_HAND: &str = "the Questions button was not added (the panel is in the layout, and \
                       the Projects list's counts open it)";

// --- the panel block ---------------------------------------------------------------

/// Whether `node` is a block showing the Corgi mod's Questions panel.
fn shows_questions(node: &Value) -> bool {
    let Some(content) = node.get("content") else {
        return false;
    };
    content.get("mod").and_then(Value::as_str) == Some(CORGI_MOD)
        && content
            .get("props")
            .and_then(|p| p.get("view"))
            .and_then(Value::as_str)
            == Some(QUESTIONS_VIEW)
}

/// How many blocks under `node` show the Questions panel.
pub fn count_questions(node: &Value) -> usize {
    usize::from(shows_questions(node)) + children(node).iter().map(count_questions).sum::<usize>()
}

/// Puts the Questions panel last into the layout's panel stack
/// (`panel-body` in the column `panel`): whether it was added. A layout
/// with the panel already, or without such a stack, is left alone.
pub fn put_questions(layout: &mut Value) -> bool {
    if count_questions(layout) > 0 {
        return false;
    }
    let Some(column) = find_mut(layout, COLUMN) else {
        return false;
    };
    let Some(stack) = find_mut(column, STACK) else {
        return false;
    };
    if stack.get("type").and_then(Value::as_str) != Some("stack") {
        return false;
    }
    let Some(children) = children_mut(stack) else {
        return false;
    };
    children.push(json!({
        "id": QUESTIONS_BLOCK,
        "type": "block",
        "content": { "mod": CORGI_MOD, "props": { "view": QUESTIONS_VIEW } }
    }));
    true
}

/// Takes every block showing the Questions panel out of `node`; a stack
/// that showed one shows its first child left. Whether there was one.
pub fn take_questions(node: &mut Value) -> bool {
    let Some(children) = children_mut(node) else {
        return false;
    };
    let gone: Vec<String> = children
        .iter()
        .filter(|child| shows_questions(child))
        .filter_map(|child| child.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let before = children.len();
    children.retain(|child| !shows_questions(child));
    let mut taken = children.len() != before;
    let first = children
        .iter()
        .find_map(|child| child.get("id").and_then(Value::as_str).map(str::to_string));
    for child in children.iter_mut() {
        taken |= take_questions(child);
    }
    if let Some(active) = node.get("active").and_then(Value::as_str)
        && gone.iter().any(|id| id == active)
        && let Some(fields) = node.as_object_mut()
    {
        match first {
            Some(first) => {
                fields.insert("active".into(), Value::String(first));
            }
            None => {
                fields.remove("active");
            }
        }
    }
    taken
}

// --- the tab bar's lines -------------------------------------------------------------

/// The lines between Corgi's markers `begin` and `end` in `text`, as a byte
/// range of whole lines; `Err` for markers that do not pair up.
fn marked(text: &str, begin: &str, end: &str) -> Result<Option<(usize, usize)>, String> {
    let mut at = 0;
    let (mut start, mut stop) = (Vec::new(), Vec::new());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with(begin) {
            start.push(at);
        } else if trimmed.starts_with(end) {
            stop.push(at + line.len());
        }
        at += line.len();
    }
    match (start.as_slice(), stop.as_slice()) {
        ([], []) => Ok(None),
        ([s], [e]) if s < e => Ok(Some((*s, *e))),
        _ => Err(format!(
            "it has Corgi's marker lines ({begin}) that do not pair up: fix them by hand"
        )),
    }
}

/// `text` without Corgi's lines.
pub fn without_button(text: &str) -> Result<String, String> {
    let mut text = text.to_string();
    for (begin, end) in [(LIST_BEGIN, LIST_END), (BUTTON_BEGIN, BUTTON_END)] {
        if let Some((start, stop)) = marked(&text, begin, end)? {
            text.replace_range(start..stop, "");
        }
    }
    Ok(text)
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// The line that adds the panel to the list the tab bar remembers per space.
fn list_lines(indent: &str) -> String {
    [
        format!(
            "{LIST_BEGIN}: corgi app-layout adds the Questions panel to the panels remembered per \
             space; corgi app-layout remove takes it out"
        ),
        format!(
            "try {{ if (find(body, \"{QUESTIONS_BLOCK}\") && !enabled.includes(\"{QUESTIONS_BLOCK}\")) \
             enabled.push(\"{QUESTIONS_BLOCK}\"); }} catch (_) {{}}"
        ),
        LIST_END.to_string(),
    ]
    .iter()
    .map(|line| format!("{indent}{line}\n"))
    .collect()
}

/// The button, shown while the layout's stack has the panel.
fn button_lines(indent: &str) -> String {
    [
        format!(
            "{BUTTON_BEGIN}: corgi app-layout puts it here; corgi app-layout remove takes it out"
        ),
        format!(
            ".when(Boolean(find(body, \"{QUESTIONS_BLOCK}\")), el => el.child(panelButton(\"{QUESTIONS_BLOCK}\", \
             \"Questions\", \"mods/corgi/question.svg\")"
        ),
        "  .when(Number(shared(\"corgi.questions.count\")) > 0, b => b.child(div().px(5).rounded(4)\
         .bg(t.accent).text_color(t.accent_foreground).text_size(10.5).font_semibold()\
         .child(String(Number(shared(\"corgi.questions.count\"))))))))"
            .to_string(),
        BUTTON_END.to_string(),
    ]
    .iter()
    .map(|line| format!("{indent}{line}\n"))
    .collect()
}

/// `text` (a tab bar's main.js) with Corgi's lines in it, put in afresh.
/// `Err` says why they cannot go in; nothing is to be written then.
pub fn with_button(text: &str) -> Result<String, String> {
    let mut text = without_button(text)?;
    if let Some((name, _)) = NEEDS.iter().find(|(_, needle)| !text.contains(needle)) {
        return Err(format!(
            "it is not like Herdr's tab bar: the button needs {name} there"
        ));
    }
    // The list: before the syncPanel call, and before any comment lines
    // just above it (another installer's marker and kept line).
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let calls: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with(SYNC))
        .map(|(n, _)| n)
        .collect();
    let [call] = calls.as_slice() else {
        return Err(
            "it is not like Herdr's tab bar: no single this.syncPanel(…) call, which remembers \
             the open panel per space"
                .into(),
        );
    };
    let mut at = *call;
    while at > 0 && lines[at - 1].trim_start().starts_with("//") {
        at -= 1;
    }
    let offset: usize = lines[..at].iter().map(|line| line.len()).sum();
    text.insert_str(offset, &list_lines(indent_of(lines[*call])));
    // The button: just before the window buttons.
    let anchors: Vec<usize> = text.match_indices(ANCHOR).map(|(at, _)| at).collect();
    let [anchor] = anchors.as_slice() else {
        return Err(format!(
            "it is not like Herdr's tab bar: no single place before its window buttons ({ANCHOR})"
        ));
    };
    let line = text[..*anchor].rfind('\n').map_or(0, |n| n + 1);
    let indent = indent_of(&text[line..]).to_string();
    if text[line..*anchor].trim().is_empty() {
        text.insert_str(line, &button_lines(&indent));
    } else {
        // The anchor is not at the start of its line: the button starts one of its own.
        let lines = format!("\n{}{indent}", button_lines(&indent));
        text.insert_str(*anchor, &lines);
    }
    Ok(text)
}

// --- the clone of Herdr's tab bar ------------------------------------------------------

/// FNV-1a, 64 bits: whether a file is still what it was, not a secret.
fn digest(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Files the app writes into a mod folder, which are not the mod's.
fn app_written(name: &str) -> bool {
    name == "jsconfig.json" || name.ends_with(".d.ts") || name.starts_with('.')
}

/// The files of the folder `dir`, by path relative to it.
fn mod_files(dir: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(folder) = todo.pop() {
        for entry in fs::read_dir(&folder).with_context(|| format!("read {}", folder.display()))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if app_written(&name) {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                todo.push(path);
            } else {
                let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                files.push((path.strip_prefix(dir)?.to_path_buf(), data));
            }
        }
    }
    Ok(files)
}

/// Herdr's tab bar cloned, as `mod.clone` makes a clone: asset paths
/// rewritten to the clone's folder, `cloned_from` in mod.json, and
/// main.js's first line saying Corgi made it.
fn clone_files(mods: &Path, version: Option<&str>) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let stock = mods.join(STOCK);
    let mut files = mod_files(&stock)?;
    let mut meta = json!({});
    for (path, data) in &mut files {
        let name = path.to_string_lossy();
        if name.ends_with(".js") {
            let text = String::from_utf8_lossy(data)
                .replace(&format!("mods/{STOCK}/"), &format!("mods/{CLONE}/"));
            *data = if name == "main.js" {
                format!("{CLONED}{}\n{text}", digest(&text))
            } else {
                text
            }
            .into_bytes();
        } else if name == "mod.json" {
            meta = serde_json::from_slice(data).unwrap_or_else(|_| json!({}));
        }
    }
    anyhow::ensure!(
        files.iter().any(|(path, _)| path == Path::new("main.js")),
        "no main.js in {}, Herdr's tab bar",
        stock.display()
    );
    if !meta.is_object() {
        meta = json!({});
    }
    meta["cloned_from"] = match version {
        Some(version) => json!({ "name": STOCK, "version": version }),
        None => json!({ "name": STOCK }),
    };
    files.retain(|(path, _)| path != Path::new("mod.json"));
    let mut text = serde_json::to_string_pretty(&meta)?;
    text.push('\n');
    files.push((PathBuf::from("mod.json"), text.into_bytes()));
    Ok(files)
}

/// Where the clone is: absent, Corgi's and as cloned but for Corgi's
/// lines, Corgi's and changed by another, or someone else's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneState {
    None,
    AsCloned,
    Changed,
    Other,
}

pub fn clone_state(mods: &Path) -> CloneState {
    let dir = mods.join(CLONE);
    if !dir.is_dir() {
        return CloneState::None;
    }
    let Ok(text) = fs::read_to_string(dir.join("main.js")) else {
        return CloneState::Other;
    };
    let (first, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let Some(hash) = first.strip_prefix(CLONED) else {
        return CloneState::Other;
    };
    match without_button(rest) {
        Ok(rest) if digest(&rest) == hash => CloneState::AsCloned,
        _ => CloneState::Changed,
    }
}

/// Writes `files` as the folder `dir`, whole, with one rename.
fn put_folder(dir: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    let parent = dir.parent().context("a mod folder has a parent")?;
    let name = dir.file_name().context("a mod folder has a name")?;
    let stage = parent.join(format!(".{}.corgi-install", name.to_string_lossy()));
    let _ = fs::remove_dir_all(&stage);
    for (path, data) in files {
        let path = stage.join(path);
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        fs::write(&path, data).with_context(|| format!("write {}", path.display()))?;
    }
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    fs::rename(&stage, dir).with_context(|| format!("put {} in place", dir.display()))
}

/// Writes `text` to `path` in one step, keeping the file's mode.
fn write_text(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("js.corgi-install");
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    if let Ok(meta) = fs::metadata(path) {
        fs::set_permissions(&temporary, meta.permissions())?;
    }
    fs::rename(&temporary, path).with_context(|| format!("write {}", path.display()))
}

// --- putting the button in, and taking it out ------------------------------------------------

/// The block of the layout's tab bar and the mod it names.
fn tab_mod(layout: &Value) -> Option<String> {
    fn find(node: &Value) -> Option<&Value> {
        if node.get("id").and_then(Value::as_str) == Some(TABS_BLOCK) {
            return Some(node);
        }
        children(node).iter().find_map(find)
    }
    find(layout)?
        .get("content")?
        .get("mod")?
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn point_tabs(layout: &mut Value, to: &str) {
    if let Some(block) = find_mut(layout, TABS_BLOCK) {
        block["content"]["mod"] = Value::String(to.to_string());
    }
}

/// What [`put_button`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Button {
    /// The tab bar had it already, as it is now.
    Unchanged,
    /// It was put into the tab bar `mod` (cloned from Herdr's when `cloned`).
    Added { tab_bar: String, cloned: bool },
    /// The layout has no panel stack, so no panel and no button.
    NoPanel,
    /// It was not, and why; nothing was changed.
    Refused(String),
}

/// Puts the Questions button into the tab bar the personal layout `name`
/// in `home` shows, when the layout has the panel: in Herdr's `tabs`
/// cloned to [`CLONE`] (the layout then names the clone), or in place in
/// a copy. `check` is `herdr-app check` of a staged mod folder: `Err` with
/// what it said refuses the change. `version` is Herdr's, for the clone's
/// `cloned_from`.
pub fn put_button(
    home: &Path,
    name: &str,
    version: Option<&str>,
    check: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Button> {
    let path = layout_path(home, name);
    let mut layout = read_layout(&path)?;
    if count_questions(&layout) == 0 {
        return Ok(Button::NoPanel);
    }
    let Some(tab_bar) = tab_mod(&layout) else {
        return Ok(Button::Refused(format!(
            "the layout has no tab bar (a block {TABS_BLOCK}) to put a Questions button in"
        )));
    };
    let mods = home.join("mods");
    let refuse = |why: String| Ok(Button::Refused(format!("{why}; {BY_HAND}")));
    let (target, mut files, cloned) = if tab_bar == STOCK {
        match clone_state(&mods) {
            CloneState::Other => {
                return refuse(format!(
                    "mods/{CLONE} is there and is not Corgi's clone of Herdr's tab bar"
                ));
            }
            // Corgi's clone from before, for another layout: shared.
            CloneState::AsCloned | CloneState::Changed => {
                (CLONE.to_string(), mod_files(&mods.join(CLONE))?, false)
            }
            CloneState::None => (CLONE.to_string(), clone_files(&mods, version)?, true),
        }
    } else {
        (tab_bar.clone(), mod_files(&mods.join(&tab_bar))?, false)
    };
    let Some(main) = files
        .iter_mut()
        .find(|(path, _)| path == Path::new("main.js"))
    else {
        return refuse(format!("mods/{target} has no main.js"));
    };
    let text = String::from_utf8_lossy(&main.1).into_owned();
    let edited = match with_button(&text) {
        Ok(edited) => edited,
        Err(why) => return refuse(format!("mods/{target}/main.js: {why}")),
    };
    let unchanged = edited == text && tab_bar == target;
    if unchanged {
        return Ok(Button::Unchanged);
    }
    main.1 = edited.clone().into_bytes();
    let stage = home.join(format!(".{target}.corgi-check"));
    let _ = fs::remove_dir_all(&stage);
    put_folder(&stage, &files)?;
    let checked = check(&stage);
    let _ = fs::remove_dir_all(&stage);
    if let Err(said) = checked {
        return refuse(format!(
            "herdr-app check refused mods/{target} with the button: {said}"
        ));
    }
    if cloned {
        put_folder(&mods.join(&target), &files)?;
    } else {
        write_text(&mods.join(&target).join("main.js"), &edited)?;
    }
    if tab_bar != target {
        point_tabs(&mut layout, &target);
        super::write(&path, &layout).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(Button::Added {
        tab_bar: target,
        cloned,
    })
}

/// The tab bar mods the personal layouts in `home` name, by layout, and
/// whether each layout has the Questions panel.
fn tab_bars(home: &Path) -> Vec<(String, String, bool)> {
    super::personal_layouts(home, |_| true)
        .into_iter()
        .filter_map(|name| {
            let layout = read_json(&layout_path(home, &name))?;
            let bar = tab_mod(&layout)?;
            Some((name, bar, count_questions(&layout) > 0))
        })
        .collect()
}

/// Takes the Questions button out of the tab bar of the layout `name`,
/// once no personal layout that names the same tab bar has the panel:
/// Corgi's lines go, and a layout on Corgi's clone, still as cloned, goes
/// back to Herdr's `tabs` (the clone goes once no layout names it). Run
/// after the panel was taken out of `name`. What it did, for a person.
pub fn take_button(home: &Path, name: &str) -> Result<Option<String>> {
    let path = layout_path(home, name);
    let Some(mut layout) = read_json(&path) else {
        return Ok(None);
    };
    let Some(tab_bar) = tab_mod(&layout) else {
        return Ok(None);
    };
    let mods = home.join("mods");
    let bars = tab_bars(home);
    if bars.iter().any(|(_, bar, panel)| *bar == tab_bar && *panel) {
        return Ok(None);
    }
    if tab_bar == CLONE && clone_state(&mods) == CloneState::AsCloned {
        point_tabs(&mut layout, STOCK);
        super::write(&path, &layout).with_context(|| format!("write {}", path.display()))?;
        if !bars
            .iter()
            .any(|(other, bar, _)| other != name && bar == CLONE)
        {
            fs::remove_dir_all(mods.join(CLONE))
                .with_context(|| format!("remove {}", mods.join(CLONE).display()))?;
        }
        return Ok(Some(format!("{name} shows Herdr's tab bar again")));
    }
    let main = mods.join(&tab_bar).join("main.js");
    let Ok(text) = fs::read_to_string(&main) else {
        return Ok(None);
    };
    match without_button(&text) {
        Ok(without) if without != text => {
            write_text(&main, &without)?;
            Ok(Some(format!(
                "took the Questions button out of mods/{tab_bar}"
            )))
        }
        Ok(_) => Ok(None),
        Err(why) => Ok(Some(format!(
            "left mods/{tab_bar}/main.js as it was: {why}"
        ))),
    }
}

#[cfg(test)]
mod tests;
