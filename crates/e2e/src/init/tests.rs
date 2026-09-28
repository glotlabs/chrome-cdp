use std::path::Path;

use protocol::schema::{InitAction, InitData};

use super::{
    DATA_PATH, Framework, FrameworkChoice, MOCKS_PATH, PackageManager, SCENARIO_PATH,
    WORKFLOW_PATH, detect, read_package, run,
};
use crate::Config;

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn package(deps: &str) -> String {
    format!(
        r#"{{"name": "app", "scripts": {{"dev": "x", "start": "y"}}, "devDependencies": {{{deps}}}}}"#
    )
}

fn detected(files: &[(&str, &str)]) -> (Option<Framework>, Option<PackageManager>) {
    let dir = project(files);
    let package = read_package(dir.path()).unwrap();
    let p = detect(dir.path(), package.as_ref(), FrameworkChoice::Detect);
    (p.framework, p.package_manager)
}

fn actions(data: &InitData) -> Vec<(String, InitAction)> {
    let root = format!("{}/", data.dir);
    data.files
        .iter()
        .map(|f| (f.path.strip_prefix(&root).unwrap().to_owned(), f.action))
        .collect()
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap()
}

#[test]
fn detects_the_framework_from_the_dependencies() {
    use Framework::{Astro, Cra, Next, SvelteKit, Vite};
    for (deps, framework) in [
        (r#""vite": "^6""#, Some(Vite)),
        (r#""next": "15""#, Some(Next)),
        (r#""react-scripts": "5""#, Some(Cra)),
        (r#""astro": "5", "vite": "6""#, Some(Astro)),
        (r#""@sveltejs/kit": "2", "vite": "6""#, Some(SvelteKit)),
        (r#""left-pad": "1""#, None),
    ] {
        let found = detected(&[("package.json", &package(deps))]).0;
        assert_eq!(found, framework, "{deps}");
    }
    assert_eq!(detected(&[]), (None, None));
}

#[test]
fn detects_the_package_manager() {
    let json = package(r#""vite": "6""#);
    let with = |files: &[(&str, &str)]| {
        let mut all = vec![("package.json", json.as_str())];
        all.extend_from_slice(files);
        detected(&all).1
    };
    assert_eq!(with(&[]), Some(PackageManager::Npm));
    assert_eq!(with(&[("pnpm-lock.yaml", "")]), Some(PackageManager::Pnpm));
    assert_eq!(with(&[("yarn.lock", "")]), Some(PackageManager::Yarn));
    assert_eq!(with(&[("bun.lockb", "")]), Some(PackageManager::Bun));
    let declared = r#"{"packageManager": "pnpm@9.1.0", "dependencies": {"vite": "6"}}"#;
    assert_eq!(
        detected(&[("package.json", declared), ("package-lock.json", "{}")]).1,
        Some(PackageManager::Pnpm)
    );
    // pnpm's setup action takes the version from packageManager, or needs one.
    let dir = project(&[("package.json", declared)]);
    run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert!(!read(dir.path(), WORKFLOW_PATH).contains("version: latest"));
    let dir = project(&[("package.json", &json), ("pnpm-lock.yaml", "")]);
    run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert!(read(dir.path(), WORKFLOW_PATH).contains("          version: latest\n"));
}

#[test]
fn the_web_server_command_fits_the_framework_and_package_manager() {
    let command = |framework, pm: PackageManager| {
        let s = super::web_server(framework, pm, None);
        (s.command, s.url)
    };
    assert_eq!(
        command(Framework::Vite, PackageManager::Npm),
        (
            "npm run dev -- --port 5173 --strictPort".into(),
            "http://localhost:5173".into()
        )
    );
    assert_eq!(
        command(Framework::Next, PackageManager::Pnpm).0,
        "pnpm run dev --port 3000"
    );
    assert_eq!(
        command(Framework::Astro, PackageManager::Yarn),
        (
            "yarn dev --port 4321".into(),
            "http://localhost:4321".into()
        )
    );
    assert_eq!(
        command(Framework::SvelteKit, PackageManager::Bun).0,
        "bun run dev --port 5173 --strictPort"
    );
    assert_eq!(command(Framework::Cra, PackageManager::Npm).0, "npm start");
    assert_eq!(
        command(Framework::Cra, PackageManager::Yarn).0,
        "yarn start"
    );
}

#[test]
fn scaffolds_a_vite_project() {
    let dir = project(&[("package.json", &package(r#""vite": "6""#))]);
    let data = run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert_eq!(data.framework.as_deref(), Some("vite"));
    assert_eq!(data.package_manager.as_deref(), Some("npm"));
    assert_eq!(data.base_url.as_deref(), Some("http://localhost:5173"));
    assert_eq!(
        actions(&data),
        [
            ("chrome-cdp.toml".into(), InitAction::Created),
            (SCENARIO_PATH.into(), InitAction::Created),
            (MOCKS_PATH.into(), InitAction::Created),
            (DATA_PATH.into(), InitAction::Created),
            (WORKFLOW_PATH.into(), InitAction::Created),
            (".gitignore".into(), InitAction::Created),
        ]
    );

    // The config is valid and says what init printed.
    let config = Config::find(dir.path()).unwrap();
    assert_eq!(config.base_url.as_deref(), Some("http://localhost:5173"));
    assert_eq!(config.default_mocks, ["base"]);
    assert_eq!(
        config.web_servers[0].command,
        "npm run dev -- --port 5173 --strictPort"
    );
    assert!(config.mocks.join("base.toml").is_file());
    // A Vite app renders in the browser: no mock_server to suggest.
    assert!(!read(dir.path(), "chrome-cdp.toml").contains("mock_server"));
    // The mock set and its data parse.
    let mocks: toml::Table = toml::from_str(&read(dir.path(), MOCKS_PATH)).unwrap();
    assert!(mocks["mock"].as_array().is_some_and(|m| m.len() == 2));
    let data_file: serde_json::Value = serde_json::from_str(&read(dir.path(), DATA_PATH)).unwrap();
    assert_eq!(data_file.as_array().map(Vec::len), Some(2));

    let workflow = read(dir.path(), WORKFLOW_PATH);
    assert!(workflow.contains("      - run: npm ci\n"), "{workflow}");
    assert!(workflow.contains("chrome-cdp test --reporter text,junit --shard ${{ matrix.shard }}"));
    // fmt and lint run before the tests (11.12).
    let checks = workflow.find("chrome-cdp fmt --check\n          chrome-cdp lint --format github");
    assert!(checks.is_some_and(|at| at < workflow.find("chrome-cdp test").unwrap()), "{workflow}");
    assert!(workflow.contains("path: e2e/.results/"));
    assert_eq!(read(dir.path(), ".gitignore"), "e2e/.results/\n");
}

#[test]
fn never_overwrites_and_a_second_run_keeps_everything() {
    let dir = project(&[
        ("package.json", &package(r#""next": "15""#)),
        (SCENARIO_PATH, "@test \"mine\"\n  goto /\n"),
    ]);
    let first = run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert_eq!(first.files[1].action, InitAction::Kept);
    assert_eq!(first.files[1].reason.as_deref(), Some("exists"));
    assert_eq!(
        read(dir.path(), SCENARIO_PATH),
        "@test \"mine\"\n  goto /\n"
    );

    let before: Vec<String> = first
        .files
        .iter()
        .map(|f| std::fs::read_to_string(&f.path).unwrap())
        .collect();
    let second = run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert!(second.files.iter().all(|f| f.action == InitAction::Kept));
    assert_eq!(
        second.files[0].reason.as_deref(),
        Some("has an [e2e] section")
    );
    assert_eq!(
        second.files[5].reason.as_deref(),
        Some("ignores e2e/.results/ already")
    );
    assert_eq!(second.base_url, None);
    let after: Vec<String> = second
        .files
        .iter()
        .map(|f| std::fs::read_to_string(&f.path).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn ssr_frameworks_get_a_mock_server_suggested() {
    let dir = project(&[("package.json", &package(r#""next": "15""#))]);
    run(dir.path(), FrameworkChoice::Detect).unwrap();
    let text = read(dir.path(), "chrome-cdp.toml");
    assert!(text.contains("# mock_server = { port = 4010 }\n"), "{text}");
    // Uncommented, it is a valid config.
    let enabled = text
        .replace("# mock_server = ", "mock_server = ")
        .replace("# env = { API_URL", "env = { API_URL");
    let config = Config::parse(&enabled, &dir.path().join("chrome-cdp.toml"))
        .unwrap()
        .unwrap();
    assert_eq!(
        config.web_servers[0].mock_server.as_ref().unwrap().port,
        4010
    );
    assert_eq!(
        config.web_servers[0].env["API_URL"],
        "http://localhost:4010"
    );
}

#[test]
fn appends_to_an_existing_config_and_gitignore() {
    let dir = project(&[
        (".chrome-cdp.toml", "session = \"shop\" # mine"),
        (".gitignore", "node_modules/\ndist"),
    ]);
    let data = run(dir.path(), FrameworkChoice::Is(Framework::Cra)).unwrap();
    assert_eq!(data.files[0].action, InitAction::Appended);
    assert!(data.files[0].path.ends_with("/.chrome-cdp.toml"));
    let text = read(dir.path(), ".chrome-cdp.toml");
    assert!(
        text.starts_with("session = \"shop\" # mine\n\n# E2E tests"),
        "{text}"
    );
    assert!(text.contains("command = \"npm start\"\n"), "{text}");
    assert!(text.contains("env = { PORT = \"3000\" }\n"), "{text}");
    assert!(!text.contains("mock_server"), "{text}");
    let config = Config::find(dir.path()).unwrap();
    assert_eq!(config.web_servers[0].env["PORT"], "3000");
    assert_eq!(
        read(dir.path(), ".gitignore"),
        "node_modules/\ndist\ne2e/.results/\n"
    );
    assert_eq!(data.files[5].action, InitAction::Appended);
}

#[test]
fn without_a_framework_the_web_server_is_left_to_fill_in() {
    let dir = project(&[]);
    let data = run(dir.path(), FrameworkChoice::Detect).unwrap();
    assert_eq!((data.framework, data.package_manager), (None, None));
    assert!(data.web_server.is_none());
    let config = Config::find(dir.path()).unwrap();
    assert!(config.web_servers.is_empty());
    assert_eq!(config.base_url.as_deref(), Some("http://localhost:3000"));
    assert!(read(dir.path(), "chrome-cdp.toml").contains("# [e2e.web_server]\n"));
    let workflow = read(dir.path(), WORKFLOW_PATH);
    assert!(!workflow.contains("setup-node"), "{workflow}");

    // --framework none skips a package.json's framework.
    let dir = project(&[("package.json", &package(r#""vite": "6""#))]);
    let data = run(dir.path(), FrameworkChoice::None).unwrap();
    assert_eq!(data.framework, None);
    assert_eq!(data.package_manager.as_deref(), Some("npm"));
    assert!(read(dir.path(), WORKFLOW_PATH).contains("npm ci"));
}

#[test]
fn refuses_what_it_cannot_add_to() {
    let dir = project(&[("chrome-cdp.toml", "session = ")]);
    let e = run(dir.path(), FrameworkChoice::Detect).unwrap_err();
    assert!(
        e.message.contains("chrome-cdp.toml: it is not valid TOML"),
        "{}",
        e.message
    );
    // Nothing was written.
    assert!(!dir.path().join("e2e").exists());

    let dir = project(&[("package.json", "{\"dependencies\": ")]);
    let e = run(dir.path(), FrameworkChoice::Detect).unwrap_err();
    assert!(e.message.contains("package.json:1:"), "{}", e.message);
    assert!(run(dir.path(), FrameworkChoice::Is(Framework::Vite)).is_err());

    let dir = project(&[("chrome-cdp.toml", ""), (".chrome-cdp.toml", "")]);
    let e = run(dir.path(), FrameworkChoice::Detect).unwrap_err();
    assert!(
        e.message
            .ends_with("has both chrome-cdp.toml and .chrome-cdp.toml")
    );

    let e = run(&dir.path().join("missing"), FrameworkChoice::Detect).unwrap_err();
    assert!(
        e.message.ends_with("/missing is not a directory"),
        "{}",
        e.message
    );
}

/// The mock set `init e2e` writes loads, and answers as its comments say.
#[test]
fn the_mock_set_written_is_a_valid_mock_set() {
    let dir = project(&[]);
    run(dir.path(), FrameworkChoice::None).unwrap();
    let config = Config::find(dir.path()).unwrap();
    assert_eq!(config.default_mocks, ["base"]);
    let source = mock::Source::find(&config.mocks, dir.path(), "base").unwrap();
    let set = mock::MockSet::load(&source).unwrap_or_else(|e| panic!("{}", e.message));
    let mut mocks = mock::Mocks::new(0);
    mocks.apply(vec![set], false);
    let explain = |url: &str| {
        mocks.explain(&mock::Request {
            method: "GET",
            url,
            headers: &[],
            body: None,
        })
    };
    let listed = explain("http://localhost:5173/api/items");
    assert_eq!(listed.answer.unwrap().name, "list items");
    assert_eq!(listed.outcome, "fulfill 200 json_file data/items.json");
    let one = explain("http://localhost:5173/api/items/7");
    assert_eq!(one.answer.unwrap().name, "item by id");
    let other = explain("http://localhost:5173/api/users");
    assert_eq!(other.scope.as_deref(), Some("base"));
}

/// The collection the mock set shows in a comment works once uncommented
/// in place of the two mocks.
#[test]
fn the_commented_collection_works() {
    let dir = project(&[]);
    run(dir.path(), FrameworkChoice::None).unwrap();
    let config = Config::find(dir.path()).unwrap();
    let file = config.mocks.join("base.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let (settings, rest) = text.split_once("[[mock]]").unwrap();
    let collection: String =
        rest.lines()
            .skip_while(|l| *l != "# [[collection]]")
            .fold(String::new(), |mut out, l| {
                out.push_str(l.trim_start_matches("# "));
                out.push('\n');
                out
            });
    assert!(
        collection.contains("seed = \"data/items.json\""),
        "{collection}"
    );
    std::fs::write(&file, format!("{settings}{collection}")).unwrap();
    let source = mock::Source::find(&config.mocks, dir.path(), "base").unwrap();
    let set = mock::MockSet::load(&source).unwrap_or_else(|e| panic!("{}", e.message));
    let mut mocks = mock::Mocks::new(0);
    mocks.apply(vec![set], false);
    let Some(mock::StateItem::Collection { items, .. }) = mocks.lookup("items") else {
        panic!("no collection items");
    };
    assert_eq!(items.len(), 2);
}

/// What `init e2e` writes is formatted as `chrome-cdp fmt` would.
#[test]
fn the_example_files_are_formatted() {
    let example = include_str!("example.cdp");
    let formatted = crate::scenario::format::format("example.cdp", example, &crate::scenario::format::Plain);
    assert_eq!(formatted.as_deref(), Ok(example));
    let mocks = include_str!("base.toml");
    assert_eq!(mock::format::format(mocks).as_deref(), Ok(mocks));
}
