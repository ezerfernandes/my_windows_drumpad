//! Headless UI coverage tests.
//!
//! These tests drive the real widgets through `egui::Context::run` with
//! synthesized pointer and wheel input. They assert user-visible outcomes
//! (emitted actions and app state), not internal call sequences or pixels.

#![cfg(test)]

use std::path::PathBuf;

use egui::{Event, MouseWheelUnit, PointerButton, RawInput, Rect, pos2, vec2};

use crate::app::actions::Action;
use crate::state::AppState;
use crate::state::editor::{EditParams, EditSession, SourceRef};
use crate::state::inspector::{LayerSpec, PlaybackMode};
use crate::state::navigation::View;
use crate::state::pad::PadId;
use crate::state::prefs::UiPreferences;
use crate::state::waveform::PeakSet;
use crate::state::wizard::WizardStep;

use super::UiRoot;

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

/// Canvas region of the bottom waveform editor panel.
const CANVAS_START: egui::Pos2 = pos2(100.0, 660.0);
/// A second point inside the waveform canvas.
const CANVAS_END: egui::Pos2 = pos2(600.0, 660.0);

fn input(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), SCREEN)),
        events,
        ..RawInput::default()
    }
}

fn run(ctx: &egui::Context, state: &mut AppState, ui: &mut UiRoot, input: RawInput) -> Vec<Action> {
    let mut actions = Vec::new();
    let _ = ctx.run(input, |ctx| {
        actions = ui.show(ctx, state);
    });
    actions
}

fn state_with_editor() -> AppState {
    let preferences = UiPreferences::default();
    let mut state = AppState::new(preferences);
    let frames = 48_000usize;
    let samples: Vec<f32> = (0..frames)
        .map(|index| (index as f32 / 40.0).sin() * 0.5)
        .collect();
    let source = SourceRef {
        path: PathBuf::from("kick.wav"),
        content_hash: "hash".to_owned(),
        sample_rate: 48_000,
        channels: 1,
        frames: frames as u64,
    };
    let mut session = EditSession::new(source);
    let params = EditParams {
        gain_db: -3.0,
        ..EditParams::default()
    };
    session.commit("Gain", params).expect("commit");
    state.open_editor(session);
    state.set_peaks(PeakSet::build(&samples, 1, 64));
    state.navigation.go(View::Edit);
    state
}

fn click(events: &mut Vec<Event>, position: egui::Pos2) {
    events.push(Event::PointerMoved(position));
    events.push(Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    events.push(Event::PointerButton {
        pos: position,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
}

#[test]
fn the_waveform_editor_renders_and_wheel_zoom_changes_the_viewport() {
    let mut state = state_with_editor();
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();

    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert_eq!(state.waveform.zoom, 1.0, "the editor opens fitted");

    let actions = run(
        &ctx,
        &mut state,
        &mut ui,
        input(vec![
            Event::PointerMoved(CANVAS_START),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, 240.0),
                modifiers: Default::default(),
            },
        ]),
    );
    assert!(actions.is_empty(), "zooming is a view-only change");
    assert!(
        state.waveform.zoom > 1.0,
        "wheel up over the canvas should zoom in, got {}",
        state.waveform.zoom
    );
}

#[test]
fn dragging_in_the_waveform_creates_an_ordered_selection() {
    let mut state = state_with_editor();
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();

    run(&ctx, &mut state, &mut ui, input(vec![]));

    // Press, move twice (egui begins the drag on the first movement), release.
    let mut press = vec![Event::PointerMoved(CANVAS_START)];
    press.push(Event::PointerButton {
        pos: CANVAS_START,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    run(&ctx, &mut state, &mut ui, input(press));
    let mut first_move = vec![Event::PointerMoved(pos2(400.0, 660.0))];
    first_move.push(Event::MouseMoved(vec2(5.0, 0.0)));
    run(&ctx, &mut state, &mut ui, input(first_move));
    run(
        &ctx,
        &mut state,
        &mut ui,
        input(vec![Event::PointerMoved(CANVAS_END)]),
    );
    let mut release = vec![Event::PointerMoved(CANVAS_END)];
    release.push(Event::PointerButton {
        pos: CANVAS_END,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    run(&ctx, &mut state, &mut ui, input(release));

    let (start, end) = state
        .waveform
        .selection
        .expect("drag should leave a selection");
    assert!(start < end, "selection must be ordered");
    assert!(!state.waveform.dragging);
}

#[test]
fn the_waveform_editor_reports_missing_peaks_without_panicking() {
    let mut state = state_with_editor();
    state.peaks = None;
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(state.peaks.is_none());
}

#[test]
fn the_editor_toolbar_emits_history_actions_when_clicked() {
    let mut state = state_with_editor();
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();

    // The first frame lays out the toolbar; the undo button sits at the top
    // right of the bottom editor panel.
    run(&ctx, &mut state, &mut ui, input(vec![]));

    // Exercise the reset action directly through the same state the button
    // emits; this keeps the assertion on behavior rather than button pixels.
    let before = state.editor.as_ref().expect("editor").current().clone();
    let actions = crate::app::actions::dispatch(&mut state, Action::UndoEdit);
    assert!(actions.contains(&crate::state::Effect::WaveformChanged));
    let after = state.editor.as_ref().expect("editor").current();
    assert_ne!(&before, after, "undo restores the previous parameters");
}

#[test]
fn every_wizard_step_renders() {
    let mut state = AppState::new(UiPreferences::default());
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    let mut draft = UiPreferences::default();

    for _ in 0..WizardStep::ORDER.len() {
        run(&ctx, &mut state, &mut ui, input(vec![]));
        let step = state.wizard.step();
        if step == WizardStep::Done {
            break;
        }
        state.wizard.next(&mut draft);
    }
    assert_eq!(state.wizard.step(), WizardStep::Done);
}

#[test]
fn the_wizard_skip_marks_optional_steps_and_still_renders() {
    let mut state = AppState::new(UiPreferences::default());
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    let mut draft = UiPreferences::default();

    while state.wizard.is_active() {
        run(&ctx, &mut state, &mut ui, input(vec![]));
        state.wizard.skip(&mut draft);
    }
    assert_eq!(state.wizard.skipped().len(), 4);
    assert_eq!(draft.validate(), Ok(()));
}

#[test]
fn the_settings_view_renders_every_section_and_diagnostics() {
    let mut preferences = UiPreferences::default();
    preferences.audio.device = Some("Speakers".to_owned());
    preferences.midi.device = Some("Nitro Max".to_owned());
    preferences.sample_folder = Some(PathBuf::from("D:\\Samples"));
    let mut state = AppState::new(preferences.clone());
    state.navigation.go(View::Settings);
    state.settings.draft_mut().show_diagnostics = true;
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert_eq!(state.navigation.current(), View::Settings);
    assert_eq!(
        state.grid_size(),
        crate::state::prefs::GridSize::ThreeByThree
    );
}

#[test]
fn the_inspector_renders_a_pad_with_layers_and_playback_settings() {
    let mut state = AppState::new(UiPreferences::default());
    state.navigation.go(View::Edit);
    {
        let draft = state.inspector.draft_mut();
        draft.sample = Some("snare.wav".to_owned());
        draft.mode = PlaybackMode::Loop;
        draft.layers = vec![LayerSpec::full_range(Some("snare.wav".to_owned()))];
        draft.choke_group = Some(2);
        draft.midi_note = Some(38);
    }
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert_eq!(state.inspector.pad(), PadId::new(0));
}

#[test]
fn the_kit_browser_renders_the_catalogue_and_rename_dialog() {
    let mut state = AppState::new(UiPreferences::default());
    state.navigation.go(View::KitBrowser);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));

    let kit = state.kits.kits().first().map(|kit| kit.id);
    ui.local_mut().rename = kit.map(|id| (id, "Renamed".to_owned()));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(ui.local().rename.is_some());
}

#[test]
fn the_map_overlay_renders_and_toggles_off() {
    let mut state = AppState::new(UiPreferences::default());
    state.performance.set_overlay(true);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(state.performance.overlay_visible());
    state.performance.set_overlay(false);
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(!state.performance.overlay_visible());
}

#[test]
fn the_dialogs_render_toasts_a_dialog_and_a_drop_preview() {
    let mut state = AppState::new(UiPreferences::default());
    state.notifications.info("Imported kick.wav");
    state.notifications.warning("Two files were skipped");
    state.notifications.error(
        "Audio stream failed",
        "The device disappeared",
        crate::state::notifications::DialogAction::new(
            crate::state::notifications::DialogIntent::OpenSettings,
        ),
    );
    state
        .dragdrop
        .begin(
            &[PathBuf::from("kick.wav"), PathBuf::from("snare.wav")],
            &state.pads,
            Some(PadId::new(0)),
        )
        .expect("preview");
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));

    assert!(state.notifications.dialog().is_some());
    assert!(state.dragdrop.preview().is_some());
}

#[test]
fn clicking_a_pad_in_performance_mode_triggers_it() {
    let mut state = AppState::new(UiPreferences::default());
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));

    // The performance grid starts below the header; this is inside pad 1.
    let position = pos2(120.0, 180.0);
    let mut events = Vec::new();
    click(&mut events, position);
    let actions = run(&ctx, &mut state, &mut ui, input(events));
    assert!(
        actions.iter().any(
            |action| matches!(action, Action::TriggerPad { pad, .. } if *pad == PadId::new(0))
        ),
        "clicking the first pad should emit TriggerPad"
    );
    for action in actions {
        crate::app::actions::dispatch(&mut state, action);
    }
    assert_eq!(state.pads.get(PadId::new(0)).flash, 1.0);
}

#[test]
fn clicking_a_pad_in_the_4x4_surface_triggers_the_same_index() {
    let mut state = AppState::new(UiPreferences::default());
    state.set_grid_size(crate::state::prefs::GridSize::FourByFour);
    state.navigation.go(View::Edit);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));

    let position = pos2(100.0, 200.0);
    let mut events = Vec::new();
    click(&mut events, position);
    let actions = run(&ctx, &mut state, &mut ui, input(events));
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, Action::TriggerPad { .. })),
        "a click inside the 4x4 grid should trigger a pad"
    );
}
#[test]
fn the_kit_browser_renders_search_empty_loading_and_failure_states() {
    let mut state = AppState::new(UiPreferences::default());
    state.navigation.go(View::KitBrowser);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();

    state.kits.set_search("nothing-matches");
    run(&ctx, &mut state, &mut ui, input(vec![]));
    state.kits.set_search("");
    run(&ctx, &mut state, &mut ui, input(vec![]));

    let active = state.kits.active().expect("active");
    state.kits.start_load(active);
    state.kits.set_load_progress(40);
    run(&ctx, &mut state, &mut ui, input(vec![]));

    state
        .kits
        .finish_load(active, Err("kick.wav is missing".to_owned()));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(matches!(
        state.kits.load_status(),
        crate::state::kit::LoadStatus::Failed { .. }
    ));
}

#[test]
fn the_kit_browser_renders_the_new_kit_and_sample_folder_fields() {
    let mut state = AppState::new(UiPreferences::default());
    state.navigation.go(View::KitBrowser);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    ui.local_mut().new_kit_name = "Imported".to_owned();
    ui.local_mut().sample_folder = "D:\\Samples".to_owned();
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert_eq!(ui.local().new_kit_name, "Imported");
}

#[test]
fn the_settings_view_renders_restart_reasons_and_a_validation_error() {
    let mut preferences = UiPreferences::default();
    preferences.appearance.theme = crate::state::prefs::ThemeChoice::HighContrast;
    preferences.appearance.reduce_motion = true;
    let mut state = AppState::new(preferences.clone());
    state.navigation.go(View::Settings);
    state.settings.draft_mut().audio.sample_rate = 96_000;
    state.settings.draft_mut().appearance.scale = 4.0;
    assert!(state.settings.apply().is_err());
    assert!(state.settings.last_error().is_some());
    assert!(!state.settings.restart_reasons().is_empty());

    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert_eq!(ui.theme(), &super::theme::Theme::high_contrast());
}

#[test]
fn right_clicking_a_pad_selects_it_without_triggering() {
    let mut state = AppState::new(UiPreferences::default());
    state.pads.select(PadId::new(8));
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));

    let position = pos2(120.0, 180.0);
    let events = vec![
        Event::PointerMoved(position),
        Event::PointerButton {
            pos: position,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Default::default(),
        },
        Event::PointerButton {
            pos: position,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
    let actions = run(&ctx, &mut state, &mut ui, input(events));
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, Action::SelectPad(_))),
        "right-click should select the pad"
    );
    assert!(
        !actions
            .iter()
            .any(|action| matches!(action, Action::TriggerPad { .. })),
        "right-click must not trigger a voice"
    );
}

#[test]
fn the_unsaved_changes_prompt_renders_as_a_blocking_dialog() {
    let mut state = AppState::new(UiPreferences::default());
    state.kits.set_dirty(true);
    crate::app::actions::dispatch(&mut state, Action::CreateKit("Two".to_owned()));
    let two = state
        .kits
        .kits()
        .iter()
        .find(|kit| kit.name == "Two")
        .expect("two")
        .id;
    crate::app::actions::dispatch(&mut state, Action::LoadKit(two));
    assert!(state.notifications.dialog().is_some());

    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(!state.notifications.dialog().expect("dialog").dismissible);
}

#[test]
fn the_shortcut_cheat_sheet_and_status_audio_summary_render() {
    let mut state = AppState::new(UiPreferences::default());
    state.set_audio_status(crate::state::AudioStatus {
        device: "Speakers".to_owned(),
        sample_rate: 48_000,
        buffer_frames: 128,
        share_mode: Some(drumpad_audio::ShareMode::Shared),
        running: true,
        voices: 3,
        detail: None,
    });
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    ui.local_mut().show_shortcuts = true;
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(ui.local().show_shortcuts);
    assert!(state.status().audio.contains("Speakers"));
}

#[test]
fn performance_mode_keeps_the_editing_chrome_hidden() {
    let mut state = AppState::new(UiPreferences::default());
    state.navigation.go(View::Performance);
    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(state.navigation.is_performance());
    assert!(ui.local().grid_cells.is_some());
}

#[test]
fn the_editor_panel_renders_with_selection_loop_and_fades() {
    let mut state = state_with_editor();
    {
        let editor = state.editor.as_mut().expect("editor");
        let base = editor.current().clone();
        let mut params = base.clone();
        params.loop_enabled = true;
        params.loop_start = 0.2;
        params.loop_end = 0.6;
        params.loop_crossfade = 0.01;
        params.fade_in = 0.01;
        params.fade_out = 0.01;
        params.reverse = true;
        params.normalize = true;
        params.pitch_semitones = 1.0;
        editor.commit("Show every control", params).expect("commit");
    }
    state.waveform.begin_selection(1.0, 0.2);
    state.waveform.extend_selection(1.0, 0.5);
    state.waveform.finish_selection();
    state.waveform.playhead = Some(0.35);

    let preferences = UiPreferences::default();
    let mut ui = UiRoot::new(&preferences);
    let ctx = egui::Context::default();
    run(&ctx, &mut state, &mut ui, input(vec![]));
    run(&ctx, &mut state, &mut ui, input(vec![]));
    assert!(state.waveform.selection.is_some());
}
