//! `find <query>` (plan 04, milestone 4.5): searches the tree a snapshot
//! would show for nodes whose role, name, value or text match, and shows
//! the best as snapshot lines with refs (from the tab's registry, like
//! the snapshot's), each with the named container it is in.
//!
//! Every word of the query must match, in any order: a part of the name
//! or value (case-insensitive), the role, or, for words of five letters or
//! more, a word of the name one typo away. Matches rank by how well the
//! whole query fits the name (exact, a prefix, a part, the words at word
//! starts), then things to act on first and text last, then page order.

use std::fmt::Write as _;

use protocol::schema::{FindData, FindMatch};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use super::prune::{self, View};
use super::render;
use super::tree::{IFRAME, Node};
use super::{AxReader, READ_ATTEMPTS, Reader};
use crate::Engine;
use crate::request::{FindRequest, millis};
use crate::tabs::tab_name;

/// Roles whose name places a match: the nearest named one is its context.
const CONTEXT_ROLES: &[&str] = &[
    "row",
    "listitem",
    "article",
    "dialog",
    "alertdialog",
    "form",
    "region",
    "navigation",
    "banner",
    "main",
    "contentinfo",
    "complementary",
    "search",
    "tabpanel",
    "menu",
    "menubar",
    "listbox",
    "tree",
    "grid",
    "table",
    "list",
    "group",
    "toolbar",
    "tablist",
    "radiogroup",
    IFRAME,
];

/// How much of a context's name is shown.
const CONTEXT_CHARS: usize = 40;

/// A search: the query's words, lower case.
#[derive(Debug, Clone)]
pub(crate) struct Query {
    phrase: String,
    words: Vec<String>,
}

impl Query {
    pub(crate) fn new(text: &str) -> Self {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        Self {
            phrase: words.join(" "),
            words,
        }
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// How well `node` matches, or `None` if a word does not.
    fn score(&self, node: &Node) -> Option<u32> {
        let name = normalize(&node.name);
        let value = node.value.as_deref().map(normalize).unwrap_or_default();
        let mut score = 0;
        for word in &self.words {
            score += if let Some(at) = name.find(word.as_str()) {
                if at == 0 || !name[..at].ends_with(char::is_alphanumeric) {
                    4
                } else {
                    3
                }
            } else if node.role == *word
                || (!node.is_text() && node.role.starts_with(word.as_str()))
                || value.contains(word.as_str())
            {
                2
            } else if word.chars().count() >= 5
                && name
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| near(w, word))
            {
                1
            } else {
                return None;
            };
        }
        if name == self.phrase {
            score += 20;
        } else if name.starts_with(&self.phrase) {
            score += 10;
        } else if name.contains(&self.phrase) {
            score += 5;
        }
        // Things to act on first, then other elements, then text.
        if node.reffable {
            score += 3;
        } else if !node.is_text() {
            score += 1;
        }
        Some(score)
    }
}

/// Lower case with whitespace collapsed.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `a` and `b` are at most one edit (insert, delete, replace or
/// swap of neighbours) apart: their optimal string alignment distance.
fn near(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    // d[i][j]: the distance between a[..i] and b[..j].
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()] <= 1
}

/// One match: where it is in the tree, and how good it is.
#[derive(Debug, Clone)]
pub(crate) struct Hit {
    /// The node, without its children.
    pub(crate) node: Node,
    pub(crate) context: Option<String>,
    pub(crate) score: u32,
    order: usize,
}

/// Every node of `nodes` that matches, best first, then in page order.
pub(crate) fn search(nodes: &[Node], query: &Query, role: Option<&str>) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut order = 0;
    visit(nodes, query, role, &mut Vec::new(), &mut order, &mut hits);
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.order.cmp(&b.order)));
    hits
}

fn visit(
    nodes: &[Node],
    query: &Query,
    role: Option<&str>,
    context: &mut Vec<String>,
    order: &mut usize,
    hits: &mut Vec<Hit>,
) {
    for node in nodes {
        *order += 1;
        if node.more.is_none()
            && role.is_none_or(|r| node.role == r)
            && let Some(score) = query.score(node)
        {
            hits.push(Hit {
                node: Node {
                    children: Vec::new(),
                    ..node.clone()
                },
                context: context.last().cloned(),
                score,
                order: *order,
            });
        }
        let named = CONTEXT_ROLES.contains(&node.role.as_str()) && !node.name.is_empty();
        if named {
            let name = prune::truncate(&node.name, CONTEXT_CHARS);
            context.push(format!("{} \"{}\"", node.role, name.replace('"', "\\\"")));
        }
        visit(&node.children, query, role, context, order, hits);
        if named {
            context.pop();
        }
    }
}

impl Engine {
    /// `find`: the nodes of the active tab (or `--tab`) that match a
    /// query, as snapshot lines with refs.
    pub async fn find(&self, req: FindRequest) -> Result<Output, ErrorInfo> {
        let query = Query::new(&req.query);
        if query.is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "find needs words to look for")
                    .hint(Hint::run(Cmd::new("find").arg("checkout"))),
            );
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let view = View {
            collapse: false,
            ..View::default()
        };
        // As in `snapshot`: a frame that loads a new document while the
        // page is read would leave refs on nodes that are gone.
        let mut attempts = 0;
        let (capture, hits) = loop {
            attempts += 1;
            let generation = running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation())
                .unwrap_or_default();
            let frames = running.tabs.frame_sources(page.id);
            let reader = AxReader {
                session: &self.session,
            };
            let read = reader.read(&frames, None, timeout);
            let capture = self.unless_blocked(&running, &page, mark, read).await?;
            let shown = prune::shape(capture.nodes.clone(), &view);
            let mut hits = search(&shown, &query, req.role.as_deref());
            let assigned = running.tabs.snapshot_memory(page.id, |memory| {
                if memory.refs.generation() != generation && attempts < READ_ATTEMPTS {
                    return false;
                }
                let shown = if req.limit == 0 {
                    hits.len()
                } else {
                    req.limit.min(hits.len())
                };
                for hit in &mut hits[..shown] {
                    if !hit.node.is_text()
                        && let Some(key) = &hit.node.key
                    {
                        hit.node.reference =
                            Some(memory.refs.assign(key, &hit.node.role, &hit.node.name));
                    }
                }
                true
            });
            if assigned != Some(false) {
                break (capture, hits);
            }
        };

        let tab = tab_name(page.id);
        if hits.is_empty() {
            let mut info = ErrorInfo::new(
                ErrorKind::NotFound,
                format!("nothing in tab {tab} matches {:?}", req.query),
            )
            .hint(
                Hint::run(
                    Cmd::new("snapshot")
                        .switch("interactive")
                        .session(&self.session),
                )
                .why("shows what can be acted on"),
            )
            .details(serde_json::json!({ "query": req.query, "role": req.role, "tab": tab }));
            if let Some(role) = &req.role {
                info = info.note("role", format!("only {role} nodes were searched"));
            }
            return Err(info);
        }
        let count = hits.len();
        let shown = if req.limit == 0 {
            count
        } else {
            req.limit.min(count)
        };
        let mut text = format!(
            "page {tab} \"{}\" {}",
            capture.title.replace('"', "\\\""),
            capture.url
        );
        let mut matches = Vec::with_capacity(shown);
        for hit in &hits[..shown] {
            text.push_str("\n- ");
            text.push_str(&render::line(&hit.node));
            if let Some(context) = &hit.context {
                let _ = write!(text, " (in {context})");
            }
            matches.push(FindMatch {
                node: render::json(&hit.node),
                context: hit.context.clone(),
            });
        }
        if shown < count {
            let _ = write!(
                text,
                "\n… {} more matches (use --limit {count} or more words)",
                format::thousands(count - shown),
            );
        }
        let data = FindData {
            tab,
            url: capture.url,
            title: capture.title,
            query: req.query.clone(),
            role: req.role.clone(),
            count: count as u64,
            matches,
        };
        let refs: Vec<String> = data
            .matches
            .iter()
            .filter_map(|m| m.node.reference.clone())
            .collect();
        let output = Output::typed(&data, text)
            .quiet(refs.join("\n"))
            .limit(req.max_chars, "--limit or --max-chars 0 for all of it");
        Ok(self.report_events(&running, mark, output, None).await)
    }
}

#[cfg(test)]
mod tests {
    use super::{Query, near, search};
    use crate::snapshot::tree::Node;

    fn node(role: &str, name: &str, children: Vec<Node>) -> Node {
        Node {
            role: role.to_owned(),
            name: name.to_owned(),
            reffable: matches!(role, "button" | "link" | "textbox"),
            children,
            ..Node::default()
        }
    }

    fn page() -> Vec<Node> {
        vec![
            node(
                "navigation",
                "Main",
                vec![node("link", "Checkout help", vec![])],
            ),
            node(
                "form",
                "Cart",
                vec![
                    node("textbox", "Coupon", vec![]),
                    node("button", "Checkout", vec![]),
                    node("text", "Free shipping at checkout over $50", vec![]),
                ],
            ),
            node("heading", "Proceed to checkout", vec![]),
        ]
    }

    fn found(query: &str, role: Option<&str>) -> Vec<String> {
        search(&page(), &Query::new(query), role)
            .into_iter()
            .map(|h| match h.context {
                Some(c) => format!("{} {:?} in {c}", h.node.role, h.node.name),
                None => format!("{} {:?}", h.node.role, h.node.name),
            })
            .collect()
    }

    #[test]
    fn the_best_match_comes_first() {
        assert_eq!(
            found("checkout", None),
            [
                "button \"Checkout\" in form \"Cart\"",
                "link \"Checkout help\" in navigation \"Main\"",
                "heading \"Proceed to checkout\"",
                "text \"Free shipping at checkout over $50\" in form \"Cart\"",
            ]
        );
    }

    #[test]
    fn every_word_must_match_the_name_or_role() {
        assert_eq!(
            found("checkout button", None),
            ["button \"Checkout\" in form \"Cart\""]
        );
        assert_eq!(
            found("coupon textbox", None),
            ["textbox \"Coupon\" in form \"Cart\""]
        );
        assert!(found("checkout banana", None).is_empty());
        assert_eq!(
            found("checkout", Some("link")),
            ["link \"Checkout help\" in navigation \"Main\""]
        );
    }

    #[test]
    fn long_words_forgive_one_typo() {
        assert_eq!(found("chekout", None).len(), 4);
        assert_eq!(
            found("coupno", None),
            ["textbox \"Coupon\" in form \"Cart\""]
        );
        // Short words must be exact: too many near misses otherwise.
        assert!(found("cat", None).is_empty());
    }

    #[test]
    fn near_is_one_edit() {
        assert!(near("checkout", "checkout"));
        assert!(near("checkout", "chekout"));
        assert!(near("checkout", "checkouts"));
        assert!(near("checkout", "chexkout"));
        assert!(near("checkout", "chcekout"));
        assert!(!near("checkout", "chekuot"));
        assert!(!near("checkout", "check"));
    }
}
