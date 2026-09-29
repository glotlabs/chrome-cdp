//! JavaScript dialogs (plan 03, milestone 3.5).
//!
//! Chrome reports a dialog (`alert`, `confirm`, `prompt`, `beforeunload`)
//! on the session of the frame that opened it and stalls the page's
//! scripts until someone answers it. The session's [`DialogPolicy`] answers
//! it at once, or leaves it open on its tab until `chrome-cdp dialog`
//! does. A command that is waiting on the page when a dialog opens is
//! interrupted (see [`Tabs::blocked`]) rather than left to time out.
//! Dialogs answered without a command are logged, so the command that ran
//! meanwhile reports them.

use cdp::domains::page;
use cdp::{RawEvent, Session};
use protocol::schema::{DialogInfo, Event};

use super::{Check, Shared, Tabs, decode, tab_name};
use crate::request::DialogPolicy;

/// A dialog that waits for `chrome-cdp dialog`.
#[derive(Debug, Clone)]
pub(crate) struct OpenDialog {
    /// The session that reported it: the answer goes there.
    pub(crate) session: Session,
    pub(crate) info: DialogInfo,
    /// When it opened, in the tabs' event sequence.
    pub(super) seq: u64,
}

/// Why a tab cannot run a command's page script now.
#[derive(Debug, Clone)]
pub(crate) enum Blocker {
    /// A dialog waits for an answer.
    Dialog(DialogInfo),
    /// The page crashed.
    Crashed,
}

/// What `policy` does with a dialog of `kind`: accept (`Some(true)`),
/// dismiss (`Some(false)`), or leave it open for `chrome-cdp dialog`.
pub(crate) fn answer(policy: DialogPolicy, kind: page::DialogType) -> Option<bool> {
    match policy {
        DialogPolicy::Accept => Some(true),
        DialogPolicy::Dismiss => Some(false),
        DialogPolicy::Manual => None,
        DialogPolicy::Auto => match kind {
            page::DialogType::Alert | page::DialogType::Beforeunload => Some(true),
            _ => None,
        },
    }
}

/// `alert`, `confirm`, `prompt` or `beforeunload`.
fn kind_name(kind: page::DialogType) -> &'static str {
    match kind {
        page::DialogType::Alert => "alert",
        page::DialogType::Confirm => "confirm",
        page::DialogType::Prompt => "prompt",
        page::DialogType::Beforeunload => "beforeunload",
        page::DialogType::Unknown => "dialog",
    }
}

/// `Page.javascriptDialogOpening` on `session`, a page or iframe session
/// of tab `tab`.
pub(super) fn opening(shared: &Shared, session: &Session, tab: u32, event: &RawEvent) {
    let opening = match decode::<page::JavascriptDialogOpening>(event) {
        Ok(opening) => opening,
        Err(e) => {
            tracing::warn!("cannot decode Page.javascriptDialogOpening: {e}");
            return;
        }
    };
    let info = DialogInfo {
        kind: kind_name(opening.r#type).to_owned(),
        message: opening.message,
        default_prompt: opening
            .default_prompt
            .filter(|_| opening.r#type == page::DialogType::Prompt),
        url: opening.url,
    };
    let policy = shared.settings.dialogs();
    let mut state = shared.lock();
    if let Some(accept) = answer(policy, opening.r#type) {
        state.log(Event::Dialog {
            tab: tab_name(tab),
            kind: info.kind.clone(),
            message: info.message.clone(),
            accepted: accept,
            by: "policy".into(),
        });
        drop(state);
        let mut params = page::HandleJavaScriptDialogParams::new(accept);
        if accept {
            params.prompt_text.clone_from(&info.default_prompt);
        }
        let call = session.call(params);
        shared.spawn(async move {
            if let Err(e) = call.await {
                tracing::debug!("cannot answer a dialog: {e}");
            }
        });
    } else {
        state.seq += 1;
        let seq = state.seq;
        if let Some(tab) = state.tabs.get_mut(&tab) {
            tab.dialog = Some(OpenDialog {
                session: session.clone(),
                info,
                seq,
            });
        }
        drop(state);
    }
    shared.bump();
}

/// `Page.javascriptDialogClosed` in tab `tab`: an open dialog closed
/// without `chrome-cdp dialog` (a navigation dismissed it, or someone
/// answered it over raw CDP).
pub(super) fn closed(shared: &Shared, tab: u32, event: &RawEvent) {
    let accepted = event.params["result"].as_bool().unwrap_or(false);
    let mut state = shared.lock();
    let Some(dialog) = state.tabs.get_mut(&tab).and_then(|t| t.dialog.take()) else {
        return;
    };
    state.log(Event::Dialog {
        tab: tab_name(tab),
        kind: dialog.info.kind,
        message: dialog.info.message,
        accepted,
        by: "other".into(),
    });
    drop(state);
    shared.bump();
}

impl Tabs {
    /// Tab `id`'s open dialog, if any.
    pub(crate) fn dialog(&self, id: u32) -> Option<OpenDialog> {
        self.shared
            .lock()
            .tabs
            .get(&id)
            .and_then(|t| t.dialog.clone())
    }

    /// Waits until tab `id` has an open dialog, or `deadline` passes (or
    /// the tab closes): `None` then. A dialog already open counts.
    pub(crate) async fn next_dialog(
        &self,
        id: u32,
        deadline: tokio::time::Instant,
    ) -> Option<OpenDialog> {
        self.watch(deadline, |state, _| match state.tabs.get(&id) {
            Some(tab) => match &tab.dialog {
                Some(dialog) => Check::Done(Some(dialog.clone())),
                None => Check::Later(None),
            },
            None => Check::Done(None),
        })
        .await
        .flatten()
    }

    /// Takes tab `id`'s open dialog to answer it: it is no longer open for
    /// commands, and its closing is not logged.
    pub(crate) fn take_dialog(&self, id: u32) -> Option<OpenDialog> {
        let dialog = self
            .shared
            .lock()
            .tabs
            .get_mut(&id)
            .and_then(|t| t.dialog.take());
        self.shared.bump();
        dialog
    }

    /// Every tab with an open dialog, by id.
    pub(crate) fn dialogs(&self) -> Vec<(u32, DialogInfo)> {
        self.shared
            .lock()
            .tabs
            .iter()
            .filter_map(|(&id, t)| Some((id, t.dialog.as_ref()?.info.clone())))
            .collect()
    }

    /// What keeps tab `id` from running page script now, if anything.
    pub(crate) fn blocker(&self, id: u32) -> Option<Blocker> {
        let state = self.shared.lock();
        let tab = state.tabs.get(&id)?;
        if tab.crashed.is_some() {
            return Some(Blocker::Crashed);
        }
        tab.dialog.as_ref().map(|d| Blocker::Dialog(d.info.clone()))
    }

    /// Resolves once the session's policy dismissed a `beforeunload` of
    /// tab `id` after event mark `since`: the page stays, so the
    /// navigation that asked to leave it is cancelled.
    pub(crate) async fn stayed(&self, id: u32, since: u64) {
        let name = tab_name(id);
        let far = tokio::time::Instant::now() + std::time::Duration::from_hours(24 * 365);
        let stayed = self
            .watch(far, |state, _| {
                let dismissed = state.logged.iter().any(|(seq, event)| {
                    *seq > since
                        && matches!(
                            event,
                            Event::Dialog { tab, kind, accepted: false, by, .. }
                                if *tab == name && kind == "beforeunload" && by == "policy"
                        )
                });
                if dismissed {
                    Check::Done(())
                } else {
                    Check::Later(None)
                }
            })
            .await;
        if stayed.is_none() {
            std::future::pending::<()>().await;
        }
    }

    /// Resolves once tab `id` gets a dialog that waits for an answer, or
    /// its page crashes, after event mark `since` (see [`Tabs::mark`]);
    /// never otherwise. Commands race it against their work.
    pub(crate) async fn blocked(&self, id: u32, since: u64) -> Blocker {
        let far = tokio::time::Instant::now() + std::time::Duration::from_hours(24 * 365);
        let found = self
            .watch(far, |state, _| {
                let Some(tab) = state.tabs.get(&id) else {
                    return Check::Later(None);
                };
                if tab.crashed.is_some_and(|seq| seq > since) {
                    return Check::Done(Blocker::Crashed);
                }
                match &tab.dialog {
                    Some(dialog) if dialog.seq > since => {
                        Check::Done(Blocker::Dialog(dialog.info.clone()))
                    }
                    _ => Check::Later(None),
                }
            })
            .await;
        match found {
            Some(blocker) => blocker,
            None => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use cdp::domains::page::DialogType;

    use super::answer;
    use crate::request::DialogPolicy;

    #[test]
    fn policies() {
        let kinds = [
            DialogType::Alert,
            DialogType::Confirm,
            DialogType::Prompt,
            DialogType::Beforeunload,
        ];
        let answers = |policy| kinds.map(|kind| answer(policy, kind));
        assert_eq!(
            answers(DialogPolicy::Auto),
            [Some(true), None, None, Some(true)]
        );
        assert_eq!(answers(DialogPolicy::Accept), [Some(true); 4]);
        assert_eq!(answers(DialogPolicy::Dismiss), [Some(false); 4]);
        assert_eq!(answers(DialogPolicy::Manual), [None; 4]);
    }
}
