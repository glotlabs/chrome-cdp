//! The fixture site's framework apps (plan 10, milestone 10.1): the same
//! todo app in React, Vue and Svelte (`testdata/site/apps/`), driven end
//! to end through the binary with the harness's [`common::Session`].
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use common::{Sandbox, chrome_path};
use fixtures::Fixtures;

/// Loads, adds, checks, filters and deletes todos in one framework's app.
/// Each step goes through the framework's state: the counter and the
/// filters only change if the app saw the input.
fn todo_app(page: &str, framework: &str) {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open", &Fixtures::shared().url(page)]);

    // The todos load after 300ms; `expect` waits for them.
    session.ok(&["expect", "#left", "text", "2 items left"]);
    session.ok(&[
        "expect",
        "role=heading",
        "text",
        &format!("{framework} todos"),
    ]);
    let found = session.ok(&["find", "item", "--role", "checkbox"]);
    for name in ["item 1", "item 2", "item 3"] {
        assert!(found.contains(name), "{found}");
    }

    session.ok(&["fill", "label=New todo", "Buy milk"]);
    session.ok(&["click", "role=button[name=\"Add\"]"]);
    session.ok(&["expect", "#left", "text", "3 items left"]);
    session.ok(&["expect", "label=New todo", "value", ""]);
    // Empty input adds nothing.
    session.ok(&["press", "Enter"]);
    session.ok(&["expect", "role=checkbox", "count", "4"]);

    session.ok(&["check", "role=checkbox[name=\"Buy milk\"]"]);
    session.ok(&["expect", "#left", "text", "2 items left"]);

    session.ok(&["click", "role=button[name=\"Done\"]"]);
    session.ok(&["expect", "role=checkbox", "count", "2"]);
    session.ok(&[
        "expect",
        "role=button[name=\"Done\"]",
        "attr",
        "aria-pressed",
        "true",
    ]);
    session.ok(&["click", "role=button[name=\"Active\"]"]);
    session.ok(&["expect", "role=checkbox", "count", "2"]);
    session.ok(&["expect", "role=checkbox[name=\"Buy milk\"]", "count", "0"]);

    session.ok(&["click", "role=button[name=\"All\"]"]);
    session.ok(&["click", "role=button[name=\"Delete item 1\"]"]);
    session.ok(&["expect", "#left", "text", "1 item left"]);
    session.ok(&["expect", "role=checkbox", "count", "3"]);

    // A missing todo is a page-level error with its kind, not a crash.
    session.fails(
        &[
            "click",
            "role=button[name=\"Delete item 1\"]",
            "--timeout",
            "500ms",
        ],
        1,
        "NotFound",
    );
    session.ok(&["expect", "--no-errors"]);
}

#[test]
fn react_todo_app() {
    todo_app("/apps/react.html", "React");
}

#[test]
fn vue_todo_app() {
    todo_app("/apps/vue.html", "Vue");
}

#[test]
fn svelte_todo_app() {
    todo_app("/apps/svelte.html", "Svelte");
}
