import fs from "node:fs";

const requiredTargets = [
  "linux-x86_64",
  "macos-aarch64",
  "macos-x86_64",
  "windows-x86_64",
];
const requiredChecks = [
  "lazy-provision",
  "chatgpt-auth",
  "api-key-auth",
  "session-followup",
  "plan-tool-terminal-image-edit-permission-cancel-review",
  "buffer-refresh",
  "history-capability",
  "provider-switch-isolation",
  "lazy-repair",
  "process-tree-exit",
];

const path = process.argv[2];
if (!path) throw new Error("usage: verify_codex_smoke RECORD.json");
const record = JSON.parse(fs.readFileSync(path, "utf8"));
if (
  record.adapter_version !== "1.1.14" ||
  record.codex_version !== "0.147.0" ||
  record.node_version !== "22.22.0"
) {
  throw new Error("smoke record does not match the pinned Codex runtime");
}
for (const target of requiredTargets) {
  const result = record.targets?.[target];
  if (!result || !requiredChecks.every((check) => result.checks?.includes(check))) {
    throw new Error(`smoke record is incomplete for ${target}`);
  }
  for (const measurement of [
    "archive_bytes",
    "installed_bytes",
    "first_connection_ms",
    "warm_connection_ms",
  ]) {
    if (!Number.isSafeInteger(result[measurement]) || result[measurement] <= 0) {
      throw new Error(`smoke record has no ${measurement} for ${target}`);
    }
  }
  if (!Array.isArray(result.unsupported_features) || typeof result.diagnostics !== "string") {
    throw new Error(`smoke record has invalid notes for ${target}`);
  }
}
