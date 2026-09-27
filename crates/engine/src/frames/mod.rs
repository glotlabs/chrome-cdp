//! Frames and execution contexts (plan 03, milestone 3.2).
//!
//! A [`FrameTree`] is one tab's frames, kept up to date from the `Page` and
//! `Runtime` events of the tab's session and of its out-of-process iframe
//! (OOPIF) sessions. Each frame knows which session owns it (an OOPIF's
//! frames belong to the OOPIF's own session) and two of its execution
//! contexts: the page's main world, and chrome-cdp's isolated world
//! ([`WORLD_NAME`]), where helper scripts run out of the page's reach.
//!
//! Events of different sessions arrive in separate tasks, so the tree
//! does not rely on their order across sessions: a frame that moves to an
//! OOPIF (`frameDetached` with reason `swap`) is kept, and whichever
//! session reports it navigating last owns it.
//!
//! Frames are addressed by id, by name or by a part of their URL
//! ([`FrameTree::find`]); plan 04's `frame=<name|url-substring|ref>`
//! locator builds on it and adds refs.

pub(crate) mod commands;

use std::collections::{HashMap, VecDeque};

use cdp::domains::{page, runtime};
use serde::{Deserialize, Serialize};

/// The name of chrome-cdp's isolated world in every frame. Page scripts
/// cannot see it or change what it sees of the page's JavaScript globals.
pub const WORLD_NAME: &str = "__chrome_cdp";

/// Where a script runs in a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum World {
    /// The page's own JavaScript world.
    #[default]
    Main,
    /// chrome-cdp's isolated world ([`WORLD_NAME`]): the same DOM, its own
    /// globals.
    Isolated,
}

/// A frame as commands show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameInfo {
    /// Chrome's frame id (a tab's main frame has the tab's target id).
    pub id: String,
    /// The parent frame's id; `None` for the main frame.
    pub parent: Option<String>,
    /// The `name` attribute of its `<iframe>`, or `""`.
    pub name: String,
    /// Its document's URL, with the fragment.
    pub url: String,
    /// 0 for the main frame, 1 for its iframes, …
    pub depth: usize,
    /// Whether it runs in another process than the main frame (an OOPIF).
    pub oopif: bool,
}

/// An execution context, as its session numbers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Context {
    /// The id of the session that reported it.
    pub(crate) session: String,
    pub(crate) id: runtime::ExecutionContextId,
}

/// Why [`FrameTree::find`] found no single frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FindError {
    /// No frame matches; the tab's frames.
    NotFound(Vec<FrameInfo>),
    /// Several frames match; those.
    Ambiguous(Vec<FrameInfo>),
}

/// One tab's frames.
#[derive(Debug)]
pub(crate) struct FrameTree {
    /// The main frame's id.
    root: String,
    /// The tab's own session id.
    root_session: String,
    frames: HashMap<String, Frame>,
    /// Orders siblings by when they were first seen.
    seq: u64,
    /// The main frame's document (loader id).
    root_loader: String,
    /// The subframes of the main frame's earlier documents, by loader id,
    /// newest last: a document restored from the back/forward cache gets
    /// them back, as Chrome does not announce them again.
    cached: VecDeque<(String, Vec<(String, Frame)>)>,
}

/// How many earlier documents' subframes a tab keeps.
const CACHED_DOCUMENTS: usize = 8;

#[derive(Debug)]
struct Frame {
    parent: Option<String>,
    name: String,
    url: String,
    /// The session that owns it.
    session: String,
    main: Option<Context>,
    isolated: Option<Context>,
    seq: u64,
}

impl FrameTree {
    /// The tree of a tab whose main frame is `root` (its target id), owned
    /// by `session`.
    pub(crate) fn new(root: &str, session: &str) -> Self {
        let mut tree = Self {
            root: root.to_owned(),
            root_session: session.to_owned(),
            frames: HashMap::new(),
            seq: 0,
            root_loader: String::new(),
            cached: VecDeque::new(),
        };
        tree.entry(root, session);
        tree
    }

    /// The frame `id`, added (owned by `session`) if it is new.
    fn entry(&mut self, id: &str, session: &str) -> &mut Frame {
        let seq = &mut self.seq;
        self.frames.entry(id.to_owned()).or_insert_with(|| {
            *seq += 1;
            Frame {
                parent: None,
                name: String::new(),
                url: String::new(),
                session: session.to_owned(),
                main: None,
                isolated: None,
                seq: *seq,
            }
        })
    }

    /// Adds the frames of `Page.getFrameTree` that are not known yet. The
    /// answer may be older than events already applied, so it never
    /// overrides them.
    pub(crate) fn seed(&mut self, session: &str, tree: &page::FrameTree) {
        let frame = &tree.frame;
        if frame.parent_id.is_none() && session == self.root_session && frame.id != self.root {
            // Not expected: a tab's main frame has its target's id.
            if let Some(mut old) = self.frames.remove(&self.root) {
                old.parent = None;
                self.frames.insert(frame.id.clone(), old);
            }
            self.root.clone_from(&frame.id);
        }
        if frame.id == self.root && self.root_loader.is_empty() {
            self.root_loader.clone_from(&frame.loader_id);
        }
        let known = self.frames.contains_key(&frame.id);
        let entry = self.entry(&frame.id, session);
        if entry.parent.is_none() {
            entry.parent.clone_from(&frame.parent_id);
        }
        if !known || entry.url.is_empty() {
            entry.name = frame.name.clone().unwrap_or_default();
            entry.url = full_url(frame);
        }
        for child in tree.child_frames.iter().flatten() {
            self.seed(session, child);
            let entry = self.entry(&child.frame.id, session);
            if entry.parent.is_none() {
                entry.parent = Some(frame.id.clone());
            }
        }
    }

    /// `Page.frameAttached` on `session`.
    pub(crate) fn attached(&mut self, session: &str, id: &str, parent: &str) {
        let frame = self.entry(id, session);
        if frame.parent.is_none() {
            frame.parent = Some(parent.to_owned());
        }
    }

    /// `Page.frameNavigated` on `session`: the frame's new document; the
    /// session owns it now. The old document's frames are gone (Chrome
    /// does not always say so); the new one's attach after this event.
    pub(crate) fn navigated(&mut self, session: &str, nav: &page::Frame) {
        let children = self.take_children(&nav.id);
        let is_root = nav.id == self.root;
        if is_root && nav.loader_id != self.root_loader {
            let old = std::mem::replace(&mut self.root_loader, nav.loader_id.clone());
            if !old.is_empty() {
                self.cached.retain(|(loader, _)| *loader != old);
                self.cached.push_back((old, children));
                if self.cached.len() > CACHED_DOCUMENTS {
                    self.cached.pop_front();
                }
            }
        }
        let frame = self.entry(&nav.id, session);
        session.clone_into(&mut frame.session);
        if !is_root && nav.parent_id.is_some() {
            frame.parent.clone_from(&nav.parent_id);
        }
        frame.name = nav.name.clone().unwrap_or_default();
        frame.url = full_url(nav);
    }

    /// `Page.frameNavigated` with type `BackForwardCacheRestore`: an
    /// earlier document of the main frame is back, with the subframes it
    /// had. Frames its sessions announced again meanwhile keep what they
    /// reported; the rest come back from when the document was left.
    pub(crate) fn restored(&mut self, session: &str, nav: &page::Frame) {
        if nav.id != self.root {
            self.navigated(session, nav);
            return;
        }
        let cached = self
            .cached
            .iter()
            .position(|(loader, _)| *loader == nav.loader_id)
            .and_then(|i| self.cached.remove(i))
            .map(|(_, frames)| frames)
            .unwrap_or_default();
        self.navigated(session, nav);
        for (id, frame) in cached {
            match self.frames.get_mut(&id) {
                Some(again) => {
                    again.seq = frame.seq;
                    if again.parent.is_none() {
                        again.parent = frame.parent;
                    }
                    if again.url.is_empty() {
                        again.name = frame.name;
                        again.url = frame.url;
                    }
                }
                None => {
                    self.frames.insert(id, frame);
                }
            }
        }
    }

    /// Removes the subframes of frame `id` and returns them.
    fn take_children(&mut self, id: &str) -> Vec<(String, Frame)> {
        let mut taken = Vec::new();
        let mut parents = vec![id.to_owned()];
        while let Some(parent) = parents.pop() {
            let children: Vec<String> = self
                .frames
                .iter()
                .filter(|(child, f)| f.parent.as_deref() == Some(&parent) && **child != self.root)
                .map(|(child, _)| child.clone())
                .collect();
            for child in children {
                if let Some(frame) = self.frames.remove(&child) {
                    parents.push(child.clone());
                    taken.push((child, frame));
                }
            }
        }
        taken
    }

    /// `Page.navigatedWithinDocument`: a new URL, the same document.
    pub(crate) fn navigated_within_document(&mut self, id: &str, url: &str) {
        if let Some(frame) = self.frames.get_mut(id) {
            url.clone_into(&mut frame.url);
        }
    }

    /// `Page.frameDetached`. A frame swapped into another process stays:
    /// its new session reports it next.
    pub(crate) fn detached(&mut self, id: &str, swap: bool) {
        if !swap {
            self.remove(id);
        }
    }

    /// Removes frame `id` and its descendants; never the main frame.
    fn remove(&mut self, id: &str) {
        if id == self.root {
            return;
        }
        let mut gone = vec![id.to_owned()];
        while let Some(id) = gone.pop() {
            if self.frames.remove(&id).is_none() {
                continue;
            }
            gone.extend(
                self.frames
                    .iter()
                    .filter(|(_, f)| f.parent.as_deref() == Some(&id))
                    .map(|(child, _)| child.clone()),
            );
        }
    }

    /// `Runtime.executionContextCreated` on `session`. Keeps a frame's
    /// default context (main world) and chrome-cdp's isolated one.
    pub(crate) fn context_created(
        &mut self,
        session: &str,
        context: &runtime::ExecutionContextDescription,
    ) {
        let Some(aux) = &context.aux_data else {
            return;
        };
        let Some(frame_id) = aux.get("frameId").and_then(|v| v.as_str()) else {
            return;
        };
        let default = aux.get("isDefault").and_then(serde_json::Value::as_bool) == Some(true);
        let ours = !default && context.name == WORLD_NAME;
        if !default && !ours {
            return;
        }
        let frame = self.entry(frame_id, session);
        let slot = if default {
            &mut frame.main
        } else {
            &mut frame.isolated
        };
        *slot = Some(Context {
            session: session.to_owned(),
            id: context.id,
        });
    }

    /// `Runtime.executionContextDestroyed` on `session`.
    pub(crate) fn context_destroyed(&mut self, session: &str, id: runtime::ExecutionContextId) {
        let gone = |c: &Context| c.session == session && c.id == id;
        for frame in self.frames.values_mut() {
            frame.main.take_if(|c| gone(c));
            frame.isolated.take_if(|c| gone(c));
        }
    }

    /// `Runtime.executionContextsCleared` on `session`.
    pub(crate) fn contexts_cleared(&mut self, session: &str) {
        for frame in self.frames.values_mut() {
            frame.main.take_if(|c| c.session == session);
            frame.isolated.take_if(|c| c.session == session);
        }
    }

    /// Records the isolated world `Page.createIsolatedWorld` made.
    pub(crate) fn isolated_created(&mut self, frame: &str, context: Context) {
        if let Some(frame) = self.frames.get_mut(frame) {
            frame.isolated = Some(context);
        }
    }

    /// An OOPIF session went away: forget the frames it still owns and its
    /// contexts.
    pub(crate) fn session_gone(&mut self, session: &str) {
        let owned: Vec<String> = self
            .frames
            .iter()
            .filter(|(_, f)| f.session == session)
            .map(|(id, _)| id.clone())
            .collect();
        for id in owned {
            self.remove(&id);
        }
        self.contexts_cleared(session);
    }

    /// The id of the session that owns frame `id`.
    pub(crate) fn owner(&self, id: &str) -> Option<&str> {
        self.frames.get(id).map(|f| f.session.as_str())
    }

    /// Frame `id`'s context in `world`, if it has one in its current
    /// session.
    pub(crate) fn context(&self, id: &str, world: World) -> Option<&Context> {
        let frame = self.frames.get(id)?;
        let context = match world {
            World::Main => frame.main.as_ref(),
            World::Isolated => frame.isolated.as_ref(),
        }?;
        (context.session == frame.session).then_some(context)
    }

    /// Every frame, parents before their children, siblings in the order
    /// they appeared.
    pub(crate) fn list(&self) -> Vec<FrameInfo> {
        let mut children: HashMap<&str, Vec<(&str, &Frame)>> = HashMap::new();
        for (id, frame) in &self.frames {
            if *id != self.root {
                let parent = frame.parent.as_deref().unwrap_or_default();
                children.entry(parent).or_default().push((id, frame));
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by_key(|(_, f)| f.seq);
        }
        let mut out = Vec::with_capacity(self.frames.len());
        let mut stack = vec![(self.root.as_str(), 0)];
        while let Some((id, depth)) = stack.pop() {
            let Some(frame) = self.frames.get(id) else {
                continue;
            };
            out.push(self.info(id, frame, depth));
            if let Some(kids) = children.get(id) {
                stack.extend(kids.iter().rev().map(|(child, _)| (*child, depth + 1)));
            }
        }
        // Frames whose parent is not known (yet): still addressable.
        if out.len() < self.frames.len() {
            let mut rest: Vec<(&String, &Frame)> = self
                .frames
                .iter()
                .filter(|(id, _)| !out.iter().any(|f| &f.id == *id))
                .collect();
            rest.sort_by_key(|(_, f)| f.seq);
            out.extend(rest.into_iter().map(|(id, f)| self.info(id, f, 1)));
        }
        out
    }

    fn info(&self, id: &str, frame: &Frame, depth: usize) -> FrameInfo {
        FrameInfo {
            id: id.to_owned(),
            parent: frame.parent.clone().filter(|_| id != self.root),
            name: frame.name.clone(),
            url: frame.url.clone(),
            depth,
            oopif: frame.session != self.root_session,
        }
    }

    /// The frame `query` names: its id, else an iframe whose name is
    /// `query`, else one whose URL contains it. The main frame matches by
    /// id only.
    pub(crate) fn find(&self, query: &str) -> Result<FrameInfo, FindError> {
        let frames = self.list();
        if let Some(frame) = frames.iter().find(|f| f.id == query) {
            return Ok(frame.clone());
        }
        let iframes = || {
            frames
                .iter()
                .filter(|f| f.parent.is_some() && !query.is_empty())
        };
        let by_name: Vec<&FrameInfo> = iframes().filter(|f| f.name == query).collect();
        let matches = if by_name.is_empty() {
            iframes().filter(|f| f.url.contains(query)).collect()
        } else {
            by_name
        };
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(FindError::NotFound(frames.clone())),
            many => Err(FindError::Ambiguous(
                many.iter().map(|f| (*f).clone()).collect(),
            )),
        }
    }
}

/// A frame's URL with its fragment; for Chrome's error page, the URL that
/// failed to load (as the address bar shows it).
fn full_url(frame: &page::Frame) -> String {
    if let Some(unreachable) = &frame.unreachable_url {
        return unreachable.clone();
    }
    match &frame.url_fragment {
        Some(fragment) if !frame.url.contains('#') => format!("{}{fragment}", frame.url),
        _ => frame.url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn frame(id: &str, parent: Option<&str>, name: &str, url: &str) -> page::Frame {
        serde_json::from_value(json!({
            "id": id,
            "parentId": parent,
            "loaderId": "L",
            "name": name,
            "url": url,
            "domainAndRegistry": "",
            "securityOrigin": "",
            "mimeType": "text/html",
            "secureContextType": "Secure",
            "crossOriginIsolatedContextType": "NotIsolated",
            "gatedAPIFeatures": [],
        }))
        .unwrap()
    }

    fn context(
        id: i64,
        frame: &str,
        default: bool,
        name: &str,
    ) -> runtime::ExecutionContextDescription {
        serde_json::from_value(json!({
            "id": id,
            "origin": "",
            "name": name,
            "uniqueId": format!("u{id}"),
            "auxData": { "frameId": frame, "isDefault": default, "type": if default { "default" } else { "isolated" } },
        }))
        .unwrap()
    }

    /// A tab `T` (session `s1`) with iframes `A` (named `pay`) and `B`,
    /// and `C` inside `A`.
    fn tree() -> FrameTree {
        let mut tree = FrameTree::new("T", "s1");
        tree.navigated("s1", &frame("T", None, "", "http://a.test/"));
        tree.attached("s1", "A", "T");
        tree.attached("s1", "B", "T");
        tree.navigated(
            "s1",
            &frame("A", Some("T"), "pay", "http://a.test/pay.html"),
        );
        tree.navigated("s1", &frame("B", Some("T"), "", "http://b.test/ad.html"));
        tree.attached("s1", "C", "A");
        tree.navigated("s1", &frame("C", Some("A"), "", "http://a.test/inner.html"));
        tree
    }

    fn ids(tree: &FrameTree) -> Vec<(String, usize)> {
        tree.list().into_iter().map(|f| (f.id, f.depth)).collect()
    }

    #[test]
    fn lists_parents_before_children_in_order() {
        let tree = tree();
        let expected = [("T", 0), ("A", 1), ("C", 2), ("B", 1)];
        assert_eq!(
            ids(&tree),
            expected.map(|(id, d)| (id.to_owned(), d)).to_vec()
        );
        let root = &tree.list()[0];
        assert_eq!(root.parent, None);
        assert_eq!(root.url, "http://a.test/");
    }

    #[test]
    fn detaching_removes_a_subtree_but_never_the_main_frame() {
        let mut tree = tree();
        tree.detached("A", false);
        assert_eq!(ids(&tree), vec![("T".into(), 0), ("B".into(), 1)]);
        tree.detached("T", false);
        assert_eq!(tree.list().len(), 2);
    }

    #[test]
    fn a_frame_swapped_into_an_oopif_moves_to_its_session() {
        let mut tree = tree();
        tree.context_created("s1", &context(5, "B", true, ""));
        // The parent reports the swap; the OOPIF session reports the frame.
        tree.detached("B", true);
        assert_eq!(tree.owner("B"), Some("s1"));
        tree.navigated("s2", &frame("B", Some("T"), "", "http://b.test/ad.html"));
        assert_eq!(tree.owner("B"), Some("s2"));
        let b = tree.list().into_iter().find(|f| f.id == "B").unwrap();
        assert!(b.oopif);
        // The old process's context is not the frame's any more.
        assert_eq!(tree.context("B", World::Main), None);
        tree.context_created("s2", &context(1, "B", true, ""));
        assert_eq!(tree.context("B", World::Main).unwrap().id, 1);
        // The OOPIF session goes away with the frame.
        tree.session_gone("s2");
        assert!(tree.list().iter().all(|f| f.id != "B"));
    }

    #[test]
    fn a_new_document_drops_the_old_ones_frames() {
        let mut tree = tree();
        tree.navigated("s1", &frame("T", None, "", "http://a.test/next"));
        assert_eq!(ids(&tree), vec![("T".into(), 0)]);
    }

    #[test]
    fn the_oopif_session_may_report_first() {
        let mut tree = FrameTree::new("T", "s1");
        tree.navigated("s2", &frame("B", Some("T"), "", "http://b.test/"));
        tree.attached("s1", "B", "T");
        tree.detached("B", true);
        assert_eq!(tree.owner("B"), Some("s2"));
        assert_eq!(ids(&tree), vec![("T".into(), 0), ("B".into(), 1)]);
    }

    #[test]
    fn contexts_follow_their_worlds() {
        let mut tree = tree();
        tree.context_created("s1", &context(1, "T", true, ""));
        tree.context_created("s1", &context(2, "T", false, WORLD_NAME));
        tree.context_created("s1", &context(3, "T", false, "some extension"));
        assert_eq!(tree.context("T", World::Main).unwrap().id, 1);
        assert_eq!(tree.context("T", World::Isolated).unwrap().id, 2);
        tree.context_destroyed("s1", 2);
        assert_eq!(tree.context("T", World::Isolated), None);
        assert_eq!(tree.context("T", World::Main).unwrap().id, 1);
        tree.contexts_cleared("s1");
        assert_eq!(tree.context("T", World::Main), None);
        tree.isolated_created(
            "T",
            Context {
                session: "s1".into(),
                id: 9,
            },
        );
        assert_eq!(tree.context("T", World::Isolated).unwrap().id, 9);
    }

    #[test]
    fn a_document_back_from_the_cache_gets_its_frames_back() {
        let at = |loader: &str, url: &str| {
            let mut nav = frame("T", None, "", url);
            nav.loader_id = loader.into();
            nav
        };
        let mut tree = tree();
        tree.context_created("s1", &context(5, "A", true, ""));
        tree.navigated("s1", &at("L2", "http://a.test/next"));
        tree.attached("s1", "D", "T");
        assert_eq!(ids(&tree), [("T".into(), 0), ("D".into(), 1)]);
        // Chrome announces the cross-site frame `B` again, but not `A`.
        tree.restored("s1", &at("L", "http://a.test/"));
        tree.navigated("s2", &frame("B", Some("T"), "", "http://b.test/ad.html"));
        assert_eq!(
            ids(&tree),
            [
                ("T".into(), 0),
                ("A".into(), 1),
                ("C".into(), 2),
                ("B".into(), 1)
            ]
        );
        assert_eq!(tree.find("pay").unwrap().id, "A");
        assert_eq!(tree.context("A", World::Main).unwrap().id, 5);
        assert_eq!(tree.owner("B"), Some("s2"));
        // And forward again: `D` is back.
        tree.restored("s1", &at("L2", "http://a.test/next"));
        assert_eq!(ids(&tree), [("T".into(), 0), ("D".into(), 1)]);
    }

    #[test]
    fn an_error_page_has_the_url_that_failed() {
        let mut tree = FrameTree::new("T", "s1");
        let mut nav = frame("T", None, "", "chrome-error://chromewebdata/");
        nav.unreachable_url = Some("http://localhost:3000/".into());
        tree.navigated("s1", &nav);
        assert_eq!(tree.list()[0].url, "http://localhost:3000/");
    }

    #[test]
    fn seeding_adds_only_unknown_frames() {
        let mut tree = FrameTree::new("T", "s1");
        tree.navigated("s1", &frame("T", None, "", "http://a.test/new"));
        let seed: page::FrameTree = serde_json::from_value(json!({
            "frame": frame("T", None, "", "http://a.test/old"),
            "childFrames": [{ "frame": frame("A", Some("T"), "pay", "http://a.test/pay") }],
        }))
        .unwrap();
        tree.seed("s1", &seed);
        let list = tree.list();
        assert_eq!(list[0].url, "http://a.test/new");
        assert_eq!(list[1].id, "A");
        assert_eq!(list[1].name, "pay");
        assert_eq!(list[1].parent.as_deref(), Some("T"));
    }

    #[test]
    fn urls_keep_their_fragment_and_follow_same_document_navigations() {
        let mut tree = FrameTree::new("T", "s1");
        let mut nav = frame("T", None, "", "http://a.test/");
        nav.url_fragment = Some("#top".into());
        tree.navigated("s1", &nav);
        assert_eq!(tree.list()[0].url, "http://a.test/#top");
        tree.navigated_within_document("T", "http://a.test/page/2");
        assert_eq!(tree.list()[0].url, "http://a.test/page/2");
    }

    #[test]
    fn finds_frames_by_id_name_or_url() {
        let tree = tree();
        assert_eq!(tree.find("B").unwrap().id, "B");
        assert_eq!(tree.find("T").unwrap().id, "T");
        assert_eq!(tree.find("pay").unwrap().id, "A");
        assert_eq!(tree.find("ad.html").unwrap().id, "B");
        assert_eq!(tree.find("inner").unwrap().id, "C");
        // The main frame's URL does not count.
        match tree.find("a.test/") {
            Err(FindError::Ambiguous(frames)) => {
                let ids: Vec<&str> = frames.iter().map(|f| f.id.as_str()).collect();
                assert_eq!(ids, ["A", "C"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(tree.find("nope"), Err(FindError::NotFound(all)) if all.len() == 4));
        assert!(matches!(tree.find(""), Err(FindError::NotFound(_))));
    }
}
