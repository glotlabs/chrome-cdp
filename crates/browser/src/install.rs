//! `chrome-cdp install` and `uninstall` (plan 02, milestone 2.2): download a
//! Chrome for Testing build into the cache and remove it again.
//!
//! An install goes to `<cache>/browsers/<channel>-<version>-<platform>/`.
//! Full Chrome and `chrome-headless-shell` of one version share that dir,
//! each in its own top directory (`chrome-linux64/`,
//! `chrome-headless-shell-linux64/`), as the Chrome for Testing zips unpack.
//!
//! The steps, all under a lock on `<cache>/install.lock` so parallel agents
//! do not install twice:
//!
//! 1. download the zip into a staging dir (`browsers/.staging-*`) next to
//!    the target, and check its size against `Content-Length`;
//! 2. unpack it there (each file's CRC is checked), and on macOS drop the
//!    `com.apple.quarantine` attribute;
//! 3. move the product's top directory into the install dir with one
//!    `rename`, so it is never seen half-written;
//! 4. write the `INSTALLATION_COMPLETE` marker last.
//!
//! A crash leaves only a staging dir, which the next install removes.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde_json::json;

use crate::cft::{
    self, Channel, ChannelVersion, IndexError, Product, Release, channel_release, parse_channels,
    version_release,
};
use crate::deps::{Distro, chromium_package_line};
use crate::host::{Host, INDEX_ENV, is_executable};
use crate::installed::{COMPLETE_MARKER, cache_dir, installed, platform_key};

/// The lock file in the cache dir.
const LOCK_FILE: &str = "install.lock";

/// The prefix of staging dirs in `browsers/`.
const STAGING_PREFIX: &str = ".staging-";

/// The channel label of a `--version` that no channel is on right now.
pub const PINNED: &str = "pinned";

/// What to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub channel: Channel,
    /// A version or a prefix of one (`154`, `154.0.8037.57`); it replaces
    /// the channel's current version.
    pub version: Option<String>,
    pub product: Product,
    /// Download again even when it is installed.
    pub force: bool,
}

/// What an install is doing, for a progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress<'a> {
    /// Another process holds the install lock.
    Waiting,
    /// `done` of `total` bytes (when the server said) of `release`.
    Downloading {
        release: &'a Release,
        done: u64,
        total: Option<u64>,
    },
    /// `done` of `total` uncompressed bytes.
    Unpacking { done: u64, total: u64 },
}

/// The browser `install` left in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// The install dir's name, `<channel>-<version>-<platform>`.
    pub id: String,
    /// `stable`, `beta`, `dev`, `canary`, or [`PINNED`].
    pub channel: String,
    pub version: String,
    pub platform: String,
    pub product: Product,
    pub dir: PathBuf,
    /// The binary to launch.
    pub executable: PathBuf,
    /// The bytes downloaded, or `None` when it was already installed.
    pub downloaded: Option<u64>,
    /// Non-fatal notes: an offline check, older installs to remove.
    pub warnings: Vec<String>,
}

/// Installing or uninstalling failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("cannot find the cache dir: HOME is not set")]
    NoCache,
    #[error("Chrome for Testing has no build for this machine ({os}-{arch})")]
    NoPlatform {
        os: &'static str,
        arch: &'static str,
        /// The distro's Chromium package command, if known.
        package: Option<&'static str>,
    },
    #[error("{0}")]
    Index(IndexError),
    #[error("cannot download {url}: {error}")]
    Http { url: String, error: String },
    #[error("the download of {url} stalled: no data for {}", format::duration(*after))]
    Stalled { url: String, after: Duration },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("the download of {url} is incomplete: {} of {}", format::size(*got), format::size(*expected))]
    Size {
        url: String,
        expected: u64,
        got: u64,
    },
    #[error("cannot unpack {url}: {error}")]
    Unpack { url: String, error: String },
    #[error("{what}: {error}")]
    Io {
        what: String,
        error: String,
        /// The cache dir, for the hint.
        cache: PathBuf,
    },
    #[error("no browser {id} is installed")]
    NotInstalled { id: String, installed: Vec<String> },
}

impl InstallError {
    /// The user-facing error (plan 08 §3).
    pub fn to_error_info(&self) -> ErrorInfo {
        let message = self.to_string();
        let failed = |hint: Hint| ErrorInfo::new(ErrorKind::InstallFailed, &message).hint(hint);
        let mirror = format!("set {INDEX_ENV} to a mirror");
        let retry = Hint::text("run the command again");
        match self {
            Self::NoCache => failed(Hint::text(format!(
                "set {} to a writable directory",
                crate::host::CACHE_ENV
            ))),
            Self::NoPlatform { package, .. } => failed(no_build_hint(*package)),
            Self::Index(IndexError::UnknownVersion { newest, .. }) => {
                let info = ErrorInfo::new(ErrorKind::BadArgs, &message).hint(
                    Hint::run(Cmd::new("install").switch("list"))
                        .why("shows each channel's current version"),
                );
                match newest {
                    Some(newest) => info.note("newest", newest),
                    None => info,
                }
            }
            Self::Index(IndexError::NoBuild { .. }) => failed(
                Hint::run(Cmd::new("install"))
                    .why("the current stable")
                    .or("pick another --version"),
            ),
            Self::Http { .. } => {
                failed(Hint::text("check the network connection and HTTPS_PROXY").or(mirror))
            }
            Self::Stalled { .. } => ErrorInfo::new(ErrorKind::Timeout, &message)
                .hint("run the command again with a longer --timeout, e.g. --timeout 2m"),
            Self::Status { status: 404, .. } => failed(
                Hint::run(Cmd::new("install").switch("list"))
                    .why("the build may have been withdrawn; this lists the current ones"),
            ),
            Self::Index(_) | Self::Status { .. } => {
                failed(Hint::text("try again later").or(mirror))
            }
            Self::Size { .. } | Self::Unpack { .. } => failed(retry),
            Self::Io { cache, .. } => failed(
                Hint::text(format!(
                    "check free space and permissions in {}",
                    cache.display()
                ))
                .or(format!(
                    "set {} to another directory",
                    crate::host::CACHE_ENV
                )),
            ),
            Self::NotInstalled { installed, .. } => {
                let list = if installed.is_empty() {
                    "none".to_owned()
                } else {
                    installed.join(", ")
                };
                ErrorInfo::new(ErrorKind::BadArgs, &message)
                    .note("installed", list)
                    .hint(Hint::run(Cmd::new("install").switch("list")))
                    .details(json!({ "installed": installed }))
            }
        }
    }
}

/// For a machine or version with no Chrome for Testing build: the distro's
/// Chromium, or `--chrome`.
fn no_build_hint(package: Option<&str>) -> Hint {
    let hint = match package {
        Some(line) => Hint::text(format!("install Chromium: {line}")),
        None => Hint::text("install Chromium with the system package manager"),
    };
    hint.or("pass --chrome /path/to/chrome")
}

impl From<IndexError> for InstallError {
    fn from(e: IndexError) -> Self {
        Self::Index(e)
    }
}

/// The Chrome for Testing platform key of `host`, or why there is none.
fn platform(host: &Host) -> Result<&'static str, InstallError> {
    platform_key(host).ok_or_else(|| InstallError::NoPlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        package: chromium_package_line(Distro::detect(host).as_ref()),
    })
}

/// Downloads and unpacks `request` into the cache, unless it is there
/// already. `timeout` bounds connecting and each wait for data.
pub async fn install(
    host: &Host,
    request: &Request,
    timeout: Duration,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<Installation, InstallError> {
    let platform = platform(host)?;
    let cache = cache_dir(host).ok_or(InstallError::NoCache)?;
    let client = Client::new(host, timeout)?;

    let (release, channel) = match resolve(&client, request, platform).await {
        Ok(found) => found,
        Err(e) if request.version.is_none() && !request.force => {
            // Offline: an installed build of the channel is good enough.
            let best = installed(host).into_iter().find(|i| {
                i.channel == request.channel.as_str() && i.products.contains(&request.product)
            });
            let Some(best) = best else { return Err(e) };
            tracing::warn!("{e}");
            let mut done = existing(
                &best.id,
                &cache,
                request.product,
                &best.channel,
                &best.version,
                platform,
            );
            done.warnings.push(format!(
                "cannot check for a newer {} version ({e}); kept {}",
                request.channel, best.id
            ));
            return Ok(done);
        }
        Err(e) => return Err(e),
    };
    let id = format!("{channel}-{}-{platform}", release.version);
    let browsers = cache.join("browsers");
    let io = |what: String| {
        let cache = cache.clone();
        move |e: io::Error| InstallError::Io {
            what,
            error: e.to_string(),
            cache,
        }
    };
    fs::create_dir_all(&browsers).map_err(io(format!("cannot create {}", browsers.display())))?;
    let _lock = lock(&cache, progress)?;
    remove_stale_staging(&browsers);

    let dir = browsers.join(&id);
    let mut done = existing(
        &id,
        &cache,
        request.product,
        &channel,
        &release.version,
        platform,
    );
    if !request.force && is_complete(&dir, request.product, platform) {
        return Ok(done);
    }

    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&browsers)
        .map_err(io(format!(
            "cannot create a staging dir in {}",
            browsers.display()
        )))?;
    let archive = staging.path().join("download.zip");
    let bytes = client.download(&release, &archive, progress).await?;

    let unpacked = staging.path().join("unpacked");
    let unpack_err = |error: String| InstallError::Unpack {
        url: release.url.clone(),
        error,
    };
    let (zip, dest) = (archive.clone(), unpacked.clone());
    crate::unzip::extract(&zip, &dest, &mut |done, total| {
        progress(Progress::Unpacking { done, total });
    })
    .map_err(unpack_err)?;
    let _ = fs::remove_file(&archive);
    let product_dir = request.product.dir(platform);
    if !is_executable(&unpacked.join(request.product.binary(platform))) {
        return Err(unpack_err(format!(
            "it has no {}",
            request.product.binary(platform)
        )));
    }
    remove_quarantine(&unpacked.join(&product_dir));

    fs::create_dir_all(&dir).map_err(io(format!("cannot create {}", dir.display())))?;
    let target = dir.join(&product_dir);
    if target.exists() {
        fs::rename(&target, staging.path().join("old"))
            .map_err(io(format!("cannot replace {}", target.display())))?;
    }
    fs::rename(unpacked.join(&product_dir), &target).map_err(io(format!(
        "cannot move the browser to {}",
        target.display()
    )))?;
    write_marker(&dir).map_err(io(format!(
        "cannot write {}",
        dir.join(COMPLETE_MARKER).display()
    )))?;
    drop(staging);

    done.downloaded = Some(bytes);
    done.warnings.extend(
        installed(host)
            .into_iter()
            .filter(|i| i.channel == channel && i.id != id)
            .map(|i| {
                format!(
                    "{} is still installed; remove it with: {}",
                    i.id,
                    Cmd::new("uninstall").arg(&i.id)
                )
            }),
    );
    Ok(done)
}

/// The [`Installation`] for `id` as it is (or will be) on disk.
fn existing(
    id: &str,
    cache: &Path,
    product: Product,
    channel: &str,
    version: &str,
    platform: &str,
) -> Installation {
    let dir = cache.join("browsers").join(id);
    Installation {
        id: id.to_owned(),
        channel: channel.to_owned(),
        version: version.to_owned(),
        platform: platform.to_owned(),
        product,
        executable: dir.join(product.binary(platform)),
        dir,
        downloaded: None,
        warnings: Vec::new(),
    }
}

fn is_complete(dir: &Path, product: Product, platform: &str) -> bool {
    dir.join(COMPLETE_MARKER).is_file() && is_executable(&dir.join(product.binary(platform)))
}

fn write_marker(dir: &Path) -> io::Result<()> {
    let mut marker = File::create(dir.join(COMPLETE_MARKER))?;
    marker.write_all(b"")?;
    marker.sync_all()
}

/// The release to download and its channel label: the channel's, or for a
/// `--version`, the channel it is current on (else [`PINNED`]).
async fn resolve(
    client: &Client,
    request: &Request,
    platform: &str,
) -> Result<(Release, String), InstallError> {
    let Some(wanted) = &request.version else {
        let channels = client.channels().await?;
        let release = channel_release(&channels, request.channel, request.product, platform)?;
        return Ok((release, request.channel.as_str().to_owned()));
    };
    let json = client.get_text(&client.index_url(cft::KNOWN_GOOD)).await?;
    let release = version_release(&json, wanted, request.product, platform)?;
    // Only a label: without it the install is still right.
    let channel = client
        .channels()
        .await
        .ok()
        .and_then(|channels| {
            channels
                .into_iter()
                .find(|c| c.version == release.version)
                .map(|c| c.channel.as_str().to_owned())
        })
        .unwrap_or_else(|| PINNED.to_owned());
    Ok((release, channel))
}

/// The current version of each channel.
pub async fn channels(host: &Host, timeout: Duration) -> Result<Vec<ChannelVersion>, InstallError> {
    Client::new(host, timeout)?.channels().await
}

/// Removes the install `id` (the whole dir, both products) and returns its
/// path.
pub fn uninstall(host: &Host, id: &str) -> Result<PathBuf, InstallError> {
    let cache = cache_dir(host).ok_or(InstallError::NoCache)?;
    let browsers = cache.join("browsers");
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && Path::new(id).components().count() == 1
        && !id.contains(['/', '\\']);
    let dir = browsers.join(id);
    if !plain || !dir.is_dir() {
        return Err(InstallError::NotInstalled {
            id: id.to_owned(),
            installed: install_ids(&browsers),
        });
    }
    let _lock = lock(&cache, &mut |_| {})?;
    // Drop the marker first so a half-deleted dir is never used.
    let _ = fs::remove_file(dir.join(COMPLETE_MARKER));
    fs::remove_dir_all(&dir).map_err(|e| InstallError::Io {
        what: format!("cannot remove {}", dir.display()),
        error: e.to_string(),
        cache: cache.clone(),
    })?;
    Ok(dir)
}

/// Every install dir name, for "installed:" notes (any platform, complete
/// or not).
fn install_ids(browsers: &Path) -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(browsers)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    ids.sort();
    ids
}

/// Takes the cache's install lock, reporting [`Progress::Waiting`] if
/// another process holds it. The OS releases it when the file closes, also
/// on a crash.
fn lock(cache: &Path, progress: &mut dyn FnMut(Progress<'_>)) -> Result<File, InstallError> {
    let path = cache.join(LOCK_FILE);
    let io = |e: io::Error| InstallError::Io {
        what: format!("cannot lock {}", path.display()),
        error: e.to_string(),
        cache: cache.to_owned(),
    };
    fs::create_dir_all(cache).map_err(io)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            progress(Progress::Waiting);
            file.lock().map_err(io)?;
        }
        Err(fs::TryLockError::Error(e)) => return Err(io(e)),
    }
    Ok(file)
}

/// Removes staging dirs a crashed install left (the caller holds the lock,
/// so none is in use).
fn remove_stale_staging(browsers: &Path) {
    for entry in fs::read_dir(browsers).into_iter().flatten().flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            tracing::debug!(dir = %entry.path().display(), "removing a stale staging dir");
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Removes `com.apple.quarantine` from every file under `dir`, so
/// Gatekeeper does not block the unsigned test build.
fn remove_quarantine(dir: &Path) {
    if cfg!(target_os = "macos") {
        remove_attribute(dir, "com.apple.quarantine");
    }
}

/// Removes the extended attribute `name` from `dir` and everything under
/// it, without following symlinks (their targets are inside `dir`).
#[cfg(unix)]
fn remove_attribute(dir: &Path, name: &str) {
    let mut stack = vec![dir.to_owned()];
    while let Some(path) = stack.pop() {
        let _ = xattr::remove(&path, name);
        if path.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            stack.extend(
                fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path()),
            );
        }
    }
}

#[cfg(not(unix))]
fn remove_attribute(_dir: &Path, _name: &str) {}

/// The HTTP client for the index and the downloads.
struct Client {
    http: reqwest::Client,
    index: String,
    timeout: Duration,
}

impl Client {
    fn new(host: &Host, timeout: Duration) -> Result<Self, InstallError> {
        // reqwest is built without a default provider (no aws-lc).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .user_agent(concat!("chrome-cdp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| InstallError::Http {
                url: cft::DEFAULT_INDEX_URL.into(),
                error: error_chain(&e),
            })?;
        let index = host
            .var(INDEX_ENV)
            .map_or(cft::DEFAULT_INDEX_URL.into(), |v| {
                v.to_string_lossy().into_owned()
            });
        Ok(Self {
            http,
            index: index.trim_end_matches('/').to_owned(),
            timeout,
        })
    }

    fn index_url(&self, file: &str) -> String {
        format!("{}/{file}", self.index)
    }

    async fn channels(&self) -> Result<Vec<ChannelVersion>, InstallError> {
        let json = self.get_text(&self.index_url(cft::LAST_KNOWN_GOOD)).await?;
        Ok(parse_channels(&json)?)
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, InstallError> {
        tracing::debug!(url, "GET");
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| self.error(url, &e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(InstallError::Status {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        Ok(response)
    }

    async fn get_text(&self, url: &str) -> Result<String, InstallError> {
        self.get(url)
            .await?
            .text()
            .await
            .map_err(|e| self.error(url, &e))
    }

    /// Streams `release` into `path` and checks its size; returns the bytes
    /// written.
    async fn download(
        &self,
        release: &Release,
        path: &Path,
        progress: &mut dyn FnMut(Progress<'_>),
    ) -> Result<u64, InstallError> {
        let url = &release.url;
        let mut response = self.get(url).await?;
        let total = response.content_length();
        let io = |e: io::Error| InstallError::Unpack {
            url: url.clone(),
            error: format!("cannot write {}: {e}", path.display()),
        };
        let mut file = io::BufWriter::new(File::create(path).map_err(io)?);
        let mut done = 0;
        progress(Progress::Downloading {
            release,
            done,
            total,
        });
        while let Some(chunk) = response.chunk().await.map_err(|e| self.error(url, &e))? {
            file.write_all(&chunk).map_err(io)?;
            done += chunk.len() as u64;
            progress(Progress::Downloading {
                release,
                done,
                total,
            });
        }
        file.into_inner()
            .map_err(|e| io(e.into_error()))?
            .sync_all()
            .map_err(io)?;
        match total {
            Some(expected) if expected != done => Err(InstallError::Size {
                url: url.clone(),
                expected,
                got: done,
            }),
            _ => Ok(done),
        }
    }

    fn error(&self, url: &str, e: &reqwest::Error) -> InstallError {
        if e.is_timeout() {
            InstallError::Stalled {
                url: url.to_owned(),
                after: self.timeout,
            }
        } else {
            InstallError::Http {
                url: url.to_owned(),
                error: error_chain(e),
            }
        }
    }
}

/// `error sending request: client error (Connect): tcp connect error:
/// Connection refused` — reqwest's own message says little on its own.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        source = s.source();
    }
    // Drop reqwest's "for url (…)": the message names the URL already.
    parts
        .iter()
        .map(|p| match p.find(" for url (") {
            Some(at) => &p[..at],
            None => p,
        })
        .collect::<Vec<_>>()
        .join(": ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use protocol::ErrorKind;

    use super::{InstallError, Progress, Request, install, uninstall};
    use crate::cft::{Channel, Product};
    use crate::host::{Arch, Host, Os};
    use crate::installed::installed;
    use crate::testing::{CHANNELS, FakeCft, Response};

    fn host(cache: &Path, server: &FakeCft) -> Host {
        Host::fake(Os::Linux, Arch::X86_64, cache)
            .with_var("CHROME_CDP_CACHE", cache)
            .with_var("CHROME_CDP_CFT_URL", &server.url)
    }

    fn request(channel: Channel, product: Product) -> Request {
        Request {
            channel,
            version: None,
            product,
            force: false,
        }
    }

    /// Runs `install` and returns its result and the progress it reported
    /// (downloads as `d`, unpacking as `u`, waiting as `w`, deduplicated).
    fn run(host: &Host, request: &Request) -> (Result<super::Installation, InstallError>, String) {
        let mut seen = String::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(install(host, request, Duration::from_secs(10), &mut |p| {
            let c = match p {
                Progress::Waiting => 'w',
                Progress::Downloading { .. } => 'd',
                Progress::Unpacking { .. } => 'u',
            };
            if !seen.ends_with(c) {
                seen.push(c);
            }
        }));
        (result, seen)
    }

    fn zips(server: &FakeCft) -> usize {
        server
            .requests()
            .iter()
            .filter(|p| p.contains("/linux64/"))
            .count()
    }

    #[test]
    fn installs_then_is_a_no_op() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);

        let (done, progress) = run(&host, &request(Channel::Stable, Product::Chrome));
        let done = done.unwrap();
        assert_eq!(progress, "du");
        assert_eq!(done.id, "stable-154.0.8037.57-linux64");
        assert_eq!(done.channel, "stable");
        assert!(done.downloaded.unwrap() > 0);
        assert!(done.warnings.is_empty(), "{:?}", done.warnings);
        assert_eq!(
            done.executable,
            dir.path()
                .join("browsers/stable-154.0.8037.57-linux64/chrome-linux64/chrome")
        );
        let link = done.dir.join("chrome-linux64/libfake-link.so");
        assert!(link.symlink_metadata().unwrap().is_symlink());
        let found = installed(&host);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].products, [Product::Chrome]);
        // Nothing but the install dir and the lock is left behind.
        let mut names: Vec<_> = std::fs::read_dir(dir.path().join("browsers"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["stable-154.0.8037.57-linux64"]);
        assert!(dir.path().join("install.lock").is_file());

        let (again, progress) = run(&host, &request(Channel::Stable, Product::Chrome));
        assert_eq!(again.unwrap().downloaded, None);
        assert_eq!(progress, "");
        assert_eq!(zips(&server), 1);

        let forced = Request {
            force: true,
            ..request(Channel::Stable, Product::Chrome)
        };
        assert!(run(&host, &forced).0.unwrap().downloaded.is_some());
        assert_eq!(zips(&server), 2);
        assert_eq!(installed(&host).len(), 1);
    }

    #[test]
    fn headless_shell_shares_the_version_dir() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);
        run(&host, &request(Channel::Stable, Product::HeadlessShell))
            .0
            .unwrap();
        let only_shell = installed(&host);
        assert!(only_shell[0].headless_shell);
        run(&host, &request(Channel::Stable, Product::Chrome))
            .0
            .unwrap();
        let both = installed(&host);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].products, [Product::Chrome, Product::HeadlessShell]);
        assert!(!both[0].headless_shell);
    }

    #[test]
    fn versions_are_labelled_by_channel_or_pinned() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);
        let version = |v: &str| Request {
            version: Some(v.into()),
            ..request(Channel::Stable, Product::Chrome)
        };
        // 155 is beta's current version.
        assert_eq!(
            run(&host, &version("155")).0.unwrap().id,
            "beta-155.0.8059.12-linux64"
        );
        let pinned = run(&host, &version("154.0.8037.3")).0.unwrap();
        assert_eq!(pinned.id, "pinned-154.0.8037.3-linux64");
        assert_eq!(installed(&host).len(), 2);

        let err = run(&host, &version("99")).0.unwrap_err();
        let info = err.to_error_info();
        assert_eq!(info.kind, ErrorKind::BadArgs);
        assert_eq!(
            info.to_text(),
            "error: Chrome for Testing has no version 99\n  newest: 156.0.8075.0\n  hint: \
             chrome-cdp install --list   (shows each channel's current version)\n"
        );
    }

    #[test]
    fn a_new_version_notes_the_old_one() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);
        crate::tests::install(
            dir.path(),
            "stable-153.0.8001.0-linux64",
            "chrome-linux64/chrome",
        );
        let done = run(&host, &request(Channel::Stable, Product::Chrome))
            .0
            .unwrap();
        assert_eq!(
            done.warnings,
            [
                "stable-153.0.8001.0-linux64 is still installed; remove it with: chrome-cdp \
                 uninstall stable-153.0.8001.0-linux64"
            ]
        );
    }

    #[test]
    fn download_failures() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);
        let zip = FakeCft::zip_path(CHANNELS[1].1, Product::Chrome, "linux64");

        let mut short = Response::ok(b"PK".to_vec());
        short.claimed_len = Some(10_000);
        server.route(&zip, short);
        let err = run(&host, &request(Channel::Beta, Product::Chrome))
            .0
            .unwrap_err();
        // A connection closed early is either a size mismatch or a body
        // error, depending on when hyper notices.
        assert!(
            matches!(err, InstallError::Size { .. } | InstallError::Http { .. }),
            "{err:?}"
        );
        assert_eq!(err.to_error_info().kind, ErrorKind::InstallFailed);

        server.route(&zip, Response::ok(b"not a zip".to_vec()));
        let err = run(&host, &request(Channel::Beta, Product::Chrome))
            .0
            .unwrap_err();
        assert!(matches!(err, InstallError::Unpack { .. }), "{err:?}");

        server.route(
            &zip,
            Response {
                status: 404,
                body: Vec::new(),
                claimed_len: None,
            },
        );
        let err = run(&host, &request(Channel::Beta, Product::Chrome))
            .0
            .unwrap_err();
        assert_eq!(
            err.to_error_info().to_text(),
            format!(
                "error: {}{zip} answered HTTP 404\n  hint: chrome-cdp install --list   (the \
                 build may have been withdrawn; this lists the current ones)\n",
                server.url
            )
        );
        // Failed installs leave nothing behind, not even staging dirs.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("browsers"))
            .unwrap()
            .collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(installed(&host).is_empty());
    }

    #[test]
    fn offline_keeps_what_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let down = Host::fake(Os::Linux, Arch::X86_64, dir.path())
            .with_var("CHROME_CDP_CACHE", dir.path())
            // Nothing listens on port 9 (discard) on test machines.
            .with_var("CHROME_CDP_CFT_URL", "http://127.0.0.1:9");
        let err = run(&down, &request(Channel::Stable, Product::Chrome))
            .0
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("cannot download http://127.0.0.1:9/last-known-good"),
            "{err}"
        );
        assert!(err.to_string().contains("onnection refused"), "{err}");

        crate::tests::install(
            dir.path(),
            "stable-153.0.8001.0-linux64",
            "chrome-linux64/chrome",
        );
        let kept = run(&down, &request(Channel::Stable, Product::Chrome))
            .0
            .unwrap();
        assert_eq!(kept.id, "stable-153.0.8001.0-linux64");
        assert_eq!(kept.downloaded, None);
        assert!(
            kept.warnings[0]
                .starts_with("cannot check for a newer stable version (cannot download"),
            "{:?}",
            kept.warnings
        );
    }

    #[test]
    fn waits_for_another_install() {
        let server = FakeCft::start("linux64");
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), &server);
        std::fs::create_dir_all(dir.path()).unwrap();
        let held = std::fs::File::create(dir.path().join("install.lock")).unwrap();
        held.lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let (done, progress) = run(&host, &request(Channel::Stable, Product::Chrome));
        release.join().unwrap();
        assert!(done.unwrap().downloaded.is_some());
        assert_eq!(progress, "wdu");
    }

    #[test]
    fn no_build_for_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        std::fs::write(dir.path().join("etc/os-release"), "ID=fedora\n").unwrap();
        let host =
            Host::fake(Os::Linux, Arch::Other, dir.path()).with_var("CHROME_CDP_CACHE", dir.path());
        let err = run(&host, &request(Channel::Stable, Product::Chrome))
            .0
            .unwrap_err();
        let text = err.to_error_info().to_text();
        assert!(
            text.ends_with(
                "\n  hint: install Chromium: sudo dnf install -y chromium, or pass --chrome \
                 /path/to/chrome\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn removes_an_attribute_from_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/b/file");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
        std::os::unix::fs::symlink("file", dir.path().join("a/b/link")).unwrap();
        // macOS's attribute cannot be set on Linux; a user one can.
        let name = if cfg!(target_os = "macos") {
            "com.apple.quarantine"
        } else {
            "user.chrome-cdp-test"
        };
        if xattr::set(&file, name, b"1").is_err() {
            eprintln!("skipping: this filesystem has no extended attributes");
            return;
        }
        xattr::set(dir.path().join("a"), name, b"1").unwrap();
        super::remove_attribute(dir.path(), name);
        assert_eq!(xattr::get(&file, name).unwrap(), None);
        assert_eq!(xattr::get(dir.path().join("a"), name).unwrap(), None);
    }

    #[test]
    fn uninstalls() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::fake(Os::Linux, Arch::X86_64, dir.path())
            .with_var("CHROME_CDP_CACHE", dir.path());
        crate::tests::install(
            dir.path(),
            "stable-1.0.0.0-linux64",
            "chrome-linux64/chrome",
        );
        crate::tests::install(dir.path(), "beta-2.0.0.0-linux64", "chrome-linux64/chrome");

        for bad in [
            "nope",
            "",
            "..",
            ".staging-x",
            "stable-1.0.0.0-linux64/chrome-linux64",
        ] {
            let err = uninstall(&host, bad).unwrap_err();
            assert!(matches!(err, InstallError::NotInstalled { .. }), "{bad}");
        }
        assert_eq!(
            uninstall(&host, "nope")
                .unwrap_err()
                .to_error_info()
                .to_text(),
            "error: no browser nope is installed\n  installed: beta-2.0.0.0-linux64, \
             stable-1.0.0.0-linux64\n  hint: chrome-cdp install --list\n"
        );
        let removed = uninstall(&host, "stable-1.0.0.0-linux64").unwrap();
        assert!(!removed.exists());
        let ids: Vec<_> = installed(&host).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["beta-2.0.0.0-linux64"]);
    }
}
