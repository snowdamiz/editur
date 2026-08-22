//! Markdown for two very different surfaces. The document preview parses the
//! source into a block tree and renders each block as a real widget — code on
//! recessed cards, tables as bordered grids, quotes behind a bar — because a
//! single flat galley cannot express any of that. The agent transcript keeps
//! its own compact flat layout at the bottom of this file, tuned for chat
//! density rather than for reading a document.

use std::{collections::HashMap, ops::Range, path::Path, sync::Arc};

use egui::{
    Align, Color32, CornerRadius, CursorIcon, FontId, Label, OpenUrl, OutputCommand, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextFormat, Ui, Vec2,
    text::{LayoutJob, LayoutSection},
};
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::theme;

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        })
}

// ---------------------------------------------------------------------------
// The document model. `parse` builds it once per buffer revision; `show`
// renders it every frame, so the model carries finished layout jobs and the
// renderer only decides widths and placement.
// ---------------------------------------------------------------------------

pub(crate) struct Document {
    blocks: Vec<Block>,
    /// Definitions hoisted to the end of the document, ordered by the number
    /// their first mention earned.
    footnotes: Vec<(usize, Vec<Block>)>,
}

enum Block {
    Heading {
        level: HeadingLevel,
        content: Inline,
    },
    Paragraph {
        content: Inline,
    },
    Code {
        language: Option<String>,
        code: String,
    },
    Quote {
        kind: Option<BlockQuoteKind>,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Table(Table),
    Rule,
}

struct ListItem {
    /// `Some` marks a task-list item with its checked state.
    checked: Option<bool>,
    blocks: Vec<Block>,
}

struct Table {
    alignments: Vec<Alignment>,
    header: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
}

/// A run of styled text plus the spans that activate when clicked. The job
/// carries no wrap width; the renderer sets it from the space it has.
struct Inline {
    job: LayoutJob,
    links: Vec<(Range<usize>, String)>,
}

/// The preview's own type scale. The document reads at full ink — the old
/// preview set body copy in `secondary` and earned the "dim" complaint.
const BODY_SIZE: f32 = 15.0;
const BODY_LINE: f32 = 24.0;
const CODE_SIZE: f32 = 13.0;
const CODE_LINE: f32 = 20.0;

#[derive(Clone, Copy)]
struct BaseText {
    size: f32,
    line: f32,
    color: Color32,
    strong: bool,
}

fn body_text() -> BaseText {
    BaseText {
        size: BODY_SIZE,
        line: BODY_LINE,
        color: theme::text().primary,
        strong: false,
    }
}

fn quote_text() -> BaseText {
    BaseText {
        color: theme::text().secondary,
        ..body_text()
    }
}

fn note_text() -> BaseText {
    BaseText {
        size: 13.0,
        line: 20.0,
        color: theme::text().secondary,
        strong: false,
    }
}

fn cell_text(header: bool) -> BaseText {
    BaseText {
        size: 14.0,
        line: 21.0,
        color: theme::text().primary,
        strong: header,
    }
}

fn heading_text(level: HeadingLevel) -> BaseText {
    let size = match level {
        HeadingLevel::H1 => 28.0,
        HeadingLevel::H2 => 22.0,
        HeadingLevel::H3 => 18.0,
        HeadingLevel::H4 => 16.0,
        HeadingLevel::H5 => 14.0,
        HeadingLevel::H6 => 13.0,
    };
    BaseText {
        size,
        line: (size * 1.35).round(),
        // H6 is the one level that reads as a kicker rather than a title.
        color: if level == HeadingLevel::H6 {
            theme::text().muted
        } else {
            theme::text().primary
        },
        strong: true,
    }
}

struct InlineBuilder {
    job: LayoutJob,
    links: Vec<(Range<usize>, String)>,
    open_links: Vec<(usize, String)>,
    base: BaseText,
    strong: usize,
    emphasis: usize,
    strikethrough: usize,
}

impl InlineBuilder {
    fn new(base: BaseText) -> Self {
        Self {
            job: LayoutJob::default(),
            links: Vec::new(),
            open_links: Vec::new(),
            base,
            strong: 0,
            emphasis: 0,
            strikethrough: 0,
        }
    }

    fn format(&self, code: bool) -> TextFormat {
        let strong = self.base.strong || self.strong > 0;
        let linked = !self.open_links.is_empty();
        let color = if linked {
            theme::accent()
        } else {
            self.base.color
        };
        TextFormat {
            font_id: if code {
                FontId::monospace((self.base.size - 2.0).max(11.0))
            } else if strong {
                FontId::new(self.base.size, theme::typography::strong_family())
            } else {
                FontId::proportional(self.base.size)
            },
            color,
            background: if code {
                theme::state::hover()
            } else {
                Color32::TRANSPARENT
            },
            italics: self.emphasis > 0,
            underline: if linked {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            strikethrough: if self.strikethrough > 0 {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            },
            line_height: Some(self.base.line),
            ..TextFormat::default()
        }
    }

    fn push(&mut self, text: &str, code: bool) {
        self.job.append(text, 0.0, self.format(code));
    }

    /// A footnote reference set as a superscript in the accent color.
    fn push_reference(&mut self, number: usize) {
        self.job.append(
            &format!("[{number}]"),
            1.0,
            TextFormat {
                font_id: FontId::proportional((self.base.size * 0.75).round()),
                color: theme::accent(),
                valign: Align::TOP,
                line_height: Some(self.base.line),
                ..TextFormat::default()
            },
        );
    }

    fn open_link(&mut self, url: String) {
        self.open_links.push((self.job.text.len(), url));
    }

    fn close_link(&mut self) {
        if let Some((start, url)) = self.open_links.pop() {
            let end = self.job.text.len();
            if end > start && !url.is_empty() {
                self.links.push((start..end, url));
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.job.text.is_empty()
    }

    fn finish(mut self) -> Inline {
        while !self.open_links.is_empty() {
            self.close_link();
        }
        Inline {
            job: self.job,
            links: self.links,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing: pulldown-cmark events fold into the block tree through a stack of
// open containers.
// ---------------------------------------------------------------------------

enum Frame {
    Blocks {
        kind: FrameKind,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
}

enum FrameKind {
    Root,
    Quote(Option<BlockQuoteKind>),
    Item { checked: Option<bool> },
    Footnote(String),
}

struct TableBuilder {
    alignments: Vec<Alignment>,
    header: Vec<Inline>,
    rows: Vec<Vec<Inline>>,
    current: Vec<Inline>,
    head: bool,
}

struct DocumentBuilder {
    stack: Vec<Frame>,
    inline: Option<InlineBuilder>,
    table: Option<TableBuilder>,
    code: Option<(Option<String>, String)>,
    numbers: HashMap<String, usize>,
    definitions: Vec<(usize, Vec<Block>)>,
    /// Inside a metadata block: front matter is configuration, not prose.
    hidden: bool,
}

pub(crate) fn parse(source: &str) -> Document {
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut builder = DocumentBuilder {
        stack: vec![Frame::Blocks {
            kind: FrameKind::Root,
            blocks: Vec::new(),
        }],
        inline: None,
        table: None,
        code: None,
        numbers: HashMap::new(),
        definitions: Vec::new(),
        hidden: false,
    };
    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(tag) => builder.start(tag),
            Event::End(tag) => builder.end(tag),
            Event::Text(text) => builder.text(&text),
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                builder.code_span(&code);
            }
            Event::SoftBreak => builder.text(" "),
            Event::HardBreak => builder.text("\n"),
            Event::Rule => {
                builder.flush_paragraph();
                builder.push(Block::Rule);
            }
            Event::TaskListMarker(checked) => builder.task(checked),
            Event::FootnoteReference(label) => {
                let number = builder.number(&label);
                builder.inline().push_reference(number);
            }
            // Raw HTML cannot be rendered; its inner text still arrives as
            // ordinary text events, so dropping the tags loses only markup.
            // `<br>` is the one tag whose whole meaning fits a text layout.
            Event::Html(html) | Event::InlineHtml(html) => builder.html(&html),
        }
    }
    builder.finish()
}

impl DocumentBuilder {
    fn base(&self) -> BaseText {
        for frame in self.stack.iter().rev() {
            match frame {
                Frame::Blocks {
                    kind: FrameKind::Quote(_),
                    ..
                } => return quote_text(),
                Frame::Blocks {
                    kind: FrameKind::Footnote(_),
                    ..
                } => return note_text(),
                _ => {}
            }
        }
        body_text()
    }

    /// The open inline run, creating an implicit paragraph for the tight-list
    /// items and stray inlines that arrive without a paragraph tag.
    fn inline(&mut self) -> &mut InlineBuilder {
        if self.inline.is_none() {
            self.inline = Some(InlineBuilder::new(self.base()));
        }
        self.inline.as_mut().expect("just created")
    }

    fn number(&mut self, label: &str) -> usize {
        let next = self.numbers.len() + 1;
        *self.numbers.entry(label.to_owned()).or_insert(next)
    }

    fn flush_paragraph(&mut self) {
        if let Some(builder) = self.inline.take()
            && !builder.is_empty()
        {
            let content = builder.finish();
            self.push(Block::Paragraph { content });
        }
    }

    fn push(&mut self, block: Block) {
        for frame in self.stack.iter_mut().rev() {
            if let Frame::Blocks { blocks, .. } = frame {
                blocks.push(block);
                return;
            }
        }
    }

    fn task(&mut self, value: bool) {
        for frame in self.stack.iter_mut().rev() {
            if let Frame::Blocks {
                kind: FrameKind::Item { checked },
                ..
            } = frame
            {
                *checked = Some(value);
                return;
            }
        }
    }

    fn text(&mut self, text: &str) {
        if self.hidden {
            return;
        }
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
            return;
        }
        // Text between table cells is structural noise, not content.
        if self.table.is_some() && self.inline.is_none() {
            return;
        }
        self.inline().push(text, false);
    }

    fn code_span(&mut self, code: &str) {
        if !self.hidden {
            self.inline().push(code, true);
        }
    }

    fn html(&mut self, html: &str) {
        if self.inline.is_some() && html.trim_start().starts_with("<br") {
            self.inline().push("\n", false);
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.flush_paragraph();
                self.inline = Some(InlineBuilder::new(self.base()));
            }
            Tag::Heading { level, .. } => {
                self.flush_paragraph();
                self.inline = Some(InlineBuilder::new(heading_text(level)));
            }
            Tag::BlockQuote(kind) => {
                self.flush_paragraph();
                self.stack.push(Frame::Blocks {
                    kind: FrameKind::Quote(kind),
                    blocks: Vec::new(),
                });
            }
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                let language = match kind {
                    CodeBlockKind::Indented => None,
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                };
                self.code = Some((language, String::new()));
            }
            Tag::List(start) => {
                self.flush_paragraph();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.stack.push(Frame::Blocks {
                    kind: FrameKind::Item { checked: None },
                    blocks: Vec::new(),
                });
            }
            Tag::FootnoteDefinition(label) => {
                self.flush_paragraph();
                self.number(&label);
                self.stack.push(Frame::Blocks {
                    kind: FrameKind::Footnote(label.to_string()),
                    blocks: Vec::new(),
                });
            }
            Tag::Table(alignments) => {
                self.flush_paragraph();
                self.table = Some(TableBuilder {
                    alignments,
                    header: Vec::new(),
                    rows: Vec::new(),
                    current: Vec::new(),
                    head: false,
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = true;
                }
            }
            Tag::TableCell => {
                let head = self.table.as_ref().is_some_and(|table| table.head);
                self.inline = Some(InlineBuilder::new(cell_text(head)));
            }
            Tag::Emphasis => self.inline().emphasis += 1,
            Tag::Strong => self.inline().strong += 1,
            Tag::Strikethrough => self.inline().strikethrough += 1,
            // An image's alt text reads as a link to the asset it stands for.
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.inline().open_link(dest_url.to_string());
            }
            Tag::MetadataBlock(_) => self.hidden = true,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_paragraph(),
            TagEnd::Heading(level) => {
                if let Some(builder) = self.inline.take() {
                    let content = builder.finish();
                    self.push(Block::Heading { level, content });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                if let Some(Frame::Blocks {
                    kind: FrameKind::Quote(kind),
                    blocks,
                }) = self.stack.pop()
                {
                    self.push(Block::Quote { kind, blocks });
                }
            }
            TagEnd::CodeBlock => {
                if let Some((language, mut code)) = self.code.take() {
                    while code.ends_with('\n') {
                        code.pop();
                    }
                    self.push(Block::Code { language, code });
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush_paragraph();
                if let Some(Frame::Blocks {
                    kind: FrameKind::Item { checked },
                    blocks,
                }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(ListItem { checked, blocks });
                }
            }
            TagEnd::FootnoteDefinition => {
                self.flush_paragraph();
                if let Some(Frame::Blocks {
                    kind: FrameKind::Footnote(label),
                    blocks,
                }) = self.stack.pop()
                {
                    let number = self.number(&label);
                    self.definitions.push((number, blocks));
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.push(Block::Table(Table {
                        alignments: table.alignments,
                        header: table.header,
                        rows: table.rows,
                    }));
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.header = std::mem::take(&mut table.current);
                    table.head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.current);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(builder) = self.inline.take()
                    && let Some(table) = &mut self.table
                {
                    table.current.push(builder.finish());
                }
            }
            TagEnd::Emphasis => {
                let inline = self.inline();
                inline.emphasis = inline.emphasis.saturating_sub(1);
            }
            TagEnd::Strong => {
                let inline = self.inline();
                inline.strong = inline.strong.saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                let inline = self.inline();
                inline.strikethrough = inline.strikethrough.saturating_sub(1);
            }
            TagEnd::Link | TagEnd::Image => self.inline().close_link(),
            TagEnd::MetadataBlock(_) => self.hidden = false,
            _ => {}
        }
    }

    fn finish(mut self) -> Document {
        self.flush_paragraph();
        while self.stack.len() > 1 {
            match self.stack.pop().expect("stack length checked") {
                Frame::Blocks { kind, blocks } => match kind {
                    FrameKind::Quote(kind) => self.push(Block::Quote { kind, blocks }),
                    FrameKind::Item { checked } => {
                        if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                            items.push(ListItem { checked, blocks });
                        }
                    }
                    FrameKind::Footnote(label) => {
                        let number = self.number(&label);
                        self.definitions.push((number, blocks));
                    }
                    FrameKind::Root => {}
                },
                Frame::List { start, items } => self.push(Block::List { start, items }),
            }
        }
        let blocks = match self.stack.pop() {
            Some(Frame::Blocks { blocks, .. }) => blocks,
            _ => Vec::new(),
        };
        let mut footnotes = self.definitions;
        footnotes.sort_by_key(|(number, _)| *number);
        Document { blocks, footnotes }
    }
}

// ---------------------------------------------------------------------------
// Rendering. Spacing is explicit here rather than inherited from egui's item
// spacing, so a document reads with a deliberate vertical rhythm.
// ---------------------------------------------------------------------------

pub(crate) fn show(
    ui: &mut Ui,
    document: &Document,
    highlight: &mut dyn FnMut(Option<&str>, &str) -> Option<LayoutJob>,
) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let mut renderer = Renderer {
        highlight,
        depth: 0,
    };
    renderer.blocks(ui, &document.blocks, theme::space::LARGE);
    if !document.footnotes.is_empty() {
        ui.add_space(theme::space::XWIDE);
        hairline(ui);
        for (number, blocks) in &document.footnotes {
            ui.add_space(theme::space::MEDIUM);
            renderer.footnote(ui, *number, blocks);
        }
    }
}

struct Renderer<'a> {
    highlight: &'a mut dyn FnMut(Option<&str>, &str) -> Option<LayoutJob>,
    /// List nesting level; picks the bullet glyph.
    depth: usize,
}

/// A table cell laid out at its final column width, or a hole in the grid.
type LaidCell<'a> = Option<(Arc<egui::Galley>, &'a Inline)>;

impl Renderer<'_> {
    fn blocks(&mut self, ui: &mut Ui, blocks: &[Block], gap: f32) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                // A heading opens a section, so it earns extra room above.
                let space = if matches!(block, Block::Heading { .. }) {
                    gap.max(theme::space::XWIDE)
                } else {
                    gap
                };
                ui.add_space(space);
            }
            self.block(ui, block);
        }
    }

    fn block(&mut self, ui: &mut Ui, block: &Block) {
        match block {
            Block::Heading { level, content } => heading(ui, *level, content),
            Block::Paragraph { content } => {
                inline_label(ui, content);
            }
            Block::Code { language, code } => self.code(ui, language.as_deref(), code),
            Block::Quote { kind, blocks } => self.quote(ui, *kind, blocks),
            Block::List { start, items } => self.list(ui, *start, items),
            Block::Table(table) => self.table(ui, table),
            Block::Rule => rule(ui),
        }
    }

    fn code(&mut self, ui: &mut Ui, language: Option<&str>, code: &str) {
        egui::Frame::new()
            .fill(theme::surface().input)
            .stroke(theme::border::hairline())
            .corner_radius(theme::corner(theme::radius::CONTROL))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(language) = language {
                        ui.label(
                            RichText::new(language.to_uppercase())
                                .font(theme::typography::micro())
                                .color(theme::text().muted),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let copy = ui.add(egui::Button::new(
                            RichText::new("Copy")
                                .font(theme::typography::small())
                                .color(theme::text().secondary),
                        ));
                        if copy.clicked() {
                            ui.ctx().output_mut(|output| {
                                output
                                    .commands
                                    .push(OutputCommand::CopyText(code.to_owned()));
                            });
                        }
                    });
                });
                ui.add_space(theme::space::SNUG);
                let mut job =
                    (self.highlight)(language, code).unwrap_or_else(|| plain_code_job(code));
                job.wrap.max_width = ui.available_width().max(1.0);
                job.wrap.break_anywhere = true;
                for section in &mut job.sections {
                    section.format.font_id.size = CODE_SIZE;
                    section.format.line_height = Some(CODE_LINE);
                    // The card provides the surface; per-glyph fills would
                    // stripe it.
                    section.format.background = Color32::TRANSPARENT;
                }
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                ui.add(Label::new(galley).selectable(true));
            });
    }

    fn quote(&mut self, ui: &mut Ui, kind: Option<BlockQuoteKind>, blocks: &[Block]) {
        let (bar, title) = match kind {
            None => (theme::text_disabled(), None),
            Some(kind) => {
                let (label, color) = match kind {
                    BlockQuoteKind::Note => ("Note", theme::semantic().info),
                    BlockQuoteKind::Tip => ("Tip", theme::semantic().success),
                    BlockQuoteKind::Important => ("Important", theme::accent()),
                    BlockQuoteKind::Warning => ("Warning", theme::semantic().warning),
                    BlockQuoteKind::Caution => ("Caution", theme::semantic().danger),
                };
                (color, Some((label, theme::ink(color))))
            }
        };
        let response = ui
            .horizontal_top(|ui| {
                ui.add_space(theme::space::LARGE);
                ui.vertical(|ui| {
                    if let Some((label, ink)) = title {
                        ui.label(
                            RichText::new(label)
                                .font(theme::typography::strong())
                                .color(ink),
                        );
                        ui.add_space(theme::space::TIGHT);
                    }
                    self.blocks(ui, blocks, theme::space::MEDIUM);
                });
            })
            .response;
        let rect = response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
            2.0,
            bar,
        );
    }

    fn list(&mut self, ui: &mut Ui, start: Option<u64>, items: &[ListItem]) {
        // One marker column for the whole list, wide enough for its last
        // number, so wrapped item text keeps a clean hanging indent.
        let widest = match start {
            Some(start) => {
                let last = start + items.len().saturating_sub(1) as u64;
                marker_galley(ui, &format!("{last}."), BODY_SIZE, BODY_LINE)
                    .size()
                    .x
            }
            None => {
                marker_galley(ui, bullet(self.depth), BODY_SIZE, BODY_LINE)
                    .size()
                    .x
            }
        };
        let column = (widest + theme::space::SMALL).max(22.0);
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                ui.add_space(theme::space::SNUG);
            }
            ui.horizontal_top(|ui| {
                let line = first_line_height(&item.blocks);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(column, line), Sense::hover());
                match item.checked {
                    Some(checked) => draw_checkbox(ui, rect, line, checked),
                    None => {
                        let text = match start {
                            Some(start) => format!("{}.", start + index as u64),
                            None => bullet(self.depth).to_owned(),
                        };
                        let galley = marker_galley(ui, &text, BODY_SIZE, line);
                        let pos = Pos2::new(
                            rect.right() - theme::space::SMALL - galley.size().x,
                            rect.top(),
                        );
                        ui.painter().galley(pos, galley, theme::text().secondary);
                    }
                }
                ui.vertical(|ui| {
                    self.depth += 1;
                    self.blocks(ui, &item.blocks, theme::space::SMALL);
                    self.depth -= 1;
                });
            });
        }
    }

    fn table(&mut self, ui: &mut Ui, table: &Table) {
        const PAD_X: f32 = 12.0;
        const PAD_Y: f32 = 6.0;
        let columns = table
            .alignments
            .len()
            .max(table.header.len())
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        let rows: Vec<&[Inline]> = std::iter::once(table.header.as_slice())
            .filter(|header| !header.is_empty())
            .chain(table.rows.iter().map(Vec::as_slice))
            .collect();
        if columns == 0 || rows.is_empty() {
            return;
        }
        // Columns take their natural width and shrink proportionally when
        // the document column cannot hold them.
        let mut widths = vec![2.0 * PAD_X + 8.0; columns];
        for row in &rows {
            for (column, cell) in row.iter().enumerate() {
                let mut job = cell.job.clone();
                job.wrap.max_width = f32::INFINITY;
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                widths[column] = widths[column].max(galley.size().x + 2.0 * PAD_X);
            }
        }
        let available = ui.available_width();
        let total: f32 = widths.iter().sum();
        if total > available {
            let scale = available / total;
            for width in &mut widths {
                *width = (*width * scale).max(48.0);
            }
        }
        let total: f32 = widths.iter().sum();
        let mut cells: Vec<Vec<LaidCell<'_>>> = Vec::with_capacity(rows.len());
        let mut heights = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut laid = Vec::with_capacity(columns);
            let mut height = cell_text(false).line;
            for (column, width) in widths.iter().enumerate() {
                match row.get(column) {
                    Some(cell) => {
                        let mut job = cell.job.clone();
                        job.wrap.max_width = (width - 2.0 * PAD_X).max(1.0);
                        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                        height = height.max(galley.size().y);
                        laid.push(Some((galley, cell)));
                    }
                    None => laid.push(None),
                }
            }
            heights.push(height + 2.0 * PAD_Y);
            cells.push(laid);
        }
        let table_height: f32 = heights.iter().sum();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(total, table_height), Sense::hover());
        if !table.header.is_empty() {
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(total, heights[0])),
                CornerRadius {
                    nw: theme::radius::CONTROL,
                    ne: theme::radius::CONTROL,
                    sw: 0,
                    se: 0,
                },
                theme::state::hover(),
            );
        }
        let mut y = rect.top();
        for height in &heights[..heights.len() - 1] {
            y += height;
            ui.painter()
                .hline(rect.x_range(), y, theme::border::hairline());
        }
        let mut x = rect.left();
        for width in &widths[..widths.len() - 1] {
            x += width;
            ui.painter()
                .vline(x, rect.y_range(), theme::border::hairline());
        }
        ui.painter().rect_stroke(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::border::hairline(),
            StrokeKind::Inside,
        );
        let mut y = rect.top();
        for (row_index, row) in cells.iter().enumerate() {
            let mut x = rect.left();
            for (column, cell) in row.iter().enumerate() {
                if let Some((galley, inline)) = cell {
                    let alignment = table
                        .alignments
                        .get(column)
                        .copied()
                        .unwrap_or(Alignment::None);
                    let width = galley.size().x;
                    let left = match alignment {
                        Alignment::Center => x + (widths[column] - width) * 0.5,
                        Alignment::Right => x + widths[column] - PAD_X - width,
                        Alignment::None | Alignment::Left => x + PAD_X,
                    };
                    let cell_rect = Rect::from_min_size(Pos2::new(left, y + PAD_Y), galley.size());
                    let response = ui.put(
                        cell_rect,
                        Label::new(Arc::clone(galley))
                            .selectable(true)
                            .sense(Sense::click_and_drag()),
                    );
                    handle_links(ui, &response, galley, inline);
                }
                x += widths[column];
            }
            y += heights[row_index];
        }
    }

    fn footnote(&mut self, ui: &mut Ui, number: usize, blocks: &[Block]) {
        ui.horizontal_top(|ui| {
            let line = first_line_height(blocks);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(24.0, line), Sense::hover());
            let galley = marker_galley(ui, &format!("{number}."), note_text().size, line);
            let pos = Pos2::new(
                rect.right() - theme::space::SNUG - galley.size().x,
                rect.top(),
            );
            ui.painter().galley(pos, galley, theme::text().muted);
            ui.vertical(|ui| {
                self.blocks(ui, blocks, theme::space::SMALL);
            });
        });
    }
}

fn heading(ui: &mut Ui, level: HeadingLevel, content: &Inline) {
    inline_label(ui, content);
    // The two top levels close with a rule, the classic document divider.
    if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
        ui.add_space(theme::space::SNUG);
        hairline(ui);
    }
}

fn inline_label(ui: &mut Ui, content: &Inline) -> egui::Response {
    let mut job = content.job.clone();
    job.wrap.max_width = ui.available_width().max(1.0);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let response = ui.add(
        Label::new(Arc::clone(&galley))
            .selectable(true)
            .sense(Sense::click_and_drag()),
    );
    handle_links(ui, &response, &galley, content);
    response
}

fn handle_links(ui: &Ui, response: &egui::Response, galley: &egui::Galley, content: &Inline) {
    if content.links.is_empty() {
        return;
    }
    let link_at = |pos: Pos2| {
        let character = galley.cursor_from_pos(pos - response.rect.min).index.0;
        let byte = galley
            .text()
            .char_indices()
            .nth(character)
            .map_or_else(|| galley.text().len(), |(byte, _)| byte);
        content
            .links
            .iter()
            .find(|(range, _)| range.contains(&byte))
            .map(|(_, url)| url.as_str())
    };
    let Some(url) = response.hover_pos().and_then(link_at) else {
        return;
    };
    ui.ctx()
        .output_mut(|output| output.cursor_icon = CursorIcon::PointingHand);
    if response.clicked() {
        ui.ctx().open_url(OpenUrl::new_tab(url));
    }
}

fn plain_code_job(code: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        code,
        0.0,
        TextFormat {
            font_id: FontId::monospace(CODE_SIZE),
            color: theme::syntax().foreground,
            line_height: Some(CODE_LINE),
            ..TextFormat::default()
        },
    );
    job
}

fn marker_galley(ui: &Ui, text: &str, size: f32, line: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::proportional(size),
            color: theme::text().secondary,
            // The same line box as the item's first row, so marker and text
            // share a baseline.
            line_height: Some(line),
            ..TextFormat::default()
        },
    );
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn bullet(depth: usize) -> &'static str {
    ["•", "◦", "▪"][depth.min(2)]
}

fn first_line_height(blocks: &[Block]) -> f32 {
    match blocks.first() {
        Some(Block::Paragraph { content } | Block::Heading { content, .. }) => content
            .job
            .sections
            .first()
            .and_then(|section| section.format.line_height)
            .unwrap_or(BODY_LINE),
        _ => BODY_LINE,
    }
}

fn draw_checkbox(ui: &Ui, rect: Rect, line: f32, checked: bool) {
    let size = 14.0;
    let center = Pos2::new(
        rect.right() - theme::space::SMALL - size * 0.5,
        rect.top() + line * 0.5,
    );
    let boxed = Rect::from_center_size(center, Vec2::splat(size));
    let painter = ui.painter();
    if checked {
        painter.rect_filled(boxed, 4.0, theme::accent());
        let stroke = Stroke::new(theme::stroke::CARET, theme::text().on_accent);
        painter.line_segment(
            [center + Vec2::new(-3.4, 0.2), center + Vec2::new(-1.0, 2.6)],
            stroke,
        );
        painter.line_segment(
            [center + Vec2::new(-1.0, 2.6), center + Vec2::new(3.6, -2.8)],
            stroke,
        );
    } else {
        painter.rect_stroke(boxed, 4.0, theme::border::strong(), StrokeKind::Inside);
    }
}

fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::stroke::DIVIDER),
        Sense::hover(),
    );
    ui.painter()
        .hline(rect.x_range(), rect.center().y, theme::border::hairline());
}

fn rule(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 1.0, theme::border::hairline_color());
}

// ---------------------------------------------------------------------------
// The agent transcript's compact flat layout. Chat bubbles want density and a
// single selectable galley, not a document's block rhythm.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CompactStyle {
    heading: Option<HeadingLevel>,
    emphasis: usize,
    strong: usize,
    strikethrough: usize,
    link: usize,
    code_block: usize,
    quote: usize,
}

struct CompactList {
    next: Option<u64>,
}

pub(crate) fn compact_layout(
    source: &str,
    wrap_width: f32,
    highlight_code: impl FnMut(Option<&str>, &str) -> Option<LayoutJob>,
) -> LayoutJob {
    compact(compact_source_layout(source, wrap_width, highlight_code))
}

fn compact_source_layout(
    source: &str,
    wrap_width: f32,
    mut highlight_code: impl FnMut(Option<&str>, &str) -> Option<LayoutJob>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let mut style = CompactStyle::default();
    let mut lists = Vec::<CompactList>::new();
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
                Tag::List(first) => lists.push(CompactList { next: first }),
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

fn append(job: &mut LayoutJob, text: &str, style: &CompactStyle, inline_code: bool) {
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
        append(job, &"\n".repeat(missing), &CompactStyle::default(), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::RawInput;

    fn paragraph(block: &Block) -> &Inline {
        match block {
            Block::Paragraph { content } => content,
            _ => panic!("expected a paragraph block"),
        }
    }

    fn format_at<'a>(inline: &'a Inline, needle: &str) -> &'a TextFormat {
        let offset = inline.job.text.find(needle).expect("needle present");
        &inline
            .job
            .sections
            .iter()
            .find(|section| section.byte_range.contains(&offset.into()))
            .expect("section covers needle")
            .format
    }

    #[test]
    fn parses_headings_body_and_fenced_code_into_blocks() {
        let document =
            parse("# Title\n\nRead **carefully** now.\n\n```rust\nlet answer = 42;\n```");

        assert_eq!(document.blocks.len(), 3);
        let Block::Heading { level, content } = &document.blocks[0] else {
            panic!("expected a heading");
        };
        assert_eq!(*level, HeadingLevel::H1);
        assert_eq!(content.job.text, "Title");
        let title = &content.job.sections[0].format;
        assert!(title.font_id.size >= 24.0);
        assert_eq!(title.font_id.family, theme::typography::strong_family());
        let body = paragraph(&document.blocks[1]);
        assert_eq!(body.job.text, "Read carefully now.");
        assert_eq!(
            format_at(body, "carefully").font_id.family,
            theme::typography::strong_family()
        );
        assert_eq!(
            format_at(body, "Read").font_id.family,
            egui::FontFamily::Proportional
        );
        let Block::Code { language, code } = &document.blocks[2] else {
            panic!("expected a code block");
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(code, "let answer = 42;");
    }

    #[test]
    fn body_text_reads_at_full_strength_rather_than_dim_gray() {
        let document = parse("Plain body copy.");

        let format = format_at(paragraph(&document.blocks[0]), "Plain");
        assert_eq!(format.color, theme::text().primary);
        assert!(format.line_height.is_some_and(|line| line >= 22.0));
        assert!(format.font_id.size >= 15.0);
    }

    #[test]
    fn tables_become_structured_grids_with_alignment() {
        let document = parse("| Metric | Result |\n| :-- | --: |\n| Startup | Pass |");

        let Block::Table(table) = &document.blocks[0] else {
            panic!("expected a table");
        };
        assert_eq!(table.alignments, vec![Alignment::Left, Alignment::Right]);
        let header: Vec<_> = table
            .header
            .iter()
            .map(|cell| cell.job.text.as_str())
            .collect();
        assert_eq!(header, ["Metric", "Result"]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][0].job.text, "Startup");
        assert_eq!(
            table.header[0].job.sections[0].format.font_id.family,
            theme::typography::strong_family(),
            "header cells must carry weight"
        );
        assert_eq!(
            table.rows[0][0].job.sections[0].format.font_id.family,
            egui::FontFamily::Proportional
        );
    }

    #[test]
    fn links_carry_their_destinations_for_activation() {
        let document = parse("Visit [Editur](https://example.com) today.");

        let content = paragraph(&document.blocks[0]);
        let (range, url) = &content.links[0];
        assert_eq!(&content.job.text[range.clone()], "Editur");
        assert_eq!(url, "https://example.com");
        let linked = format_at(content, "Editur");
        assert_eq!(linked.color, theme::accent());
        assert!(linked.underline.width > 0.0);
    }

    #[test]
    fn task_lists_and_nested_lists_keep_their_structure() {
        let document = parse("- [x] shipped\n- [ ] pending\n  1. first step");

        let Block::List { start: None, items } = &document.blocks[0] else {
            panic!("expected an unordered list");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].checked, Some(true));
        assert_eq!(items[1].checked, Some(false));
        let Block::List {
            start: Some(1),
            items: nested,
        } = &items[1].blocks[1]
        else {
            panic!("expected a nested ordered list");
        };
        assert_eq!(paragraph(&nested[0].blocks[0]).job.text, "first step");
    }

    #[test]
    fn github_alerts_carry_their_kind() {
        let document = parse("> [!WARNING]\n> Mind the ledge.");

        let Block::Quote {
            kind: Some(BlockQuoteKind::Warning),
            blocks,
        } = &document.blocks[0]
        else {
            panic!("expected a warning alert");
        };
        assert_eq!(paragraph(&blocks[0]).job.text, "Mind the ledge.");
    }

    #[test]
    fn quoted_text_recedes_to_secondary_ink() {
        let document = parse("> quoted wisdom");

        let Block::Quote { kind: None, blocks } = &document.blocks[0] else {
            panic!("expected a plain quote");
        };
        assert_eq!(
            format_at(paragraph(&blocks[0]), "quoted").color,
            theme::text().secondary
        );
    }

    #[test]
    fn inline_code_keeps_a_quiet_chip_and_monospace_face() {
        let document = parse("Use `cargo test` often.");

        let content = paragraph(&document.blocks[0]);
        let code = format_at(content, "cargo");
        assert_eq!(code.font_id.family, egui::FontFamily::Monospace);
        assert_eq!(code.background, theme::state::hover());
        assert_eq!(format_at(content, "Use").background, Color32::TRANSPARENT);
    }

    #[test]
    fn html_comments_vanish_and_line_breaks_survive() {
        let document = parse("before<!-- secret -->after\n\nfirst<br>second");

        assert_eq!(paragraph(&document.blocks[0]).job.text, "beforeafter");
        assert_eq!(paragraph(&document.blocks[1]).job.text, "first\nsecond");
    }

    #[test]
    fn footnotes_number_references_and_collect_definitions() {
        let document = parse("Claim[^src].\n\n[^src]: The receipts.");

        let content = paragraph(&document.blocks[0]);
        assert!(content.job.text.contains("[1]"));
        assert_eq!(document.footnotes.len(), 1);
        assert_eq!(document.footnotes[0].0, 1);
        assert_eq!(
            paragraph(&document.footnotes[0].1[0]).job.text,
            "The receipts."
        );
    }

    #[test]
    fn yaml_front_matter_stays_out_of_the_preview() {
        let document = parse("---\ntitle: Draft\n---\n\nActual body.");

        assert_eq!(document.blocks.len(), 1);
        assert_eq!(paragraph(&document.blocks[0]).job.text, "Actual body.");
    }

    #[test]
    fn smart_punctuation_typesets_quotes_and_dashes() {
        let document = parse("\"Ready\" -- go");

        assert_eq!(
            paragraph(&document.blocks[0]).job.text,
            "\u{201c}Ready\u{201d} \u{2013} go"
        );
    }

    #[test]
    fn a_thematic_break_becomes_a_rule_block() {
        let document = parse("above\n\n---\n\nbelow");

        assert!(matches!(document.blocks[1], Block::Rule));
        assert_eq!(document.blocks.len(), 3);
    }

    fn raw_input() -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            ..RawInput::default()
        }
    }

    fn collect_shapes(shape: &egui::Shape, into: &mut Vec<egui::Shape>) {
        if let egui::Shape::Vec(shapes) = shape {
            for inner in shapes {
                collect_shapes(inner, into);
            }
        } else {
            into.push(shape.clone());
        }
    }

    fn flatten(output: &egui::FullOutput) -> Vec<egui::Shape> {
        let mut shapes = Vec::new();
        for clipped in &output.shapes {
            collect_shapes(&clipped.shape, &mut shapes);
        }
        shapes
    }

    #[test]
    fn fenced_code_sits_on_a_recessed_card_and_reaches_the_highlighter() {
        let context = theme::test_context();
        let document = parse("```rust\nlet x = 1;\n```");
        let highlighted = std::cell::Cell::new(false);

        let output = context.run_ui(raw_input(), |ui| {
            show(ui, &document, &mut |language, code| {
                highlighted.set(true);
                assert_eq!(language, Some("rust"));
                assert_eq!(code, "let x = 1;");
                None
            });
        });

        assert!(
            highlighted.get(),
            "the preview must offer code for syntax color"
        );
        assert!(
            flatten(&output).iter().any(|shape| matches!(
                shape,
                egui::Shape::Rect(rect) if rect.fill == theme::surface().input
            )),
            "fenced code must sit on its own card"
        );
    }

    #[test]
    fn tables_paint_a_real_grid_rather_than_piped_text() {
        let context = theme::test_context();
        let document = parse("| Metric | Result |\n| --- | --- |\n| Startup | Pass |");

        let output = context.run_ui(raw_input(), |ui| {
            show(ui, &document, &mut |_, _| None);
        });

        let shapes = flatten(&output);
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::Shape::Rect(rect) if rect.fill == theme::state::hover()
            )),
            "the header row needs its own fill"
        );
        assert!(
            shapes
                .iter()
                .filter(|shape| matches!(shape, egui::Shape::LineSegment { .. }))
                .count()
                >= 2,
            "rows and columns need separator lines"
        );
        let texts: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text == "Metric"));
        assert!(texts.iter().any(|text| text == "Startup"));
    }

    #[test]
    fn a_full_document_renders_at_any_width_without_layout_panics() {
        let source = "---\ntitle: Sink\n---\n\n# Everything\n\nBody with `code`, **bold**, \
                      [link](https://example.com) and a note[^n].\n\n> [!TIP]\n> Nested:\n> \
                      - [x] done\n>   1. deep\n\n| Long header column | B |\n| --- | --: |\n| \
                      a very long cell that must shrink | 2 |\n\n---\n\n```rust\nlet x = \
                      1;\n```\n\n[^n]: Fine print.";
        let document = parse(source);
        let context = theme::test_context();

        for width in [96.0_f32, 320.0, 800.0] {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 600.0))),
                    ..RawInput::default()
                },
                |ui| {
                    show(ui, &document, &mut |_, _| None);
                },
            );
        }
    }

    #[test]
    fn clicking_a_link_opens_the_destination() {
        let context = theme::test_context();
        let document = parse("[Editur](https://example.com)");
        let draw = |events: Vec<egui::Event>| {
            context.run_ui(
                RawInput {
                    events,
                    ..raw_input()
                },
                |ui| {
                    show(ui, &document, &mut |_, _| None);
                },
            )
        };
        let pos = egui::pos2(12.0, 10.0);

        let _ = draw(vec![egui::Event::PointerMoved(pos)]);
        let hover = draw(Vec::new());
        assert_eq!(
            hover.platform_output.cursor_icon,
            CursorIcon::PointingHand,
            "a link must invite the click"
        );
        let _ = draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }]);
        let clicked = draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }]);

        assert!(
            clicked
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(
                    command,
                    OutputCommand::OpenUrl(open) if open.url == "https://example.com"
                ))
        );
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
