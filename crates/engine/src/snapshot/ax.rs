//! Engine A (plan 04, 4.3): Chrome's own accessibility tree.
//!
//! One round of calls, all in flight at once: `Accessibility.getFullAXTree`
//! for every frame (on the session that owns it, so out-of-process iframes
//! work), `DOMSnapshot.captureSnapshot` once per session for what the AX
//! tree does not say (display, `cursor`, overflow and scroll sizes,
//! `onclick`, `tabindex`, `type=password|file`), and `DOM.getFrameOwner` for
//! every iframe, to put each frame's tree under its `<iframe>`. Chrome
//! works out roles, names and hidden nodes, so they follow the specs.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use cdp::Session;
use cdp::domains::{accessibility as ax, dom, dom_snapshot};
use futures_util::future::join_all;
use serde_json::Value;

use super::tree::{Checked, IFRAME, NONE, Node, NodeKey, States, TEXT, is_interactive};
use super::{Capture, FrameSource, relative_href};

/// The computed styles read per node, in this order.
const STYLES: [&str; 4] = ["display", "cursor", "overflow-x", "overflow-y"];

/// Past this depth a tree is cut (the pipeline recurses).
const MAX_DEPTH: usize = 400;

/// What the DOM snapshot says about one element.
#[derive(Debug, Default, Clone)]
struct DomFacts {
    inline: bool,
    pointer: bool,
    /// `cursor: pointer`, but not inherited from its parent.
    pointer_root: bool,
    onclick: bool,
    tabindex: Option<i64>,
    password: bool,
    file: bool,
    scrollable: bool,
    /// `html` or `body`: never clickable or scrollable by our rules.
    document_element: bool,
    /// The parent node's backend id.
    parent: Option<i64>,
    /// `aria-valuetext`, which Chrome does not always pass on.
    valuetext: Option<String>,
}

impl DomFacts {
    /// The clickable heuristic of 4.3: `onclick`, `tabindex >= 0`, or
    /// where a `cursor: pointer` region starts.
    fn clickable(&self) -> bool {
        !self.document_element
            && (self.onclick || self.tabindex.is_some_and(|t| t >= 0) || self.pointer_root)
    }
}

/// Reads the tab through the AX tree. `frames` lists the tab's frames,
/// main frame first, each with the session that owns it.
pub(crate) async fn capture(frames: &[FrameSource], timeout: Duration) -> cdp::Result<Capture> {
    let Some(main) = frames.first() else {
        return Ok(Capture::default());
    };
    // One DOM snapshot per session.
    let mut sessions: Vec<&Session> = Vec::new();
    for frame in frames {
        if !sessions.iter().any(|s| s.id() == frame.session.id()) {
            sessions.push(&frame.session);
        }
    }
    let snapshots = join_all(sessions.iter().map(|session| {
        let mut params = dom_snapshot::CaptureSnapshotParams::new(
            STYLES.iter().map(|s| (*s).to_owned()).collect(),
        );
        params.include_dom_rects = Some(true);
        session.with_timeout(timeout).call(params)
    }));
    let trees = join_all(frames.iter().map(|frame| {
        let params = ax::GetFullAxTreeParams {
            frame_id: Some(frame.info.id.clone()),
            ..ax::GetFullAxTreeParams::default()
        };
        frame.session.with_timeout(timeout).call(params)
    }));
    let owners = join_all(frames.iter().map(|frame| async {
        let parent = frames
            .iter()
            .find(|p| Some(&p.info.id) == frame.info.parent.as_ref())?;
        let owner = parent
            .session
            .with_timeout(timeout)
            .call(dom::GetFrameOwnerParams::new(frame.info.id.clone()))
            .await
            .ok()?;
        Some((parent.info.id.clone(), owner.backend_node_id))
    }));
    let (snapshots, trees, owners) = tokio::join!(snapshots, trees, owners);

    let mut facts: HashMap<String, HashMap<i64, DomFacts>> = HashMap::new();
    for (session, snapshot) in sessions.iter().zip(snapshots) {
        let snapshot = snapshot?;
        facts.insert(
            session.id().unwrap_or_default().to_owned(),
            dom_facts(&snapshot),
        );
    }
    // The frame each `<iframe>` (parent frame, backend id) shows.
    let mut children: HashMap<(String, i64), usize> = HashMap::new();
    for (i, owner) in owners.into_iter().enumerate() {
        if let Some(owner) = owner {
            children.insert(owner, i);
        }
    }
    let mut built: Vec<Option<Vec<ax::AXNode>>> = Vec::new();
    for (i, tree) in trees.into_iter().enumerate() {
        match tree {
            Ok(tree) => built.push(Some(tree.nodes)),
            // The main frame must work; a subframe may be gone or loading.
            Err(e) if i == 0 => return Err(e),
            Err(e) => {
                tracing::debug!(frame = %frames[i].info.id, "no AX tree: {e}");
                built.push(None);
            }
        }
    }
    let present = frames
        .iter()
        .zip(&built)
        .map(|(frame, tree)| {
            let ids = tree
                .iter()
                .flatten()
                .filter_map(|n| n.backend_dom_node_id)
                .collect();
            (frame.info.id.clone(), ids)
        })
        .collect();
    let builder = Builder {
        frames,
        present,
        facts: &facts,
        children: &children,
        trees: &built,
    };
    let (title, nodes) = builder.frame(0, 0);
    Ok(Capture {
        url: main.info.url.clone(),
        title,
        nodes,
    })
}

/// Reads the facts of every element of a DOM snapshot, by backend id.
fn dom_facts(snapshot: &dom_snapshot::CaptureSnapshotReturns) -> HashMap<i64, DomFacts> {
    let strings = &snapshot.strings;
    let string = |i: i64| -> &str {
        usize::try_from(i)
            .ok()
            .and_then(|i| strings.get(i))
            .map_or("", String::as_str)
    };
    let mut out = HashMap::new();
    for doc in &snapshot.documents {
        let nodes = &doc.nodes;
        let backend = nodes.backend_node_id.as_deref().unwrap_or_default();
        let names = nodes.node_name.as_deref().unwrap_or_default();
        let parents = nodes.parent_index.as_deref().unwrap_or_default();
        let mut facts: Vec<DomFacts> = vec![DomFacts::default(); backend.len()];
        if let Some(attributes) = &nodes.attributes {
            for (i, attrs) in attributes.iter().enumerate() {
                let Some(f) = facts.get_mut(i) else { break };
                let name = names.get(i).map_or("", |&n| string(n));
                f.document_element = matches!(name, "HTML" | "BODY");
                let mut input_type = "";
                for pair in attrs.chunks(2) {
                    let [key, value] = pair else { continue };
                    match string(*key) {
                        "onclick" => f.onclick = true,
                        "tabindex" => f.tabindex = string(*value).trim().parse().ok(),
                        "type" => input_type = string(*value),
                        "aria-valuetext" => {
                            f.valuetext = Some(string(*value).to_owned()).filter(|v| !v.is_empty());
                        }
                        _ => {}
                    }
                }
                f.password = name == "INPUT" && input_type.eq_ignore_ascii_case("password");
                f.file = name == "INPUT" && input_type.eq_ignore_ascii_case("file");
            }
        }
        let layout = &doc.layout;
        for (li, &node) in layout.node_index.iter().enumerate() {
            let Some(f) = usize::try_from(node).ok().and_then(|n| facts.get_mut(n)) else {
                continue;
            };
            let styles = layout.styles.get(li).map(Vec::as_slice).unwrap_or_default();
            let style = |i: usize| styles.get(i).map_or("", |&s| string(s));
            let display = style(0);
            f.inline = display == "inline" || display == "contents";
            f.pointer = style(1) == "pointer";
            let overflow = |v: &str| matches!(v, "auto" | "scroll" | "overlay");
            let (x, y) = (overflow(style(2)), overflow(style(3)));
            if (x || y)
                && let (Some(scroll), Some(client)) = (
                    layout.scroll_rects.as_ref().and_then(|r| r.get(li)),
                    layout.client_rects.as_ref().and_then(|r| r.get(li)),
                )
                && let ([_, _, sw, sh], [_, _, cw, ch]) = (scroll.as_slice(), client.as_slice())
            {
                f.scrollable = (x && sw - cw > 1.0) || (y && sh - ch > 1.0);
            }
        }
        for i in 0..facts.len() {
            let parent = parents.get(i).and_then(|&p| usize::try_from(p).ok());
            let pointer = parent.and_then(|p| facts.get(p)).is_some_and(|p| p.pointer);
            facts[i].pointer_root = facts[i].pointer && !pointer;
            facts[i].parent = parent.and_then(|p| backend.get(p).copied());
        }
        for (i, f) in facts.into_iter().enumerate() {
            if let Some(&id) = backend.get(i) {
                out.insert(id, f);
            }
        }
    }
    out
}

struct Builder<'a> {
    frames: &'a [FrameSource],
    /// The backend ids each frame's AX tree has a node for.
    present: HashMap<String, HashSet<i64>>,
    facts: &'a HashMap<String, HashMap<i64, DomFacts>>,
    children: &'a HashMap<(String, i64), usize>,
    trees: &'a [Option<Vec<ax::AXNode>>],
}

impl Builder<'_> {
    /// Frame `i`'s title and nodes (its root's children).
    fn frame(&self, i: usize, depth: usize) -> (String, Vec<Node>) {
        let Some(Some(nodes)) = self.trees.get(i) else {
            return (String::new(), Vec::new());
        };
        let by_id: HashMap<&str, &ax::AXNode> =
            nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
        let Some(root) = nodes.first() else {
            return (String::new(), Vec::new());
        };
        let title = str_value(root.name.as_ref()).unwrap_or_default();
        let frame = &self.frames[i];
        let children = self.children_of(root, &by_id, frame, depth, false);
        (title, children)
    }

    fn children_of(
        &self,
        node: &ax::AXNode,
        by_id: &HashMap<&str, &ax::AXNode>,
        frame: &FrameSource,
        depth: usize,
        editable: bool,
    ) -> Vec<Node> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let children = node
            .child_ids
            .iter()
            .flatten()
            .filter_map(|id| by_id.get(id.as_str()))
            .filter_map(|child| self.node(child, by_id, frame, depth + 1, editable))
            .collect();
        self.restore_clickables(children, frame)
    }

    /// Chrome leaves elements with no role of their own out of the tree
    /// (a `<span>` or a `role=presentation` `<div>`), and with them the
    /// fact that they act on clicks. Puts a clickable one back around what
    /// it held (text, an icon, an image), found through the DOM snapshot's
    /// parents. Controls are left as they are: they have refs already.
    fn restore_clickables(&self, children: Vec<Node>, frame: &FrameSource) -> Vec<Node> {
        let Some(facts) = self.facts.get(frame.session.id().unwrap_or_default()) else {
            return children;
        };
        let present = |backend: i64| {
            self.present
                .get(&frame.info.id)
                .is_some_and(|p| p.contains(&backend))
        };
        // The nearest clickable ancestor with no AX node, below the
        // nearest one that has one.
        let lost = |node: &Node| -> Option<i64> {
            if is_interactive(&node.role) {
                return None;
            }
            let mut current = facts.get(&node.key.as_ref()?.backend)?.parent;
            while let Some(backend) = current {
                if present(backend) {
                    return None;
                }
                let f = facts.get(&backend)?;
                if f.clickable() {
                    return Some(backend);
                }
                current = f.parent;
            }
            None
        };
        let mut out: Vec<Node> = Vec::with_capacity(children.len());
        let mut last: Option<i64> = None;
        for child in children {
            match lost(&child) {
                Some(backend) if last == Some(backend) => {
                    if let Some(wrapper) = out.last_mut() {
                        wrapper.children.push(child);
                    }
                }
                Some(backend) => {
                    last = Some(backend);
                    out.push(Node {
                        role: "generic".to_owned(),
                        clickable: true,
                        inline: facts.get(&backend).is_some_and(|f| f.inline),
                        key: Some(NodeKey {
                            frame: frame.info.id.clone(),
                            backend,
                        }),
                        children: vec![child],
                        ..Node::default()
                    });
                }
                None => {
                    last = None;
                    out.push(child);
                }
            }
        }
        out
    }

    /// Converts `node`; `in_editable` says it is inside an editable
    /// element.
    fn node(
        &self,
        node: &ax::AXNode,
        by_id: &HashMap<&str, &ax::AXNode>,
        frame: &FrameSource,
        depth: usize,
        in_editable: bool,
    ) -> Option<Node> {
        let chrome_role = str_value(node.role.as_ref()).unwrap_or_default();
        let editable = node.properties.iter().flatten().any(|p| {
            p.name == ax::AXPropertyName::Editable
                && p.value
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty())
        });
        let mut role = if node.ignored {
            NONE.to_owned()
        } else {
            role_name(&chrome_role)?
        };
        // A contenteditable element (its editing host) is a text box.
        if editable && !in_editable && role == "generic" {
            "textbox".clone_into(&mut role);
        }
        let key = node.backend_dom_node_id.map(|backend| NodeKey {
            frame: frame.info.id.clone(),
            backend,
        });
        let name = str_value(node.name.as_ref()).unwrap_or_default();
        match role.as_str() {
            TEXT => return Some(Node::text(name, key)),
            "linebreak" => return Some(Node::text("\n", key)),
            _ => {}
        }
        let facts = node
            .backend_dom_node_id
            .and_then(|b| {
                self.facts
                    .get(frame.session.id().unwrap_or_default())?
                    .get(&b)
            })
            .cloned()
            .unwrap_or_default();
        let mut out = Node {
            role,
            name,
            value: value_text(node)
                .or_else(|| facts.valuetext.clone())
                .or_else(|| str_value(node.value.as_ref())),
            password: facts.password,
            file: facts.file,
            clickable: facts.clickable(),
            scrollable: facts.scrollable && !facts.document_element,
            inline: facts.inline,
            key,
            ..Node::default()
        };
        read_properties(node, &mut out, &frame.info.url);
        out.children = if out.role == IFRAME {
            self.iframe_children(node, frame, depth)
        } else {
            self.children_of(node, by_id, frame, depth, in_editable || editable)
        };
        Some(out)
    }

    /// The nodes of the frame an `<iframe>` of `frame` shows.
    fn iframe_children(&self, node: &ax::AXNode, frame: &FrameSource, depth: usize) -> Vec<Node> {
        let child = node
            .backend_dom_node_id
            .and_then(|backend| self.children.get(&(frame.info.id.clone(), backend)));
        match child {
            Some(&child) => self.frame(child, depth).1,
            None => Vec::new(),
        }
    }
}

/// Our role for a Chrome role; `None` for nodes a snapshot never shows.
fn role_name(chrome: &str) -> Option<String> {
    Some(
        match chrome {
            "StaticText" => TEXT,
            "InlineTextBox" | "ListMarker" => return None,
            "LineBreak" => "linebreak",
            "LabelText" => "label",
            "Iframe" | "IframePresentational" => IFRAME,
            "image" | "Canvas" | "SvgRoot" => "img",
            "DisclosureTriangle" => "button",
            // Date and time inputs: filled as text; their parts are
            // Chrome's own.
            "Date" | "DateTime" | "InputTime" => "textbox",
            "MenuListPopup" | "RootWebArea" | "WebArea" | "" => NONE,
            "MenuListOption" => "option",
            other => return Some(other.to_ascii_lowercase()),
        }
        .to_owned(),
    )
}

/// The string (or number) in an AX value.
fn str_value(value: Option<&ax::AXValue>) -> Option<String> {
    match value?.value.as_ref()? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `aria-valuetext` (or a native slider's text), which says more than
/// the number (Chrome also clamps that to the range).
fn value_text(node: &ax::AXNode) -> Option<String> {
    node.properties
        .iter()
        .flatten()
        .find(|p| p.name == ax::AXPropertyName::Valuetext)
        .and_then(|p| str_value(Some(&p.value)))
        .filter(|v| !v.is_empty())
}

/// Copies the AX properties a line shows into `out`.
fn read_properties(node: &ax::AXNode, out: &mut Node, frame_url: &str) {
    let mut states = States::default();
    for property in node.properties.iter().flatten() {
        let value = property.value.value.as_ref();
        let truthy = match value {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s != "false",
            _ => false,
        };
        match property.name {
            ax::AXPropertyName::Checked => {
                states.checked = match value.and_then(Value::as_str) {
                    Some("true") => Some(Checked::True),
                    Some("mixed") => Some(Checked::Mixed),
                    _ => Some(Checked::False),
                };
            }
            ax::AXPropertyName::Pressed => states.pressed = truthy,
            ax::AXPropertyName::Expanded => states.expanded = Some(truthy),
            ax::AXPropertyName::Selected => states.selected = truthy,
            ax::AXPropertyName::Disabled => states.disabled = truthy,
            ax::AXPropertyName::Readonly => states.readonly = truthy,
            ax::AXPropertyName::Required => states.required = truthy,
            ax::AXPropertyName::Invalid => states.invalid = truthy,
            ax::AXPropertyName::Focused => states.focused = truthy,
            ax::AXPropertyName::Level => {
                out.level = value
                    .and_then(Value::as_u64)
                    .and_then(|l| u32::try_from(l).ok());
            }
            ax::AXPropertyName::Url => {
                if let Some(url) = value.and_then(Value::as_str) {
                    out.href = Some(relative_href(url, frame_url));
                }
            }
            _ => {}
        }
    }
    // A text field is only "readonly" when it says so; Chrome also marks
    // every non-editable node.
    if !super::tree::is_text_field(&out.role) && out.role != "combobox" {
        states.readonly = false;
    }
    out.states = states;
}
