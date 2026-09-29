//! A task's working dir: what the agent starts from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::task::{Copy, Replace, Task, Vars};
use crate::report;

/// Copies, removes and replaces as `task.setup` says, then records the
/// files of `verify.unchanged`.
pub fn prepare(task: &Task, workdir: &Path, vars: &Vars) -> Result<Files, String> {
    copy_all(&task.setup.copy, workdir)?;
    if task.files().is_dir() {
        copy_dir(&task.files(), workdir, &[])?;
    }
    remove_all(&task.setup.remove, workdir)?;
    replace_all(&task.setup.replace, workdir, vars)?;
    Ok(files_under(workdir, &task.verify.unchanged))
}

/// Applies a task's known solution's file changes.
pub fn apply_solution(task: &Task, workdir: &Path, vars: &Vars) -> Result<(), String> {
    let s = &task.solution;
    copy_all(&s.copy, workdir)?;
    remove_all(&s.remove, workdir)?;
    replace_all(&s.replace, workdir, vars)
}

fn copy_all(copies: &[Copy], workdir: &Path) -> Result<(), String> {
    let root = report::workspace_root();
    for c in copies {
        let from = root.join(&c.from);
        let to = workdir.join(&c.to);
        if from.is_dir() {
            copy_dir(&from, &to, &c.exclude)?;
        } else {
            if let Some(dir) = to.parent() {
                mkdir(dir)?;
            }
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

fn mkdir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))
}

/// Copies `from` into `to`, leaving out entries named in `exclude`.
pub fn copy_dir(from: &Path, to: &Path, exclude: &[String]) -> Result<(), String> {
    mkdir(to)?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if exclude.iter().any(|x| name.to_string_lossy() == x.as_str()) {
            continue;
        }
        let target = to.join(&name);
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target, exclude)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn remove_all(paths: &[String], workdir: &Path) -> Result<(), String> {
    for p in paths {
        let path = workdir.join(p);
        let done = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        done.map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(())
}

fn replace_all(replaces: &[Replace], workdir: &Path, vars: &Vars) -> Result<(), String> {
    for r in replaces {
        let path = resolve(workdir, &r.file)?;
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let from = vars.expand(&r.from);
        if !text.contains(&from) {
            return Err(format!("{} does not contain {from:?}", path.display()));
        }
        let text = text.replace(&from, &vars.expand(&r.to));
        std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// `file` under `workdir`; a `*` in its last part must match one file.
pub fn resolve(workdir: &Path, file: &str) -> Result<PathBuf, String> {
    let path = workdir.join(file);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Err(format!("{file}: no file name"));
    };
    let Some((head, tail)) = name.split_once('*') else {
        return Ok(path);
    };
    let dir = path.parent().unwrap_or(workdir);
    let matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.len() >= head.len() + tail.len() && n.starts_with(head) && n.ends_with(tail)
        })
        .map(|e| e.path())
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!("{file} matches {} files, not one", matches.len())),
    }
}

/// Every file under the given paths (relative to `workdir`), with its bytes.
pub type Files = BTreeMap<PathBuf, Vec<u8>>;

pub fn files_under(workdir: &Path, paths: &[String]) -> Files {
    fn walk(path: &Path, out: &mut Files) {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    walk(&entry.path(), out);
                }
            }
        } else if let Ok(bytes) = std::fs::read(path) {
            out.insert(path.to_owned(), bytes);
        }
    }
    let mut out = Files::new();
    for p in paths {
        walk(&workdir.join(p), &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_resolve_to_one_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/index-123.js"), "x").unwrap();
        std::fs::write(dir.path().join("a/index-123.css"), "x").unwrap();
        assert_eq!(
            resolve(dir.path(), "a/index-*.js").unwrap(),
            dir.path().join("a/index-123.js")
        );
        assert!(resolve(dir.path(), "a/index-*").is_err());
        assert_eq!(
            resolve(dir.path(), "a/b.txt").unwrap(),
            dir.path().join("a/b.txt")
        );
    }

    #[test]
    fn replacements_must_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "port 4173").unwrap();
        let mut vars = Vars::default();
        vars.set("port", "5000");
        let r = |from: &str| Replace {
            file: "f".into(),
            from: from.into(),
            to: "port {port}".into(),
        };
        replace_all(&[r("port 4173")], dir.path(), &vars).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f")).unwrap(),
            "port 5000"
        );
        assert!(replace_all(&[r("port 4173")], dir.path(), &vars).is_err());
    }
}
