//! Pure synchronization math: expected position, clock offset estimation
//! and drift correction decisions.

use crate::protocol::{PlayStatus, PlaybackState};

/// Below this absolute drift nothing is done.
pub const DRIFT_IGNORE_MS: i64 = 50;
/// While nudging, return to normal speed once within this drift.
pub const DRIFT_SETTLED_MS: i64 = 20;
/// Above this absolute drift a hard seek is performed.
pub const DRIFT_SEEK_MS: i64 = 1000;
/// Playback speed used to catch up when behind.
pub const SPEED_FAST: f64 = 1.03;
/// Playback speed used to fall back when ahead.
pub const SPEED_SLOW: f64 = 0.97;

/// Expected position of the track at `server_now_ms`.
///
/// While playing the position advances from the anchor. If the anchor lies in
/// the future (a scheduled start) the position stays at the anchor position
/// until that time. Paused/stopped states return the anchor position.
pub fn expected_position_ms(state: &PlaybackState, server_now_ms: u64) -> u64 {
    match state.status {
        PlayStatus::Playing => {
            state.anchor_position_ms + server_now_ms.saturating_sub(state.anchor_server_ms)
        }
        PlayStatus::Paused | PlayStatus::Stopped => state.anchor_position_ms,
    }
}

/// Same as [`expected_position_ms`] but clamped to the track duration if known.
pub fn expected_position_clamped(
    state: &PlaybackState,
    server_now_ms: u64,
    duration_ms: Option<u64>,
) -> u64 {
    let p = expected_position_ms(state, server_now_ms);
    match duration_ms {
        Some(d) => p.min(d),
        None => p,
    }
}

/// One NTP-style ping/pong sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    /// Local time when the ping was sent.
    pub client_send_ms: u64,
    /// Server time stamped in the pong.
    pub server_ms: u64,
    /// Local time when the pong was received.
    pub client_recv_ms: u64,
}

impl ClockSample {
    pub fn rtt_ms(&self) -> u64 {
        self.client_recv_ms.saturating_sub(self.client_send_ms)
    }

    /// `server - local` assuming a symmetric path.
    pub fn offset_ms(&self) -> i64 {
        let mid = (self.client_send_ms as i128 + self.client_recv_ms as i128) / 2;
        (self.server_ms as i128 - mid) as i64
    }
}

/// Result of a clock offset estimation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockEstimate {
    /// Add this to local time to get server time.
    pub offset_ms: i64,
    /// Best (lowest) round trip time observed.
    pub rtt_ms: u64,
}

/// Estimates `server_ms - local_ms` from ping samples.
///
/// The samples with the lowest round trip time are the least affected by
/// queueing delays, so the best quarter (at least one, at most four) is taken
/// and the median of their offsets is used.
pub fn estimate_clock_offset(samples: &[ClockSample]) -> Option<ClockEstimate> {
    let mut valid: Vec<&ClockSample> = samples
        .iter()
        .filter(|s| s.client_recv_ms >= s.client_send_ms)
        .collect();
    if valid.is_empty() {
        return None;
    }
    valid.sort_by_key(|s| s.rtt_ms());
    let take = (valid.len() / 4).clamp(1, 4);
    let best = &valid[..take];
    let mut offsets: Vec<i64> = best.iter().map(|s| s.offset_ms()).collect();
    offsets.sort_unstable();
    let n = offsets.len();
    let offset_ms = if n % 2 == 1 {
        offsets[n / 2]
    } else {
        (offsets[n / 2 - 1] + offsets[n / 2]) / 2
    };
    Some(ClockEstimate {
        offset_ms,
        rtt_ms: best[0].rtt_ms(),
    })
}

/// Converts local time to server time.
pub fn to_server_ms(local_ms: u64, offset_ms: i64) -> u64 {
    (local_ms as i64 + offset_ms).max(0) as u64
}

/// Converts server time to local time.
pub fn to_local_ms(server_ms: u64, offset_ms: i64) -> u64 {
    (server_ms as i64 - offset_ms).max(0) as u64
}

/// What the client should do about the measured drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DriftAction {
    /// Keep everything as is.
    None,
    /// Change playback speed (1.0 restores normal speed).
    SetSpeed(f64),
    /// Seek hard to the expected position.
    Seek,
}

/// Decides how to correct drift.
///
/// `drift_ms` is `actual - expected` (positive = this client is ahead).
/// `current_speed` is the speed the player currently runs at.
pub fn decide_drift(drift_ms: i64, current_speed: f64) -> DriftAction {
    let abs = drift_ms.abs();
    if abs > DRIFT_SEEK_MS {
        return DriftAction::Seek;
    }
    let nudging = (current_speed - 1.0).abs() > 1e-6;
    let wanted = if drift_ms > 0 { SPEED_SLOW } else { SPEED_FAST };
    if nudging {
        // Overshoot (speed pushes the wrong way) or settled: back to normal.
        let wrong_direction =
            (current_speed > 1.0 && drift_ms > 0) || (current_speed < 1.0 && drift_ms < 0);
        if abs <= DRIFT_SETTLED_MS || wrong_direction {
            if abs >= DRIFT_IGNORE_MS {
                return DriftAction::SetSpeed(wanted);
            }
            return DriftAction::SetSpeed(1.0);
        }
        if (current_speed - wanted).abs() > 1e-6 {
            return DriftAction::SetSpeed(wanted);
        }
        return DriftAction::None;
    }
    if abs < DRIFT_IGNORE_MS {
        DriftAction::None
    } else {
        DriftAction::SetSpeed(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(anchor_server: u64, anchor_pos: u64) -> PlaybackState {
        PlaybackState {
            track_index: Some(0),
            status: PlayStatus::Playing,
            anchor_server_ms: anchor_server,
            anchor_position_ms: anchor_pos,
            version: 1,
        }
    }

    #[test]
    fn expected_position_playing_advances() {
        let s = playing(10_000, 5_000);
        assert_eq!(expected_position_ms(&s, 10_000), 5_000);
        assert_eq!(expected_position_ms(&s, 12_500), 7_500);
    }

    #[test]
    fn expected_position_scheduled_start_holds() {
        let s = playing(10_000, 0);
        assert_eq!(expected_position_ms(&s, 9_500), 0);
        assert_eq!(expected_position_ms(&s, 10_250), 250);
    }

    #[test]
    fn expected_position_paused_is_anchor() {
        let mut s = playing(10_000, 5_000);
        s.status = PlayStatus::Paused;
        assert_eq!(expected_position_ms(&s, 99_000), 5_000);
        s.status = PlayStatus::Stopped;
        assert_eq!(expected_position_ms(&s, 99_000), 5_000);
    }

    #[test]
    fn expected_position_clamps_to_duration() {
        let s = playing(0, 0);
        assert_eq!(expected_position_clamped(&s, 5_000, Some(3_000)), 3_000);
        assert_eq!(expected_position_clamped(&s, 5_000, None), 5_000);
    }

    #[test]
    fn sample_offset_symmetric() {
        // local 1000 -> server 5050 -> local 1100: offset 4000, rtt 100
        let s = ClockSample {
            client_send_ms: 1000,
            server_ms: 5050,
            client_recv_ms: 1100,
        };
        assert_eq!(s.rtt_ms(), 100);
        assert_eq!(s.offset_ms(), 4000);
    }

    #[test]
    fn offset_prefers_low_rtt_samples() {
        // True offset is +2000. Low-RTT samples are accurate; high-RTT ones are skewed.
        let mut samples = vec![];
        for i in 0..6u64 {
            let send = 1000 + i * 1000;
            samples.push(ClockSample {
                client_send_ms: send,
                server_ms: send + 2000 + 10,
                client_recv_ms: send + 20,
            });
        }
        // Asymmetric, slow samples (delay on the way back) would bias the estimate.
        samples.push(ClockSample {
            client_send_ms: 9000,
            server_ms: 11_010,
            client_recv_ms: 9600,
        });
        samples.push(ClockSample {
            client_send_ms: 10_000,
            server_ms: 12_010,
            client_recv_ms: 10_900,
        });
        let est = estimate_clock_offset(&samples).unwrap();
        assert_eq!(est.offset_ms, 2000);
        assert_eq!(est.rtt_ms, 20);
    }

    #[test]
    fn offset_negative_and_empty() {
        assert!(estimate_clock_offset(&[]).is_none());
        let s = ClockSample {
            client_send_ms: 10_000,
            server_ms: 4_010,
            client_recv_ms: 10_020,
        };
        let est = estimate_clock_offset(&[s]).unwrap();
        assert_eq!(est.offset_ms, -6000);
        assert_eq!(to_server_ms(10_000, est.offset_ms), 4_000);
        assert_eq!(to_local_ms(4_000, est.offset_ms), 10_000);
    }

    #[test]
    fn offset_ignores_invalid_samples() {
        let bad = ClockSample {
            client_send_ms: 100,
            server_ms: 0,
            client_recv_ms: 50,
        };
        assert!(estimate_clock_offset(&[bad]).is_none());
    }

    #[test]
    fn drift_small_is_ignored() {
        assert_eq!(decide_drift(0, 1.0), DriftAction::None);
        assert_eq!(decide_drift(49, 1.0), DriftAction::None);
        assert_eq!(decide_drift(-49, 1.0), DriftAction::None);
    }

    #[test]
    fn drift_medium_nudges_speed() {
        assert_eq!(decide_drift(200, 1.0), DriftAction::SetSpeed(SPEED_SLOW));
        assert_eq!(decide_drift(-200, 1.0), DriftAction::SetSpeed(SPEED_FAST));
        assert_eq!(decide_drift(1000, 1.0), DriftAction::SetSpeed(SPEED_SLOW));
        assert_eq!(decide_drift(-50, 1.0), DriftAction::SetSpeed(SPEED_FAST));
    }

    #[test]
    fn drift_large_seeks() {
        assert_eq!(decide_drift(1001, 1.0), DriftAction::Seek);
        assert_eq!(decide_drift(-5000, SPEED_FAST), DriftAction::Seek);
    }

    #[test]
    fn nudging_continues_until_settled() {
        // Behind by 40 ms while speeding up: keep going (hysteresis).
        assert_eq!(decide_drift(-40, SPEED_FAST), DriftAction::None);
        // Within 20 ms: back to normal.
        assert_eq!(decide_drift(-15, SPEED_FAST), DriftAction::SetSpeed(1.0));
        assert_eq!(decide_drift(18, SPEED_SLOW), DriftAction::SetSpeed(1.0));
    }

    #[test]
    fn nudging_overshoot_resets_or_reverses() {
        // Was catching up, now 30 ms ahead: stop speeding.
        assert_eq!(decide_drift(30, SPEED_FAST), DriftAction::SetSpeed(1.0));
        // Was catching up, now 300 ms ahead: reverse.
        assert_eq!(
            decide_drift(300, SPEED_FAST),
            DriftAction::SetSpeed(SPEED_SLOW)
        );
    }
}
