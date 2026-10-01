//! `multipart/form-data` bodies (a `FormData` POST): the fields a mock's
//! `match.form` and `{{form.x}}` see, and the file parts `{{form_files.x}}`
//! describes. File contents are never kept: a file part's value is its
//! file name.

use serde::Serialize;

/// One part of a multipart body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Part {
    pub(crate) name: String,
    /// What the field holds: a text part's text, a file part's file name.
    pub(crate) value: String,
    /// Set for a file part (one with a `filename`).
    pub(crate) file: Option<FormFile>,
}

/// A file part, as `{{form_files.x}}` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FormFile {
    pub(crate) filename: String,
    /// The part's `Content-Type`, empty when it has none.
    pub(crate) content_type: String,
    /// The part's content in bytes, as received.
    pub(crate) size: usize,
}

/// The `boundary` parameter of a `multipart/form-data` content type;
/// `None` for another type, `Some(Err)` when it has no boundary.
pub(crate) fn boundary(content_type: &str) -> Option<Result<String, String>> {
    let mut params = split_params(content_type).into_iter();
    let essence = params.next()?;
    if !essence.trim().eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    let found = params.find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("boundary")
            .then(|| unquote(v.trim()))
    });
    Some(match found {
        Some(b) if !b.is_empty() => Ok(b),
        _ => Err("multipart body without a boundary".to_owned()),
    })
}

/// The parts of `body`, delimited by `boundary`; parts without a
/// `Content-Disposition` name are skipped. Lines may end in CRLF or LF.
pub(crate) fn parse(body: &[u8], boundary: &str) -> Result<Vec<Part>, String> {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut at = find_delim(body, 0, delim).ok_or("multipart body without its boundary")?;
    let mut parts = Vec::new();
    loop {
        let mut p = at + delim.len();
        if body[p..].starts_with(b"--") {
            return Ok(parts);
        }
        while matches!(body.get(p), Some(b' ' | b'\t')) {
            p += 1;
        }
        if body[p..].starts_with(b"\r\n") {
            p += 2;
        } else if body[p..].starts_with(b"\n") {
            p += 1;
        } else {
            return Err("multipart body without a closing boundary".to_owned());
        }
        let next = find_delim(body, p, delim).ok_or("multipart body without a closing boundary")?;
        // The line break before a boundary belongs to it.
        let end = if next >= p + 2 && body[next - 2] == b'\r' {
            next - 2
        } else {
            next - 1
        };
        if let Some(part) = part(&body[p..end]) {
            parts.push(part);
        }
        at = next;
    }
}

/// The first `delim` at or after `from` that is a line of its own (or
/// starts the body): followed by `--`, or by blanks and a line break.
fn find_delim(body: &[u8], from: usize, delim: &[u8]) -> Option<usize> {
    let is_delim = |at: usize| {
        let Some(rest) = body[at..].strip_prefix(delim) else {
            return false;
        };
        let rest = rest
            .iter()
            .position(|b| !matches!(b, b' ' | b'\t'))
            .map_or(&[][..], |i| &rest[i..]);
        body[at + delim.len()..].starts_with(b"--")
            || rest.is_empty()
            || rest.starts_with(b"\r\n")
            || rest.starts_with(b"\n")
    };
    if from == 0 && is_delim(0) {
        return Some(0);
    }
    let mut i = from;
    while i < body.len() {
        let nl = i + body[i..].iter().position(|&b| b == b'\n')?;
        if is_delim(nl + 1) {
            return Some(nl + 1);
        }
        i = nl + 1;
    }
    None
}

/// One part: its headers up to the first blank line, then its content.
fn part(raw: &[u8]) -> Option<Part> {
    let (head, content) = split_head(raw);
    let head = String::from_utf8_lossy(head);
    let mut disposition = None;
    let mut content_type = String::new();
    for line in head.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-disposition") {
            disposition = Some(value.trim().to_owned());
        } else if name.trim().eq_ignore_ascii_case("content-type") {
            value.trim().clone_into(&mut content_type);
        }
    }
    let disposition = disposition?;
    let params = split_params(&disposition);
    let mut iter = params.iter();
    if !iter.next()?.trim().eq_ignore_ascii_case("form-data") {
        return None;
    }
    let mut name = None;
    let mut filename = None;
    for p in iter {
        let Some((k, v)) = p.split_once('=') else {
            continue;
        };
        let v = field_name(&unquote(v.trim()));
        match k.trim().to_ascii_lowercase().as_str() {
            "name" => name = Some(v),
            "filename" => filename = Some(v),
            _ => {}
        }
    }
    let name = name?;
    Some(match filename {
        Some(filename) => Part {
            name,
            value: filename.clone(),
            file: Some(FormFile {
                filename,
                content_type,
                size: content.len(),
            }),
        },
        None => Part {
            name,
            value: String::from_utf8_lossy(content).into_owned(),
            file: None,
        },
    })
}

/// A part's header block and its content.
fn split_head(raw: &[u8]) -> (&[u8], &[u8]) {
    if raw.starts_with(b"\r\n") {
        return (&[], &raw[2..]);
    }
    if raw.starts_with(b"\n") {
        return (&[], &raw[1..]);
    }
    let mut i = 0;
    while let Some(off) = raw[i..].iter().position(|&b| b == b'\n') {
        let nl = i + off;
        let rest = &raw[nl + 1..];
        if rest.starts_with(b"\r\n") {
            return (&raw[..nl], &rest[2..]);
        }
        if rest.starts_with(b"\n") {
            return (&raw[..nl], &rest[1..]);
        }
        i = nl + 1;
    }
    (raw, &[])
}

/// `a; b="c;d"; e` split at the semicolons outside quotes.
fn split_params(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// A parameter value without its quotes and backslash escapes.
fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Browsers write `"`, CR and LF in field and file names as `%22`, `%0D`
/// and `%0A`.
fn field_name(text: &str) -> String {
    text.replace("%22", "\"")
        .replace("%0D", "\r")
        .replace("%0A", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: &str, value: &str) -> Part {
        Part {
            name: name.into(),
            value: value.into(),
            file: None,
        }
    }

    #[test]
    fn boundaries() {
        assert_eq!(
            boundary("multipart/form-data; boundary=----WebKitFormBoundaryX"),
            Some(Ok("----WebKitFormBoundaryX".into()))
        );
        assert_eq!(
            boundary(r#"Multipart/Form-Data; charset=utf-8; Boundary="a b;c""#),
            Some(Ok("a b;c".into()))
        );
        assert_eq!(
            boundary("multipart/form-data"),
            Some(Err("multipart body without a boundary".into()))
        );
        assert_eq!(boundary("application/json"), None);
        assert_eq!(boundary(""), None);
    }

    #[test]
    fn text_and_file_parts_with_crlf() {
        let body = b"--XyZ\r\n\
            Content-Disposition: form-data; name=\"email\"\r\n\
            \r\n\
            ada@example.com\r\n\
            --XyZ\r\n\
            Content-Disposition: form-data; name=\"avatar\"; filename=\"me.png\"\r\n\
            Content-Type: image/png\r\n\
            \r\n\
            \x89PNG\r\n\x1a\n\x00\xff\r\n\
            --XyZ\r\n\
            Content-Disposition: form-data; name=\"tag\"\r\n\
            \r\n\
            a\r\n\
            --XyZ\r\n\
            Content-Disposition: form-data; name=\"tag\"\r\n\
            \r\n\
            b\r\n\
            --XyZ--\r\n";
        let parts = parse(body, "XyZ").unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], text("email", "ada@example.com"));
        assert_eq!(parts[1].value, "me.png");
        assert_eq!(
            parts[1].file,
            Some(FormFile {
                filename: "me.png".into(),
                content_type: "image/png".into(),
                size: 10,
            })
        );
        assert_eq!(parts[2], text("tag", "a"));
        assert_eq!(parts[3], text("tag", "b"));
    }

    #[test]
    fn lf_lines_preamble_quotes_and_empty_parts() {
        let body = "preamble\n--b\n\
            Content-Disposition: form-data; name=\"a \\\"q\\\" %22x%22\"\n\
            \n\
            line 1\nline 2\n\
            --b\n\
            Content-Disposition: form-data; name=\"empty\"\n\
            \n\
            \n\
            --b\n\
            Content-Disposition: form-data; name=\"nofile\"; filename=\"\"\n\
            Content-Type: application/octet-stream\n\
            \n\
            \n\
            --b\n\
            Content-Type: text/plain\n\
            \n\
            no name, skipped\n\
            --b--";
        let parts = parse(body.as_bytes(), "b").unwrap();
        assert_eq!(
            parts,
            vec![
                text("a \"q\" \"x\"", "line 1\nline 2"),
                text("empty", ""),
                Part {
                    name: "nofile".into(),
                    value: String::new(),
                    file: Some(FormFile {
                        filename: String::new(),
                        content_type: "application/octet-stream".into(),
                        size: 0,
                    }),
                },
            ]
        );
    }

    #[test]
    fn a_boundary_inside_a_line_is_content() {
        let body =
            b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nsee --b here\r\n--b--";
        assert_eq!(parse(body, "b").unwrap(), vec![text("x", "see --b here")]);
    }

    #[test]
    fn malformed_bodies() {
        assert_eq!(
            parse(b"email=a", "b").unwrap_err(),
            "multipart body without its boundary"
        );
        assert_eq!(
            parse(
                b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\ncut off",
                "b"
            )
            .unwrap_err(),
            "multipart body without a closing boundary"
        );
        // A line that only starts with the boundary is not one.
        assert_eq!(parse(b"--bad\r\n\r\n--b--", "b").unwrap(), vec![]);
        assert_eq!(
            parse(
                b"--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n--bx\r\n--b--",
                "b"
            )
            .unwrap(),
            vec![text("x", "--bx")]
        );
        // An empty form: just the closing boundary.
        assert_eq!(parse(b"--b--\r\n", "b").unwrap(), vec![]);
    }
}
