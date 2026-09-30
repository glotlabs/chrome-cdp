//! `compare`'s snapshot diff (plan 16, 16.5): two pages share no DOM
//! nodes, so nodes are matched by structure, not by key or ref. Siblings
//! are aligned by the longest common sequence of `(role, name)`; nodes
//! left over between two aligned ones with the same role count as
//! changed. The result is a diff of [`DiffNode`]s, shown as `snapshot
//! --diff` shows its own.
//!
//! A side read from a `snapshot --json` file or another session comes as
//! [`SnapshotNode`]s: [`from_json`] turns it back into [`Node`]s.

use protocol::schema::SnapshotNode;

use super::render::{Change, DiffNode, all_added, line, prune_unchanged, was};
use super::tree::{Checked, Node};

/// Siblings compared cell by cell at most: past it, the unaligned middle
/// of two long lists is one gap.
const MAX_CELLS: usize = 4_000_000;

/// The changes from `a` to `b`, with the unchanged ancestors of each
/// kept as context. An added node brings its whole subtree; a removed
/// one only itself. Refs are not compared.
pub(crate) fn diff(a: &[Node], b: &[Node]) -> Vec<DiffNode> {
    prune_unchanged(align(a, b))
}

/// How many changes `diff` marks: an added subtree counts once, as does
/// a removed node; a changed node's own changes below it count too.
pub(crate) fn count(diff: &[DiffNode]) -> usize {
    diff.iter()
        .map(|node| match node.change {
            Some(Change::Added | Change::Removed) => 1,
            Some(Change::Changed) => 1 + count(&node.children),
            None => count(&node.children),
        })
        .sum()
}

/// Takes refs and folds off `nodes`: a fold's run stub goes, a region
/// stub stays as its own line. Returns whether anything was folded.
pub(crate) fn neutral(nodes: &mut Vec<Node>) -> bool {
    let mut folded = false;
    nodes.retain(|n| n.fold.as_ref().is_none_or(|f| f.like.is_none()));
    for node in nodes.iter_mut() {
        folded |= node.fold.take().is_some();
        node.reference = None;
        node.key = None;
        folded |= neutral(&mut node.children);
    }
    folded
}

/// Rewrites links to `from` (an origin, `https://shop.example.com`) as
/// links to `to`, so two copies of a site on different origins compare
/// equal. A link to `from` itself becomes `/`.
pub(crate) fn rewrite_hrefs(nodes: &mut [Node], from: &str, to: &str) {
    for node in nodes {
        if let Some(href) = &node.href
            && let Some(rest) = href.strip_prefix(from)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            node.href = Some(if rest.is_empty() {
                format!("{to}/")
            } else {
                format!("{to}{rest}")
            });
        }
        rewrite_hrefs(&mut node.children, from, to);
    }
}

fn same(a: &Node, b: &Node) -> bool {
    a.role == b.role && a.name == b.name
}

/// The diff of two sibling lists, unpruned.
fn align(a: &[Node], b: &[Node]) -> Vec<DiffNode> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (ai, bj) in common(a, b).into_iter().chain([(a.len(), b.len())]) {
        gap(&a[i..ai], &b[j..bj], &mut out);
        if ai < a.len() {
            out.push(matched(&a[ai], &b[bj], None));
        }
        (i, j) = (ai + 1, bj + 1);
    }
    out
}

/// The index pairs of the longest common sequence of `(role, name)` of
/// `a` and `b`, in order.
fn common(a: &[Node], b: &[Node]) -> Vec<(usize, usize)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| same(x, y)).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| same(x, y))
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut pairs: Vec<(usize, usize)> = (0..prefix).map(|k| (k, k)).collect();
    if !am.is_empty() && !bm.is_empty() && am.len().saturating_mul(bm.len()) <= MAX_CELLS {
        // lengths[i][j]: the longest common sequence of am[i..] and bm[j..].
        let width = bm.len() + 1;
        let mut lengths = vec![0u32; (am.len() + 1) * width];
        for i in (0..am.len()).rev() {
            for j in (0..bm.len()).rev() {
                lengths[i * width + j] = if same(&am[i], &bm[j]) {
                    lengths[(i + 1) * width + j + 1] + 1
                } else {
                    lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < am.len() && j < bm.len() {
            if same(&am[i], &bm[j]) {
                pairs.push((prefix + i, prefix + j));
                (i, j) = (i + 1, j + 1);
            } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.extend((0..suffix).map(|k| (a.len() - suffix + k, b.len() - suffix + k)));
    pairs
}

/// What lies between two aligned siblings: nodes of `b` with the role of
/// a node of `a` not taken yet (in order) changed, the rest of `a`
/// removed (where it was) and the rest of `b` added.
fn gap(a: &[Node], b: &[Node], out: &mut Vec<DiffNode>) {
    let mut pairs: Vec<Option<usize>> = Vec::with_capacity(b.len());
    let mut next = 0;
    for node in b {
        let found = (next..a.len()).find(|&i| a[i].role == node.role);
        if let Some(i) = found {
            next = i + 1;
        }
        pairs.push(found);
    }
    // Removed nodes come before what was added in their place.
    let mut next = 0;
    for (j, node) in b.iter().enumerate() {
        let until = pairs[j..]
            .iter()
            .flatten()
            .next()
            .copied()
            .unwrap_or(a.len());
        removed(&a[next..until.max(next)], out);
        next = next.max(until);
        match pairs[j] {
            Some(i) => {
                out.push(matched(&a[i], node, Some(Change::Changed)));
                next = i + 1;
            }
            None => out.extend(all_added(std::slice::from_ref(node))),
        }
    }
    removed(&a[next.min(a.len())..], out);
}

fn removed(nodes: &[Node], out: &mut Vec<DiffNode>) {
    out.extend(nodes.iter().map(|node| DiffNode {
        node: Node {
            children: Vec::new(),
            ..node.clone()
        },
        change: Some(Change::Removed),
        was: None,
        children: Vec::new(),
    }));
}

/// `b` as the counterpart of `a`: changed if its line differs (or
/// `change` says so), with their children aligned.
fn matched(a: &Node, b: &Node, change: Option<Change>) -> DiffNode {
    let differs = line(a) != line(b);
    DiffNode {
        node: Node {
            children: Vec::new(),
            ..b.clone()
        },
        change: if differs {
            Some(Change::Changed)
        } else {
            change
        },
        was: differs.then(|| was(a, b)),
        children: align(&a.children, &b.children),
    }
}

/// A node of a `snapshot --json` tree as the snapshot pipeline holds it
/// (without its DOM key).
pub(crate) fn from_json(node: &SnapshotNode) -> Node {
    let mut out = Node {
        role: node.role.clone(),
        name: node.name.clone(),
        value: node.value.clone(),
        level: node.level,
        href: node.href.clone(),
        password: node.password,
        file: node.file,
        reference: node
            .reference
            .as_deref()
            .and_then(|r| r.strip_prefix('e')?.parse().ok()),
        more: node.more,
        below: node.below,
        children: node.children.iter().map(from_json).collect(),
        ..Node::default()
    };
    if node.folded.as_ref().is_some_and(|f| f.like.is_some()) {
        // A run stub: `neutral` drops it.
        out.fold = Some(Box::new(super::tree::Fold {
            like: node.folded.as_ref().and_then(|f| f.like),
            ..super::tree::Fold::default()
        }));
    } else if node.folded.is_some() {
        out.fold = Some(Box::default());
    }
    let states = &mut out.states;
    for word in &node.states {
        match word.as_str() {
            "checked" => states.checked = Some(Checked::True),
            "unchecked" => states.checked = Some(Checked::False),
            "mixed" => states.checked = Some(Checked::Mixed),
            "expanded" => states.expanded = Some(true),
            "collapsed" => states.expanded = Some(false),
            "selected" => states.selected = true,
            "disabled" => states.disabled = true,
            "readonly" => states.readonly = true,
            "required" => states.required = true,
            "invalid" => states.invalid = true,
            "focused" => states.focused = true,
            "pressed" => states.pressed = true,
            "scrollable" => out.scrollable = true,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::render::{diff_text, json};
    use super::super::tree::Node;
    use super::{count, diff, from_json, neutral, rewrite_hrefs};

    fn node(role: &str, name: &str, children: Vec<Node>) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            children,
            ..Node::default()
        }
    }

    fn link(name: &str, href: &str) -> Node {
        Node {
            href: Some(href.into()),
            ..node("link", name, vec![])
        }
    }

    fn text(diff: &[super::DiffNode]) -> String {
        let mut out = String::new();
        diff_text(diff, &mut out);
        out
    }

    #[test]
    fn a_changed_name_an_addition_a_removal_and_a_move() {
        let block = || node("region", "Offers", vec![node("text", "10% off", vec![])]);
        let mut heading = node("heading", "Your cart (2)", vec![]);
        heading.level = Some(1);
        let a = vec![node(
            "main",
            "",
            vec![
                heading.clone(),
                block(),
                node("list", "", vec![node("listitem", "Tea", vec![])]),
                link("Continue shopping", "/"),
                node("button", "Check out", vec![]),
            ],
        )];
        let b = vec![node(
            "main",
            "",
            vec![
                Node {
                    name: "Your cart (3)".into(),
                    ..heading
                },
                node("list", "", vec![node("listitem", "Tea", vec![])]),
                node("button", "Apply coupon", vec![]),
                node("button", "Check out", vec![]),
                block(),
            ],
        )];
        let changes = diff(&a, &b);
        assert_eq!(
            text(&changes),
            "
  - main
~   - heading \"Your cart (3)\" (h1) (was \"Your cart (2)\")
-   - region \"Offers\"
-   - link \"Continue shopping\" → /
+   - button \"Apply coupon\"
+   - region \"Offers\"
+     - text \"10% off\""
        );
        assert_eq!(count(&changes), 5);
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn a_change_below_a_changed_node_counts_too() {
        let a = vec![node("form", "Log in", vec![node("button", "Go", vec![])])];
        let b = vec![node(
            "form",
            "Sign in",
            vec![node("button", "Send", vec![])],
        )];
        let changes = diff(&a, &b);
        assert_eq!(
            text(&changes),
            "
~ - form \"Sign in\" (was \"Log in\")
~   - button \"Send\" (was \"Go\")"
        );
        assert_eq!(count(&changes), 2);
    }

    #[test]
    fn hrefs_move_to_the_other_origin() {
        let mut nodes = vec![
            link("Home", "https://shop.example.com"),
            link("Cart", "https://shop.example.com/cart?x=1"),
            link("Elsewhere", "https://shop.example.com.evil/"),
        ];
        rewrite_hrefs(
            &mut nodes,
            "https://shop.example.com",
            "http://localhost:5173",
        );
        let hrefs: Vec<_> = nodes.iter().map(|n| n.href.clone().unwrap()).collect();
        assert_eq!(
            hrefs,
            [
                "http://localhost:5173/",
                "http://localhost:5173/cart?x=1",
                "https://shop.example.com.evil/"
            ]
        );
    }

    #[test]
    fn json_round_trips_and_refs_are_not_compared() {
        let mut checkbox = node("checkbox", "Remember me", vec![]);
        checkbox.reference = Some(7);
        checkbox.states.checked = Some(super::Checked::True);
        checkbox.states.required = true;
        checkbox.scrollable = true;
        let tree = node("form", "Log in", vec![checkbox]);
        let back = from_json(&json(&tree));
        assert_eq!(back, tree);
        let mut other = vec![back];
        other[0].children[0].reference = Some(40);
        let mut mine = vec![tree];
        assert!(!neutral(&mut mine));
        neutral(&mut other);
        assert!(diff(&mine, &other).is_empty());
    }
}
