//! The UI thread's clock for playback timing (`state`: the extrapolated position, seeks, the
//! start-up grace, debounces). It is `Instant::now()`, except in a test that froze it with
//! `freeze`: then it only moves by `advance`, so a test of the playback clock doesn't depend on
//! how long its sleeps really took on a busy machine. Per thread, like the tests that use it.

use std::time::{Duration, Instant};

#[cfg(test)]
thread_local! {
    static FROZEN: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

pub fn now() -> Instant {
    #[cfg(test)]
    if let Some(t) = FROZEN.get() {
        return t;
    }
    Instant::now()
}

/// Time since `t` on this clock (zero if `t` is ahead of it).
pub fn since(t: Instant) -> Duration {
    now().saturating_duration_since(t)
}

/// Tests: stops this thread's clock at the present moment.
#[cfg(test)]
pub fn freeze() {
    FROZEN.set(Some(Instant::now()));
}

/// Tests: moves this thread's frozen clock on by `d`.
#[cfg(test)]
pub fn advance(d: Duration) {
    let t = FROZEN.get().expect("clock::freeze first");
    FROZEN.set(Some(t + d));
}
