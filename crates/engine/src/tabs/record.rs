//! What the tab manager adds to a recording (plan 18, milestone 18.6; see
//! [`crate::record`]): the page recorder's reports with the frame they
//! came from, main-frame navigations the browser started (the page's own
//! are left out: a replay repeats them), and dialogs someone answered
//! (not those the session's policy did: the replay's policy answers them
//! too).

use cdp::domains::page;
use cdp::domains::page::FrameStartedNavigatingNavigationType as Type;
use cdp::{Event as _, RawEvent};

use super::{Shared, State, decode};
use crate::locator::frame_step;
use crate::record::{Frame, Nav, PageEvent, What};

/// The scope of tab `tab` and whether a page opened it, if the tab is
/// there.
fn tab_facts(state: &State, tab: u32) -> Option<(Option<String>, bool)> {
    let t = state.tabs.get(&tab)?;
    let scope = t.context.as_ref().map(|(browser_id, _)| browser_id.clone());
    Some((scope, t.opener_target.is_some()))
}

/// A report of the page's recorder (`Runtime.bindingCalled` on
/// [`crate::record::RECORD_BINDING`]) from session `session_id` of tab
/// `tab`.
pub(super) fn reported(shared: &Shared, session_id: &str, tab: u32, event: &RawEvent) {
    let Some(report) = event.params["payload"]
        .as_str()
        .and_then(|p| serde_json::from_str::<PageEvent>(p).ok())
    else {
        return;
    };
    let context = event.params["executionContextId"].as_i64();
    let (scope, opener, frame) = {
        let state = shared.lock();
        let Some((scope, opener)) = tab_facts(&state, tab) else {
            return;
        };
        let Some(t) = state.tabs.get(&tab) else {
            return;
        };
        let frame = match context.and_then(|c| t.frames.frame_of(session_id, c)) {
            Some(id) if id != t.frames.root() => match frame_step(&t.frames.list(), &id) {
                Some(step) => Frame::Step(step),
                None => Frame::Unnamed,
            },
            _ => Frame::Main,
        };
        (scope, opener, frame)
    };
    // A report of another frame's history is of no use.
    if report.k == "page" && frame != Frame::Main {
        return;
    }
    let what = What::Page {
        frame,
        event: Box::new(report),
    };
    shared
        .settings
        .recorder
        .with(scope.as_deref(), |r| r.push(tab, opener, what));
}

/// `Page.frameRequestedNavigation` or `Page.frameStartedNavigating` of
/// tab `tab`: a navigation of its main frame the page did not ask for is
/// a step.
pub(super) fn navigation(shared: &Shared, tab: u32, method: &str, event: &RawEvent) {
    let (scope, opener, main) = {
        let state = shared.lock();
        let Some((scope, opener)) = tab_facts(&state, tab) else {
            return;
        };
        let main = state.tabs.get(&tab).map(|t| t.frames.root().to_owned());
        (scope, opener, main)
    };
    let recorder = &shared.settings.recorder;
    if !recorder.records(scope.as_deref()) {
        return;
    }
    let main = main.unwrap_or_default();
    match method {
        page::FrameRequestedNavigation::NAME => {
            let Ok(asked) = decode::<page::FrameRequestedNavigation>(event) else {
                return;
            };
            if asked.frame_id == main
                && asked.disposition == page::ClientNavigationDisposition::CurrentTab
            {
                recorder.with(scope.as_deref(), |r| {
                    r.asked.insert(tab);
                });
            }
        }
        page::FrameStartedNavigating::NAME => {
            let Ok(started) = decode::<page::FrameStartedNavigating>(event) else {
                return;
            };
            if started.frame_id != main {
                return;
            }
            let how = match started.navigation_type {
                Type::Reload | Type::ReloadBypassingCache => Nav::Reload,
                Type::HistorySameDocument | Type::HistoryDifferentDocument => Nav::History,
                Type::DifferentDocument | Type::SameDocument => Nav::Goto,
                _ => return,
            };
            recorder.with(scope.as_deref(), |r| {
                if !r.asked.remove(&tab) {
                    r.push(
                        tab,
                        opener,
                        What::Navigate {
                            how,
                            url: started.url,
                        },
                    );
                }
            });
        }
        _ => {}
    }
}

/// The session's policy answered a dialog of tab `tab`: its closing is no
/// step.
pub(super) fn policy_answered(shared: &Shared, state: &State, tab: u32) {
    let Some((scope, _)) = tab_facts(state, tab) else {
        return;
    };
    shared.settings.recorder.with(scope.as_deref(), |r| {
        *r.answered.entry(tab).or_default() += 1;
    });
}

/// `Page.javascriptDialogClosed` in tab `tab`.
pub(super) fn dialog_closed(shared: &Shared, tab: u32, event: &RawEvent) {
    let Some((scope, opener)) = tab_facts(&shared.lock(), tab) else {
        return;
    };
    let accept = event.params["result"].as_bool().unwrap_or(false);
    let text = event.params["userInput"]
        .as_str()
        .map(str::to_owned)
        .filter(|t| !t.is_empty());
    shared
        .settings
        .recorder
        .with(scope.as_deref(), |r| match r.answered.get_mut(&tab) {
            Some(n) if *n > 0 => *n -= 1,
            _ => r.push(tab, opener, What::Dialog { accept, text }),
        });
}
