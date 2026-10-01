//! Globs of mock files: path and URL globs (`/api/products/:id`,
//! `/api/**`, `https://*.example.com/**`) and value globs (`*@example.com`).

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use percent_encoding::percent_decode_str;
use regex::Regex;

/// A path or URL glob, compiled to a regular expression.
///
/// - `*` matches any characters but `/`; `**` any characters. A `/**`
///   segment also matches nothing, so `/api/**` matches `/api` too.
/// - `:name` right after a `/` captures one segment (or the part of it
///   before the literal text that follows, as in `:id.json`) as the
///   parameter `name`.
/// - A trailing `/` is ignored on both sides: `/api/items/` and
///   `/api/items` are one path.
#[derive(Debug, Clone)]
pub(crate) struct PathGlob {
    text: String,
    regex: Regex,
}

impl PathGlob {
    /// A glob on URL paths; it must start with `/`.
    pub(crate) fn path(text: &str) -> Result<Self, String> {
        if !text.starts_with('/') {
            return Err(format!(
                "`{text}` is no path; a path starts with / (or use `url` for a full URL)"
            ));
        }
        Self::compile(text)
    }

    /// A glob on full URLs (without their query), e.g.
    /// `https://api.example.com/v1/**`.
    pub(crate) fn url(text: &str) -> Result<Self, String> {
        if !text.contains("://") {
            return Err(format!(
                "`{text}` is no full URL; start it with a scheme (https://…), or use `path`"
            ));
        }
        Self::compile(text)
    }

    fn compile(text: &str) -> Result<Self, String> {
        if text.contains('?') || text.contains('#') {
            return Err(format!(
                "`{text}` has a query or fragment; match query parameters with `query`"
            ));
        }
        let trimmed = if text.len() > 1 && !text.ends_with("//") {
            text.strip_suffix('/').unwrap_or(text)
        } else {
            text
        };
        let chars: Vec<char> = trimmed.chars().collect();
        let mut re = String::from("^");
        let mut names: Vec<String> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let at_segment = i == 0 || chars[i - 1] == '/';
            if c == '/' && chars.get(i + 1) == Some(&'*') && chars.get(i + 2) == Some(&'*') {
                let end = i + 3;
                if end == chars.len() {
                    re.push_str("(?:/.*)?");
                    i = end;
                    continue;
                }
                if chars[end] == '/' {
                    // `/**/`: any segments, or none.
                    re.push_str("(?:/.*)?");
                    i = end;
                    continue;
                }
            }
            if c == '*' {
                if chars.get(i + 1) == Some(&'*') {
                    re.push_str(".*");
                    i += 2;
                } else {
                    re.push_str("[^/]*");
                    i += 1;
                }
                continue;
            }
            if c == ':' && at_segment && i > 0 {
                let name: String = chars[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .collect();
                if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
                    return Err(format!(
                        "`{text}`: a parameter needs a name, e.g. /api/items/:id"
                    ));
                }
                if names.contains(&name) {
                    return Err(format!("`{text}`: parameter :{name} appears twice"));
                }
                let _ = write!(re, "(?P<{name}>[^/]+?)");
                i += 1 + name.len();
                names.push(name);
                continue;
            }
            re.push_str(&regex::escape(&c.to_string()));
            i += 1;
        }
        re.push_str("/?$");
        let regex = Regex::new(&re).map_err(|e| format!("`{text}`: {e}"))?;
        Ok(Self {
            text: text.to_owned(),
            regex,
        })
    }

    /// The names of its parameters (and regex groups), in order.
    pub(crate) fn params(&self) -> Vec<&str> {
        self.regex.capture_names().flatten().collect()
    }

    /// The parameters when `subject` matches, percent-decoded.
    pub(crate) fn captures(&self, subject: &str) -> Option<BTreeMap<String, String>> {
        let caps = self.regex.captures(subject)?;
        Some(
            self.regex
                .capture_names()
                .flatten()
                .filter_map(|name| {
                    let value = caps.name(name)?.as_str();
                    Some((name.to_owned(), decode(value)))
                })
                .collect(),
        )
    }

    pub(crate) fn matches(&self, subject: &str) -> bool {
        self.regex.is_match(subject)
    }

    /// The glob as written.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Whether it is a path glob (else a full-URL glob).
    pub(crate) fn is_path(&self) -> bool {
        self.text.starts_with('/')
    }
}

impl fmt::Display for PathGlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Percent-decodes `text` (lossily).
pub(crate) fn decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// A glob on a value: `*` matches any characters (newlines too); anything
/// else matches itself.
#[derive(Debug, Clone)]
pub(crate) struct Glob {
    text: String,
    regex: Option<Regex>,
}

impl Glob {
    pub(crate) fn new(text: &str) -> Self {
        let regex = text
            .contains('*')
            .then(|| {
                let parts: Vec<String> = text.split('*').map(regex::escape).collect();
                Regex::new(&format!("(?s)^{}$", parts.join(".*"))).ok()
            })
            .flatten();
        Self {
            text: text.to_owned(),
            regex,
        }
    }

    pub(crate) fn matches(&self, value: &str) -> bool {
        match &self.regex {
            Some(regex) => regex.is_match(value),
            None => self.text == value,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(glob: &str, path: &str) -> Option<Vec<(String, String)>> {
        PathGlob::path(glob)
            .unwrap()
            .captures(path)
            .map(|p| p.into_iter().collect())
    }

    #[test]
    fn path_globs() {
        let yes = [
            ("/api/products", "/api/products"),
            ("/api/products", "/api/products/"),
            ("/api/products/", "/api/products"),
            ("/api/*", "/api/items"),
            ("/api/**", "/api"),
            ("/api/**", "/api/a/b/c"),
            ("/api/**/items", "/api/items"),
            ("/api/**/items", "/api/v1/shop/items"),
            ("/**", "/"),
            ("/**", "/anything/at/all"),
            ("/", "/"),
            ("/files/*.json", "/files/a.json"),
        ];
        for (glob, path) in yes {
            assert!(PathGlob::path(glob).unwrap().matches(path), "{glob} {path}");
        }
        let no = [
            ("/api/products", "/api/products/1"),
            ("/api/*", "/api/a/b"),
            ("/api/*", "/api"),
            ("/api/**", "/apis"),
            ("/api/**/items", "/api/itemsx"),
            ("/", "/a"),
            ("/files/*.json", "/files/a.txt"),
        ];
        for (glob, path) in no {
            assert!(
                !PathGlob::path(glob).unwrap().matches(path),
                "{glob} {path}"
            );
        }
    }

    #[test]
    fn parameters() {
        assert_eq!(
            params("/api/products/:id", "/api/products/42"),
            Some(vec![("id".into(), "42".into())])
        );
        assert_eq!(
            params("/api/:kind/:id.json", "/api/shoes/a%20b.json"),
            Some(vec![
                ("id".into(), "a b".into()),
                ("kind".into(), "shoes".into())
            ])
        );
        assert_eq!(params("/api/products/:id", "/api/products/1/x"), None);
        assert_eq!(
            params("/users/:email", "/users/a@b.com"),
            Some(vec![("email".into(), "a@b.com".into())])
        );
        assert!(PathGlob::path("/a/:id/:id").is_err());
        assert!(PathGlob::path("/a/:").is_err());
        assert!(PathGlob::path("api").is_err());
        assert!(PathGlob::path("/a?x=1").is_err());
    }

    #[test]
    fn url_globs() {
        let glob = PathGlob::url("https://*.example.com/v1/**").unwrap();
        assert!(glob.matches("https://api.example.com/v1/items"));
        assert!(!glob.matches("https://api.example.org/v1/items"));
        let port = PathGlob::url("http://localhost:5173/api/:id").unwrap();
        assert_eq!(
            port.captures("http://localhost:5173/api/7")
                .unwrap()
                .get("id")
                .map(String::as_str),
            Some("7")
        );
        assert!(PathGlob::url("/api").is_err());
    }

    #[test]
    fn value_globs() {
        assert!(Glob::new("*@example.com").matches("a@example.com"));
        assert!(!Glob::new("*@example.com").matches("a@example.org"));
        assert!(Glob::new("*").matches(""));
        assert!(Glob::new("a*c").matches("a\nb\nc"));
        assert!(Glob::new("a.c").matches("a.c"));
        assert!(!Glob::new("a.c").matches("abc"));
    }
}
