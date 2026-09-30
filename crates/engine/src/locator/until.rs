//! Waiting for a locator to match: the read commands (`text`, `html`,
//! `attr`, `value`, `box`, `markdown` with a locator), `eval --in` and
//! `locator` try again while their locator matches nothing (or names a
//! frame that is not there yet), within `--timeout`, as actions and
//! `screenshot <loc>` do. `count` does not wait: 0 is an answer.

use std::time::Duration;

use tokio::time::Instant;

use protocol::ErrorInfo;

use crate::input::waits_for;

/// How long to wait between tries.
const POLL: Duration = Duration::from_millis(50);

/// Runs `attempt` until it succeeds, fails for another reason than "not
/// there yet", or `deadline` has passed; the error then says it waited
/// the command's `timeout`. Each try bounds its own calls (by `timeout`,
/// not what is left: a last try with a few milliseconds would time out
/// instead of finding nothing).
pub(crate) async fn until_found<T, F, Fut>(
    deadline: Instant,
    timeout: Duration,
    mut attempt: F,
) -> Result<T, ErrorInfo>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ErrorInfo>>,
{
    loop {
        match attempt().await {
            Err(err) if waits_for(&err) => {
                // One last try at the deadline.
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(err.note("waited", protocol::format::duration(timeout)));
                }
                tokio::time::sleep(POLL.min(left)).await;
            }
            result => return result,
        }
    }
}
