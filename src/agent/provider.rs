use std::{
    ffi::OsString,
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
    str::FromStr,
};

use agent_client_protocol::schema::v1::AuthMethod;
use serde::{Deserialize, Serialize};

const PREFERENCE_FILE: &str = "agent-provider.json";
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
    OpenAiMark,
    AnthropicMark,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderExtensions {
    None,
    Cursor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: AuthKind,
    pub setup: Option<String>,
    pub can_authenticate: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    Agent,
    Terminal,
    Environment,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub display_name: &'static str,
    pub icon: ProviderIcon,
    pub description: &'static str,
    pub license_url: &'static str,
    pub terms_url: &'static str,
    pub terms_notice: &'static str,
    pub legal_links: &'static [ProviderLink],
    pub install_policy: InstallPolicy,
    pub extensions: ProviderExtensions,
    pub unavailable_reason: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderLink {
    pub label: &'static str,
    pub url: &'static str,
}

const CLAUDE_LEGAL_LINKS: &[ProviderLink] = &[
    ProviderLink {
        label: "Legal and credential guidance",
        url: "https://code.claude.com/docs/en/legal-and-compliance",
    },
    ProviderLink {
        label: "Data usage policy",
        url: "https://code.claude.com/docs/en/data-usage",
    },
    ProviderLink {
        label: "Privacy policy",
        url: "https://www.anthropic.com/legal/privacy",
    },
];

const CATALOG: [ProviderDescriptor; 3] = [
    ProviderDescriptor {
        id: ProviderId::Cursor,
        display_name: "Cursor",
        icon: ProviderIcon::CursorMark,
        description: "Cursor's ACP coding agent",
        license_url: "https://cursor.com/terms-of-service",
        terms_url: "https://cursor.com/terms-of-service",
        terms_notice: "Editur will download and run a pinned Cursor package in Editur's private application-data directory.",
        legal_links: &[],
        install_policy: InstallPolicy::Bundled,
        extensions: ProviderExtensions::Cursor,
        unavailable_reason: None,
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        display_name: "Codex",
        icon: ProviderIcon::OpenAiMark,
        description: "OpenAI Codex through the canonical ACP adapter",
        license_url: "https://github.com/agentclientprotocol/codex-acp/blob/v1.1.14/LICENSE",
        terms_url: "https://openai.com/policies/terms-of-use/",
        terms_notice: "Editur will download and run a pinned Codex package in Editur's private application-data directory.",
        legal_links: &[],
        install_policy: InstallPolicy::Lazy,
        extensions: ProviderExtensions::None,
        unavailable_reason: None,
    },
    ProviderDescriptor {
        id: ProviderId::Claude,
        display_name: "Claude",
        icon: ProviderIcon::AnthropicMark,
        description: "Anthropic Claude through the canonical ACP adapter",
        license_url: "https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.66.0/LICENSE",
        terms_url: "https://www.anthropic.com/legal/commercial-terms",
        terms_notice: "Editur will download and run a pinned Claude package in Editur's private application-data directory. The canonical Apache-2.0 adapter is bundled with Anthropic's proprietary Claude Agent SDK and native binary, which are governed by Anthropic's commercial terms.",
        legal_links: CLAUDE_LEGAL_LINKS,
        install_policy: InstallPolicy::Lazy,
        extensions: ProviderExtensions::None,
        unavailable_reason: None,
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

pub(crate) fn normalize_auth_methods(
    provider: ProviderId,
    methods: &[AuthMethod],
) -> Vec<AuthChoice> {
    methods
        .iter()
        .take(128)
        .map(|method| {
            let codex_api_key =
                provider == ProviderId::Codex && method.id().0.as_ref() == "api-key";
            let (kind, setup, can_authenticate) = match method {
                AuthMethod::Agent(_) if codex_api_key => {
                    let names = ["CODEX_API_KEY", "OPENAI_API_KEY"];
                    (
                        AuthKind::Environment,
                        Some(format!("variables: {}", names.join(", "))),
                        names.iter().any(|name| {
                            std::env::var_os(name).is_some_and(|value| !value.is_empty())
                        }),
                    )
                }
                AuthMethod::Agent(_) => (AuthKind::Agent, None, true),
                AuthMethod::Terminal(terminal) => {
                    let mut details = Vec::new();
                    if !terminal.args.is_empty() {
                        details.push(format!("arguments: {}", terminal.args.join(" ")));
                    }
                    if !terminal.env.is_empty() {
                        let mut names = terminal.env.keys().cloned().collect::<Vec<_>>();
                        names.sort();
                        details.push(format!("environment: {}", names.join(", ")));
                    }
                    (
                        AuthKind::Terminal,
                        (!details.is_empty()).then(|| details.join("; ")),
                        provider == ProviderId::Claude && terminal.env.is_empty(),
                    )
                }
                AuthMethod::EnvVar(environment) => {
                    let names = environment
                        .vars
                        .iter()
                        .map(|variable| variable.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    (
                        AuthKind::Environment,
                        (!names.is_empty()).then(|| format!("variables: {names}")),
                        false,
                    )
                }
                _ => (AuthKind::Unsupported, None, false),
            };
            AuthChoice {
                id: method.id().0.to_string(),
                name: method.name().to_owned(),
                description: method.description().map(str::to_owned),
                kind,
                setup,
                can_authenticate,
            }
        })
        .collect()
}

pub(crate) fn authentication_required_choices(
    provider: ProviderId,
    choices: Vec<AuthChoice>,
) -> Vec<AuthChoice> {
    if provider != ProviderId::Claude || !choices.is_empty() {
        return choices;
    }
    vec![AuthChoice {
        id: "anthropic-environment".into(),
        name: "Anthropic API or commercial cloud credentials".into(),
        description: Some(
            "Configure credentials outside Editur, then retry the Claude provider.".into(),
        ),
        kind: AuthKind::Environment,
        setup: Some(
            "variables: ANTHROPIC_API_KEY or supported commercial cloud credentials".into(),
        ),
        can_authenticate: false,
    }]
}

pub(crate) fn visible_diagnostics(provider: ProviderId, raw: String) -> String {
    if provider == ProviderId::Cursor || raw.is_empty() {
        raw
    } else {
        format!(
            "{} stderr suppressed to protect authentication and protocol data.",
            descriptor(provider).display_name
        )
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PreparedAgent {
    pub provider: ProviderId,
    pub display_name: &'static str,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    pub remove_env: Vec<&'static str>,
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
        remove_env: removed_environment(provider).to_vec(),
        version: installed.version,
        extensions: metadata.extensions,
    }
}

pub(crate) const fn removed_environment(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Cursor => &[],
        ProviderId::Codex => &["APP_SERVER_LOGS", "CODEX_PATH", "NODE_OPTIONS", "NODE_PATH"],
        ProviderId::Claude => &[
            "CLAUDE_CODE_EXECUTABLE",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "NODE_OPTIONS",
            "NODE_PATH",
        ],
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
