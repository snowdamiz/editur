use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use egui::{
    Align, Color32, Id, Label, Layout, RichText, ScrollArea, Sense, TextFormat, text::LayoutJob,
};

use super::agent_text::{agent_syntax_lines, agent_text_job, append_agent_syntax_line};
use super::{
    AGENT_CULL_MARGIN, AGENT_DIFF_HIGHLIGHT_MAX_LINES, AGENT_DIFF_PREVIEW_HEAD,
    AGENT_DIFF_PREVIEW_ROWS, find_highlighted_job, match_spans,
};
use crate::{
    agent::state::FileChange,
    icons::{self, Icon},
    syntax::{Highlighter, SyntaxManager},
    theme,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AgentDiffKind {
    Context,
    Removed,
    Added,
    Omitted,
}

#[derive(Clone, Debug)]
pub(super) struct AgentDiffLine {
    pub(super) old_number: Option<usize>,
    pub(super) new_number: Option<usize>,
    pub(super) text: String,
    pub(super) kind: AgentDiffKind,
    pub(super) omitted: usize,
}

#[derive(Debug)]
pub(super) struct AgentDiff {
    pub(super) lines: Vec<AgentDiffLine>,
    pub(super) preview: Option<Vec<AgentDiffLine>>,
    pub(super) added: usize,
    pub(super) removed: usize,
    pub(super) old_line_count: usize,
    pub(super) new_line_count: usize,
    pub(super) longest_chars: usize,
    pub(super) needed_old: Vec<usize>,
    pub(super) needed_new: Vec<usize>,
    pub(super) version: u64,
}

pub(super) fn build_agent_diff(old_text: Option<&str>, new_text: &str) -> AgentDiff {
    let old = old_text.map_or_else(Vec::new, |text| text.lines().collect::<Vec<_>>());
    let new = new_text.lines().collect::<Vec<_>>();
    let mut lines = Vec::with_capacity(old.len() + new.len());

    if old_text.is_none() {
        for (index, text) in new.iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
        }
    } else if (old.len() + 1).saturating_mul(new.len() + 1) <= 250_000 {
        // ponytail: bounded LCS keeps normal tool diffs precise; large replacements use the
        // linear fallback below instead of spending a frame on a quadratic comparison.
        let width = new.len() + 1;
        let mut common = vec![0_u32; (old.len() + 1) * width];
        for old_index in (0..old.len()).rev() {
            for new_index in (0..new.len()).rev() {
                common[old_index * width + new_index] = if old[old_index] == new[new_index] {
                    common[(old_index + 1) * width + new_index + 1] + 1
                } else {
                    common[(old_index + 1) * width + new_index]
                        .max(common[old_index * width + new_index + 1])
                };
            }
        }
        let (mut old_index, mut new_index) = (0, 0);
        while old_index < old.len() && new_index < new.len() {
            if old[old_index] == new[new_index] {
                lines.push(AgentDiffLine {
                    old_number: Some(old_index + 1),
                    new_number: Some(new_index + 1),
                    text: old[old_index].to_owned(),
                    kind: AgentDiffKind::Context,
                    omitted: 0,
                });
                old_index += 1;
                new_index += 1;
            } else if common[(old_index + 1) * width + new_index]
                >= common[old_index * width + new_index + 1]
            {
                lines.push(AgentDiffLine {
                    old_number: Some(old_index + 1),
                    new_number: None,
                    text: old[old_index].to_owned(),
                    kind: AgentDiffKind::Removed,
                    omitted: 0,
                });
                old_index += 1;
            } else {
                lines.push(AgentDiffLine {
                    old_number: None,
                    new_number: Some(new_index + 1),
                    text: new[new_index].to_owned(),
                    kind: AgentDiffKind::Added,
                    omitted: 0,
                });
                new_index += 1;
            }
        }
        while old_index < old.len() {
            lines.push(AgentDiffLine {
                old_number: Some(old_index + 1),
                new_number: None,
                text: old[old_index].to_owned(),
                kind: AgentDiffKind::Removed,
                omitted: 0,
            });
            old_index += 1;
        }
        while new_index < new.len() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(new_index + 1),
                text: new[new_index].to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
            new_index += 1;
        }
    } else {
        let prefix = old
            .iter()
            .zip(&new)
            .take_while(|(before, after)| before == after)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(before, after)| before == after)
            .count();
        for (index, text) in old.iter().take(prefix).enumerate() {
            lines.push(AgentDiffLine {
                old_number: Some(index + 1),
                new_number: Some(index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Context,
                omitted: 0,
            });
        }
        for (index, text) in old[prefix..old.len() - suffix].iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: Some(prefix + index + 1),
                new_number: None,
                text: (*text).to_owned(),
                kind: AgentDiffKind::Removed,
                omitted: 0,
            });
        }
        for (index, text) in new[prefix..new.len() - suffix].iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(prefix + index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
        }
        for index in 0..suffix {
            let old_index = old.len() - suffix + index;
            let new_index = new.len() - suffix + index;
            lines.push(AgentDiffLine {
                old_number: Some(old_index + 1),
                new_number: Some(new_index + 1),
                text: old[old_index].to_owned(),
                kind: AgentDiffKind::Context,
                omitted: 0,
            });
        }
    }

    let added = lines
        .iter()
        .filter(|line| line.kind == AgentDiffKind::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == AgentDiffKind::Removed)
        .count();
    let mut compact = Vec::with_capacity(lines.len());
    let mut start = 0;
    while start < lines.len() {
        if lines[start].kind != AgentDiffKind::Context {
            compact.push(lines[start].clone());
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < lines.len() && lines[end].kind == AgentDiffKind::Context {
            end += 1;
        }
        if end - start > 6 {
            compact.extend_from_slice(&lines[start..start + 3]);
            compact.push(AgentDiffLine {
                old_number: None,
                new_number: None,
                text: String::new(),
                kind: AgentDiffKind::Omitted,
                omitted: end - start - 6,
            });
            compact.extend_from_slice(&lines[end - 3..end]);
        } else {
            compact.extend_from_slice(&lines[start..end]);
        }
        start = end;
    }

    let preview = agent_diff_preview(&compact);
    let longest_chars = compact
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    let (needed_old, needed_new) = agent_diff_needed_lines(&compact);
    static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    AgentDiff {
        lines: compact,
        preview,
        added,
        removed,
        old_line_count: old.len(),
        new_line_count: new.len(),
        longest_chars,
        needed_old,
        needed_new,
        version: VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    }
}

pub(super) fn agent_diff_needed_lines(lines: &[AgentDiffLine]) -> (Vec<usize>, Vec<usize>) {
    let mut needed_old = Vec::new();
    let mut needed_new = Vec::new();
    for line in lines {
        match line.kind {
            AgentDiffKind::Removed => needed_old.extend(line.old_number),
            AgentDiffKind::Added | AgentDiffKind::Context => needed_new.extend(line.new_number),
            AgentDiffKind::Omitted => {}
        }
    }
    for needed in [&mut needed_old, &mut needed_new] {
        needed.sort_unstable();
        needed.dedup();
    }
    (needed_old, needed_new)
}

#[derive(Clone, Default)]
struct AgentDiffCache {
    old_text: Option<String>,
    new_text: String,
    diff: Option<Arc<AgentDiff>>,
}

#[cfg(test)]
pub(super) fn agent_diff_cache_count(ctx: &egui::Context) -> usize {
    ctx.data(|data| data.count::<AgentDiffCache>())
}

pub(super) fn cached_agent_diff(
    ui: &mut egui::Ui,
    id: Id,
    old_text: Option<&str>,
    new_text: &str,
) -> Arc<AgentDiff> {
    let cache_id = id.with("model");
    let stale = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentDiffCache>(cache_id);
        cache.diff.is_none() || cache.old_text.as_deref() != old_text || cache.new_text != new_text
    });
    if stale {
        let diff = Arc::new(build_agent_diff(old_text, new_text));
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentDiffCache>(cache_id);
            cache.old_text = old_text.map(str::to_owned);
            cache.new_text.clear();
            cache.new_text.push_str(new_text);
            cache.diff = Some(diff);
        });
    }
    ui.data_mut(|data| {
        Arc::clone(
            data.get_temp_mut_or_default::<AgentDiffCache>(cache_id)
                .diff
                .as_ref()
                .expect("agent diff was cached"),
        )
    })
}

pub(super) fn agent_diff_preview(lines: &[AgentDiffLine]) -> Option<Vec<AgentDiffLine>> {
    if lines.len() <= AGENT_DIFF_PREVIEW_ROWS {
        return None;
    }
    let tail = AGENT_DIFF_PREVIEW_ROWS - AGENT_DIFF_PREVIEW_HEAD - 1;
    let mut preview = Vec::with_capacity(AGENT_DIFF_PREVIEW_ROWS);
    preview.extend_from_slice(&lines[..AGENT_DIFF_PREVIEW_HEAD]);
    preview.push(AgentDiffLine {
        old_number: None,
        new_number: None,
        text: String::new(),
        kind: AgentDiffKind::Omitted,
        omitted: lines.len() - AGENT_DIFF_PREVIEW_HEAD - tail,
    });
    preview.extend_from_slice(&lines[lines.len() - tail..]);
    Some(preview)
}

pub(super) fn draw_agent_changed_files(
    ui: &mut egui::Ui,
    root: &Path,
    changed_paths: &HashMap<PathBuf, FileChange>,
    search: Option<(&str, Option<usize>)>,
) -> Option<PathBuf> {
    let row_count = changed_paths.len();
    let can_toggle = row_count > 5;
    let toggle_id = Id::new("agent_changed_files_toggle");
    let expanded = search.is_some()
        || ui.data(|data| {
            data.get_temp::<bool>(toggle_id.with("expanded"))
                .unwrap_or(false)
        });
    let mut rows = changed_paths.iter().collect::<Vec<_>>();
    if !expanded && rows.len() > 5 {
        rows.select_nth_unstable_by_key(5, |(path, _)| *path);
        rows.truncate(5);
    }
    rows.sort_by_key(|(path, _)| *path);
    let visible_rows = rows.len();
    let mut clicked = None;
    egui::Frame::new()
        .fill(theme::surface().raised)
        .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut toggle = None;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} file{} changed",
                        row_count,
                        if row_count == 1 { "" } else { "s" }
                    ))
                    .size(theme::typography::SMALL_SIZE)
                    .strong()
                    .color(theme::text().secondary),
                );
                if can_toggle && search.is_none() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        toggle = Some(icons::button_with_id(
                            ui,
                            Some(toggle_id),
                            if expanded {
                                Icon::ChevronUp
                            } else {
                                Icon::ChevronDown
                            },
                            if expanded {
                                "Collapse changed files"
                            } else {
                                "Expand changed files"
                            },
                            theme::text().secondary,
                            egui::Vec2::splat(theme::control::COMPACT + 2.0),
                        ));
                    });
                }
            });
            if toggle.is_some_and(|toggle| toggle.clicked()) {
                ui.data_mut(|data| data.insert_temp(toggle_id.with("expanded"), !expanded));
                ui.ctx().request_discard("changed files disclosure changed");
            }
            ui.add_space(theme::space::SNUG);
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut search_offset = 0;
            for (path, stats) in rows.into_iter().take(visible_rows) {
                let (added, removed) = (stats.added, stats.removed);
                let deleted = if path.is_absolute() {
                    !path.is_file()
                } else {
                    !root.join(path).is_file()
                };
                let (row, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), theme::control::ROW),
                    if deleted {
                        Sense::hover()
                    } else {
                        Sense::click()
                    },
                );
                let response = if deleted {
                    response.on_hover_text(format!("{} was deleted", path.display()))
                } else {
                    response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!("Open diff for {}", path.display()))
                };
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        if deleted {
                            egui::WidgetType::Label
                        } else {
                            egui::WidgetType::Button
                        },
                        ui.is_enabled(),
                        if deleted {
                            format!("{} — Deleted", path.display())
                        } else {
                            format!("Open diff for {}", path.display())
                        },
                    )
                });
                if !deleted && response.clicked() {
                    clicked = Some(path.clone());
                }
                if !deleted && response.hovered() {
                    ui.painter().rect_filled(
                        row,
                        theme::corner(theme::radius::ROW),
                        theme::state::hover(),
                    );
                }
                let icon = egui::Rect::from_center_size(
                    egui::pos2(row.left() + 12.0, row.center().y),
                    egui::Vec2::splat(icons::GRID),
                );
                icons::paint(
                    ui.painter(),
                    Icon::File,
                    icon,
                    if deleted {
                        theme::ink(theme::semantic().danger)
                    } else {
                        theme::text().muted
                    },
                );
                let mut counts_left = row.right() - theme::space::SNUG;
                if added > 0 || removed > 0 {
                    let removed_galley = ui.painter().layout_no_wrap(
                        format!("−{removed}"),
                        theme::typography::code_small(),
                        theme::ink(theme::semantic().danger),
                    );
                    counts_left -= removed_galley.size().x;
                    ui.painter().galley(
                        egui::pos2(counts_left, row.center().y - removed_galley.size().y * 0.5),
                        removed_galley,
                        theme::ink(theme::semantic().danger),
                    );
                    let added_galley = ui.painter().layout_no_wrap(
                        format!("+{added}"),
                        theme::typography::code_small(),
                        theme::ink(theme::semantic().success),
                    );
                    counts_left -= added_galley.size().x + theme::space::SNUG;
                    ui.painter().galley(
                        egui::pos2(counts_left, row.center().y - added_galley.size().y * 0.5),
                        added_galley,
                        theme::ink(theme::semantic().success),
                    );
                }
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                let directory = path
                    .parent()
                    .map(|parent| parent.strip_prefix(root).unwrap_or(parent))
                    .map(|parent| parent.display().to_string())
                    .filter(|parent| !parent.is_empty());
                let mut job = LayoutJob::default();
                job.append(
                    &name,
                    0.0,
                    TextFormat {
                        font_id: theme::typography::small(),
                        color: if deleted {
                            theme::text().muted
                        } else {
                            theme::text().primary
                        },
                        ..TextFormat::default()
                    },
                );
                if deleted {
                    job.append(
                        "Deleted",
                        theme::space::SMALL,
                        TextFormat {
                            font_id: theme::typography::micro(),
                            color: theme::ink(theme::semantic().danger),
                            ..TextFormat::default()
                        },
                    );
                }
                if let Some(directory) = directory {
                    job.append(
                        &directory,
                        theme::space::SMALL,
                        TextFormat {
                            font_id: theme::typography::micro(),
                            color: theme::text().muted,
                            ..TextFormat::default()
                        },
                    );
                }
                job.wrap.max_width =
                    (counts_left - theme::space::SMALL) - (row.left() + theme::space::XWIDE);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                if let Some((query, active)) = search {
                    let matches = match_spans(&path.display().to_string(), query);
                    let local_active = active
                        .and_then(|active| active.checked_sub(search_offset))
                        .filter(|&active| active < matches.len());
                    search_offset += matches.len();
                    job = find_highlighted_job(
                        &job,
                        &match_spans(&job.text, query),
                        local_active.unwrap_or(usize::MAX),
                    );
                }
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(
                    egui::pos2(
                        row.left() + theme::space::XWIDE,
                        row.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    theme::text().primary,
                );
            }
        });
    clicked
}

#[expect(clippy::too_many_arguments)]
pub(super) fn draw_agent_diff(
    ui: &mut egui::Ui,
    id: Id,
    path: &Path,
    old_text: Option<&str>,
    new_text: &str,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
    search: Option<(&str, Option<usize>)>,
    scroll_to_active: bool,
) -> (bool, bool) {
    let mut path_clicked = false;
    let mut scrolled = false;
    let diff = cached_agent_diff(ui, id, old_text, new_text);
    let expanded_id = id.with("expanded");
    let expanded =
        search.is_some() || ui.data(|data| data.get_temp::<bool>(expanded_id).unwrap_or(false));
    let lines = (!expanded)
        .then_some(diff.preview.as_deref())
        .flatten()
        .unwrap_or(&diff.lines);
    let can_toggle = diff.lines.len() > AGENT_DIFF_PREVIEW_ROWS;
    let file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    ui.set_width(ui.available_width());
    egui::Frame::new()
        .fill(theme::surface().input)
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let header = ui
                            .add(
                                Label::new(agent_text_job(
                                    file_name.as_ref(),
                                    ui.available_width(),
                                    theme::typography::strong(),
                                    theme::text().primary,
                                    search,
                                ))
                                .sense(Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!("Open {}", path.display()));
                        path_clicked |= header.clicked();
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("+{}  −{}", diff.added, diff.removed))
                                    .monospace()
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(theme::text().muted),
                            );
                            ui.label(
                                RichText::new(if old_text.is_some() {
                                    "MODIFIED"
                                } else {
                                    "NEW FILE"
                                })
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::accent()),
                            );
                        });
                    });
                });
            ui.painter().hline(
                ui.available_rect_before_wrap().x_range(),
                ui.cursor().top(),
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
            let row_search = search.map(|(query, active)| {
                let path_matches = match_spans(&path.display().to_string(), query).len();
                (
                    query,
                    active.and_then(|active| active.checked_sub(path_matches)),
                )
            });
            scrolled = draw_agent_diff_rows(
                ui,
                id,
                path,
                &diff,
                lines,
                old_text,
                new_text,
                highlighter,
                syntaxes,
                row_search,
                scroll_to_active,
            );
            if can_toggle {
                let label = if expanded {
                    "Collapse diff".to_owned()
                } else {
                    format!("Show all {} lines", diff.lines.len())
                };
                let toggle = ui.add_sized(
                    egui::vec2(ui.available_width(), 40.0),
                    egui::Button::new(
                        RichText::new(label)
                            .size(theme::typography::MICRO_SIZE)
                            .color(theme::accent()),
                    )
                    .frame(false),
                );
                if toggle.clicked() {
                    ui.data_mut(|data| data.insert_temp(expanded_id, !expanded));
                    ui.ctx().request_discard("diff card disclosure changed");
                }
            }
        });
    (path_clicked, scrolled)
}

#[expect(clippy::too_many_arguments)]
pub(super) fn draw_agent_diff_rows(
    ui: &mut egui::Ui,
    id: Id,
    path: &Path,
    diff: &AgentDiff,
    lines: &[AgentDiffLine],
    old_text: Option<&str>,
    new_text: &str,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
    search: Option<(&str, Option<usize>)>,
    scroll_to_active: bool,
) -> bool {
    let full = lines.len() == diff.lines.len();
    let preview_needed = (!full).then(|| agent_diff_needed_lines(lines));
    let (needed_old, needed_new) = preview_needed
        .as_ref()
        .map_or((&diff.needed_old, &diff.needed_new), |(old, new)| {
            (old, new)
        });
    let highlight = needed_old.len().max(needed_new.len()) <= AGENT_DIFF_HIGHLIGHT_MAX_LINES;
    let empty: &[usize] = &[];
    let (needed_old, needed_new) = if highlight {
        (needed_old.as_slice(), needed_new.as_slice())
    } else {
        (empty, empty)
    };
    let old_syntax = old_text.map(|text| {
        agent_syntax_lines(
            ui,
            id.with("old_syntax"),
            path,
            text,
            needed_old,
            highlighter,
            syntaxes,
            (diff.version, needed_old.len()),
        )
    });
    let new_syntax = agent_syntax_lines(
        ui,
        id.with("new_syntax"),
        path,
        new_text,
        needed_new,
        highlighter,
        syntaxes,
        (diff.version, needed_new.len()),
    );
    let old_digits = diff.old_line_count.max(1).ilog10() as usize + 1;
    let new_digits = diff.new_line_count.max(1).ilog10() as usize + 1;
    let longest = if lines.len() == diff.lines.len() {
        diff.longest_chars
    } else {
        lines
            .iter()
            .map(|line| line.text.chars().count())
            .max()
            .unwrap_or(0)
    };
    let content_width = ui.available_width();
    let desired_width =
        content_width.max(38.0 + (old_digits + new_digits + longest).min(240) as f32 * 7.3);
    let heights_id = id.with("row_heights");
    let mut row_heights = ui
        .data(|data| data.get_temp::<(f32, f32)>(heights_id))
        .unwrap_or((f32::NAN, f32::NAN));
    let mut rendered_rows = 0_usize;
    let mut active_row_rect: Option<egui::Rect> = None;
    ScrollArea::horizontal()
        .id_salt(id)
        .max_width(content_width)
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.set_width(desired_width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let clip = ui.clip_rect();
            let mut search_offset = 0;
            let mut pending_space = 0.0_f32;
            for line in lines {
                let omitted_row = line.kind == AgentDiffKind::Omitted;
                let known_height = if omitted_row {
                    row_heights.1
                } else {
                    row_heights.0
                };
                let row_top = ui.cursor().top() + pending_space;
                if !scroll_to_active
                    && known_height.is_finite()
                    && (row_top + known_height < clip.top() - AGENT_CULL_MARGIN
                        || row_top > clip.bottom() + AGENT_CULL_MARGIN)
                {
                    if let Some((query, _)) = search
                        && !omitted_row
                    {
                        search_offset += match_spans(&line.text, query).len();
                    }
                    pending_space += known_height;
                    continue;
                }
                if pending_space > 0.0 {
                    ui.add_space(pending_space);
                    pending_space = 0.0;
                }
                rendered_rows += 1;
                let mut row_is_active = false;
                let fill = match line.kind {
                    AgentDiffKind::Added => theme::diff::added(),
                    AgentDiffKind::Removed => theme::diff::removed(),
                    AgentDiffKind::Omitted => theme::surface().raised,
                    AgentDiffKind::Context => Color32::TRANSPARENT,
                };
                let row = egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::symmetric(9, 3))
                    .show(ui, |ui| {
                        ui.set_min_width(desired_width - 18.0);
                        if line.kind == AgentDiffKind::Omitted {
                            ui.label(
                                RichText::new(format!("⋯  {} lines hidden", line.omitted))
                                    .monospace()
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(theme::text().muted),
                            );
                            return;
                        }
                        let old_number = line.old_number.map_or_else(
                            || " ".repeat(old_digits),
                            |number| format!("{number:>old_digits$}"),
                        );
                        let new_number = line.new_number.map_or_else(
                            || " ".repeat(new_digits),
                            |number| format!("{number:>new_digits$}"),
                        );
                        let (sign, color) = match line.kind {
                            AgentDiffKind::Added => ("+", theme::diff::added_ink()),
                            AgentDiffKind::Removed => ("−", theme::diff::removed_ink()),
                            AgentDiffKind::Context => (" ", theme::text().secondary),
                            AgentDiffKind::Omitted => unreachable!(),
                        };
                        let number_color = match line.kind {
                            AgentDiffKind::Added => theme::diff::added_number(),
                            AgentDiffKind::Removed => theme::diff::removed_number(),
                            AgentDiffKind::Context => theme::text_disabled(),
                            AgentDiffKind::Omitted => unreachable!(),
                        };
                        let mut job = LayoutJob::default();
                        job.append(
                            &format!("{old_number} {new_number}  "),
                            0.0,
                            TextFormat {
                                font_id: theme::typography::code_small(),
                                color: number_color,
                                ..TextFormat::default()
                            },
                        );
                        job.append(
                            &format!("{sign} "),
                            0.0,
                            TextFormat {
                                font_id: theme::typography::code_small(),
                                color,
                                ..TextFormat::default()
                            },
                        );
                        let highlighted = match line.kind {
                            AgentDiffKind::Removed => old_syntax.as_deref(),
                            AgentDiffKind::Added | AgentDiffKind::Context => Some(&*new_syntax),
                            AgentDiffKind::Omitted => unreachable!(),
                        };
                        let line_number = match line.kind {
                            AgentDiffKind::Removed => line.old_number,
                            AgentDiffKind::Added | AgentDiffKind::Context => line.new_number,
                            AgentDiffKind::Omitted => None,
                        };
                        let code_start = job.text.len();
                        if let (Some(highlighted), Some(line_number)) = (highlighted, line_number) {
                            append_agent_syntax_line(
                                &mut job,
                                highlighted,
                                line_number,
                                &line.text,
                                matches!(line.kind, AgentDiffKind::Added | AgentDiffKind::Removed),
                            );
                        }
                        let job = if let Some((query, active)) = search {
                            let matches = match_spans(&line.text, query)
                                .into_iter()
                                .map(|span| code_start + span.start..code_start + span.end)
                                .collect::<Vec<_>>();
                            let local_active = active
                                .and_then(|active| active.checked_sub(search_offset))
                                .filter(|&active| active < matches.len());
                            search_offset += matches.len();
                            row_is_active = local_active.is_some();
                            find_highlighted_job(&job, &matches, local_active.unwrap_or(usize::MAX))
                        } else {
                            job
                        };
                        ui.add(
                            Label::new(job)
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .selectable(true),
                        );
                    });
                if row_is_active {
                    active_row_rect = Some(row.response.rect);
                }
                let measured = ui.cursor().top() - row_top;
                if omitted_row {
                    row_heights.1 = measured;
                } else {
                    row_heights.0 = measured;
                }
            }
            if pending_space > 0.0 {
                ui.add_space(pending_space);
            }
        });
    ui.data_mut(|data| {
        data.insert_temp(heights_id, row_heights);
        data.insert_temp(id.with("rendered_rows"), rendered_rows);
    });
    if scroll_to_active && let Some(rect) = active_row_rect {
        ui.scroll_to_rect(rect, Some(Align::Center));
        return true;
    }
    false
}

pub(super) fn agent_diff_view_header(
    ui: &mut egui::Ui,
    root: &Path,
    path: &Path,
    diff: &AgentDiff,
    modified: bool,
    dismiss_label: &str,
) -> bool {
    let mut dismissed = false;
    let file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let directory = path
        .parent()
        .map(|parent| parent.strip_prefix(root).unwrap_or(parent))
        .map(|parent| parent.display().to_string())
        .filter(|parent| !parent.is_empty());
    ui.label(
        RichText::new(file_name.as_ref())
            .size(theme::typography::BODY_SIZE)
            .strong()
            .color(theme::text().primary),
    );
    if let Some(directory) = directory {
        ui.add(
            Label::new(
                RichText::new(directory)
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text().muted),
            )
            .truncate(),
        );
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        dismissed = ui
            .add(
                egui::Button::new(
                    RichText::new(dismiss_label)
                        .size(theme::typography::SMALL_SIZE)
                        .color(theme::accent()),
                )
                .frame(false),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked();
        ui.label(
            RichText::new(format!("+{}  −{}", diff.added, diff.removed))
                .monospace()
                .size(theme::typography::MICRO_SIZE)
                .color(theme::text().muted),
        );
        ui.label(
            RichText::new(if modified { "MODIFIED" } else { "NEW FILE" })
                .size(theme::typography::MICRO_SIZE)
                .strong()
                .color(theme::accent()),
        );
    });
    dismissed
}

#[expect(clippy::too_many_arguments)]
pub(super) fn draw_agent_diff_body(
    ui: &mut egui::Ui,
    id: Id,
    path: &Path,
    diff: &AgentDiff,
    old_text: Option<&str>,
    new_text: &str,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
) {
    ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            draw_agent_diff_rows(
                ui,
                id,
                path,
                diff,
                &diff.lines,
                old_text,
                new_text,
                highlighter,
                syntaxes,
                None,
                false,
            );
        });
}

#[expect(clippy::too_many_arguments)]
pub(super) fn draw_agent_diff_view(
    ui: &mut egui::Ui,
    id: Id,
    root: &Path,
    path: &Path,
    old_text: Option<&str>,
    new_text: &str,
    dismiss_label: &str,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
) -> bool {
    let mut dismissed = false;
    ui.painter()
        .rect_filled(ui.max_rect(), 0.0, theme::surface().input);
    let diff = cached_agent_diff(ui, id, old_text, new_text);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dismissed = agent_diff_view_header(
                    ui,
                    root,
                    path,
                    &diff,
                    old_text.is_some(),
                    dismiss_label,
                );
            });
        });
    ui.painter().hline(
        ui.available_rect_before_wrap().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1.0, theme::border::strong_color()),
    );
    draw_agent_diff_body(
        ui,
        id,
        path,
        &diff,
        old_text,
        new_text,
        highlighter,
        syntaxes,
    );
    dismissed
}
