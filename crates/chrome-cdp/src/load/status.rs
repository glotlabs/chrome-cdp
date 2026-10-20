//! A running load run's status line (plan 23.4), on stderr: rewritten
//! every second when stderr is a terminal, printed every 10 s otherwise.
//!
//! ```text
//! 03:12 stage 2/3 · 412 users · 20.1/s arrivals · req p95 184 ms · errors 0.4% · cpu 63%
//! 03:40 stage 2/3 · 450 users · 20.0/s arrivals · req p95 911 ms · errors 0.4% · CLIENT SATURATED (cpu 97%)
//! ```
//!
//! The rates, p95 and error rate are the last 10 seconds'; the CPU is the
//! machine's, the last second's (plan 23.5).

use std::io::IsTerminal as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::metrics::Recent;
use super::profile::Model;
use crate::output;

/// How often the line is printed when stderr is no terminal, in seconds.
const EVERY: u64 = 10;

/// Whether the line is on the terminal now, to be cleared before another
/// message.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Clears a line, back to its start.
const CLEAR: &str = "\r\x1b[2K";

/// Where the status line goes.
#[derive(Debug, Clone, Copy)]
pub(super) struct Status {
    tty: bool,
}

impl Status {
    pub(super) fn new() -> Self {
        Self {
            tty: std::io::stderr().is_terminal(),
        }
    }

    /// Shows `line`, the status at second `t`.
    pub(super) fn show(self, t: u64, line: &str) {
        if self.tty {
            SHOWN.store(true, Ordering::Relaxed);
            output::stderr(&format!("{CLEAR}{line}"));
        } else if t.is_multiple_of(EVERY) {
            output::stderr(&format!("{line}\n"));
        }
    }

    /// Says `message` (a line) on stderr, under the status line.
    pub(super) fn say(message: &str) {
        output::stderr(&format!("{}{message}", clear()));
    }

    /// Takes the line off the terminal, at the end of the run.
    pub(super) fn end(self) {
        if self.tty {
            output::stderr(clear());
        }
    }
}

/// What clears the status line before a message: nothing when it is not
/// shown.
pub(super) fn clear() -> &'static str {
    if SHOWN.swap(false, Ordering::Relaxed) {
        CLEAR
    } else {
        ""
    }
}

/// The load generator, for the status line.
#[derive(Debug, Clone, Default)]
pub(super) struct Client {
    /// The machine's CPU use, in %.
    pub(super) cpu: Option<f64>,
    /// The limit crossed while saturated: `cpu 97%`.
    pub(super) saturated: Option<String>,
}

/// The status line at `at` into the run: the stage of `model` it is in,
/// the `users` running, the last seconds' numbers, and the client's.
pub(super) fn line(
    at: Duration,
    model: &Model,
    users: u32,
    recent: &Recent,
    client: &Client,
) -> String {
    let mut parts = vec![clock(at)];
    let stages = match model {
        Model::Open(a) => a.stages.iter().map(|s| s.duration).collect::<Vec<_>>(),
        Model::Closed(u) => u.stages.iter().map(|s| s.duration).collect(),
    };
    if stages.len() > 1 {
        let mut end = Duration::ZERO;
        let stage = stages
            .iter()
            .position(|d| {
                end += *d;
                at < end
            })
            .unwrap_or(stages.len() - 1);
        parts[0] = format!("{} stage {}/{}", parts[0], stage + 1, stages.len());
    }
    parts.push(format!(
        "{users} {}",
        if users == 1 { "user" } else { "users" }
    ));
    #[allow(clippy::cast_precision_loss)]
    let per_second = |n: u64| {
        if recent.seconds == 0 {
            0.0
        } else {
            n as f64 / recent.seconds as f64
        }
    };
    parts.push(match model {
        Model::Open(_) => format!("{:.1}/s arrivals", per_second(recent.arrivals)),
        Model::Closed(_) => format!("{:.1}/s iterations", per_second(recent.iterations)),
    });
    parts.push(match recent.p95 {
        Some(ms) => format!("req p95 {}", ms_text(ms)),
        None => "req p95 -".to_owned(),
    });
    parts.push(if recent.requests == 0 {
        "errors -".to_owned()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let rate = recent.errors as f64 / recent.requests as f64 * 100.0;
        format!("errors {}", percent(rate))
    });
    match (&client.saturated, client.cpu) {
        (Some(over), _) => parts.push(format!("CLIENT SATURATED ({over})")),
        (None, Some(cpu)) => parts.push(format!("cpu {cpu:.0}%")),
        (None, None) => {}
    }
    parts.join(" · ")
}

/// `03:12`, or `1:03:12` past an hour.
pub(super) fn clock(at: Duration) -> String {
    let s = at.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// `184 ms`, `4.2 ms`, `1.25 s`.
pub(super) fn ms_text(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// `0.4%`, `12%`, `0%`.
pub(super) fn percent(p: f64) -> String {
    if p == 0.0 {
        "0%".to_owned()
    } else if p < 10.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Client, line};
    use crate::load::metrics::Recent;
    use crate::load::profile::{Arrivals, Model, RateStage, UserStage, Users};

    #[test]
    fn the_status_line() {
        let stage = |d: u64| RateStage {
            from: 20.0,
            to: 20.0,
            duration: Duration::from_secs(d),
        };
        let open = Model::Open(Arrivals {
            stages: vec![stage(120), stage(600), stage(60)],
            max_users: 400,
        });
        let recent = Recent {
            seconds: 10,
            arrivals: 201,
            iterations: 180,
            requests: 2000,
            errors: 8,
            p95: Some(184.4),
        };
        let client = Client {
            cpu: Some(63.4),
            saturated: None,
        };
        assert_eq!(
            line(Duration::from_secs(192), &open, 412, &recent, &client),
            "03:12 stage 2/3 · 412 users · 20.1/s arrivals · req p95 184 ms · errors 0.4% · cpu 63%"
        );
        let saturated = Client {
            cpu: Some(97.2),
            saturated: Some("cpu 97%".into()),
        };
        assert_eq!(
            line(Duration::from_secs(220), &open, 450, &recent, &saturated),
            "03:40 stage 2/3 · 450 users · 20.1/s arrivals · req p95 184 ms · errors 0.4% · \
             CLIENT SATURATED (cpu 97%)"
        );
        let closed = Model::Closed(Users {
            stages: vec![UserStage {
                from: 1,
                to: 1,
                duration: Duration::from_secs(60),
            }],
            iterations: None,
        });
        assert_eq!(
            line(
                Duration::from_millis(3_900),
                &closed,
                1,
                &Recent {
                    seconds: 3,
                    iterations: 1,
                    ..Recent::default()
                },
                &Client::default()
            ),
            "00:03 · 1 user · 0.3/s iterations · req p95 - · errors -"
        );
        assert_eq!(
            line(
                Duration::from_secs(3_725),
                &closed,
                0,
                &Recent::default(),
                &Client::default()
            ),
            "1:02:05 · 0 users · 0.0/s iterations · req p95 - · errors -"
        );
    }
}
