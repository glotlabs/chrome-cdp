//! The token suite: for each snapshot corpus page, the tokens of
//! `chrome-cdp snapshot`, `snapshot -i`, `snapshot --fold` and `snapshot
//! --budget 2000`. `bench/tokens/snapshot-tokens.mjs` does the counting;
//! this runs it and keeps the result.
//! Counts are deterministic for a given tokenizer, so the suite's gate is
//! exact where the micro suite's is noisy.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::report::{self, Host};
use crate::stats::{Better, Metric};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub page: String,
    pub chrome_cdp: u64,
    pub interactive: u64,
    /// `snapshot --fold` (plan 16; 0 in reports from before it).
    #[serde(default)]
    pub fold: u64,
    /// `snapshot --budget 2000`.
    #[serde(default)]
    pub budget: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub chrome_cdp: u64,
    pub interactive: u64,
    #[serde(default)]
    pub fold: u64,
    #[serde(default)]
    pub budget: u64,
}

/// What snapshot-tokens.mjs prints with `--json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// `o200k_base` or `anthropic:<model>`.
    pub tokenizer: String,
    pub rows: Vec<Row>,
    pub total: Totals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenReport {
    pub suite: String,
    pub created: u64,
    pub host: Host,
    pub chrome: String,
    #[serde(flatten)]
    pub counts: Counts,
}

/// Runs the counter. `tokenizer` is `auto`, `anthropic` or `o200k`.
pub fn run(bin_dir: &Path, chrome: &Path, tokenizer: &str) -> Result<TokenReport, String> {
    let dir = report::workspace_root().join("bench/tokens");
    if !dir.join("node_modules/gpt-tokenizer").is_dir() {
        return Err(format!(
            "no node_modules in {}; run `npm ci` there",
            dir.display()
        ));
    }
    // The script finds the binaries as $CARGO_TARGET_DIR/$PROFILE.
    let (Some(profile), Some(target)) = (bin_dir.file_name(), bin_dir.parent()) else {
        return Err(format!(
            "{} is not a target/<profile> dir",
            bin_dir.display()
        ));
    };
    let out = Command::new("node")
        .args(["snapshot-tokens.mjs", "--json", "--tokenizer", tokenizer])
        .env("CHROME_PATH", chrome)
        .env("CARGO_TARGET_DIR", target)
        .env("PROFILE", profile)
        .current_dir(&dir)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("run node: {e}"))?;
    if !out.status.success() {
        return Err(format!("snapshot-tokens.mjs failed ({})", out.status));
    }
    let counts: Counts = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("snapshot-tokens.mjs printed no counts: {e}"))?;
    Ok(TokenReport {
        suite: "tokens".to_owned(),
        created: report::now(),
        host: Host::this(),
        chrome: crate::micro::chrome_version(chrome),
        counts,
    })
}

/// The baseline's name: one per tokenizer, as counts differ between them.
pub fn baseline_name(tokenizer: &str) -> String {
    let safe: String = tokenizer
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("tokens-{safe}")
}

/// The counts as metrics (fewer is better).
#[allow(clippy::cast_precision_loss)]
pub fn metrics(c: &Counts) -> Vec<Metric> {
    let tokens = |name: String, n: u64| Metric::value(&name, "tokens", Better::Lower, n as f64);
    let mut out = Vec::new();
    for r in &c.rows {
        out.push(tokens(format!("snapshot/{}", r.page), r.chrome_cdp));
        out.push(tokens(format!("snapshot-i/{}", r.page), r.interactive));
        out.push(tokens(format!("snapshot-fold/{}", r.page), r.fold));
        out.push(tokens(format!("snapshot-budget-2000/{}", r.page), r.budget));
    }
    out.push(tokens("snapshot/total".to_owned(), c.total.chrome_cdp));
    out.push(tokens("snapshot-i/total".to_owned(), c.total.interactive));
    out.push(tokens("snapshot-fold/total".to_owned(), c.total.fold));
    out.push(tokens(
        "snapshot-budget-2000/total".to_owned(),
        c.total.budget,
    ));
    out
}

/// The tokens section of docs/benchmarks.md.
pub fn markdown(r: &TokenReport) -> String {
    let c = &r.counts;
    let tokenizer = if c.tokenizer.starts_with("anthropic:") {
        format!("`{}` (the Anthropic token counting API)", c.tokenizer)
    } else {
        format!(
            "`{}` (offline: an approximation; set ANTHROPIC_API_KEY for Claude's own counts)",
            c.tokenizer
        )
    };
    let mut md = format!(
        "Last run: {} with {}. Tokenizer: {tokenizer}.\n\n\
         | Page | `snapshot` | `snapshot -i` | `--fold` | `--budget 2000` |\n\
         |---|---:|---:|---:|---:|\n",
        report::date(r.created),
        r.chrome
    );
    for row in &c.rows {
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} |",
            row.page, row.chrome_cdp, row.interactive, row.fold, row.budget
        );
    }
    let _ = writeln!(
        md,
        "| **total** | **{}** | **{}** | **{}** | **{}** |",
        c.total.chrome_cdp, c.total.interactive, c.total.fold, c.total.budget
    );
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> Counts {
        Counts {
            tokenizer: "o200k_base".into(),
            rows: vec![Row {
                page: "simple".into(),
                chrome_cdp: 40,
                interactive: 20,
                fold: 35,
                budget: 40,
            }],
            total: Totals {
                chrome_cdp: 40,
                interactive: 20,
                fold: 35,
                budget: 40,
            },
        }
    }

    #[test]
    fn metrics_and_markdown() {
        let m = metrics(&counts());
        let names: Vec<&str> = m.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "snapshot/simple",
                "snapshot-i/simple",
                "snapshot-fold/simple",
                "snapshot-budget-2000/simple",
                "snapshot/total",
                "snapshot-i/total",
                "snapshot-fold/total",
                "snapshot-budget-2000/total"
            ]
        );
        let r = TokenReport {
            suite: "tokens".into(),
            created: 0,
            host: Host::this(),
            chrome: "Chrome".into(),
            counts: counts(),
        };
        let md = markdown(&r);
        assert!(md.contains("| simple | 40 | 20 | 35 | 40 |"), "{md}");
        assert!(md.contains("approximation"), "{md}");
    }

    #[test]
    fn baseline_names_are_file_safe() {
        assert_eq!(baseline_name("o200k_base"), "tokens-o200k_base");
        assert_eq!(
            baseline_name("anthropic:claude-opus-5-5"),
            "tokens-anthropic-claude-opus-5-5"
        );
    }
}
