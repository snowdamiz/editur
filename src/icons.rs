//! Every glyph in the product, authored once on a 16 px grid at one stroke
//! width. Icons are paths rather than pairs of independent line segments, so a
//! chevron's two arms join at a mitre instead of stacking two caps into a
//! thickened notch.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, epaint::PathShape};

use crate::theme;

/// The box every icon is authored inside. Anything drawn at another size is
/// this geometry scaled, never redrawn.
pub(crate) const GRID: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Icon {
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Close,
    Check,
    Plus,
    Minus,
    #[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
    Square,
    Stop,
    Folder,
    File,
    Home,
    Search,
    Terminal,
    SourceControl,
    Gear,
    ArrowUp,
    Download,
    Refresh,
    History,
    Warning,
    Error,
    Info,
    Robot,
    Sparkle,
    Bolt,
    Ellipsis,
}

/// One piece of an icon, in grid coordinates.
enum Segment {
    /// A stroked polyline with mitred joins.
    Line(&'static [(f32, f32)]),
    /// A stroked closed path.
    Outline(&'static [(f32, f32)]),
    /// A solid closed path.
    Solid(&'static [(f32, f32)]),
    Circle {
        center: (f32, f32),
        radius: f32,
    },
    Dot {
        center: (f32, f32),
        radius: f32,
    },
    /// A ring of radial teeth, for the one glyph that needs them.
    Teeth {
        count: usize,
        inner: f32,
        outer: f32,
    },
}

const CHEVRON_UP: &[Segment] = &[Segment::Line(&[(4.0, 9.8), (8.0, 5.6), (12.0, 9.8)])];
const CHEVRON_DOWN: &[Segment] = &[Segment::Line(&[(4.0, 6.2), (8.0, 10.4), (12.0, 6.2)])];
const CHEVRON_LEFT: &[Segment] = &[Segment::Line(&[(9.8, 4.0), (5.6, 8.0), (9.8, 12.0)])];
const CHEVRON_RIGHT: &[Segment] = &[Segment::Line(&[(6.2, 4.0), (10.4, 8.0), (6.2, 12.0)])];
const CLOSE: &[Segment] = &[
    Segment::Line(&[(4.2, 4.2), (11.8, 11.8)]),
    Segment::Line(&[(11.8, 4.2), (4.2, 11.8)]),
];
const CHECK: &[Segment] = &[Segment::Line(&[(3.2, 8.6), (6.4, 11.8), (12.8, 4.6)])];
const PLUS: &[Segment] = &[
    Segment::Line(&[(8.0, 3.2), (8.0, 12.8)]),
    Segment::Line(&[(3.2, 8.0), (12.8, 8.0)]),
];
const MINUS: &[Segment] = &[Segment::Line(&[(3.2, 8.0), (12.8, 8.0)])];
const SQUARE: &[Segment] = &[Segment::Outline(&[
    (3.8, 3.8),
    (12.2, 3.8),
    (12.2, 12.2),
    (3.8, 12.2),
])];
const STOP: &[Segment] = &[Segment::Solid(&[
    (4.6, 4.6),
    (11.4, 4.6),
    (11.4, 11.4),
    (4.6, 11.4),
])];
const FOLDER: &[Segment] = &[Segment::Outline(&[
    (2.4, 13.0),
    (2.4, 3.8),
    (6.2, 3.8),
    (7.7, 5.8),
    (13.6, 5.8),
    (13.6, 13.0),
])];
const FILE: &[Segment] = &[
    Segment::Outline(&[
        (3.8, 2.4),
        (9.4, 2.4),
        (12.2, 5.2),
        (12.2, 13.6),
        (3.8, 13.6),
    ]),
    Segment::Line(&[(9.4, 2.4), (9.4, 5.2), (12.2, 5.2)]),
];
const HOME: &[Segment] = &[
    Segment::Outline(&[
        (3.2, 7.0),
        (8.0, 3.0),
        (12.8, 7.0),
        (12.8, 13.0),
        (3.2, 13.0),
    ]),
    Segment::Line(&[(6.6, 13.0), (6.6, 9.6), (9.4, 9.6), (9.4, 13.0)]),
];
const SEARCH: &[Segment] = &[
    Segment::Circle {
        center: (7.0, 7.0),
        radius: 3.6,
    },
    Segment::Line(&[(9.7, 9.7), (12.6, 12.6)]),
];
const TERMINAL: &[Segment] = &[
    Segment::Outline(&[(2.2, 3.4), (13.8, 3.4), (13.8, 12.6), (2.2, 12.6)]),
    Segment::Line(&[(4.8, 6.6), (7.0, 8.4), (4.8, 10.2)]),
    Segment::Line(&[(8.4, 10.2), (11.4, 10.2)]),
];
const SOURCE_CONTROL: &[Segment] = &[
    Segment::Circle {
        center: (4.0, 4.0),
        radius: 1.6,
    },
    Segment::Circle {
        center: (4.0, 12.0),
        radius: 1.6,
    },
    Segment::Circle {
        center: (12.0, 4.0),
        radius: 1.6,
    },
    Segment::Line(&[(4.0, 5.6), (4.0, 10.4)]),
    Segment::Line(&[(5.6, 4.0), (10.4, 4.0)]),
];
const GEAR: &[Segment] = &[
    Segment::Circle {
        center: (8.0, 8.0),
        radius: 4.0,
    },
    Segment::Circle {
        center: (8.0, 8.0),
        radius: 1.4,
    },
    Segment::Teeth {
        count: 8,
        inner: 4.0,
        outer: 6.4,
    },
];
const ARROW_UP: &[Segment] = &[
    Segment::Line(&[(8.0, 12.8), (8.0, 3.4)]),
    Segment::Line(&[(3.9, 7.5), (8.0, 3.4), (12.1, 7.5)]),
];
const DOWNLOAD: &[Segment] = &[
    Segment::Line(&[(8.0, 2.6), (8.0, 10.2)]),
    Segment::Line(&[(4.2, 6.4), (8.0, 10.2), (11.8, 6.4)]),
    Segment::Line(&[(3.2, 13.2), (12.8, 13.2)]),
];
const REFRESH: &[Segment] = &[
    Segment::Line(&[
        (3.0, 6.0),
        (3.7, 4.5),
        (5.0, 3.3),
        (6.6, 2.7),
        (8.3, 2.7),
        (10.1, 3.2),
        (11.5, 4.3),
        (12.3, 5.6),
    ]),
    Segment::Line(&[(9.8, 5.6), (12.3, 5.6), (12.3, 3.1)]),
    Segment::Line(&[
        (13.0, 10.0),
        (12.3, 11.5),
        (11.0, 12.7),
        (9.4, 13.3),
        (7.7, 13.3),
        (5.9, 12.8),
        (4.5, 11.7),
        (3.7, 10.4),
    ]),
    Segment::Line(&[(6.2, 10.4), (3.7, 10.4), (3.7, 12.9)]),
];
const HISTORY: &[Segment] = &[
    Segment::Circle {
        center: (8.0, 8.0),
        radius: 5.4,
    },
    Segment::Line(&[(8.0, 4.4), (8.0, 8.0), (10.8, 9.4)]),
];
const WARNING: &[Segment] = &[
    Segment::Outline(&[(8.0, 2.4), (14.8, 13.4), (1.2, 13.4)]),
    Segment::Line(&[(8.0, 6.4), (8.0, 9.6)]),
    Segment::Dot {
        center: (8.0, 11.6),
        radius: 0.8,
    },
];
const ERROR: &[Segment] = &[
    Segment::Circle {
        center: (8.0, 8.0),
        radius: 5.8,
    },
    Segment::Line(&[(5.8, 5.8), (10.2, 10.2)]),
    Segment::Line(&[(10.2, 5.8), (5.8, 10.2)]),
];
const INFO: &[Segment] = &[
    Segment::Circle {
        center: (8.0, 8.0),
        radius: 5.8,
    },
    Segment::Line(&[(8.0, 7.4), (8.0, 11.2)]),
    Segment::Dot {
        center: (8.0, 5.2),
        radius: 0.8,
    },
];
const ROBOT: &[Segment] = &[
    Segment::Line(&[(8.0, 4.6), (8.0, 2.6)]),
    Segment::Dot {
        center: (8.0, 2.0),
        radius: 0.7,
    },
    Segment::Outline(&[
        (3.2, 5.0),
        (12.8, 5.0),
        (13.6, 5.8),
        (13.6, 12.4),
        (12.8, 13.2),
        (3.2, 13.2),
        (2.4, 12.4),
        (2.4, 5.8),
    ]),
    Segment::Dot {
        center: (5.8, 8.6),
        radius: 0.8,
    },
    Segment::Dot {
        center: (10.2, 8.6),
        radius: 0.8,
    },
    Segment::Line(&[(5.8, 11.2), (10.2, 11.2)]),
];
const SPARKLE: &[Segment] = &[
    Segment::Solid(&[
        (6.4, 2.0),
        (7.7, 5.5),
        (11.2, 6.8),
        (7.7, 8.1),
        (6.4, 11.6),
        (5.1, 8.1),
        (1.6, 6.8),
        (5.1, 5.5),
    ]),
    Segment::Solid(&[
        (12.0, 9.2),
        (12.7, 11.3),
        (14.8, 12.0),
        (12.7, 12.7),
        (12.0, 14.8),
        (11.3, 12.7),
        (9.2, 12.0),
        (11.3, 11.3),
    ]),
];
const BOLT: &[Segment] = &[Segment::Solid(&[
    (8.8, 1.6),
    (3.8, 8.6),
    (7.2, 8.6),
    (6.2, 14.4),
    (12.2, 6.6),
    (8.8, 6.6),
])];
const ELLIPSIS: &[Segment] = &[
    Segment::Dot {
        center: (3.4, 8.0),
        radius: 1.1,
    },
    Segment::Dot {
        center: (8.0, 8.0),
        radius: 1.1,
    },
    Segment::Dot {
        center: (12.6, 8.0),
        radius: 1.1,
    },
];

fn segments(icon: Icon) -> &'static [Segment] {
    match icon {
        Icon::ChevronUp => CHEVRON_UP,
        Icon::ChevronDown => CHEVRON_DOWN,
        Icon::ChevronLeft => CHEVRON_LEFT,
        Icon::ChevronRight => CHEVRON_RIGHT,
        Icon::Close => CLOSE,
        Icon::Check => CHECK,
        Icon::Plus => PLUS,
        Icon::Minus => MINUS,
        Icon::Square => SQUARE,
        Icon::Stop => STOP,
        Icon::Folder => FOLDER,
        Icon::File => FILE,
        Icon::Home => HOME,
        Icon::Search => SEARCH,
        Icon::Terminal => TERMINAL,
        Icon::SourceControl => SOURCE_CONTROL,
        Icon::Gear => GEAR,
        Icon::ArrowUp => ARROW_UP,
        Icon::Download => DOWNLOAD,
        Icon::Refresh => REFRESH,
        Icon::History => HISTORY,
        Icon::Warning => WARNING,
        Icon::Error => ERROR,
        Icon::Info => INFO,
        Icon::Robot => ROBOT,
        Icon::Sparkle => SPARKLE,
        Icon::Bolt => BOLT,
        Icon::Ellipsis => ELLIPSIS,
    }
}

/// Every variant, for the tests that keep the family honest.
#[cfg(test)]
pub(crate) const ALL: [Icon; 28] = [
    Icon::ChevronUp,
    Icon::ChevronDown,
    Icon::ChevronLeft,
    Icon::ChevronRight,
    Icon::Close,
    Icon::Check,
    Icon::Plus,
    Icon::Minus,
    Icon::Square,
    Icon::Stop,
    Icon::Folder,
    Icon::File,
    Icon::Home,
    Icon::Search,
    Icon::Terminal,
    Icon::SourceControl,
    Icon::Gear,
    Icon::ArrowUp,
    Icon::Download,
    Icon::Refresh,
    Icon::History,
    Icon::Warning,
    Icon::Error,
    Icon::Info,
    Icon::Robot,
    Icon::Sparkle,
    Icon::Bolt,
    Icon::Ellipsis,
];

/// Paints `icon` centered inside `rect`, scaled from the 16 px grid so the
/// stroke keeps its optical weight at every size.
pub(crate) fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    for shape in shapes(icon, rect, color) {
        painter.add(shape);
    }
}

pub(crate) fn shapes(icon: Icon, rect: Rect, color: Color32) -> Vec<Shape> {
    let scale = rect.width().min(rect.height()) / GRID;
    let origin = rect.center() - Vec2::splat(GRID * scale * 0.5);
    let at = |(x, y): (f32, f32)| origin + Vec2::new(x * scale, y * scale);
    let stroke = Stroke::new(theme::stroke::ICON * scale, color);
    let path =
        |points: &'static [(f32, f32)]| points.iter().copied().map(at).collect::<Vec<Pos2>>();
    let mut shapes = Vec::new();
    for segment in segments(icon) {
        match segment {
            Segment::Line(points) => shapes.push(Shape::line(path(points), stroke)),
            Segment::Outline(points) => shapes.push(Shape::closed_line(path(points), stroke)),
            Segment::Solid(points) => {
                shapes.push(Shape::Path(PathShape::convex_polygon(
                    path(points),
                    color,
                    Stroke::NONE,
                )));
            }
            Segment::Circle { center, radius } => {
                shapes.push(Shape::circle_stroke(at(*center), radius * scale, stroke))
            }
            Segment::Dot { center, radius } => {
                shapes.push(Shape::circle_filled(at(*center), radius * scale, color));
            }
            Segment::Teeth {
                count,
                inner,
                outer,
            } => {
                for index in 0..*count {
                    let angle = index as f32 * std::f32::consts::TAU / *count as f32;
                    let direction = Vec2::new(angle.cos(), angle.sin());
                    let center = at((GRID * 0.5, GRID * 0.5));
                    shapes.push(Shape::line(
                        vec![
                            center + direction * *inner * scale,
                            center + direction * *outer * scale,
                        ],
                        stroke,
                    ));
                }
            }
        }
    }
    shapes
}

pub(crate) fn button_sized(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    idle: Color32,
    size: Vec2,
) -> egui::Response {
    button_with_id(ui, None, icon, label, idle, size)
}

/// The same button under a name, for the chrome whose geometry other code —
/// menus anchoring to it, tests measuring it — has to be able to find again.
pub(crate) fn button_with_id(
    ui: &mut egui::Ui,
    id: Option<egui::Id>,
    icon: Icon,
    label: &str,
    idle: Color32,
    size: Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = match id {
        Some(id) => ui.interact(rect, id, egui::Sense::click()),
        None => response,
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    paint_button(ui.painter(), icon, rect, &response, ui.is_enabled(), idle);
    response.on_hover_text(label)
}

/// The same painting for the call sites that own their own hit rect because the
/// chrome positions it by hand.
pub(crate) fn paint_button(
    painter: &Painter,
    icon: Icon,
    rect: Rect,
    response: &egui::Response,
    enabled: bool,
    idle: Color32,
) {
    let hover = theme::motion::animate(
        &response.ctx,
        response.id.with("icon_hover"),
        enabled && response.hovered(),
        theme::motion::FAST,
    );
    let press = theme::motion::animate(
        &response.ctx,
        response.id.with("icon_press"),
        enabled && response.is_pointer_button_down_on(),
        theme::motion::FAST,
    );
    let fill = if press > 0.0 {
        theme::motion::fade(theme::state::press(), press)
    } else {
        theme::motion::fade(theme::state::hover(), hover)
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, theme::corner(theme::radius::CONTROL), fill);
    }
    let color = if !enabled {
        theme::text_disabled()
    } else if response.hovered() {
        theme::text().primary
    } else {
        idle
    };
    paint(
        painter,
        icon,
        Rect::from_center_size(rect.center(), Vec2::splat(GRID)),
        color,
    );
}

/// A focus ring, drawn the one way the product draws one.
pub(crate) fn focus_ring(painter: &Painter, rect: Rect, radius: u8) {
    painter.rect_stroke(
        rect,
        theme::corner(radius),
        theme::border::focus_ring(),
        StrokeKind::Inside,
    );
}

/// Finding a painted glyph again, for the tests that check where chrome sits.
/// An icon is a path, a circle, or both, so a test that goes looking for line
/// segments finds nothing; this asks the question every such test means to ask.
#[cfg(test)]
pub(crate) mod probe {
    use egui::{Color32, Pos2, Rect, Shape, epaint::ClippedShape};

    /// Every point a stroked shape put on screen, whatever kind it is.
    pub(crate) fn stroked(shape: &Shape) -> Option<(Color32, Vec<Pos2>)> {
        match shape {
            Shape::Path(path) if path.stroke.width > 0.0 => match path.stroke.color {
                egui::epaint::ColorMode::Solid(color) => Some((color, path.points.clone())),
                egui::epaint::ColorMode::UV(_) => None,
            },
            Shape::Path(path) if path.fill != Color32::TRANSPARENT => {
                Some((path.fill, path.points.clone()))
            }
            Shape::LineSegment { points, stroke } => Some((stroke.color, points.to_vec())),
            Shape::Circle(circle) if circle.stroke.width > 0.0 => Some((
                circle.stroke.color,
                vec![
                    circle.center - egui::Vec2::splat(circle.radius),
                    circle.center + egui::Vec2::splat(circle.radius),
                ],
            )),
            _ => None,
        }
    }

    /// The bounds of every stroke drawn in `color` inside `within`, which is
    /// how a test locates one glyph among the chrome around it.
    pub(crate) fn bounds(shapes: &[ClippedShape], within: Rect, color: Color32) -> Option<Rect> {
        let bounds = shapes
            .iter()
            .filter_map(|clipped| stroked(&clipped.shape))
            .filter(|(painted, points)| {
                *painted == color && points.iter().all(|point| within.contains(*point))
            })
            .flat_map(|(_, points)| points)
            .fold(Rect::NOTHING, |bounds, point| {
                bounds.union(Rect::from_min_max(point, point))
            });
        bounds.is_finite().then_some(bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::{ALL, GRID, Icon, shapes};
    use crate::theme;
    use egui::{Color32, Rect, Shape, pos2};

    fn bounds(shapes: &[Shape]) -> Rect {
        shapes.iter().fold(Rect::NOTHING, |bounds, shape| {
            bounds.union(shape.visual_bounding_rect())
        })
    }

    #[test]
    fn every_icon_paints_inside_its_own_grid_box() {
        let box_rect = Rect::from_min_size(pos2(40.0, 24.0), egui::Vec2::splat(GRID));
        for icon in ALL {
            let painted = shapes(icon, box_rect, Color32::WHITE);
            assert!(!painted.is_empty(), "{icon:?} paints nothing");
            let bounds = bounds(&painted);
            // The bound includes half a stroke of feathering on each side.
            let slack = theme::stroke::ICON;
            assert!(
                box_rect.expand(slack).contains_rect(bounds),
                "{icon:?} paints {bounds:?} outside {box_rect:?}"
            );
        }
    }

    #[test]
    fn every_icon_uses_the_one_icon_stroke_and_scales_it_with_the_box() {
        let widths = |size: f32| {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::Vec2::splat(size));
            ALL.iter()
                .flat_map(|icon| shapes(*icon, rect, Color32::WHITE))
                .filter_map(|shape| match shape {
                    Shape::Path(path) if path.stroke.width > 0.0 => Some(path.stroke.width),
                    Shape::Circle(circle) if circle.stroke.width > 0.0 => Some(circle.stroke.width),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let authored = widths(GRID);
        assert!(!authored.is_empty());
        assert!(
            authored
                .iter()
                .all(|width| (width - theme::stroke::ICON).abs() < 0.001),
            "an icon is drawing at its own stroke width: {authored:?}"
        );
        assert!(
            widths(GRID * 2.0)
                .iter()
                .all(|width| (width - theme::stroke::ICON * 2.0).abs() < 0.001),
            "a doubled icon has to double its stroke or it reads as a hairline"
        );
    }

    #[test]
    fn a_chevron_is_one_joined_path_rather_than_two_overlapping_segments() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::Vec2::splat(GRID));
        for icon in [
            Icon::ChevronUp,
            Icon::ChevronDown,
            Icon::ChevronLeft,
            Icon::ChevronRight,
        ] {
            let painted = shapes(icon, rect, Color32::WHITE);
            assert_eq!(painted.len(), 1, "{icon:?} is not a single path");
            match &painted[0] {
                Shape::Path(path) => assert_eq!(path.points.len(), 3),
                other => panic!("{icon:?} painted {other:?}"),
            }
        }
    }

    #[test]
    fn refresh_is_two_joined_directional_arcs() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::Vec2::splat(GRID));
        let painted = shapes(Icon::Refresh, rect, Color32::WHITE);

        assert_eq!(painted.len(), 4);
        assert!(painted.iter().all(|shape| matches!(shape, Shape::Path(_))));
    }
}
