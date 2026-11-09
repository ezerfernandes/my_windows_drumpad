//! Hi-hat articulation engine for discrete Nitro Max notes and CC pedals.
//!
//! The Nitro Max sends separate notes for open, half-open, closed, pedal and
//! splash hi-hat sounds. Other modules send a continuous position on CC4 (or a
//! pedal switch on CC64). The engine normalizes both into
//! [`HiHatArticulation`] plus a sound kind:
//!
//! | Input | Articulation | Sound |
//! | --- | --- | --- |
//! | note 46 | open | hat |
//! | note 23 | half-open | hat |
//! | note 42 | closed | hat |
//! | note 44 | closed | pedal chick |
//! | note 21 | splash | splash |
//! | CC4 sweep | open/half/closed | hat |
//! | CC64 down | closed | pedal chick |
//!
//! Hysteresis keeps a pedal resting near a threshold from chattering: CC4 only
//! emits when the articulation actually changes, and the change bands are
//! separated by [`HiHatConfig::hysteresis`].

use serde::{Deserialize, Serialize};

use crate::trigger::Velocity;

/// Hi-hat position/articulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiHatArticulation {
    /// Fully open.
    Open,
    /// Half-open.
    HalfOpen,
    /// Closed with the stick.
    Closed,
    /// Pedal chick (closing without a stick hit).
    Pedal,
    /// Foot splash.
    Splash,
}

impl HiHatArticulation {
    /// Stable identifier used in kits and profiles.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::HalfOpen => "half_open",
            Self::Closed => "closed",
            Self::Pedal => "pedal",
            Self::Splash => "splash",
        }
    }
}

/// What sample/layer family the hit belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiHatSound {
    /// A stick hit on the hat, using the articulation's layer.
    Hat,
    /// The pedal closing without a stick hit.
    PedalChick,
    /// Foot splash.
    Splash,
}

/// One hi-hat result ready for pad/layer selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiHatTrigger {
    /// Articulation to choose within the hi-hat pad.
    pub articulation: HiHatArticulation,
    /// Sound family.
    pub sound: HiHatSound,
    /// Velocity to apply to the selected layer.
    pub velocity: Velocity,
}

impl HiHatTrigger {
    fn new(articulation: HiHatArticulation, sound: HiHatSound, velocity: Velocity) -> Self {
        Self {
            articulation,
            sound,
            velocity,
        }
    }
}

/// Note numbers and CC behavior for the hi-hat engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiHatConfig {
    /// Open hi-hat note.
    pub open_note: u8,
    /// Half-open note.
    pub half_open_note: u8,
    /// Closed hi-hat note.
    pub closed_note: u8,
    /// Pedal (chick) note.
    pub pedal_note: u8,
    /// Foot splash note.
    pub splash_note: u8,
    /// CC4 controls continuous position when true.
    pub cc4_position: bool,
    /// CC64 acts as a pedal switch (>= 64 is down) when true.
    pub cc64_pedal: bool,
    /// CC4 value at or below which the hat is closed.
    pub closed_threshold: u8,
    /// CC4 value at or above which the hat is open.
    pub open_threshold: u8,
    /// Extra distance past a threshold before the articulation changes.
    pub hysteresis: u8,
    /// Velocity assigned to CC-driven hits.
    pub cc_velocity: Velocity,
}

impl Default for HiHatConfig {
    fn default() -> Self {
        Self {
            open_note: 46,
            half_open_note: 23,
            closed_note: 42,
            pedal_note: 44,
            splash_note: 21,
            cc4_position: true,
            cc64_pedal: true,
            closed_threshold: 32,
            open_threshold: 96,
            hysteresis: 8,
            cc_velocity: Velocity::DEFAULT,
        }
    }
}

impl HiHatConfig {
    /// Validate thresholds in `0..=127` with `closed < open`.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.closed_threshold < self.open_threshold && self.open_threshold <= 127
    }
}

/// Stateful hi-hat pedal/position tracker.
#[derive(Debug, Clone)]
pub struct HiHatEngine {
    config: HiHatConfig,
    articulation: HiHatArticulation,
    pedal_down: bool,
    position: u8,
}

impl HiHatEngine {
    /// Build an engine with the default Nitro Max configuration.
    #[must_use]
    pub fn new(config: HiHatConfig) -> Self {
        assert!(config.is_valid(), "invalid hi-hat thresholds");
        Self {
            config,
            articulation: HiHatArticulation::Closed,
            pedal_down: false,
            position: 0,
        }
    }

    /// Default Nitro Max configuration.
    #[must_use]
    pub fn nitro_max() -> Self {
        Self::new(HiHatConfig::default())
    }

    /// Active configuration.
    #[must_use]
    pub fn config(&self) -> &HiHatConfig {
        &self.config
    }

    /// Last articulation selected by notes or CC.
    #[must_use]
    pub fn articulation(&self) -> HiHatArticulation {
        self.articulation
    }

    /// Last CC4 position, `0..=127`.
    #[must_use]
    pub fn position(&self) -> u8 {
        self.position
    }

    /// Handle a note message.
    ///
    /// Returns `None` for notes outside the configured hi-hat range so the
    /// caller can route them as ordinary pads.
    pub fn note(&mut self, note: u8, velocity: Velocity) -> Option<HiHatTrigger> {
        let (articulation, sound) = if note == self.config.open_note {
            (HiHatArticulation::Open, HiHatSound::Hat)
        } else if note == self.config.half_open_note {
            (HiHatArticulation::HalfOpen, HiHatSound::Hat)
        } else if note == self.config.closed_note {
            (HiHatArticulation::Closed, HiHatSound::Hat)
        } else if note == self.config.pedal_note {
            (HiHatArticulation::Pedal, HiHatSound::PedalChick)
        } else if note == self.config.splash_note {
            (HiHatArticulation::Splash, HiHatSound::Splash)
        } else {
            return None;
        };
        self.articulation = articulation;
        Some(HiHatTrigger::new(articulation, sound, velocity))
    }

    /// Handle a controller change.
    ///
    /// Returns a trigger only when a CC changes the articulation; a stationary
    /// or chattering pedal produces nothing.
    pub fn control_change(&mut self, controller: u8, value: u8) -> Option<HiHatTrigger> {
        match controller {
            4 if self.config.cc4_position => {
                self.position = value;
                let next = self.classify(value);
                if next == self.articulation {
                    return None;
                }
                self.articulation = next;
                Some(HiHatTrigger::new(
                    next,
                    HiHatSound::Hat,
                    self.config.cc_velocity,
                ))
            }
            64 if self.config.cc64_pedal => {
                let down = value >= 64;
                if down == self.pedal_down {
                    return None;
                }
                self.pedal_down = down;
                if down {
                    self.articulation = HiHatArticulation::Pedal;
                    Some(HiHatTrigger::new(
                        HiHatArticulation::Pedal,
                        HiHatSound::PedalChick,
                        self.config.cc_velocity,
                    ))
                } else {
                    // Lifting the pedal does not itself make a sound.
                    self.articulation = HiHatArticulation::Open;
                    None
                }
            }
            _ => None,
        }
    }

    /// Classify a CC4 value with hysteresis relative to the current
    /// articulation.
    fn classify(&self, value: u8) -> HiHatArticulation {
        let closed = self.config.closed_threshold;
        let open = self.config.open_threshold;
        let hysteresis = self.config.hysteresis;
        match self.articulation {
            HiHatArticulation::Closed | HiHatArticulation::Pedal | HiHatArticulation::Splash => {
                if value >= open {
                    HiHatArticulation::Open
                } else if value > closed.saturating_add(hysteresis) {
                    HiHatArticulation::HalfOpen
                } else {
                    HiHatArticulation::Closed
                }
            }
            HiHatArticulation::HalfOpen => {
                if value >= open {
                    HiHatArticulation::Open
                } else if value <= closed {
                    HiHatArticulation::Closed
                } else {
                    HiHatArticulation::HalfOpen
                }
            }
            HiHatArticulation::Open => {
                if value <= closed {
                    HiHatArticulation::Closed
                } else if value < open.saturating_sub(hysteresis) {
                    HiHatArticulation::HalfOpen
                } else {
                    HiHatArticulation::Open
                }
            }
        }
    }
}

impl Default for HiHatEngine {
    fn default() -> Self {
        Self::nitro_max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn velocity(value: u8) -> Velocity {
        Velocity::new(value).unwrap()
    }

    #[test]
    fn nitro_max_notes_select_articulations() {
        let mut engine = HiHatEngine::nitro_max();
        assert_eq!(
            engine.note(46, velocity(100)),
            Some(HiHatTrigger::new(
                HiHatArticulation::Open,
                HiHatSound::Hat,
                velocity(100)
            ))
        );
        assert_eq!(engine.articulation(), HiHatArticulation::Open);
        assert_eq!(
            engine.note(23, velocity(90)).unwrap().articulation,
            HiHatArticulation::HalfOpen
        );
        assert_eq!(
            engine.note(42, velocity(80)).unwrap().articulation,
            HiHatArticulation::Closed
        );
        let pedal = engine.note(44, velocity(110)).unwrap();
        assert_eq!(pedal.articulation, HiHatArticulation::Pedal);
        assert_eq!(pedal.sound, HiHatSound::PedalChick);
        let splash = engine.note(21, velocity(60)).unwrap();
        assert_eq!(splash.articulation, HiHatArticulation::Splash);
        assert_eq!(splash.sound, HiHatSound::Splash);
    }

    #[test]
    fn other_notes_are_left_for_pad_routing() {
        let mut engine = HiHatEngine::nitro_max();
        assert_eq!(engine.note(36, velocity(100)), None);
        assert_eq!(engine.articulation(), HiHatArticulation::Closed);
    }

    #[test]
    fn cc4_sweep_moves_closed_half_open() {
        let mut engine = HiHatEngine::nitro_max();
        let mut seen = Vec::new();
        for value in [0, 20, 40, 60, 80, 95, 110, 127] {
            if let Some(trigger) = engine.control_change(4, value) {
                seen.push(trigger.articulation);
            }
        }
        assert_eq!(
            seen,
            vec![HiHatArticulation::HalfOpen, HiHatArticulation::Open,]
        );
        assert_eq!(engine.articulation(), HiHatArticulation::Open);
    }

    #[test]
    fn cc4_chatter_does_not_retrigger() {
        let mut engine = HiHatEngine::nitro_max();
        // Walk to the closed/half-open boundary from below and wobble there.
        assert_eq!(engine.control_change(4, 30), None);
        let mut triggers = Vec::new();
        for value in [40, 41, 40, 41, 39, 40, 41] {
            if let Some(trigger) = engine.control_change(4, value) {
                triggers.push(trigger.articulation);
            }
        }
        assert_eq!(triggers, vec![HiHatArticulation::HalfOpen]);
        // Staying inside the band emits nothing more.
        assert_eq!(engine.control_change(4, 45), None);
        assert_eq!(engine.control_change(4, 50), None);
    }

    #[test]
    fn cc4_open_boundary_has_hysteresis() {
        let mut engine = HiHatEngine::nitro_max();
        assert_eq!(
            engine.control_change(4, 100).unwrap().articulation,
            HiHatArticulation::Open
        );
        assert_eq!(engine.control_change(4, 90), None);
        assert_eq!(
            engine.control_change(4, 80).unwrap().articulation,
            HiHatArticulation::HalfOpen
        );
    }

    #[test]
    fn cc64_pedal_chick_fires_once_per_press() {
        let mut engine = HiHatEngine::nitro_max();
        let chick = engine.control_change(64, 100).unwrap();
        assert_eq!(chick.articulation, HiHatArticulation::Pedal);
        assert_eq!(chick.sound, HiHatSound::PedalChick);
        assert_eq!(engine.control_change(64, 101), None);
        assert_eq!(engine.control_change(64, 63), None);
        assert_eq!(engine.articulation(), HiHatArticulation::Open);
        let again = engine.control_change(64, 64).unwrap();
        assert_eq!(again.sound, HiHatSound::PedalChick);
    }

    #[test]
    fn disabling_cc_modes_ignores_them() {
        let config = HiHatConfig {
            cc4_position: false,
            cc64_pedal: false,
            ..HiHatConfig::default()
        };
        let mut engine = HiHatEngine::new(config);
        assert_eq!(engine.control_change(4, 127), None);
        assert_eq!(engine.control_change(64, 127), None);
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        let config = HiHatConfig {
            closed_threshold: 100,
            open_threshold: 50,
            ..HiHatConfig::default()
        };
        assert!(!config.is_valid());
        let result = std::panic::catch_unwind(|| HiHatEngine::new(config));
        assert!(result.is_err());
    }

    #[test]
    fn articulations_expose_stable_ids() {
        let expected = [
            (HiHatArticulation::Open, "open"),
            (HiHatArticulation::HalfOpen, "half_open"),
            (HiHatArticulation::Closed, "closed"),
            (HiHatArticulation::Pedal, "pedal"),
            (HiHatArticulation::Splash, "splash"),
        ];
        for (articulation, id) in expected {
            assert_eq!(articulation.as_str(), id);
        }
    }

    #[test]
    fn position_tracks_cc4_and_default_is_nitro_max() {
        let mut engine = HiHatEngine::default();
        assert_eq!(engine.position(), 0);
        assert_eq!(engine.config().open_note, 46);
        engine.control_change(4, 77);
        assert_eq!(engine.position(), 77);
        engine.control_change(4, 12);
        assert_eq!(engine.position(), 12);
    }

    #[test]
    fn cc4_returns_to_closed_from_half_open_and_open() {
        let mut engine = HiHatEngine::nitro_max();
        let mut seen = Vec::new();
        for value in [60, 30, 127, 0] {
            if let Some(trigger) = engine.control_change(4, value) {
                seen.push(trigger.articulation);
            }
        }
        assert_eq!(
            seen,
            vec![
                HiHatArticulation::HalfOpen,
                HiHatArticulation::Closed,
                HiHatArticulation::Open,
                HiHatArticulation::Closed,
            ]
        );
    }
}
