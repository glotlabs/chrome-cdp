//! Where `screenshot`, `pdf`, `har` and `trace` save their files: the
//! path given with `-o`, or the next number in a directory of the session
//! (`<session dir>/shots/1.png`, `2.png`, …). Numbers go on from the
//! highest file there, so they survive a daemon restart and never
//! overwrite an earlier capture.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use protocol::{ErrorInfo, ErrorKind};

/// How often a numbered name is tried when another capture took it first.
const ATTEMPTS: u32 = 100;

/// Writes `bytes` to `path` (its directories made first), or to the next
/// `<n>.<ext>` in `dir`. Returns the file's path.
pub(crate) fn save(
    bytes: &[u8],
    path: Option<&Path>,
    dir: &Path,
    ext: &str,
) -> Result<PathBuf, ErrorInfo> {
    let (mut file, path) = create(path, dir, ext)?;
    file.write_all(bytes).map_err(|e| failed(&path, &e))?;
    Ok(path)
}

/// Creates the file [`save`] would write, for writing in parts. Returns
/// it and its path.
pub(crate) fn create(
    path: Option<&Path>,
    dir: &Path,
    ext: &str,
) -> Result<(File, PathBuf), ErrorInfo> {
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| failed(parent, &e))?;
        }
        let file = File::create(path).map_err(|e| failed(path, &e))?;
        return Ok((file, path.to_owned()));
    }
    fs::create_dir_all(dir).map_err(|e| failed(dir, &e))?;
    let mut n = highest(dir).map_err(|e| failed(dir, &e))? + 1;
    for _ in 0..ATTEMPTS {
        let path = dir.join(format!("{n}.{ext}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(failed(&path, &e)),
        }
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("no free file name in {}", dir.display()),
    ))
}

/// The highest `<n>` of the `<n>.<ext>` files in `dir` (0 if none).
fn highest(dir: &Path) -> io::Result<u64> {
    let mut highest = 0;
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        let number = name
            .to_str()
            .and_then(|n| n.split_once('.'))
            .and_then(|(stem, _)| stem.parse::<u64>().ok());
        if let Some(number) = number {
            highest = highest.max(number);
        }
    }
    Ok(highest)
}

pub(crate) fn failed(path: &Path, err: &io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("cannot write {}: {err}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::save;

    #[test]
    fn numbers_go_on_from_the_highest_file() {
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("shots");
        let first = save(b"a", None, &shots, "png").unwrap();
        assert_eq!(first, shots.join("1.png"));
        std::fs::write(shots.join("7.jpg"), b"x").unwrap();
        std::fs::write(shots.join("notes.txt"), b"x").unwrap();
        let next = save(b"b", None, &shots, "png").unwrap();
        assert_eq!(next, shots.join("8.png"));
        assert_eq!(std::fs::read(first).unwrap(), b"a");
    }

    #[test]
    fn a_given_path_gets_its_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/shot.png");
        assert_eq!(save(b"png", Some(&path), dir.path(), "png").unwrap(), path);
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
    }
}
