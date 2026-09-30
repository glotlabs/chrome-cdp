//! From page to source (plan 15): which line of the page's code made an
//! element.
//!
//! Milestone 15.1 has the node's creation stack, as Chrome recorded it
//! (see `tabs::node_stacks`), mapped through source maps to the files on
//! disk, the frameworks' and libraries' frames left out. `source` (15.2)
//! falls back to it when no framework adapter answers.

use protocol::ErrorInfo;
use protocol::schema::StackFrame;
use serde_json::Value;

use crate::Engine;
use crate::errors::from_cdp;
use crate::locator::LocateOptions;
use crate::observe::format::stack_frames;
use crate::request::millis;
use crate::tabs::node_stacks;

/// How many frames of a creation stack are read, across async parents.
const MAX_FRAMES: usize = 64;

/// Where a frame's code comes from, if it is a library's, a framework's
/// or the dev server's rather than the app's own.
const NOT_THE_APP: [&str; 6] = [
    "/node_modules/",
    "/.vite/deps/",
    "/@vite/",
    "/@react-refresh",
    "/webpack/runtime/",
    "/__vite",
];

impl Engine {
    /// The stack that created the element `locator` matches in tab `tab`
    /// (the active one if `None`), innermost first: each frame
    /// source-mapped and found on disk (see `map_stack`), without the
    /// frames of libraries and frameworks (`node_modules/`, Vite's
    /// prebundled deps, dev-server runtimes). `None` when Chrome has no
    /// stack for it: made before node stacks were on (`open
    /// --node-stacks`), or parsed from the HTML.
    pub async fn node_stack(
        &self,
        locator: &str,
        tab: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Option<Vec<StackFrame>>, ErrorInfo> {
        let options = LocateOptions::default();
        let locator = options.parse(locator)?;
        let running = self.running()?;
        let timeout = millis(timeout_ms);
        let page = self.page(&running, tab, timeout).await?;
        let target = {
            let _tab = self.tabs.lock(&page.target_id).await;
            self.locate_one(&running, &page, &locator, &options, None, timeout)
                .await?
        };
        let session = target.context.session.with_timeout(timeout);
        let stack = node_stacks::node_stack(&session, &target.object_id).await;
        target.release();
        let Some(stack) = stack.map_err(|e| from_cdp(&e, &self.session))? else {
            return Ok(None);
        };
        let stack = serde_json::to_value(&stack).unwrap_or(Value::Null);
        let mut frames = Vec::new();
        let mut next = Some(&stack);
        while let Some(stack) = next.filter(|_| frames.len() < MAX_FRAMES) {
            frames.extend(stack_frames(stack));
            next = stack.get("parent");
        }
        frames.truncate(MAX_FRAMES);
        let mut refs: Vec<&mut StackFrame> = frames.iter_mut().collect();
        self.map_stack(&running, &page, &mut refs, true, timeout)
            .await;
        frames.retain(|frame| !not_the_app(frame));
        Ok(Some(frames))
    }
}

/// Whether `frame` is a library's, a framework's or the dev server's.
fn not_the_app(frame: &StackFrame) -> bool {
    let place = frame.original.as_ref().map_or(&frame.url, |o| &o.source);
    NOT_THE_APP.iter().any(|part| place.contains(part))
}

#[cfg(test)]
mod tests {
    use protocol::schema::{SourcePosition, StackFrame};

    use super::not_the_app;

    fn frame(url: &str, source: Option<&str>) -> StackFrame {
        StackFrame {
            function: String::new(),
            url: url.into(),
            line: 1,
            column: 1,
            original: source.map(|source| SourcePosition {
                source: source.into(),
                line: 1,
                column: 1,
                name: None,
                file: None,
            }),
            file: None,
        }
    }

    #[test]
    fn libraries_are_not_the_app() {
        let bundle = "http://localhost:5173/assets/index.js";
        assert!(!not_the_app(&frame(
            bundle,
            Some("http://localhost:5173/src/cart.ts")
        )));
        assert!(not_the_app(&frame(
            bundle,
            Some("http://localhost:5173/node_modules/react-dom/cjs/react-dom.development.js")
        )));
        assert!(not_the_app(&frame(
            "http://localhost:5173/node_modules/.vite/deps/vue.js?v=1",
            None
        )));
        assert!(not_the_app(&frame(
            "http://localhost:5173/@vite/client",
            None
        )));
        assert!(!not_the_app(&frame(bundle, None)));
    }
}
