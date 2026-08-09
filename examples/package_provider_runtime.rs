use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let source = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: package_provider_runtime SOURCE OUTPUT")?;
    let output = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("missing output path")?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    package(&source, &output)?;
    Ok(())
}

fn package(source: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source = source.canonicalize()?;
    let mut files = Vec::new();
    collect_files(&source, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err("provider runtime directory is empty".into());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut archive = ZipWriter::new(fs::File::create(output)?);
    for path in files {
        let relative = path.strip_prefix(&source)?;
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let executable = is_executable(&path, &name)?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(if executable { 0o755 } else { 0o644 });
        archive.start_file(name, options)?;
        io::copy(&mut fs::File::open(path)?, &mut archive)?;
    }
    archive.finish()?.sync_all()?;
    Ok(())
}

fn collect_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("provider runtime contains symlink {}", path.display()).into());
        }
        if metadata.is_dir() {
            collect_files(&path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            return Err(
                format!("provider runtime contains special file {}", path.display()).into(),
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path, _name: &str) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path, name: &str) -> io::Result<bool> {
    Ok(name == "runtime/node.exe" || name.ends_with("/codex.exe"))
}

#[cfg(test)]
mod tests {
    use super::package;

    #[test]
    fn provider_archives_are_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("source/package")).unwrap();
        std::fs::write(temp.path().join("source/package/index.js"), b"code").unwrap();
        let first = temp.path().join("first.zip");
        let second = temp.path().join("second.zip");

        package(&temp.path().join("source"), &first).unwrap();
        package(&temp.path().join("source"), &second).unwrap();

        assert_eq!(
            std::fs::read(first).unwrap(),
            std::fs::read(second).unwrap()
        );
    }
}
