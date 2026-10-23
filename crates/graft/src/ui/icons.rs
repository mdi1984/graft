//! Small stroke icons, painted in a 24×24 design grid scaled to the target rect.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Commit,
    Pull,
    Push,
    Fetch,
    Slash,
    Search,
    Folder,
    ChevronDown,
    FileStatus,
    History,
    Plus,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let scale = rect.width() / 24.0;
    let p = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
    let stroke = Stroke::new(1.5, color);
    let line = |pts: &[(f32, f32)]| {
        painter.add(Shape::line(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(p(x, y), r * scale, stroke);
    };

    match icon {
        Icon::Commit => {
            circle(12.0, 12.0, 4.0);
            line(&[(2.0, 12.0), (8.0, 12.0)]);
            line(&[(16.0, 12.0), (22.0, 12.0)]);
        }
        Icon::Pull => {
            line(&[(12.0, 4.0), (12.0, 18.0)]);
            line(&[(6.0, 12.0), (12.0, 18.0), (18.0, 12.0)]);
        }
        Icon::Push => {
            line(&[(12.0, 20.0), (12.0, 6.0)]);
            line(&[(6.0, 12.0), (12.0, 6.0), (18.0, 12.0)]);
        }
        Icon::Fetch => {
            // A clockwise arc from 3 o'clock round to the top right, then an arrow head.
            let arc: Vec<Pos2> = (0..=26)
                .map(|i| (i as f32 * 12.0).to_radians())
                .map(|a| p(12.0 + 9.0 * a.cos(), 12.0 + 9.0 * a.sin()))
                .chain([p(21.0, 8.0)])
                .collect();
            painter.add(Shape::line(arc, stroke));
            line(&[(21.0, 3.0), (21.0, 8.0), (16.0, 8.0)]);
        }
        Icon::Slash => {
            line(&[(4.0, 20.0), (20.0, 4.0)]);
            line(&[(14.0, 4.0), (20.0, 4.0), (20.0, 10.0)]);
        }
        Icon::Search => {
            circle(11.0, 11.0, 7.0);
            line(&[(20.0, 20.0), (16.0, 16.0)]);
        }
        Icon::Folder => {
            line(&[
                (3.0, 7.0),
                (9.0, 7.0),
                (11.0, 9.0),
                (21.0, 9.0),
                (21.0, 19.0),
                (3.0, 19.0),
                (3.0, 7.0),
            ]);
        }
        Icon::ChevronDown => line(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]),
        Icon::FileStatus => {
            painter.rect_stroke(
                Rect::from_min_max(p(4.0, 4.0), p(20.0, 20.0)),
                2.0 * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(8.0, 12.0), (16.0, 12.0)]);
            line(&[(12.0, 8.0), (12.0, 16.0)]);
        }
        Icon::History => {
            circle(12.0, 12.0, 8.0);
            line(&[(12.0, 8.0), (12.0, 12.0), (15.0, 14.0)]);
        }
        Icon::Plus => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
    }
}
