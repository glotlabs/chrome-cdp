//! `forms` and `fill-form` (plan 16, milestone 16.4) end to end, against
//! a real Chrome (`CHROME_PATH`) and the fixture site's `forms/` pages:
//! the checkout (every field kind, a region select that appears once a
//! country is chosen, a custom combobox and listbox, a card number that
//! is invalid until filled; `#state` shows every value the page holds)
//! and two forms on one page (a login with a submit button, a newsletter
//! without).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::Path;
use std::process::Output;

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;
use serde_json::{Value, json};

/// A session in a sandbox, run from a directory of its own (so relative
/// file paths resolve there).
struct Forms {
    sandbox: Sandbox,
    dir: std::path::PathBuf,
}

impl Forms {
    fn new(chrome: &Path) -> Self {
        let sandbox = Sandbox::with_chrome(chrome);
        let dir = sandbox.dir.path().join("work");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/site/forms/receipt.pdf"),
            dir.join("receipt.pdf"),
        )
        .unwrap();
        Self { sandbox, dir }
    }

    fn run(&self, args: &[&str]) -> Output {
        common::run(self.sandbox.cli().current_dir(&self.dir).args(args))
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        stdout(&out)
    }

    fn open(&self, page: &str) {
        let url = format!("{}/forms/{page}", Fixtures::shared().a());
        let _ = self.run(&["close"]);
        self.ok(&["open", &url]);
    }

    /// What the checkout page holds, from its `#state`.
    fn state(&self, selector: &str) -> Value {
        serde_json::from_str(self.ok(&["text", selector]).trim()).unwrap_or(Value::Null)
    }

    /// The ref of the listing line of `forms` that contains `what`.
    fn reference(&self, what: &str) -> String {
        let listing = self.ok(&["forms"]);
        let line = listing
            .lines()
            .find(|l| l.contains(what))
            .unwrap_or_else(|| panic!("no {what} in {listing}"));
        let start = line.find("[e").unwrap() + 1;
        let end = start + line[start..].find(']').unwrap();
        line[start..end].to_owned()
    }
}

impl Drop for Forms {
    fn drop(&mut self) {
        let _ = self.run(&["close"]);
    }
}

/// Every field of the checkout, one per kind of value.
const CHECKOUT: &str = r##"{
  "email": "ada@example.com",
  "Full name": "Ada Lovelace",
  "card number": "4242 4242 4242 4242",
  "Country": "Norway",
  "Region": "Vestland",
  "Delivery date": "2026-10-01",
  "Wrapping colour": "#ff0000",
  "Tip": 12,
  "Quantity": 3,
  "Notes": "Ring twice",
  "Extras": ["Insurance", "SMS updates"],
  "Delivery days": ["Monday", "Friday"],
  "Save card": true,
  "Newsletter": true,
  "Shipping": "express",
  "Receipt": "receipt.pdf",
  "City": "Bergen"
}"##;

#[test]
fn fill_form_fills_every_kind_of_field_in_one_call() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("checkout.html");
    std::fs::write(forms.dir.join("order.json"), CHECKOUT).unwrap();
    let out = forms.run(&["fill-form", "--file", "order.json", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = json_out(&out)["data"].clone();
    let filled = data["filled"].as_array().unwrap();
    assert_eq!(filled.len(), 17, "{data}");
    // In the page's order, whatever the JSON's; the region once it came.
    let keys: Vec<&str> = filled.iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert_eq!(keys[..4], ["email", "Full name", "card number", "Country"]);
    assert!(keys.contains(&"Region"), "{keys:?}");
    let actions: Vec<(&str, &str)> = filled
        .iter()
        .map(|f| (f["key"].as_str().unwrap(), f["action"].as_str().unwrap()))
        .collect();
    for expected in [
        ("Country", "select"),
        ("Region", "select"),
        ("Save card", "check"),
        ("Newsletter", "check"),
        ("Shipping", "check"),
        ("Receipt", "upload"),
        ("Delivery days", "click"),
        ("City", "fill"),
        ("Tip", "fill"),
    ] {
        assert!(actions.contains(&expected), "{expected:?} in {actions:?}");
    }
    let city = filled.iter().find(|f| f["key"] == "City").unwrap();
    assert_eq!(city["options"][0]["name"], "Bergen", "{city}");
    assert_eq!(data["form"]["name"], "Checkout");
    assert_eq!(
        forms.state("#state"),
        json!({
            "email": "ada@example.com",
            "fullname": "Ada Lovelace",
            "card": "4242 4242 4242 4242",
            "country": "no",
            "region": "Vestland",
            "delivery": "2026-10-01",
            "colour": "#ff0000",
            "tip": "12",
            "quantity": "3",
            "notes": "Ring twice",
            "extras": ["insurance", "sms"],
            "save": true,
            "shipping": "express",
            "receipt": ["receipt.pdf"],
            "days": ["Monday", "Friday"],
            "newsletter": true,
            "city": "Bergen",
            "cardValid": true
        })
    );
    // forms shows the state too: the card is valid now, nothing required
    // is empty.
    let listing = forms.ok(&["forms"]);
    assert!(
        listing.starts_with("form \"Checkout\" [e1] → POST /api/orders (17 fields)\n"),
        "{listing}"
    );
    assert!(!listing.contains("invalid"), "{listing}");
    assert!(
        listing.contains("radiogroup \"Shipping\": radio \"Standard\"")
            && listing.contains("radio \"Express\" [")
            && listing.contains("] checked\n"),
        "{listing}"
    );
    assert!(listing.contains("files: receipt.pdf"), "{listing}");
    assert!(listing.contains("switch \"Newsletter\" ["), "{listing}");
    assert!(listing.contains("] checked"), "{listing}");
}

#[test]
fn keys_left_fail_after_the_rest_is_filled() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("checkout.html");
    let out = forms.run(&[
        "fill-form",
        "--values",
        r#"{"Email": "ada@example.com", "Contry": "Sweden", "mail": "x", "Save card": true}"#,
        "--json",
    ]);
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    let error = json_out(&out)["error"].clone();
    assert_eq!(error["kind"], "NotFound", "{error}");
    assert_eq!(
        error["message"],
        "\"Contry\" matches no field; \"mail\" matches no field"
    );
    let filled: Vec<&str> = error["details"]["filled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(filled, ["Email", "Save card"]);
    let state = forms.state("#state");
    assert_eq!(state["email"], "ada@example.com");
    assert_eq!(state["save"], true);
    assert_eq!(state["country"], "");
    // The text says what it meant.
    let out = forms.run(&["fill-form", "--values", r#"{"Contry": "Sweden"}"#]);
    let err = stderr(&out);
    assert!(err.contains("Contry: did you mean: \"Country\""), "{err}");
    assert!(err.contains("hint: chrome-cdp forms e"), "{err}");
    // A key that matches two fields.
    let out = forms.run(&["fill-form", "--values", r#"{"de": "x"}"#, "--json"]);
    assert_eq!(code(&out), 1);
    let error = json_out(&out)["error"].clone();
    assert_eq!(error["message"], "\"de\" matches 2 fields", "{error}");
}

#[test]
fn a_value_of_the_wrong_type_fills_nothing() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("checkout.html");
    let before = forms.state("#state");
    let out = forms.run(&[
        "fill-form",
        "--values",
        r#"{"Email": "ada@example.com", "Save card": "yes"}"#,
    ]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let err = stderr(&out);
    assert!(
        err.starts_with(
            "error: \"Save card\" is checkbox \"Save card\" [e13]: give true or false, not \"yes\""
        ),
        "{err}"
    );
    let out = forms.run(&["fill-form", "--values", r#"{"Shipping": "Drone"}"#]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("no choice \"Drone\"; its choices: \"Standard\", \"Express\""),
        "{}",
        stderr(&out)
    );
    let out = forms.run(&["fill-form", "--values", r#"["Email"]"#]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("not a JSON object"),
        "{}",
        stderr(&out)
    );
    assert_eq!(forms.state("#state"), before);
}

#[test]
fn two_forms_need_a_locator_and_submit_both_ways() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("two-forms.html");
    let out = forms.run(&["fill-form", "--values", r#"{"Email": "a@b.c"}"#, "--json"]);
    assert_eq!(code(&out), 1);
    let error = json_out(&out)["error"].clone();
    assert_eq!(error["kind"], "Ambiguous", "{error}");
    let names: Vec<&str> = error["details"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Log in", "Newsletter"]);
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("chrome-cdp fill-form e"),
        "{error}"
    );

    // With a submit button: it is clicked.
    let login = forms.reference("form \"Log in\"");
    let out = forms.ok(&[
        "fill-form",
        &login,
        "--values",
        r#"{"Email": "ada@example.com", "Password": "s3cret", "Remember me": true}"#,
        "--submit",
    ]);
    assert!(
        out.contains("filled textbox \"Password\" [")
            && out.contains("\nsubmitted: clicked button \"Sign in\" ["),
        "{out}"
    );
    assert_eq!(
        forms.ok(&["text", "#sent"]).trim(),
        r#"login {"email":"ada@example.com","password":"s3cret","remember":"on"}"#
    );
    // Passwords are not shown.
    assert!(
        forms
            .ok(&["forms", &login])
            .contains("(password) required value=***"),
    );

    // Without one: Enter in the last field. A locator inside the form
    // names it.
    let out = forms.ok(&[
        "fill-form",
        "label=Your email",
        "--values",
        r#"{"Your email": "news@example.com"}"#,
        "--submit",
    ]);
    assert!(
        out.contains("submitted: pressed Enter in textbox \"Your email\" ["),
        "{out}"
    );
    assert_eq!(
        forms.ok(&["text", "#sent"]).trim(),
        r#"newsletter {"subscriber":"news@example.com"}"#
    );
}

#[test]
fn history_keeps_fill_form_as_one_line_that_replays() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("two-forms.html");
    let login = forms.reference("form \"Log in\"");
    std::fs::write(
        forms.dir.join("login.json"),
        "{\n  \"Email\": \"ada@example.com\",\n  \"Remember me\": true\n}\n",
    )
    .unwrap();
    forms.ok(&["fill-form", &login, "--file", "login.json"]);
    let scenario = forms.ok(&["history", "--as-scenario", "logs in"]);
    let site = Fixtures::shared().a();
    assert_eq!(
        scenario,
        format!(
            "@test \"logs in\"\n  goto {site}/forms/two-forms.html\n  fill-form 'role=form[name=\"Log in\"]' --values '{{\"Email\":\"ada@example.com\",\"Remember me\":true}}'\n"
        )
    );
    std::fs::write(forms.dir.join("login.cdp"), &scenario).unwrap();
    let out = forms.run(&["lint", "login.cdp"]);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));

    // The line replays on a fresh page.
    forms.open("two-forms.html");
    let script: String = scenario.lines().skip(1).fold(String::new(), |mut out, l| {
        out.push_str(l.trim());
        out.push('\n');
        out
    });
    std::fs::write(forms.dir.join("replay.cdp"), script).unwrap();
    forms.ok(&["run", "replay.cdp"]);
    assert_eq!(
        forms.ok(&["value", "role=textbox[name=\"Email\"]"]).trim(),
        "ada@example.com"
    );
}

#[test]
fn forms_in_iframes_are_listed_and_filled() {
    let Some(chrome) = chrome_path() else { return };
    let forms = Forms::new(&chrome);
    forms.open("framed.html");
    forms.ok(&["wait", "--idle"]);
    let b = Fixtures::shared().b();
    let listing = forms.ok(&["forms"]);
    assert!(
        listing.starts_with("search \"Site\" [e1] (1 field)\n"),
        "{listing}"
    );
    let remote = format!("form \"Log in\" [e11] in frame {b}/forms/two-forms.html");
    assert!(listing.contains(&remote), "{listing}");
    // The fields outside a form of both iframes, as one group.
    assert!(
        listing.contains("\nfields outside a form (2 fields)\n"),
        "{listing}"
    );
    // Several forms in all: name one.
    let out = forms.run(&["fill-form", "--values", r#"{"Email": "a@b.c"}"#]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("the page has several forms (5)"),
        "{}",
        stderr(&out)
    );
    // The cross-site (out-of-process) one.
    let reference = forms.reference(&remote);
    forms.ok(&[
        "fill-form",
        &reference,
        "--values",
        r#"{"Email": "ada@example.com", "Password": "pw"}"#,
        "--submit",
    ]);
    assert_eq!(
        forms.ok(&["text", "frame=localhost >> #sent"]).trim(),
        r#"login {"email":"ada@example.com","password":"pw"}"#
    );
}
