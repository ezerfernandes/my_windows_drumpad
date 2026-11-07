//! UI composition: the top bar, the visible view, the status bar, and overlays.
//!
//! [`UiRoot::show`] is the only entry point the shell needs. It draws a frame and
//! returns the [`Action`]s the user produced; it never performs I/O or talks to
//! the engine. That keeps the whole interaction model testable headlessly with
//! `egui::Context::run`.

pub mod dialogs;
pub mod inspector;
pub mod kit_browser;
pub mod map_overlay;
pub mod pad_grid;
pub mod performance;
pub mod settings;
pub mod theme;
pub mod waveform;
pub mod wizard;

use std::path::PathBuf;

use egui::{Align, Color32, Layout, RichText, Ui, vec2};
use winit::keyboard::KeyCode;

use crate::app::actions::Action;
use crate::app::hotkeys::SHORTCUT_HELP;
use crate::state::kit::KitId;
use crate::state::navigation::View;
use crate::state::pad::PadId;
use crate::state::prefs::{ThemeChoice, UiPreferences};
use crate::state::AppState;

use theme::Theme;

/// Transient UI state that is not worth persisting.
#[derive(Debug, Clone)]
pub struct UiLocal {
    /// Kit search text.
    pub kit_search: String,
    /// New-kit name field.
    pub new_kit_name: String,
    /// Inline rename editor.
    pub rename: Option<(KitId, String)>,
    /// Selected row in the kit browser.
    pub selected_kit: Option<KitId>,
    /// Pad waiting for the next physical key.
    pub capturing_key_for: Option<PadId>,
    /// Inspector audition velocity.
    pub audition_velocity: u8,
    /// Source the edit sliders were last synchronised with.
    pub edit_source: Option<PathBuf>,
    /// Editor gain slider value.
    pub edit_gain_db: f32,
    /// Editor pitch slider value.
    pub edit_pitch: f32,
    /// Editor loop crossfade value.
    pub edit_crossfade: f32,
    /// Wizard sample-folder text.
    pub wizard_folder: String,
    /// Whether the shortcut cheat sheet is open.
    pub show_shortcuts: bool,
    /// Last measured performance grid height, for the shell's diagnostics.
    pub last_performance_height: f32,
    /// Pad rectangles from the last frame, for drop targeting.
    pub grid_cells: Option<[egui::Rect; 16]>,
    /// Pointer position from the last frame.
    pub pointer: Option<egui::Pos2>,
}

impl UiLocal {
    /// The pad under the pointer, when the grid was drawn last frame.
    pub fn pad_at_pointer(&self) -> Option<PadId> {
        let cells = self.grid_cells.as_ref()?;
        let pointer = self.pointer?;
        cells
            .iter()
            .position(|cell| cell.contains(pointer))
            .and_then(PadId::try_new)
    }
}

impl Default for UiLocal {
    fn default() -> Self {
        Self {
            kit_search: String::new(),
            new_kit_name: String::new(),
            rename: None,
            selected_kit: None,
            capturing_key_for: None,
            audition_velocity: 100,
            edit_source: None,
            edit_gain_db: 0.0,
            edit_pitch: 0.0,
            edit_crossfade: 0.0,
            wizard_folder: String::new(),
            show_shortcuts: false,
            last_performance_height: 0.0,
            grid_cells: None,
            pointer: None,
        }
    }
}

/// The egui root.
pub struct UiRoot {
    theme: Theme,
    theme_choice: ThemeChoice,
    scale: f32,
    reduce_motion: bool,
    fit_zoom: f32,
    local: UiLocal,
}

impl UiRoot {
    /// Build the root from persisted preferences.
    pub fn new(preferences: &UiPreferences) -> Self {
        let theme = Theme::for_choice(preferences.appearance.theme);
        Self {
            theme,
            theme_choice: preferences.appearance.theme,
            scale: preferences.appearance.scale,
            reduce_motion: preferences.appearance.reduce_motion,
            fit_zoom: 1.0,
            local: UiLocal::default(),
        }
    }

    /// Zoom needed to keep the 1280x720 stage layout on screen, set by the
    /// shell from the window's logical size.
    pub fn set_fit_zoom(&mut self, zoom: f32) {
        self.fit_zoom = zoom.clamp(0.4, 1.0);
    }

    /// The current fit zoom.
    pub fn fit_zoom(&self) -> f32 {
        self.fit_zoom
    }

    /// The active theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Transient UI state, for tests and the shell.
    pub fn local(&self) -> &UiLocal {
        &self.local
    }

    /// Mutable transient state, for tests and the shell.
    pub fn local_mut(&mut self) -> &mut UiLocal {
        &mut self.local
    }

    /// Rebuild the palette when preferences change.
    pub fn sync_theme(&mut self, preferences: &UiPreferences) {
        let choice = preferences.appearance.theme;
        let scale = preferences.appearance.scale;
        let reduce_motion = preferences.appearance.reduce_motion;
        if choice != self.theme_choice || reduce_motion != self.reduce_motion {
            self.theme = Theme::for_choice(choice);
            self.theme.reduce_motion = reduce_motion;
            self.theme_choice = choice;
            self.reduce_motion = reduce_motion;
        }
        self.scale = scale;
    }

    /// Draw one frame and return the user's actions.
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
        self.sync_theme(state.settings.draft());
        self.theme.apply(ctx, self.scale * self.fit_zoom);
        let theme = self.theme.clone();
        let mut actions = Vec::new();
        self.local.pointer = ctx.input(|input| input.pointer.hover_pos());

        // Performance mode hides all editing chrome: no top bar, no status bar.
        // The in-view header keeps the kit name, transport state, and exit.
        let performance = state.navigation.current() == View::Performance;
        if !performance {
            top_bar(ctx, &theme, state, &mut actions);
        }

        if state.navigation.current() == View::Edit && state.editor.is_some() {
            egui::TopBottomPanel::bottom("drumpad-editor")
                .resizable(true)
                .default_height(220.0)
                .min_height(120.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme.panel)
                        .inner_margin(egui::Margin::same(theme.sm as i8)),
                )
                .show(ctx, |ui| {
                    waveform::show(ui, &theme, state, &mut self.local, &mut actions);
                });
        }

        if !performance {
            status_bar(ctx, &theme, state, &mut self.local, &mut actions);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.bg)
                    .inner_margin(egui::Margin::same(theme.md as i8)),
            )
            .show(ctx, |ui| match state.navigation.current() {
                View::Performance => {
                    performance::show(ui, &theme, state, &mut self.local, &mut actions);
                }
                View::Edit => edit_view(ui, &theme, state, &mut self.local, &mut actions),
                View::KitBrowser => {
                    kit_browser::show(ui, &theme, state, &mut self.local, &mut actions);
                }
                View::Settings => {
                    settings::show(ui, &theme, state, &mut self.local, &mut actions);
                }
            });

        wizard::show(ctx, &theme, state, &mut self.local, &mut actions);
        map_overlay::show(ctx, &theme, state, &mut actions);
        dialogs::show(ctx, &theme, state, &mut actions);
        shortcuts_window(ctx, &theme, &mut self.local);

        actions
    }
}

fn top_bar(ctx: &egui::Context, theme: &Theme, state: &AppState, actions: &mut Vec<Action>) {
    egui::TopBottomPanel::top("drumpad-top")
        .exact_height(52.0)
        .frame(
            egui::Frame::new()
                .fill(theme.panel)
                .inner_margin(egui::Margin::symmetric(theme.md as i8, theme.xs as i8)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new("DRUMPAD")
                        .color(theme.accent)
                        .size(theme.h2)
                        .strong(),
                );
                ui.add_space(theme.lg);
                for view in View::ALL {
                    let selected = state.navigation.current() == view;
                    let response = ui.add(
                        egui::Button::selectable(
                            selected,
                            RichText::new(view.label())
                                .size(theme.body)
                                .color(if selected { theme.accent } else { theme.text }),
                        )
                        .min_size(vec2(110.0, 34.0)),
                    );
                    response.clone().on_hover_text(view.shortcut());
                    if response.clicked() {
                        actions.push(Action::Navigate(view));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button(
                        RichText::new(state.status().profile.clone()).color(theme.text_muted),
                        |ui| {
                            for profile in crate::state::keymap::KeyboardProfile::builtins() {
                                if ui
                                    .selectable_label(
                                        state.profile_id() == profile.id,
                                        profile.name,
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::SetProfile(profile.id));
                                    ui.close();
                                }
                            }
                        },
                    );
                    let status = state.status();
                    if status.dirty {
                        ui.label(RichText::new("UNSAVED").color(theme.warning).size(theme.small));
                    }
                    ui.label(
                        RichText::new(format!("{} · {}", status.kit, status.load))
                            .color(if matches!(
                                state.kits.load_status(),
                                crate::state::kit::LoadStatus::Failed { .. }
                            ) {
                                theme.error
                            } else {
                                theme.text_muted
                            })
                            .size(theme.small),
                    );
                });
            });
        });
}

fn edit_view(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    // Split the central area manually. `SidePanel::show_inside` mutates the
    // parent's max rect in a way that let the grid run past the status bar, so
    // the two columns are allocated from an explicit rect instead.
    let full = ui.available_rect_before_wrap();
    let (grid_rect, inspector_rect) = split_edit_columns(full, theme.md);

    ui.scope_builder(egui::UiBuilder::new().max_rect(grid_rect), |ui| {
        performance::edit_hints(ui, theme, state);
        ui.add_space(theme.xs);
        let grid_rect = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(grid_rect), |ui| {
            pad_grid::show(ui, theme, state, local, actions, false);
        });
    });

    ui.scope_builder(egui::UiBuilder::new().max_rect(inspector_rect), |ui| {
        egui::Frame::new()
            .fill(theme.panel)
            .corner_radius(theme.radius)
            .inner_margin(egui::Margin::same(theme.md as i8))
            .show(ui, |ui| {
                inspector::show(ui, theme, state, local, actions);
            });
    });
}

/// Split the Edit view into the pad-grid column and the inspector column.
///
/// Both rects stay inside `full`, so the grid can never draw behind the status
/// bar. The inspector is capped at 42% of the width and never narrower than
/// 300 points while the grid keeps at least 320 points.
fn split_edit_columns(full: egui::Rect, gap: f32) -> (egui::Rect, egui::Rect) {
    let inspector_width = 380.0_f32.min((full.width() * 0.42).max(300.0));
    let grid_width = (full.width() - inspector_width - gap).max(320.0);
    let split = full.min.x + grid_width;
    let grid_rect = egui::Rect::from_min_max(full.min, egui::pos2(split, full.max.y));
    let inspector_rect =
        egui::Rect::from_min_max(egui::pos2(split + gap, full.min.y), full.max);
    (grid_rect, inspector_rect)
}

fn status_bar(
    ctx: &egui::Context,
    theme: &Theme,
    state: &AppState,
    local: &mut UiLocal,
    actions: &mut Vec<Action>,
) {
    egui::TopBottomPanel::bottom("drumpad-status")
        .exact_height(30.0)
        .frame(
            egui::Frame::new()
                .fill(theme.panel)
                .inner_margin(egui::Margin::symmetric(theme.md as i8, theme.xs as i8)),
        )
        .show(ctx, |ui| {
            let status = state.status();
            ui.horizontal_centered(|ui| {
                ui.label(
                    RichText::new(format!("Kit: {} | Audio: {}", status.kit, status.audio))
                        .color(if state.audio.running {
                            theme.text_muted
                        } else {
                            theme.error
                        })
                        .size(theme.small),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("?")
                        .on_hover_text("Keyboard shortcuts")
                        .clicked()
                    {
                        local.show_shortcuts = !local.show_shortcuts;
                    }
                    if state.performance.metronome() {
                        ui.label(RichText::new("METRONOME").color(theme.success).size(theme.small));
                    }
                    if state.dragdrop.can_undo() && ui.small_button("Undo drop").clicked() {
                        actions.push(Action::UndoDrop);
                    }
                    ui.label(
                        RichText::new(format!("Pad {} | {}", status.pad, status.profile))
                            .color(theme.text_muted)
                            .size(theme.small),
                    );
                });
            });
        });
}

fn shortcuts_window(ctx: &egui::Context, theme: &Theme, local: &mut UiLocal) {
    if !local.show_shortcuts {
        return;
    }
    let mut open = true;
    egui::Window::new("Keyboard shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Grid::new("shortcuts")
                .num_columns(2)
                .spacing([theme.xl, theme.xs])
                .show(ui, |ui| {
                    for (keys, description) in SHORTCUT_HELP {
                        ui.label(RichText::new(keys).color(theme.accent).size(theme.small));
                        ui.label(
                            RichText::new(description)
                                .color(theme.text)
                                .size(theme.small),
                        );
                        ui.end_row();
                    }
                });
        });
    if !open {
        local.show_shortcuts = false;
    }
}

/// Consume the next key for the inspector's "Rebind" capture.
///
/// Returns `true` when the key was consumed, which means the caller must not
/// route it to the pad grid.
pub fn capture_key(state: &mut AppState, local: &mut UiLocal, key: KeyCode) -> bool {
    let Some(pad) = local.capturing_key_for.take() else {
        return false;
    };
    if key == KeyCode::Escape {
        return true;
    }
    if pad == state.inspector.pad() {
        state.inspector.draft_mut().key = Some(key);
    }
    true
}

/// A colour used by smoke tests to verify the theme was installed.
pub fn background(theme: &Theme) -> Color32 {
    theme.bg
}
#[cfg(test)]
mod tests {
    use super::*;

    fn headless(state: &mut AppState, ui: &mut UiRoot, steps: usize) {
        let ctx = egui::Context::default();
        for _ in 0..steps {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                vec2(1280.0, 720.0),
            ));
            let mut collected = Vec::new();
            let _ = ctx.run(input, |ctx| {
                collected = ui.show(ctx, state);
            });
            let _ = collected;
        }
    }

    #[test]
    fn the_shell_renders_every_view_without_panicking_at_720p() {
        let preferences = UiPreferences::default();
        let mut state = AppState::new(preferences.clone());
        let mut ui = UiRoot::new(&preferences);
        for view in View::ALL {
            state.navigation.go(view);
            headless(&mut state, &mut ui, 1);
        }
    }

    #[test]
    fn the_edit_split_keeps_both_columns_inside_the_panel() {
        let full = egui::Rect::from_min_size(egui::pos2(12.0, 64.0), vec2(1830.0, 794.0));
        let (grid, inspector) = split_edit_columns(full, 12.0);
        assert!(full.contains_rect(grid));
        assert!(full.contains_rect(inspector));
        assert!(grid.right() <= inspector.left());
        assert!(grid.width() >= 320.0);
        assert!(inspector.width() >= 300.0);
        assert_eq!(grid.max.y, full.max.y);
        assert_eq!(inspector.max.y, full.max.y);
    }

    #[test]
    fn the_edit_grid_bottom_stays_above_the_status_bar() {
        // Central panel content after the 52 pt top bar and 30 pt status bar.
        let full = egui::Rect::from_min_size(egui::pos2(12.0, 64.0), vec2(1830.0, 794.0));
        let status_top = 64.0 + 794.0 + 12.0;
        let (grid, _) = split_edit_columns(full, 12.0);
        assert!(grid.max.y <= status_top);
    }

    #[test]
    fn a_narrow_window_still_gets_a_usable_grid() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(700.0, 500.0));
        let (grid, inspector) = split_edit_columns(full, 8.0);
        assert!(grid.width() >= 320.0);
        assert!(inspector.left() >= grid.right());
    }

    #[test]
    fn the_wizard_renders_when_setup_is_incomplete() {
        let preferences = UiPreferences::default();
        let mut state = AppState::new(preferences.clone());
        assert!(state.wizard.is_active());
        let mut ui = UiRoot::new(&preferences);
        headless(&mut state, &mut ui, 1);
    }

    #[test]
    fn the_theme_can_be_switched_between_frames() {
        let mut preferences = UiPreferences::default();
        let mut state = AppState::new(preferences.clone());
        let mut ui = UiRoot::new(&preferences);
        headless(&mut state, &mut ui, 1);
        assert_eq!(ui.theme().bg, Theme::stage().bg);
        preferences.appearance.theme = ThemeChoice::HighContrast;
        ui.sync_theme(&preferences);
        assert_eq!(ui.theme().bg, Theme::high_contrast().bg);
    }

    #[test]
    fn the_fit_zoom_is_clamped_and_applied_by_the_root() {
        let preferences = UiPreferences::default();
        let mut ui = UiRoot::new(&preferences);
        ui.set_fit_zoom(0.5);
        assert_eq!(ui.fit_zoom(), 0.5);
        ui.set_fit_zoom(2.0);
        assert_eq!(ui.fit_zoom(), 1.0);
    }

    #[test]
    fn rebinding_consumes_exactly_one_key_and_escape_cancels() {
        let preferences = UiPreferences::default();
        let mut state = AppState::new(preferences.clone());
        let mut ui = UiRoot::new(&preferences);
        let mut local = UiLocal::default();

        local.capturing_key_for = Some(state.inspector.pad());
        assert!(capture_key(&mut state, &mut local, KeyCode::KeyP));
        assert_eq!(state.inspector.draft().key, Some(KeyCode::KeyP));
        assert!(local.capturing_key_for.is_none());

        local.capturing_key_for = Some(state.inspector.pad());
        assert!(capture_key(&mut state, &mut local, KeyCode::Escape));
        assert_eq!(state.inspector.draft().key, Some(KeyCode::KeyP));

        assert!(!capture_key(&mut state, &mut local, KeyCode::KeyQ));
    }
}