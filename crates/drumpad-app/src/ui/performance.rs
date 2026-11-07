//! Performance mode: large pads, no editing chrome, and the F1-F5 transport row.

use egui::{RichText, Ui, vec2};

use crate::app::actions::Action;
use crate::state::AppState;

use super::pad_grid;
use super::theme::Theme;
use super::UiLocal;

/// Draw the performance surface.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    let status = state.status();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&status.kit)
                .color(theme.text)
                .size(theme.h1)
                .strong(),
        );
        ui.add_space(theme.md);
        chip(
            ui,
            theme,
            &format!("Pad {}", status.pad),
            if status.dirty { theme.warning } else { theme.text_muted },
        );
        chip(
            ui,
            theme,
            if state.performance.metronome() {
                "Metronome ON"
            } else {
                "Metronome off"
            },
            if state.performance.metronome() {
                theme.success
            } else {
                theme.text_muted
            },
        );
        chip(ui, theme, &status.profile, theme.text_muted);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Exit (Esc)")
                .on_hover_text("Return to the previous screen")
                .clicked()
            {
                actions.push(Action::ExitPerformance);
            }
            if ui.button("Key map (F5)").clicked() {
                actions.push(Action::Transport(
                    crate::state::keymap::TransportAction::MapOverlay,
                ));
            }
        });
    });

    ui.add_space(theme.sm);
    let transport_height = theme.xl * 1.9;
    let grid_height = (ui.available_height() - transport_height - theme.sm).max(80.0);
    ui.allocate_ui(vec2(ui.available_width(), grid_height), |ui| {
        pad_grid::show(ui, theme, state, local, actions, true);
    });
    ui.add_space(theme.sm);
    pad_grid::transport_row(ui, theme, state, actions);
    local.last_performance_height = grid_height;
}

/// Draw the always-visible hint strip under the pads in Edit mode.
pub fn edit_hints(ui: &mut Ui, theme: &Theme, state: &AppState) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "{} | {} | {}",
                state.profile().name,
                state.status().kit,
                state.status().load
            ))
            .color(theme.text_muted)
            .size(theme.small),
        );
        ui.label(
            RichText::new("F1 panic  F2 metronome  F3/F4 kits  F5 map  F11 performance")
                .color(theme.text_disabled)
                .size(theme.small),
        );
    });
}

fn chip(ui: &mut Ui, theme: &Theme, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(theme.panel_alt)
        .corner_radius(theme.chip_radius)
        .inner_margin(egui::Margin::symmetric(theme.sm as i8, theme.xs as i8))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(theme.small));
        });
}
