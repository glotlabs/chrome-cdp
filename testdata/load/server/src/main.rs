//! `load-server`: serves e2e-shop with latency and errors per endpoint
//! until interrupted (see the library's docs).

use std::process::ExitCode;

use load_server::{Fault, LoadServer, Options};

const USAGE: &str = "\
usage: load-server [--port <P>] [--dist <DIR>] [--payments <P>]
                   [--latency [METHOD ]PATH=200ms±50]... [--errors [METHOD ]PATH=2%]...
                   [--seed <N>] [--threads <N>]

Serves the built app of examples/e2e-shop (or --dist) and its API on
http://localhost:<P> (a free port unless given) until Ctrl-C, with the
latency and errors given per endpoint. A PATH is matched without the
query: * is any part of one segment, ** anything. GET /__load/stats
answers what the server saw per METHOD /path; POST /__load/reset starts
the counts over.

Examples:
  load-server --port 4176
  load-server --latency /api/cart=200ms±50 --errors 'POST /api/orders=2%'
  load-server --latency '/api/**=5s'     # a slow server
";

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match parse(&args) {
        Ok(options) => options,
        Err(e) => {
            eprint!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let server = match LoadServer::start(options) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("error: cannot start the load server: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("load server: {}/", server.url());
    if let Some(payments) = server.payments() {
        println!("payments: {payments}/");
    }
    println!("press Ctrl-C to stop");
    loop {
        std::thread::park();
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        let number = |v: String| {
            v.parse::<u64>()
                .map_err(|_| format!("{flag} {v}: not a number"))
        };
        match flag.as_str() {
            "--port" => options.port = port(&value()?)?,
            "--payments" => options.payments = Some(port(&value()?)?),
            "--dist" => options.dist = value()?.into(),
            "--latency" => options.faults.push(Fault::latency(&value()?)?),
            "--errors" => options.faults.push(Fault::errors(&value()?)?),
            "--seed" => options.seed = number(value()?)?,
            "--threads" => {
                options.threads = usize::try_from(number(value()?)?).unwrap_or(usize::MAX);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !options.dist.join("index.html").is_file() {
        return Err(format!(
            "{} has no index.html: give the app's build with --dist",
            options.dist.display()
        ));
    }
    Ok(options)
}

fn port(text: &str) -> Result<u16, String> {
    text.parse()
        .map_err(|_| format!("`{text}` is not a port number"))
}
