//! Pad identity and live pad state for the performance surface.
//!
//! The music-domain [`Kit`](drumpad_model::kit) owns what a pad *is*; this
//! module owns what the UI needs to draw and animate it. Keeping the two apart
//! means the pad grid can be built and tested before the kit persistence slice
//! lands, and the composition layer can project a loaded kit into [`Pads`] with
//! a single adapter function.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Number of pads on the performance surface.
pub const PAD_COUNT: usize = 16;

/// Pads per row in the 4x4 grid.
pub const PAD_COLUMNS: usize = 4;

/// Pads per column in the 4x4 grid.
pub const PAD_ROWS: usize = 4;

/// Stable 0-based pad index, always `< [`PAD_COUNT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PadId(u8);

impl PadId {
    /// All 16 pads, row-major (pad 1 is `PadId::new(0)`, top-left).
    pub const ALL: [Self; PAD_COUNT] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
        Self(7),
        Self(8),
        Self(9),
        Self(10),
        Self(11),
        Self(12),
        Self(13),
        Self(14),
        Self(15),
    ];

    /// Panics outside `0..PAD_COUNT`; use [`PadId::try_new`] for untrusted input.
    pub const fn new(index: usize) -> Self {
        assert!(index < PAD_COUNT, "pad index out of range");
        Self(index as u8)
    }

    /// Fallible constructor for indices that come from user input or files.
    pub const fn try_new(index: usize) -> Option<Self> {
        if index < PAD_COUNT {
            Some(Self(index as u8))
        } else {
            None
        }
    }

    /// Zero-based index into the 4x4 grid.
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// One-based pad number as shown on stage (`1..=16`).
    pub const fn number(self) -> u8 {
        self.0 + 1
    }

    /// Grid row, `0` at the top.
    pub const fn row(self) -> usize {
        self.index() / PAD_COLUMNS
    }

    /// Grid column, `0` at the left.
    pub const fn column(self) -> usize {
        self.index() % PAD_COLUMNS
    }

    /// Row-major neighbour for keyboard grid navigation.
    pub fn step(self, d_row: isize, d_col: isize) -> Option<Self> {
        self.step_in(PAD_COLUMNS, PAD_ROWS, d_row, d_col)
    }

    /// Row-major neighbour on a surface with `columns` x `rows` pads.
    pub fn step_in(self, columns: usize, rows: usize, d_row: isize, d_col: isize) -> Option<Self> {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let index = self.index();
        if index >= columns * rows {
            return None;
        }
        let row = (index / columns) as isize + d_row;
        let column = (index % columns) as isize + d_col;
        if (0..rows as isize).contains(&row) && (0..columns as isize).contains(&column) {
            let next = row as usize * columns + column as usize;
            (next < columns * rows).then(|| Self::new(next))
        } else {
            None
        }
    }
}

/// Decay time of the hit flash. One frame of feedback is visible even at 30 fps.
pub const HIT_FLASH_DECAY: Duration = Duration::from_millis(320);

/// Decay time of the velocity ring.
pub const VELOCITY_RING_DECAY: Duration = Duration::from_millis(900);

/// A pad as drawn by the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct PadState {
    /// Stable grid position.
    pub id: PadId,
    /// Short name shown on the pad (for example `Kick`).
    pub name: String,
    /// Sample file name shown under the pad name, when a sample is assigned.
    pub sample: Option<String>,
    /// Key label for the active profile, for example `Q` or `F1`.
    pub key_label: String,
    /// MIDI note hint, for example `N38`.
    pub midi_hint: Option<String>,
    /// Whether the pad is the current inspector selection.
    pub selected: bool,
    /// Whether a sample is assigned. Empty pads are drawn dimmed but still hit.
    pub assigned: bool,
    /// Peak level from the engine meters, `0..=1`.
    pub level: f32,
    /// Decaying flash intensity, `0..=1`. Set to `1.0` on the trigger frame.
    pub flash: f32,
    /// Last trigger velocity, `0..=1`.
    pub velocity: f32,
    /// Decaying velocity-ring intensity, `0..=1`.
    pub velocity_ring: f32,
    /// Colour token index used by the theme, `0..`.
    pub color: u8,
    /// Row on the active surface (not the 16-pad storage grid).
    pub surface_row: u8,
    /// Column on the active surface.
    pub surface_col: u8,
}

impl PadState {
    /// An empty pad with a name placeholder.
    pub fn empty(id: PadId) -> Self {
        Self {
            id,
            name: format!("Pad {}", id.number()),
            sample: None,
            key_label: String::new(),
            midi_hint: None,
            selected: false,
            assigned: false,
            level: 0.0,
            flash: 0.0,
            velocity: 0.0,
            velocity_ring: 0.0,
            color: (id.column() as u8) % 4,
            surface_row: id.row() as u8,
            surface_col: id.column() as u8,
        }
    }

    /// Register a hit. The flash is set immediately so the very next frame
    /// shows feedback; decay happens in [`Pads::tick`].
    pub fn trigger(&mut self, velocity: f32) {
        let velocity = velocity.clamp(0.0, 1.0);
        self.flash = 1.0;
        self.velocity = velocity;
        self.velocity_ring = 1.0;
        self.level = self.level.max(velocity);
    }

    /// Advance the animation clocks by `dt`.
    pub fn tick(&mut self, dt: Duration) {
        self.flash = decay(self.flash, dt, HIT_FLASH_DECAY);
        self.velocity_ring = decay(self.velocity_ring, dt, VELOCITY_RING_DECAY);
        if self.flash <= f32::EPSILON {
            self.velocity = 0.0;
        }
    }

    /// Whether this pad currently needs repainting for an animation.
    pub fn is_animating(&self) -> bool {
        self.flash > 0.0 || self.velocity_ring > 0.0
    }

    /// Apply an engine meter level, with peak-hold behaviour for legibility.
    ///
    /// Non-finite values (a broken meter or an uninitialised voice) collapse to
    /// silence instead of poisoning the layout with a NaN-sized meter.
    pub fn set_level(&mut self, level: f32) {
        self.level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

fn decay(value: f32, dt: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 0.0;
    }
    let step = (dt.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
    (value - step).max(0.0)
}

/// The full performance surface: 16 pads plus selection.
#[derive(Debug, Clone, PartialEq)]
pub struct Pads {
    pads: [PadState; PAD_COUNT],
    selected: PadId,
}

impl Default for Pads {
    fn default() -> Self {
        Self::new()
    }
}

impl Pads {
    /// A fresh grid of empty pads with pad 1 selected.
    pub fn new() -> Self {
        let mut pads = PadId::ALL.map(PadState::empty);
        pads[0].selected = true;
        Self {
            pads,
            selected: PadId::new(0),
        }
    }

    /// All pads in grid order.
    pub fn iter(&self) -> impl Iterator<Item = &PadState> {
        self.pads.iter()
    }

    /// Mutable access to all pads in grid order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut PadState> {
        self.pads.iter_mut()
    }

    /// Pad at `id`.
    pub fn get(&self, id: PadId) -> &PadState {
        &self.pads[id.index()]
    }

    /// Mutable pad at `id`.
    pub fn get_mut(&mut self, id: PadId) -> &mut PadState {
        &mut self.pads[id.index()]
    }

    /// The pad the inspector is editing.
    pub fn selected(&self) -> PadId {
        self.selected
    }

    /// Select a pad. Selection never changes audio state.
    pub fn select(&mut self, id: PadId) {
        self.pads[self.selected.index()].selected = false;
        self.selected = id;
        self.pads[id.index()].selected = true;
    }

    /// Move the selection by grid steps, clamped to the grid.
    pub fn move_selection(&mut self, d_row: isize, d_col: isize) {
        if let Some(next) = self.selected.step(d_row, d_col) {
            self.select(next);
        }
    }

    /// Move the selection on a 9- or 16-pad surface.
    pub fn move_selection_in(
        &mut self,
        grid: crate::state::prefs::GridSize,
        d_row: isize,
        d_col: isize,
    ) {
        if let Some(next) = self
            .selected
            .step_in(grid.columns(), grid.rows(), d_row, d_col)
        {
            self.select(next);
        }
    }

    /// First pad without a sample, used by drag-and-drop batch assignment.
    pub fn first_empty(&self) -> Option<PadId> {
        self.pads.iter().find(|pad| !pad.assigned).map(|pad| pad.id)
    }

    /// Register a hit on one pad.
    pub fn trigger(&mut self, id: PadId, velocity: f32) {
        self.pads[id.index()].trigger(velocity);
    }

    /// Advance every pad animation by `dt`.
    pub fn tick(&mut self, dt: Duration) {
        for pad in &mut self.pads {
            pad.tick(dt);
        }
    }

    /// Whether any pad is mid-animation and the shell should keep repainting.
    pub fn is_animating(&self) -> bool {
        self.pads.iter().any(PadState::is_animating)
    }

    /// Set the key labels from an active profile binding table.
    pub fn set_key_labels(&mut self, labels: &[String; PAD_COUNT]) {
        for (pad, label) in self.pads.iter_mut().zip(labels.iter()) {
            pad.key_label.clone_from(label);
        }
    }

    /// Recompute surface row/column and colour for a 4x4 or 3x3 surface.
    pub fn apply_grid(&mut self, grid: crate::state::prefs::GridSize) {
        let columns = grid.columns().max(1);
        for pad in &mut self.pads {
            let index = pad.id.index();
            pad.surface_row = (index / columns) as u8;
            pad.surface_col = (index % columns) as u8;
            pad.color = pad.surface_col % 4;
        }
    }

    /// Replace a pad's displayed sample name.
    pub fn set_sample(&mut self, id: PadId, sample: Option<String>) {
        let pad = &mut self.pads[id.index()];
        pad.assigned = sample.is_some();
        pad.sample = sample;
    }

    /// Replace a pad's display name.
    pub fn set_name(&mut self, id: PadId, name: impl Into<String>) {
        self.pads[id.index()].name = name.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_ids_are_row_major() {
        assert_eq!(PadId::new(0).row(), 0);
        assert_eq!(PadId::new(3).column(), 3);
        assert_eq!(PadId::new(4).row(), 1);
        assert_eq!(PadId::new(15).number(), 16);
        assert_eq!(PadId::try_new(16), None);
    }

    #[test]
    fn grid_steps_clamp_at_the_edges() {
        let top_left = PadId::new(0);
        assert_eq!(top_left.step(-1, 0), None);
        assert_eq!(top_left.step(0, -1), None);
        assert_eq!(top_left.step(1, 1), Some(PadId::new(5)));
        let bottom_right = PadId::new(15);
        assert_eq!(bottom_right.step(1, 0), None);
        assert_eq!(bottom_right.step(0, 1), None);
        assert_eq!(bottom_right.step(-1, -1), Some(PadId::new(10)));
    }

    #[test]
    fn a_hit_flashes_on_the_trigger_frame_before_any_tick() {
        let mut pads = Pads::new();
        pads.trigger(PadId::new(3), 0.8);
        let pad = pads.get(PadId::new(3));
        assert_eq!(pad.flash, 1.0);
        assert_eq!(pad.velocity, 0.8);
        assert!(pad.is_animating());
    }

    #[test]
    fn flash_and_ring_decay_to_zero() {
        let mut pads = Pads::new();
        pads.trigger(PadId::new(0), 1.0);
        pads.tick(HIT_FLASH_DECAY);
        assert_eq!(pads.get(PadId::new(0)).flash, 0.0);
        assert!(
            pads.is_animating(),
            "the velocity ring outlives the hit flash"
        );

        pads.tick(Duration::from_secs(2));
        let pad = pads.get(PadId::new(0));
        assert_eq!(pad.velocity_ring, 0.0);
        assert_eq!(pad.velocity, 0.0);
        assert!(!pads.is_animating());
    }

    #[test]
    fn velocity_is_clamped() {
        let mut pads = Pads::new();
        pads.trigger(PadId::new(1), 4.0);
        assert_eq!(pads.get(PadId::new(1)).velocity, 1.0);
        pads.trigger(PadId::new(1), -1.0);
        assert_eq!(pads.get(PadId::new(1)).velocity, 0.0);
    }

    #[test]
    fn selection_is_exclusive_and_moves_within_the_grid() {
        let mut pads = Pads::new();
        assert_eq!(pads.selected(), PadId::new(0));
        assert!(pads.get(PadId::new(0)).selected);

        pads.select(PadId::new(9));
        assert_eq!(pads.selected(), PadId::new(9));
        assert!(!pads.get(PadId::new(0)).selected);

        pads.move_selection(0, 2);
        assert_eq!(pads.selected(), PadId::new(11));
        pads.move_selection(8, 0);
        assert_eq!(
            pads.selected(),
            PadId::new(11),
            "moving past an edge is a no-op"
        );
    }

    #[test]
    fn the_first_empty_pad_is_used_for_batch_assignment() {
        let mut pads = Pads::new();
        assert_eq!(pads.first_empty(), Some(PadId::new(0)));
        pads.set_sample(PadId::new(0), Some("kick.wav".to_owned()));
        assert_eq!(pads.first_empty(), Some(PadId::new(1)));
        for id in PadId::ALL {
            pads.set_sample(id, Some("x.wav".to_owned()));
        }
        assert_eq!(pads.first_empty(), None);
    }

    #[test]
    fn key_labels_come_from_the_profile_table() {
        let mut pads = Pads::new();
        let labels = std::array::from_fn(|index| format!("K{index}"));
        pads.set_key_labels(&labels);
        assert_eq!(pads.get(PadId::new(5)).key_label, "K5");
        assert_eq!(pads.get(PadId::new(15)).key_label, "K15");
    }

    #[test]
    fn meter_levels_clamp_to_the_meter_scale() {
        let mut pads = Pads::new();
        pads.get_mut(PadId::new(0)).set_level(1.7);
        assert_eq!(pads.get(PadId::new(0)).level, 1.0);
        pads.get_mut(PadId::new(0)).set_level(f32::NAN);
        assert!(pads.get(PadId::new(0)).level.is_nan().eq(&false));
    }
}
