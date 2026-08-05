use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
        let large_file_warning = bytes.len() > 5 * 1024 * 1024;
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
        let (line_starts, character_len) = line_index(&text);
        Ok(Self {
            path,
            text,
            line_ending,
            fingerprint: Some(fingerprint),
            dirty: false,
            revision: 0,
            large_file_warning,
            line_starts,
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
        (self.line_starts, self.character_len) = line_index(&self.text);
    }

    pub fn line_column(&self, character_offset: usize) -> (usize, usize) {
        let offset = character_offset.min(self.character_len);
        let line = self
            .line_starts
            .partition_point(|line_start| *line_start <= offset)
            .saturating_sub(1);
        (line + 1, offset - self.line_starts[line] + 1)
    }

    pub fn mark_saved(&mut self, path: &Path, fingerprint: DiskFingerprint) {
        self.path = path.to_path_buf();
        self.fingerprint = Some(fingerprint);
        self.dirty = false;
    }
}

fn line_index(text: &str) -> (Vec<usize>, usize) {
    let mut starts = vec![0];
    let mut characters = 0;
    for character in text.chars() {
        characters += 1;
        if character == '\n' {
            starts.push(characters);
        }
    }
    (starts, characters)
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
}
