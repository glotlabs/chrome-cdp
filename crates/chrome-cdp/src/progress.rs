//! The download progress bar of `chrome-cdp install`: one line on stderr,
//! redrawn in place, only when stderr is a terminal (plan 02, milestone
//! 2.2). Without a terminal, install is silent.

use std::io::{IsTerminal as _, Write};
use std::time::{Duration, Instant};

use browser::Progress;
use protocol::format;

/// How often the line is redrawn at most.
const REDRAW: Duration = Duration::from_millis(100);

pub(crate) struct Bar {
    out: Option<Box<dyn Write>>,
    /// The length of the line on screen, to blank it out.
    drawn: usize,
    last: Option<Instant>,
}

impl std::fmt::Debug for Bar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bar")
            .field("drawn", &self.drawn)
            .finish_non_exhaustive()
    }
}

impl Bar {
    /// A bar on stderr if it is a terminal, else one that prints nothing.
    pub(crate) fn stderr() -> Self {
        let tty = std::io::stderr().is_terminal();
        Self::new(tty.then(|| Box::new(std::io::stderr()) as Box<dyn Write>))
    }

    fn new(out: Option<Box<dyn Write>>) -> Self {
        Self {
            out,
            drawn: 0,
            last: None,
        }
    }

    pub(crate) fn update(&mut self, progress: Progress<'_>) {
        if self.out.is_none() {
            return;
        }
        match progress {
            Progress::Waiting => {
                self.clear();
                self.write("waiting for another chrome-cdp install to finish…\n");
            }
            Progress::Downloading {
                release,
                done,
                total,
            } => {
                if self.throttled(total.is_some_and(|t| done >= t)) {
                    return;
                }
                let amount = match total {
                    Some(total) if total > 0 => format!(
                        "{:>3}%  {} of {}",
                        done.saturating_mul(100) / total,
                        format::size(done),
                        format::size(total)
                    ),
                    _ => format::size(done),
                };
                self.draw(&format!(
                    "downloading {} {}  {amount}",
                    release.product.title(),
                    release.version
                ));
            }
            Progress::Unpacking { done, total } => {
                if self.throttled(done >= total) {
                    return;
                }
                let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
                self.draw(&format!("unpacking  {percent:>3}%"));
            }
        }
    }

    /// Blanks the line so the result or error starts on a clean one.
    pub(crate) fn finish(&mut self) {
        self.clear();
    }

    /// Whether to skip this redraw: too soon after the last, unless it is
    /// the final one.
    fn throttled(&mut self, last_one: bool) -> bool {
        let now = Instant::now();
        if !last_one && self.last.is_some_and(|t| now.duration_since(t) < REDRAW) {
            return true;
        }
        self.last = Some(now);
        false
    }

    fn draw(&mut self, line: &str) {
        let len = line.chars().count();
        let pad = " ".repeat(self.drawn.saturating_sub(len));
        self.write(&format!("\r{line}{pad}"));
        self.drawn = len;
    }

    fn clear(&mut self) {
        if self.drawn > 0 {
            let blank = " ".repeat(self.drawn);
            self.write(&format!("\r{blank}\r"));
            self.drawn = 0;
        }
    }

    fn write(&mut self, text: &str) {
        if let Some(out) = &mut self.out {
            let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use browser::{Product, Progress, Release};

    use super::Bar;

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn draws_one_line_in_place() {
        let out = Shared::default();
        let mut bar = Bar::new(Some(Box::new(out.clone())));
        let release = Release {
            version: "154.0.8037.57".into(),
            product: Product::Chrome,
            platform: "linux64".into(),
            url: String::new(),
        };
        let mb = 1024 * 1024;
        for done in [0, mb, 2 * mb] {
            bar.update(Progress::Downloading {
                release: &release,
                done,
                total: Some(2 * mb),
            });
        }
        bar.update(Progress::Unpacking { done: 5, total: 5 });
        bar.finish();
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        // The 1 MB update comes too soon after the first and is skipped;
        // the last one always shows.
        let line0 = "downloading Chrome for Testing 154.0.8037.57    0%  0B of 2MB";
        let line1 = "downloading Chrome for Testing 154.0.8037.57  100%  2MB of 2MB";
        let pad = " ".repeat(line1.len() - "unpacking  100%".len());
        let blank = " ".repeat("unpacking  100%".len());
        assert_eq!(
            text,
            format!("\r{line0}\r{line1}\runpacking  100%{pad}\r{blank}\r")
        );
    }

    #[test]
    fn silent_without_a_terminal() {
        let mut bar = Bar::new(None);
        bar.update(Progress::Waiting);
        bar.finish();
        assert_eq!(bar.drawn, 0);
    }
}
