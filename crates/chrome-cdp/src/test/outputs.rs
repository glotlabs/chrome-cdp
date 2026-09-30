//! Where the files a test's steps write go (plan 18, milestone 18.1): a
//! relative output path (`state save auth.json`, `screenshot -o a.png`,
//! `pdf -o`, `har stop -o`, `trace stop -o`, `clipboard read -o`,
//! `network show -o`, `explore -o`) resolves in the test's artifacts
//! folder, so parallel tests do not overwrite each other's files and a
//! run leaves nothing in the project. Steps run in-process, so the paths
//! are rewritten rather than the current directory changed.
//!
//! `state load FILE` reads a relative FILE from where an earlier step
//! wrote it: the step's artifacts folder, else one of `reads` or their
//! subfolders, newest first (the test's folder and its `try-N` and
//! `run-N` folders, where an earlier try's `@after` wrote it; the file's
//! `@before-all` folder, whose steps hand state to the tests), else from
//! where it always did.

use std::path::{Path, PathBuf};

use crate::cli::{
    ClipboardCommand, Command, HarCommand, NetworkCommand, StateCommand, TraceCommand,
};

/// Rewrites the relative output paths of `cli` into `dir`, creating the
/// folders they name; a relative `state load` file is looked for in `dir`,
/// then in `reads`.
pub(super) fn resolve(cli: &mut crate::cli::Cli, dir: &Path, reads: &[PathBuf]) {
    let output: Option<&mut PathBuf> = match &mut cli.command {
        Some(Command::Screenshot(args)) => args.output.as_mut(),
        Some(Command::Pdf(args)) => args.output.as_mut(),
        Some(Command::Explore(args)) => args.output.as_mut(),
        Some(Command::Har(args)) => match &mut args.command {
            HarCommand::Stop(args) => args.output.as_mut(),
            HarCommand::Start(_) => None,
        },
        Some(Command::Trace(args)) => match &mut args.command {
            TraceCommand::Stop(args) => args.output.as_mut(),
            TraceCommand::Start(_) => None,
        },
        Some(Command::Clipboard(args)) => match &mut args.command {
            ClipboardCommand::Read(args) => args.output.as_mut(),
            ClipboardCommand::Write(_) => None,
        },
        Some(Command::Network(args)) => match &mut args.command {
            Some(NetworkCommand::Show(args)) => args.output.as_mut(),
            _ => None,
        },
        Some(Command::State(args)) => match &mut args.command {
            StateCommand::Save(args) => Some(&mut args.file),
            StateCommand::Load(args) => {
                if let Some(written) = written(dir, reads, &args.file) {
                    args.file = written;
                }
                None
            }
        },
        _ => None,
    };
    let Some(path) = output else { return };
    let Some(resolved) = within(dir, path) else {
        return;
    };
    if let Some(parent) = resolved.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("cannot create {}: {e}", parent.display());
    }
    *path = resolved;
}

/// Relative file `file` as an earlier step wrote it: in `dir`, else in
/// one of `reads` or their subfolders (the newest).
fn written(dir: &Path, reads: &[PathBuf], file: &Path) -> Option<PathBuf> {
    if let Some(p) = within(dir, file).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for read in reads {
        let subfolders = std::fs::read_dir(read)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir());
        for folder in std::iter::once(read.clone()).chain(subfolders) {
            if let Some(p) = within(&folder, file)
                && let Ok(modified) = p.metadata().and_then(|m| m.modified())
                && p.is_file()
            {
                found.push((modified, p));
            }
        }
    }
    found
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

/// `path` in `dir`, when it is relative (and not `-`, stdin or stdout).
fn within(dir: &Path, path: &Path) -> Option<PathBuf> {
    (path.is_relative() && path != Path::new("-")).then(|| dir.join(path))
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::Cli;

    fn resolved(line: &str, dir: &Path, reads: &[PathBuf]) -> Cli {
        let mut cli = Cli::parse_from(std::iter::once("chrome-cdp").chain(line.split(' ')));
        resolve(&mut cli, dir, reads);
        cli
    }

    #[test]
    fn relative_outputs_go_to_the_artifacts_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let cli = resolved("screenshot -o shots/a.png", dir, &[]);
        let Some(Command::Screenshot(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.output.unwrap(), dir.join("shots/a.png"));
        assert!(dir.join("shots").is_dir());

        let cli = resolved("state save auth.json", dir, &[]);
        let Some(Command::State(args)) = cli.command else {
            panic!()
        };
        let StateCommand::Save(args) = args.command else {
            panic!()
        };
        assert_eq!(args.file, dir.join("auth.json"));

        // Absolute paths and stdout stay.
        let cli = resolved("pdf -o /tmp/x.pdf", dir, &[]);
        let Some(Command::Pdf(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.output.unwrap(), Path::new("/tmp/x.pdf"));
        let cli = resolved("clipboard read -o -", dir, &[]);
        let Some(Command::Clipboard(args)) = cli.command else {
            panic!()
        };
        let ClipboardCommand::Read(args) = args.command else {
            panic!()
        };
        assert_eq!(args.output.unwrap(), Path::new("-"));
    }

    #[test]
    fn state_load_reads_what_the_test_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("test");
        let hooks = tmp.path().join("@before-all");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&hooks).unwrap();
        let load = |line: &str| {
            let cli = resolved(line, &dir, std::slice::from_ref(&hooks));
            let Some(Command::State(args)) = cli.command else {
                panic!()
            };
            let StateCommand::Load(args) = args.command else {
                panic!()
            };
            args.file
        };
        assert_eq!(load("state load auth.json"), Path::new("auth.json"));
        std::fs::write(hooks.join("auth.json"), "{}").unwrap();
        assert_eq!(load("state load auth.json"), hooks.join("auth.json"));
        std::fs::write(dir.join("auth.json"), "{}").unwrap();
        assert_eq!(load("state load auth.json"), dir.join("auth.json"));
        // An earlier run's folder, below one of the reads.
        std::fs::create_dir_all(hooks.join("run-1")).unwrap();
        std::fs::write(hooks.join("run-1/marker.json"), "{}").unwrap();
        assert_eq!(
            load("state load marker.json"),
            hooks.join("run-1/marker.json")
        );
    }
}
