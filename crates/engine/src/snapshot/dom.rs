//! Engine B (plan 04, 4.3): a DOM walker (`injected/src/walker.ts`) in
//! chrome-cdp's isolated world.
//!
//! Every frame is walked at once, in its own document. The walker works
//! out roles, names and states in JavaScript and returns a flat list, plus
//! the elements the engine must address (controls, clickable and scrollable
//! elements, iframes, the `--scope` element); `DOM.describeNode` gives
//! their `backendNodeId`s, and an iframe's the frame it shows. Other nodes
//! have no key: `--all-refs` and `--diff` see them by place only. Closed
//! shadow roots are out of the isolated world's reach.

use std::collections::HashMap;
use std::time::Duration;

use cdp::domains::{dom, runtime};
use futures_util::future::join_all;
use protocol::ErrorInfo;
use serde::Deserialize;
use serde_json::Value;

use super::tree::{Checked, IFRAME, NONE, Node, NodeKey, States};
use super::{Capture, FrameSource, relative_href};
use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, own_properties};
use crate::tabs::Page;
use crate::{Engine, Running};

/// Calls the walker (in the injected bundle) with the `--scope` element
/// (or `null`).
fn walker_js() -> String {
    format!(
        "function (scope) {{ {} return globalThis.__ccdp.walk(scope); }}",
        injected::GUARD
    )
}

/// The object group the walker's results live in until they are read
/// (one per frame: frames of one process share a session).
const GROUP: &str = "chrome-cdp-snapshot";

/// One node as the walker reports it.
#[derive(Debug, Deserialize)]
struct Flat {
    /// The parent's index; -1 at the top.
    p: i64,
    /// A text node.
    #[serde(default)]
    t: u8,
    #[serde(default)]
    r: String,
    #[serde(default)]
    n: String,
    #[serde(default)]
    v: Option<String>,
    #[serde(default)]
    l: Option<u32>,
    #[serde(default)]
    h: Option<String>,
    /// A password field.
    #[serde(default)]
    w: u8,
    #[serde(default)]
    s: Option<FlatStates>,
    /// Inline.
    #[serde(default)]
    i: u8,
    /// Clickable.
    #[serde(default)]
    c: u8,
    /// Scrollable.
    #[serde(default)]
    x: u8,
    /// Its index in `els`.
    #[serde(default)]
    e: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct FlatStates {
    c: Option<String>,
    x: Option<bool>,
    #[serde(default)]
    s: u8,
    #[serde(default)]
    d: u8,
    #[serde(default)]
    r: u8,
    #[serde(default)]
    q: u8,
    #[serde(default)]
    i: u8,
    #[serde(default)]
    f: u8,
    #[serde(default)]
    p: u8,
}

#[derive(Debug, Deserialize)]
struct Walked {
    title: String,
    url: String,
    nodes: Vec<Flat>,
}

/// One frame's walk: its nodes, and for each `els` entry the backend id
/// and, for an iframe, the frame it shows.
struct FrameWalk {
    walked: Walked,
    described: Vec<Option<(i64, Option<String>)>>,
}

pub(crate) async fn capture(
    engine: &Engine,
    running: &Running,
    page: &Page,
    frames: &[FrameSource],
    scope: Option<&NodeKey>,
    timeout: Duration,
) -> Result<Capture, ErrorInfo> {
    let walks = join_all(
        frames
            .iter()
            .map(|frame| walk(running, page, frame, scope, timeout)),
    )
    .await;
    let mut by_frame: HashMap<String, FrameWalk> = HashMap::new();
    for (i, walk) in walks.into_iter().enumerate() {
        match walk {
            Ok(Some(walk)) => {
                by_frame.insert(frames[i].info.id.clone(), walk);
            }
            Ok(None) if i > 0 => {}
            Err(e) if i > 0 => {
                tracing::debug!(frame = %frames[i].info.id, "cannot walk: {e}");
            }
            Ok(None) => return Ok(Capture::default()),
            Err(e) => return Err(from_cdp(&e, engine.session())),
        }
    }
    let Some(main) = frames.first() else {
        return Ok(Capture::default());
    };
    let Some(walk) = by_frame.get(&main.info.id) else {
        return Ok(Capture::default());
    };
    let nodes = build(&main.info.id, &by_frame, 0);
    Ok(Capture {
        url: walk.walked.url.clone(),
        title: walk.walked.title.clone(),
        nodes,
    })
}

/// Walks one frame; `None` if it has no document to walk yet.
async fn walk(
    running: &Running,
    page: &Page,
    frame: &FrameSource,
    scope: Option<&NodeKey>,
    timeout: Duration,
) -> cdp::Result<Option<FrameWalk>> {
    let Ok(context) = running
        .tabs
        .context(page.id, &frame.info.id, World::Isolated, timeout)
        .await
    else {
        return Ok(None);
    };
    let session = context.session.with_timeout(timeout);
    let group = format!("{GROUP}-{}", frame.info.id);
    // The `--scope` element, if it is in this frame.
    let mut scope_object = None;
    if let Some(key) = scope.filter(|k| k.frame == frame.info.id) {
        let params = dom::ResolveNodeParams {
            backend_node_id: Some(key.backend),
            execution_context_id: Some(context.context_id),
            object_group: Some(group.clone()),
            ..dom::ResolveNodeParams::default()
        };
        scope_object = session.call(params).await?.object.object_id;
    }
    let mut params = runtime::CallFunctionOnParams::new(walker_js());
    params.object_group = Some(group.clone());
    params.arguments = Some(vec![runtime::CallArgument {
        value: scope_object.is_none().then_some(Value::Null),
        unserializable_value: None,
        object_id: scope_object,
    }]);
    let result = injected::call(&session, context.context_id, params).await;
    let outcome = read_walk(&session, result).await;
    let _ = session
        .call(runtime::ReleaseObjectGroupParams::new(group))
        .await;
    outcome
}

async fn read_walk(
    session: &cdp::Session,
    result: cdp::Result<runtime::CallFunctionOnReturns>,
) -> cdp::Result<Option<FrameWalk>> {
    let result = result?;
    if let Some(exception) = result.exception_details {
        tracing::debug!("the walker threw: {}", exception.text);
        return Ok(None);
    }
    let Some(object) = result.result.object_id else {
        return Ok(None);
    };
    let properties = own_properties(session, &object).await?;
    let json = properties
        .iter()
        .find(|p| p.name == "json")
        .and_then(|p| {
            p.value
                .as_ref()?
                .value
                .as_ref()?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let Ok(walked) = serde_json::from_str::<Walked>(&json) else {
        return Ok(None);
    };
    let els = properties
        .iter()
        .find(|p| p.name == "els")
        .and_then(|p| p.value.as_ref()?.object_id.clone());
    let mut elements: Vec<(usize, String)> = Vec::new();
    if let Some(els) = els {
        for property in own_properties(session, &els).await? {
            if let (Ok(index), Some(id)) = (
                property.name.parse::<usize>(),
                property.value.and_then(|v| v.object_id),
            ) {
                elements.push((index, id));
            }
        }
    }
    let count = elements.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
    let described = join_all(elements.into_iter().map(|(index, object)| async move {
        let params = dom::DescribeNodeParams {
            object_id: Some(object),
            ..dom::DescribeNodeParams::default()
        };
        let node = session.call(params).await.ok()?.node;
        Some((index, (node.backend_node_id, node.frame_id)))
    }))
    .await;
    let mut out = vec![None; count];
    for (index, found) in described.into_iter().flatten() {
        out[index] = Some(found);
    }
    Ok(Some(FrameWalk {
        walked,
        described: out,
    }))
}

/// The tree of frame `frame`, iframes filled in from `walks`.
fn build(frame: &str, walks: &HashMap<String, FrameWalk>, depth: usize) -> Vec<Node> {
    let Some(walk) = walks.get(frame) else {
        return Vec::new();
    };
    if depth > 16 {
        return Vec::new();
    }
    let flat = &walk.walked.nodes;
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); flat.len()];
    let mut top = Vec::new();
    for (i, node) in flat.iter().enumerate() {
        match usize::try_from(node.p).ok().filter(|&p| p < i) {
            Some(p) => children[p].push(i),
            None => top.push(i),
        }
    }
    let base = &walk.walked.url;
    // Built bottom-up (children come after their parent), no recursion.
    let mut built: Vec<Option<Node>> = (0..flat.len()).map(|_| None).collect();
    for i in (0..flat.len()).rev() {
        let mut node = convert(&flat[i], frame, walk, base);
        if node.role == IFRAME {
            let child_frame = flat[i]
                .e
                .and_then(|e| walk.described.get(e)?.as_ref()?.1.clone());
            if let Some(child_frame) = child_frame {
                node.children = build(&child_frame, walks, depth + 1);
            }
        } else {
            node.children = children[i]
                .iter()
                .filter_map(|&c| built[c].take())
                .collect();
        }
        built[i] = Some(node);
    }
    top.into_iter().filter_map(|i| built[i].take()).collect()
}

fn convert(flat: &Flat, frame: &str, walk: &FrameWalk, base: &str) -> Node {
    let key = flat
        .e
        .and_then(|e| walk.described.get(e)?.as_ref())
        .map(|(backend, _)| NodeKey {
            frame: frame.to_owned(),
            backend: *backend,
        });
    if flat.t == 1 {
        return Node::text(flat.n.clone(), key);
    }
    let s = flat.s.as_ref();
    let states = s.map_or_else(States::default, |s| States {
        checked: s.c.as_deref().map(|c| match c {
            "true" => Checked::True,
            "mixed" => Checked::Mixed,
            _ => Checked::False,
        }),
        expanded: s.x,
        selected: s.s == 1,
        disabled: s.d == 1,
        readonly: s.r == 1,
        required: s.q == 1,
        invalid: s.i == 1,
        focused: s.f == 1,
        pressed: s.p == 1,
    });
    Node {
        role: match flat.r.as_str() {
            "" | "presentation" => NONE.to_owned(),
            role => role.to_owned(),
        },
        name: flat.n.clone(),
        value: flat.v.clone(),
        level: flat.l,
        href: flat.h.as_deref().map(|h| relative_href(h, base)),
        password: flat.w == 1,
        states,
        clickable: flat.c == 1,
        scrollable: flat.x == 1,
        inline: flat.i == 1,
        key,
        ..Node::default()
    }
}
