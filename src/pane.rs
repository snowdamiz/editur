use egui::{CursorIcon, Id, Sense};

use crate::theme;

const DIVIDER_HIT_WIDTH: f32 = 8.0;
const MIN_WIDTH: f32 = 200.0;
const MIN_HEIGHT: f32 = 200.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DropZone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PaneId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone)]
enum PaneNode {
    Leaf(PaneId),
    Split {
        id: u64,
        axis: SplitAxis,
        fraction: f32,
        first: Box<Self>,
        second: Box<Self>,
    },
}

#[derive(Clone)]
pub(crate) struct PaneLayout {
    root: PaneNode,
    next_id: u64,
    next_split_id: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct PaneSplitHandle {
    pub(crate) id: u64,
    pub(crate) axis: SplitAxis,
    pub(crate) bounds: egui::Rect,
    pub(crate) hit_rect: egui::Rect,
}

#[derive(Clone, Copy)]
pub(crate) struct TabDrop {
    pub(crate) target: PaneId,
    pub(crate) zone: DropZone,
    pub(crate) preview: egui::Rect,
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self {
            root: PaneNode::Leaf(PaneId(0)),
            next_id: 1,
            next_split_id: 0,
        }
    }
}

impl PaneLayout {
    pub(crate) fn split(&mut self, target: PaneId, zone: DropZone) -> Option<PaneId> {
        let new = PaneId(self.next_id);
        if self.root.split(target, new, zone, self.next_split_id) {
            self.next_id += 1;
            self.next_split_id += 1;
            Some(new)
        } else {
            None
        }
    }

    pub(crate) fn insert_at_split(&mut self, target: u64) -> Option<PaneId> {
        let new = PaneId(self.next_id);
        if self.root.insert_at_split(target, new, self.next_split_id) {
            self.next_id += 1;
            self.next_split_id += 1;
            Some(new)
        } else {
            None
        }
    }

    pub(crate) fn rects(&self, available: egui::Rect) -> Vec<(PaneId, egui::Rect)> {
        let mut rects = Vec::new();
        self.root.append_rects(available, &mut rects);
        rects
    }

    pub(crate) fn panes(&self) -> Vec<PaneId> {
        self.rects(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::splat(1.0),
        ))
        .into_iter()
        .map(|(pane, _)| pane)
        .collect()
    }

    pub(crate) fn split_handles(&self, available: egui::Rect) -> Vec<PaneSplitHandle> {
        let mut handles = Vec::new();
        self.root.append_split_handles(available, &mut handles);
        handles
    }

    pub(crate) fn resize(&mut self, id: u64, bounds: egui::Rect, pointer: egui::Pos2) -> bool {
        self.root.resize(id, bounds, pointer)
    }

    pub(crate) fn resize_adjacent(
        &mut self,
        id: u64,
        available: egui::Rect,
        pointer: egui::Pos2,
    ) -> bool {
        let handles = self.split_handles(available);
        let Some(target) = handles.iter().find(|handle| handle.id == id) else {
            return false;
        };
        let preserved = handles
            .iter()
            .filter(|handle| handle.id != id && handle.axis == target.axis)
            .map(|handle| (handle.id, handle.hit_rect.center()))
            .collect::<Vec<_>>();
        if !self.resize(id, target.bounds, pointer) {
            return false;
        }
        for (preserved_id, center) in preserved {
            if let Some(handle) = self
                .split_handles(available)
                .into_iter()
                .find(|handle| handle.id == preserved_id)
            {
                self.resize(preserved_id, handle.bounds, center);
            }
        }
        true
    }

    pub(crate) fn remove(&mut self, target: PaneId) -> bool {
        if self.root.leaf_count() == 1 || !self.root.contains(target) {
            return false;
        }
        let root = std::mem::replace(&mut self.root, PaneNode::Leaf(target));
        self.root = root
            .without(target)
            .expect("another pane remains after removing a split leaf");
        true
    }
}

pub(crate) fn resize_dragged_pane_handle(
    ctx: &egui::Context,
    layout: &mut PaneLayout,
    available: egui::Rect,
    id_salt: &'static str,
    resize_adjacent: bool,
) -> Vec<PaneSplitHandle> {
    if ctx.input(|input| input.pointer.primary_down())
        && let Some(pointer) = ctx.pointer_interact_pos()
    {
        for handle in layout.split_handles(available) {
            if !ctx.is_being_dragged(Id::new((id_salt, handle.id))) {
                continue;
            }
            if resize_adjacent {
                layout.resize_adjacent(handle.id, available, pointer);
            } else {
                layout.resize(handle.id, handle.bounds, pointer);
            }
        }
    }
    layout.split_handles(available)
}

pub(crate) fn paint_pane_resize_handles(
    ui: &mut egui::Ui,
    handles: &[PaneSplitHandle],
    id_salt: &'static str,
) {
    for handle in handles {
        let response = ui.interact(
            handle.hit_rect,
            Id::new((id_salt, handle.id)),
            Sense::drag(),
        );
        let active = response.hovered() || response.dragged();
        if active {
            ui.ctx().set_cursor_icon(match handle.axis {
                SplitAxis::Horizontal => CursorIcon::ResizeVertical,
                SplitAxis::Vertical => CursorIcon::ResizeHorizontal,
            });
        }
        let center = handle.hit_rect.center();
        let line = match handle.axis {
            SplitAxis::Horizontal => [
                egui::pos2(handle.hit_rect.left(), center.y),
                egui::pos2(handle.hit_rect.right(), center.y),
            ],
            SplitAxis::Vertical => [
                egui::pos2(center.x, handle.hit_rect.top()),
                egui::pos2(center.x, handle.hit_rect.bottom()),
            ],
        };
        ui.painter()
            .line_segment(line, resize_divider_stroke(ui.ctx(), active));
    }
}

impl PaneNode {
    fn split(&mut self, target: PaneId, new: PaneId, zone: DropZone, split_id: u64) -> bool {
        match self {
            Self::Leaf(id) if *id == target && zone != DropZone::Center => {
                let existing = Self::Leaf(*id);
                let added = Self::Leaf(new);
                let (axis, first, second) = match zone {
                    DropZone::Left => (SplitAxis::Vertical, added, existing),
                    DropZone::Right => (SplitAxis::Vertical, existing, added),
                    DropZone::Top => (SplitAxis::Horizontal, added, existing),
                    DropZone::Bottom => (SplitAxis::Horizontal, existing, added),
                    DropZone::Center => return false,
                };
                *self = Self::Split {
                    id: split_id,
                    axis,
                    fraction: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, new, zone, split_id)
                    || second.split(target, new, zone, split_id)
            }
            Self::Leaf(_) => false,
        }
    }

    fn insert_at_split(&mut self, target: u64, new: PaneId, split_id: u64) -> bool {
        match self {
            Self::Split { id, .. } if *id == target => {
                let Self::Split {
                    id,
                    axis,
                    fraction,
                    first,
                    second,
                } = std::mem::replace(self, Self::Leaf(new))
                else {
                    unreachable!()
                };
                let first_fraction = fraction * 2.0 / 3.0;
                let middle_fraction = (1.0 / 3.0) / (1.0 - first_fraction);
                *self = Self::Split {
                    id,
                    axis,
                    fraction: first_fraction,
                    first,
                    second: Box::new(Self::Split {
                        id: split_id,
                        axis,
                        fraction: middle_fraction,
                        first: Box::new(Self::Leaf(new)),
                        second,
                    }),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.insert_at_split(target, new, split_id)
                    || second.insert_at_split(target, new, split_id)
            }
            Self::Leaf(_) => false,
        }
    }

    fn append_rects(&self, available: egui::Rect, rects: &mut Vec<(PaneId, egui::Rect)>) {
        match self {
            Self::Leaf(id) => rects.push((*id, available)),
            Self::Split {
                axis,
                fraction,
                first,
                second,
                ..
            } => {
                let fraction = Self::clamped_fraction(available, *axis, *fraction, first, second);
                let (first_rect, second_rect) = match axis {
                    SplitAxis::Horizontal => {
                        let middle = available.top() + available.height() * fraction;
                        (available.with_max_y(middle), available.with_min_y(middle))
                    }
                    SplitAxis::Vertical => {
                        let middle = available.left() + available.width() * fraction;
                        (available.with_max_x(middle), available.with_min_x(middle))
                    }
                };
                first.append_rects(first_rect, rects);
                second.append_rects(second_rect, rects);
            }
        }
    }

    fn append_split_handles(&self, available: egui::Rect, handles: &mut Vec<PaneSplitHandle>) {
        let Self::Split {
            id,
            axis,
            fraction,
            first,
            second,
        } = self
        else {
            return;
        };
        let fraction = Self::clamped_fraction(available, *axis, *fraction, first, second);
        let (first_rect, second_rect, hit_rect) = match axis {
            SplitAxis::Horizontal => {
                let middle = available.top() + available.height() * fraction;
                (
                    available.with_max_y(middle),
                    available.with_min_y(middle),
                    egui::Rect::from_center_size(
                        egui::pos2(available.center().x, middle),
                        egui::vec2(available.width(), DIVIDER_HIT_WIDTH),
                    ),
                )
            }
            SplitAxis::Vertical => {
                let middle = available.left() + available.width() * fraction;
                (
                    available.with_max_x(middle),
                    available.with_min_x(middle),
                    egui::Rect::from_center_size(
                        egui::pos2(middle, available.center().y),
                        egui::vec2(DIVIDER_HIT_WIDTH, available.height()),
                    ),
                )
            }
        };
        handles.push(PaneSplitHandle {
            id: *id,
            axis: *axis,
            bounds: available,
            hit_rect,
        });
        first.append_split_handles(first_rect, handles);
        second.append_split_handles(second_rect, handles);
    }

    fn resize(&mut self, target: u64, bounds: egui::Rect, pointer: egui::Pos2) -> bool {
        match self {
            Self::Split {
                id,
                axis,
                fraction,
                first,
                second,
            } if *id == target => {
                let extent = match axis {
                    SplitAxis::Horizontal => bounds.height(),
                    SplitAxis::Vertical => bounds.width(),
                };
                if extent <= 0.0 {
                    return false;
                }
                let requested = match axis {
                    SplitAxis::Horizontal => (pointer.y - bounds.top()) / extent,
                    SplitAxis::Vertical => (pointer.x - bounds.left()) / extent,
                };
                *fraction = Self::clamped_fraction(bounds, *axis, requested, first, second);
                true
            }
            Self::Split { first, second, .. } => {
                first.resize(target, bounds, pointer) || second.resize(target, bounds, pointer)
            }
            Self::Leaf(_) => false,
        }
    }

    fn clamped_fraction(
        available: egui::Rect,
        axis: SplitAxis,
        fraction: f32,
        first: &Self,
        second: &Self,
    ) -> f32 {
        let extent = match axis {
            SplitAxis::Horizontal => available.height(),
            SplitAxis::Vertical => available.width(),
        };
        let first_min = first.minimum_extent(axis);
        let second_min = second.minimum_extent(axis);
        if extent <= first_min + second_min {
            return first_min / (first_min + second_min);
        }
        fraction.clamp(first_min / extent, 1.0 - second_min / extent)
    }

    fn minimum_extent(&self, axis: SplitAxis) -> f32 {
        match self {
            Self::Leaf(_) => match axis {
                SplitAxis::Horizontal => MIN_HEIGHT,
                SplitAxis::Vertical => MIN_WIDTH,
            },
            Self::Split {
                axis: split_axis,
                first,
                second,
                ..
            } if *split_axis == axis => first.minimum_extent(axis) + second.minimum_extent(axis),
            Self::Split { first, second, .. } => {
                first.minimum_extent(axis).max(second.minimum_extent(axis))
            }
        }
    }

    fn contains(&self, target: PaneId) -> bool {
        match self {
            Self::Leaf(id) => *id == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }

    fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    fn without(self, target: PaneId) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Split {
                id,
                axis,
                fraction,
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    axis,
                    fraction,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }
}

fn tab_drop_edges(rect: egui::Rect, pointer: egui::Pos2) -> [(f32, DropZone); 4] {
    let x = ((pointer.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let y = ((pointer.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
    [
        (x, DropZone::Left),
        (1.0 - x, DropZone::Right),
        (y, DropZone::Top),
        (1.0 - y, DropZone::Bottom),
    ]
}

pub(crate) fn allowed_tab_drop_zone(rect: egui::Rect, pointer: egui::Pos2) -> DropZone {
    tab_drop_zone(rect, pointer, 0.22)
}

pub(crate) fn stable_tab_drop_zone(
    rect: egui::Rect,
    pointer: egui::Pos2,
    previous: Option<DropZone>,
) -> DropZone {
    if let Some(previous) = previous.filter(|zone| *zone != DropZone::Center)
        && tab_drop_edges(rect, pointer)
            .into_iter()
            .any(|(distance, zone)| zone == previous && distance <= 0.32 && can_split(rect, zone))
    {
        return previous;
    }
    allowed_tab_drop_zone(rect, pointer)
}

fn tab_drop_zone(rect: egui::Rect, pointer: egui::Pos2, threshold: f32) -> DropZone {
    tab_drop_edges(rect, pointer)
        .into_iter()
        .filter(|(distance, zone)| *distance <= threshold && can_split(rect, *zone))
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map_or(DropZone::Center, |(_, zone)| zone)
}

fn can_split(rect: egui::Rect, zone: DropZone) -> bool {
    let can_split_columns = rect.width() >= MIN_WIDTH * 2.0;
    let can_split_rows = rect.height() >= MIN_HEIGHT * 2.0;
    match zone {
        DropZone::Left | DropZone::Right => can_split_columns,
        DropZone::Top | DropZone::Bottom => can_split_rows,
        DropZone::Center => false,
    }
}

pub(crate) fn tab_drop_preview(rect: egui::Rect, zone: DropZone) -> egui::Rect {
    match zone {
        DropZone::Center => rect,
        DropZone::Left => rect.with_max_x(rect.center().x),
        DropZone::Right => rect.with_min_x(rect.center().x),
        DropZone::Top => rect.with_max_y(rect.center().y),
        DropZone::Bottom => rect.with_min_y(rect.center().y),
    }
}

pub(crate) fn resize_divider_stroke(ctx: &egui::Context, active: bool) -> egui::Stroke {
    egui::Stroke::new(
        ctx.input(|input| input.physical_pixel_size()),
        if active {
            theme::accent()
        } else {
            theme::border::strong_color()
        },
    )
}
