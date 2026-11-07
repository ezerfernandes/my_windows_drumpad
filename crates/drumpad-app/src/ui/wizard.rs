//! First-run setup wizard view.

use egui::{Align2, RichText, Ui, vec2};

use crate::app::actions::Action;
use crate::state::AppState;
use crate::state::keymap::KeyboardProfile;
use crate::state::prefs::{BUFFER_SIZES, SAMPLE_RATES};
use crate::state::wizard::WizardStep;

use super::UiLocal;
use super::theme::Theme;

/// Draw the wizard when it is active.
pub fn show(
    ctx: &egui::Context,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    if !state.wizard.is_active() {
        return;
    }

    let step = state.wizard.step();
    egui::Window::new("Setup")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .min_width(520.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(step.title())
                        .color(theme.text)
                        .size(theme.h2)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Step {} of {}",
                            WizardStep::ORDER
                                .iter()
                                .position(|candidate| *candidate == step)
                                .unwrap_or(0)
                                + 1,
                            WizardStep::ORDER.len()
                        ))
                        .color(theme.text_muted)
                        .size(theme.small),
                    );
                });
            });
            ui.label(
                RichText::new(step.description())
                    .color(theme.text_muted)
                    .size(theme.body),
            );
            ui.add_space(theme.md);
            ui.separator();
            ui.add_space(theme.md);

            match step {
                WizardStep::Welcome => welcome(ui, theme),
                WizardStep::Keyboard => keyboard(ui, theme, state, actions),
                WizardStep::Audio => audio(ui, theme, state),
                WizardStep::Midi => midi(ui, theme, state),
                WizardStep::SampleFolder => sample_folder(ui, theme, state, local),
                WizardStep::Done => done(ui, theme, state),
            }

            if state.wizard.was_skipped(step) {
                ui.add_space(theme.sm);
                ui.label(
                    RichText::new("You skipped this step earlier. It stays available in Settings.")
                        .color(theme.warning)
                        .size(theme.small),
                );
            }

            ui.add_space(theme.md);
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        step != WizardStep::Welcome,
                        egui::Button::new("Back").min_size(vec2(80.0, 32.0)),
                    )
                    .clicked()
                {
                    actions.push(Action::WizardBack);
                }
                if step.skippable()
                    && ui
                        .add(egui::Button::new("Skip").min_size(vec2(80.0, 32.0)))
                        .clicked()
                {
                    actions.push(Action::WizardSkip);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if step == WizardStep::Done {
                        "Finish"
                    } else {
                        "Next"
                    };
                    let button = egui::Button::new(RichText::new(label).color(theme.bg))
                        .fill(theme.accent)
                        .min_size(vec2(120.0, 34.0));
                    if ui.add(button).clicked() {
                        actions.push(if step == WizardStep::Done {
                            Action::WizardFinish
                        } else {
                            Action::WizardNext
                        });
                    }
                    if state.wizard.is_reopened() && ui.button("Close").clicked() {
                        actions.push(Action::WizardClose);
                    }
                });
            });
        });
}

fn welcome(ui: &mut Ui, theme: &Theme) {
    for (index, step) in WizardStep::ORDER.iter().enumerate() {
        if matches!(step, WizardStep::Welcome | WizardStep::Done) {
            continue;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{index}."))
                    .color(theme.accent)
                    .size(theme.body),
            );
            ui.label(
                RichText::new(step.title())
                    .color(theme.text)
                    .size(theme.body),
            );
            ui.label(
                RichText::new("can be skipped")
                    .color(theme.text_disabled)
                    .size(theme.small),
            );
        });
    }
}

fn keyboard(ui: &mut Ui, theme: &Theme, state: &mut AppState, actions: &mut Vec<Action>) {
    let current = state.settings.draft().keyboard.profile;
    for profile in KeyboardProfile::builtins() {
        let selected = current == profile.id;
        let response = ui.add_sized(
            [ui.available_width(), 46.0],
            egui::Button::selectable(
                selected,
                RichText::new(format!("{}  -  {}", profile.name, profile.kind.label()))
                    .color(if selected { theme.accent } else { theme.text })
                    .size(theme.body),
            ),
        );
        if response.clicked() {
            actions.push(Action::SetProfile(profile.id));
        }
    }
    ui.add_space(theme.sm);
    ui.label(
        RichText::new("Pads: 1 2 3 4 / Q W E R / A S D F / Z X C V. Transport: F1-F5.")
            .color(theme.text_muted)
            .size(theme.small),
    );
}

fn audio(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    egui::Grid::new("wizard-audio")
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
                        .desired_width(300.0),
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
            egui::ComboBox::from_id_salt("wizard-rate")
                .selected_text(format!("{rate} Hz"))
                .show_ui(ui, |ui| {
                    for candidate in SAMPLE_RATES {
                        ui.selectable_value(rate, candidate, format!("{candidate} Hz"));
                    }
                });
            ui.end_row();

            ui.label("Buffer");
            let buffer = &mut state.settings.draft_mut().audio.buffer_frames;
            egui::ComboBox::from_id_salt("wizard-buffer")
                .selected_text(format!("{buffer} frames"))
                .show_ui(ui, |ui| {
                    for candidate in BUFFER_SIZES {
                        ui.selectable_value(buffer, candidate, format!("{candidate} frames"));
                    }
                });
            ui.end_row();
        });
    ui.label(
        RichText::new("48 kHz and 128 frames is the reference configuration.")
            .color(theme.text_muted)
            .size(theme.small),
    );
}

fn midi(ui: &mut Ui, theme: &Theme, state: &mut AppState) {
    let device = &mut state.settings.draft_mut().midi.device;
    let mut text = device.clone().unwrap_or_default();
    if ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .hint_text("MIDI port name, for example Nitro Max")
                .desired_width(360.0),
        )
        .changed()
    {
        *device = if text.trim().is_empty() {
            None
        } else {
            Some(text)
        };
    }
    ui.label(
        RichText::new("Skip if you only use the keyboard today.")
            .color(theme.text_muted)
            .size(theme.small),
    );
}

fn sample_folder(ui: &mut Ui, theme: &Theme, state: &mut AppState, local: &mut UiLocal) {
    if local.wizard_folder.is_empty() {
        local.wizard_folder = state
            .settings
            .draft()
            .sample_folder
            .as_ref()
            .map(|folder| folder.display().to_string())
            .unwrap_or_default();
    }
    ui.add(
        egui::TextEdit::singleline(&mut local.wizard_folder)
            .hint_text("D:\\Samples")
            .desired_width(400.0),
    );
    ui.horizontal(|ui| {
        if ui.button("Use this folder").clicked() {
            let folder = local.wizard_folder.trim().to_owned();
            state.settings.draft_mut().sample_folder = if folder.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(folder))
            };
        }
        ui.label(
            RichText::new("You can also drag files onto pads at any time.")
                .color(theme.text_muted)
                .size(theme.small),
        );
    });
}

fn done(ui: &mut Ui, theme: &Theme, state: &AppState) {
    for line in state.wizard.summary(state.settings.draft()) {
        ui.label(RichText::new(line).color(theme.text).size(theme.body));
    }
    ui.add_space(theme.sm);
    ui.label(
        RichText::new("Every choice can be changed in Settings.")
            .color(theme.text_muted)
            .size(theme.small),
    );
}

/// The wizard's window id, used by the smoke test to find it.
pub const WIZARD_WINDOW_ID: &str = "Setup";
