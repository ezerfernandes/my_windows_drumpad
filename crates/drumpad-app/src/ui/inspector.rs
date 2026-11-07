//! Pad inspector view.

use egui::{RichText, Ui};

use crate::app::actions::Action;
use crate::state::AppState;
use crate::state::inspector::{MAX_LAYERS, MAX_VOLUME_DB, MIN_VOLUME_DB, PlaybackMode};
use crate::state::keymap::key_label;
use crate::state::pad::PadId;

use super::UiLocal;
use super::theme::Theme;

/// Draw the inspector for the selected pad.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    let pad = state.inspector.pad();
    ui.horizontal(|ui| {
        ui.heading(RichText::new(format!("Pad {}", pad.number())).color(theme.text));
        ui.label(
            RichText::new(state.pads.get(pad).name.clone())
                .color(theme.text_muted)
                .size(theme.body),
        );
    });
    ui.add_space(theme.xs);

    if !state.inspector.is_dirty() {
        ui.label(
            RichText::new("Editing")
                .color(theme.text_muted)
                .size(theme.small),
        );
    } else {
        ui.label(
            RichText::new("Unsaved changes")
                .color(theme.warning)
                .size(theme.small),
        );
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            sample_slot(ui, theme, state, actions);
            ui.separator();
            layers(ui, theme, state);
            ui.separator();
            playback(ui, theme, state);
            ui.separator();
            key_and_midi(ui, theme, state, local);
            ui.separator();
            audition(ui, theme, state, local, actions);
        });

    ui.add_space(theme.sm);
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply").color(theme.bg))
            .fill(theme.accent)
            .min_size(egui::vec2(96.0, 34.0));
        if ui.add_enabled(state.inspector.is_dirty(), apply).clicked() {
            actions.push(Action::CommitInspector);
        }
        if ui
            .add_enabled(
                state.inspector.is_dirty(),
                egui::Button::new("Revert").min_size(egui::vec2(80.0, 34.0)),
            )
            .clicked()
        {
            actions.push(Action::RevertInspector);
        }
        if ui
            .button("Reset")
            .on_hover_text("Clear every setting on this pad")
            .clicked()
        {
            actions.push(Action::ResetInspector);
        }
    });

    if let Some(error) = state.inspector.last_error() {
        ui.add_space(theme.xs);
        ui.label(RichText::new(error.to_string()).color(theme.error));
    }
}

fn sample_slot(ui: &mut Ui, theme: &Theme, state: &mut AppState, actions: &mut Vec<Action>) {
    ui.label(section_title(theme, "Sample slot"));
    let sample = state.inspector.draft().sample.clone();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(sample.unwrap_or_else(|| "No sample assigned".to_owned()))
                .color(theme.text_muted)
                .size(theme.small),
        );
        if ui.small_button("Clear").clicked() {
            let draft = state.inspector.draft_mut();
            draft.sample = None;
            draft.layers.clear();
        }
    });
    ui.label(
        RichText::new("Drag a file onto the pad grid to replace it.")
            .color(theme.text_disabled)
            .size(theme.small),
    );
    if let Some(editor) = state.editor.as_ref() {
        ui.label(
            RichText::new(format!(
                "Editing {} - {} edit(s) in history",
                editor.source_path().display(),
                if editor.can_undo() { "some" } else { "no" }
            ))
            .color(theme.text_disabled)
            .size(theme.small),
        );
    }
    let _ = actions;
}

fn layers(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.label(section_title(theme, "Velocity layers"));
        if ui
            .small_button("+ Add")
            .on_hover_text("Add a velocity layer")
            .clicked()
            && state.inspector.draft().layers.len() < MAX_LAYERS
        {
            let sample = state.inspector.draft().sample.clone();
            state
                .inspector
                .draft_mut()
                .layers
                .push(crate::state::inspector::LayerSpec::full_range(sample));
        }
    });

    let mut remove: Option<usize> = None;
    let count = state.inspector.draft().layers.len();
    for index in 0..count {
        ui.horizontal(|ui| {
            let draft = state.inspector.draft_mut();
            let layer = &mut draft.layers[index];
            ui.label(
                RichText::new(format!("L{}", index + 1))
                    .color(theme.text_muted)
                    .size(theme.small),
            );
            ui.add(
                egui::DragValue::new(&mut layer.velocity_min)
                    .range(1..=127)
                    .speed(1.0)
                    .prefix("v "),
            );
            ui.label("..");
            ui.add(
                egui::DragValue::new(&mut layer.velocity_max)
                    .range(1..=127)
                    .speed(1.0),
            );
            ui.add(
                egui::DragValue::new(&mut layer.gain_db)
                    .range(MIN_VOLUME_DB..=MAX_VOLUME_DB)
                    .speed(0.1)
                    .suffix(" dB")
                    .max_decimals(1),
            );
            if ui.small_button("x").on_hover_text("Remove layer").clicked() {
                remove = Some(index);
            }
        });
    }
    if count == 0 {
        ui.label(
            RichText::new("No layers yet. Add one to make the pad audible.")
                .color(theme.text_disabled)
                .size(theme.small),
        );
    }
    if let Some(index) = remove {
        state.inspector.draft_mut().layers.remove(index);
    }
}

fn playback(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.label(section_title(theme, "Playback"));
    egui::Grid::new("pad-playback")
        .num_columns(2)
        .spacing([theme.md, theme.xs])
        .show(ui, |ui| {
            let draft = state.inspector.draft_mut();

            ui.label("Mode");
            egui::ComboBox::from_id_salt("pad-mode")
                .selected_text(draft.mode.label())
                .show_ui(ui, |ui| {
                    for mode in PlaybackMode::ALL {
                        ui.selectable_value(&mut draft.mode, mode, mode.label());
                    }
                });
            ui.end_row();

            ui.label("Volume");
            ui.add(
                egui::Slider::new(&mut draft.volume_db, MIN_VOLUME_DB..=MAX_VOLUME_DB)
                    .suffix(" dB")
                    .max_decimals(1),
            );
            ui.end_row();

            ui.label("Pan");
            ui.add(
                egui::Slider::new(&mut draft.pan, -1.0..=1.0)
                    .show_value(true)
                    .max_decimals(2),
            );
            ui.end_row();

            ui.label("Tune");
            ui.add(
                egui::Slider::new(&mut draft.tune_semitones, -24.0..=24.0)
                    .suffix(" st")
                    .max_decimals(1),
            );
            ui.end_row();

            ui.label("Choke group");
            ui.horizontal(|ui| {
                let mut enabled = draft.choke_group.is_some();
                if ui.checkbox(&mut enabled, "").changed() {
                    draft.choke_group = if enabled { Some(1) } else { None };
                }
                if let Some(group) = &mut draft.choke_group {
                    ui.add(egui::DragValue::new(group).range(1..=8).speed(1.0));
                } else {
                    ui.label(
                        RichText::new("none")
                            .color(theme.text_disabled)
                            .size(theme.small),
                    );
                }
            });
            ui.end_row();
        });
}

fn key_and_midi(ui: &mut Ui, theme: &Theme, state: &mut AppState, local: &mut UiLocal) {
    ui.label(section_title(theme, "Mapping"));
    egui::Grid::new("pad-mapping")
        .num_columns(2)
        .spacing([theme.md, theme.xs])
        .show(ui, |ui| {
            ui.label("Keyboard key");
            ui.horizontal(|ui| {
                let label = state
                    .inspector
                    .draft()
                    .key
                    .map(key_label)
                    .unwrap_or_else(|| "Unmapped".to_owned());
                ui.label(RichText::new(label).color(theme.text));
                if ui.small_button("Rebind").clicked() {
                    local.capturing_key_for = Some(state.inspector.pad());
                }
                if state.inspector.draft().key.is_some() && ui.small_button("Clear").clicked() {
                    state.inspector.draft_mut().key = None;
                }
            });
            ui.end_row();

            ui.label("MIDI note");
            ui.horizontal(|ui| {
                let draft = state.inspector.draft_mut();
                let mut enabled = draft.midi_note.is_some();
                if ui.checkbox(&mut enabled, "").changed() {
                    draft.midi_note = if enabled { Some(38) } else { None };
                }
                if let Some(note) = &mut draft.midi_note {
                    ui.add(egui::DragValue::new(note).range(0..=127).speed(1.0));
                }
            });
            ui.end_row();
        });

    if local.capturing_key_for == Some(state.inspector.pad()) {
        ui.label(
            RichText::new("Press the new key now. Esc cancels.")
                .color(theme.focus)
                .size(theme.small),
        );
    }
}

fn audition(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    ui.label(section_title(theme, "Audition"));
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut local.audition_velocity, 1..=127).text("velocity"));
        if ui
            .button("Audition")
            .on_hover_text("Play this pad with its current settings")
            .clicked()
        {
            actions.push(Action::PreviewPad {
                pad: state.inspector.pad(),
                velocity: local.audition_velocity,
            });
        }
    });
    if let Some(preview) = state.preview_trigger(state.inspector.pad(), local.audition_velocity) {
        ui.label(
            RichText::new(format!(
                "gain {:+.1} dB, tune {:+.1} st, sample {}",
                preview.gain_db,
                preview.tune_semitones,
                preview.sample.as_deref().unwrap_or("<none>")
            ))
            .color(theme.text_muted)
            .size(theme.small),
        );
    } else {
        ui.label(
            RichText::new("No layer covers this velocity.")
                .color(theme.warning)
                .size(theme.small),
        );
    }
}

/// Select a pad and describe it for the inspector header.
pub fn selected_pad(state: &AppState) -> PadId {
    state.inspector.pad()
}

fn section_title(theme: &Theme, text: &str) -> RichText {
    RichText::new(text)
        .color(theme.accent)
        .size(theme.small)
        .strong()
}
