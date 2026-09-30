//! A task's real dev server (`[setup] dev`): a shell command that listens
//! on a free port while the agent works and the checks run.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a dev server may take to listen (Vite's first start
/// optimizes its dependencies).
const START: Duration = Duration::from_secs(60);

/// A running dev server; dropping it kills its process group.
#[derive(Debug)]
pub struct DevServer {
    child: Child,
    /// Its output, for the error when it does not start.
    log: tempfile::NamedTempFile,
}

impl DevServer {
    /// Runs `command` with `sh -c` in `dir` and waits until `port`
    /// accepts connections.
    pub fn start(command: &str, dir: &Path, port: u16) -> Result<Self, String> {
        let log = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        let out = log.reopen().map_err(|e| e.to_string())?;
        let err = log.reopen().map_err(|e| e.to_string())?;
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let child = cmd
            .spawn()
            .map_err(|e| format!("start the dev server `{command}`: {e}"))?;
        let mut server = Self { child, log };
        let start = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Ok(Some(status)) = server.child.try_wait() {
                let output = std::fs::read_to_string(server.log.path()).unwrap_or_default();
                return Err(format!(
                    "the dev server `{command}` exited ({status}): {}",
                    output.trim()
                ));
            }
            if start.elapsed() > START {
                return Err(format!(
                    "the dev server `{command}` did not listen on port {port} within {}s",
                    START.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(server)
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        super::run::kill_group(self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
