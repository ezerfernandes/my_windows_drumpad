//! Keyboard velocity shaping and humanization.
//!
//! A keyboard switch is on/off, so velocity comes from configuration: a fixed
//! value with optional per-pad sensitivity, or a fixed base with a random
//! spread. Humanization uses a seeded SplitMix64 generator so tests and
//! rehearsals are reproducible, and every result is clamped into `1..=127`.

use serde::{Deserialize, Serialize};

use crate::trigger::{PAD_COUNT, PadId, Velocity, VelocityExt};

/// How keyboard hits get their velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "settings")]
pub enum VelocityMode {
    /// Every hit uses the same velocity.
    Fixed(Velocity),
    /// Hits vary around `base` by at most `spread` (inclusive).
    Humanized {
        /// Center velocity.
        base: Velocity,
        /// Maximum deviation in either direction, `0..=63`.
        spread: u8,
    },
}

impl Default for VelocityMode {
    fn default() -> Self {
        Self::Fixed(Velocity::DEFAULT)
    }
}

impl VelocityMode {
    /// Center velocity before per-pad sensitivity.
    #[must_use]
    pub const fn base(self) -> Velocity {
        match self {
            Self::Fixed(value) => value,
            Self::Humanized { base, .. } => base,
        }
    }

    /// Maximum random deviation.
    #[must_use]
    pub const fn spread(self) -> u8 {
        match self {
            Self::Fixed(_) => 0,
            Self::Humanized { spread, .. } => spread,
        }
    }

    /// Build a humanized mode, clamping the spread to `0..=63`.
    #[must_use]
    pub fn humanized(base: Velocity, spread: u8) -> Self {
        Self::Humanized {
            base,
            spread: spread.min(63),
        }
    }
}

const NO_SENSITIVITY: [i8; PAD_COUNT] = [0; PAD_COUNT];

/// Keyboard velocity configuration for the whole grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardVelocityConfig {
    /// Base mode.
    pub mode: VelocityMode,
    /// Per-pad offset applied before humanization, in `-64..=63`.
    #[serde(default = "no_sensitivity")]
    pub pad_sensitivity: [i8; PAD_COUNT],
}

const fn no_sensitivity() -> [i8; PAD_COUNT] {
    NO_SENSITIVITY
}

impl Default for KeyboardVelocityConfig {
    fn default() -> Self {
        Self {
            mode: VelocityMode::default(),
            pad_sensitivity: NO_SENSITIVITY,
        }
    }
}

impl KeyboardVelocityConfig {
    /// Configuration with no per-pad offsets.
    #[must_use]
    pub fn new(mode: VelocityMode) -> Self {
        Self {
            mode,
            pad_sensitivity: NO_SENSITIVITY,
        }
    }

    /// Offset configured for `pad`.
    #[must_use]
    pub fn sensitivity(&self, pad: PadId) -> i8 {
        self.pad_sensitivity[pad.index()]
    }

    /// Set the offset for `pad`, clamping to `-64..=63`.
    pub fn set_sensitivity(&mut self, pad: PadId, offset: i8) {
        self.pad_sensitivity[pad.index()] = offset.clamp(-64, 63);
    }
}

/// Stateful generator: one per keyboard capture path.
#[derive(Debug, Clone)]
pub struct VelocityGenerator {
    config: KeyboardVelocityConfig,
    rng: SplitMix64,
}

impl VelocityGenerator {
    /// Create a generator with an explicit seed.
    #[must_use]
    pub fn new(config: KeyboardVelocityConfig, seed: u64) -> Self {
        Self {
            config,
            rng: SplitMix64::new(seed),
        }
    }

    /// Current configuration.
    #[must_use]
    pub fn config(&self) -> &KeyboardVelocityConfig {
        &self.config
    }

    /// Replace the configuration without resetting the random sequence.
    pub fn set_config(&mut self, config: KeyboardVelocityConfig) {
        self.config = config;
    }

    /// Restart the deterministic sequence.
    pub fn reseed(&mut self, seed: u64) {
        self.rng = SplitMix64::new(seed);
    }

    /// Next velocity for `pad`.
    pub fn velocity_for(&mut self, pad: PadId) -> Velocity {
        let base = i32::from(self.config.mode.base().get());
        let sensitivity = i32::from(self.config.sensitivity(pad));
        let spread = i32::from(self.config.mode.spread());
        let jitter = if spread == 0 {
            0
        } else {
            self.rng.next_in_range(spread)
        };
        Velocity::clamp_signed(base + sensitivity + jitter)
    }
}

impl Default for VelocityGenerator {
    fn default() -> Self {
        Self::new(KeyboardVelocityConfig::default(), 0)
    }
}

/// Small deterministic PRNG (SplitMix64) so velocity sequences are testable
/// and identical across runs.
#[derive(Debug, Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `-range..=range`.
    fn next_in_range(&mut self, range: i32) -> i32 {
        debug_assert!(range > 0);
        let span = u64::try_from(range * 2 + 1).expect("positive span");
        i32::try_from(self.next_u64() % span).expect("span fits i32") - range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{MAX_VELOCITY, MIN_VELOCITY, PadIdExt};

    fn pad(index: u8) -> PadId {
        PadId::from_index(index)
    }

    #[test]
    fn fixed_mode_ignores_the_random_stream() {
        let config = KeyboardVelocityConfig::new(VelocityMode::Fixed(Velocity::new(90).unwrap()));
        let mut a = VelocityGenerator::new(config, 1);
        let mut b = VelocityGenerator::new(config, 2);
        for pad in PadId::ALL {
            assert_eq!(a.velocity_for(pad), Velocity::new(90).unwrap());
            assert_eq!(b.velocity_for(pad), Velocity::new(90).unwrap());
        }
    }

    #[test]
    fn humanization_stays_within_the_configured_range() {
        let base = Velocity::new(64).unwrap();
        let config = KeyboardVelocityConfig::new(VelocityMode::humanized(base, 12));
        let mut generator = VelocityGenerator::new(config, 0xDEAD_BEEF);
        for _ in 0..10_000 {
            let velocity = generator.velocity_for(pad(5));
            assert!(
                (52..=76).contains(&velocity.get()),
                "velocity {} left 52..=76",
                velocity.get()
            );
        }
    }

    #[test]
    fn humanization_is_deterministic_for_a_seed() {
        let config =
            KeyboardVelocityConfig::new(VelocityMode::humanized(Velocity::new(80).unwrap(), 20));
        let mut a = VelocityGenerator::new(config, 42);
        let mut b = VelocityGenerator::new(config, 42);
        for _ in 0..256 {
            assert_eq!(a.velocity_for(pad(1)), b.velocity_for(pad(1)));
        }
        a.reseed(7);
        b.reseed(7);
        assert_eq!(a.velocity_for(pad(1)), b.velocity_for(pad(1)));
    }

    #[test]
    fn per_pad_sensitivity_shifts_the_result() {
        let mut config =
            KeyboardVelocityConfig::new(VelocityMode::Fixed(Velocity::new(100).unwrap()));
        config.set_sensitivity(pad(0), 20);
        config.set_sensitivity(pad(1), -20);
        config.set_sensitivity(pad(2), 100); // clamped to 63
        let mut generator = VelocityGenerator::new(config, 0);
        assert_eq!(generator.velocity_for(pad(0)).get(), 120);
        assert_eq!(generator.velocity_for(pad(1)).get(), 80);
        assert_eq!(generator.velocity_for(pad(2)).get(), 127);
        assert_eq!(config.sensitivity(pad(2)), 63);
    }

    #[test]
    fn velocity_never_leaves_the_midi_range() {
        let mut config = KeyboardVelocityConfig::new(VelocityMode::humanized(MIN_VELOCITY, 63));
        config.set_sensitivity(pad(0), -64);
        let mut generator = VelocityGenerator::new(config, 9);
        for _ in 0..1_000 {
            assert!(generator.velocity_for(pad(0)).get() >= 1);
        }

        let mut high = KeyboardVelocityConfig::new(VelocityMode::humanized(MAX_VELOCITY, 63));
        high.set_sensitivity(pad(0), 63);
        let mut generator = VelocityGenerator::new(high, 9);
        for _ in 0..1_000 {
            assert_eq!(generator.velocity_for(pad(0)), MAX_VELOCITY);
        }
    }

    #[test]
    fn configuration_round_trips_through_json() {
        let mut config =
            KeyboardVelocityConfig::new(VelocityMode::humanized(Velocity::new(70).unwrap(), 9));
        config.set_sensitivity(pad(3), -12);
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<KeyboardVelocityConfig>(&json).unwrap(),
            config
        );
    }
}
