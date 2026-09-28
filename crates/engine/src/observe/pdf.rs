//! `pdf` (plan 06, milestone 6.3): the page printed with
//! `Page.printToPDF`, as the print dialog would, to a file.
//!
//! Chrome prints only when headless; a browser with a window answers that
//! the method is not implemented. A launched headed browser fails before
//! the call, a connected one when Chrome says so; both say how to get a
//! headless one.

use std::fmt::Write as _;

use base64::Engine as _;
use cdp::domains::page as cdp_page;
use protocol::schema::PdfData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use super::save::save;
use crate::errors::from_cdp;
use crate::request::{PdfRequest, millis};
use crate::tabs::tab_name;
use crate::{Engine, Spec};

impl Engine {
    /// `pdf`: the active tab (or `--tab`) printed to a PDF file.
    pub async fn pdf(&self, req: PdfRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        if matches!(running.spec, Spec::Launched { headed: true, .. }) {
            return Err(self.pdf_headed());
        }
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let (width, height) = req.format.inches();
        let params = cdp_page::PrintToPdfParams {
            // The paper is given upright; Chrome turns it for `landscape`.
            landscape: Some(req.landscape),
            print_background: Some(req.background),
            paper_width: Some(width),
            paper_height: Some(height),
            margin_top: req.margin.map(|m| m.top),
            margin_right: req.margin.map(|m| m.right),
            margin_bottom: req.margin.map(|m| m.bottom),
            margin_left: req.margin.map(|m| m.left),
            page_ranges: req.pages.clone(),
            prefer_css_page_size: Some(req.css_page_size),
            ..cdp_page::PrintToPdfParams::default()
        };
        let print = async {
            page.session
                .with_timeout(timeout)
                .call(params)
                .await
                .map_err(|e| {
                    let message = e.to_string().to_ascii_lowercase();
                    if message.contains("not implemented") {
                        self.pdf_headed()
                    } else if message.contains("page range") {
                        self.pdf_bad_pages(req.pages.as_deref().unwrap_or_default(), &message)
                    } else {
                        from_cdp(&e, &self.session)
                    }
                })
        };
        let data = Box::pin(self.unless_blocked(&running, &page, mark, print))
            .await?
            .data;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("Chrome sent a PDF that is not base64: {e}"),
                )
            })?;
        let path = save(
            &bytes,
            req.path.as_deref(),
            &self.output_dir().join("pdfs"),
            "pdf",
        )?;
        let path_text = path.display().to_string();
        let pages = count_pages(&bytes);
        let mut text = format!("{path_text}: ");
        if let Some(pages) = pages {
            let _ = write!(
                text,
                "{pages} {}, ",
                if pages == 1 { "page" } else { "pages" }
            );
        }
        let _ = write!(
            text,
            "{}{}, {}",
            req.format.as_str(),
            if req.landscape { " landscape" } else { "" },
            format::size(bytes.len() as u64)
        );
        let data = PdfData {
            tab: tab_name(page.id),
            path: path_text.clone(),
            bytes: bytes.len() as u64,
            pages,
            paper: req.format.as_str().to_owned(),
            landscape: req.landscape,
        };
        let output = Output::typed(&data, text).quiet(path_text);
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// Chrome refused `--pages`: its syntax, or pages past the end.
    fn pdf_bad_pages(&self, pages: &str, message: &str) -> ErrorInfo {
        let why = if message.contains("exceeds") {
            "goes past the last page"
        } else {
            "is not a list of pages and ranges"
        };
        ErrorInfo::new(ErrorKind::BadArgs, format!("--pages {pages} {why}")).hint(
            Hint::run(
                Cmd::new("pdf")
                    .flag("pages", "1-3,5")
                    .session(&self.session),
            )
            .why("pages 1 to 3, and 5; 3- runs to the end"),
        )
    }

    fn pdf_headed(&self) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!(
                "Chrome prints PDFs only when headless, and session {} has a window",
                self.session
            ),
        )
        .hint(
            Hint::run(Cmd::new("close").session(&self.session))
                .why("then open starts a headless browser"),
        )
    }
}

/// How many pages a PDF has: its `/Type /Page` objects (not `/Pages`,
/// the page tree's nodes). `None` if it has none that way (compressed
/// object streams).
fn count_pages(pdf: &[u8]) -> Option<u32> {
    let mut count = 0;
    let mut rest = pdf;
    while let Some(at) = find(rest, b"/Type") {
        rest = &rest[at + 5..];
        let skipped = rest.iter().take_while(|b| b.is_ascii_whitespace()).count();
        let after = &rest[skipped..];
        if let Some(tail) = after.strip_prefix(b"/Page")
            && !tail.first().is_some_and(u8::is_ascii_alphanumeric)
        {
            count += 1;
        }
    }
    (count > 0).then_some(count)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::count_pages;

    #[test]
    fn pages_are_counted_without_the_page_tree() {
        let pdf = b"%PDF-1.4\n1 0 obj << /Type /Pages /Count 2 >>\n\
                    2 0 obj << /Type /Page /Parent 1 0 R >>\n\
                    3 0 obj <</Type/Page/Parent 1 0 R>>\n";
        assert_eq!(count_pages(pdf), Some(2));
        assert_eq!(count_pages(b"%PDF-1.7 compressed"), None);
    }
}
