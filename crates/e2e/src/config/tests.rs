use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Config, DEFAULT_TIMEOUT, DEFAULT_VIEWPORT, FreezeTime, Jobs, PROJECT_FILES, RandomSeed,
    Viewport, find_project_file, line_col,
};

fn file() -> PathBuf {
    PathBuf::from("/p/chrome-cdp.toml")
}

fn parse(text: &str) -> Config {
    Config::parse(text, &file())
        .unwrap_or_else(|e| panic!("{}", e.message))
        .expect("an [e2e] section")
}

fn error(text: &str) -> String {
    Config::parse(text, &file()).unwrap_err().message
}

/// The plan's example, as written.
const EXAMPLE: &str = r#"
session = "shop"

[e2e]
base_url = "http://localhost:5173"
tests = ["e2e/scenarios/**/*.cdp"]
mocks = "e2e/mocks"
default_mocks = ["base"]         # applied to every test unless it says otherwise
jobs = 4                         # "auto" = number of CPUs / 2
retries = 0
timeout = "30s"                  # per test
artifacts = "e2e/.results"
viewport = "1280x800"

[e2e.web_server]
command = "npm run dev -- --port 5173"
url = "http://localhost:5173"    # ready when this returns < 500
reuse_existing = true
timeout = "60s"
env = { VITE_API_URL = "/api" }

[e2e.determinism]
freeze_time = "2026-01-01T10:00:00Z"
random_seed = 42
disable_animations = true

[e2e.backends.local]
mode = "real"
web_server = { command = "docker compose up api web", url = "http://localhost:8080", timeout = "120s" }
base_url = "http://localhost:8080"
setup = ["./scripts/e2e-reset-db.sh"]
vars = "e2e/vars/local.toml"

[e2e.backends.staging]
mode = "real"
base_url = "https://staging.example.com"
vars = "e2e/vars/staging.toml"
jobs = 2
determinism = { freeze_time = false }
"#;

#[test]
fn parses_the_plan_example() {
    let c = parse(EXAMPLE);
    assert_eq!(c.root, Path::new("/p"));
    assert_eq!(c.base_url.as_deref(), Some("http://localhost:5173"));
    assert_eq!(c.tests, ["e2e/scenarios/**/*.cdp"]);
    assert_eq!(c.mocks, Path::new("/p/e2e/mocks"));
    assert_eq!(c.default_mocks, ["base"]);
    assert_eq!(c.default_backend, "mock");
    assert_eq!(c.jobs.count(), 4);
    assert_eq!(c.timeout, Duration::from_secs(30));
    assert_eq!(c.artifacts, Path::new("/p/e2e/.results"));
    assert_eq!(
        c.viewport,
        Viewport {
            width: 1280,
            height: 800
        }
    );
    let [server] = c.web_servers.as_slice() else {
        panic!("{:?}", c.web_servers)
    };
    assert_eq!(server.command, "npm run dev -- --port 5173");
    assert_eq!(server.timeout, Duration::from_secs(60));
    assert!(server.reuse_existing);
    assert_eq!(server.env["VITE_API_URL"], "/api");
    assert_eq!(server.cwd, Path::new("/p"));
    assert_eq!(
        c.determinism.freeze_time,
        Some(FreezeTime::At("2026-01-01T10:00:00Z".into()))
    );
    assert_eq!(c.determinism.random_seed, Some(RandomSeed::Seed(42)));
    assert_eq!(c.determinism.disable_animations, Some(true));

    let local = &c.backends["local"];
    let servers = local.web_servers.as_deref().unwrap();
    assert_eq!(servers[0].timeout, Duration::from_secs(120));
    assert_eq!(local.setup, ["./scripts/e2e-reset-db.sh"]);
    assert_eq!(
        local.vars.as_deref(),
        Some(Path::new("/p/e2e/vars/local.toml"))
    );
    let staging = &c.backends["staging"];
    assert!(staging.web_servers.is_none());
    assert_eq!(staging.jobs, Some(Jobs::Count(2.try_into().unwrap())));
    assert_eq!(staging.determinism.freeze_time, Some(FreezeTime::Off));
    assert!(!staging.protected);
}

#[test]
fn defaults_fill_an_empty_section() {
    let c = parse("[e2e]\n");
    assert_eq!(c.tests, [super::DEFAULT_TESTS]);
    assert_eq!(c.jobs, Jobs::Auto);
    assert!(c.jobs.count() >= 1);
    assert_eq!(c.retries, 0);
    assert!(!c.allow_console_errors);
    assert!(parse("[e2e]\nallow_console_errors = true\n").allow_console_errors);
    assert!(c.watch.is_empty());
    assert_eq!(parse("[e2e]\nwatch = [\"dist/**\"]\n").watch, ["dist/**"]);
    assert_eq!(c.timeout, DEFAULT_TIMEOUT);
    assert_eq!(c.viewport, DEFAULT_VIEWPORT);
    assert!(c.web_servers.is_empty() && c.default_mocks.is_empty());
    assert_eq!(c.determinism, super::Determinism::default());
    assert_eq!(
        c.determinism.resolve(),
        super::Fixed {
            freeze_time: Some("2026-01-01T10:00:00Z".into()),
            random_seed: Some(0),
            disable_animations: true,
            locale: Some("en-US".into()),
            timezone: Some("UTC".into()),
        }
    );
}

#[test]
fn determinism_is_on_unless_turned_off() {
    let c = parse(
        "[e2e.determinism]\nfreeze_time = false\nrandom_seed = false\n\
         disable_animations = false\nlocale = false\ntimezone = \"Europe/Oslo\"\n",
    );
    let fixed = c.determinism.resolve();
    assert_eq!(
        fixed,
        super::Fixed {
            freeze_time: None,
            random_seed: None,
            disable_animations: false,
            locale: None,
            timezone: Some("Europe/Oslo".into()),
        }
    );
    // A backend's settings go on top of [e2e]'s.
    let c = parse(EXAMPLE);
    let staging = c.determinism.with(&c.backends["staging"].determinism);
    assert_eq!(staging.freeze_time, Some(FreezeTime::Off));
    assert_eq!(staging.random_seed, Some(RandomSeed::Seed(42)));
    assert_eq!(staging.resolve().locale.as_deref(), Some("en-US"));
}

#[test]
fn a_file_without_the_section_is_none() {
    assert_eq!(
        Config::parse("session = \"x\"\n", &file()).map(|c| c.is_none()),
        Ok(true)
    );
}

#[test]
fn several_web_servers_keep_their_order() {
    let c = parse(
        "[e2e]\n\
         [[e2e.web_server]]\ncommand = \"npm run dev\"\nurl = \"http://localhost:5173\"\n\
         [[e2e.web_server]]\ncommand = \"./assets.sh\"\nurl = \"http://localhost:9000/health\"\n\
         cwd = \"static\"\nreuse_existing = false\n",
    );
    let commands: Vec<&str> = c.web_servers.iter().map(|w| w.command.as_str()).collect();
    assert_eq!(commands, ["npm run dev", "./assets.sh"]);
    assert_eq!(c.web_servers[1].cwd, Path::new("/p/static"));
    assert!(!c.web_servers[1].reuse_existing);
}

#[test]
fn errors_point_at_the_line_and_column() {
    assert_eq!(
        error("[e2e]\ntimeout = \"30\"\n"),
        "/p/chrome-cdp.toml:2:11: e2e.timeout: `30` needs a unit; use e.g. 30s or 30ms"
    );
    let typo = error("[e2e]\nbase_url = \"http://x\"\nretires = 1\n");
    assert!(
        typo.starts_with("/p/chrome-cdp.toml:3:1: unknown field `retires`, expected one of"),
        "{typo}"
    );
    assert_eq!(
        error("[e2e.web_server]\ncommand = \"x\"\nurl = \"localhost:5173\"\n"),
        "/p/chrome-cdp.toml:3:7: e2e.web_server.url: `localhost:5173` is not an absolute \
         http(s) URL, e.g. http://localhost:5173"
    );
    let missing = error("[e2e.web_server]\ncommand = \"x\"\n");
    assert!(missing.contains("missing field `url`"), "{missing}");
    let server_typo = error("[[e2e.web_server]]\ncommand = \"x\"\nurl = \"http://a\"\nport = 1\n");
    assert!(
        server_typo.starts_with("/p/chrome-cdp.toml:4:1: unknown field `port`"),
        "{server_typo}"
    );
}

#[test]
fn rejects_bad_values() {
    let jobs = error("[e2e]\njobs = 0\n");
    assert!(
        jobs.contains("a number of jobs above 0, or \"auto\""),
        "{jobs}"
    );
    assert!(error("[e2e]\njobs = \"many\"\n").contains("\"auto\""));
    assert_eq!(
        error("[e2e]\nviewport = \"1280\"\n"),
        "/p/chrome-cdp.toml:2:12: e2e.viewport: `1280` is not WIDTHxHEIGHT, e.g. 1280x800"
    );
    assert!(
        error("[e2e]\ndefault_mocks = [\"base.toml\"]\n")
            .ends_with("e2e.default_mocks: name the mock set `base`, without .toml")
    );
    assert!(error("[e2e]\ndefault_mocks = [\"../x\"]\n").contains("leaves the mocks directory"));
    assert!(error("[e2e]\ntests = []\n").ends_with("e2e.tests: name at least one glob"));
    assert!(
        error("[e2e.determinism]\nfreeze_time = true\n")
            .contains("a time such as \"2026-01-01T10:00:00Z\", or false")
    );
    assert!(error("[e2e.determinism]\nrandom_seed = -1\n").contains("a seed"));
    assert!(
        error("[e2e.determinism]\nfreeze_time = \"tomorrow\"\n")
            .contains("`tomorrow` is not a time; use ISO 8601")
    );
    assert!(error("[e2e.determinism]\nlocale = true\n").contains("or false for the browser's own"));
    assert!(error("[e2e.determinism]\ntimezone = \"\"\n").contains("such as \"en-US\""));
    assert!(error("[e2e.web_server]\ncommand = \" \"\nurl = \"http://a\"\n").ends_with("empty"));
}

#[test]
fn backends_are_checked() {
    assert!(
        error("[e2e.backends.mock]\nmode = \"real\"\n").contains("`mock` is the built-in backend")
    );
    assert!(
        error("[e2e.backends.local]\nbase_url = \"http://a\"\n").contains("set mode = \"real\"")
    );
    assert!(
        error("[e2e.backends.local]\nmode = \"mock\"\n")
            .contains("e2e.backends.local.mode: `mock` is not \"real\"")
    );
    assert!(
        error("[e2e.backends.\"my env\"]\nmode = \"real\"\n")
            .contains("`my env` is not a backend name")
    );
    let unknown =
        error("[e2e]\ndefault_backend = \"stage\"\n[e2e.backends.staging]\nmode = \"real\"\n");
    assert!(
        unknown
            .ends_with("e2e.default_backend: no backend `stage`; the backends are mock, staging"),
        "{unknown}"
    );
    let c = parse(
        "[e2e]\ndefault_backend = \"staging\"\n[e2e.backends.staging]\nmode = \"real\"\nprotected = true\nmocks = [\"third-party/stripe\"]\n",
    );
    assert_eq!(c.default_backend, "staging");
    assert!(c.backends["staging"].protected);
    assert_eq!(c.backends["staging"].mocks, ["third-party/stripe"]);
}

#[test]
fn a_syntax_error_has_a_position_too() {
    let e = error("[e2e]\nbase_url = \n");
    assert!(e.starts_with("/p/chrome-cdp.toml:2:"), "{e}");
}

#[test]
fn line_and_column_count_characters() {
    assert_eq!(line_col("ab\ncd", 0), (1, 1));
    assert_eq!(line_col("ab\ncd", 4), (2, 2));
    assert_eq!(line_col("é\nx", 3), (2, 1));
    assert_eq!(line_col("x", 99), (1, 2));
}

#[test]
fn the_project_file_is_the_nearest_one_of_either_name() {
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("a/b");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(find_project_file(&deep), Ok(None));
    std::fs::write(dir.path().join(PROJECT_FILES[1]), "").unwrap();
    assert_eq!(
        find_project_file(&deep),
        Ok(Some(dir.path().join(".chrome-cdp.toml")))
    );
    std::fs::write(dir.path().join("a/chrome-cdp.toml"), "").unwrap();
    assert_eq!(
        find_project_file(&deep),
        Ok(Some(dir.path().join("a/chrome-cdp.toml")))
    );
    std::fs::write(dir.path().join("a/.chrome-cdp.toml"), "").unwrap();
    let both = find_project_file(&deep).unwrap_err();
    assert!(
        both.message
            .ends_with("has both chrome-cdp.toml and .chrome-cdp.toml")
    );
}

#[test]
fn find_says_how_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let none = Config::find(dir.path()).unwrap_err();
    assert_eq!(
        none.hint.as_deref(),
        Some("chrome-cdp init e2e   (writes one)")
    );
    std::fs::write(dir.path().join("chrome-cdp.toml"), "session = \"x\"\n").unwrap();
    let no_section = Config::find(dir.path()).unwrap_err();
    assert!(no_section.message.ends_with("has no [e2e] section"));
    std::fs::write(
        dir.path().join("chrome-cdp.toml"),
        "[e2e]\nbase_url = \"http://localhost:3000\"\n",
    )
    .unwrap();
    let c = Config::find(&dir.path().join(".")).unwrap();
    assert_eq!(c.base_url.as_deref(), Some("http://localhost:3000"));
}
