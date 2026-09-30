//! `components [loc]` (plan 15, milestone 15.3): the page's component
//! tree, or the component that rendered an element, with its props and
//! state.
//!
//! The adapters of `injected/src/frameworks.ts` read the frameworks'
//! development builds in the page's main world (see `source`), preview
//! every value there with fixed limits, and fold alike siblings; the
//! engine resolves their places to files on disk (one pass over the
//! source maps) and prints them. Like `source`, this is the page's own
//! debug info, and nothing acts on it.

use std::fmt::Write as _;

use protocol::schema::{
    ActedElement, ComponentDetail, ComponentFramework, ComponentNode, ComponentValue,
    ComponentsData, SourceComponent, SourcePlace,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use crate::Engine;
use crate::context::Scoped as _;
use crate::frames::World;
use crate::locator::{LocateOptions, until_found};
use crate::request::{ComponentsRequest, millis};
use crate::source::{PageComponent, PagePlace, adapters, place_text};
use crate::tabs::tab_name;

/// The most components the page reads for a tree (`MAX_NODES` in
/// `frameworks.ts`).
const MAX_NODES: u64 = 5000;

/// What the tree adapter found (see `Tree` in `frameworks.ts`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageTree {
    frameworks: Vec<PageFramework>,
    roots: Vec<PageNode>,
    more: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageFramework {
    framework: String,
    version: String,
    dev: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageNode {
    name: String,
    place: Option<PagePlace>,
    props: Vec<ComponentValue>,
    children: Vec<PageNode>,
    count: u64,
    more: u64,
    framework: Option<String>,
}

/// What the adapters found for an element (see `Held` in
/// `frameworks.ts`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Held {
    framework: String,
    version: String,
    dev: bool,
    name: String,
    place: Option<PagePlace>,
    props: Option<Vec<ComponentValue>>,
    state: Option<Vec<ComponentValue>>,
    unavailable: Option<String>,
    owners: Vec<PageComponent>,
    more: u64,
}

impl Engine {
    /// `components`: the page's component tree, or with a locator the
    /// component that rendered the element.
    pub async fn components(&self, req: ComponentsRequest) -> Result<Output, ErrorInfo> {
        match req.locator.clone() {
            Some(locator) => self.component_of(&req, &locator).await,
            None => self.component_tree(&req).await,
        }
    }

    /// The trees from each framework root of the tab's main frame.
    async fn component_tree(&self, req: &ComponentsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let depth = req.depth.unwrap_or(ComponentsRequest::TREE_DEPTH).max(1);
        let mark = running.tabs.mark();
        let work = async {
            let context = self
                .frame_context(&running, &page, None, World::Main, timeout)
                .await?;
            let session = context.session.with_timeout(timeout);
            let result = session
                .call_raw(
                    "Runtime.callFunctionOn",
                    json!({
                        "functionDeclaration": adapters(&format!("__ccdpFrameworks.tree({depth})")),
                        "executionContextId": context.context_id,
                        "returnByValue": true,
                    }),
                )
                .await
                .map_err(|e| crate::errors::from_cdp(&e, &self.session))?;
            if let Some(details) = result.get("exceptionDetails") {
                tracing::debug!("the component tree adapter threw: {details}");
            }
            Ok(
                serde_json::from_value::<PageTree>(result["result"]["value"].clone())
                    .unwrap_or_default(),
            )
        };
        let tree = Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        if tree.roots.is_empty() {
            return Err(self.no_components(&tree.frameworks));
        }
        let mut places = Vec::new();
        for root in &tree.roots {
            collect_places(root, &mut places);
        }
        let mut resolved = self
            .resolve_places(&running, &page, &places, timeout)
            .await
            .into_iter();
        let roots: Vec<ComponentNode> = tree
            .roots
            .into_iter()
            .map(|root| to_node(root, &mut resolved))
            .collect();
        let frameworks = answering(&tree.frameworks);
        let page_origin = Self::page_origin(&running, &page);
        let mut quiet = String::new();
        for root in &roots {
            tree_text(root, 0, page_origin.as_deref(), &mut quiet);
        }
        let text = format!("{quiet}{}", via_text(&frameworks));
        let quiet = quiet.trim_end().to_owned();
        let more = tree.more;
        let data = ComponentsData {
            tab: tab_name(page.id),
            frameworks,
            roots,
            more,
            element: None,
            component: None,
        };
        let mut output = Output::typed(&data, text).quiet(quiet).limit(
            req.max_chars,
            "--depth 2, components LOCATOR for one part, or --max-chars 0",
        );
        if more > 0 {
            output = output.warning(format!(
                "the page has more than {MAX_NODES} components: {more} more were not read"
            ));
        }
        Ok(output)
    }

    /// The component that rendered the element `locator` matches.
    async fn component_of(
        &self,
        req: &ComponentsRequest,
        locator: &str,
    ) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let deadline = Instant::now() + timeout;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let work = async {
            let generation = running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation());
            let retry = |r: &str| Cmd::new("components").arg(r);
            let retry = &retry;
            let target = until_found(deadline, timeout, || {
                let (running, page, locator, options) = (&running, &page, &locator, &options);
                async move {
                    let _tab = self.tabs.lock(&page.target_id).await;
                    self.locate_one(running, page, locator, options, Some(retry), timeout)
                        .await
                }
            })
            .await?;
            let element = self.name_target(&running, &page, &target, generation).await;
            let held = self
                .on_elements(
                    &running,
                    &page,
                    &target.key.frame,
                    &[target.key.backend],
                    "args.map((el) => el instanceof Element ? __ccdpFrameworks.component(el) : null)",
                    timeout,
                )
                .await;
            let note = target.note.clone();
            target.release();
            Ok((element, note, held?))
        };
        let (element, note, held) =
            Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        let held = held
            .into_iter()
            .next()
            .flatten()
            .and_then(|v| serde_json::from_value::<Held>(v).ok())
            .filter(|h| !h.framework.is_empty());
        let held = match held {
            Some(held) if held.dev && !held.name.is_empty() => held,
            held => return Err(self.no_component(&element, held.as_ref())),
        };
        let places: Vec<&PagePlace> = held
            .place
            .iter()
            .chain(held.owners.iter().filter_map(|o| o.place.as_ref()))
            .collect();
        let mut resolved = self
            .resolve_places(&running, &page, &places, timeout)
            .await
            .into_iter();
        let place = held.place.as_ref().and_then(|_| resolved.next().flatten());
        let mut owners: Vec<SourceComponent> = held
            .owners
            .iter()
            .map(|o| SourceComponent {
                name: o.name.clone(),
                place: o.place.as_ref().and_then(|_| resolved.next().flatten()),
            })
            .collect();
        let depth = req.depth.unwrap_or(ComponentsRequest::OWNERS);
        let depth = usize::try_from(depth).unwrap_or(usize::MAX);
        let cut = owners.len().saturating_sub(depth);
        owners.truncate(depth);
        let version = Some(held.version.clone()).filter(|v| !v.is_empty());
        let detail = ComponentDetail {
            name: held.name,
            place,
            framework: held.framework.clone(),
            version: version.clone(),
            props: held.props,
            state: held.state,
            unavailable: held.unavailable,
            owners,
            more: held.more + cut as u64,
        };
        let page_origin = Self::page_origin(&running, &page);
        let frameworks = vec![ComponentFramework {
            name: held.framework,
            version,
        }];
        let text = format!(
            "{}{}",
            detail_text(&detail, &element, page_origin.as_deref()),
            via_text(&frameworks)
        );
        let quiet = detail
            .place
            .as_ref()
            .map(|p| place_text(p, true, page_origin.as_deref()))
            .unwrap_or_default();
        let data = ComponentsData {
            tab: tab_name(page.id),
            frameworks,
            roots: Vec::new(),
            more: 0,
            element: Some(element),
            component: Some(detail),
        };
        let mut output = Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it");
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// The `NoSource` error of a page with no component tree.
    fn no_components(&self, frameworks: &[PageFramework]) -> ErrorInfo {
        if let Some(prod) = frameworks.iter().find(|f| !f.dev) {
            let name = [prod.framework.as_str(), prod.version.as_str()]
                .join(" ")
                .trim()
                .to_owned();
            return ErrorInfo::new(
                ErrorKind::NoSource,
                format!("the page's {name} app is a production build: it keeps no components"),
            )
            .hint(Hint::text(
                "open the app from its dev server (a development build keeps its components), \
                 then run components again",
            ))
            .details(json!({ "reason": "not_dev_build" }));
        }
        ErrorInfo::new(
            ErrorKind::NoSource,
            "no React, Vue or Svelte components on the page",
        )
        .hint(
            Hint::run(Cmd::new("source").arg("LOCATOR").scoped(&self.session)).why(
                "components reads the dev builds of React 18 and 19, Vue 3 and Svelte 5; \
                 source finds where any element came from",
            ),
        )
        .details(json!({ "reason": "no_framework" }))
    }

    /// The `NoSource` error of an element no component of a dev build
    /// rendered.
    fn no_component(&self, element: &ActedElement, held: Option<&Held>) -> ErrorInfo {
        let named = crate::input::describe(element);
        let target = element.reference.clone().unwrap_or_default();
        if let Some(held) = held.filter(|h| !h.dev) {
            let name = [held.framework.as_str(), held.version.as_str()]
                .join(" ")
                .trim()
                .to_owned();
            return ErrorInfo::new(
                ErrorKind::NoSource,
                format!(
                    "{named} was rendered by a production build of {name}: it keeps no components"
                ),
            )
            .hint(Hint::text(
                "open the app from its dev server (a development build keeps its components), \
                 then run components again",
            ))
            .details(json!({ "reason": "not_dev_build" }));
        }
        ErrorInfo::new(
            ErrorKind::NoSource,
            format!("no component of a React, Vue or Svelte dev build rendered {named}"),
        )
        .hint(
            Hint::run(Cmd::new("source").arg(&target).scoped(&self.session))
                .why("where the element came from, framework or not"),
        )
        .details(json!({ "reason": "no_component" }))
    }
}

/// The places of a tree, depth first, as `to_node` takes them.
fn collect_places<'a>(node: &'a PageNode, out: &mut Vec<&'a PagePlace>) {
    if let Some(place) = &node.place {
        out.push(place);
    }
    for child in &node.children {
        collect_places(child, out);
    }
}

/// A tree node, its places taken from `resolved` in `collect_places`'
/// order.
fn to_node(
    node: PageNode,
    resolved: &mut impl Iterator<Item = Option<SourcePlace>>,
) -> ComponentNode {
    let place = node.place.as_ref().and_then(|_| resolved.next().flatten());
    ComponentNode {
        name: node.name,
        place,
        count: node.count.max(1),
        props: node.props,
        framework: node.framework,
        children: node
            .children
            .into_iter()
            .map(|c| to_node(c, resolved))
            .collect(),
        more: node.more,
    }
}

/// The frameworks whose dev builds answered, each once.
fn answering(frameworks: &[PageFramework]) -> Vec<ComponentFramework> {
    let mut out: Vec<ComponentFramework> = Vec::new();
    for f in frameworks.iter().filter(|f| f.dev) {
        let framework = ComponentFramework {
            name: f.framework.clone(),
            version: Some(f.version.clone()).filter(|v| !v.is_empty()),
        };
        if !out.contains(&framework) {
            out.push(framework);
        }
    }
    out
}

/// `via react 19 (the page's debug info)`.
fn via_text(frameworks: &[ComponentFramework]) -> String {
    let names: Vec<String> = frameworks
        .iter()
        .map(|f| match &f.version {
            Some(version) => format!("{} {version}", f.name),
            None => f.name.clone(),
        })
        .collect();
    format!("via {} (the page's debug info)", names.join(", "))
}

/// A value as `name=value`, a React hook's as `#0 value (useState)`.
fn value_text(value: &ComponentValue) -> String {
    match &value.hook {
        Some(hook) => format!("{} {} ({hook})", value.name, value.value),
        None if value.value.is_empty() => value.name.clone(),
        None => format!("{}={}", value.name, value.value),
    }
}

/// A tree's lines, two spaces per level:
///
/// ```text
/// <CartList> src/components/CartList.tsx:12  items=[3 items]
///   <CartItem> ×3 src/components/CartItem.tsx:12  item={…4 keys}
/// ```
fn tree_text(node: &ComponentNode, level: usize, page_origin: Option<&str>, out: &mut String) {
    let indent = "  ".repeat(level);
    let _ = write!(out, "{indent}<{}>", node.name);
    if node.count > 1 {
        let _ = write!(out, " ×{}", node.count);
    }
    if let Some(place) = &node.place {
        let _ = write!(out, " {}", place_text(place, false, page_origin));
    }
    if !node.props.is_empty() {
        let props: Vec<String> = node.props.iter().map(value_text).collect();
        let _ = write!(out, "  {}", props.join(" "));
    }
    out.push('\n');
    for child in &node.children {
        tree_text(child, level + 1, page_origin, out);
    }
    if node.more > 0 {
        let _ = writeln!(
            out,
            "{indent}  … {} below (a larger --depth shows them)",
            count(node.more, "component", "components")
        );
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The component that rendered an element:
///
/// ```text
/// <CartItem> src/components/CartItem.tsx:12  (holds e12)
///   props: item={id: 1, name: "Mug", qty: 2}, onRemove=ƒ
///   state: #0 2 (useState)
///   owners: <CartList> ← <App>
/// ```
fn detail_text(
    detail: &ComponentDetail,
    element: &ActedElement,
    page_origin: Option<&str>,
) -> String {
    let mut text = format!("<{}>", detail.name);
    if let Some(place) = &detail.place {
        let _ = write!(text, " {}", place_text(place, false, page_origin));
    }
    let holds = element
        .reference
        .clone()
        .unwrap_or_else(|| crate::input::describe(element));
    let _ = write!(text, "  (holds {holds})");
    match (&detail.props, &detail.state, &detail.unavailable) {
        (None, None, Some(why)) => {
            let _ = write!(text, "\n  state: not available ({why})");
        }
        (props, state, _) => {
            for (label, values) in [("props", props), ("state", state)] {
                let shown = match values {
                    Some(values) if !values.is_empty() => {
                        values.iter().map(value_text).collect::<Vec<_>>().join(", ")
                    }
                    Some(_) => "none".to_owned(),
                    None => "not available".to_owned(),
                };
                let _ = write!(text, "\n  {label}: {shown}");
            }
        }
    }
    if !detail.owners.is_empty() {
        let mut owners: Vec<String> = detail
            .owners
            .iter()
            .map(|o| format!("<{}>", o.name))
            .collect();
        if detail.more > 0 {
            owners.push(format!("… {} more", detail.more));
        }
        let _ = write!(text, "\n  owners: {}", owners.join(" ← "));
    }
    text.push('\n');
    text
}
