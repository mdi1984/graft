//! Graft draws its own window frame: the project tab strip doubles as the
//! title bar (app icon, drag to move, double-click to maximize) with
//! minimize / maximize / close at the right, and the window edges resize.

use egui::{
    Color32, Context, CursorIcon, Rect, ResizeDirection, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, ViewportCommand, pos2, vec2,
};

use crate::theme::color;

/// Width of each caption button, like Windows' own.
pub const BUTTON_WIDTH: f32 = 46.0;
/// All three caption buttons.
pub const BUTTONS_WIDTH: f32 = BUTTON_WIDTH * 3.0;
/// How far from the window edge the pointer resizes instead of clicking.
const EDGE: f32 = 5.0;
/// Corners grab a little more, so diagonal resizing is easy to hit.
const CORNER: f32 = 12.0;
const CLOSE_HOVER_BG: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

fn maximized(ctx: &Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

fn fullscreen(ctx: &Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

/// The app icon, painted from a pre-rendered PNG (mipmapped, so it stays
/// crisp at any display scale).
pub fn paint_app_icon(ui: &Ui, rect: Rect) {
    let id = egui::Id::unique("graft-app-icon");
    let texture = ui
        .data_mut(|d| d.get_temp::<TextureHandle>(id))
        .unwrap_or_else(|| {
            let image = image_from_png(include_bytes!("../../assets/icon/graft-title.png"));
            let texture = ui.ctx().load_texture(
                "graft-app-icon",
                image,
                TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear)),
            );
            ui.data_mut(|d| d.insert_temp(id, texture.clone()));
            texture
        });
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn image_from_png(bytes: &[u8]) -> egui::ColorImage {
    let icon = eframe::icon_data::from_png_bytes(bytes).expect("bundled icon is a valid PNG");
    egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    )
}

/// Empty title bar space: drag to move the window, double-click to maximize.
/// Call before adding the widgets on top of it, so they take precedence.
pub fn drag_area(ui: &Ui, rect: Rect) {
    let response = ui.interact(
        rect,
        ui.scope_id().with("window-drag"),
        Sense::click_and_drag(),
    );
    let ctx = ui.ctx();
    if response.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
    } else if response.drag_started_by(egui::PointerButton::Primary) && edge_at(ctx).is_none() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Minimize, maximize/restore and close, filling `rect` from the right.
/// Their glyphs sit on the line `glyph_y`, level with the tab titles.
pub fn caption_buttons(ui: &Ui, rect: Rect, glyph_y: f32) {
    let ctx = ui.ctx().clone();
    let is_max = maximized(&ctx);
    let buttons = [
        (Glyph::Minimize, "Minimize"),
        (
            if is_max {
                Glyph::Restore
            } else {
                Glyph::Maximize
            },
            if is_max { "Restore" } else { "Maximize" },
        ),
        (Glyph::Close, "Close"),
    ];
    for (i, (glyph, tip)) in buttons.into_iter().enumerate() {
        let left = rect.right() - BUTTONS_WIDTH + i as f32 * BUTTON_WIDTH;
        let button = Rect::from_min_size(pos2(left, rect.top()), vec2(BUTTON_WIDTH, rect.height()));
        let response = ui
            .interact(button, ui.scope_id().with(("caption", i)), Sense::click())
            .on_hover_text(tip);
        let hovered = response.hovered();
        let (bg, fg) = match (glyph, hovered) {
            (Glyph::Close, true) => (Some(CLOSE_HOVER_BG), Color32::WHITE),
            (_, true) => (Some(color::CONTROL_HOVER_BG), color::TEXT),
            _ => (None, color::TEXT_WEAK),
        };
        if let Some(bg) = bg {
            ui.painter().rect_filled(button, 0, bg);
        }
        paint_glyph(ui, pos2(button.center().x, glyph_y), glyph, fg);
        if response.clicked() {
            match glyph {
                Glyph::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                Glyph::Maximize => ctx.send_viewport_cmd(ViewportCommand::Maximized(true)),
                Glyph::Restore => ctx.send_viewport_cmd(ViewportCommand::Maximized(false)),
                Glyph::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Thin 10 px glyphs like the Windows 11 caption buttons, snapped to pixels.
fn paint_glyph(ui: &Ui, center: egui::Pos2, glyph: Glyph, fg: Color32) {
    let painter = ui.painter();
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp + 0.5 / ppp;
    let stroke = Stroke::new(1.0, fg);
    let c = pos2(snap(center.x), snap(center.y));
    let h = 5.0;
    match glyph {
        Glyph::Minimize => {
            painter.hline((c.x - h)..=(c.x + h), c.y, stroke);
        }
        Glyph::Maximize => {
            let r = Rect::from_center_size(c, vec2(2.0 * h, 2.0 * h));
            painter.rect_stroke(r, 1, stroke, egui::StrokeKind::Middle);
        }
        Glyph::Restore => {
            let front = Rect::from_min_size(pos2(c.x - h, c.y - h + 2.0), vec2(8.0, 8.0));
            painter.rect_stroke(front, 1, stroke, egui::StrokeKind::Middle);
            // The window behind: only its top and right edges show.
            painter.line(
                vec![
                    pos2(c.x - h + 2.0, c.y - h),
                    pos2(c.x + h, c.y - h),
                    pos2(c.x + h, c.y + h - 2.0),
                ],
                stroke,
            );
        }
        Glyph::Close => {
            painter.line_segment([pos2(c.x - h, c.y - h), pos2(c.x + h, c.y + h)], stroke);
            painter.line_segment([pos2(c.x + h, c.y - h), pos2(c.x - h, c.y + h)], stroke);
        }
    }
}

/// The window edge under the pointer, if it is close enough to resize.
fn edge_at(ctx: &Context) -> Option<ResizeDirection> {
    if maximized(ctx) || fullscreen(ctx) {
        return None;
    }
    let pos = ctx.input(|i| i.pointer.latest_pos())?;
    let screen = ctx.content_rect();
    let near = |d: f32, reach: f32| d <= reach;
    let (l, r) = (pos.x - screen.left(), screen.right() - pos.x);
    let (t, b) = (pos.y - screen.top(), screen.bottom() - pos.y);
    let dir = match () {
        _ if near(t, CORNER) && near(l, CORNER) => ResizeDirection::NorthWest,
        _ if near(t, CORNER) && near(r, CORNER) => ResizeDirection::NorthEast,
        _ if near(b, CORNER) && near(l, CORNER) => ResizeDirection::SouthWest,
        _ if near(b, CORNER) && near(r, CORNER) => ResizeDirection::SouthEast,
        _ if near(t, EDGE) => ResizeDirection::North,
        _ if near(b, EDGE) => ResizeDirection::South,
        _ if near(l, EDGE) => ResizeDirection::West,
        _ if near(r, EDGE) => ResizeDirection::East,
        _ => return None,
    };
    // Corners only count right at the edge on at least one side.
    let corner = matches!(
        dir,
        ResizeDirection::NorthWest
            | ResizeDirection::NorthEast
            | ResizeDirection::SouthWest
            | ResizeDirection::SouthEast
    );
    if corner && t.min(b).min(l).min(r) > EDGE {
        return None;
    }
    Some(dir)
}

/// Resize cursors at the window edges; pressing there starts a resize.
/// Call once per frame, after everything else is laid out.
pub fn resize_edges(ctx: &Context) {
    let Some(dir) = edge_at(ctx) else { return };
    // Don't fight a drag that started inside the window.
    if ctx.input(|i| i.pointer.any_down() && !i.pointer.any_pressed()) {
        return;
    }
    ctx.set_cursor_icon(match dir {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}
