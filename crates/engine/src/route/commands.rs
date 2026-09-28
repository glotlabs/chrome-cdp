//! `route`, `block`, `routes` and `unroute` (plan 07, milestone 7.1).
//!
//! Rules belong to the session, not to a tab: they apply to every tab,
//! popup and frame, those opened later too, and stay when the browser is
//! restarted. With `--context`, they belong to that browser context and
//! apply only in it. None of these commands takes a tab lock.

use std::fmt::Write as _;

use protocol::schema::{RouteData, RouteInfo, RoutedRequest, RoutesData, UnrouteData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use super::{parse_route_id, route_name};
use crate::request::{RouteRequest, RoutesRequest, UnrouteRequest, millis};
use crate::{Engine, State};

/// How much of a URL a `routes --log` line shows.
const SHOWN_URL: usize = 150;

impl Engine {
    /// `route` and `block`: adds rules after the existing ones and turns
    /// interception on in the running browser.
    pub async fn route(&self, req: RouteRequest) -> Result<Output, ErrorInfo> {
        if req.rules.is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "no rule to add").hint(Hint::run(
                    Cmd::new("route").raw("'**/api/*' --json-body '{}'"),
                )),
            );
        }
        let scope = self.context_key()?;
        let mut rules = req.rules;
        for rule in &mut rules {
            rule.context.clone_from(&scope);
        }
        let added = self.settings.routes.add(&rules)?;
        self.apply_routes(millis(req.timeout_ms)).await;
        let text: Vec<String> = added.iter().map(rule_line).collect();
        let quiet: Vec<&str> = added.iter().map(|r| r.id.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = RouteData {
            added,
            active: self.settings.routes.count(scope.as_deref()),
        };
        Ok(Output::typed(&data, text.join("\n")).quiet(quiet))
    }

    /// `routes`: the rules with their hits, or with `--log` the requests
    /// routing saw.
    pub fn routes(&self, req: &RoutesRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let routes = self.settings.routes.list(scope.as_deref());
        if !req.log {
            let mut text = String::new();
            for rule in &routes {
                let _ = writeln!(text, "{}", listed_line(rule));
            }
            if routes.is_empty() {
                let _ = write!(
                    text,
                    "no routes; {} adds one",
                    Cmd::new("route")
                        .raw("<url-pattern> --json-body '…'")
                        .session(&self.session)
                );
            }
            let quiet: Vec<&str> = routes.iter().map(|r| r.id.as_str()).collect();
            let quiet = quiet.join("\n");
            let data = RoutesData {
                routes,
                log: None,
                skipped: 0,
            };
            return Ok(Output::typed(&data, text.trim_end().to_owned())
                .quiet(quiet)
                .limit(req.max_chars, "--max-chars 0 for all of it"));
        }
        let (log, skipped) = self.settings.routes.journal(scope.as_deref(), req.limit);
        let mut text = String::new();
        if skipped > 0 {
            let more = usize::try_from(skipped).unwrap_or(usize::MAX) + req.limit;
            let _ = writeln!(
                text,
                "… {skipped} earlier {} not shown; {} shows them",
                if skipped == 1 { "request" } else { "requests" },
                Cmd::new("routes")
                    .switch("log")
                    .flag("limit", more.to_string())
                    .session(&self.session)
            );
        }
        for entry in &log {
            let _ = writeln!(text, "{}", log_line(entry));
        }
        if log.is_empty() {
            text.push_str(if routes.is_empty() {
                "no routed requests; requests are intercepted only while routes exist"
            } else {
                "no routed requests yet"
            });
        }
        let quiet: Vec<&str> = log.iter().map(|e| e.url.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = RoutesData {
            routes,
            log: Some(log),
            skipped,
        };
        Ok(Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(
                req.max_chars,
                "--max-chars 0 for all of it, or --limit N for fewer",
            ))
    }

    /// `unroute`: removes rules; without any left, requests are no longer
    /// intercepted.
    pub async fn unroute(&self, req: UnrouteRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let removed = if req.all {
            self.settings.routes.clear(scope.as_deref())
        } else {
            if req.ids.is_empty() {
                return Err(ErrorInfo::new(ErrorKind::BadArgs, "no route to remove")
                    .hint(Hint::run(
                        Cmd::new("unroute").arg("r1").session(&self.session),
                    ))
                    .hint(Hint::run(
                        Cmd::new("unroute").switch("all").session(&self.session),
                    )));
            }
            let ids = req
                .ids
                .iter()
                .map(|id| parse_route_id(id))
                .collect::<Result<Vec<u32>, _>>()?;
            self.settings
                .routes
                .remove(scope.as_deref(), &ids)
                .map_err(|missing| {
                    ErrorInfo::new(
                        ErrorKind::NotFound,
                        format!("there is no route {}", route_name(missing)),
                    )
                    .hint(
                        Hint::run(Cmd::new("routes").session(&self.session))
                            .why("lists the routes"),
                    )
                })?
        };
        self.apply_routes(millis(req.timeout_ms)).await;
        let removed: Vec<String> = removed.into_iter().map(route_name).collect();
        let active = self.settings.routes.count(scope.as_deref());
        let mut text = if removed.is_empty() {
            "no routes to remove".to_owned()
        } else {
            format!("removed {}", removed.join(", "))
        };
        let _ = write!(
            text,
            "; {}",
            match active {
                0 => "no routes left".to_owned(),
                1 => "1 route left".to_owned(),
                n => format!("{n} routes left"),
            }
        );
        let quiet = removed.join("\n");
        let data = UnrouteData { removed, active };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// Makes the running browser (if any) intercept requests as the rules
    /// now need; a browser started later gets it at its setup.
    pub(super) async fn apply_routes(&self, timeout: std::time::Duration) {
        let running = match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(_) | State::Empty => None,
        };
        if let Some(running) = running {
            running.tabs.apply_interception(timeout).await;
        }
    }
}

/// What a rule matches: `GET xhr **/api/*` (its label instead of the
/// pattern, if it has one).
fn matched(rule: &RouteInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !rule.methods.is_empty() {
        parts.push(rule.methods.join(","));
    }
    if !rule.resource_types.is_empty() && rule.label.is_none() {
        parts.push(rule.resource_types.join(","));
    }
    parts.push(rule.label.clone().unwrap_or_else(|| rule.pattern.clone()));
    parts.join(" ")
}

/// `r1 GET **/api/items → fulfill 200 application/json 17B, for the next
/// 2 requests`.
fn rule_line(rule: &RouteInfo) -> String {
    let mut line = format!("{} {} → {}", rule.id, matched(rule), rule.summary);
    match rule.left {
        Some(1) => line.push_str(", for the next request"),
        Some(left) => {
            let _ = write!(line, ", for the next {left} requests");
        }
        None => {}
    }
    line
}

/// A `routes` line: [`rule_line`] with the hits.
fn listed_line(rule: &RouteInfo) -> String {
    let mut line = format!("{} {} → {}", rule.id, matched(rule), rule.summary);
    let _ = write!(
        line,
        " ({} {}",
        rule.hits,
        if rule.hits == 1 { "hit" } else { "hits" }
    );
    if let Some(left) = rule.left {
        let _ = write!(line, ", {left} left");
    }
    line.push(')');
    line
}

/// `#12 t1 GET https://x/api/items fetch → r1 fulfill 200`.
fn log_line(entry: &RoutedRequest) -> String {
    let url: String = entry.url.chars().take(SHOWN_URL).collect();
    let more = if url.len() < entry.url.len() {
        "…"
    } else {
        ""
    };
    let mut line = format!("#{} ", entry.seq);
    if let Some(tab) = &entry.tab {
        let _ = write!(line, "{tab} ");
    }
    let _ = write!(
        line,
        "{} {url}{more} {} → ",
        entry.method, entry.resource_type
    );
    if let Some(route) = &entry.route {
        let _ = write!(line, "{route} ");
    }
    line.push_str(&entry.outcome);
    line
}
