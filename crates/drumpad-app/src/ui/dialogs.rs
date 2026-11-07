//! Dialogs, toasts, and the drag-and-drop confirmation sheet.

use egui::{Align2, RichText, vec2};

use crate::app::actions::Action;
use crate::state::AppState;
use crate::state::notifications::{Dialog, DialogAction, Severity};
use crate::state::pad::PadId;

use super::theme::Theme;

/// Draw the drop preview, the blocking dialog, and the toast stack.
pub fn show(ctx: &egui::Context, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    drop_preview(ctx, theme, state, actions);
    dialog(ctx, theme, state, actions);
    toasts(ctx, theme, state);
}

fn drop_preview(ctx: &egui::Context, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    let Some(preview) = state.dragdrop.preview() else {
        return;
    };
    egui::Window::new("Assign dropped files")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(preview.summary.message())
                    .color(theme.text)
                    .size(theme.body),
            );
            ui.add_space(theme.xs);
            for (pad, path) in &preview.targets {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Pad {}", pad.number()))
                            .color(theme.accent)
                            .size(theme.small),
                    );
                    ui.label(
                        RichText::new(
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string()),
                        )
                        .color(theme.text_muted)
                        .size(theme.small),
                    );
                });
            }
            if !preview.summary.unsupported.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} unsupported file(s) will be skipped.",
                        preview.summary.unsupported.len()
                    ))
                    .color(theme.warning)
                    .size(theme.small),
                );
            }
            ui.add_space(theme.sm);
            ui.horizontal(|ui| {
                let confirm = egui::Button::new(RichText::new("Assign").color(theme.bg))
                    .fill(theme.accent)
                    .min_size(vec2(96.0, 32.0));
                if ui.add(confirm).clicked() {
                    actions.push(Action::ConfirmDrop);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CancelDrop);
                }
                if state.dragdrop.can_undo() && ui.button("Undo last drop").clicked() {
                    actions.push(Action::UndoDrop);
                }
            });
        });
}

fn dialog(ctx: &egui::Context, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    let Some(dialog) = state.notifications.dialog() else {
        return;
    };
    let color = severity_color(theme, dialog.severity);
    egui::Window::new(dialog.title.clone())
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .min_width(440.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(&dialog.message)
                    .color(theme.text)
                    .size(theme.body),
            );
            ui.add_space(theme.md);
            ui.separator();
            ui.add_space(theme.sm);
            ui.horizontal(|ui| {
                for action in &dialog.actions {
                    let is_primary = std::ptr::eq(action, dialog.primary());
                    let button =
                        egui::Button::new(RichText::new(&action.label).color(if is_primary {
                            theme.bg
                        } else {
                            theme.text
                        }))
                        .fill(if is_primary { color } else { theme.panel_alt })
                        .min_size(vec2(130.0, 34.0));
                    if ui.add(button).clicked() {
                        actions.push(Action::ChooseDialog(action.intent));
                    }
                }
                if dialog.dismissible
                    && ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.button("Close").clicked()
                        })
                        .inner
                {
                    actions.push(Action::DismissDialog);
                }
            });
        });
}

fn toasts(ctx: &egui::Context, theme: &Theme, state: &AppState) {
    if state.notifications.toasts().is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("drumpad-toasts"))
        .anchor(Align2::RIGHT_TOP, vec2(-16.0, 64.0))
        .interactable(false)
        .show(ctx, |ui| {
            for toast in state.notifications.toasts() {
                let color = severity_color(theme, toast.severity);
                egui::Frame::new()
                    .fill(theme.panel)
                    .stroke(egui::Stroke::new(1.5_f32, color))
                    .corner_radius(theme.radius)
                    .inner_margin(egui::Margin::same(theme.sm as i8))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(severity_label(toast.severity))
                                    .color(color)
                                    .size(theme.small)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(&toast.message)
                                    .color(theme.text)
                                    .size(theme.small),
                            );
                        });
                    });
                ui.add_space(theme.xs);
            }
        });
}

fn severity_color(theme: &Theme, severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Info => theme.accent,
        Severity::Success => theme.success,
        Severity::Warning => theme.warning,
        Severity::Error => theme.error,
    }
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "Info",
        Severity::Success => "OK",
        Severity::Warning => "Warning",
        Severity::Error => "Error",
    }
}

/// Buttons for the unsaved-changes prompt, in the order the UI draws them.
pub fn unsaved_actions() -> [DialogAction; 3] {
    [
        DialogAction::new(crate::state::notifications::DialogIntent::SaveAndContinue),
        DialogAction::new(crate::state::notifications::DialogIntent::DiscardChanges),
        DialogAction::new(crate::state::notifications::DialogIntent::Cancel),
    ]
}

/// Human label for a dialog that was built by [`super::super::app::actions`].
pub fn describe(dialog: &Dialog) -> String {
    format!(
        "{}: {} ({} action(s))",
        dialog.title,
        dialog.message,
        dialog.actions.len()
    )
}

/// The pad under the pointer for a drop, when the grid has been drawn this
/// frame. Returns `None` when the pointer is outside the grid.
pub fn drop_target(pointer: Option<egui::Pos2>, cells: &[egui::Rect]) -> Option<PadId> {
    let pointer = pointer?;
    cells
        .iter()
        .position(|cell| cell.contains(pointer))
        .and_then(PadId::try_new)
}
