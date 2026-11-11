use super::*;
use crate::{
    app_layout::{restore, tests::main_layout},
    test_support::ScratchDir,
};

/// A stand-in for Herdr's `tabs` mod's main.js: the lines Corgi's edit
/// finds and uses, as Herdr 0.1.15 has them.
const STOCK_MAIN: &str = r#"import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { run, theme, window, shared, layout } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

export default class Tabs extends View {
  syncPanel(space, panel, body, enabled) {}
  render() {
    const t = theme();
    const stop = (_e, cx) => cx.stop_propagation();
    const root = layout(), w = window();
    const panel = find(root, "panel"), body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(undefined, panel, body, enabled);
    const panelButton = (id, label, icon) => h_flex().id(`panel-${id}`).child(svg(icon).size(14)).child(label);
    return h_flex().size_full()
      .when(enabled.includes("files"), el => el.child(panelButton("files", "Files", "mods/files/file.svg")))
      .when(w?.buttons, el => el.child(svg("mods/tabs/win-min.svg").size(16)));
  }
}
"#;

/// A scratch app home with Herdr's `tabs` mod and the personal layout
/// `name` holding `layout`.
fn home_with(label: &str, layouts: &[(&str, Value)]) -> ScratchDir {
    let home = ScratchDir::new(label);
    let tabs = home.join("mods").join(STOCK);
    fs::create_dir_all(&tabs).unwrap();
    fs::write(tabs.join("main.js"), STOCK_MAIN).unwrap();
    fs::write(
        tabs.join("mod.json"),
        r#"{ "place": "none", "title": "Tabs" }"#,
    )
    .unwrap();
    fs::write(tabs.join("win-min.svg"), "<svg/>").unwrap();
    fs::write(tabs.join("jsconfig.json"), "{}").unwrap();
    fs::create_dir_all(home.join("layouts")).unwrap();
    fs::write(home.join("installed.json"), r#"{"layout:main":{}}"#).unwrap();
    for (name, layout) in layouts {
        fs::write(layout_path(&home, name), layout.to_string()).unwrap();
    }
    home
}

fn with_panel() -> Value {
    let mut layout = main_layout();
    assert!(put_questions(&mut layout));
    layout
}

fn passes(_: &Path) -> Result<(), String> {
    Ok(())
}

fn tab_bar_of(home: &Path, name: &str) -> String {
    tab_mod(&read_json(&layout_path(home, name)).unwrap()).unwrap()
}

#[test]
fn the_panel_joins_the_stack_once_and_leaves_it_with_its_active_child() {
    let mut layout = main_layout();
    assert!(put_questions(&mut layout));
    assert!(!put_questions(&mut layout), "once");
    assert_eq!(count_questions(&layout), 1);
    let stack = find_mut(&mut layout, STACK).unwrap();
    let last = stack["children"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["id"], QUESTIONS_BLOCK);
    assert_eq!(last["content"]["props"]["view"], QUESTIONS_VIEW);
    stack["active"] = QUESTIONS_BLOCK.into();
    assert!(take_questions(&mut layout));
    assert_eq!(layout, main_layout());
    assert!(!take_questions(&mut layout));

    // No stack: no panel.
    let mut bare = json!({ "id": "root", "type": "row", "children": [] });
    assert!(!put_questions(&mut bare));
}

#[test]
fn herdrs_tab_bar_is_cloned_with_the_button_and_the_layout_names_the_clone() {
    let home = home_with("panel-clone", &[("corgi", with_panel())]);
    let mods = home.join("mods");
    let button = put_button(&home, "corgi", Some("0.1.15"), &passes).unwrap();
    assert_eq!(
        button,
        Button::Added {
            tab_bar: CLONE.into(),
            cloned: true
        }
    );
    assert_eq!(tab_bar_of(&home, "corgi"), CLONE);
    // Herdr's own is as it was.
    assert_eq!(
        fs::read_to_string(mods.join(STOCK).join("main.js")).unwrap(),
        STOCK_MAIN
    );
    let main = fs::read_to_string(mods.join(CLONE).join("main.js")).unwrap();
    assert!(main.starts_with(CLONED));
    assert!(
        main.contains("svg(\"mods/tabs-corgi/win-min.svg\")"),
        "assets moved"
    );
    assert!(main.contains(&format!("panelButton(\"{QUESTIONS_BLOCK}\", \"Questions\"")));
    assert!(main.contains("enabled.push(\"corgi-questions\")"));
    // The list's line goes before the call, the button before the window buttons.
    let list = main.find(LIST_BEGIN).unwrap();
    assert!(list < main.find(SYNC).unwrap());
    assert!(main.find(BUTTON_END).unwrap() < main.find(ANCHOR).unwrap());
    assert!(
        !mods.join(CLONE).join("jsconfig.json").exists(),
        "the app's own files stay out"
    );
    let meta: Value =
        serde_json::from_str(&fs::read_to_string(mods.join(CLONE).join("mod.json")).unwrap())
            .unwrap();
    assert_eq!(
        meta["cloned_from"],
        json!({ "name": "tabs", "version": "0.1.15" })
    );
    assert_eq!(clone_state(&mods), CloneState::AsCloned);
    // Again: nothing to do.
    assert_eq!(
        put_button(&home, "corgi", None, &passes).unwrap(),
        Button::Unchanged
    );

    // Remove: the layout goes back to Herdr's tab bar, and the clone goes.
    assert_eq!(restore(&home, None).unwrap(), ["corgi"]);
    assert_eq!(tab_bar_of(&home, "corgi"), STOCK);
    assert!(!mods.join(CLONE).exists());
    assert_eq!(
        count_questions(&read_json(&layout_path(&home, "corgi")).unwrap()),
        0
    );
}

/// herdr-gitlab's copy of the tab bar with the Jenkins mod's lines in it.
fn gitlab_with_jenkins() -> String {
    STOCK_MAIN
        .replace(
            r#"    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(undefined, panel, body, enabled);
"#,
            r#"    const enabled = [...(shared("workspace.panels") ?? ["files", "changes"]), "gitlab"];
    // >>> Jenkins panel: install.sh of mods/jenkins adds it
    // was: this.syncPanel(undefined, panel, body, enabled);
    this.syncPanel(undefined, panel, body, [...enabled, ...(find(body, "jenkins-panel") ? ["jenkins-panel"] : [])]);
    // <<< Jenkins panel
"#,
        )
        .replace(
            "      .when(w?.buttons,",
            "      // >>> Jenkins button: install.sh of mods/jenkins puts it here\n      \
             .when(Boolean(find(body, \"jenkins-panel\")), el => el.child(panelButton(\"jenkins-panel\", \"Jenkins\", \"x.svg\")))\n      \
             // <<< Jenkins button\n      .when(w?.buttons,",
        )
        .replace("mods/tabs/", "mods/tabs-gitlab/")
}

#[test]
fn a_copy_of_the_tab_bar_is_edited_in_place_beside_other_installers_lines() {
    let mut layout = with_panel();
    find_mut(&mut layout, TABS_BLOCK).unwrap()["content"]["mod"] = "tabs-gitlab".into();
    let home = home_with("panel-copy", &[("corgi", layout)]);
    let copy = home.join("mods").join("tabs-gitlab");
    fs::create_dir_all(&copy).unwrap();
    let before = gitlab_with_jenkins();
    fs::write(copy.join("main.js"), &before).unwrap();
    assert_eq!(
        put_button(&home, "corgi", None, &passes).unwrap(),
        Button::Added {
            tab_bar: "tabs-gitlab".into(),
            cloned: false
        }
    );
    assert_eq!(tab_bar_of(&home, "corgi"), "tabs-gitlab");
    assert!(!home.join("mods").join(CLONE).exists());
    let main = fs::read_to_string(copy.join("main.js")).unwrap();
    // Corgi's list line goes above Jenkins' marked lines, which stay whole.
    assert!(main.find(LIST_END).unwrap() < main.find("// >>> Jenkins panel").unwrap());
    // Its button after Jenkins', before the window buttons.
    assert!(main.find("// <<< Jenkins button").unwrap() < main.find(BUTTON_BEGIN).unwrap());
    assert_eq!(without_button(&main).unwrap(), before);

    // Remove takes out exactly Corgi's lines.
    restore(&home, None).unwrap();
    assert_eq!(fs::read_to_string(copy.join("main.js")).unwrap(), before);
    assert_eq!(tab_bar_of(&home, "corgi"), "tabs-gitlab");
}

#[test]
fn a_tab_bar_that_check_refuses_or_that_is_not_like_herdrs_is_left_alone() {
    let home = home_with("panel-refused", &[("corgi", with_panel())]);
    let refuses = |_: &Path| Err("SyntaxError".to_string());
    match put_button(&home, "corgi", None, &refuses).unwrap() {
        Button::Refused(why) => assert!(why.contains("herdr-app check refused"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(tab_bar_of(&home, "corgi"), STOCK);
    assert!(!home.join("mods").join(CLONE).exists());
    assert!(!home.join(format!(".{CLONE}.corgi-check")).exists());

    fs::write(
        home.join("mods").join(STOCK).join("main.js"),
        "export default class Tabs {}\n",
    )
    .unwrap();
    match put_button(&home, "corgi", None, &passes).unwrap() {
        Button::Refused(why) => assert!(why.contains("not like Herdr's tab bar"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(tab_bar_of(&home, "corgi"), STOCK);

    // Corgi's markers that do not pair up are not guessed at.
    let half = format!("{STOCK_MAIN}\n{BUTTON_BEGIN}\n");
    assert!(with_button(&half).is_err());
    assert!(without_button(&half).is_err());

    // A layout without a panel stack gets no button.
    let mut bare = main_layout();
    take_questions(&mut bare);
    let home = home_with("panel-none", &[("bare", bare)]);
    assert_eq!(
        put_button(&home, "bare", None, &passes).unwrap(),
        Button::NoPanel
    );
}

#[test]
fn the_clone_stays_while_another_layout_or_installer_needs_it() {
    let home = home_with(
        "panel-shared",
        &[("corgi", with_panel()), ("mine", with_panel())],
    );
    let mods = home.join("mods");
    put_button(&home, "corgi", None, &passes).unwrap();
    // A second layout shares the clone.
    assert_eq!(
        put_button(&home, "mine", None, &passes).unwrap(),
        Button::Added {
            tab_bar: CLONE.into(),
            cloned: false
        }
    );
    // Taking the panel out of one keeps the button for the other.
    restore(&home, Some("mine")).unwrap();
    assert_eq!(tab_bar_of(&home, "mine"), CLONE);
    assert!(
        fs::read_to_string(mods.join(CLONE).join("main.js"))
            .unwrap()
            .contains(BUTTON_BEGIN)
    );

    // Another installer changed the clone: it stays, without Corgi's lines.
    let main = mods.join(CLONE).join("main.js");
    let theirs = format!(
        "{}// another installer's line\n",
        fs::read_to_string(&main).unwrap()
    );
    fs::write(&main, &theirs).unwrap();
    assert_eq!(clone_state(&mods), CloneState::Changed);
    restore(&home, Some("corgi")).unwrap();
    assert_eq!(tab_bar_of(&home, "corgi"), CLONE);
    let left = fs::read_to_string(&main).unwrap();
    assert!(!left.contains(BUTTON_BEGIN) && !left.contains(LIST_BEGIN));
    assert!(left.contains("another installer's line"));

    // Someone else's folder of that name is not Corgi's clone.
    let home = home_with("panel-other", &[("corgi", with_panel())]);
    let other = home.join("mods").join(CLONE);
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("main.js"), STOCK_MAIN).unwrap();
    assert!(matches!(
        put_button(&home, "corgi", None, &passes).unwrap(),
        Button::Refused(_)
    ));
}
