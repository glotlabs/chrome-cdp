//! The read commands against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site (plan
//! 04, milestone 4.5): `find`, `text`, `html`, `attr`, `value`, `count`,
//! `box` and `markdown`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Engine, FindRequest, NavigateRequest, Navigation, OpenRequest, Read, ReadRequest,
    SnapshotRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{
    AttrData, BoxData, CountData, FindData, HtmlData, MarkdownData, Rect, SnapshotData,
    SnapshotNode, TextData, ValueData,
};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde::de::DeserializeOwned;

const TIMEOUT_MS: u64 = 30_000;

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

fn request(read: Read, locator: Option<&str>) -> ReadRequest {
    ReadRequest {
        read,
        locator: locator.map(str::to_owned),
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn read(engine: &Engine, read: Read, locator: Option<&str>) -> Result<Output, ErrorInfo> {
    engine.read(request(read, locator)).await
}

/// A read that must succeed, with its typed data.
async fn data<T: DeserializeOwned>(
    engine: &Engine,
    what: Read,
    locator: Option<&str>,
) -> (T, Output) {
    let label = format!("{} {locator:?}", what.command());
    let output = read(engine, what, locator)
        .await
        .unwrap_or_else(|e| panic!("{label}: {}", e.to_text()));
    (serde_json::from_value(output.data.clone()).unwrap(), output)
}

async fn find(
    engine: &Engine,
    query: &str,
    role: Option<&str>,
    limit: usize,
) -> Result<Output, ErrorInfo> {
    engine
        .find(FindRequest {
            query: query.to_owned(),
            role: role.map(str::to_owned),
            limit,
            tab: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
}

fn html(outer: bool) -> Read {
    Read::Html { outer, raw: false }
}

/// `text`: the page's rendered text (open shadow roots included, hidden
/// text left out), each match's, and a frame's document.
#[tokio::test]
async fn text_reads_what_is_shown() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-text").await;
    goto(&engine, &site.url("/read/index.html")).await;

    let (page, output): (TextData, _) = data(&engine, Read::Text, None).await;
    let text = &page.texts[0];
    assert_eq!(page.texts.len(), 1);
    assert_eq!(output.text, *text);
    assert!(
        text.starts_with("Acme Pricing\n\nYour cart\n\nThree items, ready to ship."),
        "{text}"
    );
    assert!(text.contains("Inside the shadow"), "{text}");
    assert!(!text.contains("Secret"), "{text}");
    assert!(!text.contains("Cherries"), "{text}");
    assert!(
        !text.contains("Pay now"),
        "iframes are their own documents: {text}"
    );

    let (intro, _): (TextData, _) = data(&engine, Read::Text, Some("#intro")).await;
    assert_eq!(intro.texts, ["Three items, ready to ship."]);

    let (items, output): (TextData, _) = data(&engine, Read::Text, Some(".item")).await;
    assert_eq!(items.texts, ["Apples", "Bananas", "", "Dates"]);
    assert_eq!(output.text, "Apples\nBananas\n\nDates");
    assert_eq!(
        output.warnings,
        ["1 of the 4 matches are hidden: they show no text"]
    );

    let (frame, _): (TextData, _) = data(&engine, Read::Text, Some("frame=other")).await;
    assert_eq!(frame.texts, ["Payment details\nPay now"]);

    let err = read(&engine, Read::Text, Some("#nothing"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    engine.close().await;
}

/// `html`: pretty-printed content or element, the document, `--raw`, and
/// `Ambiguous` for several matches with a hint that runs `html` again.
#[tokio::test]
async fn html_is_pretty_and_wants_one_element() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-html").await;
    goto(&engine, &site.url("/read/index.html")).await;

    let (inner, _): (HtmlData, _) = data(&engine, html(false), Some("#items")).await;
    assert!(!inner.outer);
    assert_eq!(
        inner.html,
        "<li class=\"item\">Apples</li>\n<li class=\"item\">Bananas</li>\n\
         <li class=\"item gone\">Cherries</li>\n<li class=\"item\">Dates</li>"
    );
    let (outer, _): (HtmlData, _) = data(&engine, html(true), Some("#host")).await;
    assert_eq!(
        outer.html,
        "<div id=\"host\">\n  #shadow-root (open)\n    <p class=\"shadow\">Inside the shadow</p>\n</div>"
    );
    let (editor, _): (HtmlData, _) = data(&engine, html(true), Some("#editor")).await;
    assert_eq!(
        editor.html,
        "<div id=\"editor\" contenteditable=\"true\">Edited <b>text</b></div>"
    );
    let (doc, _): (HtmlData, _) = data(&engine, html(false), None).await;
    assert!(doc.outer);
    assert!(
        doc.html.starts_with("<!doctype html>\n<html>\n  <head>\n"),
        "{}",
        doc.html
    );
    let (raw, _): (HtmlData, _) = data(
        &engine,
        Read::Html {
            outer: false,
            raw: true,
        },
        Some("#intro"),
    )
    .await;
    assert_eq!(raw.html, "Three   items,\n     ready to    ship.");

    let err = read(&engine, html(true), Some(".item")).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Ambiguous);
    let hint = err.hint.unwrap();
    assert!(
        hint.starts_with("chrome-cdp html e")
            && hint.contains(" --outer --session engine-read-html"),
        "{hint}"
    );
    engine.close().await;
}

/// `attr`, `value` and `count`.
#[tokio::test]
async fn attr_value_and_count() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-fields").await;
    goto(&engine, &site.url("/read/index.html")).await;

    let attr = |name: &str| Read::Attr { name: name.into() };
    let (class, output): (AttrData, _) = data(&engine, attr("class"), Some("#pricing")).await;
    assert_eq!(class.value.as_deref(), Some("nav link"));
    assert_eq!(output.text, "nav link");
    let (empty, output): (AttrData, _) = data(&engine, attr("data-empty"), Some("#pay")).await;
    assert_eq!(empty.value.as_deref(), Some(""));
    assert!(output.warnings.is_empty());
    let (missing, output): (AttrData, _) = data(&engine, attr("nope"), Some("#pricing")).await;
    assert_eq!(missing.value, None);
    assert_eq!(output.text, "");
    assert_eq!(
        output.warnings,
        ["#pricing has no attribute \"nope\" (it has id, href, class, data-x)"]
    );

    let table: &[(&str, &str, &str, Option<&[&str]>)] = &[
        ("#email", "input", "ada@example.com", None),
        ("#password", "password", "hunter2", None),
        ("#notes", "textarea", "Leave at\nthe door", None),
        ("#country", "select", "se", None),
        ("#extras", "select", "wrap\nbag", Some(&["wrap", "bag"])),
        ("#editor", "contenteditable", "Edited text", None),
        ("#volume", "aria", "seven", None),
        ("label=Email", "input", "ada@example.com", None),
    ];
    for (locator, kind, value, values) in table {
        let (field, output): (ValueData, _) = data(&engine, Read::Value, Some(locator)).await;
        assert_eq!(field.kind, *kind, "{locator}");
        assert_eq!(field.value, *value, "{locator}");
        assert_eq!(output.text, *value, "{locator}");
        assert_eq!(
            field.values,
            values.map(|v| v.iter().map(ToString::to_string).collect()),
            "{locator}"
        );
    }
    let err = read(&engine, Read::Value, Some("#intro"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "#intro has no value: it is a <p>, not a field");

    let (items, output): (CountData, _) = data(&engine, Read::Count, Some(".item")).await;
    assert_eq!((items.count, items.visible), (4, 3));
    assert_eq!(output.text, "4 (3 visible)");
    assert_eq!(output.quiet.as_deref(), Some("4"));
    let (none, output): (CountData, _) = data(&engine, Read::Count, Some("#nothing")).await;
    assert_eq!((none.count, none.visible), (0, 0));
    assert_eq!(output.text, "0");
    let (framed, _): (CountData, _) =
        data(&engine, Read::Count, Some("frame=other >> role=button")).await;
    assert_eq!(framed.count, 1);
    engine.close().await;
}

/// `box`: in the tab's viewport, with the offsets of same-process and
/// cross-site iframes added; out of view and not rendered say so.
#[tokio::test]
async fn box_is_in_the_tabs_viewport() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-box").await;
    goto(&engine, &site.url("/read/index.html")).await;

    let rect = |x, y, width, height| Rect {
        x,
        y,
        width,
        height,
    };
    let (shown, output): (BoxData, _) = data(&engine, Read::Box, Some("#box")).await;
    assert_eq!(shown.rect, Some(rect(30.0, 40.0, 120.0, 36.0)));
    assert!(shown.in_viewport);
    assert_eq!(output.text, "x=30 y=40 w=120 h=36");
    assert_eq!(output.quiet.as_deref(), Some("30 40 120 36"));

    // The iframes have a 5px border and 10px padding; the button sits at
    // (7, 9) in each, and the second iframe is below the first.
    let (iframe, _): (BoxData, _) = data(&engine, Read::Box, Some("frame=same")).await;
    assert_eq!(iframe.rect, Some(rect(0.0, 200.0, 330.0, 180.0)));
    let (same, _): (BoxData, _) = data(&engine, Read::Box, Some("frame=same >> #target")).await;
    assert_eq!(same.rect, Some(rect(22.0, 224.0, 40.0, 20.0)));
    let (other, _): (BoxData, _) = data(&engine, Read::Box, Some("frame=other >> #target")).await;
    assert_eq!(other.rect, Some(rect(22.0, 404.0, 40.0, 20.0)));

    let (far, output): (BoxData, _) = data(&engine, Read::Box, Some("#far")).await;
    assert_eq!(far.rect, Some(rect(10.0, 5000.0, 50.0, 20.0)));
    assert!(!far.in_viewport);
    assert!(
        output
            .text
            .starts_with("x=10 y=5000 w=50 h=20 (outside the "),
        "{}",
        output.text
    );

    let (hidden, output): (BoxData, _) = data(&engine, Read::Box, Some("#hidden-text")).await;
    assert_eq!(hidden.rect, None);
    assert!(!hidden.in_viewport);
    assert_eq!(output.text, "");
    assert_eq!(
        output.warnings,
        ["#hidden-text is not rendered, so it has no box"]
    );
    engine.close().await;
}

/// `markdown`: the main content picked by readability, converted; an
/// element asked for is converted whole.
#[tokio::test]
async fn markdown_reads_the_main_content() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-markdown").await;
    goto(&engine, &site.url("/read/article.html")).await;

    let (article, output): (MarkdownData, _) = data(&engine, Read::Markdown, None).await;
    assert_eq!(article.source, "div#content");
    assert_eq!(article.title, "The Analytical Engine");
    assert_eq!(output.text, article.markdown);
    assert_eq!(
        article.markdown,
        "# The Analytical Engine\n\n\
         By [Ada Lovelace](/authors/ada), 1843. See [Babbage](https://example.org/babbage).\n\n\
         The engine has **no pretensions** whatever to _originate_ anything, and it can do \
         whatever we know how to order it to perform.\n\n\
         ## Notes\n\n\
         - Note A covers the `mill`.\n\
         - Note G covers Bernoulli numbers:\n  1. first\n  2. second\n\n\
         > It weaves algebraical patterns, just as the Jacquard loom weaves flowers and leaves.\n\n\
         ```rust\nfor n in 1..=7 {\n    b(n);\n}\n```\n\n\
         | Note | Topic |\n| --- | --- |\n| A | The mill \\| store |\n| G | Bernoulli |\n\n\
         Line one\nLine two\n\n\
         ![A diagram](/nav/pixel.svg)"
    );

    let (footer, _): (MarkdownData, _) = data(&engine, Read::Markdown, Some("css=footer")).await;
    assert_eq!(footer.source, "footer");
    assert!(
        footer.markdown.starts_with("Copyright 1843"),
        "{}",
        footer.markdown
    );

    // A page with a <main>: it is the content, without the form's
    // controls and labels.
    goto(&engine, &site.url("/read/index.html")).await;
    let (cart, _): (MarkdownData, _) = data(&engine, Read::Markdown, None).await;
    assert_eq!(cart.source, "main#main");
    assert!(
        cart.markdown.starts_with(
            "# Your cart\n\nThree items, ready to ship.\n\n- Apples\n- Bananas\n- Dates\n\n"
        ),
        "{}",
        cart.markdown
    );
    assert!(!cart.markdown.contains("Email"), "{}", cart.markdown);
    let (frame, _): (MarkdownData, _) = data(&engine, Read::Markdown, Some("frame=same")).await;
    assert_eq!(frame.markdown, "## Payment details");
    engine.close().await;
}

/// The ref of the first node with this role and name.
fn snapshot_ref(nodes: &[SnapshotNode], role: &str, name: &str) -> Option<String> {
    nodes.iter().find_map(|n| {
        if n.role == role && n.name == name {
            n.reference.clone()
        } else {
            snapshot_ref(&n.children, role, name)
        }
    })
}

/// `find`: best matches first, with their containers, refs that the
/// snapshot shares and commands take, `--role` and `--limit`.
#[tokio::test]
async fn find_ranks_matches_and_gives_refs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-find").await;
    goto(&engine, &site.url("/read/index.html")).await;

    let output = find(&engine, "checkout", None, 10).await.unwrap();
    let found: FindData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(found.count, 2);
    let lines: Vec<&str> = output.text.lines().collect();
    assert!(
        lines[0].starts_with("page t1 \"Read commands\" http://"),
        "{}",
        lines[0]
    );
    assert_eq!(
        lines[1],
        "- button \"Checkout\" [e1] (in form \"Checkout\")"
    );
    assert_eq!(lines[2], "- form \"Checkout\" [e2]");
    assert_eq!(output.quiet.as_deref(), Some("e1\ne2"));
    assert_eq!(
        found.matches[0].context.as_deref(),
        Some("form \"Checkout\"")
    );

    // The refs are the snapshot's, and name the elements.
    let snapshot = engine
        .snapshot(SnapshotRequest {
            tab: None,
            interactive: false,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 200,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let snapshot: SnapshotData = serde_json::from_value(snapshot.data).unwrap();
    assert_eq!(
        snapshot_ref(&snapshot.nodes, "button", "Checkout").as_deref(),
        Some("e1")
    );
    let (pay, _): (AttrData, _) = data(&engine, Read::Attr { name: "id".into() }, Some("e1")).await;
    assert_eq!(pay.value.as_deref(), Some("pay"));

    // Matches in iframes say which; a typo is forgiven; --role narrows.
    let output = find(&engine, "pay now", None, 10).await.unwrap();
    let found: FindData = serde_json::from_value(output.data).unwrap();
    let contexts: Vec<_> = found.matches.iter().map(|m| m.context.as_deref()).collect();
    assert_eq!(
        contexts,
        [Some("iframe \"Same\""), Some("iframe \"Other\"")]
    );
    let output = find(&engine, "emial", None, 10).await.unwrap();
    assert!(
        output.text.contains("- textbox \"Email\" [e"),
        "{}",
        output.text
    );
    let output = find(&engine, "checkout", Some("button"), 10).await.unwrap();
    let found: FindData = serde_json::from_value(output.data).unwrap();
    assert_eq!(found.count, 1);

    // Text matches get no ref; the rest is counted past --limit.
    let output = find(&engine, "a", None, 2).await.unwrap();
    let found: FindData = serde_json::from_value(output.data).unwrap();
    assert!(found.count > 2);
    assert_eq!(found.matches.len(), 2);
    let last = output.text.lines().last().unwrap();
    assert_eq!(
        last,
        format!(
            "… {} more matches (use --limit {} or more words)",
            found.count - 2,
            found.count
        )
    );
    let output = find(&engine, "ready to ship", None, 10).await.unwrap();
    assert_eq!(
        output.text.lines().nth(1),
        Some("- text \"Three items, ready to ship.\"")
    );
    let found: FindData = serde_json::from_value(output.data).unwrap();
    assert_eq!(found.matches[0].node.reference, None);

    let err = find(&engine, "zebra", None, 10).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "nothing in tab t1 matches \"zebra\"");
    engine.close().await;
}

/// Reads fail at once while a dialog blocks the page.
#[tokio::test]
async fn reads_fail_while_a_dialog_is_open() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-read-dialog").await;
    let url = site.url("/dialogs/on-load.html?kind=confirm&message=Sure%3F");
    let _ = engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url,
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await;
    let started = std::time::Instant::now();
    let err = read(&engine, Read::Text, None).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::DialogOpen);
    let err = find(&engine, "anything", None, 10).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::DialogOpen);
    assert!(started.elapsed() < Duration::from_secs(5));
    engine.close().await;
}
