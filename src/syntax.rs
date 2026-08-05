use crate::syntax::package::{Manifest, PackageManager};
use egui::{
    Color32, FontId, TextFormat,
    text::{LayoutJob, LayoutSection},
};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, HighlightState, Highlighter as SyntectHighlighter, RangedHighlightIterator,
    ScopeSelectors, Style, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

pub mod package;

const BUILTIN_DUMP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/default_syntaxes.packdump"));

pub struct SyntaxManager {
    set: SyntaxSet,
    installed: Vec<Manifest>,
}

impl SyntaxManager {
    pub fn built_in() -> Result<Self, String> {
        syntect::dumps::from_reader::<SyntaxSet, _>(BUILTIN_DUMP)
            .map(|set| Self {
                set,
                installed: Vec::new(),
            })
            .map_err(|error| format!("cannot load built-in syntaxes: {error}"))
    }

    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let packages = PackageManager::new(data_dir.to_path_buf());
        let installed = packages.installed()?;
        let cache = packages.cache_path();
        let set = if cache.is_file() {
            syntect::dumps::from_dump_file(&cache)
                .map_err(|error| format!("cannot load {}: {error}", cache.display()))?
        } else if installed.is_empty() {
            syntect::dumps::from_reader::<SyntaxSet, _>(BUILTIN_DUMP)
                .map_err(|error| format!("cannot load built-in syntaxes: {error}"))?
        } else {
            return Err("installed syntax cache is missing; reinstall a syntax package".into());
        };
        Ok(Self { set, installed })
    }

    pub fn detect(&self, path: &Path, force_plain_text: bool) -> &SyntaxReference {
        if force_plain_text {
            return self.plain_text();
        }
        let filename = path.file_name().and_then(|name| name.to_str());
        let extension = path.extension().and_then(|extension| extension.to_str());
        if let Some(manifest) = self.installed.iter().find(|manifest| {
            filename
                .is_some_and(|filename| manifest.filenames.iter().any(|mapped| mapped == filename))
                || extension.is_some_and(|extension| {
                    manifest
                        .extensions
                        .iter()
                        .any(|mapped| mapped.eq_ignore_ascii_case(extension))
                })
        }) && let Some(syntax) = self.set.find_syntax_by_name(&manifest.display_name)
        {
            return syntax;
        }
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(|extension| self.set.find_syntax_by_extension(extension))
            .unwrap_or_else(|| self.plain_text())
    }

    pub fn plain_text(&self) -> &SyntaxReference {
        self.set
            .find_syntax_by_name("Plain Text")
            .unwrap_or_else(|| self.set.find_syntax_plain_text())
    }

    pub fn set(&self) -> &SyntaxSet {
        &self.set
    }
}

pub fn data_dir() -> Result<PathBuf, String> {
    directories::ProjectDirs::from("io", "editur", "Editur")
        .map(|directories| directories.data_dir().to_path_buf())
        .ok_or_else(|| "cannot determine the application data directory".to_owned())
}

pub struct Highlighter {
    theme: Theme,
}

#[derive(Default)]
pub struct IncrementalHighlightCache {
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
        let foreground = Color {
            r: 210,
            g: 215,
            b: 225,
            a: 255,
        };
        let scopes = [
            ("comment", color(107, 116, 136), FontStyle::ITALIC),
            ("string", color(152, 195, 121), FontStyle::empty()),
            ("keyword", color(198, 120, 221), FontStyle::empty()),
            (
                "storage.type, entity.name.type",
                color(97, 175, 239),
                FontStyle::empty(),
            ),
            (
                "entity.name.macro",
                color(229, 192, 123),
                FontStyle::empty(),
            ),
            (
                "constant.character.escape",
                color(86, 182, 194),
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
            theme: Theme {
                name: Some("Editur".into()),
                author: None,
                settings: ThemeSettings {
                    foreground: Some(foreground),
                    background: Some(color(30, 33, 39)),
                    ..ThemeSettings::default()
                },
                scopes,
            },
        })
    }

    pub fn highlight_job(
        &self,
        text: &str,
        syntax: &SyntaxReference,
        set: &SyntaxSet,
        wrap_width: f32,
    ) -> Result<LayoutJob, String> {
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
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
        if cache.syntax != syntax.name {
            cache.lines.clear();
            cache.syntax.clone_from(&syntax.name);
        }

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
                let highlighter = SyntectHighlighter::new(&self.theme);
                (
                    ParseState::new(syntax),
                    HighlightState::new(&highlighter, ScopeStack::new()),
                )
            },
            |line| (line.parse_end.clone(), line.highlight_end.clone()),
        );
        let highlighter = SyntectHighlighter::new(&self.theme);
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
        font_id: FontId::monospace(14.0),
        color: Color32::from_rgba_unmultiplied(
            style.foreground.r,
            style.foreground.g,
            style.foreground.b,
            style.foreground.a,
        ),
        italics: style.font_style.contains(FontStyle::ITALIC),
        ..TextFormat::default()
    }
}

const fn color(r: u8, g: u8, b: u8) -> Color {
    Color { r, g, b, a: 255 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::package::{Manifest, PackageManager};
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    #[test]
    fn detects_only_built_in_rust_and_plain_text() {
        let syntaxes = SyntaxManager::built_in().unwrap();
        assert_eq!(syntaxes.set.syntaxes().len(), 2);
        assert_eq!(syntaxes.detect(Path::new("main.rs"), false).name, "Rust");
        assert_eq!(
            syntaxes.detect(Path::new("notes.py"), false).name,
            "Plain Text"
        );
        assert_eq!(
            syntaxes.detect(Path::new("main.rs"), true).name,
            "Plain Text"
        );
    }

    #[test]
    fn loads_installed_extension_and_exact_filename_mappings() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let manifest = Manifest {
            format_version: 1,
            id: "python".into(),
            display_name: "Python".into(),
            version: "1.0.0".into(),
            minimum_editur_version: "0.1.0".into(),
            extensions: vec!["py".into()],
            filenames: vec!["SConstruct".into()],
            grammars: vec!["syntaxes/Python.sublime-syntax".into()],
            dependencies: vec![],
        };
        let grammar = br#"%YAML 1.2
---
name: Python
file_extensions: [py]
scope: source.python
contexts: { main: [] }
"#;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.start_file(
            "syntaxes/Python.sublime-syntax",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(grammar).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        PackageManager::new(data_dir.clone())
            .install_bytes(&bytes)
            .unwrap();

        let syntaxes = SyntaxManager::load(&data_dir).unwrap();
        assert_eq!(
            syntaxes.detect(Path::new("script.py"), false).name,
            "Python"
        );
        assert_eq!(
            syntaxes.detect(Path::new("SConstruct"), false).name,
            "Python"
        );
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
