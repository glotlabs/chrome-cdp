//! Reading positional text arguments (plan 08 §4): `-` means stdin, so
//! agents can pass JSON, scripts or long text without shell quoting.

use std::io;

use protocol::{ErrorInfo, ErrorKind};

/// The argument's text, or all of stdin for `-`. `what` names the argument
/// in errors (`params`).
pub(crate) fn text_arg(arg: &str, what: &str) -> Result<String, ErrorInfo> {
    if arg != "-" {
        return Ok(arg.to_owned());
    }
    read_stdin(&mut io::stdin().lock(), what)
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
    use super::{read_stdin, text_arg};

    #[test]
    fn plain_arguments_pass_through() {
        assert_eq!(text_arg("{}", "params").unwrap(), "{}");
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
