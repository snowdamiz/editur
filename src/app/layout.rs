use super::{
    AGENTIC_DIFF_MIN_CONVERSATION, AGENTIC_DIFF_MIN_PANEL, FIND_BAR_HEIGHT, PANE_TAB_HEIGHT,
    SIDEBAR_MIN_WIDTH, TERMINAL_MIN_HEIGHT, TITLEBAR_HEIGHT, WINDOW_CORNER_RADIUS,
    WORKSPACE_MIN_HEIGHT,
};

#[cfg(test)]
pub(super) fn split_workspace(
    content: egui::Rect,
    explorer_open: bool,
    explorer_width: f32,
    agent_open: bool,
    agent_width: f32,
) -> (Option<egui::Rect>, egui::Rect, egui::Rect) {
    let (explorer, editor, agent, _) = split_workspace_with_devin(
        content,
        explorer_open,
        explorer_width,
        agent_open,
        agent_width,
        false,
        0.0,
    );
    (explorer, editor, agent)
}

pub(super) fn split_workspace_with_devin(
    content: egui::Rect,
    explorer_open: bool,
    explorer_width: f32,
    agent_open: bool,
    agent_width: f32,
    devin_open: bool,
    devin_width: f32,
) -> (Option<egui::Rect>, egui::Rect, egui::Rect, egui::Rect) {
    let right_width = if devin_open {
        devin_width.max(320.0).min(content.width() * 0.52)
    } else if agent_open {
        agent_width.max(320.0).min(content.width() * 0.52)
    } else {
        0.0
    };
    let explorer_width = explorer_width
        .max(SIDEBAR_MIN_WIDTH)
        .min((content.width() - right_width - 160.0).max(SIDEBAR_MIN_WIDTH));
    let explorer = explorer_open
        .then(|| content.with_max_x((content.left() + explorer_width).min(content.right())));
    let right = content.with_min_x((content.right() - right_width).max(content.left()));
    let agent = if agent_open && !devin_open {
        right
    } else {
        content.with_min_x(content.right())
    };
    let devin = if devin_open {
        right
    } else {
        content.with_min_x(content.right())
    };
    let editor_right = if agent_open || devin_open {
        right.left()
    } else {
        content.right()
    };
    let editor = egui::Rect::from_min_max(
        egui::pos2(
            explorer.map_or(content.left(), |rect| rect.right()),
            content.top(),
        ),
        egui::pos2(editor_right, content.bottom()),
    );
    (explorer, editor, agent, devin)
}

pub(super) fn split_agentic_workspace(
    content: egui::Rect,
    sidebar_open: bool,
    sidebar_width: f32,
) -> (Option<egui::Rect>, egui::Rect) {
    if !sidebar_open {
        return (None, content);
    }
    let rail_width = sidebar_width
        .min(content.width() * 0.36)
        .max(SIDEBAR_MIN_WIDTH.min(content.width()));
    let sessions = content.with_max_x(content.left() + rail_width);
    let agent = content.with_min_x(sessions.right());
    (Some(sessions), agent)
}

/// Carves the diff panel out of the agentic conversation column.
pub(super) fn split_agentic_diff(
    content: egui::Rect,
    open: bool,
) -> (egui::Rect, Option<egui::Rect>) {
    if !open {
        return (content, None);
    }
    let panel_width = (content.width() * 0.55).min(content.width() - AGENTIC_DIFF_MIN_CONVERSATION);
    if panel_width < AGENTIC_DIFF_MIN_PANEL {
        return (content, None);
    }
    let panel = content.with_min_x(content.right() - panel_width);
    (content.with_max_x(panel.left()), Some(panel))
}

pub(super) fn split_bottom_panel(
    content: egui::Rect,
    open: bool,
    requested_height: f32,
) -> (egui::Rect, Option<egui::Rect>) {
    if !open {
        return (content, None);
    }
    let max_height = (content.height() - WORKSPACE_MIN_HEIGHT).max(0.0);
    let height = requested_height.max(TERMINAL_MIN_HEIGHT).min(max_height);
    let split = content.bottom() - height;
    (content.with_max_y(split), Some(content.with_min_y(split)))
}

pub(super) fn editor_column_content(rect: egui::Rect) -> egui::Rect {
    rect.with_min_y((rect.top() + TITLEBAR_HEIGHT).min(rect.bottom()))
}

pub(super) fn split_pane_content(
    rect: egui::Rect,
    find_open: bool,
) -> (egui::Rect, Option<egui::Rect>) {
    let findbar =
        find_open.then(|| rect.with_min_y((rect.bottom() - FIND_BAR_HEIGHT).max(rect.top())));
    (
        rect.with_max_y(findbar.map_or(rect.bottom(), |bar| bar.top())),
        findbar,
    )
}

pub(super) fn pane_focus_corner_radius(rect: egui::Rect, window: egui::Rect) -> egui::CornerRadius {
    let left = (rect.left() - window.left()).abs() <= 0.5;
    let right = (rect.right() - window.right()).abs() <= 0.5;
    let top = (rect.top() - window.top()).abs() <= 0.5;
    let bottom = (rect.bottom() - window.bottom()).abs() <= 0.5;
    egui::CornerRadius {
        nw: if left && top { WINDOW_CORNER_RADIUS } else { 0 },
        ne: if right && top {
            WINDOW_CORNER_RADIUS
        } else {
            0
        },
        sw: if left && bottom {
            WINDOW_CORNER_RADIUS
        } else {
            0
        },
        se: if right && bottom {
            WINDOW_CORNER_RADIUS
        } else {
            0
        },
    }
}

pub(super) fn pane_header_and_content(
    titlebar: egui::Rect,
    editor: egui::Rect,
    pane: egui::Rect,
) -> (egui::Rect, egui::Rect) {
    if (pane.top() - editor.top()).abs() <= 0.5 {
        (
            egui::Rect::from_min_max(
                egui::pos2(pane.left(), titlebar.top()),
                egui::pos2(pane.right(), titlebar.bottom()),
            ),
            pane,
        )
    } else {
        let header = pane.with_max_y((pane.top() + PANE_TAB_HEIGHT).min(pane.bottom()));
        (header, pane.with_min_y(header.bottom()))
    }
}
