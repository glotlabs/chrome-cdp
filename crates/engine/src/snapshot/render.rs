//! Snapshot text and JSON (plan 04, 4.1), and `--diff`.
//!
//! One node per line, two spaces per level:
//! `- role "name" [ref] (hN) (password) value="…" states… → href`.

use std::collections::HashMap;
use std::fmt::Write as _;

use protocol::schema::SnapshotNode;

use super::tree::{Node, NodeKey};

/// What changed about a node, for `--diff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Added,
    Removed,
    /// Its line changed; `was` holds the old parts.
    Changed,
}

impl Change {
    fn mark(self) -> char {
        match self {
            Self::Added => '+',
            Self::Removed => '-',
            Self::Changed => '~',
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
        }
    }
}

/// A node of a diff: an unchanged one is context for its changes.
#[derive(Debug, Clone)]
pub(crate) struct DiffNode {
    pub(crate) node: Node,
    pub(crate) change: Option<Change>,
    pub(crate) was: Option<String>,
    pub(crate) children: Vec<DiffNode>,
}

/// `"text"` with `"` and `\` escaped.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The line of `node`, without indentation or the leading `- `.
pub(crate) fn line(node: &Node) -> String {
    let mut line = node.role.clone();
    if let Some(more) = node.more {
        return format!("… {more} more {}", node.role);
    }
    if !node.name.is_empty() {
        line.push(' ');
        line.push_str(&quote(&node.name));
    }
    if let Some(reference) = node.reference {
        let _ = write!(line, " [e{reference}]");
    }
    for part in details(node) {
        line.push(' ');
        line.push_str(&part);
    }
    line
}

/// What follows the ref: level, password or file, value, states, href.
fn details(node: &Node) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(level) = node.level {
        parts.push(format!("(h{level})"));
    }
    if node.password {
        parts.push("(password)".to_owned());
    }
    if node.file {
        parts.push("(file)".to_owned());
    }
    if let Some(value) = &node.value {
        parts.push(format!("value={}", quote(value)));
    }
    parts.extend(states(node).into_iter().map(str::to_owned));
    if let Some(href) = &node.href {
        parts.push(format!("→ {href}"));
    }
    parts
}

fn states(node: &Node) -> Vec<&'static str> {
    let mut words = node.states.words();
    if node.scrollable {
        words.push("scrollable");
    }
    words
}

/// The snapshot as text, after its header line.
pub(crate) fn text(nodes: &[Node], out: &mut String) {
    lines(nodes, 0, None, out);
}

fn lines(nodes: &[Node], depth: usize, parent: Option<u32>, out: &mut String) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        out.push('\n');
        out.push_str(&indent);
        out.push_str("- ");
        out.push_str(&line(node));
        if node.more.is_some() {
            match parent {
                Some(r) => {
                    let _ = write!(out, " (use --scope e{r} --all)");
                }
                None => out.push_str(" (use --all)"),
            }
        }
        if let Some(below) = node.below {
            let _ = write!(out, "\n{indent}  - … {below} more below");
            match node.reference {
                Some(r) => {
                    let _ = write!(out, " (use --scope e{r})");
                }
                None => out.push_str(" (use a larger --depth)"),
            }
        }
        lines(&node.children, depth + 1, node.reference, out);
    }
}

/// The `--json` form of `node`.
pub(crate) fn json(node: &Node) -> SnapshotNode {
    SnapshotNode {
        role: node.role.clone(),
        name: node.name.clone(),
        reference: node.reference.map(|r| format!("e{r}")),
        level: node.level,
        value: node.value.clone(),
        password: node.password,
        file: node.file,
        states: states(node).into_iter().map(str::to_owned).collect(),
        href: node.href.clone(),
        more: node.more,
        below: node.below,
        change: None,
        was: None,
        children: node.children.iter().map(json).collect(),
    }
}

/// How a node is matched across two snapshots: its DOM node, else its
/// place (the parent's identity, role, name and how many alike came
/// before it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Identity {
    Dom(NodeKey),
    Place(Box<Identity>, String, String, usize),
    Root,
}

/// The identity of every node of `nodes`, parents first, in the order
/// [`walk_ids`] visits them.
fn walk_ids(nodes: &[Node], parent: &Identity, out: &mut Vec<(Identity, Identity)>) {
    let mut seen: HashMap<(&str, &str), usize> = HashMap::new();
    for node in nodes {
        let id = match &node.key {
            Some(key) if node.more.is_none() => Identity::Dom(key.clone()),
            _ => {
                let n = seen.entry((&node.role, &node.name)).or_default();
                *n += 1;
                Identity::Place(
                    Box::new(parent.clone()),
                    node.role.clone(),
                    node.name.clone(),
                    *n,
                )
            }
        };
        out.push((id.clone(), parent.clone()));
        walk_ids(&node.children, &id, out);
    }
}

/// The changes from `old` to `new`, with the unchanged ancestors of each
/// change kept as context. An added node brings its whole subtree; a
/// removed one only itself.
pub(crate) fn diff(old: &[Node], new: &[Node]) -> Vec<DiffNode> {
    let mut old_ids = Vec::new();
    walk_ids(old, &Identity::Root, &mut old_ids);
    let mut new_ids = Vec::new();
    walk_ids(new, &Identity::Root, &mut new_ids);
    let mut old_nodes: Vec<&Node> = Vec::new();
    collect(old, &mut old_nodes);
    let old_by_id: HashMap<&Identity, (usize, &Identity)> = old_ids
        .iter()
        .enumerate()
        .map(|(i, (id, parent))| (id, (i, parent)))
        .collect();
    let new_set: std::collections::HashSet<&Identity> = new_ids.iter().map(|(id, _)| id).collect();

    // Removed nodes, grouped under the parent they had (only the topmost
    // of a removed subtree).
    let mut removed: HashMap<&Identity, Vec<&Node>> = HashMap::new();
    for (i, (id, parent)) in old_ids.iter().enumerate() {
        let parent_gone = *parent != Identity::Root && !new_set.contains(parent);
        if !new_set.contains(id) && !parent_gone {
            removed.entry(parent).or_default().push(old_nodes[i]);
        }
    }

    let mut ids = new_ids.iter().map(|(id, _)| id);
    let mut tree = build(new, &mut ids, &old_by_id, &old_nodes, &removed, false);
    tree.extend(removed_nodes(removed.get(&Identity::Root)));
    prune_unchanged(tree)
}

fn collect<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
    for node in nodes {
        out.push(node);
        collect(&node.children, out);
    }
}

fn build<'a>(
    nodes: &[Node],
    ids: &mut impl Iterator<Item = &'a Identity>,
    old: &HashMap<&Identity, (usize, &Identity)>,
    old_nodes: &[&Node],
    removed: &HashMap<&Identity, Vec<&Node>>,
    in_added: bool,
) -> Vec<DiffNode> {
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let Some(id) = ids.next() else { break };
        let (change, was) = match old.get(id) {
            _ if in_added => (Some(Change::Added), None),
            None => (Some(Change::Added), None),
            Some((i, _)) => {
                let before = old_nodes[*i];
                if line(before) == line(node) {
                    (None, None)
                } else {
                    (Some(Change::Changed), Some(was(before, node)))
                }
            }
        };
        let added = change == Some(Change::Added);
        let mut children = build(&node.children, ids, old, old_nodes, removed, added);
        children.extend(removed_nodes(removed.get(id)));
        out.push(DiffNode {
            node: Node {
                children: Vec::new(),
                ..node.clone()
            },
            change,
            was,
            children,
        });
    }
    out
}

fn removed_nodes(nodes: Option<&Vec<&Node>>) -> Vec<DiffNode> {
    nodes
        .into_iter()
        .flatten()
        .map(|node| DiffNode {
            node: Node {
                children: Vec::new(),
                ..(*node).clone()
            },
            change: Some(Change::Removed),
            was: None,
            children: Vec::new(),
        })
        .collect()
}

/// The parts of `before`'s line that differ from `after`'s.
fn was(before: &Node, after: &Node) -> String {
    let mut parts = Vec::new();
    if before.role != after.role {
        parts.push(before.role.clone());
    }
    if before.name != after.name {
        parts.push(if before.name.is_empty() {
            "no name".to_owned()
        } else {
            quote(&before.name)
        });
    }
    if before.reference != after.reference {
        parts.push(
            before
                .reference
                .map_or("no ref".to_owned(), |r| format!("[e{r}]")),
        );
    }
    let (old, new) = (details(before), details(after));
    parts.extend(old.iter().filter(|p| !new.contains(p)).cloned());
    if parts.is_empty() {
        // Only additions: say what it did not have.
        for part in new.iter().filter(|p| !old.contains(p)) {
            parts.push(match part.split_once(['=', ' ']) {
                Some(("value", _)) => "no value".to_owned(),
                Some(("→", _)) => "no link".to_owned(),
                _ => format!("not {part}"),
            });
        }
    }
    parts.join(" ")
}

/// Drops unchanged subtrees with no change below them.
fn prune_unchanged(nodes: Vec<DiffNode>) -> Vec<DiffNode> {
    nodes
        .into_iter()
        .filter_map(|mut node| {
            node.children = prune_unchanged(std::mem::take(&mut node.children));
            (node.change.is_some() || !node.children.is_empty()).then_some(node)
        })
        .collect()
}

/// The diff as text, after its header line.
pub(crate) fn diff_text(nodes: &[DiffNode], out: &mut String) {
    for node in nodes {
        diff_lines(node, 0, out);
    }
}

fn diff_lines(node: &DiffNode, depth: usize, out: &mut String) {
    out.push('\n');
    out.push(node.change.map_or(' ', Change::mark));
    out.push(' ');
    out.push_str(&"  ".repeat(depth));
    out.push_str("- ");
    out.push_str(&line(&node.node));
    if let Some(was) = &node.was {
        let _ = write!(out, " (was {was})");
    }
    for child in &node.children {
        diff_lines(child, depth + 1, out);
    }
}

/// The `--json` form of a diff node.
pub(crate) fn diff_json(node: &DiffNode) -> SnapshotNode {
    SnapshotNode {
        change: node.change.map(|c| c.as_str().to_owned()),
        was: node.was.clone(),
        children: node.children.iter().map(diff_json).collect(),
        ..json(&node.node)
    }
}

/// Every node of `nodes` as added: a diff with nothing to compare with.
pub(crate) fn all_added(nodes: &[Node]) -> Vec<DiffNode> {
    nodes
        .iter()
        .map(|node| DiffNode {
            node: Node {
                children: Vec::new(),
                ..node.clone()
            },
            change: Some(Change::Added),
            was: None,
            children: all_added(&node.children),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tree::{Checked, Node, NodeKey, States};
    use super::{diff, diff_text, line, text};

    fn keyed(role: &str, name: &str, n: i64, reference: Option<u32>, children: Vec<Node>) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            key: Some(NodeKey {
                frame: "F".into(),
                backend: n,
            }),
            reference,
            children,
            ..Node::default()
        }
    }

    #[test]
    fn lines_follow_the_format() {
        let mut textbox = keyed("textbox", "Email", 1, Some(3), vec![]);
        textbox.value = Some("a\"b".into());
        textbox.states.required = true;
        assert_eq!(
            line(&textbox),
            r#"textbox "Email" [e3] value="a\"b" required"#
        );
        let mut heading = keyed("heading", "Sign in", 2, None, vec![]);
        heading.level = Some(1);
        assert_eq!(line(&heading), r#"heading "Sign in" (h1)"#);
        let mut link = keyed("link", "Pricing", 3, Some(2), vec![]);
        link.href = Some("/pricing".into());
        assert_eq!(line(&link), r#"link "Pricing" [e2] → /pricing"#);
        let mut password = keyed("textbox", "Password", 4, Some(4), vec![]);
        password.password = true;
        assert_eq!(line(&password), r#"textbox "Password" [e4] (password)"#);
        let mut file = keyed("button", "Avatar", 5, Some(5), vec![]);
        file.file = true;
        assert_eq!(line(&file), r#"button "Avatar" [e5] (file)"#);
    }

    #[test]
    fn text_nests_and_names_the_scope_for_more() {
        let mut list = keyed(
            "list",
            "",
            1,
            Some(40),
            vec![keyed("listitem", "A", 2, None, vec![])],
        );
        list.children.push(Node {
            role: "listitem".into(),
            more: Some(170),
            ..Node::default()
        });
        let mut out = String::new();
        text(&[list], &mut out);
        assert_eq!(
            out,
            "\n- list [e40]\n  - listitem \"A\"\n  - … 170 more listitem (use --scope e40 --all)"
        );
    }

    #[test]
    fn diff_marks_added_removed_and_changed_with_context() {
        let checkbox = |checked| {
            let mut n = keyed("checkbox", "Remember me", 5, Some(5), vec![]);
            n.states = States {
                checked: Some(checked),
                ..States::default()
            };
            n
        };
        let old = vec![keyed(
            "main",
            "",
            1,
            None,
            vec![
                keyed("heading", "Title", 2, None, vec![]),
                keyed(
                    "form",
                    "Login",
                    3,
                    None,
                    vec![
                        checkbox(Checked::False),
                        keyed("button", "Sign in", 6, Some(6), vec![]),
                    ],
                ),
            ],
        )];
        let new = vec![keyed(
            "main",
            "",
            1,
            None,
            vec![
                keyed("heading", "Title", 2, None, vec![]),
                keyed(
                    "form",
                    "Login",
                    3,
                    None,
                    vec![
                        checkbox(Checked::True),
                        keyed("alert", "Invalid password", 7, None, vec![]),
                    ],
                ),
            ],
        )];
        let mut out = String::new();
        diff_text(&diff(&old, &new), &mut out);
        assert_eq!(
            out,
            "
  - main
    - form \"Login\"
~     - checkbox \"Remember me\" [e5] checked (was unchecked)
+     - alert \"Invalid password\"
-     - button \"Sign in\" [e6]"
        );
        assert!(diff(&new, &new).is_empty());
    }

    #[test]
    fn keyless_nodes_match_by_place() {
        let old = vec![Node::text("Hello", None)];
        let new = vec![Node::text("Hello", None), Node::text("World", None)];
        let mut out = String::new();
        diff_text(&diff(&old, &new), &mut out);
        assert_eq!(out, "\n+ - text \"World\"");
    }
}
