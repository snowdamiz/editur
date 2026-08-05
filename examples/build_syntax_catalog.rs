use std::{
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use editur::syntax::package::{Catalog, CatalogEntry, Manifest};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let output = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dist/syntax"));
    let base_url = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "https://github.com/sn0w/editur/releases/download/syntax-v1".to_owned());
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --example build_syntax_catalog -- [OUTPUT] [BASE_URL]".into(),
        );
    }
    fs::create_dir_all(&output)?;

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let mut packages = Vec::new();
    for source in package_sources(Path::new("syntax-packages"))? {
        let manifest_bytes = fs::read(source.join("manifest.json"))?;
        let manifest = Manifest::parse(&manifest_bytes)?;
        manifest.validate(env!("CARGO_PKG_VERSION"))?;
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        archive.start_file("manifest.json", options)?;
        std::io::Write::write_all(&mut archive, &manifest_bytes)?;
        for grammar in &manifest.grammars {
            archive.start_file(grammar, options)?;
            std::io::Write::write_all(&mut archive, &fs::read(source.join(grammar))?)?;
        }
        archive.start_file("LICENSES/LICENSE.txt", options)?;
        std::io::Write::write_all(
            &mut archive,
            &fs::read(source.join("LICENSES/LICENSE.txt"))?,
        )?;
        let bytes = archive.finish()?.into_inner();
        let filename = format!("{}-{}.editur-syntax", manifest.id, manifest.version);
        fs::write(output.join(&filename), &bytes)?;
        packages.push(CatalogEntry {
            id: manifest.id,
            display_name: manifest.display_name,
            version: manifest.version,
            url: format!("{}/{filename}", base_url.trim_end_matches('/')),
            sha256: sha256_hex(&bytes),
        });
    }
    fs::write(
        output.join("catalog.json"),
        serde_json::to_vec_pretty(&Catalog {
            format_version: 1,
            packages,
        })?,
    )?;
    Ok(())
}

fn package_sources(root: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut sources = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.json").is_file())
        .collect::<Vec<_>>();
    sources.sort();
    Ok(sources)
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::package_sources;
    use std::path::Path;

    #[test]
    fn catalog_discovers_every_shipped_package_in_stable_order() {
        let sources = package_sources(Path::new("syntax-packages")).unwrap();
        let ids: Vec<_> = sources
            .iter()
            .filter_map(|path| path.file_name()?.to_str())
            .collect();

        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.contains(&"typescript"));
    }
}
