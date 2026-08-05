use std::{env, fs};

const EMBEDDED_UPDATE_BASE: Option<&str> = option_env!("EDITUR_UPDATE_BASE");
const MAX_BINARY_SIZE: u64 = 64 * 1024 * 1024;
#[cfg(target_os = "macos")]
const MAX_APP_ARCHIVE_SIZE: u64 = 128 * 1024 * 1024;
#[cfg(target_os = "macos")]
const MAX_APP_ENTRIES: usize = 32;
#[cfg(target_os = "macos")]
const MAX_APP_UNPACKED_SIZE: u64 = 128 * 1024 * 1024;
const MAX_CHECKSUM_SIZE: u64 = 1024;

pub fn run() -> Result<(), String> {
    let base = env::var("EDITUR_UPDATE_BASE")
        .ok()
        .or_else(|| EMBEDDED_UPDATE_BASE.map(str::to_owned))
        .ok_or_else(|| {
            "this build has no update source; install a release build or set EDITUR_UPDATE_BASE"
                .to_owned()
        })?;
    let asset = asset_name_for(env::consts::OS, env::consts::ARCH)?;
    let (binary_url, checksum_url) = update_urls(&base, asset)?;
    let executable = env::current_exe()
        .map_err(|error| format!("cannot locate the running Editur executable: {error}"))?;
    #[cfg(target_os = "macos")]
    let executable = fs::canonicalize(&executable)
        .map_err(|error| format!("cannot resolve {}: {error}", executable.display()))?;
    #[cfg(target_os = "macos")]
    if macos_bundle_root(&executable).is_none() {
        return migrate_macos_install(&base, &executable);
    }
    let current = fs::read(&executable)
        .map_err(|error| format!("cannot read {}: {error}", executable.display()))?;
    let checksum = download(&checksum_url, MAX_CHECKSUM_SIZE)?;
    let advertised = advertised_checksum(&checksum)?;
    if crate::syntax::package::sha256_hex(&current).eq_ignore_ascii_case(advertised) {
        println!("Editur is already up to date.");
        return Ok(());
    }

    println!("Downloading the latest release build…");
    let downloaded = download(&binary_url, MAX_BINARY_SIZE)?;
    if !verify_update(&current, &downloaded, &checksum)? {
        println!("Editur is already up to date.");
        return Ok(());
    }
    if !crate::instance::quit_running()? {
        return Err("save or discard changes in the running editor before updating".into());
    }

    #[cfg(unix)]
    {
        install_unix(&executable, &downloaded)?;
        println!("Updated Editur. The new build will run next time you launch it.");
        Ok(())
    }
    #[cfg(windows)]
    {
        stage_windows_update(&executable, &downloaded)?;
        println!("Update verified and staged; it will finish after this command exits.");
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    Err("self-update is unsupported on this operating system".into())
}

fn asset_name_for(os: &str, architecture: &str) -> Result<&'static str, String> {
    match (os, architecture) {
        ("macos", "aarch64") => Ok("editur-macos-aarch64"),
        ("macos", "x86_64") => Ok("editur-macos-x86_64"),
        ("linux", "x86_64") => Ok("editur-linux-x86_64"),
        ("windows", "x86_64") => Ok("editur-windows-x86_64.exe"),
        _ => Err(format!("updates are not published for {os}/{architecture}")),
    }
}

fn update_urls(base: &str, asset: &str) -> Result<(String, String), String> {
    let base = base.trim_end_matches('/');
    if !base.starts_with("https://")
        || base.len() == "https://".len()
        || base.bytes().any(|byte| byte.is_ascii_whitespace())
        || base.contains(['?', '#'])
    {
        return Err("update URL must be an HTTPS release directory".into());
    }
    let binary = format!("{base}/{asset}");
    let checksum = format!("{binary}.sha256");
    Ok((binary, checksum))
}

fn verify_update(current: &[u8], downloaded: &[u8], checksum: &[u8]) -> Result<bool, String> {
    let advertised = advertised_checksum(checksum)?;
    let downloaded_checksum = crate::syntax::package::sha256_hex(downloaded);
    if !downloaded_checksum.eq_ignore_ascii_case(advertised) {
        return Err("downloaded update does not match its SHA-256 checksum".into());
    }
    Ok(!crate::syntax::package::sha256_hex(current).eq_ignore_ascii_case(advertised))
}

fn advertised_checksum(checksum: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(checksum)
        .ok()
        .and_then(|value| value.split_whitespace().next())
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "release checksum is invalid".to_owned())
}

#[cfg(feature = "network")]
fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    crate::network::retry(|| {
        let mut response = ureq::get(url)
            .call()
            .map_err(|error| format!("cannot download {url}: {error}"))?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| format!("cannot read download from {url}: {error}"))
    })
}

#[cfg(not(feature = "network"))]
fn download(_url: &str, _limit: u64) -> Result<Vec<u8>, String> {
    Err("this Editur build has updates disabled".into())
}

#[cfg(unix)]
fn install_unix(executable: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::{fs, io::Write};

    let metadata = fs::symlink_metadata(executable)
        .map_err(|error| format!("cannot inspect {}: {error}", executable.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to update non-regular executable {}",
            executable.display()
        ));
    }
    let parent = executable
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", executable.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("cannot stage update in {}: {error}", parent.display()))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.flush())
        .map_err(|error| format!("cannot write staged update: {error}"))?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("cannot finalize staged update: {error}"))?;
    temporary
        .persist(executable)
        .map_err(|error| format!("cannot replace {}: {}", executable.display(), error.error))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_bundle_root(executable: &std::path::Path) -> Option<&std::path::Path> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then_some(bundle)
}

#[cfg(target_os = "macos")]
fn extract_macos_app_archive(
    bytes: &[u8],
    destination: &std::path::Path,
    max_unpacked: u64,
) -> Result<std::path::PathBuf, String> {
    use std::{
        io::{self, Cursor, Read, Write},
        os::unix::fs::PermissionsExt,
        path::{Component, Path},
    };

    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid macOS update archive: {error}"))?;
    if archive.len() > MAX_APP_ENTRIES {
        return Err(format!(
            "macOS update contains more than {MAX_APP_ENTRIES} entries"
        ));
    }
    let mut unpacked = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot read macOS update entry: {error}"))?;
        let name = entry.name().to_owned();
        let relative = Path::new(&name);
        if name.is_empty()
            || name.contains('\\')
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || relative
                .components()
                .next()
                .is_none_or(|component| component.as_os_str() != "Editur.app")
        {
            return Err(format!("unsafe macOS update path `{name}`"));
        }
        let mode = entry.unix_mode();
        if mode.is_some_and(|mode| {
            let file_type = mode & 0o170000;
            file_type != 0 && file_type != 0o040000 && file_type != 0o100000
        }) {
            return Err(format!("macOS update contains special file `{name}`"));
        }
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
            continue;
        }
        if !matches!(
            name.as_str(),
            "Editur.app/Contents/MacOS/editur"
                | "Editur.app/Contents/Info.plist"
                | "Editur.app/Contents/Resources/Editur.icns"
        ) {
            return Err(format!("unexpected macOS update entry `{name}`"));
        }
        let parent = output
            .parent()
            .ok_or_else(|| format!("macOS update path has no parent: {name}"))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
        let remaining = max_unpacked.saturating_sub(unpacked);
        let copied = io::copy(&mut entry.take(remaining + 1), &mut file)
            .map_err(|error| format!("cannot extract {}: {error}", output.display()))?;
        if copied > remaining {
            return Err(format!("macOS update expands beyond {max_unpacked} bytes"));
        }
        unpacked += copied;
        file.flush()
            .map_err(|error| format!("cannot flush {}: {error}", output.display()))?;
        let permissions = mode.map_or_else(
            || {
                if name.ends_with("/editur") {
                    0o755
                } else {
                    0o644
                }
            },
            |mode| mode & 0o777,
        );
        fs::set_permissions(&output, fs::Permissions::from_mode(permissions))
            .map_err(|error| format!("cannot set permissions on {}: {error}", output.display()))?;
    }
    Ok(destination.join("Editur.app"))
}

#[cfg(target_os = "macos")]
fn migrate_macos_install(base: &str, executable: &std::path::Path) -> Result<(), String> {
    let asset = match env::consts::ARCH {
        "aarch64" => "editur-macos-aarch64.zip",
        "x86_64" => "editur-macos-x86_64.zip",
        architecture => {
            return Err(format!(
                "updates are not published for macos/{architecture}"
            ));
        }
    };
    let (archive_url, checksum_url) = update_urls(base, asset)?;
    println!("Migrating this older install to the native Editur.app bundle…");
    let archive = download(&archive_url, MAX_APP_ARCHIVE_SIZE)?;
    let checksum = download(&checksum_url, MAX_CHECKSUM_SIZE)?;
    let advertised = advertised_checksum(&checksum)?;
    if !crate::syntax::package::sha256_hex(&archive).eq_ignore_ascii_case(advertised) {
        return Err("downloaded update does not match its SHA-256 checksum".into());
    }
    if !crate::instance::quit_running()? {
        return Err("save or discard changes in the running editor before updating".into());
    }

    let parent = executable
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", executable.display()))?;
    let extracted = tempfile::Builder::new()
        .prefix(".editur-app-")
        .tempdir_in(parent)
        .map_err(|error| format!("cannot stage Editur.app in {}: {error}", parent.display()))?;
    let staged = extract_macos_app_archive(&archive, extracted.path(), MAX_APP_UNPACKED_SIZE)?;
    install_macos_bundle(&staged, executable)?;
    println!("Updated Editur and migrated it to the native app bundle.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos_bundle(
    staged: &std::path::Path,
    executable: &std::path::Path,
) -> Result<(), String> {
    use std::os::unix::fs::symlink;

    let staged_metadata = fs::symlink_metadata(staged)
        .map_err(|error| format!("cannot inspect {}: {error}", staged.display()))?;
    let staged_executable = staged.join("Contents/MacOS/editur");
    let required_files = [
        staged_executable,
        staged.join("Contents/Info.plist"),
        staged.join("Contents/Resources/Editur.icns"),
    ];
    if !staged_metadata.is_dir()
        || staged_metadata.file_type().is_symlink()
        || required_files.iter().any(|path| {
            fs::symlink_metadata(path)
                .map(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
                .unwrap_or(true)
        })
    {
        return Err("macOS update does not contain a valid Editur.app".into());
    }
    let executable_metadata = fs::symlink_metadata(executable)
        .map_err(|error| format!("cannot inspect {}: {error}", executable.display()))?;
    if executable.file_name().is_none_or(|name| name != "editur")
        || !executable_metadata.is_file()
        || executable_metadata.file_type().is_symlink()
    {
        return Err(format!(
            "refusing to migrate non-regular executable {}",
            executable.display()
        ));
    }
    let parent = executable
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", executable.display()))?;
    let destination = parent.join("Editur.app");
    if staged == destination {
        return Err("macOS update staging path matches its destination".into());
    }
    let backup = parent.join(format!(".Editur.app.old-{}", std::process::id()));
    let link = parent.join(format!(".editur-link-{}", std::process::id()));
    if backup.exists() || link.exists() {
        return Err("macOS update staging path already exists".into());
    }

    let had_destination = if destination.exists() {
        let metadata = fs::symlink_metadata(&destination)
            .map_err(|error| format!("cannot inspect {}: {error}", destination.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("refusing to replace {}", destination.display()));
        }
        fs::rename(&destination, &backup)
            .map_err(|error| format!("cannot stage existing {}: {error}", destination.display()))?;
        true
    } else {
        false
    };
    if let Err(error) = fs::rename(staged, &destination) {
        if had_destination {
            let _ = fs::rename(&backup, &destination);
        }
        return Err(format!("cannot install {}: {error}", destination.display()));
    }
    if let Err(error) = symlink(destination.join("Contents/MacOS/editur"), &link)
        .and_then(|()| fs::rename(&link, executable))
    {
        let _ = fs::remove_file(&link);
        let _ = fs::remove_dir_all(&destination);
        if had_destination {
            let _ = fs::rename(&backup, &destination);
        }
        return Err(format!("cannot install the Editur CLI symlink: {error}"));
    }
    if had_destination {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("cannot remove {}: {error}", backup.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn stage_windows_update(executable: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::process::Command;

    validate_regular_executable(executable)?;
    let parent = executable
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", executable.display()))?;
    let staged = parent.join(format!(".editur-update-{}.exe", std::process::id()));
    write_new_file(&staged, bytes)?;
    if let Err(error) = Command::new(&staged)
        .arg("--finish-update")
        .arg(executable)
        .spawn()
    {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot start staged updater: {error}"));
    }
    Ok(())
}

#[cfg(windows)]
pub fn finish_windows(destination: &std::path::Path) -> Result<(), String> {
    use std::{process::Command, thread, time::Duration};
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::HSTRING,
    };

    let source =
        env::current_exe().map_err(|error| format!("cannot locate staged updater: {error}"))?;
    validate_windows_handoff(&source, destination, ".editur-update-")?;
    let ready = source.with_file_name(format!(".editur-ready-{}.exe", std::process::id()));
    write_new_file(
        &ready,
        &fs::read(&source)
            .map_err(|error| format!("cannot read staged updater {}: {error}", source.display()))?,
    )?;
    let ready_path = HSTRING::from(ready.as_path());
    let destination_path = HSTRING::from(destination);
    let mut last_error = None;
    for _ in 0..100 {
        match unsafe {
            MoveFileExW(
                &ready_path,
                &destination_path,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } {
            Ok(()) => {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                Command::new(destination)
                    .arg("--cleanup-update")
                    .arg(&source)
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .map_err(|error| format!("cannot start update cleanup: {error}"))?;
                return Ok(());
            }
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let _ = fs::remove_file(&ready);
    Err(format!(
        "cannot replace {}: {}",
        destination.display(),
        last_error.map_or_else(
            || "unknown Windows error".to_owned(),
            |error| error.to_string()
        )
    ))
}

#[cfg(windows)]
pub fn cleanup_windows(temporary: &std::path::Path) -> Result<(), String> {
    use std::{thread, time::Duration};

    let installed = env::current_exe()
        .map_err(|error| format!("cannot locate installed executable: {error}"))?;
    validate_windows_handoff(temporary, &installed, ".editur-update-")?;
    for _ in 0..100 {
        match fs::remove_file(temporary) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(format!(
                    "cannot remove staged updater {}: {error}",
                    temporary.display()
                ));
            }
        }
    }
    Err(format!(
        "timed out removing staged updater {}",
        temporary.display()
    ))
}

#[cfg(windows)]
fn validate_windows_handoff(
    staged: &std::path::Path,
    installed: &std::path::Path,
    prefix: &str,
) -> Result<(), String> {
    let staged_parent = staged
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", staged.display()))?;
    if installed.parent() != Some(staged_parent)
        || !staged
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".exe"))
    {
        return Err("invalid Windows update handoff paths".into());
    }
    validate_regular_executable(staged)?;
    validate_regular_executable(installed)
}

#[cfg(windows)]
fn validate_regular_executable(path: &std::path::Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(format!("{} is not a regular executable", path.display()))
    }
}

#[cfg(windows)]
fn write_new_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::{fs::OpenOptions, io::Write};

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{asset_name_for, update_urls, verify_update};

    #[test]
    fn selects_only_published_platform_assets() {
        assert_eq!(
            asset_name_for("macos", "aarch64"),
            Ok("editur-macos-aarch64")
        );
        assert_eq!(asset_name_for("macos", "x86_64"), Ok("editur-macos-x86_64"));
        assert_eq!(asset_name_for("linux", "x86_64"), Ok("editur-linux-x86_64"));
        assert_eq!(
            asset_name_for("windows", "x86_64"),
            Ok("editur-windows-x86_64.exe")
        );
        assert!(asset_name_for("linux", "aarch64").is_err());
    }

    #[test]
    fn accepts_only_a_changed_binary_with_the_advertised_checksum() {
        let checksum = b"11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437\n";
        assert_eq!(verify_update(b"old", b"new", checksum), Ok(true));
        assert_eq!(verify_update(b"new", b"new", checksum), Ok(false));
        assert!(verify_update(b"old", b"tampered", checksum).is_err());
        assert!(verify_update(b"old", b"new", b"not-a-checksum").is_err());
    }

    #[test]
    fn builds_only_https_release_urls() {
        assert_eq!(
            update_urls("https://example.com/releases/", "editur-linux-x86_64"),
            Ok((
                "https://example.com/releases/editur-linux-x86_64".into(),
                "https://example.com/releases/editur-linux-x86_64.sha256".into(),
            ))
        );
        assert!(update_urls("http://example.com/releases", "editur-linux-x86_64").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn installs_unix_updates_atomically_and_preserves_executable_mode() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("editur");
        fs::write(&executable, b"old").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        super::install_unix(&executable, b"new").unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_app_extraction_counts_actual_bytes_and_rejects_unsafe_paths() {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;

        fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, contents) in entries {
                archive
                    .start_file(*name, SimpleFileOptions::default().unix_permissions(0o755))
                    .unwrap();
                archive.write_all(contents).unwrap();
            }
            archive.finish().unwrap().into_inner()
        }

        let temp = tempfile::tempdir().unwrap();
        let oversized = archive(&[("Editur.app/Contents/MacOS/editur", b"123456789")]);
        assert!(super::extract_macos_app_archive(&oversized, temp.path(), 8).is_err());

        let unsafe_path = archive(&[("../outside", b"no")]);
        assert!(super::extract_macos_app_archive(&unsafe_path, temp.path(), 1024).is_err());
        assert!(!temp.path().join("../outside").exists());

        let valid_dir = temp.path().join("valid");
        std::fs::create_dir(&valid_dir).unwrap();
        let valid = archive(&[
            ("Editur.app/Contents/MacOS/editur", b"binary"),
            ("Editur.app/Contents/Info.plist", b"plist"),
            ("Editur.app/Contents/Resources/Editur.icns", b"icon"),
        ]);
        let app = super::extract_macos_app_archive(&valid, &valid_dir, 1024).unwrap();
        assert_eq!(
            std::fs::read(app.join("Contents/Resources/Editur.icns")).unwrap(),
            b"icon"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_bare_install_migrates_to_an_app_bundle_and_cli_symlink() {
        use std::fs;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("editur");
        fs::write(&executable, b"old").unwrap();
        let staged = temp.path().join("staged/Editur.app");
        fs::create_dir_all(staged.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(staged.join("Contents/Resources")).unwrap();
        fs::write(staged.join("Contents/MacOS/editur"), b"new").unwrap();
        fs::write(staged.join("Contents/Info.plist"), b"plist").unwrap();
        fs::write(staged.join("Contents/Resources/Editur.icns"), b"icon").unwrap();

        super::install_macos_bundle(&staged, &executable).unwrap();

        assert!(super::macos_bundle_root(&executable).is_none());
        assert_eq!(
            fs::read_link(&executable).unwrap(),
            temp.path().join("Editur.app/Contents/MacOS/editur")
        );
        assert_eq!(
            fs::read(temp.path().join("Editur.app/Contents/MacOS/editur")).unwrap(),
            b"new"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recognizes_only_executables_inside_a_macos_app_bundle() {
        let bundled = std::path::Path::new("/tmp/Editur.app/Contents/MacOS/editur");
        assert_eq!(
            super::macos_bundle_root(bundled),
            Some(std::path::Path::new("/tmp/Editur.app"))
        );
        assert!(super::macos_bundle_root(std::path::Path::new("/tmp/editur")).is_none());
    }
}
