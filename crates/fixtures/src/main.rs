//! `cargo xtask fixtures`: serves the fixture site until interrupted.

use std::process::ExitCode;

const USAGE: &str = "\
usage: fixtures [--port <A>] [--port-b <B>]

Serves testdata/site on two origins, http://127.0.0.1:<A> and
http://localhost:<B> (free ports unless given), and on
https://127.0.0.1:<free port> with a self-signed certificate, until Ctrl-C.
";

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (port_a, port_b) = match parse(&args) {
        Ok(ports) => ports,
        Err(e) => {
            eprint!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let fixtures = match fixtures::Fixtures::start(port_a, port_b) {
        Ok(fixtures) => fixtures,
        Err(e) => {
            eprintln!("error: cannot start the fixture server: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("serving {}", fixtures::site_dir().display());
    println!("origin A: {}/", fixtures.a());
    println!("origin B: {}/", fixtures.b());
    println!("HTTPS:    {}/ (self-signed)", fixtures.https());
    println!("press Ctrl-C to stop");
    loop {
        std::thread::park();
    }
}

fn parse(args: &[String]) -> Result<(u16, u16), String> {
    let (mut a, mut b) = (0, 0);
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let port = |value: Option<&String>| {
            value
                .and_then(|v| v.parse::<u16>().ok())
                .ok_or_else(|| format!("{flag} needs a port number"))
        };
        match flag.as_str() {
            "--port" => a = port(args.next())?,
            "--port-b" => b = port(args.next())?,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok((a, b))
}
