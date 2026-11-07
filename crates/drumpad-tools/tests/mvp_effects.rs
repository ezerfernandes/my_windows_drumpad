//! Headless MVP effect tests for the app's intent layer.
//!
//! The orchestrator's MVP smoke task asks for a test that builds the real
//! [`AppState`], dispatches [`Action`]s, and asserts the [`Effect`]s the
//! composition layer receives. No window, GPU, audio device, or MIDI port is
//! involved: `dispatch` is the pure intent → effect boundary.
//!
//! This lives in `crates/drumpad-tools/tests/` because the QA pair owns that
//! path. It consumes the app's public `dispatch` API; it does not edit the app
//! crate.

use drumpad_app::app::{Action, dispatch};
use drumpad_app::state::AppState;
use drumpad_app::state::Effect;
use drumpad_app::state::keymap::TransportAction;
use drumpad_app::state::pad::PadId;
use drumpad_app::state::prefs::UiPreferences;

fn state() -> AppState {
    AppState::new(UiPreferences::default())
}

#[test]
fn trigger_pad_emits_trigger_effect_and_flashes_the_pad() {
    let mut state = state();
    let pad = PadId::new(3);
    let effects = dispatch(&mut state, Action::TriggerPad { pad, velocity: 96 });
    assert_eq!(effects, vec![Effect::Trigger { pad, velocity: 96 }]);
    assert!(state.pads.get(pad).flash > 0.0, "pad did not flash");
    assert_eq!(state.pads.selected(), pad, "trigger must select the pad");
}

#[test]
fn release_pad_emits_release_effect() {
    let mut state = state();
    let pad = PadId::new(2);
    let effects = dispatch(&mut state, Action::ReleasePad(pad));
    assert_eq!(effects, vec![Effect::Release { pad }]);
}

#[test]
fn panic_emits_panic_and_silences_the_metronome() {
    let mut state = state();
    // Turn the metronome on first so panic has something to silence.
    let on = dispatch(&mut state, Action::Transport(TransportAction::Metronome));
    assert_eq!(on, vec![Effect::Metronome(true)]);
    let panic = dispatch(&mut state, Action::Transport(TransportAction::Panic));
    assert_eq!(panic, vec![Effect::Panic, Effect::Metronome(false)]);
    assert!(
        !state.performance.metronome(),
        "panic left the metronome on"
    );
    assert_eq!(state.performance.panic_requests(), 1);
}

#[test]
fn f5_toggles_the_keyboard_map_overlay() {
    let mut state = state();
    assert!(!state.performance.overlay_visible());
    let effects = dispatch(&mut state, Action::Transport(TransportAction::MapOverlay));
    assert!(effects.is_empty(), "overlay is a UI-only state change");
    assert!(
        state.performance.overlay_visible(),
        "F5 did not open the overlay"
    );
    dispatch(&mut state, Action::Transport(TransportAction::MapOverlay));
    assert!(
        !state.performance.overlay_visible(),
        "F5 did not close the overlay"
    );
}

#[test]
fn all_sixteen_pads_emit_trigger_and_release_effects() {
    let mut state = state();
    for (index, pad) in PadId::ALL.into_iter().enumerate() {
        let velocity = (index as u8 + 1).min(127);
        let trigger = dispatch(&mut state, Action::TriggerPad { pad, velocity });
        assert_eq!(
            trigger,
            vec![Effect::Trigger { pad, velocity }],
            "pad {} did not emit a trigger",
            pad.number()
        );
        let release = dispatch(&mut state, Action::ReleasePad(pad));
        assert_eq!(
            release,
            vec![Effect::Release { pad }],
            "pad {} did not emit a release",
            pad.number()
        );
    }
}

#[test]
fn status_line_shows_the_active_kit_and_pad() {
    let mut state = state();
    state.pads.select(PadId::new(7));
    let status = state.status();
    assert!(
        !status.kit.trim().is_empty(),
        "status line has no kit name; MVP checklist requires one"
    );
    assert_eq!(status.pad, 8, "status line must show the selected pad");
    assert!(
        !status.profile.trim().is_empty(),
        "status line has no keyboard profile"
    );
}

#[test]
fn default_audio_preferences_are_the_reference_format() {
    let preferences = UiPreferences::default();
    assert_eq!(preferences.audio.sample_rate, 48_000);
    assert_eq!(preferences.audio.buffer_frames, 128);
    assert!(
        !preferences.audio.exclusive,
        "v1 starts in WASAPI shared mode"
    );
}
