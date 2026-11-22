//! Local monotonic clock and server clock offset tracking.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use shared::sync::{estimate_clock_offset, ClockEstimate, ClockSample};

fn base() -> &'static (Instant, u64) {
    static BASE: OnceLock<(Instant, u64)> = OnceLock::new();
    BASE.get_or_init(|| {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        (Instant::now(), epoch)
    })
}

/// Monotonic local time in ms (epoch-based so it is in the same range as server time).
pub fn local_ms() -> u64 {
    let (i, e) = base();
    e + i.elapsed().as_millis() as u64
}

/// Converts a local ms timestamp to an `Instant`.
pub fn instant_at_local_ms(ms: u64) -> Instant {
    let (i, e) = base();
    *i + std::time::Duration::from_millis(ms.saturating_sub(*e))
}

/// Number of samples kept for the estimate.
const WINDOW: usize = 16;
/// In the low-latency mode pings are constant, so more fit in the window
/// (about 10 s of them); the best few of many are steadier.
const WINDOW_LOW: usize = 40;

/// Collects ping samples and keeps the current offset estimate.
#[derive(Debug, Default)]
pub struct ClockSync {
    samples: Vec<ClockSample>,
    estimate: Option<ClockEstimate>,
    rough_offset: Option<i64>,
    low_latency: bool,
}

impl ClockSync {
    pub fn reset(&mut self) {
        *self = Self {
            low_latency: self.low_latency,
            ..Self::default()
        };
    }

    pub fn set_low_latency(&mut self, on: bool) {
        self.low_latency = on;
    }

    /// Rough offset from the `Welcome` message (used until pongs arrive).
    pub fn set_rough(&mut self, server_ms: u64) {
        self.rough_offset = Some(server_ms as i64 - local_ms() as i64);
    }

    /// `client_recv_ms` is when the pong arrived (stamped by the network
    /// task, so a busy engine doesn't make the round trip look longer).
    pub fn add_sample(&mut self, client_send_ms: u64, server_ms: u64, client_recv_ms: u64) {
        let s = ClockSample {
            client_send_ms,
            server_ms,
            client_recv_ms,
        };
        self.samples.push(s);
        let window = if self.low_latency { WINDOW_LOW } else { WINDOW };
        if self.samples.len() > window {
            let drop = self.samples.len() - window;
            self.samples.drain(..drop);
        }
        self.estimate = estimate_clock_offset(&self.samples);
    }

    /// `server - local` in ms.
    pub fn offset_ms(&self) -> i64 {
        self.estimate
            .map(|e| e.offset_ms)
            .or(self.rough_offset)
            .unwrap_or(0)
    }

    pub fn rtt_ms(&self) -> Option<u64> {
        self.estimate.map(|e| e.rtt_ms)
    }

    pub fn is_synced(&self) -> bool {
        self.estimate.is_some()
    }

    pub fn server_now_ms(&self) -> u64 {
        shared::sync::to_server_ms(local_ms(), self.offset_ms())
    }
}
