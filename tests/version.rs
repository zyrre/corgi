//! The real `corgi --version`: the line `scripts/install.sh` and the release
//! workflow check, and its JSON form, which the commit of this checkout
//! (`build.rs`) goes into.

use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

fn corgi(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corgi"))
        .args(args)
        .output()
        .expect("run corgi")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn version_prints_the_release_and_the_commit_it_was_built_from() {
    let version = env!("CARGO_PKG_VERSION");
    let plain = corgi(&["--version"]);
    assert!(plain.status.success());
    let line = stdout(&plain);
    assert_eq!(stdout(&corgi(&["-V"])), line);

    let json = corgi(&["--version", "--json"]);
    assert!(json.status.success());
    let json: Value = serde_json::from_str(&stdout(&json)).expect("one JSON object");
    assert_eq!(json["version"], version);
    let dirty = json["dirty"].as_bool().expect("dirty is a boolean");

    // This test's own checkout, when it is one: the commit Git names.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git = |args: &[&str]| {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output();
        output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| stdout(&output))
    };
    let checkout = git(&["rev-parse", "--show-toplevel"])
        .and_then(|top| std::fs::canonicalize(top).ok())
        .filter(|top| std::fs::canonicalize(dir).is_ok_and(|dir| dir == *top));
    match checkout.and_then(|_| git(&["rev-parse", "HEAD"])) {
        Some(head) => {
            assert_eq!(json["commit"], head.as_str());
            assert_eq!(json["source"], dir.to_string_lossy().as_ref());
            let modified = if dirty { ", modified" } else { "" };
            assert_eq!(line, format!("corgi {version} ({}{modified})", &head[..7]));
        }
        // A build without Git: the release alone.
        None => {
            assert_eq!(json["commit"], Value::Null);
            assert_eq!(json["source"], Value::Null);
            assert!(!dirty);
            assert_eq!(line, format!("corgi {version}"));
        }
    }
    // What the installer accepts of a downloaded binary: the version, then
    // nothing or the commit in brackets.
    let rest = line.strip_prefix(&format!("corgi {version}")).unwrap();
    assert!(
        rest.is_empty() || (rest.starts_with(" (") && rest.ends_with(')')),
        "{line}"
    );
}

#[test]
fn version_refuses_what_it_does_not_know() {
    let unknown = corgi(&["--version", "--yaml"]);
    assert!(!unknown.status.success());
    assert!(stdout(&unknown).is_empty());
    let help = corgi(&["--version", "--help"]);
    assert!(help.status.success());
    assert!(stdout(&help).starts_with("Usage: corgi --version [--json]"));
}
