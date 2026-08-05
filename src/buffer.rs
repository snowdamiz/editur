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
        Ok(Self {
            path,
            text,
            line_ending,
            fingerprint: Some(fingerprint),
            dirty: false,
            revision: 0,
            large_file_warning,
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
    }

    pub fn mark_saved(&mut self, path: &Path, fingerprint: DiskFingerprint) {
        self.path = path.to_path_buf();
        self.fingerprint = Some(fingerprint);
        self.dirty = false;
    }
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
}
