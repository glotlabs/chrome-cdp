//! What `explore` counts as one state, and what it follows: URLs
//! normalised, links sorted by kind, and the page's skeleton.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::LazyLock;

use regex::Regex;

use crate::snapshot::{Node, is_landmark};

/// Names of controls `explore` never clicks without
/// `--allow-destructive`, whole words, ignoring case.
static DESTRUCTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(delete|remove|destroy|log ?out|sign ?out|unsubscribe|deactivate|close account|cancel (order|subscription|account)|pay|purchase|buy|place order|checkout|confirm|reset|archive|drop)\b",
    )
    .unwrap_or_else(|e| unreachable!("the destructive pattern compiles: {e}"))
});

/// Extensions of links to files, not pages.
const FILE_EXTENSIONS: &[&str] = &[
    "pdf", "zip", "gz", "tgz", "tar", "bz2", "xz", "rar", "7z", "dmg", "exe", "msi", "pkg", "deb",
    "rpm", "apk", "iso", "bin", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
    "csv", "rtf", "epub", "mp3", "mp4", "m4a", "mov", "avi", "mkv", "wav", "ogg", "webm", "flac",
    "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "tif", "tiff",
];

/// Whether a control named `name` looks destructive.
pub(super) fn destructive(name: &str) -> bool {
    DESTRUCTIVE.is_match(name)
}

/// A URL as a state's key: no fragment (unless it is a route, `#/…` or
/// `#!…`), the query's keys sorted and `utm_*` dropped. A URL that does
/// not parse is itself.
pub(super) fn normalize(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    if parsed
        .fragment()
        .is_none_or(|f| !(f.starts_with('/') || f.starts_with('!')))
    {
        parsed.set_fragment(None);
    }
    if parsed.query().is_some() {
        let mut pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(k, _)| !k.starts_with("utm_"))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        pairs.sort();
        if pairs.is_empty() {
            parsed.set_query(None);
        } else {
            parsed.query_pairs_mut().clear().extend_pairs(pairs);
        }
    }
    parsed.to_string()
}

/// A URL's origin (`http://x:8080`), for http(s) and file URLs.
pub(super) fn origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(parsed.origin().ascii_serialization()),
        "file" => Some("file://".to_owned()),
        _ => None,
    }
}

/// What a link is, to the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkKind {
    /// A page, maybe followed.
    Page,
    /// A file or a download.
    File,
    /// `mailto:`, `tel:`, `javascript:` and other schemes.
    Other,
}

/// Sorts a link to `href` (absolute) by kind; `download` is its
/// attribute.
pub(super) fn link_kind(href: &str, download: bool) -> LinkKind {
    let Ok(parsed) = url::Url::parse(href) else {
        return LinkKind::Other;
    };
    if !matches!(parsed.scheme(), "http" | "https" | "file") {
        return LinkKind::Other;
    }
    let file = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .and_then(|last| last.rsplit_once('.'))
        .is_some_and(|(_, ext)| FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
    if download || file {
        LinkKind::File
    } else {
        LinkKind::Page
    }
}

/// A page's skeleton: a hash of the roles of what `snapshot -i` shows
/// (each role once, so a list with other data is the same) and the names
/// of the headings, landmarks and dialogs, in order; and the names of
/// the dialogs.
pub(super) fn skeleton(nodes: &[Node]) -> (u64, Vec<String>) {
    fn visit(
        node: &Node,
        roles: &mut BTreeSet<String>,
        names: &mut Vec<String>,
        dialogs: &mut Vec<String>,
    ) {
        if node.reffable {
            roles.insert(node.role.clone());
        }
        match node.role.as_str() {
            "dialog" | "alertdialog" => {
                names.push(format!("{} {}", node.role, node.name));
                dialogs.push(node.name.clone());
            }
            "heading" => names.push(format!("heading {}", node.name)),
            role if is_landmark(role) => names.push(format!("{role} {}", node.name)),
            _ => {}
        }
        for child in &node.children {
            visit(child, roles, names, dialogs);
        }
    }
    let (mut roles, mut names, mut dialogs) = (BTreeSet::new(), Vec::new(), Vec::new());
    for node in nodes {
        visit(node, &mut roles, &mut names, &mut dialogs);
    }
    let mut hasher = DefaultHasher::new();
    roles.hash(&mut hasher);
    names.hash(&mut hasher);
    (hasher.finish(), dialogs)
}

#[cfg(test)]
mod tests {
    use super::{LinkKind, destructive, link_kind, normalize};

    #[test]
    fn urls_lose_fragments_tracking_and_query_order() {
        assert_eq!(normalize("http://x/a?b=2&a=1#top"), "http://x/a?a=1&b=2");
        assert_eq!(
            normalize("http://x/a?utm_source=m&utm_medium=e"),
            "http://x/a"
        );
        assert_eq!(normalize("http://x/a?q=1&utm_campaign=z"), "http://x/a?q=1");
        // A hash route is a page of its own.
        assert_eq!(normalize("http://x/#/about"), "http://x/#/about");
        assert_eq!(normalize("http://x/#!/about"), "http://x/#!/about");
        assert_eq!(normalize("http://x/#about"), "http://x/");
        assert_eq!(normalize("not a url"), "not a url");
    }

    #[test]
    fn destructive_names_are_whole_words() {
        for name in [
            "Delete account",
            "Log out",
            "Sign Out",
            "logout",
            "Cancel subscription",
            "Place order",
            "Buy now",
            "Checkout",
            "Reset",
        ] {
            assert!(destructive(name), "{name}");
        }
        for name in [
            "Dropdown",
            "Display",
            "Buyer's guide",
            "Removed items",
            "Cancel",
            "Products",
            "Sign in",
        ] {
            assert!(!destructive(name), "{name}");
        }
    }

    #[test]
    fn links_are_pages_files_or_other() {
        assert_eq!(link_kind("http://x/a.html", false), LinkKind::Page);
        assert_eq!(link_kind("http://x/docs/", false), LinkKind::Page);
        assert_eq!(link_kind("http://x/report.PDF", false), LinkKind::File);
        assert_eq!(link_kind("http://x/export", true), LinkKind::File);
        assert_eq!(link_kind("mailto:a@b.c", false), LinkKind::Other);
        assert_eq!(link_kind("tel:+4712345678", false), LinkKind::Other);
        assert_eq!(link_kind("javascript:void(0)", false), LinkKind::Other);
    }
}
