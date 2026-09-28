//! `cargo xtask fixtures`: serves the fixture site until interrupted.

use std::process::ExitCode;

const USAGE: &str = "\
usage: fixtures [--port <A>] [--port-b <B>]
       fixtures --shop <dist> [--port <A>]

Serves testdata/site on two origins, http://127.0.0.1:<A> and
http://localhost:<B> (free ports unless given), and on
https://127.0.0.1:<free port> with a self-signed certificate, until Ctrl-C.

With --shop, serves the real backend of examples/e2e-shop instead: the
built app in <dist> and its /api on http://localhost:<A>.
";

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (port_a, port_b, shop) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprint!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Some(dist) = shop {
        let shop = match fixtures::Shop::start(dist.into(), port_a) {
            Ok(shop) => shop,
            Err(e) => {
                eprintln!("error: cannot start the shop: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!("shop: {}/", shop.url());
        println!("press Ctrl-C to stop");
        loop {
            std::thread::park();
        }
    }
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

fn parse(args: &[String]) -> Result<(u16, u16, Option<String>), String> {
    let (mut a, mut b, mut shop) = (0, 0, None);
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
            "--shop" => {
                shop = Some(
                    args.next()
                        .ok_or("--shop needs the app's dist folder")?
                        .clone(),
                );
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok((a, b, shop))
}
