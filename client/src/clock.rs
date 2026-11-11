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

/// Collects ping samples and keeps the current offset estimate.
#[derive(Debug, Default)]
pub struct ClockSync {
    samples: Vec<ClockSample>,
    estimate: Option<ClockEstimate>,
    rough_offset: Option<i64>,
}

impl ClockSync {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Rough offset from the `Welcome` message (used until pongs arrive).
    pub fn set_rough(&mut self, server_ms: u64) {
        self.rough_offset = Some(server_ms as i64 - local_ms() as i64);
    }

    pub fn add_sample(&mut self, client_send_ms: u64, server_ms: u64) {
        let s = ClockSample {
            client_send_ms,
            server_ms,
            client_recv_ms: local_ms(),
        };
        self.samples.push(s);
        if self.samples.len() > WINDOW {
            let drop = self.samples.len() - WINDOW;
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
