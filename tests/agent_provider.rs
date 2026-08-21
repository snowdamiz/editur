use editur::agent::provider::{
    AccountKey, AuthSource, InstallPolicy, ProviderAccount, ProviderExtensions, ProviderIcon,
    ProviderId, catalog, load_accounts, load_selected, prepare, prepare_installed,
    prepare_installed_for_account, save_accounts, save_selected,
};
use editur::agent::provision::{
    InstalledSidecar, ProviderBundle, account_provider_data_root, account_root, provider_root,
};

#[test]
fn static_catalog_is_the_complete_provider_identity_source() {
    let providers = catalog();

    assert_eq!(
        providers
            .iter()
            .map(|provider| (provider.id, provider.display_name, provider.install_policy))
            .collect::<Vec<_>>(),
        [
            (ProviderId::Cursor, "Cursor", InstallPolicy::Bundled),
            (ProviderId::Codex, "Codex", InstallPolicy::Lazy),
            (ProviderId::Claude, "Claude", InstallPolicy::Lazy),
        ]
    );
    assert_eq!(providers[0].extensions, ProviderExtensions::Cursor);
    assert_eq!(providers[0].icon, ProviderIcon::CursorMark);
    assert_eq!(providers[1].icon, ProviderIcon::OpenAiMark);
    assert_eq!(providers[2].icon, ProviderIcon::AnthropicMark);
    assert!(
        providers[1..]
            .iter()
            .all(|provider| provider.extensions == ProviderExtensions::None)
    );
}

#[test]
fn claude_catalog_uses_the_audited_commercial_identity() {
    let claude = catalog()
        .iter()
        .find(|provider| provider.id == ProviderId::Claude)
        .unwrap();

    assert_eq!(claude.display_name, "Claude");
    assert!(!claude.description.contains("Claude Code"));
    assert_eq!(
        claude.license_url,
        "https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.66.0/LICENSE"
    );
    assert_eq!(
        claude.terms_url,
        "https://www.anthropic.com/legal/commercial-terms"
    );
    assert_eq!(claude.extensions, ProviderExtensions::None);
    assert_eq!(claude.unavailable_reason, None);
    assert!(claude.terms_notice.contains("Apache-2.0 adapter"));
    assert!(claude.terms_notice.contains("proprietary"));
    assert_eq!(
        claude
            .legal_links
            .iter()
            .map(|link| link.url)
            .collect::<Vec<_>>(),
        [
            "https://code.claude.com/docs/en/legal-and-compliance",
            "https://code.claude.com/docs/en/data-usage",
            "https://www.anthropic.com/legal/privacy",
        ]
    );
}

#[test]
fn provider_ids_round_trip_only_stable_persisted_keys() {
    for (id, key) in [
        (ProviderId::Cursor, "cursor"),
        (ProviderId::Codex, "codex"),
        (ProviderId::Claude, "claude"),
    ] {
        assert_eq!(id.as_str(), key);
        assert_eq!(key.parse(), Ok(id));
    }
    assert!("future-provider".parse::<ProviderId>().is_err());
}

#[test]
fn selected_provider_defaults_and_falls_back_to_available_cursor() {
    let data = tempfile::tempdir().unwrap();
    let available = [ProviderId::Cursor, ProviderId::Codex];

    assert_eq!(load_selected(data.path(), &available), ProviderId::Cursor);
    save_selected(data.path(), ProviderId::Codex).unwrap();
    assert_eq!(load_selected(data.path(), &available), ProviderId::Codex);
    assert_eq!(
        load_selected(data.path(), &[ProviderId::Cursor]),
        ProviderId::Cursor
    );

    std::fs::write(
        data.path().join("agent-provider.json"),
        br#"{"provider":"not-real"}"#,
    )
    .unwrap();
    assert_eq!(load_selected(data.path(), &available), ProviderId::Cursor);
}

#[test]
fn provider_preference_is_one_small_atomic_file() {
    let data = tempfile::tempdir().unwrap();

    save_selected(data.path(), ProviderId::Claude).unwrap();

    assert_eq!(
        std::fs::read_to_string(data.path().join("agent-provider.json")).unwrap(),
        "{\"provider\":\"claude\"}"
    );
    assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 1);
}

#[test]
fn account_registry_migrates_selected_provider_to_legacy_accounts() {
    let data = tempfile::tempdir().unwrap();
    save_selected(data.path(), ProviderId::Claude).unwrap();

    let registry = load_accounts(
        data.path(),
        &[ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude],
    )
    .unwrap();

    assert_eq!(registry.version, 1);
    assert_eq!(registry.accounts.len(), 3);
    assert_eq!(registry.selected.provider, ProviderId::Claude);
    assert!(registry.accounts.iter().all(|account| {
        account.label == "Current login"
            && account.auth_source == AuthSource::Legacy
            && !account.auto_failover
    }));
    assert!(data.path().join("agent-accounts.json").is_file());

    let reloaded = load_accounts(data.path(), &[ProviderId::Cursor, ProviderId::Codex]).unwrap();
    assert_eq!(reloaded, registry);
}

#[test]
fn account_registry_adds_legacy_accounts_for_providers_discovered_later() {
    let data = tempfile::tempdir().unwrap();
    let initial = load_accounts(data.path(), &[ProviderId::Cursor]).unwrap();
    assert_eq!(initial.accounts.len(), 1);

    let migrated = load_accounts(
        data.path(),
        &[ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude],
    )
    .unwrap();

    assert_eq!(migrated.selected, initial.selected);
    assert_eq!(migrated.accounts.len(), 3);
    for provider in [ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude] {
        let accounts = migrated.accounts_for(provider).collect::<Vec<_>>();
        assert_eq!(accounts.len(), 1, "missing {provider} legacy account");
        assert_eq!(accounts[0].auth_source, AuthSource::Legacy);
    }
    assert_eq!(
        load_accounts(data.path(), &[ProviderId::Cursor]).unwrap(),
        migrated,
        "the migration must be persisted atomically"
    );
}

#[test]
fn account_registry_is_bounded_versioned_and_validated() {
    let data = tempfile::tempdir().unwrap();
    let mut registry = load_accounts(data.path(), &[ProviderId::Codex]).unwrap();
    let account = ProviderAccount {
        key: AccountKey {
            provider: ProviderId::Codex,
            account_id: 2,
        },
        label: "Work".into(),
        auth_source: AuthSource::Environment {
            variable: "EDITUR_CODEX_WORK_KEY".into(),
        },
        auto_failover: true,
    };
    registry.accounts.push(account.clone());
    registry.selected = account.key;
    save_accounts(data.path(), &registry).unwrap();

    let json = std::fs::read_to_string(data.path().join("agent-accounts.json")).unwrap();
    assert!(json.contains("EDITUR_CODEX_WORK_KEY"));
    assert!(!json.contains("credential_value"));
    assert_eq!(
        load_accounts(data.path(), &[ProviderId::Codex]).unwrap(),
        registry
    );

    let invalid = [
        json.replace("\"version\":1", "\"version\":2"),
        json.replace("\"Work\"", &format!("\"{}\"", "x".repeat(65))),
        json.replace("EDITUR_CODEX_WORK_KEY", "NOT-AN-ENV-VAR"),
        json.replacen("\"account_id\":2", "\"account_id\":0", 1),
        json.replacen("{", "{\"unknown\":true,", 1),
    ];
    for contents in invalid {
        std::fs::write(data.path().join("agent-accounts.json"), contents).unwrap();
        assert!(load_accounts(data.path(), &[ProviderId::Codex]).is_err());
    }

    std::fs::write(
        data.path().join("agent-accounts.json"),
        vec![b'x'; 65 * 1024],
    )
    .unwrap();
    assert!(load_accounts(data.path(), &[ProviderId::Codex]).is_err());
}

#[test]
fn added_accounts_stay_in_provider_failover_order() {
    let data = tempfile::tempdir().unwrap();
    let mut registry = load_accounts(
        data.path(),
        &[ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude],
    )
    .unwrap();

    let first = registry
        .add_account(
            ProviderId::Codex,
            "Work".into(),
            AuthSource::ProviderManaged,
        )
        .unwrap();
    let second = registry
        .add_account(
            ProviderId::Codex,
            "Personal".into(),
            AuthSource::ProviderManaged,
        )
        .unwrap();

    assert_eq!(
        registry
            .accounts_for(ProviderId::Codex)
            .map(|account| account.key)
            .collect::<Vec<_>>(),
        vec![
            AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            first,
            second,
        ]
    );
    assert!(registry.move_account(second, -1));
    assert_eq!(
        registry
            .accounts_for(ProviderId::Codex)
            .map(|account| account.key)
            .collect::<Vec<_>>(),
        vec![
            AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            second,
            first,
        ]
    );
    assert!(registry.move_account(second, -1));
    assert_eq!(
        registry
            .accounts_for(ProviderId::Codex)
            .map(|account| account.key)
            .collect::<Vec<_>>(),
        vec![
            second,
            AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            first,
        ]
    );
}

#[test]
fn account_roots_are_numeric_distinct_and_beneath_the_provider() {
    let data = std::path::Path::new("/application-data");
    let first = AccountKey {
        provider: ProviderId::Codex,
        account_id: 1,
    };
    let second = AccountKey {
        provider: ProviderId::Codex,
        account_id: 2,
    };

    assert_eq!(
        account_root(data, first).unwrap(),
        data.join("agents/codex/accounts/1")
    );
    assert_ne!(
        account_root(data, first).unwrap(),
        account_root(data, second).unwrap()
    );
    assert_eq!(
        account_provider_data_root(data, second).unwrap(),
        data.join("agents/codex/accounts/2/provider-data")
    );
    assert!(
        account_root(
            data,
            AccountKey {
                provider: ProviderId::Codex,
                account_id: 0,
            }
        )
        .is_err()
    );
}

#[test]
fn optional_provider_preparation_has_no_local_agreement_gate() {
    let data = tempfile::tempdir().unwrap();

    for provider in [ProviderId::Codex, ProviderId::Claude] {
        let error = prepare(provider, data.path(), |_| {}).unwrap_err();
        assert!(!error.to_ascii_lowercase().contains("terms"), "{error}");
        assert!(!provider_root(data.path(), provider).exists());
    }
}

fn manifest(provider: &str) -> serde_json::Value {
    let (command, entrypoint, args, archive_url) = if provider == "cursor" {
        if cfg!(windows) {
            (
                "dist-package/node.exe",
                Some("dist-package/index.js"),
                vec!["--disable-auto-update", "acp"],
                "https://downloads.cursor.com/lab/pinned/agent.zip",
            )
        } else {
            (
                "dist-package/cursor-agent",
                None,
                vec!["--disable-auto-update", "acp"],
                "https://downloads.cursor.com/lab/pinned/agent.zip",
            )
        }
    } else {
        (
            if cfg!(windows) {
                "runtime/node.exe"
            } else {
                "runtime/bin/node"
            },
            Some("package/dist/index.js"),
            Vec::new(),
            "https://github.com/snowdamiz/editur/releases/download/provider-v1/codex.zip",
        )
    };
    let mut entries = vec![serde_json::json!({
        "path": command,
        "kind": "file",
        "size": 1,
        "sha256": "00".repeat(32),
        "executable": true
    })];
    if let Some(entrypoint) = entrypoint {
        entries.push(serde_json::json!({
            "path": entrypoint,
            "kind": "file",
            "size": 1,
            "sha256": "11".repeat(32),
            "executable": false
        }));
    }
    if provider == "codex" {
        entries.push(serde_json::json!({
            "path": "package/node_modules/@openai/codex/bin/codex.js",
            "kind": "file",
            "size": 1,
            "sha256": "33".repeat(32),
            "executable": false
        }));
    }
    let version_probes = if provider == "codex" {
        serde_json::json!([
            {
                "command": command,
                "args": ["package/dist/index.js", "--version"],
                "expected": "@agentclientprotocol/codex-acp 1.1.14"
            },
            {
                "command": command,
                "args": ["package/node_modules/@openai/codex/bin/codex.js", "--version"],
                "expected": "codex-cli 0.147.0"
            }
        ])
    } else {
        serde_json::json!([])
    };
    serde_json::json!({
        "format_version": 1,
        "agent": provider,
        "version": if provider == "cursor" { "2026.07.23-e383d2b" } else { "1.1.14" },
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "archive_url": archive_url,
        "archive_sha256": "22".repeat(32),
        "archive_format": "zip",
        "archive_size_bytes": 2,
        "max_compressed_bytes": 2,
        "max_extracted_bytes": entries.len(),
        "max_entries": entries.len(),
        "command": command,
        "entrypoint": entrypoint,
        "args": args,
        "version_probes": version_probes,
        "entries": entries,
        "license_url": if provider == "cursor" { "https://cursor.com/terms-of-service" } else { "https://github.com/agentclientprotocol/codex-acp/blob/v1.1.14/LICENSE" },
        "terms_url": if provider == "cursor" { "https://cursor.com/terms-of-service" } else { "https://openai.com/policies/terms-of-use/" }
    })
}

fn claude_native_package() -> (&'static str, &'static str) {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => (
            "@anthropic-ai/claude-agent-sdk-darwin-arm64",
            "sha512-7VxlbEosK7DODiOnsjoVd0DSJzbnaPrM2jelMHI0y8zx1UnLS3WC6EFUXbvy74F2sXqEznh2tzn7EKWInaRN6Q==",
        ),
        ("macos", "x86_64") => (
            "@anthropic-ai/claude-agent-sdk-darwin-x64",
            "sha512-X9RwDsSmbF6ultKZroaip+DL8WRgC64gHbrAwrRlAFSPNZV7zmJyP2ur8rW7KrxqmtuehdMMkw8+SAC/6hD2PA==",
        ),
        ("linux", "x86_64") => (
            "@anthropic-ai/claude-agent-sdk-linux-x64",
            "sha512-tkTJFnpR9VifvWX2fmkCAPkT6+8Wk/gVu8B5jsVekKZPiZoWRHmMXO30BnZn+f0TZhgYP+82PSX3S8crH1kn+w==",
        ),
        ("windows", "x86_64") => (
            "@anthropic-ai/claude-agent-sdk-win32-x64",
            "sha512-MuOuXhbr66HlGaWXD2f3w0k2PsvmnbkwcUZ0dAe2poFLdl72GC2dapwwOBefxm9QmoNqk9+jmv/dSKGOVWyvLw==",
        ),
        target => panic!("unsupported Claude test target: {target:?}"),
    }
}

fn claude_manifest() -> serde_json::Value {
    let mut claude = manifest("codex");
    let command = claude["command"].as_str().unwrap().to_owned();
    let (native_package, native_integrity) = claude_native_package();
    let native_path = format!(
        "package/node_modules/{native_package}/package.json",
        native_package = native_package
    );
    let native_binary = format!(
        "package/node_modules/{native_package}/{}",
        if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        }
    );
    claude["agent"] = "claude".into();
    claude["version"] = "0.66.0".into();
    claude["archive_url"] = format!(
        "https://github.com/snowdamiz/editur/releases/download/provider-v1/editur-provider-claude-{}-{}.zip",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
    .into();
    claude["args"] = serde_json::json!([]);
    claude["license_url"] =
        "https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.66.0/LICENSE".into();
    claude["terms_url"] = "https://www.anthropic.com/legal/commercial-terms".into();
    claude["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| !entry["path"].as_str().unwrap().contains("@openai/codex"));
    claude["entries"].as_array_mut().unwrap().extend([
        serde_json::json!({
            "path": "package/package.json",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": "package/README.md",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": "package/LICENSE",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": "package/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
            "kind": "file",
            "size": 1,
            "sha256": "44".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": "package/node_modules/@anthropic-ai/claude-agent-sdk/LICENSE.md",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": "package/node_modules/@anthropic-ai/claude-agent-sdk/README.md",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": native_path,
            "kind": "file",
            "size": 1,
            "sha256": "55".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": format!("package/node_modules/{native_package}/LICENSE.md"),
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": format!("package/node_modules/{native_package}/README.md"),
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
        serde_json::json!({
            "path": native_binary,
            "kind": "file",
            "size": 1,
            "sha256": "66".repeat(32),
            "executable": true
        }),
        serde_json::json!({
            "path": "runtime/LICENSE",
            "kind": "file",
            "size": 1,
            "sha256": "77".repeat(32),
            "executable": false
        }),
    ]);
    let entry_count = claude["entries"].as_array().unwrap().len();
    claude["max_entries"] = entry_count.into();
    claude["max_extracted_bytes"] = entry_count.into();
    claude["version_probes"] = serde_json::json!([
        {
            "command": command,
            "args": ["package/dist/index.js", "--version"],
            "expected": "0.66.0"
        },
        {
            "command": command,
            "args": ["package/dist/index.js", "--cli", "--version"],
            "expected": "2.1.220 (Claude Code)"
        }
    ]);
    claude["package_probes"] = serde_json::json!([
        {
            "path": "package/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
            "name": "@anthropic-ai/claude-agent-sdk",
            "version": "0.3.220",
            "integrity": "sha512-glc7SdwPkOkLw8oxwLo9PKTdLJGqW/PIR4urWXFoRtX9YllwozsEVc5Tc1+EvLSkfrsxPJqQWqOgpjUOQXf1oA=="
        },
        {
            "path": native_path,
            "name": native_package,
            "version": "0.3.220",
            "integrity": native_integrity
        }
    ]);
    claude
}

#[test]
fn format_v1_cursor_manifest_loads_as_a_one_provider_bundle() {
    let bundle = ProviderBundle::parse(&serde_json::to_vec(&manifest("cursor")).unwrap()).unwrap();

    assert_eq!(bundle.available(), vec![ProviderId::Cursor]);
    assert_eq!(
        bundle.manifest(ProviderId::Cursor).unwrap().version,
        "2026.07.23-e383d2b"
    );
    assert!(bundle.manifest(ProviderId::Codex).is_err());
}

#[test]
fn format_v2_bundle_accepts_pinned_providers_and_rejects_duplicate_or_unknown_ids() {
    let valid = serde_json::json!({
        "format_version": 2,
        "providers": [manifest("cursor"), manifest("codex"), claude_manifest()]
    });
    let bundle = ProviderBundle::parse(&serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(
        bundle.available(),
        vec![ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude]
    );
    let without_claude = serde_json::json!({
        "format_version": 2,
        "providers": [manifest("cursor"), manifest("codex")]
    });
    assert_eq!(
        ProviderBundle::parse(&serde_json::to_vec(&without_claude).unwrap())
            .unwrap()
            .available(),
        vec![ProviderId::Cursor, ProviderId::Codex]
    );

    let duplicate = serde_json::json!({
        "format_version": 2,
        "providers": [manifest("cursor"), manifest("cursor")]
    });
    assert!(
        ProviderBundle::parse(&serde_json::to_vec(&duplicate).unwrap())
            .unwrap_err()
            .contains("duplicate")
    );

    let mut unknown = manifest("codex");
    unknown["agent"] = "future".into();
    let unknown = serde_json::json!({"format_version": 2, "providers": [unknown]});
    assert!(ProviderBundle::parse(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let missing_cursor = serde_json::json!({
        "format_version": 2,
        "providers": [manifest("codex")]
    });
    assert!(
        ProviderBundle::parse(&serde_json::to_vec(&missing_cursor).unwrap())
            .unwrap_err()
            .contains("Cursor")
    );
}

#[test]
fn claude_manifest_pins_adapter_sdk_and_native_binary() {
    let claude = claude_manifest();

    let parsed =
        editur::agent::provision::SidecarManifest::parse(&serde_json::to_vec(&claude).unwrap())
            .unwrap();

    assert_eq!(parsed.provider().unwrap(), ProviderId::Claude);
    assert!(parsed.args.is_empty());
    assert_eq!(parsed.version_probes.len(), 2);
    assert_eq!(parsed.package_probes.len(), 2);

    for (field, value) in [
        ("args", serde_json::json!(["--hide-claude-auth"])),
        (
            "package_probes",
            serde_json::json!([{
                "path": "package/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
                "name": "@anthropic-ai/claude-agent-sdk",
                "version": "latest",
                "integrity": "mutable"
            }]),
        ),
    ] {
        let mut changed = claude.clone();
        changed[field] = value;
        assert!(
            editur::agent::provision::SidecarManifest::parse(
                &serde_json::to_vec(&changed).unwrap(),
            )
            .is_err(),
            "changed Claude {field} was accepted"
        );
    }

    let mut missing_notice = claude.clone();
    missing_notice["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["path"] != "runtime/LICENSE");
    missing_notice["max_entries"] = missing_notice["entries"].as_array().unwrap().len().into();
    missing_notice["max_extracted_bytes"] =
        missing_notice["entries"].as_array().unwrap().len().into();
    let error = editur::agent::provision::SidecarManifest::parse(
        &serde_json::to_vec(&missing_notice).unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("release policy"), "{error}");

    let mut non_executable_native = claude.clone();
    let native = non_executable_native["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| {
            entry["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("/claude") || path.ends_with("/claude.exe"))
        })
        .unwrap();
    native["executable"] = false.into();
    let error = editur::agent::provision::SidecarManifest::parse(
        &serde_json::to_vec(&non_executable_native).unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("release policy"), "{error}");

    let mut wrong_terms = claude.clone();
    wrong_terms["terms_url"] = "https://example.com/terms".into();
    let error = editur::agent::provision::SidecarManifest::parse(
        &serde_json::to_vec(&wrong_terms).unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("license or terms"), "{error}");

    let mut wrong_native_package = claude.clone();
    wrong_native_package["entries"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": "package/node_modules/@anthropic-ai/claude-agent-sdk-linux-arm64/claude",
            "kind": "file",
            "size": 1,
            "sha256": "88".repeat(32),
            "executable": true
        }));
    let entry_count = wrong_native_package["entries"].as_array().unwrap().len();
    wrong_native_package["max_entries"] = entry_count.into();
    wrong_native_package["max_extracted_bytes"] = entry_count.into();
    let error = editur::agent::provision::SidecarManifest::parse(
        &serde_json::to_vec(&wrong_native_package).unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("release policy"), "{error}");

    let mut wrong_native_version = claude;
    wrong_native_version["version_probes"][1]["expected"] = "mutable native".into();
    let error = editur::agent::provision::SidecarManifest::parse(
        &serde_json::to_vec(&wrong_native_version).unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("version probes"), "{error}");
}

#[test]
fn provider_policy_rejects_host_command_and_unpinned_claude_distribution() {
    let mut codex = manifest("codex");
    codex["archive_url"] = "https://example.com/codex.zip".into();
    assert!(ProviderBundle::parse(&serde_json::to_vec(&codex).unwrap()).is_err());

    let mut codex = manifest("codex");
    codex["command"] = "bin/arbitrary".into();
    assert!(ProviderBundle::parse(&serde_json::to_vec(&codex).unwrap()).is_err());

    let mut claude = manifest("codex");
    claude["agent"] = "claude".into();
    assert!(ProviderBundle::parse(&serde_json::to_vec(&claude).unwrap()).is_err());
}

#[test]
fn codex_manifest_pins_both_managed_version_probes() {
    let mut codex = manifest("codex");
    let probes = codex
        .as_object_mut()
        .unwrap()
        .remove("version_probes")
        .unwrap();
    let error =
        editur::agent::provision::SidecarManifest::parse(&serde_json::to_vec(&codex).unwrap())
            .unwrap_err();
    assert!(error.contains("version probes"), "{error}");

    codex["version_probes"] = probes;

    editur::agent::provision::SidecarManifest::parse(&serde_json::to_vec(&codex).unwrap()).unwrap();
    codex["version_probes"][1]["expected"] = "codex-cli latest".into();
    assert!(
        editur::agent::provision::SidecarManifest::parse(&serde_json::to_vec(&codex).unwrap())
            .unwrap_err()
            .contains("version probes")
    );
}

#[test]
fn managed_provider_namespaces_cannot_collide() {
    let data = std::path::Path::new("/application-data");
    assert_eq!(
        provider_root(data, ProviderId::Cursor),
        data.join("agents/cursor")
    );
    assert_eq!(
        provider_root(data, ProviderId::Codex),
        data.join("agents/codex")
    );
    assert_eq!(
        provider_root(data, ProviderId::Claude),
        data.join("agents/claude")
    );
}

#[test]
fn installed_package_becomes_an_exact_provider_owned_launch() {
    let data = tempfile::tempdir().unwrap();
    let root = provider_root(data.path(), ProviderId::Codex);
    let command = root.join("versions/1.1.14/runtime/bin/node");
    let entrypoint = root
        .join("versions/1.1.14/package/dist/index.js")
        .to_string_lossy()
        .into_owned();
    let installed = InstalledSidecar {
        command: command.clone(),
        args: vec![entrypoint.clone()],
        version: "1.1.14".into(),
    };

    let prepared = prepare_installed(ProviderId::Codex, installed, data.path());

    assert_eq!(prepared.provider, ProviderId::Codex);
    assert_eq!(prepared.display_name, "Codex");
    assert_eq!(prepared.command, command);
    assert_eq!(prepared.args, [entrypoint]);
    assert_eq!(
        prepared.env,
        [(
            "NODE_COMPILE_CACHE".into(),
            root.join("cache").into_os_string()
        )]
    );
    assert_eq!(
        prepared.remove_env,
        ["APP_SERVER_LOGS", "CODEX_PATH", "NODE_OPTIONS", "NODE_PATH"]
    );
    assert!(prepared.command.is_absolute());
    assert!(prepared.args.iter().all(|argument| {
        argument.starts_with('-') || std::path::Path::new(argument).is_absolute()
    }));
    assert_eq!(prepared.extensions, ProviderExtensions::None);
}

#[test]
fn cursor_launch_keeps_its_existing_identity_and_auto_update_arguments() {
    let data = std::path::Path::new("/application-data");
    let installed = InstalledSidecar {
        command: "/application-data/agents/cursor/versions/pinned/cursor-agent".into(),
        args: vec!["--disable-auto-update".into(), "acp".into()],
        version: "pinned".into(),
    };

    let prepared = prepare_installed(ProviderId::Cursor, installed, data);

    assert_eq!(prepared.args, ["--disable-auto-update", "acp"]);
    assert!(prepared.env.contains(&(
        "CURSOR_INVOKED_AS".into(),
        std::ffi::OsString::from("cursor-agent")
    )));
    assert_eq!(prepared.extensions, ProviderExtensions::Cursor);
}

#[test]
fn claude_launch_uses_provider_owned_auth_and_strips_runtime_overrides() {
    let data = tempfile::tempdir().unwrap();
    let root = provider_root(data.path(), ProviderId::Claude);
    let command = root.join("versions/0.66.0/runtime/bin/node");
    let entrypoint = root
        .join("versions/0.66.0/package/dist/index.js")
        .to_string_lossy()
        .into_owned();
    let installed = InstalledSidecar {
        command: command.clone(),
        args: vec![entrypoint.clone()],
        version: "0.66.0".into(),
    };

    let prepared = prepare_installed(ProviderId::Claude, installed, data.path());

    assert_eq!(prepared.display_name, "Claude");
    assert_eq!(prepared.args, [entrypoint]);
    assert!(
        !prepared
            .remove_env
            .iter()
            .any(|name| name == "ANTHROPIC_API_KEY")
    );
    assert_eq!(
        prepared.env,
        [(
            "NODE_COMPILE_CACHE".into(),
            root.join("cache").into_os_string()
        )]
    );
    assert_eq!(
        prepared.remove_env,
        [
            "CLAUDE_CODE_EXECUTABLE",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "NODE_OPTIONS",
            "NODE_PATH",
        ]
    );
    assert!(prepared.command.is_absolute());
    assert!(prepared.args.iter().all(|argument| {
        argument.starts_with('-') || std::path::Path::new(argument).is_absolute()
    }));
    assert_eq!(prepared.extensions, ProviderExtensions::None);
}

#[test]
fn account_launches_isolate_provider_state_and_never_debug_credentials() {
    let data = tempfile::tempdir().unwrap();
    let installed = |provider: ProviderId| InstalledSidecar {
        command: provider_root(data.path(), provider).join("versions/pinned/agent"),
        args: Vec::new(),
        version: "pinned".into(),
    };
    let source = "EDITUR_TEST_CODEX_ACCOUNT_KEY_8D7C";
    unsafe { std::env::set_var(source, "credential_value_must_be_redacted") };
    let codex = ProviderAccount {
        key: AccountKey {
            provider: ProviderId::Codex,
            account_id: 7,
        },
        label: "Work".into(),
        auth_source: AuthSource::Environment {
            variable: source.into(),
        },
        auto_failover: true,
    };

    let prepared =
        prepare_installed_for_account(&codex, installed(ProviderId::Codex), data.path()).unwrap();
    unsafe { std::env::remove_var(source) };

    let provider_data = account_provider_data_root(data.path(), codex.key).unwrap();
    assert!(
        prepared
            .env
            .iter()
            .any(|(name, value)| { name == "CODEX_HOME" && value == provider_data.as_os_str() })
    );
    assert!(prepared.env.iter().any(|(name, value)| {
        name == "CODEX_API_KEY" && value == "credential_value_must_be_redacted"
    }));
    assert!(
        prepared
            .remove_env
            .iter()
            .any(|name| name == "CODEX_API_KEY")
    );
    assert!(
        prepared
            .remove_env
            .iter()
            .any(|name| name == "OPENAI_API_KEY")
    );
    assert!(prepared.remove_env.iter().any(|name| name == source));
    assert_eq!(
        std::fs::read_to_string(provider_data.join("config.toml")).unwrap(),
        "cli_auth_credentials_store = \"file\"\n"
    );
    assert!(!format!("{prepared:?}").contains("credential_value_must_be_redacted"));

    let claude = ProviderAccount {
        key: AccountKey {
            provider: ProviderId::Claude,
            account_id: 8,
        },
        label: "Team".into(),
        auth_source: AuthSource::ProviderManaged,
        auto_failover: false,
    };
    let prepared =
        prepare_installed_for_account(&claude, installed(ProviderId::Claude), data.path()).unwrap();
    let provider_data = account_provider_data_root(data.path(), claude.key).unwrap();
    assert!(prepared.env.iter().any(|(name, value)| {
        name == "CLAUDE_CONFIG_DIR" && value == provider_data.as_os_str()
    }));
    assert!(
        prepared
            .remove_env
            .iter()
            .any(|name| name == "CLAUDE_CODE_OAUTH_TOKEN")
    );
    assert!(
        prepared
            .remove_env
            .iter()
            .any(|name| name == "ANTHROPIC_API_KEY")
    );
}

#[test]
fn cursor_plan_accounts_get_an_isolated_sign_in_through_the_patched_entrypoint() {
    let data = tempfile::tempdir().unwrap();
    let dist = provider_root(data.path(), ProviderId::Cursor).join("versions/pinned/dist-package");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("node"), b"node-runtime").unwrap();
    std::fs::write(dist.join("node.exe"), b"node-runtime").unwrap();
    std::fs::write(
        dist.join("index.js"),
        "class o{getAuthFilePath(e){return join(homedir(),`.${e}`,\"auth.json\")}}",
    )
    .unwrap();
    let installed = InstalledSidecar {
        command: dist.join("cursor-agent"),
        args: vec!["--disable-auto-update".into(), "acp".into()],
        version: "pinned".into(),
    };
    let account = ProviderAccount {
        key: AccountKey {
            provider: ProviderId::Cursor,
            account_id: 4,
        },
        label: "Work".into(),
        auth_source: AuthSource::ProviderManaged,
        auto_failover: false,
    };

    let prepared = prepare_installed_for_account(&account, installed, data.path()).unwrap();

    let provider_data = account_provider_data_root(data.path(), account.key).unwrap();
    let node = if cfg!(windows) { "node.exe" } else { "node" };
    assert_eq!(prepared.command, dist.join(node));
    assert!(prepared.args[0].ends_with("editur-multi-account-index.js"));
    assert_eq!(&prepared.args[1..], ["--disable-auto-update", "acp"]);
    let patched = std::fs::read_to_string(&prepared.args[0]).unwrap();
    assert!(
        patched.contains(
            "getAuthFilePath(e){const editurAuthFile=process.env.EDITUR_CURSOR_AUTH_FILE;\
             if(editurAuthFile)return editurAuthFile;"
        ),
        "the isolated entrypoint must consult EDITUR_CURSOR_AUTH_FILE first"
    );
    let expectations = [
        ("CURSOR_CONFIG_DIR", provider_data.join("config")),
        ("CURSOR_DATA_DIR", provider_data.join("data")),
        ("EDITUR_CURSOR_AUTH_FILE", provider_data.join("auth.json")),
    ];
    for (name, path) in expectations {
        assert!(
            prepared
                .env
                .iter()
                .any(|(key, value)| key == name && value == path.as_os_str()),
            "{name} must point inside the account's provider data"
        );
        assert!(path.starts_with(&provider_data));
    }
    assert!(
        prepared
            .env
            .iter()
            .any(|(name, value)| name == "AGENT_CLI_CREDENTIAL_STORE" && value == "file")
    );
    assert!(provider_data.join("config").is_dir());
    assert!(provider_data.join("data").is_dir());
    for ambient in ["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"] {
        assert!(prepared.remove_env.iter().any(|name| name == ambient));
    }

    // The managed bundle stays pristine for integrity verification.
    assert_eq!(
        std::fs::read_to_string(dist.join("index.js")).unwrap(),
        "class o{getAuthFilePath(e){return join(homedir(),`.${e}`,\"auth.json\")}}"
    );
}

#[test]
fn cursor_plan_accounts_fail_closed_when_the_credential_anchor_moves() {
    let data = tempfile::tempdir().unwrap();
    let dist = provider_root(data.path(), ProviderId::Cursor).join("versions/pinned/dist-package");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("node"), b"node-runtime").unwrap();
    std::fs::write(dist.join("node.exe"), b"node-runtime").unwrap();
    std::fs::write(dist.join("index.js"), "class o{readAuthData(){}}").unwrap();
    let account = ProviderAccount {
        key: AccountKey {
            provider: ProviderId::Cursor,
            account_id: 4,
        },
        label: "Work".into(),
        auth_source: AuthSource::ProviderManaged,
        auto_failover: false,
    };

    let error = prepare_installed_for_account(
        &account,
        InstalledSidecar {
            command: dist.join("cursor-agent"),
            args: vec!["--disable-auto-update".into(), "acp".into()],
            version: "pinned".into(),
        },
        data.path(),
    )
    .unwrap_err();

    assert!(error.contains("Editur needs an update"), "{error}");
    assert!(!dist.join("editur-multi-account-index.js").exists());
}
