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
    let spec_path = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "usage: build_agent_manifest SPEC OS ARCH OUTPUT".to_owned())?,
    );
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
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let release = ReleaseSpec::parse(
        &fs::read(&spec_path)
            .map_err(|error| format!("cannot read {}: {error}", spec_path.display()))?,
    )?;
    let distribution = release.select(&os, &architecture)?;
    let mut response = ureq::get(&distribution.archive_url)
        .call()
        .map_err(|error| format!("cannot download {}: {error}", distribution.archive_url))?;
    if response.get_uri().scheme_str() != Some("https")
        || response
            .get_uri()
            .authority()
            .is_none_or(|authority| authority.host() != "downloads.cursor.com")
    {
        return Err("Cursor archive redirect left https://downloads.cursor.com".into());
    }
    let archive = response
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE_BYTES)
        .read_to_vec()
        .map_err(|error| format!("cannot read {}: {error}", distribution.archive_url))?;
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
