//! The files `fmt` and `lint` look at (plan 11, milestone 11.12): `.cdp`
//! files, mock files (the `.toml` files of the mocks directory), and for
//! `lint` audit baselines (the `.json` files of `e2e/audit/` and
//! `.chrome-cdp/audit/`, plan 17).

use std::path::{Path, PathBuf};

use e2e::Config;
use protocol::{ErrorInfo, ErrorKind, Hint};

/// Folders never walked into, besides hidden ones.
const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build"];

/// What a run of `fmt` or `lint` looks at.
#[derive(Debug)]
pub(crate) struct Files {
    /// The project's `[e2e]` config (the defaults, rooted at the current
    /// directory, without one).
    pub(crate) config: Config,
    /// `.cdp` files (absolute), sorted.
    pub(crate) cdp: Vec<PathBuf>,
    /// Mock files (absolute), sorted.
    pub(crate) mocks: Vec<PathBuf>,
    /// Audit baselines (absolute), sorted; `fmt` leaves them alone.
    pub(crate) audits: Vec<PathBuf>,
}

impl Files {
    pub(crate) fn len(&self) -> usize {
        self.cdp.len() + self.mocks.len() + self.audits.len()
    }
}

/// The folders of audit baselines below a project root.
const AUDIT_DIRS: [&str; 2] = ["e2e/audit", ".chrome-cdp/audit"];

/// The files of `paths` (files or folders); without any, the project's:
/// every `.cdp` file below its root and every mock file.
pub(crate) fn find(paths: &[PathBuf]) -> Result<Files, ErrorInfo> {
    let cwd = std::env::current_dir().map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read the current directory: {e}"),
        )
    })?;
    let config = crate::test::plan::project(&cwd)?;
    let mut files = Files {
        cdp: Vec::new(),
        mocks: Vec::new(),
        audits: Vec::new(),
        config,
    };
    let mocks_dir = canonical(&files.config.mocks);
    let artifacts = canonical(&files.config.artifacts);
    let audit_dirs: Vec<PathBuf> = AUDIT_DIRS
        .iter()
        .map(|d| canonical(&files.config.root.join(d)))
        .collect();
    let dirs = Dirs {
        mocks: &mocks_dir,
        audits: &audit_dirs,
    };
    if paths.is_empty() {
        let root = files.config.root.clone();
        walk(&root, &artifacts, &mut |path| files.add(path, &dirs, false));
        // `.chrome-cdp` is hidden, so the walk leaves it out.
        let hidden = root.join(AUDIT_DIRS[1]);
        walk(&hidden, &artifacts, &mut |path| {
            files.add(path, &dirs, false)
        });
        if files.len() == 0 {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!("no .cdp or mock files in {}", files.config.root.display()),
            )
            .hint(Hint::text(
                "name the files or folders, or run `chrome-cdp init e2e` to start a project",
            )));
        }
    }
    for path in paths {
        let path = cwd.join(path);
        if path.is_dir() {
            let before = files.len();
            walk(&path, &artifacts, &mut |p| files.add(p, &dirs, false));
            if files.len() == before {
                return Err(ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("no .cdp or mock files in {}", path.display()),
                )
                .hint(Hint::text(
                    "a folder means its .cdp files, and its .toml files below the mocks directory",
                )));
            }
        } else if path.is_file() {
            if !files.add(path.clone(), &dirs, true) {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!(
                        "{} is not a .cdp, mock (.toml) or audit baseline (.json) file",
                        path.display()
                    ),
                )
                .hint(Hint::text(
                    "name .cdp scenario or step files, mock files or audit baselines",
                )));
            }
        } else {
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!("{} does not exist", path.display()),
            )
            .hint(Hint::text("paths are relative to the current directory")));
        }
    }
    for list in [&mut files.cdp, &mut files.mocks, &mut files.audits] {
        list.sort();
        list.dedup();
    }
    Ok(files)
}

/// Where mock files and audit baselines are.
struct Dirs<'a> {
    mocks: &'a Path,
    audits: &'a [PathBuf],
}

impl Files {
    /// Adds `path` if it is a `.cdp` file, a mock file (a `.toml` file
    /// below the mocks directory, or any `.toml` file when `named`) or an
    /// audit baseline (a `.json` file below an audit directory, or any
    /// when `named`); whether it did.
    fn add(&mut self, path: PathBuf, dirs: &Dirs<'_>, named: bool) -> bool {
        match path.extension().and_then(|e| e.to_str()) {
            Some("cdp") => {
                self.cdp.push(path);
                true
            }
            Some("toml") if named || canonical(&path).starts_with(dirs.mocks) => {
                self.mocks.push(path);
                true
            }
            Some("json")
                if named || dirs.audits.iter().any(|d| canonical(&path).starts_with(d)) =>
            {
                self.audits.push(path);
                true
            }
            _ => false,
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Every file below `dir`, leaving out hidden folders, build output and
/// the artifacts folder.
fn walk(dir: &Path, artifacts: &Path, found: &mut dyn FnMut(PathBuf) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.')
                && !SKIPPED.contains(&name.as_ref())
                && canonical(&path) != artifacts
            {
                walk(&path, artifacts, found);
            }
        } else {
            found(path);
        }
    }
}

/// How `path` is named in text output: relative to the current directory
/// when it is below it.
pub(crate) fn shown(path: &Path) -> String {
    e2e::scenario::display_name(path)
}
