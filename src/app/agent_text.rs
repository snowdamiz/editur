use std::{collections::HashMap, path::Path, path::PathBuf, sync::Arc};

use egui::{Color32, Id, Label, TextFormat, text::LayoutJob};

use super::{find_highlighted_job, match_spans, plain_text_job};
use crate::{
    markdown,
    syntax::{Highlighter, SyntaxManager},
    theme,
};

const GALLEY_WIDTH_SLOTS: usize = 2;

#[derive(Clone)]
struct AgentFindGalley {
    width: u32,
    query: String,
    active: usize,
    galley: Arc<egui::Galley>,
}

impl AgentFindGalley {
    fn matches(&self, width: u32, query: &str, active: usize) -> bool {
        self.width == width && self.query == query && self.active == active
    }
}

#[derive(Clone, Default)]
struct AgentMarkdownCache {
    source: String,
    galleys: Vec<(u32, Arc<egui::Galley>)>,
    find: Option<AgentFindGalley>,
}

#[derive(Clone, Default)]
struct AgentCodeCache {
    source: String,
    path: PathBuf,
    galleys: Vec<(u32, Arc<egui::Galley>)>,
    find: Option<AgentFindGalley>,
}

#[derive(Clone)]
pub(super) struct AgentSyntaxLines {
    job: LayoutJob,
    ranges: HashMap<usize, std::ops::Range<usize>>,
}

#[derive(Clone, Default)]
struct AgentSyntaxLinesCache {
    version: (u64, usize),
    lines: Option<Arc<AgentSyntaxLines>>,
}

#[expect(clippy::too_many_arguments)]
pub(super) fn agent_syntax_lines(
    ui: &mut egui::Ui,
    id: Id,
    path: &Path,
    source: &str,
    needed: &[usize],
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
    version: (u64, usize),
) -> Arc<AgentSyntaxLines> {
    let stale = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentSyntaxLinesCache>(id);
        cache.lines.is_none() || cache.version != version
    });
    if stale {
        let mut hunk = String::new();
        let mut ranges = HashMap::with_capacity(needed.len());
        let mut wanted = needed.iter().copied().peekable();
        for (index, line) in source.lines().enumerate() {
            let Some(&next) = wanted.peek() else {
                break;
            };
            if index + 1 == next {
                wanted.next();
                let start = hunk.len();
                hunk.push_str(line);
                ranges.insert(index + 1, start..hunk.len());
                hunk.push('\n');
            }
        }
        let syntax = syntaxes.detect(path, false);
        let job = highlighter
            .highlight_job(&hunk, syntax, syntaxes.set(), f32::INFINITY)
            .unwrap_or_else(|_| plain_text_job(&hunk, f32::INFINITY));
        let lines = Arc::new(AgentSyntaxLines { job, ranges });
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentSyntaxLinesCache>(id);
            cache.version = version;
            cache.lines = Some(lines);
        });
    }
    ui.data_mut(|data| {
        Arc::clone(
            data.get_temp_mut_or_default::<AgentSyntaxLinesCache>(id)
                .lines
                .as_ref()
                .expect("agent syntax lines were cached"),
        )
    })
}

pub(super) fn append_agent_syntax_line(
    target: &mut LayoutJob,
    highlighted: &AgentSyntaxLines,
    line_number: usize,
    fallback: &str,
    tinted: bool,
) {
    let Some(range) = highlighted.ranges.get(&line_number) else {
        target.append(
            fallback,
            0.0,
            TextFormat {
                font_id: theme::typography::code_small(),
                color: if tinted {
                    theme::text().primary
                } else {
                    theme::text().secondary
                },
                ..TextFormat::default()
            },
        );
        return;
    };
    let sections = &highlighted.job.sections;
    let first = sections.partition_point(|section| section.byte_range.end.0 <= range.start);
    for section in &sections[first..] {
        if section.byte_range.start.0 >= range.end {
            break;
        }
        let start = section.byte_range.start.0.max(range.start);
        let end = section.byte_range.end.0.min(range.end);
        if start < end {
            let mut format = section.format.clone();
            format.font_id.size = 12.0;
            if tinted {
                format.color = theme::diff::code(format.color);
            }
            target.append(&highlighted.job.text[start..end], 0.0, format);
        }
    }
}

#[expect(clippy::too_many_arguments)]
pub(super) fn agent_code_galley(
    ui: &mut egui::Ui,
    id: Id,
    path: &Path,
    source: &str,
    width: f32,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
    search: Option<(&str, Option<usize>)>,
) -> Arc<egui::Galley> {
    let width_key = width.round().to_bits();
    if let Some((query, active)) = search {
        let active = active.unwrap_or(usize::MAX);
        let cached = ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentCodeCache>(id);
            (cache.path == path && cache.source == source)
                .then_some(cache.find.as_ref())
                .flatten()
                .filter(|find| find.matches(width_key, query, active))
                .map(|find| Arc::clone(&find.galley))
        });
        if let Some(galley) = cached {
            return galley;
        }
        let syntax = syntaxes.detect(path, false);
        let mut job = highlighter
            .highlight_job(source, syntax, syntaxes.set(), width)
            .unwrap_or_else(|_| plain_text_job(source, width));
        job.wrap.break_anywhere = true;
        for section in &mut job.sections {
            section.format.font_id.size = 12.5;
        }
        let matches = match_spans(&job.text, query);
        let job = find_highlighted_job(&job, &matches, active);
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentCodeCache>(id);
            if cache.path != path || cache.source != source {
                cache.source.clear();
                cache.source.push_str(source);
                cache.path = path.to_path_buf();
                cache.galleys.clear();
            }
            cache.find = Some(AgentFindGalley {
                width: width_key,
                query: query.to_owned(),
                active,
                galley: Arc::clone(&galley),
            });
        });
        return galley;
    }
    let cached = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentCodeCache>(id);
        if cache.path != path || cache.source != source {
            return None;
        }
        cache
            .galleys
            .iter()
            .position(|(key, _)| *key == width_key)
            .map(|index| {
                let entry = cache.galleys.remove(index);
                let galley = Arc::clone(&entry.1);
                cache.galleys.insert(0, entry);
                galley
            })
    });
    if let Some(galley) = cached {
        return galley;
    }
    let syntax = syntaxes.detect(path, false);
    let mut job = highlighter
        .highlight_job(source, syntax, syntaxes.set(), width)
        .unwrap_or_else(|_| plain_text_job(source, width));
    job.wrap.break_anywhere = true;
    for section in &mut job.sections {
        section.format.font_id.size = 12.5;
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentCodeCache>(id);
        if cache.path != path || cache.source != source {
            cache.source.clear();
            cache.source.push_str(source);
            cache.path = path.to_path_buf();
            cache.galleys.clear();
        }
        cache.galleys.insert(0, (width_key, Arc::clone(&galley)));
        cache.galleys.truncate(GALLEY_WIDTH_SLOTS);
    });
    galley
}

#[expect(clippy::too_many_arguments)]
pub(super) fn agent_markdown_galley(
    ui: &mut egui::Ui,
    id: Id,
    source: &str,
    width: f32,
    highlighter: &Highlighter,
    syntaxes: &SyntaxManager,
    bright: bool,
    search: Option<(&str, Option<usize>)>,
) -> Arc<egui::Galley> {
    let width_key = width.round().to_bits();
    if let Some((query, active)) = search {
        let active = active.unwrap_or(usize::MAX);
        let cached = ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
            (cache.source == source)
                .then_some(cache.find.as_ref())
                .flatten()
                .filter(|find| find.matches(width_key, query, active))
                .map(|find| Arc::clone(&find.galley))
        });
        if let Some(galley) = cached {
            return galley;
        }
        let mut job = markdown::compact_layout(source, width, |language, code| {
            let syntax = language
                .map(|language| syntaxes.detect_token(language))
                .unwrap_or_else(|| syntaxes.plain_text());
            highlighter
                .highlight_job(code, syntax, syntaxes.set(), width)
                .ok()
        });
        if bright {
            for section in &mut job.sections {
                if section.format.color == theme::text().secondary {
                    section.format.color = theme::text().primary;
                }
            }
        }
        let matches = match_spans(&job.text, query);
        let job = find_highlighted_job(&job, &matches, active);
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
            if cache.source != source {
                cache.source.clear();
                cache.source.push_str(source);
                cache.galleys.clear();
            }
            cache.find = Some(AgentFindGalley {
                width: width_key,
                query: query.to_owned(),
                active,
                galley: Arc::clone(&galley),
            });
        });
        return galley;
    }
    let cached = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
        if cache.source != source {
            return None;
        }
        cache
            .galleys
            .iter()
            .position(|(key, _)| *key == width_key)
            .map(|index| {
                let entry = cache.galleys.remove(index);
                let galley = Arc::clone(&entry.1);
                cache.galleys.insert(0, entry);
                galley
            })
    });
    if let Some(galley) = cached {
        return galley;
    }
    let mut job = markdown::compact_layout(source, width, |language, code| {
        let syntax = language
            .map(|language| syntaxes.detect_token(language))
            .unwrap_or_else(|| syntaxes.plain_text());
        highlighter
            .highlight_job(code, syntax, syntaxes.set(), width)
            .ok()
    });
    if bright {
        for section in &mut job.sections {
            if section.format.color == theme::text().secondary {
                section.format.color = theme::text().primary;
            }
        }
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
        if cache.source != source {
            cache.source.clear();
            cache.source.push_str(source);
            cache.galleys.clear();
        }
        cache.galleys.insert(0, (width_key, Arc::clone(&galley)));
        cache.galleys.truncate(GALLEY_WIDTH_SLOTS);
    });
    galley
}

pub(super) fn agent_text_job(
    text: &str,
    width: f32,
    font_id: egui::FontId,
    color: Color32,
    search: Option<(&str, Option<usize>)>,
) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font_id, color, width);
    job.wrap.break_anywhere = true;
    if let Some((query, active)) = search {
        job = find_highlighted_job(
            &job,
            &match_spans(text, query),
            active.unwrap_or(usize::MAX),
        );
    }
    job
}

pub(super) fn agent_search_label(
    ui: &mut egui::Ui,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    search: Option<(&str, Option<usize>)>,
) -> egui::Response {
    let job = agent_text_job(text, ui.available_width(), font_id, color, search);
    ui.add(Label::new(job).wrap())
}
