//! BPMN markers drawn as vector shapes (egui's default font has no glyphs for them).

use std::f32::consts::{PI, TAU};

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Envelope; filled for throwing/sending.
    Message {
        filled: bool,
    },
    Timer,
    /// Triangle; filled for throwing.
    Signal {
        filled: bool,
    },
    Link {
        filled: bool,
    },
    Error,
    Escalation,
    Terminate,
    User,
    Service,
    Script,
    BusinessRule,
    Manual,
    /// Collapsed marker (call activity).
    Plus,
}

fn polygon(painter: &Painter, pts: Vec<Pos2>, fill: Color32, stroke: Stroke) {
    painter.add(Shape::convex_polygon(pts, fill, stroke));
}

/// Draw `icon` centred at `c` in a square of side `s`.
pub fn draw(painter: &Painter, icon: Icon, c: Pos2, s: f32, ink: Color32, paper: Color32) {
    let st = Stroke::new((s / 14.0).clamp(0.6, 2.0), ink);
    let p = |x: f32, y: f32| c + Vec2::new(x * s, y * s);
    match icon {
        Icon::Message { filled } => {
            let r = Rect::from_center_size(c, Vec2::new(0.8 * s, 0.55 * s));
            let line = if filled {
                Stroke::new(st.width, paper)
            } else {
                st
            };
            painter.rect(
                r,
                0.0,
                if filled { ink } else { Color32::TRANSPARENT },
                st,
                eframe::egui::StrokeKind::Middle,
            );
            painter.line_segment([r.left_top(), r.center()], line);
            painter.line_segment([r.right_top(), r.center()], line);
        }
        Icon::Timer => {
            painter.circle_stroke(c, 0.4 * s, st);
            for k in 0..12 {
                let a = k as f32 * TAU / 12.0;
                let d = Vec2::angled(a);
                painter.line_segment([c + d * 0.32 * s, c + d * 0.4 * s], st);
            }
            painter.line_segment([c, p(0.0, -0.28)], st);
            painter.line_segment([c, p(0.18, 0.05)], st);
        }
        Icon::Signal { filled } => {
            polygon(
                painter,
                vec![p(0.0, -0.4), p(0.38, 0.3), p(-0.38, 0.3)],
                if filled { ink } else { Color32::TRANSPARENT },
                st,
            );
        }
        Icon::Link { filled } => {
            let pts = vec![
                p(-0.4, -0.12),
                p(0.05, -0.12),
                p(0.05, -0.32),
                p(0.42, 0.0),
                p(0.05, 0.32),
                p(0.05, 0.12),
                p(-0.4, 0.12),
            ];
            painter.add(Shape::closed_line(pts.clone(), st));
            if filled {
                polygon(
                    painter,
                    vec![p(0.05, -0.32), p(0.42, 0.0), p(0.05, 0.32)],
                    ink,
                    st,
                );
                painter.rect_filled(Rect::from_min_max(p(-0.4, -0.12), p(0.05, 0.12)), 0.0, ink);
            }
        }
        Icon::Error => {
            let pts = vec![
                p(-0.35, 0.38),
                p(-0.12, -0.3),
                p(0.08, 0.05),
                p(0.35, -0.38),
                p(0.12, 0.3),
                p(-0.08, -0.05),
            ];
            painter.add(Shape::closed_line(pts, st));
        }
        Icon::Escalation => {
            painter.add(Shape::closed_line(
                vec![p(0.0, -0.4), p(0.3, 0.38), p(0.0, 0.1), p(-0.3, 0.38)],
                st,
            ));
        }
        Icon::Terminate => {
            painter.circle_filled(c, 0.32 * s, ink);
        }
        Icon::User => {
            painter.circle_stroke(p(0.0, -0.2), 0.17 * s, st);
            let n = 16;
            let body: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = PI + PI * i as f32 / n as f32;
                    p(0.32 * a.cos(), 0.38 + 0.32 * a.sin())
                })
                .collect();
            painter.add(Shape::closed_line(body, st));
        }
        Icon::Service => {
            for k in 0..8 {
                let d = Vec2::angled(k as f32 * TAU / 8.0);
                painter.line_segment(
                    [c + d * 0.25 * s, c + d * 0.42 * s],
                    Stroke::new(st.width * 2.2, ink),
                );
            }
            painter.circle(c, 0.28 * s, paper, st);
            painter.circle_stroke(c, 0.1 * s, st);
        }
        Icon::Script => {
            let r = Rect::from_center_size(c, Vec2::new(0.6 * s, 0.8 * s));
            painter.rect_stroke(r, 0.0, st, eframe::egui::StrokeKind::Middle);
            for y in [-0.2, 0.0, 0.2] {
                painter.line_segment([p(-0.18, y), p(0.18, y)], st);
            }
        }
        Icon::BusinessRule => {
            let r = Rect::from_center_size(c, Vec2::new(0.8 * s, 0.6 * s));
            let header = Rect::from_min_max(r.left_top(), Pos2::new(r.right(), r.top() + 0.18 * s));
            painter.rect_filled(header, 0.0, ink);
            painter.rect_stroke(r, 0.0, st, eframe::egui::StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(r.left() + 0.28 * s, header.bottom()),
                    Pos2::new(r.left() + 0.28 * s, r.bottom()),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(r.left(), r.top() + 0.39 * s),
                    Pos2::new(r.right(), r.top() + 0.39 * s),
                ],
                st,
            );
        }
        Icon::Manual => {
            // A flat hand pointing right: palm, four fingers of different length, thumb.
            let palm = Rect::from_min_max(p(-0.45, -0.12), p(-0.05, 0.34));
            painter.rect_stroke(palm, 0.08 * s, st, eframe::egui::StrokeKind::Middle);
            for (y, len) in [(-0.06, 0.42), (0.06, 0.48), (0.18, 0.44), (0.29, 0.34)] {
                painter.line_segment([p(-0.05, y), p(-0.05 + len, y)], st);
            }
            painter.add(Shape::line(
                vec![
                    p(-0.3, -0.12),
                    p(-0.05, -0.38),
                    p(0.12, -0.3),
                    p(-0.05, -0.12),
                ],
                st,
            ));
        }
        Icon::Plus => {
            let r = Rect::from_center_size(c, Vec2::splat(0.8 * s));
            painter.rect_stroke(r, 0.0, st, eframe::egui::StrokeKind::Middle);
            painter.line_segment([p(-0.25, 0.0), p(0.25, 0.0)], st);
            painter.line_segment([p(0.0, -0.25), p(0.0, 0.25)], st);
        }
    }
}
