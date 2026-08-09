use std::{env, fs, path::PathBuf};

use editur::agent::provision::{SidecarManifest, provision_from_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let manifest = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: verify_provider_package MANIFEST ARCHIVE")?;
    let archive = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("missing archive path")?;
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }
    let manifest = SidecarManifest::parse(&fs::read(manifest)?)?;
    let data = tempfile::tempdir()?;
    let installed = provision_from_bytes(&manifest, data.path(), &fs::read(archive)?)?;
    if manifest.agent == "codex" {
        let entrypoint = PathBuf::from(&installed.args[0]);
        let version_root = entrypoint
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Codex entrypoint has no package root")?
            .parent()
            .ok_or("Codex package has no version root")?;
        let output = std::process::Command::new(&installed.command)
            .arg(version_root.join("package/node_modules/@openai/codex/bin/codex.js"))
            .arg("--version")
            .output()?;
        let reported = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() || !reported.contains("0.147.0") {
            return Err(
                format!("Codex dependency reported an unexpected version: {reported}").into(),
            );
        }
    }
    println!(
        "Verified {} {} at {}",
        manifest.agent,
        installed.version,
        installed.command.display()
    );
    Ok(())
}
