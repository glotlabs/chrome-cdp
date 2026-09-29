//! The rule every download of chrome-cdp follows (plan 10.7): HTTPS, or
//! plain HTTP only to this machine (the tests' servers, a local mirror).
//! It holds for the URLs a mirror variable names, the URLs an index
//! lists, and every redirect, so a download is never downgraded to HTTP
//! on the way.

use reqwest::Url;
use reqwest::redirect::Policy;

/// How many redirects a download may follow (reqwest's default).
const MAX_REDIRECTS: usize = 10;

/// Fails when `url` is not HTTPS, or HTTP to a loopback host.
pub fn check_url(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|e| format!("{url} is not a URL: {e}"))?;
    if is_secure(&parsed) {
        Ok(())
    } else {
        Err(format!(
            "{url} is not HTTPS: downloads use HTTPS, or HTTP only to this machine"
        ))
    }
}

/// The redirect policy of chrome-cdp's HTTP clients: at most
/// [`MAX_REDIRECTS`], each to a URL [`check_url`] accepts.
pub fn redirect_policy() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if is_secure(attempt.url()) {
            attempt.follow()
        } else {
            let to = attempt.url().to_string();
            attempt.error(format!("it redirects to {to}, which is not HTTPS"))
        }
    })
}

fn is_secure(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Domain(name)) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                name == "localhost" || name.ends_with(".localhost")
            }
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_url;

    #[test]
    fn https_or_http_to_this_machine() {
        for ok in [
            "https://storage.googleapis.com/chrome-for-testing-public/x.zip",
            "http://127.0.0.1:8080/cft",
            "http://localhost:9/x",
            "http://[::1]:9/x",
        ] {
            assert_eq!(check_url(ok), Ok(()), "{ok}");
        }
        for bad in [
            "http://mirror.example/cft",
            "http://127.0.0.1.evil.example/x",
            "ftp://x/y",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }
}
