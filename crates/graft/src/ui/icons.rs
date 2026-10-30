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
    Minus,
    Close,
    Download,
    FolderPlus,
    Copy,
    Terminal,
    Tag,
    Cloud,
    ChevronRight,
    ChevronLeft,
    Merge,
    Settings,
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
        Icon::Minus => line(&[(5.0, 12.0), (19.0, 12.0)]),
        Icon::Settings => {
            circle(12.0, 12.0, 2.5);
            circle(12.0, 12.0, 6.5);
            for k in 0..6 {
                let (s, c) = (k as f32 * std::f32::consts::TAU / 6.0).sin_cos();
                line(&[
                    (12.0 + 6.5 * c, 12.0 + 6.5 * s),
                    (12.0 + 9.5 * c, 12.0 + 9.5 * s),
                ]);
            }
        }
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Download => {
            line(&[(12.0, 3.0), (12.0, 15.0)]);
            line(&[(7.0, 10.0), (12.0, 15.0), (17.0, 10.0)]);
            line(&[(4.0, 16.0), (4.0, 20.0), (20.0, 20.0), (20.0, 16.0)]);
        }
        Icon::FolderPlus => {
            line(&[
                (3.0, 7.0),
                (9.0, 7.0),
                (11.0, 9.0),
                (21.0, 9.0),
                (21.0, 19.0),
                (3.0, 19.0),
                (3.0, 7.0),
            ]);
            line(&[(12.0, 11.5), (12.0, 16.5)]);
            line(&[(9.5, 14.0), (14.5, 14.0)]);
        }
        Icon::Copy => {
            painter.rect_stroke(
                Rect::from_min_max(p(8.0, 8.0), p(20.0, 20.0)),
                2.0 * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(16.0, 4.0), (4.0, 4.0), (4.0, 16.0)]);
        }
        Icon::Terminal => {
            line(&[(4.0, 7.0), (9.0, 12.0), (4.0, 17.0)]);
            line(&[(12.0, 18.0), (20.0, 18.0)]);
        }
        Icon::Tag => {
            line(&[
                (3.0, 3.0),
                (11.0, 3.0),
                (21.0, 13.0),
                (13.0, 21.0),
                (3.0, 11.0),
                (3.0, 3.0),
            ]);
            circle(7.5, 7.5, 1.2);
        }
        Icon::Cloud => {
            let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<Pos2> {
                (0..=12)
                    .map(|i| (from + (to - from) * i as f32 / 12.0).to_radians())
                    .map(|a| p(cx + r * a.cos(), cy + r * a.sin()))
                    .collect()
            };
            let mut pts = arc(7.0, 14.0, 4.0, 90.0, 270.0);
            pts.extend(arc(12.0, 10.0, 5.5, 200.0, 340.0));
            pts.extend(arc(17.0, 14.0, 4.0, 270.0, 450.0));
            pts.push(pts[0]);
            painter.add(Shape::line(pts, stroke));
        }
        Icon::ChevronRight => line(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]),
        Icon::ChevronLeft => line(&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)]),
        Icon::Merge => {
            circle(7.0, 5.0, 2.2);
            circle(7.0, 19.0, 2.2);
            circle(17.0, 12.0, 2.2);
            line(&[(7.0, 7.2), (7.0, 16.8)]);
            line(&[(7.0, 7.2), (7.0, 9.0), (14.8, 12.0)]);
        }
    }
}
