use std::{collections::HashSet, env, fs, path::PathBuf};

use editur::agent::provision::{ProviderBundle, SidecarManifest};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let output = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: build_provider_bundle OUTPUT MANIFEST...")?;
    let mut providers = Vec::new();
    let mut ids = HashSet::new();
    for path in arguments.map(PathBuf::from) {
        let manifest = SidecarManifest::parse(&fs::read(&path)?)?;
        if !ids.insert(manifest.provider()?) {
            return Err(format!("duplicate provider manifest {}", path.display()).into());
        }
        providers.push(manifest);
    }
    if providers.is_empty() {
        return Err("provider bundle requires at least one manifest".into());
    }
    let bundle = ProviderBundle {
        format_version: 2,
        providers,
    };
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    ProviderBundle::parse(&bytes)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, bytes)?;
    Ok(())
}
