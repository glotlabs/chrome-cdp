//! Just enough of AppArmor's profile language to answer one question: does
//! a profile in `/etc/apparmor.d` attach to this binary and grant it
//! `userns`? Ubuntu 23.10+ blocks Chrome's sandbox without one.
//!
//! It reads profile headers (`profile NAME ATTACHMENT flags=(…) {` or
//! `/attachment {`), the `userns` rule in their bodies, and `@{VAR} = …`
//! definitions. Globs support `*`, `**`, `?`, `[…]` and `{a,b}`. `@{HOME}`
//! and `@{HOMEDIRS}` get their default values (plus this user's home);
//! variables defined elsewhere match anything.

use std::collections::BTreeMap;
use std::path::Path;

/// Brace expansions tried per attachment, at most.
const MAX_EXPANSIONS: usize = 4096;

/// One profile's attachment and whether it grants `userns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Profile {
    pub(crate) name: String,
    /// The path glob it attaches to; `None` for a profile attached by name
    /// only.
    pub(crate) attach: Option<String>,
    pub(crate) userns: bool,
    /// The file's own `@{VAR}` definitions.
    vars: BTreeMap<String, Vec<String>>,
}

/// Parses the profiles in one policy file.
pub(crate) fn parse(text: &str) -> Vec<Profile> {
    let mut vars: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut policy = String::new();
    for line in text.lines() {
        let line = strip_comment(line);
        if let Some((name, values, append)) = var_definition(line) {
            let entry = vars.entry(name).or_default();
            if !append {
                entry.clear();
            }
            entry.extend(values);
        } else {
            policy.push_str(line);
            policy.push('\n');
        }
    }

    let mut profiles = Vec::new();
    let mut header: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut body_userns = false;
    let mut current: Option<(String, Option<String>)> = None;
    for token in tokens(&policy) {
        let balance = brace_balance(&token);
        if depth == 0 {
            if balance > 0 && token.ends_with('{') {
                let head = token.trim_end_matches('{');
                if !head.is_empty() {
                    header.push(head.to_owned());
                }
                current = profile_header(&header);
                header.clear();
                depth = 1;
                body_userns = false;
            } else if token.ends_with(',') {
                header.clear();
            } else {
                header.push(token);
            }
            continue;
        }
        if balance > 0 && token.ends_with('{') {
            depth += 1;
        } else if token == "}" || (balance < 0 && token.ends_with('}')) {
            depth -= 1;
            if depth == 0
                && let Some((name, attach)) = current.take()
            {
                profiles.push(Profile {
                    name,
                    attach,
                    userns: body_userns,
                    vars: vars.clone(),
                });
            }
        } else if depth == 1 && matches!(token.as_str(), "userns," | "userns" | "userns create,") {
            body_userns = true;
        }
    }
    profiles
}

/// `NAME`, `ATTACHMENT` from the tokens before a block's `{`.
fn profile_header(header: &[String]) -> Option<(String, Option<String>)> {
    let is_path = |t: &str| {
        let t = t.trim_matches('"');
        t.starts_with('/') || t.starts_with("@{")
    };
    if let Some(at) = header.iter().rposition(|t| t == "profile") {
        let name = header.get(at + 1)?.trim_matches('"').to_owned();
        let attach = header
            .get(at + 2)
            .filter(|t| is_path(t))
            .map(|t| t.trim_matches('"').to_owned());
        // `profile /path {` is named by its path.
        let attach = attach.or_else(|| is_path(&name).then(|| name.clone()));
        return Some((name, attach));
    }
    let path = header.iter().rev().find(|t| is_path(t))?;
    let path = path.trim_matches('"').to_owned();
    Some((path.clone(), Some(path)))
}

/// Removes a `#` comment (outside quotes).
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `@{name} = a b` or `@{name} += a b`.
fn var_definition(line: &str) -> Option<(String, Vec<String>, bool)> {
    let line = line.trim();
    let rest = line.strip_prefix("@{")?;
    let (name, rest) = rest.split_once('}')?;
    let rest = rest.trim_start();
    let (append, values) = if let Some(v) = rest.strip_prefix("+=") {
        (true, v)
    } else {
        (false, rest.strip_prefix('=')?)
    };
    let values = tokens(values)
        .into_iter()
        .map(|v| v.trim_matches('"').to_owned())
        .collect();
    Some((name.to_owned(), values, append))
}

/// Splits on whitespace, keeping `"quoted strings"` whole.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
            cur.push(c);
        } else if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `{` minus `}` in a token.
fn brace_balance(token: &str) -> i32 {
    token.chars().fold(0, |n, c| match c {
        '{' => n + 1,
        '}' => n - 1,
        _ => n,
    })
}

impl Profile {
    /// Whether this profile attaches to `binary` (an absolute, resolved
    /// path). `home` is this user's home dir.
    pub(crate) fn attaches_to(&self, binary: &Path, home: Option<&Path>) -> bool {
        let Some(attach) = &self.attach else {
            return false;
        };
        let Some(path) = binary.to_str() else {
            return false;
        };
        let mut vars = self.vars.clone();
        let mut homes = vec!["@{HOMEDIRS}/*/".to_owned(), "/root/".to_owned()];
        if let Some(home) = home.and_then(Path::to_str) {
            homes.push(format!("{home}/"));
        }
        vars.entry("HOME".into()).or_insert(homes);
        vars.entry("HOMEDIRS".into())
            .or_insert_with(|| vec!["/home/".into()]);
        let Some(pattern) = substitute(attach, &vars, 0) else {
            return false;
        };
        let mut expanded = Vec::new();
        expand_braces(&pattern, &mut expanded);
        expanded
            .iter()
            .any(|p| glob_match(collapse_slashes(p).as_bytes(), path.as_bytes()))
    }
}

/// Replaces `@{VAR}` with `{value1,value2}` (unknown variables match
/// anything). `None` if definitions nest too deep.
fn substitute(pattern: &str, vars: &BTreeMap<String, Vec<String>>, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(at) = rest.find("@{") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        match vars.get(name) {
            Some(values) if !values.is_empty() => {
                let values: Option<Vec<String>> = values
                    .iter()
                    .map(|v| substitute(v, vars, depth + 1))
                    .collect();
                out.push('{');
                out.push_str(&values?.join(","));
                out.push('}');
            }
            _ => out.push_str("**"),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Expands the first top-level `{a,b}` and recurses, up to
/// [`MAX_EXPANSIONS`] results.
fn expand_braces(pattern: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_EXPANSIONS {
        return;
    }
    let bytes = pattern.as_bytes();
    let Some(open) = bytes.iter().position(|&b| b == b'{') else {
        out.push(pattern.to_owned());
        return;
    };
    let mut depth = 0;
    let mut close = None;
    let mut commas = Vec::new();
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            b',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    let Some(close) = close else {
        out.push(pattern.to_owned());
        return;
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let mut start = open + 1;
    for end in commas.into_iter().chain([close]) {
        expand_braces(&format!("{head}{}{tail}", &pattern[start..end]), out);
        start = end + 1;
    }
}

fn collapse_slashes(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for c in p.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    out
}

/// AppArmor glob matching: `*` within one path segment, `**` across
/// segments, `?` one non-`/` char, `[…]` a class.
fn glob_match(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => {
            (0..=s.len()).any(|i| glob_match(&p[2..], &s[i..]))
        }
        Some(b'*') => {
            for i in 0..=s.len() {
                if glob_match(&p[1..], &s[i..]) {
                    return true;
                }
                if s.get(i) == Some(&b'/') {
                    break;
                }
            }
            false
        }
        Some(b'?') => s.first().is_some_and(|&c| c != b'/') && glob_match(&p[1..], &s[1..]),
        Some(b'[') => {
            let Some(end) = p.iter().skip(1).position(|&b| b == b']').map(|e| e + 1) else {
                return s.first() == Some(&b'[') && glob_match(&p[1..], &s[1..]);
            };
            let Some(&c) = s.first() else {
                return false;
            };
            let class = &p[1..end];
            let (negate, class) = match class.first() {
                Some(b'^' | b'!') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    hit |= (class[i]..=class[i + 2]).contains(&c);
                    i += 3;
                } else {
                    hit |= class[i] == c;
                    i += 1;
                }
            }
            hit != negate && glob_match(&p[end + 1..], &s[1..])
        }
        Some(b'\\') if p.len() > 1 => s.first() == Some(&p[1]) && glob_match(&p[2..], &s[1..]),
        Some(&c) => s.first() == Some(&c) && glob_match(&p[1..], &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{glob_match, parse};

    const UBUNTU_CHROME: &str = "\
# This profile allows everything and only exists to give the
# application a name instead of having the label \"unconfined\"

abi <abi/4.0>,
include <tunables/global>

profile chrome /opt/google/chrome/chrome flags=(unconfined) {
  userns,

  # Site-specific additions and overrides. See local/README for details.
  include if exists <local/chrome>
}
";

    const UBUNTU_CHROMIUM: &str = "\
abi <abi/5.0>,
include <tunables/global>

@{chromium} = {,ungoogled-}chromium{,-browser}

profile chromium /usr/lib/@{chromium}/@{chromium} flags=(unconfined) {
  userns,
  @{exec_path} mr,
  include if exists <local/chromium>
}
";

    /// The one on the maintainer's machine, header over three lines.
    const CHROME_CDP: &str = "\
abi <abi/5.0>,
include <tunables/global>

profile chrome-cdp
@{HOME}/.cache/chrome-cdp/browsers/*/{chrome-linux64/chrome,chrome-headless-shell-linux64/chrome-headless-shell}
flags=(unconfined) {
  userns,
  include if exists <local/chrome-cdp>
}
";

    fn attaches(text: &str, path: &str) -> bool {
        parse(text)
            .iter()
            .any(|p| p.userns && p.attaches_to(Path::new(path), Some(Path::new("/srv/u"))))
    }

    #[test]
    fn ubuntu_profiles() {
        let p = parse(UBUNTU_CHROME);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "chrome");
        assert_eq!(p[0].attach.as_deref(), Some("/opt/google/chrome/chrome"));
        assert!(p[0].userns);
        assert!(attaches(UBUNTU_CHROME, "/opt/google/chrome/chrome"));
        assert!(!attaches(UBUNTU_CHROME, "/opt/google/chrome/chrome2"));

        assert!(attaches(UBUNTU_CHROMIUM, "/usr/lib/chromium/chromium"));
        assert!(attaches(
            UBUNTU_CHROMIUM,
            "/usr/lib/ungoogled-chromium-browser/ungoogled-chromium-browser"
        ));
        assert!(!attaches(UBUNTU_CHROMIUM, "/usr/lib/chromium/chrome"));
    }

    #[test]
    fn multi_line_headers_and_home() {
        let cft = "/.cache/chrome-cdp/browsers/stable-154.0.8037.57-linux64/chrome-linux64/chrome";
        assert!(attaches(CHROME_CDP, &format!("/home/petter{cft}")));
        assert!(attaches(CHROME_CDP, &format!("/root{cft}")));
        // This user's home, wherever it is.
        assert!(attaches(CHROME_CDP, &format!("/srv/u{cft}")));
        assert!(!attaches(CHROME_CDP, &format!("/opt/x{cft}")));
        assert!(!attaches(
            CHROME_CDP,
            "/home/petter/.cache/chrome-cdp/browsers/x/chrome-linux-arm64/chrome"
        ));
        assert!(attaches(
            CHROME_CDP,
            "/home/petter/.cache/chrome-cdp/browsers/x/chrome-headless-shell-linux64/chrome-headless-shell"
        ));
    }

    #[test]
    fn profiles_without_userns_or_attachment_do_not_count() {
        let text =
            "profile foo /opt/x flags=(complain) {\n  /** r,\n}\nprofile bar {\n  userns,\n}\n";
        let p = parse(text);
        assert_eq!(p.len(), 2);
        assert!(!p[0].userns);
        assert_eq!(p[1].attach, None);
        assert!(!attaches(text, "/opt/x"));
    }

    #[test]
    fn bare_path_profiles_and_child_blocks() {
        let text = "/usr/bin/foo flags=(unconfined) {\n  userns,\n  ^hat {\n    /x r,\n  }\n}\n\"/opt/my app/bin\" {\n  userns,\n}\n";
        let p = parse(text);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(attaches(text, "/usr/bin/foo"));
        assert!(attaches(text, "/opt/my app/bin"));
    }

    #[test]
    fn globs() {
        let m = |p: &str, s: &str| glob_match(p.as_bytes(), s.as_bytes());
        assert!(m("/a/*/c", "/a/b/c"));
        assert!(!m("/a/*/c", "/a/b/x/c"));
        assert!(m("/a/**/c", "/a/b/x/c"));
        assert!(m("/a/?", "/a/b"));
        assert!(!m("/a/?", "/a//"));
        assert!(m("/a/[a-c]x", "/a/bx"));
        assert!(!m("/a/[^a-c]x", "/a/bx"));
        assert!(m("/a\\*", "/a*"));
    }
}
