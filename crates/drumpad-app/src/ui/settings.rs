//! Settings view: audio, MIDI, keyboard, appearance, and diagnostics.
//!
//! Every field writes into the settings draft. Apply validates, persists, and
//! explains restart requirements; an invalid combination is refused with the
//! reason shown next to the button.

use egui::{RichText, Ui};

use crate::app::actions::Action;
use crate::state::AppState;
use crate::state::keymap::KeyboardProfile;
use crate::state::prefs::{BUFFER_SIZES, SAMPLE_RATES, ThemeChoice, UiPreferences};

use super::UiLocal;
use super::theme::Theme;

/// Draw the settings screen.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    _local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    ui.heading(RichText::new("Settings").color(theme.text));
    ui.add_space(theme.sm);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            audio(ui, theme, state);
            ui.separator();
            midi(ui, theme, state);
            ui.separator();
            keyboard(ui, theme, state, actions);
            ui.separator();
            library(ui, theme, state);
            ui.separator();
            appearance(ui, theme, state);
            ui.separator();
            advanced(ui, theme, state, actions);
        });

    ui.add_space(theme.sm);
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply").color(theme.bg))
            .fill(theme.accent)
            .min_size(egui::vec2(96.0, 34.0));
        if ui.add_enabled(state.settings.is_dirty(), apply).clicked() {
            actions.push(Action::ApplySettings);
        }
        if ui
            .add_enabled(
                state.settings.is_dirty(),
                egui::Button::new("Revert").min_size(egui::vec2(80.0, 34.0)),
            )
            .clicked()
        {
            actions.push(Action::RevertSettings);
        }
    });

    if let Some(error) = state.settings.last_error() {
        ui.label(RichText::new(error.to_string()).color(theme.error));
    }
    let reasons = state.settings.restart_reasons();
    for reason in reasons {
        ui.label(
            RichText::new(format!("Restart required: {}", reason.explanation()))
                .color(theme.warning)
                .size(theme.small),
        );
    }
}

fn audio(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.label(section_title(theme, "Audio output"));
    egui::Grid::new("settings-audio")
        .num_columns(2)
        .spacing([theme.md, theme.sm])
        .show(ui, |ui| {
            ui.label("Device");
            let device = &mut state.settings.draft_mut().audio.device;
            let mut text = device.clone().unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("System default")
                        .desired_width(320.0),
                )
                .changed()
            {
                *device = if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                };
            }
            ui.end_row();

            ui.label("Sample rate");
            let rate = &mut state.settings.draft_mut().audio.sample_rate;
            egui::ComboBox::from_id_salt("sample-rate")
                .selected_text(format!("{rate} Hz"))
                .show_ui(ui, |ui| {
                    for candidate in SAMPLE_RATES {
                        ui.selectable_value(rate, candidate, format!("{candidate} Hz"));
                    }
                });
            ui.end_row();

            ui.label("Buffer");
            let buffer = &mut state.settings.draft_mut().audio.buffer_frames;
            egui::ComboBox::from_id_salt("buffer-size")
                .selected_text(format!("{buffer} frames"))
                .show_ui(ui, |ui| {
                    for candidate in BUFFER_SIZES {
                        ui.selectable_value(buffer, candidate, format!("{candidate} frames"));
                    }
                });
            ui.end_row();

            ui.label("Mode");
            ui.add_enabled(
                false,
                egui::Checkbox::new(
                    &mut state.settings.draft_mut().audio.exclusive,
                    "WASAPI exclusive",
                ),
            )
            .on_disabled_hover_text(
                "Exclusive mode is not implemented in this build. Shared mode is used.",
            );
            ui.end_row();
        });
}

fn midi(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.label(section_title(theme, "MIDI input"));
    egui::Grid::new("settings-midi")
        .num_columns(2)
        .spacing([theme.md, theme.sm])
        .show(ui, |ui| {
            ui.label("Device");
            let device = &mut state.settings.draft_mut().midi.device;
            let mut text = device.clone().unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("No MIDI device")
                        .desired_width(320.0),
                )
                .changed()
            {
                *device = if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                };
            }
            ui.end_row();

            ui.label("Channel");
            let channel = &mut state.settings.draft_mut().midi.channel;
            egui::ComboBox::from_id_salt("midi-channel")
                .selected_text(if *channel == 0 {
                    "Omni".to_owned()
                } else {
                    format!("Channel {channel}")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(channel, 0, "Omni");
                    for candidate in 1..=16 {
                        ui.selectable_value(channel, candidate, format!("Channel {candidate}"));
                    }
                });
            ui.end_row();
        });
}

fn keyboard(ui: &mut Ui, theme: &Theme, state: &mut AppState, actions: &mut Vec<Action>) {
    ui.label(section_title(theme, "Keyboard"));
    let current = state.settings.draft().keyboard.profile;
    let name = KeyboardProfile::builtin(current)
        .map(|profile| profile.name)
        .unwrap_or_else(|| "Full keyboard".to_owned());
    ui.horizontal(|ui| {
        ui.label("Profile");
        egui::ComboBox::from_id_salt("keyboard-profile")
            .selected_text(name)
            .show_ui(ui, |ui| {
                for profile in KeyboardProfile::builtins() {
                    if ui
                        .selectable_label(current == profile.id, profile.name)
                        .clicked()
                    {
                        actions.push(Action::SetProfile(profile.id));
                    }
                }
            });
        if ui.button("Show map").clicked() {
            state.performance.set_overlay(true);
        }
    });
    ui.label(
        RichText::new("The Finding 35-key keypad is the reference one-hand layout.")
            .color(theme.text_muted)
            .size(theme.small),
    );

    let current_grid = state.grid_size();
    ui.horizontal(|ui| {
        ui.label("Pad grid");
        egui::ComboBox::from_id_salt("grid-size")
            .selected_text(current_grid.label())
            .show_ui(ui, |ui| {
                for candidate in crate::state::prefs::GridSize::ALL {
                    let selected = current_grid == candidate;
                    if ui.selectable_label(selected, candidate.label()).clicked() && !selected {
                        actions.push(Action::SetGridSize(candidate));
                    }
                }
            });
    });
    ui.label(
        RichText::new(
            "3x3 uses 1 2 3 / Q W E / A S D for the first nine pads; the kit keeps all 16.",
        )
        .color(theme.text_disabled)
        .size(theme.small),
    );

    let show_labels = &mut state.settings.draft_mut().keyboard.show_key_labels;
    ui.checkbox(show_labels, "Show key labels on pads");
}

fn library(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.label(section_title(theme, "Sample library"));
    ui.horizontal(|ui| {
        ui.label("Sample folder");
        let folder = &mut state.settings.draft_mut().sample_folder;
        let mut text = folder
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text("D:\\Samples")
                    .desired_width(360.0),
            )
            .changed()
        {
            *folder = if text.trim().is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(text))
            };
        }
    });
    ui.label(
        RichText::new("Used by the Kits view when creating a kit from a folder. Apply to persist.")
            .color(theme.text_disabled)
            .size(theme.small),
    );
}

fn appearance(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    ui.label(section_title(theme, "Appearance"));
    egui::Grid::new("settings-appearance")
        .num_columns(2)
        .spacing([theme.md, theme.sm])
        .show(ui, |ui| {
            ui.label("Theme");
            let choice = &mut state.settings.draft_mut().appearance.theme;
            egui::ComboBox::from_id_salt("theme-choice")
                .selected_text(choice.label())
                .show_ui(ui, |ui| {
                    for candidate in ThemeChoice::ALL {
                        ui.selectable_value(choice, candidate, candidate.label());
                    }
                });
            ui.end_row();

            ui.label("UI scale");
            ui.add(
                egui::Slider::new(&mut state.settings.draft_mut().appearance.scale, 0.75..=2.0)
                    .suffix("x")
                    .max_decimals(2),
            );
            ui.end_row();
        });
    ui.checkbox(
        &mut state.settings.draft_mut().appearance.show_midi_hints,
        "Show MIDI note hints on pads",
    );
    ui.checkbox(
        &mut state.settings.draft_mut().appearance.reduce_motion,
        "Reduce hit animation",
    );
}

fn advanced(ui: &mut Ui, theme: &Theme, state: &mut AppState, actions: &mut Vec<Action>) {
    ui.label(section_title(theme, "Advanced and diagnostics"));
    ui.checkbox(
        &mut state.settings.draft_mut().show_diagnostics,
        "Show diagnostics",
    );
    ui.horizontal(|ui| {
        if ui.button("Run first-run setup again").clicked() {
            actions.push(Action::WizardReopen);
        }
        if ui.button("Open log folder").clicked() {
            actions.push(Action::ReportError {
                title: "Logs".to_owned(),
                message: "The log folder contains local-only diagnostics.".to_owned(),
            });
        }
    });

    if state.settings.draft().show_diagnostics {
        ui.add_space(theme.xs);
        egui::Grid::new("diagnostics")
            .num_columns(2)
            .spacing([theme.md, theme.xs])
            .show(ui, |ui| {
                let status = state.status();
                ui.label("Kit");
                ui.label(status.kit);
                ui.end_row();
                ui.label("Load");
                ui.label(status.load);
                ui.end_row();
                ui.label("Keyboard");
                ui.label(status.profile);
                ui.end_row();
                ui.label("Frame");
                ui.label(state.frame.to_string());
                ui.end_row();
                ui.label("Version");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
                ui.label("Audio backend");
                ui.label("Shared-mode backend owned by E02");
                ui.end_row();
            });
    }
}

/// The persisted preferences the screen is editing, for the shell's title bar.
pub fn draft(preferences: &UiPreferences) -> String {
    format!(
        "{} Hz, {} frames",
        preferences.audio.sample_rate, preferences.audio.buffer_frames
    )
}

fn section_title(theme: &Theme, text: &str) -> RichText {
    RichText::new(text)
        .color(theme.accent)
        .size(theme.small)
        .strong()
}
