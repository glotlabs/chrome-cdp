//! Big pages in `snapshot` (plan 16, milestone 16.3): `--fold` folds runs
//! of siblings with the same shape, `--budget` folds whole regions until
//! the text fits a number of tokens.
//!
//! Folding comes after the views (`-i`, `--depth`) and before long lists
//! collapse; refs are given after it, to folded nodes too, so they keep
//! working. A fold keeps what it hid ([`Fold::hidden`]) and gets a region
//! ([`super::regions`]) that `snapshot --expand` shows.

use std::collections::{HashMap, HashSet};

use super::prune::{LIST_LIMIT, assign_refs, collapse};
use super::refs::Refs;
use super::regions::{Region, Regions};
use super::render;
use super::tree::{Fold, Node, NodeKey, is_landmark};
use crate::request::SnapshotEngine;

/// How many siblings in a row with one shape fold.
const RUN: usize = 4;

/// How deep a node's shape looks: it and three levels below.
const SHAPE_DEPTH: usize = 4;

/// How many nodes below it a node needs to be a region.
const REGION: usize = 20;

/// How a snapshot folds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Folding {
    /// `--fold`: fold runs of siblings with one shape.
    pub(crate) repeats: bool,
    /// `--budget`: fold regions until the text is at most this many
    /// estimated tokens (runs first, when over).
    pub(crate) budget: Option<usize>,
}

impl Folding {
    /// Whether it folds anything.
    pub(crate) fn any(&self) -> bool {
        self.repeats || self.budget.is_some()
    }
}

/// What else the text holds, and what must not fold.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Frame<'a> {
    /// Characters of the text besides the nodes' lines (the header).
    pub(crate) extra_chars: usize,
    /// The top-level nodes are the `--scope` root or what `--expand`
    /// shows: they never fold, nor do a run's siblings at the top.
    pub(crate) roots: bool,
    /// With `--expand`, each group's region.
    pub(crate) regions: &'a [u32],
}

/// Estimated tokens: characters ÷ 3, a fixed rule so the output is
/// deterministic. Snapshot text has about 3 characters a token (refs,
/// quotes and marks split finely), so ÷ 4 undercounted by a third.
pub(crate) fn tokens(chars: usize) -> usize {
    chars.div_ceil(3)
}

/// What a tab's snapshot memory gives folding.
pub(crate) struct Registries<'a> {
    pub(crate) refs: &'a mut Refs,
    pub(crate) regions: &'a mut Regions,
    pub(crate) all_refs: bool,
    pub(crate) engine: SnapshotEngine,
}

/// Collapses long lists, folds as `folding` says and gives refs, to the
/// nodes of each group (a group per `--expand` region, else one).
/// `collapse` is off with `--all`.
pub(crate) fn arrange(
    groups: &mut [Vec<Node>],
    folding: &Folding,
    frame: Frame<'_>,
    collapse_lists: bool,
    reg: &mut Registries<'_>,
) {
    let finish = |groups: &mut [Vec<Node>], reg: &mut Registries<'_>| {
        for group in groups.iter_mut() {
            if collapse_lists {
                collapse(group);
            }
            assign_refs(group, reg.refs, reg.all_refs, reg.engine);
        }
    };
    if !folding.any() {
        finish(groups, reg);
        return;
    }
    if let Some(budget) = folding.budget
        && !folding.repeats
    {
        // Under budget, nothing folds.
        let mut plain = groups.to_vec();
        finish(&mut plain, reg);
        if estimate(&plain, frame) <= budget {
            for (group, plain) in groups.iter_mut().zip(plain) {
                *group = plain;
            }
            return;
        }
    }
    for group in groups.iter_mut() {
        repeats(group, !frame.roots);
    }
    finish(groups, reg);
    for group in groups.iter_mut() {
        number(group, 0, reg.regions);
    }
    if let Some(budget) = folding.budget {
        fit(groups, budget, frame, reg.regions);
        if !folding.repeats {
            // Runs folded to fit; show what still fits of them.
            fill(groups, budget, frame, collapse_lists, reg.regions);
            for group in groups.iter_mut() {
                number(group, 0, reg.regions);
            }
            fit(groups, budget, frame, reg.regions);
        }
    }
}

/// A run stub shown: how deep, where, and its region.
#[derive(Debug)]
struct RunStub {
    depth: usize,
    group: usize,
    path: Vec<usize>,
    region: u32,
}

/// The run stubs shown in `nodes`, in page order.
fn run_stubs(nodes: &[Node], group: usize, path: &mut Vec<usize>, out: &mut Vec<RunStub>) {
    for (i, node) in nodes.iter().enumerate() {
        path.push(i);
        if let Some(fold) = &node.fold
            && fold.like.is_some()
        {
            out.push(RunStub {
                depth: path.len(),
                group,
                path: path.clone(),
                region: fold.region,
            });
        }
        run_stubs(&node.children, group, path, out);
        path.pop();
    }
}

/// The siblings of the node at `path` in `groups[group]`.
fn siblings<'a>(groups: &'a mut [Vec<Node>], group: usize, path: &[usize]) -> &'a mut Vec<Node> {
    match path.split_last() {
        Some((_, [])) | None => &mut groups[group],
        Some((_, parent)) => &mut at(&mut groups[group], parent).children,
    }
}

/// `--budget` without `--fold`: shows the siblings of each run again, one
/// at a time, while the text fits `budget` and (unless `--all`) the list
/// stays within [`LIST_LIMIT`]: outer runs first, then in page order.
fn fill(
    groups: &mut [Vec<Node>],
    budget: usize,
    frame: Frame<'_>,
    limit: bool,
    regions: &mut Regions,
) {
    let mut full: HashSet<u32> = HashSet::new();
    loop {
        let mut stubs = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            run_stubs(group, g, &mut Vec::new(), &mut stubs);
        }
        let Some(stub) = stubs
            .into_iter()
            .filter(|s| !full.contains(&s.region))
            .min_by_key(|s| s.depth)
        else {
            return;
        };
        let at = stub.path.last().copied().unwrap_or_default();
        let depth = stub.depth - 1;
        let list = siblings(groups, stub.group, &stub.path);
        let Some(fold) = list[at].fold.as_deref_mut() else {
            return;
        };
        let member = fold.hidden.remove(0);
        fold.like = Some(u32::try_from(fold.hidden.len()).unwrap_or(u32::MAX));
        let emptied = fold.hidden.is_empty();
        let role = member.role.clone();
        list.insert(at, member);
        let removed = emptied.then(|| list.remove(at + 1));
        let shown = list
            .iter()
            .filter(|n| n.role == role && n.fold.is_none())
            .count();
        let fits = (!limit || shown <= LIST_LIMIT) && estimate(groups, frame) <= budget;
        let list = siblings(groups, stub.group, &stub.path);
        if fits {
            number(std::slice::from_mut(&mut list[at]), depth, regions);
            continue;
        }
        let member = list.remove(at);
        if let Some(removed) = removed {
            list.insert(at, removed);
        }
        if let Some(fold) = list[at].fold.as_deref_mut() {
            fold.hidden.insert(0, member);
            fold.like = Some(u32::try_from(fold.hidden.len()).unwrap_or(u32::MAX));
        }
        full.insert(stub.region);
    }
}

/// The estimated tokens of `groups` as text.
fn estimate(groups: &[Vec<Node>], frame: Frame<'_>) -> usize {
    let mut text = String::new();
    for (i, group) in groups.iter().enumerate() {
        match frame.regions.get(i) {
            Some(&region) => render::region_text(group, region, &HashMap::new(), &mut text),
            None => render::text(group, &mut text),
        }
    }
    tokens(frame.extra_chars + text.chars().count())
}

/// A node's shape: the roles of its subtree, in order, to
/// [`SHAPE_DEPTH`] levels, without names, values or states.
fn shape(node: &Node, depth: usize, out: &mut String) {
    out.push_str(&node.role);
    if node.more.is_some() {
        out.push('…');
    }
    if depth > 1 && !node.children.is_empty() {
        out.push('(');
        for (i, child) in node.children.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            shape(child, depth - 1, out);
        }
        out.push(')');
    }
}

/// Whether `node` has focus or holds the node that has.
fn has_focus(node: &Node) -> bool {
    node.states.focused || node.children.iter().any(has_focus)
}

/// Whether `node` may be part of a run: something with content, a key
/// (for its region) and no focus.
fn runs(node: &Node) -> bool {
    !node.is_text()
        && node.more.is_none()
        && node.fold.is_none()
        && node.key.is_some()
        && !node.children.is_empty()
        && !has_focus(node)
}

/// Folds runs of [`RUN`] or more siblings with one shape, below `nodes`
/// and (unless `top` is false) among them: the first stays, the rest go
/// into a stub.
fn repeats(nodes: &mut Vec<Node>, top: bool) {
    let shapes: Vec<Option<String>> = nodes
        .iter()
        .map(|node| {
            runs(node).then(|| {
                let mut out = String::new();
                shape(node, SHAPE_DEPTH, &mut out);
                out
            })
        })
        .collect();
    for node in nodes.iter_mut() {
        repeats(&mut node.children, true);
    }
    if !top {
        return;
    }
    let old = std::mem::take(nodes);
    let mut i = 0;
    let mut old = old.into_iter().map(Some).collect::<Vec<_>>();
    while i < old.len() {
        let mut end = i + 1;
        if shapes[i].is_some() {
            while end < old.len() && shapes[end] == shapes[i] {
                end += 1;
            }
        }
        let first = old[i].take().unwrap_or_default();
        if end - i < RUN {
            nodes.push(first);
            for rest in &mut old[i + 1..end] {
                nodes.push(rest.take().unwrap_or_default());
            }
        } else {
            let role = first.role.clone();
            let hidden: Vec<Node> = old[i + 1..end]
                .iter_mut()
                .filter_map(Option::take)
                .collect();
            nodes.push(first);
            nodes.push(Node {
                role,
                fold: Some(Box::new(Fold {
                    like: Some(u32::try_from(hidden.len()).unwrap_or(u32::MAX)),
                    hidden,
                    ..Fold::default()
                })),
                ..Node::default()
            });
        }
        i = end;
    }
}

/// Counts what a fold at `depth` hides (its nodes, refs and tokens) and
/// gives it its region.
fn account(node: &mut Node, depth: usize, regions: &mut Regions) {
    let key = node.key.clone();
    let Some(fold) = node.fold.as_deref_mut() else {
        return;
    };
    let (region, depth) = if fold.like.is_some() {
        let keys = fold.hidden.iter().filter_map(|n| n.key.clone()).collect();
        (Region::Run(keys), depth)
    } else {
        let Some(key) = key else { return };
        (Region::Node(key), depth + 1)
    };
    let mut nodes = 0;
    let mut refs = 0;
    count(&fold.hidden, &mut nodes, &mut refs);
    let mut text = String::new();
    render::open_text(&fold.hidden, depth, &mut text);
    fold.nodes = nodes;
    fold.refs = refs;
    fold.tokens = u32::try_from(tokens(text.chars().count())).unwrap_or(u32::MAX);
    fold.region = regions.assign(region).unwrap_or_default();
}

/// Counts the nodes of `nodes`, folded ones too, and their refs.
fn count(nodes: &[Node], total: &mut u32, refs: &mut u32) {
    for node in nodes {
        if let Some(fold) = &node.fold {
            count(&fold.hidden, total, refs);
            if fold.like.is_some() {
                continue;
            }
        }
        *total += 1;
        if node.reference.is_some() {
            *refs += 1;
        }
        count(&node.children, total, refs);
    }
}

/// Accounts for every fold shown in `nodes` at `depth`.
fn number(nodes: &mut [Node], depth: usize, regions: &mut Regions) {
    for node in nodes {
        account(node, depth, regions);
        number(&mut node.children, depth + 1, regions);
    }
}

/// Roles that fold before any other: the page's parts and its long
/// collections.
fn folds_first(role: &str) -> bool {
    is_landmark(role)
        || matches!(
            role,
            "list"
                | "table"
                | "grid"
                | "treegrid"
                | "tree"
                | "listbox"
                | "menu"
                | "menubar"
                | "dialog"
                | "alertdialog"
                | "form"
                | "group"
                | "radiogroup"
        )
}

/// A node `--budget` may fold: where it is, and how it ranks.
#[derive(Debug)]
struct Candidate {
    group: usize,
    path: Vec<usize>,
    later: bool,
    size: usize,
}

/// How far `--budget` goes to fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pass {
    /// Regions: nodes with [`REGION`] nodes below them.
    Regions,
    /// Any node with something below it.
    Smaller,
    /// The top-level landmarks too.
    Landmarks,
}

/// Folds the best region, one at a time, until `groups` fit `budget`;
/// then, if they still do not, smaller nodes, and last the top-level
/// landmarks.
fn fit(groups: &mut [Vec<Node>], budget: usize, frame: Frame<'_>, regions: &mut Regions) {
    let h1 = first_h1(groups);
    for pass in [Pass::Regions, Pass::Smaller, Pass::Landmarks] {
        while estimate(groups, frame) > budget {
            let mut found = Vec::new();
            for (g, group) in groups.iter().enumerate() {
                let mut path = Vec::new();
                candidates(group, g, &mut path, pass, frame, h1.as_ref(), &mut found);
            }
            // The best: roles that fold first, then the largest.
            // Of the same size, the last: the top of the page stays.
            let Some(best) = found
                .into_iter()
                .rev()
                .min_by_key(|c| (c.later, std::cmp::Reverse(c.size)))
            else {
                break;
            };
            let depth = best.path.len() - 1;
            let node = at(&mut groups[best.group], &best.path);
            node.fold = Some(Box::new(Fold {
                hidden: std::mem::take(&mut node.children),
                ..Fold::default()
            }));
            account(node, depth, regions);
        }
    }
}

/// The node at `path` (child indices from the top).
fn at<'a>(nodes: &'a mut [Node], path: &[usize]) -> &'a mut Node {
    let (first, rest) = path.split_first().unwrap_or((&0, &[]));
    let node = &mut nodes[*first];
    if rest.is_empty() {
        node
    } else {
        at(&mut node.children, rest)
    }
}

/// The key of the first `h1` shown.
fn first_h1(groups: &[Vec<Node>]) -> Option<NodeKey> {
    let mut found = None;
    for group in groups {
        for node in group {
            node.walk(&mut |n| {
                if found.is_none() && n.role == "heading" && n.level == Some(1) {
                    found.clone_from(&n.key);
                }
            });
        }
    }
    found
}

/// Collects the nodes of `nodes` that may fold, and returns how many
/// nodes are shown in it and whether one of them has focus.
fn candidates(
    nodes: &[Node],
    group: usize,
    path: &mut Vec<usize>,
    pass: Pass,
    frame: Frame<'_>,
    h1: Option<&NodeKey>,
    out: &mut Vec<Candidate>,
) -> (usize, bool) {
    let mut total = 0;
    let mut focus = false;
    for (i, node) in nodes.iter().enumerate() {
        path.push(i);
        let (size, inner) = candidates(&node.children, group, path, pass, frame, h1, out);
        let focused = inner || node.states.focused;
        let top = path.len() == 1;
        let may = node.key.is_some()
            && node.fold.is_none()
            && node.more.is_none()
            && size > 0
            && !focused
            && !(top && frame.roots)
            && node.key.as_ref() != h1
            && (pass == Pass::Landmarks || !(top && is_landmark(&node.role)))
            && (pass > Pass::Regions || size >= REGION);
        if may {
            out.push(Candidate {
                group,
                path: path.clone(),
                later: !folds_first(&node.role),
                size,
            });
        }
        path.pop();
        total += 1 + size;
        focus |= focused;
    }
    (total, focus)
}

#[cfg(test)]
mod tests {
    use super::super::refs::Refs;
    use super::super::regions::{Lookup, Region, Regions};
    use super::super::render;
    use super::super::tree::{Node, NodeKey};
    use super::{Folding, Frame, Registries, arrange, estimate};
    use crate::request::SnapshotEngine;

    fn key(n: i64) -> NodeKey {
        NodeKey {
            frame: "F".into(),
            backend: n,
        }
    }

    fn el(role: &str, name: &str, n: i64, children: Vec<Node>) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            key: Some(key(n)),
            reffable: matches!(role, "button" | "link"),
            children,
            ..Node::default()
        }
    }

    fn card(i: i64) -> Node {
        el(
            "listitem",
            "",
            1000 + i * 10,
            vec![
                el("link", &format!("Mug {i}"), 1001 + i * 10, vec![]),
                Node::text(format!("{i} €"), None),
                el("button", "Add to cart", 1002 + i * 10, vec![]),
            ],
        )
    }

    fn cards(n: i64) -> Vec<Node> {
        vec![el("list", "Products", 1, (1..=n).map(card).collect())]
    }

    fn run(nodes: Vec<Node>, folding: &Folding) -> (String, Vec<Node>, Refs, Regions) {
        let mut groups = vec![nodes];
        let mut refs = Refs::default();
        let mut regions = Regions::default();
        arrange(
            &mut groups,
            folding,
            Frame::default(),
            true,
            &mut Registries {
                refs: &mut refs,
                regions: &mut regions,
                all_refs: false,
                engine: SnapshotEngine::Ax,
            },
        );
        let nodes = groups.pop().unwrap();
        let mut text = String::new();
        render::text(&nodes, &mut text);
        (text, nodes, refs, regions)
    }

    #[test]
    fn runs_fold_after_the_first_and_keep_their_refs() {
        let folding = Folding {
            repeats: true,
            budget: None,
        };
        let (text, _, refs, regions) = run(cards(60), &folding);
        assert_eq!(
            text,
            "
- list \"Products\"
  - listitem
    - link \"Mug 1\" [e1]
    - text \"1 €\"
    - button \"Add to cart\" [e2]
  - … 59 more listitem like the one above [region r1]"
        );
        // Every card's link and button has a ref, in page order.
        assert_eq!(refs.get(&key(1601)), Some(119));
        let Lookup::Live(Region::Run(keys)) = regions.lookup(1) else {
            panic!("r1 is a run")
        };
        assert_eq!(keys.len(), 59);
        assert_eq!(keys[0], key(1020));

        // Three alike stay; so does a different one in between.
        let (text, _, _, _) = run(cards(3), &folding);
        assert!(!text.contains("like the one above"), "{text}");
        let mut list = cards(8);
        list[0].children[4] = el("listitem", "Sold out", 9, vec![]);
        let (text, _, _, _) = run(list, &folding);
        assert!(
            text.contains("  - … 3 more listitem like the one above [region r1]\n  - listitem \"Sold out\"\n  - listitem\n"),
            "{text}"
        );
    }

    #[test]
    fn a_node_with_focus_is_never_folded() {
        let mut list = cards(10);
        list[0].children[3].children[2].states.focused = true;
        let (text, _, _, _) = run(
            list,
            &Folding {
                repeats: true,
                budget: None,
            },
        );
        assert!(
            text.contains("button \"Add to cart\" [e8] focused"),
            "{text}"
        );
        assert!(
            text.contains("… 5 more listitem like the one above"),
            "{text}"
        );
        assert!(!text.contains("Mug 6"), "{text}");
    }

    #[test]
    fn a_budget_folds_regions_until_it_fits() {
        let page = vec![
            el("heading", "Shop", 2, vec![]),
            el(
                "main",
                "",
                3,
                vec![
                    el("region", "Offers", 4, cards(12)),
                    el(
                        "navigation",
                        "Pages",
                        5,
                        vec![el("button", "Next", 6, vec![])],
                    ),
                ],
            ),
        ];
        let under = Folding {
            repeats: false,
            budget: Some(10_000),
        };
        let (text, _, _, _) = run(page.clone(), &under);
        assert!(!text.contains("region r"), "under budget: {text}");

        let folding = Folding {
            repeats: false,
            budget: Some(60),
        };
        let (text, nodes, _, regions) = run(page, &folding);
        assert!(estimate(&[nodes], Frame::default()) <= 60, "{text}");
        assert_eq!(
            text,
            "
- heading \"Shop\"
- main
  - region \"Offers\" [region r2: 49 nodes, 24 refs, ~423 tokens]
  - navigation \"Pages\"
    - button \"Next\" [e25]"
        );
        assert_eq!(regions.lookup(2), Lookup::Live(Region::Node(key(4))));
    }
}
