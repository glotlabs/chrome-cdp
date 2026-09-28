//! `http` (plan 11, milestone 11.11): a request sent from the browser
//! context, outside the page. The daemon sends it (`engine::http`); the
//! CLI makes the URL absolute and checks the status against
//! `--expect-status`. The test runner keeps a `--save`d answer for
//! `${saved.NAME…}` (`crate::test`).

use daemon::rpc;
use engine::HttpRequest;
use protocol::schema::HttpData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::{GlobalArgs, HttpArgs};
use crate::{input, route, session};

/// How much of a failed answer's body the error shows.
const BODY_SHOWN: usize = 300;

pub(crate) fn run(args: &HttpArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let method = args.method.trim().to_ascii_uppercase();
    if method.is_empty() || !method.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("`{}` is not an HTTP method", args.method),
        )
        .hint(Hint::text("e.g. http GET /api/items")));
    }
    let statuses = Statuses::parse(&args.expect_status)?;
    if let Some(name) = &args.save
        && !protocol::script::is_name(name)
    {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--save {name}: not a name (letters, digits, _ and -)"),
        )
        .hint(Hint::text("e.g. --save order, then ${saved.order.id}")));
    }
    let (url, guessed) = crate::mock::absolute_url(&args.url, global.base_url.as_deref())?;
    if guessed {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} is a path, and no base_url is set", args.url),
        )
        .hint(Hint::text(
            "give a full URL (http://…), or set base_url in chrome-cdp.toml",
        )));
    }
    let mut headers: Vec<(String, String)> = args
        .headers
        .iter()
        .map(|h| route::header(h, "--header"))
        .collect::<Result<_, _>>()?;
    let has = |headers: &[(String, String)], name: &str| {
        headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    let body = match (&args.json_body, &args.body) {
        (Some(text), _) => {
            let value = route::json(text, "--json-body")?;
            if !has(&headers, "content-type") {
                headers.push(("Content-Type".into(), "application/json".into()));
            }
            Some(value.to_string())
        }
        (None, Some(text)) => Some(input::text_arg(text, "--body")?),
        (None, None) => None,
    };
    if body.is_some() && !has(&headers, "content-type") {
        headers.push(("Content-Type".into(), "text/plain;charset=UTF-8".into()));
    }
    let request = HttpRequest {
        method: method.clone(),
        url,
        headers,
        body,
        timeout_ms: session::timeout_ms(global),
        max_chars: args.max_chars,
    };
    let output = session::call_running(global, rpc::HTTP, &request)?;
    let data: HttpData = serde_json::from_value(output.data.clone()).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("the daemon's http answer does not parse: {e}"),
        )
    })?;
    if statuses.accepts(data.status) {
        return Ok(output);
    }
    Err(unexpected(args, &data, &statuses))
}

/// The error for a status `--expect-status` does not take.
fn unexpected(args: &HttpArgs, data: &HttpData, statuses: &Statuses) -> ErrorInfo {
    let mut info = ErrorInfo::new(
        ErrorKind::ExpectFailed,
        format!(
            "{} {} answered {}, not {}",
            data.method, data.url, data.status, statuses.text
        ),
    );
    let body = match &data.body {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) if !data.json => text.clone(),
        value => value.to_string(),
    };
    if !body.is_empty() {
        let shown: String = body.chars().take(BODY_SHOWN).collect();
        let more = if shown.len() < body.len() { "…" } else { "" };
        info = info.note("body", format!("{}{more}", shown.replace('\n', " ")));
    }
    let by = match (&data.mock, &data.route, &data.unmatched) {
        (Some(mock), _, _) => format!("mock {} ({})", mock.name, mock.at),
        (_, Some(rule), _) => format!("route {rule}"),
        (_, _, Some(set)) => format!("no mock; mock set {set}'s unmatched policy"),
        _ => "the server".to_owned(),
    };
    info = info.note("answered by", by);
    let again = Cmd::new("http")
        .arg(&data.method)
        .arg(&args.url)
        .flag("expect-status", data.status.to_string());
    info.hint(
        Hint::text(format!(
            "if {} is what the test expects, say so: {again}",
            data.status
        ))
        .to_string(),
    )
    .details(serde_json::to_value(data).unwrap_or_default())
}

/// `--expect-status`: `201`, `2xx`, `200,204`, `any`.
struct Statuses {
    /// As given.
    text: String,
    /// `None` for any; else (low, high) ranges, inclusive.
    ranges: Option<Vec<(u16, u16)>>,
}

impl Statuses {
    fn parse(text: &str) -> Result<Self, ErrorInfo> {
        let bad = |part: &str| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--expect-status: `{part}` is not a status"),
            )
            .hint(Hint::text("give statuses like 201, 2xx or 200,204, or any"))
        };
        let text = text.trim();
        if text.eq_ignore_ascii_case("any") {
            return Ok(Self {
                text: text.to_owned(),
                ranges: None,
            });
        }
        let mut ranges = Vec::new();
        for part in text.split(',').map(str::trim) {
            let lower = part.to_ascii_lowercase();
            let range = match lower.strip_suffix("xx") {
                Some(digit) if digit.len() == 1 => {
                    let d: u16 = digit.parse().map_err(|_| bad(part))?;
                    (1..=5)
                        .contains(&d)
                        .then_some((d * 100, d * 100 + 99))
                        .ok_or_else(|| bad(part))?
                }
                Some(_) => return Err(bad(part)),
                None => {
                    let code: u16 = part.parse().map_err(|_| bad(part))?;
                    if !(100..=599).contains(&code) {
                        return Err(bad(part));
                    }
                    (code, code)
                }
            };
            ranges.push(range);
        }
        Ok(Self {
            text: text.to_owned(),
            ranges: Some(ranges),
        })
    }

    fn accepts(&self, status: u16) -> bool {
        self.ranges
            .as_ref()
            .is_none_or(|r| r.iter().any(|&(lo, hi)| (lo..=hi).contains(&status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_parse() {
        let s = Statuses::parse("2xx").unwrap();
        assert!(s.accepts(200) && s.accepts(299) && !s.accepts(301));
        let s = Statuses::parse("200, 204,4XX").unwrap();
        assert!(s.accepts(204) && s.accepts(404) && !s.accepts(201));
        assert!(Statuses::parse("any").unwrap().accepts(500));
        for bad in ["", "2x", "9xx", "abc", "600", "20"] {
            assert!(Statuses::parse(bad).is_err(), "{bad}");
        }
    }
}
