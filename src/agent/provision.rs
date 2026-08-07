use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(feature = "network")]
use ureq::ResponseExt;

const MAX_COMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 2_048;
const VERIFICATION_RECEIPT: &str = ".editur-verified.json";
const EMBEDDED_MANIFEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-sidecar.json"));

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSpec {
    pub version: String,
    pub distributions: Vec<Distribution>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distribution {
    pub os: String,
    pub architecture: String,
    pub archive_url: String,
    pub command: String,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub archive_format: ArchiveFormat,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarManifest {
    pub format_version: u32,
    pub agent: String,
    pub version: String,
    pub os: String,
    pub architecture: String,
    pub archive_url: String,
    pub archive_sha256: String,
    pub archive_format: ArchiveFormat,
    pub archive_size_bytes: u64,
    pub max_compressed_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_entries: usize,
    pub command: String,
    #[serde(default)]
    pub entrypoint: Option<String>,
    pub args: Vec<String>,
    pub entries: Vec<ManagedEntry>,
    pub license_url: String,
    pub terms_url: String,
}

impl SidecarManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid Cursor sidecar manifest: {error}"))?;
        if manifest.format_version != 1 || manifest.agent != "cursor" {
            return Err("unsupported Cursor sidecar manifest format version".into());
        }
        validate_version(&manifest.version)?;
        if !manifest
            .archive_url
            .starts_with("https://downloads.cursor.com/")
        {
            return Err("Cursor archive URL must use https://downloads.cursor.com".into());
        }
        if manifest.os != std::env::consts::OS || manifest.architecture != std::env::consts::ARCH {
            return Err(format!(
                "Cursor sidecar manifest is not for the current platform ({}/{})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
        }
        let extracted_size = manifest
            .entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.size));
        if manifest.archive_size_bytes == 0
            || manifest.archive_size_bytes > manifest.max_compressed_bytes
            || manifest.max_compressed_bytes == 0
            || manifest.max_compressed_bytes > MAX_COMPRESSED_BYTES
            || manifest.max_extracted_bytes == 0
            || manifest.max_extracted_bytes > MAX_EXTRACTED_BYTES
            || manifest.max_entries == 0
            || manifest.max_entries > MAX_ARCHIVE_ENTRIES
            || manifest.entries.len() > manifest.max_entries
            || extracted_size.is_none_or(|size| size > manifest.max_extracted_bytes)
        {
            return Err("Cursor sidecar manifest has unsafe extraction limits".into());
        }
        if !valid_sha256(&manifest.archive_sha256)
            || manifest.entries.iter().any(|entry| {
                entry.kind == EntryKind::File
                    && entry
                        .sha256
                        .as_deref()
                        .is_none_or(|hash| !valid_sha256(hash))
            })
        {
            return Err("Cursor sidecar manifest has an invalid SHA-256 value".into());
        }
        validate_relative_path(&manifest.command)?;
        if let Some(entrypoint) = &manifest.entrypoint {
            validate_relative_path(entrypoint)?;
        }
        let mut paths = HashSet::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            validate_relative_path(&entry.path)?;
            if !paths.insert(entry.path.as_str()) {
                return Err(format!("duplicate manifest path `{}`", entry.path));
            }
        }
        if !manifest
            .entries
            .iter()
            .any(|entry| entry.path == manifest.command && entry.kind == EntryKind::File)
        {
            return Err("Cursor sidecar manifest command is not a declared file".into());
        }
        if manifest.entrypoint.as_ref().is_some_and(|entrypoint| {
            !manifest
                .entries
                .iter()
                .any(|entry| entry.path == *entrypoint && entry.kind == EntryKind::File)
        }) {
            return Err("Cursor sidecar manifest entrypoint is not a declared file".into());
        }
        Ok(manifest)
    }

    pub fn generate(
        release: &ReleaseSpec,
        distribution: &Distribution,
        bytes: &[u8],
    ) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_COMPRESSED_BYTES {
            return Err("Cursor Agent archive exceeds the release compressed-size limit".into());
        }
        let entries = match distribution.archive_format {
            ArchiveFormat::TarGz => inspect_tar_gz(bytes)?,
            ArchiveFormat::Zip => inspect_zip(bytes)?,
        };
        if entries.len() > MAX_ARCHIVE_ENTRIES
            || entries
                .iter()
                .try_fold(0_u64, |total, entry| total.checked_add(entry.size))
                .is_none_or(|total| total > MAX_EXTRACTED_BYTES)
        {
            return Err("Cursor Agent archive exceeds the release extraction limits".into());
        }
        if !entries
            .iter()
            .any(|entry| entry.path == distribution.command && entry.kind == EntryKind::File)
            || distribution.entrypoint.as_ref().is_some_and(|entrypoint| {
                !entries
                    .iter()
                    .any(|entry| entry.path == *entrypoint && entry.kind == EntryKind::File)
            })
        {
            return Err(format!(
                "Cursor Agent archive is missing declared command `{}`",
                distribution.command
            ));
        }
        Ok(Self {
            format_version: 1,
            agent: "cursor".into(),
            version: release.version.clone(),
            os: distribution.os.clone(),
            architecture: distribution.architecture.clone(),
            archive_url: distribution.archive_url.clone(),
            archive_sha256: crate::syntax::package::sha256_hex(bytes),
            archive_format: distribution.archive_format,
            archive_size_bytes: bytes.len() as u64,
            max_compressed_bytes: MAX_COMPRESSED_BYTES,
            max_extracted_bytes: MAX_EXTRACTED_BYTES,
            max_entries: MAX_ARCHIVE_ENTRIES,
            command: distribution.command.clone(),
            entrypoint: distribution.entrypoint.clone(),
            args: distribution.args.clone(),
            entries,
            license_url: "https://cursor.com/terms-of-service".into(),
            terms_url: "https://cursor.com/terms-of-service".into(),
        })
    }
}

pub fn embedded_manifest() -> Result<SidecarManifest, String> {
    if EMBEDDED_MANIFEST.is_empty() {
        return Err("this Editur build does not include a Cursor Agent manifest".into());
    }
    SidecarManifest::parse(EMBEDDED_MANIFEST)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedEntry {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
}

fn inspect_zip(bytes: &[u8]) -> Result<Vec<ManagedEntry>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid Cursor Agent ZIP archive: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("Cursor Agent archive exceeds the release entry-count limit".into());
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut seen = HashSet::with_capacity(archive.len());
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect Cursor Agent archive: {error}"))?;
        let path = entry.name().to_owned();
        validate_relative_path(&path)?;
        if !seen.insert(path.clone()) {
            return Err(format!("duplicate archive path `{path}`"));
        }
        if entry.unix_mode().is_some_and(|mode| {
            let kind = mode & 0o170000;
            kind != 0 && kind != 0o040000 && kind != 0o100000
        }) {
            return Err(format!("archive contains special file `{path}`"));
        }
        let kind = if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let size = entry.size();
        let sha256 = (kind == EntryKind::File)
            .then(|| checksum_reader(&mut entry, size))
            .transpose()?;
        entries.push(ManagedEntry {
            path,
            kind,
            size,
            sha256,
            executable: kind == EntryKind::File
                && entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn inspect_tar_gz(bytes: &[u8]) -> Result<Vec<ManagedEntry>, String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for entry in archive
        .entries()
        .map_err(|error| format!("invalid Cursor Agent tar.gz archive: {error}"))?
    {
        let mut entry =
            entry.map_err(|error| format!("cannot inspect Cursor Agent archive: {error}"))?;
        if entries.len() >= MAX_ARCHIVE_ENTRIES {
            return Err("Cursor Agent archive exceeds the release entry-count limit".into());
        }
        let path = entry
            .path()
            .map_err(|error| format!("invalid Cursor Agent archive path: {error}"))?
            .to_str()
            .ok_or_else(|| "Cursor Agent archive path is not valid UTF-8".to_owned())?
            .to_owned();
        validate_relative_path(&path)?;
        if !seen.insert(path.clone()) {
            return Err(format!("duplicate archive path `{path}`"));
        }
        let entry_type = entry.header().entry_type();
        let kind = if entry_type.is_dir() {
            EntryKind::Directory
        } else if entry_type.is_file() {
            EntryKind::File
        } else {
            return Err(format!("archive contains special file `{path}`"));
        };
        let size = entry.size();
        let executable = kind == EntryKind::File
            && entry
                .header()
                .mode()
                .map_err(|error| format!("invalid mode for `{path}`: {error}"))?
                & 0o111
                != 0;
        let sha256 = (kind == EntryKind::File)
            .then(|| checksum_reader(&mut entry, size))
            .transpose()?;
        entries.push(ManagedEntry {
            path,
            kind,
            size,
            sha256,
            executable,
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn checksum_reader(reader: &mut impl Read, expected_size: u64) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash Cursor Agent archive entry: {error}"))?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| "Cursor Agent archive entry size overflowed".to_owned())?;
        if size > expected_size || size > MAX_EXTRACTED_BYTES {
            return Err("Cursor Agent archive entry exceeds its declared size".into());
        }
        hasher.update(&buffer[..count]);
    }
    if size != expected_size {
        return Err("Cursor Agent archive entry size does not match its header".into());
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[derive(Debug, Eq, PartialEq)]
pub struct InstalledSidecar {
    pub command: std::path::PathBuf,
    pub args: Vec<String>,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VerificationReceipt {
    version: String,
    archive_sha256: String,
    files: Vec<VerifiedFile>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifiedFile {
    path: String,
    modified_seconds: u64,
    modified_nanoseconds: u32,
}

pub fn installed(manifest: &SidecarManifest, data_dir: &Path) -> Result<InstalledSidecar, String> {
    validate_version(&manifest.version)?;
    let destination = data_dir
        .join("agents/cursor/versions")
        .join(&manifest.version);
    if !verification_receipt_matches(manifest, &destination) {
        verify_installed(manifest, &destination)?;
        let _ = write_verification_receipt(manifest, &destination);
    }
    Ok(installed_sidecar(manifest, &destination))
}

#[cfg(feature = "network")]
pub fn ensure(
    manifest: &SidecarManifest,
    data_dir: &Path,
    mut progress: impl FnMut(DownloadProgress),
) -> Result<InstalledSidecar, String> {
    if let Ok(installed) = installed(manifest, data_dir) {
        return with_provision_lock(data_dir, || {
            let agent_dir = data_dir.join("agents/cursor");
            let previous = activate(&agent_dir, &manifest.version)?;
            cleanup_obsolete_versions(
                &agent_dir.join("versions"),
                &manifest.version,
                previous.as_deref(),
            );
            Ok(installed)
        });
    }
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
    let download = tempfile::tempdir_in(data_dir)
        .map_err(|error| format!("cannot stage Cursor Agent download: {error}"))?;
    let mut response = ureq::get(&manifest.archive_url)
        .call()
        .map_err(|error| format!("cannot download {}: {error}", manifest.archive_url))?;
    if !valid_cursor_archive_uri(response.get_uri()) {
        return Err("Cursor archive redirect left https://downloads.cursor.com".into());
    }
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if total.is_some_and(|size| size > manifest.max_compressed_bytes) {
        return Err("Cursor Agent archive exceeds its compressed-size limit".into());
    }
    progress(DownloadProgress {
        downloaded: 0,
        total,
    });
    let archive_path = download.path().join("archive");
    let mut archive = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&archive_path)
        .map_err(|error| format!("cannot create {}: {error}", archive_path.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", manifest.archive_url))?;
        if count == 0 {
            break;
        }
        downloaded = downloaded
            .checked_add(count as u64)
            .ok_or_else(|| "Cursor Agent archive size overflowed".to_owned())?;
        if downloaded > manifest.max_compressed_bytes {
            return Err("Cursor Agent archive exceeds its compressed-size limit".into());
        }
        archive
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write {}: {error}", archive_path.display()))?;
        progress(DownloadProgress { downloaded, total });
    }
    archive
        .flush()
        .and_then(|()| archive.sync_all())
        .map_err(|error| format!("cannot finish {}: {error}", archive_path.display()))?;
    let bytes = fs::read(&archive_path)
        .map_err(|error| format!("cannot read {}: {error}", archive_path.display()))?;
    provision_from_bytes(manifest, data_dir, &bytes)
}

#[cfg(not(feature = "network"))]
pub fn ensure(
    manifest: &SidecarManifest,
    data_dir: &Path,
    _progress: impl FnMut(DownloadProgress),
) -> Result<InstalledSidecar, String> {
    installed(manifest, data_dir)
        .map_err(|_| "this Editur build cannot download Cursor Agent".into())
}

pub fn provision_from_bytes(
    manifest: &SidecarManifest,
    data_dir: &Path,
    bytes: &[u8],
) -> Result<InstalledSidecar, String> {
    provision_from_bytes_with(manifest, data_dir, bytes, |command, entrypoint, version| {
        let mut process = std::process::Command::new(command);
        if let Some(entrypoint) = entrypoint {
            process.arg(entrypoint);
        }
        let output = process
            .arg("--version")
            .current_dir(command.parent().unwrap_or(data_dir))
            .output()
            .map_err(|error| format!("cannot validate Cursor Agent: {error}"))?;
        let reported = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() || !reported.contains(version) {
            return Err(format!(
                "Cursor Agent reported an unexpected version: {}",
                reported.trim()
            ));
        }
        Ok(())
    })
}

fn provision_from_bytes_with(
    manifest: &SidecarManifest,
    data_dir: &Path,
    bytes: &[u8],
    validate: impl FnOnce(&Path, Option<&Path>, &str) -> Result<(), String>,
) -> Result<InstalledSidecar, String> {
    with_provision_lock(data_dir, || {
        provision_from_bytes_locked(manifest, data_dir, bytes, validate)
    })
}

fn provision_from_bytes_locked(
    manifest: &SidecarManifest,
    data_dir: &Path,
    bytes: &[u8],
    validate: impl FnOnce(&Path, Option<&Path>, &str) -> Result<(), String>,
) -> Result<InstalledSidecar, String> {
    validate_version(&manifest.version)?;
    validate_relative_path(&manifest.command)?;
    if let Some(entrypoint) = &manifest.entrypoint {
        validate_relative_path(entrypoint)?;
    }
    let agent_dir = data_dir.join("agents/cursor");
    let versions = agent_dir.join("versions");
    fs::create_dir_all(&versions)
        .map_err(|error| format!("cannot create {}: {error}", versions.display()))?;
    let destination = versions.join(&manifest.version);
    if destination.exists() && verify_installed(manifest, &destination).is_ok() {
        let _ = write_verification_receipt(manifest, &destination);
        let previous = activate(&agent_dir, &manifest.version)?;
        cleanup_obsolete_versions(&versions, &manifest.version, previous.as_deref());
        return Ok(installed_sidecar(manifest, &destination));
    }
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&versions)
        .map_err(|error| format!("cannot stage Cursor Agent: {error}"))?;
    extract_archive(manifest, bytes, staging.path())?;
    let staged_command = staging.path().join(&manifest.command);
    if !staged_command.is_file() {
        return Err(format!(
            "Cursor Agent command is missing: {}",
            manifest.command
        ));
    }
    let staged_entrypoint = manifest
        .entrypoint
        .as_ref()
        .map(|entrypoint| staging.path().join(entrypoint));
    if staged_entrypoint
        .as_ref()
        .is_some_and(|path| !path.is_file())
    {
        return Err("Cursor Agent entrypoint is missing".into());
    }
    validate(
        &staged_command,
        staged_entrypoint.as_deref(),
        &manifest.version,
    )?;
    let _ = write_verification_receipt(manifest, staging.path());
    let staged = staging.keep();
    let _replaced = if destination.exists() {
        let replaced = tempfile::Builder::new()
            .prefix(".replace-")
            .tempdir_in(&versions)
            .map_err(|error| format!("cannot stage Cursor Agent repair: {error}"))?;
        let previous = replaced.path().join("previous");
        fs::rename(&destination, &previous)
            .map_err(|error| format!("cannot stage corrupt Cursor Agent for repair: {error}"))?;
        if let Err(error) = fs::rename(&staged, &destination) {
            let rollback = fs::rename(&previous, &destination);
            let _ = fs::remove_dir_all(&staged);
            return Err(match rollback {
                Ok(()) => format!("cannot activate repaired Cursor Agent: {error}"),
                Err(rollback) => format!(
                    "cannot activate repaired Cursor Agent: {error}; rollback failed: {rollback}"
                ),
            });
        }
        Some(replaced)
    } else {
        if let Err(error) = fs::rename(&staged, &destination) {
            let _ = fs::remove_dir_all(&staged);
            return Err(format!("cannot activate Cursor Agent: {error}"));
        }
        None
    };
    let previous = activate(&agent_dir, &manifest.version)?;
    cleanup_obsolete_versions(&versions, &manifest.version, previous.as_deref());
    Ok(installed_sidecar(manifest, &destination))
}

fn with_provision_lock<T>(
    data_dir: &Path,
    run: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let agent_dir = data_dir.join("agents/cursor");
    fs::create_dir_all(&agent_dir)
        .map_err(|error| format!("cannot create {}: {error}", agent_dir.display()))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(agent_dir.join(".provision.lock"))
        .map_err(|error| format!("cannot open Cursor Agent provision lock: {error}"))?;
    lock.lock()
        .map_err(|error| format!("cannot lock Cursor Agent provisioning: {error}"))?;
    run()
}

fn installed_sidecar(manifest: &SidecarManifest, version_dir: &Path) -> InstalledSidecar {
    let mut args = manifest.args.clone();
    if let Some(entrypoint) = &manifest.entrypoint {
        args.insert(
            0,
            version_dir.join(entrypoint).to_string_lossy().into_owned(),
        );
    }
    InstalledSidecar {
        command: version_dir.join(&manifest.command),
        args,
        version: manifest.version.clone(),
    }
}

fn activate(agent_dir: &Path, version: &str) -> Result<Option<String>, String> {
    let active = read_version_marker(&agent_dir.join("active"));
    if active.as_deref() == Some(version) {
        return Ok(read_version_marker(&agent_dir.join("previous")));
    }
    let previous = if active.as_deref().is_some_and(|active| active != version) {
        active
    } else {
        read_version_marker(&agent_dir.join("previous"))
    };
    if let Some(previous) = &previous {
        write_version_marker(agent_dir, "previous", previous)?;
    }
    write_version_marker(agent_dir, "active", version)?;
    Ok(previous)
}

fn read_version_marker(path: &Path) -> Option<String> {
    let version = fs::read_to_string(path).ok()?;
    (Path::new(&version).components().count() == 1
        && matches!(
            Path::new(&version).components().next(),
            Some(Component::Normal(_))
        ))
    .then_some(version)
}

fn write_version_marker(agent_dir: &Path, name: &str, version: &str) -> Result<(), String> {
    let mut marker = tempfile::NamedTempFile::new_in(agent_dir)
        .map_err(|error| format!("cannot stage Cursor Agent {name} marker: {error}"))?;
    marker
        .write_all(version.as_bytes())
        .and_then(|()| marker.flush())
        .and_then(|()| marker.as_file().sync_all())
        .map_err(|error| format!("cannot write Cursor Agent {name} marker: {error}"))?;
    marker
        .persist(agent_dir.join(name))
        .map_err(|error| format!("cannot activate Cursor Agent: {}", error.error))?;
    Ok(())
}

fn cleanup_obsolete_versions(versions: &Path, current: &str, previous: Option<&str>) {
    let Ok(entries) = fs::read_dir(versions) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == current || previous == Some(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && !file_type.is_symlink() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn verify_installed(manifest: &SidecarManifest, version_dir: &Path) -> Result<(), String> {
    for entry in &manifest.entries {
        let path = version_dir.join(&entry.path);
        verify_metadata(entry, &path)?;
        if entry.kind == EntryKind::File {
            let checksum = file_checksum(&path)?;
            if entry
                .sha256
                .as_ref()
                .is_none_or(|expected| !checksum.eq_ignore_ascii_case(expected))
            {
                return Err(format!(
                    "managed Cursor Agent file has changed: {}",
                    entry.path
                ));
            }
        }
    }
    Ok(())
}

fn verification_receipt_matches(manifest: &SidecarManifest, version_dir: &Path) -> bool {
    let Ok(bytes) = fs::read(version_dir.join(VERIFICATION_RECEIPT)) else {
        return false;
    };
    let Ok(receipt) = serde_json::from_slice::<VerificationReceipt>(&bytes) else {
        return false;
    };
    receipt.version == manifest.version
        && receipt
            .archive_sha256
            .eq_ignore_ascii_case(&manifest.archive_sha256)
        && verification_receipt(manifest, version_dir).as_ref() == Ok(&receipt)
}

fn write_verification_receipt(
    manifest: &SidecarManifest,
    version_dir: &Path,
) -> Result<(), String> {
    let receipt = verification_receipt(manifest, version_dir)?;
    let bytes = serde_json::to_vec(&receipt)
        .map_err(|error| format!("cannot encode Cursor Agent verification receipt: {error}"))?;
    let mut staged = tempfile::NamedTempFile::new_in(version_dir)
        .map_err(|error| format!("cannot stage Cursor Agent verification receipt: {error}"))?;
    staged
        .write_all(&bytes)
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| format!("cannot write Cursor Agent verification receipt: {error}"))?;
    staged
        .persist(version_dir.join(VERIFICATION_RECEIPT))
        .map_err(|error| {
            format!(
                "cannot save Cursor Agent verification receipt: {}",
                error.error
            )
        })?;
    Ok(())
}

fn verification_receipt(
    manifest: &SidecarManifest,
    version_dir: &Path,
) -> Result<VerificationReceipt, String> {
    let mut files = Vec::new();
    for entry in &manifest.entries {
        let path = version_dir.join(&entry.path);
        let metadata = verify_metadata(entry, &path)?;
        if entry.kind != EntryKind::File {
            continue;
        }
        let modified = metadata
            .modified()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        files.push(VerifiedFile {
            path: entry.path.clone(),
            modified_seconds: modified.as_secs(),
            modified_nanoseconds: modified.subsec_nanos(),
        });
    }
    Ok(VerificationReceipt {
        version: manifest.version.clone(),
        archive_sha256: manifest.archive_sha256.clone(),
        files,
    })
}

fn verify_metadata(entry: &ManagedEntry, path: &Path) -> Result<fs::Metadata, String> {
    validate_relative_path(&entry.path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("managed Cursor Agent file is missing: {error}"))?;
    if metadata.file_type().is_symlink()
        || (entry.kind == EntryKind::File && !metadata.is_file())
        || (entry.kind == EntryKind::Directory && !metadata.is_dir())
        || (entry.kind == EntryKind::File && metadata.len() != entry.size)
        || (entry.kind == EntryKind::File && !executable_matches(&metadata, entry.executable))
    {
        return Err(format!(
            "managed Cursor Agent file has changed: {}",
            entry.path
        ));
    }
    Ok(metadata)
}

fn file_checksum(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("cannot verify {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot verify {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(unix)]
fn executable_matches(metadata: &fs::Metadata, executable: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;

    (metadata.permissions().mode() & 0o111 != 0) == executable
}

#[cfg(not(unix))]
fn executable_matches(_metadata: &fs::Metadata, _executable: bool) -> bool {
    true
}

fn verify_archive(manifest: &SidecarManifest, bytes: &[u8]) -> Result<(), String> {
    let size = u64::try_from(bytes.len()).map_err(|_| "Cursor Agent archive is too large")?;
    if size > manifest.max_compressed_bytes || size != manifest.archive_size_bytes {
        return Err("Cursor Agent archive exceeds its compressed-size limit".into());
    }
    let checksum = crate::syntax::package::sha256_hex(bytes);
    if !checksum.eq_ignore_ascii_case(&manifest.archive_sha256) {
        return Err("Cursor Agent archive failed SHA-256 verification".into());
    }
    Ok(())
}

fn extract_archive(
    manifest: &SidecarManifest,
    bytes: &[u8],
    destination: &Path,
) -> Result<(), String> {
    verify_archive(manifest, bytes)?;
    if manifest.archive_format == ArchiveFormat::TarGz {
        return extract_tar_gz(manifest, bytes, destination);
    }
    let mut expected = HashMap::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        if expected.insert(entry.path.as_str(), entry).is_some() {
            return Err(format!("duplicate manifest path `{}`", entry.path));
        }
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid Cursor Agent ZIP archive: {error}"))?;
    if archive.len() > manifest.max_entries {
        return Err("Cursor Agent archive exceeds its entry-count limit".into());
    }
    let mut extracted_bytes = 0_u64;
    let mut seen = HashSet::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect Cursor Agent archive: {error}"))?;
        let name = entry.name();
        validate_relative_path(name)?;
        if !seen.insert(name.to_owned()) {
            return Err(format!("duplicate archive path `{name}`"));
        }
        let expected = expected
            .get(name)
            .ok_or_else(|| format!("unexpected archive path `{name}`"))?;
        let kind = if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        if kind != expected.kind || entry.size() != expected.size {
            return Err(format!("archive metadata does not match `{name}`"));
        }
        if entry.unix_mode().is_some_and(|mode| {
            let kind = mode & 0o170000;
            kind != 0 && kind != 0o040000 && kind != 0o100000
        }) {
            return Err(format!("archive contains special file `{name}`"));
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "Cursor Agent archive extracted size overflowed".to_owned())?;
        if extracted_bytes > manifest.max_extracted_bytes {
            return Err("Cursor Agent archive exceeds its extracted-size limit".into());
        }
    }
    if seen.len() != manifest.entries.len() {
        return Err("Cursor Agent archive does not match its pinned entry count".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid Cursor Agent ZIP archive: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot read Cursor Agent archive: {error}"))?;
        let expected = expected
            .get(entry.name())
            .ok_or_else(|| format!("unexpected archive path `{}`", entry.name()))?;
        let output = destination.join(&expected.path);
        if expected.kind == EntryKind::Directory {
            fs::create_dir_all(&output)
                .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = entry
                .read(&mut buffer)
                .map_err(|error| format!("cannot extract {}: {error}", output.display()))?;
            if count == 0 {
                break;
            }
            written = written
                .checked_add(count as u64)
                .ok_or_else(|| "Cursor Agent archive extracted size overflowed".to_owned())?;
            if written > expected.size {
                return Err(format!(
                    "archive entry `{}` exceeds its pinned size",
                    expected.path
                ));
            }
            file.write_all(&buffer[..count])
                .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
            hasher.update(&buffer[..count]);
        }
        let checksum = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if written != expected.size
            || expected
                .sha256
                .as_ref()
                .is_none_or(|expected| !checksum.eq_ignore_ascii_case(expected))
        {
            return Err(format!(
                "archive entry `{}` failed verification",
                expected.path
            ));
        }
        set_executable(&output, expected.executable)?;
    }
    Ok(())
}

fn extract_tar_gz(
    manifest: &SidecarManifest,
    bytes: &[u8],
    destination: &Path,
) -> Result<(), String> {
    let mut expected = HashMap::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        if expected.insert(entry.path.as_str(), entry).is_some() {
            return Err(format!("duplicate manifest path `{}`", entry.path));
        }
    }
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("invalid Cursor Agent tar.gz archive: {error}"))?;
    let mut count = 0_usize;
    let mut extracted_bytes = 0_u64;
    let mut seen = HashSet::with_capacity(manifest.entries.len());
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot inspect Cursor Agent archive: {error}"))?;
        count += 1;
        if count > manifest.max_entries {
            return Err("Cursor Agent archive exceeds its entry-count limit".into());
        }
        let path = entry
            .path()
            .map_err(|error| format!("invalid Cursor Agent archive path: {error}"))?;
        let name = path
            .to_str()
            .ok_or_else(|| "Cursor Agent archive path is not valid UTF-8".to_owned())?;
        validate_relative_path(name)?;
        if !seen.insert(name.to_owned()) {
            return Err(format!("duplicate archive path `{name}`"));
        }
        let expected = expected
            .get(name)
            .ok_or_else(|| format!("unexpected archive path `{name}`"))?;
        let entry_type = entry.header().entry_type();
        let kind = if entry_type.is_dir() {
            EntryKind::Directory
        } else if entry_type.is_file() {
            EntryKind::File
        } else {
            return Err(format!("archive contains special file `{name}`"));
        };
        let size = entry.size();
        if kind != expected.kind || size != expected.size {
            return Err(format!("archive metadata does not match `{name}`"));
        }
        extracted_bytes = extracted_bytes
            .checked_add(size)
            .ok_or_else(|| "Cursor Agent archive extracted size overflowed".to_owned())?;
        if extracted_bytes > manifest.max_extracted_bytes {
            return Err("Cursor Agent archive exceeds its extracted-size limit".into());
        }
    }
    if count != manifest.entries.len() {
        return Err("Cursor Agent archive entry count does not match its manifest".into());
    }

    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .map_err(|error| format!("invalid Cursor Agent tar.gz archive: {error}"))?
    {
        let mut entry =
            entry.map_err(|error| format!("cannot read Cursor Agent archive: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("invalid Cursor Agent archive path: {error}"))?;
        let name = path
            .to_str()
            .ok_or_else(|| "Cursor Agent archive path is not valid UTF-8".to_owned())?;
        let expected = expected
            .get(name)
            .ok_or_else(|| format!("unexpected archive path `{name}`"))?;
        let output = destination.join(&expected.path);
        if expected.kind == EntryKind::Directory {
            fs::create_dir_all(&output)
                .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
        let mut hasher = Sha256::new();
        let mut written = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| format!("cannot extract {}: {error}", output.display()))?;
            if read == 0 {
                break;
            }
            written = written
                .checked_add(read as u64)
                .ok_or_else(|| "Cursor Agent archive extracted size overflowed".to_owned())?;
            if written > expected.size {
                return Err(format!(
                    "archive entry `{}` exceeds its pinned size",
                    expected.path
                ));
            }
            file.write_all(&buffer[..read])
                .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
            hasher.update(&buffer[..read]);
        }
        let checksum = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if written != expected.size
            || expected
                .sha256
                .as_ref()
                .is_none_or(|expected| !checksum.eq_ignore_ascii_case(expected))
        {
            return Err(format!(
                "archive entry `{}` failed verification",
                expected.path
            ));
        }
        set_executable(&output, expected.executable)?;
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe archive path `{value}`"));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || Path::new(version).components().count() != 1
        || !matches!(
            Path::new(version).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err("invalid Cursor Agent version in sidecar manifest".into());
    }
    Ok(())
}

#[cfg(feature = "network")]
fn valid_cursor_archive_uri(uri: &ureq::http::Uri) -> bool {
    uri.scheme_str() == Some("https")
        && uri.authority().is_some_and(|authority| {
            authority.host() == "downloads.cursor.com"
                && authority.port_u16().is_none_or(|port| port == 443)
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )
    .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<(), String> {
    Ok(())
}

impl ReleaseSpec {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let spec: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid Cursor release spec: {error}"))?;
        if spec.distributions.iter().any(|distribution| {
            !distribution
                .archive_url
                .starts_with("https://downloads.cursor.com/")
        }) {
            return Err("Cursor archive URL must use https://downloads.cursor.com".into());
        }
        Ok(spec)
    }

    pub fn select(&self, os: &str, architecture: &str) -> Result<&Distribution, String> {
        self.distributions
            .iter()
            .find(|distribution| distribution.os == os && distribution.architecture == architecture)
            .ok_or_else(|| format!("Cursor Agent is unavailable for {os}/{architecture}"))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    #[cfg(unix)]
    use super::provision_from_bytes;
    #[cfg(feature = "network")]
    use super::valid_cursor_archive_uri;
    use super::{
        ArchiveFormat, EntryKind, MAX_ARCHIVE_ENTRIES, ManagedEntry, ReleaseSpec, SidecarManifest,
        cleanup_obsolete_versions, embedded_manifest, ensure, extract_archive, installed,
        provision_from_bytes_with, verify_archive, verify_installed,
    };

    fn manifest() -> SidecarManifest {
        SidecarManifest {
            format_version: 1,
            agent: "cursor".into(),
            version: "2026.07.23-e383d2b".into(),
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            archive_url: "https://downloads.cursor.com/lab/pinned/agent.tar.gz".into(),
            archive_sha256: "770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c"
                .into(),
            archive_format: ArchiveFormat::TarGz,
            archive_size_bytes: 4,
            max_compressed_bytes: 8,
            max_extracted_bytes: 16,
            max_entries: 4,
            command: "dist-package/agent".into(),
            entrypoint: None,
            args: vec!["acp".into()],
            entries: Vec::new(),
            license_url: "https://cursor.com/terms-of-service".into(),
            terms_url: "https://cursor.com/terms-of-service".into(),
        }
    }

    fn zip_manifest(bytes: &[u8]) -> SidecarManifest {
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        manifest
    }

    #[test]
    fn release_spec_selects_the_pinned_current_platform() {
        let spec = ReleaseSpec::parse(
            br#"{
                "version": "2026.07.23-e383d2b",
                "distributions": [{
                    "os": "macos",
                    "architecture": "aarch64",
                    "archive_url": "https://downloads.cursor.com/lab/pinned/agent.tar.gz",
                    "command": "dist-package/cursor-agent",
                    "args": ["acp"],
                    "archive_format": "tar_gz"
                }]
            }"#,
        )
        .unwrap();

        let distribution = spec.select("macos", "aarch64").unwrap();

        assert_eq!(
            (
                spec.version.as_str(),
                distribution.archive_url.as_str(),
                distribution.archive_format,
            ),
            (
                "2026.07.23-e383d2b",
                "https://downloads.cursor.com/lab/pinned/agent.tar.gz",
                ArchiveFormat::TarGz,
            )
        );
    }

    #[test]
    fn release_spec_refuses_an_unpinned_download_host() {
        let error = ReleaseSpec::parse(
            br#"{
                "version": "2026.07.23-e383d2b",
                "distributions": [{
                    "os": "macos",
                    "architecture": "aarch64",
                    "archive_url": "https://downloads.cursor.com.evil.test/agent.tar.gz",
                    "command": "dist-package/cursor-agent",
                    "args": ["acp"],
                    "archive_format": "tar_gz"
                }]
            }"#,
        )
        .unwrap_err();

        assert!(error.contains("downloads.cursor.com"), "{error}");
    }

    #[cfg(feature = "network")]
    #[test]
    fn redirect_validation_keeps_the_exact_cursor_host() {
        assert!(valid_cursor_archive_uri(
            &"https://downloads.cursor.com/agent.tar.gz".parse().unwrap()
        ));
        assert!(!valid_cursor_archive_uri(
            &"https://downloads.cursor.com.evil.test/agent.tar.gz"
                .parse()
                .unwrap()
        ));
        assert!(!valid_cursor_archive_uri(
            &"http://downloads.cursor.com/agent.tar.gz".parse().unwrap()
        ));
    }

    #[test]
    fn cleanup_keeps_the_current_and_one_prior_managed_version() {
        let temp = tempfile::tempdir().unwrap();
        for version in ["current", "prior", "obsolete"] {
            std::fs::create_dir(temp.path().join(version)).unwrap();
        }

        cleanup_obsolete_versions(temp.path(), "current", Some("prior"));

        assert!(temp.path().join("current").is_dir());
        assert!(temp.path().join("prior").is_dir());
        assert!(!temp.path().join("obsolete").exists());
    }

    #[test]
    fn installed_directory_size_is_not_compared_with_archive_header_size() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("dist-package")).unwrap();
        let mut manifest = manifest();
        manifest.entries = vec![ManagedEntry {
            path: "dist-package".into(),
            kind: EntryKind::Directory,
            size: 0,
            sha256: None,
            executable: false,
        }];

        verify_installed(&manifest, temp.path()).unwrap();
    }

    #[test]
    fn development_build_without_a_release_manifest_fails_clearly() {
        let error = embedded_manifest().unwrap_err();

        assert!(
            error.contains("does not include a Cursor Agent manifest"),
            "{error}"
        );
    }

    #[test]
    fn archive_verification_refuses_the_wrong_checksum() {
        let error = verify_archive(&manifest(), b"evil").unwrap_err();

        assert!(error.contains("SHA-256"), "{error}");
    }

    #[test]
    fn archive_verification_refuses_an_oversized_response() {
        let mut manifest = manifest();
        manifest.max_compressed_bytes = 3;

        let error = verify_archive(&manifest, b"good").unwrap_err();

        assert!(error.contains("compressed-size limit"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_an_unsupported_archive() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["archive_format"] = serde_json::Value::String("raw".into());

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("unknown variant `raw`"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_an_unpinned_download_host() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["archive_url"] =
            serde_json::Value::String("https://downloads.cursor.com.evil.test/agent.zip".into());

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("downloads.cursor.com"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_a_different_platform() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["os"] = serde_json::Value::String(
            if cfg!(target_os = "windows") {
                "linux"
            } else {
                "windows"
            }
            .into(),
        );

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("current platform"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_an_unknown_format_version() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["format_version"] = serde_json::Value::from(2);

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("format version"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_unsafe_managed_paths() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["command"] = serde_json::Value::String("../agent".into());

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("unsafe archive path"), "{error}");
    }

    #[test]
    fn manifest_parser_requires_unique_declared_executables() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        let entry = serde_json::json!({
            "path": "dist-package/agent",
            "kind": "file",
            "size": 5,
            "sha256": crate::syntax::package::sha256_hex(b"agent"),
            "executable": true
        });
        json["entries"] = serde_json::json!([entry.clone(), entry]);

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("duplicate"), "{error}");

        json["entries"] = serde_json::json!([]);
        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(error.contains("command"), "{error}");
    }

    #[test]
    fn manifest_parser_refuses_limits_above_the_compiled_ceiling() {
        let mut json = serde_json::to_value(manifest()).unwrap();
        json["max_entries"] = serde_json::Value::from(MAX_ARCHIVE_ENTRIES + 1);

        let error = SidecarManifest::parse(&serde_json::to_vec(&json).unwrap()).unwrap_err();

        assert!(error.contains("unsafe extraction limits"), "{error}");
    }

    #[test]
    fn extraction_rejects_path_traversal_without_writing_outside_staging() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"bad").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        let temp = tempfile::tempdir().unwrap();

        let error = extract_archive(&manifest, &bytes, temp.path()).unwrap_err();

        assert!(
            error.contains("unsafe archive path") && !temp.path().join("escape").exists(),
            "{error}"
        );
    }

    #[test]
    fn extraction_rejects_the_extracted_size_limit() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"123456789").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 8;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 9,
            sha256: Some(crate::syntax::package::sha256_hex(b"123456789")),
            executable: false,
        }];
        let temp = tempfile::tempdir().unwrap();

        let error = extract_archive(&manifest, &bytes, temp.path()).unwrap_err();

        assert!(error.contains("extracted-size limit"), "{error}");
    }

    #[test]
    fn extraction_writes_only_the_files_pinned_by_the_manifest() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();

        extract_archive(&manifest, &bytes, temp.path()).unwrap();

        assert_eq!(
            std::fs::read(temp.path().join("dist-package/agent")).unwrap(),
            b"agent"
        );
    }

    #[test]
    fn tar_gz_extraction_writes_the_pinned_executable() {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_path("dist-package/agent").unwrap();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append(&header, &b"agent"[..]).unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let mut manifest = manifest();
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();

        extract_archive(&manifest, &bytes, temp.path()).unwrap();

        assert_eq!(
            std::fs::read(temp.path().join("dist-package/agent")).unwrap(),
            b"agent"
        );
    }

    #[test]
    fn provisioner_activates_a_staged_version_only_after_validation() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();

        let installed =
            provision_from_bytes_with(&manifest, temp.path(), &bytes, |path, _, version| {
                assert_eq!(
                    (std::fs::read(path).unwrap(), version),
                    (b"agent".to_vec(), manifest.version.as_str())
                );
                Ok(())
            })
            .unwrap();

        assert_eq!(
            (
                installed.command,
                std::fs::read_to_string(temp.path().join("agents/cursor/active")).unwrap(),
            ),
            (
                temp.path()
                    .join("agents/cursor/versions")
                    .join(&manifest.version)
                    .join("dist-package/agent"),
                manifest.version,
            )
        );
    }

    #[test]
    fn failed_update_leaves_the_prior_version_active() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let temp = tempfile::tempdir().unwrap();
        let mut prior = zip_manifest(&bytes);
        prior.version = "prior".into();
        provision_from_bytes_with(&prior, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();
        let mut next = zip_manifest(&bytes);
        next.version = "next".into();

        assert!(
            provision_from_bytes_with(&next, temp.path(), &bytes, |_, _, _| {
                Err("interrupted validation".into())
            })
            .is_err()
        );

        let agent_dir = temp.path().join("agents/cursor");
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("active")).unwrap(),
            "prior"
        );
        assert!(
            agent_dir
                .join("versions/prior/dist-package/agent")
                .is_file()
        );
        assert!(!agent_dir.join("versions/next").exists());
    }

    #[test]
    fn provisioner_refuses_a_missing_declared_entrypoint() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/node.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"node").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 4;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.command = "dist-package/node.exe".into();
        manifest.entrypoint = Some("dist-package/index.js".into());
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/node.exe".into(),
            kind: EntryKind::File,
            size: 4,
            sha256: Some(crate::syntax::package::sha256_hex(b"node")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();

        let error = provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(()))
            .unwrap_err();

        assert!(error.contains("entrypoint is missing"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn provisioner_refuses_a_command_reporting_the_wrong_version() {
        let script = b"#!/bin/sh\necho wrong-version\n";
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
        archive.write_all(script).unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = script.len() as u64;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: script.len() as u64,
            sha256: Some(crate::syntax::package::sha256_hex(script)),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();

        let error = provision_from_bytes(&manifest, temp.path(), &bytes).unwrap_err();

        assert!(error.contains("reported an unexpected version"), "{error}");
    }

    #[test]
    fn an_already_verified_version_is_an_idempotent_no_op() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();
        let first =
            provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();

        let second = provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| {
            panic!("an already verified package must not be revalidated")
        })
        .unwrap();

        assert_eq!(second, first);
    }

    #[test]
    fn provisioning_records_a_fast_launch_verification_receipt() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let manifest = zip_manifest(&bytes);
        let temp = tempfile::tempdir().unwrap();

        provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();

        assert!(
            temp.path()
                .join("agents/cursor/versions")
                .join(&manifest.version)
                .join(".editur-verified.json")
                .is_file()
        );
    }

    #[test]
    fn launch_receipt_does_not_hide_a_changed_managed_file() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let manifest = zip_manifest(&bytes);
        let temp = tempfile::tempdir().unwrap();
        let sidecar =
            provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();
        std::fs::write(&sidecar.command, b"evil!").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&sidecar.command)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();

        assert!(installed(&manifest, temp.path()).is_err());
    }

    #[test]
    fn concurrent_provisioning_is_serialized() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = std::sync::Arc::new(archive.finish().unwrap().into_inner());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let first_root = root.clone();
        let first_bytes = std::sync::Arc::clone(&bytes);
        let (first_started_tx, first_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = std::thread::spawn(move || {
            provision_from_bytes_with(
                &zip_manifest(first_bytes.as_slice()),
                &first_root,
                first_bytes.as_slice(),
                |_, _, _| {
                    first_started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
            )
        });
        first_started_rx.recv().unwrap();

        let second_bytes = std::sync::Arc::clone(&bytes);
        let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            provision_from_bytes_with(
                &zip_manifest(second_bytes.as_slice()),
                &root,
                second_bytes.as_slice(),
                |_, _, _| {
                    second_started_tx.send(()).unwrap();
                    Ok(())
                },
            )
        });
        let raced = second_started_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_ok();
        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert!(
            !raced,
            "a second provisioner entered the staging transaction"
        );
    }

    #[test]
    fn ensuring_an_already_verified_version_does_not_download() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_url = "https://downloads.cursor.com/this-must-not-be-requested".into();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();
        let installed =
            provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();
        let active = temp.path().join("agents/cursor/active");
        let unchanged = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(&active)
            .unwrap()
            .set_modified(unchanged)
            .unwrap();

        let ensured = ensure(&manifest, temp.path(), |_| {}).unwrap();

        assert_eq!(ensured, installed);
        assert_eq!(
            std::fs::metadata(active).unwrap().modified().unwrap(),
            unchanged
        );
    }

    #[test]
    fn a_corrupt_managed_version_is_repaired_from_the_pinned_archive() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let mut manifest = manifest();
        manifest.archive_format = ArchiveFormat::Zip;
        manifest.archive_size_bytes = bytes.len() as u64;
        manifest.max_compressed_bytes = bytes.len() as u64;
        manifest.max_extracted_bytes = 5;
        manifest.max_entries = 1;
        manifest.archive_sha256 = crate::syntax::package::sha256_hex(&bytes);
        manifest.entries = vec![ManagedEntry {
            path: "dist-package/agent".into(),
            kind: EntryKind::File,
            size: 5,
            sha256: Some(crate::syntax::package::sha256_hex(b"agent")),
            executable: true,
        }];
        let temp = tempfile::tempdir().unwrap();
        let installed =
            provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| Ok(())).unwrap();
        std::fs::write(&installed.command, b"evil!").unwrap();
        let validated = std::cell::Cell::new(false);

        provision_from_bytes_with(&manifest, temp.path(), &bytes, |_, _, _| {
            validated.set(true);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            (std::fs::read(&installed.command).unwrap(), validated.get()),
            (b"agent".to_vec(), true)
        );
    }

    #[test]
    fn generated_manifest_pins_every_archive_file_and_checksum() {
        let spec = ReleaseSpec::parse(
            br#"{
                "version": "2026.07.23-e383d2b",
                "distributions": [{
                    "os": "macos",
                    "architecture": "aarch64",
                    "archive_url": "https://downloads.cursor.com/lab/pinned/agent.zip",
                    "command": "dist-package/agent",
                    "args": ["acp"],
                    "archive_format": "zip"
                }]
            }"#,
        )
        .unwrap();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "dist-package/agent",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"agent").unwrap();
        let bytes = archive.finish().unwrap().into_inner();

        let generated =
            SidecarManifest::generate(&spec, spec.select("macos", "aarch64").unwrap(), &bytes)
                .unwrap();

        let expected_file_checksum = crate::syntax::package::sha256_hex(b"agent");
        assert_eq!(
            (
                generated.archive_sha256,
                generated.entries[0].path.as_str(),
                generated.entries[0].sha256.as_deref(),
            ),
            (
                crate::syntax::package::sha256_hex(&bytes),
                "dist-package/agent",
                Some(expected_file_checksum.as_str()),
            )
        );
    }
}
