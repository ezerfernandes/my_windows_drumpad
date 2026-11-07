//! The 4x4 pad grid.
//!
//! The layout is a pure function of the available rectangle so the "all 16 pads
//! fit at 1280x720" promise is a unit test, not a screenshot review. The widget
//! draws the hit flash, velocity ring, level meter, selection ring, sample name,
//! key label, and MIDI hint.

use egui::{Align2, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::actions::Action;
use crate::state::keymap::TransportAction;
use crate::state::pad::{PadId, PAD_COLUMNS, PAD_ROWS};
use crate::state::AppState;

use super::theme::Theme;

/// Geometry of the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    /// One rectangle per pad, row-major.
    pub cells: [Rect; 16],
    /// Size of one pad.
    pub cell: Vec2,
}

/// Lay out a 4x4 grid inside `rect` with `gap` between cells.
pub fn layout(rect: Rect, gap: f32) -> GridLayout {
    let gap = gap.max(0.0);
    let cell_width = ((rect.width() - gap * (PAD_COLUMNS as f32 - 1.0)) / PAD_COLUMNS as f32)
        .max(1.0);
    let cell_height =
        ((rect.height() - gap * (PAD_ROWS as f32 - 1.0)) / PAD_ROWS as f32).max(1.0);

    let mut cells = [Rect::NOTHING; 16];
    for index in 0..16 {
        let row = index / PAD_COLUMNS;
        let column = index % PAD_COLUMNS;
        let min = pos2(
            rect.left() + column as f32 * (cell_width + gap),
            rect.top() + row as f32 * (cell_height + gap),
        );
        cells[index] = Rect::from_min_size(min, vec2(cell_width, cell_height));
    }
    GridLayout {
        cells,
        cell: vec2(cell_width, cell_height),
    }
}

/// The smallest pad edge the design accepts, in logical points.
pub const MIN_PAD_EDGE: f32 = 96.0;

/// Whether a grid with this layout is still comfortable to hit.
pub fn is_usable(layout: &GridLayout) -> bool {
    layout.cell.x >= MIN_PAD_EDGE && layout.cell.y >= MIN_PAD_EDGE * 0.72
}

/// Draw the grid. `large` is used by performance mode.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &AppState,
    local: &mut super::UiLocal,
    actions: &mut Vec<Action>,
    large: bool,
) {
    let available = ui.available_rect_before_wrap();
    let gap = if large { theme.md } else { theme.sm };
    let grid = layout(available, gap);
    local.grid_cells = Some(grid.cells);

    for id in PadId::ALL {
        let pad = state.pads.get(id);
        let rect = grid.cells[id.index()];
        let response = ui.interact(rect, ui.id().with(("pad", id.index())), Sense::click());

        let painter = ui.painter();
        let fill = theme.pad_fill(pad);
        painter.rect_filled(rect, CornerRadius::same(theme.radius), fill);

        // Selection ring or a quiet hairline.
        let (border, width) = theme.pad_border(pad);
        painter.rect_stroke(
            rect.shrink(width * 0.5),
            CornerRadius::same(theme.radius),
            Stroke::new(width, border),
            StrokeKind::Inside,
        );

        // Velocity ring: a second inset ring that fades out.
        if pad.velocity_ring > 0.01 {
            let inset = 5.0 + (1.0 - pad.velocity_ring) * 4.0;
            painter.rect_stroke(
                rect.shrink(inset),
                CornerRadius::same(theme.radius.saturating_sub(2)),
                Stroke::new(2.0_f32,
                    theme
                        .accent
                        .gamma_multiply(pad.velocity_ring * 0.85),
                ),
                StrokeKind::Inside,
            );
        }

        let inner = rect.shrink(if large { theme.lg } else { theme.sm + 2.0 });
        let painter = painter.with_clip_rect(inner.expand(2.0));

        // Pad name.
        let name_size = if large { theme.h1 } else { theme.body };
        painter.text(
            inner.left_top(),
            Align2::LEFT_TOP,
            &pad.name,
            FontId::proportional(name_size),
            theme.text,
        );

        // Sample name.
        let sample = pad.sample.as_deref().unwrap_or("Empty - drop a sample");
        let sample_color = if pad.assigned {
            theme.text_muted
        } else {
            theme.text_disabled
        };
        painter.text(
            inner.left_top() + vec2(0.0, name_size + theme.small * 0.6),
            Align2::LEFT_TOP,
            sample,
            theme.small_font(),
            sample_color,
        );

        // Velocity value, top right.
        if pad.velocity > 0.0 {
            painter.text(
                inner.right_top(),
                Align2::RIGHT_TOP,
                format!("{:.0}", pad.velocity * 127.0),
                theme.mono_font(),
                theme.text_muted,
            );
        }

        // Key chip, bottom left.
        if !pad.key_label.is_empty() {
            let chip_width = (pad.key_label.chars().count() as f32 * theme.small * 0.62
                + theme.sm * 1.2)
                .max(theme.sm * 2.4);
            let chip = Rect::from_min_size(
                pos2(inner.left(), inner.bottom() - theme.small * 1.5),
                vec2(chip_width, theme.small * 1.5),
            );
            painter.rect_filled(
                chip,
                CornerRadius::same(theme.chip_radius),
                theme.bg.gamma_multiply(0.55),
            );
            painter.text(
                chip.center(),
                Align2::CENTER_CENTER,
                &pad.key_label,
                FontId::proportional(theme.small),
                theme.text,
            );
        }

        // MIDI hint, bottom right.
        if let Some(hint) = &pad.midi_hint {
            painter.text(
                inner.right_bottom(),
                Align2::RIGHT_BOTTOM,
                hint,
                theme.mono_font(),
                theme.text_muted,
            );
        }

        // Level meter: a thin vertical bar hugging the right edge.
        if pad.level > 0.001 {
            let bar = Rect::from_min_max(
                pos2(rect.right() - 7.0, rect.top() + theme.sm),
                pos2(rect.right() - 3.0, rect.bottom() - theme.sm),
            );
            painter.rect_filled(
                bar,
                CornerRadius::same(2),
                theme.bg.gamma_multiply(0.6),
            );
            let filled = Rect::from_min_max(
                pos2(bar.left(), bar.bottom() - bar.height() * pad.level),
                bar.right_bottom(),
            );
            let color = if pad.level > 0.95 {
                theme.error
            } else if pad.level > 0.8 {
                theme.warning
            } else {
                theme.success
            };
            painter.rect_filled(filled, CornerRadius::same(2), color);
        }

        if response.clicked() {
            actions.push(Action::TriggerPad {
                pad: id,
                velocity: 100,
            });
        }
        if response.secondary_clicked() {
            actions.push(Action::SelectPad(id));
        }
        response.on_hover_ui(|ui| {
            ui.label(format!("Pad {} - {}", id.number(), pad.name));
            if let Some(sample) = &pad.sample {
                ui.label(format!("Sample: {sample}"));
            }
            ui.label(format!("Key: {}", pad.key_label));
            if let Some(hint) = &pad.midi_hint {
                ui.label(format!("MIDI: {hint}"));
            }
            ui.label("Click to play, right-click to select.");
        });
    }
}

/// Draw the F1-F5 transport row used by performance mode.
pub fn transport_row(
    ui: &mut Ui,
    theme: &Theme,
    state: &AppState,
    actions: &mut Vec<Action>,
) {
    let available = ui.available_width();
    let gap = theme.sm;
    let width = (available - gap * 4.0) / 5.0;
    ui.horizontal(|ui| {
        for (index, action) in TransportAction::ALL.into_iter().enumerate() {
            if index > 0 {
                ui.add_space(gap);
            }
            let active = match action {
                TransportAction::Metronome => state.performance.metronome(),
                TransportAction::MapOverlay => state.performance.overlay_visible(),
                _ => false,
            };
            let flash = state.performance.flash(action);
            let text = format!(
                "F{}\n{}\n{}",
                index + 1,
                action.label(),
                if active { "ON" } else { "" }
            );
            let button = egui::Button::new(
                egui::RichText::new(text.trim_end()).size(theme.body).color(theme.text),
            )
            .min_size(vec2(width, theme.xl * 1.6))
            .fill(if active || flash > 0.0 {
                theme.accent.gamma_multiply(0.25 + flash * 0.4)
            } else {
                theme.panel_alt
            });
            if ui.add(button).clicked() {
                actions.push(Action::Transport(action));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Rect;

    fn screen(width: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height))
    }

    #[test]
    fn sixteen_pads_are_laid_out_row_major_without_overlap() {
        let rect = screen(800.0, 600.0);
        let grid = layout(rect, 8.0);
        assert_eq!(grid.cells[0].left(), rect.left());
        assert_eq!(grid.cells[0].top(), rect.top());
        assert_eq!(grid.cells[15].right(), rect.right());
        assert_eq!(grid.cells[15].bottom(), rect.bottom());
        for index in 0..16 {
            assert_eq!(grid.cells[index].width(), grid.cell.x);
            assert_eq!(grid.cells[index].height(), grid.cell.y);
            for other in (index + 1)..16 {
                assert!(
                    !grid.cells[index].intersects(grid.cells[other]),
                    "pads {index} and {other} overlap"
                );
            }
        }
    }

    #[test]
    fn all_sixteen_pads_fit_at_1280x720() {
        // Central panel after the 52 pt top bar and 30 pt status bar.
        let rect = screen(1280.0, 720.0 - 52.0 - 30.0);
        let grid = layout(rect, 8.0);
        assert!(is_usable(&grid), "cell {:?} is too small at 1280x720", grid.cell);
        for cell in grid.cells {
            assert!(rect.contains_rect(cell));
        }
    }

    #[test]
    fn performance_mode_pads_are_large_at_1080p() {
        let rect = screen(1920.0, 1080.0 - 52.0 - 30.0 - 90.0);
        let grid = layout(rect, 12.0);
        assert!(
            grid.cell.x >= 300.0 && grid.cell.y >= 180.0,
            "performance pads are only {:?}",
            grid.cell
        );
    }

    #[test]
    fn a_tiny_window_still_produces_positive_cells() {
        let grid = layout(screen(80.0, 60.0), 8.0);
        assert!(grid.cell.x >= 1.0 && grid.cell.y >= 1.0);
        for cell in grid.cells {
            assert!(cell.width() > 0.0 && cell.height() > 0.0);
        }
    }
}
