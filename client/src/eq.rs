//! 10-band graphic equalizer, applied by mpv as one lavfi filter graph.
//! Gain changes while playing go through `af-command`, so the filter chain
//! is never rebuilt mid-song (no dropout, no timing hiccup).

use serde::{Deserialize, Serialize};

pub const BANDS: usize = 10;
/// Center frequencies (Hz).
pub const FREQS: [u32; BANDS] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const MAX_DB: f64 = 12.0;
/// mpv filter label.
pub const LABEL: &str = "synctube-eq";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    /// dB per band, -12..=12.
    pub gains: [f64; BANDS],
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            gains: [0.0; BANDS],
        }
    }
}

impl EqSettings {
    pub fn clamped(mut self) -> Self {
        for g in &mut self.gains {
            *g = if g.is_finite() {
                g.clamp(-MAX_DB, MAX_DB)
            } else {
                0.0
            };
        }
        self
    }

    /// Lowers the level by the largest boost, so boosted bands never clip.
    pub fn preamp_db(&self) -> f64 {
        -self.gains.iter().copied().fold(0.0, f64::max)
    }

    /// The preset whose gains these are, if any.
    pub fn preset(&self) -> Option<usize> {
        PRESETS.iter().position(|(_, g)| {
            g.iter()
                .zip(self.gains.iter())
                .all(|(a, b)| (a - b).abs() < 0.05)
        })
    }
}

/// Name and gains of each built-in preset.
pub const PRESETS: [(&str, [f64; BANDS]); 6] = [
    ("Ravno", [0.0; BANDS]),
    ("Bas", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Rok", [4.5, 3.5, 2.0, 0.0, -1.0, -0.5, 1.0, 2.5, 3.5, 4.0]),
    ("Pop", [-1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0, -0.5, -1.0]),
    (
        "Vokal",
        [-2.0, -1.5, -1.0, 1.0, 3.0, 3.5, 3.0, 2.0, 0.5, -0.5],
    ),
    (
        "Elektronika",
        [5.0, 4.0, 1.5, 0.0, -1.5, 0.5, 1.0, 2.0, 4.0, 4.5],
    ),
];

/// mpv `af` value for these settings ("" = no filter). Depends only on
/// `enabled`, so dragging a slider never rebuilds the filter chain.
pub fn af_value(eq: &EqSettings) -> String {
    if !eq.enabled {
        return String::new();
    }
    let bands: Vec<String> = FREQS
        .iter()
        .zip(eq.gains.iter())
        .enumerate()
        .map(|(i, (f, g))| format!("equalizer@b{i}=f={f}:t=o:w=1:g={g:.1}"))
        .collect();
    format!(
        "@{LABEL}:lavfi=[volume@pre=volume={:.1}dB:precision=float,{}]",
        eq.preamp_db(),
        bands.join(",")
    )
}

/// Instance names for `af-command` targets.
pub fn band_target(i: usize) -> String {
    format!("equalizer@b{i}")
}

pub const PREAMP_TARGET: &str = "volume@pre";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_and_preamp() {
        let mut eq = EqSettings::default();
        assert_eq!(af_value(&eq), "");
        eq.enabled = true;
        assert!(
            af_value(&eq).contains("g=0.0"),
            "on but flat keeps the filter"
        );
        eq.gains[0] = 6.0;
        eq.gains[9] = -3.0;
        let v = af_value(&eq);
        assert!(v.starts_with("@synctube-eq:lavfi=[volume@pre=volume=-6.0dB"));
        assert!(v.contains("equalizer@b0=f=32:t=o:w=1:g=6.0"));
        assert!(v.contains("equalizer@b9=f=16000:t=o:w=1:g=-3.0]"));
        assert_eq!(eq.preset(), None);
        eq.gains = PRESETS[1].1;
        assert_eq!(eq.preset(), Some(1));
        eq.gains[3] = 99.0;
        assert_eq!(eq.clamped().gains[3], MAX_DB);
    }
}
