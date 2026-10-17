//! `screenshot` and `pdf` end to end (plan 06, milestone 6.3): the
//! session's shots/ and pdfs/ directories, `-o` relative to the current
//! directory (its extension picking the format), and `-o -` to stdout.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::Fixtures;

#[test]
fn files_paths_and_stdout() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/shots/index.html");
    let out = sandbox.run(&["open", &page]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // Without -o: the session's shots/ directory, numbered.
    let out = sandbox.run(&["screenshot", "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let shots = sandbox.runtime_dir().join("chrome-cdp/default/shots");
    assert_eq!(stdout(&out), format!("{}\n", shots.join("1.png").display()));
    assert!(
        std::fs::read(shots.join("1.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );

    // -o is relative to the current directory; .jpg makes a jpeg.
    let cwd = common::tempdir();
    let out = run(sandbox.cli().current_dir(cwd.path()).args([
        "screenshot",
        "#red",
        "-o",
        "shots/red.jpg",
        "--json",
    ]));
    let json = json_out(&out);
    let path = cwd.path().join("shots/red.jpg");
    assert_eq!(json["data"]["path"], path.display().to_string(), "{json}");
    assert_eq!(json["data"]["format"], "jpeg");
    assert_eq!(
        (
            json["data"]["width"].as_u64(),
            json["data"]["height"].as_u64()
        ),
        (Some(200), Some(100))
    );
    assert_eq!(&std::fs::read(&path).unwrap()[..2], &[0xFF, 0xD8]);

    // -o - writes the image to stdout, and keeps no file.
    let out = run(sandbox.cli().args(["screenshot", "#red", "-o", "-"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(out.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(std::fs::read_dir(&shots).unwrap().count(), 1);

    // pdf: the pdfs/ directory, or stdout.
    let out = sandbox.run(&["pdf", "--format", "a4"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let pdf = sandbox.runtime_dir().join("chrome-cdp/default/pdfs/1.pdf");
    assert!(
        stdout(&out).starts_with(&format!("{}: ", pdf.display())),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains(" A4, "), "{}", stdout(&out));
    let out = run(sandbox.cli().args(["pdf", "-o", "-"]));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(out.stdout.starts_with(b"%PDF-"));

    // --json cannot share stdout with the file.
    let out = sandbox.run(&["screenshot", "-o", "-", "--json"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        json_out(&out)["error"]["message"],
        "-o - writes the file to stdout, where --json cannot go too"
    );
}
