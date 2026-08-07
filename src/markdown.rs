use std::path::Path;

use egui::{Color32, FontId, Stroke, TextFormat, text::LayoutJob};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let mut style = Style::default();
    let mut lists = Vec::<List>::new();
    let mut table_cell = 0;
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
                Tag::CodeBlock(_) => style.code_block += 1,
                Tag::List(first) => lists.push(List { next: first }),
                Tag::Item => {
                    ensure_newlines(&mut job, 1);
                    append(
                        &mut job,
                        &"  ".repeat(lists.len().saturating_sub(1)),
                        &style,
                        false,
                    );
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

pub(crate) fn compact_layout(source: &str, wrap_width: f32) -> LayoutJob {
    let mut job = layout(source, wrap_width);
    for section in &mut job.sections {
        section.format.font_id.size = (section.format.font_id.size * 0.85).min(18.0);
        section.format.line_height = section
            .format
            .line_height
            .map(|height| (height * 0.82).min(23.0));
    }
    job
}

fn append(job: &mut LayoutJob, text: &str, style: &Style, inline_code: bool) {
    let heading_size = style.heading.map_or(15.0, |level| match level {
        HeadingLevel::H1 => 28.0,
        HeadingLevel::H2 => 24.0,
        HeadingLevel::H3 => 21.0,
        HeadingLevel::H4 => 18.0,
        HeadingLevel::H5 => 16.0,
        HeadingLevel::H6 => 15.0,
    });
    let code = inline_code || style.code_block > 0;
    let color = if style.link > 0 {
        Color32::from_rgb(105, 213, 230)
    } else if style.strong > 0 || style.heading.is_some() {
        Color32::from_rgb(238, 240, 246)
    } else if style.quote > 0 {
        Color32::from_rgb(170, 177, 193)
    } else {
        Color32::from_rgb(210, 214, 224)
    };
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: if code {
                FontId::monospace(14.0)
            } else {
                FontId::monospace(heading_size)
            },
            color,
            background: if code {
                Color32::from_rgb(38, 38, 42)
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
                heading_size + 8.0
            } else {
                23.0
            }),
            ..TextFormat::default()
        },
    );
}

fn ensure_newlines(job: &mut LayoutJob, count: usize) {
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
            "Title\n\nRead carefully and visit Editur.\n\n• first\n• second\n\nlet answer = 42;\n"
        );
        let title = job.sections.first().expect("title formatting");
        assert!(title.format.font_id.size > 20.0);
        assert!(job.sections.iter().any(|section| {
            section.format.font_id == FontId::monospace(14.0)
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
    fn preview_uses_the_editors_readable_monospace_font() {
        let job = layout("# Title\n\nBody text with `code`.", 600.0);

        assert!(
            job.sections
                .iter()
                .all(|section| { section.format.font_id.family == egui::FontFamily::Monospace })
        );
    }

    #[test]
    fn recognizes_markdown_file_extensions_case_insensitively() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown(Path::new("notes.txt")));
    }
}
