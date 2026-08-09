use editur::agent::provider::{
    InstallPolicy, ProviderExtensions, ProviderId, accept_terms, catalog, load_selected,
    prepare_installed, save_selected, terms_accepted,
};
use editur::agent::provision::{InstalledSidecar, ProviderBundle, provider_root};

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
    assert!(
        providers[1..]
            .iter()
            .all(|provider| provider.extensions == ProviderExtensions::None)
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
fn optional_provider_terms_are_accepted_per_provider() {
    let data = tempfile::tempdir().unwrap();

    assert!(!terms_accepted(data.path(), ProviderId::Codex));
    accept_terms(data.path(), ProviderId::Codex).unwrap();
    assert!(terms_accepted(data.path(), ProviderId::Codex));
    assert!(!terms_accepted(data.path(), ProviderId::Claude));
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
        "max_extracted_bytes": 2,
        "max_entries": entries.len(),
        "command": command,
        "entrypoint": entrypoint,
        "args": args,
        "entries": entries,
        "license_url": if provider == "cursor" { "https://cursor.com/terms-of-service" } else { "https://github.com/agentclientprotocol/codex-acp/blob/v1.1.14/LICENSE" },
        "terms_url": if provider == "cursor" { "https://cursor.com/terms-of-service" } else { "https://openai.com/policies/terms-of-use/" }
    })
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
        "providers": [manifest("cursor"), manifest("codex")]
    });
    let bundle = ProviderBundle::parse(&serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(
        bundle.available(),
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
fn provider_policy_rejects_host_command_and_blocked_claude_distribution() {
    let mut codex = manifest("codex");
    codex["archive_url"] = "https://example.com/codex.zip".into();
    assert!(ProviderBundle::parse(&serde_json::to_vec(&codex).unwrap()).is_err());

    let mut codex = manifest("codex");
    codex["command"] = "bin/arbitrary".into();
    assert!(ProviderBundle::parse(&serde_json::to_vec(&codex).unwrap()).is_err());

    let mut claude = manifest("codex");
    claude["agent"] = "claude".into();
    assert!(
        ProviderBundle::parse(&serde_json::to_vec(&claude).unwrap())
            .unwrap_err()
            .contains("unavailable")
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
    let data = std::path::Path::new("/application-data");
    let installed = InstalledSidecar {
        command: "/application-data/agents/codex/versions/1.1.14/runtime/bin/node".into(),
        args: vec!["/application-data/agents/codex/versions/1.1.14/package/dist/index.js".into()],
        version: "1.1.14".into(),
    };

    let prepared = prepare_installed(ProviderId::Codex, installed, data);

    assert_eq!(prepared.provider, ProviderId::Codex);
    assert_eq!(prepared.display_name, "Codex");
    assert_eq!(
        prepared.command,
        std::path::Path::new("/application-data/agents/codex/versions/1.1.14/runtime/bin/node")
    );
    assert_eq!(
        prepared.args,
        ["/application-data/agents/codex/versions/1.1.14/package/dist/index.js"]
    );
    assert_eq!(
        prepared.env,
        [(
            "NODE_COMPILE_CACHE".into(),
            data.join("agents/codex/cache").into_os_string()
        )]
    );
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
