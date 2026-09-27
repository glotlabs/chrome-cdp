//! `snapshot` (plan 04, milestones 4.1 and 4.3): the page as a compact,
//! deterministic accessibility tree, with a ref (`e12`) on everything an
//! agent can act on.
//!
//! Two engines read the page behind one trait, [`Reader`]: [`ax`] (Chrome's
//! accessibility tree, the default) and [`dom`] (a DOM walker injected into
//! chrome-cdp's isolated world). Both return the same [`tree::Node`]s, so
//! pruning, refs, `--interactive`/`--depth`/`--scope`, `--diff` and
//! rendering ([`prune`], [`render`]) are shared.
//!
//! Each tab keeps its refs ([`refs`]) and the tree of its last snapshot
//! (for `--diff`) in [`Memory`].

mod ax;
mod dom;
mod prune;
mod refs;
mod render;
mod tree;

use std::time::Duration;

use cdp::Session;
use cdp::domains::{dom as cdp_dom, runtime};
use protocol::schema::{SnapshotData, SnapshotNode};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;

use crate::errors::from_cdp;
use crate::frames::{FrameInfo, World};
use crate::locator::{Locator, parse_locator};
use crate::request::{SnapshotEngine, SnapshotRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

use prune::View;
use refs::Refs;
pub(crate) use refs::parse_ref;
use tree::{Node, NodeKey};

/// The engine `snapshot` uses unless `--engine` says otherwise; chosen by
/// the corpus comparison in plan 04's decisions log.
pub const DEFAULT_ENGINE: SnapshotEngine = SnapshotEngine::Ax;

/// A frame of the tab and the session that owns it.
#[derive(Debug, Clone)]
pub(crate) struct FrameSource {
    pub(crate) info: FrameInfo,
    pub(crate) session: Session,
}

/// What an engine read: the main frame's URL and title, and its nodes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Capture {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) nodes: Vec<Node>,
}

/// A snapshot engine: reads a tab's frames (main frame first) into nodes.
/// `scope` is the `--scope` node, which must come back with its key.
pub(crate) trait Reader {
    async fn read(
        &self,
        frames: &[FrameSource],
        scope: Option<&NodeKey>,
        timeout: Duration,
    ) -> Result<Capture, ErrorInfo>;
}

/// Engine A: Chrome's accessibility tree.
#[derive(Debug)]
pub(crate) struct AxReader<'a> {
    pub(crate) session: &'a str,
}

/// Engine B: the injected DOM walker.
#[derive(Debug)]
pub(crate) struct DomReader<'a> {
    pub(crate) running: &'a Running,
    pub(crate) page: &'a Page,
    pub(crate) engine: &'a Engine,
}

impl Reader for AxReader<'_> {
    async fn read(
        &self,
        frames: &[FrameSource],
        _scope: Option<&NodeKey>,
        timeout: Duration,
    ) -> Result<Capture, ErrorInfo> {
        ax::capture(frames, timeout)
            .await
            .map_err(|e| from_cdp(&e, self.session))
    }
}

impl Reader for DomReader<'_> {
    async fn read(
        &self,
        frames: &[FrameSource],
        scope: Option<&NodeKey>,
        timeout: Duration,
    ) -> Result<Capture, ErrorInfo> {
        dom::capture(self.engine, self.running, self.page, frames, scope, timeout).await
    }
}

/// What a tab remembers between snapshots.
#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub(crate) refs: Refs,
    /// The last snapshot's tree as the engine read it, for `--diff`.
    last: Option<Vec<Node>>,
}

impl SnapshotEngine {
    /// The `--engine` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ax => "ax",
            Self::Dom => "dom",
        }
    }
}

/// How `--scope` names the subtree.
enum Scope {
    Ref(u32),
    Locator(Locator),
}

/// Checks `--scope` without a browser.
fn parse_scope(text: &str) -> Result<Scope, ErrorInfo> {
    match parse_ref(text) {
        Some(number) => Ok(Scope::Ref(number)),
        None => parse_locator(text).map(Scope::Locator).map_err(|e| {
            e.hint(Hint::text(
                "use a ref from the snapshot (e12), css=<selector> or xpath=<expression>",
            ))
        }),
    }
}

/// Checks a snapshot request's arguments (the `--scope` locator) without a
/// browser, so the CLI can fail before it starts one.
pub fn check_snapshot(req: &SnapshotRequest) -> Result<(), ErrorInfo> {
    req.scope.as_deref().map(parse_scope).transpose().map(drop)
}

impl Engine {
    /// `snapshot`: the active tab (or `--tab`) as an accessibility tree.
    pub async fn snapshot(&self, req: SnapshotRequest) -> Result<Output, ErrorInfo> {
        let scope = req.scope.as_deref().map(parse_scope).transpose()?;
        let view = View {
            interactive: req.interactive,
            depth: req.depth,
            all_refs: req.all_refs,
            collapse: !req.all,
            text_limit: req.text_limit,
            no_text: req.no_text,
        };
        let engine = req.engine.unwrap_or(DEFAULT_ENGINE);
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let frames = running.tabs.frame_sources(page.id);
        let scope_key = match &scope {
            None => None,
            Some(scope) => Some(
                self.scope_key(&running, &page, &frames, scope, timeout)
                    .await?,
            ),
        };
        let capture = {
            let read = async {
                match engine {
                    SnapshotEngine::Ax => {
                        AxReader {
                            session: &self.session,
                        }
                        .read(&frames, scope_key.as_ref(), timeout)
                        .await
                    }
                    SnapshotEngine::Dom => {
                        DomReader {
                            running: &running,
                            page: &page,
                            engine: self,
                        }
                        .read(&frames, scope_key.as_ref(), timeout)
                        .await
                    }
                }
            };
            self.unless_blocked(&running, &page, mark, read).await?
        };
        let scoped = |nodes: &[Node]| -> Option<Vec<Node>> {
            match &scope_key {
                None => Some(nodes.to_vec()),
                Some(key) => prune::find(nodes, key).map(|n| vec![n]),
            }
        };
        let Some(current) = scoped(&capture.nodes) else {
            return Err(self.scope_gone(&req, &page));
        };
        let mut shown = prune::shape(current, &view);
        let before = running
            .tabs
            .snapshot_memory(page.id, |memory| {
                prune::assign_refs(&mut shown, &mut memory.refs, view.all_refs);
                let before = memory.last.replace(capture.nodes.clone());
                before.map(|old| {
                    let mut old = prune::shape(scoped(&old).unwrap_or_default(), &view);
                    prune::lookup_refs(&mut old, &memory.refs);
                    old
                })
            })
            .flatten();

        let tab = tab_name(page.id);
        let mut text = format!(
            "page {tab} \"{}\" {}",
            capture.title.replace('"', "\\\""),
            capture.url
        );
        let (nodes, diff): (Vec<SnapshotNode>, Option<bool>) = if req.diff {
            let compared = before.is_some();
            let changes = match &before {
                Some(old) => render::diff(old, &shown),
                None => render::all_added(&shown),
            };
            if !compared {
                text.push_str("\n(no earlier snapshot of this tab; all of it is new)");
            } else if changes.is_empty() {
                text.push_str("\n(no changes since the last snapshot)");
            }
            render::diff_text(&changes, &mut text);
            (
                changes.iter().map(render::diff_json).collect(),
                Some(compared),
            )
        } else {
            render::text(&shown, &mut text);
            (shown.iter().map(render::json).collect(), None)
        };
        let data = SnapshotData {
            tab,
            url: capture.url,
            title: capture.title,
            engine: engine.as_str().to_owned(),
            diff,
            nodes,
        };
        let more = "--interactive, --scope <ref> or --max-chars 0 for all of it";
        let text = fit(&text, req.max_chars, more);
        let output = Output::typed(&data, text).limit(req.max_chars, more);
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// The node `--scope` names.
    async fn scope_key(
        &self,
        running: &Running,
        page: &Page,
        frames: &[FrameSource],
        scope: &Scope,
        timeout: Duration,
    ) -> Result<NodeKey, ErrorInfo> {
        match scope {
            Scope::Ref(number) => {
                let entry = running
                    .tabs
                    .snapshot_memory(page.id, |m| m.refs.entry(*number).cloned())
                    .flatten();
                entry.map(|e| e.key).ok_or_else(|| {
                    ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("tab {} has no ref e{number}", tab_name(page.id)),
                    )
                    .hint(
                        Hint::run(Cmd::new("snapshot").session(&self.session))
                            .why("shows the refs the page has now"),
                    )
                })
            }
            Scope::Locator(locator) => {
                let main = frames.first().ok_or_else(|| {
                    ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("tab {} went away", tab_name(page.id)),
                    )
                })?;
                let object = self
                    .in_frame(running, page, None, World::Isolated, timeout, |context| {
                        let mut params = runtime::CallFunctionOnParams::new(FIND_JS.to_owned());
                        params.execution_context_id = Some(context.context_id);
                        params.arguments = Some(vec![runtime::CallArgument {
                            value: Some(json!(locator)),
                            unserializable_value: None,
                            object_id: None,
                        }]);
                        context.session.with_timeout(timeout).call(params)
                    })
                    .await?
                    .map_err(|e| from_cdp(&e, &self.session))?;
                if let Some(exception) = object.exception_details {
                    let message = exception
                        .exception
                        .and_then(|e| e.description)
                        .unwrap_or(exception.text);
                    let first = message.lines().next().unwrap_or_default().to_owned();
                    return Err(ErrorInfo::new(
                        ErrorKind::BadLocator,
                        format!("--scope {locator}: {first}"),
                    )
                    .hint(Hint::text("check the selector")));
                }
                let Some(object_id) = object.result.object_id else {
                    return Err(ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("no element matches --scope {locator}"),
                    )
                    .hint(
                        Hint::run(Cmd::new("snapshot").session(&self.session))
                            .why("shows the page; scope it by a ref from there"),
                    ));
                };
                let params = cdp_dom::DescribeNodeParams {
                    object_id: Some(object_id),
                    ..cdp_dom::DescribeNodeParams::default()
                };
                let described = main
                    .session
                    .with_timeout(timeout)
                    .call(params)
                    .await
                    .map_err(|e| from_cdp(&e, &self.session))?;
                Ok(NodeKey {
                    frame: main.info.id.clone(),
                    backend: described.node.backend_node_id,
                })
            }
        }
    }

    /// The error when the `--scope` node is not in the tree.
    fn scope_gone(&self, req: &SnapshotRequest, page: &Page) -> ErrorInfo {
        let scope = req.scope.clone().unwrap_or_default();
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!(
                "--scope {scope} is not shown in tab {}: it is hidden or no longer in the page",
                tab_name(page.id)
            ),
        )
        .hint(
            Hint::run(Cmd::new("snapshot").session(&self.session))
                .why("shows what the page has now"),
        )
    }
}

/// Finds the first element a `{kind, value}` locator matches, in the
/// isolated world.
const FIND_JS: &str = "function (loc) {
  if (loc.kind === 'css') return document.querySelector(loc.value);
  return document.evaluate(loc.value, document, null,
    XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
}";

/// Cuts `text` at a line boundary so that it and the note that says so fit
/// in `max_chars` (0 = no limit).
fn fit(text: &str, max_chars: usize, more: &str) -> String {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let note = |dropped: usize, lines: usize| {
        format!(
            "… snapshot truncated: {} more chars ({} lines); use {more}",
            format::thousands(dropped),
            format::thousands(lines)
        )
    };
    let budget =
        max_chars.saturating_sub(note(text.len(), text.lines().count()).chars().count() + 1);
    let cut = text
        .char_indices()
        .nth(budget)
        .map_or(text.len(), |(at, _)| at);
    let at = text[..cut].rfind('\n').unwrap_or(cut);
    let (kept, rest) = text.split_at(at);
    let rest = rest.trim_start_matches('\n');
    format!(
        "{kept}\n{}",
        note(rest.chars().count(), rest.lines().count())
    )
}

/// `url` as a link shows it: relative (path, query, fragment) when on the
/// origin of `base`, the frame's URL; cut at 200 characters.
pub(crate) fn relative_href(url: &str, base: &str) -> String {
    let shown = match (url::Url::parse(url), url::Url::parse(base)) {
        (Ok(u), Ok(b))
            if u.origin() == b.origin() && matches!(u.scheme(), "http" | "https" | "file") =>
        {
            let mut rel = u.path().to_owned();
            if let Some(query) = u.query() {
                rel.push('?');
                rel.push_str(query);
            }
            if let Some(fragment) = u.fragment() {
                rel.push('#');
                rel.push_str(fragment);
            }
            rel
        }
        _ => url.to_owned(),
    };
    prune::truncate(&shown, 200)
}

#[cfg(test)]
mod tests {
    use super::{fit, relative_href};

    #[test]
    fn hrefs_are_relative_on_the_same_origin() {
        let base = "https://acme.test/login?next=1";
        assert_eq!(relative_href("https://acme.test/", base), "/");
        assert_eq!(
            relative_href("https://acme.test/a/b?c=1#top", base),
            "/a/b?c=1#top"
        );
        assert_eq!(
            relative_href("https://other.test/docs", base),
            "https://other.test/docs"
        );
        assert_eq!(
            relative_href("http://acme.test/", base),
            "http://acme.test/"
        );
        assert_eq!(relative_href("mailto:a@b.c", base), "mailto:a@b.c");
    }

    #[test]
    fn fit_cuts_whole_lines_and_says_how_to_see_more() {
        let text = (0..100)
            .map(|i| format!("- line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        let cut = fit(&text, 300, "--max-chars 0");
        assert!(cut.chars().count() <= 300, "{}", cut.chars().count());
        let last = cut.lines().last().unwrap();
        assert!(last.starts_with("… snapshot truncated: "), "{last}");
        assert!(last.ends_with("; use --max-chars 0"), "{last}");
        let kept = cut.lines().count() - 1;
        assert!(cut.lines().take(kept).all(|l| l.starts_with("- line ")));
        assert_eq!(fit(&text, 0, "x"), text);
    }
}
