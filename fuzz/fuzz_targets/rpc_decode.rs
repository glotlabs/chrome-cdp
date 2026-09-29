//! The CLI <-> daemon decoder never panics on a line, and what it reads
//! survives a round trip. A response the CLI reads also renders, in every
//! mode, without panicking: the CLI prints whatever its daemon sends.
//!
//! Run with `cargo +nightly fuzz run rpc_decode` from the repository root.
//! The seeds in `fuzz/seeds/rpc_decode/` are real requests and responses.

#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::rpc::{decode_params, decode_request, decode_response};
use protocol::{ErrorInfo, Mode, Output, Style, render};
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(line) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(req) = decode_request(line) {
        let again = serde_json::to_string(&req).expect("a request serialises");
        assert_eq!(decode_request(&again).expect("a request reads back"), req);
        let _ = decode_params::<Value>(req.params);
    }

    if let Ok(resp) = decode_response(line) {
        let again = serde_json::to_string(&resp).expect("a response serialises");
        assert_eq!(
            decode_response(&again).expect("a response reads back"),
            resp
        );
        let result: Result<Output, ErrorInfo> = match resp.into_result() {
            Ok(value) => match serde_json::from_value(value) {
                Ok(output) => Ok(output),
                Err(_) => return,
            },
            Err(error) => Err(error.into_error_info()),
        };
        for mode in [Mode::Text, Mode::Quiet, Mode::Json] {
            for style in [Style::PLAIN, Style::COLOR] {
                let _ = render(mode, result.clone(), style);
            }
        }
    }
});
