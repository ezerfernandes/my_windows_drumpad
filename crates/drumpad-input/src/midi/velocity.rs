//! MIDI velocity curves.
//!
//! A curve is compiled once into a 128-entry lookup table so the MIDI callback
//! never evaluates a powf or allocates. Linear, soft, hard and fixed curves are
//! monotonic where that is meaningful; custom multi-point curves interpolate
//! between user points and are validated to be strictly increasing in input.

use serde::{Deserialize, Serialize};

use crate::trigger::{MIN_VELOCITY, PAD_COUNT, PadId, Velocity, VelocityExt};

/// A velocity response curve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "type", content = "settings")]
pub enum VelocityCurve {
    /// Identity.
    #[default]
    Linear,
    /// Lifts soft hits (square root).
    Soft,
    /// Reins in hard hits (square).
    Hard,
    /// Every note uses one velocity.
    Fixed(Velocity),
    /// Piecewise-linear points `(input, output)`, sorted by input.
    Custom(Vec<(u8, u8)>),
}

/// A curve that cannot be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurveError {
    /// A custom curve needs at least two points.
    #[error("custom velocity curve needs at least two points")]
    TooFewPoints,
    /// Points must be strictly increasing in input.
    #[error("custom velocity curve points must have strictly increasing input values")]
    NotIncreasing,
    /// Inputs and outputs must stay within `0..=127` (`1..=127` for output).
    #[error("custom velocity curve point ({input}, {output}) is out of range")]
    OutOfRange {
        /// Input velocity.
        input: u8,
        /// Output velocity.
        output: u8,
    },
}

/// Compiled 128-entry velocity table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VelocityCurveTable {
    table: [u8; 128],
}

impl VelocityCurveTable {
    /// Compile a curve.
    pub fn new(curve: &VelocityCurve) -> Result<Self, CurveError> {
        let mut table = [1_u8; 128];
        for (index, slot) in table.iter_mut().enumerate() {
            let raw = index as u8;
            let shaped = match curve {
                VelocityCurve::Linear => i32::from(raw),
                VelocityCurve::Soft => {
                    let normalized = f64::from(raw.saturating_sub(1)) / 126.0;
                    (1.0 + normalized.sqrt() * 126.0).round() as i32
                }
                VelocityCurve::Hard => {
                    let normalized = f64::from(raw.saturating_sub(1)) / 126.0;
                    (1.0 + normalized * normalized * 126.0).round() as i32
                }
                VelocityCurve::Fixed(velocity) => i32::from(velocity.get()),
                VelocityCurve::Custom(points) => custom_lookup(points, raw)?,
            };
            *slot = Velocity::clamp_signed(shaped).get();
        }
        Ok(Self { table })
    }

    /// Map a raw MIDI velocity `0..=127`; zero is mapped as the softest hit.
    #[must_use]
    pub fn map_raw(&self, raw: u8) -> Velocity {
        let value = self.table[usize::from(raw.min(127))];
        Velocity::new(value).unwrap_or(MIN_VELOCITY)
    }

    /// Map a validated velocity.
    #[must_use]
    pub fn map(&self, velocity: Velocity) -> Velocity {
        self.map_raw(velocity.get())
    }

    /// Whole table for a UI curve preview.
    #[must_use]
    pub fn table(&self) -> &[u8; 128] {
        &self.table
    }
}

fn custom_lookup(points: &[(u8, u8)], raw: u8) -> Result<i32, CurveError> {
    if points.len() < 2 {
        return Err(CurveError::TooFewPoints);
    }
    for (input, output) in points {
        if !(1..=127).contains(output) {
            return Err(CurveError::OutOfRange {
                input: *input,
                output: *output,
            });
        }
    }
    for window in points.windows(2) {
        if window[0].0 >= window[1].0 {
            return Err(CurveError::NotIncreasing);
        }
    }
    // Clamp outside the entered range to the nearest endpoint.
    if raw <= points[0].0 {
        return Ok(i32::from(points[0].1));
    }
    if raw >= points[points.len() - 1].0 {
        return Ok(i32::from(points[points.len() - 1].1));
    }
    for window in points.windows(2) {
        let (x0, y0) = window[0];
        let (x1, y1) = window[1];
        if raw >= x0 && raw <= x1 {
            let span = i32::from(x1) - i32::from(x0);
            let offset = i32::from(raw) - i32::from(x0);
            let rise = i32::from(y1) - i32::from(y0);
            return Ok(i32::from(y0) + (rise * offset) / span);
        }
    }
    Ok(i32::from(points[points.len() - 1].1))
}

/// Curve plus per-pad sensitivity, shared by every MIDI trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiVelocityConfig {
    /// Response curve.
    pub curve: VelocityCurve,
    /// Per-pad offset applied after the curve, in `-64..=63`.
    #[serde(default = "no_sensitivity")]
    pub pad_sensitivity: [i8; PAD_COUNT],
}

const fn no_sensitivity() -> [i8; PAD_COUNT] {
    [0; PAD_COUNT]
}

impl Default for MidiVelocityConfig {
    fn default() -> Self {
        Self {
            curve: VelocityCurve::Linear,
            pad_sensitivity: no_sensitivity(),
        }
    }
}

impl MidiVelocityConfig {
    /// Curve with no pad offsets.
    #[must_use]
    pub fn new(curve: VelocityCurve) -> Self {
        Self {
            curve,
            pad_sensitivity: no_sensitivity(),
        }
    }

    /// Build the lookup table.
    pub fn compile(&self) -> Result<VelocityCurveTable, CurveError> {
        VelocityCurveTable::new(&self.curve)
    }

    /// Map a trigger velocity for `pad` through the curve and sensitivity.
    pub fn map(&self, table: &VelocityCurveTable, velocity: Velocity, pad: PadId) -> Velocity {
        let shaped = i32::from(table.map(velocity).get());
        let offset = i32::from(self.pad_sensitivity[pad.index()]);
        Velocity::clamp_signed(shaped + offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::PadIdExt;

    fn table(curve: VelocityCurve) -> VelocityCurveTable {
        VelocityCurveTable::new(&curve).unwrap()
    }

    #[test]
    fn linear_preserves_the_input() {
        let table = table(VelocityCurve::Linear);
        for raw in 1..=127 {
            assert_eq!(table.map_raw(raw).get(), raw);
        }
        assert_eq!(table.map_raw(0), MIN_VELOCITY);
    }

    #[test]
    fn soft_and_hard_curves_are_monotonic_with_expected_endpoints() {
        for curve in [VelocityCurve::Soft, VelocityCurve::Hard] {
            let table = table(curve);
            assert_eq!(table.map_raw(1).get(), 1);
            assert_eq!(table.map_raw(127).get(), 127);
            let mut previous = 0;
            for raw in 1..=127 {
                let value = table.map_raw(raw).get();
                assert!(value >= previous, "{raw}: {value} < {previous}");
                previous = value;
            }
        }
    }

    #[test]
    fn soft_lifts_and_hard_reins_in_mid_velocities() {
        assert!(table(VelocityCurve::Soft).map_raw(64).get() > 64);
        assert!(table(VelocityCurve::Hard).map_raw(64).get() < 64);
    }

    #[test]
    fn fixed_ignores_the_input() {
        let table = table(VelocityCurve::Fixed(Velocity::new(42).unwrap()));
        for raw in 0..=127 {
            assert_eq!(table.map_raw(raw).get(), 42);
        }
    }

    #[test]
    fn custom_curves_interpolate_and_clamp() {
        let curve = VelocityCurve::Custom(vec![(1, 10), (64, 80), (127, 127)]);
        let table = table(curve);
        assert_eq!(table.map_raw(1).get(), 10);
        assert_eq!(table.map_raw(64).get(), 80);
        assert_eq!(table.map_raw(127).get(), 127);
        let midpoint = table.map_raw(32).get();
        assert!((40..=50).contains(&midpoint), "midpoint was {midpoint}");
        assert_eq!(table.map_raw(0).get(), 10);
    }

    #[test]
    fn invalid_custom_curves_are_rejected() {
        assert_eq!(
            VelocityCurveTable::new(&VelocityCurve::Custom(vec![(1, 10)])),
            Err(CurveError::TooFewPoints)
        );
        assert_eq!(
            VelocityCurveTable::new(&VelocityCurve::Custom(vec![(10, 20), (10, 30)])),
            Err(CurveError::NotIncreasing)
        );
        assert!(matches!(
            VelocityCurveTable::new(&VelocityCurve::Custom(vec![(1, 0), (127, 127)])),
            Err(CurveError::OutOfRange { .. })
        ));
    }

    #[test]
    fn per_pad_sensitivity_applies_after_the_curve() {
        let mut config = MidiVelocityConfig::new(VelocityCurve::Linear);
        config.pad_sensitivity[0] = 10;
        config.pad_sensitivity[1] = -10;
        config.pad_sensitivity[2] = 100; // serialized value is not clamped here
        let table = config.compile().unwrap();
        let value = Velocity::new(100).unwrap();
        assert_eq!(config.map(&table, value, PadId::from_index(0)).get(), 110);
        assert_eq!(config.map(&table, value, PadId::from_index(1)).get(), 90);
        assert_eq!(config.map(&table, value, PadId::from_index(2)).get(), 127);
    }

    #[test]
    fn preview_table_matches_mapping() {
        let table = table(VelocityCurve::Soft);
        for raw in 0..=127_usize {
            assert_eq!(table.table()[raw], table.map_raw(raw as u8).get());
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let mut config = MidiVelocityConfig::new(VelocityCurve::Custom(vec![(1, 5), (127, 120)]));
        config.pad_sensitivity[3] = -7;
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<MidiVelocityConfig>(&json).unwrap(),
            config
        );
    }
}
