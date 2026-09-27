//! `cdp-gen [--check]`: regenerates `crates/cdp/src/domains` from
//! `crates/cdp/protocol`. Usually run as `cargo xtask gen-cdp`.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = match args.as_slice() {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => {
            eprintln!("usage: cdp-gen [--check]");
            return ExitCode::from(2);
        }
    };
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.ancestors().nth(2).unwrap_or(manifest_dir);
    let layout = cdp_gen::Layout::new(root);

    let result = cdp_gen::generate_formatted(&layout).and_then(|files| {
        if check {
            let drift = cdp_gen::check(&layout, &files)?;
            if drift.is_empty() {
                Ok(format!("{} is up to date", layout.out_dir.display()))
            } else {
                Err(cdp_gen::Error::from(format!(
                    "generated CDP code is out of date:\n{drift}run `cargo xtask gen-cdp` and commit the result"
                )))
            }
        } else {
            cdp_gen::write(&layout, &files)?;
            Ok(format!("wrote {} files to {}", files.len(), layout.out_dir.display()))
        }
    });
    match result {
        Ok(msg) => {
            eprintln!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
