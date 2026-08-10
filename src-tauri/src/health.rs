//! Circuit breakers for the external endpoints Cora fans out against.
//!
//! Retrying a single call is the right answer to a blip. It is the wrong
//! answer to an outage: Cora's traffic is inherently parallel — the poller
//! pre-warms an analysis per PR whose head moved, three org loops each fire
//! dozens of chunked GraphQL queries per cycle — so one unreachable endpoint
//! turned into scores of doomed requests, each grinding through its own
//! backoff ladder, and each pre-warm burning a slot of the daily
//! auto-analyze budget for nothing.
//!
//! So the retry ladders handle "this call was unlucky", and a breaker per
//! endpoint handles "the endpoint is down": the first caller to exhaust its
//! retries opens the breaker, and every caller behind it fails immediately —
//! no network, no budget spent — until a cooldown elapses. Then exactly one
//! call is let through to test the water. It closes on the first success.
//!
//! One breaker per *endpoint*, not per client: [`BedrockHealth`] and
//! [`GitHubHealth`] wrap the same mechanism and live in Tauri state, so every
//! caller — poller loops, UI commands, analysis runs — shares one verdict.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cooldowns by consecutive outage, in seconds; the last repeats. Starts short
/// because most outages are brief and the cost of probing is one request.
const COOLDOWN_SECS: [u64; 4] = [30, 60, 120, 300];

#[derive(Default)]
struct State {
    /// Outages in a row, for picking the cooldown. Reset by any success.
    outages: u32,
    /// When the next probe may go out. `None` means the breaker is closed.
    retry_at: Option<Instant>,
}

/// One endpoint's shared health. `Arc` it into clients that outlive a borrow
/// of Tauri state; the newtypes below are what `lib.rs` registers.
#[derive(Default)]
pub struct EndpointHealth(Mutex<State>);

/// Bedrock's breaker, shared by the analysis engine and the assistant chat.
#[derive(Default)]
pub struct BedrockHealth(pub Arc<EndpointHealth>);

/// GitHub's breaker, shared by the poller loops and every UI command.
#[derive(Default)]
pub struct GitHubHealth(pub Arc<EndpointHealth>);

impl std::ops::Deref for BedrockHealth {
    type Target = EndpointHealth;
    fn deref(&self) -> &EndpointHealth {
        &self.0
    }
}

impl std::ops::Deref for GitHubHealth {
    type Target = EndpointHealth;
    fn deref(&self) -> &EndpointHealth {
        &self.0
    }
}

/// What a caller should do about the breaker right now.
pub enum Admission {
    /// Closed, or this caller is the probe — go.
    Proceed,
    /// Open. Carries the wait remaining, for the message shown to the user.
    Blocked(Duration),
}

impl EndpointHealth {
    /// Ask permission to call the endpoint, taking the probe slot when due.
    ///
    /// Claiming the slot pushes `retry_at` a full cooldown forward, which both
    /// holds everyone else back while the probe is in flight and — the reason
    /// it is done this way rather than with an "in flight" flag — means a probe
    /// that never reports back cannot wedge the breaker shut. A caller that
    /// dies, panics, or returns down some path that forgets to record simply
    /// loses its turn, and the next probe goes out one cooldown later.
    pub fn admit(&self) -> Admission {
        let mut st = self.0.lock().unwrap();
        let Some(retry_at) = st.retry_at else {
            return Admission::Proceed;
        };
        let now = Instant::now();
        if now < retry_at {
            return Admission::Blocked(retry_at - now);
        }
        let idx = (st.outages as usize).saturating_sub(1).min(COOLDOWN_SECS.len() - 1);
        st.retry_at = Some(now + Duration::from_secs(COOLDOWN_SECS[idx]));
        Admission::Proceed
    }

    /// A call got through. Any success clears an outage outright — a working
    /// endpoint should not stay half-shut because it failed earlier.
    pub fn record_success(&self) {
        let mut st = self.0.lock().unwrap();
        *st = State::default();
    }

    /// A call exhausted its retries against an unreachable endpoint. Returns
    /// the cooldown now in force, for logging.
    pub fn record_outage(&self) -> Duration {
        let mut st = self.0.lock().unwrap();
        let idx = (st.outages as usize).min(COOLDOWN_SECS.len() - 1);
        let cooldown = Duration::from_secs(COOLDOWN_SECS[idx]);
        st.outages = st.outages.saturating_add(1);
        st.retry_at = Some(Instant::now() + cooldown);
        cooldown
    }

    /// True when the breaker is holding calls back. Lets background work skip
    /// before it spends anything it cannot get back — a budget slot, a poll.
    pub fn is_open(&self) -> bool {
        matches!(self.admit_peek(), Admission::Blocked(_))
    }

    /// Read the breaker without claiming the probe slot.
    fn admit_peek(&self) -> Admission {
        let st = self.0.lock().unwrap();
        match st.retry_at {
            Some(retry_at) => {
                let now = Instant::now();
                if now < retry_at {
                    Admission::Blocked(retry_at - now)
                } else {
                    Admission::Proceed
                }
            }
            None => Admission::Proceed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_breaker_admits_everyone() {
        let h = EndpointHealth::default();
        assert!(matches!(h.admit(), Admission::Proceed));
        assert!(matches!(h.admit(), Admission::Proceed));
        assert!(!h.is_open());
    }

    #[test]
    fn an_outage_blocks_the_stampede_behind_it() {
        // The case that motivated this: one failure, then a queue of pre-warms
        // that must not each repeat the full retry ladder.
        let h = EndpointHealth::default();
        h.record_outage();
        assert!(h.is_open());
        for _ in 0..40 {
            assert!(matches!(h.admit(), Admission::Blocked(_)));
        }
    }

    #[test]
    fn cooldown_lengthens_per_outage_then_holds() {
        let h = EndpointHealth::default();
        assert_eq!(h.record_outage(), Duration::from_secs(30));
        assert_eq!(h.record_outage(), Duration::from_secs(60));
        assert_eq!(h.record_outage(), Duration::from_secs(120));
        assert_eq!(h.record_outage(), Duration::from_secs(300));
        assert_eq!(h.record_outage(), Duration::from_secs(300));
    }

    #[test]
    fn one_probe_goes_out_and_the_rest_wait() {
        let h = EndpointHealth::default();
        h.record_outage();
        // Force the cooldown to have elapsed.
        h.0.lock().unwrap().retry_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(matches!(h.admit(), Admission::Proceed), "first caller probes");
        assert!(matches!(h.admit(), Admission::Blocked(_)), "second waits");
    }

    #[test]
    fn success_reopens_the_gate_completely() {
        let h = EndpointHealth::default();
        h.record_outage();
        h.record_outage();
        h.record_success();
        assert!(!h.is_open());
        // And the next outage starts the ladder over, not where it left off.
        assert_eq!(h.record_outage(), Duration::from_secs(30));
    }

    #[test]
    fn a_probe_that_never_reports_back_cannot_wedge_it_shut() {
        // Terminal failures (expired SSO, a 401, a rejected mutation) return
        // down paths that record nothing. That must cost one turn, not the
        // breaker: the endpoint would stay shut for every caller forever.
        let h = EndpointHealth::default();
        h.record_outage();
        h.0.lock().unwrap().retry_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(matches!(h.admit(), Admission::Proceed), "probe goes out");
        // ...and vanishes without calling record_success or record_outage.
        assert!(h.is_open(), "others still held while it was in flight");
        h.0.lock().unwrap().retry_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(matches!(h.admit(), Admission::Proceed), "next probe still goes out");
    }

    #[test]
    fn a_failed_probe_reopens_with_a_longer_cooldown() {
        let h = EndpointHealth::default();
        h.record_outage();
        h.0.lock().unwrap().retry_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(matches!(h.admit(), Admission::Proceed));
        assert_eq!(h.record_outage(), Duration::from_secs(60), "probe failed → longer wait");
        assert!(matches!(h.admit(), Admission::Blocked(_)), "and closed again");
    }
}
