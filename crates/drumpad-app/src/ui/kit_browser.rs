//! Kit browser view: search, list, CRUD, load progress, and unsaved prompts.

use egui::{RichText, Ui};

use crate::app::actions::Action;
use crate::state::kit::{KitId, LoadStatus};
use crate::state::AppState;

use super::theme::Theme;
use super::UiLocal;

/// Draw the kit browser.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.heading(RichText::new("Kits").color(theme.text));
        ui.add_space(theme.lg);
        ui.add(
            egui::TextEdit::singleline(&mut local.kit_search)
                .hint_text("Search kits")
                .desired_width(240.0),
        );
        if local.kit_search != state.kits.search() {
            actions.push(Action::SetKitSearch(local.kit_search.clone()));
        }
        ui.add_space(theme.sm);
        ui.add(
            egui::TextEdit::singleline(&mut local.new_kit_name)
                .hint_text("New kit name")
                .desired_width(180.0),
        );
        if ui.button("Create").clicked() && !local.new_kit_name.trim().is_empty() {
            actions.push(Action::CreateKit(local.new_kit_name.trim().to_owned()));
            local.new_kit_name.clear();
        }
    });

    ui.add_space(theme.sm);
    load_status(ui, theme, state, actions);
    ui.separator();

    let active = state.kits.active();
    if local.selected_kit.is_none() {
        local.selected_kit = active;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 64.0)
        .show(ui, |ui| {
            let kits = state.kits.visible_kits();
            if kits.is_empty() {
                ui.label(
                    RichText::new("No kits match the search.")
                        .color(theme.text_muted)
                        .size(theme.body),
                );
            }
            for kit in kits {
                let selected = local.selected_kit == Some(kit.id);
                let is_active = active == Some(kit.id);
                let label = format!(
                    "{}{}{}",
                    kit.name,
                    if is_active { "  (active)" } else { "" },
                    if kit.dirty { "  *" } else { "" }
                );
                let response = ui.add_sized(
                    [ui.available_width(), 36.0],
                    egui::Button::selectable(
                        selected,
                        RichText::new(label).size(theme.body).color(if is_active {
                            theme.accent
                        } else {
                            theme.text
                        }),
                    ),
                );
                if response.clicked() {
                    local.selected_kit = Some(kit.id);
                }
                if response.double_clicked() {
                    actions.push(Action::LoadKit(kit.id));
                }
                response.on_hover_text("Double-click to load");
            }
        });

    ui.add_space(theme.sm);
    ui.horizontal(|ui| {
        let selected = local.selected_kit;
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Load"))
            .clicked()
            && let Some(kit) = selected
        {
            actions.push(Action::LoadKit(kit));
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Duplicate"))
            .clicked()
            && let Some(kit) = selected
        {
            actions.push(Action::DuplicateKit(kit));
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Rename"))
            .clicked()
            && let Some(kit) = selected
        {
            let name = state
                .kits
                .get(kit)
                .map(|kit| kit.name.clone())
                .unwrap_or_default();
            local.rename = Some((kit, name));
        }
        if ui
            .add_enabled(selected.is_some(), egui::Button::new("Delete"))
            .clicked()
            && let Some(kit) = selected
        {
            actions.push(Action::DeleteKit(kit));
        }
        ui.separator();
        if ui
            .add_enabled(
                state.kits.active().is_some(),
                egui::Button::new("Save active").fill(theme.accent.gamma_multiply(0.5)),
            )
            .clicked()
        {
            actions.push(Action::SaveActiveKit);
        }
    });

    rename_dialog(ui, theme, local, actions);
}

fn load_status(ui: &mut Ui, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    match state.kits.load_status() {
        LoadStatus::Idle => {
            ui.label(RichText::new("No kit loaded").color(theme.text_muted));
        }
        LoadStatus::Loading { kit, percent } => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Loading {}", kit_label(state, *kit)))
                        .color(theme.accent),
                );
                ui.add(egui::ProgressBar::new(f32::from(*percent) / 100.0).desired_width(240.0));
            });
            ui.label(
                RichText::new("The window stays responsive while samples load.")
                    .color(theme.text_disabled)
                    .size(theme.small),
            );
        }
        LoadStatus::Ready { kit } => {
            ui.label(
                RichText::new(format!("{} is ready", kit_label(state, *kit)))
                    .color(theme.success),
            );
        }
        LoadStatus::Failed { kit, message } => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} failed: {message}", kit_label(state, *kit)))
                        .color(theme.error),
                );
                if ui.button("Retry").clicked() {
                    actions.push(Action::LoadKit(*kit));
                }
            });
        }
    }
}

fn kit_label(state: &AppState, id: KitId) -> String {
    state
        .kits
        .get(id)
        .map_or_else(|| id.to_string(), |kit| kit.name.clone())
}

fn rename_dialog(ui: &mut Ui, theme: &Theme, local: &mut UiLocal, actions: &mut Vec<Action>) {
    let Some((kit, name)) = &mut local.rename else {
        return;
    };
    let mut close = false;
    let mut confirm = false;
    egui::Window::new("Rename kit")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.label(RichText::new("New name").color(theme.text_muted));
            ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("Rename").clicked() {
                    confirm = true;
                }
            });
        });
    if confirm {
        actions.push(Action::RenameKit {
            kit: *kit,
            name: name.trim().to_owned(),
        });
        local.rename = None;
    } else if close {
        local.rename = None;
    }
}
