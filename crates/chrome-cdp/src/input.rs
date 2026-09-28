//! Reading positional text arguments (plan 08 §4): `-` means stdin, so
//! agents can pass JSON, scripts or long text without shell quoting.
//! File arguments become absolute paths here, since the daemon that uses
//! them runs elsewhere.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use protocol::{ErrorInfo, ErrorKind, Hint};

/// The argument's text, or all of stdin for `-`. `what` names the argument
/// in errors (`params`).
pub(crate) fn text_arg(arg: &str, what: &str) -> Result<String, ErrorInfo> {
    if arg != "-" {
        return Ok(arg.to_owned());
    }
    if STDIN_TAKEN.load(Ordering::Relaxed) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {what} from stdin: it holds the batch script"),
        )
        .hint(Hint::text(format!(
            "put the {what} on the line, quoted, or use chrome-cdp run <file.cdp>"
        ))));
    }
    read_stdin(&mut io::stdin().lock(), what)
}

/// Set once `batch` has read its script from stdin, so `-` arguments of
/// its steps fail instead of reading nothing.
static STDIN_TAKEN: AtomicBool = AtomicBool::new(false);

/// All of stdin, which `-` arguments can no longer read (`batch`).
pub(crate) fn take_stdin(what: &str) -> Result<String, ErrorInfo> {
    STDIN_TAKEN.store(true, Ordering::Relaxed);
    read_stdin(&mut io::stdin().lock(), what)
}

/// `path` made absolute against the current directory (not resolving
/// links), as a string.
pub(crate) fn absolute_path(path: &Path) -> Result<String, ErrorInfo> {
    let absolute = std::path::absolute(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot make {} absolute: {e}", path.display()),
        )
    })?;
    absolute.into_os_string().into_string().map_err(|p| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} is not valid UTF-8", Path::new(&p).display()),
        )
    })
}

fn read_stdin(stdin: &mut impl io::Read, what: &str) -> Result<String, ErrorInfo> {
    let mut buf = String::new();
    stdin.read_to_string(&mut buf).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {what} from stdin: {e}"),
        )
    })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{absolute_path, read_stdin, text_arg};

    #[test]
    fn plain_arguments_pass_through() {
        assert_eq!(text_arg("{}", "params").unwrap(), "{}");
    }

    #[test]
    fn file_paths_become_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let path = absolute_path(std::path::Path::new("a/b.txt")).unwrap();
        assert_eq!(path, cwd.join("a/b.txt").to_string_lossy());
        assert_eq!(absolute_path(std::path::Path::new("/x/y")).unwrap(), "/x/y");
    }

    #[test]
    fn dash_reads_stdin() {
        assert_eq!(read_stdin(&mut &b"hello\n"[..], "text").unwrap(), "hello\n");
        let err = read_stdin(&mut &[0xff, 0xfe][..], "params").unwrap_err();
        assert!(
            err.message.starts_with("cannot read params from stdin: "),
            "{err}"
        );
        assert_eq!(err.exit_code(), 2);
    }
}
