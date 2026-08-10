# Claude stable-release evidence

Release-owner legal approval is recorded in `legal-review.json`. Before creating
a stable `v*` tag, add `release/claude-smoke/<tag>.json` with the redacted
four-target provider-login results.
Validate it with:

```sh
node scripts/verify_claude_smoke.mjs release/claude-smoke/<tag>.json
```

Use `scripts/verify_claude_smoke.test.mjs` as the schema example. Replace every
placeholder measurement, authentication method, binary identity, and note with the real
result. Never commit account, credential, prompt, project, or provider-log data.
Stable CI rejects missing or incomplete evidence.
