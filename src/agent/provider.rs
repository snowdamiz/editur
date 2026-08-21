use std::{
    collections::HashSet,
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
const ACCOUNT_FILE: &str = "agent-accounts.json";
const ACCOUNT_REGISTRY_VERSION: u32 = 1;
const MAX_ACCOUNT_FILE_BYTES: u64 = 64 * 1024;
const MAX_ACCOUNTS: usize = 128;
const MAX_ACCOUNT_LABEL_BYTES: usize = 64;
const MAX_ENVIRONMENT_NAME_BYTES: usize = 128;
pub const MAX_ACCOUNT_ID: u64 = i64::MAX as u64;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountKey {
    pub provider: ProviderId,
    pub account_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    Legacy,
    ProviderManaged,
    Environment { variable: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAccount {
    pub key: AccountKey,
    pub label: String,
    pub auth_source: AuthSource,
    pub auto_failover: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountRegistry {
    pub version: u32,
    pub selected: AccountKey,
    pub accounts: Vec<ProviderAccount>,
}

impl Default for AccountRegistry {
    fn default() -> Self {
        let selected = AccountKey {
            provider: ProviderId::Cursor,
            account_id: 1,
        };
        Self {
            version: ACCOUNT_REGISTRY_VERSION,
            selected,
            accounts: vec![ProviderAccount {
                key: selected,
                label: "Current login".into(),
                auth_source: AuthSource::Legacy,
                auto_failover: false,
            }],
        }
    }
}

impl AccountRegistry {
    pub fn account(&self, key: AccountKey) -> Option<&ProviderAccount> {
        self.accounts.iter().find(|account| account.key == key)
    }

    pub fn accounts_for(&self, provider: ProviderId) -> impl Iterator<Item = &ProviderAccount> {
        self.accounts
            .iter()
            .filter(move |account| account.key.provider == provider)
    }

    pub fn next_account_id(&self) -> Result<u64, String> {
        self.accounts
            .iter()
            .map(|account| account.key.account_id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|id| *id <= MAX_ACCOUNT_ID)
            .ok_or_else(|| "ACP account id limit reached".to_owned())
    }

    pub fn add_account(
        &mut self,
        provider: ProviderId,
        label: String,
        auth_source: AuthSource,
    ) -> Result<AccountKey, String> {
        let key = AccountKey {
            provider,
            account_id: self.next_account_id()?,
        };
        let account = ProviderAccount {
            key,
            label,
            auth_source,
            auto_failover: false,
        };
        validate_account(&account)?;
        if self.accounts.len() >= MAX_ACCOUNTS {
            return Err(format!("configure at most {MAX_ACCOUNTS} ACP accounts"));
        }
        self.accounts.push(account);
        Ok(key)
    }

    pub fn select(&mut self, key: AccountKey) -> Result<(), String> {
        self.account(key)
            .ok_or_else(|| "selected ACP account does not exist".to_owned())?;
        self.selected = key;
        Ok(())
    }

    pub fn move_account(&mut self, key: AccountKey, offset: isize) -> bool {
        let positions = self
            .accounts
            .iter()
            .enumerate()
            .filter_map(|(index, account)| (account.key.provider == key.provider).then_some(index))
            .collect::<Vec<_>>();
        let Some(position) = positions
            .iter()
            .position(|index| self.accounts[*index].key == key)
        else {
            return false;
        };
        let target = position
            .saturating_add_signed(offset)
            .min(positions.len() - 1);
        if target == position {
            return false;
        }
        self.accounts.swap(positions[position], positions[target]);
        true
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
    if raw.is_empty() {
        return raw;
    }
    if provider != ProviderId::Cursor {
        format!(
            "{} stderr suppressed to protect authentication and protocol data.",
            descriptor(provider).display_name
        )
    } else {
        raw.lines()
            .map(|line| {
                if [
                    "CURSOR_API_KEY",
                    "CURSOR_AUTH_TOKEN",
                    "CODEX_API_KEY",
                    "OPENAI_API_KEY",
                    "ANTHROPIC_API_KEY",
                    "Authorization:",
                    "Bearer ",
                ]
                .iter()
                .any(|marker| line.contains(marker))
                {
                    "[credential diagnostic redacted]"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Eq, PartialEq)]
pub struct PreparedAgent {
    pub provider: ProviderId,
    pub account: AccountKey,
    pub display_name: &'static str,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    pub remove_env: Vec<String>,
    pub version: String,
    pub extensions: ProviderExtensions,
}

impl fmt::Debug for PreparedAgent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAgent")
            .field("provider", &self.provider)
            .field("account", &self.account)
            .field("display_name", &self.display_name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("remove_env", &self.remove_env)
            .field("version", &self.version)
            .field("extensions", &self.extensions)
            .finish()
    }
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

pub fn prepare_account(
    account: &ProviderAccount,
    data_dir: &Path,
    progress: impl FnMut(super::provision::DownloadProgress),
) -> Result<PreparedAgent, String> {
    validate_account(account)?;
    let bundle = super::provision::embedded_bundle()?;
    let installed =
        super::provision::ensure(bundle.manifest(account.key.provider)?, data_dir, progress)?;
    prepare_installed_for_account(account, installed, data_dir)
}

pub fn prepare_installed(
    provider: ProviderId,
    installed: super::provision::InstalledSidecar,
    data_dir: &Path,
) -> PreparedAgent {
    let account = ProviderAccount {
        key: AccountKey {
            provider,
            account_id: 1,
        },
        label: "Current login".into(),
        auth_source: AuthSource::Legacy,
        auto_failover: false,
    };
    prepare_installed_for_account(&account, installed, data_dir)
        .expect("synthetic legacy account is valid")
}

pub fn prepare_installed_for_account(
    account: &ProviderAccount,
    mut installed: super::provision::InstalledSidecar,
    data_dir: &Path,
) -> Result<PreparedAgent, String> {
    validate_account(account)?;
    let provider = account.key.provider;
    let metadata = descriptor(provider);
    let mut env = Vec::with_capacity(4);
    let mut remove_env = removed_environment(provider)
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    if provider == ProviderId::Cursor {
        env.push(("CURSOR_INVOKED_AS".into(), OsString::from("cursor-agent")));
    }
    env.push((
        "NODE_COMPILE_CACHE".into(),
        super::provision::provider_root(data_dir, provider)
            .join("cache")
            .into_os_string(),
    ));
    match &account.auth_source {
        AuthSource::Legacy => {}
        AuthSource::ProviderManaged => {
            let provider_data =
                super::provision::account_provider_data_root(data_dir, account.key)?;
            fs::create_dir_all(&provider_data).map_err(|error| {
                format!(
                    "cannot create isolated {} account data: {error}",
                    metadata.display_name
                )
            })?;
            match provider {
                ProviderId::Cursor => {
                    // The stock CLI keeps one browser sign-in per machine
                    // (keychain or a hard-coded home path), so isolated
                    // accounts run an Editur-patched entrypoint whose
                    // credential file follows EDITUR_CURSOR_AUTH_FILE and
                    // live entirely inside the account's provider-data.
                    installed = super::provision::cursor_isolated_sidecar(&installed)?;
                    let config = provider_data.join("config");
                    let data = provider_data.join("data");
                    for directory in [&config, &data] {
                        fs::create_dir_all(directory).map_err(|error| {
                            format!("cannot create isolated Cursor account data: {error}")
                        })?;
                    }
                    env.push(("CURSOR_CONFIG_DIR".into(), config.into_os_string()));
                    env.push(("CURSOR_DATA_DIR".into(), data.into_os_string()));
                    env.push(("AGENT_CLI_CREDENTIAL_STORE".into(), OsString::from("file")));
                    env.push((
                        super::provision::CURSOR_AUTH_FILE_VARIABLE.into(),
                        provider_data.join("auth.json").into_os_string(),
                    ));
                    remove_env.extend(["CURSOR_API_KEY".into(), "CURSOR_AUTH_TOKEN".into()]);
                }
                ProviderId::Codex => {
                    let config = provider_data.join("config.toml");
                    if !config.exists() {
                        write_atomic(
                            &provider_data,
                            "config.toml",
                            b"cli_auth_credentials_store = \"file\"\n",
                            "isolated Codex configuration",
                        )?;
                    }
                    env.push(("CODEX_HOME".into(), provider_data.into_os_string()));
                    remove_env.extend(["CODEX_API_KEY".into(), "OPENAI_API_KEY".into()]);
                }
                ProviderId::Claude => {
                    env.push(("CLAUDE_CONFIG_DIR".into(), provider_data.into_os_string()));
                    remove_env.push("ANTHROPIC_API_KEY".into());
                }
            }
        }
        AuthSource::Environment { variable } => {
            let value = std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("environment variable {variable} is not set"))?;
            remove_env.push(variable.clone());
            match provider {
                ProviderId::Cursor => {
                    remove_env.extend(["CURSOR_API_KEY".into(), "CURSOR_AUTH_TOKEN".into()]);
                    let target = if variable.ends_with("_AUTH_TOKEN") {
                        "CURSOR_AUTH_TOKEN"
                    } else {
                        "CURSOR_API_KEY"
                    };
                    env.push((target.into(), value));
                }
                ProviderId::Codex => {
                    let provider_data =
                        super::provision::account_provider_data_root(data_dir, account.key)?;
                    fs::create_dir_all(&provider_data).map_err(|error| {
                        format!("cannot create isolated Codex account data: {error}")
                    })?;
                    if !provider_data.join("config.toml").exists() {
                        write_atomic(
                            &provider_data,
                            "config.toml",
                            b"cli_auth_credentials_store = \"file\"\n",
                            "isolated Codex configuration",
                        )?;
                    }
                    remove_env.extend(["CODEX_API_KEY".into(), "OPENAI_API_KEY".into()]);
                    env.push(("CODEX_HOME".into(), provider_data.into_os_string()));
                    env.push(("CODEX_API_KEY".into(), value));
                }
                ProviderId::Claude => {
                    let provider_data =
                        super::provision::account_provider_data_root(data_dir, account.key)?;
                    fs::create_dir_all(&provider_data).map_err(|error| {
                        format!("cannot create isolated Claude account data: {error}")
                    })?;
                    remove_env.push("ANTHROPIC_API_KEY".into());
                    env.push(("CLAUDE_CONFIG_DIR".into(), provider_data.into_os_string()));
                    env.push(("ANTHROPIC_API_KEY".into(), value));
                }
            }
        }
    }
    Ok(PreparedAgent {
        provider,
        account: account.key,
        display_name: metadata.display_name,
        command: installed.command,
        args: installed.args,
        env,
        remove_env,
        version: installed.version,
        extensions: metadata.extensions,
    })
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

pub fn load_accounts(data_dir: &Path, available: &[ProviderId]) -> Result<AccountRegistry, String> {
    let path = data_dir.join(ACCOUNT_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_ACCOUNT_FILE_BYTES {
                return Err("ACP account registry is not a bounded regular file".into());
            }
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot read ACP account registry: {error}"))?;
            let mut registry = serde_json::from_slice::<AccountRegistry>(&bytes)
                .map_err(|error| format!("cannot decode ACP account registry: {error}"))?;
            validate_registry(&registry)?;
            let mut changed = false;
            for provider in available {
                if registry.accounts_for(*provider).next().is_none() {
                    registry.add_account(*provider, "Current login".into(), AuthSource::Legacy)?;
                    changed = true;
                }
            }
            if changed {
                save_accounts(data_dir, &registry)?;
            }
            Ok(registry)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let providers = if available.is_empty() {
                &[ProviderId::Cursor][..]
            } else {
                available
            };
            let accounts = providers
                .iter()
                .copied()
                .enumerate()
                .map(|(index, provider)| ProviderAccount {
                    key: AccountKey {
                        provider,
                        account_id: index as u64 + 1,
                    },
                    label: "Current login".into(),
                    auth_source: AuthSource::Legacy,
                    auto_failover: false,
                })
                .collect::<Vec<_>>();
            let preferred = load_selected(data_dir, providers);
            let selected = accounts
                .iter()
                .find(|account| account.key.provider == preferred)
                .unwrap_or(&accounts[0])
                .key;
            let registry = AccountRegistry {
                version: ACCOUNT_REGISTRY_VERSION,
                selected,
                accounts,
            };
            save_accounts(data_dir, &registry)?;
            Ok(registry)
        }
        Err(error) => Err(format!("cannot inspect ACP account registry: {error}")),
    }
}

pub fn save_accounts(data_dir: &Path, registry: &AccountRegistry) -> Result<(), String> {
    validate_registry(registry)?;
    let bytes = serde_json::to_vec(registry)
        .map_err(|error| format!("cannot encode ACP account registry: {error}"))?;
    if bytes.len() as u64 > MAX_ACCOUNT_FILE_BYTES {
        return Err("ACP account registry exceeds its size limit".into());
    }
    write_atomic(data_dir, ACCOUNT_FILE, &bytes, "ACP account registry")
}

fn validate_registry(registry: &AccountRegistry) -> Result<(), String> {
    if registry.version != ACCOUNT_REGISTRY_VERSION {
        return Err(format!(
            "unsupported ACP account registry version {}",
            registry.version
        ));
    }
    if registry.accounts.is_empty() || registry.accounts.len() > MAX_ACCOUNTS {
        return Err(format!(
            "configure between 1 and {MAX_ACCOUNTS} ACP accounts"
        ));
    }
    let mut keys = HashSet::with_capacity(registry.accounts.len());
    let mut legacy = HashSet::new();
    for account in &registry.accounts {
        validate_account(account)?;
        if !keys.insert(account.key) {
            return Err("ACP account ids must be unique per provider".into());
        }
        if account.auth_source == AuthSource::Legacy && !legacy.insert(account.key.provider) {
            return Err("each ACP provider can have only one legacy account".into());
        }
    }
    if !keys.contains(&registry.selected) {
        return Err("selected ACP account does not exist".into());
    }
    Ok(())
}

fn validate_account(account: &ProviderAccount) -> Result<(), String> {
    if account.key.account_id == 0 || account.key.account_id > MAX_ACCOUNT_ID {
        return Err("ACP account id is outside the supported range".into());
    }
    let label = account.label.trim();
    if label.is_empty()
        || label.len() > MAX_ACCOUNT_LABEL_BYTES
        || label.chars().any(char::is_control)
    {
        return Err(format!(
            "ACP account labels must be 1-{MAX_ACCOUNT_LABEL_BYTES} printable bytes"
        ));
    }
    if let AuthSource::Environment { variable } = &account.auth_source
        && !valid_environment_name(variable)
    {
        return Err("ACP account environment variable name is invalid".into());
    }
    Ok(())
}

pub(crate) fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && name.len() <= MAX_ENVIRONMENT_NAME_BYTES
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
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
