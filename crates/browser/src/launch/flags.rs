//! The Chrome command line (plan 02, milestone 2.3).
//!
//! [`FLAGS`] and [`DISABLED_FEATURES`] are the one place the launch flags
//! live; `docs/chrome-flags.md` explains each one (a test keeps the two in
//! step). Users add their own with `--chrome-arg`.

use std::path::Path;

use protocol::{ErrorInfo, ErrorKind};

use crate::host::Os;

/// Where a flag applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Only {
    Always,
    Linux,
    MacOs,
    /// Unless `--headed`.
    Headless,
}

/// The flags every launch gets (besides `--user-data-dir`,
/// `--disable-features` and, when needed, `--no-sandbox`).
pub const FLAGS: &[(&str, Only)] = &[
    ("--remote-debugging-pipe", Only::Always),
    ("--headless", Only::Headless),
    ("--no-first-run", Only::Always),
    ("--no-default-browser-check", Only::Always),
    ("--disable-background-networking", Only::Always),
    ("--disable-background-timer-throttling", Only::Always),
    ("--disable-backgrounding-occluded-windows", Only::Always),
    ("--disable-renderer-backgrounding", Only::Always),
    ("--disable-breakpad", Only::Always),
    ("--disable-client-side-phishing-detection", Only::Always),
    ("--disable-component-update", Only::Always),
    ("--disable-default-apps", Only::Always),
    ("--disable-dev-shm-usage", Only::Linux),
    ("--disable-field-trial-config", Only::Always),
    ("--disable-hang-monitor", Only::Always),
    ("--disable-ipc-flooding-protection", Only::Always),
    ("--disable-popup-blocking", Only::Always),
    ("--disable-prompt-on-repost", Only::Always),
    ("--disable-search-engine-choice-screen", Only::Always),
    ("--disable-sync", Only::Always),
    ("--metrics-recording-only", Only::Always),
    ("--password-store=basic", Only::Linux),
    ("--use-mock-keychain", Only::MacOs),
    ("--export-tagged-pdf", Only::Always),
];

/// Features turned off with one `--disable-features=` flag.
pub const DISABLED_FEATURES: &[&str] = &[
    "Translate",
    "MediaRouter",
    "DialMediaRouteProvider",
    "OptimizationHints",
    "AcceptCHFrame",
    "AutofillServerCommunication",
    "GlobalMediaControls",
    "LensOverlay",
    "HttpsUpgrades",
    "PaintHolding",
];

/// The flags for `os`, in [`FLAGS`] order, plus `--disable-features`.
pub fn default_flags(os: Os, headed: bool) -> Vec<String> {
    let mut flags: Vec<String> = FLAGS
        .iter()
        .filter(|(_, only)| match only {
            Only::Always => true,
            Only::Linux => os == Os::Linux,
            Only::MacOs => os == Os::MacOs,
            Only::Headless => !headed,
        })
        .map(|(flag, _)| (*flag).to_owned())
        .collect();
    flags.push(format!(
        "--disable-features={}",
        DISABLED_FEATURES.join(",")
    ));
    flags
}

/// Checks the `--chrome-arg` values: each must be a `--flag`, and must not
/// take over what launch owns.
pub fn check_extra(extra: &[String]) -> Result<(), ErrorInfo> {
    for arg in extra {
        let name = arg.split_once('=').map_or(arg.as_str(), |(n, _)| n);
        let bad = |message: String, hint: &str| {
            Err(ErrorInfo::new(ErrorKind::BadArgs, message).hint(hint))
        };
        if !arg.starts_with("--") || arg.len() == 2 {
            return bad(
                format!("--chrome-arg `{arg}` is not a Chrome flag"),
                "pass flags in their --name=value form, e.g. --chrome-arg=--window-size=1280,800",
            );
        }
        match name {
            "--user-data-dir" => {
                return bad(
                    "--chrome-arg cannot set --user-data-dir".into(),
                    "use chrome-cdp's own --user-data-dir <path> or --profile <name>",
                );
            }
            "--remote-debugging-pipe" | "--remote-debugging-io-pipes" => {
                return bad(
                    format!("--chrome-arg cannot set {name}"),
                    "chrome-cdp always talks to Chrome over its own pipe; drop the flag",
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// The full argument list after the program name: the defaults for `os`,
/// `--no-sandbox` if asked, the profile, the user's extra flags, and the
/// first page. A user `--disable-features` / `--enable-features` is merged
/// with ours, since Chrome only honours the last of each.
pub(crate) fn command_line(
    os: Os,
    headed: bool,
    no_sandbox: bool,
    profile: &Path,
    extra: &[String],
) -> Vec<String> {
    let mut disabled: Vec<String> = DISABLED_FEATURES.iter().map(|f| (*f).to_owned()).collect();
    let mut enabled: Vec<String> = Vec::new();
    let mut rest = Vec::new();
    for arg in extra {
        if let Some(list) = arg.strip_prefix("--disable-features=") {
            disabled.extend(list.split(',').filter(|f| !f.is_empty()).map(str::to_owned));
        } else if let Some(list) = arg.strip_prefix("--enable-features=") {
            enabled.extend(list.split(',').filter(|f| !f.is_empty()).map(str::to_owned));
        } else if arg == "--no-sandbox" && no_sandbox {
            // Already added below.
        } else {
            rest.push(arg.clone());
        }
    }
    // What the user enables wins over our defaults.
    disabled.retain(|f| !enabled.contains(f));
    let mut seen = std::collections::HashSet::new();
    disabled.retain(|f| seen.insert(f.clone()));

    let mut args: Vec<String> = default_flags(os, headed)
        .into_iter()
        .filter(|f| !f.starts_with("--disable-features="))
        .collect();
    if !disabled.is_empty() {
        args.push(format!("--disable-features={}", disabled.join(",")));
    }
    if !enabled.is_empty() {
        args.push(format!("--enable-features={}", enabled.join(",")));
    }
    if no_sandbox {
        args.push("--no-sandbox".into());
    }
    args.push(format!("--user-data-dir={}", profile.display()));
    args.extend(rest);
    args.push("about:blank".into());
    args
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DISABLED_FEATURES, FLAGS, check_extra, command_line, default_flags};
    use crate::host::Os;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn flags_depend_on_the_os_and_headed() {
        let linux = default_flags(Os::Linux, false);
        assert!(linux.contains(&"--headless".to_owned()));
        assert!(linux.contains(&"--disable-dev-shm-usage".to_owned()));
        assert!(linux.contains(&"--password-store=basic".to_owned()));
        assert!(!linux.contains(&"--use-mock-keychain".to_owned()));
        assert_eq!(linux[0], "--remote-debugging-pipe");
        assert!(
            linux
                .last()
                .unwrap()
                .starts_with("--disable-features=Translate,MediaRouter,")
        );

        let mac = default_flags(Os::MacOs, true);
        assert!(!mac.contains(&"--headless".to_owned()));
        assert!(!mac.contains(&"--disable-dev-shm-usage".to_owned()));
        assert!(mac.contains(&"--use-mock-keychain".to_owned()));
    }

    #[test]
    fn command_line_puts_profile_extra_flags_and_the_page_last() {
        let args = command_line(
            Os::Linux,
            false,
            false,
            Path::new("/p"),
            &strings(&["--lang=de"]),
        );
        let n = args.len();
        assert_eq!(
            args[n - 3..],
            strings(&["--user-data-dir=/p", "--lang=de", "about:blank"])
        );
        assert!(!args.contains(&"--no-sandbox".to_owned()));

        let args = command_line(
            Os::Linux,
            false,
            true,
            Path::new("/p"),
            &strings(&["--no-sandbox"]),
        );
        assert_eq!(args.iter().filter(|a| *a == "--no-sandbox").count(), 1);
    }

    #[test]
    fn user_features_are_merged_with_ours() {
        let args = command_line(
            Os::Linux,
            false,
            false,
            Path::new("/p"),
            &strings(&[
                "--disable-features=Foo,Translate",
                "--enable-features=HttpsUpgrades,Bar",
            ]),
        );
        let disabled: Vec<&String> = args
            .iter()
            .filter(|a| a.starts_with("--disable-features="))
            .collect();
        assert_eq!(disabled.len(), 1);
        let list = disabled[0].trim_start_matches("--disable-features=");
        assert!(list.ends_with(",Foo"), "{list}");
        assert_eq!(list.matches("Translate").count(), 1);
        assert!(!list.contains("HttpsUpgrades"), "{list}");
        assert!(args.contains(&"--enable-features=HttpsUpgrades,Bar".to_owned()));
    }

    #[test]
    fn extra_flags_are_checked() {
        assert!(check_extra(&strings(&["--lang=de", "--window-size=800,600"])).is_ok());
        for bad in [
            "https://example.com",
            "-v",
            "--",
            "--user-data-dir=/x",
            "--remote-debugging-pipe",
        ] {
            let err = check_extra(&strings(&[bad])).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{bad}");
        }
    }

    /// `docs/chrome-flags.md` documents every flag and feature.
    #[test]
    fn every_flag_is_documented() {
        let doc = include_str!("../../../../docs/chrome-flags.md");
        for (flag, _) in FLAGS {
            assert!(
                doc.contains(&format!("`{flag}`")),
                "{flag} is not in docs/chrome-flags.md"
            );
        }
        for feature in DISABLED_FEATURES {
            assert!(
                doc.contains(&format!("`{feature}`")),
                "{feature} is not in docs/chrome-flags.md"
            );
        }
    }
}
