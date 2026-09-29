//! `trace start` and `trace stop` (plan 06, milestone 6.5): a Chrome
//! performance trace, as DevTools' Performance panel records one.
//!
//! `trace start` sends `Tracing.start` on the tab's session (as DevTools
//! and Puppeteer do: the trace then opens on that tab's page) with
//! `transferMode: ReturnAsStream`. `trace stop` ends it, waits for
//! `Tracing.tracingComplete` and its stream handle, and copies the stream
//! to the file chunk by chunk with `IO.read`, so a large trace is never
//! held whole in memory. The file is Chrome's JSON trace format
//! (gzipped when `-o` ends in `.gz`), which the Performance panel and
//! Perfetto open.

use std::io::Write as _;
use std::time::Instant;

use base64::Engine as _;
use cdp::Session;
use cdp::domains::{io as cdp_io, tracing as cdp_tracing};
use protocol::schema::{TraceStartData, TraceStopData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use super::save::{create_private, failed};
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::request::{TraceStartRequest, TraceStopRequest, millis};
use crate::tabs::tab_name;
use crate::{Engine, Running};

/// The Performance panel's categories (as Puppeteer's `tracing.start`
/// has them), plus `loading` for LCP and layout shifts.
const DEFAULT_CATEGORIES: &[&str] = &[
    "-*",
    "devtools.timeline",
    "v8.execute",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
    "toplevel",
    "blink.console",
    "blink.user_timing",
    "latencyInfo",
    "loading",
    "disabled-by-default-devtools.timeline.stack",
    "disabled-by-default-v8.cpu_profiler",
];

/// The film strip's category.
const SCREENSHOTS: &str = "disabled-by-default-devtools.screenshot";

/// How much one `IO.read` asks for.
const CHUNK: i64 = 1 << 20;

/// A running `trace start`.
#[derive(Debug, Clone)]
pub(crate) struct TraceRecording {
    session: Session,
    tab: u32,
    started: Instant,
    categories: Vec<String>,
    screenshots: bool,
}

impl Engine {
    /// `trace start`: starts a trace on the tab's session.
    pub async fn trace_start(&self, req: TraceStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let current = lock(&running.trace).clone();
        if let Some(current) = current {
            let data = TraceStartData {
                tab: tab_name(current.tab),
                categories: current.categories.clone(),
                screenshots: current.screenshots,
                already: true,
            };
            let text = format!(
                "still recording a trace of {} since {} ago; {} saves it",
                tab_name(current.tab),
                format::duration(current.started.elapsed()),
                stop_cmd(&self.session),
            );
            return Ok(Output::typed(&data, text).quiet(tab_name(current.tab)));
        }
        let mut categories: Vec<String> = if req.categories.is_empty() {
            DEFAULT_CATEGORIES.iter().map(|&c| c.to_owned()).collect()
        } else {
            req.categories.clone()
        };
        if req.screenshots && !categories.iter().any(|c| c == SCREENSHOTS) {
            categories.push(SCREENSHOTS.to_owned());
        }
        let (excluded, included): (Vec<String>, Vec<String>) =
            categories.iter().cloned().partition(|c| c.starts_with('-'));
        let config = cdp_tracing::TraceConfig {
            record_mode: None,
            trace_buffer_size_in_kb: None,
            enable_sampling: None,
            enable_systrace: None,
            enable_argument_filter: None,
            included_categories: Some(included),
            excluded_categories: Some(
                excluded
                    .into_iter()
                    .map(|c| c.trim_start_matches('-').to_owned())
                    .collect(),
            ),
            synthetic_delays: None,
            memory_dump_config: None,
        };
        let params = cdp_tracing::StartParams {
            transfer_mode: Some(cdp_tracing::StartTransferMode::ReturnAsStream),
            stream_format: Some(cdp_tracing::StreamFormat::Json),
            trace_config: Some(config),
            ..cdp_tracing::StartParams::default()
        };
        page.session
            .with_timeout(timeout)
            .call(params)
            .await
            .map_err(|e| {
                let info = from_cdp(&e, &self.session);
                if e.to_string().contains("already") {
                    info.hint(
                        Hint::run(stop_cmd(&self.session))
                            .why("another trace runs in this browser"),
                    )
                } else {
                    info
                }
            })?;
        *lock(&running.trace) = Some(TraceRecording {
            session: page.session.clone(),
            tab: page.id,
            started: Instant::now(),
            categories: categories.clone(),
            screenshots: req.screenshots,
        });
        let tab = tab_name(page.id);
        let text = format!(
            "recording a trace of {tab}{}; {} saves it",
            if req.screenshots {
                " with screenshots"
            } else {
                ""
            },
            stop_cmd(&self.session),
        );
        let data = TraceStartData {
            tab: tab.clone(),
            categories,
            screenshots: req.screenshots,
            already: false,
        };
        Ok(Output::typed(&data, text).quiet(tab))
    }

    /// `trace stop`: ends the trace and saves it.
    pub async fn trace_stop(&self, req: TraceStopRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let Some(recording) = lock(&running.trace).clone() else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("session {} is not recording a trace", self.session),
            )
            .hint(
                Hint::run(Cmd::new("trace start").scoped(&self.session))
                    .why("then act, then trace stop"),
            ));
        };
        let result = Box::pin(self.trace_save(&running, &recording, &req)).await;
        // Whatever happened, Chrome's trace is over (or its tab is gone).
        *lock(&running.trace) = None;
        result
    }

    async fn trace_save(
        &self,
        running: &Running,
        recording: &TraceRecording,
        req: &TraceStopRequest,
    ) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let session = recording.session.with_timeout(timeout);
        let gzip = req
            .path
            .as_deref()
            .and_then(|p| p.extension())
            .is_some_and(|e| e.eq_ignore_ascii_case("gz"));
        let duration = recording.started.elapsed();
        let tab = tab_name(recording.tab);
        // Chrome answers `Tracing.end` before it sends the stream.
        let mut complete = session.listen::<cdp_tracing::TracingComplete>();
        session
            .call(cdp_tracing::EndParams::default())
            .await
            .map_err(|e| self.trace_gone(running, recording, &e))?;
        let complete = tokio::time::timeout(timeout, complete.next())
            .await
            .map_err(|_| {
                ErrorInfo::new(
                    ErrorKind::Timeout,
                    format!(
                        "Chrome did not hand over the trace within {}",
                        format::duration(timeout)
                    ),
                )
                .hint(
                    Hint::run(
                        Cmd::new("trace stop")
                            .flag("timeout", "2m")
                            .scoped(&self.session),
                    )
                    .why("a long trace takes a while to collect"),
                )
            })?
            .ok_or_else(|| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("{tab} went away before Chrome handed over the trace"),
                )
            })?;
        let Some(stream) = complete.stream else {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                "Chrome ended the trace without a stream to read it from",
            ));
        };
        // Chrome gzips only when asked at the start, so compress here.
        let (file, path) = create_private(
            req.path.as_deref(),
            &self.output_dir().join("traces"),
            "json",
        )?;
        let mut sink: Box<dyn std::io::Write + Send> = if gzip {
            Box::new(flate2::write::GzEncoder::new(
                file,
                flate2::Compression::default(),
            ))
        } else {
            Box::new(std::io::BufWriter::new(file))
        };
        let copied = copy(&session, &stream, sink.as_mut(), &path)
            .await
            .map_err(|e| e.unwrap_or_else(|e| from_cdp(&e, &self.session)));
        let _ = session.call(cdp_io::CloseParams::new(stream.clone())).await;
        copied?;
        sink.flush().map_err(|e| failed(&path, &e))?;
        drop(sink);
        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
        let path_text = path.display().to_string();
        let text = format!(
            "{path_text}: trace of {tab}, {}, {}; open it in DevTools' Performance panel or \
             https://ui.perfetto.dev",
            format::duration(duration),
            format::size(bytes),
        );
        let data = TraceStopData {
            path: path_text.clone(),
            bytes,
            tab,
            duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            data_loss: complete.data_loss_occurred,
        };
        let mut output = Output::typed(&data, text).quiet(path_text);
        if complete.data_loss_occurred {
            output = output.warning(
                "Chrome's trace buffer filled up and events were lost; trace a shorter span, or \
                 fewer --categories",
            );
        }
        Ok(output)
    }

    fn trace_gone(
        &self,
        running: &Running,
        recording: &TraceRecording,
        err: &cdp::CdpError,
    ) -> ErrorInfo {
        let tab = tab_name(recording.tab);
        if running.tabs.info(recording.tab).is_none() {
            return ErrorInfo::new(
                ErrorKind::NotFound,
                format!("{tab} closed while it recorded a trace; the trace is gone"),
            )
            .hint(Hint::run(Cmd::new("trace start").scoped(&self.session)));
        }
        from_cdp(err, &self.session)
    }
}

/// Copies stream `stream` to `sink` (the file at `path`); a failed read
/// is `Err(Err(_))`.
async fn copy(
    session: &Session,
    stream: &str,
    sink: &mut (dyn std::io::Write + Send),
    path: &std::path::Path,
) -> Result<(), Result<ErrorInfo, cdp::CdpError>> {
    loop {
        let mut read = cdp_io::ReadParams::new(stream.to_owned());
        read.size = Some(CHUNK);
        let chunk = session.call(read).await.map_err(Err)?;
        if chunk.base64_encoded == Some(true) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data)
                .map_err(|e| {
                    Ok(ErrorInfo::new(
                        ErrorKind::ProtocolError,
                        format!("Chrome sent trace data that is not base64: {e}"),
                    ))
                })?;
            sink.write_all(&bytes)
        } else {
            sink.write_all(chunk.data.as_bytes())
        }
        .map_err(|e| Ok(failed(path, &e)))?;
        if chunk.eof {
            return Ok(());
        }
    }
}

fn stop_cmd(session: &str) -> Cmd {
    Cmd::new("trace stop")
        .raw("-o")
        .arg("trace.json")
        .scoped(session)
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
