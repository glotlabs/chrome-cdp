//! chrome-cdp in dirs of its own: its sessions, config and temp files
//! never touch the user's, and dropping the sandbox closes its browsers.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

#[derive(Debug)]
pub struct Sandbox {
    pub dir: TempDir,
    bin: PathBuf,
    chrome: PathBuf,
}

impl Sandbox {
    /// `bin` is the chrome-cdp binary, `chrome` the Chrome it drives.
    pub fn new(bin: &Path, chrome: &Path) -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("ccdp-bench-")
            .tempdir()
            .map_err(|e| format!("create a temp dir: {e}"))?;
        for sub in ["run", "config", "tmp"] {
            std::fs::create_dir_all(dir.path().join(sub))
                .map_err(|e| format!("create {sub}: {e}"))?;
        }
        Ok(Self {
            dir,
            bin: bin.to_owned(),
            chrome: chrome.to_owned(),
        })
    }

    /// The variables that point chrome-cdp at this sandbox.
    pub fn env(&self) -> Vec<(&'static str, PathBuf)> {
        let d = self.dir.path();
        vec![
            ("XDG_RUNTIME_DIR", d.join("run")),
            ("XDG_CONFIG_HOME", d.join("config")),
            ("TMPDIR", d.join("tmp")),
            ("CHROME_CDP_CHROME", self.chrome.clone()),
        ]
    }

    /// A `chrome-cdp` command in the sandbox, with no `CHROME_CDP_*`
    /// variable of the caller's.
    pub fn cli(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CHROME_CDP_") {
                cmd.env_remove(key);
            }
        }
        cmd.envs(self.env())
            .current_dir(self.dir.path())
            .stdin(Stdio::null());
        cmd
    }

    /// Runs `chrome-cdp <args>`.
    pub fn run<I, S>(&self, args: I) -> Result<Output, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.cli();
        cmd.args(args);
        cmd.output()
            .map_err(|e| format!("run {}: {e}", self.bin.display()))
    }

    /// Runs `chrome-cdp <args>` and fails unless it exits 0.
    pub fn ok<I, S>(&self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<S> = args.into_iter().collect();
        let shown: Vec<String> = args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect();
        let out = self.run(&args)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "chrome-cdp {} failed ({}): {}",
                shown.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self
            .cli()
            .args(["close", "--all"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
