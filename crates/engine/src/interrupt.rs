//! Commands stop when the page cannot go on (plan 03, milestone 3.5): a
//! JavaScript dialog that waits for an answer stalls every script of the
//! page, and a crashed page runs none. A command that waits on the page
//! when that happens returns at once with `DialogOpen` (exit 5) or
//! `PageCrashed` (exit 4), naming the command that unblocks it, instead of
//! running into its timeout.

use protocol::schema::DialogInfo;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::json;

use crate::tabs::dialogs::Blocker;
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// How much of a dialog's text an error or report line shows.
const SHOWN_MESSAGE: usize = 200;

impl Engine {
    /// Runs `work` on `page`, unless the tab gets a dialog that waits for
    /// an answer, or its page crashes, after event mark `since` first.
    pub(crate) async fn unless_blocked<T>(
        &self,
        running: &Running,
        page: &Page,
        since: u64,
        work: impl Future<Output = Result<T, ErrorInfo>>,
    ) -> Result<T, ErrorInfo> {
        tokio::select! {
            biased;
            result = work => result,
            blocker = running.tabs.blocked(page.id, since) => {
                Err(self.blocked_error(running, page.id, &blocker))
            }
        }
    }

    /// Fails at once if `page` has an open dialog or crashed.
    pub(crate) fn check_not_blocked(
        &self,
        running: &Running,
        page: &Page,
    ) -> Result<(), ErrorInfo> {
        match running.tabs.blocker(page.id) {
            Some(blocker) => Err(self.blocked_error(running, page.id, &blocker)),
            None => Ok(()),
        }
    }

    /// A warning for a raw call to a tab that cannot run page script now
    /// (the call is sent anyway: it may be what unblocks it).
    pub(crate) fn blocked_warning(&self, running: &Running, page: &Page) -> Option<String> {
        let blocker = running.tabs.blocker(page.id)?;
        let error = self.blocked_error(running, page.id, &blocker);
        Some(match error.hint {
            Some(hint) => format!("{}; {hint}", error.message),
            None => error.message,
        })
    }

    /// The error of a command that ran into `blocker` in tab `id`.
    pub(crate) fn blocked_error(&self, running: &Running, id: u32, blocker: &Blocker) -> ErrorInfo {
        // Name the tab in the hint unless it is the one commands act on.
        let tab = (running.tabs.active() != Some(id)).then(|| tab_name(id));
        match blocker {
            Blocker::Dialog(dialog) => dialog_open(&self.session, id, tab.as_deref(), dialog),
            Blocker::Crashed => {
                let url = running.tabs.info(id).map(|t| t.url).unwrap_or_default();
                let mut reload = Cmd::new("reload");
                if let Some(tab) = &tab {
                    reload = reload.flag("tab", tab);
                }
                ErrorInfo::new(
                    ErrorKind::PageCrashed,
                    format!("the page in tab {} crashed", tab_name(id)),
                )
                .note("page", url.clone())
                .hint(Hint::run(reload.session(&self.session)).why("loads it again"))
                .details(json!({ "tab": tab_name(id), "url": url }))
            }
        }
    }
}

/// `DialogOpen`: tab `id` shows `dialog`; `tab` is the `--tab` the hint
/// needs, if any.
pub(crate) fn dialog_open(
    session: &str,
    id: u32,
    tab: Option<&str>,
    dialog: &DialogInfo,
) -> ErrorInfo {
    let command = |verb: &str| {
        let mut cmd = Cmd::new("dialog").arg(verb);
        if verb == "accept" && dialog.kind == "prompt" {
            cmd = cmd.raw("'<text>'");
        }
        if let Some(tab) = tab {
            cmd = cmd.flag("tab", tab);
        }
        cmd.session(session).to_string()
    };
    let hint = if dialog.kind == "beforeunload" {
        Hint::text(format!(
            "{} (leaves the page), or {} (stays)",
            command("accept"),
            command("dismiss")
        ))
    } else {
        Hint::text(format!("{}, or {}", command("accept"), command("dismiss")))
    };
    ErrorInfo::new(
        ErrorKind::DialogOpen,
        format!("dialog open in tab {}: {}", tab_name(id), describe(dialog)),
    )
    .hint(hint)
    .details(json!({ "tab": tab_name(id), "dialog": dialog }))
}

/// `confirm "Delete item?"`, or `beforeunload (leave the page?)`.
pub(crate) fn describe(dialog: &DialogInfo) -> String {
    describe_kind(&dialog.kind, &dialog.message)
}

/// [`describe`] from the parts.
pub(crate) fn describe_kind(kind: &str, message: &str) -> String {
    if kind == "beforeunload" {
        return "beforeunload (leave the page?)".to_owned();
    }
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let shown = if flat.chars().count() > SHOWN_MESSAGE {
        let cut: String = flat.chars().take(SHOWN_MESSAGE).collect();
        format!("{cut}…")
    } else {
        flat
    };
    format!("{kind} {shown:?}")
}

#[cfg(test)]
mod tests {
    use protocol::schema::DialogInfo;

    use super::{describe, dialog_open};

    fn dialog(kind: &str, message: &str) -> DialogInfo {
        DialogInfo {
            kind: kind.into(),
            message: message.into(),
            default_prompt: None,
            url: "http://x/".into(),
        }
    }

    #[test]
    fn dialog_open_names_the_commands() {
        let info = dialog_open("default", 1, None, &dialog("confirm", "Delete item?"));
        assert_eq!(info.exit_code(), 5);
        assert_eq!(
            info.to_text(),
            "error: dialog open in tab t1: confirm \"Delete item?\"\n  \
             hint: chrome-cdp dialog accept, or chrome-cdp dialog dismiss\n"
        );
        let info = dialog_open("shop", 2, Some("t2"), &dialog("prompt", "Name?"));
        assert_eq!(
            info.hint.as_deref(),
            Some(
                "chrome-cdp dialog accept '<text>' --tab t2 --session shop, \
                 or chrome-cdp dialog dismiss --tab t2 --session shop"
            )
        );
    }

    #[test]
    fn long_messages_are_cut_to_one_line() {
        let long = format!("line one\nline two {}", "x".repeat(300));
        let shown = describe(&dialog("alert", &long));
        assert!(
            shown.starts_with("alert \"line one line two xxx"),
            "{shown}"
        );
        assert!(shown.ends_with("…\""), "{shown}");
        assert_eq!(
            describe(&dialog("beforeunload", "")),
            "beforeunload (leave the page?)"
        );
    }
}
