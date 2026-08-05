use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    pub name: OsString,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
}

pub fn read_directory(path: &Path) -> Result<Vec<TreeEntry>, String> {
    let entries =
        fs::read_dir(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let name = entry.file_name();
        if name == OsStr::new(".git") {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        result.push(TreeEntry {
            name,
            path: entry.path(),
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        });
    }
    result.sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_string_lossy().to_lowercase()));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_one_level_directories_first_without_git_or_following_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("z-dir")).unwrap();
        fs::create_dir(temp.path().join("a-dir")).unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join("b.rs"), "").unwrap();
        fs::write(temp.path().join(".env"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            temp.path().join("a-dir"),
            temp.path().join("directory-link"),
        )
        .unwrap();

        let entries = read_directory(temp.path()).unwrap();
        let names: Vec<_> = entries
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect();
        #[cfg(unix)]
        assert_eq!(names, ["a-dir", "z-dir", ".env", "b.rs", "directory-link"]);
        #[cfg(not(unix))]
        assert_eq!(names, ["a-dir", "z-dir", ".env", "b.rs"]);
        assert!(!names.iter().any(|name| name == ".git"));
        assert!(entries[0].is_dir);
        assert!(entries[1].is_dir);
        #[cfg(unix)]
        {
            let link = entries.last().unwrap();
            assert!(link.is_symlink);
            assert!(!link.is_dir);
        }
    }
}
