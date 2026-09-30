//! How well an element of the page matches a fingerprint (plan 18,
//! milestone 18.2), and what a fingerprint keeps of class names and
//! paths. Pure functions, so the weights have tests.
//!
//! Each signal the fingerprint has adds its weight to the most an element
//! could score; an element gets the weight of each signal it shares. An
//! element of another role (not of the same family) gets at most half
//! marks. Its confidence is its share of the most it could have had,
//! lowered when another element comes close (see [`rank`]).

use protocol::schema::{Fingerprint, HealCandidate};

/// Test ids equal.
const TESTID: f64 = 2.0;
/// Roles equal; half of it for roles of one family ([`FAMILIES`]).
const ROLE: f64 = 3.0;
/// Accessible names equal, or similar ([`similar`]).
const NAME: f64 = 3.0;
const NAME_SIMILAR: f64 = 2.0;
/// Labels, placeholders and texts, each: equal, or similar.
const TEXTS: f64 = 2.0;
const TEXTS_SIMILAR: f64 = 1.5;
/// The container's role and name equal.
const CONTAINER: f64 = 2.0;
const ID: f64 = 1.0;
/// Times the class names' Jaccard index.
const CLASSES: f64 = 1.0;
/// Tag and `type` attribute both equal.
const TAG: f64 = 1.0;
/// The place among its role's elements in the container equal.
const INDEX: f64 = 0.5;

/// How close two texts must be to be similar ([`similar`]).
const SIMILAR: f64 = 0.7;
/// How close two words must be to count as one.
const WORD: f64 = 0.8;
/// Confidence levels.
pub const HIGH: f64 = 0.8;
pub const MEDIUM: f64 = 0.6;
/// How far apart the best two scores must be for the best to keep its
/// whole score: `(best − second) / GAP + 0.5`, at most 1.
const GAP: f64 = 0.2;

/// Roles that stand in for each other after a redesign.
const FAMILIES: [&[&str]; 2] = [&["button", "link"], &["textbox", "searchbox", "combobox"]];

/// How many class names a fingerprint keeps.
pub const CLASSES_KEPT: usize = 5;

/// An element's score against a fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// 0 to 1: what it got over the most it could have had.
    pub score: f64,
    /// `same testid`, `similar name`, `name differs`, ….
    pub reasons: Vec<String>,
}

/// Scores element `el` against fingerprint `fp`.
pub fn score(fp: &Fingerprint, el: &Fingerprint) -> Scored {
    let mut s = Scoring::default();
    if !fp.testid.is_empty() {
        s.exact("testid", TESTID, fp.testid == el.testid);
    }
    let mut other_role = false;
    if !fp.role.is_empty() {
        if fp.role == el.role {
            s.exact("role", ROLE, true);
        } else if akin(&fp.role, &el.role) {
            s.max += ROLE;
            s.got += ROLE / 2.0;
            s.reasons.push("role of the same family".into());
        } else {
            s.exact("role", ROLE, false);
            other_role = true;
        }
    }
    if !fp.name.is_empty() {
        s.text("name", NAME, NAME_SIMILAR, &fp.name, &el.name);
    }
    // A label or text that is the name says nothing more.
    let own = |value: &str| !value.is_empty() && !same_text(value, &fp.name);
    if own(&fp.label) {
        s.text("label", TEXTS, TEXTS_SIMILAR, &fp.label, &el.label);
    }
    if own(&fp.placeholder) {
        s.text(
            "placeholder",
            TEXTS,
            TEXTS_SIMILAR,
            &fp.placeholder,
            &el.placeholder,
        );
    }
    if own(&fp.text) {
        s.text("text", TEXTS, TEXTS_SIMILAR, &fp.text, &el.text);
    }
    if let Some(container) = &fp.container {
        let same = el
            .container
            .as_ref()
            .is_some_and(|c| c.role == container.role && same_text(&c.name, &container.name));
        s.exact("container", CONTAINER, same);
    }
    if !fp.id.is_empty() {
        s.quiet(ID, fp.id == el.id, "id");
    }
    if !fp.classes.is_empty() {
        let j = jaccard(&fp.classes, &el.classes);
        s.max += CLASSES;
        s.got += CLASSES * j;
    }
    if !fp.tag.is_empty() {
        s.quiet(TAG, fp.tag == el.tag && fp.kind == el.kind, "tag");
    }
    if let Some(index) = fp.index {
        s.quiet(INDEX, el.index == Some(index), "");
    }
    if other_role {
        s.got = s.got.min(s.max / 2.0);
    }
    Scored {
        score: if s.max > 0.0 { s.got / s.max } else { 0.0 },
        reasons: s.reasons,
    }
}

#[derive(Default)]
struct Scoring {
    max: f64,
    got: f64,
    reasons: Vec<String>,
}

impl Scoring {
    /// A signal that is equal or not, named in the reasons either way.
    fn exact(&mut self, what: &str, weight: f64, equal: bool) {
        self.max += weight;
        if equal {
            self.got += weight;
            self.reasons.push(format!("same {what}"));
        } else {
            self.reasons.push(format!("{what} differs"));
        }
    }

    /// A signal named in the reasons only when equal (`what` not empty).
    fn quiet(&mut self, weight: f64, equal: bool, what: &str) {
        self.max += weight;
        if equal {
            self.got += weight;
            if !what.is_empty() {
                self.reasons.push(format!("same {what}"));
            }
        }
    }

    /// A text signal: equal, similar, or different.
    fn text(&mut self, what: &str, weight: f64, similar_weight: f64, want: &str, have: &str) {
        self.max += weight;
        if same_text(want, have) {
            self.got += weight;
            self.reasons.push(format!("same {what}"));
        } else if similar(want, have) {
            self.got += similar_weight;
            self.reasons.push(format!("similar {what}"));
        } else {
            self.reasons.push(format!("{what} differs"));
        }
    }
}

/// Whether roles `a` and `b` are of one family (not equal).
fn akin(a: &str, b: &str) -> bool {
    a != b
        && FAMILIES
            .iter()
            .any(|family| family.contains(&a) && family.contains(&b))
}

fn lower(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Equal but for case and whitespace.
fn same_text(a: &str, b: &str) -> bool {
    lower(a) == lower(b)
}

/// Whether `a` and `b` are close, case and whitespace aside: their edit
/// similarity (1 − Levenshtein distance over the longer length) is 0.7 or
/// more, or two thirds of the words of the longer are in the shorter
/// (`Add to cart`, `Add to basket`; a word counts when its edit
/// similarity is 0.8 or more), or the shorter (of 3 characters or more)
/// is a run of whole words of the longer (`Sign in`, `Sign in now`).
pub fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (lower(a), lower(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b || edit(&a, &b) >= SIMILAR {
        return true;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if short.len() >= 3 && format!(" {long} ").contains(&format!(" {short} ")) {
        return true;
    }
    let (wa, wb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    let (few, many) = if wa.len() <= wb.len() {
        (&wa, &wb)
    } else {
        (&wb, &wa)
    };
    let shared = few
        .iter()
        .filter(|w| many.iter().any(|o| edit(w, o) >= WORD))
        .count();
    3 * shared >= 2 * many.len()
}

/// 1 − Levenshtein distance over the longer length, in characters.
fn edit(a: &str, b: &str) -> f64 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let d = strsim::levenshtein(a, b) as f64 / longest as f64;
    1.0 - d
}

/// |A ∩ B| / |A ∪ B|; 0 for two empty sets.
fn jaccard(a: &[String], b: &[String]) -> f64 {
    let both = a.iter().filter(|c| b.contains(c)).count();
    let either = a.len() + b.iter().filter(|c| !a.contains(c)).count();
    if either == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let j = both as f64 / either as f64;
    j
}

/// A candidate of [`rank`]: its index among the elements, and how it
/// matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub index: usize,
    pub score: f64,
    pub confidence: f64,
    pub reasons: Vec<String>,
}

/// The `most` elements of `elements` that best match `fp`, best first,
/// each with its confidence: its score, times `(score − the best other
/// score) / 0.2 + 0.5` when that is below 1 (so a runner-up that comes
/// close lowers it, and two equal elements get half marks at best).
pub fn rank(fp: &Fingerprint, elements: &[Fingerprint], most: usize) -> Vec<Ranked> {
    let mut scored: Vec<(usize, Scored)> = elements
        .iter()
        .enumerate()
        .map(|(i, el)| (i, score(fp, el)))
        .collect();
    scored.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));
    let scores: Vec<f64> = scored.iter().map(|(_, s)| s.score).collect();
    scored
        .into_iter()
        .enumerate()
        .take(most)
        .map(|(n, (index, s))| {
            let other = scores
                .iter()
                .enumerate()
                .filter(|&(m, _)| m != n)
                .map(|(_, &x)| x)
                .fold(0.0, f64::max);
            let factor = ((s.score - other) / GAP + 0.5).clamp(0.0, 1.0);
            Ranked {
                index,
                score: s.score,
                confidence: round((s.score * factor).min(1.0)),
                reasons: s.reasons,
            }
        })
        .collect()
}

/// Two decimals, so output and JSON stay short.
fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `high`, `medium` or `low`.
pub fn level(confidence: f64) -> &'static str {
    if confidence >= HIGH {
        "high"
    } else if confidence >= MEDIUM {
        "medium"
    } else {
        "low"
    }
}

/// `button "Add to basket"`, or the tag and text for an element with no
/// role.
pub fn element(el: &Fingerprint) -> String {
    let what = if el.role.is_empty() {
        &el.tag
    } else {
        &el.role
    };
    let name = if el.name.is_empty() {
        &el.text
    } else {
        &el.name
    };
    if name.is_empty() {
        what.clone()
    } else {
        format!("{what} \"{}\"", name.replace('"', "\\\""))
    }
}

/// A heal candidate of `el`, ranked.
pub fn candidate(el: &Fingerprint, ranked: &Ranked, locator: Option<String>) -> HealCandidate {
    HealCandidate {
        locator,
        element: element(el),
        confidence: ranked.confidence,
        level: level(ranked.confidence).to_owned(),
        reasons: ranked.reasons.clone(),
    }
}

/// The class names a fingerprint keeps: up to 5, less those that look
/// generated (CSS-in-JS and CSS modules hashes such as `css-1x2y3z`,
/// `sc-bdVaJa`, `Button_root__3xYz9`), which change with every build.
pub fn kept_classes(classes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for class in classes {
        if !generated(class) && !out.contains(class) {
            out.push(class.clone());
        }
        if out.len() == CLASSES_KEPT {
            break;
        }
    }
    out
}

/// Whether class name `class` looks made by a build: a known prefix of
/// CSS-in-JS libraries, a part (between `-` and `_`) of 5 or more
/// characters that mixes letters and digits, or a short part of random
/// case (`kJxvGt`).
fn generated(class: &str) -> bool {
    const PREFIXES: [&str; 6] = ["css-", "sc-", "jsx-", "emotion-", "svelte-", "tw-"];
    if PREFIXES.iter().any(|p| class.starts_with(p)) {
        return true;
    }
    class.split(['-', '_']).any(|part| {
        let digits = part.chars().any(|c| c.is_ascii_digit());
        let letters = part.chars().any(|c| c.is_ascii_alphabetic());
        let upper = part.chars().filter(char::is_ascii_uppercase).count();
        let random_case =
            part.len() <= 8 && part.starts_with(|c: char| c.is_ascii_lowercase()) && upper >= 2;
        (part.len() >= 5 && digits && letters) || random_case
    })
}

/// The path of a page as a template: numbers, UUIDs and long hex runs
/// become `:id` (`/products/12` is `/products/:id`).
pub fn page_template(path: &str) -> String {
    path.split('/')
        .map(|part| if is_id(part) { ":id" } else { part })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_id(part: &str) -> bool {
    if part.is_empty() {
        return false;
    }
    if part.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    let hex = part.chars().all(|c| c.is_ascii_hexdigit());
    let uuid = part.len() == 36
        && part.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    uuid || (hex && part.len() >= 8 && part.chars().any(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use protocol::schema::{Fingerprint, FingerprintContainer};

    use super::*;

    fn button(name: &str) -> Fingerprint {
        Fingerprint {
            role: "button".into(),
            name: name.into(),
            tag: "button".into(),
            text: name.into(),
            classes: vec!["button".into(), "button-primary".into()],
            container: Some(FingerprintContainer {
                role: "main".into(),
                name: String::new(),
            }),
            index: Some(0),
            ..Fingerprint::default()
        }
    }

    fn with_testid(mut fp: Fingerprint, id: &str) -> Fingerprint {
        fp.testid = id.into();
        fp
    }

    #[test]
    fn similar_texts() {
        assert!(similar("Add to cart", "Add to basket"));
        assert!(similar("Add to cart", "add  to cart"));
        assert!(similar("Place order", "Place your order"));
        assert!(similar("Remove Keyboard", "Delete Keyboard"));
        assert!(similar("Sign in", "Sign in now"));
        assert!(similar("Checkout", "Check out"));
        assert!(!similar("Sign in", "Log in"));
        assert!(!similar("Back to products", "All products"));
        assert!(!similar("in", "Sign in"));
        assert!(!similar("", "x"));
    }

    #[test]
    fn a_renamed_button_with_its_test_id_is_high() {
        let fp = with_testid(button("Add to cart"), "add-to-cart");
        let mut now = with_testid(button("Add to basket"), "add-to-cart");
        now.classes = vec!["btn".into(), "btn--primary".into()];
        let s = score(&fp, &now);
        // 2 + 3 + 2 + 2 + 1 + 0.5 of 2 + 3 + 3 + 2 + 1 + 1 + 0.5.
        assert!((s.score - 10.5 / 12.5).abs() < 1e-9, "{s:?}");
        assert_eq!(
            s.reasons,
            [
                "same testid",
                "same role",
                "similar name",
                "same container",
                "same tag"
            ]
        );
        let ranked = rank(&fp, &[button("Search"), now], 3);
        assert_eq!(ranked[0].index, 1);
        assert_eq!(level(ranked[0].confidence), "high");
        assert!((ranked[0].confidence - 0.84).abs() < 1e-9);
    }

    #[test]
    fn a_new_test_id_with_the_rest_the_same_is_high() {
        let fp = Fingerprint {
            role: "strong".into(),
            tag: "strong".into(),
            testid: "price".into(),
            text: "$89.00".into(),
            container: Some(FingerprintContainer {
                role: "main".into(),
                name: String::new(),
            }),
            index: Some(0),
            ..Fingerprint::default()
        };
        let now = Fingerprint {
            testid: "product-price".into(),
            ..fp.clone()
        };
        let ranked = rank(&fp, &[now], 3);
        assert!((ranked[0].score - 8.5 / 10.5).abs() < 1e-9);
        assert_eq!(level(ranked[0].confidence), "high");
        assert_eq!(ranked[0].reasons[0], "testid differs");
    }

    #[test]
    fn another_role_gets_half_marks_at_most() {
        let fp = button("Save");
        let mut heading = button("Save");
        heading.role = "heading".into();
        heading.tag = "h2".into();
        assert!((score(&fp, &heading).score - 0.5).abs() < 1e-9);
        // A link is of the button's family: half the role's weight, no cap.
        let mut link = button("Save");
        link.role = "link".into();
        link.tag = "a".into();
        let s = score(&fp, &link).score;
        assert!(s > 0.5 && s < 1.0, "{s}");
    }

    #[test]
    fn a_close_runner_up_lowers_the_confidence() {
        let fp = with_testid(button("Remove"), "remove");
        let twin = with_testid(button("Remove"), "remove");
        let ranked = rank(&fp, &[twin.clone(), twin], 3);
        // Equal scores: half marks.
        assert!((ranked[0].confidence - 0.5).abs() < 1e-9);
        assert_eq!(level(ranked[0].confidence), "low");
        assert!(rank(&fp, &[], 3).is_empty());
    }

    #[test]
    fn a_removed_link_is_low() {
        let mut fp = button("Back to products");
        fp.role = "link".into();
        fp.tag = "a".into();
        fp.classes.clear();
        let mut shop = fp.clone();
        shop.name = "Shop".into();
        shop.text = "Shop".into();
        shop.container = Some(FingerprintContainer {
            role: "navigation".into(),
            name: String::new(),
        });
        let ranked = rank(&fp, &[shop], 3);
        assert_eq!(level(ranked[0].confidence), "low");
        assert_eq!(
            ranked[0].reasons,
            ["same role", "name differs", "container differs", "same tag"]
        );
    }

    #[test]
    fn a_label_that_is_the_name_counts_once() {
        let fp = Fingerprint {
            role: "textbox".into(),
            name: "Email".into(),
            label: "Email".into(),
            tag: "input".into(),
            kind: "email".into(),
            ..Fingerprint::default()
        };
        let mut now = fp.clone();
        now.name = "E-mail".into();
        now.label = "E-mail".into();
        let s = score(&fp, &now);
        assert!(!s.reasons.iter().any(|r| r.contains("label")), "{s:?}");
        assert!((s.score - (3.0 + 2.0 + 1.0) / 7.0).abs() < 1e-9);
    }

    #[test]
    fn generated_class_names_are_left_out() {
        let classes: Vec<String> = [
            "btn",
            "css-1x2y3z",
            "sc-bdVaJa",
            "kJxvGt",
            "Button_root__3xYz9",
            "btn--primary",
            "card-title",
            "ButtonPrimary",
            "a",
            "b",
            "c",
            "btn",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            kept_classes(&classes),
            ["btn", "btn--primary", "card-title", "ButtonPrimary", "a"]
        );
    }

    #[test]
    fn paths_become_templates() {
        assert_eq!(page_template("/products/12"), "/products/:id");
        assert_eq!(page_template("/"), "/");
        assert_eq!(
            page_template("/orders/0f8fad5b-d9cb-469f-a165-70867728950e/items"),
            "/orders/:id/items"
        );
        assert_eq!(page_template("/c/deadbeef42"), "/c/:id");
        assert_eq!(page_template("/c/facade"), "/c/facade");
        assert_eq!(page_template("/v2/search"), "/v2/search");
    }

    #[test]
    fn elements_are_described_by_role_and_name() {
        assert_eq!(element(&button("Add")), "button \"Add\"");
        let strong = Fingerprint {
            role: "strong".into(),
            text: "$1".into(),
            ..Fingerprint::default()
        };
        assert_eq!(element(&strong), "strong \"$1\"");
    }
}
