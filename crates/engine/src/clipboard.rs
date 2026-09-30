//! `clipboard read` and `clipboard write` (plan 05, milestone 5.4; HTML
//! and images, plan 14, milestone 14.3).
//!
//! The page's own async clipboard API (`navigator.clipboard`) does the
//! work, in chrome-cdp's isolated world of the tab's main frame, so the
//! text is what a page's paste reads and a `Control+V` pastes. The API
//! needs two things a user gives and an automated tab lacks: permission
//! (granted to the page's origin with `Browser.setPermission`, and kept,
//! as a user's "allow" is) and a focused document (emulated with
//! `Emulation.setFocusEmulationEnabled` for the call only, the page
//! brought to the front: a page behind another reads an empty clipboard
//! even with focus emulated). The grants
//! are the session's, so plan 07's `permissions` lists them.
//!
//! Text alone goes through `readText`/`writeText`; HTML and PNG images
//! through `read()`/`write()` and a `ClipboardItem`, the types Chrome's
//! async clipboard takes without the unsanitized-write permission.

use std::fmt::Write as _;
use std::path::Path;

use base64::Engine as _;
use cdp::CdpError;
use cdp::domains::browser::{PermissionDescriptor, PermissionSetting, SetPermissionParams};
use cdp::domains::emulation::SetFocusEmulationEnabledParams;
use cdp::domains::page;
use cdp::domains::runtime::{self, CallFunctionOnReturns};
use protocol::schema::{ClipboardReadData, ClipboardWriteData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::observe::save::save;
use crate::observe::screenshot::dimensions;
use crate::request::{ClipboardOp, ClipboardRequest, millis};
use crate::tabs::{Page, tab_name};
use crate::{Engine, ImageFormat, Running};

/// The permissions the async clipboard API asks for.
const PERMISSIONS: [&str; 2] = ["clipboard-read", "clipboard-write"];

/// Where the page is, as the clipboard API sees it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageOrigin {
    origin: String,
    secure: bool,
    url: String,
}

impl Engine {
    /// Reads or writes the clipboard through the page in the tab
    /// `req` names, holding the tab's lock.
    pub async fn clipboard(&self, req: ClipboardRequest) -> Result<Output, ErrorInfo> {
        if let ClipboardOp::Read {
            mime: Some(mime), ..
        } = &req.op
            && !TYPES.contains(&mime.as_str())
        {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read {mime}: the types are {}", TYPES.join(", ")),
            ));
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let _tab = self.tabs.lock(&page.target_id).await;
        let work = self.use_clipboard(&running, &page, &req);
        let output = self.unless_blocked(&running, &page, mark, work).await?;
        Ok(self.report_events(&running, mark, output, None).await)
    }

    async fn use_clipboard(
        &self,
        running: &Running,
        page: &Page,
        req: &ClipboardRequest,
    ) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let context = self
            .frame_context(running, page, None, World::Isolated, timeout)
            .await?;
        let session = context.session.with_timeout(timeout);
        let call = |function: &str, arguments: Vec<Value>| {
            let mut params = runtime::CallFunctionOnParams::new(function.to_owned());
            params.execution_context_id = Some(context.context_id);
            params.arguments = Some(
                arguments
                    .into_iter()
                    .map(|value| runtime::CallArgument {
                        value: Some(value),
                        unserializable_value: None,
                        object_id: None,
                    })
                    .collect(),
            );
            params.return_by_value = Some(true);
            params.await_promise = Some(true);
            session.call(params)
        };
        let origin = call(
            "function () { return { origin: location.origin, secure: isSecureContext, url: location.href }; }",
            Vec::new(),
        )
        .await
        .map_err(|e| from_cdp(&e, &self.session))?;
        let origin: PageOrigin = origin
            .result
            .value
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "reading the page's origin gave nothing",
                )
            })?;
        if origin.origin == "null" || !origin.secure {
            return Err(self.no_clipboard(page, &origin));
        }
        let browser = running.conn.browser().with_timeout(timeout);
        // The page is of the call's context, whose permissions they are.
        let scope = self.context_key()?;
        for name in PERMISSIONS {
            let mut params = SetPermissionParams::new(
                PermissionDescriptor {
                    name: name.to_owned(),
                    sysex: None,
                    user_visible_only: None,
                    allow_without_sanitization: None,
                    allow_without_gesture: None,
                    pan_tilt_zoom: None,
                },
                PermissionSetting::Granted,
            );
            params.origin = Some(origin.origin.clone());
            params.browser_context_id.clone_from(&scope);
            browser
                .call(params)
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            // `permissions` lists it, and can take it back.
            self.settings.emulation.record_permission(
                scope.as_deref(),
                name,
                "granted",
                Some(&origin.origin),
            );
        }
        let page_session = page.session.with_timeout(timeout);
        // A page behind another reads an empty clipboard, focus emulation
        // or not: bring it to the front for the call.
        page_session
            .call(page::BringToFrontParams::default())
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        page_session
            .call(SetFocusEmulationEnabledParams::new(true))
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let output = match &req.op {
            ClipboardOp::Read { mime, path } => {
                let mime = mime.as_deref().unwrap_or(TEXT);
                if mime == TEXT {
                    self.read_text(&call, page, &origin, req.max_chars).await
                } else {
                    self.read_type(&call, page, &origin, mime, path.as_deref())
                        .await
                }
            }
            ClipboardOp::Write { text, html, image } => {
                self.write_item(
                    &call,
                    page,
                    &origin,
                    text.as_deref(),
                    html.as_deref(),
                    image.as_deref(),
                )
                .await
            }
        };
        // Focus goes back to what it really is (a later click may give
        // the page focus, as it does a user's).
        let _ = page_session
            .call(SetFocusEmulationEnabledParams::new(false))
            .await;
        // `--tab` named another tab than the active one: that one goes
        // back to the front.
        if let Some(active) = running.tabs.active().filter(|&id| id != page.id) {
            let _ = running.tabs.bring_to_front(active, timeout).await;
        }
        output
    }

    /// `clipboard read`: the text, through `readText` (an empty clipboard
    /// reads as nothing, with a warning).
    async fn read_text<F>(
        &self,
        call: &impl Fn(&str, Vec<Value>) -> F,
        page: &Page,
        origin: &PageOrigin,
        max_chars: usize,
    ) -> Result<Output, ErrorInfo>
    where
        F: Future<Output = Result<CallFunctionOnReturns, CdpError>>,
    {
        let value = self
            .run(
                call(
                    "function () { return navigator.clipboard.readText(); }",
                    Vec::new(),
                ),
                origin,
                "read",
            )
            .await?;
        let text = value
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut output = Output::typed(
            &ClipboardReadData {
                tab: tab_name(page.id),
                text: text.clone(),
                mime: None,
                path: None,
                bytes: None,
            },
            text.clone(),
        )
        .quiet(text.clone())
        .limit(max_chars, "--max-chars 0 for all of it");
        if text.is_empty() {
            output = output.warning("the clipboard holds no text");
        }
        Ok(output)
    }

    /// `clipboard read --type T`: the first item's part of type `mime`,
    /// through `read()`. HTML is printed; an image is saved.
    async fn read_type<F>(
        &self,
        call: &impl Fn(&str, Vec<Value>) -> F,
        page: &Page,
        origin: &PageOrigin,
        mime: &str,
        path: Option<&Path>,
    ) -> Result<Output, ErrorInfo>
    where
        F: Future<Output = Result<CallFunctionOnReturns, CdpError>>,
    {
        let value = self
            .run(call(READ_TYPE, vec![json!(mime)]), origin, "read")
            .await?;
        let read: TypeRead = value
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    "reading the clipboard gave nothing",
                )
            })?;
        let tab = tab_name(page.id);
        if let Some(text) = read.text {
            return Ok(Output::typed(
                &ClipboardReadData {
                    tab,
                    text: text.clone(),
                    mime: Some(mime.to_owned()),
                    path: None,
                    bytes: None,
                },
                text.clone(),
            )
            .quiet(text));
        }
        let Some(base64) = read.base64 else {
            return Err(not_on_clipboard(mime, &read.types, &self.session));
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64)
            .map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("the page sent an image that is not base64: {e}"),
                )
            })?;
        let saved = save(&bytes, path, &self.output_dir().join("clipboard"), "png")?;
        let shown = saved.display().to_string();
        let size = bytes.len() as u64;
        let mut text = format!("{shown}: {mime}");
        if let Some((width, height)) = dimensions(ImageFormat::Png, &bytes) {
            let _ = write!(text, " {width}x{height}");
        }
        let _ = write!(text, ", {}", format::size(size));
        Ok(Output::typed(
            &ClipboardReadData {
                tab,
                text: String::new(),
                mime: Some(mime.to_owned()),
                path: Some(shown.clone()),
                bytes: Some(size),
            },
            text,
        )
        .quiet(shown))
    }

    /// `clipboard write`: text alone through `writeText`; HTML or an image
    /// as one `ClipboardItem` with every part given.
    async fn write_item<F>(
        &self,
        call: &impl Fn(&str, Vec<Value>) -> F,
        page: &Page,
        origin: &PageOrigin,
        text: Option<&str>,
        html: Option<&str>,
        image: Option<&Path>,
    ) -> Result<Output, ErrorInfo>
    where
        F: Future<Output = Result<CallFunctionOnReturns, CdpError>>,
    {
        let tab = tab_name(page.id);
        if html.is_none() && image.is_none() {
            let text = text.unwrap_or_default();
            self.run(
                call(
                    "function (text) { return navigator.clipboard.writeText(text); }",
                    vec![json!(text)],
                ),
                origin,
                "write",
            )
            .await?;
            let chars = text.chars().count();
            let plural = if chars == 1 { "" } else { "s" };
            return Ok(Output::typed(
                &ClipboardWriteData {
                    tab,
                    chars,
                    types: vec![TEXT.to_owned()],
                },
                format!("wrote {chars} character{plural} to the clipboard"),
            )
            .quiet(""));
        }
        let png = image.map(read_png).transpose()?;
        let encoded = png
            .as_ref()
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
        let value = self
            .run(
                call(WRITE_ITEM, vec![json!(text), json!(html), json!(encoded)]),
                origin,
                "write",
            )
            .await?;
        let written = value
            .as_ref()
            .and_then(Value::as_str)
            .map(|t| t.chars().count());
        let mut types = Vec::new();
        let mut parts = Vec::new();
        if let Some(html) = html {
            types.push(HTML.to_owned());
            parts.push(format!("{HTML} ({})", format::size(html.len() as u64)));
        }
        if let Some(chars) = written {
            let plural = if chars == 1 { "" } else { "s" };
            types.push(TEXT.to_owned());
            parts.push(format!("{TEXT} ({chars} character{plural})"));
        }
        if let Some(png) = &png {
            types.push(PNG.to_owned());
            let mut part = format!("{PNG} (");
            if let Some((width, height)) = dimensions(ImageFormat::Png, png) {
                let _ = write!(part, "{width}x{height}, ");
            }
            let _ = write!(part, "{})", format::size(png.len() as u64));
            parts.push(part);
        }
        Ok(Output::typed(
            &ClipboardWriteData {
                tab,
                chars: written.unwrap_or_default(),
                types,
            },
            format!("wrote {} to the clipboard", parts.join(", ")),
        )
        .quiet(""))
    }

    /// Awaits a clipboard call in the page: its value, or the page's
    /// exception as an error.
    async fn run(
        &self,
        call: impl Future<Output = Result<CallFunctionOnReturns, CdpError>>,
        origin: &PageOrigin,
        verb: &str,
    ) -> Result<Option<Value>, ErrorInfo> {
        let result = call.await.map_err(|e| from_cdp(&e, &self.session))?;
        if let Some(exception) = result.exception_details {
            let message = exception
                .exception
                .and_then(|e| e.description)
                .unwrap_or(exception.text);
            let first = message.lines().next().unwrap_or_default();
            return Err(ErrorInfo::new(
                ErrorKind::NotEnabled,
                format!("the page could not {verb} the clipboard: {first}"),
            )
            .note("page", origin.url.clone())
            .hint(Hint::text(
                "the page may block the clipboard (a Permissions-Policy header); \
                 use press Mod+C / Mod+V in a field instead",
            )));
        }
        Ok(result.result.value)
    }

    /// The error for a page the clipboard API is not available to.
    fn no_clipboard(&self, page: &Page, origin: &PageOrigin) -> ErrorInfo {
        let why = if origin.origin == "null" {
            "it has no origin"
        } else {
            "it is not a secure context (https, or http on localhost)"
        };
        ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!(
                "the page in tab {} cannot use the clipboard: {why}",
                tab_name(page.id)
            ),
        )
        .note("page", origin.url.clone())
        .hint(
            Hint::run(Cmd::new("goto").raw("<https-url>").scoped(&self.session))
                .why("the clipboard works through the page"),
        )
        .details(json!({ "url": origin.url, "origin": origin.origin }))
    }
}

/// The types `clipboard` reads and writes.
const TEXT: &str = "text/plain";
const HTML: &str = "text/html";
const PNG: &str = "image/png";

/// The types `clipboard read --type` takes.
const TYPES: [&str; 3] = [TEXT, HTML, PNG];

/// Reads the first clipboard item that has the type given: `{ types,
/// text }` for a text type, `{ types, base64 }` for an image, `{ types }`
/// when no item has it.
const READ_TYPE: &str = r"async function (type) {
  const items = await navigator.clipboard.read();
  const types = [...new Set(items.flatMap((item) => [...item.types]))];
  const item = items.find((item) => item.types.includes(type));
  if (!item) return { types };
  const blob = await item.getType(type);
  if (type.startsWith('text/')) return { types, text: await blob.text() };
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return { types, base64: btoa(binary) };
}";

/// Writes one `ClipboardItem` with the parts given (`null` for none; it
/// may arrive as `undefined`): the HTML's text stands in for missing
/// text. Returns the text written.
const WRITE_ITEM: &str = r"async function (text, html, png) {
  const parts = {};
  if (html != null) {
    parts['text/html'] = new Blob([html], { type: 'text/html' });
    if (text == null) {
      text = new DOMParser().parseFromString(html, 'text/html').body.textContent;
    }
  }
  if (text != null) parts['text/plain'] = new Blob([text], { type: 'text/plain' });
  if (png != null) {
    const binary = atob(png);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    parts['image/png'] = new Blob([bytes], { type: 'image/png' });
  }
  await navigator.clipboard.write([new ClipboardItem(parts)]);
  return text ?? null;
}";

/// What [`READ_TYPE`] found.
#[derive(Debug, Deserialize)]
struct TypeRead {
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    base64: Option<String>,
}

/// The error for a type the clipboard does not hold.
fn not_on_clipboard(mime: &str, types: &[String], session: &str) -> ErrorInfo {
    let has = if types.is_empty() {
        "the clipboard is empty".to_owned()
    } else {
        format!("the clipboard has {}", types.join(", "))
    };
    let mut err = ErrorInfo::new(
        ErrorKind::NotFound,
        format!("the clipboard holds no {mime}: {has}"),
    )
    .details(json!({ "type": mime, "types": types }));
    if let Some(first) = types.first() {
        let mut read = Cmd::new("clipboard read").flag("type", first);
        if !first.starts_with("text/") {
            read = read.raw("-o <file.png>");
        }
        err = err.hint(Hint::run(read.scoped(session)));
    }
    err
}

/// The PNG `clipboard write --file` puts on the clipboard: Chrome's async
/// clipboard takes no other image type.
fn read_png(path: &Path) -> Result<Vec<u8>, ErrorInfo> {
    let is_png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{} is not a PNG: the clipboard takes image/png only",
                path.display()
            ),
        )
        .hint(Hint::text("convert the image to PNG first")));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} is not a PNG image", path.display()),
        ));
    }
    Ok(bytes)
}
