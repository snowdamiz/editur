use std::path::Path;

use egui::{
    Color32, FontId, Stroke, TextFormat,
    text::{LayoutJob, LayoutSection},
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme;

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        })
}

#[derive(Default)]
struct Style {
    heading: Option<HeadingLevel>,
    emphasis: usize,
    strong: usize,
    strikethrough: usize,
    link: usize,
    code_block: usize,
    quote: usize,
}

struct List {
    next: Option<u64>,
}

pub(crate) fn layout(source: &str, wrap_width: f32) -> LayoutJob {
    layout_with_code_highlighting(source, wrap_width, |_, _| None)
}

fn layout_with_code_highlighting(
    source: &str,
    wrap_width: f32,
    mut highlight_code: impl FnMut(Option<&str>, &str) -> Option<LayoutJob>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let mut style = Style::default();
    let mut lists = Vec::<List>::new();
    let mut table_cell = 0;
    let mut code_block = None;
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => style.heading = Some(level),
                Tag::BlockQuote(_) => {
                    ensure_newlines(&mut job, 1);
                    append(&mut job, "│ ", &style, false);
                    style.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    style.code_block += 1;
                    let language = match kind {
                        CodeBlockKind::Indented => None,
                        CodeBlockKind::Fenced(language) => Some(language.to_string()),
                    };
                    code_block = Some((language, job.text.len(), job.sections.len()));
                }
                Tag::List(first) => lists.push(List { next: first }),
                Tag::Item => {
                    ensure_newlines(&mut job, 1);
                    append(&mut job, &"  ".repeat(lists.len()), &style, false);
                    let marker = lists.last_mut().map_or_else(
                        || "• ".to_owned(),
                        |list| match list.next.as_mut() {
                            Some(next) => {
                                let marker = format!("{next}. ");
                                *next += 1;
                                marker
                            }
                            None => "• ".to_owned(),
                        },
                    );
                    append(&mut job, &marker, &style, false);
                }
                Tag::TableHead | Tag::TableRow => table_cell = 0,
                Tag::TableCell => {
                    if table_cell > 0 {
                        append(&mut job, "  |  ", &style, false);
                    }
                    table_cell += 1;
                }
                Tag::Emphasis => style.emphasis += 1,
                Tag::Strong => style.strong += 1,
                Tag::Strikethrough => style.strikethrough += 1,
                Tag::Link { .. } | Tag::Image { .. } => style.link += 1,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    ensure_newlines(&mut job, if lists.is_empty() { 2 } else { 1 })
                }
                TagEnd::Heading(_) => {
                    style.heading = None;
                    ensure_newlines(&mut job, 2);
                }
                TagEnd::BlockQuote(_) => {
                    style.quote = style.quote.saturating_sub(1);
                    ensure_newlines(&mut job, 2);
                }
                TagEnd::CodeBlock => {
                    style.code_block = style.code_block.saturating_sub(1);
                    if let Some((language, start, section_start)) = code_block.take()
                        && let Some(highlighted) =
                            highlight_code(language.as_deref(), &job.text[start..])
                    {
                        job.sections.truncate(section_start);
                        job.sections
                            .extend(highlighted.sections.into_iter().map(|section| {
                                let mut format = section.format;
                                format.background = theme::state::selected();
                                LayoutSection {
                                    leading_space: section.leading_space,
                                    byte_range: (start + section.byte_range.start.0).into()
                                        ..(start + section.byte_range.end.0).into(),
                                    format,
                                }
                            }));
                    }
                    ensure_newlines(&mut job, 2);
                }
                TagEnd::List(_) => {
                    lists.pop();
                    ensure_newlines(&mut job, if lists.is_empty() { 2 } else { 1 });
                }
                TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow => ensure_newlines(&mut job, 1),
                TagEnd::Table => ensure_newlines(&mut job, 2),
                TagEnd::Emphasis => style.emphasis = style.emphasis.saturating_sub(1),
                TagEnd::Strong => style.strong = style.strong.saturating_sub(1),
                TagEnd::Strikethrough => {
                    style.strikethrough = style.strikethrough.saturating_sub(1);
                }
                TagEnd::Link | TagEnd::Image => style.link = style.link.saturating_sub(1),
                _ => {}
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                append(&mut job, &text, &style, false);
            }
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                append(&mut job, &text, &style, true);
            }
            Event::SoftBreak => append(&mut job, " ", &style, false),
            Event::HardBreak => append(&mut job, "\n", &style, false),
            Event::Rule => {
                ensure_newlines(&mut job, 1);
                append(&mut job, "────────────────────────", &style, false);
                ensure_newlines(&mut job, 2);
            }
            Event::TaskListMarker(checked) => {
                append(&mut job, if checked { "☑ " } else { "☐ " }, &style, false);
            }
            Event::FootnoteReference(label) => {
                append(&mut job, &format!("[{label}]"), &style, false);
            }
        }
    }
    while job.text.ends_with("\n\n") {
        job.text.pop();
        if let Some(section) = job.sections.last_mut() {
            section.byte_range.end.0 = section.byte_range.end.0.saturating_sub(1);
        }
    }
    job
}

pub(crate) fn compact_layout(
    source: &str,
    wrap_width: f32,
    highlight_code: impl FnMut(Option<&str>, &str) -> Option<LayoutJob>,
) -> LayoutJob {
    compact(layout_with_code_highlighting(
        source,
        wrap_width,
        highlight_code,
    ))
}

fn compact(mut job: LayoutJob) -> LayoutJob {
    if job.text.ends_with('\n') {
        job.text.pop();
        for section in &mut job.sections {
            section.byte_range.end.0 = section.byte_range.end.0.min(job.text.len());
        }
        job.sections
            .retain(|section| !section.byte_range.is_empty());
    }
    for section in &mut job.sections {
        section.format.font_id.size = section.format.font_id.size.min(20.0);
        section.format.line_height = section.format.line_height.map(|height| height.min(25.0));
    }
    job
}

fn append(job: &mut LayoutJob, text: &str, style: &Style, inline_code: bool) {
    let heading_size = style.heading.map_or(14.0, |level| match level {
        HeadingLevel::H1 => 24.0,
        HeadingLevel::H2 => 21.0,
        HeadingLevel::H3 => 18.0,
        HeadingLevel::H4 => 16.0,
        HeadingLevel::H5 => 15.0,
        HeadingLevel::H6 => 14.0,
    });
    let code = inline_code || style.code_block > 0;
    let color = if style.link > 0 {
        theme::accent()
    } else if style.strong > 0 || style.heading.is_some() {
        theme::text().primary
    } else if style.quote > 0 {
        theme::text().muted
    } else {
        theme::text().secondary
    };
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: if code {
                theme::typography::code_small()
            } else if style.strong > 0 || style.heading.is_some() {
                FontId::new(heading_size, theme::typography::strong_family())
            } else {
                FontId::proportional(heading_size)
            },
            color,
            background: if inline_code {
                theme::state::hover()
            } else if style.code_block > 0 {
                theme::state::selected()
            } else {
                Color32::TRANSPARENT
            },
            italics: style.emphasis > 0,
            underline: if style.link > 0 {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            strikethrough: if style.strikethrough > 0 {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            line_height: Some(if style.heading.is_some() {
                heading_size + 6.0
            } else {
                21.0
            }),
            ..TextFormat::default()
        },
    );
}

fn ensure_newlines(job: &mut LayoutJob, count: usize) {
    if job.text.is_empty() {
        return;
    }
    let missing = count.saturating_sub(job.text.chars().rev().take_while(|c| *c == '\n').count());
    if missing > 0 {
        append(job, &"\n".repeat(missing), &Style::default(), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_common_markdown_as_formatted_text() {
        let job = layout(
            "# Title\n\nRead **carefully** and visit [Editur](https://example.com).\n\n- first\n- second\n\n```rust\nlet answer = 42;\n```",
            600.0,
        );

        assert_eq!(
            job.text,
            "Title\n\nRead carefully and visit Editur.\n\n  • first\n  • second\n\nlet answer = 42;\n"
        );
        let title = job.sections.first().expect("title formatting");
        assert!(title.format.font_id.size > 20.0);
        assert!(job.sections.iter().any(|section| {
            section.format.font_id == theme::typography::code_small()
                && section.format.background != Color32::TRANSPARENT
        }));
    }

    #[test]
    fn renders_table_headers_and_rows_on_separate_lines() {
        let job = layout(
            "| Metric | Result |\n| --- | --- |\n| Startup | Pass |",
            600.0,
        );

        assert_eq!(job.text, "Metric  |  Result\nStartup  |  Pass\n");
    }

    #[test]
    fn preview_uses_proportional_text_and_monospace_code_at_a_compact_scale() {
        let job = layout("# Title\n\nBody text with `code`.", 600.0);

        let font_at = |needle: &str| {
            let offset = job.text.find(needle).unwrap();
            &job.sections
                .iter()
                .find(|section| {
                    section.byte_range.start.0 <= offset && offset < section.byte_range.end.0
                })
                .unwrap()
                .format
                .font_id
        };
        let title = font_at("Title");
        let body = font_at("Body");
        let code = font_at("code");
        assert_eq!(title.family, theme::typography::strong_family());
        assert_eq!(body.family, egui::FontFamily::Proportional);
        assert_eq!(code.family, egui::FontFamily::Monospace);
        assert!(title.size <= 24.0);
        assert!(body.size <= 14.0);
        assert!(code.size <= 13.5);
    }

    #[test]
    fn compact_agent_markdown_uses_readable_body_metrics() {
        let job = compact_layout("Body copy", 320.0, |_, _| None);
        let body = &job.sections[0].format;

        assert!(
            body.font_id.size >= 14.0,
            "agent body was {}px",
            body.font_id.size
        );
        assert!(
            body.line_height.is_some_and(|height| height >= 21.0),
            "agent line box was {:?}",
            body.line_height
        );
    }

    #[test]
    fn compact_agent_markdown_uses_semibold_for_visual_hierarchy() {
        let job = compact_layout("## Result\n\nRead **carefully**.", 320.0, |_, _| None);
        let family_at = |needle: &str| {
            let offset = job.text.find(needle).unwrap();
            job.sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .unwrap()
                .format
                .font_id
                .family
                .clone()
        };

        assert_eq!(family_at("Result"), theme::typography::strong_family());
        assert_eq!(family_at("carefully"), theme::typography::strong_family());
        assert_eq!(family_at("Read"), egui::FontFamily::Proportional);
    }

    #[test]
    fn compact_agent_markdown_indents_list_hierarchy() {
        let job = compact_layout("- top\n  - nested", 320.0, |_, _| None);

        assert_eq!(job.text, "  • top\n    • nested");
    }

    #[test]
    fn inline_code_is_quieter_than_fenced_code() {
        let job = layout("Use `inline`.\n\n```text\nblock\n```", 320.0);
        let background_at = |needle: &str| {
            let offset = job.text.find(needle).unwrap();
            job.sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .unwrap()
                .format
                .background
        };

        assert_eq!(background_at("inline"), theme::state::hover());
        assert_eq!(background_at("block"), theme::state::selected());
    }

    #[test]
    fn compact_agent_markdown_does_not_reserve_a_trailing_blank_line() {
        let job = compact_layout("Response", 320.0, |_, _| None);

        assert_eq!(job.text, "Response");
    }

    #[test]
    fn recognizes_markdown_file_extensions_case_insensitively() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown(Path::new("notes.txt")));
    }
}
