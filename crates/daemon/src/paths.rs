//! Where a session lives (plan 02, milestone 2.4).
//!
//! Each session has a directory holding `sock`, `pid`, `meta.json`,
//! `daemon.log`, `chrome.log` and two lock files:
//!
//! - Linux: `$XDG_RUNTIME_DIR/chrome-cdp/<session>/`, or
//!   `/tmp/chrome-cdp-<uid>/<session>/` without it;
//! - macOS: `$TMPDIR/chrome-cdp/<session>/` (`$TMPDIR` is per user there).
//!
//! A Unix socket path must fit in `sun_path` (104 bytes on macOS, 108 on
//! Linux), and macOS's `$TMPDIR` alone takes about 50. When `<dir>/sock`
//! is too long, the session lives in `/tmp/ccdp-<uid>/<hash>/` instead, the
//! hash taken over the long directory's path so two base dirs never share
//! one. Directories are 0700; shared bases under `/tmp` must belong to us.

use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

/// The longest session name.
pub const MAX_NAME: usize = 64;

/// Room in `sun_path`, including the terminating NUL.
#[cfg(target_os = "macos")]
const SUN_PATH: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH: usize = 108;

/// What the paths depend on, captured so tests can fake it.
#[derive(Debug, Clone)]
pub struct Env {
    /// macOS rules instead of Linux ones.
    pub macos: bool,
    pub uid: u32,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub tmpdir: Option<PathBuf>,
    /// The socket path limit in bytes, NUL included.
    pub sun_path: usize,
}

impl Env {
    /// This process's environment.
    pub fn current() -> Self {
        let abs = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        Self {
            macos: cfg!(target_os = "macos"),
            uid: browser::launch::effective_uid(),
            xdg_runtime_dir: abs("XDG_RUNTIME_DIR"),
            tmpdir: abs("TMPDIR"),
            sun_path: SUN_PATH,
        }
    }

    /// The directory that holds sessions by name.
    pub fn base(&self) -> PathBuf {
        let per_user = if self.macos {
            self.tmpdir.as_ref()
        } else {
            self.xdg_runtime_dir.as_ref()
        };
        match per_user {
            Some(dir) => dir.join("chrome-cdp"),
            None => PathBuf::from(format!("/tmp/chrome-cdp-{}", self.uid)),
        }
    }

    /// The directory for sessions whose normal path is too long.
    pub fn short_base(&self) -> PathBuf {
        PathBuf::from(format!("/tmp/ccdp-{}", self.uid))
    }

    /// Both bases, for listing sessions.
    pub fn bases(&self) -> [PathBuf; 2] {
        [self.base(), self.short_base()]
    }
}

/// Checks a session name: `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`.
pub fn check_name(name: &str) -> Result<(), ErrorInfo> {
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && name.len() <= MAX_NAME;
    if ok {
        return Ok(());
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("`{name}` is not a session name"),
    )
    .hint(Hint::text(format!(
        "use letters, digits, `.`, `_` and `-`, starting with a letter or digit, at most \
             {MAX_NAME} chars, e.g. {}",
        Cmd::new("open").flag("session", "shop")
    ))))
}

/// A session's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    pub name: String,
    /// Where the session would live if the socket path fit: the identity
    /// of the session, recorded in `meta.json`.
    pub home: PathBuf,
    /// Where it lives: `home`, or a short dir under `/tmp/ccdp-<uid>/`.
    pub dir: PathBuf,
    pub sock: PathBuf,
    pub pid: PathBuf,
    pub meta: PathBuf,
    /// Held by the daemon while it runs: its liveness test.
    pub daemon_lock: PathBuf,
    /// Held by a CLI while it starts the daemon.
    pub start_lock: PathBuf,
    pub daemon_log: PathBuf,
    pub chrome_log: PathBuf,
}

impl SessionPaths {
    /// The paths of session `name` (which must pass [`check_name`]).
    pub fn new(env: &Env, name: &str) -> Self {
        let home = env.base().join(name);
        let fits = home.join("sock").as_os_str().len() < env.sun_path;
        let dir = if fits {
            home.clone()
        } else {
            env.short_base().join(format!(
                "{:016x}",
                fnv64(home.as_os_str().as_encoded_bytes())
            ))
        };
        Self::in_dir(name, home, dir)
    }

    fn in_dir(name: &str, home: PathBuf, dir: PathBuf) -> Self {
        Self {
            name: name.to_owned(),
            home,
            sock: dir.join("sock"),
            pid: dir.join("pid"),
            meta: dir.join("meta.json"),
            daemon_lock: dir.join("daemon.lock"),
            start_lock: dir.join("start.lock"),
            daemon_log: dir.join("daemon.log"),
            chrome_log: dir.join("chrome.log"),
            dir,
        }
    }

    /// The paths of a session found in `dir` while listing.
    pub(crate) fn found(name: &str, home: PathBuf, dir: PathBuf) -> Self {
        Self::in_dir(name, home, dir)
    }

    /// Creates the session dir and its base (0700), checking that both are
    /// ours and private: under `/tmp`, another user could have made them.
    pub fn create(&self, env: &Env) -> Result<(), ErrorInfo> {
        let base = self.dir.parent().unwrap_or(Path::new("/"));
        create_private(base, env.uid)
            .and_then(|()| create_private(&self.dir, env.uid))
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::DaemonUnavailable,
                    format!("cannot use the session dir {}: {e}", self.dir.display()),
                )
                .hint(Hint::text(format!(
                    "remove it or fix its owner and mode (0700), or set {}",
                    if env.macos {
                        "TMPDIR"
                    } else {
                        "XDG_RUNTIME_DIR"
                    }
                )))
            })
    }
}

/// Creates `dir` (and its parents) with mode 0700. `dir` must then be a
/// real directory owned by `uid` and closed to others, so another user
/// cannot plant a socket or read our logs.
fn create_private(dir: &Path, uid: u32) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    check_private(dir, uid)
}

/// Checks an existing `dir` as [`SessionPaths::create`] would, without
/// creating anything: a real directory owned by `uid`, closed to others.
pub fn check_private(dir: &Path, uid: u32) -> io::Result<()> {
    let meta = dir.symlink_metadata()?;
    if !meta.is_dir() {
        return Err(io::Error::other("not a directory"));
    }
    if meta.uid() != uid {
        return Err(io::Error::other(format!(
            "owned by uid {}, not {uid}",
            meta.uid()
        )));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "mode {:o} lets other users in",
            meta.permissions().mode() & 0o777
        )));
    }
    Ok(())
}

/// 64-bit FNV-1a.
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use super::{Env, SessionPaths, check_name, create_private};

    fn linux(xdg: Option<&str>) -> Env {
        Env {
            macos: false,
            uid: 1000,
            xdg_runtime_dir: xdg.map(PathBuf::from),
            tmpdir: None,
            sun_path: 108,
        }
    }

    fn mac(tmpdir: &str) -> Env {
        Env {
            macos: true,
            uid: 501,
            xdg_runtime_dir: None,
            tmpdir: Some(PathBuf::from(tmpdir)),
            sun_path: 104,
        }
    }

    #[test]
    fn names() {
        for ok in ["default", "a", "shop-2", "A.b_c", &"x".repeat(64)] {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-a", ".x", "a/b", "a b", "ü", &"x".repeat(65), ".."] {
            let err = check_name(bad).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{bad}");
        }
    }

    #[test]
    fn session_dirs_per_os() {
        let p = SessionPaths::new(&linux(Some("/run/user/1000")), "default");
        assert_eq!(p.dir, Path::new("/run/user/1000/chrome-cdp/default"));
        assert_eq!(p.sock, Path::new("/run/user/1000/chrome-cdp/default/sock"));
        assert_eq!(p.home, p.dir);
        let p = SessionPaths::new(&linux(None), "shop");
        assert_eq!(p.dir, Path::new("/tmp/chrome-cdp-1000/shop"));
        let tmp = "/var/folders/xy/0123456789abcdefghijklmn0000gn/T/";
        let p = SessionPaths::new(&mac(tmp), "default");
        assert_eq!(p.dir, Path::new(tmp).join("chrome-cdp/default"));
        assert_eq!(p.chrome_log, p.dir.join("chrome.log"));
    }

    #[test]
    fn long_socket_paths_are_shortened() {
        let tmp = "/var/folders/xy/0123456789abcdefghijklmn0000gn/T/";
        let env = mac(tmp);
        let name = "a-rather-long-session-name-for-the-checkout-flow-tests-0123456";
        let p = SessionPaths::new(&env, name);
        assert_eq!(p.home, Path::new(tmp).join("chrome-cdp").join(name));
        assert!(p.dir.starts_with("/tmp/ccdp-501/"), "{}", p.dir.display());
        assert!(p.sock.as_os_str().len() < 104);
        assert_eq!(p.name, name);
        // Stable, and different for another base dir or name.
        assert_eq!(SessionPaths::new(&env, name).dir, p.dir);
        assert_ne!(
            SessionPaths::new(&mac("/private/tmp/other/T"), name).dir,
            p.dir
        );
        assert_ne!(SessionPaths::new(&env, &name[1..]).dir, p.dir);
        // Exactly at the limit: `<home>/sock` plus NUL must fit.
        let base = "/run/user/1000/chrome-cdp/";
        let fits = "x".repeat(108 - base.len() - "/sock".len() - 1);
        let env = linux(Some("/run/user/1000"));
        assert_eq!(
            SessionPaths::new(&env, &fits).dir,
            Path::new(base).join(&fits)
        );
        let over = format!("{fits}x");
        assert!(
            SessionPaths::new(&env, &over)
                .dir
                .starts_with("/tmp/ccdp-1000")
        );
    }

    #[test]
    fn private_dirs_are_checked() {
        let uid = browser::launch::effective_uid();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        create_private(&dir, uid).unwrap();
        assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = create_private(&dir, uid).unwrap_err();
        assert_eq!(err.to_string(), "mode 755 lets other users in");
        let err = create_private(&dir, uid + 1).unwrap_err();
        assert!(err.to_string().starts_with("owned by uid"), "{err}");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert_eq!(
            create_private(&link, uid).unwrap_err().to_string(),
            "not a directory"
        );
    }
}
