//! HTTP authentication (plan 03, milestone 3.5): `open --http-credentials`.
//!
//! Chrome only reports an authentication challenge (`Fetch.authRequired`)
//! for a request `Fetch` intercepts, so with credentials set every page and
//! iframe session intercepts all requests (`Fetch.enable` with the pattern
//! `*`) and lets each one go on at once (`Fetch.continueRequest`). A
//! challenge gets the credentials; a second challenge for the same request
//! means they were refused, and is cancelled so the page shows the
//! server's 401 instead of asking forever. Without credentials, `Fetch`
//! stays off and requests are not paused at all.
//!
//! Plan 07's routes intercept requests through the same `Fetch` domain;
//! [`super::fetch`] turns interception on for either and hands challenges
//! here.

use std::collections::HashSet;

use cdp::domains::fetch;
use cdp::{RawEvent, Session};

use super::{Shared, decode};
use crate::request::HttpCredentials;

/// A host session's authentication state: the requests already answered
/// with the credentials.
#[derive(Debug, Default)]
pub(super) struct Auth {
    answered: HashSet<String>,
}

impl Auth {
    /// `Fetch.authRequired`: answers with the session's credentials, once
    /// per request.
    pub(super) fn auth_required(&mut self, shared: &Shared, session: &Session, event: &RawEvent) {
        let required = match decode::<fetch::AuthRequired>(event) {
            Ok(required) => required,
            Err(e) => {
                tracing::warn!("cannot decode Fetch.authRequired: {e}");
                return;
            }
        };
        let credentials = shared.settings.http_credentials();
        let first = self.answered.insert(required.request_id.clone());
        if self.answered.len() > 1024 {
            self.answered.clear();
            self.answered.insert(required.request_id.clone());
        }
        let response = response(credentials.as_ref(), first);
        tracing::debug!(
            origin = %required.auth_challenge.origin,
            "answering an HTTP auth challenge with {:?}",
            response.response
        );
        let params = fetch::ContinueWithAuthParams::new(required.request_id, response);
        let call = session.call(params);
        shared.spawn(async move {
            if let Err(e) = call.await {
                tracing::debug!("cannot answer an auth challenge: {e}");
            }
        });
    }
}

/// The answer to a challenge: the credentials the first time, else give
/// up (they were refused).
fn response(credentials: Option<&HttpCredentials>, first: bool) -> fetch::AuthChallengeResponse {
    match credentials {
        Some(credentials) if first => fetch::AuthChallengeResponse {
            response: fetch::AuthChallengeResponseResponse::ProvideCredentials,
            username: Some(credentials.username.clone()),
            password: Some(credentials.password.clone()),
        },
        _ => fetch::AuthChallengeResponse {
            response: fetch::AuthChallengeResponseResponse::CancelAuth,
            username: None,
            password: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use cdp::domains::fetch::AuthChallengeResponseResponse as Answer;

    use super::response;
    use crate::request::HttpCredentials;

    #[test]
    fn credentials_are_tried_once() {
        let credentials = HttpCredentials {
            username: "ada".into(),
            password: "s3cret".into(),
        };
        let first = response(Some(&credentials), true);
        assert_eq!(first.response, Answer::ProvideCredentials);
        assert_eq!(first.password.as_deref(), Some("s3cret"));
        assert_eq!(
            response(Some(&credentials), false).response,
            Answer::CancelAuth
        );
        assert_eq!(response(None, true).response, Answer::CancelAuth);
    }
}
