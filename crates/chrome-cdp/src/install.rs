//! `chrome-cdp install` and `uninstall` (plan 02, milestone 2.2), and
//! `--auto-install` for commands that need a Chrome.
//!
//! The download itself lives in `browser::install`; this module adds the
//! progress bar, the Linux check that the browser starts (and
//! `--with-deps`, which installs missing system libraries with sudo), and
//! the output.

use std::fmt::Write as _;
use std::io::{BufRead as _, IsTerminal as _};
use std::os::fd::AsFd as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use browser::{
    CannotRun, DepsFix, DiscoverError, Host, Installation, Os, Product, Remedy, Request,
};
use protocol::schema::{
    AvailableBrowser, InstallData, InstallList, InstallListEntry, InstalledBrowser, UninstallData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::json;

use crate::cli::{Channel, InstallArgs, UninstallArgs};
use crate::progress::Bar;

/// Runs `install`.
pub(crate) fn install(args: &InstallArgs, timeout: Duration) -> Result<Output, ErrorInfo> {
    let host = Host::current();
    if args.list {
        return list(&host, timeout);
    }
    let request = Request {
        channel: args.channel.to_browser(),
        version: args.version.clone(),
        product: if args.headless_shell {
            Product::HeadlessShell
        } else {
            Product::Chrome
        },
        force: args.force,
    };
    let done = download(&host, &request, timeout)?;
    let again = again(args);
    let packages = ensure_runs(&host, &done, args.with_deps, &again)?;
    let mut output = installed_output(&done, &packages);
    if args.with_deps && host.os != Os::Linux {
        output = output.warning("--with-deps only installs Linux packages; nothing to do here");
    }
    Ok(output)
}

/// Runs `uninstall`.
pub(crate) fn uninstall(args: &UninstallArgs) -> Result<Output, ErrorInfo> {
    let dir = browser::uninstall(&Host::current(), &args.id).map_err(|e| e.to_error_info())?;
    let dir = dir.display().to_string();
    let text = format!("removed {} ({dir})", args.id);
    let data = UninstallData {
        id: args.id.clone(),
        dir: dir.clone(),
    };
    Ok(Output::typed(&data, text).quiet(dir))
}

/// Finds the Chrome to launch. With `auto_install`, a machine with none
/// gets the current stable Chrome for Testing first.
pub(crate) fn find_chrome(
    explicit: Option<&Path>,
    auto_install: bool,
    timeout: Duration,
) -> Result<browser::Chrome, ErrorInfo> {
    let host = Host::current();
    match browser::discover(explicit, &host) {
        Err(DiscoverError::NotFound {
            remedy: Remedy::Install,
            ..
        }) if auto_install => {
            let request = Request {
                channel: browser::Channel::Stable,
                version: None,
                product: Product::Chrome,
                force: false,
            };
            let done = download(&host, &request, timeout)?;
            ensure_runs(&host, &done, false, &Cmd::new("install"))?;
            let mut chrome = browser::discover(explicit, &host).map_err(|e| e.to_error_info())?;
            if let Some(bytes) = done.downloaded {
                chrome.warnings.insert(
                    0,
                    format!(
                        "installed {} {} ({}) because --auto-install is set",
                        done.product.title(),
                        done.version,
                        format::size(bytes)
                    ),
                );
            }
            Ok(chrome)
        }
        other => other.map_err(|e| e.to_error_info()),
    }
}

/// Runs `browser::install` with a progress bar on a terminal.
fn download(host: &Host, request: &Request, timeout: Duration) -> Result<Installation, ErrorInfo> {
    let runtime = runtime()?;
    let mut bar = Bar::stderr();
    let result = runtime.block_on(browser::install(host, request, timeout, &mut |p| {
        bar.update(p);
    }));
    bar.finish();
    result.map_err(|e| e.to_error_info())
}

fn runtime() -> Result<tokio::runtime::Runtime, ErrorInfo> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::InstallFailed,
                format!("cannot start the runtime: {e}"),
            )
        })
}

/// The `install` command that gets the same browser, for hints.
fn again(args: &InstallArgs) -> Cmd {
    let mut cmd = Cmd::new("install");
    if let Some(version) = &args.version {
        cmd = cmd.flag("version", version);
    } else if args.channel != Channel::Stable {
        cmd = cmd.flag("channel", args.channel.to_browser().as_str());
    }
    if args.headless_shell {
        cmd = cmd.switch("headless-shell");
    }
    cmd
}

/// Checks (on Linux) that the installed browser starts. With `with_deps`,
/// installs the missing system libraries and checks again; returns the
/// packages installed.
fn ensure_runs(
    host: &Host,
    done: &Installation,
    with_deps: bool,
    again: &Cmd,
) -> Result<Vec<String>, ErrorInfo> {
    let Err(cannot) = browser::check_runs(host, &done.executable) else {
        return Ok(Vec::new());
    };
    let fix = cannot.fix.as_ref().filter(|f| f.command.is_some());
    match fix {
        Some(fix) if with_deps => {
            run_fix(fix)?;
            match browser::check_runs(host, &done.executable) {
                Ok(()) => Ok(fix.packages.clone()),
                Err(still) => Err(cannot_run(done, &still, again, true)),
            }
        }
        _ => Err(cannot_run(done, &cannot, again, false)),
    }
}

/// The error for an installed browser that does not start.
fn cannot_run(done: &Installation, cannot: &CannotRun, again: &Cmd, tried: bool) -> ErrorInfo {
    let path = done.executable.display().to_string();
    let info = ErrorInfo::new(
        ErrorKind::LaunchFailed,
        format!(
            "{} {} is installed but cannot start: {}",
            done.product.title(),
            done.version,
            cannot.reason
        ),
    )
    .note("path", &path);
    let Some(fix) = &cannot.fix else {
        return info.hint(format!(
            "run it to see why: {} --version",
            format::shell_quote(&path)
        ));
    };
    let info = info.note("missing", fix.missing.join(", ")).details(json!({
        "path": path,
        "missing": fix.missing,
        "packages": fix.packages,
        "command": fix.sudo_line(),
    }));
    match fix.sudo_line() {
        Some(line) if !tried => info.hint(
            Hint::run(again.clone().switch("with-deps"))
                .why(format!("runs {line}"))
                .or(format!("run {line}")),
        ),
        Some(line) => info.hint(format!(
            "`{line}` did not provide them all; install the rest with the package manager"
        )),
        None => info.hint("install the packages that provide them with the system package manager"),
    }
}

/// Runs the package manager line of `fix` (with sudo unless root), after
/// asking on a terminal. Without one, it runs only if sudo needs no
/// password (CI).
fn run_fix(fix: &DepsFix) -> Result<(), ErrorInfo> {
    let command = fix.command.clone().unwrap_or_default();
    let root = is_root();
    let argv: Vec<String> = if root {
        command
    } else {
        std::iter::once("sudo".to_owned()).chain(command).collect()
    };
    let line = argv.join(" ");
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if interactive {
        crate::output::stderr(&format!(
            "chrome-cdp needs these system libraries: {}\nIt will run: {line}\nContinue? [y/N] ",
            fix.missing.join(", ")
        ));
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "cancelled; no packages were installed",
            )
            .hint(format!("run it yourself: {line}")));
        }
    } else if !root && !passwordless_sudo() {
        return Err(ErrorInfo::new(
            ErrorKind::LaunchFailed,
            "--with-deps needs sudo, which wants a password, and there is no terminal to ask on",
        )
        .hint(format!("run it in a terminal: {line}")));
    }
    tracing::info!("running {line}");
    // The package manager's output goes to stderr: stdout is the result.
    let stdout = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map_or_else(|_| Stdio::null(), Stdio::from);
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(stdout)
        .status()
        .map_err(|e| {
            ErrorInfo::new(
                ErrorKind::InstallFailed,
                format!("cannot run {}: {e}", argv[0]),
            )
            .hint(format!("run it yourself: {line}"))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(ErrorInfo::new(
            ErrorKind::InstallFailed,
            format!("`{line}` failed ({status})"),
        )
        .hint("read its output above, fix the cause and run the command again"))
    }
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

fn passwordless_sudo() -> bool {
    Command::new("sudo")
        .args(["-n", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn installed_output(done: &Installation, packages: &[String]) -> Output {
    let path = done.executable.display().to_string();
    let what = format!(
        "{} {} ({})",
        done.product.title(),
        done.version,
        done.channel
    );
    let mut text = match done.downloaded {
        Some(bytes) => format!("installed {what}, {}: {path}", format::size(bytes)),
        None => format!("{what} is already installed: {path}"),
    };
    if !packages.is_empty() {
        text.push_str("\ninstalled packages: ");
        text.push_str(&packages.join(" "));
    }
    let data = InstallData::Installed(InstalledBrowser {
        id: done.id.clone(),
        channel: done.channel.clone(),
        version: done.version.clone(),
        platform: done.platform.clone(),
        product: done.product.as_str().to_owned(),
        path: path.clone(),
        downloaded: done.downloaded.is_some(),
        download_size: done.downloaded.unwrap_or(0),
        packages_installed: packages.to_vec(),
    });
    let output = Output::typed(&data, text).quiet(path);
    done.warnings.iter().fold(output, Output::warning)
}

/// `install --list`: the installs for this platform, and each channel's
/// current version (skipped with a warning when offline).
fn list(host: &Host, timeout: Duration) -> Result<Output, ErrorInfo> {
    let platform = browser::platform_key(host);
    let installed = browser::installed(host);
    let mut warnings = Vec::new();
    let channels = if platform.is_some() {
        runtime()?
            .block_on(browser::channels(host, timeout))
            .unwrap_or_else(|e| {
                warnings.push(format!("cannot list the available versions: {e}"));
                Vec::new()
            })
    } else {
        warnings.push("Chrome for Testing has no build for this machine".to_owned());
        Vec::new()
    };
    let available: Vec<AvailableBrowser> = channels
        .iter()
        .filter(|c| platform.is_some_and(|p| c.release(Product::Chrome, p).is_some()))
        .map(|c| AvailableBrowser {
            channel: c.channel.as_str().to_owned(),
            version: c.version.clone(),
            installed: installed
                .iter()
                .find(|i| i.version == c.version)
                .map(|i| i.id.clone()),
        })
        .collect();
    let entries: Vec<InstallListEntry> = installed
        .iter()
        .map(|i| InstallListEntry {
            id: i.id.clone(),
            channel: i.channel.clone(),
            version: i.version.clone(),
            products: i.products.iter().map(|p| p.as_str().to_owned()).collect(),
            path: i.executable.display().to_string(),
        })
        .collect();

    // One line per install, not a table: a table would cut the path.
    let mut text = String::from("installed:\n");
    if entries.is_empty() {
        text.push_str("(none)\n");
    }
    for e in &entries {
        let _ = writeln!(text, "{} ({}): {}", e.id, e.products.join(", "), e.path);
    }
    if !available.is_empty() {
        let _ = writeln!(
            text,
            "\navailable (Chrome for Testing, {}):",
            platform.unwrap_or_default()
        );
        let mut rows = vec![vec![
            "CHANNEL".to_owned(),
            "VERSION".into(),
            "INSTALLED".into(),
        ]];
        rows.extend(available.iter().map(|a| {
            vec![
                a.channel.clone(),
                a.version.clone(),
                a.installed.clone().unwrap_or_else(|| "-".into()),
            ]
        }));
        text.push_str(&format::table(&rows));
        text.push('\n');
    }
    let quiet = entries
        .iter()
        .map(|e| e.id.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let data = InstallData::List(InstallList {
        platform: platform.map(str::to_owned),
        installed: entries,
        available,
    });
    let output = Output::typed(&data, text.trim_end()).quiet(quiet);
    Ok(warnings.into_iter().fold(output, Output::warning))
}
