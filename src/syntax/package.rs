use crate::cli::valid_language_id;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use syntect::parsing::{ParseState, SyntaxSet, SyntaxSetBuilder};

const MAX_ARCHIVE_SIZE: usize = 2 * 1024 * 1024;
const MAX_UNPACKED_SIZE: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const MAX_CATALOG_SIZE: u64 = 512 * 1024;
pub const OFFICIAL_CATALOG: &str =
    "https://github.com/snowdamiz/editur/releases/download/syntax-v1/catalog.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub minimum_editur_version: String,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub grammars: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub format_version: u32,
    pub packages: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let catalog: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid syntax catalog: {error}"))?;
        if catalog.format_version != 1 {
            return Err(format!(
                "unsupported syntax catalog format {}",
                catalog.format_version
            ));
        }
        let mut ids = HashSet::new();
        for package in &catalog.packages {
            if !valid_language_id(&package.id)
                || package.display_name.trim().is_empty()
                || version_core(&package.version).is_err()
                || !package.url.starts_with("https://")
                || package.url.chars().any(char::is_whitespace)
                || package.sha256.len() != 64
                || !package.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !ids.insert(&package.id)
            {
                return Err(format!("invalid catalog entry for `{}`", package.id));
            }
        }
        Ok(catalog)
    }

    pub fn resolve(&self, id: &str) -> Option<&CatalogEntry> {
        self.packages.iter().find(|package| package.id == id)
    }
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|error| format!("invalid manifest.json: {error}"))
    }

    pub fn validate(&self, editur_version: &str) -> Result<(), String> {
        if self.format_version != 1 {
            return Err(format!(
                "unsupported syntax-package format {}",
                self.format_version
            ));
        }
        if !valid_language_id(&self.id) {
            return Err("invalid language ID".into());
        }
        if self.display_name.trim().is_empty() {
            return Err("display name cannot be empty".into());
        }
        let package_version = version_core(&self.version)?;
        let minimum_version = version_core(&self.minimum_editur_version)?;
        let current_version = version_core(editur_version)?;
        if package_version == [0, 0, 0] {
            return Err("package version cannot be 0.0.0".into());
        }
        if minimum_version > current_version {
            return Err(format!(
                "package requires Editur {} or newer",
                self.minimum_editur_version
            ));
        }
        if self.extensions.is_empty() && self.filenames.is_empty() {
            return Err("manifest must map at least one extension or filename".into());
        }
        unique(&self.extensions, "extension")?;
        if self.extensions.iter().any(|extension| {
            extension.is_empty()
                || extension.starts_with('.')
                || extension.contains(['/', '\\'])
                || extension.chars().any(char::is_whitespace)
        }) {
            return Err("invalid filename extension".into());
        }
        unique(&self.filenames, "filename")?;
        if self.filenames.iter().any(|filename| {
            filename.is_empty()
                || filename == "."
                || filename == ".."
                || filename.contains(['/', '\\'])
        }) {
            return Err("invalid exact filename".into());
        }
        if self.grammars.is_empty() {
            return Err("manifest must include at least one grammar".into());
        }
        unique(&self.grammars, "grammar")?;
        if self.grammars.iter().any(|grammar| {
            let Some(filename) = grammar.strip_prefix("syntaxes/") else {
                return true;
            };
            filename.is_empty()
                || filename.contains(['/', '\\'])
                || !filename.ends_with(".sublime-syntax")
        }) {
            return Err("grammar paths must match syntaxes/*.sublime-syntax".into());
        }
        unique(&self.dependencies, "dependency")?;
        if self
            .dependencies
            .iter()
            .any(|dependency| !valid_language_id(dependency) || dependency == &self.id)
        {
            return Err("invalid syntax-package dependency".into());
        }
        Ok(())
    }
}

fn unique(values: &[String], label: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    if values.iter().all(|value| seen.insert(value)) {
        Ok(())
    } else {
        Err(format!("duplicate {label} in manifest"))
    }
}

fn version_core(version: &str) -> Result<[u64; 3], String> {
    let core = version
        .split(['-', '+'])
        .next()
        .ok_or_else(|| format!("invalid version `{version}`"))?;
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(format!("invalid version `{version}`"));
    }
    let mut parsed = [0; 3];
    for (index, part) in parts.into_iter().enumerate() {
        parsed[index] = part
            .parse()
            .map_err(|_| format!("invalid version `{version}`"))?;
    }
    Ok(parsed)
}

pub struct PackageManager {
    data_dir: PathBuf,
}

impl PackageManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn package_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("packages").join(id)
    }

    pub fn cache_path(&self) -> PathBuf {
        self.data_dir.join("syntaxes.packdump")
    }

    pub fn install_bytes(&self, bytes: &[u8]) -> Result<Manifest, String> {
        let manifest = inspect_archive(bytes)?;
        manifest.validate(env!("CARGO_PKG_VERSION"))?;
        fs::create_dir_all(self.data_dir.join("packages"))
            .map_err(|error| format!("cannot create syntax-package directory: {error}"))?;
        let destination = self.package_dir(&manifest.id);
        if destination.exists() {
            return Err(format!("syntax `{}` is already installed", manifest.id));
        }
        for dependency in &manifest.dependencies {
            if !self.package_dir(dependency).join("manifest.json").is_file() {
                return Err(format!("syntax `{}` requires `{dependency}`", manifest.id));
            }
        }

        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(self.data_dir.join("packages"))
            .map_err(|error| format!("cannot stage syntax package: {error}"))?;
        extract_archive(bytes, staging.path())?;
        let syntax_set = self.compile(Some(staging.path()), None)?;
        if syntax_set
            .find_syntax_by_name(&manifest.display_name)
            .is_none()
        {
            return Err(format!(
                "package `{}` has no grammar named `{}`",
                manifest.id, manifest.display_name
            ));
        }
        let cache = self.write_cache(&syntax_set)?;

        fs::rename(staging.path(), &destination)
            .map_err(|error| format!("cannot activate syntax `{}`: {error}", manifest.id))?;
        if let Err(error) = cache.persist(self.cache_path()) {
            let rollback = fs::remove_dir_all(&destination);
            return Err(match rollback {
                Ok(()) => format!("cannot replace syntax cache: {}", error.error),
                Err(rollback_error) => format!(
                    "cannot replace syntax cache: {}; rollback failed: {rollback_error}",
                    error.error
                ),
            });
        }
        Ok(manifest)
    }

    pub fn install(&self, source: &OsStr) -> Result<Manifest, String> {
        let path = Path::new(source);
        if path.exists()
            || path
                .extension()
                .is_some_and(|extension| extension == "editur-syntax")
            || path.components().count() > 1
        {
            let bytes = read_limited_file(path)?;
            return self.install_bytes(&bytes);
        }
        let id = source
            .to_str()
            .ok_or_else(|| "language ID must be valid UTF-8".to_owned())?;
        if !valid_language_id(id) {
            return Err(format!("invalid language ID `{id}`"));
        }
        let catalog_url =
            std::env::var("EDITUR_SYNTAX_CATALOG").unwrap_or_else(|_| OFFICIAL_CATALOG.to_owned());
        let catalog = Self::fetch_catalog(&catalog_url)?;
        let package = catalog
            .resolve(id)
            .ok_or_else(|| format!("syntax `{id}` is not in the official catalog"))?;
        let bytes = download(&package.url, MAX_ARCHIVE_SIZE as u64)?;
        verify_checksum(&bytes, &package.sha256)?;
        let manifest = inspect_archive(&bytes)?;
        if manifest.id != package.id || manifest.version != package.version {
            return Err(format!(
                "catalog metadata does not match downloaded package `{id}`"
            ));
        }
        self.install_bytes(&bytes)
    }

    pub fn fetch_catalog(url: &str) -> Result<Catalog, String> {
        if !url.starts_with("https://") {
            return Err("syntax catalog URL must use HTTPS".into());
        }
        Catalog::parse(&download(url, MAX_CATALOG_SIZE)?)
    }

    pub fn installed(&self) -> Result<Vec<Manifest>, String> {
        let packages = self.data_dir.join("packages");
        if !packages.exists() {
            return Ok(Vec::new());
        }
        let mut manifests = Vec::new();
        for entry in fs::read_dir(&packages)
            .map_err(|error| format!("cannot read installed syntaxes: {error}"))?
        {
            let entry = entry.map_err(|error| format!("cannot read installed syntax: {error}"))?;
            let path = entry.path().join("manifest.json");
            if path.is_file() {
                manifests.push(read_manifest(&path)?);
            }
        }
        manifests.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(manifests)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        if !valid_language_id(id) || id == "rust" {
            return Err(format!("syntax `{id}` cannot be removed"));
        }
        let destination = self.package_dir(id);
        if !destination.join("manifest.json").is_file() {
            return Err(format!("syntax `{id}` is not installed"));
        }
        if let Some(dependent) = self.installed()?.into_iter().find(|manifest| {
            manifest
                .dependencies
                .iter()
                .any(|dependency| dependency == id)
        }) {
            return Err(format!("syntax `{id}` is required by `{}`", dependent.id));
        }

        let trash = tempfile::Builder::new()
            .prefix(".remove-")
            .tempdir_in(self.data_dir.join("packages"))
            .map_err(|error| format!("cannot stage syntax removal: {error}"))?;
        let removed = trash.path().join("package");
        fs::rename(&destination, &removed)
            .map_err(|error| format!("cannot remove syntax `{id}`: {error}"))?;

        let rebuild = self.compile(None, None).and_then(|set| {
            let cache = self.write_cache(&set)?;
            cache
                .persist(self.cache_path())
                .map(|_| ())
                .map_err(|error| format!("cannot replace syntax cache: {}", error.error))
        });
        if let Err(error) = rebuild {
            return match fs::rename(&removed, &destination) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; rollback failed: {rollback_error}")),
            };
        }
        Ok(())
    }

    fn compile(
        &self,
        candidate: Option<&Path>,
        exclude: Option<&str>,
    ) -> Result<SyntaxSet, String> {
        let built_in = syntect::dumps::from_reader::<SyntaxSet, _>(super::BUILTIN_DUMP)
            .map_err(|error| format!("cannot load built-in syntaxes: {error}"))?;
        let mut builder = built_in.into_builder();
        let packages = self.data_dir.join("packages");
        if packages.is_dir() {
            for entry in fs::read_dir(&packages)
                .map_err(|error| format!("cannot read installed syntaxes: {error}"))?
            {
                let entry =
                    entry.map_err(|error| format!("cannot read installed syntax: {error}"))?;
                let id = entry.file_name();
                if id.to_str() == exclude || !entry.path().join("manifest.json").is_file() {
                    continue;
                }
                builder
                    .add_from_folder(entry.path(), true)
                    .map_err(|error| format!("cannot compile installed syntax: {error}"))?;
            }
        }
        if let Some(candidate) = candidate {
            builder
                .add_from_folder(candidate, true)
                .map_err(|error| format!("cannot compile syntax package: {error}"))?;
        }
        build_and_check(builder)
    }

    fn write_cache(&self, syntax_set: &SyntaxSet) -> Result<tempfile::NamedTempFile, String> {
        fs::create_dir_all(&self.data_dir)
            .map_err(|error| format!("cannot create syntax data directory: {error}"))?;
        let mut cache = tempfile::Builder::new()
            .prefix(".syntaxes-")
            .tempfile_in(&self.data_dir)
            .map_err(|error| format!("cannot create syntax cache: {error}"))?;
        syntect::dumps::dump_to_writer(syntax_set, &mut cache)
            .map_err(|error| format!("cannot serialize syntax cache: {error}"))?;
        cache
            .flush()
            .and_then(|()| cache.as_file().sync_all())
            .map_err(|error| format!("cannot flush syntax cache: {error}"))?;
        Ok(cache)
    }
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest = Manifest::parse(&bytes)?;
    manifest.validate(env!("CARGO_PKG_VERSION"))?;
    Ok(manifest)
}

fn read_limited_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.len() > MAX_ARCHIVE_SIZE as u64 {
        return Err(format!("syntax package exceeds {MAX_ARCHIVE_SIZE} bytes"));
    }
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
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
    Err("this Editur build has syntax downloads disabled".to_owned())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut result = String::with_capacity(64);
    for byte in digest {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn verify_checksum(bytes: &[u8], advertised: &str) -> Result<(), String> {
    if sha256_hex(bytes).eq_ignore_ascii_case(advertised) {
        Ok(())
    } else {
        Err("syntax-package checksum does not match the catalog".into())
    }
}

fn build_and_check(builder: SyntaxSetBuilder) -> Result<SyntaxSet, String> {
    let set = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| builder.build()))
        .map_err(|_| "syntax linking failed".to_owned())?;
    for syntax in set.syntaxes() {
        let mut parser = ParseState::new(syntax);
        parser
            .parse_line("\n", &set)
            .map_err(|error| format!("syntax `{}` failed to compile: {error}", syntax.name))?;
    }
    Ok(set)
}

fn inspect_archive(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_ARCHIVE_SIZE {
        return Err(format!("syntax package exceeds {} bytes", MAX_ARCHIVE_SIZE));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid syntax-package archive: {error}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!(
            "syntax package contains more than {MAX_ENTRIES} entries"
        ));
    }
    let mut names = HashSet::new();
    let mut unpacked = 0_u64;
    let mut manifest_bytes = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot inspect syntax-package entry: {error}"))?;
        let name = entry.name().to_owned();
        validate_archive_path(&name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("syntax package contains symlink `{name}`"));
        }
        if !names.insert(name.clone()) {
            return Err(format!("syntax package contains duplicate entry `{name}`"));
        }
        unpacked = unpacked
            .checked_add(entry.size())
            .ok_or_else(|| "syntax package size overflow".to_owned())?;
        if unpacked > MAX_UNPACKED_SIZE {
            return Err(format!(
                "syntax package expands beyond {MAX_UNPACKED_SIZE} bytes"
            ));
        }
        if name == "manifest.json" {
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|error| format!("cannot read manifest.json: {error}"))?;
            manifest_bytes = Some(contents);
        }
    }
    let manifest = Manifest::parse(
        manifest_bytes
            .as_deref()
            .ok_or_else(|| "syntax package is missing manifest.json".to_owned())?,
    )?;
    let declared: HashSet<_> = manifest.grammars.iter().map(String::as_str).collect();
    for name in names.iter().filter(|name| !name.ends_with('/')) {
        if name == "manifest.json"
            || declared.contains(name.as_str())
            || name
                .strip_prefix("LICENSES/")
                .is_some_and(|license| !license.is_empty() && !license.contains('/'))
        {
            continue;
        }
        return Err(format!("unexpected syntax-package entry `{name}`"));
    }
    if manifest
        .grammars
        .iter()
        .any(|grammar| !names.contains(grammar))
    {
        return Err("manifest references a missing grammar".into());
    }
    Ok(manifest)
}

fn validate_archive_path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains('\\') {
        return Err(format!("invalid syntax-package path `{name}`"));
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_))
                && !(matches!(component, Component::CurDir) && name == "./")
        })
    {
        return Err(format!("unsafe syntax-package path `{name}`"));
    }
    Ok(path.to_path_buf())
}

fn extract_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("invalid syntax-package archive: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("cannot read syntax-package entry: {error}"))?;
        let relative = validate_archive_path(entry.name())?;
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::File::create(&output)
            .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
        io::copy(&mut entry, &mut file)
            .map_err(|error| format!("cannot extract {}: {error}", output.display()))?;
        file.flush()
            .map_err(|error| format!("cannot flush {}: {error}", output.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn valid_manifest() -> Manifest {
        Manifest {
            format_version: 1,
            id: "python".into(),
            display_name: "Python".into(),
            version: "1.0.0".into(),
            minimum_editur_version: "0.1.0".into(),
            extensions: vec!["py".into(), "pyw".into()],
            filenames: vec![],
            grammars: vec!["syntaxes/Python.sublime-syntax".into()],
            dependencies: vec![],
        }
    }

    #[test]
    fn validates_the_versioned_data_only_manifest_contract() {
        valid_manifest().validate("0.1.0").unwrap();

        let mut invalid = valid_manifest();
        invalid.format_version = 2;
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.id = "Python!".into();
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.display_name.clear();
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.extensions = vec![".py".into()];
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.filenames = vec!["dir/Pipfile".into()];
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.grammars = vec!["../Python.sublime-syntax".into()];
        assert!(invalid.validate("0.1.0").is_err());
        invalid = valid_manifest();
        invalid.minimum_editur_version = "9.0.0".into();
        assert!(invalid.validate("0.1.0").is_err());
    }

    #[test]
    fn rejects_unknown_manifest_fields() {
        let json = br#"{
            "format_version": 1,
            "id": "python",
            "display_name": "Python",
            "version": "1.0.0",
            "minimum_editur_version": "0.1.0",
            "extensions": ["py"],
            "filenames": [],
            "grammars": ["syntaxes/Python.sublime-syntax"],
            "dependencies": [],
            "post_install": "run-me"
        }"#;
        assert!(Manifest::parse(json).is_err());
    }

    fn package(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, contents) in entries {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn python_manifest() -> Vec<u8> {
        serde_json::to_vec(&valid_manifest()).unwrap()
    }

    #[test]
    fn shipped_python_and_markdown_packages_validate_and_compile() {
        let mut builder = syntect::dumps::from_reader::<SyntaxSet, _>(super::super::BUILTIN_DUMP)
            .unwrap()
            .into_builder();
        for id in ["python", "markdown"] {
            let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("syntax-packages")
                .join(id);
            read_manifest(&directory.join("manifest.json")).unwrap();
            builder.add_from_folder(directory, true).unwrap();
        }
        let set = build_and_check(builder).unwrap();
        assert!(set.find_syntax_by_name("Python").is_some());
        assert!(set.find_syntax_by_name("Markdown").is_some());
    }

    const PYTHON_SYNTAX: &[u8] = br#"%YAML 1.2
---
name: Python
file_extensions: [py, pyw]
scope: source.python
contexts:
  main:
    - match: '\b(?:class|def|return)\b'
      scope: keyword.control.python
"#;

    #[test]
    fn installs_a_bounded_data_only_archive_and_rebuilds_the_cache() {
        let temp = tempfile::tempdir().unwrap();
        let manager = PackageManager::new(temp.path().join("data"));
        let manifest = python_manifest();
        let archive = package(&[
            ("manifest.json", &manifest),
            ("syntaxes/Python.sublime-syntax", PYTHON_SYNTAX),
        ]);

        let installed = manager.install_bytes(&archive).unwrap();
        assert_eq!(installed.id, "python");
        assert!(
            manager
                .package_dir("python")
                .join("manifest.json")
                .is_file()
        );
        assert!(manager.cache_path().is_file());
        assert!(manager.install_bytes(&archive).is_err());

        let traversal = package(&[
            ("manifest.json", &manifest),
            ("../escape", b"bad"),
            ("syntaxes/Python.sublime-syntax", PYTHON_SYNTAX),
        ]);
        assert!(manager.install_bytes(&traversal).is_err());
        assert!(!temp.path().join("escape").exists());
    }

    #[test]
    fn lists_and_removes_packages_with_an_atomic_cache_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let manager = PackageManager::new(temp.path().join("data"));
        let manifest = python_manifest();
        manager
            .install_bytes(&package(&[
                ("manifest.json", &manifest),
                ("syntaxes/Python.sublime-syntax", PYTHON_SYNTAX),
            ]))
            .unwrap();

        assert_eq!(manager.installed().unwrap(), vec![valid_manifest()]);
        manager.remove("python").unwrap();
        assert!(manager.installed().unwrap().is_empty());
        assert!(!manager.package_dir("python").exists());
        let set: SyntaxSet = syntect::dumps::from_dump_file(manager.cache_path()).unwrap();
        assert_eq!(set.syntaxes().len(), 2);
        assert!(manager.remove("rust").is_err());
    }

    #[test]
    fn validates_catalog_entries_and_advertised_checksums() {
        let checksum = sha256_hex(b"package");
        let json = format!(
            r#"{{"format_version":1,"packages":[{{"id":"python","display_name":"Python","version":"1.0.0","url":"https://example.com/python.editur-syntax","sha256":"{checksum}"}}]}}"#
        );
        let catalog = Catalog::parse(json.as_bytes()).unwrap();
        assert_eq!(catalog.resolve("python").unwrap().display_name, "Python");
        verify_checksum(b"package", &checksum).unwrap();
        assert!(verify_checksum(b"tampered", &checksum).is_err());

        let insecure = json.replace("https://", "http://");
        assert!(Catalog::parse(insecure.as_bytes()).is_err());
    }
}
