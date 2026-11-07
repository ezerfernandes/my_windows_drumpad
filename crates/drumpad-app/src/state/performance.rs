//! Performance-mode state: the F1-F5 transport row, metronome, panic, and the
//! keyboard map overlay.
//!
//! Performance mode is a presentation mode, not a second app: it shares pads,
//! kits, and the engine with the editor. It hides editing chrome and refuses
//! edits, which is what "no accidental edits are possible" means for the UI.

use std::time::Duration;

use super::keymap::TransportAction;

/// How long a transport button stays lit after being triggered.
pub const TRANSPORT_FLASH_DECAY: Duration = Duration::from_millis(220);

/// What the shell should do after a transport action fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportEffect {
    /// Stop all voices.
    Panic,
    /// Metronome is now on (`true`) or off.
    Metronome(bool),
    /// Load the previous kit.
    PreviousKit,
    /// Load the next kit.
    NextKit,
    /// Map overlay is now visible (`true`) or hidden.
    Overlay(bool),
}

/// Performance-mode state.
#[derive(Debug, Clone, Default)]
pub struct Performance {
    metronome: bool,
    overlay: bool,
    panic_requests: u64,
    last: Option<TransportAction>,
    flash: [f32; 5],
}

impl Performance {
    /// Fresh performance state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the metronome is on.
    pub fn metronome(&self) -> bool {
        self.metronome
    }

    /// Whether the map overlay is visible.
    pub fn overlay_visible(&self) -> bool {
        self.overlay
    }

    /// How many panic requests have been issued this session.
    pub fn panic_requests(&self) -> u64 {
        self.panic_requests
    }

    /// The last transport action.
    pub fn last_action(&self) -> Option<TransportAction> {
        self.last
    }

    /// Flash intensity for the transport button at `index` (`0..5`).
    pub fn flash(&self, action: TransportAction) -> f32 {
        self.flash[index_of(action)]
    }

    /// Trigger a transport action and return its effect.
    pub fn trigger(&mut self, action: TransportAction) -> TransportEffect {
        self.last = Some(action);
        self.flash[index_of(action)] = 1.0;
        match action {
            TransportAction::Panic => {
                self.panic_requests += 1;
                TransportEffect::Panic
            }
            TransportAction::Metronome => {
                self.metronome = !self.metronome;
                TransportEffect::Metronome(self.metronome)
            }
            TransportAction::PreviousKit => TransportEffect::PreviousKit,
            TransportAction::NextKit => TransportEffect::NextKit,
            TransportAction::MapOverlay => {
                self.overlay = !self.overlay;
                TransportEffect::Overlay(self.overlay)
            }
        }
    }

    /// Set the overlay explicitly (for example from the top bar or Escape).
    pub fn set_overlay(&mut self, visible: bool) {
        self.overlay = visible;
    }

    /// Turn the metronome off (panic and shutdown both do this).
    pub fn silence_metronome(&mut self) {
        self.metronome = false;
    }

    /// Advance button flashes.
    pub fn tick(&mut self, dt: Duration) {
        let step = (dt.as_secs_f32() / TRANSPORT_FLASH_DECAY.as_secs_f32()).clamp(0.0, 1.0);
        for flash in &mut self.flash {
            *flash = (*flash - step).max(0.0);
        }
    }

    /// Whether any button is animating.
    pub fn is_animating(&self) -> bool {
        self.flash.iter().any(|flash| *flash > 0.0)
    }
}

const fn index_of(action: TransportAction) -> usize {
    match action {
        TransportAction::Panic => 0,
        TransportAction::Metronome => 1,
        TransportAction::PreviousKit => 2,
        TransportAction::NextKit => 3,
        TransportAction::MapOverlay => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_counts_requests_and_does_not_touch_the_metronome() {
        let mut performance = Performance::new();
        assert_eq!(
            performance.trigger(TransportAction::Panic),
            TransportEffect::Panic
        );
        assert_eq!(performance.panic_requests(), 1);
        assert!(!performance.metronome());
    }

    #[test]
    fn f2_toggles_the_metronome() {
        let mut performance = Performance::new();
        assert_eq!(
            performance.trigger(TransportAction::Metronome),
            TransportEffect::Metronome(true)
        );
        assert!(performance.metronome());
        assert_eq!(
            performance.trigger(TransportAction::Metronome),
            TransportEffect::Metronome(false)
        );
        assert!(!performance.metronome());
    }

    #[test]
    fn f3_and_f4_ask_for_kit_navigation() {
        let mut performance = Performance::new();
        assert_eq!(
            performance.trigger(TransportAction::PreviousKit),
            TransportEffect::PreviousKit
        );
        assert_eq!(
            performance.trigger(TransportAction::NextKit),
            TransportEffect::NextKit
        );
        assert_eq!(performance.last_action(), Some(TransportAction::NextKit));
    }

    #[test]
    fn f5_toggles_the_map_overlay() {
        let mut performance = Performance::new();
        assert_eq!(
            performance.trigger(TransportAction::MapOverlay),
            TransportEffect::Overlay(true)
        );
        assert!(performance.overlay_visible());
        assert_eq!(
            performance.trigger(TransportAction::MapOverlay),
            TransportEffect::Overlay(false)
        );
        assert!(!performance.overlay_visible());
    }

    #[test]
    fn transport_buttons_flash_and_decay_independently() {
        let mut performance = Performance::new();
        performance.trigger(TransportAction::Panic);
        assert_eq!(performance.flash(TransportAction::Panic), 1.0);
        assert_eq!(performance.flash(TransportAction::Metronome), 0.0);
        assert!(performance.is_animating());

        performance.tick(TRANSPORT_FLASH_DECAY);
        assert_eq!(performance.flash(TransportAction::Panic), 0.0);
        assert!(!performance.is_animating());
    }

    #[test]
    fn every_action_maps_to_a_distinct_flash_slot() {
        let mut performance = Performance::new();
        for (index, action) in TransportAction::ALL.into_iter().enumerate() {
            performance.trigger(action);
            let lit: Vec<usize> = TransportAction::ALL
                .iter()
                .enumerate()
                .filter(|(_, action)| performance.flash(**action) > 0.0)
                .map(|(index, _)| index)
                .collect();
            assert!(lit.contains(&index));
        }
    }
}
