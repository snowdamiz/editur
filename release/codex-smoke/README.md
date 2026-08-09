# Codex stable-release smoke record

Before creating a stable `v*` tag, copy the JSON shape below to
`release/codex-smoke/<tag>.json` and replace every value with the redacted
results from the manual matrix in `CODEX_ACP_PLAN.md`. CI rejects stable
promotion unless all four native targets and all required checks are recorded.

```json
{
  "adapter_version": "1.1.14",
  "codex_version": "0.147.0",
  "node_version": "22.22.0",
  "targets": {
    "linux-x86_64": {
      "checks": ["lazy-provision", "chatgpt-auth", "api-key-auth", "session-followup", "plan-tool-terminal-image-edit-permission-cancel-review", "buffer-refresh", "history-capability", "provider-switch-isolation", "lazy-repair", "process-tree-exit"],
      "archive_bytes": 1,
      "installed_bytes": 1,
      "first_connection_ms": 1,
      "warm_connection_ms": 1,
      "unsupported_features": [],
      "diagnostics": "redacted"
    }
  }
}
```

Repeat the target object for `macos-aarch64`, `macos-x86_64`, and
`windows-x86_64`. Never commit account, credential, prompt, or project data.
