//! Source maps for console stack frames (plan 06, milestone 6.1).
//!
//! `console --sourcemaps` (and any command on a localhost page) resolves
//! each frame it shows to the original source: it loads the script,
//! finds its map (the `SourceMap` header or the last
//! `//# sourceMappingURL=` comment, inline `data:` maps included), loads
//! and parses the map with the `sourcemap` crate, and looks the frame up.
//! Scripts and maps are loaded through Chrome
//! (`Network.loadNetworkResource`, as DevTools does), so cookies and HTTP
//! authentication apply and no request leaves from the daemon. Each
//! script's outcome is cached per URL, a missing map too.
//!
//! Logpoints (plan 15, milestone 15.4) look the other way: the sources a
//! script's map lists ([`Mapped::sources`]) and where an original line
//! runs in the script ([`Mapped::generated`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use cdp::Session;
use cdp::domains::{io, network};
use protocol::schema::{SourcePosition, StackFrame};
use sourcemap::DecodedMap;
use tokio::time::Instant;
use url::Url;

/// The largest script or map loaded.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// How many scripts the cache remembers before it starts over.
const CACHE_LEN: usize = 256;

/// How far from a script's end its `sourceMappingURL` comment may be.
const TAIL_BYTES: usize = 4096;

/// The source maps of the scripts seen so far, by script URL (`None`: the
/// script has none, or it could not be loaded).
#[derive(Debug, Default)]
pub(crate) struct SourceMaps {
    cache: Mutex<HashMap<String, Option<Arc<Mapped>>>>,
}

/// A script's parsed map and where it came from.
pub(crate) struct Mapped {
    /// Never an index map: those are flattened when loaded.
    map: DecodedMap,
    /// What relative sources are resolved against: the map's URL (the
    /// script's, for an inline map).
    base: Url,
}

impl std::fmt::Debug for Mapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapped")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl SourceMaps {
    /// Sets `original` on each of `frames` a source map resolves, loading
    /// scripts and maps through frame `frame_id` of `session` until
    /// `deadline`. URLs in `documents` (pages, whose inline scripts have
    /// no map of their own) are not loaded.
    pub(crate) async fn resolve(
        &self,
        session: &Session,
        frame_id: &str,
        documents: &HashSet<String>,
        frames: &mut [&mut StackFrame],
        deadline: Instant,
    ) {
        let mut maps: HashMap<String, Option<Arc<Mapped>>> = HashMap::new();
        for frame in frames.iter() {
            let url = &frame.url;
            if maps.contains_key(url) || documents.contains(url) || !is_http(url) {
                continue;
            }
            let cached = self.lock().get(url).cloned();
            let mapped = if let Some(mapped) = cached {
                mapped
            } else {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let Ok(loaded) =
                    tokio::time::timeout(left, load(&session.with_timeout(left), frame_id, url))
                        .await
                else {
                    // Out of time: try again next time.
                    break;
                };
                let loaded = loaded.map(Arc::new);
                let mut cache = self.lock();
                if cache.len() >= CACHE_LEN {
                    cache.clear();
                }
                cache.insert(url.clone(), loaded.clone());
                loaded
            };
            maps.insert(url.clone(), mapped);
        }
        for frame in frames.iter_mut() {
            if let Some(Some(mapped)) = maps.get(&frame.url) {
                frame.original = mapped.lookup(frame.line, frame.column);
            }
        }
    }

    /// The map of script `url`, whose `Debugger.scriptParsed` named
    /// `source_map_url` (relative to the script, or a `data:` URL), loaded
    /// through frame `frame_id` (none in a worker) until `deadline`;
    /// `None` if it has none or it cannot be loaded. Cached as
    /// [`SourceMaps::resolve`]'s are.
    pub(crate) async fn of_script(
        &self,
        session: &Session,
        frame_id: Option<&str>,
        url: &str,
        source_map_url: &str,
        deadline: Instant,
    ) -> Option<Arc<Mapped>> {
        if let Some(cached) = self.lock().get(url).cloned() {
            return cached;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || source_map_url.is_empty() {
            return None;
        }
        let script_url = Url::parse(url).ok()?;
        let session = session.with_timeout(left);
        let loaded = tokio::time::timeout(
            left,
            load_map(&session, frame_id, &script_url, source_map_url),
        )
        .await
        // Out of time: try again next time.
        .ok()?
        .map(Arc::new);
        let mut cache = self.lock();
        if cache.len() >= CACHE_LEN {
            cache.clear();
        }
        cache.insert(url.to_owned(), loaded.clone());
        loaded
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<Arc<Mapped>>>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where an original line runs in the generated script (see
/// [`Mapped::generated`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Generated {
    /// 0-based, as `Debugger.setBreakpointByUrl` takes them.
    pub(crate) line: u32,
    pub(crate) column: u32,
    /// The original line (1-based) it is the code of: the one asked for,
    /// or the next one with code.
    pub(crate) original_line: u32,
}

/// How many lines past the one asked for [`Mapped::generated`] looks for
/// code.
const LINES_AHEAD: u32 = 100;

impl Mapped {
    /// The sources the map lists, as URLs (resolved against the map's).
    pub(crate) fn sources(&self) -> Vec<String> {
        let Some(map) = self.regular() else {
            return Vec::new();
        };
        let mut sources: Vec<String> = map.sources().map(|s| self.source_url(s)).collect();
        sources.dedup();
        sources
    }

    /// Where 1-based `line` of `source` (a URL [`Mapped::sources`] gave)
    /// runs: the generated place of its first mapped column (from
    /// `column` on, if given), or of the next line with code (at most
    /// [`LINES_AHEAD`] lines on).
    pub(crate) fn generated(
        &self,
        source: &str,
        line: u32,
        column: Option<u32>,
    ) -> Option<Generated> {
        let map = self.regular()?;
        let wanted = line.checked_sub(1)?;
        let from = (wanted, column.map_or(0, |c| c.saturating_sub(1)));
        let ids: Vec<u32> = (0..map.get_source_count())
            .filter(|&id| {
                map.get_source(id)
                    .is_some_and(|s| self.source_url(s) == source)
            })
            .collect();
        // The best token so far: its original line and column, then its
        // generated place.
        let mut best: Option<((u32, u32), (u32, u32))> = None;
        for token in map.tokens() {
            if !ids.contains(&token.get_src_id()) {
                continue;
            }
            let src = token.get_src();
            if src < from || src.0 > wanted.saturating_add(LINES_AHEAD) {
                continue;
            }
            let candidate = (src, token.get_dst());
            if best.is_none_or(|b| candidate < b) {
                best = Some(candidate);
            }
        }
        let ((src_line, _), (line, column)) = best?;
        Some(Generated {
            line,
            column,
            original_line: src_line.checked_add(1)?,
        })
    }

    /// The map as a regular one (an index map was flattened when loaded).
    fn regular(&self) -> Option<&sourcemap::SourceMap> {
        match &self.map {
            DecodedMap::Regular(map) => Some(map),
            DecodedMap::Hermes(map) => Some(map),
            DecodedMap::Index(_) => None,
        }
    }

    /// A source of the map as a URL.
    fn source_url(&self, source: &str) -> String {
        match Url::parse(source) {
            Ok(url) => url.to_string(),
            Err(_) => self
                .base
                .join(source)
                .map_or_else(|_| source.to_owned(), |u| u.to_string()),
        }
    }

    /// The original place of 1-based `line` and `column` of the script.
    pub(crate) fn lookup(&self, line: u32, column: u32) -> Option<SourcePosition> {
        let (line, column) = (line.checked_sub(1)?, column.checked_sub(1)?);
        let token = self.map.lookup_token(line, column)?;
        // The nearest mapping before the place, but on its line only: one
        // on an earlier line belongs to other code.
        if token.get_dst_line() != line {
            return None;
        }
        let source = self.source_url(token.get_source()?);
        Some(SourcePosition {
            source,
            line: token.get_src_line().checked_add(1)?,
            column: token.get_src_col().checked_add(1)?,
            name: token.get_name().map(str::to_owned),
            file: None,
        })
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Loads script `url` and its source map; `None` if either is missing.
async fn load(session: &Session, frame_id: &str, url: &str) -> Option<Mapped> {
    let script_url = Url::parse(url).ok()?;
    let (script, headers) = fetch(session, Some(frame_id), url).await?;
    let header = headers.iter().find_map(|(name, value)| {
        let name = name.to_ascii_lowercase();
        (name == "sourcemap" || name == "x-sourcemap")
            .then(|| value.as_str().map(str::to_owned))
            .flatten()
    });
    let reference = header.or_else(|| mapping_url(&script))?;
    load_map(session, Some(frame_id), &script_url, &reference).await
}

/// Loads the source map `reference` of the script at `script_url` names
/// (a URL relative to it, or a `data:` URL).
async fn load_map(
    session: &Session,
    frame_id: Option<&str>,
    script_url: &Url,
    reference: &str,
) -> Option<Mapped> {
    let (map, base) = if reference.starts_with("data:") {
        let map = sourcemap::decode_data_url(reference)
            .ok()
            .or_else(|| decode_plain_data_url(reference))?;
        (map, script_url.clone())
    } else {
        let map_url = script_url.join(reference).ok()?;
        if !is_http(map_url.as_str()) {
            return None;
        }
        let (bytes, _) = fetch(session, frame_id, map_url.as_str()).await?;
        (sourcemap::decode_slice(&bytes).ok()?, map_url)
    };
    // An index map's sections, as one map, so it can be looked up both
    // ways.
    let map = match map {
        DecodedMap::Index(index) => DecodedMap::Regular(index.flatten().ok()?),
        map => map,
    };
    Some(Mapped { map, base })
}

/// A `data:application/json,…` map that is URL-encoded, not base64 (the
/// crate reads only base64).
fn decode_plain_data_url(url: &str) -> Option<DecodedMap> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    if meta.ends_with(";base64") {
        return None;
    }
    let decoded = percent_decode(data);
    sourcemap::decode_slice(&decoded).ok()
}

fn percent_decode(text: &str) -> Vec<u8> {
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
    out
}

/// The last `//# sourceMappingURL=…` (or the old `//@`) near the end of a
/// script.
fn mapping_url(script: &[u8]) -> Option<String> {
    let start = script.len().saturating_sub(TAIL_BYTES);
    let tail = String::from_utf8_lossy(&script[start..]);
    tail.lines().rev().find_map(|line| {
        let line = line.trim();
        let rest = line
            .strip_prefix("//# sourceMappingURL=")
            .or_else(|| line.strip_prefix("//@ sourceMappingURL="))?;
        let url = rest.split_whitespace().next()?;
        (!url.is_empty()).then(|| url.to_owned())
    })
}

/// Loads `url` as frame `frame_id` would (its cookies; a worker has no
/// frame), through Chrome: the body and the response headers; `None` if
/// it failed or is too big.
async fn fetch(
    session: &Session,
    frame_id: Option<&str>,
    url: &str,
) -> Option<(Vec<u8>, network::Headers)> {
    let mut params = network::LoadNetworkResourceParams::new(
        url.to_owned(),
        network::LoadNetworkResourceOptions {
            disable_cache: false,
            include_credentials: true,
        },
    );
    params.frame_id = frame_id.map(str::to_owned);
    let resource = session.call(params).await.ok()?.resource;
    let failed = !resource.success || resource.http_status_code.is_some_and(|s| s >= 400.0);
    let handle = resource.stream?;
    let body = if failed {
        None
    } else {
        read(session, &handle).await
    };
    let _ = session.call(io::CloseParams::new(handle)).await;
    Some((body?, resource.headers.unwrap_or_default()))
}

/// Reads a stream to its end; `None` past [`MAX_BYTES`] or on an error.
async fn read(session: &Session, handle: &str) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let chunk = session
            .call(io::ReadParams::new(handle.to_owned()))
            .await
            .ok()?;
        if chunk.base64_encoded == Some(true) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk.data.as_bytes())
                .ok()?;
            body.extend_from_slice(&bytes);
        } else {
            body.extend_from_slice(chunk.data.as_bytes());
        }
        if body.len() > MAX_BYTES {
            return None;
        }
        if chunk.eof {
            return Some(body);
        }
    }
}

/// Whether a page at `url` is on this machine (`localhost`, `*.localhost`,
/// a loopback address): its source maps are resolved without
/// `--sourcemaps`.
pub(crate) fn is_local(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// How long resolving may take at most, whatever the command's timeout.
pub(crate) const RESOLVE_LIMIT: Duration = Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use super::{Mapped, is_local, mapping_url};
    use url::Url;

    #[test]
    fn finds_the_mapping_url() {
        let script = b"console.log(1);\n//# sourceMappingURL=app.js.map\n";
        assert_eq!(mapping_url(script).as_deref(), Some("app.js.map"));
        assert_eq!(
            mapping_url(b"//@ sourceMappingURL=old.map"),
            Some("old.map".into())
        );
        assert_eq!(mapping_url(b"var x = 1;"), None);
    }

    #[test]
    fn localhost_pages() {
        assert!(is_local("http://localhost:5173/"));
        assert!(is_local("http://app.localhost/"));
        assert!(is_local("http://127.0.0.1:8000/x"));
        assert!(is_local("http://[::1]:3000/"));
        assert!(!is_local("https://example.com/"));
        assert!(!is_local("about:blank"));
    }

    #[test]
    fn looks_frames_up() {
        // `function go() {\n  boom();\n}` in src/app.ts, bundled to one
        // line: `function go(){boom()}`.
        let map = sourcemap::decode_slice(
            br#"{"version":3,"sources":["../src/app.ts"],"names":["go","boom"],"mappings":"AAAA,SAASA,KACPC"}"#,
        )
        .unwrap();
        let mapped = Mapped {
            map,
            base: Url::parse("http://localhost:1/dist/app.js.map").unwrap(),
        };
        let place = mapped.lookup(1, 15).unwrap();
        assert_eq!(place.source, "http://localhost:1/src/app.ts");
        assert_eq!((place.line, place.column), (2, 3));
        assert_eq!(place.name.as_deref(), Some("boom"));
        assert!(mapped.lookup(2, 1).is_none());
    }

    #[test]
    fn finds_where_original_lines_run() {
        // src/cart.ts:
        //   1 // A comment.
        //   2 export function add(a, b) {
        //   3   // Adds.
        //   4   return a + b;
        //   5 }
        // bundled to two lines: `function add(a, b) {` and
        // `  return a + b; }`, with a map that has line 2 at 0 and 9,
        // and line 4 at column 2 (at 2 and 9 of the second line).
        let map = sourcemap::decode_slice(
            br#"{"version":3,"sources":["../src/cart.ts","../src/other.ts"],"names":[],"mappings":"AACA,SAAA;EAEE,OAAA"}"#,
        )
        .unwrap();
        let mapped = Mapped {
            map,
            base: Url::parse("http://localhost:1/dist/app.js.map").unwrap(),
        };
        assert_eq!(
            mapped.sources(),
            [
                "http://localhost:1/src/cart.ts",
                "http://localhost:1/src/other.ts"
            ]
        );
        let cart = "http://localhost:1/src/cart.ts";
        let at = |line| {
            mapped
                .generated(cart, line, None)
                .map(|g| (g.line, g.column, g.original_line))
        };
        // The line's first mapped column.
        assert_eq!(at(2), Some((0, 0, 2)));
        assert_eq!(at(4), Some((1, 2, 4)));
        // A comment moves to the next line with code.
        assert_eq!(at(1), Some((0, 0, 2)));
        assert_eq!(at(3), Some((1, 2, 4)));
        // Past the code, and in a source with no mappings: nothing.
        assert_eq!(at(5), None);
        assert_eq!(
            mapped.generated("http://localhost:1/src/other.ts", 1, None),
            None
        );
        // A column: the first mapping from it on.
        let g = mapped.generated(cart, 2, Some(1)).unwrap();
        assert_eq!((g.line, g.column), (0, 0));
        // Both ways agree.
        let back = mapped.lookup(2, 3).unwrap();
        assert_eq!((back.source.as_str(), back.line), (cart, 4));
    }
}
