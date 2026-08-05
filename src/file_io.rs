use crate::buffer::{Buffer, DiskFingerprint};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OpenTarget {
    pub root: PathBuf,
    pub file: Option<PathBuf>,
    pub create: bool,
}

#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => formatter.write_str("the file changed on disk"),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SaveError {}

pub fn load_buffer(path: &Path) -> Result<Buffer, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let fingerprint = fingerprint_from(&metadata, &bytes);
    Buffer::from_bytes(path.to_path_buf(), bytes, fingerprint)
}

pub fn disk_fingerprint(path: &Path) -> Result<DiskFingerprint, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    Ok(fingerprint_from(&metadata, &bytes))
}

fn fingerprint_from(metadata: &fs::Metadata, bytes: &[u8]) -> DiskFingerprint {
    DiskFingerprint {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        hash: Sha256::digest(bytes).into(),
    }
}

pub fn safe_save(buffer: &mut Buffer, destination: &Path) -> Result<(), SaveError> {
    let expected = (destination == buffer.path)
        .then_some(buffer.fingerprint.as_ref())
        .flatten();
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(SaveError::Conflict),
        Ok(metadata) if !metadata.is_file() => {
            return Err(SaveError::Io(format!(
                "{} is not a regular file",
                destination.display()
            )));
        }
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(SaveError::Io(format!(
                "cannot inspect {}: {error}",
                destination.display()
            )));
        }
    };
    let current = metadata
        .as_ref()
        .map(|_| disk_fingerprint(destination).map_err(SaveError::Io))
        .transpose()?;
    if current.as_ref() != expected {
        return Err(SaveError::Conflict);
    }

    let parent = destination.parent().ok_or_else(|| {
        SaveError::Io(format!("{} has no parent directory", destination.display()))
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        SaveError::Io(format!(
            "cannot create temporary file in {}: {error}",
            parent.display()
        ))
    })?;
    temporary
        .write_all(&buffer.encoded())
        .and_then(|()| temporary.flush())
        .map_err(|error| {
            SaveError::Io(format!("cannot write {}: {error}", destination.display()))
        })?;
    if let Some(metadata) = metadata {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| {
                SaveError::Io(format!(
                    "cannot preserve permissions for {}: {error}",
                    destination.display()
                ))
            })?;
    }
    temporary.as_file().sync_all().map_err(|error| {
        SaveError::Io(format!("cannot flush {}: {error}", destination.display()))
    })?;
    temporary.persist(destination).map_err(|error| {
        SaveError::Io(format!(
            "cannot replace {}: {}",
            destination.display(),
            error.error
        ))
    })?;

    let fingerprint = disk_fingerprint(destination).map_err(SaveError::Io)?;
    buffer.mark_saved(destination, fingerprint);
    Ok(())
}

pub fn resolve_target(cwd: &Path, input: Option<&Path>) -> Result<OpenTarget, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|error| format!("cannot use {}: {error}", cwd.display()))?;
    let Some(input) = input else {
        fs::read_dir(&cwd).map_err(|error| format!("cannot read {}: {error}", cwd.display()))?;
        return Ok(OpenTarget {
            root: cwd,
            file: None,
            create: false,
        });
    };

    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        cwd.join(input)
    };

    if candidate.exists() {
        let path = candidate
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", candidate.display()))?;
        if path.is_dir() {
            fs::read_dir(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            return Ok(OpenTarget {
                root: path,
                file: None,
                create: false,
            });
        }
        if !path.is_file() {
            return Err(format!("{} is not a file or directory", path.display()));
        }
        fs::File::open(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
        return Ok(OpenTarget {
            root: if path.starts_with(&cwd) {
                cwd
            } else {
                parent.to_path_buf()
            },
            file: Some(path),
            create: false,
        });
    }

    let name = candidate
        .file_name()
        .ok_or_else(|| format!("invalid path: {}", candidate.display()))?;
    let parent = candidate
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", candidate.display()))?
        .canonicalize()
        .map_err(|error| format!("cannot use parent of {}: {error}", candidate.display()))?;
    if !parent.is_dir() {
        return Err(format!("{} is not a directory", parent.display()));
    }
    fs::read_dir(&parent).map_err(|error| format!("cannot read {}: {error}", parent.display()))?;
    let path = parent.join(name);
    Ok(OpenTarget {
        root: if path.starts_with(&cwd) { cwd } else { parent },
        file: Some(path),
        create: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use std::fs;

    #[test]
    fn resolves_file_directory_and_new_path_roots() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("project");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let inside_file = cwd.join("inside.rs");
        let outside_file = outside.join("outside.rs");
        fs::write(&inside_file, "fn main() {}\n").unwrap();
        fs::write(&outside_file, "fn main() {}\n").unwrap();
        let cwd = cwd.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        let inside_file = inside_file.canonicalize().unwrap();
        let outside_file = outside_file.canonicalize().unwrap();

        assert_eq!(
            resolve_target(&cwd, None).unwrap(),
            OpenTarget {
                root: cwd.clone(),
                file: None,
                create: false,
            }
        );
        assert_eq!(
            resolve_target(&cwd, Some(Path::new("inside.rs"))).unwrap(),
            OpenTarget {
                root: cwd.clone(),
                file: Some(inside_file.clone()),
                create: false,
            }
        );
        assert_eq!(
            resolve_target(&cwd, Some(&outside_file)).unwrap(),
            OpenTarget {
                root: outside.clone(),
                file: Some(outside_file),
                create: false,
            }
        );
        assert_eq!(
            resolve_target(&cwd, Some(&outside)).unwrap(),
            OpenTarget {
                root: outside,
                file: None,
                create: false,
            }
        );
        assert_eq!(
            resolve_target(&cwd, Some(Path::new("new.rs"))).unwrap(),
            OpenTarget {
                root: cwd.clone(),
                file: Some(cwd.join("new.rs")),
                create: true,
            }
        );
        assert!(resolve_target(&cwd, Some(Path::new("missing/new.rs"))).is_err());
    }

    #[test]
    fn saves_atomically_preserves_mode_and_detects_external_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        fs::write(&path, "fn old() {}\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }

        let mut buffer = load_buffer(&path).unwrap();
        assert_eq!(buffer.text, "fn old() {}\n");
        buffer.text = "fn new() {}\n".into();
        buffer.mark_changed();
        safe_save(&mut buffer, &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fn new() {}\r\n");
        assert!(!buffer.dirty);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }

        buffer.text = "fn editur() {}\n".into();
        buffer.mark_changed();
        fs::write(&path, "fn external() {}\r\n").unwrap();
        assert!(matches!(
            safe_save(&mut buffer, &path),
            Err(SaveError::Conflict)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"fn external() {}\r\n");
        assert!(buffer.dirty);

        let new_path = temp.path().join("new.rs");
        let mut new_buffer = Buffer::new(new_path.clone());
        new_buffer.text = "created\n".into();
        safe_save(&mut new_buffer, &new_path).unwrap();
        assert_eq!(fs::read_to_string(new_path).unwrap(), "created\n");
        assert!(!new_buffer.dirty);
    }

    #[test]
    fn failed_save_keeps_the_buffer_dirty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("new.rs");
        let mut buffer = Buffer::new(path);
        buffer.text = "unsaved".into();
        let destination = temp.path().join("missing").join("new.rs");

        assert!(safe_save(&mut buffer, &destination).is_err());
        assert!(buffer.dirty);
    }

    #[test]
    fn deleted_and_replaced_files_conflict_while_unicode_paths_save() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("héllo.rs");
        fs::write(&path, "fn original() {}\n").unwrap();
        let mut buffer = load_buffer(&path).unwrap();
        buffer.text = "fn edited() {}\n".into();
        buffer.mark_changed();

        fs::remove_file(&path).unwrap();
        assert!(matches!(
            safe_save(&mut buffer, &path),
            Err(SaveError::Conflict)
        ));
        assert!(buffer.dirty);

        fs::write(&path, "fn replacement() {}\n").unwrap();
        assert!(matches!(
            safe_save(&mut buffer, &path),
            Err(SaveError::Conflict)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn replacement() {}\n");

        let copy = temp.path().join("保存.rs");
        safe_save(&mut buffer, &copy).unwrap();
        assert_eq!(fs::read_to_string(copy).unwrap(), "fn edited() {}\n");
        assert!(!buffer.dirty);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_replace_a_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.rs");
        let link = temp.path().join("link.rs");
        fs::write(&target, "untouched\n").unwrap();
        symlink(&target, &link).unwrap();
        let mut buffer = Buffer::new(link.clone());
        buffer.text = "changed\n".into();

        assert!(matches!(
            safe_save(&mut buffer, &link),
            Err(SaveError::Conflict)
        ));
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched\n");
        assert!(buffer.dirty);
    }
}
