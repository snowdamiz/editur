use std::{
    ffi::OsString,
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

const PREFERENCE_FILE: &str = "agent-provider.json";
const TERMS_FILE: &str = "agent-provider-terms.json";
const MAX_PREFERENCE_BYTES: u64 = 4 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Cursor,
    Codex,
    Claude,
}

impl ProviderId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cursor" => Ok(Self::Cursor),
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            _ => Err(format!("unknown ACP provider `{value}`")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallPolicy {
    Bundled,
    Lazy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderIcon {
    CursorMark,
    Glyph(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderExtensions {
    None,
    Cursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub display_name: &'static str,
    pub icon: ProviderIcon,
    pub description: &'static str,
    pub license_url: &'static str,
    pub terms_url: &'static str,
    pub install_policy: InstallPolicy,
    pub extensions: ProviderExtensions,
    pub unavailable_reason: Option<&'static str>,
}

const CATALOG: [ProviderDescriptor; 3] = [
    ProviderDescriptor {
        id: ProviderId::Cursor,
        display_name: "Cursor",
        icon: ProviderIcon::CursorMark,
        description: "Cursor's ACP coding agent",
        license_url: "https://cursor.com/terms-of-service",
        terms_url: "https://cursor.com/terms-of-service",
        install_policy: InstallPolicy::Bundled,
        extensions: ProviderExtensions::Cursor,
        unavailable_reason: None,
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        display_name: "Codex",
        icon: ProviderIcon::Glyph("◎"),
        description: "OpenAI Codex through the canonical ACP adapter",
        license_url: "https://github.com/agentclientprotocol/codex-acp/blob/v1.1.14/LICENSE",
        terms_url: "https://openai.com/policies/terms-of-use/",
        install_policy: InstallPolicy::Lazy,
        extensions: ProviderExtensions::None,
        unavailable_reason: None,
    },
    ProviderDescriptor {
        id: ProviderId::Claude,
        display_name: "Claude",
        icon: ProviderIcon::Glyph("✦"),
        description: "Anthropic Claude through the canonical ACP adapter",
        license_url: "https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.66.0/LICENSE",
        terms_url: "https://www.anthropic.com/legal/consumer-terms",
        install_policy: InstallPolicy::Lazy,
        extensions: ProviderExtensions::None,
        unavailable_reason: Some("Unavailable: distribution and licensing review incomplete"),
    },
];

pub const fn catalog() -> &'static [ProviderDescriptor] {
    &CATALOG
}

pub fn descriptor(id: ProviderId) -> &'static ProviderDescriptor {
    CATALOG
        .iter()
        .find(|provider| provider.id == id)
        .expect("every ProviderId has a catalog entry")
}

#[derive(Debug, Eq, PartialEq)]
pub struct PreparedAgent {
    pub provider: ProviderId,
    pub display_name: &'static str,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    pub version: String,
    pub extensions: ProviderExtensions,
}

pub fn prepare(
    provider: ProviderId,
    data_dir: &Path,
    progress: impl FnMut(super::provision::DownloadProgress),
) -> Result<PreparedAgent, String> {
    let bundle = super::provision::embedded_bundle()?;
    let installed = super::provision::ensure(bundle.manifest(provider)?, data_dir, progress)?;
    Ok(prepare_installed(provider, installed, data_dir))
}

pub fn prepare_installed(
    provider: ProviderId,
    installed: super::provision::InstalledSidecar,
    data_dir: &Path,
) -> PreparedAgent {
    let metadata = descriptor(provider);
    let mut env = Vec::with_capacity(2);
    if provider == ProviderId::Cursor {
        env.push(("CURSOR_INVOKED_AS".into(), OsString::from("cursor-agent")));
    }
    env.push((
        "NODE_COMPILE_CACHE".into(),
        super::provision::provider_root(data_dir, provider)
            .join("cache")
            .into_os_string(),
    ));
    PreparedAgent {
        provider,
        display_name: metadata.display_name,
        command: installed.command,
        args: installed.args,
        env,
        version: installed.version,
        extensions: metadata.extensions,
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    provider: ProviderId,
}

pub fn load_selected(data_dir: &Path, available: &[ProviderId]) -> ProviderId {
    let selected = read_small_file(&data_dir.join(PREFERENCE_FILE))
        .and_then(|bytes| serde_json::from_slice::<Preference>(&bytes).ok())
        .map(|preference| preference.provider);
    selected
        .filter(|provider| available.contains(provider))
        .unwrap_or(ProviderId::Cursor)
}

pub fn save_selected(data_dir: &Path, provider: ProviderId) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Preference { provider })
        .map_err(|error| format!("cannot encode selected ACP provider: {error}"))?;
    write_atomic(data_dir, PREFERENCE_FILE, &bytes, "selected ACP provider")
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AcceptedTerms {
    providers: Vec<ProviderId>,
}

pub fn terms_accepted(data_dir: &Path, provider: ProviderId) -> bool {
    read_small_file(&data_dir.join(TERMS_FILE))
        .and_then(|bytes| serde_json::from_slice::<AcceptedTerms>(&bytes).ok())
        .is_some_and(|accepted| accepted.providers.contains(&provider))
}

pub fn accept_terms(data_dir: &Path, provider: ProviderId) -> Result<(), String> {
    let mut accepted = read_small_file(&data_dir.join(TERMS_FILE))
        .and_then(|bytes| serde_json::from_slice::<AcceptedTerms>(&bytes).ok())
        .unwrap_or_default();
    if !accepted.providers.contains(&provider) {
        accepted.providers.push(provider);
    }
    let bytes = serde_json::to_vec(&accepted)
        .map_err(|error| format!("cannot encode accepted ACP provider terms: {error}"))?;
    write_atomic(data_dir, TERMS_FILE, &bytes, "accepted ACP provider terms")
}

fn read_small_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_PREFERENCE_BYTES)
        .then(|| fs::read(path).ok())
        .flatten()
}

fn write_atomic(
    data_dir: &Path,
    name: &str,
    bytes: &[u8],
    description: &str,
) -> Result<(), String> {
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("cannot create application data directory: {error}"))?;
    let mut staged = tempfile::NamedTempFile::new_in(data_dir)
        .map_err(|error| format!("cannot stage {description}: {error}"))?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| format!("cannot write {description}: {error}"))?;
    staged
        .persist(data_dir.join(name))
        .map_err(|error| format!("cannot save {description}: {}", error.error))?;
    Ok(())
}
