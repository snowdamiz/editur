use crate::theme;
use egui::{
    Color32, TextFormat,
    text::{LayoutJob, LayoutSection},
};
use std::cell::{Ref, RefCell};
use std::path::Path;
use std::str::FromStr;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, HighlightState, Highlighter as SyntectHighlighter, RangedHighlightIterator,
    ScopeSelectors, Style, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const BUILTIN_DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/default_syntaxes.packdump"));

pub struct SyntaxManager {
    set: SyntaxSet,
}

impl SyntaxManager {
    pub fn built_in() -> Result<Self, String> {
        syntect::dumps::from_reader::<SyntaxSet, _>(BUILTIN_DUMP)
            .map(|set| Self { set })
            .map_err(|error| format!("cannot load built-in syntaxes: {error}"))
    }

    pub fn detect(&self, path: &Path, force_plain_text: bool) -> &SyntaxReference {
        if force_plain_text {
            return self.plain_text();
        }
        path.file_name()
            .and_then(|filename| filename.to_str())
            .and_then(|filename| {
                self.set.find_syntax_by_extension(filename).or_else(|| {
                    self.set
                        .find_syntax_by_extension(filename.trim_start_matches('.'))
                })
            })
            .or_else(|| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(|extension| self.set.find_syntax_by_extension(extension))
            })
            .unwrap_or_else(|| self.plain_text())
    }

    pub fn plain_text(&self) -> &SyntaxReference {
        self.set
            .find_syntax_by_name("Plain Text")
            .unwrap_or_else(|| self.set.find_syntax_plain_text())
    }

    pub fn detect_token(&self, token: &str) -> &SyntaxReference {
        self.set
            .find_syntax_by_token(token)
            .unwrap_or_else(|| self.plain_text())
    }

    pub fn set(&self) -> &SyntaxSet {
        &self.set
    }
}

pub struct Highlighter {
    theme: RefCell<(u64, Theme)>,
}

#[derive(Default)]
pub struct IncrementalHighlightCache {
    appearance: u64,
    syntax: String,
    lines: Vec<CachedLine>,
}

struct CachedLine {
    text: String,
    parse_start: ParseState,
    highlight_start: HighlightState,
    parse_end: ParseState,
    highlight_end: HighlightState,
    sections: Vec<(std::ops::Range<usize>, TextFormat)>,
}

impl Highlighter {
    pub fn new() -> Result<Self, String> {
        let roles = theme::syntax();
        let foreground = color(roles.foreground);
        let scopes = [
            ("comment", color(roles.comment), FontStyle::ITALIC),
            ("string", color(roles.string), FontStyle::empty()),
            ("keyword", color(roles.keyword), FontStyle::empty()),
            (
                "storage.type, entity.name.type",
                color(roles.declared_type),
                FontStyle::empty(),
            ),
            (
                "entity.name.macro",
                color(roles.macro_name),
                FontStyle::empty(),
            ),
            (
                "entity.name.function, support.function, variable",
                color(roles.macro_name),
                FontStyle::empty(),
            ),
            (
                "constant.numeric, constant.language",
                color(roles.escape),
                FontStyle::empty(),
            ),
            (
                "text.html entity.name.tag",
                color(roles.keyword),
                FontStyle::empty(),
            ),
            (
                "text.html entity.other.attribute-name",
                color(roles.declared_type),
                FontStyle::empty(),
            ),
            (
                "constant.character.escape",
                color(roles.escape),
                FontStyle::empty(),
            ),
            (
                "markup.heading",
                color(roles.declared_type),
                FontStyle::empty(),
            ),
            ("markup.raw", color(roles.string), FontStyle::empty()),
            ("markup.bold", color(roles.macro_name), FontStyle::BOLD),
            ("markup.italic", color(roles.keyword), FontStyle::ITALIC),
            (
                "markup.underline.link, string.other.link",
                color(roles.link),
                FontStyle::empty(),
            ),
            (
                "punctuation.definition.heading, punctuation.definition.list",
                color(roles.escape),
                FontStyle::empty(),
            ),
        ]
        .into_iter()
        .map(|(selector, foreground, font_style)| {
            ScopeSelectors::from_str(selector)
                .map(|scope| ThemeItem {
                    scope,
                    style: StyleModifier {
                        foreground: Some(foreground),
                        background: None,
                        font_style: Some(font_style),
                    },
                })
                .map_err(|error| format!("invalid built-in theme selector: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            theme: RefCell::new((
                theme::appearance(),
                Theme {
                    name: Some("Editur".into()),
                    author: None,
                    settings: ThemeSettings {
                        foreground: Some(foreground),
                        background: Some(color(theme::surface().editor)),
                        ..ThemeSettings::default()
                    },
                    scopes,
                },
            )),
        })
    }

    fn theme(&self) -> Result<Ref<'_, Theme>, String> {
        let appearance = theme::appearance();
        if self.theme.borrow().0 != appearance {
            self.theme.replace(Self::new()?.theme.into_inner());
        }
        Ok(Ref::map(self.theme.borrow(), |cached| &cached.1))
    }

    pub fn highlight_job(
        &self,
        text: &str,
        syntax: &SyntaxReference,
        set: &SyntaxSet,
        wrap_width: f32,
    ) -> Result<LayoutJob, String> {
        let theme = self.theme()?;
        let mut highlighter = HighlightLines::new(syntax, &theme);
        let mut job = LayoutJob::default();
        job.wrap.max_width = wrap_width;
        for line in LinesWithEndings::from(text) {
            for (style, segment) in highlighter
                .highlight_line(line, set)
                .map_err(|error| format!("cannot highlight {}: {error}", syntax.name))?
            {
                job.append(segment, 0.0, text_format(style));
            }
        }
        Ok(job)
    }

    pub fn highlight_job_incremental(
        &self,
        text: &str,
        syntax: &SyntaxReference,
        set: &SyntaxSet,
        wrap_width: f32,
        cache: &mut IncrementalHighlightCache,
    ) -> Result<LayoutJob, String> {
        let appearance = theme::appearance();
        if cache.syntax != syntax.name || cache.appearance != appearance {
            cache.lines.clear();
            cache.syntax.clone_from(&syntax.name);
            cache.appearance = appearance;
        }
        let theme = self.theme()?;

        let new_lines: Vec<_> = LinesWithEndings::from(text).collect();
        let mut old: Vec<_> = std::mem::take(&mut cache.lines)
            .into_iter()
            .map(Some)
            .collect();
        let prefix = old
            .iter()
            .zip(&new_lines)
            .take_while(|(old, new)| old.as_ref().is_some_and(|old| old.text == **new))
            .count();
        let mut suffix = 0;
        while suffix < old.len().saturating_sub(prefix)
            && suffix < new_lines.len().saturating_sub(prefix)
            && old[old.len() - suffix - 1]
                .as_ref()
                .is_some_and(|old| old.text == new_lines[new_lines.len() - suffix - 1])
        {
            suffix += 1;
        }

        let mut lines = Vec::with_capacity(new_lines.len());
        for line in old.iter_mut().take(prefix) {
            if let Some(line) = line.take() {
                lines.push(line);
            }
        }
        let (mut parse, mut highlight) = lines.last().map_or_else(
            || {
                let highlighter = SyntectHighlighter::new(&theme);
                (
                    ParseState::new(syntax),
                    HighlightState::new(&highlighter, ScopeStack::new()),
                )
            },
            |line| (line.parse_end.clone(), line.highlight_end.clone()),
        );
        let highlighter = SyntectHighlighter::new(&theme);
        let new_suffix_start = new_lines.len() - suffix;
        let old_suffix_start = old.len() - suffix;

        let mut index = prefix;
        while index < new_lines.len() {
            if index >= new_suffix_start {
                let old_index = old_suffix_start + index - new_suffix_start;
                if old[old_index].as_ref().is_some_and(|line| {
                    line.parse_start == parse && line.highlight_start == highlight
                }) {
                    for line in old.iter_mut().skip(old_index) {
                        if let Some(line) = line.take() {
                            lines.push(line);
                        }
                    }
                    break;
                }
            }

            let line = new_lines[index];
            let parse_start = parse.clone();
            let highlight_start = highlight.clone();
            let operations = parse
                .parse_line(line, set)
                .map_err(|error| format!("cannot highlight {}: {error}", syntax.name))?;
            let sections =
                RangedHighlightIterator::new(&mut highlight, &operations, line, &highlighter)
                    .map(|(style, _, range)| (range, text_format(style)))
                    .collect();
            lines.push(CachedLine {
                text: line.to_owned(),
                parse_start,
                highlight_start,
                parse_end: parse.clone(),
                highlight_end: highlight.clone(),
                sections,
            });
            index += 1;
        }
        cache.lines = lines;

        let section_count = cache.lines.iter().map(|line| line.sections.len()).sum();
        let mut job = LayoutJob {
            text: text.to_owned(),
            sections: Vec::with_capacity(section_count),
            ..LayoutJob::default()
        };
        job.wrap.max_width = wrap_width;
        let mut offset = 0;
        for line in &cache.lines {
            for (range, format) in &line.sections {
                let byte_range = (offset + range.start).into()..(offset + range.end).into();
                if let Some(previous) = job.sections.last_mut()
                    && previous.format == *format
                    && previous.byte_range.end == byte_range.start
                {
                    previous.byte_range.end = byte_range.end;
                    continue;
                }
                job.sections.push(LayoutSection {
                    leading_space: 0.0,
                    byte_range,
                    format: format.clone(),
                });
            }
            offset += line.text.len();
        }
        Ok(job)
    }
}

fn text_format(style: Style) -> TextFormat {
    TextFormat {
        font_id: theme::typography::code_editor(),
        color: theme::color::literal(style.foreground.r, style.foreground.g, style.foreground.b),
        italics: style.font_style.contains(FontStyle::ITALIC),
        ..TextFormat::default()
    }
}

/// syntect carries its own color type, so a palette role has to cross over
/// once on the way in and once on the way out.
fn color(color: Color32) -> Color {
    Color {
        r: color.r(),
        g: color.g(),
        b: color.b(),
        a: 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighted(path: &str, source: &str) -> LayoutJob {
        let syntaxes = SyntaxManager::built_in().unwrap();
        Highlighter::new()
            .unwrap()
            .highlight_job(
                source,
                syntaxes.detect(Path::new(path), false),
                syntaxes.set(),
                800.0,
            )
            .unwrap()
    }

    fn token_color(job: &LayoutJob, source: &str, token: &str) -> Color32 {
        let offset = source.find(token).unwrap();
        job.sections
            .iter()
            .find(|section| section.byte_range.contains(&offset.into()))
            .unwrap()
            .format
            .color
    }

    #[test]
    fn detects_previously_optional_syntaxes_without_installing_packages() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        for path in [
            "main.c",
            "main.cpp",
            "main.cs",
            "style.css",
            "Dockerfile",
            ".env",
            "main.go",
            "schema.graphql",
            "index.html",
            "Main.java",
            "main.js",
            "data.json",
            "Main.kt",
            "main.lua",
            "Makefile",
            "README.md",
            "index.php",
            "script.py",
            "Gemfile",
            "script.sh",
            "schema.sql",
            "main.swift",
            "Cargo.toml",
            "main.ts",
            "document.xml",
            "config.yaml",
        ] {
            assert_ne!(
                syntaxes.detect(Path::new(path), false).name,
                "Plain Text",
                "missing built-in syntax for {path}"
            );
        }
        assert_eq!(
            syntaxes.detect(Path::new("main.rs"), true).name,
            "Plain Text"
        );
    }

    #[test]
    fn detects_web_component_extensions() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        for (path, expected) in [
            ("index.html", "HTML"),
            ("component.jsx", "JavaScript"),
            ("component.tsx", "TypeScript"),
            ("component.astro", "Astro"),
            ("component.vue", "HTML"),
            ("component.svelte", "HTML"),
            ("module.mjs", "JavaScript"),
            ("config.cjs", "JavaScript"),
        ] {
            assert_eq!(
                syntaxes.detect(Path::new(path), false).name,
                expected,
                "wrong built-in syntax for {path}"
            );
        }
    }

    #[test]
    fn detects_common_modern_extensions() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        for (path, expected) in [
            ("script.ps1", "PowerShell"),
            ("module.psm1", "PowerShell"),
            ("manifest.psd1", "PowerShell"),
            ("styles.scss", "CSS"),
            ("styles.less", "CSS"),
            ("README.mdx", "Markdown"),
            ("settings.jsonc", "JSON"),
        ] {
            assert_eq!(
                syntaxes.detect(Path::new(path), false).name,
                expected,
                "wrong built-in syntax for {path}"
            );
        }
    }

    #[test]
    fn detects_previously_manifest_only_filenames() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        for path in [
            "Dockerfile.dev",
            "Dockerfile.test",
            "Dockerfile.production",
            "Containerfile",
            "Containerfile.dev",
            ".env.local",
            ".env.example",
            ".env.sample",
            ".env.development",
            ".env.development.local",
            ".env.test",
            ".env.test.local",
            ".env.staging",
            ".env.production",
            ".env.production.local",
            "makefile",
            "GNUmakefile",
            "README",
            "CHANGELOG",
            "SConstruct",
            "SConscript",
            "Rakefile",
            "Guardfile",
            "Vagrantfile",
            ".bashrc",
            ".zshrc",
            ".profile",
            "bashrc",
            "zshrc",
        ] {
            assert_ne!(
                syntaxes.detect(Path::new(path), false).name,
                "Plain Text",
                "missing built-in syntax for {path}"
            );
        }
    }

    #[test]
    fn highlights_representative_rust_constructs() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        let syntax = syntaxes.detect(Path::new("main.rs"), false);
        let source = "fn main() { let value: Type = r####\"raw\"####; println!(\"{}\", value); // comment\n}\n";
        let job = Highlighter::new()
            .unwrap()
            .highlight_job(source, syntax, syntaxes.set(), 800.0)
            .unwrap();
        let color = |token: &str| {
            let offset = source.find(token).unwrap();
            job.sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .unwrap()
                .format
                .color
        };

        let foreground = color("main");
        assert_ne!(color("fn"), foreground);
        assert_ne!(color("Type"), foreground);
        assert_ne!(color("raw"), foreground);
        assert_ne!(color("println"), foreground);
        assert_ne!(color("comment"), foreground);
    }

    #[test]
    fn highlights_representative_markdown_constructs() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        let syntax = syntaxes.detect(Path::new("README.md"), false);
        let source = "# Heading\n\nUse `cargo test` and **bold**.\n";
        let job = Highlighter::new()
            .unwrap()
            .highlight_job(source, syntax, syntaxes.set(), 800.0)
            .unwrap();
        let color = |token: &str| {
            let offset = source.find(token).unwrap();
            job.sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .unwrap()
                .format
                .color
        };

        let foreground = color("Use");
        assert_ne!(color("Heading"), foreground);
        assert_ne!(color("cargo test"), foreground);
        assert_ne!(color("bold"), foreground);
    }

    #[test]
    fn rust_string_with_apostrophe_does_not_leak_into_following_lines() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        let syntax = syntaxes.detect(Path::new("main.rs"), false);
        let source = "fn first() {\n    let error = Err(\"Editur's local instance port\".into());\n}\npub fn second() {}\n";
        let job = Highlighter::new()
            .unwrap()
            .highlight_job(source, syntax, syntaxes.set(), 800.0)
            .unwrap();
        let color_at = |offset: usize| {
            job.sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .unwrap()
                .format
                .color
        };

        let string = color_at(source.find("Editur").unwrap());
        let keyword = color_at(source.rfind("pub").unwrap());
        assert_ne!(string, keyword);
        assert_eq!(keyword, theme::syntax().keyword);
    }

    #[test]
    fn rust_lifetimes_and_character_literals_do_not_leak() {
        let source = "fn borrow<'a>(value: &'a str) -> &'a str { value }\nconst TICK: char = '`';\npub fn after() {}\n";
        let job = highlighted("main.rs", source);

        assert_eq!(token_color(&job, source, "'`'"), theme::syntax().string);
        assert_eq!(token_color(&job, source, "pub"), theme::syntax().keyword);
    }

    #[test]
    fn highlights_html_like_tags_and_attributes() {
        let source = "<main class=\"shell\">Hello</main>\n";
        for path in [
            "index.html",
            "component.astro",
            "component.vue",
            "component.svelte",
        ] {
            let job = highlighted(path, source);
            let foreground = token_color(&job, source, "Hello");
            assert_ne!(
                token_color(&job, source, "main"),
                foreground,
                "missing tag color for {path}"
            );
            assert_ne!(
                token_color(&job, source, "class"),
                foreground,
                "missing attribute color for {path}"
            );
            assert_eq!(token_color(&job, source, "shell"), theme::syntax().string);
        }
    }

    #[test]
    fn highlights_astro_frontmatter() {
        let source = "---\nconst title = \"Hello\";\n---\n<h1>{title}</h1>\n";
        let job = highlighted("component.astro", source);

        assert_ne!(
            token_color(&job, source, "const"),
            token_color(&job, source, "title")
        );
        assert_eq!(token_color(&job, source, "Hello"), theme::syntax().string);
        assert_ne!(
            token_color(&job, source, "h1"),
            token_color(&job, source, "title")
        );
    }

    #[test]
    fn highlights_javascript_and_typescript_in_component_files() {
        let source = "export const view = <Card title=\"Hello\" />;\n";
        for path in ["component.jsx", "component.tsx"] {
            let job = highlighted(path, source);
            assert_eq!(token_color(&job, source, "export"), theme::syntax().keyword);
            assert_eq!(token_color(&job, source, "Hello"), theme::syntax().string);
        }
    }

    #[test]
    fn highlights_representative_powershell_constructs() {
        let source = "param([string]$Name)\n# comment\nWrite-Host \"Hello $Name\"\n$Count = 42\n";
        let job = highlighted("script.ps1", source);

        assert_eq!(token_color(&job, source, "param"), theme::syntax().keyword);
        assert_eq!(
            token_color(&job, source, "string"),
            theme::syntax().declared_type
        );
        assert_eq!(
            token_color(&job, source, "Write-Host"),
            theme::syntax().macro_name
        );
        assert_eq!(token_color(&job, source, "Hello"), theme::syntax().string);
        assert_eq!(
            token_color(&job, source, "comment"),
            theme::syntax().comment
        );
        assert_eq!(
            token_color(&job, source, "$Count"),
            theme::syntax().macro_name
        );
        assert_eq!(token_color(&job, source, "42"), theme::syntax().escape);
    }

    #[test]
    fn highlighter_follows_palette_changes_after_construction() {
        let _flag = theme::PALETTE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        theme::set_light(false);
        let highlighter = Highlighter::new().unwrap();
        theme::set_light(true);
        let syntaxes = SyntaxManager::built_in().unwrap();
        let source = "[package]\nname = \"editur\"\n";
        let job = highlighter
            .highlight_job(
                source,
                syntaxes.detect(Path::new("Cargo.toml"), false),
                syntaxes.set(),
                800.0,
            )
            .unwrap();
        let offset = source.find("name").unwrap();
        let foreground = job
            .sections
            .iter()
            .find(|section| section.byte_range.contains(&offset.into()))
            .unwrap()
            .format
            .color;

        let expected = theme::syntax().foreground;
        let contrast = theme::contrast_ratio(foreground, theme::surface().editor);
        theme::set_light(false);

        assert_eq!(foreground, expected);
        assert!(contrast >= 4.5);
    }

    #[test]
    fn incremental_highlighting_matches_a_fresh_parse_after_line_edits() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        let syntax = syntaxes.detect(Path::new("main.rs"), false);
        let highlighter = Highlighter::new().unwrap();
        let mut cache = IncrementalHighlightCache::default();
        let mut source =
            "fn main() {\n    let text = r#\"hello\"#;\n    println!(\"{text}\");\n}\n".to_owned();

        for replacement in ["world", "world\nagain", "done"] {
            source = source.replacen("hello", replacement, 1);
            let incremental = highlighter
                .highlight_job_incremental(&source, syntax, syntaxes.set(), 800.0, &mut cache)
                .unwrap();
            let fresh = highlighter
                .highlight_job(&source, syntax, syntaxes.set(), 800.0)
                .unwrap();
            assert_eq!(incremental, fresh);
            source = source.replacen(replacement, "hello", 1);
        }
    }
}
