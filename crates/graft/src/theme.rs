//! Graft's dark and light themes: palettes, text sizes and egui style overrides.

use egui::{
    CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, ThemePreference, Vec2,
};

use crate::fonts::SEMIBOLD;

/// The active palette. Every color is a function, so the whole UI follows a
/// theme switch on the next frame.
pub mod color {
    use std::sync::atomic::{AtomicBool, Ordering};

    use egui::Color32;

    const fn hex(rgb: u32) -> Color32 {
        Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    static USE_LIGHT: AtomicBool = AtomicBool::new(false);

    /// Point every color below at the light or the dark palette.
    pub fn set_light(light: bool) {
        USE_LIGHT.store(light, Ordering::Relaxed);
    }

    pub fn is_light() -> bool {
        USE_LIGHT.load(Ordering::Relaxed)
    }

    pub fn palette(light: bool) -> &'static Palette {
        if light { &LIGHT } else { &DARK }
    }

    fn active() -> &'static Palette {
        palette(is_light())
    }

    /// Declares `Palette`, the `DARK` and `LIGHT` palettes, and one function
    /// per color that reads the active palette.
    macro_rules! palette {
        ($($(#[$doc:meta])* $name:ident: $dark:literal / $light:literal,)*) => {
            pub struct Palette {
                $(pub $name: Color32,)*
            }
            pub const DARK: Palette = Palette { $($name: hex($dark),)* };
            pub const LIGHT: Palette = Palette { $($name: hex($light),)* };
            $(
                $(#[$doc])*
                #[allow(dead_code, reason = "some colors only feed the egui style")]
                pub fn $name() -> Color32 {
                    active().$name
                }
            )*
        };
    }

    palette! {
        // Surfaces. Dark: darkest to lightest; light: the reverse.
        app_bg: 0x121417 / 0xffffff,
        sidebar_bg: 0x16191d / 0xf6f7f9,
        bar_bg: 0x181b1f / 0xf3f4f6,
        control_bg: 0x22262c / 0xffffff,
        control_hover_bg: 0x2a2f36 / 0xeef0f3,
        control_active_bg: 0x323840 / 0xe3e7eb,
        selected_row_bg: 0x1f2e3d / 0xdbe9f8,
        hover_row_bg: 0x1c2025 / 0xeff1f4,

        border: 0x2a2f36 / 0xdfe3e8,
        control_border: 0x2f343b / 0xcdd3da,
        control_hover_border: 0x3a4048 / 0xadb5bf,

        text: 0xe4e7eb / 0x1f2328,
        text_mid: 0xc3cad3 / 0x3b424a,
        text_weak: 0xa3abb5 / 0x58616b,
        text_faint: 0x7d8590 / 0x7f8893,
        /// Text on a hovered or pressed control.
        text_strong: 0xffffff / 0x000000,
        /// Background of selected text.
        selection_bg: 0x264f78 / 0xb9d6fb,

        accent: 0x8fd19e / 0x1a7f37,
        accent_hover: 0xa3dcb0 / 0x218c40,
        /// Text on an accent-filled button.
        on_accent: 0x0b1a0f / 0xffffff,

        /// Strip of project tabs above the toolbar.
        tabstrip_bg: 0x0e1013 / 0xe7eaee,

        // Badges: dirty count (orange), ahead count (blue), neutral count.
        dirty_bg: 0x3a2a1a / 0xfdebd7,
        dirty_fg: 0xf2b77a / 0xa44f00,
        ahead_bg: 0x1b2838 / 0xdcecfd,
        ahead_fg: 0x8cc2ff / 0x0b62c9,
        neutral_badge_bg: 0x2f343b / 0xe6e9ed,
        neutral_badge_fg: 0xc9d1d9 / 0x3b424a,

        // The Slash button is the one destructive action in the toolbar.
        slash_bg: 0x2a1f17 / 0xfff3e8,
        slash_hover_bg: 0x352619 / 0xffe6cf,
        slash_border: 0x5a3b22 / 0xefbb8c,
        slash_fg: 0xf2b77a / 0xa44f00,

        branch_dot_agent: 0xf2a65a / 0xc05a00,

        removed: 0xff9a8f / 0xc8232f,
        added: 0x7ee08f / 0x1a7f37,
        tag: 0xf5d76e / 0x8a6200,

        // Syntax colors in diffs.
        syn_keyword: 0xd6a3ff / 0x8250df,
        syn_string: 0xe5b07a / 0x0a3069,
        syn_comment: 0x7d8590 / 0x6e7781,
        syn_number: 0x7fd8d0 / 0x0550ae,
        syn_function: 0x8cc2ff / 0x0b62c9,
        syn_type: 0xf5d76e / 0x953800,
    }

    const DARK_LANES: [Color32; 8] = [
        hex(0x8fd19e),
        hex(0x8cc2ff),
        hex(0xf2b77a),
        hex(0xd6a3ff),
        hex(0xff9a8f),
        hex(0x7fd8d0),
        hex(0xf5d76e),
        hex(0xf39ac7),
    ];
    const LIGHT_LANES: [Color32; 8] = [
        hex(0x1a7f37),
        hex(0x0b62c9),
        hex(0xbc5400),
        hex(0x8250df),
        hex(0xc8232f),
        hex(0x167a80),
        hex(0x8a6200),
        hex(0xbf3989),
    ];

    /// Commit graph lanes, assigned round-robin.
    pub fn lanes() -> [Color32; 8] {
        if is_light() { LIGHT_LANES } else { DARK_LANES }
    }
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

/// Install both styles and pick the theme; `System` follows Windows.
pub fn apply(ctx: &egui::Context, preference: ThemePreference) {
    ctx.set_style_of(Theme::Dark, style(Theme::Dark));
    ctx.set_style_of(Theme::Light, style(Theme::Light));
    ctx.set_theme(preference);
    sync(ctx);
}

/// Point `color::*` at the theme egui is using. Call at the start of every
/// frame, since Windows can switch under a `System` preference. True when the
/// theme changed.
pub fn sync(ctx: &egui::Context) -> bool {
    let light = ctx.theme() == Theme::Light;
    let changed = light != color::is_light();
    color::set_light(light);
    changed
}

fn style(theme: Theme) -> egui::Style {
    let p = color::palette(theme == Theme::Light);

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
    *v = theme.default_visuals();
    v.panel_fill = p.app_bg;
    v.window_fill = p.bar_bg;
    v.window_stroke = Stroke::new(1.0, p.control_border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.extreme_bg_color = p.app_bg;
    v.text_edit_bg_color = Some(p.app_bg);
    v.faint_bg_color = p.sidebar_bg;
    v.code_bg_color = p.sidebar_bg;
    v.hyperlink_color = p.ahead_fg;
    v.warn_fg_color = p.dirty_fg;
    v.error_fg_color = p.removed;
    v.weak_text_color = Some(p.text_faint);
    v.selection.bg_fill = p.selection_bg;
    v.selection.stroke = Stroke::new(1.0, p.text);

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bar_bg;
    w.noninteractive.weak_bg_fill = p.bar_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = p.control_bg;
    w.inactive.weak_bg_fill = p.control_bg;
    w.inactive.bg_stroke = Stroke::new(1.0, p.control_border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = p.control_hover_bg;
    w.hovered.weak_bg_fill = p.control_hover_bg;
    w.hovered.bg_stroke = Stroke::new(1.0, p.control_hover_border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.control_active_bg;
    w.active.weak_bg_fill = p.control_active_bg;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open = w.active;

    style
}
