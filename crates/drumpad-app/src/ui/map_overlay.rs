//! Keyboard map overlay.
//!
//! Shows the full chart for the active profile: the 4x4 pad block, the F1-F5
//! transport row, and the deliberately unmapped keys. Keys held on the physical
//! keyboard light up, which makes the overlay usable as a teaching aid on stage.

use egui::{Align2, Key, RichText, Ui, vec2};

use crate::app::actions::Action;
use crate::state::keymap::{ChartSection, KeyAction, TransportAction};
use crate::state::AppState;

use super::theme::Theme;

/// Draw the overlay.
pub fn show(ctx: &egui::Context, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    if !state.performance.overlay_visible() {
        return;
    }

    let mut open = true;
    egui::Window::new("Keyboard map")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(state.profile().name.clone())
                    .color(theme.text)
                    .size(theme.h2)
                    .strong(),
            );
            ui.label(
                RichText::new("Click a pad key to play it. Esc or F5 closes this map.")
                    .color(theme.text_muted)
                    .size(theme.small),
            );
            ui.add_space(theme.sm);

            for section in state.profile().chart() {
                section_ui(ui, theme, state, actions, &section);
                ui.add_space(theme.sm);
            }
        });

    if !open {
        actions.push(Action::Transport(TransportAction::MapOverlay));
    }
}

fn section_ui(
    ui: &mut Ui,
    theme: &Theme,
    state: &AppState,
    actions: &mut Vec<Action>,
    section: &ChartSection,
) {
    ui.label(
        RichText::new(&section.title)
            .color(theme.accent)
            .size(theme.small)
            .strong(),
    );
    for row in &section.rows {
        ui.horizontal(|ui| {
            for key in &row.keys {
                let down = state.router.is_down(key.key);
                let mut text = RichText::new(&key.label).size(theme.h2).color(if down {
                    theme.bg
                } else {
                    theme.text
                });
                if down {
                    text = text.strong();
                }
                let mut button = egui::Button::new(text)
                    .min_size(vec2(theme.xl * 1.9, theme.xl * 1.7))
                    .fill(if down { theme.focus } else { theme.panel_alt });
                if let KeyAction::Pad(pad) = key.action {
                    let pad_name = state.pads.get(pad).name.clone();
                    button = button.selected(state.pads.selected() == pad);
                    let response = ui.add(button);
                    if response.clicked() {
                        actions.push(Action::TriggerPad {
                            pad,
                            velocity: 100,
                        });
                    }
                    response.on_hover_text(format!("Pad {} - {pad_name}", pad.number()));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("{}", pad.number()))
                                .color(theme.text_muted)
                                .size(theme.small),
                        );
                    });
                } else {
                    let caption = match key.action {
                        KeyAction::Transport(action) => action.label(),
                        KeyAction::Unmapped(_) => "unmapped",
                        _ => "",
                    };
                    let response = ui.add(button);
                    response.on_hover_text(caption);
                }
            }
        });
    }
}

/// Whether the escape key should close the overlay before leaving performance.
pub fn escape_closes(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(Key::Escape))
}
