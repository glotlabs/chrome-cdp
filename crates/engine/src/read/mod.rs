//! The read commands (plan 04, milestone 4.5): `text`, `html`, `attr`,
//! `value`, `count`, `box` and `markdown` (`find` lives with `snapshot`,
//! whose tree it searches).
//!
//! Each resolves its locator through the locator engine
//! ([`Engine::call_on`]) and runs one function in chrome-cdp's isolated
//! world, most of it in the helper bundle (`injected/src/read.ts`).
//! `text` and `count` take every match; the others want exactly one
//! element and fail with `Ambiguous` otherwise. `text`, `html` and
//! `markdown` read the page without a locator, and a frame's document
//! when the locator names only a frame (`frame=payment`, an iframe's
//! ref). Without a locator, `text` and `markdown` put each visible
//! iframe's content where its `<iframe>` sits ([`frames`]).

mod frames;

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{
    AttrData, BoxData, CountData, HtmlData, MarkdownData, Rect, TextData, ValueData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::Value;

use crate::context::Scoped as _;
use crate::locator::{Called, FrameOnly, LocateOptions, Locator, Want};
use crate::request::{Read, ReadRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

use self::frames::FramesRead;

/// Checks a read request's arguments (its locator, and that the commands
/// that need one have it) without a browser, so the CLI can fail before
/// it starts one.
pub fn check_read(req: &ReadRequest) -> Result<(), ErrorInfo> {
    read_locator(req).map(drop)
}

fn locate_options(req: &ReadRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

fn read_locator(req: &ReadRequest) -> Result<Option<Locator>, ErrorInfo> {
    let locator = req
        .locator
        .as_deref()
        .map(|l| locate_options(req).parse(l))
        .transpose()?;
    let needs = !matches!(req.read, Read::Text | Read::Html { .. } | Read::Markdown);
    if needs && locator.is_none() {
        let command = req.read.command();
        return Err(
            ErrorInfo::new(ErrorKind::BadArgs, format!("{command} needs a locator"))
                .hint(Hint::run(Cmd::new(command).switch("help"))),
        );
    }
    Ok(locator)
}

/// One read in progress: what it reads, and where.
struct Reader<'a> {
    engine: &'a Engine,
    running: &'a Running,
    page: &'a Page,
    req: &'a ReadRequest,
    locator: Option<&'a Locator>,
    options: LocateOptions,
    timeout: Duration,
}

impl Engine {
    /// `text`, `html`, `attr`, `value`, `count`, `box` and `markdown`, on
    /// the active tab (or `--tab`).
    pub async fn read(&self, req: ReadRequest) -> Result<Output, ErrorInfo> {
        let locator = read_locator(&req)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let reader = Reader {
            engine: self,
            running: &running,
            page: &page,
            req: &req,
            locator: locator.as_ref(),
            options: locate_options(&req),
            timeout,
        };
        let output = self
            .unless_blocked(&running, &page, mark, reader.run())
            .await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }
}

impl Engine {
    /// Where frame `frame`'s viewport starts in tab `page`'s: the offsets of
    /// the `<iframe>` content boxes it is in, added up.
    pub(crate) async fn frame_offset(
        &self,
        running: &Running,
        page: &Page,
        frame: &str,
        timeout: Duration,
    ) -> Result<(f64, f64), ErrorInfo> {
        let (mut x, mut y) = (0.0, 0.0);
        let mut frame = frame.to_owned();
        let frames = running.tabs.frames(page.id);
        while frames.iter().any(|f| f.id == frame && f.parent.is_some()) {
            let (parent, context, root) = self.frame_owner(running, page, &frame, timeout).await?;
            let mut params =
                cdp::domains::runtime::CallFunctionOnParams::new(CONTENT_ORIGIN_JS.to_owned());
            params.object_id = Some(root.object_id.clone());
            params.return_by_value = Some(true);
            let result = context.session.with_timeout(timeout).call(params).await;
            let release = cdp::domains::runtime::ReleaseObjectParams {
                object_id: root.object_id,
            };
            let _ = context.session.call(release).await;
            let result = result.map_err(|e| crate::from_cdp(&e, &self.session))?;
            if let Some([dx, dy]) = result
                .result
                .value
                .and_then(|v| serde_json::from_value::<[f64; 2]>(v).ok())
            {
                x += dx;
                y += dy;
            }
            frame = parent;
        }
        Ok((x, y))
    }
}

#[derive(Debug, Deserialize)]
struct Shown {
    text: String,
    visible: bool,
}

#[derive(Debug, Deserialize)]
struct Attribute {
    value: Option<String>,
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FieldValue {
    value: Option<Value>,
    kind: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Counted {
    count: u64,
    visible: u64,
}

#[derive(Debug, Deserialize)]
struct Readable {
    markdown: String,
    source: String,
    url: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct Viewport {
    width: f64,
    height: f64,
}

/// Where a frame's content box starts in its `<iframe>`'s viewport: the
/// element's box plus its border and padding.
const CONTENT_ORIGIN_JS: &str = "function () {
  const r = this.getBoundingClientRect();
  const s = getComputedStyle(this);
  return [r.left + this.clientLeft + parseFloat(s.paddingLeft),
          r.top + this.clientTop + parseFloat(s.paddingTop)];
}";

impl Reader<'_> {
    async fn run(&self) -> Result<Output, ErrorInfo> {
        match &self.req.read {
            Read::Text => self.text().await,
            Read::Html { outer, raw } => self.html(*outer, *raw).await,
            Read::Attr { name } => self.attr(name).await,
            Read::Value => self.value().await,
            Read::Count => self.count().await,
            Read::Box => self.bounding_box().await,
            Read::Markdown => self.markdown().await,
        }
    }

    fn tab(&self) -> String {
        tab_name(self.page.id)
    }

    fn locator_text(&self) -> String {
        self.locator.map(ToString::to_string).unwrap_or_default()
    }

    /// Calls `function` on the match(es).
    async fn call(
        &self,
        want: Want<'_>,
        frame_only: FrameOnly,
        function: &str,
    ) -> Result<Called, ErrorInfo> {
        self.engine
            .call_on(
                self.running,
                self.page,
                self.locator,
                &self.options,
                want,
                frame_only,
                function,
                self.timeout,
            )
            .await
    }

    /// Calls `function` on the one match; an `Ambiguous` error's hint runs
    /// this command again with the first candidate's ref.
    async fn call_one(&self, frame_only: FrameOnly, function: &str) -> Result<Called, ErrorInfo> {
        let retry = |r: &str| self.retry(r);
        self.call(Want::One(Some(&retry)), frame_only, function)
            .await
    }

    /// This command with locator `locator`.
    fn retry(&self, locator: &str) -> Cmd {
        let cmd = Cmd::new(self.req.read.command());
        match &self.req.read {
            Read::Attr { name } => cmd.arg(locator).arg(name),
            Read::Html { outer, raw } => {
                let mut cmd = cmd.arg(locator);
                if *outer {
                    cmd = cmd.switch("outer");
                }
                if *raw {
                    cmd = cmd.switch("raw");
                }
                cmd
            }
            _ => cmd.arg(locator),
        }
    }

    /// Adds the ref re-resolve note, if any.
    fn noted(output: Output, called: &Called) -> Output {
        match &called.note {
            Some(note) => output.warning(note.clone()),
            None => output,
        }
    }

    /// The page's own read, with its iframes (see [`frames`]).
    async fn page_read(&self, function: &str, field: Option<&str>) -> Result<Called, ErrorInfo> {
        let read = FramesRead { function, field };
        let value = self
            .engine
            .read_frames(self.running, self.page, &read, self.timeout)
            .await?;
        Ok(Called {
            value,
            note: None,
            frame: String::new(),
        })
    }

    async fn text(&self) -> Result<Output, ErrorInfo> {
        let called = if self.locator.is_some() {
            self.call(
                Want::All,
                FrameOnly::Document,
                "function () { return this.map((el) => ({ text: globalThis.__ccdp.text(el),\n\
                 visible: el instanceof Document || globalThis.__ccdp.visible(el) })); }",
            )
            .await?
        } else {
            let mut called = self
                .page_read(
                    "function (owners) { return globalThis.__ccdp.text(this, owners); }",
                    None,
                )
                .await?;
            called.value = serde_json::json!([{ "text": called.value, "visible": true }]);
            called
        };
        let shown: Vec<Shown> = parse(&called.value)?;
        if let Some(locator) = self.locator
            && shown.is_empty()
        {
            return Err(self.engine.not_found(self.page, locator));
        }
        let hidden = shown.iter().filter(|s| !s.visible).count();
        let texts: Vec<String> = shown.into_iter().map(|s| s.text).collect();
        let text = texts.join("\n");
        let data = TextData {
            tab: self.tab(),
            locator: self.locator.map(ToString::to_string),
            texts,
        };
        let mut output = Output::typed(&data, text).limit(
            self.req.max_chars,
            "--max-chars 0 for all of it, or a locator for one part",
        );
        if hidden > 0 {
            output = output.warning(if data.texts.len() == 1 {
                format!("{} is hidden: it shows no text", self.locator_text())
            } else {
                format!(
                    "{hidden} of the {} matches are hidden: they show no text",
                    data.texts.len()
                )
            });
        }
        Ok(Self::noted(output, &called))
    }

    async fn html(&self, outer: bool, raw: bool) -> Result<Output, ErrorInfo> {
        let function = if raw {
            format!(
                "function () {{ if (this instanceof Document) {{\n\
                 const d = this.doctype ? `<!doctype ${{this.doctype.name}}>\\n` : '';\n\
                 return d + this.documentElement.outerHTML; }}\n\
                 return {}; }}",
                if outer {
                    "this.outerHTML"
                } else {
                    "this.innerHTML"
                }
            )
        } else {
            format!("function () {{ return globalThis.__ccdp.html(this, {outer}); }}")
        };
        let called = self.call_one(FrameOnly::Document, &function).await?;
        let html = called.value.as_str().unwrap_or_default().to_owned();
        let data = HtmlData {
            tab: self.tab(),
            locator: self.locator.map(ToString::to_string),
            outer: outer || self.locator.is_none(),
            raw,
            html: html.clone(),
        };
        let output = Output::typed(&data, html).limit(
            self.req.max_chars,
            "--max-chars 0 for all of it, or a locator for one part",
        );
        Ok(Self::noted(output, &called))
    }

    async fn attr(&self, name: &str) -> Result<Output, ErrorInfo> {
        let quoted = serde_json::to_string(name).unwrap_or_default();
        let function = format!(
            "function () {{ return {{ value: this.getAttribute({quoted}),\n\
             names: this.getAttributeNames ? this.getAttributeNames() : [] }}; }}"
        );
        let called = self.call_one(FrameOnly::Owner, &function).await?;
        let attribute: Attribute = parse(&called.value)?;
        let data = AttrData {
            tab: self.tab(),
            locator: self.locator_text(),
            name: name.to_owned(),
            value: attribute.value.clone(),
        };
        let mut output = Output::typed(&data, attribute.value.clone().unwrap_or_default())
            .limit(self.req.max_chars, "--max-chars 0 for all of it");
        if attribute.value.is_none() {
            let has = if attribute.names.is_empty() {
                "it has none".to_owned()
            } else {
                format!("it has {}", attribute.names.join(", "))
            };
            output = output.warning(format!(
                "{} has no attribute {name:?} ({has})",
                self.locator_text()
            ));
        }
        Ok(Self::noted(output, &called))
    }

    async fn value(&self) -> Result<Output, ErrorInfo> {
        let called = self
            .call_one(
                FrameOnly::Owner,
                "function () { return globalThis.__ccdp.value(this); }",
            )
            .await?;
        let field: FieldValue = parse(&called.value)?;
        if let Some(error) = field.error {
            let locator = self.locator_text();
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!("{locator} has no value: {error}"),
            )
            .hint(
                Hint::run(Cmd::new("text").arg(&locator).scoped(&self.engine.session))
                    .why("its text")
                    .or("name an input, textarea, select or editable element"),
            ));
        }
        let (value, values) = match field.value {
            Some(Value::Array(items)) => {
                let items: Vec<String> = items
                    .into_iter()
                    .map(|v| v.as_str().unwrap_or_default().to_owned())
                    .collect();
                (items.join("\n"), Some(items))
            }
            Some(Value::String(s)) => (s, None),
            _ => (String::new(), None),
        };
        let data = ValueData {
            tab: self.tab(),
            locator: self.locator_text(),
            kind: field.kind.unwrap_or_default(),
            value: value.clone(),
            values,
        };
        let output =
            Output::typed(&data, value).limit(self.req.max_chars, "--max-chars 0 for all of it");
        Ok(Self::noted(output, &called))
    }

    async fn count(&self) -> Result<Output, ErrorInfo> {
        let called = self
            .call(
                Want::All,
                FrameOnly::Owner,
                "function () { return { count: this.length,\n\
                 visible: this.filter((el) => globalThis.__ccdp.visible(el)).length }; }",
            )
            .await?;
        let counted: Counted = parse(&called.value)?;
        let data = CountData {
            tab: self.tab(),
            locator: self.locator_text(),
            count: counted.count,
            visible: counted.visible,
        };
        let text = if counted.visible == counted.count {
            counted.count.to_string()
        } else {
            format!("{} ({} visible)", counted.count, counted.visible)
        };
        let output = Output::typed(&data, text).quiet(counted.count.to_string());
        Ok(Self::noted(output, &called))
    }

    async fn bounding_box(&self) -> Result<Output, ErrorInfo> {
        let called = self
            .call_one(
                FrameOnly::Owner,
                "function () { if (!this.getClientRects().length) return null;\n\
                 const r = this.getBoundingClientRect();\n\
                 return [r.x, r.y, r.width, r.height]; }",
            )
            .await?;
        let rect: Option<[f64; 4]> = parse(&called.value)?;
        let rect = match rect {
            Some([x, y, width, height]) => {
                let (dx, dy) = self
                    .engine
                    .frame_offset(self.running, self.page, &called.frame, self.timeout)
                    .await?;
                Some(Rect {
                    x: round(x + dx),
                    y: round(y + dy),
                    width: round(width),
                    height: round(height),
                })
            }
            None => None,
        };
        let viewport: Viewport = {
            let called = self
                .engine
                .call_on(
                    self.running,
                    self.page,
                    None,
                    &self.options,
                    Want::One(None),
                    FrameOnly::Document,
                    "function () { return { width: innerWidth, height: innerHeight }; }",
                    self.timeout,
                )
                .await?;
            parse(&called.value)?
        };
        let in_viewport = rect.is_some_and(|r| {
            r.x < viewport.width
                && r.y < viewport.height
                && r.x + r.width > 0.0
                && r.y + r.height > 0.0
        });
        let data = BoxData {
            tab: self.tab(),
            locator: self.locator_text(),
            rect,
            in_viewport,
        };
        let (text, quiet) = match rect {
            Some(r) => {
                let mut text = format!("x={} y={} w={} h={}", r.x, r.y, r.width, r.height);
                if !in_viewport {
                    let _ = write!(
                        text,
                        " (outside the {}x{} viewport)",
                        viewport.width, viewport.height
                    );
                }
                (text, format!("{} {} {} {}", r.x, r.y, r.width, r.height))
            }
            None => (String::new(), String::new()),
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        if rect.is_none() {
            output = output.warning(format!(
                "{} is not rendered, so it has no box",
                self.locator_text()
            ));
        }
        Ok(Self::noted(output, &called))
    }

    async fn markdown(&self) -> Result<Output, ErrorInfo> {
        let called = if self.locator.is_some() {
            self.call_one(
                FrameOnly::Document,
                "function () { const r = globalThis.__ccdp.markdown(this);\n\
                 return { ...r, url: location.href, title: document.title }; }",
            )
            .await?
        } else {
            self.page_read(
                "function (owners) { const r = globalThis.__ccdp.markdown(this, owners);\n\
                 return { ...r, url: location.href, title: document.title }; }",
                Some("markdown"),
            )
            .await?
        };
        let readable: Readable = parse(&called.value)?;
        let data = MarkdownData {
            tab: self.tab(),
            url: readable.url,
            title: readable.title,
            locator: self.locator.map(ToString::to_string),
            source: readable.source,
            markdown: readable.markdown.clone(),
        };
        let mut output = Output::typed(&data, readable.markdown).limit(
            self.req.max_chars,
            "--max-chars 0 for all of it, or a locator for one part",
        );
        if data.markdown.is_empty() {
            output = output.warning(format!("{} has no readable text", data.source));
        }
        Ok(Self::noted(output, &called))
    }
}

/// Rounds CSS pixels to hundredths, so boxes print and compare stably.
fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Reads a bundle function's answer.
fn parse<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, ErrorInfo> {
    serde_json::from_value(value.clone()).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::ProtocolError,
            format!("unexpected answer from the page: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{check_read, round};
    use crate::request::{Read, ReadRequest};

    fn request(read: Read, locator: Option<&str>) -> ReadRequest {
        ReadRequest {
            read,
            locator: locator.map(str::to_owned),
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: 1000,
            max_chars: 0,
        }
    }

    #[test]
    fn element_reads_need_a_locator() {
        for read in [
            Read::Attr {
                name: "href".into(),
            },
            Read::Value,
            Read::Count,
            Read::Box,
        ] {
            let command = read.command();
            let err = check_read(&request(read, None)).unwrap_err();
            assert_eq!(err.message, format!("{command} needs a locator"));
            assert_eq!(err.exit_code(), 2);
        }
        for read in [
            Read::Text,
            Read::Markdown,
            Read::Html {
                outer: false,
                raw: false,
            },
        ] {
            check_read(&request(read, None)).unwrap();
        }
        let err = check_read(&request(Read::Text, Some("Save"))).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn boxes_round_to_hundredths() {
        assert!((round(10.004_999) - 10.0).abs() < f64::EPSILON);
        assert!((round(-3.456) - -3.46).abs() < f64::EPSILON);
    }
}
