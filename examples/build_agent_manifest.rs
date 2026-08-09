use std::{env, fs, path::PathBuf};

use editur::agent::provision::{ReleaseSpec, SidecarManifest};
use ureq::ResponseExt;

const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("editur agent manifest: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let spec_path = PathBuf::from(arguments.next().ok_or_else(|| {
        "usage: build_agent_manifest SPEC OS ARCH OUTPUT [LOCAL_ARCHIVE]".to_owned()
    })?);
    let os = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| "OS must be valid UTF-8".to_owned())?;
    let architecture = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| "architecture must be valid UTF-8".to_owned())?;
    let output = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "missing output path".to_owned())?,
    );
    let local_archive = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let release = ReleaseSpec::parse(
        &fs::read(&spec_path)
            .map_err(|error| format!("cannot read {}: {error}", spec_path.display()))?,
    )?;
    let distribution = release.select(&os, &architecture)?;
    let archive = if let Some(path) = local_archive {
        let bytes =
            fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(format!("{} exceeds the archive size limit", path.display()));
        }
        bytes
    } else {
        let mut response = ureq::get(&distribution.archive_url)
            .call()
            .map_err(|error| format!("cannot download {}: {error}", distribution.archive_url))?;
        if !release.valid_archive_uri(response.get_uri()) {
            return Err("ACP provider archive redirect left its approved host".into());
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_ARCHIVE_BYTES)
            .read_to_vec()
            .map_err(|error| format!("cannot read {}: {error}", distribution.archive_url))?
    };
    let manifest = SidecarManifest::generate(&release, distribution, &archive)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(
        &output,
        serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("cannot serialize manifest: {error}"))?,
    )
    .map_err(|error| format!("cannot write {}: {error}", output.display()))
}
