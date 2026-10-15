# Contributing to Corgi

Issues and pull requests are welcome at
[github.com/zyrre/corgi](https://github.com/zyrre/corgi).

[AGENTS.md](AGENTS.md) is the one guide for working on Corgi, whether you are
a person or a coding agent. It has how to build and test (`cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` and
`cargo build --release`; on macOS run the tests as `TMPDIR=/tmp cargo test`),
the shell tests for the scripts, how to redraw the README images, and the
handoff to follow before you call a change done. [ARCHITECTURE.md](ARCHITECTURE.md)
explains how Corgi works and why.

Some rules in AGENTS.md belong to the maintainer's own agent workflow, such
as the Herdr preview workspaces, the Omarchy reload steps and the release
steps. Follow them if they help; a pull request only needs the checks above
to pass, which CI runs on every pull request.

By contributing you agree that your work is licensed under the
[MIT license](LICENSE).
