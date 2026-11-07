//! Design tokens: colour, type, spacing, radius, and the egui style built from
//! them.
//!
//! Every colour in the app comes from a [`Theme`]. Two themes ship: `Stage`, the
//! dark high-contrast performance palette, and `HighContrast`, the same layout
//! with brighter focus rings and text for a very dark room. The contrast ratios
//! the design promises are asserted in tests, so a palette tweak that makes the
//! stage unreadable fails CI.

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

use crate::state::pad::{PadId, PadState};
use crate::state::prefs::ThemeChoice;

/// Colour and metric tokens for one theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Window background.
    pub bg: Color32,
    /// Raised panel background.
    pub panel: Color32,
    /// Alternate row / secondary panel.
    pub panel_alt: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text that still has to be readable on stage.
    pub text_muted: Color32,
    /// Disabled text; never used for information the user needs.
    pub text_disabled: Color32,
    /// Interactive accent.
    pub accent: Color32,
    /// Accent hover.
    pub accent_hover: Color32,
    /// Success state.
    pub success: Color32,
    /// Warning state.
    pub warning: Color32,
    /// Error state.
    pub error: Color32,
    /// Keyboard focus ring.
    pub focus: Color32,
    /// Grid gaps and padding.
    pub xs: f32,
    /// Small gap.
    pub sm: f32,
    /// Medium gap.
    pub md: f32,
    /// Large gap.
    pub lg: f32,
    /// Section gap.
    pub xl: f32,
    /// Display type size.
    pub display: f32,
    /// Heading size.
    pub h1: f32,
    /// Subheading size.
    pub h2: f32,
    /// Body size.
    pub body: f32,
    /// Small print.
    pub small: f32,
    /// Monospace / numeric size.
    pub mono: f32,
    /// Corner radius for cards and buttons.
    pub radius: u8,
    /// Corner radius for small chips.
    pub chip_radius: u8,
    /// Base pad colours, one per column.
    pub pad_bases: [Color32; 4],
    /// Whether hit animations should be reduced.
    pub reduce_motion: bool,
}

impl Theme {
    /// The default stage theme.
    pub const fn stage() -> Self {
        Self {
            bg: Color32::from_rgb(0x0B, 0x0D, 0x10),
            panel: Color32::from_rgb(0x14, 0x18, 0x1D),
            panel_alt: Color32::from_rgb(0x1B, 0x20, 0x27),
            text: Color32::from_rgb(0xF2, 0xF5, 0xF8),
            text_muted: Color32::from_rgb(0xB4, 0xBE, 0xC9),
            text_disabled: Color32::from_rgb(0x6E, 0x78, 0x84),
            accent: Color32::from_rgb(0x4C, 0xC9, 0xF0),
            accent_hover: Color32::from_rgb(0x7A, 0xDB, 0xF7),
            success: Color32::from_rgb(0x5A, 0xE0, 0xA8),
            warning: Color32::from_rgb(0xFF, 0xC4, 0x4D),
            error: Color32::from_rgb(0xFF, 0x6E, 0x6E),
            focus: Color32::from_rgb(0xFF, 0xE0, 0x66),
            xs: 4.0,
            sm: 8.0,
            md: 12.0,
            lg: 20.0,
            xl: 32.0,
            display: 40.0,
            h1: 28.0,
            h2: 22.0,
            body: 17.0,
            small: 13.0,
            mono: 14.0,
            radius: 10,
            chip_radius: 5,
            pad_bases: [
                Color32::from_rgb(0x12, 0x3B, 0x40),
                Color32::from_rgb(0x4A, 0x31, 0x00),
                Color32::from_rgb(0x2E, 0x21, 0x48),
                Color32::from_rgb(0x10, 0x2B, 0x4A),
            ],
            reduce_motion: false,
        }
    }

    /// The high-contrast variant for very dark rooms.
    pub const fn high_contrast() -> Self {
        Self {
            bg: Color32::from_rgb(0x00, 0x00, 0x00),
            panel: Color32::from_rgb(0x0E, 0x10, 0x13),
            panel_alt: Color32::from_rgb(0x18, 0x1C, 0x21),
            text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            text_muted: Color32::from_rgb(0xD0, 0xD8, 0xE0),
            text_disabled: Color32::from_rgb(0x7A, 0x84, 0x90),
            accent: Color32::from_rgb(0x6F, 0xE0, 0xFF),
            accent_hover: Color32::from_rgb(0xA8, 0xED, 0xFF),
            success: Color32::from_rgb(0x74, 0xFF, 0xC0),
            warning: Color32::from_rgb(0xFF, 0xD1, 0x66),
            error: Color32::from_rgb(0xFF, 0x8A, 0x8A),
            focus: Color32::from_rgb(0xFF, 0xF0, 0x80),
            ..Self::stage()
        }
    }

    /// Theme for a persisted choice.
    pub const fn for_choice(choice: ThemeChoice) -> Self {
        match choice {
            ThemeChoice::Stage => Self::stage(),
            ThemeChoice::HighContrast => Self::high_contrast(),
        }
    }

    /// Body font.
    pub fn body_font(&self) -> FontId {
        FontId::new(self.body, FontFamily::Proportional)
    }

    /// Small font.
    pub fn small_font(&self) -> FontId {
        FontId::new(self.small, FontFamily::Proportional)
    }

    /// Heading font.
    pub fn heading_font(&self) -> FontId {
        FontId::new(self.h1, FontFamily::Proportional)
    }

    /// Monospace font.
    pub fn mono_font(&self) -> FontId {
        FontId::new(self.mono, FontFamily::Monospace)
    }

    /// Base colour for a pad, shaded by row so the 4x4 grid is easy to scan.
    pub fn pad_base(&self, pad: &PadState) -> Color32 {
        let base = self.pad_bases[pad.color as usize % self.pad_bases.len()];
        let shade = match pad.id.row() {
            0 => 1.0,
            1 => 0.88,
            2 => 0.78,
            _ => 0.70,
        };
        scale_rgb(base, shade)
    }

    /// Hit-flash colour: pad base moved toward the accent, not white, so the
    /// label keeps its contrast.
    pub fn pad_flash(&self, pad: &PadState) -> Color32 {
        let amount = if self.reduce_motion {
            (pad.flash * 0.18).min(0.18)
        } else {
            pad.flash * 0.38
        };
        lerp_color(self.pad_base(pad), self.accent, amount)
    }

    /// Selection border colour.
    pub fn pad_border(&self, pad: &PadState) -> (Color32, f32) {
        if pad.selected {
            (self.focus, 3.0)
        } else {
            (self.panel_alt, 1.0)
        }
    }

    /// Colour for a pad in a specific state, used by tests and the widget.
    pub fn pad_fill(&self, pad: &PadState) -> Color32 {
        if pad.flash > 0.0 {
            self.pad_flash(pad)
        } else {
            self.pad_base(pad)
        }
    }

    /// Install the theme into an egui context.
    pub fn apply(&self, ctx: &egui::Context, scale: f32) {
        let mut style = (*ctx.style()).clone();
        let mut visuals = Visuals::dark();
        visuals.panel_fill = self.panel;
        visuals.window_fill = self.panel;
        visuals.extreme_bg_color = self.bg;
        visuals.faint_bg_color = self.panel_alt;
        visuals.override_text_color = Some(self.text);
        visuals.hyperlink_color = self.accent;
        visuals.warn_fg_color = self.warning;
        visuals.error_fg_color = self.error;
        visuals.selection.bg_fill = self.accent.gamma_multiply(0.35);
        visuals.selection.stroke = Stroke::new(1.0_f32, self.text);
        visuals.window_stroke = Stroke::new(1.0_f32, self.panel_alt);
        visuals.widgets.noninteractive.bg_fill = self.panel;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text);
        visuals.widgets.inactive.bg_fill = self.panel_alt;
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text);
        visuals.widgets.hovered.bg_fill = self.accent.gamma_multiply(0.28);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, self.text);
        visuals.widgets.active.bg_fill = self.accent.gamma_multiply(0.40);
        visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, self.text);
        visuals.widgets.open.bg_fill = self.panel_alt;
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = CornerRadius::same(self.radius);
        }
        style.visuals = visuals;

        style.text_styles = [
            (TextStyle::Heading, FontId::new(self.h1, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(self.body, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(self.body, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(self.small, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(self.mono, FontFamily::Monospace)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(self.sm, self.sm);
        style.spacing.button_padding = egui::vec2(self.md, self.sm);
        style.spacing.window_margin = egui::Margin::same(self.lg as i8);
        style.spacing.menu_margin = egui::Margin::same(self.sm as i8);
        ctx.set_style(style);
        ctx.set_zoom_factor(scale.clamp(0.75, 2.0));
    }
}

fn scale_rgb(color: Color32, factor: f32) -> Color32 {
    let channel = |value: u8| ((value as f32 * factor).round() as i32).clamp(0, 255) as u8;
    Color32::from_rgb(
        channel(color.r()),
        channel(color.g()),
        channel(color.b()),
    )
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// WCAG relative luminance of an sRGB colour.
pub fn relative_luminance(color: Color32) -> f32 {
    let channel = |value: u8| {
        let value = value as f32 / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// WCAG contrast ratio between two opaque colours.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (lighter, darker) = if la > lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

/// The pad colours for one row, used by the theme.
pub fn pad_color_index(id: PadId) -> u8 {
    id.column() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::pad::{Pads, PadId};

    #[test]
    fn text_is_readable_on_every_background_in_both_themes() {
        for theme in [Theme::stage(), Theme::high_contrast()] {
            assert!(
                contrast_ratio(theme.text, theme.bg) >= 12.0,
                "text/bg contrast is only {}",
                contrast_ratio(theme.text, theme.bg)
            );
            assert!(
                contrast_ratio(theme.text, theme.panel) >= 10.0,
                "text/panel contrast is only {}",
                contrast_ratio(theme.text, theme.panel)
            );
            assert!(
                contrast_ratio(theme.text_muted, theme.panel) >= 4.5,
                "muted text is below the 4.5:1 body-text floor"
            );
            assert!(
                contrast_ratio(theme.accent, theme.bg) >= 5.0,
                "accent is below 5:1"
            );
            for status in [theme.success, theme.warning, theme.error] {
                assert!(
                    contrast_ratio(status, theme.panel) >= 4.0,
                    "status colour {status:?} is too dim on the panel"
                );
            }
            assert!(
                contrast_ratio(theme.focus, theme.bg) >= 8.0,
                "the focus ring must be obvious on stage"
            );
        }
    }

    #[test]
    fn pad_labels_stay_readable_through_the_hit_flash() {
        let theme = Theme::stage();
        let mut pads = Pads::new();
        for id in PadId::ALL {
            pads.trigger(id, 1.0);
            let pad = pads.get(id);
            assert!(
                contrast_ratio(theme.text, theme.pad_flash(pad)) >= 4.5,
                "pad {} flash drops the label below 4.5:1",
                id.number()
            );
            assert!(
                contrast_ratio(theme.text, theme.pad_base(pad)) >= 7.0,
                "pad {} base is too light for white text",
                id.number()
            );
        }
    }

    #[test]
    fn every_pad_row_gets_a_distinct_shade() {
        let theme = Theme::stage();
        let pads = Pads::new();
        let shades: Vec<u32> = (0..4)
            .map(|row| {
                let pad = pads.get(PadId::new(row * 4));
                let color = theme.pad_base(pad);
                u32::from_be_bytes([0, color.r(), color.g(), color.b()])
            })
            .collect();
        let unique: std::collections::HashSet<_> = shades.iter().collect();
        assert_eq!(unique.len(), 4, "rows must be visually separated");
    }

    #[test]
    fn reduce_motion_damps_the_flash() {
        let mut theme = Theme::stage();
        let mut pads = Pads::new();
        pads.trigger(PadId::new(0), 1.0);
        let pad = pads.get(PadId::new(0));
        let normal = theme.pad_flash(pad);
        theme.reduce_motion = true;
        let reduced = theme.pad_flash(pad);
        assert_ne!(normal, reduced);
        assert!(
            relative_luminance(reduced) < relative_luminance(normal),
            "reduce-motion should be dimmer"
        );
    }

    #[test]
    fn selected_pads_get_the_focus_ring() {
        let theme = Theme::stage();
        let mut pads = Pads::new();
        assert_eq!(theme.pad_border(pads.get(PadId::new(0))), (theme.focus, 3.0));
        pads.select(PadId::new(1));
        assert_eq!(theme.pad_border(pads.get(PadId::new(0))).1, 1.0);
        assert_eq!(theme.pad_border(pads.get(PadId::new(1))).0, theme.focus);
    }

    #[test]
    fn contrast_ratio_is_symmetric_and_bounded() {
        let theme = Theme::stage();
        let a = contrast_ratio(theme.text, theme.bg);
        let b = contrast_ratio(theme.bg, theme.text);
        assert!((a - b).abs() < f32::EPSILON);
        assert!((1.0..=21.0).contains(&a));
        assert_eq!(contrast_ratio(theme.bg, theme.bg), 1.0);
    }

    #[test]
    fn the_two_themes_have_the_same_layout_metrics() {
        let stage = Theme::stage();
        let high = Theme::high_contrast();
        assert_eq!(stage.body, high.body);
        assert_eq!(stage.radius, high.radius);
        assert_eq!(stage.pad_bases, high.pad_bases);
    }
}
