//! `snapshot` (plan 04, milestones 4.1 to 4.3): the page as a compact,
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
//! (for `--diff`) in [`Memory`]; [`resolve`] turns a ref back into its
//! element for every command that takes one. [`find`] searches the same
//! tree and gives refs from the same registry.

mod ax;
pub(crate) mod compare;
mod dom;
mod find;
pub use find::check_find;
mod fold;
mod prune;
mod refs;
mod regions;
mod render;
mod resolve;
mod tree;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use cdp::Session;
use protocol::schema::{SnapshotData, SnapshotNode};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::FrameInfo;
use crate::locator::{LocateOptions, Locator};
use crate::request::{SnapshotEngine, SnapshotRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

use fold::{Folding, Frame, Registries};
use prune::View;
use refs::Refs;
pub(crate) use refs::parse_ref;
use regions::{Region, Regions, parse_region};
pub use resolve::RefCall;
pub(crate) use resolve::Target;
use tree::Node;
pub(crate) use tree::NodeKey;

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

/// What [`Engine::read_tree`] read: the capture, the nodes shown (with
/// their refs), the tab's previous tree shaped the same way (for
/// `--diff`), and whether the main frame loaded a new page since.
#[derive(Debug)]
struct Tree {
    capture: Capture,
    shown: Vec<Node>,
    /// With `--expand`, how many of `shown`'s nodes each region has.
    groups: Vec<usize>,
    before: Option<Vec<Node>>,
    new_page: bool,
}

/// What part of the page a snapshot shows.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Within<'a> {
    Page,
    /// `--scope`: this node's subtree.
    Node(&'a NodeKey),
    /// `--expand`: these regions, one after the other.
    Regions(&'a [(u32, Region)]),
}

impl<'a> Within<'a> {
    /// `--scope`'s node, or the whole page.
    pub(crate) fn scope(key: Option<&'a NodeKey>) -> Self {
        key.map_or(Self::Page, Self::Node)
    }

    /// The nodes of `nodes` it shows, a group per region; `Err` with the
    /// scope's or a region's number if it is not in them.
    fn groups(self, nodes: &[Node]) -> Result<Vec<Vec<Node>>, Option<u32>> {
        match self {
            Self::Page => Ok(vec![nodes.to_vec()]),
            Self::Node(key) => prune::find(nodes, key).map(|n| vec![vec![n]]).ok_or(None),
            Self::Regions(regions) => regions
                .iter()
                .map(|(number, region)| {
                    let found: Vec<Node> = match region {
                        Region::Node(key) => prune::find(nodes, key).into_iter().collect(),
                        Region::Run(keys) => {
                            keys.iter().filter_map(|k| prune::find(nodes, k)).collect()
                        }
                    };
                    if found.is_empty() {
                        Err(Some(*number))
                    } else {
                        Ok(found)
                    }
                })
                .collect(),
        }
    }
}

/// What a tab remembers between snapshots.
#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub(crate) refs: Refs,
    /// What `--fold` and `--budget` folded, for `--expand`.
    regions: Regions,
    /// The last snapshot's tree as the engine read it, for `--diff`.
    last: Option<Vec<Node>>,
    /// The main frame loaded a new document since `last` was read.
    new_page: bool,
}

impl Memory {
    /// Frames `frames` loaded new documents; `main` if one is the main
    /// frame.
    pub(crate) fn navigated(&mut self, frames: &[String], main: bool) {
        self.refs.navigated(frames, main);
        self.regions.navigated(frames, main);
        self.new_page |= main;
    }
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

/// Checks a snapshot request's arguments (the `--scope` locator) without a
/// browser, so the CLI can fail before it starts one.
pub fn check_snapshot(req: &SnapshotRequest) -> Result<(), ErrorInfo> {
    if req.source && req.diff {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "--source cannot be used with --diff: a diff shows what changed, not where it came from",
        )
        .hint(Hint::text(
            "run snapshot --source for the whole page, or chrome-cdp source <ref> for one element",
        )));
    }
    if !req.expand.is_empty() {
        let with = if req.scope.is_some() {
            Some("--scope")
        } else if req.diff {
            Some("--diff")
        } else {
            None
        };
        if let Some(with) = with {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "--expand cannot be used with {with}: a region is its own part of the page"
                ),
            )
            .hint(Hint::text(
                "run snapshot --expand <region> alone, or take the snapshot again",
            )));
        }
        if let Some(bad) = req.expand.iter().find(|r| parse_region(r).is_none()) {
            return Err(bad_region(bad));
        }
    }
    scope_locator(req.scope.as_deref(), &locate_options(req)).map(drop)
}

/// The error for an `--expand` value that is not a region.
fn bad_region(text: &str) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("--expand {text}: not a region; regions look like r4"),
    )
    .hint(Hint::text(
        "snapshot --fold or --budget N prints regions ([region r4]) where it folds",
    ))
}

/// A `--scope` locator.
pub(crate) fn scope_locator(
    scope: Option<&str>,
    options: &LocateOptions,
) -> Result<Option<Locator>, ErrorInfo> {
    scope
        .map(|scope| {
            options.parse(scope).map_err(|e| {
                let message = format!("--scope: {}", e.message);
                ErrorInfo { message, ..e }
            })
        })
        .transpose()
}

fn locate_options(req: &SnapshotRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

impl Engine {
    /// `snapshot`: the active tab (or `--tab`) as an accessibility tree.
    pub async fn snapshot(&self, req: SnapshotRequest) -> Result<Output, ErrorInfo> {
        check_snapshot(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let output = self.snapshot_page(&running, &page, &req, mark).await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// Reads `page` as `req` asks (its `tab` is not looked at); the caller
    /// holds the tab's lock. `mark` is the event mark from before the
    /// command, for a dialog that blocks the read.
    pub(crate) async fn snapshot_page(
        &self,
        running: &Running,
        page: &Page,
        req: &SnapshotRequest,
        mark: u64,
    ) -> Result<Output, ErrorInfo> {
        let options = locate_options(req);
        let scope = scope_locator(req.scope.as_deref(), &options)?;
        let view = View {
            interactive: req.interactive,
            depth: req.depth,
            all_refs: req.all_refs,
            collapse: !req.all,
            text_limit: req.text_limit,
            no_text: req.no_text,
        };
        let engine = req.engine.unwrap_or(DEFAULT_ENGINE);
        let timeout = millis(req.timeout_ms);
        let retry = |r: &str| Cmd::new("snapshot").flag("scope", r);
        let (scope_key, note) = self
            .scope_key(running, page, scope.as_ref(), &options, &retry, timeout)
            .await?;
        // A diff compares unfolded trees: it is short already.
        let folding = if req.diff {
            Folding::default()
        } else {
            Folding {
                repeats: req.fold,
                budget: req.budget.filter(|&b| b > 0),
            }
        };
        let regions = self.expanded_regions(running, page, &req.expand)?;
        let within = if req.expand.is_empty() {
            Within::scope(scope_key.as_ref())
        } else {
            Within::Regions(&regions)
        };
        let Tree {
            capture,
            shown,
            groups,
            before,
            new_page,
        } = self
            .read_tree(running, page, req, &view, within, &folding, mark)
            .await?;

        let tab = tab_name(page.id);
        let mut warnings = Vec::new();
        let mut text = header(page, &capture);
        let (nodes, diff): (Vec<SnapshotNode>, Option<bool>) = if req.diff {
            let compared = before.is_some();
            let changes = match &before {
                Some(old) => render::diff(old, &shown),
                None => render::all_added(&shown),
            };
            if !compared {
                text.push_str("\n(no earlier snapshot of this tab; all of it is new)");
            } else if new_page {
                // Nodes of a new document never match the old ones by key,
                // so the diff matches them by place.
                text.push_str(
                    "\n(a new page since the last snapshot; its nodes are compared by place)",
                );
            }
            if compared && changes.is_empty() {
                text.push_str("\n(no changes since the last snapshot)");
            }
            render::diff_text(&changes, &mut text);
            (
                changes.iter().map(render::diff_json).collect(),
                Some(compared),
            )
        } else if req.source {
            let refs = refs_of(&shown);
            let sources = self.node_sources(running, page, &refs, timeout).await;
            let origin = crate::input::origin(&capture.url);
            let suffixes = sources
                .iter()
                .map(|(r, source)| (*r, crate::source::node_source_text(source, origin)))
                .collect();
            group_text(&shown, &groups, &regions, &suffixes, &mut text);
            let mut nodes = json_groups(&shown, &groups, &regions);
            render::json_sources(&mut nodes, &sources);
            if sources.is_empty()
                && let Some((first, _)) = refs.first()
            {
                warnings.push(format!(
                    "--source found nothing on this page (no framework's debug info, no node \
                     stacks); {} says why",
                    Cmd::new("source")
                        .arg(format!("e{first}"))
                        .scoped(&self.session)
                ));
            }
            (nodes, None)
        } else {
            group_text(&shown, &groups, &regions, &HashMap::new(), &mut text);
            (json_groups(&shown, &groups, &regions), None)
        };
        if let Some(first) = folded_regions(&shown).first() {
            warnings.push(format!(
                "parts of the page are folded; {} shows one",
                Cmd::new("snapshot")
                    .flag("expand", format!("r{first}"))
                    .scoped(&self.session)
            ));
        }
        let data = SnapshotData {
            tab,
            url: capture.url,
            title: capture.title,
            engine: engine.as_str().to_owned(),
            diff,
            new_page: new_page && req.diff,
            nodes,
        };
        let more = "--interactive, --scope <ref> or --max-chars 0 for all of it";
        let text = fit(&text, req.max_chars, more);
        let mut output = Output::typed(&data, text).limit(req.max_chars, more);
        if let Some(note) = note {
            output = output.warning(note);
        }
        for warning in warnings {
            output = output.warning(warning);
        }
        Ok(output)
    }

    /// Reads `page` with `req`'s engine and gives refs to the nodes
    /// `view` shows (of the part `within` names), folded as `folding`
    /// says, remembering the tree for the next `--diff`. The caller holds
    /// the tab's lock.
    #[allow(clippy::too_many_arguments)]
    async fn read_tree(
        &self,
        running: &Running,
        page: &Page,
        req: &SnapshotRequest,
        view: &View,
        within: Within<'_>,
        folding: &Folding,
        mark: u64,
    ) -> Result<Tree, ErrorInfo> {
        let engine = req.engine.unwrap_or(DEFAULT_ENGINE);
        let timeout = millis(req.timeout_ms);
        // A frame that loads a new document while the page is read would
        // leave refs on nodes that are gone: read it again.
        let mut attempts = 0;
        loop {
            attempts += 1;
            let generation = running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation())
                .unwrap_or_default();
            let frames = running.tabs.frame_sources(page.id);
            let read = async {
                match engine {
                    SnapshotEngine::Ax => {
                        AxReader {
                            session: &self.session,
                        }
                        .read(&frames, within_key(within), timeout)
                        .await
                    }
                    SnapshotEngine::Dom => {
                        DomReader {
                            running,
                            page,
                            engine: self,
                        }
                        .read(&frames, within_key(within), timeout)
                        .await
                    }
                }
            };
            let capture = self.unless_blocked(running, page, mark, read).await?;
            let mut groups: Vec<Vec<Node>> = match within.groups(&capture.nodes) {
                Ok(groups) => groups
                    .into_iter()
                    .map(|group| prune::shape_uncollapsed(group, view))
                    .collect(),
                Err(None) => return Err(self.scope_gone(req.scope.as_deref(), page)),
                Err(Some(region)) => return Err(self.region_gone(region, page)),
            };
            let numbers: Vec<u32> = match within {
                Within::Regions(regions) => regions.iter().map(|(n, _)| *n).collect(),
                Within::Page | Within::Node(_) => Vec::new(),
            };
            let frame = Frame {
                extra_chars: header(page, &capture).chars().count() + region_labels(within),
                roots: !matches!(within, Within::Page),
                regions: &numbers,
            };
            let assigned = running.tabs.snapshot_memory(page.id, |memory| {
                if memory.refs.generation() != generation && attempts < READ_ATTEMPTS {
                    return None;
                }
                fold::arrange(
                    &mut groups,
                    folding,
                    frame,
                    view.collapse,
                    &mut Registries {
                        refs: &mut memory.refs,
                        regions: &mut memory.regions,
                        all_refs: view.all_refs,
                        engine,
                    },
                );
                let before = memory.last.replace(capture.nodes.clone());
                let new_page = std::mem::take(&mut memory.new_page) && before.is_some();
                let before = before.map(|old| {
                    let old = within.groups(&old).unwrap_or_default().concat();
                    let mut old = prune::shape(old, view);
                    prune::lookup_refs(&mut old, &memory.refs);
                    old
                });
                Some((before, new_page))
            });
            let (before, new_page) = match assigned {
                Some(Some(found)) => found,
                Some(None) => continue,
                None => {
                    // The tab is gone: no refs to give.
                    fold::arrange(
                        &mut groups,
                        &Folding::default(),
                        frame,
                        view.collapse,
                        &mut Registries {
                            refs: &mut Refs::default(),
                            regions: &mut Regions::default(),
                            all_refs: false,
                            engine,
                        },
                    );
                    for group in &mut groups {
                        forget_refs(group);
                    }
                    (None, false)
                }
            };
            return Ok(Tree {
                capture,
                groups: groups.iter().map(Vec::len).collect(),
                shown: groups.concat(),
                before,
                new_page,
            });
        }
    }

    /// The refs `snapshot -i` shows for `page` now, with their nodes, in
    /// page order; reading them counts as the tab's last snapshot (for
    /// `--diff`). The caller holds the tab's lock.
    pub(crate) async fn interactive_refs(
        &self,
        running: &Running,
        page: &Page,
        mark: u64,
        timeout: Duration,
    ) -> Result<Vec<(u32, NodeKey)>, ErrorInfo> {
        let req = SnapshotRequest {
            tab: None,
            interactive: true,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 0,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            source: false,
            fold: false,
            budget: None,
            expand: Vec::new(),
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            max_chars: 0,
        };
        let view = View {
            interactive: true,
            ..View::default()
        };
        let tree = self
            .read_tree(
                running,
                page,
                &req,
                &view,
                Within::Page,
                &Folding::default(),
                mark,
            )
            .await?;
        Ok(refs_of(&tree.shown))
    }

    /// `page` as `expect-snapshot` compares it: the lines `snapshot -i`
    /// (with `full`, `snapshot --all`) prints after its header, without
    /// refs, so a baseline holds from one session to the next. Long lists
    /// are not collapsed: a change past the cut would not show. Reading it
    /// counts as the tab's last snapshot (for `--diff`). `retry` is the
    /// command an `Ambiguous` scope's hint runs with a candidate's ref.
    /// The caller holds the tab's lock.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn baseline_text(
        &self,
        running: &Running,
        page: &Page,
        scope: Option<&Locator>,
        options: &LocateOptions,
        full: bool,
        retry: &(dyn Fn(&str) -> Cmd + Sync),
        mark: u64,
        timeout: Duration,
    ) -> Result<String, ErrorInfo> {
        let view = View {
            interactive: !full,
            collapse: false,
            ..View::default()
        };
        let req = SnapshotRequest {
            tab: None,
            interactive: view.interactive,
            scope: scope.map(ToString::to_string),
            depth: None,
            diff: false,
            all_refs: false,
            all: true,
            text_limit: view.text_limit,
            no_text: false,
            engine: None,
            strict_refs: options.strict_refs,
            testid_attribute: options.testid_attribute.clone(),
            source: false,
            fold: false,
            budget: None,
            expand: Vec::new(),
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            max_chars: 0,
        };
        let (scope_key, _) = self
            .scope_key(running, page, scope, options, retry, timeout)
            .await?;
        let mut tree = self
            .read_tree(
                running,
                page,
                &req,
                &view,
                Within::scope(scope_key.as_ref()),
                &Folding::default(),
                mark,
            )
            .await?;
        forget_refs(&mut tree.shown);
        let mut text = String::new();
        render::text(&tree.shown, &mut text);
        Ok(text.trim_start_matches('\n').to_owned())
    }

    /// The node a `--scope` locator names, and the note if its ref was
    /// re-resolved. `retry` is the command an `Ambiguous` hint runs with
    /// a candidate's ref.
    pub(crate) async fn scope_key(
        &self,
        running: &Running,
        page: &Page,
        scope: Option<&Locator>,
        options: &LocateOptions,
        retry: &(dyn Fn(&str) -> Cmd + Sync),
        timeout: Duration,
    ) -> Result<(Option<NodeKey>, Option<String>), ErrorInfo> {
        let Some(scope) = scope else {
            return Ok((None, None));
        };
        let target = self
            .locate_one(running, page, scope, options, Some(retry), timeout)
            .await?;
        let found = (Some(target.key.clone()), target.note.clone());
        target.release();
        Ok(found)
    }

    /// The regions `--expand` names, as the tab keeps them.
    fn expanded_regions(
        &self,
        running: &Running,
        page: &Page,
        expand: &[String],
    ) -> Result<Vec<(u32, Region)>, ErrorInfo> {
        let numbers = expand
            .iter()
            .map(|r| parse_region(r).ok_or_else(|| bad_region(r)))
            .collect::<Result<Vec<_>, _>>()?;
        let found = running
            .tabs
            .snapshot_memory(page.id, |m| {
                numbers
                    .iter()
                    .map(|&n| (n, m.regions.lookup(n)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let tab = tab_name(page.id);
        found
            .into_iter()
            .map(|(n, lookup)| match lookup {
                regions::Lookup::Live(region) => Ok((n, region)),
                regions::Lookup::StalePage => Err(self.stale_region(
                    n,
                    format!("region r{n} is from an earlier page of tab {tab}"),
                )),
                regions::Lookup::StaleFrame => Err(self.stale_region(
                    n,
                    format!("region r{n} is from an earlier document of its frame in tab {tab}"),
                )),
                regions::Lookup::Unknown => {
                    Err(self.stale_region(n, format!("tab {tab} has no region r{n}")))
                }
            })
            .collect()
    }

    /// A region that cannot be expanded: take a new snapshot.
    fn stale_region(&self, number: u32, message: String) -> ErrorInfo {
        ErrorInfo::new(ErrorKind::RefStale, message)
            .hint(
                Hint::run(Cmd::new("snapshot").switch("fold").scoped(&self.session))
                    .why("gives the page's regions now"),
            )
            .details(serde_json::json!({ "region": format!("r{number}") }))
    }

    /// The error when a region's nodes are no longer in the page.
    fn region_gone(&self, number: u32, page: &Page) -> ErrorInfo {
        self.stale_region(
            number,
            format!(
                "region r{number} is no longer in tab {}: the page removed it",
                tab_name(page.id)
            ),
        )
    }

    /// The error when the `--scope` node is not in the tree.
    pub(crate) fn scope_gone(&self, scope: Option<&str>, page: &Page) -> ErrorInfo {
        let scope = scope.unwrap_or_default();
        ErrorInfo::new(
            ErrorKind::NotFound,
            format!(
                "--scope {scope} is not shown in tab {}: it is hidden or no longer in the page",
                tab_name(page.id)
            ),
        )
        .hint(
            Hint::run(Cmd::new("snapshot").scoped(&self.session))
                .why("shows what the page has now"),
        )
    }
}

/// Renders `shown`, with `sources`' suffixes; with `--expand`, a region
/// at a time, each after its label.
fn group_text(
    shown: &[Node],
    groups: &[usize],
    regions: &[(u32, Region)],
    sources: &HashMap<u32, String>,
    text: &mut String,
) {
    if regions.is_empty() {
        render::text_with_sources(shown, sources, text);
        return;
    }
    let mut rest = shown;
    for (size, (number, _)) in groups.iter().zip(regions) {
        let (group, after) = rest.split_at((*size).min(rest.len()));
        let _ = write!(text, "\nregion r{number}");
        render::region_text(group, *number, sources, text);
        rest = after;
    }
}

/// The `--json` nodes of `shown`; with `--expand`, each top-level node
/// names its region.
fn json_groups(shown: &[Node], groups: &[usize], regions: &[(u32, Region)]) -> Vec<SnapshotNode> {
    let mut nodes: Vec<SnapshotNode> = shown.iter().map(render::json).collect();
    if !regions.is_empty() {
        let mut at = 0;
        for (size, (number, _)) in groups.iter().zip(regions) {
            for node in nodes.iter_mut().skip(at).take(*size) {
                node.region = Some(format!("r{number}"));
            }
            at += size;
        }
    }
    nodes
}

/// The node a reader must return with its key: `--scope`'s.
fn within_key(within: Within<'_>) -> Option<&NodeKey> {
    match within {
        Within::Node(key) => Some(key),
        Within::Page | Within::Regions(_) => None,
    }
}

/// A snapshot's first line: `page t1 "Title" https://…`.
fn header(page: &Page, capture: &Capture) -> String {
    format!(
        "page {} \"{}\" {}",
        tab_name(page.id),
        capture.title.replace('"', "\\\""),
        capture.url
    )
}

/// The characters of the lines before each region `--expand` shows
/// (`region r4`, [`group_text`]).
fn region_labels(within: Within<'_>) -> usize {
    match within {
        Within::Regions(regions) => regions
            .iter()
            .map(|(number, _)| format!("\nregion r{number}").chars().count())
            .sum(),
        Within::Page | Within::Node(_) => 0,
    }
}

/// The regions shown in `nodes`, in page order.
fn folded_regions(nodes: &[Node]) -> Vec<u32> {
    let mut found = Vec::new();
    for node in nodes {
        node.walk(&mut |n| {
            if let Some(fold) = &n.fold {
                found.push(fold.region);
            }
        });
    }
    found
}

/// The refs of `nodes` and their descendants, with their nodes, in page
/// order.
fn refs_of(nodes: &[Node]) -> Vec<(u32, NodeKey)> {
    let mut refs = Vec::new();
    for node in nodes {
        node.walk(&mut |n| {
            if let (Some(reference), Some(key)) = (n.reference, &n.key) {
                refs.push((reference, key.clone()));
            }
        });
    }
    refs
}

/// Takes the refs off `nodes` and their descendants.
fn forget_refs(nodes: &mut [Node]) {
    for node in nodes {
        node.reference = None;
        forget_refs(&mut node.children);
    }
}

/// How often `snapshot` reads a page that keeps loading new documents
/// before it gives refs anyway (a redirect loop; refs on nodes that went
/// then resolve as if a re-render had replaced them).
const READ_ATTEMPTS: u32 = 3;

/// `nodes`, or the subtree of the `--scope` node (`None` if it is not in
/// them).
pub(crate) fn scoped(scope: Option<&NodeKey>, nodes: &[Node]) -> Option<Vec<Node>> {
    match scope {
        None => Some(nodes.to_vec()),
        Some(key) => prune::find(nodes, key).map(|n| vec![n]),
    }
}

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
