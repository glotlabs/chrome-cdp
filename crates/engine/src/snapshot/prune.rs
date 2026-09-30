//! Shapes an engine's tree into what a snapshot shows (plan 04, 4.1):
//! pruning, the clickable promotion of 4.3, the `--interactive` and
//! `--depth` views, long-list collapsing and refs.
//!
//! The order matters: prune first (so hoisted text merges), then the
//! views, then collapse what is still long, and give refs last, only to
//! nodes that are shown.

use std::collections::HashMap;

use super::refs::Refs;
use super::tree::{
    CLICKABLE, IFRAME, Node, NodeKey, is_interactive, is_landmark, is_structural, is_text_field,
    name_from_content, shows_value,
};
use crate::request::SnapshotEngine;

/// How many siblings of one role a list shows before the rest collapse
/// into one `… N more` line.
pub(crate) const LIST_LIMIT: usize = 30;

/// How long a shown value may be.
const VALUE_LIMIT: usize = 100;

/// A role that marks a block boundary between hoisted children while
/// pruning, so text on either side does not merge. Never shown.
const BREAK: &str = "\u{0}break";

/// What the snapshot shows (its flags).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct View {
    /// `--interactive`: only reffed nodes and their landmarks.
    pub(crate) interactive: bool,
    /// `--depth`: levels shown.
    pub(crate) depth: Option<u32>,
    /// `--all-refs`: a ref on every node.
    pub(crate) all_refs: bool,
    /// Collapse long lists (off with `--all`).
    pub(crate) collapse: bool,
    /// Longest text or name shown (0 = no limit).
    pub(crate) text_limit: usize,
    /// `--no-text`: no text nodes.
    pub(crate) no_text: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            interactive: false,
            depth: None,
            all_refs: false,
            collapse: true,
            text_limit: 200,
            no_text: false,
        }
    }
}

/// The node `key` names in `nodes`, taken out with its subtree.
pub(crate) fn find(nodes: &[Node], key: &NodeKey) -> Option<Node> {
    for node in nodes {
        if node.key.as_ref() == Some(key) {
            return Some(node.clone());
        }
        if let Some(found) = find(&node.children, key) {
            return Some(found);
        }
    }
    None
}

/// Applies everything but refs: pruning, the views and collapsing.
pub(crate) fn shape(nodes: Vec<Node>, view: &View) -> Vec<Node> {
    let mut nodes = shape_uncollapsed(nodes, view);
    if view.collapse {
        collapse(&mut nodes);
    }
    nodes
}

/// [`shape`] without collapsing long lists: folding comes first
/// ([`super::fold`]).
pub(crate) fn shape_uncollapsed(nodes: Vec<Node>, view: &View) -> Vec<Node> {
    let mut nodes = finish(prune_list(nodes, view, false), view);
    if view.interactive {
        nodes = interactive(nodes);
    }
    if let Some(depth) = view.depth {
        limit_depth(&mut nodes, depth.max(1));
    }
    nodes
}

/// Gives every reffable node shown (every node with `--all-refs`) its
/// ref, in document order. `engine` read them. Folded nodes get theirs
/// too, so refs from an earlier snapshot keep working.
pub(crate) fn assign_refs(nodes: &mut [Node], refs: &mut Refs, all: bool, engine: SnapshotEngine) {
    for node in nodes {
        if (node.reffable || all)
            && node.more.is_none()
            && let Some(key) = &node.key
        {
            node.reference = Some(refs.assign_from(key, &node.role, &node.name, engine));
        }
        assign_refs(&mut node.children, refs, all, engine);
        if let Some(fold) = &mut node.fold {
            assign_refs(&mut fold.hidden, refs, all, engine);
        }
    }
}

/// Looks up the refs nodes already have, giving none: for a tree that is
/// not shown as such (the old side of a diff).
pub(crate) fn lookup_refs(nodes: &mut [Node], refs: &Refs) {
    for node in nodes {
        if node.more.is_none()
            && let Some(key) = &node.key
        {
            node.reference = refs.get(key);
        }
        lookup_refs(&mut node.children, refs);
    }
}

fn prune_list(nodes: Vec<Node>, view: &View, inside: bool) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        out.extend(prune(node, view, inside));
    }
    merge_text(out)
}

/// Prunes one node; `inside` says an ancestor is a control (a clickable
/// inside it is part of it). Returns what takes its place: itself,
/// its hoisted children (between block breaks), or nothing.
fn prune(mut node: Node, view: &View, inside: bool) -> Vec<Node> {
    if node.is_text() {
        return if view.no_text { Vec::new() } else { vec![node] };
    }
    if node.clickable
        && !inside
        && !is_interactive(&node.role)
        && !is_landmark(&node.role)
        && node.role != "label"
    {
        if is_structural(&node.role) {
            CLICKABLE.clone_into(&mut node.role);
        }
        node.reffable = true;
    }
    if is_interactive(&node.role) || node.scrollable {
        node.reffable = true;
    }
    // What is clicked inside a control or clickable is part of it; a
    // scrollable box or an iframe only holds things.
    let control = node.role == CLICKABLE || (is_interactive(&node.role) && node.role != IFRAME);
    node.children = if is_text_field(&node.role) {
        Vec::new()
    } else {
        prune_list(std::mem::take(&mut node.children), view, inside || control)
    };
    if is_structural(&node.role) && clean(&node.name).is_empty() && !node.reffable {
        return hoist(node);
    }
    node.children = finish(std::mem::take(&mut node.children), view);
    node.name = clean(&node.name);
    if node.role == CLICKABLE && node.name.is_empty() {
        node.name = clean(&all_text(&node.children));
    }
    if !node.reffable && node.name.is_empty() && node.children.is_empty() {
        // An unnamed image or figure with nothing in it says nothing.
        if matches!(node.role.as_str(), "img" | "figure" | "list" | "listitem") {
            return Vec::new();
        }
    }
    if !node.name.is_empty() {
        // A legend or caption that repeats the name.
        if node
            .children
            .first()
            .is_some_and(|c| c.is_text() && c.name == node.name)
        {
            node.children.remove(0);
        }
        if name_from_content(&node.role) {
            node.children.retain(has_reffable);
            // `cell "Edit Delete"` over its two buttons: the children say it.
            if !node.reffable
                && !node.children.is_empty()
                && clean(&all_text(&node.children)) == node.name
            {
                node.name.clear();
            }
        } else if let [only] = node.children.as_slice()
            && only.is_text()
            && only.name == node.name
        {
            node.children.clear();
        }
    } else if let [only] = node.children.as_slice()
        && only.is_text()
        && node.role != IFRAME
    {
        // `listitem "Apples"`: an unnamed node whose only content is text
        // shows it as its name.
        node.name.clone_from(&only.name);
        node.children.clear();
    }
    // An editable combobox's text is its value.
    if let Some(value) = &node.value {
        let value = clean(value);
        node.children.retain(|c| !(c.is_text() && c.name == value));
    }
    node.name = truncate(&node.name, view.text_limit);
    node.value = if node.password || !shows_value(&node.role) {
        None
    } else {
        node.value
            .take()
            .map(|v| truncate(&clean(&v), VALUE_LIMIT))
            .filter(|v| !v.is_empty())
    };
    if node.role != "heading" {
        node.level = None;
    }
    if node.role != "link" {
        node.href = None;
    }
    vec![node]
}

/// A structural node's children in its place; a block keeps its text
/// apart from its neighbours'.
fn hoist(node: Node) -> Vec<Node> {
    if node.inline {
        return node.children;
    }
    let mut out = Vec::with_capacity(node.children.len() + 2);
    out.push(block_break());
    out.extend(node.children);
    out.push(block_break());
    out
}

fn block_break() -> Node {
    Node {
        role: BREAK.to_owned(),
        ..Node::default()
    }
}

/// Joins runs of text nodes as they are (spaces at the edges matter until
/// the run is complete), but not across block breaks.
fn merge_text(nodes: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
    for node in nodes {
        match out.last_mut() {
            Some(last) if last.is_text() && node.is_text() => last.name.push_str(&node.name),
            _ => out.push(node),
        }
    }
    out
}

/// A node's final children: without block breaks, text tidied and cut,
/// empty text and label text that repeats a control's name dropped.
fn finish(nodes: Vec<Node>, view: &View) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len());
    for mut node in nodes {
        if node.role == BREAK {
            continue;
        }
        if node.is_text() {
            node.name = truncate(&clean(&node.name), view.text_limit);
            if node.name.is_empty() {
                continue;
            }
        }
        out.push(node);
    }
    drop_label_text(out)
}

/// Drops text that only repeats the name of the control next to it: a
/// `<label>`'s text, which the control's name already says.
fn drop_label_text(nodes: Vec<Node>) -> Vec<Node> {
    let repeats = |text: &Node, other: Option<&Node>| {
        other.is_some_and(|o| o.reffable && !o.is_text() && o.name == text.name)
    };
    let keep: Vec<bool> = (0..nodes.len())
        .map(|i| {
            let node = &nodes[i];
            !node.is_text()
                || !(repeats(node, i.checked_sub(1).and_then(|p| nodes.get(p)))
                    || repeats(node, nodes.get(i + 1)))
        })
        .collect();
    nodes
        .into_iter()
        .zip(keep)
        .filter_map(|(node, keep)| keep.then_some(node))
        .collect()
}

/// Whether `node` or a node below it gets a ref.
fn has_reffable(node: &Node) -> bool {
    node.reffable || node.children.iter().any(has_reffable)
}

/// The text of `nodes` and everything below them, names included.
fn all_text(nodes: &[Node]) -> String {
    let mut parts = Vec::new();
    for node in nodes {
        node.walk(&mut |n| {
            if !n.name.is_empty() && (n.is_text() || n.children.is_empty()) {
                parts.push(n.name.as_str());
            }
        });
    }
    parts.join(" ")
}

/// `--interactive`: reffed nodes, with the landmarks around them, and the
/// tables and rows that keep a row's controls together.
pub(crate) fn interactive(nodes: Vec<Node>) -> Vec<Node> {
    let mut out = Vec::new();
    for mut node in nodes {
        let children = interactive(std::mem::take(&mut node.children));
        let context = is_landmark(&node.role)
            || matches!(node.role.as_str(), "table" | "grid" | "treegrid" | "row");
        if node.reffable || (context && !children.is_empty()) {
            node.children = children;
            out.push(node);
        } else {
            out.extend(children);
        }
    }
    out
}

/// `--depth`: cuts what is deeper, counting it in `below`.
fn limit_depth(nodes: &mut [Node], depth: u32) {
    for node in nodes {
        if depth <= 1 {
            if !node.children.is_empty() {
                node.below = Some(u32::try_from(node.descendants()).unwrap_or(u32::MAX));
                node.children.clear();
            }
        } else {
            limit_depth(&mut node.children, depth - 1);
        }
    }
}

/// Collapses a long list: once a role has had [`LIST_LIMIT`] siblings, the
/// list is cut there, and what follows becomes one `more` placeholder per
/// role (text is dropped; a few nodes of rarer roles stay). Cutting once,
/// rather than per role, keeps mixed lists (a row's checkbox, link and
/// buttons) together. Returns whether it cut `nodes`; a parent that loses
/// children gets a ref, so the hint can name it.
/// A fold's run stub is kept where it is and not counted.
pub(crate) fn collapse(nodes: &mut Vec<Node>) -> bool {
    for node in nodes.iter_mut() {
        if collapse(&mut node.children) {
            node.reffable = true;
        }
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut cut = None;
    let mut totals: HashMap<&str, usize> = HashMap::new();
    let counted = |n: &Node| !n.is_text() && n.fold.as_ref().is_none_or(|f| f.like.is_none());
    for node in nodes.iter().filter(|n| counted(n)) {
        *totals.entry(node.role.as_str()).or_default() += 1;
    }
    for (i, node) in nodes.iter().enumerate() {
        if !counted(node) {
            continue;
        }
        let count = seen.entry(node.role.as_str()).or_default();
        *count += 1;
        if *count > LIST_LIMIT {
            cut = Some(i);
            break;
        }
    }
    let Some(cut) = cut else {
        return false;
    };
    let over: Vec<String> = totals
        .into_iter()
        .filter(|(_, n)| *n > LIST_LIMIT)
        .map(|(role, _)| role.to_owned())
        .collect();
    let tail = nodes.split_off(cut);
    let mut more: Vec<(String, u32)> = Vec::new();
    let mut kept = Vec::new();
    for node in tail {
        if node.is_text() {
            continue;
        }
        if counted(&node) && over.contains(&node.role) {
            match more.iter_mut().find(|(role, _)| *role == node.role) {
                Some((_, n)) => *n += 1,
                None => more.push((node.role, 1)),
            }
        } else {
            kept.push(node);
        }
    }
    nodes.extend(more.into_iter().map(|(role, n)| Node {
        role,
        more: Some(n),
        ..Node::default()
    }));
    nodes.extend(kept);
    true
}

/// Collapses whitespace runs to one space and trims.
pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Cuts `text` to `limit` characters (0 = no limit), ending in `…`.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if limit == 0 {
        return text.to_owned();
    }
    match text.char_indices().nth(limit) {
        None => text.to_owned(),
        Some((at, _)) => {
            let kept = text[..at].trim_end();
            format!("{kept}…")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::super::tree::{Checked, NONE, Node, NodeKey, States, TEXT};
    use super::{LIST_LIMIT, View, assign_refs, find, shape, truncate};
    use crate::request::SnapshotEngine;
    use crate::snapshot::refs::Refs;

    fn key(n: i64) -> NodeKey {
        NodeKey {
            frame: "F".into(),
            backend: n,
        }
    }

    fn el(role: &str, name: &str, children: Vec<Node>) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            children,
            ..Node::default()
        }
    }

    fn keyed(role: &str, name: &str, n: i64, children: Vec<Node>) -> Node {
        Node {
            key: Some(key(n)),
            ..el(role, name, children)
        }
    }

    fn text(t: &str) -> Node {
        Node::text(t, None)
    }

    /// `role "name"` lines, indented, for comparing shapes.
    fn lines(nodes: &[Node]) -> Vec<String> {
        fn go(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
            for n in nodes {
                let mut line = format!("{}{}", "  ".repeat(depth), n.role);
                if !n.name.is_empty() {
                    let _ = write!(line, " {:?}", n.name);
                }
                if let Some(r) = n.reference {
                    let _ = write!(line, " [e{r}]");
                }
                if let Some(more) = n.more {
                    let _ = write!(line, " +{more}");
                }
                if let Some(below) = n.below {
                    let _ = write!(line, " v{below}");
                }
                out.push(line);
                go(&n.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        go(nodes, 0, &mut out);
        out
    }

    #[test]
    fn unnamed_groups_are_hoisted_and_inline_text_merges() {
        let bold = Node {
            inline: true,
            ..el("generic", "", vec![text("world")])
        };
        let page = vec![el(
            NONE,
            "",
            vec![
                el("paragraph", "", vec![text("Hello "), bold, text("! ok")]),
                el("paragraph", "", vec![text("Second")]),
                el("generic", "Named box", vec![text("x")]),
            ],
        )];
        assert_eq!(
            lines(&shape(page, &View::default())),
            [
                "text \"Hello world! ok\"",
                "text \"Second\"",
                "generic \"Named box\"",
                "  text \"x\"",
            ]
        );
    }

    #[test]
    fn spaces_at_the_edge_of_inline_elements_survive() {
        let span = |t: &str| Node {
            inline: true,
            ..el("generic", "", vec![text(t)])
        };
        let page = vec![el(
            "paragraph",
            "",
            vec![span("Hello "), text("world"), span(" again")],
        )];
        assert_eq!(
            lines(&shape(page, &View::default())),
            ["text \"Hello world again\""]
        );
    }

    #[test]
    fn names_that_repeat_the_content_drop_it() {
        let page = vec![
            el("heading", "Sign in", vec![text("Sign in")]),
            el(
                "link",
                "Docs Read more",
                vec![el("img", "Docs", vec![]), text("Read more")],
            ),
            el("listitem", "", vec![text(" Apples ")]),
            el("region", "Intro", vec![text("Intro")]),
            el(
                "textbox",
                "Email",
                vec![el("generic", "", vec![text("typed")])],
            ),
        ];
        assert_eq!(
            lines(&shape(page, &View::default())),
            [
                "heading \"Sign in\"",
                "link \"Docs Read more\"",
                "listitem \"Apples\"",
                "region \"Intro\"",
                "textbox \"Email\"",
            ]
        );
    }

    #[test]
    fn clickable_divs_are_promoted_but_not_inside_controls() {
        let div = |name: &str| Node {
            clickable: true,
            ..el("generic", "", vec![text(name)])
        };
        let page = vec![
            div("Open menu"),
            el("button", "Save", vec![div("Save")]),
            Node {
                clickable: true,
                ..el("label", "", vec![text("Email")])
            },
        ];
        let shaped = shape(page, &View::default());
        assert_eq!(
            lines(&shaped),
            [
                "clickable \"Open menu\"",
                "button \"Save\"",
                "text \"Email\""
            ]
        );
        assert!(shaped[0].reffable && shaped[1].reffable && !shaped[2].reffable);
    }

    #[test]
    fn interactive_keeps_refs_and_their_landmarks() {
        let page = vec![
            el(
                "main",
                "",
                vec![
                    el("heading", "Title", vec![]),
                    el(
                        "list",
                        "",
                        vec![el("listitem", "", vec![el("link", "A", vec![])])],
                    ),
                ],
            ),
            el("contentinfo", "", vec![text("© 2026")]),
        ];
        let view = View {
            interactive: true,
            ..View::default()
        };
        assert_eq!(lines(&shape(page, &view)), ["main", "  link \"A\""]);
    }

    #[test]
    fn long_lists_collapse_and_their_parent_gets_a_ref() {
        let items = (0..40)
            .map(|i| keyed("listitem", &format!("Item {i}"), 100 + i, vec![]))
            .collect();
        let page = vec![keyed("list", "", 1, items)];
        let mut shaped = shape(page.clone(), &View::default());
        let mut refs = Refs::default();
        assign_refs(&mut shaped, &mut refs, false, SnapshotEngine::Ax);
        let shown = lines(&shaped);
        assert_eq!(shown[0], "list [e1]");
        assert_eq!(shown.len(), 1 + LIST_LIMIT + 1);
        assert_eq!(shown[LIST_LIMIT + 1], "  listitem +10");
        let all = View {
            collapse: false,
            ..View::default()
        };
        assert_eq!(lines(&shape(page, &all)).len(), 41);
    }

    #[test]
    fn mixed_lists_are_cut_in_one_place() {
        let rows: Vec<Node> = (0..40)
            .flat_map(|i| {
                [
                    keyed("checkbox", &format!("Select {i}"), 1000 + i, vec![]),
                    keyed("button", "Edit", 2000 + i, vec![]),
                    keyed("button", "Delete", 3000 + i, vec![]),
                ]
            })
            .chain([keyed("searchbox", "Search", 9, vec![])])
            .collect();
        let shaped = shape(vec![keyed("main", "", 1, rows)], &View::default());
        let shown = lines(&shaped);
        // 15 rows (30 buttons) and the next checkbox, then the rest by role.
        assert_eq!(shown.len(), 1 + 46 + 3, "{shown:#?}");
        assert_eq!(
            shown[47..],
            ["  button +50", "  checkbox +24", "  searchbox \"Search\""]
        );
    }

    #[test]
    fn interactive_keeps_rows_together() {
        let row = |i: i64| {
            el(
                "row",
                "",
                vec![
                    el("cell", &format!("Invoice {i}"), vec![]),
                    el("cell", "", vec![keyed("button", "Delete", i, vec![])]),
                ],
            )
        };
        let page = vec![el("main", "", vec![el("table", "", vec![row(1), row(2)])])];
        let view = View {
            interactive: true,
            ..View::default()
        };
        assert_eq!(
            lines(&shape(page, &view)),
            [
                "main",
                "  table",
                "    row",
                "      button \"Delete\"",
                "    row",
                "      button \"Delete\"",
            ]
        );
    }

    #[test]
    fn depth_counts_what_it_leaves_out() {
        let page = vec![el(
            "main",
            "",
            vec![el("navigation", "", vec![el("link", "A", vec![])])],
        )];
        let view = View {
            depth: Some(1),
            ..View::default()
        };
        assert_eq!(lines(&shape(page, &view)), ["main v2"]);
    }

    #[test]
    fn values_states_and_passwords() {
        let page = vec![
            Node {
                value: Some("x".repeat(150)),
                ..el("textbox", "Bio", vec![])
            },
            Node {
                value: Some("hunter2".into()),
                password: true,
                ..el("textbox", "Password", vec![])
            },
            Node {
                states: States {
                    checked: Some(Checked::False),
                    ..States::default()
                },
                value: Some("on".into()),
                ..el("checkbox", "Remember", vec![])
            },
        ];
        let shaped = shape(page, &View::default());
        assert_eq!(shaped[0].value.as_ref().unwrap().chars().count(), 101);
        assert_eq!(shaped[1].value, None);
        assert_eq!(shaped[2].value, None);
        assert_eq!(shaped[2].states.words(), ["unchecked"]);
    }

    #[test]
    fn no_text_and_text_limit() {
        let page = vec![
            el("paragraph", "", vec![text(&"word ".repeat(100))]),
            el("button", "Go", vec![]),
        ];
        let view = View {
            text_limit: 20,
            ..View::default()
        };
        let shaped = shape(page.clone(), &view);
        assert_eq!(shaped[0].role, TEXT);
        assert_eq!(shaped[0].name, "word word word word…");
        let view = View {
            no_text: true,
            ..View::default()
        };
        assert_eq!(lines(&shape(page, &view)), ["button \"Go\""]);
    }

    #[test]
    fn scope_finds_a_subtree() {
        let page = vec![keyed(
            "main",
            "",
            1,
            vec![keyed(
                "form",
                "Login",
                2,
                vec![keyed("button", "Go", 3, vec![])],
            )],
        )];
        let form = find(&page, &key(2)).unwrap();
        assert_eq!(form.name, "Login");
        assert!(find(&page, &key(9)).is_none());
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        assert_eq!(truncate("héllo wörld", 5), "héllo…");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 0), "hello");
    }
}
