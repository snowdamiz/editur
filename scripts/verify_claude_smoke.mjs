import fs from "node:fs";
import { fileURLToPath } from "node:url";

const requiredTargets = [
  "linux-x86_64",
  "macos-aarch64",
  "macos-x86_64",
  "windows-x86_64",
];
const requiredChecks = [
  "lazy-provision",
  "missing-credentials-guidance",
  "provider-login-auth",
  "capability-capture",
  "session-followup",
  "plan-tool-image-edit-permission-cancel-review-mcp",
  "buffer-refresh",
  "history-capability",
  "flattened-subagent",
  "provider-switch-isolation",
  "lazy-repair",
  "process-tree-exit",
];

export function verifyClaudeSmoke(record, legal = record.legal_review) {
  if (
    record.adapter_version !== "0.66.0" ||
    record.sdk_version !== "0.3.220" ||
    record.native_version !== "2.1.220 (Claude Code)" ||
    record.node_version !== "22.22.0"
  ) {
    throw new Error("smoke record does not match the pinned Claude runtime");
  }
  if (
    !legal ||
    !legal.distribution_approved ||
    !legal.branding_approved ||
    !legal.commercial_auth_approved ||
    typeof legal.reviewer !== "string" ||
    !legal.reviewer.trim() ||
    !Number.isFinite(Date.parse(legal.reviewed_at))
  ) {
    throw new Error("smoke record is missing release-owner legal approval");
  }
  for (const target of requiredTargets) {
    const result = record.targets?.[target];
    if (
      !result ||
      typeof result.auth_method !== "string" ||
      !result.auth_method.trim() ||
      typeof result.native_binary !== "string" ||
      !result.native_binary.trim() ||
      !requiredChecks.every((check) => result.checks?.includes(check))
    ) {
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
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const path = process.argv[2];
  if (!path) throw new Error("usage: verify_claude_smoke RECORD.json");
  const legal = JSON.parse(
    fs.readFileSync(
      fileURLToPath(
        new URL("../release/claude-smoke/legal-review.json", import.meta.url),
      ),
      "utf8",
    ),
  );
  verifyClaudeSmoke(JSON.parse(fs.readFileSync(path, "utf8")), legal);
}
