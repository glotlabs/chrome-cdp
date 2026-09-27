//! `browser::launch` against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::PathBuf;

use browser::{Host, LaunchOptions, ProfileChoice};

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

#[tokio::test]
async fn launch_writes_the_log_and_close_cleans_up() {
    let Some(path) = chrome_path() else { return };
    let host = Host::current();
    let chrome = browser::discover(Some(&path), &host).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("chrome.log");
    std::fs::write(&log, "an older run\n").unwrap();
    let options = LaunchOptions {
        log_file: Some(log.clone()),
        // A flag that makes Chrome write to stderr.
        args: vec!["--enable-logging=stderr".into(), "--v=0".into()],
        ..LaunchOptions::default()
    };
    let launched = browser::launch(&chrome, &options, &host).await.unwrap();
    assert!(
        launched.product().contains("Chrome/"),
        "{}",
        launched.product()
    );
    assert!(launched.ephemeral_profile());
    assert!(launched.command_line().contains(&"--headless".to_owned()));
    assert!(launched.command_line().contains(&format!(
        "--user-data-dir={}",
        launched.profile_dir().display()
    )));
    let profile = launched.profile_dir().to_owned();
    let run_dir = profile.parent().unwrap().to_owned();
    assert!(run_dir.join("tmp").is_dir());
    assert!(launched.warnings.is_empty(), "{:?}", launched.warnings);
    launched.close().await;

    assert!(!run_dir.exists(), "{}", run_dir.display());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("chrome.log.1")).unwrap(),
        "an older run\n"
    );
    assert!(log.metadata().unwrap().len() > 0, "Chrome logged nothing");
}

#[tokio::test]
async fn a_kept_profile_stays_after_close() {
    let Some(path) = chrome_path() else { return };
    let host = Host::current();
    let chrome = browser::discover(Some(&path), &host).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("kept");
    let options = LaunchOptions {
        profile: ProfileChoice::Dir(profile.clone()),
        ..LaunchOptions::default()
    };
    let launched = browser::launch(&chrome, &options, &host).await.unwrap();
    assert!(!launched.ephemeral_profile());
    assert_eq!(launched.profile_dir(), profile);
    launched.close().await;
    assert!(profile.join("Local State").is_file());
    // Chrome removed its lock on a clean exit, so the profile can be reused.
    assert!(profile.join("SingletonLock").symlink_metadata().is_err());
}
