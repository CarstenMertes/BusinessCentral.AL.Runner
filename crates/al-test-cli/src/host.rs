//! The host the command line gives a session (ADR-006): the real wall clock, and AL Runner's
//! identity. The core crates read both through `al-host`; only this crate touches the OS.

use al_host::{AlRunnerIdentity, Clock, Host};
use std::time::{SystemTime, UNIX_EPOCH};

/// The system clock, read on every call.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc_millis(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            // A clock set before 1970: the same distance on the other side.
            Err(e) => -(e.duration().as_millis() as i64),
        }
    }
}

/// The real clock and AL Runner's identity.
pub fn system_host() -> Host {
    Host::new(SystemClock, AlRunnerIdentity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_is_after_2024_and_does_not_run_backwards() {
        let c = SystemClock;
        let (a, b) = (c.now_utc_millis(), c.now_utc_millis());
        assert!(a > 1_704_067_200_000, "{a}");
        assert!(b >= a);
    }
}
