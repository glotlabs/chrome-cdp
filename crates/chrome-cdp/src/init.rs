//! `chrome-cdp init e2e` (plan 11, milestone 11.1): the scaffolding is
//! `e2e::init`; this renders what it did.

use e2e::init::{Framework, FrameworkChoice};
use protocol::schema::{InitAction, InitData};
use protocol::{Cmd, ErrorInfo, Output, format};

use crate::cli::{FrameworkArg, InitArgs, InitCommand};

pub(crate) fn run(args: &InitArgs) -> Result<Output, ErrorInfo> {
    let InitCommand::E2e(e2e) = &args.command;
    let choice = match e2e.framework {
        None => FrameworkChoice::Detect,
        Some(FrameworkArg::None) => FrameworkChoice::None,
        Some(FrameworkArg::Vite) => FrameworkChoice::Is(Framework::Vite),
        Some(FrameworkArg::Next) => FrameworkChoice::Is(Framework::Next),
        Some(FrameworkArg::Cra) => FrameworkChoice::Is(Framework::Cra),
        Some(FrameworkArg::Astro) => FrameworkChoice::Is(Framework::Astro),
        Some(FrameworkArg::Sveltekit) => FrameworkChoice::Is(Framework::SvelteKit),
    };
    let dir = e2e.dir.clone().unwrap_or_else(|| ".".into());
    let data = e2e::init::run(&dir, choice)?;
    let text = text(&data);
    Ok(Output::typed(&data, text).quiet(data.dir.clone()))
}

/// A line per file (`created <path>  <why>`), then the next step.
fn text(data: &InitData) -> String {
    let rows: Vec<Vec<String>> = data
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let action = match f.action {
                InitAction::Created => "created",
                InitAction::Appended => "appended",
                InitAction::Kept => "kept",
            };
            let reason = f.reason.clone().unwrap_or_default();
            // The config's line says what serves the app.
            let why = match (i, f.action, &data.web_server) {
                (0, InitAction::Kept, _) | (1.., _, _) => reason,
                (0, _, Some(server)) => format!("web server: {}", server.command),
                (0, _, None) => "no framework in package.json; fill in [e2e.web_server]".into(),
            };
            vec![action.to_owned(), f.path.clone(), why]
        })
        .collect();
    let mut text = format::table(&rows);
    text.push('\n');
    let next = match (&data.base_url, &data.web_server) {
        (Some(_), Some(_)) => format!(
            "next: {} starts the app and runs the example scenario",
            Cmd::new("test")
        ),
        (Some(_), None) => format!(
            "next: set [e2e.web_server] in {} to the command that serves the app",
            data.files[0].path
        ),
        (None, _) => format!(
            "next: {} shows the project's [e2e] keys",
            Cmd::new("help").arg("config")
        ),
    };
    text.push_str(&next);
    text
}

#[cfg(test)]
mod tests {
    use protocol::script::{Item, parse};

    /// Every test of the example scenario is a valid script for `run`.
    #[test]
    fn the_example_scenarios_steps_are_valid_commands() {
        let parsed = parse("example.cdp", e2e::init::SCENARIO);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let mut tests = 0;
        for item in &parsed.script.items {
            let Item::Directive(test) = item else {
                continue;
            };
            assert_eq!(test.name, "test");
            tests += 1;
            let lines: Vec<String> = test
                .body
                .iter()
                .filter_map(|item| match item {
                    Item::Command(line) => Some(format!("{}\n", line.text)),
                    _ => None,
                })
                .collect();
            let steps = crate::script::check(&lines.concat()).unwrap();
            assert!(steps.len() >= 2, "{steps:?}");
        }
        assert_eq!(tests, 1);
    }

    /// `chrome-cdp test` reads the example scenario.
    #[test]
    fn the_example_scenario_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.cdp");
        std::fs::write(&path, e2e::init::SCENARIO).unwrap();
        let scenario =
            e2e::scenario::load(&path, "example.cdp").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(scenario.tests.len(), 1);
    }
}
