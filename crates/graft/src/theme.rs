//! Graft's dark theme: palette, text sizes and egui style overrides.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Vec2};

use crate::fonts::SEMIBOLD;

pub mod color {
    use egui::Color32;

    const fn hex(rgb: u32) -> Color32 {
        Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    // Surfaces, darkest to lightest.
    pub const APP_BG: Color32 = hex(0x121417);
    pub const SIDEBAR_BG: Color32 = hex(0x16191d);
    pub const BAR_BG: Color32 = hex(0x181b1f);
    pub const CONTROL_BG: Color32 = hex(0x22262c);
    pub const CONTROL_HOVER_BG: Color32 = hex(0x2a2f36);
    pub const CONTROL_ACTIVE_BG: Color32 = hex(0x323840);
    pub const SELECTED_ROW_BG: Color32 = hex(0x1f2e3d);
    pub const HOVER_ROW_BG: Color32 = hex(0x1c2025);

    pub const BORDER: Color32 = hex(0x2a2f36);
    pub const CONTROL_BORDER: Color32 = hex(0x2f343b);

    pub const TEXT: Color32 = hex(0xe4e7eb);
    pub const TEXT_MID: Color32 = hex(0xc3cad3);
    pub const TEXT_WEAK: Color32 = hex(0xa3abb5);
    pub const TEXT_FAINT: Color32 = hex(0x7d8590);

    pub const ACCENT: Color32 = hex(0x8fd19e);
    pub const ACCENT_HOVER: Color32 = hex(0xa3dcb0);
    /// Text on an accent-filled button.
    pub const ON_ACCENT: Color32 = hex(0x0b1a0f);

    /// Strip of project tabs above the toolbar.
    pub const TABSTRIP_BG: Color32 = hex(0x0e1013);

    // Badges: dirty count (orange), ahead count (blue), neutral count.
    pub const DIRTY_BG: Color32 = hex(0x3a2a1a);
    pub const DIRTY_FG: Color32 = hex(0xf2b77a);
    pub const AHEAD_BG: Color32 = hex(0x1b2838);
    pub const AHEAD_FG: Color32 = hex(0x8cc2ff);
    pub const NEUTRAL_BADGE_BG: Color32 = hex(0x2f343b);
    pub const NEUTRAL_BADGE_FG: Color32 = hex(0xc9d1d9);

    // The Slash button is the one destructive action in the toolbar.
    pub const SLASH_BG: Color32 = hex(0x2a1f17);
    pub const SLASH_HOVER_BG: Color32 = hex(0x352619);
    pub const SLASH_BORDER: Color32 = hex(0x5a3b22);
    pub const SLASH_FG: Color32 = hex(0xf2b77a);

    pub const BRANCH_DOT_AGENT: Color32 = hex(0xf2a65a);

    pub const REMOVED: Color32 = hex(0xff9a8f);
    pub const ADDED: Color32 = hex(0x7ee08f);
    pub const TAG: Color32 = hex(0xf5d76e);

    /// Commit graph lanes, assigned round-robin.
    pub const LANES: [Color32; 8] = [
        hex(0x8fd19e),
        hex(0x8cc2ff),
        hex(0xf2b77a),
        hex(0xd6a3ff),
        hex(0xff9a8f),
        hex(0x7fd8d0),
        hex(0xf5d76e),
        hex(0xf39ac7),
    ];
}

pub const RADIUS: u8 = 6;
pub const BODY_SIZE: f32 = 13.0;
pub const SMALL_SIZE: f32 = 11.0;
pub const MONO_SIZE: f32 = 12.0;

pub fn body() -> FontId {
    FontId::proportional(BODY_SIZE)
}

pub fn small() -> FontId {
    FontId::proportional(SMALL_SIZE)
}

pub fn mono() -> FontId {
    FontId::monospace(MONO_SIZE)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Apply the theme to every egui style. Graft is dark-only for now.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(Theme::Dark);
    ctx.set_style_of(Theme::Dark, style());
}

fn style() -> egui::Style {
    use color::*;

    let mut style = egui::Style {
        text_styles: [
            (TextStyle::Small, small()),
            (TextStyle::Body, body()),
            (TextStyle::Button, body()),
            (TextStyle::Monospace, mono()),
            (TextStyle::Heading, semibold(16.0)),
        ]
        .into(),
        ..Default::default()
    };

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(6.0, 4.0);
    s.button_padding = Vec2::new(10.0, 5.0);
    s.interact_size = Vec2::new(24.0, 24.0);
    s.window_margin = Margin::same(12);
    s.menu_margin = Margin::same(6);

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = APP_BG;
    v.window_fill = BAR_BG;
    v.window_stroke = Stroke::new(1.0, CONTROL_BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.extreme_bg_color = APP_BG;
    v.text_edit_bg_color = Some(APP_BG);
    v.faint_bg_color = SIDEBAR_BG;
    v.code_bg_color = SIDEBAR_BG;
    v.hyperlink_color = AHEAD_FG;
    v.warn_fg_color = DIRTY_FG;
    v.error_fg_color = REMOVED;
    v.weak_text_color = Some(TEXT_FAINT);
    v.selection.bg_fill = Color32::from_rgb(0x26, 0x4f, 0x78);
    v.selection.stroke = Stroke::new(1.0, TEXT);

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BAR_BG;
    w.noninteractive.weak_bg_fill = BAR_BG;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = CONTROL_BG;
    w.inactive.weak_bg_fill = CONTROL_BG;
    w.inactive.bg_stroke = Stroke::new(1.0, CONTROL_BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = CONTROL_HOVER_BG;
    w.hovered.weak_bg_fill = CONTROL_HOVER_BG;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x40, 0x48));
    w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = CONTROL_ACTIVE_BG;
    w.active.weak_bg_fill = CONTROL_ACTIVE_BG;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open = w.active;

    style
}
