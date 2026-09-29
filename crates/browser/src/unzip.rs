//! Unpacking a Chrome for Testing zip, keeping unix modes and symlinks
//! (macOS app bundles need both).

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Component, Path};

use zip::ZipArchive;

/// Extracts the zip at `archive` into `dest`, calling `progress(done,
/// total)` in uncompressed bytes. Entries may not leave `dest`, by path or
/// by symlink, and nothing is written through a symlink an earlier entry
/// made (links chained through links could otherwise point anywhere).
/// Each file's CRC is checked as it is read.
pub(crate) fn extract(
    archive: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("cannot open the download: {e}"))?;
    let mut zip = ZipArchive::new(io::BufReader::new(file))
        .map_err(|e| format!("the download is not a valid zip: {e}"))?;
    let total = (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.size()))
        .sum();
    let mut done = 0;
    progress(done, total);
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("the download is not a valid zip: {e}"))?;
        let name = entry.name().to_owned();
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("the zip has an unsafe path: {name}"))?;
        let path = dest.join(&rel);
        let io_err = |e: io::Error| format!("cannot unpack {name}: {e}");
        if let Some(link) = first_link(dest, &rel).map_err(io_err)? {
            return Err(format!(
                "the zip writes {name} through the symlink {}",
                link.display()
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(&path).map_err(io_err)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        if entry.is_symlink() {
            let mut target = String::new();
            io::Read::read_to_string(&mut entry, &mut target).map_err(io_err)?;
            if !stays_inside(&rel, Path::new(&target)) {
                return Err(format!(
                    "the zip has a symlink that points outside it: {name} -> {target}"
                ));
            }
            symlink(Path::new(&target), &path).map_err(io_err)?;
        } else {
            let mut out = BufWriter::new(File::create(&path).map_err(io_err)?);
            io::copy(&mut entry, &mut out).map_err(io_err)?;
            out.into_inner()
                .map_err(|e| io_err(e.into_error()))?
                .sync_all()
                .map_err(io_err)?;
            set_mode(&path, entry.unix_mode()).map_err(io_err)?;
        }
        done += entry.size();
        progress(done, total);
    }
    Ok(())
}

/// The first path of `dest`/`rel`, from the top, that is a symlink: an
/// entry at `rel` would be written through it. `None` when there is none
/// (a path that does not exist yet has none below it).
fn first_link(dest: &Path, rel: &Path) -> io::Result<Option<std::path::PathBuf>> {
    let mut path = dest.to_path_buf();
    for component in rel.components() {
        path.push(component);
        match path.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(Some(path)),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Whether a symlink at `link` (relative to the root) pointing at `target`
/// resolves inside the root, judged on the paths alone.
fn stays_inside(link: &Path, target: &Path) -> bool {
    let mut depth: Vec<Component<'_>> = Vec::new();
    let base = link.parent().unwrap_or(Path::new(""));
    for component in base.components().chain(target.components()) {
        match component {
            Component::Normal(_) => depth.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::other("symlinks need a unix system"))
}

/// Applies the permission bits of `mode` (no setuid/setgid/sticky); no mode
/// leaves the default.
#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    match mode {
        Some(mode) => fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777)),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use zip::write::SimpleFileOptions;

    use super::{extract, stays_inside};

    fn top_dirs(dest: &Path) -> Vec<std::path::PathBuf> {
        let mut dirs: Vec<_> = std::fs::read_dir(dest)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        dirs.sort();
        dirs
    }

    /// One zip entry: a file with contents and mode, or a symlink.
    pub(crate) enum Entry<'a> {
        File(&'a str, &'a str, u32),
        Link(&'a str, &'a str),
        Dir(&'a str),
    }

    pub(crate) fn write_zip(path: &Path, entries: &[Entry<'_>]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for entry in entries {
            match entry {
                Entry::File(name, body, mode) => {
                    let options = SimpleFileOptions::default().unix_permissions(*mode);
                    zip.start_file(*name, options).unwrap();
                    zip.write_all(body.as_bytes()).unwrap();
                }
                Entry::Link(name, target) => {
                    zip.add_symlink(*name, *target, SimpleFileOptions::default())
                        .unwrap();
                }
                Entry::Dir(name) => {
                    zip.add_directory(*name, SimpleFileOptions::default())
                        .unwrap();
                }
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn keeps_modes_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        write_zip(
            &archive,
            &[
                Entry::Dir("chrome-mac-arm64/"),
                Entry::File(
                    "chrome-mac-arm64/App.app/Contents/Frameworks/F.framework/Versions/A/F",
                    "binary",
                    0o755,
                ),
                Entry::Link(
                    "chrome-mac-arm64/App.app/Contents/Frameworks/F.framework/Versions/Current",
                    "A",
                ),
                Entry::Link(
                    "chrome-mac-arm64/App.app/Contents/Frameworks/F.framework/F",
                    "Versions/Current/F",
                ),
                Entry::File("chrome-mac-arm64/readme", "text", 0o644),
            ],
        );
        let dest = dir.path().join("out");
        let mut calls = Vec::new();
        extract(&archive, &dest, &mut |done, total| {
            calls.push((done, total));
        })
        .unwrap();

        assert_eq!(top_dirs(&dest), [dest.join("chrome-mac-arm64")]);
        let framework = dest.join("chrome-mac-arm64/App.app/Contents/Frameworks/F.framework");
        assert_eq!(
            std::fs::read_to_string(framework.join("F")).unwrap(),
            "binary"
        );
        assert!(framework.join("F").symlink_metadata().unwrap().is_symlink());
        let mode = |p: &Path| p.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&framework.join("Versions/A/F")), 0o755);
        assert_eq!(mode(&dest.join("chrome-mac-arm64/readme")), 0o644);
        assert_eq!(calls.first(), Some(&(0, 29)));
        assert_eq!(calls.last(), Some(&(29, 29)));
    }

    #[test]
    fn refuses_to_leave_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        write_zip(&archive, &[Entry::Link("top/evil", "../../etc/passwd")]);
        let err = extract(&archive, &dir.path().join("out"), &mut |_, _| {}).unwrap_err();
        assert_eq!(
            err,
            "the zip has a symlink that points outside it: top/evil -> ../../etc/passwd"
        );

        write_zip(&archive, &[Entry::File("../evil", "x", 0o644)]);
        let err = extract(&archive, &dir.path().join("out"), &mut |_, _| {}).unwrap_err();
        assert_eq!(err, "the zip has an unsafe path: ../evil");

        // Each link stays inside on its own, but the second is made through
        // the first, so it would point at the destination's parent, and the
        // file would be written there.
        write_zip(
            &archive,
            &[
                Entry::Link("a/b", ".."),
                Entry::Link("a/b/c", ".."),
                Entry::File("a/b/c/escaped", "x", 0o644),
            ],
        );
        let out = dir.path().join("chain");
        let err = extract(&archive, &out, &mut |_, _| {}).unwrap_err();
        assert_eq!(
            err,
            format!(
                "the zip writes a/b/c through the symlink {}",
                out.join("a/b").display()
            )
        );
        assert!(!dir.path().join("escaped").exists());

        // Nor over a link to a file.
        write_zip(
            &archive,
            &[Entry::Link("a/f", "g"), Entry::File("a/./f", "x", 0o644)],
        );
        let out = dir.path().join("over");
        let err = extract(&archive, &out, &mut |_, _| {}).unwrap_err();
        assert!(
            err.starts_with("the zip writes a/./f through the symlink"),
            "{err}"
        );
        assert!(!out.join("a/g").exists());

        std::fs::write(&archive, "not a zip").unwrap();
        let err = extract(&archive, &dir.path().join("out"), &mut |_, _| {}).unwrap_err();
        assert!(err.starts_with("the download is not a valid zip"), "{err}");
    }

    #[test]
    fn symlink_targets() {
        let inside = |link: &str, target: &str| stays_inside(Path::new(link), Path::new(target));
        assert!(inside("a/b/link", "../c"));
        assert!(inside("a/link", "./b/../c"));
        assert!(!inside("a/link", "../../c"));
        assert!(!inside("link", "/etc/passwd"));
    }
}
