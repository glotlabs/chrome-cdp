//! `upload <loc> <file>...` (plan 05, milestone 5.4).
//!
//! A file input (or a `<label>` of one) gets the files straight from
//! `DOM.setFileInputFiles`, which fires `input` and `change` as a user's
//! choice does; it may be hidden, as most upload widgets hide theirs, but
//! must be enabled. Anything else is a button that opens a file chooser:
//! the chooser is intercepted (`Page.setInterceptFileChooserDialog`), the
//! element clicked like `click` does, and the files go to the input the
//! chooser is for. `--clear` gives the input no files.
//!
//! `--drop` is for drop zones with no file input: the element goes
//! through the pipeline like `click`, and the files are dropped on it
//! with drag events (`Input.dispatchDragEvent`: enter, over, drop, their
//! `dataTransfer.files` holding them), as a user drags them in from the
//! desktop. The page must take the drop (cancel `dragover`), or no `drop`
//! fires and `upload` fails.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use cdp::domains::dom::{ResolveNodeParams, SetFileInputFilesParams};
use cdp::domains::input::{DispatchDragEventParams, DispatchDragEventType, DragData};
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
use crate::context::Scoped as _;
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
    /// It takes a directory (`webkitdirectory`).
    #[serde(default)]
    directory: bool,
}

/// Is `this` a file input or a label of one, and what does it take.
pub(crate) const PROBE: &str = "function () {\n\
    const input = this.localName === 'label' && this.control ? this.control : this;\n\
    const file = input.localName === 'input' && input.type === 'file';\n\
    return {\n\
        file,\n\
        multiple: file && input.multiple,\n\
        disabled: file && input.disabled,\n\
        directory: file && input.webkitdirectory,\n\
    };\n\
}";

/// The file input of `this` (itself, or the control of a label).
const INPUT: &str = "function () {\n\
    return this.localName === 'label' && this.control ? this.control : this;\n\
}";

/// Clears the file input of `this` (a user cannot):
/// no files, then `input` and `change`. Whether it held any.
const CLEAR: &str = "function () {\n\
    const input = this.localName === 'label' && this.control ? this.control : this;\n\
    if (!input.files || !input.files.length) return false;\n\
    input.files = new DataTransfer().files;\n\
    input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));\n\
    input.dispatchEvent(new Event('change', { bubbles: true }));\n\
    return true;\n\
}";

/// Watches the drop and dragover events of `this`'s window until
/// [`DROPPED`] asks: the capture listeners see every one, even one the
/// page stops.
const WATCH_DROP: &str = "function () {\n\
    const view = this.ownerDocument.defaultView;\n\
    const watch = { drops: [], overs: [] };\n\
    watch.drop = (e) => watch.drops.push(e);\n\
    watch.over = (e) => watch.overs.push(e);\n\
    watch.stop = () => {\n\
        view.removeEventListener('drop', watch.drop, true);\n\
        view.removeEventListener('dragover', watch.over, true);\n\
    };\n\
    view.addEventListener('drop', watch.drop, true);\n\
    view.addEventListener('dragover', watch.over, true);\n\
    this[Symbol.for('chrome-cdp.drop')] = watch;\n\
}";

/// Whether the page took the dragover in `this`'s window since
/// [`WATCH_DROP`] (cancelled it, read once it has run its course), as
/// it must for a drop. Chrome fires it after `Input.dispatchDragEvent`
/// returns, and not at all when a drop target the page showed on
/// dragenter (an overlay) is not laid out yet: `null` until it comes.
const ACCEPTED: &str = "function () {\n\
    const watch = this[Symbol.for('chrome-cdp.drop')];\n\
    const over = watch && watch.overs[watch.overs.length - 1];\n\
    return over ? over.defaultPrevented : null;\n\
}";

/// How long a drop waits for its dragover to reach the page.
const DRAGOVER_WAIT: Duration = Duration::from_millis(300);

/// Whether a `drop` fired in `this`'s window since [`WATCH_DROP`], which
/// stops watching. Anywhere: many pages show an overlay on `dragenter`
/// that takes the drop in the element's place.
const DROPPED: &str = "function () {\n\
    const key = Symbol.for('chrome-cdp.drop');\n\
    const watch = this[key];\n\
    if (!watch) return false;\n\
    watch.stop();\n\
    delete this[key];\n\
    return watch.drops.length > 0;\n\
}";

/// `dragOperationsMask`: copy, as files dragged from the desktop offer.
const DRAG_COPY: i64 = 1;

/// At most this many files go up in one directory.
const MAX_DIRECTORY_FILES: usize = 10_000;

/// What `upload` gives the page: files, or one directory's.
#[derive(Debug, Default)]
pub(super) struct Upload {
    /// The files (a directory's, recursively, sorted by relative path).
    files: Vec<UploadedFile>,
    /// The directory, for a `webkitdirectory` input (absolute).
    directory: Option<String>,
}

impl Upload {
    /// The paths `DOM.setFileInputFiles` takes: the directory itself
    /// (Chrome lists it and sets each file's `webkitRelativePath`), or
    /// the files.
    fn paths(&self) -> Vec<String> {
        match &self.directory {
            Some(directory) => vec![directory.clone()],
            None => paths(&self.files),
        }
    }
}

/// Checks the files of `upload` without a browser: they are there and are
/// files, or they are one directory (with at most
/// [`MAX_DIRECTORY_FILES`] files). Paths must be absolute (the CLI makes
/// them so).
pub(super) fn check_files(files: &[String]) -> Result<Upload, ErrorInfo> {
    let bad = |path: &str, why: &str| {
        ErrorInfo::new(ErrorKind::BadArgs, format!("{path} {why}"))
            .hint(Hint::text("upload takes the paths of files that exist"))
            .details(json!({ "path": path }))
    };
    let mut checked = Vec::new();
    for path in files {
        let as_path = Path::new(path);
        if !as_path.is_absolute() {
            return Err(bad(path, "is not an absolute path"));
        }
        let meta = std::fs::metadata(as_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                bad(path, "does not exist")
            } else {
                bad(path, &format!("cannot be read: {e}"))
            }
        })?;
        if meta.is_dir() {
            if files.len() > 1 {
                return Err(bad(
                    path,
                    "is a directory; upload takes one directory alone, or files",
                ));
            }
            return directory(path);
        }
        checked.push(UploadedFile {
            path: path.clone(),
            name: file_name(as_path),
            size: meta.len(),
            relative_path: None,
        });
    }
    Ok(Upload {
        files: checked,
        directory: None,
    })
}

/// The last part of `path`.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The files of directory `path`, recursively, sorted by their path in
/// it; symbolic links are not followed (nor uploaded).
fn directory(path: &str) -> Result<Upload, ErrorInfo> {
    let root = Path::new(path);
    let base = file_name(root);
    let failed = |at: &Path, e: &std::io::Error| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read {}: {e}", at.display()),
        )
    };
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| failed(&dir, &e))? {
            let entry = entry.map_err(|e| failed(&dir, &e))?;
            let at = entry.path();
            let meta = entry.metadata().map_err(|e| failed(&at, &e))?;
            if meta.is_dir() {
                dirs.push(at);
            } else if meta.is_file() {
                if files.len() == MAX_DIRECTORY_FILES {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("{path} has more than {MAX_DIRECTORY_FILES} files"),
                    )
                    .hint(Hint::text("upload a smaller directory"))
                    .details(json!({ "path": path, "limit": MAX_DIRECTORY_FILES })));
                }
                let inside = at.strip_prefix(root).unwrap_or(&at);
                let relative = Path::new(&base).join(inside);
                files.push(UploadedFile {
                    path: at.to_string_lossy().into_owned(),
                    name: file_name(&at),
                    size: meta.len(),
                    relative_path: Some(relative.to_string_lossy().into_owned()),
                });
            }
        }
    }
    if files.is_empty() {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{path} is a directory with no files"),
        )
        .details(json!({ "path": path })));
    }
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(Upload {
        files,
        directory: Some(path.to_owned()),
    })
}

impl Actor<'_> {
    /// `upload <loc> <file>...` and `upload <loc> --clear`.
    pub(super) async fn upload(&self) -> Result<Output, ErrorInfo> {
        let Action::Upload {
            locator,
            files,
            clear,
            force,
            drop,
        } = &self.req.action
        else {
            unreachable!("not upload")
        };
        let files = if *clear {
            Upload::default()
        } else {
            check_files(files)?
        };
        if *drop {
            return self.drop_files(locator, files, *force).await;
        }
        let parsed = self.parse(locator)?;
        // The last look's answer while the input is disabled: a look with
        // almost no time left can time out instead, which says less.
        let mut disabled: Option<ErrorInfo> = None;
        let late = |err: ErrorInfo, disabled: &mut Option<ErrorInfo>| match disabled.take() {
            Some(seen) if err.kind == ErrorKind::Timeout => seen,
            _ => err,
        };
        loop {
            let target = match self.locate_waiting(&parsed).await {
                Ok(target) => target,
                Err(err) => return Err(late(err, &mut disabled)),
            };
            let probe = self.call(&target, PROBE, Vec::new()).await;
            let probe: Probe = match probe {
                Ok(value) => serde_json::from_value(value).unwrap_or_default(),
                Err(err) => {
                    target.release();
                    return Err(late(err, &mut disabled));
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
                    return Err(late(err, &mut disabled));
                }
            };
            if probe.disabled && !*force {
                target.release();
                let err = ErrorInfo::new(
                    ErrorKind::NotEnabled,
                    format!("{} is disabled", describe(&element)),
                )
                .note("waited", protocol::format::duration(self.timeout))
                .note(
                    "why",
                    Cmd::new("why")
                        .arg(locator)
                        .flag("for", "upload")
                        .scoped(self.session())
                        .to_string(),
                )
                .hint(
                    Hint::run(Cmd::new("snapshot").scoped(self.session()))
                        .why("a control is often disabled until the form is complete"),
                )
                .details(json!({ "locator": locator, "check": "enabled" }));
                if Instant::now() + POLL < self.deadline {
                    disabled = Some(err);
                    tokio::time::sleep(POLL).await;
                    continue;
                }
                return Err(err);
            }
            if let Err(err) = self.takes(locator, &element, &probe, &files) {
                target.release();
                return Err(err);
            }
            let set = self.input_of(&target).await;
            let set = match set {
                Ok(input) => self.fill_input(&target.context, input, &files).await,
                Err(err) => Err(err),
            };
            let note = target.note.clone();
            target.release();
            let filled = set?;
            let mut output = self.uploaded(locator, element, files, Via::Input, &filled);
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
        files: &Upload,
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
        let result = if files.files.is_empty() {
            // `DOM.setFileInputFiles` ignores an empty list.
            let mut params = runtime::CallFunctionOnParams::new(CLEAR.to_owned());
            params.object_id = Some(input.clone());
            params.return_by_value = Some(true);
            session
                .call(params)
                .await
                .map(|r| r.result.value == Some(serde_json::Value::Bool(true)))
        } else {
            let mut params = SetFileInputFilesParams::new(files.paths());
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
        files: Upload,
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
                            .scoped(self.session()),
                    )
                    .why("the page's file input, if it has one"),
                ));
            };
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
            let mut params = runtime::CallFunctionOnParams::new(PROBE.to_owned());
            params.object_id = Some(input.clone());
            params.return_by_value = Some(true);
            let probe = session.call(params).await.map(|r| {
                r.result
                    .value
                    .and_then(|v| serde_json::from_value::<Probe>(v).ok())
                    .unwrap_or_default()
            });
            let checked = match probe {
                Ok(mut probe) => {
                    probe.multiple = chooser.mode != FileChooserOpenedMode::SelectSingle;
                    self.takes(locator, &element, &probe, &files)
                }
                Err(e) => Err(from_cdp(&e, self.session())),
            };
            if let Err(err) = checked {
                release(&context, input);
                return Err(err);
            }
            self.fill_input(&context, input, &files).await
        }
        .await;
        let _ = session
            .call(SetInterceptFileChooserDialogParams::new(false))
            .await;
        let filled = result?;
        let mut output = self.uploaded(locator, element, files, Via::Chooser, &filled);
        if let Some(note) = note {
            output = output.warning(note);
        }
        Ok(output)
    }

    /// Drops the files on the element `locator` names, with drag events.
    async fn drop_files(
        &self,
        locator: &str,
        files: Upload,
        force: bool,
    ) -> Result<Output, ErrorInfo> {
        if let Some(directory) = &files.directory {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{directory} is a directory; --drop drops files"),
            )
            .hint(Hint::text(
                "pass the files, or upload the directory into a webkitdirectory input",
            ))
            .details(json!({ "path": directory })));
        }
        let parsed = self.parse(locator)?;
        let checks = Checks {
            force,
            ..Checks::default()
        };
        let retry = |l: &str| self.retry(l);
        let ready = self.ready(&parsed, checks, &retry).await?;
        let (x, y) = (ready.ready.x, ready.ready.y);
        let result = async {
            self.call(&ready.ready.target, WATCH_DROP, Vec::new())
                .await?;
            // Drag events go to the tab's session at a point in its
            // viewport, as the mouse's do, whatever the element's frame.
            let session = self.page.session.with_timeout(self.timeout);
            let data = DragData {
                items: Vec::new(),
                files: Some(files.paths()),
                drag_operations_mask: DRAG_COPY,
            };
            let drag = |kind| session.call(DispatchDragEventParams::new(kind, x, y, data.clone()));
            for kind in [
                DispatchDragEventType::DragEnter,
                DispatchDragEventType::DragOver,
            ] {
                drag(kind).await.map_err(|e| from_cdp(&e, self.session()))?;
            }
            // A drop the page does not take is Chrome's to handle: it
            // opens the files in a tab, as for a user's drop. Cancel the
            // drag instead, so a refused drop changes nothing. With no
            // dragover in the element's window, drop anyway: `DROPPED`
            // tells whether something took it.
            let asked = Instant::now();
            let accepted = loop {
                let accepted = self.call(&ready.ready.target, ACCEPTED, Vec::new()).await?;
                if !accepted.is_null() || asked.elapsed() >= DRAGOVER_WAIT {
                    break accepted;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            let last = if accepted == serde_json::Value::Bool(false) {
                DispatchDragEventType::DragCancel
            } else {
                DispatchDragEventType::Drop
            };
            drag(last).await.map_err(|e| from_cdp(&e, self.session()))?;
            let dropped = self.call(&ready.ready.target, DROPPED, Vec::new()).await?;
            Ok::<_, ErrorInfo>(dropped == serde_json::Value::Bool(true))
        }
        .await;
        let element = ready.element.clone();
        let note = ready.ready.target.note.clone();
        ready.release();
        if !result? {
            return Err(self.no_drop(locator, &element));
        }
        let filled = Filled {
            changed: true,
            accept: String::new(),
        };
        let mut output = self.uploaded(locator, element, files, Via::Drop, &filled);
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
        upload: Upload,
        via: Via,
        filled: &Filled,
    ) -> Output {
        let Upload { files, directory } = upload;
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
            let (verb, how) = match via {
                Via::Input => ("uploaded", "to"),
                Via::Chooser => ("uploaded", "through the file chooser of"),
                Via::Drop => ("dropped", "on"),
            };
            if let Some(directory) = &directory {
                let plural = if files.len() == 1 { "" } else { "s" };
                format!(
                    "{verb} directory {} ({} file{plural}) {how} {described}: {listed}",
                    file_name(Path::new(directory)),
                    files.len()
                )
            } else if files.len() == 1 {
                format!("{verb} {listed} {how} {described}")
            } else {
                format!("{verb} {} files {how} {described}: {listed}", files.len())
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
            chooser: via == Via::Chooser,
            drop: via == Via::Drop,
            directory,
        };
        let output = Output::typed(&data, text).quiet(quiet);
        match warning {
            Some(warning) => output.warning(warning),
            None => output,
        }
    }

    /// Checks that the input `probe` describes takes what `files` is:
    /// a directory for a `webkitdirectory` input, files for another, one
    /// file for a single one.
    fn takes(
        &self,
        locator: &str,
        element: &ActedElement,
        probe: &Probe,
        files: &Upload,
    ) -> Result<(), ErrorInfo> {
        let described = describe(element);
        match (&files.directory, probe.directory) {
            (Some(directory), false) => Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{directory} is a directory, and {described} takes files"),
            )
            .hint(Hint::text("this input takes files; pass the files"))
            .details(json!({ "locator": locator, "path": directory }))),
            (None, true) if !files.files.is_empty() => Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{described} takes a directory, not files"),
            )
            .hint(
                Hint::run(
                    Cmd::new("upload")
                        .arg(locator)
                        .raw("<directory>")
                        .scoped(self.session()),
                )
                .why("this input takes a directory"),
            )
            .details(json!({ "locator": locator, "files": files.files.len() }))),
            (None, _) if !probe.multiple && files.files.len() > 1 => {
                Err(self.one_file(locator, element, files.files.len()))
            }
            _ => Ok(()),
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
                .scoped(self.session()),
        ))
        .details(json!({ "locator": locator, "files": count }))
    }

    /// A drop the page did not take.
    fn no_drop(&self, locator: &str, element: &ActedElement) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::NotEnabled,
            format!(
                "{} took no drop: the page did not accept files dropped there",
                describe(element)
            ),
        )
        .hint(
            Hint::run(
                Cmd::new("upload")
                    .arg("css=input[type=file]")
                    .raw("<file>")
                    .scoped(self.session()),
            )
            .why("the page's file input, even a hidden one"),
        )
        .details(json!({ "locator": locator, "check": "drop" }))
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
                    .scoped(self.session()),
            )
            .why("the page's file input, even a hidden one"),
        )
        .details(json!({ "locator": locator, "check": "chooser" }))
    }
}

/// How the files reached the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Straight into a file input.
    Input,
    /// Through the file chooser a click opened.
    Chooser,
    /// Dropped on the element (`--drop`).
    Drop,
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
        .map(|f| {
            let name = f.relative_path.as_deref().unwrap_or(&f.name);
            format!("{name} ({})", protocol::format::size(f.size))
        })
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
            relative_path: None,
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
    }

    #[test]
    fn a_directory_goes_alone_with_its_files_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("photos");
        std::fs::create_dir_all(root.join("b/c")).unwrap();
        std::fs::write(root.join("z.txt"), "z").unwrap();
        std::fs::write(root.join("b/c/y.txt"), "yy").unwrap();
        std::fs::write(root.join("b/x.txt"), "xxx").unwrap();
        std::os::unix::fs::symlink(root.join("z.txt"), root.join("link.txt")).unwrap();
        let path = root.to_string_lossy().into_owned();
        let upload = check_files(std::slice::from_ref(&path)).unwrap();
        assert_eq!(upload.directory.as_deref(), Some(path.as_str()));
        assert_eq!(upload.paths(), std::slice::from_ref(&path));
        let listed: Vec<(&str, &str, u64)> = upload
            .files
            .iter()
            .map(|f| (f.relative_path.as_deref().unwrap(), f.name.as_str(), f.size))
            .collect();
        assert_eq!(
            listed,
            [
                ("photos/b/c/y.txt", "y.txt", 2),
                ("photos/b/x.txt", "x.txt", 3),
                ("photos/z.txt", "z.txt", 1),
            ]
        );

        let file = root.join("z.txt").to_string_lossy().into_owned();
        let err = check_files(&[path.clone(), file]).unwrap_err();
        assert!(
            err.message
                .ends_with("is a directory; upload takes one directory alone, or files"),
            "{}",
            err.message
        );
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let empty = empty.to_string_lossy().into_owned();
        let err = check_files(std::slice::from_ref(&empty)).unwrap_err();
        assert_eq!(err.message, format!("{empty} is a directory with no files"));
    }
}
