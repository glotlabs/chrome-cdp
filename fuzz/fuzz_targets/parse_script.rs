//! The `.cdp` parser never panics, and every span it returns lies within
//! the input, on character boundaries, at the line and column it names.
//!
//! Run with `cargo +nightly fuzz run parse_script` from the repository
//! root. `crates/protocol/src/script/tests.rs` checks the same properties
//! with proptest on every `cargo test`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::script::{Item, Span, parse};

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let parsed = parse("fuzz.cdp", src);
    let mut spans = Vec::new();
    collect(&parsed.script.items, &mut spans);
    spans.extend(parsed.errors.iter().map(|e| e.span.clone()));
    for span in spans {
        assert!(span.start <= span.end && span.end <= src.len(), "{span:?}");
        assert!(src.is_char_boundary(span.start) && src.is_char_boundary(span.end));
        let before = &src[..span.start];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        assert_eq!((span.line as usize, span.column as usize), (line, column));
    }
});

fn collect(items: &[Item], out: &mut Vec<Span>) {
    for item in items {
        match item {
            Item::Command(l) => {
                out.push(l.span.clone());
                out.extend(l.comment.iter().map(|c| c.span.clone()));
                for w in &l.words {
                    out.push(w.span.clone());
                    out.extend(w.vars().map(|v| v.span.clone()));
                }
            }
            Item::Directive(d) => {
                out.push(d.span.clone());
                out.extend(d.comment.iter().map(|c| c.span.clone()));
                for w in &d.args {
                    out.push(w.span.clone());
                    out.extend(w.vars().map(|v| v.span.clone()));
                }
                collect(&d.body, out);
            }
            Item::Comment(c) => out.push(c.span.clone()),
        }
    }
}
