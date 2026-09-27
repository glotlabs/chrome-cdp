//! `upload <loc> <file>...` (plan 05, milestone 5.4).
//!
//! A file input (or a `<label>` of one) gets the files straight from
//! `DOM.setFileInputFiles`, which fires `input` and `change` as a user's
//! choice does; it may be hidden, as most upload widgets hide theirs, but
//! must be enabled. Anything else is a button that opens a file chooser:
//! the chooser is intercepted (`Page.setInterceptFileChooserDialog`), the
//! element clicked like `click` does, and the files go to the input the
//! chooser is for. `--clear` gives the input no files.

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::dom::{ResolveNodeParams, SetFileInputFilesParams};
use cdp::domains::page::{
    FileChooserOpened, FileChooserOpenedMode, SetInterceptFileChooserDialogParams,
};
use cdp::domains::runtime;
use protocol::schema::{ActedElement, UploadData, UploadedFile};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use super::{Actor, Checks, describe};
use crate::errors::from_cdp;
use crate::locator::release;
use crate::request::{Action, Button};
use crate::snapshot::Target;
use crate::tabs::FrameContext;

/// How long `upload` waits for a click to open a file chooser.
const CHOOSER_WAIT: Duration = Duration::from_secs(3);

/// How often `upload` looks again at a disabled file input.
const POLL: Duration = Duration::from_millis(50);

/// How many files the text output names.
const SHOWN_FILES: usize = 10;

/// What giving an input its files did.
struct Filled {
    /// It held other files before.
    changed: bool,
    /// Its `accept` attribute.
    accept: String,
}

/// What the page says about the element `upload` got.
#[derive(Debug, Default, Deserialize)]
struct Probe {
    /// It is a file input, or a label of one.
    #[serde(default)]
    file: bool,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    disabled: bool,
}

/// Is `this` a file input or a label of one, and what does it take.
const PROBE: &str = "function () {\n\
    const input = this.localName === 'label' && this.control ? this.control : this;\n\
    const file = input.localName === 'input' && input.type === 'file';\n\
    return { file, multiple: file && input.multiple, disabled: file && input.disabled };\n\
}";

/// The file input of `this` (itself, or the control of a label).
const INPUT: &str = "function () {\n\
    return this.localName === 'label' && this.control ? this.control : this;\n\
}";

/// Clears the file input of `this` as Playwright does (a user cannot):
/// no files, then `input` and `change`. Whether it held any.
const CLEAR: &str = "function () {\n\
    const input = this.localName === 'label' && this.control ? this.control : this;\n\
    if (!input.files || !input.files.length) return false;\n\
    input.files = new DataTransfer().files;\n\
    input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));\n\
    input.dispatchEvent(new Event('change', { bubbles: true }));\n\
    return true;\n\
}";

/// Checks the files of `upload` without a browser: they are there and are
/// files. Paths must be absolute (the CLI makes them so).
pub(super) fn check_files(files: &[String]) -> Result<Vec<UploadedFile>, ErrorInfo> {
    files
        .iter()
        .map(|path| {
            let bad = |why: &str| {
                ErrorInfo::new(ErrorKind::BadArgs, format!("{path} {why}"))
                    .hint(Hint::text("upload takes the paths of files that exist"))
                    .details(json!({ "path": path }))
            };
            let as_path = std::path::Path::new(path);
            if !as_path.is_absolute() {
                return Err(bad("is not an absolute path"));
            }
            let meta = std::fs::metadata(as_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    bad("does not exist")
                } else {
                    bad(&format!("cannot be read: {e}"))
                }
            })?;
            if meta.is_dir() {
                return Err(bad("is a directory; upload takes files"));
            }
            let name = as_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(UploadedFile {
                path: path.clone(),
                name,
                size: meta.len(),
            })
        })
        .collect()
}

impl Actor<'_> {
    /// `upload <loc> <file>...` and `upload <loc> --clear`.
    pub(super) async fn upload(&self) -> Result<Output, ErrorInfo> {
        let Action::Upload {
            locator,
            files,
            clear,
            force,
        } = &self.req.action
        else {
            unreachable!("not upload")
        };
        let files = if *clear {
            Vec::new()
        } else {
            check_files(files)?
        };
        let parsed = self.parse(locator)?;
        loop {
            let target = self.locate_waiting(&parsed).await?;
            let probe = self.call(&target, PROBE, Vec::new()).await;
            let probe: Probe = match probe {
                Ok(value) => serde_json::from_value(value).unwrap_or_default(),
                Err(err) => {
                    target.release();
                    return Err(err);
                }
            };
            if !probe.file {
                target.release();
                return self.upload_through_chooser(locator, files, *force).await;
            }
            let element = match self.name(&target).await {
                Ok(element) => element,
                Err(err) => {
                    target.release();
                    return Err(err);
                }
            };
            if probe.disabled && !*force {
                target.release();
                if Instant::now() + POLL < self.deadline {
                    tokio::time::sleep(POLL).await;
                    continue;
                }
                return Err(ErrorInfo::new(
                    ErrorKind::NotEnabled,
                    format!("{} is disabled", describe(&element)),
                )
                .note("waited", protocol::format::duration(self.timeout))
                .hint(
                    Hint::run(Cmd::new("snapshot").session(self.session()))
                        .why("a control is often disabled until the form is complete"),
                )
                .details(json!({ "locator": locator, "check": "enabled" })));
            }
            if !probe.multiple && files.len() > 1 {
                target.release();
                return Err(self.one_file(locator, &element, files.len()));
            }
            let set = self.input_of(&target).await;
            let set = match set {
                Ok(input) => self.fill_input(&target.context, input, &files).await,
                Err(err) => Err(err),
            };
            let note = target.note.clone();
            target.release();
            let filled = set?;
            let mut output = self.uploaded(locator, element, files, false, &filled);
            if let Some(note) = note {
                output = output.warning(note);
            }
            return Ok(output);
        }
    }

    /// The file input of `target` (itself, or the control of its label),
    /// as a new handle.
    async fn input_of(&self, target: &Target) -> Result<String, ErrorInfo> {
        let session = target.context.session.with_timeout(self.timeout);
        let mut params = runtime::CallFunctionOnParams::new(INPUT.to_owned());
        params.object_id = Some(target.object_id.clone());
        let input = session
            .call(params)
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        input.result.object_id.ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                "finding the file input gave nothing",
            )
        })
    }

    /// Gives the file input `input` (a handle in `context`, released
    /// here) the files, or clears it for none.
    async fn fill_input(
        &self,
        context: &FrameContext,
        input: String,
        files: &[UploadedFile],
    ) -> Result<Filled, ErrorInfo> {
        let session = context.session.with_timeout(self.timeout);
        let mut params =
            runtime::CallFunctionOnParams::new("function () { return this.accept; }".to_owned());
        params.object_id = Some(input.clone());
        params.return_by_value = Some(true);
        let accept = match session.call(params).await {
            Ok(r) => r
                .result
                .value
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            Err(e) => {
                release(context, input);
                return Err(from_cdp(&e, self.session()));
            }
        };
        let result = if files.is_empty() {
            // `DOM.setFileInputFiles` ignores an empty list.
            let mut params = runtime::CallFunctionOnParams::new(CLEAR.to_owned());
            params.object_id = Some(input.clone());
            params.return_by_value = Some(true);
            session
                .call(params)
                .await
                .map(|r| r.result.value == Some(serde_json::Value::Bool(true)))
        } else {
            let mut params = SetFileInputFilesParams::new(paths(files));
            params.object_id = Some(input.clone());
            session.call(params).await.map(|_| true)
        };
        release(context, input);
        let changed = result.map_err(|e| from_cdp(&e, self.session()))?;
        Ok(Filled { changed, accept })
    }

    /// Clicks the element `locator` names, and gives the files to the file
    /// chooser that opens.
    async fn upload_through_chooser(
        &self,
        locator: &str,
        files: Vec<UploadedFile>,
        force: bool,
    ) -> Result<Output, ErrorInfo> {
        let parsed = self.parse(locator)?;
        let checks = Checks {
            enabled: true,
            force,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        let (x, y) = (ready.ready.x, ready.ready.y);
        let element = ready.element.clone();
        let note = ready.ready.target.note.clone();
        // The chooser opens in the element's frame (its own target when
        // out of process).
        let context = ready.ready.target.context.clone();
        let session = context.session.with_timeout(self.timeout);
        ready.release();
        let mut choosers = session.listen::<FileChooserOpened>();
        session
            .call(SetInterceptFileChooserDialogParams::new(true))
            .await
            .map_err(|e| from_cdp(&e, self.session()))?;
        let result = async {
            self.move_to(x, y, 1, 0).await?;
            self.press(Button::Left, true, 1, 0).await?;
            self.press(Button::Left, false, 1, 0).await?;
            let wait = CHOOSER_WAIT.min(self.left());
            let Ok(Some(chooser)) = tokio::time::timeout(wait, choosers.next()).await else {
                return Err(self.no_chooser(locator, &element));
            };
            let Some(backend_node_id) = chooser.backend_node_id else {
                return Err(ErrorInfo::new(
                    ErrorKind::NotEnabled,
                    format!(
                        "clicking {} opened a file picker that is not a file input's \
                         (showOpenFilePicker), which chrome-cdp cannot fill",
                        describe(&element)
                    ),
                )
                .hint(
                    Hint::run(
                        Cmd::new("upload")
                            .arg("css=input[type=file]")
                            .raw("<file>")
                            .session(self.session()),
                    )
                    .why("the page's file input, if it has one"),
                ));
            };
            if chooser.mode == FileChooserOpenedMode::SelectSingle && files.len() > 1 {
                return Err(self.one_file(locator, &element, files.len()));
            }
            let input = session
                .call(ResolveNodeParams {
                    node_id: None,
                    backend_node_id: Some(backend_node_id),
                    object_group: None,
                    execution_context_id: Some(context.context_id),
                })
                .await
                .map_err(|e| from_cdp(&e, self.session()))?
                .object
                .object_id
                .ok_or_else(|| {
                    ErrorInfo::new(
                        ErrorKind::ProtocolError,
                        "finding the file chooser's input gave nothing",
                    )
                })?;
            self.fill_input(&context, input, &files).await
        }
        .await;
        let _ = session
            .call(SetInterceptFileChooserDialogParams::new(false))
            .await;
        let filled = result?;
        let mut output = self.uploaded(locator, element, files, true, &filled);
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// The output of an upload.
    fn uploaded(
        &self,
        locator: &str,
        element: ActedElement,
        files: Vec<UploadedFile>,
        chooser: bool,
        filled: &Filled,
    ) -> Output {
        let described = describe(&element);
        let refused: Vec<&str> = files
            .iter()
            .filter(|f| refused(&filled.accept, &f.name))
            .map(|f| f.name.as_str())
            .collect();
        let warning = (!refused.is_empty()).then(|| {
            format!(
                "the input accepts {}; the page may refuse {}",
                filled.accept.trim(),
                refused.join(", ")
            )
        });
        let text = if files.is_empty() && !filled.changed {
            format!("{described} had no files")
        } else if files.is_empty() {
            format!("cleared the files of {described}")
        } else {
            let listed = file_list(&files);
            let how = if chooser {
                "through the file chooser of"
            } else {
                "to"
            };
            if files.len() == 1 {
                format!("uploaded {listed} {how} {described}")
            } else {
                format!("uploaded {} files {how} {described}: {listed}", files.len())
            }
        };
        let quiet = element
            .reference
            .clone()
            .unwrap_or_else(|| "uploaded".to_owned());
        let data = UploadData {
            tab: self.tab(),
            locator: locator.to_owned(),
            element,
            files,
            chooser,
        };
        let output = Output::typed(&data, text).quiet(quiet);
        match warning {
            Some(warning) => output.warning(warning),
            None => output,
        }
    }

    /// More files than a single-file input takes.
    fn one_file(&self, locator: &str, element: &ActedElement, count: usize) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} takes one file, not {count}", describe(element)),
        )
        .hint(Hint::run(
            Cmd::new("upload")
                .arg(locator)
                .raw("<file>")
                .session(self.session()),
        ))
        .details(json!({ "locator": locator, "files": count }))
    }

    /// A click that opened no file chooser.
    fn no_chooser(&self, locator: &str, element: &ActedElement) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!(
                "clicking {} opened no file chooser, and it is not a file input",
                describe(element)
            ),
        )
        .note(
            "waited",
            protocol::format::duration(CHOOSER_WAIT.min(self.timeout)),
        )
        .hint(
            Hint::run(
                Cmd::new("upload")
                    .arg("css=input[type=file]")
                    .raw("<file>")
                    .session(self.session()),
            )
            .why("the page's file input, even a hidden one"),
        )
        .details(json!({ "locator": locator, "check": "chooser" }))
    }
}

/// Whether an input with this `accept` attribute surely does not take a
/// file named `name` (a type it names by extension or MIME type; an
/// extension this does not know is given the benefit of the doubt).
fn refused(accept: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let mime = name
        .rsplit_once('.')
        .and_then(|(_, extension)| mime_type(extension));
    let mut tokens = accept
        .split(',')
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .peekable();
    if tokens.peek().is_none() {
        return false;
    }
    !tokens.any(|token| {
        if token.starts_with('.') {
            name.ends_with(&token)
        } else if let Some(mime) = mime {
            match token.strip_suffix('*') {
                Some(prefix) => mime.starts_with(prefix),
                None => mime == token,
            }
        } else {
            true
        }
    })
}

/// The MIME type of common file extensions.
fn mime_type(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "xml" => "application/xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

fn paths(files: &[UploadedFile]) -> Vec<String> {
    files.iter().map(|f| f.path.clone()).collect()
}

/// `a.txt (5B), b.png (12KB)`, at most [`SHOWN_FILES`] of them.
fn file_list(files: &[UploadedFile]) -> String {
    let mut text = files
        .iter()
        .take(SHOWN_FILES)
        .map(|f| format!("{} ({})", f.name, protocol::format::size(f.size)))
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > SHOWN_FILES {
        let _ = write!(text, ", … {} more", files.len() - SHOWN_FILES);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> UploadedFile {
        UploadedFile {
            path: format!("/tmp/{name}"),
            name: name.to_owned(),
            size,
        }
    }

    #[test]
    fn files_are_listed_with_their_sizes() {
        assert_eq!(file_list(&[file("a.txt", 5)]), "a.txt (5B)");
        let many: Vec<UploadedFile> = (0..12).map(|i| file(&format!("{i}.txt"), 2048)).collect();
        let listed = file_list(&many);
        assert!(listed.starts_with("0.txt (2KB), 1.txt (2KB)"), "{listed}");
        assert!(listed.ends_with("9.txt (2KB), … 2 more"), "{listed}");
    }

    #[test]
    fn accept_rules_out_other_types() {
        assert!(!refused("", "a.txt"));
        assert!(!refused("image/*", "a.PNG"));
        assert!(refused("image/*", "notes.md"));
        assert!(!refused("image/png, .pdf", "report.pdf"));
        assert!(refused("image/png,.pdf", "photo.jpg"));
        assert!(!refused("application/vnd.custom", "data.bin"));
        assert!(!refused("text/*", "data.bin"));
        assert!(refused(".csv", "data.bin"));
    }

    #[test]
    fn missing_and_relative_files_are_bad_args() {
        let err = check_files(&["relative.txt".to_owned()]).unwrap_err();
        assert_eq!(err.message, "relative.txt is not an absolute path");
        let missing = std::env::temp_dir().join("chrome-cdp-no-such-file.txt");
        let missing = missing.to_string_lossy().into_owned();
        let err = check_files(std::slice::from_ref(&missing)).unwrap_err();
        assert_eq!(err.message, format!("{missing} does not exist"));
        assert_eq!(err.exit_code(), 2);
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let err = check_files(std::slice::from_ref(&dir)).unwrap_err();
        assert!(err.message.ends_with("is a directory; upload takes files"));
    }
}
