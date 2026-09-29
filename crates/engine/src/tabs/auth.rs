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
        let credentials = shared
            .settings
            .http_credentials()
            .filter(|c| answers(c, &required.auth_challenge));
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

/// Whether `credentials` answer `challenge`. Credentials scoped to an
/// origin are not for another one's challenge (a third-party frame, a
/// redirect elsewhere); a proxy's challenge gets them only when it is the
/// origin named, since they are the site's.
fn answers(credentials: &HttpCredentials, challenge: &fetch::AuthChallenge) -> bool {
    let proxy = challenge.source == Some(fetch::AuthChallengeSource::Proxy);
    (!proxy || credentials.origin.is_some()) && credentials.applies_to(&challenge.origin)
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

    use cdp::domains::fetch::{AuthChallenge, AuthChallengeSource};

    use super::{answers, response};
    use crate::request::HttpCredentials;

    #[test]
    fn credentials_are_tried_once() {
        let credentials = HttpCredentials {
            username: "ada".into(),
            password: "s3cret".into(),
            origin: None,
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

    #[test]
    fn credentials_can_be_scoped_to_an_origin() {
        let mut credentials = HttpCredentials {
            username: "ada".into(),
            password: "s3cret".into(),
            origin: None,
        };
        assert!(credentials.applies_to("https://anyone.example"));
        credentials.origin = Some("https://App.test:443/".into());
        for yes in ["https://app.test", "https://app.test/admin?x=1"] {
            assert!(credentials.applies_to(yes), "{yes}");
        }
        for no in [
            "http://app.test",
            "https://app.test:8443",
            "https://evil.app.test",
            "https://app.test.evil.example",
            "",
            "null",
        ] {
            assert!(!credentials.applies_to(no), "{no}");
        }
        credentials.origin = Some("not an origin".into());
        assert!(!credentials.applies_to("https://app.test"));
    }

    #[test]
    fn a_proxy_gets_them_only_when_named() {
        let challenge = |source, origin: &str| AuthChallenge {
            source,
            origin: origin.into(),
            scheme: "basic".into(),
            realm: String::new(),
        };
        let mut credentials = HttpCredentials {
            username: "ada".into(),
            password: "s3cret".into(),
            origin: None,
        };
        let proxy = challenge(Some(AuthChallengeSource::Proxy), "http://proxy.test:3128");
        assert!(answers(&credentials, &challenge(None, "https://app.test")));
        assert!(answers(
            &credentials,
            &challenge(Some(AuthChallengeSource::Server), "https://app.test")
        ));
        assert!(!answers(&credentials, &proxy));
        credentials.origin = Some("http://proxy.test:3128".into());
        assert!(answers(&credentials, &proxy));
    }
}
