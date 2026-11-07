//! Zero-latency soft-knee master limiter and clip guard.
//!
//! The limiter is a stereo-linked peak limiter with a soft knee, a short
//! attack, and a slower release. It adds **zero latency** (it is a memoryless
//! gain stage plus an envelope follower), so it does not need to be reported
//! in the output latency budget. A final clamp guarantees that no sample can
//! leave the engine above 0 dBFS even under extreme overload.

/// Limiter configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterConfig {
    /// Level above which gain reduction starts, in linear amplitude.
    /// `0.891` is -1 dBFS.
    pub threshold: f32,
    /// Fraction of the threshold used as the soft-knee width.
    pub knee_fraction: f32,
    /// Attack time constant in milliseconds.
    pub attack_ms: f32,
    /// Release time constant in milliseconds.
    pub release_ms: f32,
    /// When false the limiter is bypassed (the final clip guard still runs).
    pub enabled: bool,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            threshold: 0.891,
            knee_fraction: 0.12,
            attack_ms: 1.0,
            release_ms: 60.0,
            enabled: true,
        }
    }
}

/// Stereo peak limiter state.
#[derive(Debug, Clone)]
pub struct Limiter {
    config: LimiterConfig,
    attack_coeff: f32,
    release_coeff: f32,
    gain: f32,
    max_reduction: f32,
}

impl Limiter {
    /// Build a limiter for `sample_rate`.
    pub fn new(sample_rate: u32, mut config: LimiterConfig) -> Self {
        config.knee_fraction = config.knee_fraction.clamp(0.0, 1.0);
        let rate = sample_rate.max(1) as f32;
        let attack_coeff = time_coeff(config.attack_ms, rate);
        let release_coeff = time_coeff(config.release_ms, rate);
        Self {
            config,
            attack_coeff,
            release_coeff,
            gain: 1.0,
            max_reduction: 0.0,
        }
    }

    /// Current configuration.
    pub fn config(&self) -> LimiterConfig {
        self.config
    }

    /// Enable or bypass limiting.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if !enabled {
            self.gain = 1.0;
        }
    }

    /// True when limiting is active.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Latency added by the limiter in frames. Always zero by design.
    pub fn latency_frames(&self) -> u32 {
        0
    }

    /// Largest gain reduction observed since construction, in dB (positive).
    pub fn max_reduction_db(&self) -> f32 {
        let remaining = (1.0 - self.max_reduction).max(1e-9);
        -20.0 * remaining.log10()
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.config.enabled {
            return (clip_guard(left), clip_guard(right));
        }
        let peak = left.abs().max(right.abs());
        let desired = self.desired_gain(peak);
        let coeff = if desired < self.gain {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.gain += (desired - self.gain) * coeff;
        self.gain = self.gain.clamp(0.0, 1.0);
        let reduction = 1.0 - self.gain;
        if reduction > self.max_reduction {
            self.max_reduction = reduction;
        }
        (clip_guard(left * self.gain), clip_guard(right * self.gain))
    }

    /// Process an interleaved stereo block in place.
    pub fn process_block(&mut self, interleaved: &mut [f32]) {
        for frame in interleaved.chunks_exact_mut(2) {
            let (l, r) = self.process(frame[0], frame[1]);
            frame[0] = l;
            frame[1] = r;
        }
    }

    #[inline]
    fn desired_gain(&self, peak: f32) -> f32 {
        let threshold = self.config.threshold.clamp(1e-6, 1.0);
        if peak <= threshold * (1.0 - self.config.knee_fraction) {
            return 1.0;
        }
        if peak >= threshold {
            return threshold / peak;
        }
        // Smooth polynomial knee between the linear and limited regions.
        let knee_start = threshold * (1.0 - self.config.knee_fraction);
        let t = (peak - knee_start) / (threshold - knee_start).max(1e-9);
        let limit = threshold / peak;
        1.0 + (limit - 1.0) * t * t
    }
}

/// Largest representable output magnitude, strictly below 1.0 (0 dBFS).
/// Keeping the ceiling one ULP below full scale means an offline render never
/// records a sample that equals digital full scale, while remaining inaudible.
pub const MAX_OUTPUT: f32 = 1.0 - f32::EPSILON;

/// Final safety net: never emit a sample at or outside digital full scale.
#[inline]
pub fn clip_guard(x: f32) -> f32 {
    if x > MAX_OUTPUT {
        MAX_OUTPUT
    } else if x < -MAX_OUTPUT {
        -MAX_OUTPUT
    } else if x.is_nan() {
        0.0
    } else {
        x
    }
}

#[inline]
fn time_coeff(ms: f32, rate: f32) -> f32 {
    let seconds = ms.max(0.01) / 1000.0;
    (1.0 - (-1.0 / (seconds * rate)).exp()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(freq: f32, amp: f32, rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / rate).sin() * amp)
            .collect()
    }

    #[test]
    fn transparent_below_threshold() {
        let rate = 48_000;
        let mut limiter = Limiter::new(rate, LimiterConfig::default());
        let input = sine(440.0, 0.5, rate as f32, 4800); // -6 dBFS
        let mut worst = 0.0f32;
        for &s in &input {
            let (l, r) = limiter.process(s, s);
            worst = worst.max((l - s).abs().max((r - s).abs()));
        }
        assert!(worst < 1e-6, "limiter coloured a -6 dBFS sine: {worst}");
        assert!(limiter.max_reduction_db() < 0.0001);
    }

    #[test]
    fn stress_never_exceeds_zero_dbfs() {
        let rate = 48_000;
        let mut limiter = Limiter::new(rate, LimiterConfig::default());
        let mut peak = 0.0f32;
        // Four full-scale voices per channel is a hard overload.
        for i in 0..48_000 {
            let a = (2.0 * PI * 100.0 * i as f32 / rate as f32).sin() * 4.0;
            let b = (2.0 * PI * 350.0 * i as f32 / rate as f32).sin() * 4.0;
            let (l, r) = limiter.process(a + b, a - b);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak <= 1.0, "limiter emitted {peak} (> 0 dBFS)");
        // The limiter must actually have engaged.
        assert!(limiter.max_reduction_db() > 6.0);
    }

    #[test]
    fn attack_reduces_gain_and_release_recovers() {
        let rate = 48_000;
        let mut limiter = Limiter::new(rate, LimiterConfig::default());
        // 20 ms of a loud constant.
        let mut late_gain = 1.0;
        for _ in 0..960 {
            let (l, _) = limiter.process(2.0, 2.0);
            late_gain = l / 2.0;
        }
        // Gain reduction must land near the threshold / peak ratio.
        assert!((late_gain - 0.4455).abs() < 0.05, "gain {late_gain}");
        // 500 ms of quiet: gain recovers towards unity.
        let mut recovered = 0.0;
        for _ in 0..24_000 {
            let (l, _) = limiter.process(0.1, 0.1);
            recovered = l / 0.1;
        }
        assert!(
            recovered > 0.95,
            "limiter did not release, gain {recovered}"
        );
    }

    #[test]
    fn declares_zero_latency() {
        let limiter = Limiter::new(48_000, LimiterConfig::default());
        assert_eq!(limiter.latency_frames(), 0);
    }

    #[test]
    fn bypass_is_exact_passthrough_below_clip() {
        let mut limiter = Limiter::new(48_000, LimiterConfig::default());
        limiter.set_enabled(false);
        let (l, r) = limiter.process(0.75, -0.75);
        assert_eq!((l, r), (0.75, -0.75));
    }

    #[test]
    fn clip_guard_handles_non_finite_values() {
        assert_eq!(clip_guard(2.0), MAX_OUTPUT);
        assert_eq!(clip_guard(-2.0), -MAX_OUTPUT);
        assert_eq!(clip_guard(f32::NAN), 0.0);
        assert_eq!(clip_guard(0.25), 0.25);
    }
}
