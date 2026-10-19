//! `chrome-cdp init load` (plan 23.3): an example load profile, with the
//! project's real backend and some of its tests as the journeys, every
//! key there is explained, so a person or an LLM can make it the run they
//! want.

use std::fmt::Write as _;
use std::path::Path;

use e2e::Config;
use e2e::scenario::select;
use protocol::schema::{InitAction, InitData, InitFile};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output, format};

use super::profile;
use crate::cli::InitLoadArgs;

/// How many of the project's tests the example takes as journeys.
const JOURNEYS: usize = 3;

pub(crate) fn run(args: &InitLoadArgs) -> Result<Output, ErrorInfo> {
    let dir = std::path::absolute(args.dir.as_deref().unwrap_or(Path::new("."))).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot resolve the directory: {e}"),
        )
    })?;
    let config = crate::test::plan::project(&dir)?;
    let name = &args.name;
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--name {name}: a profile's name is a file name without .toml, e.g. shop"),
        ));
    }
    let path = config.root.join(profile::DIR).join(format!("{name}.toml"));
    let (backend, has_backend) = backend(&config);
    let journeys = journeys(&config);
    let file = if path.exists() {
        InitFile {
            path: path.display().to_string(),
            action: InitAction::Kept,
            reason: Some("exists".into()),
        }
    } else {
        let text = example(&backend, has_backend, &journeys);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, text));
        written.map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot write {}: {e}", path.display()),
            )
        })?;
        InitFile {
            path: path.display().to_string(),
            action: InitAction::Created,
            reason: Some(format!(
                "{}, backend {backend}",
                match journeys.len() {
                    0 => "an example journey".to_owned(),
                    1 => "1 journey".to_owned(),
                    n => format!("{n} journeys"),
                }
            )),
        }
    };
    let data = InitData {
        dir: config.root.display().to_string(),
        framework: None,
        package_manager: None,
        base_url: None,
        web_server: None,
        files: vec![file],
    };
    let mut text = format::table(&[vec![
        match data.files[0].action {
            InitAction::Kept => "kept",
            _ => "created",
        }
        .to_owned(),
        data.files[0].path.clone(),
        data.files[0].reason.clone().unwrap_or_default(),
    ]]);
    let _ = write!(
        text,
        "\nnext: {} says what it would run; {} runs it",
        Cmd::new("load").arg(name).switch("plan"),
        Cmd::new("load").arg(name)
    );
    if !has_backend {
        let _ = write!(
            text,
            "\n      (add a real backend [e2e.backends.{backend}] to {} first)",
            config.file.display()
        );
    }
    Ok(Output::typed(&data, text).quiet(data.files[0].path.clone()))
}

/// The backend to load: the default one if it is real, else the first
/// real one; `staging` (to be added) when there is none.
fn backend(config: &Config) -> (String, bool) {
    let real = |name: &str| {
        config
            .backend(name)
            .is_ok_and(|b| b.mode == e2e::backend::Mode::Real)
    };
    if real(&config.default_backend) {
        return (config.default_backend.clone(), true);
    }
    match config.backends.keys().find(|name| real(name)) {
        Some(name) => (name.clone(), true),
        None => ("staging".to_owned(), false),
    }
}

/// Up to [`JOURNEYS`] of the project's tests, as `file:test name`.
fn journeys(config: &Config) -> Vec<String> {
    let mut found = Vec::new();
    for file in select::files_matching(&config.root, &config.tests) {
        let key = crate::test::durations::key(&config.root, &file);
        let Ok(scenario) = e2e::scenario::load(&file, &key) else {
            continue;
        };
        for test in scenario.tests.iter().filter(|t| t.settings.skip.is_none()) {
            found.push(format!("{key}:{}", test.name));
            if found.len() == JOURNEYS {
                return found;
            }
        }
    }
    found
}

/// The weights of `n` journeys: the first most.
fn weights(n: usize) -> &'static [u32] {
    match n {
        0 | 1 => &[100],
        2 => &[70, 30],
        _ => &[60, 30, 10],
    }
}

/// The example profile's text.
fn example(backend: &str, has_backend: bool, journeys: &[String]) -> String {
    let mut text = String::from(
        "# A load profile (docs/e2e/load.md#profiles). `chrome-cdp load NAME --plan`\n\
         # says what it would run; `chrome-cdp load NAME` runs it. Paths are relative\n\
         # to the project root.\n\n",
    );
    let _ = write!(text, "backend = \"{backend}\"");
    text.push_str(if has_backend {
        "   # a real backend of the config; --backend picks another\n"
    } else {
        "   # add it to the config: [e2e.backends.staging] with mode = \"real\"\n"
    });
    text.push_str(
        "\nthink = \"2s..8s\"           # what a bare `think` step waits: \"3s\", \"2s..8s\" or \"exp(4s)\"\n\
         # think_between_steps = true  # a think after every action the journey did not pause after\n\
         # on_failure = \"stop-run\"     # end the run once max_failures iterations failed\n\
         # max_failures = 100\n",
    );
    let example = format!(
        "{}:{}",
        e2e::init::SCENARIO_PATH,
        example_test().unwrap_or_default()
    );
    let tests: Vec<&str> = if journeys.is_empty() {
        vec![example.as_str()]
    } else {
        journeys.iter().map(String::as_str).collect()
    };
    for (test, weight) in tests.iter().zip(weights(tests.len())) {
        let _ = write!(
            text,
            "\n[[journeys]]\ntest = \"{}\"   # a scenario file and a test's name (or line)\nweight = {weight}\n",
            test.replace('\\', "\\\\").replace('"', "\\\"")
        );
    }
    text.push_str(
        "# A journey can take a row of data per iteration, as ${row.email}:\n\
         # data = \"e2e/load/users.csv\"   # a CSV with a header line, or a JSON array of objects\n\
         # data_order = \"sequential\"     # or \"random\"\n\
         # data_exhausted = \"wrap\"       # or \"stop\": each row once, then the run ends\n\
         \n\
         # The open model: users arrive at a rate whether or not the site keeps up,\n\
         # as on a real site; each does one iteration.\n\
         [arrivals]\n\
         stages = [\n\
         \x20 { rate = \"0..2/s\", duration = \"1m\" },    # ramp up\n\
         \x20 { rate = \"2/s\", duration = \"5m\" },       # hold\n\
         \x20 { rate = \"2..0/s\", duration = \"30s\" },   # ramp down\n\
         ]\n\
         max_users = 50   # the most at once; an arrival above it is dropped and counted\n\
         \n\
         # Or, instead of [arrivals], the closed model: a number of users, each\n\
         # looping the journeys. A slow server then slows the arrivals too.\n\
         # [users]\n\
         # count = 20\n\
         # ramp_up = \"30s\"\n\
         # duration = \"5m\"\n",
    );
    text
}

/// The name of `init e2e`'s example test.
fn example_test() -> Option<String> {
    let parsed = protocol::script::parse("example.cdp", e2e::init::SCENARIO);
    parsed.script.items.iter().find_map(|item| match item {
        protocol::script::Item::Directive(d) if d.name == "test" => d.args.first()?.literal(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{example, example_test};
    use crate::load::profile::{self, Model};

    #[test]
    fn the_example_is_a_profile() {
        for journeys in [
            vec![],
            vec!["e2e/a.cdp:one".to_owned()],
            vec![
                "e2e/a.cdp:one".into(),
                "e2e/a.cdp:two \"quoted\"".into(),
                "e2e/b.cdp:3".into(),
            ],
        ] {
            let text = example("local", true, &journeys);
            let p = profile::parse(&text, "example.toml", Path::new("/p"))
                .unwrap_or_else(|e| panic!("{e:?}\n{text}"));
            assert_eq!(p.backend.as_deref(), Some("local"));
            assert_eq!(p.journeys.len(), journeys.len().max(1));
            assert!(matches!(p.model, Model::Open(_)));
            assert_eq!(p.think.to_string(), "2s..8s");
        }
        let text = example("staging", false, &[]);
        assert!(text.contains(&format!(
            "test = \"e2e/scenarios/example.cdp:{}\"",
            example_test().unwrap()
        )));
        // The closed model in the comments parses too, instead of
        // [arrivals].
        let users = "[users]\ncount = 20\nramp_up = \"30s\"\nduration = \"5m\"\n";
        let commented = users.lines().fold(String::new(), |mut text, line| {
            text.push_str("# ");
            text.push_str(line);
            text.push('\n');
            text
        });
        assert!(text.contains(&commented), "{text}");
        let closed = format!("{}{users}", &text[..text.find("[arrivals]").unwrap()]);
        let p = profile::parse(&closed, "example.toml", Path::new("/p"))
            .unwrap_or_else(|e| panic!("{e:?}\n{closed}"));
        assert!(matches!(p.model, Model::Closed(_)));
    }
}
