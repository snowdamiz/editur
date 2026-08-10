import test from "node:test";
import assert from "node:assert/strict";

import { verifyClaudeSmoke } from "./verify_claude_smoke.mjs";

const checks = [
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
const result = {
  checks,
  auth_method: "Claude subscription",
  native_binary: "pinned target binary",
  archive_bytes: 1,
  installed_bytes: 1,
  first_connection_ms: 1,
  warm_connection_ms: 1,
  unsupported_features: [],
  diagnostics: "redacted",
};
const record = {
  adapter_version: "0.66.0",
  sdk_version: "0.3.220",
  native_version: "2.1.220 (Claude Code)",
  node_version: "22.22.0",
  legal_review: {
    distribution_approved: true,
    branding_approved: true,
    commercial_auth_approved: true,
    reviewer: "release owner",
    reviewed_at: "2026-08-09T00:00:00Z",
  },
  targets: Object.fromEntries(
    ["linux-x86_64", "macos-aarch64", "macos-x86_64", "windows-x86_64"].map(
      (target) => [target, result],
    ),
  ),
};

test("complete Claude release evidence passes and missing legal approval fails", () => {
  assert.doesNotThrow(() => verifyClaudeSmoke(record));
  assert.throws(() =>
    verifyClaudeSmoke({
      ...record,
      legal_review: { ...record.legal_review, distribution_approved: false },
    }),
  );
});

test("a recorded release-owner approval can be supplied separately", () => {
  const { legal_review, ...smoke } = record;

  assert.doesNotThrow(() => verifyClaudeSmoke(smoke, legal_review));
});
