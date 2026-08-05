use std::{env, fs};

const EMBEDDED_UPDATE_BASE: Option<&str> = option_env!("EDITUR_UPDATE_BASE");
const MAX_BINARY_SIZE: u64 = 64 * 1024 * 1024;
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
    let mut response = ureq::get(url)
        .call()
        .map_err(|error| format!("cannot download {url}: {error}"))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| format!("cannot read download from {url}: {error}"))
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
}
