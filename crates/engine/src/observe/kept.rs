//! Response bodies kept for `repro start` (plan 18, milestone 18.5).
//!
//! Chrome keeps a response's body only while the page that loaded it is
//! open (and its buffer has room), so a document and its scripts are gone
//! once the tab navigates. While a repro recording runs, each request in
//! its scope has its body asked of Chrome the moment it finishes loading
//! (see `tabs::network_event`) and kept here, by session and request id,
//! where `har export` finds it. The store is bounded: past [`CAPACITY`]
//! bytes, bodies are counted instead of kept.

use std::collections::HashMap;

/// How many bytes of bodies are kept at most.
pub(crate) const CAPACITY: usize = 512 * 1024 * 1024;

/// A body as `Network.getResponseBody` gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptBody {
    pub(crate) body: String,
    pub(crate) base64: bool,
}

/// The bodies of a repro recording.
#[derive(Debug)]
pub(crate) struct KeptBodies {
    /// The requests whose bodies are kept.
    scope: mock::record::Scope,
    bodies: HashMap<(String, String), KeptBody>,
    bytes: usize,
    capacity: usize,
    /// Bodies not kept because the store was full.
    pub(crate) over: usize,
}

impl KeptBodies {
    pub(crate) fn new(scope: mock::record::Scope) -> Self {
        Self::with_capacity(scope, CAPACITY)
    }

    fn with_capacity(scope: mock::record::Scope, capacity: usize) -> Self {
        Self {
            scope,
            bodies: HashMap::new(),
            bytes: 0,
            capacity,
            over: 0,
        }
    }

    /// Whether the body of a request of `url` and resource type `kind` is
    /// wanted.
    pub(crate) fn wants(&self, url: &str, kind: &str) -> bool {
        self.scope.matches(url, kind)
    }

    /// Whether the store already has request `request_id` of `session`.
    pub(crate) fn has(&self, session: &str, request_id: &str) -> bool {
        self.bodies
            .contains_key(&(session.to_owned(), request_id.to_owned()))
    }

    /// Keeps a body, unless the store is full.
    pub(crate) fn keep(&mut self, session: &str, request_id: &str, body: KeptBody) {
        let size = body.body.len();
        let key = (session.to_owned(), request_id.to_owned());
        let old = self.bodies.get(&key).map_or(0, |b| b.body.len());
        if self.bytes - old + size > self.capacity {
            self.over += 1;
            return;
        }
        self.bodies.insert(key, body);
        self.bytes = self.bytes - old + size;
    }

    pub(crate) fn get(&self, session: &str, request_id: &str) -> Option<&KeptBody> {
        self.bodies
            .get(&(session.to_owned(), request_id.to_owned()))
    }

    /// How many bodies are kept.
    pub(crate) fn len(&self) -> usize {
        self.bodies.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(text: &str) -> KeptBody {
        KeptBody {
            body: text.to_owned(),
            base64: false,
        }
    }

    #[test]
    fn bodies_are_kept_until_the_store_is_full() {
        let scope = mock::record::Scope::new(&["/**".to_owned()]).unwrap();
        let mut kept = KeptBodies::with_capacity(scope, 10);
        assert!(kept.wants("http://a.test/x.js", "script"));
        kept.keep("s", "1", body("12345"));
        kept.keep("s", "2", body("123456"));
        assert_eq!(kept.over, 1);
        assert_eq!(kept.len(), 1);
        // The same request again replaces its body.
        kept.keep("s", "1", body("1234567890"));
        assert_eq!(kept.get("s", "1"), Some(&body("1234567890")));
        assert!(kept.has("s", "1") && !kept.has("s", "2"));
    }

    #[test]
    fn the_scope_picks_the_requests() {
        let scope = mock::record::Scope::new(&["/api/**".to_owned()]).unwrap();
        let kept = KeptBodies::new(scope);
        assert!(kept.wants("http://a.test/api/cart", "fetch"));
        assert!(!kept.wants("http://a.test/app.js", "script"));
    }
}
