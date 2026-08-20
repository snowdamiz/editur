use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const LARGE_FILE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskFingerprint {
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextEdit {
    pub start_character: usize,
    pub start_byte: usize,
    pub deleted_characters: usize,
    pub deleted_bytes: usize,
    pub inserted_characters: usize,
    pub inserted_bytes: usize,
    pub inserted_line_starts: Vec<(usize, usize)>,
}

#[derive(Debug)]
pub struct Buffer {
    pub path: PathBuf,
    pub text: String,
    pub line_ending: LineEnding,
    pub fingerprint: Option<DiskFingerprint>,
    pub dirty: bool,
    pub revision: u64,
    pub large_file_warning: bool,
    line_starts: Vec<usize>,
    line_byte_starts: Vec<usize>,
    character_len: usize,
}

impl Buffer {
    pub fn from_bytes(
        path: PathBuf,
        bytes: Vec<u8>,
        fingerprint: DiskFingerprint,
    ) -> Result<Self, String> {
        if bytes.contains(&0) {
            return Err(format!("{} appears to be a binary file", path.display()));
        }
        let large_file_warning = bytes.len() > LARGE_FILE_BYTES;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not valid UTF-8", path.display()))?;
        let line_ending = if text.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let text = if line_ending == LineEnding::CrLf {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        let (line_starts, line_byte_starts, character_len) = line_index(&text);
        Ok(Self {
            path,
            text,
            line_ending,
            fingerprint: Some(fingerprint),
            dirty: false,
            revision: 0,
            large_file_warning,
            line_starts,
            line_byte_starts,
            character_len,
        })
    }

    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            text: String::new(),
            line_ending: LineEnding::Lf,
            fingerprint: None,
            dirty: true,
            revision: 0,
            large_file_warning: false,
            line_starts: vec![0],
            line_byte_starts: vec![0],
            character_len: 0,
        }
    }

    pub fn encoded(&self) -> Vec<u8> {
        match self.line_ending {
            LineEnding::Lf => self.text.as_bytes().to_vec(),
            LineEnding::CrLf => self.text.replace('\n', "\r\n").into_bytes(),
        }
    }

    pub fn mark_changed(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        (self.line_starts, self.line_byte_starts, self.character_len) = line_index(&self.text);
    }

    pub(crate) fn mark_changed_with_edits(&mut self, edits: &[TextEdit]) {
        if edits.is_empty() {
            self.mark_changed();
            return;
        }
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        for edit in edits {
            self.apply_edit(edit);
        }
    }

    fn apply_edit(&mut self, edit: &TextEdit) {
        let deleted_end = edit.start_character + edit.deleted_characters;
        let first_removed = self
            .line_starts
            .partition_point(|start| *start <= edit.start_character);
        let after_removed = self
            .line_starts
            .partition_point(|start| *start <= deleted_end);
        self.line_starts.drain(first_removed..after_removed);
        self.line_byte_starts.drain(first_removed..after_removed);

        for start in &mut self.line_starts[first_removed..] {
            *start = shift_index(*start, edit.inserted_characters, edit.deleted_characters);
        }
        for start in &mut self.line_byte_starts[first_removed..] {
            *start = shift_index(*start, edit.inserted_bytes, edit.deleted_bytes);
        }

        self.line_starts.splice(
            first_removed..first_removed,
            edit.inserted_line_starts
                .iter()
                .map(|(character, _)| edit.start_character + character),
        );
        self.line_byte_starts.splice(
            first_removed..first_removed,
            edit.inserted_line_starts
                .iter()
                .map(|(_, byte)| edit.start_byte + byte),
        );
        self.character_len = shift_index(
            self.character_len,
            edit.inserted_characters,
            edit.deleted_characters,
        );
    }

    pub fn line_column(&self, character_offset: usize) -> (usize, usize) {
        let offset = character_offset.min(self.character_len);
        let line = self
            .line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        (line + 1, offset - self.line_starts[line] + 1)
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn character_len(&self) -> usize {
        self.character_len
    }

    pub fn byte_index(&self, character_offset: usize) -> usize {
        let offset = character_offset.min(self.character_len);
        let line = self
            .line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        let byte_start = self.line_byte_starts[line];
        self.text[byte_start..]
            .char_indices()
            .nth(offset - self.line_starts[line])
            .map_or(self.text.len(), |(index, _)| byte_start + index)
    }

    pub(crate) fn vim_parts(&mut self) -> (&mut String, &[usize], &[usize], usize) {
        (
            &mut self.text,
            &self.line_starts,
            &self.line_byte_starts,
            self.character_len,
        )
    }

    pub fn mark_saved(&mut self, path: &Path, fingerprint: DiskFingerprint) {
        self.path = path.to_path_buf();
        self.fingerprint = Some(fingerprint);
        self.dirty = false;
    }
}

fn line_index(text: &str) -> (Vec<usize>, Vec<usize>, usize) {
    let mut starts = vec![0];
    let mut byte_starts = vec![0];
    let mut characters = 0;
    for (byte, character) in text.char_indices() {
        characters += 1;
        if character == '\n' {
            starts.push(characters);
            byte_starts.push(byte + 1);
        }
    }
    (starts, byte_starts, characters)
}

fn shift_index(index: usize, inserted: usize, deleted: usize) -> usize {
    index.saturating_sub(deleted).saturating_add(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(size: u64) -> DiskFingerprint {
        DiskFingerprint {
            size,
            modified: None,
            hash: [7; 32],
        }
    }

    #[test]
    fn normalizes_and_restores_crlf_without_lossy_decoding() {
        let mut buffer = Buffer::from_bytes(
            PathBuf::from("windows.rs"),
            b"fn main() {\r\n    println!(\"hi\");\r\n}\r\n".to_vec(),
            fingerprint(40),
        )
        .unwrap();

        assert_eq!(buffer.line_ending, LineEnding::CrLf);
        assert_eq!(buffer.text, "fn main() {\n    println!(\"hi\");\n}\n");
        buffer.text.push_str("// done\n");
        buffer.mark_changed();
        assert!(buffer.dirty);
        assert_eq!(
            buffer.encoded(),
            b"fn main() {\r\n    println!(\"hi\");\r\n}\r\n// done\r\n"
        );

        assert!(Buffer::from_bytes(PathBuf::from("bad"), vec![0xff], fingerprint(1)).is_err());
        assert!(
            Buffer::from_bytes(
                PathBuf::from("binary"),
                b"hello\0world".to_vec(),
                fingerprint(11)
            )
            .is_err()
        );
    }

    #[test]
    fn cursor_position_uses_one_based_logical_lines_and_columns() {
        let mut buffer = Buffer::new(PathBuf::from("indexed.txt"));
        buffer.text = "one\ntwø\nthree".into();
        buffer.mark_changed();

        assert_eq!(buffer.line_column(6), (2, 3));
    }

    #[test]
    fn character_to_byte_lookup_handles_unicode_without_crossing_lines() {
        let mut buffer = Buffer::new(PathBuf::from("indexed.txt"));
        buffer.text = "αβ\nx💡z".into();
        buffer.mark_changed();

        assert_eq!(
            (0..=6)
                .map(|character| buffer.byte_index(character))
                .collect::<Vec<_>>(),
            [0, 2, 4, 5, 6, 10, 11]
        );
    }

    #[test]
    fn incremental_edit_keeps_unicode_line_and_byte_indexes_exact() {
        let mut buffer = Buffer::from_bytes(
            PathBuf::from("indexed.txt"),
            "αβ\nhello\nworld".as_bytes().to_vec(),
            fingerprint(18),
        )
        .unwrap();
        buffer.text.replace_range(6..11, "i\nthere\n");
        buffer.mark_changed_with_edits(&[TextEdit {
            start_character: 4,
            start_byte: 6,
            deleted_characters: 5,
            deleted_bytes: 5,
            inserted_characters: 8,
            inserted_bytes: 8,
            inserted_line_starts: vec![(2, 2), (8, 8)],
        }]);
        let rebuilt = Buffer::from_bytes(
            PathBuf::from("rebuilt.txt"),
            buffer.text.as_bytes().to_vec(),
            fingerprint(buffer.text.len() as u64),
        )
        .unwrap();

        assert_eq!(
            (
                buffer.line_count(),
                buffer.character_len(),
                (0..=buffer.character_len())
                    .map(|character| (buffer.line_column(character), buffer.byte_index(character)))
                    .collect::<Vec<_>>(),
            ),
            (
                rebuilt.line_count(),
                rebuilt.character_len(),
                (0..=rebuilt.character_len())
                    .map(|character| (
                        rebuilt.line_column(character),
                        rebuilt.byte_index(character)
                    ))
                    .collect::<Vec<_>>(),
            )
        );
    }
}
