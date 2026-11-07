//! Waveform editor: multi-resolution peaks, zoom/scroll, selection, trim, fades,
//! normalize, gain, reverse, loop points, pitch, and undo/redo.
//!
//! The widget only reads peaks and parameters. Every change is emitted as
//! [`Action::CommitEdit`], so the edit model stays the single source of truth
//! and undo works uniformly.

use std::path::PathBuf;

use egui::{
    Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::app::actions::Action;
use crate::state::editor::{EditParams, SourceRef, MAX_GAIN_DB, MIN_GAIN_DB, pitch_factor};
use crate::state::waveform::{PeakSet, WaveformView};
use crate::state::AppState;

use super::theme::Theme;
use super::UiLocal;

/// Draw the sample editor. Returns silently when no sample is open.
pub fn show(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    let Some(editor) = state.editor.as_ref() else {
        ui.label(
            RichText::new("Select a pad with a sample to edit it.")
                .color(theme.text_disabled)
                .size(theme.small),
        );
        return;
    };

    sync_locals(editor, local);

    let source = editor.source().clone();
    let total = source.duration_seconds();
    let base = editor.current().clone();
    let file_name = source
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.path.display().to_string());
    let can_undo = editor.can_undo();
    let can_redo = editor.can_redo();
    let undo_label = editor.undo_label().map(str::to_owned);
    let redo_label = editor.redo_label().map(str::to_owned);
    let sample_rate = source.sample_rate;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Sample editor")
                .color(theme.accent)
                .size(theme.small)
                .strong(),
        );
        ui.label(RichText::new(file_name).color(theme.text).size(theme.body));
        ui.label(
            RichText::new(format!(
                "{total:.2}s @ {sample_rate} Hz x{}",
                source.channels
            ))
            .color(theme.text_muted)
            .size(theme.small),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_redo, egui::Button::new("Redo"))
                .on_hover_text(redo_label.unwrap_or_else(|| "Nothing to redo".to_owned()))
                .clicked()
            {
                actions.push(Action::RedoEdit);
            }
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo"))
                .on_hover_text(undo_label.unwrap_or_else(|| "Nothing to undo".to_owned()))
                .clicked()
            {
                actions.push(Action::UndoEdit);
            }
            if ui.button("Reset").clicked() {
                actions.push(Action::ResetEdit);
            }
            if ui.button("Fit").clicked() {
                state.waveform.fit();
            }
        });
    });

    let canvas_height = (ui.available_height() - 92.0).clamp(80.0, 260.0);
    let width = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(vec2(width, canvas_height), Sense::click_and_drag());
    let rect = response.rect;
    painter.rect_filled(rect, CornerRadius::same(theme.radius), theme.bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(theme.radius),
        Stroke::new(1.0_f32, theme.panel_alt),
        StrokeKind::Inside,
    );

    // Input: wheel zooms around the pointer, shift+wheel scrolls, drag selects.
    if response.hovered()
        && let Some(pointer) = response.hover_pos()
    {
        let anchor = state
            .waveform
            .x_to_time(total, pointer.x - rect.left(), rect.width());
        let scroll = ui.input(|input| input.raw_scroll_delta);
        if scroll.y.abs() > 0.0 {
            if ui.input(|input| input.modifiers.shift) {
                state.waveform.scroll_by(total, -scroll.y * 0.0025);
            } else {
                state
                    .waveform
                    .zoom_around(total, anchor, (scroll.y * 0.0015).exp());
            }
        }
    }

    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = state
            .waveform
            .x_to_time(total, pointer.x - rect.left(), rect.width());
        state.waveform.begin_selection(total, time);
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = state
            .waveform
            .x_to_time(total, pointer.x - rect.left(), rect.width());
        state.waveform.extend_selection(total, time);
    }
    if response.drag_stopped() {
        state.waveform.finish_selection();
    }
    if response.double_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = state
            .waveform
            .x_to_time(total, pointer.x - rect.left(), rect.width());
        state.waveform.playhead = Some(time);
    }

    let view = &state.waveform;
    let (left, right) = view.range(total);

    if let Some((start, end)) = view.selection {
        let x0 = view.time_to_x(total, start, rect.width()) + rect.left();
        let x1 = view.time_to_x(total, end, rect.width()) + rect.left();
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())),
            CornerRadius::ZERO,
            theme.accent.gamma_multiply(0.18),
        );
    }

    let mut drew_peaks = false;
    if let Some(peaks) = state.peaks.as_ref() {
        drew_peaks = draw_peaks(&painter, theme, peaks, view, total, sample_rate, rect);
    }
    if !drew_peaks {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Waveform peaks are not built yet",
            FontId::proportional(theme.body),
            theme.text_disabled,
        );
    }

    painter.line_segment(
        [
            pos2(rect.left(), rect.center().y),
            pos2(rect.right(), rect.center().y),
        ],
        Stroke::new(1.0_f32, theme.panel_alt),
    );

    if let Some((loop_start, loop_end)) = base.loop_window() {
        for time in [loop_start, loop_end] {
            if (left..=right).contains(&time) {
                let x = view.time_to_x(total, time, rect.width()) + rect.left();
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(2.0_f32, theme.warning),
                );
            }
        }
    }

    if let Some(playhead) = view.playhead {
        let x = view.time_to_x(total, playhead, rect.width()) + rect.left();
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0_f32, theme.success),
        );
    }

    let selection_text = view
        .selection
        .map(|(start, end)| format!("selection {start:.3}s..{end:.3}s ({:.2}s)", end - start))
        .unwrap_or_else(|| "drag to select".to_owned());
    painter.text(
        rect.left_bottom() + vec2(theme.sm, -theme.sm),
        Align2::LEFT_BOTTOM,
        format!("view {left:.3}s..{right:.3}s | {selection_text}"),
        theme.small_font(),
        theme.text_muted,
    );

    let selection = view.selection;
    let factor = pitch_factor(base.pitch_semitones);
    let to_source = |time: f32| base.trim_start + time * factor;

    ui.add_space(theme.xs);
    edit_controls(
        ui,
        theme,
        local,
        actions,
        &base,
        &source,
        selection,
        to_source,
        factor,
    );
}

fn draw_peaks(
    painter: &egui::Painter,
    theme: &Theme,
    peaks: &PeakSet,
    view: &WaveformView,
    total: f32,
    sample_rate: u32,
    rect: Rect,
) -> bool {
    if peaks.is_empty() || sample_rate == 0 {
        return false;
    }
    let (left, right) = view.range(total);
    let visible_frames = ((right - left) * sample_rate as f32).max(1.0) as usize;
    let Some(level) = peaks.level_for_span(visible_frames, rect.width() as usize) else {
        return false;
    };
    if level.buckets.is_empty() {
        return false;
    }

    let frames_per_bucket = level.frames_per_bucket.max(1);
    let left_frames = left * sample_rate as f32;
    let first = (left_frames / frames_per_bucket as f32).floor().max(0.0) as usize;
    let last = (((right * sample_rate as f32) / frames_per_bucket as f32).ceil() as usize)
        .min(level.buckets.len());
    let half = rect.height() * 0.46;
    let center = rect.center().y;
    let stroke = Stroke::new(
        (rect.width() / level.buckets.len() as f32).max(1.0),
        theme.accent,
    );

    for (index, bucket) in level.buckets.iter().enumerate().take(last).skip(first) {
        let time = index as f32 * frames_per_bucket as f32 / sample_rate as f32;
        let x = view.time_to_x(total, time, rect.width()) + rect.left();
        let y_max = center - bucket.max.clamp(-1.0, 1.0) * half;
        let y_min = center - bucket.min.clamp(-1.0, 1.0) * half;
        painter.line_segment([pos2(x, y_max), pos2(x, y_min)], stroke);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn edit_controls(
    ui: &mut Ui,
    theme: &Theme,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
    base: &EditParams,
    source: &SourceRef,
    selection: Option<(f32, f32)>,
    to_source: impl Fn(f32) -> f32,
    factor: f32,
) {
    let commit = |actions: &mut Vec<Action>, label: &str, params: EditParams| {
        actions.push(Action::CommitEdit {
            label: label.to_owned(),
            params: Box::new(params),
        });
    };
    let trimmed = base.trimmed_duration(source);

    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(selection.is_some(), egui::Button::new("Trim to selection"))
            .clicked()
            && let Some((start, end)) = selection
        {
            let mut params = base.clone();
            let trim_end = base.effective_trim_end(source);
            params.trim_start = to_source(start).clamp(0.0, (trim_end - 0.001).max(0.0));
            params.trim_end = Some(to_source(end).min(trim_end));
            commit(actions, "Trim", params);
        }
        if ui
            .add_enabled(selection.is_some(), egui::Button::new("Fade in here"))
            .clicked()
            && let Some((start, end)) = selection
        {
            let mut params = base.clone();
            params.fade_in = ((end - start) * factor).min(trimmed);
            commit(actions, "Fade in", params);
        }
        if ui
            .add_enabled(selection.is_some(), egui::Button::new("Fade out here"))
            .clicked()
            && let Some((start, end)) = selection
        {
            let mut params = base.clone();
            params.fade_out = ((end - start) * factor).min(trimmed);
            commit(actions, "Fade out", params);
        }
        if ui
            .add_enabled(!base.normalize, egui::Button::new("Normalize"))
            .clicked()
        {
            let mut params = base.clone();
            params.normalize = true;
            commit(actions, "Normalize", params);
        }
        if ui
            .add_enabled(!base.reverse, egui::Button::new("Reverse"))
            .clicked()
        {
            let mut params = base.clone();
            params.reverse = true;
            commit(actions, "Reverse", params);
        }
        if ui
            .button(if base.loop_enabled {
                "Loop: on"
            } else {
                "Loop: off"
            })
            .clicked()
        {
            let mut params = base.clone();
            params.loop_enabled = !base.loop_enabled;
            if params.loop_enabled {
                if let Some((start, end)) = selection {
                    params.loop_start = to_source(start);
                    params.loop_end = to_source(end);
                } else {
                    params.loop_start = base.trim_start;
                    params.loop_end = base.effective_trim_end(source);
                }
                params.loop_crossfade = 0.0;
            }
            commit(actions, "Loop", params);
        }
        if base.loop_enabled && selection.is_some() && ui.button("Loop = selection").clicked() {
            let (start, end) = selection.expect("selection");
            let mut params = base.clone();
            params.loop_start = to_source(start);
            params.loop_end = to_source(end);
            commit(actions, "Loop points", params);
        }
    });

    ui.horizontal(|ui| {
        ui.add(
            egui::Slider::new(&mut local.edit_gain_db, MIN_GAIN_DB..=MAX_GAIN_DB)
                .text("pad gain")
                .suffix(" dB")
                .max_decimals(1),
        );
        if ui.button("Apply gain").clicked() {
            let mut params = base.clone();
            params.gain_db = local.edit_gain_db;
            commit(actions, "Gain", params);
        }

        ui.add(
            egui::Slider::new(&mut local.edit_pitch, -24.0..=24.0)
                .text("pitch")
                .suffix(" st")
                .max_decimals(1),
        );
        if ui.button("Apply pitch").clicked() {
            let mut params = base.clone();
            params.pitch_semitones = local.edit_pitch;
            commit(actions, "Pitch", params);
        }

        let max_crossfade = ((base.loop_end - base.loop_start) * 0.5).max(0.001);
        ui.add_enabled(
            base.loop_enabled,
            egui::Slider::new(&mut local.edit_crossfade, 0.0..=max_crossfade)
                .text("loop xfade")
                .suffix(" s")
                .max_decimals(3),
        );
        if base.loop_enabled && ui.button("Apply xfade").clicked() {
            let mut params = base.clone();
            params.loop_crossfade = local.edit_crossfade.min(max_crossfade);
            commit(actions, "Loop crossfade", params);
        }
    });

    if base.normalize {
        ui.label(
            RichText::new("Normalized to -0.1 dBFS")
                .color(theme.text_muted)
                .size(theme.small),
        );
    }
}

fn sync_locals(editor: &crate::state::editor::EditSession, local: &mut UiLocal) {
    let source: PathBuf = editor.source_path().to_path_buf();
    if local.edit_source.as_ref() == Some(&source) {
        return;
    }
    local.edit_source = Some(source);
    local.edit_gain_db = editor.current().gain_db;
    local.edit_pitch = editor.current().pitch_semitones;
    local.edit_crossfade = editor.current().loop_crossfade;
}
