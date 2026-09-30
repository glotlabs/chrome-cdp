//! Places as paths on disk (plan 15, milestone 15.1).
//!
//! An agent that sees a place in the page's code wants the file to open,
//! not a URL. [`to_disk`] turns what the browser knows (a source map's
//! source, a script's URL, a framework's file name) into a file under
//! the **project root**, and [`shown`] prints it relative to that root.
//!
//! The root is the CLI's: the directory with the project's config
//! (`chrome-cdp.toml`, `.chrome-cdp.toml`, `.chrome-cdp/`) or `.git`
//! nearest the current one, or `source_root` in the config. It travels
//! beside each call's params (`protocol::rpc::Request::root`), like
//! `--context`: the daemon runs the call inside [`in_root`], and the code
//! that prints places asks [`root`]. The daemon runs on the same machine,
//! so it checks the same disk.

use std::future::Future;
use std::path::{Component, Path, PathBuf};

tokio::task_local! {
    /// The project root of the call being handled.
    static ROOT: Option<PathBuf>;
}

/// Runs `fut` (a call's handling) with `root` as the project root.
pub async fn in_root<F: Future>(root: Option<PathBuf>, fut: F) -> F::Output {
    ROOT.scope(root, fut).await
}

/// The project root of the call being handled, if the CLI sent one.
pub(crate) fn root() -> Option<PathBuf> {
    ROOT.try_with(Clone::clone).ok().flatten()
}

/// The file on disk (an absolute path) that `source` names, if there is
/// one:
///
/// - an absolute path that exists (React 18's `_debugSource.fileName`,
///   Vue's `__file`) is that file;
/// - Vite's `/@fs/<absolute path>` is that path;
/// - otherwise a URL (`http://localhost:5173/src/app.ts?t=1`,
///   `webpack://app/./src/app.ts`) loses its scheme, host, query and
///   fragment, and path segments are dropped from the left until
///   `<root>/<rest>` is a file: the longest such rest wins. A rest of the
///   file name alone does not count (too many `index.ts`). An absolute
///   path that does not exist (a build made elsewhere, in a container)
///   is matched the same way.
pub(crate) fn to_disk(source: &str, root: Option<&Path>) -> Option<PathBuf> {
    let path = path_of(source)?;
    if let Some(fs) = path.strip_prefix("/@fs/") {
        let fs = absolute(fs)?;
        if fs.is_file() {
            return Some(fs);
        }
    }
    if source.starts_with('/') && Path::new(&path).is_file() {
        return Some(PathBuf::from(&path));
    }
    let root = root?;
    let segments: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if segments.contains(&"..") {
        return None;
    }
    (0..segments.len().saturating_sub(1)).find_map(|start| {
        let file = segments[start..]
            .iter()
            .fold(root.to_path_buf(), |dir, s| dir.join(s));
        file.is_file().then_some(file)
    })
}

/// How a file on disk prints: relative to `root` when inside it
/// (`src/app.ts`), else absolute.
pub(crate) fn shown(file: &Path, root: Option<&Path>) -> String {
    let relative = root.and_then(|root| file.strip_prefix(root).ok());
    let path = relative.unwrap_or(file);
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match relative {
        Some(_) => parts.join("/"),
        None => path.to_string_lossy().into_owned(),
    }
}

/// Whether `source` (a URL, a path) and `place` (a path or URL an agent
/// gave, `src/cart.ts`) name the same file: `source` is on disk as
/// `file` (the file `place` names, when it is one), or one's path
/// segments end with all of the other's. `place` may be a file name
/// alone (the agent chose it); `source` must have two segments to be the
/// end of `place` (too many `index.ts`).
pub(crate) fn same_file(
    source: &str,
    place: &str,
    file: Option<&Path>,
    root: Option<&Path>,
) -> bool {
    if let Some(file) = file
        && to_disk(source, root).is_some_and(|found| found == file)
    {
        return true;
    }
    let (Some(source), Some(place)) = (segments(source), segments(place)) else {
        return false;
    };
    if source.is_empty() || place.is_empty() {
        return false;
    }
    source.ends_with(&place) || (source.len() >= 2 && place.ends_with(&source))
}

/// The segments of `source`'s path, without `.`: `webpack://app/./src/x.ts`
/// is `["src", "x.ts"]`.
fn segments(source: &str) -> Option<Vec<String>> {
    let path = path_of(source)?;
    Some(
        path.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(str::to_owned)
            .collect(),
    )
}

/// `source` without its query and fragment: the same file across a hot
/// update's `?t=123`.
pub(crate) fn without_query(source: &str) -> &str {
    source.split(['?', '#']).next().unwrap_or_default()
}

/// `source`'s path: a URL's (decoded, without query and fragment), or
/// `source` itself when it is a path (Svelte's `src/App.svelte` is
/// relative).
fn path_of(source: &str) -> Option<String> {
    let Some((scheme, rest)) = source.split_once("://") else {
        // `data:`, `blob:`, `about:blank` are no files.
        let scheme = source.split_once(':').map(|(s, _)| s);
        return (scheme.is_none_or(|s| s.contains('/')) && !source.contains(' '))
            .then(|| source.to_owned());
    };
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    // `file:///x` has an empty host; `webpack://app/./src` a made-up one.
    let path = rest.find('/').map_or("", |at| &rest[at..]);
    Some(percent_decode(path))
}

/// `/home/me/app/src/x.ts` (after `/@fs`) as an absolute path, normal.
fn absolute(path: &str) -> Option<PathBuf> {
    let path = Path::new("/").join(path);
    path.components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        .then_some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{same_file, shown, to_disk, without_query};

    /// A project: `app/src/cart.ts`, `app/src/index.ts`,
    /// `app/src/components/index.ts` and `app/index.ts`, and a file
    /// outside it, `lib/util.ts`.
    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        for file in [
            "app/src/cart.ts",
            "app/src/index.ts",
            "app/src/components/index.ts",
            "app/index.ts",
            "lib/util.ts",
        ] {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        (dir, root)
    }

    fn disk(source: &str, root: &Path) -> Option<String> {
        to_disk(source, Some(root)).map(|file| shown(&file, Some(root)))
    }

    #[test]
    fn absolute_paths_are_used_as_they_are() {
        let (dir, root) = project();
        let cart = root.join("src/cart.ts");
        assert_eq!(to_disk(cart.to_str().unwrap(), Some(&root)), Some(cart));
        // Outside the root: printed absolute.
        let util = dir.path().join("lib/util.ts");
        let found = to_disk(util.to_str().unwrap(), Some(&root)).unwrap();
        assert_eq!(found, util);
        assert_eq!(shown(&found, Some(&root)), util.to_string_lossy());
        // Without a root too.
        assert_eq!(to_disk(util.to_str().unwrap(), None), Some(util));
    }

    #[test]
    fn vite_fs_paths() {
        let (dir, root) = project();
        let util = dir.path().join("lib/util.ts");
        let url = format!("http://localhost:5173/@fs{}?v=1", util.display());
        assert_eq!(to_disk(&url, Some(&root)), Some(util));
    }

    #[test]
    fn urls_lose_their_scheme_host_and_query() {
        let (_dir, root) = project();
        assert_eq!(
            disk("http://localhost:5173/src/cart.ts?t=123", &root).as_deref(),
            Some("src/cart.ts")
        );
        assert_eq!(
            disk("http://localhost:5173/src/cart.ts#x", &root).as_deref(),
            Some("src/cart.ts")
        );
        assert_eq!(
            disk("webpack://app/./src/cart.ts", &root).as_deref(),
            Some("src/cart.ts")
        );
        assert_eq!(
            disk("webpack:///src/components/index.ts", &root).as_deref(),
            Some("src/components/index.ts")
        );
    }

    #[test]
    fn the_longest_suffix_wins() {
        let (_dir, root) = project();
        // Segments are dropped until the rest is under the root.
        assert_eq!(
            disk("http://x/assets/app/src/components/index.ts", &root).as_deref(),
            Some("src/components/index.ts")
        );
        // A build made elsewhere: its absolute path does not exist here.
        assert_eq!(
            disk("/build/checkout/app/src/cart.ts", &root).as_deref(),
            Some("src/cart.ts")
        );
        // `app/index.ts` is `<root>/index.ts`, not `<root>/app/index.ts`.
        assert_eq!(
            disk("http://x/app/src/index.ts", &root).as_deref(),
            Some("src/index.ts")
        );
    }

    #[test]
    fn a_file_name_alone_is_no_match() {
        let (_dir, root) = project();
        assert_eq!(disk("http://x/index.ts", &root), None);
        assert_eq!(disk("http://x/elsewhere/cart.ts", &root), None);
        assert_eq!(disk("http://x/src/missing.ts", &root), None);
        assert_eq!(disk("http://x/../../etc/passwd", &root), None);
        assert_eq!(disk("not a url", &root), None);
        assert_eq!(disk("data:text/javascript,1", &root), None);
        // A relative path (Svelte's) is a rest as it is.
        assert_eq!(disk("src/cart.ts", &root).as_deref(), Some("src/cart.ts"));
        assert_eq!(disk("cart.ts", &root), None);
        assert_eq!(to_disk("http://x/src/cart.ts", None), None);
    }

    #[test]
    fn places_name_sources() {
        let (_dir, root) = project();
        let source = "http://localhost:1/apps-src/plain/src/cart.ts?t=5";
        let same = |place: &str| same_file(source, place, None, Some(&root));
        assert!(same("src/cart.ts"));
        assert!(same("cart.ts"));
        assert!(same("plain/src/cart.ts"));
        assert!(same("/home/me/apps-src/plain/src/cart.ts"));
        assert!(same("http://localhost:5173/src/cart.ts"));
        assert!(!same("lib/cart.ts"));
        assert!(!same("src/cart.tsx"));
        assert!(!same(""));
        // A source that is a file name alone is not the end of a path.
        assert!(!same_file(
            "http://x/cart.ts",
            "src/cart.ts",
            None,
            Some(&root)
        ));
        assert!(same_file("http://x/cart.ts", "cart.ts", None, Some(&root)));
        // The file the place names, found on disk from the source.
        let cart = root.join("src/cart.ts");
        assert!(same_file(
            "webpack://app/./src/cart.ts",
            "../app/src/cart.ts",
            Some(&cart),
            Some(&root)
        ));
        assert_eq!(
            without_query(source),
            "http://localhost:1/apps-src/plain/src/cart.ts"
        );
    }
}
